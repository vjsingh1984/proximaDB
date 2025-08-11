use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};

// CacheKey for String already implemented in vector_data.rs
impl CacheKey for u64 {}

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
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

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
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let cache = BaseCacheImpl::<String, TestValue>::new(10);
    
    let key = "non_existent".to_string();
    let retrieved = cache.get_with_hooks(&key).await;
    assert!(retrieved.is_none());
}

#[tokio::test]
async fn test_invalidation() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

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
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

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

#[tokio::test]
async fn test_metrics_recording() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let cache = BaseCacheImpl::<String, TestValue>::new(10);
    
    let key1 = "key1".to_string();
    let key2 = "key2".to_string();
    let value = TestValue {
        data: vec![1, 2, 3],
    };
    
    // Put and get
    cache.put_with_hooks(key1.clone(), value.clone()).await;
    let _ = cache.get_with_hooks(&key1).await; // Hit
    let _ = cache.get_with_hooks(&key2).await; // Miss
    
    let metrics = cache.metrics().snapshot();
    assert_eq!(metrics.total_gets, 2);
    assert_eq!(metrics.misses, 1);
    assert!(metrics.l1_hits > 0);
}