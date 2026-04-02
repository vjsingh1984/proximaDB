/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! SST Engine Test Helpers
//!
//! Consolidated test helper functions for SST engine tests.
//! This module provides reusable utilities for:
//! - Engine creation
//! - Record creation
//! - Configuration setup
//! - Test data generation
//! - Filesystem setup
//! - Search helpers

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::unified::{InMemoryCodebookStore, UnifiedQuantizationEngine};
use crate::core::SstConfig;
use crate::core::search::results::OptimizedSearchRecord;
use crate::proto::proximadb_v1::{Collection, CollectionConfig, MetadataItem, VectorRecord};
use crate::storage::engines::impls::sst::readers::sst_query_engine::CollectionContext;
use crate::storage::engines::impls::sst::{SstEngine, SstRecord, UnifiedSstableReader};
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory, UnifiedCachingFilesystem};
// manifest module is not exposed - tests may need refactoring
// use crate::storage::engines::impls::sst::manifest::SstManifest;
use crate::core::BloomFilterConfig;
use crate::core::search::{
    CollectionConfig as SearchCollectionConfig, FilterableColumn, SearchParams, SearchPlan,
    StorageInfo,
};

// ============================================================================
// Engine Creation Utilities
// ============================================================================

/// Create a test SST engine with default configuration
///
/// # Returns
/// A fully initialized SST engine ready for testing
pub async fn create_test_engine() -> SstEngine {
    let config = SstConfig::default();
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    SstEngine::new_with_config(config, filesystem, distance_compute)
        .await
        .unwrap()
}

/// Create a test SST engine with custom configuration
///
/// # Arguments
/// * `config` - Custom SST configuration
///
/// # Returns
/// A fully initialized SST engine with the provided configuration
pub async fn create_test_engine_with_config(config: SstConfig) -> SstEngine {
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    SstEngine::new_with_config(config, filesystem, distance_compute)
        .await
        .unwrap()
}

/// Create a test SST engine with custom distance metric
///
/// # Arguments
/// * `metric` - Distance metric to use
///
/// # Returns
/// A fully initialized SST engine with the specified distance metric
pub async fn create_test_engine_with_metric(metric: DistanceMetric) -> SstEngine {
    let config = SstConfig::default();
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(metric));

    SstEngine::new_with_config(config, filesystem, distance_compute)
        .await
        .unwrap()
}

// ============================================================================
// Configuration Utilities
// ============================================================================

/// Create a test SST configuration with common defaults
///
/// # Arguments
/// * `base_path` - Base directory path for data storage
///
/// # Returns
/// SST configuration suitable for testing
pub fn create_test_sst_config(base_path: &str) -> SstConfig {
    SstConfig {
        // Level configuration
        level_count: 4,
        max_levels: 4,
        compaction_threshold: 2,
        compaction_config: None,
        max_files_per_level: 4,
        level_size_multiplier: 4.0,

        // Block and file settings
        block_size_kb: 16384,

        // Storage type
        compaction_strategy: "leveled".to_string(),
        compression: "none".to_string(),
        compression_level: 0,
        vector_encoding_strategy: "FullVector".to_string(),

        // Bloom filter
        bloom_filter_config: Some(BloomFilterConfig {
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        }),
        decompression_cache_config: None,

        // Cache
        cache_size_mb: 32,

        // Background operations
        background_thread_count: 2,

        // Directories
        data_directory: format!("{}/data", base_path),

        // Memory mapping
        mmap_enabled: false,
        prefetch_enabled: false,
        prefetch_size_kb: 0,

        // Block format
        block_format: "ProximaBlocks".to_string(),
    }
}

/// Create a simple test SST configuration
///
/// # Returns
/// Minimal SST configuration for basic tests
pub fn create_test_config() -> SstConfig {
    SstConfig {
        block_size_kb: 64, // Use 64KB blocks for tests
        decompression_cache_config: None,
        ..SstConfig::default()
    }
}

/// Create a test filesystem configuration
///
/// # Returns
/// Default filesystem configuration for testing
pub fn create_test_filesystem_config() -> FilesystemConfig {
    FilesystemConfig::default()
}

// ============================================================================
// Filesystem Setup Utilities
// ============================================================================

/// Create a test filesystem factory
///
/// # Returns
/// FilesystemFactory configured for testing
pub async fn create_test_filesystem() -> Arc<FilesystemFactory> {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap().to_string();

    let mut config = FilesystemConfig::default();
    config.default_fs = Some(format!("file://{}", base_path));

    // Keep temp_dir alive by leaking it for test duration
    std::mem::forget(temp_dir);

    Arc::new(FilesystemFactory::create(config).await.unwrap())
}

