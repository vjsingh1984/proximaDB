// Cache module - Specialized caches with shared infrastructure

pub mod traits;
pub mod base;
pub mod eviction;
pub mod backend;
pub mod specialized;
pub mod orchestrator;
pub mod metrics;
pub mod config;
pub mod health_monitor;
pub mod performance_optimizer;

#[cfg(test)]
mod tests;

// Re-export main types
pub use traits::{BaseCache, CacheKey, CacheValue, CacheEntry};
pub use base::BaseCacheImpl;
pub use eviction::{EvictionStrategy, LRUStrategy, LFUStrategy, ARCStrategy};
pub use backend::{StorageBackend, CacheTier};
pub use orchestrator::{CrossCacheOrchestrator, CacheType, AccessPatternTracker, DynamicMemoryAllocator};
pub use metrics::CacheMetrics;
pub use config::{CacheConfig, GlobalCacheConfig, EvictionPolicy};
pub use health_monitor::{CacheMonitoringDashboard, DashboardState};
pub use performance_optimizer::{CacheOptimizer, OptimizationReport};

// Re-export specialized caches
pub use specialized::{
    VectorStore,
    QueryCache,
    BitmapFilterCache,
    IndexNodeCache,
    MetadataStore,
};

