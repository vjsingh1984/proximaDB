//! Matrix Builder Module for RAPTOR Engine
//!
//! Consolidates all matrix building logic for the Matrix Trinity architecture:
//! - P² Matrix: Intra-rowgroup pairwise distances
//! - K² Matrix: Inter-centroid distances  
//! - P×K Matrix: Vector-to-centroid distances (adaptive coverage)

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use crate::core::hardware_capabilities::HardwareCapabilities;
use crate::storage::engines::core::ops::fastlanes_encoding::FastLanesScheme;

use super::common::{
    CompressionType, DeltaEntry, HierarchicalData, InterCentroidCompressionMetadata,
    InterCentroidMatrix, P2Matrix, SparseData, SparseEntry, VectorCentroidCompressionMetadata,
    VectorCentroidMatrix, VectorCentroidStorageStrategy,
};

/// Matrix builder for RAPTOR's Matrix Trinity architecture
pub struct MatrixBuilder {
    distance_compute: Arc<UnifiedDistanceCompute>,
    hardware: Arc<HardwareCapabilities>,
    distance_metric: DistanceMetric,
}

impl MatrixBuilder {
    pub fn new(
        distance_compute: Arc<UnifiedDistanceCompute>,
        hardware: Arc<HardwareCapabilities>,
        distance_metric: DistanceMetric,
    ) -> Self {
        Self {
            distance_compute,
            hardware,
            distance_metric,
        }
    }

    /// Build P² matrix for intra-rowgroup navigation
    /// This matrix stores pairwise distances between all vectors in a rowgroup
    pub fn build_p2_matrix(&self, vectors: &[Vec<f32>], dimension: usize) -> Result<P2Matrix> {
        let num_vectors = vectors.len();
        if num_vectors == 0 {
            return Ok(P2Matrix {
                num_vectors: 0,
                distances: Vec::new(),
                min_distance: 0.0,
                max_distance: 0.0,
                compression: FastLanesScheme::BitPacked { bits: 16 },
                compressed_size: 0,
            });
        }

        info!("Building P² matrix for {} vectors", num_vectors);

        // Calculate all pairwise distances
        let mut distances = Vec::with_capacity(num_vectors * num_vectors);
        let mut min_dist = f32::MAX;
        let mut max_dist = f32::MIN;

        for i in 0..num_vectors {
            for j in 0..num_vectors {
                let dist = if i == j {
                    0.0
                } else {
                    self.distance_compute
                        .calculate_distance(&vectors[i], &vectors[j], &self.distance_metric)
                        .raw_value
                };

                distances.push(dist);
                if dist > 0.0 {
                    min_dist = min_dist.min(dist);
                    max_dist = max_dist.max(dist);
                }
            }
        }

        // Compress distances using 16-bit quantization
        let scale = if max_dist > min_dist {
            65535.0 / (max_dist - min_dist)
        } else {
            1.0
        };

        let compressed: Vec<u16> = distances
            .iter()
            .map(|&d| {
                if d == 0.0 {
                    0
                } else {
                    ((d - min_dist) * scale) as u16
                }
            })
            .collect();

        // Convert to bytes for storage
        let mut compressed_bytes = Vec::with_capacity(compressed.len() * 2);
        for val in compressed {
            compressed_bytes.extend_from_slice(&val.to_le_bytes());
        }

        debug!(
            "P² matrix compressed: {} -> {} bytes ({:.1}% reduction)",
            distances.len() * 4,
            compressed_bytes.len(),
            (1.0 - compressed_bytes.len() as f32 / (distances.len() * 4) as f32) * 100.0
        );

        let compressed_size = compressed_bytes.len() as u32;
        Ok(P2Matrix {
            num_vectors: num_vectors as u32,
            distances: compressed_bytes,
            min_distance: min_dist,
            max_distance: max_dist,
            compression: FastLanesScheme::BitPacked { bits: 16 },
            compressed_size,
        })
    }

