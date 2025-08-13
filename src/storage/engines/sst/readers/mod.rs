//! LSM Reader implementations
//!
//! This module contains optimized readers for LSM storage:
//! - UnifiedSstableReader: Main reader with strategy selection
//! - Block-level caching and optimization
//! - Metadata bloom filters for efficient filtering
//! - Predictive prefetching for intelligent read-ahead

pub mod unified_sstable_reader;
pub mod predictive_prefetcher;
pub mod block_filter;

// Test modules
#[cfg(test)]
pub mod tests;

pub use unified_sstable_reader::{
    UnifiedSstableReader,
    CollectionContext,
};

pub use predictive_prefetcher::{
    PredictivePrefetcher,
    PrefetchConfig,
    PrefetchStats,
};

pub use block_filter::{
    BlockFilter, 
    IntelligentBlockFilter, 
    QueryType, 
    MetadataFilter, 
    BlockFilterStrategy
};

