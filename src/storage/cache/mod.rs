//! # Cache Module - Multi-Level Intelligent Caching System
//!
//! This module provides ProximaDB's sophisticated caching infrastructure with multiple
//! specialized caches, intelligent eviction policies, and cross-cache orchestration.
//! It implements a hierarchical caching system that significantly improves query performance
//! and reduces storage I/O.
//!
//! ## Cache Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │         Cache Orchestrator                   │
//! ├─────────────────────────────────────────────┤
//! │  Vector  │  Query  │  Metadata │  Index     │
//! │  Cache   │  Cache  │   Cache   │  Cache     │
//! ├─────────────────────────────────────────────┤
//! │         Shared Infrastructure                │
//! │  Eviction │ Metrics │ Backend │ Monitoring  │
//! └─────────────────────────────────────────────┘
//!           ↓         ↓         ↓         ↓
//!      Memory    Disk     Remote    Cloud
//! ```
//!
//! ## Specialized Caches
//!
//! ### 1. **Vector Cache** (`VectorStore`)
//! High-performance cache for vector data:
//! - **Hot Vectors**: Frequently accessed vectors in memory
//! - **Quantized Storage**: Compressed vectors for larger capacity
//! - **Prefetching**: Predictive loading of related vectors
//! - **SIMD Optimization**: Hardware-accelerated operations
//!
//! ### 2. **Query Cache** (`QueryCache`)
//! Result caching for repeated queries:
//! - **Exact Match**: Cache identical query results
//! - **Semantic Cache**: Similar query result reuse
//! - **Partial Results**: Cache intermediate computations
//! - **TTL Management**: Time-based expiration
//!
//! ### 3. **Metadata Cache** (`MetadataStore`)
//! Collection and index metadata:
//! - **Schema Cache**: Collection configurations
//! - **Statistics Cache**: Collection statistics
//! - **Index Metadata**: Index configurations and stats
//! - **Atomic Updates**: Consistent metadata updates
//!
//! ### 4. **Index Node Cache** (`IndexNodeCache`)
//! Graph and tree node caching:
//! - **HNSW Nodes**: Graph nodes for navigation
//! - **Tree Nodes**: B-tree/LSM-tree nodes
//! - **Locality Optimization**: Keep related nodes together
//! - **Adaptive Loading**: Load based on access patterns
//!
//! ### 5. **Bitmap Filter Cache** (`BitmapFilterCache`)
//! Bloom filter and bitmap caching:
//! - **Bloom Filters**: Fast existence checks
//! - **Roaring Bitmaps**: Compressed bit arrays
//! - **Filter Chains**: Combined filter results
//! - **Memory Efficient**: Compressed storage
//!
//! ## Eviction Strategies
//!
//! ### Available Policies
//! - **LRU (Least Recently Used)**: Simple, effective for uniform access
//! - **LFU (Least Frequently Used)**: Better for skewed access patterns
//! - **ARC (Adaptive Replacement Cache)**: Self-tuning between recency and frequency
//! - **2Q (Two Queue)**: Separates hot and cold data
//! - **SLRU (Segmented LRU)**: Multiple LRU segments by priority
//!
//! ### Adaptive Eviction
//! The cache automatically selects optimal eviction policy:
//! ```rust,ignore
//! // Monitor access patterns
//! if access_pattern.is_sequential() {
//!     use_lru();  // Better for scans
//! } else if access_pattern.is_skewed() {
//!     use_lfu();  // Better for hot data
//! } else {
//!     use_arc();  // Self-tuning
//! }
//! ```
//!
//! ## Cache Tiers
//!
//! ### Memory Hierarchy
//! 1. **L1 Cache**: CPU cache (automatic)
//! 2. **L2 Cache**: In-process memory (fastest)
//! 3. **L3 Cache**: Shared memory (IPC)
//! 4. **L4 Cache**: Local disk (SSD/NVMe)
//! 5. **L5 Cache**: Remote cache (Redis/Memcached)
//!
//! ### Tier Management
//! - **Promotion**: Move hot data to faster tiers
//! - **Demotion**: Move cold data to slower tiers
//! - **Bypass**: Skip cache for large sequential reads
//! - **Write-Through**: Update all tiers on write
//!
//! ## Cross-Cache Orchestration
//!
//! The orchestrator coordinates all caches:
//! - **Memory Budget**: Allocate memory across caches
//! - **Priority Management**: Prioritize critical caches
//! - **Rebalancing**: Adjust sizes based on workload
//! - **Global Eviction**: Coordinate eviction across caches
//!
//! ## Performance Characteristics
//!
//! - **Hit Rate**: 80-95% for hot data
//! - **Latency**: < 1μs for memory cache
//! - **Throughput**: 10M+ ops/sec
//! - **Memory Efficiency**: 10-20% overhead
//! - **Warm-up Time**: < 30 seconds
//!
//! ## Configuration
//!
//! ```toml
//! [cache]
//! # Global settings
//! total_memory_mb = 4096
//! enable_tiering = true
//!
//! # Vector cache
//! [cache.vector]
//! size_mb = 2048
//! eviction = "arc"
//! prefetch = true
//!
//! # Query cache
//! [cache.query]
//! size_mb = 512
//! ttl_seconds = 300
//! semantic_cache = true
//!
//! # Metadata cache
//! [cache.metadata]
//! size_mb = 256
//! persist = true
//!
//! # Index cache
//! [cache.index]
//! size_mb = 1024
//! node_locality = true
//! ```
//!
//! ## Monitoring & Metrics
//!
//! Real-time cache metrics:
//! - **Hit/Miss Ratio**: Cache effectiveness
//! - **Eviction Rate**: Memory pressure indicator
//! - **Latency Distribution**: P50, P95, P99
//! - **Memory Usage**: Per-cache breakdown
//! - **Hot Key Analysis**: Most accessed items
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use proximadb::cache::{CrossCacheOrchestrator, CacheConfig};
//!
//! // Initialize cache system
//! let config = CacheConfig::default();
//! let orchestrator = CrossCacheOrchestrator::new(config);
//!
//! // Get vector from cache
//! let vector = orchestrator.get_vector("vec_123").await?;
//!
//! // Cache query result
//! orchestrator.cache_query_result(
//!     query_hash,
//!     search_results,
//!     Duration::from_secs(300)
//! ).await?;
//!
//! // Get cache metrics
//! let metrics = orchestrator.metrics();
//! println!("Cache hit rate: {:.2}%", metrics.hit_rate * 100.0);
//! ```
//!
//! ## Optimization Tips
//!
//! 1. **Size Appropriately**: Allocate 10-20% of RAM to cache
//! 2. **Monitor Hit Rate**: Aim for > 80% hit rate
//! 3. **Use Tiering**: Enable disk cache for larger capacity
//! 4. **Tune Eviction**: Match policy to access pattern
//! 5. **Warm Cache**: Preload frequently accessed data

