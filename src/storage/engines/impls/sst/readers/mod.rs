//! LSM Reader implementations
//!
//! This module contains optimized readers for LSM storage:
//! - SstQueryEngine: High-level query execution and business logic
//! - Block-level caching and optimization
//! - Metadata bloom filters for efficient filtering
//! - Predictive prefetching for intelligent read-ahead

pub mod block_filter;
pub mod morsel_scheduler;
pub mod predictive_prefetcher;
pub mod sst_query_engine; // High-level query logic (formerly sst_query_engine) // TD-039: Morsel-driven parallelism

// Test modules
#[cfg(test)]
pub mod tests;

pub use sst_query_engine::{CollectionContext, UnifiedSstableReader};

pub use predictive_prefetcher::{PredictivePrefetcher, PrefetchConfig, PrefetchStats};

// TD-039: Morsel-driven parallelism exports
pub use morsel_scheduler::{MORSEL_SIZE, Morsel, MorselScheduler};

pub use block_filter::{
    BlockFilter, BlockFilterStrategy, IntelligentBlockFilter, MetadataFilter, QueryType,
};
