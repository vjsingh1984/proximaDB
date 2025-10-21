//! Search optimization modules

pub mod optimization;

pub use optimization::{
    // Metadata projection (reuses existing infrastructure)
    AccessTracker,
    // Batch strategy (delegates to UnifiedDistanceCompute)
    BatchStrategy,
    BatchStrategySelector,
    // Arc-based cloning
    CloneStatistics,
    CloneStrategy,
    CloneStrategySelector,
    FieldName,
    MemorySharingConfig,
    MetadataProjectionConfig,
    MetadataProjectionOptimizer,
    estimate_projection_benefit,
    extract_field_names,
    should_process_sequentially,
};
