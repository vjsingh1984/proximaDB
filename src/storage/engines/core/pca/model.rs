//! Enhanced PCA Model with eigendecomposition for spatial clustering
//!
//! This module provides a production-ready PCA implementation using
//! eigendecomposition for true dimensionality reduction. The model is
//! used by SST, HELIX, and SWIFT engines for spatial curve encoding.

use anyhow::Result;
use nalgebra::{DMatrix, DVector, SymmetricEigen};
use serde::{Deserialize, Serialize};

use crate::proto::proximadb_v1::VectorRecord;

/// Enhanced PCA model with proper eigendecomposition
///
/// This model is shared across all storage engines (SST, HELIX, SWIFT) for
/// dimensionality reduction before spatial curve encoding. Each engine uses
/// a different spatial curve (Z-order, Hilbert, AdaCurve) but they all share
/// this PCA infrastructure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedPCAModel {
    /// Principal components (eigenvectors) - each row is a component
    pub components: DMatrix<f32>,
    /// Mean vector for centering
    pub mean: DVector<f32>,
    /// Eigenvalues (variance explained)
    pub eigenvalues: DVector<f32>,
    /// Cumulative variance explained (0.0-1.0)
    pub cumulative_variance: Vec<f32>,
    /// Number of components to use
    pub n_components: usize,
    /// Original dimension of input vectors
    pub original_dim: usize,
    /// Total variance in original data
    pub total_variance: f32,
    /// Model version for tracking updates
    pub version: u32,
    /// Training sample size
    pub training_samples: usize,
}

impl EnhancedPCAModel {
    /// Train a PCA model using eigendecomposition
    ///
    /// # Arguments
    /// * `records` - Training vectors
    /// * `n_components` - Number of principal components to extract
    ///
    /// # Returns
    /// A trained PCA model ready for projection
    pub fn train(records: &[VectorRecord], n_components: usize) -> Result<Self> {
        if records.is_empty() {
            anyhow::bail!("Cannot train PCA on empty records");
        }

        let original_dim = records[0].vector.len();
        let n_samples = records.len();

        // Validate dimensions
        if n_components > original_dim {
            anyhow::bail!(
                "n_components ({}) cannot exceed original dimensions ({})",
                n_components,
                original_dim
            );
        }

        if n_components > n_samples {
            anyhow::bail!(
                "n_components ({}) cannot exceed number of samples ({})",
                n_components,
                n_samples
            );
        }

        // Convert records to matrix (samples x features)
        let mut data_matrix = DMatrix::zeros(n_samples, original_dim);
        for (i, record) in records.iter().enumerate() {
            for (j, &val) in record.vector.iter().enumerate() {
                data_matrix[(i, j)] = val;
            }
        }

        // Calculate mean vector
        let mean = calculate_mean(&data_matrix);

        // Center the data
        let centered = center_data(&data_matrix, &mean);

        // Compute covariance matrix
        let covariance = compute_covariance(&centered);

        // Perform eigendecomposition
        let eigen = SymmetricEigen::new(covariance);

        // Sort eigenvectors by eigenvalues (descending)
        let (sorted_eigenvectors, sorted_eigenvalues) =
            sort_by_eigenvalues(eigen.eigenvectors, eigen.eigenvalues);

        // Select top n_components
        let components = sorted_eigenvectors.columns(0, n_components).transpose();
        let eigenvalues = sorted_eigenvalues.rows(0, n_components).clone_owned();

        // Calculate variance explained
        let total_variance: f32 = sorted_eigenvalues.iter().sum();
        let cumulative_variance = calculate_cumulative_variance(&eigenvalues, total_variance);

        Ok(Self {
            components,
            mean,
            eigenvalues,
            cumulative_variance,
            n_components,
            original_dim,
            total_variance,
            version: 1,
            training_samples: n_samples,
        })
    }

