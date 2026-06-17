//! Consumer handle. Subscribes to specific partitions (no global subscribe —
//! the caller MUST declare which partitions it owns, mirroring Kafka's
//! assignor pattern). Polls messages, ack'd messages move the per-partition
//! committed offset forward.
//!
//! Cross-process safety comes from `leases.rs`: each subscribe acquires
//! a `lease.meta` file via temp+rename+reread CAS, and a background
//! renewer task refreshes it at half the lease duration. On `Consumer`
//! drop the renewer is cancelled and the lease expires naturally —
//! another replica can then take over.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::QueueClient;
use crate::error::{QueueError, Result};
use crate::memory_tier::MemoryEntry;
use crate::message::{Message, MessageId};
use crate::topic::PartitionId;

#[derive(Clone)]
pub struct Consumer {
    inner: Arc<ConsumerInner>,
}

pub(crate) struct ConsumerInner {
    client: Arc<QueueClient>,
    group_id: String,
    /// (topic, partition) -> in-flight messages (poll'd but not yet ack'd).
    in_flight: DashMap<(String, PartitionId), Mutex<InFlight>>,
    /// Renewer tasks keeping each subscribed partition's lease alive.
    /// Dropped when the last Consumer Arc goes away → renewers exit
    /// → leases expire naturally → another replica can take them over.
    renewers: Mutex<Vec<RenewerHandle>>,
}

struct RenewerHandle {
    _shutdown_tx: oneshot::Sender<()>,
    /// Background task handle held so the task is owned by this struct
    /// even if `await`ing it is not strictly required at every drop
    /// site. Marked `#[allow(dead_code)]` because the read side is
    /// implicit (handle ownership keeps the renewer alive).
    #[allow(dead_code)]
    join: JoinHandle<()>,
}

#[derive(Default)]
struct InFlight {
    /// Messages handed out via `poll` that haven't been ack'd or nack'd.
    pending: Vec<MemoryEntry>,
}

impl Consumer {
    pub(crate) fn new(client: Arc<QueueClient>, group_id: String) -> Self {
        Self {
            inner: Arc::new(ConsumerInner {
                client,
                group_id,
                in_flight: DashMap::new(),
                renewers: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn group_id(&self) -> &str {
        &self.inner.group_id
    }

    /// Acquire ownership of the requested partitions for this consumer.
    /// Each partition's lease is acquired via the cross-process
    /// `leases::try_acquire` CAS; a non-expired conflicting holder
    /// produces `QueueError::LeaseConflict`.
    pub async fn subscribe(&self, topic: &str, partitions: &[PartitionId]) -> Result<()> {
        let state = match self.inner.client.topic_state(topic).await {
            Some(s) => s,
            None => {
                self.inner
                    .client
                    .ensure_topic_async(topic, Default::default())
                    .await?
            }
        };
        let lease_duration = state.config.lease_duration;
        let holder_id = self.inner.client.instance_id().to_string();
        let fs = self.inner.client.fs().clone();
        let root = self.inner.client.root_path().clone();

        for &p in partitions {
            if (p as usize) >= state.memory.len() {
                return Err(QueueError::PartitionNotFound {
                    topic: topic.to_string(),
                    partition: p,
                });
            }
            // Acquire the cross-process lease before doing any in-process
            // bookkeeping — a conflict here means we're not allowed to
            // touch this partition.
            crate::leases::try_acquire(&fs, &root, topic, p, &holder_id, lease_duration).await?;

            self.inner
                .in_flight
                .entry((topic.to_string(), p))
                .or_insert_with(|| Mutex::new(InFlight::default()));

            // Spawn a renewer task — every lease_duration/2 it re-runs
            // try_acquire to push the expiry forward. On Consumer drop
            // the shutdown_tx is dropped, the select! exits, the lease
            // expires naturally, and another replica can take over.
            let (tx, mut rx) = oneshot::channel::<()>();
            let renew_fs = fs.clone();
            let renew_root = root.clone();
            let renew_topic = topic.to_string();
            let renew_holder = holder_id.clone();
            let interval = lease_duration / 2;
            let join = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = &mut rx => {
                            // Clean shutdown — best-effort delete the
                            // lease.meta so a follow-on subscriber
                            // (different replica, same process+next
                            // QueueClient) doesn't wait for expiry.
                            // Failure here is non-fatal: the lease
                            // will expire on its own.
                            let lease_path = renew_root
                                .join(&renew_topic)
                                .join(p.to_string())
                                .join("lease.meta");
                            let _ = renew_fs.delete(&lease_path).await;
                            break;
                        }
                        _ = tokio::time::sleep(interval) => {
                            if let Err(e) = crate::leases::renew(
                                &renew_fs,
                                &renew_root,
                                &renew_topic,
                                p,
                                &renew_holder,
                                lease_duration,
                            ).await {
                                warn!(
                                    topic = %renew_topic,
                                    partition = p,
                                    holder = %renew_holder,
                                    error = %e,
                                    "lease renewal failed; another replica may take over",
                                );
                                break;
                            }
                        }
                    }
                }
            });
            self.inner.renewers.lock().await.push(RenewerHandle {
                _shutdown_tx: tx,
                join,
            });
        }
        debug!(
            group = %self.inner.group_id,
            topic = topic,
            partitions = ?partitions,
            instance = %holder_id,
            "consumer subscribed (lease acquired)"
        );
        Ok(())
    }

