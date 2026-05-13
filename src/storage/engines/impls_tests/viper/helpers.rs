//! Consolidated test helpers for VIPER storage engine tests
//!
//! This module provides reusable helper functions for VIPER engine testing,
//! consolidating common patterns from across 146 tests in 20 files.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, SqlValue, StorageAssignment, VectorRecord,
};
use crate::storage::engines::viper::{ViperEngine, ViperEngineConfig};
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::storage::traits::UnifiedStorageEngine;
use crate::utils::StoragePath;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

// ============================================================================
// Engine Creation Utilities
// ============================================================================

/// Create a VIPER engine with default test configuration
///
/// # Returns
/// A tuple containing (ViperEngine, TempDir) - the tempdir must be kept alive
/// for the duration of the test.
///
/// # Example
/// ```ignore
/// let (engine, _temp_dir) = create_test_viper_engine().await?;
/// assert_eq!(engine.engine_name(), "VIPER");
/// ```
pub async fn create_test_viper_engine() -> Result<(ViperEngine, TempDir)> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_string_lossy().to_string();

    let fs_config = FilesystemConfig {
        default_fs: Some(format!("file://{}", temp_path)),
        ..Default::default()
    };

    let filesystem = Arc::new(FilesystemFactory::create(fs_config).await?);
    let viper_engine =
        ViperEngine::from_core_config(crate::core::config::ViperConfig::default(), filesystem)
            .await?;

    Ok((viper_engine, temp_dir))
}

/// Create a VIPER engine with custom configuration
///
/// # Arguments
/// * `base_path` - Base path for engine storage
/// * `config` - Custom ViperEngineConfig
///
/// # Example
/// ```ignore
/// let mut config = ViperEngineConfig::default();
/// config.flush_size_bytes = Some(512 * 1024);
/// let engine = create_viper_engine_with_config("/tmp/test", config).await?;
/// ```
pub async fn create_viper_engine_with_config(
    base_path: &str,
    _config: ViperEngineConfig,
) -> Result<ViperEngine> {
    let mut fs_config = FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", base_path));

    let filesystem_factory = Arc::new(FilesystemFactory::create(fs_config).await?);
    ViperEngine::from_core_config(
        crate::core::config::ViperConfig::default(),
        filesystem_factory,
    )
    .await
}

/// Create a VIPER engine configuration for compaction testing
///
/// Sets small flush sizes and row group sizes for faster test execution.
pub fn create_compaction_test_config(_base_path: &str) -> ViperEngineConfig {
    let mut config = ViperEngineConfig::default();
    config.enable_ml_clustering = false;
    config.flush_size_bytes = Some(512 * 1024); // 512KB for faster testing
    config.row_group_size = 100; // Small row groups for testing
    config
}

/// Create a default test configuration for basic VIPER operations
pub fn create_default_test_config(_base_path: &str) -> ViperEngineConfig {
    let mut config = ViperEngineConfig::default();
    config.enable_ml_clustering = false;
    config.flush_size_bytes = Some(1024 * 1024); // 1MB flush size
    config
}

// ============================================================================
// Collection Configuration Utilities
// ============================================================================

/// Create a test collection with storage assignment
///
/// # Arguments
/// * `collection_id` - Unique identifier for the collection
/// * `base_path` - Base storage path
///
/// # Example
/// ```ignore
/// let collection = create_test_collection("my_collection", "/tmp/test");
/// assert_eq!(collection.id, "my_collection");
/// ```
pub fn create_test_collection(collection_id: &str, base_path: &str) -> Collection {
    Collection {
        id: collection_id.to_string(),
        config: Some(CollectionConfig {
            name: collection_id.to_string(),
            dimension: 128,
            distance_metric: Some(0), // Cosine
            storage_engine: Some(1),  // VIPER
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            storage_config: None,
            primary_index: Some(String::new()),
            auto_index_selection: Some(false),
            description: None,
            tags: vec![],
            owner: None,
            embedding_models: vec![],
            record_schema: None,
            enable_proxima_record: None,
            text_columns: vec![],
            text_storage_configs: vec![],
        }),
        stats: None,
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
        storage_assignment: Some(StorageAssignment {
            primary_path: format!("file://{}", base_path),
            backup_paths: vec![],
            engine: 1,
            engine_config: HashMap::new(),
            base_location: format!("file://{}", base_path),
            assigned_at: chrono::Utc::now().timestamp(),
        }),
    }
}

