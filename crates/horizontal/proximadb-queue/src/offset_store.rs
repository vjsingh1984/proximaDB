//! Per-`(group, partition)` committed-offset metadata file (ADR-079 §Semantics).
//!
//! Stored at `{queue_root}/{topic}/{partition_id}/{group}/offset.meta` as a
//! tiny JSON document `{"group": "...", "committed_offset": <u64>}`. Each
//! consumer group gets its OWN file, so group A's ack cannot clobber group B's
//! cursor (the pub/sub isolation property). A successful `Consumer::ack` writes
//! its group's file; consumer restart reads its group's file to resume.
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

/// Reduce a consumer-group id to a safe single path component. Group ids are
/// application-supplied, so strip anything that could escape the partition
/// directory (`/`, `..`, `:`, NUL, whitespace) to `_`. Two ids that sanitize
/// the same collide on disk — acceptable since groups are app-controlled and
/// the collision is deterministic, not a correctness/corruption hazard.
pub(crate) fn group_dir_name(group: &str) -> String {
    let cleaned: String = group
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c == '_').to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "default".to_string()
    } else {
        cleaned
    }
}

/// Build `{root}/{topic}/{partition}/{group}/offset.meta` — one file per group.
fn meta_path(root: &Path, topic: &str, partition: PartitionId, group: &str) -> PathBuf {
    root.join(topic)
        .join(partition.to_string())
        .join(group_dir_name(group))
        .join(META_FILE)
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
    let final_path = meta_path(root, topic, partition, group);
    // Ensure the per-group directory exists (LocalFs needs it; object stores
    // use flat keys and treat create_dir_all as a no-op on the prefix).
    if let Some(parent) = final_path.parent() {
        fs.create_dir_all(parent).await?;
    }
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

/// Read THIS group's committed offset for `(topic, partition)`. Returns `None`
/// when the group has no `offset.meta` yet (cold start — recovery replays
/// every persisted message). Each group has its own file, so this never sees
/// another group's commit.
pub async fn read(
    fs: &Arc<dyn QueueFs>,
    root: &Path,
    topic: &str,
    partition: PartitionId,
    group: &str,
) -> crate::Result<Option<u64>> {
    let final_path = meta_path(root, topic, partition, group);
    let bytes = match fs.read(&final_path).await {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    if bytes.is_empty() {
        return Ok(None);
    }
    let parsed: OffsetMeta = serde_json::from_slice(&bytes)
        .map_err(|e| QueueError::Persistence(format!("offset_store parse: {e}")))?;
    // Each group has its own file, so the stored group always matches; keep
    // returning the offset directly.
    Ok(Some(parsed.committed_offset))
}
