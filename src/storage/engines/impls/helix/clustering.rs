//! PCA and Hilbert curve clustering for HELIX engine
//!
//! This module implements the core clustering logic that makes HELIX unique:
//! - PCA for dimensionality reduction
//! - Hilbert curve mapping for locality preservation
//! - Liquid clustering based on access patterns

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::persistence::filesystem::FileSystem;

/// Hilbert key type
///
/// 64-bit Hilbert curve key used for spatial indexing and clustering.
/// Vectors that are close in high-dimensional space map to close Hilbert keys.
pub type HilbertKey = u64;

/// PCA model for dimensionality reduction
///
/// Principal Component Analysis model that reduces high-dimensional vectors
/// to lower-dimensional representations while preserving maximum variance.
/// Uses real SVD-based computation for accurate dimensionality reduction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PCAModel {
    /// Principal components (eigenvectors), one per component
    pub components: Vec<Vec<f32>>,
    /// Mean vector for centering input data
    pub mean: Vec<f32>,
    /// Variance explained by each component (normalized to sum to 1.0)
    pub explained_variance: Vec<f32>,
    /// Number of components in the reduced space
    pub n_components: usize,
    /// Original high dimension before reduction
    pub original_dim: usize,
    /// Model version for tracking updates
    pub version: u32,
}

impl PCAModel {
    /// Train a PCA model from vector records using real SVD-based PCA
    ///
    /// This implementation uses nalgebra's SVD to compute principal components,
    /// providing true dimensionality reduction that preserves maximum variance.
    pub fn train(records: &[VectorRecord], n_components: usize) -> Result<Self> {
        if records.is_empty() {
            anyhow::bail!("Cannot train PCA on empty records");
        }

        let original_dim = records[0].vector.len();
        let n_samples = records.len();

        // For very small datasets, use randomized projection as fallback
        // (SVD on tiny datasets may be unstable)
        if n_samples < n_components * 2 {
            tracing::warn!(
                "[HELIX PCA] Too few samples ({}) for {} components, using fallback",
                n_samples,
                n_components
            );
            return Self::train_randomized_fallback(records, n_components);
        }

        tracing::info!(
            "[HELIX PCA] Training real SVD-based PCA: {} samples, {} dims -> {} components",
            n_samples,
            original_dim,
            n_components
        );

        // Step 1: Calculate mean
        let mean: Vec<f32> = (0..original_dim)
            .map(|j| {
                let sum: f32 = records.iter().map(|r| r.vector[j]).sum();
                sum / n_samples as f32
            })
            .collect();

        // Step 2: Build centered data matrix and compute SVD
        // For large datasets (>10K samples), use power iteration/randomized SVD
        let (components, explained_variance) = if n_samples > 10_000 || original_dim > 1024 {
            Self::compute_pca_power_iteration(records, &mean, n_components, original_dim)?
        } else {
            Self::compute_pca_svd(records, &mean, n_components, n_samples, original_dim)?
        };

        tracing::info!(
            "[HELIX PCA] Training complete: top eigenvalue ratio = {:.4}",
            explained_variance.first().unwrap_or(&0.0)
                / explained_variance.iter().sum::<f32>().max(1e-10)
        );

        Ok(Self {
            components,
            mean,
            explained_variance,
            n_components,
            original_dim,
            version: 1,
        })
    }

