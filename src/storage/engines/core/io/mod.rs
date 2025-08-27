//! I/O infrastructure for storage engines
//! 
//! Provides zero-copy I/O, filesystem abstractions, and bandwidth optimization

pub mod zero_copy;

// Re-export main types
pub use zero_copy::{
    ZeroCopyIOSystem,
    bandwidth_optimizer::BandwidthOptimizer,
    access_tracker::AccessPatternTracker,
    metrics::SystemPerformanceMetrics,
    traits::{MetadataSerializer, QueryContext, FileAccessRequest},
    config::ZeroCopyIOConfig,
};

// Note: ZeroCopyFilesystem is in crate::storage::persistence::filesystem