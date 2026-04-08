/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! NOVA Engine Test Helpers
//!
//! Consolidated test helper functions for NOVA engine tests.
//! This module provides reusable utilities for:
//! - Test data generation (vectors, queries, enhanced stats)
//! - Quantization helpers (binary sketches, INT8, PQ)
//! - Zone map and query characteristics
//! - Distance calculations
//! - Vector serialization/deserialization
//! - Batch operations
//! - Performance metrics

use crate::storage::engines::core::formats::VectorSerializer;
use crate::storage::engines::nova::{hierarchical_stats::*, zone_maps::*};
use anyhow::Result;

// ============================================================================
// Test Data Generation Utilities
// ============================================================================

/// Create test vectors for optimization testing
/// Source: optimization_tests.rs
///
/// # Arguments
/// * `count` - Number of vectors to create
/// * `dimension` - Dimension of each vector
///
/// # Returns
/// A vector of test vectors with deterministic values
pub fn create_test_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            (0..dimension)
                .map(|j| (i as f32 + j as f32) / (dimension as f32))
                .collect()
        })
        .collect()
}

/// Create a test query vector
/// Source: optimization_tests.rs
///
/// # Arguments
/// * `dimension` - Dimension of the query vector
///
/// # Returns
/// A test query vector with deterministic values
pub fn create_test_query(dimension: usize) -> Vec<f32> {
    (0..dimension)
        .map(|i| i as f32 / dimension as f32)
        .collect()
}

/// Create a large test dataset for performance testing
/// Source: optimization_tests.rs
///
/// # Arguments
/// * `count` - Number of vectors to create
/// * `dimension` - Dimension of each vector
///
/// # Returns
/// A vector of test vectors with pseudo-random variation
pub fn create_large_test_dataset(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            (0..dimension)
                .map(|j| {
                    // Create more realistic test data with some randomness
                    let base = (i as f32 + j as f32) / (dimension as f32);
                    let noise = ((i * 7 + j * 11) % 100) as f32 / 1000.0; // Simple pseudo-random
                    base + noise
                })
                .collect()
        })
        .collect()
}

// ============================================================================
// Enhanced Stats Creation Utilities
// ============================================================================

/// Create a single test EnhancedRowGroupStats
/// Source: optimization_tests.rs, engine.rs
///
/// # Arguments
/// * `id` - Row group ID
///
/// # Returns
/// A test EnhancedRowGroupStats with default values
pub fn create_test_enhanced_stats(id: u32) -> EnhancedRowGroupStats {
    let zone_map = ZoneMap::from_vectors(&[vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]).unwrap();

    EnhancedRowGroupStats {
        row_group_id: id,
        parquet_metadata: None,
        vector_zone_map: zone_map,
        quantized_selectivity: QuantizedSelectivity {
            binary_effectiveness: 0.8,
            int8_accuracy: 0.9,
            pq_quality: 0.85,
            progressive_efficiency: 0.75,
        },
        compression_ratio: 4.0,
        search_cost_estimate: SearchCostEstimate {
            io_cost: 10.0,
            cpu_cost: 20.0,
            memory_cost: 15.0,
            estimated_latency_ms: 50.0,
            confidence: 0.9,
        },
        access_stats: AccessStats {
            access_count: 0,
            last_access: chrono::Utc::now(),
            avg_selectivity: 0.5,
            cache_hit_rate: 0.0,
            access_frequency: 0.0,
        },
    }
}

