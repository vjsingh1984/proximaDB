//! Unit tests for LSM engine flush and compaction operations

use proximadb::storage::engines::lsm::{LsmEngine, LsmConfig, LsmRecord};
use proximadb::storage::engines::lsm::compaction::{CompactionConfig, CompactionStrategy};
use proximadb::core::VectorRecord;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::metadata::backends::memory_backend::MemoryMetadataBackend;
use proximadb::storage::metadata::store::AtomicMetadataStore;
use proximadb::proto::proximadb::{Collection, CollectionConfig as ProtoCollectionConfig, DistanceMetric, StorageEngine};
use proximadb::network::multi_server::SharedServices;
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::assignment_service::AssignmentService;
use std::sync::Arc;
use std::collections::HashMap;
use tempfile::TempDir;
use std::time::Duration;
use tokio::time::sleep;

/// Create test LSM engine with custom config
async fn create_test_lsm_engine(
    compaction_config: Option<CompactionConfig>,
) -> (LsmEngine, Arc<SharedServices>, TempDir) {
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
    let metadata_store = Arc::new(AtomicMetadataStore::new(metadata_backend));
    
    // Create assignment service
    let assignment_service = Arc::new(AssignmentService::new(temp_dir.path().to_path_buf()));
    
    // Create collection service
    let collection_service = Arc::new(CollectionService::new(
        temp_dir.path().to_path_buf(),
        assignment_service.clone(),
        metadata_store.clone(),
    ));
    
    // Create shared services
    let shared_services = Arc::new(SharedServices {
        collection_service: collection_service.clone(),
        assignment_service,
        metadata_store: metadata_store.clone(),
    });
    
    // Create LSM config
    let mut lsm_config = LsmConfig::default();
    lsm_config.data_directory = temp_dir.path().to_string_lossy().to_string();
    
    if let Some(comp_config) = compaction_config {
        lsm_config.compaction = comp_config;
    } else {
        // Set aggressive compaction for testing
        lsm_config.compaction = CompactionConfig {
            strategy: CompactionStrategy::Leveled,
            level_multiplier: 10,
            min_files_to_compact: 2,
            max_files_per_level: 10,
            background_threads: 2,
        };
    }
    
    // Create LSM engine
    let lsm_engine = LsmEngine::new(
        lsm_config,
        filesystem,
        shared_services.clone(),
    )
    .await
    .expect("Failed to create LSM engine");
    
    (lsm_engine, shared_services, temp_dir)
}

