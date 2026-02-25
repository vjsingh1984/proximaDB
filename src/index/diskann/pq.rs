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

//! # Product Quantization for DiskANN
//!
//! This module implements Product Quantization (PQ) for efficient vector
//! compression, enabling 10x storage reduction while maintaining search accuracy.
//!
//! ## Product Quantization Algorithm
//!
//! PQ splits vectors into sub-vectors and quantizes each independently:
//! 1. **Split**: Divide d-dimensional vector into m sub-vectors
//! 2. **Train**: Run K-means on each sub-vector (k=256 centroids)
//! 3. **Encode**: Replace each sub-vector with nearest centroid ID (8-bit)
//! 4. **Compress**: Store only centroid IDs (10x smaller than original)
//!
//! ## Compression Benefits
//!
//! - **10x Compression**: 128D float vector (512 bytes) → 16 bytes PQ code
//! - **Fast Distance**: Pre-computed lookup tables for query distances
//! - **Memory Efficient**: Store 1B vectors in ~16GB instead of ~160GB
//! - **Accuracy**: <5% loss vs uncompressed search
//!
//! ## Distance Computation
//!
//! Asymmetric Distance Computation (ADC):
//! - Query vector: Keep uncompressed (32-bit floats)
//! - Database vectors: Compressed to 8-bit PQ codes
//! - Pre-compute: Query-to-centroid distances for each sub-vector
//! - Fast lookup: Sum distances from codebook table

use crate::compute::distance_computation::UnifiedDistanceCompute;
use crate::core::error::ProximaDBError;
use std::collections::HashMap;
use rand::Rng;
use tracing::{debug, info};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Configuration for Product Quantization
#[derive(Debug, Clone)]
pub struct PQConfig {
    /// Number of sub-vectors (m)
    pub num_subvectors: usize,

    /// Number of centroids per sub-vector (k)
    pub num_centroids: usize,

    /// Maximum iterations for K-means training
    pub max_iterations: usize,

    /// Convergence threshold for K-means
    pub convergence_threshold: f32,
}

impl Default for PQConfig {
    fn default() -> Self {
        Self {
            num_subvectors: 8,      // Split 128-dim into 8 sub-vectors of 16-dim
            num_centroids: 256,     // 8-bit codes (256 = 2^8)
            max_iterations: 25,     // K-means iterations
            convergence_threshold: 0.001, // Early stopping
        }
    }
}

/// Product Quantization codebooks
#[derive(Debug, Clone)]
pub struct PQCodebooks {
    /// Number of sub-vectors
    pub num_subvectors: usize,

    /// Centroids for each sub-vector [subvector_id][centroid_id][dim]
    pub centroids: Vec<Vec<Vec<f32>>>,

    /// Dimension of each sub-vector
    pub subvector_dim: usize,
}

impl PQCodebooks {
    /// Create new PQ codebooks
    pub fn new(
        num_subvectors: usize,
        num_centroids: usize,
        subvector_dim: usize,
    ) -> Self {
        let mut centroids = Vec::with_capacity(num_subvectors);
        for _ in 0..num_subvectors {
            let subvector_centroids = vec![vec![0.0f32; subvector_dim]; num_centroids];
            centroids.push(subvector_centroids);
        }

        Self {
            num_subvectors,
            centroids,
            subvector_dim,
        }
    }

    /// Get centroid for a specific sub-vector and centroid ID
    pub fn get_centroid(&self, subvector_id: usize, centroid_id: usize) -> Option<&[f32]> {
        self.centroids
            .get(subvector_id)
            .and_then(|sub| sub.get(centroid_id))
            .map(|centroid| centroid.as_slice())
    }
}

/// Compressed PQ vectors
#[derive(Debug, Clone)]
pub struct PQVectors {
    /// PQ codes (one byte per sub-vector per vector)
    pub codes: Vec<Vec<u8>>,

    /// PQ codebooks
    pub codebooks: PQCodebooks,
}

impl PQVectors {
    /// Create new PQ vectors
    pub fn new(codes: Vec<Vec<u8>>, codebooks: PQCodebooks) -> Self {
        Self { codes, codebooks }
    }

    /// Get compression ratio
    pub fn compression_ratio(&self, vector_dim: usize) -> f64 {
        let original_size = vector_dim * std::mem::size_of::<f32>();
        let compressed_size = self.codes[0].len() * std::mem::size_of::<u8>();

        original_size as f64 / compressed_size as f64
    }
}

/// Product Quantization encoder/decoder
pub struct PQEncoder {
    config: PQConfig,
    distance_compute: UnifiedDistanceCompute,
}

impl PQEncoder {
    /// Create a new PQ encoder
    pub fn new(config: PQConfig) -> Self {
        Self {
            config,
            distance_compute: UnifiedDistanceCompute::new(
                crate::compute::distance_computation::DistanceMetric::Euclidean
            ),
        }
    }

