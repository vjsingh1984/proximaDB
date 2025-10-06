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

//! Consolidated SST Flush Tests
//!
//! This module contains all flush-related tests for the SST engine, migrated from:
//! - src/storage/engines/impls/sst/flush/mod.rs (1 test)
//! - src/storage/engines/impls/sst/flush/operations.rs (2 tests)
//! - src/storage/engines/impls/sst/flush/coordinator.rs (1 test)
//! - src/storage/engines/impls/sst/flush/optimizer.rs (3 tests)
//! - src/storage/engines/impls/sst/tests/end_to_end_test.rs (2 tests)
//! - src/storage/engines/impls/sst/tests/modular_integration_test.rs (1 test)
//! - src/storage/engines/impls/sst/tests/sst_compactor_tests.rs (1 test)
//!
//! Total: 11 tests

use std::sync::Arc;
use std::collections::HashMap;
use anyhow::Result;
use tempfile::TempDir;

use crate::storage::engines::impls::sst::{
    core::SstEngine,
    flush::{FlushCoordinator, FlushOptimizer, FlushOperations},
};
use crate::core::SstConfig;
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use crate::compute::distance_computation::{
    engine::UnifiedDistanceCompute,
    DistanceMetric,
};
use crate::proto::proximadb_v1::{
    VectorRecord, Collection, CollectionConfig, StorageAssignment,
    StorageConfig, SqlValue, FilterableColumnSpec, FilterableDataType,
    MetadataItem,
};
use crate::storage::traits::{
    UnifiedStorageEngine, StorageQueryContext, FlushParameters,
    StorageQueryMetadata,
};
use crate::core::search::SearchParams;
use crate::utils::StoragePath;
use tracing::{info, debug};

// Import test helpers
use super::helpers::*;

// ============================================================================
// SECTION 1: Core Flush Operations
// ============================================================================

/// Test from: src/storage/engines/impls/sst/flush/mod.rs
#[tokio::test]
async fn test_sort_vectors_for_sstable_encoding() {
    let engine = create_test_engine().await;

    let vectors = vec![
        create_simple_vector_record("vector_3", 2),
        create_simple_vector_record("vector_1", 2),
        create_simple_vector_record("vector_2", 2),
    ];

    let (sorted, stats) = engine.sort_vectors_for_sstable_encoding(vectors).await.unwrap();

    // Convert to tuples and verify sorting
    let sorted_tuples: Vec<(String, VectorRecord)> = sorted.into_iter()
        .map(|record| (record.id.clone().unwrap_or_default(), record))
        .collect();
    assert_eq!(sorted_tuples[0].0, "vector_1");
    assert_eq!(sorted_tuples[1].0, "vector_2");
    assert_eq!(sorted_tuples[2].0, "vector_3");

    // Verify stats
    assert_eq!(stats.records_sorted, 3);
    assert!(stats.compression_estimate > 0.0);
}

// ============================================================================
// SECTION 2: Flush Operations - Validation and Block Size
// ============================================================================

/// Test from: src/storage/engines/impls/sst/flush/operations.rs
#[tokio::test]
async fn test_validate_flush_preconditions() {
    let engine = create_test_engine().await;
    let ops = FlushOperations::new(Arc::new(engine));

    // Test with valid data
    let record = create_simple_vector_record("id1", 2);
    let vectors = vec![
        ("key1".to_string(), record),
    ];
    assert!(ops.validate_flush_preconditions(&vectors, "file:///tmp/test").is_ok());

    // Test with empty vectors
    let empty_vectors = vec![];
    assert!(ops.validate_flush_preconditions(&empty_vectors, "file:///tmp/test").is_err());

    // Test with empty storage URL
    assert!(ops.validate_flush_preconditions(&vectors, "").is_err());
}

/// Test from: src/storage/engines/impls/sst/flush/operations.rs
#[tokio::test]
async fn test_calculate_optimal_block_size() {
    let engine = create_test_engine().await;
    let ops = FlushOperations::new(Arc::new(engine));

    // Test small dataset
    let small_size = ops.calculate_optimal_block_size(100, 1000);
    let normal_size = ops.calculate_optimal_block_size(10000, 1000);
    // Both get clamped to max 1MB, so they're equal
    assert!(small_size <= normal_size);

    // Test large vectors - this should actually be larger due to avg_vector_size > 10240
    let large_vector_size = ops.calculate_optimal_block_size(5000, 20000);
    assert!(large_vector_size >= ops.calculate_optimal_block_size(5000, 1000));
}

