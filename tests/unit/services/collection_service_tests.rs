//! Unit tests for CollectionService

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;

use proximadb::services::collection_service::CollectionService;
use proximadb::storage::metadata::backends::filestore_backend::{
    FilestoreMetadataBackend, FilestoreMetadataConfig,
};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::proto::proximadb::{DistanceMetric, IndexingAlgorithm, CollectionConfig, StorageEngine};
use proximadb::core::config::StorageConfig;

/// Create test collection service
async fn create_test_service() -> Result<(Arc<CollectionService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    
    // Create filesystem
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(
        FilesystemFactory::new(fs_config).await?
    );
    
    // Create metadata backend
    let metadata_config = FilestoreMetadataConfig {
        storage_url: format!("file://{}/metadata", temp_dir.path().display()),
        enable_compression: true,
        enable_snapshots: true,
        snapshot_threshold: 100,
        keep_snapshots: 3,
        backup_url: None,
        temp_dir: None,
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(metadata_config, filesystem).await?
    );
    
    // Create collection service with storage config
    let storage_config = StorageConfig::default();
    let service = Arc::new(CollectionService::new(metadata_backend, storage_config).await?);
    
    Ok((service, temp_dir))
}

#[tokio::test]
async fn test_create_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create collection config
    let config = CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 384,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Viper as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        filterable_columns: vec![],
        index_configs: vec![],
        quantization_config: None,
        primary_index_name: "default".to_string(),
        enable_automatic_index_selection: false,
        description: Some("Test collection".to_string()),
        tags: vec!["test".to_string()],
        owner: Some("test_user".to_string()),
        compression: None,
        optimization_hints: None,
        storage_location: None,
    };
    
    let response = service.create_collection(&config).await?;
    
    assert!(response.success);
    assert!(response.storage_path.is_some());
    
    Ok(())
}

#[tokio::test]
async fn test_get_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create a collection first
    let config = CollectionConfig {
        name: "test_get".to_string(),
        dimension: 256,
        distance_metric: DistanceMetric::Euclidean as i32,
        storage_engine: StorageEngine::Sst as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Flat as i32,
        filterable_columns: vec![],
        index_configs: vec![],
        quantization_config: None,
        primary_index_name: "default".to_string(),
        enable_automatic_index_selection: false,
        description: None,
        tags: vec![],
        owner: None,
        compression: None,
        optimization_hints: None,
        storage_location: None,
    };
    
    let create_response = service.create_collection(&config).await?;
    assert!(create_response.success);
    
    // Get by name
    let collection = service.get_proto_collection("test_get").await?;
    assert!(collection.is_some());
    
    let collection = collection.unwrap();
    assert_eq!(collection.config.as_ref().unwrap().name, "test_get");
    
    Ok(())
}

#[tokio::test]
async fn test_list_collections() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create multiple collections
    for i in 0..3 {
        let config = CollectionConfig {
            name: format!("collection_{}", i),
            compression: None,
            optimization_hints: None,
            storage_location: None,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Viper as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "default".to_string(),
            enable_automatic_index_selection: false,
            description: None,
            tags: vec![],
            owner: None,
        };
        
        let response = service.create_collection(&config).await?;
        assert!(response.success);
    }
    
    // List all collections
    let collections = service.list_collections().await?;
    assert_eq!(collections.len(), 3);
    
    Ok(())
}

#[tokio::test]
async fn test_delete_collection() -> Result<()> {
    let (service, _temp_dir) = create_test_service().await?;
    
    // Create a collection
    let config = CollectionConfig {
        name: "test_delete".to_string(),
        dimension: 64,
        distance_metric: DistanceMetric::Manhattan as i32,
        storage_engine: StorageEngine::Sst as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Flat as i32,
        filterable_columns: vec![],
        index_configs: vec![],
        quantization_config: None,
        primary_index_name: "default".to_string(),
        enable_automatic_index_selection: false,
        description: None,
        tags: vec![],
        owner: None,
        compression: None,
        optimization_hints: None,
        storage_location: None,
    };
    
    let create_response = service.create_collection(&config).await?;
    assert!(create_response.success);
    
    // Delete the collection
    let delete_response = service.delete_collection("test_delete").await?;
    assert!(delete_response.success);
    
    // Verify it's gone
    let collection = service.get_proto_collection("test_delete").await?;
    assert!(collection.is_none());
    
    Ok(())
}