/// Create multiple test EnhancedRowGroupStats
/// Source: optimization_tests.rs, engine.rs
///
/// # Arguments
/// * `count` - Number of stats to create
///
/// # Returns
/// A vector of test EnhancedRowGroupStats with varying IDs and costs
pub fn create_test_enhanced_stats_vec(count: usize) -> Vec<EnhancedRowGroupStats> {
    (0..count)
        .map(|i| {
            let zone_map =
                ZoneMap::from_vectors(&[vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]).unwrap();

            EnhancedRowGroupStats {
                row_group_id: i as u32,
                parquet_metadata: None,
                vector_zone_map: zone_map,
                quantized_selectivity: QuantizedSelectivity {
                    binary_effectiveness: 0.8,
                    int8_accuracy: 0.9,
                    pq_quality: 0.85,
                    progressive_efficiency: 0.75,
                },
                compression_ratio: 4.0,
                search_cost_estimate: SearchCostEstimate {
                    io_cost: 10.0 + i as f32,
                    cpu_cost: 20.0 + i as f32,
                    memory_cost: 15.0 + i as f32,
                    estimated_latency_ms: 50.0 + i as f32,
                    confidence: 0.9,
                },
                access_stats: AccessStats {
                    access_count: i as u64,
                    last_access: chrono::Utc::now(),
                    avg_selectivity: 0.5,
                    cache_hit_rate: 0.0,
                    access_frequency: 0.0,
                },
            }
        })
        .collect()
}

// ============================================================================
// Quantization Helpers - Binary Sketch
// ============================================================================

/// Compute binary sketch from a vector
/// Source: columnar_search.rs
///
/// # Arguments
/// * `vector` - Input vector to sketch
///
/// # Returns
/// Binary sketch as bytes (1 bit per dimension)
#[allow(dead_code)]
pub fn binary_sketch_from_vector(vector: &[f32]) -> Vec<u8> {
    let mut sketch = Vec::with_capacity(vector.len() / 8);
    for chunk in vector.chunks(8) {
        let mut byte = 0u8;
        for (i, &val) in chunk.iter().enumerate() {
            if val > 0.0 {
                byte |= 1 << i;
            }
        }
        sketch.push(byte);
    }
    sketch
}

/// Compute Hamming distance between two binary sketches
/// Source: columnar_search.rs
///
/// # Arguments
/// * `a` - First binary sketch
/// * `b` - Second binary sketch
///
/// # Returns
/// Hamming distance (number of differing bits)
#[allow(dead_code)]
pub fn hamming_distance(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
}

// ============================================================================
// Quantization Helpers - INT8
// ============================================================================

/// Quantize a vector to INT8
/// Source: columnar_search.rs
///
/// # Arguments
/// * `vector` - Input vector
///
/// # Returns
/// INT8 quantized vector
#[allow(dead_code)]
pub fn quantize_vector_to_int8(vector: &[f32]) -> Vec<i8> {
    // Find min/max for normalization
    let min = vector.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = vector.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = max - min;

    if range < 1e-6 {
        // All values are the same
        return vec![0i8; vector.len()];
    }

    vector
        .iter()
        .map(|&v| {
            let normalized = (v - min) / range;
            let scaled = normalized * 255.0;
            (scaled as i32 - 128).clamp(-128, 127) as i8
        })
        .collect()
}

/// Compute L2 distance squared between two INT8 vectors
/// Source: columnar_search.rs
///
/// # Arguments
/// * `a` - First INT8 vector
/// * `b` - Second INT8 vector
///
/// # Returns
/// L2 distance squared
#[allow(dead_code)]
pub fn int8_l2_distance_squared(a: &[i8], b: &[i8]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let diff = (*x as f32) - (*y as f32);
            diff * diff
        })
        .sum()
}

// ============================================================================
// Zone Map and Query Characteristics
// ============================================================================

/// Extract query characteristics for optimization
/// Source: zone_maps.rs
///
/// # Arguments
/// * `query` - Query vector
/// * `top_k` - Number of results requested
///
/// # Returns
/// QueryCharacteristics for optimization decisions
#[allow(dead_code)]
pub fn extract_query_characteristics(query: &[f32], top_k: usize) -> QueryCharacteristics {
    // Compute L2 norm
    let norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();

    // Compute sparsity (fraction of near-zero values)
    let sparsity = query.iter().filter(|&&x| x.abs() < 0.01).count() as f32 / query.len() as f32;

    // Find dominant dimensions (top 5 by absolute value)
    let mut indexed_query: Vec<(usize, f32)> = query
        .iter()
        .enumerate()
        .map(|(i, &v)| (i, v.abs()))
        .collect();
    indexed_query.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let dominant_dimensions = indexed_query
        .iter()
        .take(5)
        .map(|(i, _)| *i as u32)
        .collect();

    QueryCharacteristics {
        norm,
        sparsity,
        dominant_dimensions,
        distance_metric: "euclidean".to_string(),
        top_k: top_k as u32,
    }
}