/// Create a test collection with custom dimension
///
/// # Arguments
/// * `collection_id` - Unique identifier for the collection
/// * `dimension` - Vector dimension size
///
/// # Example
/// ```ignore
/// let collection = create_test_collection_with_dimension("test_col", "/tmp", 256);
/// assert_eq!(collection.config.unwrap().dimension, 256);
/// ```
pub fn create_test_collection_with_dimension(
    collection_id: &str,
    base_path: &str,
    dimension: usize,
) -> Collection {
    let mut collection = create_test_collection(collection_id, base_path);
    if let Some(ref mut config) = collection.config {
        config.dimension = dimension as u32;
    }
    collection
}

/// Create a test collection configuration (proto type)
///
/// Returns the proto Collection type for use in unified storage operations.
pub fn create_test_collection_config(collection_id: &str, dimension: usize) -> Collection {
    Collection {
        id: collection_id.to_string(),
        config: Some(CollectionConfig {
            name: collection_id.to_string(),
            dimension: dimension as u32,
            distance_metric: Some(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32),
            storage_engine: Some(crate::proto::proximadb_v1::StorageEngine::Viper as i32),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ============================================================================
// Setup and Teardown Utilities
// ============================================================================

/// Set up storage assignment for a test collection
///
/// Creates necessary directory structure including data, temp, and WAL directories.
///
/// # Arguments
/// * `collection_id` - Collection identifier
/// * `base_path` - Base storage path
///
/// # Example
/// ```ignore
/// setup_test_assignment("my_collection", "/tmp/test").await?;
/// // Directories are now ready for testing
/// ```
pub async fn setup_test_assignment(collection_id: &str, base_path: &str) -> Result<()> {
    use tokio::fs;

    // Create necessary directories
    let data_dir = StoragePath::collection_data_path(base_path, collection_id);
    fs::create_dir_all(&data_dir).await?;

    // Create temp directory for atomic writes
    let temp_dir = StoragePath::data_file_path(base_path, collection_id, "___temp");
    fs::create_dir_all(&temp_dir).await?;

    // Create WAL directory
    let wal_dir = format!("{}/{}/write_buffer", base_path, collection_id);
    fs::create_dir_all(&wal_dir).await?;

    Ok(())
}

// ============================================================================
// Test Data Generation Utilities
// ============================================================================

/// Create a basic test vector with default metadata
///
/// # Arguments
/// * `id` - Vector identifier
/// * `dimension` - Vector dimension
/// * `value` - Fill value for all dimensions
///
/// # Example
/// ```ignore
/// let vector = create_test_vector("vec_1", 128, 0.5);
/// assert_eq!(vector.id, "vec_1");
/// assert_eq!(vector.vector.len(), 128);
/// ```
pub fn create_test_vector(id: &str, dimension: usize, value: f32) -> VectorRecord {
    let mut metadata = HashMap::new();
    metadata.insert(
        "category".to_string(),
        SqlValue {
            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                format!("cat_{}", (value * 10.0) as i32 % 5),
            )),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: vec![value; dimension],
        metadata,
        timestamp: Some(chrono::Utc::now().timestamp()),
        updated_at: Some(chrono::Utc::now().timestamp()),
        expires_at: None,
        version: Some(1),
        source: None,
    }
}

/// Create a test vector with sequential values
///
/// Useful for testing vector similarity and distance calculations.
///
/// # Arguments
/// * `id` - Vector identifier
/// * `dimension` - Vector dimension
///
/// # Example
/// ```ignore
/// let vector = create_sequential_test_vector("vec_1", 128);
/// assert_eq!(vector.vector[0], 0.0);
/// assert_eq!(vector.vector[127], 127.0 / 128.0);
/// ```
pub fn create_sequential_test_vector(id: &str, dimension: usize) -> VectorRecord {
    let mut metadata = HashMap::new();
    metadata.insert(
        "test_key".to_string(),
        SqlValue {
            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                "test_value".to_string(),
            )),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: (0..dimension)
            .map(|i| (i as f32) / (dimension as f32))
            .collect(),
        metadata,
        timestamp: Some(chrono::Utc::now().timestamp()),
        updated_at: Some(chrono::Utc::now().timestamp()),
        expires_at: None,
        version: Some(1),
        source: Some("test".to_string()),
    }
}

/// Create multiple test vector records
///
/// Generates a batch of test vectors for bulk operations.
///
/// # Arguments
/// * `collection_id` - Collection identifier (used in metadata)
/// * `count` - Number of vectors to generate
///
/// # Example
/// ```ignore
/// let vectors = create_test_vector_records("test_col", 100);
/// assert_eq!(vectors.len(), 100);
/// ```
pub fn create_test_vector_records(_collection_id: &str, count: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let now = chrono::Utc::now().timestamp_millis();
            VectorRecord {
                id: format!("test_vector_{}", i),
                vector: vec![0.1 * i as f32, 0.2 * i as f32, 0.3 * i as f32],
                metadata: {
                    let mut metadata = HashMap::new();
                    metadata.insert(
                        "category".to_string(),
                        SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                format!("category_{}", i % 3),
                            )),
                        },
                    );
                    metadata.insert(
                        "priority".to_string(),
                        SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                i.to_string(),
                            )),
                        },
                    );
                    metadata
                },
                timestamp: Some(now),
                updated_at: Some(now),
                expires_at: None,
                version: Some(1),
                source: None,
            }
        })
        .collect()
}