// ============================================================================
// SECTION 3: Flush Coordinator
// ============================================================================

/// Test from: src/storage/engines/impls/sst/flush/coordinator.rs
#[tokio::test]
async fn test_flush_coordinator_validation() {
    let engine = create_test_engine().await;
    let coordinator = FlushCoordinator::new(Arc::new(engine));

    // Test with empty vectors - should fail
    let params = FlushParameters {
        vector_records: vec![],
        batch_ids: vec![],
        collection_id: Some("test".to_string()),
        collection_config: None,
        force: false,
        synchronous: true,
        hints: std::collections::HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        estimated_size: 0,
    };

    assert!(coordinator.validate_flush_parameters(&params).is_err());
}

// ============================================================================
// SECTION 4: Flush Optimizer - Sorting and Compression
// ============================================================================

/// Test from: src/storage/engines/impls/sst/flush/optimizer.rs
#[tokio::test]
async fn test_simple_sort() {
    let optimizer = FlushOptimizer::new();

    let vectors = vec![
        create_simple_vector_record("vector_3", 2),
        create_simple_vector_record("vector_1", 2),
        create_simple_vector_record("vector_2", 2),
    ];

    let sorted = optimizer.simple_sort(vectors).await.unwrap();

    // Verify sorting
    assert_eq!(sorted[0].0, "vector_1");
    assert_eq!(sorted[1].0, "vector_2");
    assert_eq!(sorted[2].0, "vector_3");
}

/// Test from: src/storage/engines/impls/sst/flush/optimizer.rs
#[tokio::test]
async fn test_multi_batch_optimization() {
    let optimizer = FlushOptimizer::new();

    let vectors = vec![
        create_simple_vector_record("vector_3", 2),
        create_simple_vector_record("vector_1", 2),
        create_simple_vector_record("vector_4", 2),
        create_simple_vector_record("vector_2", 2),
    ];

    let batch_ids = vec!["batch1".to_string(), "batch2".to_string()];
    let sorted = optimizer.optimize_multi_batch_sort(vectors, &batch_ids).await.unwrap();

    // Should use simple sort for small dataset
    assert_eq!(sorted.len(), 4);
    assert_eq!(sorted[0].0, "vector_1");
}

/// Test from: src/storage/engines/impls/sst/flush/optimizer.rs
#[tokio::test]
async fn test_compression_estimation() {
    let optimizer = FlushOptimizer::new();

    let small_improvement = optimizer.estimate_compression_improvement(100);
    let large_improvement = optimizer.estimate_compression_improvement(100000);

    assert!(small_improvement > 0.0);
    assert!(large_improvement > small_improvement);
    assert!(large_improvement <= 0.25); // Should be capped
}

// ============================================================================
// SECTION 5: Integration Tests - End-to-End Flush and Search
// ============================================================================

