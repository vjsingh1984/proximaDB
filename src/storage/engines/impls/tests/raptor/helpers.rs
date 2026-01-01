/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! RAPTOR Engine Test Helpers
//!
//! Consolidated test helper functions for RAPTOR engine tests.
//! This module provides reusable utilities for:
//! - Test data generation (vectors, centroids, clustered data)
//! - Mock P2Matrix structures
//! - Distance calculations
//! - Matrix building and verification

use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;
use std::sync::Arc;

// ============================================================================
// Mock Structures for Testing
// ============================================================================

/// Mock P2Matrix struct for testing
/// Source: p2_matrix_tests.rs
#[derive(Debug, Clone)]
pub struct P2Matrix {
    pub num_vectors: usize,
    pub distances: Vec<u16>,
    pub min_distance: f32,
    pub max_distance: f32,
    pub compression: ProximaScheme,
    pub compressed_size: usize,
}

impl P2Matrix {
    pub fn get_distance(&self, i: usize, j: usize) -> u16 {
        if i == j {
            return 0;
        }
        let (row, col) = if i < j { (i, j) } else { (j, i) };
        let index = row * self.num_vectors + col - (row * (row + 1)) / 2 - row - 1;
        self.distances[index]
    }
}

// ============================================================================
// Test Data Generation Utilities
// ============================================================================

/// Helper to create test vectors
/// Source: boundary_spillover_tests.rs
pub fn create_test_vectors(num_vectors: usize, dimension: usize) -> Vec<Vec<f32>> {
    (0..num_vectors)
        .map(|i| {
            (0..dimension)
                .map(|j| ((i + j) as f32 / (num_vectors + dimension) as f32))
                .collect()
        })
        .collect()
}

/// Helper to create clustered vectors (for testing spillover)
/// Source: boundary_spillover_tests.rs
pub fn create_clustered_vectors(
    num_clusters: usize,
    vectors_per_cluster: usize,
    dimension: usize,
    noise: f32,
) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut all_vectors = Vec::new();
    let mut centroids = Vec::new();

    for c in 0..num_clusters {
        // Create centroid
        let centroid: Vec<f32> = (0..dimension)
            .map(|d| ((c * dimension + d) as f32).sin())
            .collect();
        centroids.push(centroid.clone());

        // Create vectors around centroid
        for v in 0..vectors_per_cluster {
            let mut vector = centroid.clone();
            for d in 0..dimension {
                vector[d] += noise * ((v + d) as f32 / vectors_per_cluster as f32 - 0.5);
            }
            all_vectors.push(vector);
        }
    }

    (all_vectors, centroids)
}

// ============================================================================
// ENGINE CREATION & INITIALIZATION
// ============================================================================

use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, CompressionAlgorithm, StorageAssignment, StorageConfig,
    StorageEngine, VectorRecord,
};
use crate::storage::engines::impls::raptor::RaptorEngine;
use crate::storage::engines::impls::raptor::config::RaptorConfig;
use crate::storage::persistence::filesystem::FileSystem;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Counter for generating unique test IDs
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique test path for test isolation
/// Each test gets its own directory to prevent race conditions
#[allow(dead_code)]
pub fn generate_unique_test_path() -> String {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let thread_id = std::thread::current().id();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("/tmp/raptor_test_{}_{}_{}", counter, timestamp, format!("{:?}", thread_id).replace("ThreadId(", "").replace(")", ""))
}

/// Generate a unique collection ID for test isolation
#[allow(dead_code)]
pub fn generate_unique_collection_id() -> String {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    format!("test_collection_{}_{}", counter, timestamp)
}

/// Create a test RAPTOR engine with default configuration
/// Source: tests.rs
#[allow(dead_code)]
pub async fn create_test_engine() -> Result<RaptorEngine> {
    // Note: cleanup is now handled per-collection in create_test_collection_isolated
    RaptorEngine::new().await
}

/// Create a test RAPTOR engine with specific compression algorithm
/// Source: compression_tests.rs
#[allow(dead_code)]
pub async fn create_test_engine_with_compression(
    _compression: CompressionAlgorithm,
) -> Result<RaptorEngine> {
    // For now, just create a default engine since the constructor doesn't take config
    // Compression will be tested via the collection config in the flush parameters
    RaptorEngine::new().await
}

