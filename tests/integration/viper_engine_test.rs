//! Unit tests for VIPER engine
//!
//! Tests the main VIPER engine coordination and integration functionality including:
//! - Engine initialization and configuration
//! - Collection metadata management
//! - Parquet file discovery
//! - Search method integration

use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::test;

use proximadb::storage::engines::viper::types::{
    CollectionMetadata, CompressionStats, PartitionStrategy,
};
use proximadb::storage::engines::viper::ViperEngine;
use proximadb::core::ViperConfig;
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

/// Test helper to create a temporary directory
fn create_temp_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp directory")
}

#[test]
async fn test_viper_engine_initialization() {
    let temp_dir = create_temp_dir();
    let filesystem = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("Failed to create filesystem factory"),
    );

    let config = ViperConfig::default();
    let engine = ViperEngine::new()
        .await
        .expect("Failed to create VIPER engine");

    // Test that engine is properly initialized
    assert_eq!(engine.get_config().enable_ml_clustering, true);
    assert_eq!(engine.get_config().enable_background_compaction, true);
}

#[test]
async fn test_collection_metadata_management() {
    let temp_dir = create_temp_dir();
    let filesystem = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("Failed to create filesystem factory"),
    );

    let config = ViperConfig::default();
    let engine = ViperEngine::new()
        .await
        .expect("Failed to create VIPER engine");

    let collection_id = "test_collection".to_string();

    // Test that initially no metadata exists
    let metadata = engine.get_collection_metadata(&collection_id).await;
    assert!(metadata.is_none());

    // Create and update metadata
    let test_metadata = CollectionMetadata {
        collection_id: collection_id.clone(),
        vector_dimensions: 256,
        distance_metric: "euclidean".to_string(),
        created_at: std::time::SystemTime::now(),
        updated_at: std::time::SystemTime::now(),
        total_vectors: 50_000,
        total_size_bytes: 25_000_000,
        active_clusters: vec!["cluster_1".to_string(), "cluster_2".to_string()],
        quantization_enabled: true,
        quantization: None,
        partition_strategy: PartitionStrategy::ByCluster,
        compression_stats: CompressionStats::default(),
        filterable_columns: Vec::new(),
        schema_version: Some(1),
        flush_size_bytes: Some(32 * 1024 * 1024),
    };

    engine
        .update_collection_metadata(collection_id.clone(), test_metadata.clone())
        .await;

    // Test that metadata was stored
    let retrieved_metadata = engine.get_collection_metadata(&collection_id).await;
    assert!(retrieved_metadata.is_some());
    let retrieved = retrieved_metadata.unwrap();
    assert_eq!(retrieved.collection_id, collection_id);
    assert_eq!(retrieved.vector_dimensions, 256);
    assert_eq!(retrieved.distance_metric, "euclidean");
    assert_eq!(retrieved.total_vectors, 50_000);
    assert_eq!(retrieved.active_clusters.len(), 2);
}

#[test]
async fn test_parquet_file_discovery() {
    let temp_dir = create_temp_dir();
    let filesystem = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("Failed to create filesystem factory"),
    );

    let config = ViperConfig::default();
    let engine = ViperEngine::new()
        .await
        .expect("Failed to create VIPER engine");

    let collection_id = "file_discovery_test".to_string();

    // Test Parquet file discovery
    let parquet_files = engine
        .get_parquet_files_for_collection(&collection_id)
        .await
        .expect("Failed to get Parquet files");

    // Should return mock files for now
    assert!(parquet_files.len() > 0);
    for file in &parquet_files {
        assert!(file.contains(&collection_id.to_string()));
        assert!(file.ends_with(".parquet"));
    }
}

#[test]
async fn test_cluster_prediction() {
    let temp_dir = create_temp_dir();
    let filesystem = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("Failed to create filesystem factory"),
    );

    let config = ViperConfig::default();
    let engine = ViperEngine::new()
        .await
        .expect("Failed to create VIPER engine");

    let collection_id = "cluster_prediction_test".to_string();
    let test_vector = vec![1.0, 2.0, 3.0, 4.0];

    // Test cluster prediction
    let cluster_prediction = engine
        .predict_cluster(&collection_id, &test_vector)
        .await
        .expect("Failed to predict cluster");

    // Should return None for now (not implemented)
    assert!(cluster_prediction.is_none());
}

#[test]
async fn test_search_vectors_basic() {
    let temp_dir = create_temp_dir();
    let filesystem = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("Failed to create filesystem factory"),
    );

    let config = ViperConfig::default();
    let engine = ViperEngine::new()
        .await
        .expect("Failed to create VIPER engine");

    let collection_id = "search_test_collection";
    let query_vector = vec![1.0, 2.0, 3.0, 4.0];
    let k = 10;

    // Test basic search functionality
    let search_results = engine
        .search_vectors(collection_id, &query_vector, k)
        .await
        .expect("Failed to search vectors");

    // Should return empty results for now (mock implementation)
    assert_eq!(search_results.len(), 0);
}

