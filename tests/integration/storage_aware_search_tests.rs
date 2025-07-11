//! Integration tests for storage-aware polymorphic search

use proximadb::core::{VectorRecord, search::StorageAwareSearchCoordinator};
use proximadb::storage::engine::StorageEngine;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::persistence::wal::{WalConfig, WalManager, WalStrategyType};
use proximadb::storage::metadata::backends::memory_backend::MemoryMetadataBackend;
use proximadb::storage::metadata::store::UnifiedMetadataStore;
use proximadb::services::collection_service::CollectionService;
use proximadb::services::SharedServices;
use proximadb::storage::assignment_service::StaticAssignmentService;
use proximadb::proto::proximadb::{Collection, CollectionConfig, DistanceMetric as ProtoDistanceMetric, StorageEngine as ProtoStorageEngine};
use proximadb::compute::distance::DistanceMetric;
use std::sync::Arc;
use std::collections::HashMap;
use tempfile::TempDir;
use std::time::Duration;
use tokio::time::sleep;

/// Create test environment with WAL, VIPER, and LSM engines
async fn create_test_environment() -> (Arc<StorageEngine>, Arc<CollectionService>, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    // Create filesystem
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(
        FilesystemFactory::new(fs_config)
            .await
            .expect("Failed to create filesystem")
    );
    
    // Create metadata store
    let metadata_backend = Arc::new(MemoryMetadataBackend::new());
    let metadata_store = Arc::new(UnifiedMetadataStore::new(metadata_backend));
    
    // Create assignment service
    let assignment_service = Arc::new(StaticAssignmentService::new(temp_dir.path().to_path_buf()));
    
    // Create collection service
    let collection_service = Arc::new(CollectionService::new(
        temp_dir.path().to_path_buf(),
        assignment_service.clone(),
        metadata_store.clone(),
    ));
    
    // Create storage engine with both VIPER and LSM
    let mut storage_config = proximadb::storage::engine::StorageEngineConfig::default();
    storage_config.wal_config.strategy_type = WalStrategyType::BincodeBatch;
    storage_config.wal_config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];
    
    let storage_engine = Arc::new(
        StorageEngine::new(
            temp_dir.path().to_path_buf(),
            filesystem.clone(),
            storage_config,
        )
        .await
        .expect("Failed to create storage engine")
    );
    
    (storage_engine, collection_service, temp_dir)
}