/// Clean up test data directory for a specific path
/// Source: tests.rs
#[allow(dead_code)]
pub async fn cleanup_test_data_at_path(base_path: &str) -> Result<()> {
    // Create a filesystem instance using the factory
    let fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    let filesystem_factory =
        crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config).await?;
    let filesystem = filesystem_factory.get_unified_caching_filesystem(
        &format!("file://{}", base_path),
        "cleanup".to_string(),
        "raptor".to_string(),
    )?;

    // Try to remove the test data directory
    let test_data_dir = format!("file://{}", base_path);
    match filesystem.remove_dir_all(&test_data_dir).await {
        Ok(_) => {
            tracing::debug!("Cleaned up test data directory: {}", test_data_dir);
        }
        Err(e) => {
            // Directory might not exist, which is fine
            tracing::debug!(
                "Could not clean test data directory (may not exist): {}: {:?}",
                test_data_dir,
                e
            );
        }
    }

    Ok(())
}

/// Clean up test data directory before each test (legacy, uses default path)
/// Source: tests.rs
#[allow(dead_code)]
pub async fn cleanup_test_data() -> Result<()> {
    cleanup_test_data_at_path("/tmp/test_collection").await
}

// ============================================================================
// VECTORRECORD GENERATION
// ============================================================================

/// Generate test VectorRecords with incrementing values
/// Source: compression_tests.rs
#[allow(dead_code)]
pub fn create_test_vector_records(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{}", i),
            vector: (0..dimension)
                .map(|j| (i as f32 + j as f32) * 0.1)
                .collect(),
            metadata: HashMap::new(),
            version: Some(1),
            timestamp: Some(1234567890 + i as i64),
            ..Default::default()
        })
        .collect()
}

// ============================================================================
// COLLECTION CONFIGURATION HELPERS
// ============================================================================

/// Create a collection with specific compression algorithm (isolated with unique path)
/// Source: compression_tests.rs
#[allow(dead_code)]
pub fn create_collection_with_compression(compression: CompressionAlgorithm) -> Collection {
    let unique_path = generate_unique_test_path();
    let collection_id = generate_unique_collection_id();
    Collection {
        id: collection_id,
        config: Some(CollectionConfig {
            dimension: 4,
            storage_config: Some(StorageConfig {
                storage_path: Some(unique_path.clone()),
                data_paths: vec![],
                compression: Some(compression as i32),
                max_file_size_mb: Some(100),
                enable_caching: Some(true),
            }),
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: unique_path.clone(),
            backup_paths: vec![],
            engine: StorageEngine::Raptor as i32,
            engine_config: HashMap::new(),
            base_location: unique_path,
            assigned_at: chrono::Utc::now().timestamp(),
        }),
        ..Default::default()
    }
}

/// Create a default test collection (isolated with unique path)
/// Source: tests.rs
#[allow(dead_code)]
pub fn create_test_collection() -> Collection {
    let unique_path = generate_unique_test_path();
    let collection_id = generate_unique_collection_id();
    Collection {
        id: collection_id,
        config: Some(CollectionConfig {
            dimension: 4,
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: unique_path.clone(),
            backup_paths: vec![],
            engine: StorageEngine::Raptor as i32,
            engine_config: HashMap::new(),
            base_location: unique_path,
            assigned_at: chrono::Utc::now().timestamp(),
        }),
        ..Default::default()
    }
}

/// Create a test collection with custom dimension (isolated with unique path)
/// Source: compression_tests.rs (modified)
#[allow(dead_code)]
pub fn create_test_collection_with_dimension(dimension: usize) -> Collection {
    let unique_path = generate_unique_test_path();
    let collection_id = generate_unique_collection_id();
    Collection {
        id: collection_id,
        config: Some(CollectionConfig {
            dimension: dimension as u32,
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: unique_path.clone(),
            backup_paths: vec![],
            engine: StorageEngine::Raptor as i32,
            engine_config: HashMap::new(),
            base_location: unique_path,
            assigned_at: chrono::Utc::now().timestamp(),
        }),
        ..Default::default()
    }
}

// ============================================================================
// RAPTOR CONFIG HELPERS
// ============================================================================