    /// Poll up to `max_batch` messages across owned partitions. Blocks up to
    /// `max_wait` for at least one message to arrive.
    pub async fn poll(&self, max_batch: usize, max_wait: Duration) -> Result<Vec<Message>> {
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            let mut out = Vec::with_capacity(max_batch);
            for entry in self.inner.in_flight.iter() {
                let (topic, partition) = entry.key().clone();
                let state = match self.inner.client.topic_state(&topic).await {
                    Some(s) => s,
                    None => continue,
                };
                let part = match state.memory.get(partition as usize) {
                    Some(p) => p.clone(),
                    None => continue,
                };
                let remaining = max_batch.saturating_sub(out.len());
                if remaining == 0 {
                    break;
                }
                let batch = part.try_pop_batch(remaining);
                if !batch.is_empty() {
                    let mut tracker = entry.value().lock().await;
                    for memo in &batch {
                        tracker.pending.push(memo.clone());
                    }
                    drop(tracker);
                    for memo in batch {
                        out.push(memo.message);
                    }
                }
            }
            if !out.is_empty() {
                return Ok(out);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(out);
            }
            if let Some(entry) = self.inner.in_flight.iter().next() {
                let (topic, partition) = entry.key().clone();
                if let Some(state) = self.inner.client.topic_state(&topic).await
                    && let Some(part) = state.memory.get(partition as usize).cloned()
                {
                    let _ = tokio::time::timeout_at(deadline, part.notify.notified()).await;
                }
            } else {
                return Ok(out);
            }
        }
    }

    /// Acknowledge messages as fully processed. Clears them from the
    /// in-flight tracker AND persists the new committed offset per
    /// (topic, partition) via `offset_store::commit` so a process
    /// restart resumes at the right cursor instead of re-delivering.
    pub async fn ack(&self, message_ids: &[MessageId]) -> Result<()> {
        let target_ids: HashSet<&MessageId> = message_ids.iter().collect();

        let mut to_commit: Vec<(String, PartitionId, u64)> = Vec::new();
        for entry in self.inner.in_flight.iter() {
            let (topic, partition) = entry.key().clone();
            let mut tracker = entry.value().lock().await;
            let mut max_acked: Option<u64> = None;
            tracker.pending.retain(|memo| {
                if target_ids.contains(&memo.message_id) {
                    max_acked = Some(max_acked.map_or(memo.offset, |m| m.max(memo.offset)));
                    false
                } else {
                    true
                }
            });
            if let Some(offset) = max_acked {
                to_commit.push((topic, partition, offset));
            }
        }

        for (topic, partition, offset) in to_commit {
            crate::offset_store::commit(
                self.inner.client.fs(),
                self.inner.client.root_path(),
                &topic,
                partition,
                &self.inner.group_id,
                offset,
            )
            .await?;
        }
        Ok(())
    }

    /// Negative ack — return messages to the front of their partition for
    /// retry with incremented `attempt_count`. Phase 1B scaffold drops them
    /// (no requeue) and just clears the in-flight tracker; full retry +
    /// DLQ promotion lands with the disk tier.
    pub async fn nack(&self, message_ids: &[MessageId]) -> Result<()> {
        self.ack(message_ids).await
    }
}

impl Drop for ConsumerInner {
    /// On last-clone drop, signal each renewer to shut down and let
    /// it cleanly delete its `lease.meta` (best-effort) so a follow-on
    /// subscriber doesn't have to wait for the lease to expire.
    ///
    /// We do NOT call `JoinHandle::abort()` here — that would kill the
    /// task before it could run its shutdown branch. Instead, dropping
    /// the Vec drops each `_shutdown_tx`, which signals the renewer's
    /// `select!` to exit via its `rx => { ... cleanup ... }` arm.
    fn drop(&mut self) {
        // Best-effort: signal shutdown by dropping the renewers vec.
        // The shutdown_tx fields drop, the renewer tasks notice the
        // rx side closed, and they run the lease.meta delete inside
        // their select! before exiting.
        if let Ok(mut renewers) = self.renewers.try_lock() {
            renewers.clear();
        }
    }
}
