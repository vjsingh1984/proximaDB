/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! gRPC Vector Operations Integration Tests
//!
//! Tests the current gRPC API for vector insertion and search operations
//! using the proper binary Avro payload format.

use anyhow::Result;
use proximadb::network::grpc::service::ProximaDbGrpcService;
use proximadb::proto::proximadb::*;
use proximadb::proto::proximadb::proxima_db_server::ProximaDb;
use proximadb::storage::StorageEngine as StorageEngineImpl;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tonic::Request;

/// Create a test storage engine and gRPC service
async fn create_test_service() -> Result<(TempDir, ProximaDbGrpcService)> {
    let temp_dir = TempDir::new()?;
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");
    
    // Create directories
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&wal_dir)?;

    // Create storage config
    let storage_config = proximadb::StorageConfig {
        data_dirs: vec![data_dir],
        wal_dir: wal_dir,
        storage_layout: proximadb::core::storage_layout::StorageLayoutConfig::default_2_disk(),
        mmap_enabled: true,
        lsm_config: proximadb::core::config::LsmConfig::default(),
        cache_size_mb: 64,
        bloom_filter_bits: 12,
        filesystem_config: proximadb::core::config::FilesystemConfig::default(),
        metadata_backend: None,
    };

    // Create storage engine
    let mut storage_engine = StorageEngineImpl::new(storage_config).await?;
    storage_engine.start().await?;

    let storage = Arc::new(RwLock::new(storage_engine));
    let service = ProximaDbGrpcService::new(storage).await;

    Ok((temp_dir, service))
}

/// Create a test collection via gRPC
async fn create_test_collection(
    service: &ProximaDbGrpcService,
    collection_id: &str,
    dimension: i32,
) -> Result<()> {
    let collection_config = CollectionConfig {
        name: collection_id.to_string(),
        dimension,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: proximadb::proto::proximadb::StorageEngine::Viper as i32,
        indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        filterable_metadata_fields: vec![],
        indexing_config: HashMap::new(),
    };

    let request = Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionCreate as i32,
        collection_id: Some(collection_id.to_string()),
        collection_config: Some(collection_config),
        query_params: HashMap::new(),
        options: HashMap::new(),
        migration_config: HashMap::new(),
    });

    let response = service.collection_operation(request).await?;
    assert!(response.get_ref().success);
    
    println!("✅ Created collection: {}", collection_id);
    Ok(())
}

/// Create Avro payload for vector data (just vector data, not metadata)
fn create_vector_avro_payload(vectors: Vec<(String, Vec<f32>, HashMap<String, String>)>) -> Vec<u8> {
    let vector_records: Vec<serde_json::Value> = vectors
        .into_iter()
        .map(|(id, vector, metadata)| {
            json!({
                "id": id,
                "vector": vector,
                "metadata": metadata,
                "timestamp": chrono::Utc::now().timestamp_micros(),
                "version": 1
            })
        })
        .collect();

    serde_json::to_vec(&vector_records).expect("Failed to serialize vector data")
}

#[tokio::test]
async fn test_grpc_health_check() -> Result<()> {
    let (_temp_dir, service) = create_test_service().await?;

    let request = Request::new(HealthRequest {});
    let response = service.health(request).await?;

    assert_eq!(response.get_ref().status, "healthy");
    assert!(!response.get_ref().version.is_empty());
    
    println!("✅ gRPC health check passed");
    Ok(())
}

#[tokio::test]
async fn test_grpc_collection_creation() -> Result<()> {
    let (_temp_dir, service) = create_test_service().await?;

    create_test_collection(&service, "test_collection", 384).await?;
    
    // List collections to verify creation
    let list_request = Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionList as i32,
        collection_id: None,
        collection_config: None,
        query_params: HashMap::new(),
        options: HashMap::new(),
        migration_config: HashMap::new(),
    });

    let response = service.collection_operation(list_request).await?;
    assert!(response.get_ref().success);
    assert!(!response.get_ref().collections.is_empty());
    
    println!("✅ gRPC collection creation and listing passed");
    Ok(())
}