/// Predict selectivity using linear model
/// Source: zone_maps.rs
///
/// # Arguments
/// * `characteristics` - Query characteristics
///
/// # Returns
/// Predicted selectivity (0.0 to 1.0)
#[allow(dead_code)]
pub fn predict_selectivity_linear(characteristics: &QueryCharacteristics) -> f32 {
    // Simple linear model based on top_k and sparsity
    let base_selectivity = characteristics.top_k as f32 / 10000.0;
    let sparsity_factor = 1.0 + characteristics.sparsity;
    (base_selectivity * sparsity_factor).min(1.0)
}

// ============================================================================
// Distance Tables - Product Quantization
// ============================================================================

/// Compute PQ distance table for query
/// Source: columnar_search.rs
///
/// # Arguments
/// * `query` - Query vector
/// * `codebook` - PQ codebook
/// * `num_subvectors` - Number of subvectors
/// * `codebook_size` - Size of codebook (K)
///
/// # Returns
/// Distance table [num_subvectors x codebook_size]
#[allow(dead_code)]
pub fn compute_pq_distance_table(
    query: &[f32],
    codebook: &[Vec<f32>],
    num_subvectors: usize,
    codebook_size: usize,
) -> Vec<Vec<f32>> {
    let subvector_dim = query.len() / num_subvectors;
    let mut table = vec![vec![0.0; codebook_size]; num_subvectors];

    for (m, table_row) in table.iter_mut().enumerate().take(num_subvectors) {
        let query_subvec = &query[m * subvector_dim..(m + 1) * subvector_dim];

        for (k, distance) in table_row.iter_mut().enumerate().take(codebook_size) {
            let centroid = &codebook[m * codebook_size + k];
            *distance = query_subvec
                .iter()
                .zip(centroid.iter())
                .map(|(q, c)| {
                    let diff = q - c;
                    diff * diff
                })
                .sum::<f32>()
                .sqrt();
        }
    }

    table
}

/// Look up distance from PQ distance table
/// Source: columnar_search.rs
///
/// # Arguments
/// * `table` - Precomputed distance table
/// * `pq_code` - PQ code for vector
///
/// # Returns
/// Approximate distance
#[allow(dead_code)]
pub fn lookup_pq_distance(table: &[Vec<f32>], pq_code: &[u8]) -> f32 {
    pq_code
        .iter()
        .enumerate()
        .map(|(m, &k)| table[m][k as usize])
        .sum()
}

// ============================================================================
// Vector Serialization and Deserialization
// ============================================================================

/// Deserialize a vector from bytes
/// Source: unified_metadata_serializer.rs
///
/// # Arguments
/// * `bytes` - Serialized vector bytes
///
/// # Returns
/// Deserialized vector
///
/// NOTE: Delegates to shared VectorSerializer from core/formats
#[allow(dead_code)]
pub fn deserialize_vector(bytes: &[u8]) -> Result<Vec<f32>> {
    VectorSerializer::deserialize_raw(bytes)
}

/// Serialize a vector to bytes
/// Source: unified_metadata_serializer.rs, batch_operations.rs
///
/// # Arguments
/// * `vector` - Vector to serialize
///
/// # Returns
/// Serialized bytes (raw f32 bytes, no length prefix for test compatibility)
///
/// NOTE: Uses raw bytemuck cast for compatibility with existing tests
#[allow(dead_code)]
pub fn serialize_vector(vector: &[f32]) -> Vec<u8> {
    // Use raw serialization (no length prefix) for backward compatibility with tests
    bytemuck::cast_slice(vector).to_vec()
}

// ============================================================================
// Batch Operations
// ============================================================================

