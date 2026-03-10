//! Matrix Builder Module for RAPTOR Engine
//!
//! Consolidates all matrix building logic for the Matrix Trinity architecture:
//! - P² Matrix: Intra-rowgroup pairwise distances
//! - K² Matrix: Inter-centroid distances
//! - P×K Matrix: Vector-to-centroid distances (adaptive coverage)
//!
//! ## GPU Acceleration
//!
//! On macOS with Metal feature enabled, P² matrix building can be accelerated
//! using GPU MPS (Metal Performance Shaders). The GPU path computes all N×N
//! pairwise distances in a single dispatch, providing significant speedup
//! over the CPU SIMD path for large rowgroups.

use anyhow::Result;
use std::sync::Arc;
#[cfg(feature = "gpu")]
use tracing::warn;
use tracing::{debug, info};

use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use crate::core::hardware_capabilities::HardwareCapabilities;
use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;

#[cfg(feature = "gpu")]
use crate::compute::gpu::distance::GpuDistanceCompute;
#[cfg(feature = "gpu")]
use crate::core::hardware_capabilities::GpuBackend;

use super::common::{
    CompressionType, DeltaEntry, HierarchicalData, InterCentroidCompressionMetadata,
    InterCentroidMatrix, P2Matrix, SparseData, SparseEntry, VectorCentroidCompressionMetadata,
    VectorCentroidMatrix, VectorCentroidStorageStrategy,
};

/// Matrix builder for RAPTOR's Matrix Trinity architecture
///
/// Supports both CPU SIMD and GPU MPS paths for P² matrix building.
/// GPU acceleration is automatically used when:
/// - The `gpu` feature is enabled
/// - Metal MPS is available (macOS with Apple Silicon)
/// - The rowgroup has >= 100 vectors (GPU overhead worthwhile)
pub struct MatrixBuilder {
    distance_compute: Arc<UnifiedDistanceCompute>,
    _hardware: Arc<HardwareCapabilities>,
    distance_metric: DistanceMetric,
    /// Optional GPU compute for accelerated pairwise distance
    #[cfg(feature = "gpu")]
    gpu_compute: Option<Arc<GpuDistanceCompute>>,
}

impl MatrixBuilder {
    /// Create a new MatrixBuilder with optional GPU acceleration
    ///
    /// GPU acceleration is automatically enabled when:
    /// - The `gpu` feature is compiled in
    /// - Metal MPS devices are detected (macOS)
    pub fn new(
        distance_compute: Arc<UnifiedDistanceCompute>,
        hardware: Arc<HardwareCapabilities>,
        distance_metric: DistanceMetric,
    ) -> Self {
        #[cfg(feature = "gpu")]
        let gpu_compute = {
            // Prefer cached detection from global hardware capabilities to avoid re-probing
            GpuDistanceCompute::from_capabilities(&hardware)
                .or_else(|| GpuDistanceCompute::new().ok())
                .and_then(|gpu| {
                    if gpu.is_available() && gpu.backend() == GpuBackend::MPS {
                        info!(
                            "🚀 RAPTOR MatrixBuilder: GPU MPS acceleration enabled for P² matrix"
                        );
                        Some(Arc::new(gpu))
                    } else {
                        debug!("GPU available but not MPS, using CPU SIMD for P² matrix");
                        None
                    }
                })
        };

        Self {
            distance_compute,
            _hardware: hardware,
            distance_metric,
            #[cfg(feature = "gpu")]
            gpu_compute,
        }
    }

    /// Check if GPU acceleration is available for this builder
    #[cfg(feature = "gpu")]
    pub fn has_gpu(&self) -> bool {
        self.gpu_compute.is_some()
    }

    #[cfg(not(feature = "gpu"))]
    pub fn has_gpu(&self) -> bool {
        false
    }

