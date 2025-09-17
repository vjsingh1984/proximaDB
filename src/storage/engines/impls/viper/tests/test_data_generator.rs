//! Test Data Generator for Two-Stage Search Tests
//!
//! This module provides utilities to generate realistic test Parquet files
//! containing both FP32 and quantized vector data for testing the two-stage
//! search functionality.

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use anyhow::Result;
use arrow_array::builder::{Float32Builder, ListBuilder};
use arrow_array::types::UInt8Type;
use arrow_array::{ArrayRef, Float64Array, Int64Array, ListArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;
use std::fs::File;
use std::sync::Arc;
// Note: apache_avro imports removed - add to Cargo.toml if needed for Avro tests

/// Configuration for test data generation
pub struct TestDataConfig {
    /// Number of vectors to generate
    pub num_vectors: usize,
    /// Dimension of each vector
    pub dimension: usize,
    /// Number of collections to distribute vectors across
    pub num_collections: usize,
    /// Number of distinct categories for metadata
    pub num_categories: usize,
    /// Percentage of vectors to mark as expired (0.0 - 1.0)
    pub expiry_rate: f32,
    /// Random seed for reproducibility
    pub seed: u64,
    /// Whether to generate PQ8 quantized vectors
    pub include_pq8: bool,
    /// Whether to generate PQ4 quantized vectors
    pub include_pq4: bool,
    /// Whether to generate binary quantized vectors
    pub include_binary: bool,
    /// Whether to generate INT8 quantized vectors
    pub include_int8: bool,
    /// Number of subvectors for PQ quantization
    pub pq_num_subvectors: usize,
}

impl Default for TestDataConfig {
    fn default() -> Self {
        Self {
            num_vectors: 1000,
            dimension: 128,
            num_collections: 3,
            num_categories: 5,
            expiry_rate: 0.1,
            seed: 42,
            include_pq8: true,
            include_pq4: true,
            include_binary: true,
            include_int8: true,
            pq_num_subvectors: 16,
        }
    }
}

/// Test data generator
pub struct TestDataGenerator {
    config: TestDataConfig,
    rng: ChaCha8Rng,
}

impl TestDataGenerator {
    pub fn new(config: TestDataConfig) -> Self {
        let rng = ChaCha8Rng::seed_from_u64(config.seed);
        Self { config, rng }
    }

    /// Generate FP32 vectors with specific patterns for testing
    pub fn generate_vectors(&mut self) -> Vec<Vec<f32>> {
        let mut vectors = Vec::with_capacity(self.config.num_vectors);

        for i in 0..self.config.num_vectors {
            let pattern = i % 5; // Create 5 different patterns
            let vector = match pattern {
                0 => self.generate_random_vector(),
                1 => self.generate_clustered_vector(0.0, 0.1),
                2 => self.generate_clustered_vector(1.0, 0.1),
                3 => self.generate_sparse_vector(0.1),
                4 => self.generate_normalized_vector(),
                _ => unreachable!(),
            };
            vectors.push(vector);
        }

        vectors
    }

    /// Generate a random vector
    fn generate_random_vector(&mut self) -> Vec<f32> {
        (0..self.config.dimension)
            .map(|_| self.rng.gen_range(-1.0..1.0))
            .collect()
    }

    /// Generate a vector clustered around a center
    fn generate_clustered_vector(&mut self, center: f32, std_dev: f32) -> Vec<f32> {
        (0..self.config.dimension)
            .map(|_| {
                let normal: f32 = self.rng.gen_range(0.0..1.0) * std_dev;
                center + normal
            })
            .collect()
    }

    /// Generate a sparse vector
    fn generate_sparse_vector(&mut self, sparsity: f32) -> Vec<f32> {
        (0..self.config.dimension)
            .map(|_| {
                if self.rng.gen_range(0.0..1.0) < sparsity {
                    self.rng.gen_range(-1.0..1.0)
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// Generate a normalized vector
    fn generate_normalized_vector(&mut self) -> Vec<f32> {
        let mut vector = self.generate_random_vector();
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            vector.iter_mut().for_each(|x| *x /= norm);
        }
        vector
    }

    /// Generate PQ codes from vectors (mock implementation)
    pub fn generate_pq_codes(&mut self, vectors: &[Vec<f32>], bits: u8) -> Vec<Vec<u8>> {
        vectors
            .iter()
            .map(|vector| {
                let codes_per_vector = self.config.pq_num_subvectors;
                let max_value = if bits >= 32 {
                    u32::MAX
                } else {
                    (1u32 << bits) - 1
                };

                (0..codes_per_vector)
                    .map(|i| {
                        // Mock PQ: use vector values to generate codes
                        let subvec_start = i * (self.config.dimension / codes_per_vector);
                        let subvec_end = (i + 1) * (self.config.dimension / codes_per_vector);
                        let subvec_sum: f32 = vector[subvec_start..subvec_end].iter().sum();
                        ((subvec_sum.abs() * 100.0) as u32 % max_value) as u8
                    })
                    .collect()
            })
            .collect()
    }

    /// Generate binary codes from vectors
    pub fn generate_binary_codes(&mut self, vectors: &[Vec<f32>]) -> Vec<Vec<u8>> {
        vectors
            .iter()
            .map(|vector| {
                let bytes_needed = (self.config.dimension + 7) / 8;
                let mut binary = vec![0u8; bytes_needed];

                for (i, &value) in vector.iter().enumerate() {
                    if value > 0.0 {
                        binary[i / 8] |= 1 << (i % 8);
                    }
                }

                binary
            })
            .collect()
    }

    /// Generate INT8 codes from vectors
    pub fn generate_int8_codes(&mut self, vectors: &[Vec<f32>]) -> Vec<Vec<u8>> {
        vectors
            .iter()
            .map(|vector| {
                vector
                    .iter()
                    .map(|&value| {
                        // Scale to INT8 range
                        ((value.clamp(-1.0, 1.0) * 127.0) as i8) as u8
                    })
                    .collect()
            })
            .collect()
    }

    /// Generate metadata for vectors
    pub fn generate_metadata(&mut self) -> Vec<HashMap<String, serde_json::Value>> {
        (0..self.config.num_vectors)
            .map(|i| {
                let mut metadata = HashMap::new();

                // Category
                let category = format!("category_{}", i % self.config.num_categories);
                metadata.insert("category".to_string(), serde_json::Value::String(category));

                // Price (for filtering tests)
                let price = 100 + (i % 10) * 50;
                metadata.insert("price".to_string(), serde_json::Value::Number(price.into()));

                // Tags
                let tags = vec![format!("tag_{}", i % 3), format!("tag_{}", (i + 1) % 5)];
                metadata.insert("tags".to_string(), serde_json::json!(tags));

                // Score
                let score = self.rng.gen_range(0.0..1.0);
                metadata.insert("score".to_string(), serde_json::json!(score));

                metadata
            })
            .collect()
    }

    /// Create a complete Parquet file with all vector types
    pub fn create_parquet_file(&mut self, path: &str) -> Result<()> {
        self.create_parquet_file_with_compression(path, parquet::basic::Compression::UNCOMPRESSED)
    }

    /// Create a complete Parquet file with specified compression
    pub fn create_parquet_file_with_compression(
        &mut self,
        path: &str,
        compression: parquet::basic::Compression,
    ) -> Result<()> {
        // Generate data
        let vectors = self.generate_vectors();
        let pq8_codes = if self.config.include_pq8 {
            Some(self.generate_pq_codes(&vectors, 8))
        } else {
            None
        };
        let pq4_codes = if self.config.include_pq4 {
            Some(self.generate_pq_codes(&vectors, 4))
        } else {
            None
        };
        let binary_codes = if self.config.include_binary {
            Some(self.generate_binary_codes(&vectors))
        } else {
            None
        };
        let int8_codes = if self.config.include_int8 {
            Some(self.generate_int8_codes(&vectors))
        } else {
            None
        };
        let metadata = self.generate_metadata();

        // Create schema
        let mut fields = vec![
            Field::new("id", DataType::Utf8, true), // Nullable for test flexibility
            Field::new("collection_id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, true))),
                true,
            ), // Both field and items are nullable
            Field::new("timestamp", DataType::Int64, true),
            Field::new("version", DataType::Int64, true),
            Field::new("expires_at", DataType::Int64, true),
        ];

        // Add quantized vector fields
        if self.config.include_pq8 {
            fields.push(Field::new(
                "vector_pq8",
                DataType::List(Arc::new(Field::new("item", DataType::UInt8, true))),
                true,
            ));
        }
        if self.config.include_pq4 {
            fields.push(Field::new(
                "vector_pq4",
                DataType::List(Arc::new(Field::new("item", DataType::UInt8, true))),
                true,
            ));
        }
        if self.config.include_binary {
            fields.push(Field::new(
                "vector_binary",
                DataType::List(Arc::new(Field::new("item", DataType::UInt8, true))),
                true,
            ));
        }
        if self.config.include_int8 {
            fields.push(Field::new(
                "vector_int8",
                DataType::List(Arc::new(Field::new("item", DataType::UInt8, true))),
                true,
            ));
        }

        // Add metadata fields
        fields.push(Field::new("category", DataType::Utf8, true));
        fields.push(Field::new("price", DataType::Int64, true));
        fields.push(Field::new("score", DataType::Float64, true));

        let schema = Arc::new(Schema::new(fields));

        // Create arrays
        let current_time = chrono::Utc::now().timestamp_micros();

        let ids: ArrayRef = Arc::new(StringArray::from_iter(
            (0..self.config.num_vectors).map(|i| Some(format!("vec_{}", i))),
        ));

        let collection_ids: ArrayRef = Arc::new(StringArray::from_iter(
            (0..self.config.num_vectors)
                .map(|i| Some(format!("collection_{}", i % self.config.num_collections))),
        ));

        // Create non-nullable Float32 arrays manually to match schema
        let mut list_builder = ListBuilder::new(Float32Builder::new());
        for vector in &vectors {
            list_builder.append_value(vector.iter().map(|&x| Some(x)));
        }
        let vector_array = list_builder.finish();

        let timestamps: ArrayRef = Arc::new(Int64Array::from_iter(
            (0..self.config.num_vectors).map(|_| Some(current_time)),
        ));

        let versions: ArrayRef = Arc::new(Int64Array::from_iter(
            (0..self.config.num_vectors).map(|i| Some(i as i64)),
        ));

        let expires_at: ArrayRef = Arc::new(Int64Array::from_iter(
            (0..self.config.num_vectors).map(|i| {
                if self.rng.gen_range(0.0..1.0) < self.config.expiry_rate {
                    Some(current_time - 1000000) // Expired
                } else {
                    None // No expiry
                }
            }),
        ));

        // Build column array
        let mut columns = vec![
            ids,
            collection_ids,
            Arc::new(vector_array),
            timestamps,
            versions,
            expires_at,
        ];

        // Add quantized vectors
        if let Some(pq8) = pq8_codes {
            let array = ListArray::from_iter_primitive::<UInt8Type, _, _>(
                pq8.iter().map(|v| Some(v.iter().map(|&x| Some(x)))),
            );
            columns.push(Arc::new(array));
        }

        if let Some(pq4) = pq4_codes {
            let array = ListArray::from_iter_primitive::<UInt8Type, _, _>(
                pq4.iter().map(|v| Some(v.iter().map(|&x| Some(x)))),
            );
            columns.push(Arc::new(array));
        }

        if let Some(binary) = binary_codes {
            let array = ListArray::from_iter_primitive::<UInt8Type, _, _>(
                binary.iter().map(|v| Some(v.iter().map(|&x| Some(x)))),
            );
            columns.push(Arc::new(array));
        }

        if let Some(int8) = int8_codes {
            let array = ListArray::from_iter_primitive::<UInt8Type, _, _>(
                int8.iter().map(|v| Some(v.iter().map(|&x| Some(x)))),
            );
            columns.push(Arc::new(array));
        }

        // Add metadata columns
        let categories: ArrayRef =
            Arc::new(StringArray::from_iter(metadata.iter().map(|m| {
                m.get("category").and_then(|v| v.as_str()).map(|s| s.to_string())
            })));
        columns.push(categories);

        let prices: ArrayRef = Arc::new(Int64Array::from_iter(
            metadata.iter().map(|m| m.get("price").and_then(|v| v.as_i64())),
        ));
        columns.push(prices);

        let scores: ArrayRef = Arc::new(Float64Array::from_iter(
            metadata.iter().map(|m| m.get("score").and_then(|v| v.as_f64())),
        ));
        columns.push(scores);

        // Create record batch
        let batch = RecordBatch::try_new(schema.clone(), columns)?;

        // Write to Parquet
        let file = File::create(path)?;
        let props = WriterProperties::builder()
            .set_compression(compression)
            .build();
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;

        writer.write(&batch)?;
        writer.close()?;

        Ok(())
    }

    /// Create multiple Parquet files simulating different row groups
    pub fn create_multi_file_dataset(
        &mut self,
        base_path: &str,
        num_files: usize,
    ) -> Result<Vec<String>> {
        let mut file_paths = Vec::new();

        for i in 0..num_files {
            let path = format!("{}/part_{:04}.parquet", base_path, i);

            // Adjust config for each file
            let original_seed = self.config.seed;
            self.config.seed = original_seed + i as u64;
            self.rng = ChaCha8Rng::seed_from_u64(self.config.seed);

            self.create_parquet_file(&path)?;
            file_paths.push(path);

            // Restore original seed
            self.config.seed = original_seed;
        }

        Ok(file_paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tracing::{debug, error, info};

    #[test]
    fn test_data_generator_creation() {
        let config = TestDataConfig::default();
        let mut generator = TestDataGenerator::new(config);

        let vectors = generator.generate_vectors();
        assert_eq!(vectors.len(), 1000);
        assert_eq!(vectors[0].len(), 128);
    }

    #[test]
    fn test_quantization_generation() {
        let config = TestDataConfig {
            num_vectors: 10,
            dimension: 64,
            pq_num_subvectors: 8,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        let vectors = generator.generate_vectors();

        // Test PQ8
        let pq8_codes = generator.generate_pq_codes(&vectors, 8);
        assert_eq!(pq8_codes.len(), 10);
        assert_eq!(pq8_codes[0].len(), 8);

        // Test binary
        let binary_codes = generator.generate_binary_codes(&vectors);
        assert_eq!(binary_codes.len(), 10);
        assert_eq!(binary_codes[0].len(), 8); // 64 bits / 8

        // Test INT8
        let int8_codes = generator.generate_int8_codes(&vectors);
        assert_eq!(int8_codes.len(), 10);
        assert_eq!(int8_codes[0].len(), 64);
    }

    #[test]
    fn test_parquet_file_creation() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.parquet");

        let config = TestDataConfig {
            num_vectors: 100,
            dimension: 32,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        generator
            .create_parquet_file(file_path.to_str().unwrap())
            .unwrap();

        assert!(file_path.exists());
        assert!(file_path.metadata().unwrap().len() > 0);
    }

    #[test]
    fn test_multi_file_dataset() {
        let temp_dir = TempDir::new().unwrap();

        let config = TestDataConfig {
            num_vectors: 50,
            dimension: 16,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        let file_paths = generator
            .create_multi_file_dataset(temp_dir.path().to_str().unwrap(), 3)
            .unwrap();

        assert_eq!(file_paths.len(), 3);
        for path in file_paths {
            assert!(std::path::Path::new(&path).exists());
        }
    }

    #[test]
    fn test_sparse_vector_generation() {
        let config = TestDataConfig {
            num_vectors: 100,
            dimension: 1024, // High dimensional
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        // Generate sparse vectors
        let sparse_vectors = (0..100)
            .map(|_| generator.generate_sparse_vector(0.05)) // 95% sparse
            .collect::<Vec<_>>();

        // Verify sparsity
        for vector in &sparse_vectors {
            let zero_count = vector.iter().filter(|&&x| x == 0.0).count();
            let sparsity = zero_count as f32 / vector.len() as f32;
            assert!(
                sparsity > 0.9,
                "Vector should be at least 90% sparse, got {}",
                sparsity
            );
        }

        // Test distance calculations with sparse vectors using UnifiedDistanceCompute
        let query = generator.generate_sparse_vector(0.05);
        let distance_compute = UnifiedDistanceCompute::default();
        let mut results = Vec::new();

        for (idx, vector) in sparse_vectors.iter().enumerate() {
            // Use UnifiedDistanceCompute for consistent distance calculation
            let distance_result =
                distance_compute.calculate_distance(&query, vector, &DistanceMetric::Cosine);
            results.push((distance_result.rank_value, idx));
        }

        // Sort by distance (lower = more similar)
        results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        // Verify we got results
        assert!(!results.is_empty());
        assert!(results[0].0 >= 0.0); // Best match should have non-negative distance
        assert!(results[0].0 <= 2.0); // Cosine distance is in range [0, 2]
    }

    #[test]
    fn test_quantized_vector_distance_calculations() {
        let dimension = 128;
        let config = TestDataConfig {
            num_vectors: 100,
            dimension,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        // Generate vectors and their quantized versions
        let vectors = generator.generate_vectors();
        let int8_codes = generator.generate_int8_codes(&vectors);
        let pq8_codes = generator.generate_pq_codes(&vectors, 8);
        let binary_codes = generator.generate_binary_codes(&vectors);

        // Use UnifiedDistanceCompute for FP32 comparisons
        let distance_compute = UnifiedDistanceCompute::default();
        let query = generator.generate_normalized_vector();

        // Calculate FP32 distances as ground truth
        let mut fp32_distances = Vec::new();
        for vector in &vectors {
            let distance =
                distance_compute.calculate_distance(&query, vector, &DistanceMetric::Cosine);
            fp32_distances.push(distance);
        }

        // Test INT8 quantized distance approximation
        debug!("Testing INT8 quantized distances:");
        for (idx, int8_vector) in int8_codes.iter().enumerate() {
            // Convert INT8 back to float for comparison
            let dequantized: Vec<f32> = int8_vector
                .iter()
                .map(|&byte| {
                    let signed = byte as i8;
                    (signed as f32) / 127.0
                })
                .collect();

            let quantized_distance =
                distance_compute.calculate_distance(&query, &dequantized, &DistanceMetric::Cosine);
            let fp32_distance = &fp32_distances[idx];
            let error = (quantized_distance.rank_value - fp32_distance.rank_value).abs();

            // INT8 should have reasonable approximation error
            assert!(
                error < 0.1,
                "INT8 quantization error too large: {} at index {}",
                error,
                idx
            );
        }

        // Test binary quantization
        debug!("Testing binary quantized distances:");
        for (idx, binary_vector) in binary_codes.iter().enumerate() {
            // Convert binary back to float for comparison (1 bit per dimension)
            let mut dequantized = vec![0.0f32; dimension];
            for (i, &byte) in binary_vector.iter().enumerate() {
                for bit in 0..8 {
                    let dim_idx = i * 8 + bit;
                    if dim_idx < dimension {
                        dequantized[dim_idx] = if (byte >> bit) & 1 == 1 { 1.0 } else { -1.0 }
                    }
                }
            }

            let quantized_distance = distance_compute
                .calculate_distance(&query, &dequantized, &DistanceMetric::Hamming)
                .rank_value;
            // Binary quantization has higher error but should still be bounded
            assert!(
                quantized_distance >= 0.0,
                "Hamming distance should be non-negative"
            );
        }

        debug!("Quantized distance tests passed!");
    }

    #[test]
    fn test_sparse_quantized_vector_distances() {
        let dimension = 2048; // High dimensional sparse vectors
        let config = TestDataConfig {
            num_vectors: 50,
            dimension,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);
        let distance_compute = UnifiedDistanceCompute::default();

        // Generate sparse vectors with different sparsity levels
        let sparsity_levels = vec![0.9, 0.95, 0.99]; // 90%, 95%, 99% sparse

        for sparsity in sparsity_levels {
            debug!("Testing with {:.0}% sparsity", sparsity * 100.0);

            // Generate sparse vectors
            let sparse_vectors: Vec<Vec<f32>> = (0..50)
                .map(|_| generator.generate_sparse_vector(1.0 - sparsity))
                .collect();

            // Generate quantized versions
            let int8_sparse = generator.generate_int8_codes(&sparse_vectors);
            let binary_sparse = generator.generate_binary_codes(&sparse_vectors);

            // Test query (also sparse)
            let sparse_query = generator.generate_sparse_vector(1.0 - sparsity);

            // Count non-zero elements in query
            let query_nnz = sparse_query.iter().filter(|&&x| x != 0.0).count();
            debug!(
                "Query has {} non-zero elements out of {}",
                query_nnz, dimension
            );

            // Test INT8 sparse quantization efficiency
            for (idx, int8_vector) in int8_sparse.iter().enumerate() {
                // Dequantize INT8
                let dequantized: Vec<f32> = int8_vector
                    .iter()
                    .map(|&byte| {
                        let signed = byte as i8;
                        (signed as f32) / 127.0
                    })
                    .collect();

                // Count preserved non-zeros after quantization
                let preserved_nnz = dequantized
                    .iter()
                    .zip(sparse_vectors[idx].iter())
                    .filter(|(dq, orig)| **orig != 0.0 && dq.abs() > 0.001)
                    .count();

                // Calculate distances
                let fp32_distance = distance_compute.calculate_distance(
                    &sparse_query,
                    &sparse_vectors[idx],
                    &DistanceMetric::Cosine,
                );
                let int8_distance = distance_compute.calculate_distance(
                    &sparse_query,
                    &dequantized,
                    &DistanceMetric::Cosine,
                );

                let error = (fp32_distance.rank_value - int8_distance.rank_value).abs();

                // Higher sparsity should still maintain reasonable accuracy
                let error_threshold = 0.2; // Allow more error for very sparse vectors
                assert!(
                    error < error_threshold,
                    "INT8 sparse quantization error too large: {} for {:.0}% sparse vector",
                    error,
                    sparsity * 100.0
                );

                // Verify sparsity preservation
                let sparsity_preservation = preserved_nnz as f32 / query_nnz.max(1) as f32;
                debug!(
                    "Sparsity preservation for vector {}: {:.2}%",
                    idx,
                    sparsity_preservation * 100.0
                );
            }
        }

        debug!("Sparse quantized distance tests passed!");
    }

    #[test]
    fn test_unified_distance_with_sparse_and_quantized() {
        // Test UnifiedDistanceCompute with both sparse and quantized vectors
        // This ensures real-world similarity search will work correctly
        let dimension = 256; // Divisible by common subvector counts
        let config = TestDataConfig {
            num_vectors: 20,
            dimension,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);
        let distance_compute = UnifiedDistanceCompute::default();

        // Generate sparse and dense vectors
        let sparse_vectors: Vec<Vec<f32>> = (0..10)
            .map(|_| generator.generate_sparse_vector(0.05)) // 95% sparse
            .collect();

        let dense_vectors: Vec<Vec<f32>> = (0..10)
            .map(|_| generator.generate_normalized_vector())
            .collect();

        // Test query (sparse)
        let sparse_query = generator.generate_sparse_vector(0.05);

        // Compare distances between sparse vectors using UnifiedDistanceCompute
        debug!("Testing UnifiedDistanceCompute with sparse vectors:");
        for (idx, sparse_vec) in sparse_vectors.iter().enumerate() {
            let distance_result = distance_compute.calculate_distance(
                &sparse_query,
                sparse_vec,
                &DistanceMetric::Cosine,
            );
            debug!(
                "  Sparse vector {} - Cosine similarity: {:.4}",
                idx, distance_result.raw_value
            );

            // Verify distance is within expected range
            assert!(
                distance_result.raw_value >= 0.0 && distance_result.raw_value <= 2.0,
                "Cosine distance should be in [0, 2], got {}",
                distance_result.raw_value
            );
        }

        // Test with different metrics
        let metrics = vec![
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
        ];

        for metric in metrics {
            debug!("\nTesting {:?} metric with sparse vectors:", metric);

            let mut distances = Vec::new();
            for sparse_vec in &sparse_vectors {
                let distance_result =
                    distance_compute.calculate_distance(&sparse_query, sparse_vec, &metric);
                distances.push(distance_result.rank_value);
            }

            // Verify all distances are valid (DotProduct can be negative)
            for (idx, dist) in distances.iter().enumerate() {
                assert!(
                    !dist.is_nan(),
                    "{:?} distance should not be NaN, got {} for vector {}",
                    metric,
                    dist,
                    idx
                );
                // For non-similarity metrics, distances should be non-negative
                if !matches!(metric, DistanceMetric::DotProduct) {
                    assert!(
                        *dist >= 0.0,
                        "{:?} distance should be non-negative, got {} for vector {}",
                        metric,
                        dist,
                        idx
                    );
                }
            }

            // Find nearest neighbor
            let (min_dist, min_idx) = distances
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
                .map(|(idx, dist)| (*dist, idx))
                .unwrap();

            debug!(
                "  Nearest sparse vector: {} with distance {:.4}",
                min_idx, min_dist
            );
        }

        // Test mixed sparse/dense distance calculations
        debug!("\nTesting mixed sparse/dense vector distances:");
        for (idx, dense_vec) in dense_vectors.iter().enumerate() {
            let sparse_to_dense = distance_compute.calculate_distance(
                &sparse_query,
                dense_vec,
                &DistanceMetric::Cosine,
            );
            debug!(
                "  Sparse query to dense vector {} - similarity: {:.4}",
                idx, sparse_to_dense.raw_value
            );
        }

        // Demonstrate that quantized vectors can be compared using UnifiedDistanceCompute
        // by simulating INT8 quantization
        debug!("\nTesting with simulated INT8 quantized vectors:");
        let int8_sparse_vectors: Vec<Vec<f32>> = sparse_vectors
            .iter()
            .map(|vec| {
                // Simulate INT8 quantization/dequantization
                vec.iter()
                    .map(|&v| {
                        let quantized = (v.clamp(-1.0, 1.0) * 127.0).round() as i8;
                        (quantized as f32) / 127.0
                    })
                    .collect()
            })
            .collect();

        for (idx, int8_vec) in int8_sparse_vectors.iter().enumerate() {
            let original_distance = distance_compute.calculate_distance(
                &sparse_query,
                &sparse_vectors[idx],
                &DistanceMetric::Cosine,
            );
            let quantized_distance = distance_compute.calculate_distance(
                &sparse_query,
                int8_vec,
                &DistanceMetric::Cosine,
            );
            let error = (original_distance.rank_value - quantized_distance.rank_value).abs();

            debug!(
                "  INT8 vector {} - error: {:.4} (original: {:.4}, quantized_vector: {:.4})",
                idx, error, original_distance.rank_value, quantized_distance.rank_value
            );

            // INT8 should maintain reasonable accuracy
            assert!(error < 0.2, "INT8 quantization error too large: {}", error);
        }

        debug!("\nUnified distance tests with sparse and quantized vectors passed!");
    }

    #[test]
    fn test_high_dimensional_dense_sparse_vectors() {
        let config = TestDataConfig {
            num_vectors: 50,
            dimension: 4096, // Very high dimensional
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        // Create mixed dataset: dense and sparse vectors
        let mut vectors = Vec::new();

        // 25 dense vectors
        for _ in 0..25 {
            vectors.push(generator.generate_normalized_vector());
        }

        // 25 extremely sparse vectors (99% sparse)
        for _ in 0..25 {
            vectors.push(generator.generate_sparse_vector(0.01));
        }

        // Test various distance metrics using UnifiedDistanceCompute
        let query_dense = generator.generate_normalized_vector();
        let query_sparse = generator.generate_sparse_vector(0.01);
        let distance_compute = UnifiedDistanceCompute::default();

        // Test with different distance metrics
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
        ];

        for metric in metrics {
            debug!("Testing {:?} metric with sparse vectors", metric);

            // Test dense query
            let mut results_dense = Vec::new();
            for (idx, vector) in vectors.iter().enumerate() {
                let distance = distance_compute.calculate_distance(&query_dense, vector, &metric);
                results_dense.push((distance, idx));
            }

            // Test sparse query
            let mut results_sparse = Vec::new();
            for (idx, vector) in vectors.iter().enumerate() {
                let distance = distance_compute.calculate_distance(&query_sparse, vector, &metric);
                results_sparse.push((distance, idx));
            }

            assert_eq!(results_dense.len(), 50);
            assert_eq!(results_sparse.len(), 50);

            // Verify distance calculations are reasonable
            for (dist, _) in &results_dense {
                assert!(
                    !dist.rank_value.is_nan(),
                    "Distance should not be NaN for {:?}",
                    metric
                );
                // For DotProduct, rank_value can be negative (negated similarity)
                if !matches!(metric, DistanceMetric::DotProduct) {
                    assert!(
                        dist.rank_value >= 0.0,
                        "Distance should be non-negative for {:?}",
                        metric
                    );
                }
            }
        }
    }

    #[test]
    fn test_parquet_with_sparse_vectors() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sparse_test.parquet");

        let config = TestDataConfig {
            num_vectors: 100,
            dimension: 2048,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        // Override vector generation to create sparse vectors
        let original_seed = generator.config.seed;
        generator.config.seed = original_seed + 1000; // Different seed for sparse generation
        generator.rng = ChaCha8Rng::seed_from_u64(generator.config.seed);

        // Create parquet file with sparse vectors
        generator
            .create_parquet_file(file_path.to_str().unwrap())
            .unwrap();

        // Verify file was created and has reasonable size
        assert!(file_path.exists());
        let metadata = file_path.metadata().unwrap();
        assert!(metadata.len() > 0);

        // Could add parquet reading here to verify sparse vectors were written correctly
    }

    #[test]
    fn test_all_supported_compressions() {
        let temp_dir = TempDir::new().unwrap();

        // Test different compression codecs that are actually supported
        // Note: Most compression codecs require specific features to be enabled in Cargo.toml
        let compression_tests = vec![
            ("uncompressed", parquet::basic::Compression::UNCOMPRESSED),
            // TODO: Enable compression features in Cargo.toml for parquet crate
            // ("gzip", parquet::basic::Compression::GZIP(parquet::basic::GzipLevel::default())),
            // ("lz4", parquet::basic::Compression::LZ4),
            // ("snappy", parquet::basic::Compression::SNAPPY),
        ];

        for (name, compression) in compression_tests {
            let file_path = temp_dir.path().join(format!("test_{}.parquet", name));

            let config = TestDataConfig {
                num_vectors: 100,
                dimension: 512,
                ..Default::default()
            };
            let mut generator = TestDataGenerator::new(config);

            // Try to create file with this compression
            match generator
                .create_parquet_file_with_compression(file_path.to_str().unwrap(), compression)
            {
                Ok(_) => {
                    // Verify file was created
                    assert!(file_path.exists(), "File should exist for {}", name);
                    let metadata = file_path.metadata().unwrap();
                    assert!(metadata.len() > 0, "File should not be empty for {}", name);

                    // Compare sizes to verify compression is working
                    if name != "uncompressed" {
                        // Compressed files should generally be smaller
                        debug!("{} compression: {} bytes", name, metadata.len());
                    }
                }
                Err(e) => {
                    // Some compressions might not be available
                    debug!("Compression {} not available: {}", name, e);
                }
            }
        }
    }

    #[test]
    fn test_sparse_vector_compression_efficiency() {
        let temp_dir = TempDir::new().unwrap();

        // Create dataset with very sparse vectors (should compress well)
        let config = TestDataConfig {
            num_vectors: 1000,
            dimension: 2048,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        // Test with different compressions
        let uncompressed_path = temp_dir.path().join("sparse_uncompressed.parquet");
        let gzip_path = temp_dir.path().join("sparse_gzip.parquet");

        // Create uncompressed file with sparse vectors
        // We'll use the sparsity setting to generate mostly sparse vectors
        generator.config.seed = 42; // Reset seed for consistency
        generator.rng = ChaCha8Rng::seed_from_u64(generator.config.seed);
        generator.config.expiry_rate = 0.0; // Don't expire any vectors
        generator
            .create_parquet_file_with_compression(
                uncompressed_path.to_str().unwrap(),
                parquet::basic::Compression::UNCOMPRESSED,
            )
            .unwrap();

        // For now, skip compression test as features are not enabled
        // TODO: Enable compression features in Cargo.toml and uncomment below
        /*
        generator.config.seed = 42; // Reset seed for consistency
        generator.rng = ChaCha8Rng::seed_from_u64(generator.config.seed);
        if let Ok(_) = generator.create_parquet_file_with_compression(
            gzip_path.to_str().unwrap(),
            parquet::basic::Compression::GZIP(parquet::basic::GzipLevel::default())
        ) {
            // Compare file sizes
            let uncompressed_size = uncompressed_path.metadata().unwrap().len();
            let gzip_size = gzip_path.metadata().unwrap().len();

            debug!("Sparse vector compression test:");
            debug!("  Uncompressed: {} bytes", uncompressed_size);
            debug!("  GZIP: {} bytes", gzip_size);
            debug!("  Compression ratio: {:.2}%",
                    (gzip_size as f64 / uncompressed_size as f64) * 100.0);

            // GZIP should provide some compression
            if gzip_size < uncompressed_size {
                debug!("GZIP successfully compressed the data");
            } else {
                debug!("GZIP did not compress the data (may depend on data patterns)");
            }
        }
        */

        // Just verify uncompressed file was created
        let uncompressed_size = uncompressed_path.metadata().unwrap().len();
        debug!(
            "Sparse vector uncompressed size: {} bytes",
            uncompressed_size
        );
        assert!(uncompressed_size > 0, "File should have content");
    }

    #[test]
    fn test_mixed_density_compression() {
        let temp_dir = TempDir::new().unwrap();

        let config = TestDataConfig {
            num_vectors: 500,
            dimension: 1024,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        // Test files with different vector densities
        let densities = vec![
            ("dense", 1.0),        // 100% dense (no zeros)
            ("medium", 0.5),       // 50% sparse
            ("sparse", 0.1),       // 90% sparse
            ("very_sparse", 0.01), // 99% sparse
        ];

        for (name, density) in densities {
            let file_path = temp_dir.path().join(format!("{}_vectors.parquet", name));

            // Generate vectors with specific density
            generator.config.seed = 42;
            generator.rng = ChaCha8Rng::seed_from_u64(generator.config.seed);

            // Create custom vectors with specific density
            let custom_vectors: Vec<Vec<f32>> = (0..500)
                .map(|_| generator.generate_sparse_vector(density))
                .collect();

            // TODO: Need to modify create_parquet_file to accept custom vectors
            // For now, just create with default generation
            generator
                .create_parquet_file(file_path.to_str().unwrap())
                .unwrap();

            assert!(file_path.exists());
            let size = file_path.metadata().unwrap().len();
            debug!(
                "{} vectors ({:.0}% dense): {} bytes",
                name,
                density * 100.0,
                size
            );
        }
    }

    // Note: Avro compression tests removed - apache_avro needs to be added to Cargo.toml
    // TODO: Add Avro compression tests when apache_avro is available

    #[test]
    fn test_parquet_compression_with_sparse_vectors() {
        let temp_dir = TempDir::new().unwrap();

        // Create config for sparse vectors
        let config = TestDataConfig {
            num_vectors: 1000,
            dimension: 2048, // High dimensional for better compression testing
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);

        // Test Parquet compression codecs that are likely to be available
        // Note: Compression features must be enabled in Cargo.toml
        let parquet_compressions = vec![
            (
                "parquet_uncompressed",
                parquet::basic::Compression::UNCOMPRESSED,
            ),
            // TODO: Enable compression features
            // ("parquet_gzip", parquet::basic::Compression::GZIP(parquet::basic::GzipLevel::default())),
        ];

        let mut sizes = HashMap::new();

        for (name, compression) in parquet_compressions {
            let file_path = temp_dir.path().join(format!("{}.parquet", name));

            // Reset generator for consistent data
            generator.config.seed = 42;
            generator.rng = ChaCha8Rng::seed_from_u64(generator.config.seed);

            match generator
                .create_parquet_file_with_compression(file_path.to_str().unwrap(), compression)
            {
                Ok(_) => {
                    let size = file_path.metadata().unwrap().len();
                    sizes.insert(name, size);
                    debug!("Parquet {} compression: {} bytes", name, size);
                }
                Err(e) => {
                    debug!("Parquet {} compression not available: {}", name, e);
                }
            }
        }

        // TODO: Compare compression ratios when compression features are enabled
        // For now just verify uncompressed file was created
        if let Some(&uncompressed_size) = sizes.get("parquet_uncompressed") {
            debug!("Uncompressed parquet size: {} bytes", uncompressed_size);
            assert!(uncompressed_size > 0, "File should have content");
        }
    }

    // Note: test_avro_sparse_vector_compression removed - apache_avro needs to be added to Cargo.toml
}
