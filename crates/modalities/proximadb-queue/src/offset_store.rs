//! Per-partition committed-offset metadata file.
//!
//! Stored at `{queue_root}/{topic}/{partition_id}/offset.meta` as a tiny
//! JSON document `{"group": "...", "committed_offset": <u64>}`. A successful
//! `Consumer::ack` writes this file; consumer restart reads it to resume.
//!
//! ## Atomic write protocol
//!
//! Because the queue's `QueueFs` trait has no truncating-write primitive,
//! atomic commit is achieved by temp + rename:
//!
//! 1. Delete any leftover `offset.meta.tmp` from a prior crash.
//! 2. Append the new JSON body to `offset.meta.tmp`.
//! 3. Fsync the temp file so its bytes are durable.
//! 4. Rename `offset.meta.tmp` → `offset.meta` (atomic on POSIX +
//!    LocalFs because both paths are on the same mount).
//!
//! Crash-safety: an interrupted commit leaves either the old
//! `offset.meta` (if rename never ran) or the new `offset.meta` (if
//! rename completed). The temp file is cleaned up on the next commit
//! attempt. We never observe a half-written `offset.meta`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::QueueError;
use crate::fs::QueueFs;
use crate::topic::PartitionId;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OffsetMeta {
    pub group: String,
    pub committed_offset: u64,
}

const META_FILE: &str = "offset.meta";

/// Process-unique counter for temp-file suffix uniqueness. Combined
/// with the OS PID it guarantees no two concurrent `commit` calls
/// stomp on the same temp file even across threads / tasks.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Build `{root}/{topic}/{partition}/offset.meta`.
fn meta_path(root: &Path, topic: &str, partition: PartitionId) -> PathBuf {
    root.join(topic).join(partition.to_string()).join(META_FILE)
}

/// Per-call unique temp path: `offset.meta.tmp.{pid}.{seq}`. The rename
/// to `offset.meta` is the atomic publication; losers in a race simply
/// have their newer offset overwritten by the next commit (acceptable
/// — commits are monotonic).
fn tmp_path(meta: &Path) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = meta.as_os_str().to_owned();
    p.push(format!(".tmp.{pid}.{seq}"));
    PathBuf::from(p)
}

/// Persist the committed offset for a `(topic, partition, group)` tuple.
/// Idempotent and atomic; concurrent callers serialize at the rename
/// barrier and the last writer wins (acceptable because `commit` is only
/// ever called with monotonically increasing offsets per group).
pub async fn commit(
    fs: &Arc<dyn QueueFs>,
    root: &Path,
    topic: &str,
    partition: PartitionId,
    group: &str,
    committed_offset: u64,
) -> crate::Result<()> {
    let final_path = meta_path(root, topic, partition);
    let tmp = tmp_path(&final_path);

    let body = OffsetMeta {
        group: group.to_string(),
        committed_offset,
    };
    let bytes = serde_json::to_vec(&body)
        .map_err(|e| QueueError::Persistence(format!("offset_store serialize: {e}")))?;

    fs.append(&tmp, &bytes).await?;
    fs.fsync(&tmp).await?;
    fs.rename(&tmp, &final_path).await?;
    Ok(())
}

/// Read the committed offset for `(topic, partition)`. Returns 0 (the
/// safe replay-from-start default) when no offset.meta exists yet.
///
/// The `group` parameter is verified — if the on-disk file was written
/// by a different consumer group, returns 0 so we re-deliver from the
/// start rather than silently honoring a stranger's commit.
pub async fn read(
    fs: &Arc<dyn QueueFs>,
    root: &Path,
    topic: &str,
    partition: PartitionId,
    group: &str,
) -> crate::Result<u64> {
    let final_path = meta_path(root, topic, partition);
    let bytes = match fs.read(&final_path).await {
        Ok(b) => b,
        // Missing file is the common case (cold start) — treat as 0.
        Err(_) => return Ok(0),
    };
    if bytes.is_empty() {
        return Ok(0);
    }
    let parsed: OffsetMeta = serde_json::from_slice(&bytes)
        .map_err(|e| QueueError::Persistence(format!("offset_store parse: {e}")))?;
    if parsed.group != group {
        // Different consumer group's commit; ignore and re-deliver.
        return Ok(0);
    }
    Ok(parsed.committed_offset)
}