    /// Train a PCA model from raw vectors (not VectorRecord)
    ///
    /// This is useful when training from block centroids or other raw vector data.
    pub fn train_from_vectors(vectors: &[Vec<f32>], n_components: usize) -> Result<Self> {
        if vectors.is_empty() {
            anyhow::bail!("Cannot train PCA on empty vectors");
        }

        let original_dim = vectors[0].len();
        let n_samples = vectors.len();

        if n_components > original_dim {
            anyhow::bail!(
                "n_components ({}) cannot exceed original dimensions ({})",
                n_components,
                original_dim
            );
        }

        if n_components > n_samples {
            anyhow::bail!(
                "n_components ({}) cannot exceed number of samples ({})",
                n_components,
                n_samples
            );
        }

        // Convert vectors to matrix
        let mut data_matrix = DMatrix::zeros(n_samples, original_dim);
        for (i, vec) in vectors.iter().enumerate() {
            for (j, &val) in vec.iter().enumerate() {
                data_matrix[(i, j)] = val;
            }
        }

        let mean = calculate_mean(&data_matrix);
        let centered = center_data(&data_matrix, &mean);
        let covariance = compute_covariance(&centered);
        let eigen = SymmetricEigen::new(covariance);
        let (sorted_eigenvectors, sorted_eigenvalues) =
            sort_by_eigenvalues(eigen.eigenvectors, eigen.eigenvalues);

        let components = sorted_eigenvectors.columns(0, n_components).transpose();
        let eigenvalues = sorted_eigenvalues.rows(0, n_components).clone_owned();
        let total_variance: f32 = sorted_eigenvalues.iter().sum();
        let cumulative_variance = calculate_cumulative_variance(&eigenvalues, total_variance);

        Ok(Self {
            components,
            mean,
            eigenvalues,
            cumulative_variance,
            n_components,
            original_dim,
            total_variance,
            version: 1,
            training_samples: n_samples,
        })
    }

    /// Train with automatic component selection based on target variance
    ///
    /// # Arguments
    /// * `records` - Training vectors
    /// * `target_variance` - Target cumulative variance ratio (0.0-1.0)
    /// * `max_components` - Maximum number of components to consider
    pub fn train_auto(
        records: &[VectorRecord],
        target_variance: f32,
        max_components: usize,
    ) -> Result<Self> {
        // First train with max components to get variance information
        let full_model = Self::train(records, max_components.min(records[0].vector.len()))?;

        // Find optimal number of components
        let optimal = full_model.optimal_components_for_variance(target_variance);

        // Retrain with optimal components
        Self::train(records, optimal)
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

        // Convert to DVector
        let vec_dv = DVector::from_vec(vector.to_vec());

        // Center the vector
        let centered = &vec_dv - &self.mean;

        // Project using principal components
        let projected = &self.components * centered;

        Ok(projected.iter().cloned().collect())
    }

    /// Project multiple vectors efficiently (batch operation)
    pub fn project_batch(&self, vectors: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        vectors.iter().map(|v| self.project(v)).collect()
    }

    /// Reconstruct a vector from its projection (for validation)
    pub fn reconstruct(&self, projection: &[f32]) -> Result<Vec<f32>> {
        if projection.len() != self.n_components {
            anyhow::bail!(
                "Projection dimension {} doesn't match n_components {}",
                projection.len(),
                self.n_components
            );
        }

        let proj_dv = DVector::from_vec(projection.to_vec());

        // Reconstruct: X_reconstructed = PC^T * projection + mean
        let reconstructed = self.components.transpose() * proj_dv + &self.mean;

        Ok(reconstructed.iter().cloned().collect())
    }

    /// Get the variance explained ratio for each component
    pub fn explained_variance_ratio(&self) -> Vec<f32> {
        self.eigenvalues
            .iter()
            .map(|&ev| ev / self.total_variance)
            .collect()
    }

