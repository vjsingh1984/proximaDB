//! Migration tests for transitioning from UnifiedCrossEngineCache to specialized caches

use std::sync::Arc;
use std::collections::HashMap;

use crate::storage::cache::{
    VectorDataCache, MetadataCache, UnifiedCacheAdapter,
    BaseCache, CacheMetrics,
};
use crate::storage::unified_cache::{
    UnifiedCrossEngineCache, UnifiedCacheConfig, CacheKey as OldCacheKey,
};
use crate::proto::proximadb::VectorRecord;

#[tokio::test]
async fn test_adapter_basic_operations() {
    // Initialize hardware capabilities for test
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Create legacy cache
    let config = UnifiedCacheConfig {
        l1_memory_mb: 128,
        l2_nvme_gb: 1,
        l3_network_enabled: false,
        cross_engine_sharing: true,
        promotion_threshold: 3,
        eviction_policy: crate::storage::unified_cache::EvictionPolicy::LRU,
    };
    
    let legacy_cache = Arc::new(UnifiedCrossEngineCache::new(config));
    
    // Create specialized caches
    let vector_cache = Arc::new(VectorDataCache::new(64 * 1024 * 1024)); // 64MB
    let metadata_cache = Arc::new(MetadataCache::new(32 * 1024 * 1024)); // 32MB
    
    // Create adapter
    let adapter = UnifiedCacheAdapter::new(
        legacy_cache.clone(),
        vector_cache.clone(),
        metadata_cache.clone(),
    );

    // Test basic put/get operations
    let key = "test_collection_sst_vec1";
    let vector = VectorRecord {
        id: Some("vec1".to_string()),
        vector: vec![1.0, 2.0, 3.0],
        metadata: HashMap::new(),
        collection_id: "test_collection".to_string(),
        version: 1,
        timestamp: 0,
    };

    // Put through adapter
    adapter.put(&key.to_string(), vector.clone()).await.unwrap();

    // Get through adapter
    let retrieved = adapter.get(&key.to_string()).await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, vector.id);

    // Verify data is in both caches during migration
    assert!(vector_cache.contains(&key.to_string()).await);
    
    // Test removal
    adapter.remove(&key.to_string()).await.unwrap();
    assert!(!adapter.contains(&key.to_string()).await);
}

#[tokio::test]
async fn test_migration_data_transfer() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Setup caches
    let config = UnifiedCacheConfig {
        l1_memory_mb: 128,
        l2_nvme_gb: 1,
        l3_network_enabled: false,
        cross_engine_sharing: true,
        promotion_threshold: 3,
        eviction_policy: crate::storage::unified_cache::EvictionPolicy::LRU,
    };
    
    let legacy_cache = Arc::new(UnifiedCrossEngineCache::new(config));
    let vector_cache = Arc::new(VectorDataCache::new(64 * 1024 * 1024));
    let metadata_cache = Arc::new(MetadataCache::new(32 * 1024 * 1024));
    
    // Pre-populate legacy cache with test data
    for i in 0..10 {
        let old_key = OldCacheKey {
            engine: "sst".to_string(),
            collection_id: "test_collection".to_string(),
            data_type: crate::storage::unified_cache::CacheDataType::Vector,
            item_id: format!("vec{}", i),
        };
        
        let vector = VectorRecord {
            id: Some(format!("vec{}", i)),
            vector: vec![i as f32; 3],
            metadata: HashMap::new(),
            collection_id: "test_collection".to_string(),
            version: 1,
            timestamp: 0,
        };
        
        legacy_cache.put_vector(&old_key, vector).await.unwrap();
    }

    // Create adapter and migrate
    let adapter = UnifiedCacheAdapter::new(
        legacy_cache.clone(),
        vector_cache.clone(),
        metadata_cache.clone(),
    );
    
    adapter.migrate_data().await.unwrap();

    // Verify all data migrated to specialized caches
    for i in 0..10 {
        let key = format!("test_collection_sst_vec{}", i);
        assert!(vector_cache.contains(&key).await);
        
        let vector = vector_cache.get(&key).await;
        assert!(vector.is_some());
        assert_eq!(vector.unwrap().id, Some(format!("vec{}", i)));
    }
}

