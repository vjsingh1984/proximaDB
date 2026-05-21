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