/// Create test vectors with custom metadata
///
/// # Arguments
/// * `id` - Vector identifier
/// * `dimension` - Vector dimension
/// * `metadata_key` - Metadata field name
/// * `metadata_value` - Metadata field value
///
/// # Example
/// ```ignore
/// let vector = create_test_vector_with_metadata("vec_1", 128, "status", "active");
/// assert!(vector.metadata.contains_key("status"));
/// ```
pub fn create_test_vector_with_metadata(
    id: &str,
    dimension: usize,
    metadata_key: &str,
    metadata_value: &str,
) -> VectorRecord {
    let mut metadata = HashMap::new();
    metadata.insert(
        metadata_key.to_string(),
        SqlValue {
            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                metadata_value.to_string(),
            )),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: (0..dimension)
            .map(|i| (i as f32) / (dimension as f32))
            .collect(),
        metadata,
        timestamp: Some(chrono::Utc::now().timestamp()),
        updated_at: Some(chrono::Utc::now().timestamp()),
        expires_at: None,
        version: Some(1),
        source: None,
    }
}

// ============================================================================
// Search and Query Utilities
// ============================================================================

/// Convert SearchParams to SearchPlan for unified search interface
///
/// # Arguments
/// * `params` - Search parameters
/// * `collection_id` - Collection identifier
///
/// # Example
/// ```ignore
/// use crate::core::search::SearchParams;
/// let params = SearchParams {
///     vector: Some(vec![0.5; 128]),
///     top_k: Some(10),
///     ..Default::default()
/// };
/// let plan = convert_search_params_to_plan(&params, "my_collection");
/// ```
pub fn convert_search_params_to_plan(
    params: &crate::core::search::SearchParams,
    collection_id: &str,
) -> crate::core::search::unified_interface::SearchPlan {
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::search::unified_interface::{CollectionConfig, SearchPlan, StorageInfo};

    SearchPlan {
        collection_id: collection_id.to_string(),
        collection_config: Some(CollectionConfig {
            default_distance_metric: params.distance_metric.unwrap_or(DistanceMetric::Cosine),
            vector_dimension: 128,
            enable_quantization: false,
            enable_metadata_filtering: params.filter_expression.is_some(),
            estimated_document_count: 1000,
        }),
        filterable_columns: vec![],
        available_quantization: vec![],
        storage_info: StorageInfo {
            is_cloud_storage: false,
            storage_type: "Local".to_string(),
            estimated_size_mb: 1.0,
            file_count: 1,
            supports_range_requests: false,
            file_paths: None,
        },
        filter_expression: params.filter_expression.clone(),
        query_vector: params.vector.clone(),
        top_k: params.top_k.unwrap_or(10),
        min_score: None,
        enable_early_termination: true,
    }
}

