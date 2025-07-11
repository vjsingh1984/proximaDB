//! Unit tests for CollectionService with native types
//!
//! Tests the new API that uses native types instead of Avro serialization

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;

use proximadb::services::collection_service::{CollectionService, CollectionServiceBuilder};
use proximadb::storage::metadata::backends::filestore_backend::{
    FilestoreMetadataBackend, FilestoreMetadataConfig,
};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::schema::collection_avro::{DistanceMetric, IndexingAlgorithm};
use proximadb::storage::metadata::StorageEngineType;

/// Create test collection service
async fn create_test_service() -> Result<(Arc<CollectionService>, TempDir)> {
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
        FilestoreMetadataBackend::new(metadata_config, filesystem).await?
    );
    
    // Create collection service
    let service = Arc::new(CollectionService::new(metadata_backend).await?);
    
    Ok((service, temp_dir))
}

#[tokio::test]
async fn test_create_collection_native_types() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create collection with native types
    let response = service.create_collection(
        "test_collection".to_string(),
        384,
        DistanceMetric::Cosine,
        StorageEngineType::Viper,
        IndexingAlgorithm::Hnsw,
        vec!["category".to_string(), "priority".to_string()],
        serde_json::json!({
            "description": "Test collection",
            "owner": "test_user"
        }),
    ).await?;
    
    assert!(response.success);
    assert!(response.collection.is_some());
    
    let collection = response.collection.unwrap();
    assert_eq!(collection.name, "test_collection");
    assert_eq!(collection.dimension, 384);
    assert_eq!(collection.distance_metric, DistanceMetric::Cosine);
    assert_eq!(collection.storage_engine, StorageEngineType::Viper);
    
    Ok(())
}

#[tokio::test]
async fn test_update_collection_native_types() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // First create a collection
    let create_response = service.create_collection(
        "test_update".to_string(),
        128,
        DistanceMetric::Euclidean,
        StorageEngineType::Lsm,
        IndexingAlgorithm::Flat,
        vec![],
        serde_json::json!({}),
    ).await?;
    
    assert!(create_response.success);
    
    // Update with native types - using Option<Option<T>> pattern
    let update_response = service.update_collection(
        "test_update",
        Some(Some("Updated description".to_string())), // Set description
        Some(vec!["tag1".to_string(), "tag2".to_string()]), // Set tags
        Some(None), // Clear owner
        Some(serde_json::json!({
            "custom_field": "custom_value"
        })),
    ).await?;
    
    assert!(update_response.success);
    assert!(update_response.collection.is_some());
    
    let updated = update_response.collection.unwrap();
    assert_eq!(updated.name, "test_update");
    // Verify config was updated
    assert!(updated.config.contains_key("custom_field"));
    
    Ok(())
}

#[tokio::test]
async fn test_get_collection_by_name() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create a collection
    service.create_collection(
        "test_get".to_string(),
        256,
        DistanceMetric::DotProduct,
        StorageEngineType::Viper,
        IndexingAlgorithm::Hnsw,
        vec![],
        serde_json::json!({}),
    ).await?;
    
    // Get by name
    let collection = service.get_collection_unified("test_get").await?;
    assert!(collection.is_some());
    
    let col = collection.unwrap();
    assert_eq!(col.name, "test_get");
    assert_eq!(col.dimension, 256);
    assert_eq!(col.distance_metric, DistanceMetric::DotProduct);
    
    Ok(())
}

#[tokio::test]
async fn test_get_collection_by_uuid() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create a collection
    let response = service.create_collection(
        "test_uuid".to_string(),
        512,
        DistanceMetric::Hamming,
        StorageEngineType::Lsm,
        IndexingAlgorithm::Ivf,
        vec![],
        serde_json::json!({}),
    ).await?;
    
    let created = response.collection.unwrap();
    let uuid = created.id.clone();
    
    // Get by UUID
    let collection = service.get_collection_unified(&uuid).await?;
    assert!(collection.is_some());
    
    let col = collection.unwrap();
    assert_eq!(col.id, uuid);
    assert_eq!(col.name, "test_uuid");
    
    Ok(())
}

