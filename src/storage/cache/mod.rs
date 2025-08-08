// Cache module - Specialized caches with shared infrastructure

pub mod traits;
pub mod base;
pub mod eviction;
pub mod backend;
pub mod specialized;
pub mod coordinator;
pub mod metrics;

#[cfg(test)]
mod tests;

// Re-export main types
pub use traits::{BaseCache, CacheKey, CacheValue};
pub use base::BaseCacheImpl;
pub use eviction::{EvictionStrategy, LRUStrategy, LFUStrategy, ARCStrategy};
pub use backend::{StorageBackend, CacheTier};
pub use coordinator::CacheCoordinator;
pub use metrics::CacheMetrics;

// Re-export specialized caches
pub use specialized::{
    VectorDataCache,
    QueryResultCache,
    FilterBitmapCache,
    IndexStructureCache,
    MetadataCache,
};