    /// Determine optimal number of components for target variance
    pub fn optimal_components_for_variance(&self, target_variance_ratio: f32) -> usize {
        for (i, &cum_var) in self.cumulative_variance.iter().enumerate() {
            if cum_var >= target_variance_ratio {
                return i + 1;
            }
        }
        self.n_components
    }

    /// Compute reconstruction error for quality assessment
    pub fn reconstruction_error(&self, vector: &[f32]) -> Result<f32> {
        let projected = self.project(vector)?;
        let reconstructed = self.reconstruct(&projected)?;

        let error: f32 = vector
            .iter()
            .zip(reconstructed.iter())
            .map(|(orig, recon)| (orig - recon).powi(2))
            .sum::<f32>()
            .sqrt();

        Ok(error)
    }

    /// Get the total variance explained by the model
    pub fn variance_explained(&self) -> f32 {
        self.cumulative_variance.last().copied().unwrap_or(0.0)
    }

    /// Incremental update placeholder
    ///
    /// For WORM workloads, models are typically retrained during flush/compaction
    /// rather than incrementally updated.
    pub fn incremental_update(&mut self, new_records: &[VectorRecord]) -> Result<()> {
        self.version += 1;
        self.training_samples += new_records.len();

        tracing::info!(
            "Incremental PCA update: version {} with {} new samples",
            self.version,
            new_records.len()
        );

        Ok(())
    }
}

// Serialization support
impl EnhancedPCAModel {
    /// Serialize the model to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let components_data: Vec<f32> = self.components.iter().copied().collect();
        let mean_data: Vec<f32> = self.mean.iter().copied().collect();
        let eigenvalues_data: Vec<f32> = self.eigenvalues.iter().copied().collect();

        let serializable = (
            self.components.nrows(),
            self.components.ncols(),
            components_data,
            mean_data,
            eigenvalues_data,
            self.cumulative_variance.clone(),
            self.n_components,
            self.version,
            self.training_samples,
        );

        bincode::serialize(&serializable)
            .map_err(|e| anyhow::anyhow!("Failed to serialize PCA model: {}", e))
    }

    /// Deserialize the model from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let (
            nrows,
            ncols,
            components_data,
            mean_data,
            eigenvalues_data,
            cumulative_variance,
            n_components,
            model_version,
            training_samples,
        ): (
            usize,
            usize,
            Vec<f32>,
            Vec<f32>,
            Vec<f32>,
            Vec<f32>,
            usize,
            u32,
            usize,
        ) = bincode::deserialize(data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize PCA model: {}", e))?;

        let components = DMatrix::from_vec(nrows, ncols, components_data);
        let mean = DVector::from_vec(mean_data);
        let eigenvalues = DVector::from_vec(eigenvalues_data);
        let total_variance = eigenvalues.iter().sum();

        Ok(Self {
            components,
            mean,
            eigenvalues,
            cumulative_variance,
            n_components,
            original_dim: ncols,
            total_variance,
            version: model_version,
            training_samples,
        })
    }
}

// Helper functions

fn calculate_mean(data: &DMatrix<f32>) -> DVector<f32> {
    let n_samples = data.nrows() as f32;
    let n_features = data.ncols();

    let mut mean = DVector::zeros(n_features);
    for j in 0..n_features {
        let col_sum: f32 = data.column(j).iter().sum();
        mean[j] = col_sum / n_samples;
    }
    mean
}

fn center_data(data: &DMatrix<f32>, mean: &DVector<f32>) -> DMatrix<f32> {
    let mut centered = data.clone();
    for i in 0..centered.nrows() {
        let mut row = centered.row_mut(i);
        for j in 0..row.len() {
            row[j] -= mean[j];
        }
    }
    centered
}

fn compute_covariance(centered: &DMatrix<f32>) -> DMatrix<f32> {
    let n = centered.nrows() as f32 - 1.0;
    (centered.transpose() * centered) / n
}

