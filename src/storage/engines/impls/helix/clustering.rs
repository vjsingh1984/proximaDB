//! PCA and Hilbert curve clustering for HELIX engine
//!
//! This module implements the core clustering logic that makes HELIX unique:
//! - PCA for dimensionality reduction
//! - Hilbert curve mapping for locality preservation
//! - Liquid clustering based on access patterns

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::proto::proximadb_v1::VectorRecord;

/// Hilbert key type
pub type HilbertKey = u64;

/// PCA model for dimensionality reduction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PCAModel {
    /// Principal components (eigenvectors)
    pub components: Vec<Vec<f32>>,
    /// Mean vector for centering
    pub mean: Vec<f32>,
    /// Variance explained by each component
    pub explained_variance: Vec<f32>,
    /// Number of components to use
    pub n_components: usize,
    /// Original dimension
    pub original_dim: usize,
    /// Model version for tracking updates
    pub version: u32,
}

impl PCAModel {
    /// Train a PCA model from vector records
    pub fn train(records: &[VectorRecord], n_components: usize) -> Result<Self> {
        if records.is_empty() {
            anyhow::bail!("Cannot train PCA on empty records");
        }

        let original_dim = records[0].vector.len();
        let n_samples = records.len();

        // Calculate mean
        let mean: Vec<f32> = (0..original_dim)
            .map(|j| {
                let sum: f32 = records.iter().map(|r| r.vector[j]).sum();
                sum / n_samples as f32
            })
            .collect();

        // For now, use random projection as a placeholder
        // In production, use proper PCA with eigendecomposition
        let components = Self::random_projection(original_dim, n_components);
        let explained_variance = vec![1.0 / n_components as f32; n_components];

        Ok(Self {
            components,
            mean,
            explained_variance,
            n_components,
            original_dim,
            version: 1,
        })
    }

    /// Random projection for dimensionality reduction (placeholder)
    fn random_projection(original_dim: usize, n_components: usize) -> Vec<Vec<f32>> {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);

        (0..n_components)
            .map(|_| {
                (0..original_dim)
                    .map(|_| rng.gen_range(-1.0..1.0))
                    .collect()
            })
            .collect()
    }

    /// Transform a vector (alias for project)
    pub fn transform(&self, vector: &[f32]) -> Result<Vec<f32>> {
        self.project(vector)
    }

    /// Project a vector to lower dimensions
    pub fn project(&self, vector: &[f32]) -> Result<Vec<f32>> {
        if vector.len() != self.original_dim {
            anyhow::bail!(
                "Vector dimension {} doesn't match PCA model dimension {}",
                vector.len(),
                self.original_dim
            );
        }

        // Center the vector
        let centered: Vec<f32> = vector.iter().zip(&self.mean).map(|(v, m)| v - m).collect();

        // Project using principal components
        let projected: Vec<f32> = self
            .components
            .iter()
            .map(|component| centered.iter().zip(component).map(|(c, p)| c * p).sum())
            .collect();

        Ok(projected)
    }

    /// Project and compute Hilbert key with default configuration
    pub fn project_and_compute_hilbert(&self, vector: &[f32]) -> Result<HilbertKey> {
        self.project_and_compute_hilbert_with_config(vector, 16)
    }

    /// Project and compute Hilbert key with configurable bits per dimension
    pub fn project_and_compute_hilbert_with_config(
        &self,
        vector: &[f32],
        bits_per_dimension: usize,
    ) -> Result<HilbertKey> {
        let projected = self.project(vector)?;
        Ok(compute_hilbert_key_with_config(
            &projected,
            bits_per_dimension,
        ))
    }

    /// Update model with new data (incremental PCA)
    pub fn update(&mut self, new_records: &[VectorRecord]) -> Result<()> {
        // Simplified: retrain from scratch
        // In production, use incremental PCA algorithms
        let new_model = Self::train(new_records, self.n_components)?;
        self.components = new_model.components;
        self.mean = new_model.mean;
        self.explained_variance = new_model.explained_variance;
        self.version += 1;
        Ok(())
    }
}

/// Compute Hilbert key from a low-dimensional vector using true Hilbert curve
pub fn compute_hilbert_key(vector: &[f32]) -> HilbertKey {
    compute_hilbert_key_with_config(vector, 16) // Use default if not specified
}

/// Compute Hilbert key with configurable bits per dimension
pub fn compute_hilbert_key_with_config(vector: &[f32], bits_per_dimension: usize) -> HilbertKey {
    use super::hilbert_curve::HilbertUtils;

    if vector.is_empty() {
        return 0;
    }

    // Use proper Hilbert curve encoding with configurable resolution
    HilbertUtils::vector_to_hilbert_key(vector, bits_per_dimension)
}

/// Liquid clustering configuration
#[derive(Debug, Clone)]
pub struct LiquidClusteringConfig {
    /// Enable adaptive clustering
    pub enabled: bool,
    /// Query pattern window size
    pub query_window_size: usize,
    /// Re-clustering threshold (query count)
    pub recluster_threshold: usize,
    /// Access frequency weight
    pub access_weight: f32,
    /// Recency weight
    pub recency_weight: f32,
}

