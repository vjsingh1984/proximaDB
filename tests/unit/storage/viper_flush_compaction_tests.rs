//! Unit tests for VIPER engine flush and compaction operations

use proximadb::storage::engines::viper::{ViperEngine, ViperConfig, CompressionConfig, SchemaConfig};
use proximadb::storage::engines::viper::compaction::{CompactionConfig, CompactionStrategy};
use proximadb::storage::engines::viper::pipeline::{ViperPipeline, SortingStrategy};
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

/// Create test VIPER engine with custom config
async fn create_test_viper_engine(
    compaction_config: Option<CompactionConfig>,
) -> (ViperEngine, Arc<SharedServices>, TempDir) {
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
    
    // Create VIPER config
    let mut viper_config = ViperConfig::default();
    viper_config.data_dir = temp_dir.path().to_path_buf();
    
    if let Some(comp_config) = compaction_config {
        viper_config.compaction = comp_config;
    } else {
        // Set aggressive compaction for testing
        viper_config.compaction = CompactionConfig {
            strategy: CompactionStrategy::SizeTiered,
            max_file_size_mb: 10,
            min_files_to_compact: 2,
            level_multiplier: 4,
            enable_sorted_rewrite: true,
            background_threads: 2,
        };
    }
    
    // Create VIPER engine
    let viper_engine = ViperEngine::new(
        viper_config,
        filesystem,
        shared_services.clone(),
    )
    .await
    .expect("Failed to create VIPER engine");
    
    (viper_engine, shared_services, temp_dir)
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
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Viper as i32,
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
    async fn test_viper_basic_flush() {
        let (viper_engine, shared_services, _temp_dir) = create_test_viper_engine(None).await;
        let collection_id = "test_collection";
        
        // Create collection
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Insert vectors
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
        
        // Insert to memtable
        for vector in &vectors {
            viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        // Get memtable stats before flush
        let stats_before = viper_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        assert_eq!(stats_before.memtable_size, 10, "Should have 10 vectors in memtable");
        
        // Flush to disk
        let flush_result = viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush collection");
        
        assert!(flush_result.success, "Flush should succeed");
        assert!(flush_result.vectors_flushed > 0, "Should flush some vectors");
        
        // Get stats after flush
        let stats_after = viper_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        
        assert_eq!(stats_after.memtable_size, 0, "Memtable should be empty after flush");
        assert_eq!(stats_after.total_parquet_files, 1, "Should have one Parquet file");
        assert_eq!(stats_after.total_vectors, 10, "Total vectors should remain the same");
    }
    
    #[tokio::test]
    async fn test_viper_flush_with_metadata() {
        let (viper_engine, shared_services, _temp_dir) = create_test_viper_engine(None).await;
        let collection_id = "metadata_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Insert vectors with complex metadata
        let vectors: Vec<VectorRecord> = (0..5)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::from([
                    ("category".to_string(), serde_json::json!(if i % 2 == 0 { "A" } else { "B" })),
                    ("score".to_string(), serde_json::json!(i * 100)),
                    ("tags".to_string(), serde_json::json!(vec![format!("tag{}", i), "common"]))
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
        
        // Insert vectors
        for vector in &vectors {
            viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        // Flush
        viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush");
        
        // Verify metadata is preserved after flush
        for i in 0..5 {
            let retrieved = viper_engine.get_vector(collection_id, &format!("vec{}", i))
                .await
                .expect("Failed to get vector");
            
            assert!(retrieved.is_some(), "Vector should exist after flush");
            let vector = retrieved.unwrap();
            
            assert_eq!(
                vector.metadata.get("category").and_then(|v| v.as_str()),
                Some(if i % 2 == 0 { "A" } else { "B" }),
                "Category metadata should be preserved"
            );
            assert_eq!(
                vector.metadata.get("score").and_then(|v| v.as_i64()),
                Some(i as i64 * 100),
                "Score metadata should be preserved"
            );
        }
    }
    
    #[tokio::test]
    async fn test_viper_compaction_trigger() {
        let mut compaction_config = CompactionConfig::default();
        compaction_config.min_files_to_compact = 2;
        compaction_config.max_file_size_mb = 1; // Small size to trigger compaction easily
        
        let (viper_engine, shared_services, _temp_dir) = 
            create_test_viper_engine(Some(compaction_config)).await;
        let collection_id = "compaction_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Create multiple small batches to generate multiple files
        for batch in 0..3 {
            let vectors: Vec<VectorRecord> = (0..20)
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
                viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                    .await
                    .expect("Failed to insert vector");
            }
            
            // Flush each batch
            viper_engine.flush_collection(collection_id)
                .await
                .expect("Failed to flush batch");
        }
        
        // Check file count before compaction
        let stats_before = viper_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        assert_eq!(stats_before.total_parquet_files, 3, "Should have 3 Parquet files");
        
        // Trigger compaction
        let compaction_result = viper_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact collection");
        
        assert!(compaction_result.success, "Compaction should succeed");
        assert!(compaction_result.files_compacted > 0, "Should compact some files");
        
        // Check file count after compaction
        let stats_after = viper_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        assert!(
            stats_after.total_parquet_files < stats_before.total_parquet_files,
            "Should have fewer files after compaction"
        );
        assert_eq!(stats_after.total_vectors, 60, "Total vectors should remain the same");
    }
    
    #[tokio::test]
    async fn test_viper_compaction_with_deletes() {
        let (viper_engine, shared_services, _temp_dir) = create_test_viper_engine(None).await;
        let collection_id = "delete_compaction_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Insert vectors
        let vectors: Vec<VectorRecord> = (0..30)
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
            viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        // Flush
        viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush");
        
        // Delete some vectors
        for i in 0..10 {
            viper_engine.delete_vector(collection_id, &format!("vec{}", i))
                .await
                .expect("Failed to delete vector");
        }
        
        // Flush deletes
        viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush deletes");
        
        // Compact
        let compaction_result = viper_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact");
        
        assert!(compaction_result.success, "Compaction should succeed");
        
        // Verify deleted vectors are not in compacted files
        let stats = viper_engine.get_collection_stats(collection_id)
            .await
            .expect("Failed to get stats");
        
        assert_eq!(stats.total_vectors, 20, "Should have 20 vectors after deletes");
        
        // Verify deleted vectors cannot be retrieved
        for i in 0..10 {
            let result = viper_engine.get_vector(collection_id, &format!("vec{}", i)).await;
            assert!(result.is_ok());
            assert!(result.unwrap().is_none(), "Deleted vector {} should not exist", i);
        }
    }
    
    #[tokio::test]
    async fn test_viper_sorted_compaction() {
        let mut compaction_config = CompactionConfig::default();
        compaction_config.enable_sorted_rewrite = true;
        compaction_config.min_files_to_compact = 2;
        
        let (viper_engine, shared_services, _temp_dir) = 
            create_test_viper_engine(Some(compaction_config)).await;
        let collection_id = "sorted_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // Insert vectors with timestamps in random order
        let mut vectors: Vec<VectorRecord> = (0..40)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::from([
                    ("timestamp_order".to_string(), serde_json::json!(i))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros() + (40 - i) as i64 * 1000,
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            })
            .collect();
        
        // Shuffle vectors
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();
        vectors.shuffle(&mut rng);
        
        // Insert in two batches
        for vector in &vectors[0..20] {
            viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush first batch");
        
        for vector in &vectors[20..40] {
            viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush second batch");
        
        // Trigger sorted compaction
        let compaction_result = viper_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact");
        
        assert!(compaction_result.success, "Compaction should succeed");
        assert!(
            compaction_result.compaction_type.contains("sorted"),
            "Should use sorted compaction"
        );
    }
    
    #[tokio::test]
    async fn test_viper_column_projection_optimization() {
        let (viper_engine, shared_services, _temp_dir) = create_test_viper_engine(None).await;
        let collection_id = "projection_test";
        
        create_test_collection(&shared_services, collection_id, 128).await;
        
        // Insert high-dimensional vectors with metadata
        let vectors: Vec<VectorRecord> = (0..100)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: (0..128).map(|d| (i * 128 + d) as f32 * 0.01).collect(),
                metadata: HashMap::from([
                    ("category".to_string(), serde_json::json!(i % 5)),
                    ("large_text".to_string(), serde_json::json!("x".repeat(1000))),
                    ("score".to_string(), serde_json::json!(i * 10))
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
            viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush");
        
        // Search with metadata filter (should use column projection)
        let query_vector: Vec<f32> = (0..128).map(|i| i as f32).collect();
        let filter = serde_json::json!({
            "category": 2
        });
        
        let results = viper_engine.search_vectors(
            collection_id,
            &query_vector,
            10,
            Some(proximadb::compute::distance::DistanceMetric::Cosine),
            Some(filter),
            None,
        )
        .await
        .expect("Search should succeed");
        
        // Verify results are filtered correctly
        assert!(!results.is_empty(), "Should return filtered results");
        for result in &results {
            assert_eq!(
                result.metadata.get("category").and_then(|v| v.as_i64()),
                Some(2),
                "All results should match filter"
            );
        }
    }
    
    #[tokio::test]
    async fn test_viper_concurrent_flush_operations() {
        let (viper_engine, shared_services, _temp_dir) = create_test_viper_engine(None).await;
        let viper_engine = Arc::new(viper_engine);
        
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
                viper_engine.insert_vector(&collection_id, &vector.id, vector.clone())
                    .await
                    .expect("Failed to insert vector");
            }
        }
        
        // Flush all collections concurrently
        let mut handles = vec![];
        
        for i in 0..3 {
            let engine = viper_engine.clone();
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
            let stats = viper_engine.get_collection_stats(&collection_id)
                .await
                .expect("Failed to get stats");
            
            assert_eq!(stats.memtable_size, 0, "Memtable should be empty");
            assert_eq!(stats.total_vectors, 50, "Should have all vectors");
            assert!(stats.total_parquet_files > 0, "Should have Parquet files");
        }
    }
    
    #[tokio::test]
    async fn test_viper_schema_evolution() {
        let (viper_engine, shared_services, _temp_dir) = create_test_viper_engine(None).await;
        let collection_id = "schema_evolution_test";
        
        create_test_collection(&shared_services, collection_id, 4).await;
        
        // First batch with basic metadata
        let batch1: Vec<VectorRecord> = (0..10)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::from([
                    ("field1".to_string(), serde_json::json!(i))
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
        
        for vector in &batch1 {
            viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush batch 1");
        
        // Second batch with additional metadata fields
        let batch2: Vec<VectorRecord> = (10..20)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32; 4],
                metadata: HashMap::from([
                    ("field1".to_string(), serde_json::json!(i)),
                    ("field2".to_string(), serde_json::json!(format!("text{}", i))),
                    ("field3".to_string(), serde_json::json!(i as f64 * 1.5))
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
        
        for vector in &batch2 {
            viper_engine.insert_vector(collection_id, &vector.id, vector.clone())
                .await
                .expect("Failed to insert vector");
        }
        
        viper_engine.flush_collection(collection_id)
            .await
            .expect("Failed to flush batch 2");
        
        // Compact to merge schemas
        viper_engine.compact_collection(collection_id)
            .await
            .expect("Failed to compact");
        
        // Verify all vectors are accessible with their metadata
        for i in 0..20 {
            let vector = viper_engine.get_vector(collection_id, &format!("vec{}", i))
                .await
                .expect("Failed to get vector")
                .expect("Vector should exist");
            
            assert!(vector.metadata.contains_key("field1"), "field1 should exist");
            
            if i >= 10 {
                assert!(vector.metadata.contains_key("field2"), "field2 should exist for newer vectors");
                assert!(vector.metadata.contains_key("field3"), "field3 should exist for newer vectors");
            }
        }
    }
}