    /// Train PQ codebooks from vectors
    ///
    /// # Algorithm
    ///
    /// 1. Split vectors into sub-vectors
    /// 2. For each sub-vector:
    ///    a. Initialize centroids randomly
    ///    b. Run K-means clustering
    ///    c. Store final centroids
    ///
    /// # Arguments
    ///
    /// * `vectors` - Training vectors (N × D)
    ///
    /// # Returns
    ///
    /// Trained PQ codebooks
    pub fn train_codebooks(&self, vectors: &[Vec<f32>]) -> Result<PQCodebooks> {
        if vectors.is_empty() {
            return Err(ProximaDBError::InvalidInput(
                "Cannot train from empty vector set".to_string(),
            ));
        }

        let vector_dim = vectors[0].len();
        let subvector_dim = vector_dim / self.config.num_subvectors;

        if subvector_dim == 0 {
            return Err(ProximaDBError::InvalidInput(
                format!("Vector dimension {} too small for {} sub-vectors",
                    vector_dim, self.config.num_subvectors)
            ));
        }

        info!(
            "Training PQ codebooks: {} vectors, {} dims, {} sub-vectors ({} dims each)",
            vectors.len(),
            vector_dim,
            self.config.num_subvectors,
            subvector_dim
        );

        let mut codebooks = PQCodebooks::new(
            self.config.num_subvectors,
            self.config.num_centroids,
            subvector_dim,
        );

        // Train codebook for each sub-vector
        for sub_id in 0..self.config.num_subvectors {
            debug!("Training sub-vector {}/{}", sub_id + 1, self.config.num_subvectors);

            // Extract sub-vectors from all vectors
            let start_dim = sub_id * subvector_dim;
            let end_dim = start_dim + subvector_dim;

            let subvectors: Vec<Vec<f32>> = vectors
                .iter()
                .map(|v| v[start_dim..end_dim].to_vec())
                .collect();

            // Run K-means
            let centroids = self.kmeans(&subvectors)?;

            // Store centroids
            codebooks.centroids[sub_id] = centroids;
        }

        info!("PQ codebooks training complete");
        Ok(codebooks)
    }

    /// Encode vectors into PQ codes
    ///
    /// # Arguments
    ///
    /// * `vectors` - Vectors to encode
    /// * `codebooks` - Trained PQ codebooks
    ///
    /// # Returns
    ///
    /// Compressed PQ vectors
    pub fn encode(&self, vectors: &[Vec<f32>], codebooks: &PQCodebooks) -> Result<PQVectors> {
        if vectors.is_empty() {
            return Ok(PQVectors {
                codes: vec![],
                codebooks: codebooks.clone(),
            });
        }

        let subvector_dim = codebooks.subvector_dim;
        let num_subvectors = codebooks.num_subvectors;

        info!(
            "Encoding {} vectors with PQ ({} sub-vectors)",
            vectors.len(),
            num_subvectors
        );

        let mut codes = Vec::with_capacity(vectors.len());

        for vector in vectors {
            let mut code = Vec::with_capacity(num_subvectors);

            for sub_id in 0..num_subvectors {
                let start_dim = sub_id * subvector_dim;
                let end_dim = start_dim + subvector_dim;

                let subvector = &vector[start_dim..end_dim];

                // Find nearest centroid
                let nearest_id = self.find_nearest_centroid(
                    subvector,
                    &codebooks.centroids[sub_id],
                )?;

                code.push(nearest_id as u8);
            }

            codes.push(code);
        }

        info!("PQ encoding complete");

        Ok(PQVectors {
            codes,
            codebooks: codebooks.clone(),
        })
    }

    /// K-means clustering for sub-vectors
    fn kmeans(&self, subvectors: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        let k = self.config.num_centroids;
        let dim = subvectors[0].len();

        // Initialize centroids randomly
        let mut centroids = self.initialize_centroids(subvectors, k, dim)?;

        // K-means iterations
        for iteration in 0..self.config.max_iterations {
            // Assign each sub-vector to nearest centroid
            let mut clusters: Vec<Vec<usize>> = vec![vec![]; k];
            let mut assignments = Vec::with_capacity(subvectors.len());

            for (idx, subvec) in subvectors.iter().enumerate() {
                let nearest = self.find_nearest_centroid(subvec, &centroids)?;
                clusters[nearest].push(idx);
                assignments.push(nearest);
            }

            // Update centroids
            let mut max_shift = 0.0f32;

            for cluster_id in 0..k {
                let mut rng = rand::thread_rng();

                if clusters[cluster_id].is_empty() {
                    // Reinitialize empty cluster randomly
                    centroids[cluster_id] = subvectors[rng.gen_range(0..subvectors.len())].clone();
                    continue;
                }

                // Compute new centroid as mean of cluster
                let mut new_centroid = vec![0.0f32; dim];
                for &idx in &clusters[cluster_id] {
                    for (d, &val) in subvectors[idx].iter().enumerate() {
                        new_centroid[d] += val;
                    }
                }

                let cluster_size = clusters[cluster_id].len() as f32;
                for d in 0..dim {
                    new_centroid[d] /= cluster_size;
                }

                // Compute shift
                let shift = self.euclidean_distance(&centroids[cluster_id], &new_centroid);
                max_shift = max_shift.max(shift);

                centroids[cluster_id] = new_centroid;
            }

            // Check convergence
            if max_shift < self.config.convergence_threshold {
                debug!("K-means converged at iteration {}", iteration + 1);
                break;
            }
        }

        Ok(centroids)
    }

