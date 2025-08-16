// Multi-level quantization for SST blocks
// Clean implementation optimized for progressive search
// MIGRATION: Integrating with universal quantization adapter

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::compute::distance_computation::DistanceMetric;
use crate::storage::engines::common::{
    UniversalQuantizationAdapter,
    UniversalQuantizationConfig,
    quantization_common::{
        ProgressiveQuantizationStage,
        UniversalQuantizationLevel,
        // Temporarily disabled - these types may be in different locations
        // ProgressiveSearchResult,
        // QuantizedVector,
    },
};

/// Quantized block with multiple levels for progressive search
#[derive(Debug)]
pub struct QuantizedBlock {
    /// Level 1: Binary sketches (1 bit per dimension)
    pub binary_sketches: Vec<BinarySketch>,
    
    /// Level 2: INT8 vectors (8 bits per dimension)
    pub int8_vectors: Vec<Int8Vector>,
    
    /// Level 3: Product Quantization codes (4-8 bits per dimension)
    pub pq_codes: Vec<PQCode>,
    
    /// Precomputed distance tables for PQ
    pub distance_tables: Arc<RwLock<HashMap<QueryId, DistanceTable>>>,
    
    /// Block-specific quantization parameters
    pub local_scale: f32,
    pub local_offset: f32,
}

/// Binary sketch - 1 bit per dimension
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinarySketch {
    /// Packed bits - each u64 holds 64 dimensions
    pub bits: Vec<u64>,
    pub dimension: usize,
}

impl BinarySketch {
    pub fn new(dimension: usize) -> Self {
        let n_words = (dimension + 63) / 64;
        Self {
            bits: vec![0; n_words],
            dimension,
        }
    }
    
    pub fn from_vector(vector: &[f32], threshold: f32) -> Self {
        let dimension = vector.len();
        let mut sketch = Self::new(dimension);
        
        for (i, &value) in vector.iter().enumerate() {
            if value > threshold {
                let word_idx = i / 64;
                let bit_idx = i % 64;
                sketch.bits[word_idx] |= 1u64 << bit_idx;
            }
        }
        
        sketch
    }
    
    pub fn hamming_distance(&self, other: &BinarySketch) -> u32 {
        let mut distance = 0u32;
        for (a, b) in self.bits.iter().zip(other.bits.iter()) {
            distance += (a ^ b).count_ones();
        }
        distance
    }
}

/// INT8 quantized vector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Int8Vector {
    pub values: Vec<i8>,
    pub scale: f32,
    pub zero_point: i8,
}

impl Int8Vector {
    pub fn from_vector(vector: &[f32]) -> Self {
        // Find min and max
        let min = vector.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max = vector.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        
        // Calculate scale and zero point
        let range = max - min;
        let scale = if range > 0.0 { range / 255.0 } else { 1.0 };
        let zero_point = (-min / scale).round() as i8;
        
        // Quantize values
        let values: Vec<i8> = vector.iter()
            .map(|&v| {
                let scaled = (v / scale + zero_point as f32).round();
                scaled.max(-128.0).min(127.0) as i8
            })
            .collect();
        
        Self {
            values,
            scale,
            zero_point,
        }
    }
    
    pub fn to_vector(&self) -> Vec<f32> {
        self.values.iter()
            .map(|&v| (v as f32 - self.zero_point as f32) * self.scale)
            .collect()
    }
    
    pub fn l2_distance_squared(&self, other: &Int8Vector) -> f32 {
        let mut sum = 0i32;
        for (a, b) in self.values.iter().zip(other.values.iter()) {
            let diff = *a as i32 - *b as i32;
            sum += diff * diff;
        }
        
        // Account for different scales
        let scale_factor = self.scale * other.scale;
        sum as f32 * scale_factor
    }
}

/// Product Quantization code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PQCode {
    /// Subspace indices
    pub codes: Vec<u8>,
    /// Number of subspaces
    pub n_subspaces: u8,
}

impl PQCode {
    pub fn new(n_subspaces: u8) -> Self {
        Self {
            codes: vec![0; n_subspaces as usize],
            n_subspaces,
        }
    }
    