impl Default for LiquidClusteringConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            query_window_size: 1000,
            recluster_threshold: 10000,
            access_weight: 0.7,
            recency_weight: 0.3,
        }
    }
}

/// Query pattern tracker for liquid clustering
#[derive(Debug, Clone, Default)]
pub struct QueryPatternTracker {
    /// Vector ID -> access count
    pub access_counts: HashMap<String, usize>,
    /// Vector ID -> last access time
    pub last_access: HashMap<String, chrono::DateTime<chrono::Utc>>,
    /// Total queries tracked
    pub total_queries: usize,
    /// Hilbert key access histogram
    pub hilbert_histogram: HashMap<u64, usize>,
}

impl QueryPatternTracker {
    /// Record a query access
    pub fn record_access(&mut self, vector_id: &str, hilbert_key: HilbertKey) {
        *self.access_counts.entry(vector_id.to_string()).or_insert(0) += 1;
        self.last_access
            .insert(vector_id.to_string(), chrono::Utc::now());
        *self.hilbert_histogram.entry(hilbert_key).or_insert(0) += 1;
        self.total_queries += 1;
    }

    /// Get clustering hints for a set of vectors
    pub fn get_clustering_hints(
        &self,
        vector_ids: &[String],
        config: &LiquidClusteringConfig,
    ) -> HashMap<String, f32> {
        let now = chrono::Utc::now();
        let mut scores = HashMap::new();

        for id in vector_ids {
            let access_count = self.access_counts.get(id).copied().unwrap_or(0);
            let last_access = self.last_access.get(id);

            // Calculate access frequency score
            let freq_score =
                (access_count as f32 / self.total_queries.max(1) as f32) * config.access_weight;

            // Calculate recency score
            let recency_score = if let Some(last) = last_access {
                let age_seconds = (now - *last).num_seconds().max(1) as f32;
                (1.0 / age_seconds.ln()).min(1.0) * config.recency_weight
            } else {
                0.0
            };

            scores.insert(id.clone(), freq_score + recency_score);
        }

        scores
    }

    /// Identify hot regions in Hilbert space
    pub fn identify_hot_regions(&self, threshold: f32) -> Vec<(HilbertKey, HilbertKey)> {
        let mut hot_regions = Vec::new();

        // Simple clustering of hot keys
        let total = self.hilbert_histogram.values().sum::<usize>() as f32;
        let mut hot_keys: Vec<u64> = self
            .hilbert_histogram
            .iter()
            .filter(|(_, count)| **count as f32 / total > threshold)
            .map(|(&key, _)| key)
            .collect();

        hot_keys.sort_unstable();

        // Merge adjacent keys into ranges
        if !hot_keys.is_empty() {
            let mut start = hot_keys[0];
            let mut end = hot_keys[0];

            for &key in &hot_keys[1..] {
                if key <= end + 1000 {
                    // Extend range
                    end = key;
                } else {
                    // New range
                    hot_regions.push((start, end));
                    start = key;
                    end = key;
                }
            }
            hot_regions.push((start, end));
        }

        hot_regions
    }
}

/// Sort records by Hilbert keys
pub fn sort_by_hilbert(records: &mut [VectorRecord], hilbert_keys: &[HilbertKey]) -> Result<()> {
    if records.len() != hilbert_keys.len() {
        anyhow::bail!("Records and Hilbert keys length mismatch");
    }

    // Create indices and sort
    let mut indices: Vec<usize> = (0..records.len()).collect();
    indices.sort_by_key(|&i| hilbert_keys[i]);

    // Reorder records in-place using the sorted indices
    // We need to use a temporary vector since we can't modify while iterating
    let sorted_records: Vec<VectorRecord> = indices.iter().map(|&i| records[i].clone()).collect();

    // Copy sorted records back into the original slice
    for (i, record) in sorted_records.into_iter().enumerate() {
        records[i] = record;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::storage::engines::impls::helix::hilbert_curve::HilbertCurve;
    use super::*;

    #[test]
    fn test_hilbert_2d() {
        let curve = HilbertCurve::new(2, 16); // 2 dimensions, 16 bits per dimension (within 21-bit limit)
        let key1 = curve.encode(&[0, 0]);
        let key2 = curve.encode(&[65535, 65535]); // Max value for 16 bits
        assert!(key1 < key2);
    }

    #[test]
    fn test_pca_model() {
        let records = vec![
            VectorRecord {
                id: "1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            },
            VectorRecord {
                id: "2".to_string(),
                vector: vec![4.0, 5.0, 6.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            },
        ];

        let model = PCAModel::train(&records, 2).unwrap();
        assert_eq!(model.n_components, 2);
        assert_eq!(model.original_dim, 3);

        let projected = model.project(&[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(projected.len(), 2);
    }

    #[test]
    fn test_query_pattern_tracker() {
        let mut tracker = QueryPatternTracker::default();
        tracker.record_access("vec1", 100);
        tracker.record_access("vec1", 100);
        tracker.record_access("vec2", 200);

        assert_eq!(tracker.access_counts["vec1"], 2);
        assert_eq!(tracker.access_counts["vec2"], 1);
        assert_eq!(tracker.total_queries, 3);
    }
}