    /// Build P² matrix for intra-rowgroup navigation
    /// This matrix stores pairwise distances between all vectors in a rowgroup
    ///
    /// Uses GPU acceleration (Metal MPS) when available for significant speedup
    /// on large rowgroups. Falls back to CPU SIMD for small rowgroups or when
    /// GPU is not available.
    pub fn build_p2_matrix(&self, vectors: &[Vec<f32>], dimension: usize) -> Result<P2Matrix> {
        #[cfg(not(feature = "gpu"))]
        let _dimension = dimension; // Suppress unused warning when GPU is disabled
        let num_vectors = vectors.len();
        if num_vectors == 0 {
            return Ok(P2Matrix {
                num_vectors: 0,
                distances: Vec::new(),
                min_distance: 0.0,
                max_distance: 0.0,
                compression: ProximaScheme::BitPacked { bits: 16 },
                compressed_size: 0,
            });
        }

        // Try GPU path first for rowgroups >= 100 vectors (GPU overhead worthwhile)
        #[cfg(feature = "gpu")]
        if num_vectors >= 100 {
            if let Some(ref gpu) = self.gpu_compute {
                // Only MPS pairwise kernels are implemented today
                if gpu.backend() == GpuBackend::MPS {
                    match self.build_p2_matrix_gpu(vectors, dimension, gpu.clone()) {
                        Ok(matrix) => {
                            info!(
                                "🚀 P² matrix built on GPU: {} vectors, {}×{} = {} distances",
                                num_vectors,
                                num_vectors,
                                num_vectors,
                                num_vectors * num_vectors
                            );
                            return Ok(matrix);
                        }
                        Err(e) => {
                            warn!("GPU P² matrix failed, falling back to CPU SIMD: {}", e);
                            // Fall through to CPU path
                        }
                    }
                }
            }
        }

        // CPU SIMD path (fallback or for small rowgroups)
        info!("Building P² matrix for {} vectors (CPU SIMD)", num_vectors);

        // Calculate all pairwise distances
        let mut distances = Vec::with_capacity(num_vectors * num_vectors);
        let mut min_dist = f32::MAX;
        let mut max_dist = f32::MIN;

        // Process row by row using batch distance computation
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        for i in 0..num_vectors {
            // Compute distances from vector i to all vectors using batch method
            let row_distances = self.distance_compute.batch_distance_pooled_simd(
                &vectors[i],
                &vector_refs,
                &self.distance_metric,
            );

            // Process the row results
            for (j, dist_result) in row_distances.into_iter().enumerate() {
                let dist = if i == j { 0.0 } else { dist_result.distance };
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
            compression: ProximaScheme::BitPacked { bits: 16 },
            compressed_size,
        })
    }

