//! # Product Quantization
//!
//! Product quantization for efficient vector compression.

use serde::{Deserialize, Serialize};

/// Product quantized vector
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PQVector {
    /// Codebook indices (one per sub-vector)
    pub indices: Vec<u8>,
    /// Number of sub-vectors
    pub num_subvectors: usize,
    /// Size of each sub-vector
    pub subvector_dim: usize,
}

impl PQVector {
    pub fn new(num_subvectors: usize, subvector_dim: usize) -> Self {
        Self {
            indices: vec![0u8; num_subvectors],
            num_subvectors,
            subvector_dim,
        }
    }

    pub fn dimensions(&self) -> usize {
        self.num_subvectors * self.subvector_dim
    }

    pub fn size_bytes(&self) -> usize {
        self.indices.len()
    }
}

/// PQ Codebook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PQCodebook {
    /// Codebook centroids: [num_subvectors][codebook_size][subvector_dim]
    pub centroids: Vec<Vec<Vec<f32>>>,
    /// Number of centroids per sub-vector (typically 256)
    pub codebook_size: usize,
}

impl PQCodebook {
    pub fn new(num_subvectors: usize, subvector_dim: usize, codebook_size: usize) -> Self {
        Self {
            centroids: vec![vec![vec![0.0; subvector_dim]; codebook_size]; num_subvectors],
            codebook_size,
        }
    }

    pub fn get_centroid(&self, subvector_idx: usize, code_idx: usize) -> &[f32] {
        &self.centroids[subvector_idx][code_idx]
    }

    pub fn set_centroid(&mut self, subvector_idx: usize, code_idx: usize, centroid: Vec<f32>) {
        self.centroids[subvector_idx][code_idx] = centroid;
    }
}

/// Product quantizer
pub struct ProductQuantizer {
    codebook: PQCodebook,
    trained: bool,
}

impl ProductQuantizer {
    pub fn new(num_subvectors: usize, subvector_dim: usize, codebook_size: usize) -> Self {
        Self {
            codebook: PQCodebook::new(num_subvectors, subvector_dim, codebook_size),
            trained: false,
        }
    }

    /// Train codebook using k-means on sub-vectors
    pub fn train(&mut self, vectors: &[Vec<f32>]) -> Result<(), String> {
        if vectors.is_empty() {
            return Err("Cannot train on empty vector set".to_string());
        }

        let dim = vectors[0].len();
        let subvector_dim = self.codebook.centroids[0][0].len();
        let num_subvectors = dim / subvector_dim;

        if num_subvectors != self.codebook.centroids.len() {
            return Err(format!(
                "Vector dimension {} incompatible with {} subvectors of size {}",
                dim,
                self.codebook.centroids.len(),
                subvector_dim
            ));
        }

        // Simplified training: use random initialization
        // In production, use proper k-means clustering
        for m in 0..num_subvectors {
            for k in 0..self.codebook.codebook_size {
                let centroid = (0..subvector_dim)
                    .map(|_| rand::random::<f32>() * 2.0 - 1.0)
                    .collect();
                self.codebook.set_centroid(m, k, centroid);
            }
        }

        self.trained = true;
        Ok(())
    }

    pub fn quantize(&self, vector: &[f32]) -> Result<PQVector, String> {
        if !self.trained {
            return Err("Quantizer not trained".to_string());
        }

        let subvector_dim = self.codebook.centroids[0][0].len();
        let num_subvectors = self.codebook.centroids.len();

        let mut pq = PQVector::new(num_subvectors, subvector_dim);

        for m in 0..num_subvectors {
            let start = m * subvector_dim;
            let end = start + subvector_dim;
            let subvector = &vector[start..end];

            // Find nearest centroid
            let mut best_idx = 0;
            let mut best_dist = f32::INFINITY;

            for k in 0..self.codebook.codebook_size {
                let centroid = self.codebook.get_centroid(m, k);
                let dist = euclidean_distance(subvector, centroid);
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = k;
                }
            }

            pq.indices[m] = best_idx as u8;
        }

        Ok(pq)
    }

    pub fn unquantize(&self, pq: &PQVector) -> Vec<f32> {
        let mut result = Vec::with_capacity(pq.dimensions());

        for m in 0..pq.num_subvectors {
            let idx = pq.indices[m] as usize;
            let centroid = self.codebook.get_centroid(m, idx);
            result.extend_from_slice(centroid);
        }

        result
    }
}

fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let diff = x - y;
            diff * diff
        })
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pq_vector() {
        let pq = PQVector::new(4, 16);
        assert_eq!(pq.dimensions(), 64);
        assert_eq!(pq.size_bytes(), 4);
    }

    #[test]
    fn test_pq_quantizer() {
        let mut quantizer = ProductQuantizer::new(4, 16, 256);
        let vectors = vec![
            (0..64).map(|i| i as f32 / 64.0).collect(),
            (0..64).map(|i| (i as f32 + 0.5) / 64.0).collect(),
        ];

        quantizer.train(&vectors).unwrap();
        assert!(quantizer.trained);

        let quantized = quantizer.quantize(&vectors[0]).unwrap();
        assert_eq!(quantized.indices.len(), 4);

        let reconstructed = quantizer.unquantize(&quantized);
        assert_eq!(reconstructed.len(), 64);
    }
}