/// Create a collection context for parquet reader testing
///
/// # Example
/// ```ignore
/// let context = create_test_collection_context();
/// assert_eq!(context.collection_id, "test_collection");
/// ```
pub fn create_test_collection_context()
-> crate::storage::engines::core::formats::columnar::CollectionContext {
    use crate::storage::engines::core::formats::columnar::CollectionContext;

    CollectionContext {
        collection_id: "test_collection".to_string(),
        dimension: 128,
        distance_metric: "cosine".to_string(),
        quantization_config: None,
    }
}

// ============================================================================
// Parquet and Columnar Utilities
// ============================================================================

/// Create a UnifiedParquetReader for testing
///
/// # Arguments
/// * `file_paths` - List of parquet file paths
///
/// # Example
/// ```ignore
/// let reader = create_test_parquet_reader(vec!["file:///tmp/test.parquet".to_string()]).await?;
/// ```
pub async fn create_test_parquet_reader(
    file_paths: Vec<String>,
) -> Result<crate::storage::engines::core::formats::columnar::UnifiedParquetReader> {
    use crate::storage::engines::core::formats::columnar::UnifiedParquetReader;

    let fs_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::create(fs_config).await?);
    let base_fs = filesystem_factory.get_filesystem("file://")?;
    let cached_filesystem = Arc::new(
        crate::storage::persistence::filesystem::unified_filesystem::UnifiedCachingFilesystem::new(
            base_fs,
            "test_collection".to_string(),
            "viper".to_string(),
        ),
    );

    UnifiedParquetReader::new(
        file_paths,
        128,
        filesystem_factory,
        cached_filesystem,
        "test_collection".to_string(),
        "viper".to_string(),
    )
}

/// Debug parquet file contents
///
/// Reads and logs the contents of a parquet file for debugging purposes.
///
/// # Arguments
/// * `file_path` - Path to parquet file
/// * `label` - Label for debug output
///
/// # Example
/// ```ignore
/// debug_parquet_file("file:///tmp/test.parquet", "Test File").await?;
/// ```
pub async fn debug_parquet_file(file_path: &str, label: &str) -> Result<()> {
    use tracing::debug;

    debug!("\n=\r DEBUG: {} - Reading {}", label, file_path);

    let fs = FilesystemFactory::create(Default::default()).await?;
    let filesystem = fs.get_filesystem(file_path)?;

    match filesystem.read(file_path).await {
        Ok(data) => {
            debug!("   File exists, size: {} bytes", data.len());

            match ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(data)) {
                Ok(builder) => {
                    debug!("  == Schema: {:?}", builder.schema());

                    let reader = builder.build()?;
                    let mut total_rows = 0;
                    let mut batch_count = 0;

                    for batch_result in reader {
                        match batch_result {
                            Ok(batch) => {
                                batch_count += 1;
                                let rows = batch.num_rows();
                                total_rows += rows;
                                debug!("  == Batch {}: {} rows", batch_count, rows);
                            }
                            Err(e) => {
                                debug!("  L Error reading batch: {}", e);
                            }
                        }
                    }

                    debug!("  == Total: {} batches, {} rows", batch_count, total_rows);

                    if total_rows == 0 {
                        debug!("  WARNING: File contains NO DATA!");
                    }
                }
                Err(e) => {
                    debug!("  L Failed to create parquet reader: {}", e);
                }
            }
        }
        Err(e) => {
            debug!("  L Failed to read file: {}", e);
        }
    }

    Ok(())
}

// ============================================================================
// Filesystem Utilities
// ============================================================================

/// Create a test filesystem factory
///
/// # Example
/// ```ignore
/// let filesystem = create_test_filesystem().await?;
/// ```
pub async fn create_test_filesystem() -> Result<Arc<FilesystemFactory>> {
    let config = FilesystemConfig::default();
    Ok(Arc::new(FilesystemFactory::create(config).await?))
}

/// Create a test filesystem factory with custom base path
///
/// # Arguments
/// * `base_path` - Base storage path
///
/// # Example
/// ```ignore
/// let filesystem = create_test_filesystem_with_path("/tmp/test").await?;
/// ```
pub async fn create_test_filesystem_with_path(base_path: &str) -> Result<Arc<FilesystemFactory>> {
    let mut config = FilesystemConfig::default();
    config.default_fs = Some(format!("file://{}", base_path));
    Ok(Arc::new(FilesystemFactory::create(config).await?))
}

// ============================================================================
// Configuration Helpers
// ============================================================================

