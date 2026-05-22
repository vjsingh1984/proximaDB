//! Startup recovery — replay disk segments past the per-partition committed
//! offset back into the memory tier so consumers resume seamlessly across
//! process restarts.
//!
//! Walks each topic's partition directories, reads framed messages from
//! every `.qseg` file in segment-id order, and pushes them into the
//! topic's `PartitionMemory` ring buffer. Phase 2B does not yet consult
//! the per-partition committed offset (that's Phase 2C's `offset_store`);
//! every persisted message that hasn't been deleted off disk is replayed.
//! Once 2C lands, recovery will skip messages with `offset <=
//! committed_offset`.
//!
//! Crash-safety: each message frame is `[4 BE: payload_len][bincode bytes]`.
//! A truncated final frame (process killed mid-fsync before
//! group_commit completed) is detected and silently skipped — the
//! producer never received an ack for that message, so it's safe to drop.

use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::QueueClient;
use crate::error::QueueError;
use crate::fs::QueueFs;
use crate::message::Message;
use crate::topic::PartitionId;

/// Consumer-group key recovery uses to look up `offset.meta`. Phase 2C
/// locks the design to one consumer group per topic (README invariant
/// #4), so this is sufficient. When multi-group support lands, recovery
/// will glob `offset.*.meta` and take the minimum committed offset
/// across groups.
const DEFAULT_GROUP_FOR_RECOVERY: &str = "g";

const SEGMENT_EXT: &str = "qseg";

/// Extract `(segment_id, path)` from a directory entry whose filename
/// looks like `0000000123.qseg`. Returns None for any other entry
/// (the marker sidecars, hidden files, etc.).
fn parse_segment_entry(path: std::path::PathBuf) -> Option<(u64, std::path::PathBuf)> {
    let name = path.file_name()?.to_str()?.to_owned();
    let stem = name.strip_suffix(&format!(".{SEGMENT_EXT}"))?;
    let segment_id = stem.parse::<u64>().ok()?;
    Some((segment_id, path))
}

/// Replay persisted segments back into the in-memory tier on startup.
///
/// Walks every registered topic, iterates its declared partitions,
/// reads each partition directory's `.qseg` files in segment-id order,
/// and pushes deserialized messages into the partition's
/// `PartitionMemory`. Auto-created topics (registered lazily after
/// startup) don't have segments to recover, so the lazy path skips
/// recovery — only topics declared in `QueueConfig::topics` go through
/// here.
pub async fn recover(client: &QueueClient) -> crate::Result<usize> {
    let topic_names = client.topic_names().await;
    if topic_names.is_empty() {
        debug!("recovery: no topics registered; nothing to replay");
        return Ok(0);
    }

    // Compute the archive root once (None when not configured).
    let archive_root: Option<std::path::PathBuf> = client
        .config()
        .object_archive
        .as_deref()
        .map(crate::object_tier::resolve_archive_root)
        .transpose()?;

    let mut total_replayed = 0usize;
    for topic in topic_names {
        let Some(state) = client.topic_state(&topic).await else {
            continue;
        };
        for partition_id in 0..state.config.partition_count {
            let partition_dir = client
                .root_path()
                .join(&topic)
                .join(partition_id.to_string());
            let archive_partition_dir = archive_root
                .as_ref()
                .map(|root| root.join(&topic).join(partition_id.to_string()));
            let count = replay_partition(
                client.fs(),
                &partition_dir,
                archive_partition_dir.as_deref(),
                &topic,
                partition_id,
                &state,
            )
            .await?;
            total_replayed += count;
        }
    }

    info!(messages_replayed = total_replayed, "recovery complete");
    Ok(total_replayed)
}

