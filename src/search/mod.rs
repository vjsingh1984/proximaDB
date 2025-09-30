//! Search optimization modules

pub mod optimization;

pub use optimization::{
    // Arc-based cloning
    CloneStatistics, CloneStrategy, CloneStrategySelector, MemorySharingConfig,
    // Batch strategy (delegates to UnifiedDistanceCompute)
    BatchStrategy, BatchStrategySelector, should_process_sequentially,
    // Metadata projection (reuses existing infrastructure)
    AccessTracker, FieldName, MetadataProjectionConfig, MetadataProjectionOptimizer,
    estimate_projection_benefit, extract_field_names,
};