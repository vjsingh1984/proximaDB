//! Integration with other systems

pub mod eventlog_consumer;
pub mod flush_integration;
pub mod collection_state;
pub mod tiering_manager;
pub mod memory_tracker;

// Re-export main types
pub use eventlog_consumer::{
    EventLogConsumer, ConsumerConfig, ConsumerStats,
};

pub use flush_integration::{
    FlushIntegration, FlushConfig, FlushStats,
};

pub use collection_state::{
    CollectionStateManager, CollectionTierState, TierLevel, CloudStorageType,
};

pub use tiering_manager::{
    AxisTieringManager, AxisTieringConfig, TieringStats,
};

pub use memory_tracker::{
    IndexMemoryTracker, IndexMemoryStatus, Index,
    MemoryState, EvictionReason, MemoryStats,
};