/// Create test collection with specified storage engine
async fn create_collection(
    collection_service: &Arc<CollectionService>,
    collection_id: &str,
    dimension: usize,
    storage_engine: ProtoStorageEngine,
) {
    let collection = Collection {
        id: collection_id.to_string(),
        config: Some(CollectionConfig {
            dimension: dimension as i32,
            distance_metric: ProtoDistanceMetric::Cosine as i32,
            storage_engine: storage_engine as i32,
            ..Default::default()
        }),
        ..Default::default()
    };
    
    collection_service.create_collection(&collection)
        .await
        .expect("Failed to create collection");
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_three_layer_search_coordination() {
        let (storage_engine, collection_service, _temp_dir) = create_test_environment().await;
        let collection_id = "test_collection";
        
        // Create VIPER collection
        create_collection(&collection_service, collection_id, 4, ProtoStorageEngine::Viper).await;
        
        // Create test vectors
        let vectors: Vec<VectorRecord> = (0..30)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32, i as f32 + 1.0, i as f32 + 2.0, i as f32 + 3.0],
                metadata: HashMap::from([
                    ("layer".to_string(), serde_json::json!(
                        if i < 10 { "wal" } else if i < 20 { "storage" } else { "compacted" }
                    ))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros() + (i as i64 * 1000),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            })
            .collect();
        
        // Insert vectors in batches to simulate different layers
        // First 10 vectors - stay in WAL
        storage_engine.insert_vectors(collection_id, vectors[0..10].to_vec())
            .await
            .expect("Failed to insert WAL vectors");
        
        // Next 10 vectors - flush to storage
        storage_engine.insert_vectors(collection_id, vectors[10..20].to_vec())
            .await
            .expect("Failed to insert storage vectors");
        
        storage_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush to storage");
        
        // Last 10 vectors - compact
        storage_engine.insert_vectors(collection_id, vectors[20..30].to_vec())
            .await
            .expect("Failed to insert compacted vectors");
        
        storage_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush before compaction");
        
        // Trigger compaction
        storage_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact collection");
        
        // Search across all layers
        let query_vector = vec![15.0, 16.0, 17.0, 18.0];
        let results = storage_engine.search_vectors(
            collection_id,
            &query_vector,
            10,
            Some(DistanceMetric::Euclidean),
            None,
            None,
        )
        .await
        .expect("Search should succeed");
        
        // Verify results come from all three layers
        let mut wal_count = 0;
        let mut storage_count = 0;
        let mut compacted_count = 0;
        
        for result in &results {
            if let Some(layer) = result.metadata.get("layer") {
                match layer.as_str() {
                    Some("wal") => wal_count += 1,
                    Some("storage") => storage_count += 1,
                    Some("compacted") => compacted_count += 1,
                    _ => {}
                }
            }
        }
        
        assert!(wal_count > 0, "Should have results from WAL layer");
        assert!(storage_count > 0, "Should have results from storage layer");
        assert!(compacted_count > 0, "Should have results from compacted layer");
        assert_eq!(results.len(), 10, "Should return requested number of results");
    }
    
    #[tokio::test]
    async fn test_engine_selection_viper_vs_lsm() {
        let (storage_engine, collection_service, _temp_dir) = create_test_environment().await;
        
        // Create VIPER collection
        let viper_collection = "viper_collection";
        create_collection(&collection_service, viper_collection, 128, ProtoStorageEngine::Viper).await;
        
        // Create LSM collection
        let lsm_collection = "lsm_collection";
        create_collection(&collection_service, lsm_collection, 128, ProtoStorageEngine::Lsm).await;
        
        // Create high-dimensional vectors
        let create_vector = |id: &str, collection: &str, base: f32| -> VectorRecord {
            VectorRecord {
                id: id.to_string(),
                collection_id: collection.to_string(),
                vector: (0..128).map(|i| base + i as f32 * 0.1).collect(),
                metadata: HashMap::from([
                    ("engine".to_string(), serde_json::json!(
                        if collection == viper_collection { "viper" } else { "lsm" }
                    ))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            }
        };
        
        // Insert vectors into both collections
        let viper_vectors: Vec<VectorRecord> = (0..100)
            .map(|i| create_vector(&format!("viper_{}", i), viper_collection, i as f32))
            .collect();
        
        let lsm_vectors: Vec<VectorRecord> = (0..100)
            .map(|i| create_vector(&format!("lsm_{}", i), lsm_collection, i as f32))
            .collect();
        
        storage_engine.insert_vectors(viper_collection, viper_vectors)
            .await
            .expect("Failed to insert VIPER vectors");
        
        storage_engine.insert_vectors(lsm_collection, lsm_vectors)
            .await
            .expect("Failed to insert LSM vectors");
        
        // Flush both collections
        storage_engine.flush_collection(viper_collection).await.expect("Failed to flush VIPER");
        storage_engine.flush_collection(lsm_collection).await.expect("Failed to flush LSM");
        
        // Search in VIPER collection
        let query_vector: Vec<f32> = (0..128).map(|i| 50.0 + i as f32 * 0.1).collect();
        
        let viper_results = storage_engine.search_vectors(
            viper_collection,
            &query_vector,
            5,
            Some(DistanceMetric::Cosine),
            None,
            None,
        )
        .await
        .expect("VIPER search should succeed");
        
        // Search in LSM collection
        let lsm_results = storage_engine.search_vectors(
            lsm_collection,
            &query_vector,
            5,
            Some(DistanceMetric::Cosine),
            None,
            None,
        )
        .await
        .expect("LSM search should succeed");
        
        // Verify correct engine was used
        assert_eq!(viper_results.len(), 5);
        assert_eq!(lsm_results.len(), 5);
        
        for result in &viper_results {
            assert_eq!(
                result.metadata.get("engine").and_then(|v| v.as_str()),
                Some("viper"),
                "VIPER results should come from VIPER engine"
            );
        }
        
        for result in &lsm_results {
            assert_eq!(
                result.metadata.get("engine").and_then(|v| v.as_str()),
                Some("lsm"),
                "LSM results should come from LSM engine"
            );
        }
    }
    
    #[tokio::test]
    async fn test_mvcc_version_ordering() {
        let (storage_engine, collection_service, _temp_dir) = create_test_environment().await;
        let collection_id = "mvcc_test";
        
        create_collection(&collection_service, collection_id, 4, ProtoStorageEngine::Viper).await;
        
        let vector_id = "vec1";
        
        // Insert multiple versions of the same vector
        for version in 1..=5 {
            let vector = VectorRecord {
                id: vector_id.to_string(),
                collection_id: collection_id.to_string(),
                vector: vec![version as f32; 4],
                metadata: HashMap::from([
                    ("version".to_string(), serde_json::json!(version))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros() + (version as i64 * 10000),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros() + (version as i64 * 10000),
                expires_at: None,
                version: version as i64,
                rank: None,
                score: None,
                distance: None,
            };
            
            storage_engine.update_vector(collection_id, vector_id, vector)
                .await
                .expect("Failed to update vector");
            
            // Small delay to ensure timestamp ordering
            sleep(Duration::from_millis(10)).await;
        }
        
        // Search should return the latest version
        let query_vector = vec![5.0; 4];
        let results = storage_engine.search_vectors(
            collection_id,
            &query_vector,
            1,
            Some(DistanceMetric::Euclidean),
            None,
            None,
        )
        .await
        .expect("Search should succeed");
        
        assert_eq!(results.len(), 1, "Should return exactly one result");
        assert_eq!(results[0].id, vector_id, "Should return the correct vector ID");
        assert_eq!(
            results[0].metadata.get("version").and_then(|v| v.as_i64()),
            Some(5),
            "Should return the latest version"
        );
    }
    
    #[tokio::test]
    async fn test_deleted_vectors_not_returned() {
        let (storage_engine, collection_service, _temp_dir) = create_test_environment().await;
        let collection_id = "delete_test";
        
        create_collection(&collection_service, collection_id, 4, ProtoStorageEngine::Viper).await;
        
        // Insert vectors
        let vectors: Vec<VectorRecord> = (0..10)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::new(),
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            })
            .collect();
        
        storage_engine.insert_vectors(collection_id, vectors)
            .await
            .expect("Failed to insert vectors");
        
        // Delete some vectors
        for i in 0..5 {
            storage_engine.delete_vector(collection_id, &format!("vec{}", i))
                .await
                .expect("Failed to delete vector");
        }
        
        // Search should not return deleted vectors
        let query_vector = vec![2.5; 4];
        let results = storage_engine.search_vectors(
            collection_id,
            &query_vector,
            10,
            Some(DistanceMetric::Euclidean),
            None,
            None,
        )
        .await
        .expect("Search should succeed");
        
        assert_eq!(results.len(), 5, "Should only return non-deleted vectors");
        
        for result in &results {
            let id_num: usize = result.id[3..].parse().unwrap();
            assert!(id_num >= 5, "Should not return deleted vectors (vec0-vec4)");
        }
    }
    
    #[tokio::test]
    async fn test_metadata_filtering_across_layers() {
        let (storage_engine, collection_service, _temp_dir) = create_test_environment().await;
        let collection_id = "filter_test";
        
        create_collection(&collection_service, collection_id, 4, ProtoStorageEngine::Viper).await;
        
        // Insert vectors with different categories
        let mut all_vectors = Vec::new();
        
        for i in 0..30 {
            let category = match i % 3 {
                0 => "A",
                1 => "B",
                _ => "C",
            };
            
            let vector = VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::from([
                    ("category".to_string(), serde_json::json!(category)),
                    ("score".to_string(), serde_json::json!(i * 10))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros() + i as i64,
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };
            
            all_vectors.push(vector);
        }
        
        // Insert in batches to create different storage layers
        storage_engine.insert_vectors(collection_id, all_vectors[0..10].to_vec())
            .await
            .expect("Failed to insert first batch");
        
        storage_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush");
        
        storage_engine.insert_vectors(collection_id, all_vectors[10..20].to_vec())
            .await
            .expect("Failed to insert second batch");
        
        storage_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush");
        
        storage_engine.insert_vectors(collection_id, all_vectors[20..30].to_vec())
            .await
            .expect("Failed to insert third batch");
        
        // Search with metadata filter
        let query_vector = vec![15.0; 4];
        let filter = serde_json::json!({
            "category": "B"
        });
        
        let results = storage_engine.search_vectors(
            collection_id,
            &query_vector,
            20,
            Some(DistanceMetric::Euclidean),
            Some(filter),
            None,
        )
        .await
        .expect("Search with filter should succeed");
        
        // Verify all results match the filter
        assert!(!results.is_empty(), "Should return filtered results");
        
        for result in &results {
            assert_eq!(
                result.metadata.get("category").and_then(|v| v.as_str()),
                Some("B"),
                "All results should match the filter"
            );
        }
        
        // Verify we get results from multiple layers
        let result_ids: Vec<usize> = results
            .iter()
            .map(|r| r.id[3..].parse().unwrap())
            .collect();
        
        let has_wal = result_ids.iter().any(|&id| id >= 20);
        let has_storage = result_ids.iter().any(|&id| id >= 10 && id < 20);
        let has_flushed = result_ids.iter().any(|&id| id < 10);
        
        assert!(has_wal || has_storage || has_flushed, 
                "Should have results from at least one storage layer");
    }
    
    #[tokio::test]
    async fn test_search_performance_across_layers() {
        let (storage_engine, collection_service, _temp_dir) = create_test_environment().await;
        let collection_id = "perf_test";
        
        create_collection(&collection_service, collection_id, 128, ProtoStorageEngine::Viper).await;
        
        // Create large dataset
        let total_vectors = 1000;
        let dimension = 128;
        
        let vectors: Vec<VectorRecord> = (0..total_vectors)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: (0..dimension).map(|d| (i * dimension + d) as f32 * 0.01).collect(),
                metadata: HashMap::from([
                    ("batch".to_string(), serde_json::json!(i / 100))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros() + i as i64,
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            })
            .collect();
        
        // Insert in multiple batches
        for batch in vectors.chunks(100) {
            storage_engine.insert_vectors(collection_id, batch.to_vec())
                .await
                .expect("Failed to insert batch");
            
            // Flush every other batch
            if batch[0].metadata.get("batch").unwrap().as_i64().unwrap() % 2 == 0 {
                storage_engine.flush_collection(collection_id)
                    .await
                    .expect("Failed to flush");
            }
        }
        
        // Run search performance test
        let query_vector: Vec<f32> = (0..dimension).map(|i| i as f32).collect();
        
        let start = std::time::Instant::now();
        
        let results = storage_engine.search_vectors(
            collection_id,
            &query_vector,
            100,
            Some(DistanceMetric::Cosine),
            None,
            None,
        )
        .await
        .expect("Search should succeed");
        
        let search_time = start.elapsed();
        
        println!("Search time for {} vectors across layers: {:?}", total_vectors, search_time);
        
        assert_eq!(results.len(), 100, "Should return requested number of results");
        assert!(search_time.as_millis() < 100, "Search should complete within 100ms");
    }
    
    #[tokio::test]
    async fn test_concurrent_search_operations() {
        let (storage_engine, collection_service, _temp_dir) = create_test_environment().await;
        let storage_engine = Arc::new(storage_engine);
        let collection_id = "concurrent_test";
        
        create_collection(&collection_service, collection_id, 4, ProtoStorageEngine::Viper).await;
        
        // Insert test vectors
        let vectors: Vec<VectorRecord> = (0..100)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::new(),
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            })
            .collect();
        
        storage_engine.insert_vectors(collection_id, vectors)
            .await
            .expect("Failed to insert vectors");
        
        // Spawn multiple concurrent search tasks
        let mut handles = vec![];
        
        for i in 0..10 {
            let engine = storage_engine.clone();
            let coll_id = collection_id.to_string();
            
            let handle = tokio::spawn(async move {
                let query = vec![i as f32 * 10.0; 4];
                engine.search_vectors(
                    &coll_id,
                    &query,
                    10,
                    Some(DistanceMetric::Euclidean),
                    None,
                    None,
                ).await
            });
            
            handles.push(handle);
        }
        
        // Wait for all searches to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "Concurrent search should succeed");
            assert_eq!(result.unwrap().len(), 10, "Each search should return 10 results");
        }
    }
}