/// Setup test directories structure
///
/// # Arguments
/// * `base_path` - Base directory path
///
/// # Returns
/// Result indicating success or failure
pub async fn setup_test_directories(base_path: &Path) -> Result<()> {
    use tokio::fs;
    fs::create_dir_all(base_path).await?;
    fs::create_dir_all(base_path.join("data")).await?;
    fs::create_dir_all(base_path.join("wal")).await?;
    Ok(())
}

// ============================================================================
// Record Creation Utilities
// ============================================================================

/// Create a test SstRecord with default values
///
/// # Arguments
/// * `id` - Record identifier
/// * `vector_dim` - Dimension of the vector
///
/// # Returns
/// SstRecord with populated test data
pub fn create_test_record(id: &str, vector_dim: usize) -> SstRecord {
    use serde_json::json;

    let metadata = json!({
        "test_key": "test_value"
    });

    SstRecord {
        id: id.to_string(),
        vector: Some(vec![1.0; vector_dim]),
        metadata: Some(metadata),
        timestamp: 1000,
        is_tombstone: false,
        sequence_number: 1,
        level: 0,
    }
}

/// Create a test VectorRecord
///
/// # Arguments
/// * `id` - Record identifier
/// * `vector` - Vector data
/// * `timestamp` - Record timestamp
/// * `expires_at` - Optional expiration timestamp
/// * `metadata_items` - Metadata items
///
/// # Returns
/// VectorRecord with the specified data
pub fn create_test_vector_record(
    id: String,
    vector: Vec<f32>,
    timestamp: u32,
    expires_at: Option<u32>,
    metadata_items: Vec<MetadataItem>,
) -> VectorRecord {
    use crate::proto::proximadb_v1::{SqlValue, sql_value};

    // Convert Vec<MetadataItem> to HashMap<String, SqlValue>
    let mut metadata = HashMap::new();
    for item in metadata_items {
        if let Some(value) = item.value {
            let sql_value = match value {
                crate::proto::proximadb_v1::metadata_item::Value::StringValue(s) => SqlValue {
                    value: Some(sql_value::Value::StringValue(s)),
                },
                crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n) => SqlValue {
                    value: Some(sql_value::Value::NumberValue(n)),
                },
                crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b) => SqlValue {
                    value: Some(sql_value::Value::BoolValue(b)),
                },
                _ => continue,
            };
            metadata.insert(item.key, sql_value);
        }
    }

    VectorRecord {
        id,
        vector,
        metadata,
        timestamp: Some(timestamp as i64),
        updated_at: None,
        expires_at: expires_at.map(|t| t as i64),
        version: None,
        ..Default::default()
    }
}

/// Create a simple test vector record with minimal data
///
/// # Arguments
/// * `id` - Record identifier
/// * `dim` - Vector dimension
///
/// # Returns
/// VectorRecord with simple test data
pub fn create_simple_vector_record(id: &str, dim: usize) -> VectorRecord {
    VectorRecord {
        id: id.to_string(),
        vector: vec![0.1; dim],
        metadata: HashMap::new(),
        timestamp: Some(1000),
        updated_at: None,
        expires_at: None,
        version: None,
        ..Default::default()
    }
}

// ============================================================================
// Search Helper Utilities
// ============================================================================

/// Create a test UnifiedSstableReader with local filesystem
///
/// # Returns
/// Configured UnifiedSstableReader for testing
pub async fn create_test_sstable_reader() -> Arc<UnifiedSstableReader> {

    let fs_config = FilesystemConfig::default();
    let fs_factory = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());
    let fs = fs_factory
        .get_filesystem("file:///tmp/proximadb-test")
        .unwrap();

    let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        fs,
        "test_collection".to_string(),
        "sst".to_string(),
    ));

    Arc::new(UnifiedSstableReader::new(
        fs_factory,
        unified_fs,
        "test_collection".to_string(),
    ))
}

/// Create a test UnifiedSstableReader
///
/// # Returns
/// Basic UnifiedSstableReader for testing
pub async fn create_test_reader() -> UnifiedSstableReader {

    let config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(config).await.unwrap());
    let fs = filesystem
        .get_filesystem("file:///tmp/proximadb-test")
        .unwrap();

    let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        fs,
        "test_collection".to_string(),
        "sst".to_string(),
    ));

    UnifiedSstableReader::new(filesystem, unified_fs, "test_collection".to_string())
}