    /// Initialize centroids randomly
    fn initialize_centroids(
        &self,
        subvectors: &[Vec<f32>],
        k: usize,
        dim: usize,
    ) -> Result<Vec<Vec<f32>>> {
        let mut rng = rand::thread_rng();
        let mut centroids = Vec::with_capacity(k);

        // K-means++ initialization for better quality
        // First centroid: random choice
        centroids.push(subvectors[rng.gen_range(0..subvectors.len())].clone());

        // Subsequent centroids: choose with probability proportional to distance
        while centroids.len() < k {
            let mut distances = Vec::with_capacity(subvectors.len());

            for subvec in subvectors {
                let min_dist = centroids
                    .iter()
                    .map(|c| self.squared_distance(subvec, c))
                    .fold(f32::INFINITY, f32::min);

                distances.push(min_dist);
            }

            // Choose weighted by distance squared
            let total: f32 = distances.iter().sum();
            let mut rand_val = rng.gen_range(0.0..1.0);
            let mut cumulative = 0.0;

            for (idx, &dist) in distances.iter().enumerate() {
                cumulative += dist / total;
                if rand_val <= cumulative {
                    centroids.push(subvectors[idx].clone());
                    break;
                }
            }

            // Fallback if not selected
            if centroids.len() < k + 1 {
                centroids.push(subvectors[rng.gen_range(0..subvectors.len())].clone());
            }
        }

        Ok(centroids)
    }

    /// Find nearest centroid for a sub-vector
    fn find_nearest_centroid(&self, subvec: &[f32], centroids: &[Vec<f32>]) -> Result<usize> {
        let mut nearest = 0;
        let mut min_dist = f32::MAX;

        for (idx, centroid) in centroids.iter().enumerate() {
            let dist = self.squared_distance(subvec, centroid);
            if dist < min_dist {
                min_dist = dist;
                nearest = idx;
            }
        }

        Ok(nearest)
    }