/// Create a RaptorConfig suitable for testing
/// Source: tests.rs (extracted pattern)
#[allow(dead_code)]
pub fn create_test_raptor_config() -> RaptorConfig {
    RaptorConfig {
        rowgroup_size: 100,
        compression: crate::storage::engines::impls::raptor::config::CompressionCodec::Snappy,
        enable_statistics: true,
        enable_bloom_filters: false,
        bloom_fpp: 0.01,
        enable_simd: false,
        cache_size_mb: 10,
        enable_prefetching: false,
        enable_range_reads: false,
        compaction_threshold_files: 5,
        buffer_pool_size_mb: 10,
        cache_eviction_policy: crate::storage::engines::impls::raptor::config::EvictionPolicy::Lru,
        clustering_config: None,
        compression_level: 3,
        use_proximaencoder: false,
        simd_lanes: 8,
        prefetch_size_mb: 1,
        enable_clustering: false,
        num_clusters: None,
        target_rowgroup_size: None,
        use_component_boosting: false,
        enable_complex_types: false,
        dimension: 4,
        compaction_config: None,
        compaction_min_size_mb: 10,
        enable_clustering_aware_compaction: false,
        max_parallel_reads: 4,
    }
}

/// Create a RaptorConfig with custom dimension
/// Source: tests.rs (extracted pattern)
#[allow(dead_code)]
pub fn create_test_raptor_config_with_dimension(dimension: usize) -> RaptorConfig {
    RaptorConfig {
        rowgroup_size: 100,
        compression: crate::storage::engines::impls::raptor::config::CompressionCodec::Snappy,
        enable_statistics: true,
        enable_bloom_filters: false,
        bloom_fpp: 0.01,
        enable_simd: false,
        cache_size_mb: 10,
        enable_prefetching: false,
        enable_range_reads: false,
        compaction_threshold_files: 5,
        buffer_pool_size_mb: 10,
        cache_eviction_policy: crate::storage::engines::impls::raptor::config::EvictionPolicy::Lru,
        clustering_config: None,
        compression_level: 3,
        use_proximaencoder: false,
        simd_lanes: 8,
        prefetch_size_mb: 1,
        enable_clustering: false,
        num_clusters: None,
        target_rowgroup_size: Some(50),
        use_component_boosting: false,
        enable_complex_types: false,
        dimension,
        compaction_config: None,
        compaction_min_size_mb: 10,
        enable_clustering_aware_compaction: false,
        max_parallel_reads: 4,
    }
}

// ============================================================================
// QUANTIZATION CONFIG HELPERS
// ============================================================================

use crate::compute::quantization::types::UnifiedQuantizationLevel;

/// Create internal quantization config for testing
/// Source: smart_rowgroup_sizing.rs
#[allow(dead_code)]
pub fn create_test_quantization_config()
-> crate::storage::engines::impls::raptor::smart_rowgroup_sizing::InternalQuantizationConfig {
    crate::storage::engines::impls::raptor::smart_rowgroup_sizing::InternalQuantizationConfig {
        primary_level: UnifiedQuantizationLevel::int8(),
        store_fp32: true,
        compression_ratio: 4.0,
    }
}

/// Create quantization config with PQ8
/// Source: smart_rowgroup_sizing.rs
#[allow(dead_code)]
pub fn create_pq8_quantization_config()
-> crate::storage::engines::impls::raptor::smart_rowgroup_sizing::InternalQuantizationConfig {
    crate::storage::engines::impls::raptor::smart_rowgroup_sizing::InternalQuantizationConfig {
        primary_level: UnifiedQuantizationLevel::pq8(32),
        store_fp32: true,
        compression_ratio: 4.0,
    }
}

// ============================================================================
// HARDWARE & COMPUTE HELPERS
// ============================================================================

use crate::core::hardware_capabilities::HardwareCapabilities;

/// Initialize hardware capabilities for testing
/// Source: Multiple test files
#[allow(dead_code)]
pub fn init_hardware_capabilities() -> Arc<HardwareCapabilities> {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
    crate::core::hardware_capabilities::get_hardware_capabilities()
}

/// Create a UnifiedDistanceCompute instance for testing
/// Source: boundary_spillover_tests.rs, p2_matrix_tests.rs
#[allow(dead_code)]
pub fn create_distance_compute(metric: DistanceMetric) -> Arc<UnifiedDistanceCompute> {
    Arc::new(UnifiedDistanceCompute::new(metric))
}

// ============================================================================
// MATRIX BUILDER HELPERS
// ============================================================================

