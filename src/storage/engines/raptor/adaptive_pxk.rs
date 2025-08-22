//! Adaptive P×K Storage Implementation
//! 
//! Implements the refined formula with minimum 10% coverage floor
//! and intelligent compression strategies based on K/D relationship

use super::common::RowGroupMetadata;
use super::config::{PxKStrategy, CompressionStrategy};
use crate::core::compression::StandardCompression;
use crate::compute::quantization::storage_engine::StorageQuantizationEngine;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use anyhow::Result;
use std::collections::HashSet;
use serde::{Deserialize, Serialize};

/// Vector to centroid distance matrix
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorCentroidMatrix {
    pub distances: Vec<Vec<f32>>,
    pub num_vectors: usize,
    pub num_clusters: usize,
}

/// Selection reason for sparse storage
#[derive(Debug, Clone)]
pub enum SelectionReason {
    /// Near cluster boundary
    Boundary,
    /// Cluster medoid/representative
    Representative,
    /// Far from centroid
    Outlier,
    /// Maximizes coverage diversity
    Diverse,
    /// Random statistical sample
    Random,
}

/// Vector selection info for sparse storage
#[derive(Debug, Clone)]
pub struct VectorSelection {
    pub vector_idx: u32,
    pub selection_reason: SelectionReason,
    pub importance_score: f32,
}

/// Trait for different P×K storage implementations
pub trait PxKStorageImpl: Send + Sync {
    /// Store distances for a vector
    fn store_distances(&mut self, vector_idx: usize, distances: &[f32]) -> Result<()>;
    
    /// Get distances for a vector
    fn get_distances(&self, vector_idx: usize) -> Option<Vec<f32>>;
    
    /// Detect boundary vectors
    fn detect_boundaries(&self, threshold: f32) -> Vec<BoundaryInfo>;
    
    /// Get memory usage in bytes
    fn memory_usage(&self) -> usize;
    
    /// Serialize to bytes
    fn serialize(&self) -> Result<Vec<u8>>;
}

/// Boundary information for a vector
#[derive(Debug, Clone)]
pub struct BoundaryInfo {
    pub vector_idx: u32,
    pub primary_cluster: u32,
    pub primary_distance: f32,
    pub secondary_cluster: u32,
    pub secondary_distance: f32,
    pub boundary_ratio: f32,
}

/// Dense full storage (no compression)
pub struct DenseFullStorage {
    matrix: VectorCentroidMatrix,
}

impl PxKStorageImpl for DenseFullStorage {
    fn store_distances(&mut self, vector_idx: usize, distances: &[f32]) -> Result<()> {
        self.matrix.distances[vector_idx] = distances.to_vec();
        Ok(())
    }
    
    fn get_distances(&self, vector_idx: usize) -> Option<Vec<f32>> {
        self.matrix.distances.get(vector_idx).cloned()
    }
    
    fn detect_boundaries(&self, threshold: f32) -> Vec<BoundaryInfo> {
        let mut boundaries = Vec::new();
        
        for (idx, distances) in self.matrix.distances.iter().enumerate() {
            if distances.len() < 2 {
                continue;
            }
            
            // Find two closest clusters
            let mut sorted: Vec<(usize, f32)> = distances
                .iter()
                .enumerate()
                .map(|(i, &d)| (i, d))
                .collect();
            sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            
            let ratio = sorted[0].1 / sorted[1].1;
            if ratio > threshold {
                boundaries.push(BoundaryInfo {
                    vector_idx: idx as u32,
                    primary_cluster: sorted[0].0 as u32,
                    primary_distance: sorted[0].1,
                    secondary_cluster: sorted[1].0 as u32,
                    secondary_distance: sorted[1].1,
                    boundary_ratio: ratio,
                });
            }
        }
        
        boundaries
    }
    
    fn memory_usage(&self) -> usize {
        self.matrix.distances.len() * self.matrix.num_clusters * 4
    }
    
    fn serialize(&self) -> Result<Vec<u8>> {
        // Serialize matrix to bytes
        bincode::serialize(&self.matrix).map_err(Into::into)
    }
}