    /// Build K² matrix for inter-centroid navigation
    /// This matrix stores distances between all centroids globally
    pub fn build_k2_matrix(
        &self,
        centroids: &[Vec<f32>],
        dimension: usize,
    ) -> Result<InterCentroidMatrix> {
        let num_centroids = centroids.len();
        if num_centroids == 0 {
            return Ok(InterCentroidMatrix {
                num_centroids: 0,
                compressed_data: Vec::new(),
                compression_metadata: InterCentroidCompressionMetadata {
                    min_distance: 0.0,
                    max_distance: 1.0,
                    scale_factor: 1.0,
                    compression_type: CompressionType::Quantized16Bit,
                    row_encodings: Vec::new(),
                    row_compressed_sizes: Vec::new(),
                },
                lookup_table: Vec::new(),
            });
        }

        info!("Building K² matrix for {} centroids", num_centroids);

        // Calculate all centroid-to-centroid distances
        let mut distances = Vec::with_capacity(num_centroids * num_centroids);
        let mut min_dist = f32::MAX;
        let mut max_dist = f32::MIN;

        for i in 0..num_centroids {
            for j in 0..num_centroids {
                let dist = if i == j {
                    0.0
                } else {
                    self.distance_compute
                        .calculate_distance(&centroids[i], &centroids[j], &self.distance_metric)
                        .raw_value
                };

                distances.push(dist);
                if dist > 0.0 {
                    min_dist = min_dist.min(dist);
                    max_dist = max_dist.max(dist);
                }
            }
        }

        // Compress using 16-bit quantization
        let scale_factor = if max_dist > min_dist {
            65535.0 / (max_dist - min_dist)
        } else {
            1.0
        };

        let mut compressed_data = Vec::with_capacity(num_centroids * num_centroids * 2);
        let mut row_compressed_sizes = Vec::with_capacity(num_centroids);

        for i in 0..num_centroids {
            let row_start = compressed_data.len();

            for j in 0..num_centroids {
                let dist = distances[i * num_centroids + j];
                let quantized = if dist == 0.0 {
                    0u16
                } else {
                    ((dist - min_dist) * scale_factor) as u16
                };
                compressed_data.extend_from_slice(&quantized.to_le_bytes());
            }

            row_compressed_sizes.push((compressed_data.len() - row_start) as u16);
        }

        // Build lookup table for fast access
        let mut lookup_table = Vec::with_capacity(num_centroids);
        let mut offset = 0u32;
        for size in &row_compressed_sizes {
            lookup_table.push(offset);
            offset += *size as u32;
        }

        debug!(
            "K² matrix compressed: {} -> {} bytes ({:.1}% reduction)",
            distances.len() * 4,
            compressed_data.len(),
            (1.0 - compressed_data.len() as f32 / (distances.len() * 4) as f32) * 100.0
        );

        Ok(InterCentroidMatrix {
            num_centroids: num_centroids as u32,
            compressed_data,
            compression_metadata: InterCentroidCompressionMetadata {
                min_distance: min_dist,
                max_distance: max_dist,
                scale_factor,
                compression_type: CompressionType::Quantized16Bit,
                row_encodings: vec![FastLanesScheme::BitPacked { bits: 16 }; num_centroids],
                row_compressed_sizes,
            },
            lookup_table,
        })
    }