/// Create a MatrixBuilder for testing
/// Source: boundary_spillover_tests.rs, p2_matrix_tests.rs
#[allow(dead_code)]
pub fn create_matrix_builder(
    metric: DistanceMetric,
) -> crate::storage::engines::impls::raptor::matrix_builder::MatrixBuilder {
    let hardware = init_hardware_capabilities();
    let distance_compute = create_distance_compute(metric);

    crate::storage::engines::impls::raptor::matrix_builder::MatrixBuilder::new(
        distance_compute,
        hardware,
        metric,
    )
}

// ============================================================================
// CLOUD I/O PROFILE HELPERS
// ============================================================================

/// Create S3 Standard I/O profile for testing
/// Source: smart_rowgroup_sizing.rs
#[allow(dead_code)]
pub fn create_s3_io_profile()
-> crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CloudIOProfile {
    crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CloudIOProfile::s3_standard()
}

/// Create GCS Standard I/O profile for testing
/// Source: smart_rowgroup_sizing.rs
#[allow(dead_code)]
pub fn create_gcs_io_profile()
-> crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CloudIOProfile {
    crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CloudIOProfile::gcs_standard()
}

/// Create ADLS Gen2 I/O profile for testing
/// Source: smart_rowgroup_sizing.rs
#[allow(dead_code)]
pub fn create_adls_io_profile()
-> crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CloudIOProfile {
    crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CloudIOProfile::adls_gen2()
}

// ============================================================================
// SMART ROWGROUP SIZER HELPERS
// ============================================================================

/// Create a SmartRowGroupSizer for S3 with OpenAI embeddings
/// Source: smart_rowgroup_sizing.rs
#[allow(dead_code)]
pub fn create_openai_s3_sizer()
-> crate::storage::engines::impls::raptor::smart_rowgroup_sizing::SmartRowGroupSizer {
    crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CommonConfigurations::openai_s3()
}

/// Create a SmartRowGroupSizer for GCS with BERT embeddings
/// Source: smart_rowgroup_sizing.rs
#[allow(dead_code)]
pub fn create_bert_gcs_sizer()
-> crate::storage::engines::impls::raptor::smart_rowgroup_sizing::SmartRowGroupSizer {
    crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CommonConfigurations::bert_gcs()
}

/// Create a SmartRowGroupSizer for ADLS with research vectors
/// Source: smart_rowgroup_sizing.rs
#[allow(dead_code)]
pub fn create_research_adls_sizer()
-> crate::storage::engines::impls::raptor::smart_rowgroup_sizing::SmartRowGroupSizer {
    crate::storage::engines::impls::raptor::smart_rowgroup_sizing::CommonConfigurations::research_adls()
}

// ============================================================================
// METADATA & SERIALIZATION HELPERS
// ============================================================================

/// Create RaptorCachedMetadata for testing
/// Source: unified_metadata_serializer.rs
#[allow(dead_code)]
pub fn create_test_raptor_metadata()
-> crate::storage::engines::impls::raptor::unified_metadata_serializer::RaptorCachedMetadata {
    crate::storage::engines::impls::raptor::unified_metadata_serializer::RaptorCachedMetadata {
        file_size: 1024000,
        vector_count: 10000,
        dimension: 768,
        centroid_stats: Vec::new(),
        rowgroup_offsets: vec![0, 51200, 102400],
        bloom_filter_data: vec![0xFF; 1024],
        compression_metadata: Default::default(),
        creation_timestamp: 1234567890,
        pxk_coverage: 0.85,
        has_hnsw: true,
        hnsw_offset: Some(204800),
    }
}

// ============================================================================
// ARTUS BLOOM FILTER HELPERS
// ============================================================================

/// Create ArtusBloomConfig for testing
/// Source: artus_bloom.rs
#[allow(dead_code)]
pub fn create_artus_bloom_config()
-> crate::storage::engines::impls::raptor::artus_bloom::ArtusBloomConfig {
    crate::storage::engines::impls::raptor::artus_bloom::ArtusBloomConfig::default()
}

/// Create ArtusColumnStats for testing
/// Source: artus_bloom.rs
#[allow(dead_code)]
pub fn create_artus_column_stats(
    column_name: &str,
    cardinality: usize,
) -> crate::storage::engines::impls::raptor::artus_bloom::ArtusColumnStats {
    crate::storage::engines::impls::raptor::artus_bloom::ArtusColumnStats {
        column_name: column_name.to_string(),
        cardinality,
        null_ratio: 0.01,
        access_frequency: 20000,
        selectivity: 0.3,
        data_type: crate::storage::engines::impls::raptor::artus_bloom::ColumnData::String,
        bloom_benefit_score: 0.8,
    }
}