    /// Build P² matrix using GPU MPS acceleration
    /// Computes all N×N pairwise distances in a single GPU dispatch
    #[cfg(feature = "gpu")]
    fn build_p2_matrix_gpu(
        &self,
        vectors: &[Vec<f32>],
        dimension: usize,
        gpu: Arc<GpuDistanceCompute>,
    ) -> Result<P2Matrix> {
        use tokio::runtime::Handle;

        let num_vectors = vectors.len();

        // Convert internal DistanceMetric to proto DistanceMetric for GPU API
        let proto_metric = match self.distance_metric {
            DistanceMetric::Euclidean => crate::proto::proximadb_v1::DistanceMetric::Euclidean,
            DistanceMetric::Cosine => crate::proto::proximadb_v1::DistanceMetric::Cosine,
            DistanceMetric::DotProduct => crate::proto::proximadb_v1::DistanceMetric::DotProduct,
            DistanceMetric::Manhattan => crate::proto::proximadb_v1::DistanceMetric::Manhattan,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unsupported metric for GPU P² matrix: {:?}",
                    self.distance_metric
                ));
            }
        };

        // Execute GPU computation - block on async call
        let distances = Handle::current().block_on(async {
            gpu.calculate_pairwise_matrix_mps(vectors, proto_metric)
                .await
        })?;

        // Find min/max for compression
        let mut min_dist = f32::MAX;
        let mut max_dist = f32::MIN;
        for (i, &dist) in distances.iter().enumerate() {
            let row = i / num_vectors;
            let col = i % num_vectors;
            if row != col && dist > 0.0 {
                min_dist = min_dist.min(dist);
                max_dist = max_dist.max(dist);
            }
        }

        // Handle edge case where all distances are 0
        if min_dist == f32::MAX {
            min_dist = 0.0;
        }
        if max_dist == f32::MIN {
            max_dist = 1.0;
        }

        // Compress distances using 16-bit quantization
        let scale = if max_dist > min_dist {
            65535.0 / (max_dist - min_dist)
        } else {
            1.0
        };

        let compressed: Vec<u16> = distances
            .iter()
            .enumerate()
            .map(|(i, &d)| {
                let row = i / num_vectors;
                let col = i % num_vectors;
                if row == col || d == 0.0 {
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
            "P² matrix (GPU): {} vectors, {} distances, {} -> {} bytes ({:.1}% reduction)",
            num_vectors,
            distances.len(),
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
            compression: ProximaScheme::BitPacked { bits: 16 },
            compressed_size,
        })
    }

    /// Build K² matrix for inter-centroid navigation
    /// OPTIMIZED: Only stores upper triangle (j > i) for 50% space savings
    /// Matrix is symmetric: distance(i,j) = distance(j,i), diagonal = 0
    pub fn build_k2_matrix(
        &self,
        centroids: &[Vec<f32>],
        _dimension: usize,
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

        // Upper triangle size: k*(k-1)/2 instead of k*k (50% savings)
        let upper_triangle_size = num_centroids * (num_centroids - 1) / 2;
        info!(
            "Building K² matrix for {} centroids (upper triangle: {} entries, 50% savings)",
            num_centroids, upper_triangle_size
        );

        // Calculate ONLY upper triangle distances (j > i)
        let mut distances = Vec::with_capacity(upper_triangle_size);
        let mut min_dist = f32::MAX;
        let mut max_dist = f32::MIN;

        for i in 0..num_centroids {
            for j in (i + 1)..num_centroids {
                // Only compute where j > i (strict upper triangle)
                let dist = self
                    .distance_compute
                    .calculate_distance(&centroids[i], &centroids[j], &self.distance_metric)
                    .raw_value;

                distances.push(dist);
                min_dist = min_dist.min(dist);
                max_dist = max_dist.max(dist);
            }
        }

        // Handle edge case of single centroid
        if distances.is_empty() {
            min_dist = 0.0;
            max_dist = 1.0;
        }

        // Compress using 16-bit quantization
        let scale_factor = if max_dist > min_dist {
            65535.0 / (max_dist - min_dist)
        } else {
            1.0
        };

        // Compressed data: upper triangle in row-major order
        // Layout: [d(0,1), d(0,2), ..., d(0,k-1), d(1,2), d(1,3), ..., d(k-2,k-1)]
        let mut compressed_data = Vec::with_capacity(upper_triangle_size * 2);

        for &dist in &distances {
            let quantized = ((dist - min_dist) * scale_factor) as u16;
            compressed_data.extend_from_slice(&quantized.to_le_bytes());
        }

        // For upper triangle, we don't need per-row sizes or lookup table
        // Access is O(1) via formula: idx = i*(2k-i-1)/2 + (j-i-1)
        // But we keep the structures for compatibility
        let row_compressed_sizes: Vec<u16> = (0..num_centroids)
            .map(|i| ((num_centroids - i - 1) * 2) as u16) // Each row i has (k-i-1) elements
            .collect();

        // Lookup table: cumulative offset for each row's start
        let mut lookup_table = Vec::with_capacity(num_centroids);
        let mut offset = 0u32;
        for i in 0..num_centroids {
            lookup_table.push(offset);
            offset += (num_centroids - i - 1) as u32 * 2; // 2 bytes per u16
        }

        let full_matrix_size = num_centroids * num_centroids * 4; // k² × 4 bytes (f32)
        let compressed_size = compressed_data.len();
        let savings_pct = (1.0 - compressed_size as f32 / full_matrix_size as f32) * 100.0;

        debug!(
            "K² matrix: {} centroids, {} bytes (vs {} full matrix, {:.1}% savings)",
            num_centroids, compressed_size, full_matrix_size, savings_pct
        );

        Ok(InterCentroidMatrix {
            num_centroids: num_centroids as u32,
            compressed_data,
            compression_metadata: InterCentroidCompressionMetadata {
                min_distance: min_dist,
                max_distance: max_dist,
                scale_factor,
                compression_type: CompressionType::Quantized16Bit,
                row_encodings: vec![ProximaScheme::BitPacked { bits: 16 }; num_centroids],
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

                    distances
                        .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

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

    /// Decompress all distances from centroid `centroid_idx` to all other centroids.
    /// Returns a vector of length k where result[j] = distance(centroid_idx, j).
    ///
    /// Since K² uses upper triangle storage, we need to handle:
    /// - j == centroid_idx: return 0.0 (diagonal)
    /// - j < centroid_idx: lookup d[j][centroid_idx] (swap i,j)
    /// - j > centroid_idx: lookup d[centroid_idx][j] (direct)
    pub fn decompress_k2_row(
        &self,
        matrix: &InterCentroidMatrix,
        centroid_idx: usize,
    ) -> Result<Vec<f32>> {
        let k = matrix.num_centroids as usize;
        if centroid_idx >= k {
            return Err(anyhow::anyhow!(
                "Centroid index {} out of bounds (k={})",
                centroid_idx,
                k
            ));
        }

        let min_dist = matrix.compression_metadata.min_distance;
        let scale_factor = matrix.compression_metadata.scale_factor;
        let mut distances = Vec::with_capacity(k);

        for j in 0..k {
            if j == centroid_idx {
                // Diagonal: distance to self is 0
                distances.push(0.0);
            } else {
                // Ensure upper triangle access (i < j)
                let (i, jj) = if centroid_idx < j {
                    (centroid_idx, j)
                } else {
                    (j, centroid_idx)
                };

                // Calculate linear index in upper triangle storage:
                // Layout: [d(0,1), d(0,2), ..., d(0,k-1), d(1,2), ..., d(k-2,k-1)]
                // For row i, elements start at position i*(2k-i-1)/2
                // Position of d(i,j) is: i*(2k-i-1)/2 + (j-i-1)
                let total_before_row_i = i * (2 * k - i - 1) / 2;
                let position_in_row_i = jj - i - 1;
                let linear_index = total_before_row_i + position_in_row_i;

                // Read quantized u16 value
                let byte_offset = linear_index * 2;
                if byte_offset + 2 > matrix.compressed_data.len() {
                    return Err(anyhow::anyhow!(
                        "Index out of bounds: linear_index={}, byte_offset={}, data_len={}",
                        linear_index,
                        byte_offset,
                        matrix.compressed_data.len()
                    ));
                }

                let quantized = u16::from_le_bytes([
                    matrix.compressed_data[byte_offset],
                    matrix.compressed_data[byte_offset + 1],
                ]);

                // Dequantize: d = min + q / scale_factor
                let dist = if quantized == 0 && min_dist == 0.0 {
                    0.0
                } else {
                    quantized as f32 / scale_factor + min_dist
                };

                distances.push(dist);
            }
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

        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        let builder = MatrixBuilder::new(distance_compute, hardware, DistanceMetric::Cosine);

        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];

        let matrix = builder.build_p2_matrix(&vectors, 3).unwrap();
        assert_eq!(matrix.num_vectors, 3);
        assert!(!matrix.distances.is_empty());
        assert!(matrix.compressed_size > 0);
    }

    #[test]
    fn test_k2_matrix_building() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        let builder = MatrixBuilder::new(distance_compute, hardware, DistanceMetric::Euclidean);

        let centroids = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.5, 0.5]];

        let matrix = builder.build_k2_matrix(&centroids, 2).unwrap();
        assert_eq!(matrix.num_centroids, 3);
        assert!(!matrix.compressed_data.is_empty());
        assert_eq!(matrix.lookup_table.len(), 3);
    }

    #[test]
    fn test_adaptive_pxk_coverage() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Test coverage formula for different k and d values
        // Formula: exp(-2 * ln(k/d + 1)) produces HIGH values for LOW k/d ratios
        let test_cases = vec![
            (10, 100, 0.8),   // Low k/d ratio (0.1) -> high coverage (~0.82)
            (100, 100, 0.2),  // k = d (1.0) -> moderate coverage (~0.25)
            (1000, 100, 0.1), // High k/d ratio (10) -> minimum coverage (0.1)
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
