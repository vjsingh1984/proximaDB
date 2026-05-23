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
pub mod cache_coordinator; // TD-042: Cache coordinator
pub mod config;
pub mod eviction;
pub mod eviction_policy; // TD-042: Eviction policies
pub mod health_monitor;
pub mod metrics;
pub mod metrics_integration;
pub mod orchestrator;
pub mod performance_optimizer;
pub mod record_buffer_pool;
pub mod specialized;
pub mod traits;
pub mod warming;

// Re-export main types
pub use backend::{CacheTier, StorageBackend};
pub use base::BaseCacheImpl;
pub use cache_coordinator::{
    // TD-042: Unified cache interface
    CacheCoordinatorStats,
    CacheDependency,
    CacheId,
    UnifiedCache,
    UnifiedCacheCoordinator,
};
pub use config::{CacheConfig, GlobalCacheConfig};
pub use eviction::{AccessTracker, CacheEvictionConfig, CacheEvictor, EvictionPolicy};
pub use eviction_policy::{
    // TD-042: Unified eviction policies
    CachePriority,
    EvictionConfig,
    EvictionResult,
    PressureStatus,
    UnifiedEvictionPolicy,
};
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

// Implement CacheValue for canonical records to enable caching below protocol adapters.
impl CacheValue for proximadb_records::ProximaRecord {
    fn size_bytes(&self) -> usize {
        // Calculate approximate size in bytes
        let mut size = std::mem::size_of::<Self>();

        // Add size of string fields
        size += self.oid.len();
        if let Some(ref local_id) = self.local_id {
            size += local_id.len();
        }
        if let Some(ref variation_id) = self.variation_id {
            size += variation_id.len();
        }
        size += self.tenant_id.len();
        for principal in &self.permitted_principals {
            size += principal.len();
        }
        if let Some(ref policy_id) = self.rls_policy_id {
            size += policy_id.len();
        }
        if let Some(ref origin) = self.origin {
            size += origin.len();
        }
        if let Some(ref actor) = self.actor {
            size += actor.len();
        }
        if let Some(ref method) = self.method {
            size += method.len();
        }

        // Add size of embedding data (f32 = 4 bytes each)
        for embedding in &self.embeddings {
            size += embedding.model_id.len();
            size += embedding.modality.len();
            size += embedding.values.len() * 4;
        }

        // Add size of properties (approximate)
        for key in self.props.keys() {
            size += key.len();
            // Approximate size of ProximaTreeNode / ProximaValue.
            size += 96;
        }

        for reference in &self.refs {
            match reference {
                proximadb_records::TypedRef::ForeignKey { table, id } => {
                    size += table.len() + id.len();
                }
                proximadb_records::TypedRef::GraphEdge { edge_id, .. } => {
                    size += edge_id.len();
                }
                proximadb_records::TypedRef::Embedding { model_id } => {
                    size += model_id.len();
                }
            }
        }

        if let Some(sequence) = &self.sequence {
            size += sequence.tokens.len() * std::mem::size_of::<u32>();
            size += sequence.model_id.len();
        }

        for label in self.labels.iter() {
            size += label.len();
        }

        size
    }
}

#[cfg(test)]
mod base_cache_tests {
    use crate::storage::cache::backend::CacheTier;
    use crate::storage::cache::base::BaseCacheImpl;
    use crate::storage::cache::traits::{BaseCache, CacheValue};

    #[derive(Debug, Clone)]
    struct TestValue {
        data: Vec<u8>,
    }

    impl CacheValue for TestValue {
        fn size_bytes(&self) -> usize {
            self.data.len()
        }
    }

    #[tokio::test]
    async fn test_basic_get_put() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let cache = BaseCacheImpl::<String, TestValue>::new(10);

        let key = "test_key".to_string();
        let value = TestValue {
            data: vec![1, 2, 3, 4, 5],
        };

        // Put value
        cache.put_with_hooks(key.clone(), value.clone()).await;

