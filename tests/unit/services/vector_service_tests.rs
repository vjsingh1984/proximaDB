//! Unit tests for VectorService operations

use proximadb::services::vector_service::{VectorService, VectorInsertRequest, VectorSearchRequest};
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::engine::StorageEngine;
use proximadb::storage::assignment_service::{AssignmentService, StaticAssignmentService};
use proximadb::index::axis::manager::AxisManager;
use proximadb::proto::proximadb::{Collection, CollectionConfig, DistanceMetric as ProtoDistanceMetric, StorageEngine as ProtoStorageEngine};
use proximadb::core::{VectorRecord, VectorId};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::metadata::backends::memory_backend::MemoryMetadataBackend;
use proximadb::storage::metadata::store::UnifiedMetadataStore;
use proximadb::services::SharedServices;
use std::sync::Arc;
use std::collections::HashMap;
use tempfile::TempDir;
use serde_json::json;

/// Create a test VectorService with in-memory backends
async fn create_test_vector_service() -> (VectorService, TempDir) {
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
    
    // Create collection service with basic config
    let collection_service = Arc::new(CollectionService::new(
        temp_dir.path().to_path_buf(),
        Arc::new(StaticAssignmentService::new(temp_dir.path().to_path_buf())),
        metadata_store.clone(),
    ));
    
    // Create storage engine
    let storage_engine = Arc::new(
        StorageEngine::new(
            temp_dir.path().to_path_buf(),
            filesystem.clone(),
            Default::default(),
        )
        .await
        .expect("Failed to create storage engine")
    );
    
    // Create shared services
    let shared_services = SharedServices {
        collection_service: collection_service.clone(),
        assignment_service: Arc::new(StaticAssignmentService::new(temp_dir.path().to_path_buf())),
        metadata_store: metadata_store.clone(),
    };
    
    // Create AXIS manager with shared services
    let axis_manager = Arc::new(
        AxisManager::new(Arc::new(shared_services))
            .await
            .expect("Failed to create AXIS manager")
    );
    
    // Create vector service
    let vector_service = VectorService::new(
        storage_engine,
        collection_service,
        filesystem,
        metadata_store,
        axis_manager,
    )
    .await
    .expect("Failed to create vector service");
    
    (vector_service, temp_dir)
}

