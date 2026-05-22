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

const SEGMENT_EXT: &str = "qseg";

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
            let count =
                replay_partition(client.fs(), &partition_dir, &topic, partition_id, &state).await?;
            total_replayed += count;
        }
    }

    info!(messages_replayed = total_replayed, "recovery complete");
    Ok(total_replayed)
}

async fn replay_partition(
    fs: &Arc<dyn QueueFs>,
    partition_dir: &Path,
    topic: &str,
    partition: PartitionId,
    state: &crate::TopicState,
) -> crate::Result<usize> {
    // List + filter segments under this partition.
    let entries = match fs.list(partition_dir).await {
        Ok(list) => list,
        Err(_) => {
            // Directory may not exist on a fresh root — that's not an
            // error, just nothing to recover.
            return Ok(0);
        }
    };
    let mut segments: Vec<(u64, std::path::PathBuf)> = entries
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            let stem = name.strip_suffix(&format!(".{SEGMENT_EXT}"))?;
            let segment_id = stem.parse::<u64>().ok()?;
            Some((segment_id, path))
        })
        .collect();
    segments.sort_by_key(|(id, _)| *id);

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

    let mut replayed = 0usize;
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
            let mut len_buf = [0u8; 4];
            if cursor.read_exact(&mut len_buf).is_err() {
                break; // EOF
            }
            let len = u32::from_be_bytes(len_buf) as usize;
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
            // Re-enqueue. Memory-full during replay is a config error
            // (capacity is too small for the persisted backlog); surface
            // it loudly rather than silently dropping.
            if mem.try_enqueue(message).is_err() {
                return Err(QueueError::Persistence(format!(
                    "recovery: memory tier full at topic={topic} partition={partition} \
                     segment={segment_id} replayed={replayed} — increase memory_capacity"
                )));
            }
            replayed += 1;
        }
    }

    if replayed > 0 {
        debug!(
            topic = topic,
            partition = partition,
            messages = replayed,
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

    fn topic_state(capacity: usize) -> TopicState {
        TopicState {
            config: TopicConfig {
                partition_count: 1,
                memory_capacity: capacity,
                ..TopicConfig::default()
            },
            memory: vec![Arc::new(PartitionMemory::new(0, capacity))],
            disk_writers: Vec::new(),
        }
    }

    fn frame(message: &Message) -> Vec<u8> {
        let encoded = bincode::serialize(message).unwrap();
        let mut framed = Vec::with_capacity(4 + encoded.len());
        framed.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        framed.extend_from_slice(&encoded);
        framed
    }

    #[tokio::test]
    async fn replay_partition_returns_zero_for_missing_partition_dir() {
        let fs = LocalFs::new_arc();
        let root = tempfile::tempdir().unwrap();
        let state = topic_state(4);

        let replayed = replay_partition(&fs, &root.path().join("missing"), "orders", 0, &state)
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

        let mut segment_bytes = frame(&Message::new("orders", "tenant-a", b"survives".to_vec()));
        segment_bytes.extend_from_slice(&99u32.to_be_bytes());
        segment_bytes.extend_from_slice(b"short");
        fs.append(&partition_dir.join("0000000000.qseg"), &segment_bytes)
            .await
            .unwrap();
        let state = topic_state(4);

        let replayed = replay_partition(&fs, &partition_dir, "orders", 0, &state)
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

        let mut segment_bytes = frame(&Message::new("orders", "tenant-a", b"first".to_vec()));
        segment_bytes.extend_from_slice(&frame(&Message::new(
            "orders",
            "tenant-a",
            b"second".to_vec(),
        )));
        fs.append(&partition_dir.join("0000000000.qseg"), &segment_bytes)
            .await
            .unwrap();
        let state = topic_state(1);

        let error = replay_partition(&fs, &partition_dir, "orders", 0, &state)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("memory tier full"));
    }
}