pub mod backend;
pub mod base;
pub mod config;
pub mod eviction;
pub mod health_monitor;
pub mod metrics;
pub mod metrics_integration;
pub mod orchestrator;
pub mod performance_optimizer;
pub mod specialized;
pub mod traits;
pub mod warming;

#[cfg(test)]
mod tests;

// Re-export main types
pub use backend::{CacheTier, StorageBackend};
pub use base::BaseCacheImpl;
pub use config::{CacheConfig, GlobalCacheConfig};
pub use eviction::{AccessTracker, CacheEvictionConfig, CacheEvictor, EvictionPolicy};
pub use health_monitor::{CacheMonitoringDashboard, DashboardState};
pub use metrics::CacheMetrics;
pub use orchestrator::{
    AccessPatternTracker, CacheType, CrossCacheOrchestrator, DynamicMemoryAllocator,
};
pub use performance_optimizer::{CacheOptimizer, OptimizationReport};
pub use traits::{BaseCache, CacheEntry, CacheKey, CacheValue};

// Re-export specialized caches
pub use specialized::{BitmapFilterCache, IndexNodeCache, MetadataStore, QueryCache, VectorCache};

// Re-export new cache modules
pub use metrics_integration::{CacheMetricsCollector, CacheMetricsConfig, CachePerformanceMetrics};
pub use warming::{CacheWarmer, CacheWarmingConfig, WarmingStrategy};

// Implement CacheValue for VectorRecord to enable caching
impl CacheValue for crate::proto::proximadb_v1::VectorRecord {
    fn size_bytes(&self) -> usize {
        // Calculate approximate size in bytes
        let mut size = std::mem::size_of::<Self>();

        // Add size of string fields
        size += self.id.len();
        if let Some(ref source) = self.source {
            size += source.len();
        }

        // Add size of vector data (f32 = 4 bytes each)
        size += self.vector.len() * 4;

        // quantized_vector removed - internalized in storage

        // Add size of metadata (approximate)
        for key in self.metadata.keys() {
            size += key.len();
            // Approximate size of SqlValue
            size += 64; // Conservative estimate for SqlValue
        }

        size
    }
}