/// Sparse coverage storage with compression
pub struct SparseCoverageStorage {
    coverage: f32,
    compression: CompressionStrategy,
    selected_vectors: Vec<VectorSelection>,
    compressed_distances: Vec<Vec<u8>>,
    num_clusters: usize,
    quantization_engine: StorageQuantizationEngine,
}

impl SparseCoverageStorage {
    /// Create new sparse storage
    pub fn new(coverage: f32, compression: CompressionStrategy, num_clusters: usize) -> Self {
        Self {
            coverage,
            compression,
            selected_vectors: Vec::new(),
            compressed_distances: Vec::new(),
            num_clusters,
            quantization_engine: StorageQuantizationEngine::new(),
        }
    }
    
    /// Helper to quantize to u16
    fn quantize_to_u16(&self, distances: &[f32]) -> (Vec<u16>, f32, f32) {
        let min = distances.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = distances.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let range = max - min;
        
        let quantized: Vec<u16> = distances.iter().map(|&d| {
            let normalized = (d - min) / range;
            (normalized * 65535.0) as u16
        }).collect();
        
        (quantized, min, max)
    }
    
    /// Select vectors intelligently based on coverage
    pub fn select_vectors(
        &mut self,
        vectors: &[Vec<f32>],
        centroids: &[Vec<f32>],
    ) -> Vec<VectorSelection> {
        let num_select = (vectors.len() as f32 * self.coverage) as usize;
        let mut selected = Vec::with_capacity(num_select);
        let mut selected_indices = HashSet::new();
        
        // Priority 1: Boundary vectors (40% of budget)
        let boundary_budget = (num_select as f32 * 0.4) as usize;
        let boundaries = self.select_boundary_vectors(vectors, centroids, boundary_budget);
        for b in boundaries {
            selected_indices.insert(b.vector_idx);
            selected.push(b);
        }
        
        // Priority 2: Representatives (20% of budget)
        let rep_budget = (num_select as f32 * 0.2) as usize;
        let representatives = self.select_representatives(vectors, rep_budget, &selected_indices);
        for r in representatives {
            selected_indices.insert(r.vector_idx);
            selected.push(r);
        }
        
        // Priority 3: Outliers (20% of budget)
        let outlier_budget = (num_select as f32 * 0.2) as usize;
        let outliers = self.select_outliers(vectors, centroids, outlier_budget, &selected_indices);
        for o in outliers {
            selected_indices.insert(o.vector_idx);
            selected.push(o);
        }
        
        // Priority 4: Diverse sample (remaining budget)
        let remaining = num_select.saturating_sub(selected.len());
        let diverse = self.select_diverse(vectors, remaining, &selected_indices);
        selected.extend(diverse);
        
        self.selected_vectors = selected.clone();
        selected
    }
    
    fn select_boundary_vectors(
        &self,
        vectors: &[Vec<f32>],
        centroids: &[Vec<f32>],
        budget: usize,
    ) -> Vec<VectorSelection> {
        let mut boundaries = Vec::new();
        let distance_compute = UnifiedDistanceCompute::new();
        
        for (idx, vector) in vectors.iter().enumerate() {
            // Calculate distances to all centroids
            let mut distances: Vec<f32> = centroids
                .iter()
                .map(|c| distance_compute.cosine(vector, c))
                .collect();
            distances.sort_by(|a, b| a.partial_cmp(b).unwrap());
            
            if distances.len() >= 2 {
                let ratio = distances[0] / distances[1];
                if ratio > 0.8 {
                    boundaries.push(VectorSelection {
                        vector_idx: idx as u32,
                        selection_reason: SelectionReason::Boundary,
                        importance_score: ratio,
                    });
                }
            }
        }
        
        // Sort by importance and take top budget
        boundaries.sort_by(|a, b| b.importance_score.partial_cmp(&a.importance_score).unwrap());
        boundaries.truncate(budget);
        boundaries
    }
    
