//! I/O infrastructure for storage engines
//!
//! Provides zero-copy I/O, filesystem abstractions, and bandwidth optimization

pub mod zero_copy;

// Re-export main types
pub use zero_copy::{
    ZeroCopyIOSystem,
    access_tracker::AccessPatternTracker,
    bandwidth_optimizer::BandwidthOptimizer,
    config::ZeroCopyIOConfig,
    metrics::SystemPerformanceMetrics,
    traits::{FileAccessRequest, MetadataSerializer, QueryContext},
};

// Note: ZeroCopyFilesystem is in crate::storage::persistence::filesystem