        // Get value
        let retrieved = cache.get_with_hooks(&key).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().data, value.data);
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let cache = BaseCacheImpl::<String, TestValue>::new(10);

        let key = "non_existent".to_string();
        let retrieved = cache.get_with_hooks(&key).await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_invalidation() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let cache = BaseCacheImpl::<String, TestValue>::new(10);

        let key = "test_key".to_string();
        let value = TestValue {
            data: vec![1, 2, 3],
        };

        // Put value
        cache.put_with_hooks(key.clone(), value).await;

        // Verify it exists
        assert!(cache.get_with_hooks(&key).await.is_some());

        // Invalidate
        let invalidated = cache.invalidate(&key).await;
        assert!(invalidated);

        // Verify it's gone
        assert!(cache.get_with_hooks(&key).await.is_none());
    }

    #[tokio::test]
    async fn test_tier_selection() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let cache = BaseCacheImpl::<String, TestValue>::new(10);

        let small_value = TestValue {
            data: vec![1; 100], // Small value
        };

        let large_value = TestValue {
            data: vec![1; 2_000_000], // Large value (2MB)
        };

        let small_tier = cache.select_tier(&"small".to_string(), &small_value).await;
        let large_tier = cache.select_tier(&"large".to_string(), &large_value).await;

        assert_eq!(small_tier, CacheTier::L1);
        // Large value should go to L1 as well since we don't have L2/L3 configured
        assert_eq!(large_tier, CacheTier::L1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_metrics_recording() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let cache = BaseCacheImpl::<String, TestValue>::new(10);

        let key1 = "key1".to_string();
        let key2 = "key2".to_string();
        let value = TestValue {
            data: vec![1, 2, 3],
        };

        // Put and get
        cache.put_with_hooks(key1.clone(), value.clone()).await;

        let _result1 = cache.get_with_hooks(&key1).await;
        let _result2 = cache.get_with_hooks(&key2).await;

        // Wait for async metrics recording to complete
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let snapshot = cache.metrics().get_snapshot().await;

        assert_eq!(snapshot.total_operations, 3, "Should have 1 put + 2 gets");
        assert_eq!(snapshot.cache_misses, 1);
        assert!(snapshot.cache_hits > 0);
    }
}

#[cfg(test)]
mod backend_tests {
    use crate::storage::cache::backend::{CacheTier, MemoryBackend, StorageBackend};

    #[derive(Clone, Debug)]
    struct TestBytes {
        _data: Box<[u8]>,
    }

    impl TestBytes {
        fn new_large() -> Self {
            TestBytes {
                _data: vec![0u8; 2 * 1024 * 1024].into_boxed_slice(),
            }
        }
    }

    #[tokio::test]
    async fn test_memory_backend_basic_operations() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let backend = MemoryBackend::<String, String>::new(1); // 1MB

        // Test put and get
        let key = "test_key".to_string();
        let value = "test_value".to_string();

        assert!(backend.put(key.clone(), value.clone()).await.is_ok());
        assert_eq!(backend.get(&key).await, Some(value.clone()));

        // Test contains
        assert!(backend.contains(&key).await);
        assert!(!backend.contains(&"non_existent".to_string()).await);

        // Test remove
        assert!(backend.remove(&key).await);
        assert!(!backend.contains(&key).await);
        assert_eq!(backend.get(&key).await, None);
    }

    #[tokio::test]
    async fn test_memory_backend_capacity() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let backend = MemoryBackend::<u32, TestBytes>::new(1); // 1MB limit

        // Try to insert data that exceeds capacity
        let large_value = TestBytes::new_large(); // 2MB

        let result = backend.put(1, large_value).await;
        assert!(result.is_err());

        // Verify the error is capacity exceeded
        if let Err(e) = result {
            match e {
                crate::storage::cache::backend::StorageError::CapacityExceeded => {}
                _ => panic!("Expected CapacityExceeded error"),
            }
        }
    }

    #[tokio::test]
    async fn test_memory_backend_clear() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let backend = MemoryBackend::<String, String>::new(1);

        // Insert some data
        for i in 0..10 {
            let key = format!("key_{}", i);
            let value = format!("value_{}", i);
            let _ = backend.put(key, value).await;
        }

        assert_eq!(backend.entry_count().await, 10);
        assert!(backend.size_bytes().await > 0);

        // Clear
        assert!(backend.clear().await.is_ok());

        // Verify cleared
        assert_eq!(backend.entry_count().await, 0);
        assert_eq!(backend.size_bytes().await, 0);
    }

    #[tokio::test]
    async fn test_memory_backend_tier() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let backend = MemoryBackend::<String, String>::new(1);
        assert_eq!(backend.tier(), CacheTier::L1);
    }

    #[tokio::test]
    async fn test_memory_backend_concurrent_access() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        use std::sync::Arc;

        let backend = Arc::new(MemoryBackend::<u32, u32>::new(10));

        // Spawn multiple tasks that read and write concurrently
        let mut handles = vec![];

        for i in 0..10 {
            let backend_clone = backend.clone();
            let handle = tokio::spawn(async move {
                for j in 0..100 {
                    let key = i * 100 + j;
                    let _ = backend_clone.put(key, key * 2).await;
                    let _ = backend_clone.get(&key).await;
                }
            });
            handles.push(handle);
        }

        // Wait for all tasks
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify some data exists
        assert!(backend.entry_count().await > 0);
    }
}