    fn select_representatives(
        &self,
        vectors: &[Vec<f32>],
        budget: usize,
        selected: &HashSet<u32>,
    ) -> Vec<VectorSelection> {
        // Simple k-medoids selection
        let mut representatives = Vec::new();
        let distance_compute = UnifiedDistanceCompute::new();
        
        // Find vectors closest to mean
        let dim = vectors[0].len();
        let mut mean = vec![0.0; dim];
        for v in vectors {
            for (i, &val) in v.iter().enumerate() {
                mean[i] += val;
            }
        }
        for val in &mut mean {
            *val /= vectors.len() as f32;
        }
        
        let mut distances: Vec<(usize, f32)> = vectors
            .iter()
            .enumerate()
            .filter(|(idx, _)| !selected.contains(&(*idx as u32)))
            .map(|(idx, v)| (idx, distance_compute.cosine(v, &mean)))
            .collect();
        
        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        
        for (idx, dist) in distances.iter().take(budget) {
            representatives.push(VectorSelection {
                vector_idx: *idx as u32,
                selection_reason: SelectionReason::Representative,
                importance_score: 1.0 / (1.0 + dist),
            });
        }
        
        representatives
    }
    
    fn select_outliers(
        &self,
        vectors: &[Vec<f32>],
        centroids: &[Vec<f32>],
        budget: usize,
        selected: &HashSet<u32>,
    ) -> Vec<VectorSelection> {
        let mut outliers = Vec::new();
        let distance_compute = UnifiedDistanceCompute::new();
        
        for (idx, vector) in vectors.iter().enumerate() {
            if selected.contains(&(idx as u32)) {
                continue;
            }
            
            // Find distance to nearest centroid
            let min_dist = centroids
                .iter()
                .map(|c| distance_compute.cosine(vector, c))
                .min_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or(f32::MAX);
            
            outliers.push(VectorSelection {
                vector_idx: idx as u32,
                selection_reason: SelectionReason::Outlier,
                importance_score: min_dist,
            });
        }
        
        // Sort by distance (furthest first) and take top budget
        outliers.sort_by(|a, b| b.importance_score.partial_cmp(&a.importance_score).unwrap());
        outliers.truncate(budget);
        outliers
    }
    
    fn select_diverse(
        &self,
        vectors: &[Vec<f32>],
        budget: usize,
        selected: &HashSet<u32>,
    ) -> Vec<VectorSelection> {
        let mut diverse = Vec::new();
        let distance_compute = UnifiedDistanceCompute::new();
        let mut selected_diverse = HashSet::new();
        
        // Start with random vector
        let available: Vec<usize> = (0..vectors.len())
            .filter(|i| !selected.contains(&(*i as u32)))
            .collect();
        
        if available.is_empty() {
            return diverse;
        }
        
        let first = available[0];
        selected_diverse.insert(first);
        diverse.push(VectorSelection {
            vector_idx: first as u32,
            selection_reason: SelectionReason::Diverse,
            importance_score: 1.0,
        });
        
        // Furthest-first traversal
        while diverse.len() < budget && diverse.len() < available.len() {
            let mut best_idx = None;
            let mut best_min_dist = 0.0;
            
            for &idx in &available {
                if selected_diverse.contains(&idx) {
                    continue;
                }
                
                // Find minimum distance to any selected vector
                let min_dist = selected_diverse
                    .iter()
                    .map(|&s| distance_compute.cosine(&vectors[idx], &vectors[s]))
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap_or(f32::MAX);
                
                if min_dist > best_min_dist {
                    best_min_dist = min_dist;
                    best_idx = Some(idx);
                }
            }
            
            if let Some(idx) = best_idx {
                selected_diverse.insert(idx);
                diverse.push(VectorSelection {
                    vector_idx: idx as u32,
                    selection_reason: SelectionReason::Diverse,
                    importance_score: best_min_dist,
                });
            } else {
                break;
            }
        }
        
        diverse
    }
    