    /// Build P×K matrix for spillover detection
    /// Uses adaptive coverage based on the formula: coverage(k,d) = max(0.1, min(1.0, exp(-2 × log(k/d + 1))))
    pub fn build_pxk_matrix(
        &self,
        vectors: &[Vec<f32>],
        centroids: &[Vec<f32>],
        dimension: usize,
        rowgroup_id: u16,
    ) -> Result<VectorCentroidMatrix> {
        let num_vectors = vectors.len();
        let num_centroids = centroids.len();

        if num_vectors == 0 || num_centroids == 0 {
            return Ok(VectorCentroidMatrix {
                rowgroup_id,
                num_vectors: 0,
                num_centroids: 0,
                storage_strategy: VectorCentroidStorageStrategy::Sparse, // Default to sparse for empty
                compressed_data: Vec::new(),
                compression_metadata: VectorCentroidCompressionMetadata {
                    centroid_stats: Vec::new(),
                    global_min_distance: 0.0,
                    global_max_distance: 1.0,
                    global_mean_distance: 0.5,
                    centroid_encodings: Vec::new(),
                },
                hierarchical_data: None,
                sparse_data: None,
            });
        }

        info!(
            "Building P×K matrix for rowgroup {} ({} vectors × {} centroids)",
            rowgroup_id, num_vectors, num_centroids
        );

        // Calculate adaptive coverage percentage
        let k = num_centroids as f32;
        let d = dimension as f32;
        let coverage = (0.1_f32).max(1.0_f32.min((-2.0 * (k / d + 1.0).ln()).exp()));

        debug!(
            "Adaptive P×K coverage: {:.1}% for k={}, d={}",
            coverage * 100.0,
            k,
            d
        );

        // Determine storage strategy based on coverage
        let storage_strategy = if coverage >= 0.8 {
            VectorCentroidStorageStrategy::Full
        } else if coverage >= 0.3 {
            VectorCentroidStorageStrategy::Hierarchical
        } else {
            VectorCentroidStorageStrategy::Sparse
        };

        // Calculate distances and build appropriate data structures
        let mut compressed_data = Vec::new();
        let mut global_min = f32::MAX;
        let mut global_max = f32::MIN;
        let mut global_sum = 0.0;
        let mut count = 0;

        // Strategy-specific data
        let mut hierarchical_data: Option<HierarchicalData> = None;
        let mut sparse_data: Option<SparseData> = None;

        match storage_strategy {
            VectorCentroidStorageStrategy::Full => {
                // Store all P×K distances in compressed form
                for i in 0..num_vectors {
                    for j in 0..num_centroids {
                        let dist = self
                            .distance_compute
                            .calculate_distance(&vectors[i], &centroids[j], &self.distance_metric)
                            .raw_value;

                        let quantized = ((dist * 65535.0).min(65535.0)) as u16;
                        compressed_data.extend_from_slice(&quantized.to_le_bytes());

                        global_min = global_min.min(dist);
                        global_max = global_max.max(dist);
                        global_sum += dist;
                        count += 1;
                    }
                }
            }
            VectorCentroidStorageStrategy::Hierarchical => {
                // Calculate mean distances per centroid
                let mut mean_distances = vec![0.0; num_centroids];
                let mut delta_entries = Vec::new();

                for j in 0..num_centroids {
                    let mut sum = 0.0;
                    for i in 0..num_vectors {
                        let dist = self
                            .distance_compute
                            .calculate_distance(&vectors[i], &centroids[j], &self.distance_metric)
                            .raw_value;
                        sum += dist;
                    }
                    mean_distances[j] = sum / num_vectors as f32;
                }

                // Store significant deltas
                for i in 0..num_vectors {
                    for j in 0..num_centroids {
                        let dist = self
                            .distance_compute
                            .calculate_distance(&vectors[i], &centroids[j], &self.distance_metric)
                            .raw_value;

                        let delta = dist - mean_distances[j];
                        if delta.abs() > 0.1 {
                            // Significant delta threshold
                            delta_entries.push(DeltaEntry {
                                vector_index: i as u32,
                                centroid_index: j as u16,
                                delta_value: delta,
                            });
                        }

                        global_min = global_min.min(dist);
                        global_max = global_max.max(dist);
                        global_sum += dist;
                        count += 1;
                    }
                }

                hierarchical_data = Some(HierarchicalData {
                    mean_distances,
                    sparse_deltas: delta_entries,
                });
            }
            VectorCentroidStorageStrategy::Sparse => {
                // Store only top-k closest centroids per vector
                let k = ((coverage * num_centroids as f32) as usize)
                    .max(3)
                    .min(num_centroids);
                let mut sparse_entries = Vec::new();

                for i in 0..num_vectors {
                    let mut distances: Vec<(usize, f32)> = centroids
                        .iter()
                        .enumerate()
                        .map(|(j, centroid)| {
                            let dist = self
                                .distance_compute
                                .calculate_distance(&vectors[i], centroid, &self.distance_metric)
                                .raw_value;
                            (j, dist)
                        })
                        .collect();

                    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

                    // For SparseEntry, we need to store individual entries per centroid
                    for &(centroid_idx, dist) in distances.iter().take(k) {
                        let quantized = ((dist * 255.0 / global_max.max(1.0)).min(255.0)) as u8;
                        sparse_entries.push(SparseEntry {
                            vector_idx: i as u32,
                            centroid_idx: centroid_idx as u32,
                            quantized_distance: quantized,
                        });

                        global_min = global_min.min(dist);
                        global_max = global_max.max(dist);
                        global_sum += dist;
                        count += 1;
                    }
                }

                sparse_data = Some(SparseData {
                    top_k: k as u32,
                    entries: sparse_entries,
                    boundary_bloom_filter: None, // Can be added later
                    sparsity_ratio: (k as f32 / num_centroids as f32),
                });
            }
        }

        let global_mean = if count > 0 {
            global_sum / count as f32
        } else {
            0.5
        };

        debug!(
            "P×K matrix built with {:?} strategy: {} bytes for {} entries",
            storage_strategy,
            compressed_data.len(),
            count
        );

        Ok(VectorCentroidMatrix {
            rowgroup_id,
            num_vectors: num_vectors as u32,
            num_centroids: num_centroids as u32,
            storage_strategy,
            compressed_data,
            compression_metadata: VectorCentroidCompressionMetadata {
                centroid_stats: Vec::new(), // Will be populated during write
                global_min_distance: global_min,
                global_max_distance: global_max,
                global_mean_distance: global_mean,
                centroid_encodings: Vec::new(),
            },
            hierarchical_data,
            sparse_data,
        })
    }