#[test]
async fn test_search_vectors_in_cluster() {
    let temp_dir = create_temp_dir();
    let filesystem = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("Failed to create filesystem factory"),
    );

    let config = ViperConfig::default();
    let engine = ViperEngine::new()
        .await
        .expect("Failed to create VIPER engine");

    let collection_id = "cluster_search_test";
    let query_vector = vec![1.0, 2.0, 3.0, 4.0];
    let k = 5;
    let cluster_id = "cluster_1";

    // Test cluster-specific search
    let cluster_results = engine
        .search_vectors_in_cluster(collection_id, &query_vector, k, cluster_id)
        .await
        .expect("Failed to search in cluster");

    // Should return empty results for now (mock implementation)
    assert_eq!(cluster_results.len(), 0);
}

#[test]
async fn test_engine_configuration_access() {
    let temp_dir = create_temp_dir();
    let filesystem = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("Failed to create filesystem factory"),
    );

    let mut config = ViperConfig::default();
    config.enable_ml_clustering = false;
    config.enable_quantization = false;
    config.initial_cluster_count = 5;

    let engine = ViperEngine::new()
        .await
        .expect("Failed to create VIPER engine");

    // Test configuration access
    let retrieved_config = engine.get_config();
    assert!(!retrieved_config.enable_ml_clustering);
    assert!(!retrieved_config.enable_quantization);
    assert_eq!(retrieved_config.initial_cluster_count, 5);
}

#[test]
async fn test_engine_default_creation() {
    // Test that the default engine can be created
    let engine = ViperEngine::default();

    // Test that default configuration is applied
    let config = engine.get_config();
    assert!(config.enable_ml_clustering);
    assert!(config.enable_background_compaction);
    assert_eq!(config.initial_cluster_count, 10);
}

#[test]
async fn test_multiple_collections() {
    let temp_dir = create_temp_dir();
    let filesystem = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("Failed to create filesystem factory"),
    );

    let config = ViperConfig::default();
    let engine = ViperEngine::new()
        .await
        .expect("Failed to create VIPER engine");

    let collection_1 = "collection_1".to_string();
    let collection_2 = "collection_2".to_string();

    // Create metadata for both collections
    let metadata_1 = CollectionMetadata {
        collection_id: collection_1.clone(),
        vector_dimensions: 128,
        distance_metric: "cosine".to_string(),
        created_at: std::time::SystemTime::now(),
        updated_at: std::time::SystemTime::now(),
        total_vectors: 10_000,
        total_size_bytes: 5_000_000,
        active_clusters: vec!["cluster_a".to_string()],
        quantization_enabled: false,
        quantization: None,
        partition_strategy: PartitionStrategy::ByTimestamp,
        compression_stats: CompressionStats::default(),
        filterable_columns: Vec::new(),
        schema_version: Some(1),
        flush_size_bytes: Some(16 * 1024 * 1024),
    };

    let metadata_2 = CollectionMetadata {
        collection_id: collection_2.clone(),
        vector_dimensions: 256,
        distance_metric: "euclidean".to_string(),
        created_at: std::time::SystemTime::now(),
        updated_at: std::time::SystemTime::now(),
        total_vectors: 20_000,
        total_size_bytes: 10_000_000,
        active_clusters: vec!["cluster_b".to_string(), "cluster_c".to_string()],
        quantization_enabled: true,
        quantization: None,
        partition_strategy: PartitionStrategy::ByCluster,
        compression_stats: CompressionStats::default(),
        filterable_columns: Vec::new(),
        schema_version: Some(1),
        flush_size_bytes: Some(32 * 1024 * 1024),
    };

    // Update metadata for both collections
    engine
        .update_collection_metadata(collection_1.clone(), metadata_1)
        .await;
    engine
        .update_collection_metadata(collection_2.clone(), metadata_2)
        .await;

    // Test that both collections are accessible
    let retrieved_1 = engine.get_collection_metadata(&collection_1).await.unwrap();
    let retrieved_2 = engine.get_collection_metadata(&collection_2).await.unwrap();

    assert_eq!(retrieved_1.vector_dimensions, 128);
    assert_eq!(retrieved_1.distance_metric, "cosine");
    assert_eq!(retrieved_1.active_clusters.len(), 1);

    assert_eq!(retrieved_2.vector_dimensions, 256);
    assert_eq!(retrieved_2.distance_metric, "euclidean");
    assert_eq!(retrieved_2.active_clusters.len(), 2);
}