#[tokio::test]
async fn test_migration_mode_transition() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let config = UnifiedCacheConfig {
        l1_memory_mb: 128,
        l2_nvme_gb: 1,
        l3_network_enabled: false,
        cross_engine_sharing: true,
        promotion_threshold: 3,
        eviction_policy: crate::storage::unified_cache::EvictionPolicy::LRU,
    };
    
    let legacy_cache = Arc::new(UnifiedCrossEngineCache::new(config));
    let vector_cache = Arc::new(VectorDataCache::new(64 * 1024 * 1024));
    let metadata_cache = Arc::new(MetadataCache::new(32 * 1024 * 1024));
    
    let mut adapter = UnifiedCacheAdapter::new(
        legacy_cache.clone(),
        vector_cache.clone(),
        metadata_cache.clone(),
    );

    // Add data during migration mode
    let key = "test_collection_sst_vec1";
    let vector = VectorRecord {
        id: Some("vec1".to_string()),
        vector: vec![1.0, 2.0, 3.0],
        metadata: HashMap::new(),
        collection_id: "test_collection".to_string(),
        version: 1,
        timestamp: 0,
    };

    adapter.put(&key.to_string(), vector.clone()).await.unwrap();
    
    // Complete migration
    adapter.complete_migration();

    // After migration, legacy cache should not be used
    // Add new data - should only go to specialized cache
    let key2 = "test_collection_sst_vec2";
    let vector2 = VectorRecord {
        id: Some("vec2".to_string()),
        vector: vec![4.0, 5.0, 6.0],
        metadata: HashMap::new(),
        collection_id: "test_collection".to_string(),
        version: 1,
        timestamp: 0,
    };

    adapter.put(&key2.to_string(), vector2.clone()).await.unwrap();
    
    // Verify vec2 is only in specialized cache
    assert!(vector_cache.contains(&key2.to_string()).await);
    
    // Legacy cache should not have vec2
    let old_key2 = OldCacheKey {
        engine: "sst".to_string(),
        collection_id: "test_collection".to_string(),
        data_type: crate::storage::unified_cache::CacheDataType::Vector,
        item_id: "vec2".to_string(),
    };
    assert!(!legacy_cache.contains(&old_key2).await);
}

#[tokio::test]
async fn test_cache_metrics_during_migration() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let config = UnifiedCacheConfig {
        l1_memory_mb: 128,
        l2_nvme_gb: 1,
        l3_network_enabled: false,
        cross_engine_sharing: true,
        promotion_threshold: 3,
        eviction_policy: crate::storage::unified_cache::EvictionPolicy::LRU,
    };
    
    let legacy_cache = Arc::new(UnifiedCrossEngineCache::new(config));
    let vector_cache = Arc::new(VectorDataCache::new(64 * 1024 * 1024));
    let metadata_cache = Arc::new(MetadataCache::new(32 * 1024 * 1024));
    
    let adapter = UnifiedCacheAdapter::new(
        legacy_cache,
        vector_cache.clone(),
        metadata_cache,
    );

    // Perform operations
    for i in 0..5 {
        let key = format!("test_collection_sst_vec{}", i);
        let vector = VectorRecord {
            id: Some(format!("vec{}", i)),
            vector: vec![i as f32; 3],
            metadata: HashMap::new(),
            collection_id: "test_collection".to_string(),
            version: 1,
            timestamp: 0,
        };
        
        adapter.put(&key, vector).await.unwrap();
    }

    // Check metrics
    let metrics = adapter.metrics();
    assert!(metrics.total_puts() >= 5);
    
    // Perform gets
    for i in 0..5 {
        let key = format!("test_collection_sst_vec{}", i);
        let _ = adapter.get(&key).await;
    }

    assert!(metrics.total_gets() >= 5);
    assert!(metrics.hit_rate() > 0.0);
}