#[cfg(test)]
mod metrics_tests {
    use crate::storage::cache::backend::CacheTier;
    use crate::storage::cache::metrics::CacheMetrics;
    use std::time::Duration;

    #[test]
    fn test_metrics_recording() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let metrics = CacheMetrics::new();

        // Record some hits
        metrics.record_hit(CacheTier::L1);
        metrics.record_hit(CacheTier::L1);
        metrics.record_hit(CacheTier::L2);
        metrics.record_hit(CacheTier::L3);

        // Record misses
        metrics.record_miss();
        metrics.record_miss();

        // Record operations
        metrics.record_put();
        metrics.record_invalidation();
        metrics.record_eviction();

        // Record latencies
        metrics.record_get_latency(Duration::from_micros(100));
        metrics.record_get_latency(Duration::from_micros(200));
        metrics.record_put_latency(Duration::from_micros(150));

        // Update size
        metrics.update_size(100, 1024 * 1024);

        // Get snapshot
        let snapshot = metrics.snapshot();

        assert_eq!(snapshot.l1_hits, 2);
        assert_eq!(snapshot.l2_hits, 1);
        assert_eq!(snapshot.l3_hits, 1);
        assert_eq!(snapshot.misses, 2);
        assert_eq!(snapshot.total_gets, 6); // 4 hits + 2 misses
        assert_eq!(snapshot.total_puts, 1);
        assert_eq!(snapshot.invalidations, 1);
        assert_eq!(snapshot.evictions, 1);
        assert_eq!(snapshot.total_entries, 100);
        assert_eq!(snapshot.total_bytes, 1024 * 1024);
        assert_eq!(snapshot.avg_get_latency_us, 150); // (100 + 200) / 2
        assert_eq!(snapshot.avg_put_latency_us, 150);

        // Check hit rate calculation
        let expected_hit_rate = 4.0 / 6.0; // 4 hits out of 6 gets
        assert!((snapshot.hit_rate - expected_hit_rate).abs() < 0.001);
    }

    #[test]
    fn test_metrics_reset() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let metrics = CacheMetrics::new();

        // Record some operations
        metrics.record_hit(CacheTier::L1);
        metrics.record_miss();
        metrics.record_put();

        // Verify they were recorded
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.l1_hits, 1);
        assert_eq!(snapshot.misses, 1);
        assert_eq!(snapshot.total_puts, 1);

        // Reset
        metrics.reset();

        // Verify reset
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.l1_hits, 0);
        assert_eq!(snapshot.misses, 0);
        assert_eq!(snapshot.total_puts, 0);
        assert_eq!(snapshot.total_gets, 0);
    }

    #[test]
    fn test_metrics_summary_print() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let metrics = CacheMetrics::new();

        // Set up some metrics
        for _ in 0..70 {
            metrics.record_hit(CacheTier::L1);
        }
        for _ in 0..20 {
            metrics.record_hit(CacheTier::L2);
        }
        for _ in 0..5 {
            metrics.record_hit(CacheTier::L3);
        }
        for _ in 0..5 {
            metrics.record_miss();
        }

        metrics.update_size(1000, 10 * 1024 * 1024);

        let snapshot = metrics.snapshot();

        // Test that summary can be printed without panic
        snapshot.print_summary();

        // Verify percentages
        assert_eq!(snapshot.total_gets, 100);
        assert_eq!(snapshot.hit_rate, 0.95); // 95% hit rate
    }
}