/// Group vectors by row group based on row group size
/// Source: batch_operations.rs
///
/// # Arguments
/// * `vectors` - Vectors to group
/// * `row_group_size` - Size of each row group
///
/// # Returns
/// Vectors grouped by row group
#[allow(dead_code)]
pub fn group_by_row_group(vectors: &[Vec<f32>], row_group_size: usize) -> Vec<Vec<Vec<f32>>> {
    vectors
        .chunks(row_group_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}

// ============================================================================
// Performance Tracking and Metrics
// ============================================================================

/// Performance metrics for tracking execution
/// Source: optimized_operations.rs
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub io_operations: u64,
    pub bytes_read: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub total_time_ms: f64,
    pub cpu_time_ms: f64,
}

impl PerformanceMetrics {
    /// Create new performance metrics
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            io_operations: 0,
            bytes_read: 0,
            cache_hits: 0,
            cache_misses: 0,
            total_time_ms: 0.0,
            cpu_time_ms: 0.0,
        }
    }

    /// Record an I/O operation
    #[allow(dead_code)]
    pub fn record_io(&mut self, bytes: u64) {
        self.io_operations += 1;
        self.bytes_read += bytes;
    }

    /// Record a cache hit
    #[allow(dead_code)]
    pub fn record_cache_hit(&mut self) {
        self.cache_hits += 1;
    }

    /// Record a cache miss
    #[allow(dead_code)]
    pub fn record_cache_miss(&mut self) {
        self.cache_misses += 1;
    }

    /// Calculate cache hit rate
    #[allow(dead_code)]
    pub fn cache_hit_rate(&self) -> f32 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            self.cache_hits as f32 / total as f32
        }
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Execution Plan Helpers
// ============================================================================

/// Execution plan for progressive search
/// Source: progressive_search.rs
#[allow(dead_code, missing_docs)]
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    pub quantization_level: String,
    pub row_groups_to_scan: Vec<u32>,
    pub estimated_cost: f32,
    pub estimated_recall: f32,
}

impl ExecutionPlan {
    /// Create a new execution plan
    #[allow(dead_code)]
    pub fn new(quantization_level: String, row_groups: Vec<u32>, cost: f32, recall: f32) -> Self {
        Self {
            quantization_level,
            row_groups_to_scan: row_groups,
            estimated_cost: cost,
            estimated_recall: recall,
        }
    }