async fn replay_partition(
    fs: &Arc<dyn QueueFs>,
    partition_dir: &Path,
    archive_partition_dir: Option<&Path>,
    topic: &str,
    partition: PartitionId,
    state: &crate::TopicState,
) -> crate::Result<usize> {
    // Merge segments from local disk + (optionally) the archive. For
    // each segment_id, prefer the local disk copy (faster); fall back
    // to the archive only when the disk copy is missing. This is the
    // fresh-node-rebuild path: ECS pod reschedules onto a new node
    // with empty NVMe; recovery loads segments straight from the
    // archive.
    let mut segments: std::collections::BTreeMap<u64, std::path::PathBuf> =
        std::collections::BTreeMap::new();
    let local_entries = fs.list(partition_dir).await.unwrap_or_default();
    for path in local_entries {
        if let Some((id, p)) = parse_segment_entry(path) {
            segments.insert(id, p);
        }
    }
    if let Some(archive_dir) = archive_partition_dir {
        let archive_entries = fs.list(archive_dir).await.unwrap_or_default();
        for path in archive_entries {
            if let Some((id, p)) = parse_segment_entry(path) {
                // Archive is the fallback — only insert if disk had nothing.
                segments.entry(id).or_insert(p);
            }
        }
    }
    // BTreeMap iterator is already sorted by segment_id ascending,
    // which is the order replay needs (lower offsets first).
    let segments: Vec<(u64, std::path::PathBuf)> = segments.into_iter().collect();

    if segments.is_empty() {
        return Ok(0);
    }

    let mem = state
        .memory
        .get(partition as usize)
        .ok_or(QueueError::PartitionNotFound {
            topic: topic.to_string(),
            partition,
        })?
        .clone();
    let disk_writer = state
        .disk_writers
        .get(partition as usize)
        .ok_or(QueueError::PartitionNotFound {
            topic: topic.to_string(),
            partition,
        })?
        .clone();

    // Read the per-partition committed offset for the default consumer
    // group. None = no offset.meta on disk (cold start, replay all).
    // Some(c) = skip messages whose frame-offset <= c (already acked).
    let root_path = partition_dir
        .parent()
        .and_then(|topic_dir| topic_dir.parent())
        .ok_or_else(|| {
            QueueError::Persistence(format!(
                "recovery: cannot derive queue root from {partition_dir:?}"
            ))
        })?;
    let committed =
        crate::offset_store::read(fs, root_path, topic, partition, DEFAULT_GROUP_FOR_RECOVERY)
            .await?;

    let mut replayed = 0usize;
    let mut skipped = 0usize;
    let mut max_offset: Option<u64> = None;
    for (segment_id, path) in segments {
        let bytes = match fs.read(&path).await {
            Ok(b) => b,
            Err(e) => {
                warn!(?path, error = %e, "recovery: failed to read segment, skipping");
                continue;
            }
        };
        if bytes.is_empty() {
            continue;
        }

        let mut cursor = Cursor::new(&bytes[..]);
        loop {
            // Frame: [4 BE len][8 BE offset][len bytes bincode payload].
            let mut len_buf = [0u8; 4];
            if cursor.read_exact(&mut len_buf).is_err() {
                break; // EOF
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut offset_buf = [0u8; 8];
            if cursor.read_exact(&mut offset_buf).is_err() {
                debug!(
                    ?path,
                    "recovery: truncated offset header, stopping segment scan"
                );
                break;
            }
            let frame_offset = u64::from_be_bytes(offset_buf);
            // Defensive: bail if the declared length would overflow the
            // remaining segment bytes (truncated final frame from a
            // crashed producer).
            let remaining = bytes.len().saturating_sub(cursor.position() as usize);
            if len > remaining {
                debug!(
                    ?path,
                    declared = len,
                    available = remaining,
                    "recovery: truncated trailing frame, stopping segment scan"
                );
                break;
            }
            let mut payload = vec![0u8; len];
            if cursor.read_exact(&mut payload).is_err() {
                break;
            }
            let message: Message = match bincode::deserialize(&payload) {
                Ok(m) => m,
                Err(e) => {
                    warn!(?path, error = %e, "recovery: failed to deserialize message, skipping");
                    continue;
                }
            };

            // Track the max frame_offset across ALL replayed frames
            // (including skipped ones) so the disk writer's next_offset
            // resumes past every previously-assigned offset.
            max_offset = Some(max_offset.map_or(frame_offset, |m| m.max(frame_offset)));

            // Skip if already acked by the consumer group.
            if let Some(c) = committed {
                if frame_offset <= c {
                    skipped += 1;
                    continue;
                }
            }

            // Re-enqueue with the frame-recorded offset so MessageId is
            // stable across crash/replay.
            if mem.enqueue_with_offset(message, frame_offset).is_err() {
                return Err(QueueError::Persistence(format!(
                    "recovery: memory tier full at topic={topic} partition={partition} \
                     segment={segment_id} replayed={replayed} — increase memory_capacity"
                )));
            }
            replayed += 1;
        }
    }

    // Bump the disk writer past the highest observed offset so newly-
    // appended messages don't collide with recovered ones.
    if let Some(max) = max_offset {
        disk_writer.set_next_offset(max + 1);
    }

    if replayed > 0 || skipped > 0 {
        debug!(
            topic = topic,
            partition = partition,
            messages_replayed = replayed,
            messages_skipped = skipped,
            committed_offset = ?committed,
            "recovery: partition replayed"
        );
    }
    Ok(replayed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::LocalFs;
    use crate::memory_tier::PartitionMemory;
    use crate::{TopicConfig, TopicState};

    async fn topic_state(capacity: usize, root: &Path) -> TopicState {
        // Open a real disk writer per partition since Phase 2C-b's
        // replay_partition bumps the writer's next_offset after replay.
        let fs: Arc<dyn QueueFs> = LocalFs::new_arc();
        let cfg = TopicConfig {
            partition_count: 1,
            memory_capacity: capacity,
            ..TopicConfig::default()
        };
        let writer = crate::disk_tier::PartitionDiskWriter::open(
            "orders".to_string(),
            0,
            root.to_path_buf(),
            fs,
            cfg.clone(),
        )
        .await
        .expect("open writer");
        TopicState {
            config: cfg,
            memory: vec![Arc::new(PartitionMemory::new(0, capacity))],
            disk_writers: vec![writer],
        }
    }

    /// Build a frame in the new format: [4 BE len][8 BE offset][payload].
    /// The offset value here is what the disk writer would have assigned
    /// when this message was originally written; tests pass it explicitly
    /// so the recovery skip / max-offset behavior can be exercised
    /// deterministically.
    fn frame_with_offset(message: &Message, offset: u64) -> Vec<u8> {
        let encoded = bincode::serialize(message).unwrap();
        let mut framed = Vec::with_capacity(4 + 8 + encoded.len());
        framed.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        framed.extend_from_slice(&offset.to_be_bytes());
        framed.extend_from_slice(&encoded);
        framed
    }

    #[tokio::test]
    async fn replay_partition_returns_zero_for_missing_partition_dir() {
        let fs = LocalFs::new_arc();
        let root = tempfile::tempdir().unwrap();
        let state = topic_state(4, root.path()).await;

        let replayed =
            replay_partition(&fs, &root.path().join("missing"), None, "orders", 0, &state)
                .await
                .unwrap();

        assert_eq!(replayed, 0);
        assert_eq!(state.memory[0].depth(), 0);
    }

    #[tokio::test]
    async fn replay_partition_replays_valid_frames_and_stops_at_truncated_tail() {
        let fs = LocalFs::new_arc();
        let root = tempfile::tempdir().unwrap();
        let partition_dir = root.path().join("orders").join("0");
        fs.create_dir_all(&partition_dir).await.unwrap();
        fs.append(&partition_dir.join("ignore.txt"), b"not a segment")
            .await
            .unwrap();

        let mut segment_bytes =
            frame_with_offset(&Message::new("orders", "tenant-a", b"survives".to_vec()), 0);
        // Append a truncated tail: length header claims 99 bytes, only
        // 5 actually follow. Recovery must stop the scan cleanly.
        segment_bytes.extend_from_slice(&99u32.to_be_bytes());
        segment_bytes.extend_from_slice(&0u64.to_be_bytes());
        segment_bytes.extend_from_slice(b"short");
        fs.append(&partition_dir.join("0000000000.qseg"), &segment_bytes)
            .await
            .unwrap();
        let state = topic_state(4, root.path()).await;

        let replayed = replay_partition(&fs, &partition_dir, None, "orders", 0, &state)
            .await
            .unwrap();

        assert_eq!(replayed, 1);
        let restored = state.memory[0].try_pop_batch(10);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].message.payload, b"survives");
    }

    #[tokio::test]
    async fn replay_partition_surfaces_memory_full_as_persistence_error() {
        let fs = LocalFs::new_arc();
        let root = tempfile::tempdir().unwrap();
        let partition_dir = root.path().join("orders").join("0");
        fs.create_dir_all(&partition_dir).await.unwrap();

        let mut segment_bytes =
            frame_with_offset(&Message::new("orders", "tenant-a", b"first".to_vec()), 0);
        segment_bytes.extend_from_slice(&frame_with_offset(
            &Message::new("orders", "tenant-a", b"second".to_vec()),
            1,
        ));
        fs.append(&partition_dir.join("0000000000.qseg"), &segment_bytes)
            .await
            .unwrap();
        let state = topic_state(1, root.path()).await;

        let error = replay_partition(&fs, &partition_dir, None, "orders", 0, &state)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("memory tier full"));
    }
}