fn sort_by_eigenvalues(
    eigenvectors: DMatrix<f32>,
    eigenvalues: DVector<f32>,
) -> (DMatrix<f32>, DVector<f32>) {
    let mut indices: Vec<usize> = (0..eigenvalues.len()).collect();
    indices.sort_by(|&i, &j| {
        eigenvalues[j]
            .partial_cmp(&eigenvalues[i])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut sorted_vectors = DMatrix::zeros(eigenvectors.nrows(), eigenvectors.ncols());
    let mut sorted_values = DVector::zeros(eigenvalues.len());

    for (new_idx, &old_idx) in indices.iter().enumerate() {
        sorted_vectors.set_column(new_idx, &eigenvectors.column(old_idx));
        sorted_values[new_idx] = eigenvalues[old_idx];
    }

    (sorted_vectors, sorted_values)
}

fn calculate_cumulative_variance(eigenvalues: &DVector<f32>, total_variance: f32) -> Vec<f32> {
    let mut cumulative = Vec::with_capacity(eigenvalues.len());
    let mut sum = 0.0;

    for &ev in eigenvalues.iter() {
        sum += ev;
        cumulative.push(sum / total_variance);
    }

    cumulative
}

/// Lightweight PCA model quality metrics
#[derive(Debug, Clone)]
pub struct ModelQuality {
    pub version: u32,
    pub avg_reconstruction_error: f32,
    pub variance_explained: f32,
    pub training_samples: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_records(n: usize, dim: usize) -> Vec<VectorRecord> {
        (0..n)
            .map(|i| {
                let vector: Vec<f32> = (0..dim)
                    .map(|j| (i as f32) * (j as f32 + 1.0) / 100.0)
                    .collect();
                VectorRecord {
                    id: format!("vec_{}", i),
                    vector,
                    metadata: HashMap::new(),
                    timestamp: Some(0),
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                }
            })
            .collect()
    }

    #[test]
    fn test_pca_model_train() {
        let records = create_test_records(100, 16);
        let model = EnhancedPCAModel::train(&records, 4).unwrap();

        assert_eq!(model.n_components, 4);
        assert_eq!(model.original_dim, 16);
        assert_eq!(model.training_samples, 100);
    }

    #[test]
    fn test_pca_project_reconstruct() {
        let records = create_test_records(100, 16);
        let model = EnhancedPCAModel::train(&records, 4).unwrap();

        let test_vec: Vec<f32> = (0..16).map(|i| i as f32 / 10.0).collect();
        let projected = model.project(&test_vec).unwrap();
        assert_eq!(projected.len(), 4);

        let reconstructed = model.reconstruct(&projected).unwrap();
        assert_eq!(reconstructed.len(), 16);
    }

    #[test]
    fn test_pca_variance_explained() {
        let records = create_test_records(100, 8);
        let model = EnhancedPCAModel::train(&records, 4).unwrap();

        let ratios = model.explained_variance_ratio();
        assert_eq!(ratios.len(), 4);

        // First component should explain more than second
        assert!(ratios[0] >= ratios[1]);

        // Total should sum to ~1.0 (or less for partial components)
        let total: f32 = ratios.iter().sum();
        assert!(total <= 1.0);
    }

    #[test]
    fn test_pca_serialization() {
        let records = create_test_records(100, 16);
        let model = EnhancedPCAModel::train(&records, 4).unwrap();

        let bytes = model.to_bytes().unwrap();
        let restored = EnhancedPCAModel::from_bytes(&bytes).unwrap();

        assert_eq!(restored.n_components, model.n_components);
        assert_eq!(restored.original_dim, model.original_dim);
        assert_eq!(restored.version, model.version);
    }

    #[test]
    fn test_pca_train_from_vectors() {
        let vectors: Vec<Vec<f32>> = (0..50)
            .map(|i| (0..8).map(|j| (i * j) as f32 / 100.0).collect())
            .collect();

        let model = EnhancedPCAModel::train_from_vectors(&vectors, 3).unwrap();
        assert_eq!(model.n_components, 3);
        assert_eq!(model.original_dim, 8);
    }
}
