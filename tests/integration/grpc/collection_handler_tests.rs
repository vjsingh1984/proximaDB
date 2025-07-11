//! Integration tests for gRPC collection handlers
//!
//! Tests the complete flow from gRPC requests through handlers to collection service

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;
use tonic::Request;

use proximadb::proto::proximadb::{
    proximadb_server::Proximadb,
    CollectionRequest as ProtoCollectionRequest,
    CollectionConfig as ProtoCollectionConfig,
    CollectionOperation,
    StorageEngine,
    IndexingAlgorithm,
    IndexConfig,
    HnswConfig,
    QuantizationConfig,
};
use proximadb::network::grpc::service::ProximaDbGrpcService;
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::metadata::backends::filestore_backend::{
    FilestoreMetadataBackend, FilestoreMetadataConfig,
};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

/// Create test gRPC service with all dependencies
async fn create_test_grpc_service() -> Result<(ProximaDbGrpcService, TempDir)> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_string_lossy().to_string();
    
    // Create filesystem factory
    let fs_config = FilesystemConfig {
        default_fs: Some(format!("file://{}", temp_path)),
        ..Default::default()
    };
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
    
    // Create metadata backend
    let metadata_config = FilestoreMetadataConfig {
        storage_url: format!("file://{}/metadata", temp_path),
        enable_compression: false,
        enable_snapshots: false,
        ..Default::default()
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(metadata_config, filesystem.clone()).await?
    );
    
    // Create collection service
    let collection_service = Arc::new(CollectionService::new(metadata_backend).await?);
    
    // Create gRPC service
    let grpc_service = ProximaDbGrpcService::new(
        None, // vector_service
        Some(collection_service),
        None, // assignment_service
        filesystem,
    );
    
    Ok((grpc_service, temp_dir))
}

