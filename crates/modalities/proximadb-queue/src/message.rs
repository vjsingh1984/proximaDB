//! Message envelope + identifier shapes.

use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::topic::PartitionId;

/// Stable identifier for a persisted message. Format
/// `{partition}:{segment}:{offset}` so consumers can extract the partition
/// without parsing the message body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl MessageId {
    pub fn new(partition: PartitionId, segment_id: u64, offset: u64) -> Self {
        Self(format!("{partition}:{segment_id}:{offset}"))
    }

    pub fn partition(&self) -> Option<PartitionId> {
        self.0.split(':').next().and_then(|s| s.parse().ok())
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One producer message. Opaque payload — encoded by the caller (typically
/// bincode or JSON of a `RichRecordBatchRequest` for embed-ingest).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub topic: String,
    /// Partition key — hashed via `partition_for` to select a partition.
    pub tenant_id: String,
    pub payload: Vec<u8>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Producer-side timestamp (UTC). Consumers use this for lag metrics.
    #[serde(default = "default_now")]
    pub produced_at: SystemTime,
    /// Incremented by `Consumer::nack` for retry semantics. Producers send
    /// with `attempt_count = 0`.
    #[serde(default)]
    pub attempt_count: u32,
}

fn default_now() -> SystemTime {
    SystemTime::now()
}

impl Message {
    pub fn new(topic: impl Into<String>, tenant_id: impl Into<String>, payload: Vec<u8>) -> Self {
        Self {
            topic: topic.into(),
            tenant_id: tenant_id.into(),
            payload,
            headers: HashMap::new(),
            produced_at: SystemTime::now(),
            attempt_count: 0,
        }
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
}

/// Returned to the producer after a successful send. In `SyncMode::Strict`,
/// `fsynced_at` is `Some(...)` indicating the segment is durable on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageReceipt {
    pub message_id: MessageId,
    pub partition: PartitionId,
    pub offset: u64,
    pub fsynced_at: Option<SystemTime>,
    /// Soft backpressure hint — tells the producer the memory tier is
    /// approaching saturation. None when below the soft threshold.
    pub backpressure_hint: Option<BackpressureHint>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BackpressureHint {
    /// Memory tier > 75% of capacity. Consider easing producer rate.
    Soft,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_id_formats_displays_and_extracts_partition() {
        let id = MessageId::new(7, 42, 99);

        assert_eq!(id.0, "7:42:99");
        assert_eq!(id.to_string(), "7:42:99");
        assert_eq!(id.partition(), Some(7));
    }

    #[test]
    fn message_id_partition_parsing_rejects_malformed_prefixes() {
        assert_eq!(
            MessageId("not-a-partition:1:2".to_string()).partition(),
            None
        );
        assert_eq!(MessageId("".to_string()).partition(), None);
    }

    #[test]
    fn message_new_sets_identity_payload_defaults_and_headers() {
        let message = Message::new("topic-a", "tenant-a", b"payload".to_vec())
            .with_header("content-type", "application/json")
            .with_header("trace-id", "abc");

        assert_eq!(message.topic, "topic-a");
        assert_eq!(message.tenant_id, "tenant-a");
        assert_eq!(message.payload, b"payload");
        assert_eq!(
            message.headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            message.headers.get("trace-id").map(String::as_str),
            Some("abc")
        );
        assert_eq!(message.attempt_count, 0);
    }

    #[test]
    fn receipt_carries_durability_and_backpressure_metadata() {
        let receipt = MessageReceipt {
            message_id: MessageId::new(1, 0, 2),
            partition: 1,
            offset: 2,
            fsynced_at: None,
            backpressure_hint: Some(BackpressureHint::Soft),
        };

        assert_eq!(receipt.message_id.partition(), Some(1));
        assert_eq!(receipt.partition, 1);
        assert_eq!(receipt.offset, 2);
        assert!(receipt.fsynced_at.is_none());
        assert!(matches!(
            receipt.backpressure_hint,
            Some(BackpressureHint::Soft)
        ));
    }

    #[test]
    fn message_round_trips_through_bincode() {
        let message =
            Message::new("topic-a", "tenant-a", vec![1, 2, 3]).with_header("schema", "v1");

        let restored: Message =
            bincode::deserialize(&bincode::serialize(&message).unwrap()).unwrap();

        assert_eq!(restored.topic, message.topic);
        assert_eq!(restored.tenant_id, message.tenant_id);
        assert_eq!(restored.payload, message.payload);
        assert_eq!(restored.headers, message.headers);
        assert_eq!(restored.attempt_count, 0);
    }
}