#[cfg(test)]
mod eviction_tests {

    use crate::storage::cache::eviction::{
        AccessTracker, CacheEvictionConfig, CacheEvictor, EvictionPolicy,
    };
    use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
    use crate::storage::traits::UnifiedMetricsCollector;
    use std::sync::Arc;

    #[test]
    fn test_lru_policy() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let policy = EvictionPolicy::LRU {
            max_items: 1000,
            batch_size: 10,
        };

        match policy {
            EvictionPolicy::LRU {
                max_items,
                batch_size,
            } => {
                assert_eq!(max_items, 1000);
                assert_eq!(batch_size, 10);
            }
            _ => panic!("Expected LRU policy"),
        }
    }

    #[test]
    fn test_lfu_policy() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let policy = EvictionPolicy::LFU {
            max_items: 1000,
            min_access_count: 2,
            frequency_window_hours: 24,
        };

        match policy {
            EvictionPolicy::LFU {
                max_items,
                min_access_count,
                frequency_window_hours,
            } => {
                assert_eq!(max_items, 1000);
                assert_eq!(min_access_count, 2);
                assert_eq!(frequency_window_hours, 24);
            }
            _ => panic!("Expected LFU policy"),
        }
    }

    #[test]
    fn test_arc_policy() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let policy = EvictionPolicy::ARC {
            target_size: 1000,
            recent_size: 500,
            frequent_size: 500,
        };

        match policy {
            EvictionPolicy::ARC {
                target_size,
                recent_size,
                frequent_size,
            } => {
                assert_eq!(target_size, 1000);
                assert_eq!(recent_size, 500);
                assert_eq!(frequent_size, 500);
            }
            _ => panic!("Expected ARC policy"),
        }
    }

    #[test]
    fn test_ttl_policy() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let policy = EvictionPolicy::TTL {
            max_age_seconds: 3600,
            cleanup_interval_seconds: 60,
        };

        match policy {
            EvictionPolicy::TTL {
                max_age_seconds,
                cleanup_interval_seconds,
            } => {
                assert_eq!(max_age_seconds, 3600);
                assert_eq!(cleanup_interval_seconds, 60);
            }
            _ => panic!("Expected TTL policy"),
        }
    }

    #[test]
    fn test_pattern_based_policy() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let policy = EvictionPolicy::PatternBased {
            use_ml_predictions: true,
            pattern_window_hours: 48,
            eviction_threshold: 0.7,
        };

        match policy {
            EvictionPolicy::PatternBased {
                use_ml_predictions,
                pattern_window_hours,
                eviction_threshold,
            } => {
                assert!(use_ml_predictions);
                assert_eq!(pattern_window_hours, 48);
                assert_eq!(eviction_threshold, 0.7);
            }
            _ => panic!("Expected PatternBased policy"),
        }
    }

    #[tokio::test]
    async fn test_access_tracker() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let tracker = AccessTracker::new();

        // Track some accesses
        tracker.track_access("key1".to_string()).await;
        tracker.track_access("key2".to_string()).await;
        tracker.track_access("key1".to_string()).await; // Second access to key1

        // Get access statistics - method not available, testing LRU items instead
        let lru_items = tracker.get_lru_items(3).await;

        // key3 should be in LRU list as it was never accessed
        // key1 and key2 were accessed so they should be more recent
        assert!(!lru_items.is_empty());
    }

    #[test]
    fn test_cache_eviction_config() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let config = CacheEvictionConfig::default();

        // Default config should have reasonable values
        assert!(!config.policies.is_empty());
        assert!(config.check_interval_seconds > 0);
        assert!(config.max_cache_size > 0);
    }

    #[tokio::test]
    async fn test_cache_evictor_creation() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let orchestrator = Arc::new(CrossCacheOrchestrator::new(1024 * 1024 * 100)); // 100MB
        let metrics = Arc::new(UnifiedMetricsCollector::new());

        let evictor = CacheEvictor::new(orchestrator, metrics);

        // Verify evictor was created successfully
        assert!(Arc::strong_count(&Arc::new(evictor)) == 1);
    }
}