// ============================================================================
// CALCULATION HELPERS
// ============================================================================

/// Calculate adaptive P×K coverage based on formula
/// Source: boundary_spillover_tests.rs
#[allow(dead_code)]
pub fn calculate_adaptive_pxk_coverage(k: usize, d: usize) -> f32 {
    let k_f = k as f32;
    let d_f = d as f32;
    (0.1_f32).max(1.0_f32.min((-2.0 * (k_f / d_f + 1.0).ln()).exp()))
}

/// Calculate boundary ratio for expansion detection
/// Source: boundary_spillover_tests.rs
#[allow(dead_code)]
pub fn calculate_boundary_ratio(d_i: f32, d_j: f32) -> f32 {
    d_i / d_j
}

/// Check if boundary should expand based on ratio
/// Source: boundary_spillover_tests.rs
#[allow(dead_code)]
pub fn should_expand_boundary(ratio: f32) -> bool {
    ratio > 0.8
}

// ============================================================================
// ASSERTION HELPERS
// ============================================================================

/// Verify P2 matrix distance symmetry
/// Source: p2_matrix_tests.rs
#[allow(dead_code)]
pub fn assert_p2_distance_symmetry(matrix: &P2Matrix) {
    for i in 0..matrix.num_vectors {
        for j in 0..matrix.num_vectors {
            assert_eq!(
                matrix.get_distance(i, j),
                matrix.get_distance(j, i),
                "Distance symmetry failed for ({}, {})",
                i,
                j
            );
        }
    }
}

/// Assert self-distance is zero
/// Source: p2_matrix_tests.rs
#[allow(dead_code)]
pub fn assert_self_distance_zero(matrix: &P2Matrix) {
    for i in 0..matrix.num_vectors {
        assert_eq!(
            matrix.get_distance(i, i),
            0,
            "Self-distance should be 0 for vector {}",
            i
        );
    }
}

// ============================================================================
// MEMORY CALCULATION HELPERS
// ============================================================================

/// Calculate upper triangle matrix size
/// Source: p2_matrix_tests.rs
#[allow(dead_code)]
pub fn calculate_upper_triangle_size(n: usize) -> usize {
    n * (n - 1) / 2
}

/// Calculate P2 matrix memory requirements
/// Source: p2_matrix_tests.rs
#[allow(dead_code)]
pub fn calculate_p2_memory_bytes(p: usize) -> usize {
    p * (p - 1) / 2
}

/// Calculate memory savings from centralized footer
/// Source: tests.rs
#[allow(dead_code)]
pub fn calculate_memory_savings(
    num_rowgroups: usize,
    dimension: usize,
    neighbors_per_rowgroup: usize,
) -> (usize, usize, f32) {
    // Distributed: storing neighbor centroids inline
    let distributed_size = num_rowgroups * neighbors_per_rowgroup * dimension * 4;

    // Centralized: all centroids in footer
    let centralized_size = num_rowgroups * dimension * 4;

    let savings_bytes = distributed_size - centralized_size;
    let savings_pct = (savings_bytes as f32 / distributed_size as f32) * 100.0;

    (distributed_size, centralized_size, savings_pct)
}

// ============================================================================
// PERFORMANCE ESTIMATION HELPERS
// ============================================================================

/// Estimate centroid distance matrix performance impact
/// Source: tests.rs
#[allow(dead_code)]
pub fn estimate_matrix_compute_time(k: usize) -> (usize, f64) {
    let num_distances = k * (k - 1) / 2;
    let estimated_ms = (num_distances as f64 * 0.5) / 1000.0;
    (num_distances, estimated_ms)
}

/// Assess performance impact based on compute time
/// Source: tests.rs
#[allow(dead_code)]
pub fn assess_performance_impact(estimated_ms: f64) -> &'static str {
    if estimated_ms < 1.0 {
        "Negligible (<1ms)"
    } else if estimated_ms < 10.0 {
        "Acceptable (<10ms)"
    } else if estimated_ms < 100.0 {
        "Noticeable (10-100ms)"
    } else {
        "Problematic (>100ms) - Use lazy loading"
    }
}