    /// Compute PCA using full SVD (for moderate-sized datasets)
    fn compute_pca_svd(
        records: &[VectorRecord],
        mean: &[f32],
        n_components: usize,
        n_samples: usize,
        original_dim: usize,
    ) -> Result<(Vec<Vec<f32>>, Vec<f32>)> {
        use nalgebra::DMatrix;

        // Build centered data matrix (n_samples x original_dim)
        let mut data = DMatrix::<f64>::zeros(n_samples, original_dim);
        for (i, record) in records.iter().enumerate() {
            for (j, &val) in record.vector.iter().enumerate() {
                data[(i, j)] = (val - mean[j]) as f64;
            }
        }

        // Compute SVD: X = U * S * V^T
        // The right singular vectors V are the principal components
        let svd = data.svd(false, true);

        let v_t = svd
            .v_t
            .ok_or_else(|| anyhow::anyhow!("SVD failed to compute V^T"))?;
        let singular_values = svd.singular_values;

        // Extract top n_components from V^T (V^T is original_dim x original_dim)
        // Each row of V^T is a principal component
        let mut components = Vec::with_capacity(n_components);
        let mut explained_variance = Vec::with_capacity(n_components);

        // Total variance for normalization
        let total_variance: f64 =
            singular_values.iter().map(|s| s * s).sum::<f64>() / (n_samples - 1) as f64;

        for i in 0..n_components.min(v_t.nrows()) {
            // Extract i-th row of V^T as the i-th principal component
            let component: Vec<f32> = (0..original_dim).map(|j| v_t[(i, j)] as f32).collect();
            components.push(component);

            // Explained variance = singular_value^2 / (n-1)
            let variance = singular_values[i] * singular_values[i] / (n_samples - 1) as f64;
            explained_variance.push((variance / total_variance.max(1e-10)) as f32);
        }

        Ok((components, explained_variance))
    }

    /// Compute PCA using power iteration (for large datasets)
    /// This is more memory-efficient for very large datasets
    fn compute_pca_power_iteration(
        records: &[VectorRecord],
        mean: &[f32],
        n_components: usize,
        original_dim: usize,
    ) -> Result<(Vec<Vec<f32>>, Vec<f32>)> {
        use rand::{Rng, SeedableRng};

        let n_samples = records.len();
        let n_iterations = 10; // Usually converges in 5-10 iterations

        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let mut components = Vec::with_capacity(n_components);
        let mut explained_variance = Vec::with_capacity(n_components);

        // Deflation-based power iteration for multiple components
        let mut deflated_records: Vec<Vec<f32>> = records
            .iter()
            .map(|r| {
                r.vector
                    .iter()
                    .zip(mean.iter())
                    .map(|(v, m)| v - m)
                    .collect()
            })
            .collect();

        for _comp_idx in 0..n_components {
            // Initialize random vector
            let mut v: Vec<f64> = (0..original_dim)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect();

            // Normalize
            let norm: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
            v.iter_mut().for_each(|x| *x /= norm.max(1e-10));

            // Power iteration: v = X^T * X * v, normalized
            for _ in 0..n_iterations {
                // Compute X * v (project onto v)
                let xv: Vec<f64> = deflated_records
                    .iter()
                    .map(|row| {
                        row.iter()
                            .zip(&v)
                            .map(|(&r, &vi)| r as f64 * vi)
                            .sum::<f64>()
                    })
                    .collect();

                // Compute X^T * (X * v)
                let mut new_v = vec![0.0_f64; original_dim];
                for (row, &xvi) in deflated_records.iter().zip(&xv) {
                    for (j, &r) in row.iter().enumerate() {
                        new_v[j] += r as f64 * xvi;
                    }
                }

                // Normalize
                let norm: f64 = new_v.iter().map(|x| x * x).sum::<f64>().sqrt();
                v = new_v.iter().map(|x| x / norm.max(1e-10)).collect();
            }

            // Compute eigenvalue (variance explained)
            let xv: Vec<f64> = deflated_records
                .iter()
                .map(|row| {
                    row.iter()
                        .zip(&v)
                        .map(|(&r, &vi)| r as f64 * vi)
                        .sum::<f64>()
                })
                .collect();
            let eigenvalue: f64 = xv.iter().map(|x| x * x).sum::<f64>() / (n_samples - 1) as f64;

            components.push(v.iter().map(|&x| x as f32).collect());
            explained_variance.push(eigenvalue as f32);

            // Deflate: remove component from data
            for (row, &proj) in deflated_records.iter_mut().zip(&xv) {
                for (j, r) in row.iter_mut().enumerate() {
                    *r -= (proj * v[j]) as f32;
                }
            }
        }

        // Normalize explained variance to sum to 1
        let total: f32 = explained_variance.iter().sum();
        if total > 1e-10 {
            explained_variance.iter_mut().for_each(|e| *e /= total);
        }

        Ok((components, explained_variance))
    }