/// Create test collection
async fn create_test_collection(
    shared_services: &Arc<SharedServices>,
    collection_id: &str,
    dimension: usize,
) {
    let collection = Collection {
        id: collection_id.to_string(),
        config: Some(ProtoCollectionConfig {
            dimension: dimension as i32,
            distance_metric: DistanceMetric::Euclidean as i32,
            storage_engine: StorageEngine::Lsm as i32,
            ..Default::default()
        }),
        ..Default::default()
    };
    
    shared_services.collection_service.create_collection(&collection)
        .await
        .expect("Failed to create collection");
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb::storage::traits::VectorStorage;
    
    #[tokio::test]
    async fn test_lsm_basic_flush() {
        let (lsm_engine, shared_services, _temp_dir) = create_test_lsm_engine(None).await;
        let collection_id = "test_collection";
        
        // Create collection
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Insert vectors into memtable
        let vectors: Vec<VectorRecord> = (0..10)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::from([
                    ("index".to_string(), serde_json::json!(i))
                ]),
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
        
        // Insert vectors
        for vector in &vectors {
            lsm_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        // Get stats before flush
        let stats_before = lsm_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        assert_eq!(stats_before.memtable_size, 10, "Should have 10 vectors in memtable");
        
        // Flush to SST
        let flush_result = lsm_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush collection");
        
        assert!(flush_result.success, "Flush should succeed");
        assert!(flush_result.vectors_flushed > 0, "Should flush some vectors");
        
        // Get stats after flush
        let stats_after = lsm_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        
        assert_eq!(stats_after.memtable_size, 0, "Memtable should be empty after flush");
        assert_eq!(stats_after.total_sst_files, 1, "Should have one SST file");
        assert_eq!(stats_after.total_vectors, 10, "Total vectors should remain the same");
    }
    
    #[tokio::test]
    async fn test_lsm_flush_with_tombstones() {
        let (lsm_engine, shared_services, _temp_dir) = create_test_lsm_engine(None).await;
        let collection_id = "tombstone_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
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
        
        for vector in &vectors {
            lsm_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        // Delete some vectors (creates tombstones)
        for i in 0..5 {
            lsm_engine.delete_vector(collection_id, &format!("vec{}", i))
                .await
                .expect("Failed to delete vector");
        }
        
        // Flush with tombstones
        let flush_result = lsm_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush");
        
        assert!(flush_result.success, "Flush with tombstones should succeed");
        
        // Verify deleted vectors are marked as tombstones
        let stats = lsm_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        
        // The count includes tombstones
        assert_eq!(stats.total_vectors, 5, "Should have 5 active vectors");
        
        // Verify deleted vectors cannot be retrieved
        for i in 0..5 {
            let result = lsm_engine.get_vector(collection_id, &format!("vec{}", i)).await;
            assert!(result.is_ok());
            assert!(result.unwrap().is_none(), "Deleted vector {} should not exist", i);
        }
    }
    
    #[tokio::test]
    async fn test_lsm_leveled_compaction() {
        let mut compaction_config = CompactionConfig::default();
        compaction_config.strategy = CompactionStrategy::Leveled;
        compaction_config.min_files_to_compact = 2;
        compaction_config.level_multiplier = 5;
        
        let (lsm_engine, shared_services, _temp_dir) = 
            create_test_lsm_engine(Some(compaction_config)).await;
        let collection_id = "leveled_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Create multiple SST files by flushing batches
        for batch in 0..4 {
            let vectors: Vec<VectorRecord> = (0..25)
                .map(|i| VectorRecord {
                    id: format!("batch{}_vec{}", batch, i),
                    collection_id: collection_id.to_string(),
                    vector: vec![batch as f32 * 100.0 + i as f32; 4],
                    metadata: HashMap::from([
                        ("batch".to_string(), serde_json::json!(batch))
                    ]),
                    timestamp: chrono::Utc::now().timestamp_micros() + (batch * 1000 + i) as i64,
                    created_at: chrono::Utc::now().timestamp_micros(),
                    updated_at: chrono::Utc::now().timestamp_micros(),
                    expires_at: None,
                    version: 1,
                    rank: None,
                    score: None,
                    distance: None,
                })
                .collect();
            
            for vector in &vectors {
                lsm_engine.insert_vector(collection_id, &vector.id, vector.clone())
                    .await
                    .expect("Failed to insert vector");
            }
            
            // Flush each batch
            lsm_engine.flush_collection(collection_id)
                .await
                .expect("Failed to flush batch");
        }
        
        // Check file count before compaction
        let stats_before = lsm_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        assert_eq!(stats_before.total_sst_files, 4, "Should have 4 SST files");
        
        // Trigger compaction
        let compaction_result = lsm_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact collection");
        
        assert!(compaction_result.success, "Compaction should succeed");
        assert!(compaction_result.files_compacted > 0, "Should compact some files");
        
        // Check file organization after compaction
        let stats_after = lsm_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        
        assert!(
            stats_after.total_sst_files <= stats_before.total_sst_files,
            "Should have same or fewer files after compaction"
        );
        assert_eq!(stats_after.total_vectors, 100, "Total vectors should remain the same");
        
        // Verify leveled structure
        assert!(
            compaction_result.compaction_type.contains("leveled"),
            "Should use leveled compaction"
        );
    }
    
    #[tokio::test]
    async fn test_lsm_size_tiered_compaction() {
        let mut compaction_config = CompactionConfig::default();
        compaction_config.strategy = CompactionStrategy::SizeTiered;
        compaction_config.min_files_to_compact = 3;
        
        let (lsm_engine, shared_services, _temp_dir) = 
            create_test_lsm_engine(Some(compaction_config)).await;
        let collection_id = "size_tiered_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Create files of different sizes
        let batch_sizes = vec![10, 20, 30, 40];
        
        for (batch, size) in batch_sizes.iter().enumerate() {
            let vectors: Vec<VectorRecord> = (0..*size)
                .map(|i| VectorRecord {
                    id: format!("batch{}_vec{}", batch, i),
                    collection_id: collection_id.to_string(),
                    vector: vec![i as f32; 4],
                    metadata: HashMap::from([
                        ("batch".to_string(), serde_json::json!(batch)),
                        ("size".to_string(), serde_json::json!(*size))
                    ]),
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
            
            for vector in &vectors {
                lsm_engine.insert_vector(collection_id, &vector.id, vector.clone())
                    .await
                    .expect("Failed to insert vector");
            }
            
            lsm_engine.flush_collection(collection_id)
                .await
                .expect("Failed to flush");
        }
        
        // Trigger size-tiered compaction
        let compaction_result = lsm_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact");
        
        assert!(compaction_result.success, "Size-tiered compaction should succeed");
        assert!(
            compaction_result.compaction_type.contains("size-tiered"),
            "Should use size-tiered compaction"
        );
        
        // Verify all vectors are still accessible
        let total_vectors: usize = batch_sizes.iter().sum();
        let stats = lsm_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        
        assert_eq!(stats.total_vectors, total_vectors, "All vectors should be preserved");
    }
    
    #[tokio::test]
    async fn test_lsm_compaction_with_expired_records() {
        let (lsm_engine, shared_services, _temp_dir) = create_test_lsm_engine(None).await;
        let collection_id = "expired_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        let current_time = chrono::Utc::now().timestamp_micros();
        let expired_time = current_time - (24 * 60 * 60 * 1_000_000); // 24 hours ago
        
        // Insert mix of active and expired vectors
        let mut vectors = Vec::new();
        
        // Active vectors
        for i in 0..20 {
            vectors.push(VectorRecord {
                id: format!("active_{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::new(),
                timestamp: current_time,
                created_at: current_time,
                updated_at: current_time,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            });
        }
        
        // Expired vectors
        for i in 0..10 {
            vectors.push(VectorRecord {
                id: format!("expired_{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::new(),
                timestamp: expired_time,
                created_at: expired_time,
                updated_at: expired_time,
                expires_at: Some(expired_time),
                version: 1,
                rank: None,
                score: None,
                distance: None,
            });
        }
        
        // Insert all vectors
        for vector in &vectors {
            lsm_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        // Flush to create SST file
        lsm_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush");
        
        // Compact to remove expired records
        let compaction_result = lsm_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact");
        
        assert!(compaction_result.success, "Compaction should succeed");
        assert_eq!(
            compaction_result.expired_records_deleted, 10,
            "Should delete 10 expired records"
        );
        
        // Verify only active records remain
        let stats = lsm_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        
        assert_eq!(stats.total_vectors, 20, "Only active vectors should remain");
    }
    
    #[tokio::test]
    async fn test_lsm_bloom_filter_effectiveness() {
        let (lsm_engine, shared_services, _temp_dir) = create_test_lsm_engine(None).await;
        let collection_id = "bloom_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Insert vectors
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
        
        for vector in &vectors {
            lsm_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        // Flush to create SST with bloom filter
        lsm_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush");
        
        // Test bloom filter with existing keys
        for i in 0..10 {
            let result = lsm_engine.get_vector(collection_id, &format!("vec{}", i)).await;
            assert!(result.is_ok());
            assert!(result.unwrap().is_some(), "Should find existing vector");
        }
        
        // Test bloom filter with non-existent keys
        // Bloom filter should prevent unnecessary disk reads
        for i in 1000..1010 {
            let result = lsm_engine.get_vector(collection_id, &format!("vec{}", i)).await;
            assert!(result.is_ok());
            assert!(result.unwrap().is_none(), "Should not find non-existent vector");
        }
    }
    
    #[tokio::test]
    async fn test_lsm_concurrent_flush_operations() {
        let (lsm_engine, shared_services, _temp_dir) = create_test_lsm_engine(None).await;
        let lsm_engine = Arc::new(lsm_engine);
        
        // Create multiple collections
        for i in 0..3 {
            let collection_id = format!("collection_{}", i);
            create_test_collection(&shared_services, &collection_id, 4).await;
        }
        
        // Insert vectors to all collections
        for i in 0..3 {
            let collection_id = format!("collection_{}", i);
            let vectors: Vec<VectorRecord> = (0..50)
                .map(|j| VectorRecord {
                    id: format!("vec{}", j),
                    collection_id: collection_id.clone(),
                    vector: vec![i as f32 * 100.0 + j as f32; 4],
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
            
            for vector in &vectors {
                lsm_engine.insert_vector(&collection_id, &vector.id, vector.clone())
                    .await
                    .expect("Failed to insert vector");
            }
        }
        
        // Flush all collections concurrently
        let mut handles = vec![];
        
        for i in 0..3 {
            let engine = lsm_engine.clone();
            let collection_id = format!("collection_{}", i);
            
            let handle = tokio::spawn(async move {
                engine.flush_collection(&collection_id).await
            });
            
            handles.push(handle);
        }
        
        // Wait for all flushes to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "Concurrent flush should succeed");
            assert!(result.unwrap().success, "Flush should be successful");
        }
        
        // Verify all collections were flushed
        for i in 0..3 {
            let collection_id = format!("collection_{}", i);
            let stats = lsm_engine.get_collection_stats(&collection_id)
                .await
                .expect("Failed to get stats");
            
            assert_eq!(stats.memtable_size, 0, "Memtable should be empty");
            assert_eq!(stats.total_vectors, 50, "Should have all vectors");
            assert!(stats.total_sst_files > 0, "Should have SST files");
        }
    }
    
    #[tokio::test]
    async fn test_lsm_recovery_after_crash() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let collection_id = "recovery_test";
        
        // Phase 1: Create engine, insert data, and flush
        {
            let (lsm_engine, shared_services, _) = create_test_lsm_engine(None).await;
            create_test_collection(&shared_services, collection_id, 4).await;
            
            // Insert vectors
            let vectors: Vec<VectorRecord> = (0..20)
                .map(|i| VectorRecord {
                    id: format!("vec{}", i),
                    collection_id: collection_id.to_string(),
                    vector: vec![i as f32; 4],
                    metadata: HashMap::from([
                        ("persistent".to_string(), serde_json::json!(true))
                    ]),
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
            
            for vector in &vectors {
                lsm_engine.insert_vector(collection_id, &vector.id, vector.clone())
                    .await
                    .expect("Failed to insert vector");
            }
            
            // Flush to persist data
            lsm_engine.flush_collection(collection_id)
                .await
                .expect("Failed to flush");
            
            // Engine goes out of scope, simulating crash
        }
        
        // Phase 2: Create new engine and verify data recovery
        {
            let (lsm_engine, _, _) = create_test_lsm_engine(None).await;
            
            // Verify all vectors are recovered
            for i in 0..20 {
                let result = lsm_engine.get_vector(collection_id, &format!("vec{}", i)).await;
                assert!(result.is_ok());
                let vector = result.unwrap();
                assert!(vector.is_some(), "Vector {} should be recovered", i);
                
                let recovered = vector.unwrap();
                assert_eq!(
                    recovered.metadata.get("persistent").and_then(|v| v.as_bool()),
                    Some(true),
                    "Metadata should be preserved"
                );
            }
            
            let stats = lsm_engine.get_collection_stats(collection_id)
                .await
                .expect("Failed to get stats");
            
            assert_eq!(stats.total_vectors, 20, "All vectors should be recovered");
        }
    }
    
    #[tokio::test]
    async fn test_lsm_performance_under_load() {
        let (lsm_engine, shared_services, _temp_dir) = create_test_lsm_engine(None).await;
        let collection_id = "performance_test";
        
        create_test_collection(&shared_services, collection_id, 128).await;
        
        // Insert large number of high-dimensional vectors
        let start_time = std::time::Instant::now();
        let vector_count = 1000;
        let dimension = 128;
        
        for i in 0..vector_count {
            let vector = VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: (0..dimension).map(|d| (i * dimension + d) as f32 * 0.01).collect(),
                metadata: HashMap::from([
                    ("index".to_string(), serde_json::json!(i))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };
            
            lsm_engine.insert_vector(collection_id, &vector.id, vector)
                .await
                .expect("Failed to insert vector");
            
            // Flush periodically to test mixed workload
            if i % 100 == 99 {
                lsm_engine.flush_collection(collection_id)
                    .await
                    .expect("Failed to flush");
            }
        }
        
        let insert_time = start_time.elapsed();
        
        // Final flush
        lsm_engine.flush_collection(collection_id)
            .await
            .expect("Failed to final flush");
        
        // Compact all files
        let compact_start = std::time::Instant::now();
        lsm_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact");
        let compact_time = compact_start.elapsed();
        
        // Calculate performance metrics
        let inserts_per_sec = vector_count as f64 / insert_time.as_secs_f64();
        
        println!("LSM Performance Metrics:");
        println!("  Insert time: {:?}", insert_time);
        println!("  Compact time: {:?}", compact_time);
        println!("  Inserts/sec: {:.2}", inserts_per_sec);
        
        // Performance assertions
        assert!(insert_time.as_secs() < 10, "Insert should complete within 10 seconds");
        assert!(compact_time.as_secs() < 5, "Compaction should complete within 5 seconds");
        
        let stats = lsm_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        
        assert_eq!(stats.total_vectors, vector_count, "All vectors should be stored");
    }
}