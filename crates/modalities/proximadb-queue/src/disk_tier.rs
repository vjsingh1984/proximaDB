//! Per-partition disk tier - appends serialized messages to segment files
//! and coordinates fsync via the per-partition `GroupCommitCoordinator`.
//!
//! ## Segment naming
//!
//! Mirrors the WAL convention from
//! `src/storage/persistence/write_ahead_log/disk_manager.rs:127-142`:
//!
//! ```text
//! {queue_root}/{topic}/{partition_id}/{segment_id:010}.qseg
//! ```
//!
//! `segment_id` is a zero-padded monotonic u64 so list ordering is
//! lexicographic = chronological.
//!
//! Active segment is the highest-numbered file in the partition
//! directory. When it crosses `disk_rotation_size_mb`, the writer
//! seals it (atomic by ceasing writes) and opens a new active segment
//! with `segment_id + 1`.
//!
//! ## Write protocol
//!
//! Each appended message is framed as:
//!
//! ```text
//! [4 bytes BE: payload_len] [payload_len bytes: bincode(Message)]
//! ```
//!
//! Bincode is the workspace's default binary format; recovery reads
//! the file in this same framing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::trace;

use crate::config::TopicConfig;
use crate::error::QueueError;
use crate::fs::{QueueFs, Result};
use crate::group_commit::{GroupCommitConfig, GroupCommitCoordinator};
use crate::message::Message;
use crate::topic::PartitionId;

const SEGMENT_EXT: &str = "qseg";

/// File-on-disk record describing one segment.
#[derive(Debug, Clone)]
pub struct Segment {
    pub topic: String,
    pub partition: PartitionId,
    pub segment_id: u64,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sealed: bool,
}

/// Per-partition disk writer. One instance per `(topic, partition)`.
pub struct PartitionDiskWriter {
    topic: String,
    partition: PartitionId,
    root: PathBuf,
    fs: Arc<dyn QueueFs>,
    group_commit: Arc<GroupCommitCoordinator>,
    config: TopicConfig,
    /// Mutex around (active_segment_id, active_segment_size_bytes).
    /// Locked briefly during append + rotation check.
    state: Mutex<DiskWriterState>,
}

#[derive(Debug)]
struct DiskWriterState {
    active_segment_id: u64,
    active_segment_path: PathBuf,
    active_segment_size: u64,
}

impl PartitionDiskWriter {
    pub async fn open(
        topic: String,
        partition: PartitionId,
        root: PathBuf,
        fs: Arc<dyn QueueFs>,
        config: TopicConfig,
    ) -> Result<Arc<Self>> {
        let partition_dir = root.join(&topic).join(partition.to_string());
        fs.create_dir_all(&partition_dir).await?;

        // Discover existing segments to resume from. Highest id is the active
        // one (or 0 if none exist).
        let existing = list_segments(&*fs, &partition_dir, &topic, partition).await?;
        let (active_segment_id, active_segment_path, active_segment_size) =
            if let Some(latest) = existing.into_iter().max_by_key(|s| s.segment_id) {
                (latest.segment_id, latest.path, latest.size_bytes)
            } else {
                let id = 0u64;
                let path = segment_path(&partition_dir, id);
                // Touch the file so subsequent appends/lists are consistent.
                fs.append(&path, &[]).await?;
                (id, path, 0)
            };

        let group_commit = GroupCommitCoordinator::new(
            fs.clone(),
            GroupCommitConfig {
                max_wait: config.group_commit_max_wait,
                max_batch: config.group_commit_max_batch,
            },
        );

        Ok(Arc::new(Self {
            topic,
            partition,
            root,
            fs,
            group_commit,
            config,
            state: Mutex::new(DiskWriterState {
                active_segment_id,
                active_segment_path,
                active_segment_size,
            }),
        }))
    }

    /// Serialize + append + register fsync. Returns the segment path that
    /// the caller should await on via `wait_for_fsync` (Strict mode).
    pub async fn append(self: &Arc<Self>, message: &Message) -> Result<AppendOutcome> {
        let bytes = bincode::serialize(message)
            .map_err(|e| QueueError::Persistence(format!("serialize: {e}")))?;
        let mut framed = Vec::with_capacity(4 + bytes.len());
        framed.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        framed.extend_from_slice(&bytes);

        let frame_len = framed.len() as u64;
        let mut state = self.state.lock().await;

        // Rotation check - if appending this frame would push past the
        // rotation threshold AND the segment is non-empty, seal + open new.
        let rotation_bytes = (self.config.disk_rotation_size_mb as u64) * 1024 * 1024;
        if state.active_segment_size > 0 && state.active_segment_size + frame_len > rotation_bytes {
            let new_id = state.active_segment_id + 1;
            let new_path = segment_path(
                &self.root.join(&self.topic).join(self.partition.to_string()),
                new_id,
            );
            self.fs.append(&new_path, &[]).await?;
            state.active_segment_id = new_id;
            state.active_segment_path = new_path;
            state.active_segment_size = 0;
            trace!(
                topic = %self.topic,
                partition = self.partition,
                segment_id = new_id,
                "segment rotated"
            );
        }

        let active_path = state.active_segment_path.clone();
        self.fs.append(&active_path, &framed).await?;
        state.active_segment_size += frame_len;
        drop(state);

        Ok(AppendOutcome {
            segment_path: active_path,
        })
    }

    /// Block until the named segment has been fsync'd by the group-commit
    /// coordinator. Strict-mode producers call this after `append`.
    pub async fn wait_for_fsync(self: &Arc<Self>, segment_path: PathBuf) -> Result<()> {
        self.group_commit.wait_for_fsync(segment_path).await
    }

    pub async fn segments(&self) -> Result<Vec<Segment>> {
        let partition_dir = self.root.join(&self.topic).join(self.partition.to_string());
        list_segments(&*self.fs, &partition_dir, &self.topic, self.partition).await
    }
}

#[derive(Debug)]
pub struct AppendOutcome {
    pub segment_path: PathBuf,
}

/// Build the segment file path for a given partition + segment_id.
fn segment_path(partition_dir: &Path, segment_id: u64) -> PathBuf {
    partition_dir.join(format!("{segment_id:010}.{SEGMENT_EXT}"))
}

async fn list_segments(
    fs: &dyn QueueFs,
    partition_dir: &Path,
    topic: &str,
    partition: PartitionId,
) -> Result<Vec<Segment>> {
    let entries = fs.list(partition_dir).await?;
    let mut segments = Vec::new();
    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(&format!(".{SEGMENT_EXT}")) else {
            continue;
        };
        let Ok(segment_id) = stem.parse::<u64>() else {
            continue;
        };
        let size_bytes = fs.metadata(&path).await.map(|m| m.size_bytes).unwrap_or(0);
        segments.push(Segment {
            topic: topic.to_string(),
            partition,
            segment_id,
            path,
            size_bytes,
            sealed: false, // sealing is purely logical - active = highest id
        });
    }
    Ok(segments)
}
