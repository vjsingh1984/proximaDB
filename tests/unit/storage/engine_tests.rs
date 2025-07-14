//! Comprehensive tests for storage engine module
//! Target: 80%+ coverage for storage engine implementation

use proximadb::storage::engine::{StorageEngine, StorageConfig};
use proximadb::storage::builder::{StorageBuilder, StorageBuilderError};
use proximadb::storage::traits::{UnifiedStorageEngine, FlushResult, CompactionResult};
use proximadb::core::{VectorRecord, Collection};
use proximadb::compute::distance::DistanceMetric;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;

async fn create_test_storage() -> (Arc<RwLock<StorageEngine>>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let mut config = StorageConfig::default();
    config.data_path = temp_dir.path().join("data").to_str().unwrap().to_string();
    config.rocksdb_path = temp_dir.path().join("rocksdb").to_str().unwrap().to_string();
    
    let engine = Arc::new(RwLock::new(
        StorageEngine::new_without_collection_service(config)
            .await
            .unwrap()
    ));
    
    (engine, temp_dir)
}

#[tokio::test]
async fn test_storage_engine_creation() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    let read_guard = engine.read().await;
    assert!(read_guard.is_initialized());
    
    // Check default configuration
    let config = read_guard.get_config();
    assert_eq!(config.engine_type, "unified");
    assert!(config.enable_compression);
}

#[tokio::test]
async fn test_collection_operations() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    // Create a collection
    let collection = Collection {
        id: "test_coll".to_string(),
        name: "Test Collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Cosine,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        config: Default::default(),
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.create_collection(&collection).await.unwrap();
    }
    
    // Verify collection exists
    {
        let read_guard = engine.read().await;
        let exists = read_guard.collection_exists("test_coll").await.unwrap();
        assert!(exists);
        
        // Get collection
        let retrieved = read_guard.get_collection("test_coll").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "test_coll");
    }
    
    // Delete collection
    {
        let mut write_guard = engine.write().await;
        let deleted = write_guard.delete_collection("test_coll").await.unwrap();
        assert!(deleted);
    }
    
    // Verify deletion
    {
        let read_guard = engine.read().await;
        let exists = read_guard.collection_exists("test_coll").await.unwrap();
        assert!(!exists);
    }
}

