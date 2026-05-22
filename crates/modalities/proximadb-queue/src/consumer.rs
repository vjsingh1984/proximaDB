//! Consumer handle. Subscribes to specific partitions (no global subscribe —
//! the caller MUST declare which partitions it owns, mirroring Kafka's
//! assignor pattern). Polls messages, ack'd messages move the per-partition
//! committed offset forward.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::debug;

use crate::QueueClient;
use crate::error::{QueueError, Result};
use crate::memory_tier::MemoryEntry;
use crate::message::{Message, MessageId};
use crate::topic::PartitionId;

#[derive(Clone)]
pub struct Consumer {
    client: Arc<QueueClient>,
    group_id: String,
    /// (topic, partition) -> in-flight messages (poll'd but not yet ack'd).
    /// In a full implementation this also tracks the committed offset and
    /// nack-pending messages — Phase 1B scaffold tracks only in-flight.
    in_flight: Arc<DashMap<(String, PartitionId), Mutex<InFlight>>>,
}

#[derive(Default)]
struct InFlight {
    /// Messages handed out via `poll` that haven't been ack'd or nack'd.
    pending: Vec<MemoryEntry>,
}

impl Consumer {
    pub(crate) fn new(client: Arc<QueueClient>, group_id: String) -> Self {
        Self {
            client,
            group_id,
            in_flight: Arc::new(DashMap::new()),
        }
    }

    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Acquire ownership of the requested partitions for this consumer.
    /// In Phase 1B scaffold this is best-effort: an in-process lease that
    /// is enforced by `in_flight` membership. Cross-process leasing via
    /// disk-backed lease files lands when the disk tier is wired.
    pub async fn subscribe(&self, topic: &str, partitions: &[PartitionId]) -> Result<()> {
        // Auto-create the topic if missing (same shape as Producer::send).
        let state = match self.client.topic_state(topic).await {
            Some(s) => s,
            None => {
                self.client
                    .ensure_topic_async(topic, Default::default())
                    .await?
            }
        };
        for &p in partitions {
            if (p as usize) >= state.memory.len() {
                return Err(QueueError::PartitionNotFound {
                    topic: topic.to_string(),
                    partition: p,
                });
            }
            self.in_flight
                .entry((topic.to_string(), p))
                .or_insert_with(|| Mutex::new(InFlight::default()));
        }
        debug!(
            group = %self.group_id,
            topic = topic,
            partitions = ?partitions,
            "consumer subscribed"
        );
        Ok(())
    }

    /// Poll up to `max_batch` messages across owned partitions. Blocks up to
    /// `max_wait` for at least one message to arrive.
    pub async fn poll(&self, max_batch: usize, max_wait: Duration) -> Result<Vec<Message>> {
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            let mut out = Vec::with_capacity(max_batch);
            for entry in self.in_flight.iter() {
                let (topic, partition) = entry.key().clone();
                let state = match self.client.topic_state(&topic).await {
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
            // Nothing yet — wait on the first owned partition's notify or
            // until deadline.
            if tokio::time::Instant::now() >= deadline {
                return Ok(out);
            }
            if let Some(entry) = self.in_flight.iter().next() {
                let (topic, partition) = entry.key().clone();
                if let Some(state) = self.client.topic_state(&topic).await {
                    if let Some(part) = state.memory.get(partition as usize).cloned() {
                        let _ = tokio::time::timeout_at(deadline, part.notify.notified()).await;
                    }
                }
            } else {
                // No subscribed partitions; nothing to wait for.
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

        // Walk in_flight entries; per (topic, partition), drop the
        // acked messages AND track the max offset seen so we can commit
        // it after we release the per-partition lock.
        let mut to_commit: Vec<(String, PartitionId, u64)> = Vec::new();
        for entry in self.in_flight.iter() {
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

        // Persist outside the in_flight lock to keep contention minimal.
        // Idempotent + monotonic; safe to retry on transient failure.
        for (topic, partition, offset) in to_commit {
            crate::offset_store::commit(
                self.client.fs(),
                self.client.root_path(),
                &topic,
                partition,
                &self.group_id,
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