    /// Fallback to randomized projection for very small datasets
    fn train_randomized_fallback(records: &[VectorRecord], n_components: usize) -> Result<Self> {
        let original_dim = records[0].vector.len();
        let n_samples = records.len();

        // Calculate mean
        let mean: Vec<f32> = (0..original_dim)
            .map(|j| {
                let sum: f32 = records.iter().map(|r| r.vector[j]).sum();
                sum / n_samples as f32
            })
            .collect();

        // Use random projection with orthogonalization for better quality
        let components = Self::random_orthogonal_projection(original_dim, n_components);
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

    /// Random orthogonal projection (better than pure random for fallback)
    fn random_orthogonal_projection(original_dim: usize, n_components: usize) -> Vec<Vec<f32>> {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);

        let mut components: Vec<Vec<f32>> = Vec::with_capacity(n_components);

        for _ in 0..n_components {
            // Generate random vector
            let mut v: Vec<f32> = (0..original_dim)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect();

            // Orthogonalize against previous components (Gram-Schmidt)
            for prev in &components {
                let dot: f32 = v.iter().zip(prev).map(|(a, b)| a * b).sum();
                for (vi, &pi) in v.iter_mut().zip(prev) {
                    *vi -= dot * pi;
                }
            }

            // Normalize
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 1e-10 {
                v.iter_mut().for_each(|x| *x /= norm);
            }

            components.push(v);
        }

        components
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

    /// Serialize PCA model to bytes for persistence
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize PCA model: {}", e))
    }

    /// Deserialize PCA model from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        bincode::deserialize(bytes)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize PCA model: {}", e))
    }

    /// Save PCA model to filesystem
    pub async fn save_to_file(
        &self,
        filesystem: &Arc<
            crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem,
        >,
        model_path: &str,
    ) -> Result<()> {
        let bytes = self.to_bytes()?;
        filesystem
            .write(model_path, &bytes, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write PCA model to {}: {}", model_path, e))?;
        tracing::info!(
            "[HELIX] Saved PCA model to {} (version: {}, {} bytes)",
            model_path,
            self.version,
            bytes.len()
        );
        Ok(())
    }

    /// Load PCA model from filesystem
    pub async fn load_from_file(
        filesystem: &Arc<
            crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem,
        >,
        model_path: &str,
    ) -> Result<Self> {
        let bytes = filesystem
            .read(model_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read PCA model from {}: {}", model_path, e))?;
        let model = Self::from_bytes(&bytes)?;
        tracing::info!(
            "[HELIX] Loaded PCA model from {} (version: {}, {} components)",
            model_path,
            model.version,
            model.n_components
        );
        Ok(model)
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
///
/// Configures adaptive clustering that reorganizes data based on
/// access patterns to improve query performance over time.
#[derive(Debug, Clone)]
pub struct LiquidClusteringConfig {
    /// Enable adaptive clustering
    pub enabled: bool,
    /// Query pattern window size for tracking
    pub query_window_size: usize,
    /// Re-clustering threshold (query count)
    pub recluster_threshold: usize,
    /// Access frequency weight in scoring (0.0-1.0)
    pub access_weight: f32,
    /// Recency weight in scoring (0.0-1.0)
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
///
/// Tracks access patterns for vectors to enable adaptive re-clustering
/// based on query frequency and recency.
#[derive(Debug, Clone, Default)]
pub struct QueryPatternTracker {
    /// Vector ID -> access count
    pub access_counts: HashMap<String, usize>,
    /// Vector ID -> last access time
    pub last_access: HashMap<String, chrono::DateTime<chrono::Utc>>,
    /// Total queries tracked
    pub total_queries: usize,
    /// Hilbert key access histogram for hot region detection
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
    use super::*;
    use crate::storage::engines::impls::helix::hilbert_curve::HilbertCurve;

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