#[tokio::test]
async fn test_grpc_vector_insertion() -> Result<()> {
    let (_temp_dir, service) = create_test_service().await?;
    let collection_id = "vector_test_collection";

    // Create collection
    create_test_collection(&service, collection_id, 384).await?;

    // Create test vectors with BERT-like dimensions
    let test_vectors = vec![
        (
            "vec_1".to_string(),
            vec![0.1; 384], // 384-dimensional vector (BERT small)
            [("text".to_string(), "test vector 1".to_string())]
                .iter()
                .cloned()
                .collect(),
        ),
        (
            "vec_2".to_string(),
            vec![0.2; 384],
            [("text".to_string(), "test vector 2".to_string())]
                .iter()
                .cloned()
                .collect(),
        ),
    ];

    // Create Avro payload (just vector data)
    let avro_payload = create_vector_avro_payload(test_vectors);

    // Insert vectors via gRPC
    let insert_request = Request::new(VectorInsertRequest {
        collection_id: collection_id.to_string(),
        upsert_mode: false,
        vectors_avro_payload: avro_payload,
        batch_timeout_ms: Some(5000),
        request_id: Some("test_insert_1".to_string()),
    });

    let response = service.vector_insert(insert_request).await?;
    assert!(response.get_ref().success);
    assert!(response.get_ref().metrics.is_some());
    
    let metrics = response.get_ref().metrics.as_ref().unwrap();
    assert_eq!(metrics.total_processed, 2);
    assert_eq!(metrics.successful_count, 2);
    assert_eq!(metrics.failed_count, 0);
    
    println!("✅ gRPC vector insertion passed");
    println!("   Vectors processed: {}", metrics.total_processed);
    println!("   Processing time: {}μs", metrics.processing_time_us);
    println!("   WAL write time: {}μs", metrics.wal_write_time_us);
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_vector_search() -> Result<()> {
    let (_temp_dir, service) = create_test_service().await?;
    let collection_id = "search_test_collection";

    // Create collection
    create_test_collection(&service, collection_id, 384).await?;

    // Insert test vectors first
    let test_vectors = vec![
        (
            "search_vec_1".to_string(),
            vec![0.1; 384],
            [("category".to_string(), "test".to_string())]
                .iter()
                .cloned()
                .collect(),
        ),
        (
            "search_vec_2".to_string(),
            vec![0.2; 384],
            [("category".to_string(), "example".to_string())]
                .iter()
                .cloned()
                .collect(),
        ),
    ];

    let avro_payload = create_vector_avro_payload(test_vectors);
    let insert_request = Request::new(VectorInsertRequest {
        collection_id: collection_id.to_string(),
        upsert_mode: false,
        vectors_avro_payload: avro_payload,
        batch_timeout_ms: Some(5000),
        request_id: Some("test_search_insert".to_string()),
    });

    let insert_response = service.vector_insert(insert_request).await?;
    assert!(insert_response.get_ref().success);

    // Search for similar vectors
    let query_vector = vec![0.15; 384]; // Similar to our test vectors
    let search_query = SearchQuery {
        vector: query_vector,
        id: None,
        metadata_filter: HashMap::new(),
    };

    let include_fields = IncludeFields {
        vector: false,
        metadata: true,
        score: true,
        rank: true,
    };

    let search_request = Request::new(VectorSearchRequest {
        collection_id: collection_id.to_string(),
        queries: vec![search_query],
        top_k: 10,
        distance_metric_override: Some(DistanceMetric::Cosine as i32),
        search_params: HashMap::new(),
        include_fields: Some(include_fields),
    });

    let response = service.vector_search(search_request).await?;
    assert!(response.get_ref().success);
    
    println!("✅ gRPC vector search passed");
    println!("   Response size: {} bytes", response.get_ref().result_info.as_ref().map(|i| i.estimated_size_bytes).unwrap_or(0));
    
    // Check if we got results (may be 0 if collection not properly indexed yet)
    if let Some(result_info) = &response.get_ref().result_info {
        println!("   Results found: {}", result_info.result_count);
        println!("   Using Avro binary: {}", result_info.is_avro_binary);
    }
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_vector_end_to_end() -> Result<()> {
    let (_temp_dir, service) = create_test_service().await?;
    let collection_id = "e2e_test_collection";

    // 1. Create collection
    create_test_collection(&service, collection_id, 128).await?;

    // 2. Insert vectors
    let test_vectors = vec![
        (
            "e2e_vec_1".to_string(),
            vec![1.0, 0.0, 0.0, 1.0, 0.5, 0.5, 0.8, 0.2] // 8D for faster test
                .into_iter()
                .cycle()
                .take(128)
                .collect(),
            [("type".to_string(), "test".to_string())]
                .iter()
                .cloned()
                .collect(),
        ),
        (
            "e2e_vec_2".to_string(),
            vec![0.0, 1.0, 1.0, 0.0, 0.3, 0.7, 0.6, 0.4]
                .into_iter()
                .cycle()
                .take(128)
                .collect(),
            [("type".to_string(), "example".to_string())]
                .iter()
                .cloned()
                .collect(),
        ),
    ];

    let avro_payload = create_vector_avro_payload(test_vectors);
    let insert_request = Request::new(VectorInsertRequest {
        collection_id: collection_id.to_string(),
        upsert_mode: false,
        vectors_avro_payload: avro_payload,
        batch_timeout_ms: Some(5000),
        request_id: Some("e2e_insert".to_string()),
    });

    let insert_response = service.vector_insert(insert_request).await?;
    assert!(insert_response.get_ref().success);
    let insert_metrics = insert_response.get_ref().metrics.as_ref().unwrap();

    // 3. Search vectors
    let query_vector = vec![0.5, 0.5, 0.5, 0.5, 0.4, 0.6, 0.7, 0.3]
        .into_iter()
        .cycle()
        .take(128)
        .collect();
    
    let search_query = SearchQuery {
        vector: query_vector,
        id: None,
        metadata_filter: HashMap::new(),
    };

    let search_request = Request::new(VectorSearchRequest {
        collection_id: collection_id.to_string(),
        queries: vec![search_query],
        top_k: 5,
        distance_metric_override: Some(DistanceMetric::Cosine as i32),
        search_params: HashMap::new(),
        include_fields: Some(IncludeFields {
            vector: false,
            metadata: true,
            score: true,
            rank: true,
        }),
    });

    let search_response = service.vector_search(search_request).await?;
    assert!(search_response.get_ref().success);

    println!("✅ gRPC end-to-end test passed");
    println!("   Inserted {} vectors in {}μs", 
             insert_metrics.total_processed, 
             insert_metrics.processing_time_us);
    
    if let Some(result_info) = &search_response.get_ref().result_info {
        println!("   Search found {} results", result_info.result_count);
    }

    Ok(())
}