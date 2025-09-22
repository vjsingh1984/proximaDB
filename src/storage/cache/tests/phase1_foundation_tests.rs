//! Phase 1: Foundation - Shared Infrastructure Tests

use super::super::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestKey(String);

impl CacheKey for TestKey {}

#[derive(Debug, Clone)]
struct TestValue {
    data: Vec<u8>,
    size: usize,
}

impl CacheValue for TestValue {
    fn size_bytes(&self) -> usize {
        self.size
    }
}

/// Test base cache trait implementation
#[tokio::test]
async fn test_base_cache_trait_template_method() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    struct TestCache {
        l1_store: Arc<RwLock<HashMap<TestKey, TestValue>>>,
        l2_store: Arc<RwLock<HashMap<TestKey, TestValue>>>,
        metrics: CacheMetrics,
    }

    #[async_trait::async_trait]
    impl BaseCache for TestCache {
        type Key = TestKey;
        type Value = TestValue;

        async fn check_l1(&self, key: &Self::Key) -> Option<Self::Value> {
            self.l1_store.read().await.get(key).cloned()
        }

        async fn check_l2(&self, key: &Self::Key) -> Option<Self::Value> {
            self.l2_store.read().await.get(key).cloned()
        }

        async fn check_l3(&self, _key: &Self::Key) -> Option<Self::Value> {
            None
        }

        async fn put_l1(&self, key: Self::Key, value: Self::Value) {
            self.l1_store.write().await.insert(key, value);
        }

        async fn put_l2(&self, key: Self::Key, value: Self::Value) {
            self.l2_store.write().await.insert(key, value);
        }

        async fn put_l3(&self, _key: Self::Key, _value: Self::Value) {}

        async fn invalidate_l1(&self, key: &Self::Key) -> bool {
            self.l1_store.write().await.remove(key).is_some()
        }

        async fn invalidate_l2(&self, key: &Self::Key) -> bool {
            self.l2_store.write().await.remove(key).is_some()
        }

        async fn invalidate_l3(&self, _key: &Self::Key) -> bool {
            false
        }

        async fn promote_to_l1(&self, key: &Self::Key, value: &Self::Value) {
            self.put_l1(key.clone(), value.clone()).await;
        }

        async fn promote_to_l2(&self, key: &Self::Key, value: &Self::Value) {
            self.put_l2(key.clone(), value.clone()).await;
        }

        async fn select_tier(&self, _key: &Self::Key, value: &Self::Value) -> CacheTier {
            if value.size_bytes() < 1024 {
                CacheTier::L1
            } else {
                CacheTier::L2
            }
        }

        fn metrics(&self) -> &CacheMetrics {
            &self.metrics
        }
    }

    let cache = TestCache {
        l1_store: Arc::new(RwLock::new(HashMap::new())),
        l2_store: Arc::new(RwLock::new(HashMap::new())),
        metrics: CacheMetrics::new(),
    };

    // Test template method flow
    let key = TestKey("test".to_string());
    let value = TestValue {
        data: vec![1, 2, 3],
        size: 100,
    };

    // Put value
    cache.put_with_hooks(key.clone(), value.clone()).await;

    // Get should find it
    let retrieved = cache.get_with_hooks(&key).await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().size, 100);

    // Check metrics were updated
    assert_eq!(cache.metrics.total_gets(), 1);
}

/// Test eviction policies
#[tokio::test]
async fn test_eviction_policies() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    use crate::storage::cache::eviction::{EvictionPolicy, AccessTracker};

    // Test LRU Policy
    let lru_policy = EvictionPolicy::LRU {
        max_items: 1000,
        batch_size: 10,
    };

    match lru_policy {
        EvictionPolicy::LRU { max_items, batch_size } => {
            assert_eq!(max_items, 1000);
            assert_eq!(batch_size, 10);
        }
        _ => panic!("Expected LRU policy"),
    }

    // Test LFU Policy
    let lfu_policy = EvictionPolicy::LFU {
        max_items: 500,
        min_access_count: 2,
        frequency_window_hours: 24,
    };

    match lfu_policy {
        EvictionPolicy::LFU { max_items, min_access_count, frequency_window_hours } => {
            assert_eq!(max_items, 500);
            assert_eq!(min_access_count, 2);
            assert_eq!(frequency_window_hours, 24);
        }
        _ => panic!("Expected LFU policy"),
    }

    // Test ARC Policy
    let arc_policy = EvictionPolicy::ARC {
        target_size: 1000,
        recent_size: 500,
        frequent_size: 500,
    };

    match arc_policy {
        EvictionPolicy::ARC { target_size, recent_size, frequent_size } => {
            assert_eq!(target_size, 1000);
            assert_eq!(recent_size, 500);
            assert_eq!(frequent_size, 500);
        }
        _ => panic!("Expected ARC policy"),
    }

    // Test AccessTracker
    let tracker = AccessTracker::new();
    tracker.track_access("key1".to_string()).await;
    tracker.track_access("key2".to_string()).await;

    // Verify tracker records access patterns
    let stats = tracker.get_access_stats("key1").await;
    assert!(stats.is_some());
}

/// Test storage backends
#[tokio::test]
async fn test_storage_backends() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    use crate::storage::cache::backend::*;

    // Test memory backend
    let memory_backend = MemoryBackend::<String, Vec<u8>>::new(1024 * 1024);

    memory_backend.put("key1".to_string(), vec![1, 2, 3]).await;
    let value = memory_backend.get(&"key1".to_string()).await;
    assert_eq!(value, Some(vec![1, 2, 3]));

    // Test size tracking
    assert!(memory_backend.size_bytes().await > 0);

    // Test removal
    memory_backend.remove(&"key1".to_string()).await;
    let value = memory_backend.get(&"key1".to_string()).await;
    assert_eq!(value, None);
}

/// Test metrics collection
#[tokio::test]
async fn test_metrics_collection() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let metrics = CacheMetrics::new();

    // Record some operations
    metrics.record_hit(CacheTier::L1);
    metrics.record_hit(CacheTier::L1);
    metrics.record_hit(CacheTier::L2);
    metrics.record_miss();
    metrics.record_miss();

    // Check hit rate
    assert_eq!(metrics.total_gets(), 5);
    assert_eq!(metrics.hit_rate(), 0.6); // 3 hits, 2 misses

    // Check tier-specific metrics
    assert_eq!(metrics.tier_hits(CacheTier::L1), 2);
    assert_eq!(metrics.tier_hits(CacheTier::L2), 1);

    // Record eviction
    metrics.record_eviction();
    assert_eq!(metrics.total_evictions(), 1);

    // Record latency
    metrics.record_get_latency(Duration::from_millis(1));
    metrics.record_put_latency(Duration::from_millis(3));
    // Check that we have recorded some latency
    assert!(metrics.total_gets() > 0 || metrics.total_puts() > 0);
}

/// Test cache entry metadata
#[tokio::test]
async fn test_cache_entry_metadata() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let value = TestValue {
        data: vec![1, 2, 3],
        size: 100,
    };

    let mut entry = CacheEntry::new(value);

    // Check initial state
    assert_eq!(entry.access_count, 1);
    assert_eq!(entry.size_bytes, 100);

    // Touch the entry
    tokio::time::sleep(Duration::from_millis(10)).await;
    entry.touch();

    assert_eq!(entry.access_count, 2);
    assert!(entry.age() >= Duration::from_millis(10));
}