    pub fn encode(vector: &[f32], codebooks: &[Codebook]) -> Self {
        let n_subspaces = codebooks.len() as u8;
        let subspace_dim = vector.len() / codebooks.len();
        let mut codes = Vec::with_capacity(n_subspaces as usize);
        
        for (i, codebook) in codebooks.iter().enumerate() {
            let start = i * subspace_dim;
            let end = start + subspace_dim;
            let subvector = &vector[start..end];
            
            // Find nearest centroid
            let mut best_idx = 0;
            let mut best_dist = f32::INFINITY;
            
            for (idx, centroid) in codebook.centroids.iter().enumerate() {
                let dist = euclidean_distance(subvector, centroid);
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = idx;
                }
            }
            
            codes.push(best_idx as u8);
        }
        
        Self {
            codes,
            n_subspaces,
        }
    }
}

/// Codebook for Product Quantization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Codebook {
    pub segment_id: u8,
    pub dimension: usize,
    pub centroids: Vec<Vec<f32>>,
}

/// Distance table for fast PQ distance computation
#[derive(Debug, Clone)]
pub struct DistanceTable {
    /// Precomputed distances from query to all centroids
    /// Shape: [n_subspaces][256] for 8-bit PQ
    pub distances: Vec<Vec<f32>>,
}

impl DistanceTable {
    pub fn compute(query: &[f32], codebooks: &[Codebook]) -> Self {
        let n_subspaces = codebooks.len();
        let subspace_dim = query.len() / n_subspaces;
        let mut distances = Vec::with_capacity(n_subspaces);
        
        for (i, codebook) in codebooks.iter().enumerate() {
            let start = i * subspace_dim;
            let end = start + subspace_dim;
            let subquery = &query[start..end];
            
            let mut subspace_distances = Vec::with_capacity(256);
            for centroid in &codebook.centroids {
                let dist = euclidean_distance(subquery, centroid);
                subspace_distances.push(dist);
            }
            
            // Pad to 256 if needed
            while subspace_distances.len() < 256 {
                subspace_distances.push(f32::INFINITY);
            }
            
            distances.push(subspace_distances);
        }
        
        Self { distances }
    }
    
    pub fn lookup_distance(&self, pq_code: &PQCode) -> f32 {
        let mut sum = 0.0;
        for (i, &code) in pq_code.codes.iter().enumerate() {
            sum += self.distances[i][code as usize];
        }
        sum
    }
}

/// Query identifier for caching distance tables
pub type QueryId = u64;

/// Quantized index for routing queries
#[derive(Debug)]
pub struct QuantizedIndex {
    /// Global codebooks for PQ
    pub codebooks: Vec<Codebook>,
    
    /// Hierarchical quantized centroids for routing
    pub level1_centroids: Vec<BinaryCentroid>,
    pub level2_centroids: Vec<Int8Centroid>,
    pub level3_centroids: Vec<PQCentroid>,
    
    /// Inverted index: centroid -> block list
    pub centroid_to_blocks: HashMap<CentroidId, Vec<BlockId>>,
    
    /// Precomputed distances between centroids
    pub centroid_distances: Vec<Vec<f32>>,
    
    /// Vector dimension
    dimension: usize,
}