    /// Compress distances based on strategy
    fn compress_distances(&self, distances: &[f32]) -> Vec<u8> {
        match self.compression {
            CompressionStrategy::Uncompressed => {
                bincode::serialize(distances).unwrap()
            }
            CompressionStrategy::Float16 => {
                // Use quantization engine to convert to 16-bit representation
                let (quantized, min, max) = self.quantize_to_u16(distances);
                let mut result = Vec::new();
                result.extend_from_slice(&min.to_le_bytes());
                result.extend_from_slice(&max.to_le_bytes());
                for val in quantized {
                    result.extend_from_slice(&val.to_le_bytes());
                }
                result
            }
            CompressionStrategy::Quantized8 => {
                let (quantized, min, max) = self.quantization_engine
                    .quantize_to_u8(distances);
                let mut result = Vec::new();
                result.extend_from_slice(&min.to_le_bytes());
                result.extend_from_slice(&max.to_le_bytes());
                result.extend_from_slice(&quantized);
                result
            }
            CompressionStrategy::Quantized4 => {
                // Pack two 4-bit values into each byte
                let (quantized, min, max) = self.quantization_engine
                    .quantize_to_u8(distances);
                let mut result = Vec::new();
                result.extend_from_slice(&min.to_le_bytes());
                result.extend_from_slice(&max.to_le_bytes());
                
                for chunk in quantized.chunks(2) {
                    let byte = if chunk.len() == 2 {
                        ((chunk[0] >> 4) << 4) | (chunk[1] >> 4)
                    } else {
                        chunk[0] >> 4
                    };
                    result.push(byte);
                }
                result
            }
            CompressionStrategy::DeltaEncoded => {
                // Sort and store deltas
                let mut sorted = distances.to_vec();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                
                let mut result = Vec::new();
                result.extend_from_slice(&sorted[0].to_le_bytes());
                
                for window in sorted.windows(2) {
                    let delta = ((window[1] - window[0]) * 1000.0) as i16;
                    result.extend_from_slice(&delta.to_le_bytes());
                }
                result
            }
            CompressionStrategy::BitPacked => {
                // Bit packing based on range
                let min = distances.iter().cloned().fold(f32::INFINITY, f32::min);
                let max = distances.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let range = max - min;
                
                let mut result = Vec::new();
                result.extend_from_slice(&min.to_le_bytes());
                result.extend_from_slice(&range.to_le_bytes());
                
                // Use 12 bits per value (4096 levels)
                let mut bit_buffer = 0u32;
                let mut bits_in_buffer = 0;
                
                for &d in distances {
                    let normalized = ((d - min) / range * 4095.0) as u32;
                    bit_buffer |= normalized << bits_in_buffer;
                    bits_in_buffer += 12;
                    
                    while bits_in_buffer >= 8 {
                        result.push((bit_buffer & 0xFF) as u8);
                        bit_buffer >>= 8;
                        bits_in_buffer -= 8;
                    }
                }
                
                if bits_in_buffer > 0 {
                    result.push((bit_buffer & 0xFF) as u8);
                }
                
                result
            }
        }
    }
}

impl PxKStorageImpl for SparseCoverageStorage {
    fn store_distances(&mut self, vector_idx: usize, distances: &[f32]) -> Result<()> {
        // Only store if this vector was selected
        if self.selected_vectors.iter().any(|v| v.vector_idx == vector_idx as u32) {
            let compressed = self.compress_distances(distances);
            self.compressed_distances.push(compressed);
        }
        Ok(())
    }
    
    fn get_distances(&self, vector_idx: usize) -> Option<Vec<f32>> {
        // Find if this vector was selected
        let position = self.selected_vectors
            .iter()
            .position(|v| v.vector_idx == vector_idx as u32)?;
        
        // Decompress distances
        let compressed = &self.compressed_distances[position];
        
        match self.compression {
            CompressionStrategy::Uncompressed => {
                bincode::deserialize(compressed).ok()
            }
            CompressionStrategy::Float16 => {
                // Decompress from 16-bit quantized format
                if compressed.len() < 8 {
                    return None;
                }
                let min = f32::from_le_bytes([compressed[0], compressed[1], compressed[2], compressed[3]]);
                let max = f32::from_le_bytes([compressed[4], compressed[5], compressed[6], compressed[7]]);
                let range = max - min;
                
                let mut distances = Vec::new();
                for chunk in compressed[8..].chunks_exact(2) {
                    let val = u16::from_le_bytes([chunk[0], chunk[1]]);
                    let normalized = val as f32 / 65535.0;
                    distances.push(min + normalized * range);
                }
                Some(distances)
            }
            // ... other decompression strategies ...
            _ => None, // Simplified for now
        }
    }
    
    fn detect_boundaries(&self, threshold: f32) -> Vec<BoundaryInfo> {
        // Only detect boundaries for selected vectors
        Vec::new() // Simplified for now
    }
    
