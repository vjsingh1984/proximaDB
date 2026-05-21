//! Queue error types.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, QueueError>;

#[derive(Debug, Error)]
pub enum QueueError {
    #[error("topic not found: {0}")]
    TopicNotFound(String),

    #[error("partition not found: topic={topic}, partition={partition}")]
    PartitionNotFound { topic: String, partition: u32 },

    /// Soft backpressure — caller MAY retry or accept slight slowdown.
    /// Returned when memory tier is between soft and hard threshold.
    #[error("queue backpressure (soft): memory tier at {pct:.0}%")]
    BackpressureSoft { pct: f32 },

    /// Hard backpressure — caller MUST back off. Includes suggested
    /// retry-after in milliseconds.
    #[error("queue backpressure (hard): memory tier at {pct:.0}%; retry after {retry_after_ms}ms")]
    Backpressure { pct: f32, retry_after_ms: u64 },

    /// Disk write or fsync failed. Caller should treat as unrecoverable
    /// for the current request.
    #[error("queue persistence failure: {0}")]
    Persistence(String),

    /// Consumer lease conflict — another instance owns the partition.
    #[error("lease conflict on topic={topic}, partition={partition}: held by {holder}")]
    LeaseConflict {
        topic: String,
        partition: u32,
        holder: String,
    },

    /// Message acknowledgement targeted an unknown message id.
    #[error("unknown message id: {0}")]
    UnknownMessageId(String),

    /// Wrapper for unexpected I/O or serialization errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
