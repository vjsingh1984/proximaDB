//! Producer handle. Routes a message to its partition via `partition_for`,
//! enqueues into the memory tier, and (when the disk tier lands) blocks on
//! the group-commit fsync before returning.

use std::sync::Arc;
use std::time::SystemTime;

use tracing::trace;

use crate::QueueClient;
use crate::config::SyncMode;
use crate::error::{QueueError, Result};
use crate::message::{Message, MessageReceipt};
use crate::topic::partition_for;

#[derive(Clone)]
pub struct Producer {
    client: Arc<QueueClient>,
}

impl Producer {
    pub(crate) fn new(client: Arc<QueueClient>) -> Self {
        Self { client }
    }

    /// Send a single message. Strict mode blocks until the message is on
    /// disk (group-commit fsync) before returning. Lazy mode returns
    /// after the memory + disk-append; fsync happens in the background.
    ///
    /// Write order: hard-pressure check → disk_writer.append (durable
    /// segment) → memory_tier.try_enqueue (consumer visibility) → (Strict)
    /// wait_for_fsync. We append to disk *before* memory so a memory-tier
    /// full rejection doesn't leak phantom messages to consumers.
    pub async fn send(&self, message: Message) -> Result<MessageReceipt> {
        let topic_name = message.topic.clone();
        let state = match self.client.topic_state(&topic_name).await {
            Some(s) => s,
            None => {
                self.client
                    .ensure_topic_async(&topic_name, Default::default())
                    .await?
            }
        };

        let partition_id = partition_for(&message.tenant_id, state.config.partition_count);
        let part = state
            .memory
            .get(partition_id as usize)
            .ok_or(QueueError::PartitionNotFound {
                topic: topic_name.clone(),
                partition: partition_id,
            })?
            .clone();
        let disk_writer = state
            .disk_writers
            .get(partition_id as usize)
            .ok_or(QueueError::PartitionNotFound {
                topic: topic_name.clone(),
                partition: partition_id,
            })?
            .clone();

        // Hard backpressure check before any I/O. Memory-full rejection
        // would later leak phantom disk writes — fail fast instead.
        if let Some(crate::memory_tier::PressureLevel::Hard(pct)) = part.pressure() {
            return Err(QueueError::Backpressure {
                pct: pct * 100.0,
                retry_after_ms: 100,
            });
        }

        // Persist to disk first (memory rejection wouldn't leak).
        let outcome = disk_writer.append(&message).await?;

        let mut to_send = message;
        let (entry, backpressure_hint) = part.try_enqueue(to_send.clone()).map_err(|m| {
            to_send = m;
            QueueError::Backpressure {
                pct: part.depth_pct() * 100.0,
                retry_after_ms: 100,
            }
        })?;

        // Strict mode: block on the group-commit fsync barrier so the
        // returned receipt's `fsynced_at` is a real guarantee.
        // Lazy mode: return as soon as the memory enqueue succeeds; the
        // group-commit drainer will fsync the segment in the background
        // within `group_commit_max_wait`.
        let sync_mode = state
            .config
            .sync_mode_override
            .unwrap_or(self.client.config().default_sync_mode);
        let fsynced_at = match sync_mode {
            SyncMode::Strict => {
                disk_writer.wait_for_fsync(outcome.segment_path).await?;
                Some(SystemTime::now())
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueueConfig, TopicConfig};
    use std::collections::HashMap;

    fn lazy_single_partition_config(root: String) -> QueueConfig {
        let mut topics = HashMap::new();
        topics.insert(
            "orders".to_string(),
            TopicConfig {
                partition_count: 1,
                memory_capacity: 8,
                sync_mode_override: Some(SyncMode::Lazy),
                ..TopicConfig::default()
            },
        );
        QueueConfig {
            root,
            default_sync_mode: SyncMode::Lazy,
            topics,
            ..QueueConfig::default()
        }
    }

    #[tokio::test]
    async fn send_batch_returns_receipts_in_input_order() {
        let dir = tempfile::tempdir().unwrap();
        let client = QueueClient::open(lazy_single_partition_config(format!(
            "file://{}",
            dir.path().display()
        )))
        .await
        .unwrap();
        let producer = Producer::new(client);

        assert!(producer.send_batch(Vec::new()).await.unwrap().is_empty());

        let receipts = producer
            .send_batch(vec![
                Message::new("orders", "tenant-a", b"first".to_vec()),
                Message::new("orders", "tenant-a", b"second".to_vec()),
                Message::new("orders", "tenant-a", b"third".to_vec()),
            ])
            .await
            .unwrap();

        assert_eq!(receipts.len(), 3);
        assert_eq!(
            receipts
                .iter()
                .map(|receipt| receipt.offset)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(receipts.iter().all(|receipt| receipt.partition == 0));
        assert!(receipts.iter().all(|receipt| receipt.fsynced_at.is_none()));
    }
}
