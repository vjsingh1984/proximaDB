/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Centralized test data generation utilities
//!
//! This module provides standard test data generators used across all tests
//! to ensure consistency and avoid duplication.

use proximadb::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;

/// Standard test vector generator with consistent behavior
pub struct TestVectorGenerator {
    rng: StdRng,
    dimension: usize,
}

impl TestVectorGenerator {
    /// Create a new generator with a specific seed for reproducibility
    pub fn new(dimension: usize, seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            dimension,
        }
    }

    /// Create a generator with default seed
    pub fn default_with_dimension(dimension: usize) -> Self {
        Self::new(dimension, 42)
    }

    /// Generate a batch of test vectors
    pub fn generate_vectors(&mut self, count: usize, prefix: &str) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| self.generate_single_vector(format!("{}-{}", prefix, i)))
            .collect()
    }

    /// Generate a single test vector
    pub fn generate_single_vector(&mut self, id: String) -> VectorRecord {
        let vector: Vec<f32> = (0..self.dimension)
            .map(|_| self.rng.gen_range(-1.0..1.0))
            .collect();

        VectorRecord {
            id,
            vector,
            metadata: self.generate_metadata(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    /// Generate test vectors with specific patterns
    pub fn generate_pattern_vectors(
        &mut self,
        count: usize,
        pattern: VectorPattern,
    ) -> Vec<VectorRecord> {
        match pattern {
            VectorPattern::Random => self.generate_vectors(count, "random"),
            VectorPattern::Sequential => self.generate_sequential_vectors(count),
            VectorPattern::Clustered { clusters } => {
                self.generate_clustered_vectors(count, clusters)
            }
            VectorPattern::Sparse { sparsity } => self.generate_sparse_vectors(count, sparsity),
        }
    }

    /// Generate sequential vectors (useful for testing ordering)
    fn generate_sequential_vectors(&mut self, count: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| {
                let vector: Vec<f32> = (0..self.dimension)
                    .map(|j| (i * self.dimension + j) as f32 / (count * self.dimension) as f32)
                    .collect();

                VectorRecord {
                    id: format!("seq-{}", i),
                    vector,
                    metadata: self.generate_metadata(),
                    timestamp: Some(i as i64),
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                }
            })
            .collect()
    }

    /// Generate clustered vectors (useful for testing clustering algorithms)
    fn generate_clustered_vectors(&mut self, count: usize, clusters: usize) -> Vec<VectorRecord> {
        let vectors_per_cluster = count / clusters;
        let mut result = Vec::new();

        for cluster_id in 0..clusters {
            // Generate cluster centroid
            let centroid: Vec<f32> = (0..self.dimension)
                .map(|_| self.rng.gen_range(-1.0..1.0))
                .collect();

            // Generate vectors around centroid
            for i in 0..vectors_per_cluster {
                let mut vector = centroid.clone();
                for v in &mut vector {
                    *v += self.rng.gen_range(-0.1..0.1); // Small perturbation
                }

                result.push(VectorRecord {
                    id: format!("cluster-{}-{}", cluster_id, i),
                    vector,
                    metadata: self.generate_metadata_with_cluster(cluster_id),
                    timestamp: (cluster_id * vectors_per_cluster + i) as i64,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                });
            }
        }

        result
    }

    /// Generate sparse vectors (useful for testing sparse data handling)
    fn generate_sparse_vectors(&mut self, count: usize, sparsity: f32) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| {
                let vector: Vec<f32> = (0..self.dimension)
                    .map(|_| {
                        if self.rng.gen_range(0.0..1.0) < sparsity {
                            0.0
                        } else {
                            self.rng.gen_range(-1.0..1.0)
                        }
                    })
                    .collect();

                VectorRecord {
                    id: format!("sparse-{}", i),
                    vector,
                    metadata: self.generate_metadata(),
                    timestamp: Some(i as i64),
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                }
            })
            .collect()
    }

    /// Generate random metadata
    fn generate_metadata(&mut self) -> HashMap<String, SqlValue> {
        HashMap::from([
            ("category".to_string(), SqlValue {
                value: Some(sql_value::Value::StringValue(format!(
                    "cat-{}",
                    self.rng.gen_range(0..5)
                ))),
            }),
            ("score".to_string(), SqlValue {
                value: Some(sql_value::Value::NumberValue(
                    self.rng.gen_range(0.0..100.0),
                )),
            }),
            ("active".to_string(), SqlValue {
                value: Some(sql_value::Value::BoolValue(
                    self.rng.gen_bool(0.7),
                )),
            }),
        ])
    }

    /// Generate metadata with cluster information
    fn generate_metadata_with_cluster(&mut self, cluster_id: usize) -> HashMap<String, SqlValue> {
        let mut metadata = self.generate_metadata();
        metadata.insert("cluster_id".to_string(), SqlValue {
            value: Some(sql_value::Value::Int64Value(cluster_id as i64)),
        });
        metadata
    }
}

/// Patterns for generating test vectors
pub enum VectorPattern {
    Random,
    Sequential,
    Clustered { clusters: usize },
    Sparse { sparsity: f32 },
}

/// Quick helper function for simple cases
pub fn generate_test_vectors(count: usize, dimension: usize, prefix: &str) -> Vec<VectorRecord> {
    let mut generator = TestVectorGenerator::default_with_dimension(dimension);
    generator.generate_vectors(count, prefix)
}

/// Generate test query vector
pub fn generate_query_vector(dimension: usize, seed: u64) -> Vec<f32> {
    let mut rng = StdRng::seed_from_u64(seed);
    (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_generation() {
        let mut generator = TestVectorGenerator::new(128, 42);
        let vectors = generator.generate_vectors(10, "test");
        assert_eq!(vectors.len(), 10);
        assert_eq!(vectors[0].vector.len(), 128);
    }

    #[test]
    fn test_pattern_generation() {
        let mut generator = TestVectorGenerator::new(64, 42);

        let clustered =
            generator.generate_pattern_vectors(100, VectorPattern::Clustered { clusters: 5 });
        assert_eq!(clustered.len(), 100);

        let sparse =
            generator.generate_pattern_vectors(50, VectorPattern::Sparse { sparsity: 0.9 });
        assert_eq!(sparse.len(), 50);

        // Check sparsity
        let zero_count: usize = sparse
            .iter()
            .flat_map(|v| &v.vector)
            .filter(|&&x| x == 0.0)
            .count();
        let total_count = sparse.len() * 64;
        let sparsity_ratio = zero_count as f32 / total_count as f32;
        assert!(sparsity_ratio > 0.8); // Should be close to 0.9
    }
}