/// Create a test collection
async fn create_test_collection(collection_service: &Arc<CollectionService>, collection_id: &str, dimension: usize) {
    let collection = Collection {
        id: collection_id.to_string(),
        config: Some(CollectionConfig {
            dimension: dimension as i32,
            distance_metric: ProtoDistanceMetric::Cosine as i32,
            storage_engine: ProtoStorageEngine::Viper as i32,
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
    use proximadb::storage::persistence::wal::schema::create_avro_vector_batch;
    
    #[tokio::test]
    async fn test_handle_vector_insert_single() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let collection_id = "test_collection";
        
        // Create collection first
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Create test vector
        let vector_record = VectorRecord {
            id: "vec1".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp_micros(),
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        
        // Create Avro payload
        let avro_payload = create_avro_vector_batch(&[vector_record])
            .expect("Failed to create Avro payload");
        
        // Test insert
        let result = vector_service.handle_vector_batch(collection_id, &avro_payload).await;
        assert!(result.is_ok(), "Vector insert should succeed");
    }
    
    #[tokio::test]
    async fn test_handle_vector_insert_batch() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let collection_id = "test_collection";
        
        // Create collection
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Create multiple test vectors
        let vectors: Vec<VectorRecord> = (0..10)
            .map(|i| VectorRecord {
                id: format!("vec{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32, i as f32 + 1.0, i as f32 + 2.0, i as f32 + 3.0],
                metadata: HashMap::from([(
                    "index".to_string(),
                    serde_json::Value::Number(i.into())
                )]),
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
        
        // Create Avro payload
        let avro_payload = create_avro_vector_batch(&vectors)
            .expect("Failed to create Avro payload");
        
        // Test batch insert
        let result = vector_service.handle_vector_batch(collection_id, &avro_payload).await;
        assert!(result.is_ok(), "Batch vector insert should succeed");
    }
    
    #[tokio::test]
    async fn test_handle_vector_search_basic() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let collection_id = "test_collection";
        
        // Create collection
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Insert test vectors
        let vectors: Vec<VectorRecord> = vec![
            VectorRecord {
                id: "vec1".to_string(),
                collection_id: collection_id.to_string(),
                vector: vec![1.0, 0.0, 0.0, 0.0],
                metadata: HashMap::new(),
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
            VectorRecord {
                id: "vec2".to_string(),
                collection_id: collection_id.to_string(),
                vector: vec![0.0, 1.0, 0.0, 0.0],
                metadata: HashMap::new(),
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
        ];
        
        let avro_payload = create_avro_vector_batch(&vectors)
            .expect("Failed to create Avro payload");
        
        vector_service.handle_vector_batch(collection_id, &avro_payload)
            .await
            .expect("Failed to insert vectors");
        
        // Test search
        let search_request = VectorSearchRequest {
            collection_id: collection_id.to_string(),
            query_vectors: vec![vec![1.0, 0.0, 0.0, 0.0]],
            k: 2,
            distance_metric: Some("cosine".to_string()),
            filters: None,
            search_params: None,
        };
        
        let search_json = serde_json::to_string(&search_request).unwrap();
        let result = vector_service.handle_vector_search(&search_json).await;
        
        assert!(result.is_ok(), "Vector search should succeed");
        let response = result.unwrap();
        assert_eq!(response.results.len(), 1, "Should return results for one query");
        assert!(!response.results[0].matches.is_empty(), "Should find matches");
    }
    
    #[tokio::test]
    async fn test_handle_vector_search_with_filters() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let collection_id = "test_collection";
        
        // Create collection
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Insert test vectors with metadata
        let vectors: Vec<VectorRecord> = vec![
            VectorRecord {
                id: "vec1".to_string(),
                collection_id: collection_id.to_string(),
                vector: vec![1.0, 0.0, 0.0, 0.0],
                metadata: HashMap::from([
                    ("category".to_string(), json!("A")),
                    ("score".to_string(), json!(100))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
            VectorRecord {
                id: "vec2".to_string(),
                collection_id: collection_id.to_string(),
                vector: vec![0.9, 0.1, 0.0, 0.0],
                metadata: HashMap::from([
                    ("category".to_string(), json!("B")),
                    ("score".to_string(), json!(50))
                ]),
                timestamp: chrono::Utc::now().timestamp_micros(),
                created_at: chrono::Utc::now().timestamp_micros(),
                updated_at: chrono::Utc::now().timestamp_micros(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
        ];
        
        let avro_payload = create_avro_vector_batch(&vectors)
            .expect("Failed to create Avro payload");
        
        vector_service.handle_vector_batch(collection_id, &avro_payload)
            .await
            .expect("Failed to insert vectors");
        
        // Test search with metadata filter
        let search_request = VectorSearchRequest {
            collection_id: collection_id.to_string(),
            query_vectors: vec![vec![1.0, 0.0, 0.0, 0.0]],
            k: 2,
            distance_metric: Some("cosine".to_string()),
            filters: Some(json!({
                "category": "A"
            })),
            search_params: None,
        };
        
        let search_json = serde_json::to_string(&search_request).unwrap();
        let result = vector_service.handle_vector_search(&search_json).await;
        
        assert!(result.is_ok(), "Vector search with filters should succeed");
        let response = result.unwrap();
        assert_eq!(response.results[0].matches.len(), 1, "Should only return filtered results");
        assert_eq!(response.results[0].matches[0].id, "vec1", "Should return correct filtered vector");
    }
    
    #[tokio::test]
    async fn test_handle_vector_delete() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let collection_id = "test_collection";
        
        // Create collection
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Insert a vector
        let vector = VectorRecord {
            id: "vec1".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp_micros(),
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        
        let avro_payload = create_avro_vector_batch(&[vector])
            .expect("Failed to create Avro payload");
        
        vector_service.handle_vector_batch(collection_id, &avro_payload)
            .await
            .expect("Failed to insert vector");
        
        // Test delete
        let result = vector_service.delete_vector(collection_id, "vec1").await;
        assert!(result.is_ok(), "Vector delete should succeed");
        
        // Verify vector is deleted by searching
        let search_request = VectorSearchRequest {
            collection_id: collection_id.to_string(),
            query_vectors: vec![vec![1.0, 2.0, 3.0, 4.0]],
            k: 1,
            distance_metric: Some("cosine".to_string()),
            filters: None,
            search_params: None,
        };
        
        let search_json = serde_json::to_string(&search_request).unwrap();
        let search_result = vector_service.handle_vector_search(&search_json).await;
        
        assert!(search_result.is_ok());
        let response = search_result.unwrap();
        assert!(response.results[0].matches.is_empty(), "Deleted vector should not be found");
    }
    
    #[tokio::test]
    async fn test_handle_vector_update() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let collection_id = "test_collection";
        let vector_id = "vec1";
        
        // Create collection
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Insert initial vector
        let initial_vector = VectorRecord {
            id: vector_id.to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 0.0, 0.0, 0.0],
            metadata: HashMap::from([("version".to_string(), json!("v1"))]),
            timestamp: chrono::Utc::now().timestamp_micros(),
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        
        let avro_payload = create_avro_vector_batch(&[initial_vector])
            .expect("Failed to create Avro payload");
        
        vector_service.handle_vector_batch(collection_id, &avro_payload)
            .await
            .expect("Failed to insert initial vector");
        
        // Update vector
        let updated_vector = VectorRecord {
            id: vector_id.to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![0.0, 1.0, 0.0, 0.0],
            metadata: HashMap::from([("version".to_string(), json!("v2"))]),
            timestamp: chrono::Utc::now().timestamp_micros() + 1000,
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros() + 1000,
            expires_at: None,
            version: 2,
            rank: None,
            score: None,
            distance: None,
        };
        
        let update_result = vector_service.update_vector(
            collection_id,
            vector_id,
            updated_vector.clone()
        ).await;
        
        assert!(update_result.is_ok(), "Vector update should succeed");
        
        // Verify update by searching
        let search_request = VectorSearchRequest {
            collection_id: collection_id.to_string(),
            query_vectors: vec![vec![0.0, 1.0, 0.0, 0.0]],
            k: 1,
            distance_metric: Some("cosine".to_string()),
            filters: None,
            search_params: None,
        };
        
        let search_json = serde_json::to_string(&search_request).unwrap();
        let search_result = vector_service.handle_vector_search(&search_json).await;
        
        assert!(search_result.is_ok());
        let response = search_result.unwrap();
        assert_eq!(response.results[0].matches[0].id, vector_id);
        assert_eq!(
            response.results[0].matches[0].metadata.get("version"),
            Some(&json!("v2")),
            "Should return updated metadata"
        );
    }
    
    #[tokio::test]
    async fn test_health_check() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        
        let health = vector_service.health_check().await;
        assert!(health.is_ok(), "Health check should succeed");
        
        let health_status = health.unwrap();
        assert!(health_status.healthy, "Service should be healthy");
        assert!(health_status.uptime_seconds > 0, "Uptime should be positive");
    }
    
    #[tokio::test]
    async fn test_force_flush_collection() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let collection_id = "test_collection";
        
        // Create collection
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Insert vectors
        let vectors: Vec<VectorRecord> = (0..5)
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
        
        let avro_payload = create_avro_vector_batch(&vectors)
            .expect("Failed to create Avro payload");
        
        vector_service.handle_vector_batch(collection_id, &avro_payload)
            .await
            .expect("Failed to insert vectors");
        
        // Test force flush
        let result = vector_service.force_flush_collection(collection_id).await;
        assert!(result.is_ok(), "Force flush should succeed");
    }
    
    #[tokio::test]
    async fn test_concurrent_operations() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let vector_service = Arc::new(vector_service);
        let collection_id = "test_collection";
        
        // Create collection
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Spawn multiple concurrent insert tasks
        let mut handles = vec![];
        
        for i in 0..10 {
            let service = vector_service.clone();
            let coll_id = collection_id.to_string();
            
            let handle = tokio::spawn(async move {
                let vector = VectorRecord {
                    id: format!("vec{}", i),
                    collection_id: coll_id.clone(),
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
                };
                
                let avro_payload = create_avro_vector_batch(&[vector])
                    .expect("Failed to create Avro payload");
                
                service.handle_vector_batch(&coll_id, &avro_payload).await
            });
            
            handles.push(handle);
        }
        
        // Wait for all inserts to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "Concurrent insert should succeed");
        }
        
        // Verify all vectors were inserted
        let search_request = VectorSearchRequest {
            collection_id: collection_id.to_string(),
            query_vectors: vec![vec![0.0; 4]],
            k: 10,
            distance_metric: Some("euclidean".to_string()),
            filters: None,
            search_params: None,
        };
        
        let search_json = serde_json::to_string(&search_request).unwrap();
        let result = vector_service.handle_vector_search(&search_json).await;
        
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.results[0].matches.len(), 10, "All vectors should be inserted");
    }
    
    #[tokio::test]
    async fn test_error_handling_invalid_collection() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        
        // Try to insert into non-existent collection
        let vector = VectorRecord {
            id: "vec1".to_string(),
            collection_id: "non_existent".to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp_micros(),
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        
        let avro_payload = create_avro_vector_batch(&[vector])
            .expect("Failed to create Avro payload");
        
        let result = vector_service.handle_vector_batch("non_existent", &avro_payload).await;
        assert!(result.is_err(), "Insert to non-existent collection should fail");
    }
    
    #[tokio::test]
    async fn test_error_handling_dimension_mismatch() {
        let (vector_service, _temp_dir) = create_test_vector_service().await;
        let collection_id = "test_collection";
        
        // Create collection with dimension 4
        create_test_collection(&vector_service.collection_service, collection_id, 4).await;
        
        // Try to insert vector with wrong dimension
        let vector = VectorRecord {
            id: "vec1".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 2.0, 3.0], // Wrong dimension (3 instead of 4)
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp_micros(),
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        
        let avro_payload = create_avro_vector_batch(&[vector])
            .expect("Failed to create Avro payload");
        
        let result = vector_service.handle_vector_batch(collection_id, &avro_payload).await;
        assert!(result.is_err(), "Insert with dimension mismatch should fail");
    }
}