/// Create a test search context
///
/// # Returns
/// SearchPlan with default test values
pub fn create_test_search_context() -> SearchPlan {
    use crate::core::search::unified_interface::ColumnData;

    SearchPlan {
        collection_id: "test_collection".to_string(),
        collection_config: Some(SearchCollectionConfig {
            default_distance_metric: DistanceMetric::Cosine,
            vector_dimension: 128,
            enable_quantization: false,
            enable_metadata_filtering: true,
            estimated_document_count: 10000,
        }),
        filterable_columns: vec![
            FilterableColumn {
                name: "category".to_string(),
                data_type: ColumnData::String,
                is_indexed: true,
                estimated_cardinality: Some(100),
            },
            FilterableColumn {
                name: "score".to_string(),
                data_type: ColumnData::Float,
                is_indexed: false,
                estimated_cardinality: Some(1000),
            },
        ],
        available_quantization: vec![],
        storage_info: StorageInfo {
            is_cloud_storage: false,
            storage_type: "LSM".to_string(),
            estimated_size_mb: 100.0,
            file_count: 5,
            supports_range_requests: true,
            file_paths: None,
        },
        filter_expression: None,
        query_vector: None,
        top_k: 10,
        min_score: None,
        enable_early_termination: false,
    }
}

/// Create test search parameters
///
/// # Returns
/// SearchParams with default values for testing
pub fn create_test_search_params() -> SearchParams {
    SearchParams {
        query_vectors: Some(vec![vec![0.1; 128]]),
        vector: None,
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Cosine),
        filter_expression: None,
        filters: None,
        accuracy_threshold: None,
        include_expired: Some(false),
        timeout_ms: None,
        enable_two_stage: Some(false),
        quantization_hint: None,
        enable_clustering_hint: Some(false),
        runtime_hints: None,
        enable_metadata_filtering_hint: Some(true),
        custom_hints: None,
        requires_ordering: None,
        enable_progressive_search: None,
        progressive_scenario: None,
        progressive_recalls: None,
        optimization_hint: None,
        search_mode: crate::core::search::SearchMode::default(),
        block_prune: crate::core::search::BlockPruneConfig::default(),
        text_query: None,
        hybrid_mode: crate::core::search::HybridSearchMode::default(),
        vector_weight: None,
    }
}

/// Create mock search results for testing
///
/// # Arguments
/// * `count` - Number of mock results to create
///
/// # Returns
/// Vector of mock OptimizedSearchRecord objects
pub fn create_mock_search_results(count: usize) -> Vec<OptimizedSearchRecord> {
    use crate::proto::proximadb_v1::{SqlValue, sql_value};

    (0..count)
        .map(|i| {
            let mut metadata = HashMap::new();
            metadata.insert(
                "category".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::StringValue("test".to_string())),
                },
            );
            metadata.insert(
                "index".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::NumberValue(i as f64)),
                },
            );

            OptimizedSearchRecord::new(format!("result_{}", i), 1.0 - (i as f32 * 0.1))
                .with_similarity(1.0 - (i as f32 * 0.1))
                .add_vector((0..128).map(|j| (i * 128 + j) as f32 / 1000.0).collect())
                .with_metadata(metadata)
                .with_version_info(1, chrono::Utc::now().timestamp())
        })
        .collect()
}

/// Create a test collection context
///
/// # Returns
/// CollectionContext for SSTable reader tests
pub fn create_test_collection_context() -> CollectionContext {
    CollectionContext {
        file_path: "/tmp/lsm".to_string(),
        sstable_files: vec![
            "/tmp/lsm/sst_001.sstable".to_string(),
            "/tmp/lsm/sst_002.sstable".to_string(),
        ],
        total_vectors: 10000,
        metadata_columns: vec!["category".to_string(), "status".to_string()],
        level: 0,
        creation_time: chrono::Utc::now(),
        io_optimization_hints: None,
        collection: None,
    }
}

// ============================================================================
// Manifest and Metadata Utilities
// ============================================================================

/// Create a test SST manifest
///
/// # Returns
/// Tuple of (SstManifest, TempDir) for testing
/// Disabled - SstManifest is not accessible (manifest module not exposed)
// pub async fn create_test_manifest() -> (SstManifest, TempDir) {
//     let temp_dir = TempDir::new().unwrap();
//     let filesystem = Arc::new(
//         FilesystemFactory::new(Default::default()).await.unwrap()
//     );
//
//     let storage_url = format!("file://{}", temp_dir.path().display());
//     let manifest = SstManifest::new(
//         "test_collection".to_string(),
//         storage_url,
//         filesystem,
//         None,
//     );
//
//     (manifest, temp_dir)
// }
// ============================================================================
// Collection and Storage Utilities
// ============================================================================
/// Create a unique collection ID for tests
///
/// # Arguments
/// * `prefix` - Prefix for the collection ID
///
/// # Returns
/// Unique collection identifier
pub fn unique_collection_id(prefix: &str) -> String {
    format!("{}_{}", prefix, crate::utils::uuid::Uuid::new_v4())
}