#[tokio::test]
async fn test_grpc_create_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_grpc_service().await?;
    
    // Create proto collection config
    let config = ProtoCollectionConfig {
        name: "test_grpc_collection".to_string(),
        dimension: 384,
        distance_metric: 1, // Cosine
        storage_engine: StorageEngine::Viper as i32,
        indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        filterable_metadata_fields: vec!["category".to_string(), "priority".to_string()],
        index_config: Some(IndexConfig {
            update_mode: 0, // Synchronous
            async_update_timeout_ms: 5000,
            async_update_batch_size: 100,
            enable_background_optimization: true,
            hnsw_config: Some(HnswConfig {
                m: 16,
                ef_construction: 200,
                ef_search: 100,
                max_partition_size: 10000,
            }),
            ivf_config: None,
            build_concurrency: 4,
            memory_limit_mb: 1024,
            checkpoint_interval_ms: 60000,
        }),
        quantization_config: None,
        filterable_columns: vec![],
    };
    
    let request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Create as i32,
        collection_id: None,
        collection_config: Some(config),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response = service.manage_collection(request).await?;
    let response = response.into_inner();
    
    assert!(response.success);
    assert!(response.collection.is_some());
    
    let created = response.collection.unwrap();
    assert_eq!(created.name, "test_grpc_collection");
    assert_eq!(created.dimension, 384);
    assert_eq!(created.distance_metric, 1); // Cosine
    assert_eq!(created.storage_engine, StorageEngine::Viper as i32);
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_create_collection_with_quantization() -> Result<()> {
    let (service, _temp_dir) = create_test_grpc_service().await?;
    
    let config = ProtoCollectionConfig {
        name: "test_quantized".to_string(),
        dimension: 768,
        distance_metric: 2, // Euclidean
        storage_engine: StorageEngine::Viper as i32,
        indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        filterable_metadata_fields: vec![],
        index_config: None,
        quantization_config: Some(QuantizationConfig {
            r#type: "pq".to_string(),
            bits: 8,
            subvectors: 16,
            codebook_size: 256,
            enable_progressive: true,
            accuracy_threshold: 0.95,
            compression_ratio: 4.0,
            train_samples: 10000,
        }),
        filterable_columns: vec![],
    };
    
    let request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Create as i32,
        collection_id: None,
        collection_config: Some(config),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response = service.manage_collection(request).await?;
    let response = response.into_inner();
    
    assert!(response.success);
    assert!(response.collection.is_some());
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_list_collections() -> Result<()> {
    let (service, _temp_dir) = create_test_grpc_service().await?;
    
    // Create a few collections first
    for i in 0..3 {
        let config = ProtoCollectionConfig {
            name: format!("grpc_list_test_{}", i),
            dimension: 128,
            distance_metric: 1,
            storage_engine: StorageEngine::Viper as i32,
            indexing_algorithm: IndexingAlgorithm::Flat as i32,
            filterable_metadata_fields: vec![],
            index_config: None,
            quantization_config: None,
            filterable_columns: vec![],
        };
        
        let request = Request::new(ProtoCollectionRequest {
            operation: CollectionOperation::Create as i32,
            collection_id: None,
            collection_config: Some(config),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        });
        
        service.manage_collection(request).await?;
    }
    
    // List collections
    let request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::List as i32,
        collection_id: None,
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response = service.manage_collection(request).await?;
    let response = response.into_inner();
    
    assert!(response.success);
    assert_eq!(response.collections.len(), 3);
    
    let names: Vec<String> = response.collections.iter()
        .map(|c| c.name.clone())
        .collect();
    
    assert!(names.contains(&"grpc_list_test_0".to_string()));
    assert!(names.contains(&"grpc_list_test_1".to_string()));
    assert!(names.contains(&"grpc_list_test_2".to_string()));
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_get_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_grpc_service().await?;
    
    // Create a collection
    let config = ProtoCollectionConfig {
        name: "grpc_get_test".to_string(),
        dimension: 256,
        distance_metric: 3, // DotProduct
        storage_engine: StorageEngine::Lsm as i32,
        indexing_algorithm: IndexingAlgorithm::Ivf as i32,
        filterable_metadata_fields: vec!["field1".to_string()],
        index_config: None,
        quantization_config: None,
        filterable_columns: vec![],
    };
    
    let create_request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Create as i32,
        collection_id: None,
        collection_config: Some(config),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let create_response = service.manage_collection(create_request).await?;
    assert!(create_response.into_inner().success);
    
    // Get collection by name
    let get_request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Get as i32,
        collection_id: Some("grpc_get_test".to_string()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response = service.manage_collection(get_request).await?;
    let response = response.into_inner();
    
    assert!(response.success);
    assert!(response.collection.is_some());
    
    let collection = response.collection.unwrap();
    assert_eq!(collection.name, "grpc_get_test");
    assert_eq!(collection.dimension, 256);
    assert_eq!(collection.distance_metric, 3); // DotProduct
    assert_eq!(collection.storage_engine, StorageEngine::Lsm as i32);
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_update_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_grpc_service().await?;
    
    // Create a collection
    let config = ProtoCollectionConfig {
        name: "grpc_update_test".to_string(),
        dimension: 128,
        distance_metric: 1,
        storage_engine: StorageEngine::Viper as i32,
        indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        filterable_metadata_fields: vec![],
        index_config: None,
        quantization_config: None,
        filterable_columns: vec![],
    };
    
    let create_request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Create as i32,
        collection_id: None,
        collection_config: Some(config.clone()),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    service.manage_collection(create_request).await?;
    
    // Update collection
    let mut update_params = std::collections::HashMap::new();
    update_params.insert("description".to_string(), "Updated via gRPC".to_string());
    update_params.insert("tags".to_string(), r#"["tag1","tag2"]"#.to_string());
    update_params.insert("owner".to_string(), "grpc_user".to_string());
    
    let update_request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Update as i32,
        collection_id: Some("grpc_update_test".to_string()),
        collection_config: None,
        query_params: update_params,
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response = service.manage_collection(update_request).await?;
    let response = response.into_inner();
    
    assert!(response.success);
    assert!(response.collection.is_some());
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_delete_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_grpc_service().await?;
    
    // Create a collection
    let config = ProtoCollectionConfig {
        name: "grpc_delete_test".to_string(),
        dimension: 128,
        distance_metric: 1,
        storage_engine: StorageEngine::Viper as i32,
        indexing_algorithm: IndexingAlgorithm::Flat as i32,
        filterable_metadata_fields: vec![],
        index_config: None,
        quantization_config: None,
        filterable_columns: vec![],
    };
    
    let create_request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Create as i32,
        collection_id: None,
        collection_config: Some(config),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    service.manage_collection(create_request).await?;
    
    // Delete collection
    let delete_request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Delete as i32,
        collection_id: Some("grpc_delete_test".to_string()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response = service.manage_collection(delete_request).await?;
    let response = response.into_inner();
    
    assert!(response.success);
    
    // Verify it's deleted
    let get_request = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Get as i32,
        collection_id: Some("grpc_delete_test".to_string()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let get_response = service.manage_collection(get_request).await?;
    let get_response = get_response.into_inner();
    
    assert!(!get_response.success);
    assert!(get_response.error_message.is_some());
    assert!(get_response.error_message.unwrap().contains("not found"));
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_duplicate_collection_error() -> Result<()> {
    let (service, _temp_dir) = create_test_grpc_service().await?;
    
    let config = ProtoCollectionConfig {
        name: "grpc_duplicate".to_string(),
        dimension: 128,
        distance_metric: 1,
        storage_engine: StorageEngine::Viper as i32,
        indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        filterable_metadata_fields: vec![],
        index_config: None,
        quantization_config: None,
        filterable_columns: vec![],
    };
    
    // Create first collection
    let request1 = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Create as i32,
        collection_id: None,
        collection_config: Some(config.clone()),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response1 = service.manage_collection(request1).await?;
    assert!(response1.into_inner().success);
    
    // Try to create duplicate
    let request2 = Request::new(ProtoCollectionRequest {
        operation: CollectionOperation::Create as i32,
        collection_id: None,
        collection_config: Some(config),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response2 = service.manage_collection(request2).await?;
    let response2 = response2.into_inner();
    
    assert!(!response2.success);
    assert!(response2.error_message.is_some());
    assert!(response2.error_message.unwrap().contains("already exists"));
    
    Ok(())
}

#[tokio::test]
async fn test_grpc_invalid_operation() -> Result<()> {
    let (service, _temp_dir) = create_test_grpc_service().await?;
    
    // Send invalid operation
    let request = Request::new(ProtoCollectionRequest {
        operation: 999, // Invalid operation
        collection_id: None,
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    });
    
    let response = service.manage_collection(request).await;
    
    // Should return an error status
    assert!(response.is_err());
    
    Ok(())
}