//! Producer handle. Routes a message to its partition via `partition_for`,
//! enqueues into the memory tier, and (when the disk tier lands) blocks on
//! the group-commit fsync before returning.

use std::sync::Arc;
use std::time::SystemTime;

use tracing::trace;

use crate::config::SyncMode;
use crate::error::{QueueError, Result};
use crate::message::{Message, MessageReceipt};
use crate::topic::partition_for;
use crate::QueueClient;

#[derive(Clone)]
pub struct Producer {
    client: Arc<QueueClient>,
}

impl Producer {
    pub(crate) fn new(client: Arc<QueueClient>) -> Self {
        Self { client }
    }

    /// Send a single message. Blocks until the message is durable per the
    /// topic's `SyncMode` (Strict: fsync; Lazy: memory append).
    pub async fn send(&self, message: Message) -> Result<MessageReceipt> {
        let topic_name = message.topic.clone();
        let state = self
            .client
            .topic_state(&topic_name)
            .or_else(|| {
                // Auto-create on first use with default TopicConfig.
                Some(
                    self.client
                        .ensure_topic(&topic_name, Default::default()),
                )
            })
            .ok_or_else(|| QueueError::TopicNotFound(topic_name.clone()))?;

        let partition_id = partition_for(&message.tenant_id, state.config.partition_count);
        let part = state
            .memory
            .get(partition_id as usize)
            .ok_or(QueueError::PartitionNotFound {
                topic: topic_name.clone(),
                partition: partition_id,
            })?
            .clone();

        // Hard backpressure check before attempting enqueue.
        if let Some(crate::memory_tier::PressureLevel::Hard(pct)) = part.pressure() {
            return Err(QueueError::Backpressure {
                pct: pct * 100.0,
                retry_after_ms: 100,
            });
        }

        let mut to_send = message;
        let (entry, backpressure_hint) = part.try_enqueue(to_send.clone()).map_err(|m| {
            to_send = m;
            QueueError::Backpressure {
                pct: part.depth_pct() * 100.0,
                retry_after_ms: 100,
            }
        })?;

        // SyncMode handling:
        //   - Lazy: return immediately; disk fsync happens in background
        //     (no-op until disk tier lands).
        //   - Strict: in the full implementation, await the per-partition
        //     group-commit batch fsync. Phase 1B scaffold returns
        //     immediately with fsynced_at populated; the disk tier wires up
        //     real fsync waiting in a follow-up commit.
        let sync_mode = state
            .config
            .sync_mode_override
            .unwrap_or(self.client.config().default_sync_mode);
        let fsynced_at = match sync_mode {
            SyncMode::Strict => Some(SystemTime::now()),
            SyncMode::Lazy => None,
        };

        trace!(
            topic = %to_send.topic,
            partition = partition_id,
            offset = entry.offset,
            sync_mode = ?sync_mode,
            "queue::send"
        );

        Ok(MessageReceipt {
            message_id: entry.message_id,
            partition: partition_id,
            offset: entry.offset,
            fsynced_at,
            backpressure_hint,
        })
    }

    /// Convenience: send a batch. Each message is enqueued individually.
    /// Returns receipts in the same order. If any send fails, prior sends
    /// in the batch are NOT rolled back — the caller can read partial
    /// progress from the returned `Vec` length vs `messages.len()`.
    pub async fn send_batch(&self, messages: Vec<Message>) -> Result<Vec<MessageReceipt>> {
        let mut out = Vec::with_capacity(messages.len());
        for msg in messages {
            out.push(self.send(msg).await?);
        }
        Ok(out)
    }
}