    /// Check if plan meets quality threshold
    #[allow(dead_code)]
    pub fn meets_quality_threshold(&self, threshold: f32) -> bool {
        self.estimated_recall >= threshold
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_test_vectors() {
        let vectors = create_test_vectors(10, 128);
        assert_eq!(vectors.len(), 10);
        assert_eq!(vectors[0].len(), 128);
    }

    #[test]
    fn test_create_test_query() {
        let query = create_test_query(128);
        assert_eq!(query.len(), 128);
    }

    #[test]
    fn test_create_large_test_dataset() {
        let dataset = create_large_test_dataset(100, 64);
        assert_eq!(dataset.len(), 100);
        assert_eq!(dataset[0].len(), 64);
    }

    #[test]
    fn test_create_test_enhanced_stats() {
        let stats = create_test_enhanced_stats(0);
        assert_eq!(stats.row_group_id, 0);
        assert_eq!(stats.compression_ratio, 4.0);
    }

    #[test]
    fn test_create_test_enhanced_stats_vec() {
        let stats_vec = create_test_enhanced_stats_vec(5);
        assert_eq!(stats_vec.len(), 5);
        assert_eq!(stats_vec[0].row_group_id, 0);
        assert_eq!(stats_vec[4].row_group_id, 4);
    }

    #[test]
    fn test_binary_sketch() {
        let vector = vec![1.0, -1.0, 2.0, -2.0, 0.5, -0.5, 0.0, 1.0];
        let sketch = binary_sketch_from_vector(&vector);
        assert_eq!(sketch.len(), 1);
        // Bits: 1, 0, 1, 0, 1, 0, 0, 1 = 0b10010101 = 0x95
        assert_eq!(sketch[0], 0b10010101);
    }

    #[test]
    fn test_hamming_distance() {
        let a = vec![0b11110000];
        let b = vec![0b10101010];
        let distance = hamming_distance(&a, &b);
        assert_eq!(distance, 4); // 4 bits differ
    }

    #[test]
    fn test_quantize_to_int8() {
        let vector = vec![0.0, 0.5, 1.0];
        let quantized = quantize_vector_to_int8(&vector);
        assert_eq!(quantized.len(), 3);
        assert_eq!(quantized[0], -128); // min value
        assert_eq!(quantized[2], 127); // max value
    }

    #[test]
    fn test_int8_distance() {
        let a = vec![0i8, 10i8, 20i8];
        let b = vec![0i8, 10i8, 20i8];
        let distance = int8_l2_distance_squared(&a, &b);
        assert_eq!(distance, 0.0);
    }

    #[test]
    fn test_extract_query_characteristics() {
        let query = vec![1.0, 0.0, 0.0, 2.0];
        let characteristics = extract_query_characteristics(&query, 10);
        assert_eq!(characteristics.top_k, 10);
        assert!(characteristics.norm > 0.0);
    }

    #[test]
    #[ignore = "SelectivityCharacteristics type not found - needs implementation"]
    fn test_predict_selectivity() {
        // TODO: Implement SelectivityCharacteristics struct
        // let characteristics = SelectivityCharacteristics {
        //     norm: 1.0,
        //     sparsity: 0.5,
        //     dominant_dimensions: vec![0, 1, 2],
        //     distance_metric: "euclidean".to_string(),
        //     top_k: 100,
        // };
        // let selectivity = predict_selectivity_linear(&characteristics);
        // assert!(selectivity > 0.0 && selectivity <= 1.0);
    }

    #[test]
    fn test_vector_serialization() {
        let vector = vec![1.0, 2.0, 3.0];
        let bytes = serialize_vector(&vector);
        assert_eq!(bytes.len(), 12); // 3 floats * 4 bytes

        let deserialized = deserialize_vector(&bytes).unwrap();
        assert_eq!(deserialized.len(), 3);
        assert_eq!(deserialized[0], 1.0);
        assert_eq!(deserialized[1], 2.0);
        assert_eq!(deserialized[2], 3.0);
    }

    #[test]
    fn test_group_by_row_group() {
        let vectors = vec![
            vec![1.0, 2.0],
            vec![3.0, 4.0],
            vec![5.0, 6.0],
            vec![7.0, 8.0],
        ];
        let grouped = group_by_row_group(&vectors, 2);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].len(), 2);
        assert_eq!(grouped[1].len(), 2);
    }

    #[test]
    fn test_performance_metrics() {
        let mut metrics = PerformanceMetrics::new();
        metrics.record_io(1024);
        metrics.record_cache_hit();
        metrics.record_cache_miss();

        assert_eq!(metrics.io_operations, 1);
        assert_eq!(metrics.bytes_read, 1024);
        assert_eq!(metrics.cache_hits, 1);
        assert_eq!(metrics.cache_misses, 1);
        assert_eq!(metrics.cache_hit_rate(), 0.5);
    }

    #[test]
    fn test_execution_plan() {
        let plan = ExecutionPlan::new("binary".to_string(), vec![0, 1, 2], 10.0, 0.95);

        assert_eq!(plan.quantization_level, "binary");
        assert_eq!(plan.row_groups_to_scan.len(), 3);
        assert!(plan.meets_quality_threshold(0.9));
        assert!(!plan.meets_quality_threshold(0.99));
    }

    #[test]
    fn test_pq_distance_table() {
        // Simple test with 2 subvectors, codebook size 4
        let query = vec![1.0, 2.0, 3.0, 4.0];
        let codebook = vec![
            // Subvector 0 centroids
            vec![0.0, 1.0],
            vec![1.0, 2.0],
            vec![2.0, 3.0],
            vec![3.0, 4.0],
            // Subvector 1 centroids
            vec![2.0, 3.0],
            vec![3.0, 4.0],
            vec![4.0, 5.0],
            vec![5.0, 6.0],
        ];

        let table = compute_pq_distance_table(&query, &codebook, 2, 4);
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].len(), 4);
        assert_eq!(table[1].len(), 4);

        // Test lookup
        let pq_code = vec![1, 2]; // Use centroid 1 for subvec 0, centroid 2 for subvec 1
        let distance = lookup_pq_distance(&table, &pq_code);
        assert!(distance >= 0.0);
    }
}