    fn memory_usage(&self) -> usize {
        self.compressed_distances.iter().map(|d| d.len()).sum()
    }
    
    fn serialize(&self) -> Result<Vec<u8>> {
        // Serialize all data
        let data = (
            &self.coverage,
            &self.compression,
            &self.selected_vectors,
            &self.compressed_distances,
            &self.num_clusters,
        );
        bincode::serialize(&data).map_err(Into::into)
    }
}

/// Main adaptive P×K storage
pub struct AdaptivePxKStorage {
    pub strategy: PxKStrategy,
    pub storage: Box<dyn PxKStorageImpl>,
}

impl AdaptivePxKStorage {
    /// Create new adaptive storage based on K and D
    pub fn new(k: usize, d: usize, p: usize) -> Self {
        let (strategy, coverage) = Self::determine_strategy(k, d);
        let storage = Self::create_storage(&strategy, k, p);
        
        Self {
            strategy,
            storage,
        }
    }
    
    /// Determine optimal strategy based on K/D relationship
    pub fn determine_strategy(k: usize, d: usize) -> (PxKStrategy, f32) {
        let sqrt_d = (d as f32).sqrt() as usize;
        
        match k {
            // Full storage for small K
            k if k < sqrt_d => (PxKStrategy::DenseFull, 1.0),
            
            // Compressed full storage
            k if k < d / 4 => (PxKStrategy::DenseCompressed, 1.0),
            
            // Adaptive sparse storage
            k if k < 10000 => {
                let coverage = Self::calculate_optimal_coverage(k, d);
                let compression = Self::select_compression(k, d, coverage);
                (PxKStrategy::SparseCoverage { coverage, compression }, coverage)
            }
            
            // Learned index for extreme K
            _ => (PxKStrategy::LearnedIndex, 0.0),
        }
    }
    
    /// Calculate optimal coverage using refined formula
    fn calculate_optimal_coverage(k: usize, d: usize) -> f32 {
        const MIN_COVERAGE: f32 = 0.10;  // 10% floor
        let ratio = k as f32 / d as f32;
        
        if ratio < 0.25 {  // k < d/4
            1.0
        } else if ratio < 1.0 {  // k < d
            // Linear decay from 100% to 50%
            1.0 - 0.5 * (ratio - 0.25) / 0.75
        } else {
            // Logarithmic decay with floor
            let coverage = d as f32 / (k as f32 * (ratio + 2.0).log2());
            coverage.max(MIN_COVERAGE)
        }
    }
    
    /// Select compression strategy based on K/D ratio and coverage
    fn select_compression(k: usize, d: usize, coverage: f32) -> CompressionStrategy {
        let ratio = k as f32 / d as f32;
        
        if ratio < 0.25 {
            CompressionStrategy::Uncompressed
        } else if ratio < 1.0 {
            if coverage > 0.8 {
                CompressionStrategy::Float16
            } else {
                CompressionStrategy::Quantized8
            }
        } else {
            if coverage > 0.3 {
                CompressionStrategy::Quantized8
            } else if coverage > 0.15 {
                CompressionStrategy::Quantized4
            } else {
                CompressionStrategy::DeltaEncoded
            }
        }
    }
    
    /// Create storage implementation based on strategy
    fn create_storage(strategy: &PxKStrategy, k: usize, p: usize) -> Box<dyn PxKStorageImpl> {
        match strategy {
            PxKStrategy::DenseFull | PxKStrategy::DenseCompressed => {
                Box::new(DenseFullStorage {
                    matrix: VectorCentroidMatrix {
                        distances: Vec::with_capacity(p),
                        num_vectors: p,
                        num_clusters: k,
                    },
                })
            }
            PxKStrategy::SparseCoverage { coverage, compression } => {
                Box::new(SparseCoverageStorage::new(*coverage, compression.clone(), k))
            }
            PxKStrategy::LearnedIndex => {
                // TODO: Implement learned index
                Box::new(DenseFullStorage {
                    matrix: VectorCentroidMatrix {
                        distances: Vec::new(),
                        num_vectors: 0,
                        num_clusters: k,
                    },
                })
            }
        }
    }
    
    /// Get memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        self.storage.memory_usage()
    }
}