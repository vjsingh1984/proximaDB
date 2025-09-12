//! Integration with other systems

pub mod collection_state;
pub mod eventlog_consumer;
pub mod flush_integration;
pub mod memory_tracker;
pub mod tiering_manager;

// Re-export main types
pub use eventlog_consumer::{ConsumerConfig, ConsumerStats, EventLogConsumer};

pub use flush_integration::{FlushConfig, FlushIntegration, FlushStats};

pub use collection_state::{
    CloudStorageType, CollectionStateManager, CollectionTierState, TierLevel,
};

pub use tiering_manager::{AxisTieringConfig, AxisTieringManager, TieringStats};

pub use memory_tracker::{
    EvictionReason, Index, IndexMemoryStatus, IndexMemoryTracker, MemoryState, MemoryStats,
};
