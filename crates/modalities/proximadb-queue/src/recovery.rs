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
/// Phase-staged: the `QueueClient` accessors that earlier drafts of this
/// function depended on (`topic_names`, `topic_state`, `root_path`, `fs`)
/// are being moved behind a new public surface. Until that lands, this
/// function is a no-op stub so the workspace compiles end-to-end and the
/// recovery wire-up is preserved in one place for the follow-up. Any
/// downstream caller (e.g. lib.rs startup) keeps the same signature.
pub async fn recover(_client: &QueueClient) -> crate::Result<usize> {
    debug!("recovery: noop while QueueClient public accessors land");
    Ok(0)
}

#[allow(dead_code)]
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