    /// Decompress a row from K² matrix for boundary detection
    pub fn decompress_k2_row(
        &self,
        matrix: &InterCentroidMatrix,
        centroid_idx: usize,
    ) -> Result<Vec<f32>> {
        if centroid_idx >= matrix.num_centroids as usize {
            return Err(anyhow::anyhow!(
                "Centroid index {} out of bounds",
                centroid_idx
            ));
        }

        let row_size = matrix.compression_metadata.row_compressed_sizes[centroid_idx] as usize;
        let offset = matrix.lookup_table[centroid_idx] as usize;

        let row_data = &matrix.compressed_data[offset..offset + row_size];
        let mut distances = Vec::with_capacity(matrix.num_centroids as usize);

        let min_dist = matrix.compression_metadata.min_distance;
        let scale_factor = matrix.compression_metadata.scale_factor;

        for i in 0..matrix.num_centroids as usize {
            let quantized = u16::from_le_bytes([row_data[i * 2], row_data[i * 2 + 1]]);

            let dist = if quantized == 0 {
                0.0
            } else {
                quantized as f32 / scale_factor + min_dist
            };

            distances.push(dist);
        }

        Ok(distances)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_p2_matrix_building() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let hardware = get_hardware_capabilities();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(hardware.clone()));

        let builder = MatrixBuilder::new(distance_compute, hardware, DistanceMetric::Cosine);

        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];

        let matrix = builder.build_p2_matrix(&vectors, 3).unwrap();
        assert_eq!(matrix.num_vectors, 3);
        assert!(!matrix.distances.is_none());
        assert!(matrix.compressed_size > 0);
    }

    #[test]
    fn test_k2_matrix_building() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let hardware = get_hardware_capabilities();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(hardware.clone()));

        let builder = MatrixBuilder::new(distance_compute, hardware, DistanceMetric::Euclidean);

        let centroids = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.5, 0.5]];

        let matrix = builder.build_k2_matrix(&centroids, 2).unwrap();
        assert_eq!(matrix.num_centroids, 3);
        assert!(!matrix.compressed_data.is_none());
        assert_eq!(matrix.lookup_table.len(), 3);
    }

    #[test]
    fn test_adaptive_pxk_coverage() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Test coverage formula for different k and d values
        let test_cases = vec![
            (10, 100, 0.1),   // Low k/d ratio -> minimum coverage
            (100, 100, 0.37), // k = d -> moderate coverage
            (1000, 100, 0.9), // High k/d ratio -> high coverage
        ];

        for (k, d, expected_min) in test_cases {
            let coverage =
                (0.1_f32).max(1.0_f32.min((-2.0 * ((k as f32) / (d as f32) + 1.0).ln()).exp()));

            assert!(
                coverage >= expected_min,
                "Coverage {:.2} should be >= {:.2} for k={}, d={}",
                coverage,
                expected_min,
                k,
                d
            );
        }
    }
}
