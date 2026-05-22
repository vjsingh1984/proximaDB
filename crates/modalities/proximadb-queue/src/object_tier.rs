//! Object-store tier — archives sealed disk segments to a second
//! `QueueFs` backend (typically a cloud blob store: `adls://`, `s3://`,
//! `gcs://`, or another `file://` mount for testing).
//!
//! ## Why
//!
//! Local NVMe survives process restart but NOT node loss. When an ECS
//! task or k8s pod is rescheduled on a different node, the local disk
//! is gone. The object tier mirrors every sealed segment to durable
//! shared storage so a new node can recover the queue state without
//! depending on the prior node's disk.
//!
//! ## Lifecycle
//!
//! 1. Producer rotates the active segment when it crosses
//!    `disk_rotation_size_mb`. The old segment is now "sealed"
//!    (`segment_id < active_segment_id`).
//! 2. `ObjectTierUploader` runs on a background tokio task; every
//!    `poll_interval` it scans each partition for sealed segments
//!    without an `{segment_id}.uploaded` sidecar marker, copies the
//!    bytes to `{archive_root}/{topic}/{partition}/{segment_id}.qseg`,
//!    then writes the marker on success.
//! 3. The reaper (Phase 2D-b) later deletes the on-disk segment after
//!    the marker is present AND all consumer groups have committed
//!    past the segment's last offset.
//!
//! The marker is the synchronization point: it's only written after a
//! successful upload, so a crashed upload mid-write leaves the marker
//! absent and the next scan retries.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::QueueClient;
use crate::error::QueueError;
use crate::fs::QueueFs;

const SEGMENT_EXT: &str = "qseg";
const UPLOADED_MARKER_EXT: &str = "uploaded";

/// Default scan interval. Tunable via config in a follow-up.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct ObjectTierUploader {
    fs: Arc<dyn QueueFs>,
    disk_root: PathBuf,
    archive_root: PathBuf,
    poll_interval: Duration,
}

impl ObjectTierUploader {
    pub fn new(fs: Arc<dyn QueueFs>, disk_root: PathBuf, archive_root: PathBuf) -> Self {
        Self {
            fs,
            disk_root,
            archive_root,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Spawn the upload loop and return its `JoinHandle` plus a
    /// shutdown sender. Send `()` (or drop the sender) to stop the loop
    /// after its current iteration.
    pub fn start(self, client: Arc<QueueClient>) -> (JoinHandle<()>, oneshot::Sender<()>) {
        let (tx, mut rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            info!(
                disk_root = ?self.disk_root,
                archive_root = ?self.archive_root,
                poll_ms = self.poll_interval.as_millis() as u64,
                "object-tier uploader started"
            );
            let mut ticker = tokio::time::interval(self.poll_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    _ = ticker.tick() => {
                        if let Err(e) = self.scan_once(&client).await {
                            warn!(error = %e, "object-tier scan failed");
                        }
                    }
                }
            }
            info!("object-tier uploader stopped");
        });
        (handle, tx)
    }

    /// One pass: for each topic + partition, upload any sealed segment
    /// that lacks an `.uploaded` marker. Active segments (those still
    /// growing) are skipped — the disk_writer's `active_segment_id`
    /// tells us which one to leave alone.
    async fn scan_once(&self, client: &QueueClient) -> crate::Result<()> {
        for topic in client.topic_names().await {
            let Some(state) = client.topic_state(&topic).await else {
                continue;
            };
            for partition_id in 0..state.config.partition_count {
                let writer = match state.disk_writers.get(partition_id as usize) {
                    Some(w) => w.clone(),
                    None => continue,
                };
                let active_id = writer.active_segment_id().await;
                let segments = writer.segments().await.unwrap_or_default();
                for segment in segments {
                    // Skip the active (still-growing) segment.
                    if segment.segment_id >= active_id {
                        continue;
                    }
                    if let Err(e) = self
                        .upload_segment(&topic, partition_id, segment.segment_id, &segment.path)
                        .await
                    {
                        warn!(
                            topic = %topic,
                            partition = partition_id,
                            segment_id = segment.segment_id,
                            error = %e,
                            "object-tier upload failed; will retry next scan",
                        );
                    }
                }
            }
        }
        Ok(())
    }

    async fn upload_segment(
        &self,
        topic: &str,
        partition: u32,
        segment_id: u64,
        disk_path: &Path,
    ) -> crate::Result<()> {
        let dst = self.archive_segment_path(topic, partition, segment_id);
        let marker = marker_path(disk_path);

        // Already uploaded? Bail.
        if self.fs.metadata(&marker).await.is_ok() {
            return Ok(());
        }

        // Ensure archive partition dir exists.
        if let Some(parent) = dst.parent() {
            self.fs.create_dir_all(parent).await?;
        }

        // Copy bytes disk → archive. For LocalFs this is two syscalls;
        // for object stores it's a PUT. Both are atomic at the file
        // level — partial writes are detectable via the marker absence.
        let bytes = self.fs.read(disk_path).await?;
        // Use a per-call temp path then rename so a concurrent
        // re-upload (shouldn't happen, but defensive) doesn't expose
        // a torn write.
        let tmp = dst.with_extension(format!("{SEGMENT_EXT}.uploading"));
        let _ = self.fs.delete(&tmp).await;
        self.fs.append(&tmp, &bytes).await?;
        self.fs.fsync(&tmp).await?;
        self.fs.rename(&tmp, &dst).await?;

        // Sidecar marker — only written after the rename above commits.
        self.fs.append(&marker, &[]).await?;
        self.fs.fsync(&marker).await?;
        debug!(
            topic = %topic,
            partition = partition,
            segment_id = segment_id,
            bytes = bytes.len(),
            "object-tier uploaded segment",
        );
        Ok(())
    }

    fn archive_segment_path(&self, topic: &str, partition: u32, segment_id: u64) -> PathBuf {
        self.archive_root
            .join(topic)
            .join(partition.to_string())
            .join(format!("{segment_id:010}.{SEGMENT_EXT}"))
    }
}

fn marker_path(disk_segment: &Path) -> PathBuf {
    let mut p = disk_segment.as_os_str().to_owned();
    p.push(format!(".{UPLOADED_MARKER_EXT}"));
    PathBuf::from(p)
}

/// Parse an `object_archive` URL into a local PathBuf. Same shape as
/// `lib::resolve_local_root` — object-store URLs are rejected until the
/// `proximadb-filesystem` extraction lands.
pub(crate) fn resolve_archive_root(archive: &str) -> crate::Result<PathBuf> {
    if let Some(stripped) = archive.strip_prefix("file://") {
        Ok(PathBuf::from(stripped))
    } else if archive.contains("://") {
        Err(QueueError::Persistence(format!(
            "object_archive scheme not supported by LocalFs: {archive}; \
             wait for proximadb-filesystem extraction",
        )))
    } else {
        Ok(PathBuf::from(archive))
    }
}