#[tokio::test]
async fn test_list_collections() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create multiple collections
    for i in 0..3 {
        service.create_collection(
            format!("test_list_{}", i),
            128,
            DistanceMetric::Cosine,
            StorageEngineType::Viper,
            IndexingAlgorithm::Hnsw,
            vec![],
            serde_json::json!({}),
        ).await?;
    }
    
    // List all collections
    let collections = service.list_collections().await?;
    assert_eq!(collections.len(), 3);
    
    // Verify all collections are present
    let names: Vec<String> = collections.iter().map(|c| c.name.clone()).collect();
    assert!(names.contains(&"test_list_0".to_string()));
    assert!(names.contains(&"test_list_1".to_string()));
    assert!(names.contains(&"test_list_2".to_string()));
    
    Ok(())
}

#[tokio::test]
async fn test_delete_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create a collection
    service.create_collection(
        "test_delete".to_string(),
        384,
        DistanceMetric::Manhattan,
        StorageEngineType::Viper,
        IndexingAlgorithm::Annoy,
        vec![],
        serde_json::json!({}),
    ).await?;
    
    // Verify it exists
    let exists = service.get_collection_unified("test_delete").await?;
    assert!(exists.is_some());
    
    // Delete it
    let delete_response = service.delete_collection("test_delete").await?;
    assert!(delete_response.success);
    
    // Verify it's gone
    let gone = service.get_collection_unified("test_delete").await?;
    assert!(gone.is_none());
    
    Ok(())
}

#[tokio::test]
async fn test_duplicate_collection_error() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create first collection
    let response1 = service.create_collection(
        "duplicate_test".to_string(),
        128,
        DistanceMetric::Cosine,
        StorageEngineType::Viper,
        IndexingAlgorithm::Hnsw,
        vec![],
        serde_json::json!({}),
    ).await?;
    
    assert!(response1.success);
    
    // Try to create duplicate
    let response2 = service.create_collection(
        "duplicate_test".to_string(),
        256, // Different dimension
        DistanceMetric::Euclidean,
        StorageEngineType::Lsm,
        IndexingAlgorithm::Flat,
        vec![],
        serde_json::json!({}),
    ).await?;
    
    assert!(!response2.success);
    assert!(response2.error_message.is_some());
    assert!(response2.error_message.unwrap().contains("already exists"));
    
    Ok(())
}

#[tokio::test]
async fn test_delete_nonexistent_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Try to delete non-existent collection
    let response = service.delete_collection("does_not_exist").await?;
    
    assert!(!response.success);
    assert!(response.error_message.is_some());
    assert!(response.error_message.unwrap().contains("not found"));
    
    Ok(())
}

#[tokio::test] 
async fn test_collection_with_filterable_fields() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    let filterable_fields = vec![
        "category".to_string(),
        "priority".to_string(), 
        "author".to_string(),
        "timestamp".to_string(),
    ];
    
    let response = service.create_collection(
        "test_filterable".to_string(),
        768,
        DistanceMetric::Cosine,
        StorageEngineType::Viper,
        IndexingAlgorithm::Hnsw,
        filterable_fields.clone(),
        serde_json::json!({
            "description": "Collection with filterable metadata"
        }),
    ).await?;
    
    assert!(response.success);
    
    let collection = response.collection.unwrap();
    assert_eq!(collection.filterable_metadata_fields, filterable_fields);
    
    Ok(())
}

#[tokio::test]
async fn test_collection_service_builder() -> Result<()> {
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
        ..Default::default()
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(metadata_config, filesystem).await?
    );
    
    // Use builder pattern
    let service = CollectionServiceBuilder::new()
        .with_metadata_backend(metadata_backend)
        .build()
        .await?;
    
    // Test basic operation
    let response = service.create_collection(
        "builder_test".to_string(),
        128,
        DistanceMetric::Cosine,
        StorageEngineType::Viper,
        IndexingAlgorithm::Hnsw,
        vec![],
        serde_json::json!({}),
    ).await?;
    
    assert!(response.success);
    
    Ok(())
}