/// Create default VIPER pipeline config for testing
///
/// # Example
/// ```ignore
/// let config = create_default_pipeline_config();
/// assert!(config.processing_config.enable_preprocessing);
/// ```
pub fn create_default_pipeline_config()
-> crate::storage::engines::viper::pipeline::ViperPipelineConfig {
    use crate::storage::engines::viper::pipeline::*;

    ViperPipelineConfig {
        processing_config: ProcessingConfig {
            enable_preprocessing: true,
            enable_postprocessing: true,
            batch_size: 100,
            compression: true,
            sorting_strategy: SortingStrategy::ByTimestamp,
            quantization_level: None,
        },
        flushing_config: FlushingConfig {
            compression_algorithm: CompressionAlgorithm::Snappy,
            compression_level: 6,
            enable_dictionary_encoding: true,
            row_group_size: 1000,
            write_batch_size: 1000,
            enable_statistics: true,
        },
        compaction_config: CompactionConfig {
            enable_ml_compaction: false,
            worker_count: 2,
            compaction_interval_secs: 300,
            target_file_size_mb: 100,
            max_files_per_merge: 10,
            reclustering_quality_threshold: 0.8,
        },
        enable_background_processing: true,
        stats_interval_secs: 30,
    }
}

// ============================================================================
// Assertion Helpers
// ============================================================================

/// Assert that a search result contains expected vector IDs
///
/// # Arguments
/// * `results` - Search results
/// * `expected_ids` - Expected vector IDs
///
/// # Example
/// ```ignore
/// assert_search_contains_ids(&results, &["vec_1", "vec_2"]);
/// ```
pub fn assert_search_contains_ids(
    results: &[crate::core::search::results::OptimizedSearchRecord],
    expected_ids: &[&str],
) {
    let result_ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();

    for expected_id in expected_ids {
        assert!(
            result_ids.contains(expected_id),
            "Expected ID '{}' not found in search results. Found: {:?}",
            expected_id,
            result_ids
        );
    }
}

/// Assert that a collection directory structure is valid
///
/// # Arguments
/// * `base_path` - Base storage path
/// * `collection_id` - Collection identifier
///
/// # Example
/// ```ignore
/// assert_collection_directory_structure("/tmp/test", "my_collection").await?;
/// ```
pub async fn assert_collection_directory_structure(
    base_path: &str,
    collection_id: &str,
) -> Result<()> {
    use tokio::fs;

    let data_dir = StoragePath::collection_data_path(base_path, collection_id);
    assert!(
        fs::metadata(&data_dir).await.is_ok(),
        "Data directory should exist: {}",
        data_dir
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_test_viper_engine() {
        let result = create_test_viper_engine().await;
        assert!(result.is_ok());

        let (engine, _temp_dir) = result.unwrap();
        assert_eq!(engine.engine_name(), "VIPER");
    }

    #[test]
    fn test_create_test_collection() {
        let collection = create_test_collection("test_col", "/tmp/test");
        assert_eq!(collection.id, "test_col");
        assert!(collection.config.is_some());
    }

    #[test]
    fn test_create_test_vector() {
        let vector = create_test_vector("vec_1", 128, 0.5);
        assert_eq!(vector.id, "vec_1");
        assert_eq!(vector.vector.len(), 128);
        assert_eq!(vector.vector[0], 0.5);
    }

    #[test]
    fn test_create_sequential_test_vector() {
        let vector = create_sequential_test_vector("vec_1", 128);
        assert_eq!(vector.id, "vec_1");
        assert_eq!(vector.vector.len(), 128);
        assert_eq!(vector.vector[0], 0.0);
        assert_eq!(vector.vector[127], 127.0 / 128.0);
    }

    #[test]
    fn test_create_test_vector_records() {
        let vectors = create_test_vector_records("test_col", 10);
        assert_eq!(vectors.len(), 10);

        for (i, vector) in vectors.iter().enumerate() {
            assert_eq!(vector.id, format!("test_vector_{}", i));
        }
    }

    #[test]
    fn test_create_default_pipeline_config() {
        let config = create_default_pipeline_config();
        assert!(config.processing_config.enable_preprocessing);
        assert!(config.processing_config.enable_postprocessing);
        assert_eq!(config.processing_config.batch_size, 100);
    }
}