#[tokio::test]
async fn test_vector_operations() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    // Create collection first
    let collection = Collection {
        id: "vector_test".to_string(),
        name: "Vector Test".to_string(),
        dimension: 4,
        distance_metric: DistanceMetric::Euclidean,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        config: Default::default(),
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.create_collection(&collection).await.unwrap();
    }
    
    // Add vectors
    let vector1 = VectorRecord {
        id: Some("vec1".to_string()),
        collection_id: "vector_test".to_string(),
        vector: vec![1.0, 0.0, 0.0, 0.0],
        metadata: vec![
            ("key1".to_string(), "value1".to_string()),
            ("key2".to_string(), "value2".to_string()),
        ],
        timestamp: chrono::Utc::now().timestamp_micros(),
        created_at: chrono::Utc::now().timestamp_micros(),
        updated_at: chrono::Utc::now().timestamp_micros(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };
    
    let vector2 = VectorRecord {
        id: Some("vec2".to_string()),
        collection_id: "vector_test".to_string(),
        vector: vec![0.0, 1.0, 0.0, 0.0],
        metadata: vec![],
        timestamp: chrono::Utc::now().timestamp_micros(),
        created_at: chrono::Utc::now().timestamp_micros(),
        updated_at: chrono::Utc::now().timestamp_micros(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.add_vector(vector1.clone()).await.unwrap();
        write_guard.add_vector(vector2.clone()).await.unwrap();
    }
    
    // Get vector by ID
    {
        let read_guard = engine.read().await;
        let retrieved = read_guard.get_vector_by_id("vector_test", "vec1").await.unwrap();
        assert!(retrieved.is_some());
        let vec = retrieved.unwrap();
        assert_eq!(vec.id, Some("vec1".to_string()));
        assert_eq!(vec.vector, vec![1.0, 0.0, 0.0, 0.0]);
        assert_eq!(vec.metadata.len(), 2);
    }
    
    // Update vector
    let mut updated_vector = vector1.clone();
    updated_vector.vector = vec![2.0, 0.0, 0.0, 0.0];
    updated_vector.version = 2;
    
    {
        let mut write_guard = engine.write().await;
        write_guard.update_vector(updated_vector).await.unwrap();
    }
    
    // Verify update
    {
        let read_guard = engine.read().await;
        let retrieved = read_guard.get_vector_by_id("vector_test", "vec1").await.unwrap();
        assert_eq!(retrieved.unwrap().vector, vec![2.0, 0.0, 0.0, 0.0]);
    }
    
    // Delete vector
    {
        let mut write_guard = engine.write().await;
        let deleted = write_guard.delete_vector("vector_test", "vec1").await.unwrap();
        assert!(deleted);
    }
    
    // Verify deletion
    {
        let read_guard = engine.read().await;
        let retrieved = read_guard.get_vector_by_id("vector_test", "vec1").await.unwrap();
        assert!(retrieved.is_none());
    }
}

#[tokio::test]
async fn test_search_operations() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    // Setup collection and vectors
    let collection = Collection {
        id: "search_test".to_string(),
        name: "Search Test".to_string(),
        dimension: 3,
        distance_metric: DistanceMetric::Cosine,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        config: Default::default(),
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.create_collection(&collection).await.unwrap();
    }
    
    // Add test vectors
    let vectors = vec![
        ("search1", vec![1.0, 0.0, 0.0]),
        ("search2", vec![0.0, 1.0, 0.0]),
        ("search3", vec![0.0, 0.0, 1.0]),
        ("search4", vec![0.707, 0.707, 0.0]),
        ("search5", vec![0.577, 0.577, 0.577]),
    ];
    
    {
        let mut write_guard = engine.write().await;
        for (id, vector) in vectors {
            let record = VectorRecord {
                id: Some(id.to_string()),
                collection_id: "search_test".to_string(),
                vector,
                metadata: vec![],
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };
            write_guard.add_vector(record).await.unwrap();
        }
    }
    
    // Perform search
    {
        let read_guard = engine.read().await;
        let query = vec![1.0, 0.0, 0.0];
        let results = read_guard.search_vectors(
            "search_test",
            &query,
            5,
            None,
            DistanceMetric::Cosine,
        ).await.unwrap();
        
        assert!(!results.is_empty());
        assert!(results.len() <= 5);
        
        // First result should be the exact match
        assert_eq!(results[0].id, Some("search1".to_string()));
        
        // Results should be ordered by distance
        for window in results.windows(2) {
            assert!(window[0].distance.unwrap() <= window[1].distance.unwrap());
        }
    }
}

#[tokio::test]
async fn test_flush_operations() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    // Create collection
    let collection = Collection {
        id: "flush_test".to_string(),
        name: "Flush Test".to_string(),
        dimension: 2,
        distance_metric: DistanceMetric::Euclidean,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        config: Default::default(),
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.create_collection(&collection).await.unwrap();
    }
    
    // Add vectors
    {
        let mut write_guard = engine.write().await;
        for i in 0..10 {
            let record = VectorRecord {
                id: Some(format!("flush_vec_{}", i)),
                collection_id: "flush_test".to_string(),
                vector: vec![i as f32, 0.0],
                metadata: vec![],
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };
            write_guard.add_vector(record).await.unwrap();
        }
    }
    
    // Perform flush
    {
        let mut write_guard = engine.write().await;
        let result = write_guard.flush(Some("flush_test")).await.unwrap();
        
        match result {
            FlushResult::Success { vectors_flushed, bytes_written, .. } => {
                assert!(vectors_flushed > 0);
                assert!(bytes_written > 0);
            }
            _ => panic!("Flush should succeed"),
        }
    }
}

#[tokio::test]
async fn test_statistics() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    // Get initial stats
    {
        let read_guard = engine.read().await;
        let stats = read_guard.get_statistics().await.unwrap();
        assert_eq!(stats.total_collections, 0);
        assert_eq!(stats.total_vectors, 0);
    }
    
    // Add collection and vectors
    let collection = Collection {
        id: "stats_test".to_string(),
        name: "Stats Test".to_string(),
        dimension: 2,
        distance_metric: DistanceMetric::Euclidean,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        config: Default::default(),
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.create_collection(&collection).await.unwrap();
        
        for i in 0..5 {
            let record = VectorRecord {
                id: Some(format!("stats_vec_{}", i)),
                collection_id: "stats_test".to_string(),
                vector: vec![i as f32, 0.0],
                metadata: vec![],
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };
            write_guard.add_vector(record).await.unwrap();
        }
    }
    
    // Check updated stats
    {
        let read_guard = engine.read().await;
        let stats = read_guard.get_statistics().await.unwrap();
        assert_eq!(stats.total_collections, 1);
        assert_eq!(stats.total_vectors, 5);
        assert!(stats.total_size_bytes > 0);
    }
}

