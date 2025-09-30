//! Search optimization module
//!
//! Provides performance optimizations for search operations including:
//! - Arc-based memory sharing with dimension-aware strategy selection
//! - Clone strategy optimization to avoid performance inversions
//! - Batch strategy selection (delegates to UnifiedDistanceCompute)
//! - Metadata projection to reduce deserialization overhead

pub mod clone_strategy;
pub mod batch_strategy;
pub mod field_projection;
pub mod metadata_projection;

pub use clone_strategy::{
    CloneStatistics, CloneStrategy, CloneStrategySelector, MemorySharingConfig,
};

pub use batch_strategy::{
    BatchStrategy, BatchStrategySelector, should_process_sequentially,
};

pub use field_projection::{
    FieldProjection, FieldName,
};

pub use metadata_projection::{
    // Re-export AccessTracker from cache eviction (no duplicate!)
    AccessTracker,
    // Metadata projection types
    MetadataProjectionConfig, MetadataProjectionOptimizer,
    estimate_projection_benefit, extract_field_names,
};