#[derive(Debug, Clone)]
pub struct BinaryCentroid {
    pub id: CentroidId,
    pub sketch: BinarySketch,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct Int8Centroid {
    pub id: CentroidId,
    pub vector: Int8Vector,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct PQCentroid {
    pub id: CentroidId,
    pub code: PQCode,
    pub count: usize,
}

pub type CentroidId = u32;
pub type BlockId = u32;

impl QuantizedIndex {
    pub fn new(dimension: usize) -> Self {
        Self {
            codebooks: Vec::new(),
            level1_centroids: Vec::new(),
            level2_centroids: Vec::new(),
            level3_centroids: Vec::new(),
            centroid_to_blocks: HashMap::new(),
            centroid_distances: Vec::new(),
            dimension,
        }
    }
    
    /// Train codebooks from sample vectors
    pub fn train_codebooks(&mut self, vectors: &[Vec<f32>], n_subspaces: usize) -> Result<()> {
        if vectors.is_empty() {
            return Err(anyhow!("Cannot train codebooks with empty vectors"));
        }
        
        let dimension = vectors[0].len();
        let subspace_dim = dimension / n_subspaces;
        
        self.codebooks.clear();
        
        for i in 0..n_subspaces {
            let start = i * subspace_dim;
            let end = start + subspace_dim;
            
            // Extract subvectors for this subspace
            let subvectors: Vec<Vec<f32>> = vectors.iter()
                .map(|v| v[start..end].to_vec())
                .collect();
            
            // Train codebook using k-means (simplified)
            let codebook = train_codebook(i as u8, &subvectors, 256)?;
            self.codebooks.push(codebook);
        }
        
        Ok(())
    }
    
    /// Find candidate blocks for a query using hierarchical filtering
    pub fn find_candidate_blocks(
        &self,
        query: &[f32],
        n_candidates: usize,
        distance_metric: DistanceMetric,
    ) -> Vec<(BlockId, f32)> {
        // Level 1: Binary filtering
        let binary_query = BinarySketch::from_vector(query, 0.0);
        let mut level1_candidates = Vec::new();
        
        for centroid in &self.level1_centroids {
            let dist = binary_query.hamming_distance(&centroid.sketch) as f32;
            level1_candidates.push((centroid.id, dist));
        }
        
        level1_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        level1_candidates.truncate(n_candidates * 10);
        
        // Level 2: INT8 filtering
        let int8_query = Int8Vector::from_vector(query);
        let mut level2_candidates = Vec::new();
        
        for (centroid_id, _) in level1_candidates {
            if let Some(int8_centroid) = self.level2_centroids.iter().find(|c| c.id == centroid_id) {
                let dist = int8_query.l2_distance_squared(&int8_centroid.vector);
                level2_candidates.push((centroid_id, dist));
            }
        }
        
        level2_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        level2_candidates.truncate(n_candidates * 5);
        
        // Level 3: PQ filtering
        let pq_query = PQCode::encode(query, &self.codebooks);
        let distance_table = DistanceTable::compute(query, &self.codebooks);
        let mut block_candidates = Vec::new();
        
        for (centroid_id, _) in level2_candidates {
            if let Some(blocks) = self.centroid_to_blocks.get(&centroid_id) {
                for &block_id in blocks {
                    // Compute PQ distance (would need actual PQ code for block)
                    let dist = 0.0; // Placeholder
                    block_candidates.push((block_id, dist));
                }
            }
        }
        
        block_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        block_candidates.truncate(n_candidates);
        
        block_candidates
    }
}

impl QuantizedBlock {
    pub fn new(dimension: usize) -> Self {
        Self {
            binary_sketches: Vec::new(),
            int8_vectors: Vec::new(),
            pq_codes: Vec::new(),
            distance_tables: Arc::new(RwLock::new(HashMap::new())),
            local_scale: 1.0,
            local_offset: 0.0,
        }
    }
    
    /// Quantize vectors in this block using universal adapter
    pub fn quantize_vectors_with_adapter(
        &mut self, 
        vectors: &[Vec<f32>], 
        adapter: &UniversalQuantizationAdapter,
        config: &UniversalQuantizationConfig,
    ) -> Result<()> {
        // Use universal adapter for quantization
        let result = adapter.quantize_progressive(vectors, config)?;
        
        // Extract quantized representations from result
        for quantized in result.quantized_vectors {
            // Store binary sketches
            if let Some(binary) = quantized.binary_sketch {
                self.binary_sketches.push(BinarySketch {
                    bits: binary,
                    dimension: vectors[0].len(),
                });
            }
            
            // Store INT8 vectors
            if let Some(int8) = quantized.int8_vector {
                self.int8_vectors.push(Int8Vector {
                    values: int8.values,
                    scale: int8.scale,
                    zero_point: int8.zero_point as i8,
                });
            }
            
            // Store PQ codes
            if let Some(pq) = quantized.pq_code {
                self.pq_codes.push(PQCode {
                    codes: pq.codes,
                    n_subspaces: pq.n_subspaces as u8,
                });
            }
        }
        
        // Update local quantization parameters
        if let Some(scale) = result.statistics.get("global_scale") {
            self.local_scale = scale.as_f64().unwrap_or(1.0) as f32;
        }
        if let Some(offset) = result.statistics.get("global_offset") {
            self.local_offset = offset.as_f64().unwrap_or(0.0) as f32;
        }
        
        Ok(())
    }
    
    /// Legacy quantize vectors method (deprecated, use quantize_vectors_with_adapter)
    pub fn quantize_vectors(&mut self, vectors: &[Vec<f32>], config: &super::QuantizationConfig) -> Result<()> {
        for vector in vectors {
            // Binary quantization
            if config.enable_binary {
                let sketch = BinarySketch::from_vector(vector, config.binary_threshold);
                self.binary_sketches.push(sketch);
            }
            
            // INT8 quantization
            if config.enable_int8 {
                let int8_vec = Int8Vector::from_vector(vector);
                self.int8_vectors.push(int8_vec);
            }
            
            // PQ quantization
            if config.enable_pq && !config.pq_codebooks.is_empty() {
                let pq_code = PQCode::encode(vector, &config.pq_codebooks);
                self.pq_codes.push(pq_code);
            }
        }
        
        Ok(())
    }
}

// Helper functions

fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

fn train_codebook(segment_id: u8, vectors: &[Vec<f32>], n_centroids: usize) -> Result<Codebook> {
    // Simplified k-means implementation
    // In production, use a proper clustering library
    
    if vectors.is_empty() {
        return Err(anyhow!("Cannot train codebook with empty vectors"));
    }
    
    let dimension = vectors[0].len();
    let mut centroids = Vec::new();
    
    // Initialize with random vectors
    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();
    let mut indices: Vec<usize> = (0..vectors.len()).collect();
    indices.shuffle(&mut rng);
    
    for i in 0..n_centroids.min(vectors.len()) {
        centroids.push(vectors[indices[i]].clone());
    }
    
    // Simple k-means iterations (in production, use proper implementation)
    for _ in 0..10 {
        // Assignment step
        let mut clusters: Vec<Vec<Vec<f32>>> = vec![Vec::new(); n_centroids];
        
        for vector in vectors {
            let mut best_idx = 0;
            let mut best_dist = f32::INFINITY;
            
            for (idx, centroid) in centroids.iter().enumerate() {
                let dist = euclidean_distance(vector, centroid);
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = idx;
                }
            }
            
            clusters[best_idx].push(vector.clone());
        }
        
        // Update step
        for (idx, cluster) in clusters.iter().enumerate() {
            if !cluster.is_empty() {
                let mut new_centroid = vec![0.0; dimension];
                for vector in cluster {
                    for (i, &v) in vector.iter().enumerate() {
                        new_centroid[i] += v;
                    }
                }
                for v in &mut new_centroid {
                    *v /= cluster.len() as f32;
                }
                centroids[idx] = new_centroid;
            }
        }
    }
    
    Ok(Codebook {
        segment_id,
        dimension,
        centroids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_binary_sketch() {
        let vector = vec![0.1, -0.2, 0.3, -0.4, 0.5];
        let sketch = BinarySketch::from_vector(&vector, 0.0);
        
        // Check that positive values are set
        assert_eq!(sketch.bits[0] & 1, 1);  // 0.1 > 0
        assert_eq!((sketch.bits[0] >> 2) & 1, 1);  // 0.3 > 0
        assert_eq!((sketch.bits[0] >> 4) & 1, 1);  // 0.5 > 0
        
        // Test Hamming distance
        let vector2 = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let sketch2 = BinarySketch::from_vector(&vector2, 0.0);
        let distance = sketch.hamming_distance(&sketch2);
        assert_eq!(distance, 2);  // Two bits differ
    }
    
    #[test]
    fn test_int8_quantization() {
        let vector = vec![0.0, 1.0, 2.0, 3.0, 4.0];
        let int8_vec = Int8Vector::from_vector(&vector);
        
        // Test reconstruction
        let reconstructed = int8_vec.to_vector();
        for (original, reconstructed) in vector.iter().zip(reconstructed.iter()) {
            assert!((original - reconstructed).abs() < 0.1);
        }
        
        // Test distance computation
        let vector2 = vec![0.5, 1.5, 2.5, 3.5, 4.5];
        let int8_vec2 = Int8Vector::from_vector(&vector2);
        let dist = int8_vec.l2_distance_squared(&int8_vec2);
        assert!(dist > 0.0);
    }
    
    #[test]
    fn test_pq_encoding() {
        // Create simple codebooks
        let codebooks = vec![
            Codebook {
                segment_id: 0,
                dimension: 2,
                centroids: vec![
                    vec![0.0, 0.0],
                    vec![1.0, 0.0],
                    vec![0.0, 1.0],
                    vec![1.0, 1.0],
                ],
            },
            Codebook {
                segment_id: 1,
                dimension: 2,
                centroids: vec![
                    vec![0.0, 0.0],
                    vec![1.0, 0.0],
                    vec![0.0, 1.0],
                    vec![1.0, 1.0],
                ],
            },
        ];
        
        let vector = vec![0.9, 0.1, 0.1, 0.9];
        let pq_code = PQCode::encode(&vector, &codebooks);
        
        assert_eq!(pq_code.n_subspaces, 2);
        assert_eq!(pq_code.codes.len(), 2);
    }
}