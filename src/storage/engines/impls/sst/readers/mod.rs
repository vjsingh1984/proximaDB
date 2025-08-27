//! LSM Reader implementations
//!
//! This module contains optimized readers for LSM storage:
//! - SstQueryEngine: High-level query execution and business logic
//! - Block-level caching and optimization
//! - Metadata bloom filters for efficient filtering
//! - Predictive prefetching for intelligent read-ahead

pub mod sst_query_engine;  // High-level query logic (formerly sst_query_engine)
pub mod predictive_prefetcher;
pub mod block_filter;

// Test modules
#[cfg(test)]
pub mod tests;

pub use sst_query_engine::{
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