/// Create a test collection with default configuration
///
/// # Arguments
/// * `collection_id` - Collection identifier
/// * `dimension` - Vector dimension
///
/// # Returns
/// Collection object configured for testing
pub fn create_test_collection(collection_id: &str, dimension: u32) -> Collection {
    Collection {
        id: collection_id.to_string(),
        config: Some(CollectionConfig {
            name: collection_id.to_string(),
            dimension,
            distance_metric: Some(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32),
            storage_engine: Some(crate::proto::proximadb_v1::StorageEngine::Sst as i32),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ============================================================================
// Quantization Helper Utilities
// ============================================================================

/// Create a test quantization engine
///
/// # Returns
/// UnifiedQuantizationEngine configured for testing
pub fn create_test_quantization_engine() -> Arc<UnifiedQuantizationEngine> {
    Arc::new(UnifiedQuantizationEngine::new(
        Arc::new(UnifiedDistanceCompute::default()),
        Arc::new(InMemoryCodebookStore::new()),
    ))
}

// ============================================================================
// Data Generation Utilities
// ============================================================================

/// Generate test vectors with specified pattern
///
/// # Arguments
/// * `count` - Number of vectors to generate
/// * `dimension` - Vector dimension
/// * `pattern` - Pattern type: "sequential", "random", or "uniform"
///
/// # Returns
/// Vector of generated test vectors
pub fn generate_test_vectors(count: usize, dimension: usize, pattern: &str) -> Vec<Vec<f32>> {
    match pattern {
        "sequential" => (0..count)
            .map(|i| {
                (0..dimension)
                    .map(|j| (i * dimension + j) as f32 / 1000.0)
                    .collect()
            })
            .collect(),
        "uniform" => {
            vec![vec![0.1; dimension]; count]
        }
        _ => {
            // Default to simple pattern
            (0..count)
                .map(|i| vec![i as f32 * 0.01; dimension])
                .collect()
        }
    }
}

/// Generate test metadata items
///
/// # Arguments
/// * `count` - Number of metadata items
/// * `prefix` - Key prefix
///
/// # Returns
/// Vector of test metadata items
pub fn generate_test_metadata(count: usize, prefix: &str) -> Vec<MetadataItem> {
    (0..count)
        .map(|i| MetadataItem {
            key: format!("{}_{}", prefix, i),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue(format!(
                    "value_{}",
                    i
                )),
            ),
        })
        .collect()
}

// ============================================================================
// Test Assertions and Validation
// ============================================================================

/// Validate that a flush result is successful
///
/// # Arguments
/// * `result` - FlushResult to validate
///
/// # Panics
/// If the flush result indicates failure
pub fn assert_flush_success(result: &crate::storage::traits::FlushResult) {
    assert!(result.success, "Flush operation should succeed");
    assert!(
        result.entries_flushed.unwrap_or(0) > 0,
        "Should have flushed at least one entry"
    );
    assert!(
        result.bytes_written.unwrap_or(0) > 0,
        "Should have written some bytes"
    );
}

/// Validate that search results match expectations
///
/// # Arguments
/// * `results` - Search results to validate
/// * `expected_count` - Expected number of results
///
/// # Panics
/// If results don't match expectations
pub fn assert_search_results(results: &[OptimizedSearchRecord], expected_count: usize) {
    assert_eq!(
        results.len(),
        expected_count,
        "Should have {} search results",
        expected_count
    );

    for (i, result) in results.iter().enumerate() {
        assert!(
            !result.id.is_empty(),
            "Result {} should have non-empty ID",
            i
        );
        assert!(
            result.similarity.is_some(),
            "Result {} should have similarity score",
            i
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_test_engine() {
        let _engine = create_test_engine().await;
        // Engine created successfully - no method to check engine name directly
    }

    #[test]
    fn test_create_test_record() {
        let record = create_test_record("test_id", 128);
        assert_eq!(record.id, "test_id");
        assert_eq!(record.vector.as_ref().unwrap().len(), 128);
        assert!(!record.is_tombstone);
    }

    #[test]
    fn test_unique_collection_id() {
        let id1 = unique_collection_id("test");
        let id2 = unique_collection_id("test");
        assert_ne!(id1, id2, "Collection IDs should be unique");
        assert!(id1.starts_with("test_"), "Should have correct prefix");
    }

    #[test]
    fn test_generate_test_vectors() {
        let vectors = generate_test_vectors(10, 128, "sequential");
        assert_eq!(vectors.len(), 10);
        assert_eq!(vectors[0].len(), 128);

        let uniform = generate_test_vectors(5, 64, "uniform");
        assert_eq!(uniform.len(), 5);
        assert!(uniform.iter().all(|v| v.iter().all(|&x| x == 0.1)));
    }

    #[test]
    fn test_create_mock_search_results() {
        let results = create_mock_search_results(5);
        assert_eq!(results.len(), 5);
        assert_eq!(results[0].id, "result_0");
        assert!(results[0].similarity.is_some());
    }
}