#[tokio::test]
async fn test_error_handling() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    // Try to get non-existent collection
    {
        let read_guard = engine.read().await;
        let result = read_guard.get_collection("non_existent").await.unwrap();
        assert!(result.is_none());
    }
    
    // Try to add vector to non-existent collection
    {
        let mut write_guard = engine.write().await;
        let record = VectorRecord {
            id: Some("test".to_string()),
            collection_id: "non_existent".to_string(),
            vector: vec![1.0, 0.0],
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp_micros(),
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        let result = write_guard.add_vector(record).await;
        assert!(result.is_err());
    }
    
    // Try to create duplicate collection
    let collection = Collection {
        id: "dup_test".to_string(),
        name: "Duplicate Test".to_string(),
        dimension: 2,
        distance_metric: DistanceMetric::Euclidean,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        config: Default::default(),
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.create_collection(&collection).await.unwrap();
        
        // Try to create again
        let result = write_guard.create_collection(&collection).await;
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn test_storage_builder() {
    let temp_dir = TempDir::new().unwrap();
    
    let builder = StorageBuilder::new()
        .with_data_dir(temp_dir.path().join("data").to_str().unwrap())
        .with_rocksdb_path(temp_dir.path().join("rocksdb").to_str().unwrap())
        .with_cache_dir(temp_dir.path().join("cache").to_str().unwrap())
        .with_max_memory_usage(1024 * 1024 * 1024) // 1GB
        .with_compression(true)
        .with_compression_level(3);
    
    let engine = builder.build().await.unwrap();
    
    // Verify configuration was applied
    let config = engine.get_config();
    assert!(config.enable_compression);
    assert_eq!(config.compression_level, 3);
    assert_eq!(config.max_memory_usage, 1024 * 1024 * 1024);
}

#[tokio::test]
async fn test_batch_operations() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    // Create collection
    let collection = Collection {
        id: "batch_test".to_string(),
        name: "Batch Test".to_string(),
        dimension: 2,
        distance_metric: DistanceMetric::Euclidean,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        config: Default::default(),
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.create_collection(&collection).await.unwrap();
    }
    
    // Batch insert vectors
    let mut vectors = Vec::new();
    for i in 0..100 {
        vectors.push(VectorRecord {
            id: Some(format!("batch_{}", i)),
            collection_id: "batch_test".to_string(),
            vector: vec![i as f32, 0.0],
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp_micros(),
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        });
    }
    
    {
        let mut write_guard = engine.write().await;
        write_guard.add_vectors_batch(vectors).await.unwrap();
    }
    
    // Verify all vectors were added
    {
        let read_guard = engine.read().await;
        let stats = read_guard.get_collection_stats("batch_test").await.unwrap();
        assert_eq!(stats.vector_count, 100);
    }
}

#[tokio::test]
async fn test_concurrent_access() {
    let (engine, _temp_dir) = create_test_storage().await;
    
    // Create collection
    let collection = Collection {
        id: "concurrent_test".to_string(),
        name: "Concurrent Test".to_string(),
        dimension: 2,
        distance_metric: DistanceMetric::Euclidean,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        config: Default::default(),
    };
    
    {
        let mut write_guard = engine.write().await;
        write_guard.create_collection(&collection).await.unwrap();
    }
    
    // Spawn multiple tasks for concurrent operations
    let mut handles = vec![];
    
    for i in 0..10 {
        let engine_clone = engine.clone();
        let handle = tokio::spawn(async move {
            let record = VectorRecord {
                id: Some(format!("concurrent_{}", i)),
                collection_id: "concurrent_test".to_string(),
                vector: vec![i as f32, 0.0],
                metadata: vec![],
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };
            
            let mut write_guard = engine_clone.write().await;
            write_guard.add_vector(record).await
        });
        handles.push(handle);
    }
    
    // Wait for all operations to complete
    for handle in handles {
        handle.await.unwrap().unwrap();
    }
    
    // Verify all vectors were added
    {
        let read_guard = engine.read().await;
        let stats = read_guard.get_collection_stats("concurrent_test").await.unwrap();
        assert_eq!(stats.vector_count, 10);
    }
}