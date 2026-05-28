//! Cross-process partition leases — prevents two consumer instances
//! (running on different replicas) from competing for the same
//! `(topic, partition)` and producing duplicate work.
//!
//! ## Mechanism
//!
//! Each `(topic, partition)` has a `lease.meta` file at
//! `{queue_root}/{topic}/{partition}/lease.meta` containing
//! `{holder_id, expires_at_unix_nanos}`. Acquisition uses the same
//! temp-write + atomic-rename pattern as `offset_store`, plus a
//! re-read after the rename to detect lost-race situations.
//!
//! ## Acquisition flow
//!
//! 1. Read the existing `lease.meta` (if any). If it exists, is not
//!    expired, and belongs to someone else → `LeaseConflict`.
//! 2. Write our own `LeaseMeta` to a per-call temp path, fsync, then
//!    atomic-rename to `lease.meta`. Last rename wins.
//! 3. Re-read `lease.meta`. If `holder_id != ours`, we lost the rename
//!    race → `LeaseConflict`. Otherwise we hold the lease.
//!
//! Two callers acquiring concurrently both pass step 1, both rename in
//! step 2 (one wins), step 3 detects the loser. Correctness comes from
//! the atomic rename, not a global lock — so this works across
//! processes / pods.
//!
//! ## Renewal
//!
//! `Consumer::subscribe` spawns a background renewer task that calls
//! `renew()` every `lease_duration / 2` to keep the lease alive while
//! the consumer is running. On `Consumer` drop the renewer is
//! cancelled; the lease then expires naturally at its
//! `expires_at_unix_nanos` and a new consumer can take it over via
//! `try_acquire`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::QueueError;
use crate::fs::QueueFs;
use crate::topic::PartitionId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseMeta {
    pub holder_id: String,
    /// Unix-epoch nanoseconds. Compared against `now_unix_nanos()` to
    /// determine expiry without needing a wall-clock-synced cluster.
    /// On unsynced clocks the lease may release early or late by a few
    /// seconds — acceptable for the at-least-once + idempotent-consumer
    /// contract since at most one consumer makes progress at any time.
    pub expires_at_unix_nanos: u128,
}

const LEASE_FILE: &str = "lease.meta";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn lease_path(root: &Path, topic: &str, partition: PartitionId) -> PathBuf {
    root.join(topic)
        .join(partition.to_string())
        .join(LEASE_FILE)
}

fn tmp_path(meta: &Path) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = meta.as_os_str().to_owned();
    p.push(format!(".tmp.{pid}.{seq}"));
    PathBuf::from(p)
}

fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// Acquire (or take over an expired) lease on `(topic, partition)` for
/// `holder_id`. Returns `LeaseConflict` if a non-expired lease is held
/// by someone else.
pub async fn try_acquire(
    fs: &Arc<dyn QueueFs>,
    root: &Path,
    topic: &str,
    partition: PartitionId,
    holder_id: &str,
    lease_duration: Duration,
) -> crate::Result<()> {
    let path = lease_path(root, topic, partition);
    let now = now_unix_nanos();

    // Step 1: existing lease check.
    if let Ok(bytes) = fs.read(&path).await
        && !bytes.is_empty()
    {
        match serde_json::from_slice::<LeaseMeta>(&bytes) {
            Ok(existing)
                if existing.holder_id != holder_id && existing.expires_at_unix_nanos > now =>
            {
                return Err(QueueError::LeaseConflict {
                    topic: topic.to_string(),
                    partition,
                    holder: existing.holder_id,
                });
            }
            _ => {} // missing / corrupt / expired / our own — proceed.
        }
    }

    // Step 2: write our lease via temp + atomic rename.
    let new_meta = LeaseMeta {
        holder_id: holder_id.to_string(),
        expires_at_unix_nanos: now + lease_duration.as_nanos(),
    };
    let bytes = serde_json::to_vec(&new_meta)
        .map_err(|e| QueueError::Persistence(format!("lease serialize: {e}")))?;
    let tmp = tmp_path(&path);
    fs.append(&tmp, &bytes).await?;
    fs.fsync(&tmp).await?;
    fs.rename(&tmp, &path).await?;

    // Step 3: re-read to detect lost-race situations. Two concurrent
    // try_acquire calls can both pass step 1; one wins step 2's rename;
    // step 3 surfaces the conflict to the loser.
    let final_bytes = fs.read(&path).await?;
    let final_meta: LeaseMeta = serde_json::from_slice(&final_bytes)
        .map_err(|e| QueueError::Persistence(format!("lease re-read: {e}")))?;
    if final_meta.holder_id != holder_id {
        return Err(QueueError::LeaseConflict {
            topic: topic.to_string(),
            partition,
            holder: final_meta.holder_id,
        });
    }
    Ok(())
}

/// Refresh our lease's expiry. Same protocol as `try_acquire`; a stale
/// holder (we crashed and the lease expired and got taken over) gets a
/// `LeaseConflict` and should stop processing.
pub async fn renew(
    fs: &Arc<dyn QueueFs>,
    root: &Path,
    topic: &str,
    partition: PartitionId,
    holder_id: &str,
    lease_duration: Duration,
) -> crate::Result<()> {
    try_acquire(fs, root, topic, partition, holder_id, lease_duration).await
}

#[cfg(test)]
pub(crate) async fn read_meta(
    fs: &Arc<dyn QueueFs>,
    root: &Path,
    topic: &str,
    partition: PartitionId,
) -> crate::Result<Option<LeaseMeta>> {
    let path = lease_path(root, topic, partition);
    match fs.read(&path).await {
        Ok(bytes) if !bytes.is_empty() => {
            Ok(Some(serde_json::from_slice(&bytes).map_err(|e| {
                QueueError::Persistence(format!("lease read: {e}"))
            })?))
        }
        _ => Ok(None),
    }
}