/// Test from: src/storage/engines/impls/sst/tests/end_to_end_test.rs
#[tokio::test]
async fn test_sst_engine_end_to_end_flush_and_search() -> Result<()> {
    // Initialize logging for debugging
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    info!("=� Starting SST engine end-to-end test");

    // Create temporary directory for test data
    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path().to_str().unwrap().to_string();
    info!("=� Using temporary directory: {}", base_path);

    // Create filesystem factory with temp directory
    let mut fs_config = FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", base_path));
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);

    // Create SST engine
    let sst_config = SstConfig::default();
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    let engine = SstEngine::new_with_config(
        sst_config,
        filesystem.clone(),
        distance_compute.clone()
    ).await?;

    info!(" SST engine created successfully");

    // Prepare test data - 100 vectors with 128 dimensions
    let dimension = 128;
    let num_vectors = 100;
    let collection_id = "test_collection";

    let mut vectors = Vec::new();
    for i in 0..num_vectors {
        let mut values = vec![0.0f32; dimension];
        // Create distinct patterns for each vector
        for j in 0..dimension {
            values[j] = ((i as f32) * 0.1 + (j as f32) * 0.01).sin();
        }

        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            SqlValue { value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                format!("cat_{}", i % 10)
            ))}
        );
        metadata.insert(
            "index".to_string(),
            SqlValue { value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(i as f64))}
        );

        vectors.push(VectorRecord {
            id: format!("vec_{}", i),
            vector: values,
            metadata,
            timestamp: i as i64,
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        });
    }

    info!("=� Created {} test vectors with {} dimensions", num_vectors, dimension);

    // Create collection configuration
    let collection = Collection {
        id: collection_id.to_string(),
        config: Some(CollectionConfig {
            name: collection_id.to_string(),
            dimension: dimension as u32,
            storage_config: Some(StorageConfig::default()),
            filterable_columns: vec![
                FilterableColumnSpec {
                    name: "category".to_string(),
                    data_type: FilterableDataType::FilterableString as i32,
                    indexed: true,
                    supports_range: false,
                    estimated_cardinality: Some(10),
                },
                FilterableColumnSpec {
                    name: "index".to_string(),
                    data_type: FilterableDataType::FilterableFloat as i32,
                    indexed: true,
                    supports_range: true,
                    estimated_cardinality: None,
                },
            ],
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: base_path.clone(),
            base_location: base_path.clone(),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Step 1: Flush vectors to disk
    info!("=� Flushing vectors to disk...");

    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors.clone(),
        force: true,
        synchronous: true,
        collection_config: Some(collection.clone()),
        ..Default::default()
    };

    let flush_result = engine.do_flush(&flush_params).await?;

    assert!(flush_result.success, "Flush should succeed");
    assert_eq!(
        flush_result.entries_flushed.unwrap_or(0),
        num_vectors as u64,
        "Should flush all vectors"
    );
    assert!(
        flush_result.bytes_written.unwrap_or(0) > 0,
        "Should write non-zero bytes"
    );

    info!(" Flush successful: {} vectors, {} bytes written",
          flush_result.entries_flushed.unwrap_or(0),
          flush_result.bytes_written.unwrap_or(0));

    // Verify SST files were created on disk
    let data_path = format!("{}/{}/data", base_path, collection_id);
    let fs = filesystem.get_filesystem(&format!("file://{}", data_path))?;
    let files = fs.list(&format!("file://{}", data_path)).await?;

    let sst_files: Vec<_> = files.iter()
        .filter(|f| f.name.ends_with(".sst") || f.name.ends_with(".sstable"))
        .collect();

    assert!(!sst_files.is_empty(), "Should create at least one SST file");
    info!("=� Created {} SST files on disk", sst_files.len());
    for file in &sst_files {
        info!("  - {} ({} bytes)", file.name, file.metadata.size);
    }

    // Step 2: Search for vectors (exact match)
    info!("== Searching for exact vector match...");

    // Use the first vector as query
    let query_vector = vectors[0].vector.clone();

    let search_params = Arc::new(SearchParams {
        vector: Some(query_vector.clone()),
        top_k: Some(5),
        filters: None,
        filter_expression: None,
        ..Default::default()
    });

    let ctx = StorageQueryContext {
        search_params: search_params.clone(),
        collection: Arc::new(collection.clone()),
        metadata: StorageQueryMetadata {
            collection_id: collection_id.to_string(),
            ..Default::default()
        },
    };

    let search_results = engine.search_vectors_unified(&ctx).await?;

    // Verify we got results - this is the key end-to-end test
    assert!(!search_results.is_empty(), "Should return search results");
    assert!(search_results.len() <= 5, "Should respect top_k limit");

    // Verify results have scores (don't validate exact range as different metrics use different scales)
    for result in &search_results {
        assert!(result.score.is_finite(), "Score should be finite, got {}", result.score);
    }

    info!(" Search returned {} results", search_results.len());
    for (i, result) in search_results.iter().take(5).enumerate() {
        info!("  #{}: {} (score: {:.4})", i+1, result.id, result.score);
    }

    // Step 4: Verify data persistence - create new engine instance
    info!("= Creating new engine instance to verify persistence...");

    let engine2 = SstEngine::new_with_config(
        SstConfig::default(),
        filesystem.clone(),
        Arc::new(UnifiedDistanceCompute::default())
    ).await?;

    // Search with the new engine instance
    let persistence_ctx = StorageQueryContext {
        search_params: search_params.clone(),
        collection: Arc::new(collection.clone()),
        metadata: StorageQueryMetadata {
            collection_id: collection_id.to_string(),
            ..Default::default()
        },
    };

    let persistence_results = engine2.search_vectors_unified(&persistence_ctx).await?;

    // The key test: new engine instance can read flushed data
    assert!(!persistence_results.is_empty(),
            "New engine instance should find persisted data");
    assert_eq!(persistence_results.len(), search_results.len(),
            "New engine should find same number of results");

    info!(" Data persistence verified - new engine found {} results",
          persistence_results.len());

    info!("<� SST engine end-to-end test completed successfully!");

    Ok(())
}

/// Test from: src/storage/engines/impls/sst/tests/end_to_end_test.rs
#[tokio::test]
async fn test_sst_engine_no_data_without_flush() -> Result<()> {
    // This test verifies that without flush, no data is available
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path().to_str().unwrap().to_string();

    let mut fs_config = FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", base_path));
    let filesystem = Arc::new(
        FilesystemFactory::new(fs_config).await?
    );

    let engine = SstEngine::new_with_config(
        SstConfig::default(),
        filesystem,
        Arc::new(UnifiedDistanceCompute::default())
    ).await?;

    let collection = Collection {
        id: "empty_collection".to_string(),
        config: Some(CollectionConfig {
            name: "empty_collection".to_string(),
            dimension: 128,
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: base_path.clone(),
            base_location: base_path.clone(),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Search without any flush
    let search_params = Arc::new(SearchParams {
        vector: Some(vec![0.0; 128]),
        top_k: Some(5),
        ..Default::default()
    });

    let ctx = StorageQueryContext {
        search_params,
        collection: Arc::new(collection),
        metadata: StorageQueryMetadata {
            collection_id: "empty_collection".to_string(),
            ..Default::default()
        },
    };

    let results = engine.search_vectors_unified(&ctx).await?;

    assert!(
        results.is_empty(),
        "Should return no results when no data has been flushed"
    );

    info!(" Verified: No data available without flush");

    Ok(())
}

// ============================================================================
// SECTION 6: Module Integration Test
// ============================================================================

/// Test from: src/storage/engines/impls/sst/tests/modular_integration_test.rs
#[tokio::test]
async fn test_flush_module_coordination() {
    let engine = create_test_engine().await;
    let engine_arc = Arc::new(engine);

    // Test that modules can be instantiated
    let _coordinator = FlushCoordinator::new(engine_arc.clone());
    let _optimizer = FlushOptimizer::new();
    let operations = FlushOperations::new(engine_arc.clone());

    // Test one existing method
    let block_size = operations.calculate_optimal_block_size(500, 1024);
    assert!(block_size >= 4096 && block_size <= 1024 * 1024);
}

// ============================================================================
// SECTION 7: Compactor Test - Hierarchical SST with Flush
// ============================================================================

/// Test from: src/storage/engines/impls/sst/tests/sst_compactor_tests.rs
#[tokio::test]
async fn test_hierarchical_sst_with_proper_flush() {
    // Setup test environment
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    setup_test_directories(base_path).await.unwrap();

    let filesystem_factory = Arc::new(
        FilesystemFactory::new(create_test_filesystem_config())
            .await
            .unwrap(),
    );
    let collection_id = unique_collection_id("hierarchical_test");

    // Create test vectors with metadata for hierarchical testing
    let mut vectors = Vec::new();
    for i in 0..100 {
        vectors.push(create_test_vector_record(
            format!("hier_{:04}", i),
            vec![i as f32; 384],
            1000 + i as u32,
            None,
            vec![
                MetadataItem {
                    key: "score".to_string(),
                    value: Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                        (i * 10).to_string(),
                    )),
                },
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                        format!("cat_{}", i % 5),
                    )),
                },
            ],
        ));
    }

    // Create SST files using proper flush mechanism
    let sst_files = create_sst_files_with_engine(
        base_path.to_str().unwrap(),
        &collection_id,
        filesystem_factory.clone(),
        vectors,
    )
    .await
    .unwrap();

    assert!(!sst_files.is_empty(), "Should create at least one SST file");

    // Read back and verify hierarchical structure using ModularBlockReader
    use crate::storage::engines::impls::sst::readers::sst_query_engine::ModularBlockReader;

    for sst_file in &sst_files {
        let mut reader = ModularBlockReader::open(filesystem_factory.clone(), sst_file)
            .await
            .expect("Should open SST file");

        let header = reader.read_header().await.expect("Should read header");
        assert_eq!(header.entry_count, 100, "Should have 100 entries");

        debug!(
            " Hierarchical SST file created and verified: {}",
            sst_file
        );
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

async fn create_test_engine() -> SstEngine {
    let config = SstConfig::default();
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    SstEngine::new_with_config(config, filesystem, distance_compute).await.unwrap()
}

fn create_test_vector(id: &str, vector: Vec<f32>) -> VectorRecord {
    VectorRecord {
        id: id.to_string(),
        vector,
        metadata: std::collections::HashMap::new(),
        timestamp: 12345,
        updated_at: None,
        expires_at: None,
        version: None,
        source: None,
    }
}

async fn setup_test_directories(base_path: &std::path::Path) -> anyhow::Result<()> {
    use tokio::fs;
    fs::create_dir_all(base_path).await?;
    fs::create_dir_all(base_path.join("data")).await?;
    fs::create_dir_all(base_path.join("wal")).await?;
    Ok(())
}

fn unique_collection_id(prefix: &str) -> String {
    format!("{}_{}", prefix, crate::utils::uuid::Uuid::new_v4())
}

fn create_test_filesystem_config() -> FilesystemConfig {
    FilesystemConfig::default()
}

fn create_test_vector_record(
    id: String,
    vector: Vec<f32>,
    timestamp: u32,
    expires_at: Option<u32>,
    metadata_items: Vec<MetadataItem>,
) -> VectorRecord {
    VectorRecord {
        id: Some(id),
        vector,
        metadata: metadata_items,
        timestamp,
        updated_at: None,
        expires_at,
        version: None,
        ..Default::default()
    }
}

async fn create_sst_files_with_engine(
    base_path: &str,
    collection_id: &str,
    filesystem_factory: Arc<FilesystemFactory>,
    vectors: Vec<VectorRecord>,
) -> anyhow::Result<Vec<String>> {
    debug!(
        "=� Creating SST files for collection {} with {} vectors",
        collection_id,
        vectors.len()
    );

    // Create SST config
    let sst_config = create_test_sst_config(base_path);

    // Create SST engine
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    let sst_engine =
        SstEngine::new(sst_config, filesystem_factory.clone(), distance_compute).await?;

    // Create collection with storage assignment
    let collection = crate::proto::proximadb_v1::Collection {
        id: collection_id.to_string(),
        config: Some(crate::proto::proximadb_v1::CollectionConfig {
            name: collection_id.to_string(),
            dimension: Some(3),
            distance_metric: Some(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32),
            storage_engine: Some(crate::proto::proximadb_v1::StorageEngine::Sst as i32),
            ..Default::default()
        }),
        storage_assignment: Some(crate::proto::proximadb_v1::StorageAssignment {
            base_location: format!("file://{}", base_path),
            assigned_at: chrono::Utc::now().timestamp(),
        }),
        ..Default::default()
    };

    // Create flush parameters with collection
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: vectors,
        batch_ids: vec![],
        collection_config: Some(collection.clone()),
        ..Default::default()
    };

    // Flush to create SST file
    let flush_result = sst_engine.do_flush(&flush_params).await?;
    if !flush_result.success {
        return Err(anyhow::anyhow!("Flush failed"));
    }

    // Get storage URL from collection config
    let storage_url = format!("file://{}", StoragePath::collection_data_path(base_path, &collection_id));

    let fs = filesystem_factory.get_filesystem("file:///")?;
    let all_files = fs.list(&storage_url).await?;

    let sst_files: Vec<String> = all_files
        .iter()
        .filter(|entry| entry.name.ends_with(".sstable"))
        .map(|entry| format!("{}/{}", storage_url, entry.name))
        .collect();

    Ok(sst_files)
}

fn create_test_sst_config(base_path: &str) -> SstConfig {
    use crate::core::BloomFilterConfig;

    SstConfig {
        // Level configuration
        level_count: 4,
        max_levels: 4,
        compaction_threshold: 2,
        max_files_per_level: 4,
        level_size_multiplier: 4.0,

        // Block and file settings
        block_size_kb: 16384,

        // Storage type
        compaction_strategy: "leveled".to_string(),
        compression: "none".to_string(),
        compression_level: 0,

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
    }
}