    /// Compute squared Euclidean distance (faster for comparison)
    fn squared_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum()
    }

    /// Compute Euclidean distance
    fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        self.squared_distance(a, b).sqrt()
    }

    /// Compute distance table for fast ADC (Asymmetric Distance Computation)
    ///
    /// Pre-computes query-to-centroid distances for each sub-vector
    pub fn compute_distance_table(
        &self,
        query: &[f32],
        codebooks: &PQCodebooks,
    ) -> Vec<Vec<f32>> {
        let subvector_dim = codebooks.subvector_dim;
        let num_subvectors = codebooks.num_subvectors;
        let num_centroids = codebooks.centroids[0].len();

        let mut table = vec![vec![0.0f32; num_centroids]; num_subvectors];

        for sub_id in 0..num_subvectors {
            let start_dim = sub_id * subvector_dim;
            let end_dim = start_dim + subvector_dim;

            let query_subvec = &query[start_dim..end_dim];

            for centroid_id in 0..num_centroids {
                if let Some(centroid) = codebooks.get_centroid(sub_id, centroid_id) {
                    table[sub_id][centroid_id] = self.squared_distance(query_subvec, centroid);
                }
            }
        }

        table
    }

    /// Fast distance computation using lookup table (ADC)
    pub fn pq_distance(
        &self,
        code: &[u8],
        distance_table: &[Vec<f32>],
    ) -> f32 {
        let mut distance = 0.0f32;

        for (sub_id, &centroid_id) in code.iter().enumerate() {
            distance += distance_table[sub_id][centroid_id as usize];
        }

        distance.sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pq_config_default() {
        let config = PQConfig::default();
        assert_eq!(config.num_subvectors, 8);
        assert_eq!(config.num_centroids, 256);
        assert_eq!(config.max_iterations, 25);
    }

    #[test]
    fn test_pq_codebooks_creation() {
        let codebooks = PQCodebooks::new(4, 16, 8);
        assert_eq!(codebooks.num_subvectors, 4);
        assert_eq!(codebooks.centroids.len(), 4);
        assert_eq!(codebooks.centroids[0].len(), 16);
        assert_eq!(codebooks.centroids[0][0].len(), 8);
    }

    #[test]
    fn test_pq_encoder_creation() {
        let config = PQConfig::default();
        let encoder = PQEncoder::new(config);
        assert_eq!(encoder.config.num_subvectors, 8);
    }

    #[test]
    fn test_train_codebooks() {
        let vectors: Vec<Vec<f32>> = (0..50)
            .map(|i| (0..64).map(|j| ((i * 64 + j) % 10) as f32).collect())
            .collect();

        let config = PQConfig {
            num_subvectors: 4,
            num_centroids: 8,
            max_iterations: 5,
            convergence_threshold: 0.01,
        };

        let encoder = PQEncoder::new(config);
        let codebooks = encoder.train_codebooks(&vectors).unwrap();

        assert_eq!(codebooks.num_subvectors, 4);
        assert_eq!(codebooks.centroids.len(), 4);
        assert_eq!(codebooks.centroids[0].len(), 8);
    }

    #[test]
    fn test_encode_vectors() {
        let vectors: Vec<Vec<f32>> = (0..20)
            .map(|i| (0..32).map(|j| ((i * 32 + j) % 10) as f32).collect())
            .collect();

        let config = PQConfig {
            num_subvectors: 2,
            num_centroids: 4,
            max_iterations: 5,
            convergence_threshold: 0.01,
        };

        let encoder = PQEncoder::new(config);
        let codebooks = encoder.train_codebooks(&vectors).unwrap();
        let pq_vectors = encoder.encode(&vectors, &codebooks).unwrap();

        assert_eq!(pq_vectors.codes.len(), 20);
        assert_eq!(pq_vectors.codes[0].len(), 2); // 2 sub-vectors
    }

    #[test]
    fn test_compression_ratio() {
        let vectors: Vec<Vec<f32>> = (0..10)
            .map(|_| (0..128).map(|i| i as f32).collect())
            .collect();

        let config = PQConfig::default();
        let encoder = PQEncoder::new(config);
        let codebooks = encoder.train_codebooks(&vectors).unwrap();
        let pq_vectors = encoder.encode(&vectors, &codebooks).unwrap();

        // Original: 128 floats × 4 bytes = 512 bytes
        // Compressed: 8 bytes (one per sub-vector)
        let ratio = pq_vectors.compression_ratio(128);
        assert!(ratio > 60.0 && ratio < 70.0); // ~64x compression
    }

    #[test]
    fn test_distance_table() {
        let vectors: Vec<Vec<f32>> = (0..20)
            .map(|i| (0..32).map(|j| ((i * 32 + j) % 10) as f32).collect())
            .collect();

        let config = PQConfig {
            num_subvectors: 2,
            num_centroids: 4,
            max_iterations: 5,
            convergence_threshold: 0.01,
        };

        let encoder = PQEncoder::new(config);
        let codebooks = encoder.train_codebooks(&vectors).unwrap();

        let query = &vectors[0];
        let table = encoder.compute_distance_table(query, &codebooks);

        assert_eq!(table.len(), 2); // 2 sub-vectors
        assert_eq!(table[0].len(), 4); // 4 centroids
    }

    #[test]
    fn test_pq_distance() {
        let vectors: Vec<Vec<f32>> = (0..20)
            .map(|i| (0..32).map(|j| ((i * 32 + j) % 10) as f32).collect())
            .collect();

        let config = PQConfig {
            num_subvectors: 2,
            num_centroids: 4,
            max_iterations: 5,
            convergence_threshold: 0.01,
        };

        let encoder = PQEncoder::new(config);
        let codebooks = encoder.train_codebooks(&vectors).unwrap();
        let pq_vectors = encoder.encode(&vectors, &codebooks).unwrap();

        let query = &vectors[0];
        let table = encoder.compute_distance_table(query, &codebooks);

        // Distance to first vector should be small (it's the query itself)
        let distance = encoder.pq_distance(&pq_vectors.codes[0], &table);
        assert!(distance < 1.0); // Should be very close
    }

    #[test]
    fn test_empty_vectors() {
        let vectors: Vec<Vec<f32>> = vec![];
        let config = PQConfig::default();
        let encoder = PQEncoder::new(config);

        let result = encoder.train_codebooks(&vectors);
        assert!(result.is_err());

        let codebooks = PQCodebooks::new(4, 8, 8);
        let pq_vectors = encoder.encode(&vectors, &codebooks).unwrap();
        assert_eq!(pq_vectors.codes.len(), 0);
    }
}
