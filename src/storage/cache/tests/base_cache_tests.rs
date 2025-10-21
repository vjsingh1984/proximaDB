use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};

// CacheKey for String already implemented in vector_data.rs
// CacheKey for u64 already implemented in traits.rs

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
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default().unwrap();

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
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default().unwrap();

    let cache = BaseCacheImpl::<String, TestValue>::new(10);

    let key = "non_existent".to_string();
    let retrieved = cache.get_with_hooks(&key).await;
    assert!(retrieved.is_none());
}

#[tokio::test]
async fn test_invalidation() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default().unwrap();

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
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default().unwrap();

    let cache = BaseCacheImpl::<String, TestValue>::new(10);

    let small_value = TestValue {
        data: vec![1; 100], // Small value
    };

    let large_value = TestValue {
        data: vec![1; 2_000_000], // Large value (2MB)
    };

    let small_tier = cache.select_tier(&"small".to_string(), &small_value).await;
    let large_tier = cache.select_tier(&"large".to_string(), &large_value).await;

    assert_eq!(small_tier, crate::storage::cache::backend::CacheTier::L1);
    // Large value should go to L1 as well since we don't have L2/L3 configured
    assert_eq!(large_tier, crate::storage::cache::backend::CacheTier::L1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_metrics_recording() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default().unwrap();

    let cache = BaseCacheImpl::<String, TestValue>::new(10);

    let key1 = "key1".to_string();
    let key2 = "key2".to_string();
    let value = TestValue {
        data: vec![1, 2, 3],
    };

    // Put and get
    cache.put_with_hooks(key1.clone(), value.clone()).await;
    eprintln!("After put");

    let result1 = cache.get_with_hooks(&key1).await;
    eprintln!("After get1: got value = {}", result1.is_some());

    let result2 = cache.get_with_hooks(&key2).await;
    eprintln!("After get2: got value = {}", result2.is_some());

    // Wait for async metrics recording to complete (metrics are recorded via tokio::spawn)
    // With multi-thread runtime, spawned tasks run on other threads
    // Need enough time for all 3 spawned tasks to complete and acquire write locks
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    let snapshot = cache.metrics().get_snapshot().await;
    eprintln!(
        "Snapshot: total_operations={}, cache_hits={}, cache_misses={}",
        snapshot.total_operations, snapshot.cache_hits, snapshot.cache_misses
    );
    assert_eq!(snapshot.total_operations, 3, "Should have 1 put + 2 gets");
    assert_eq!(snapshot.cache_misses, 1);
    assert!(snapshot.cache_hits > 0);
}
