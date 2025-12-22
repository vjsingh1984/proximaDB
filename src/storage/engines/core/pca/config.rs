//! PCA Configuration for spatial clustering
//!
//! This module provides configuration structures for PCA model training,
//! drift detection, and adaptive dimensionality selection.

use serde::{Deserialize, Serialize};

/// Configuration for PCA model training and management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PCAConfig {
    /// Minimum number of vectors required before training a PCA model
    pub min_training_vectors: usize,
    /// Retrain the model after this many new vectors are added
    pub retrain_interval_vectors: usize,
    /// Drift threshold (0.0-1.0) for triggering automatic retraining
    /// A drift score above this triggers model revalidation
    pub drift_threshold: f32,
    /// Target number of PCA dimensions (typically 8-64)
    /// Set to 0 for automatic selection based on variance explained
    pub target_dimensions: usize,
    /// Target variance ratio for automatic dimension selection (0.0-1.0)
    /// Default 0.95 means keep enough components to explain 95% of variance
    pub target_variance_ratio: f32,
    /// Maximum number of dimensions even if variance target not met
    pub max_dimensions: usize,
    /// Enable PCA-based spatial clustering
    pub enabled: bool,
}

impl Default for PCAConfig {
    fn default() -> Self {
        Self {
            min_training_vectors: 1000,
            retrain_interval_vectors: 10000,
            drift_threshold: 0.1,
            target_dimensions: 0, // Auto-select
            target_variance_ratio: 0.95,
            max_dimensions: 64,
            enabled: true,
        }
    }
}

impl PCAConfig {
    /// Create a configuration optimized for high-dimensional data
    pub fn high_dimensional() -> Self {
        Self {
            min_training_vectors: 500,
            retrain_interval_vectors: 5000,
            drift_threshold: 0.15,
            target_dimensions: 16,
            target_variance_ratio: 0.90,
            max_dimensions: 32,
            enabled: true,
        }
    }

    /// Create a configuration optimized for low-latency applications
    pub fn low_latency() -> Self {
        Self {
            min_training_vectors: 2000,
            retrain_interval_vectors: 50000,
            drift_threshold: 0.2,
            target_dimensions: 8,
            target_variance_ratio: 0.85,
            max_dimensions: 16,
            enabled: true,
        }
    }

    /// Create a configuration optimized for high recall (quality)
    pub fn high_recall() -> Self {
        Self {
            min_training_vectors: 500,
            retrain_interval_vectors: 5000,
            drift_threshold: 0.05,
            target_dimensions: 0, // Auto-select
            target_variance_ratio: 0.98,
            max_dimensions: 64,
            enabled: true,
        }
    }
}

/// Configuration for PCA model manager lifecycle
#[derive(Debug, Clone)]
pub struct PCAManagerConfig {
    /// Maximum number of model versions to retain
    pub max_versions: usize,
    /// Drift threshold for triggering retraining (0.0-1.0)
    pub drift_threshold: f32,
    /// Minimum samples for training
    pub min_training_samples: usize,
    /// Model evaluation window size (number of samples for drift detection)
    pub evaluation_window: usize,
    /// Auto-retrain interval in hours (0 = disabled)
    pub retrain_interval_hours: u64,
    /// Enable incremental PCA updates between full retrains
    pub enable_incremental: bool,
}

impl Default for PCAManagerConfig {
    fn default() -> Self {
        Self {
            max_versions: 5,
            drift_threshold: 0.3,
            min_training_samples: 10000,
            evaluation_window: 1000,
            retrain_interval_hours: 24,
            enable_incremental: false,
        }
    }
}

impl PCAManagerConfig {
    /// Create from PCAConfig for convenience
    pub fn from_pca_config(config: &PCAConfig) -> Self {
        Self {
            max_versions: 5,
            drift_threshold: config.drift_threshold,
            min_training_samples: config.min_training_vectors,
            evaluation_window: 1000,
            retrain_interval_hours: 24,
            enable_incremental: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pca_config_default() {
        let config = PCAConfig::default();
        assert_eq!(config.min_training_vectors, 1000);
        assert_eq!(config.target_variance_ratio, 0.95);
        assert!(config.enabled);
    }

    #[test]
    fn test_pca_config_presets() {
        let high_dim = PCAConfig::high_dimensional();
        assert_eq!(high_dim.target_dimensions, 16);

        let low_lat = PCAConfig::low_latency();
        assert_eq!(low_lat.target_dimensions, 8);

        let high_recall = PCAConfig::high_recall();
        assert_eq!(high_recall.target_variance_ratio, 0.98);
    }

    #[test]
    fn test_manager_config_from_pca() {
        let pca_config = PCAConfig::default();
        let manager_config = PCAManagerConfig::from_pca_config(&pca_config);
        assert_eq!(
            manager_config.drift_threshold,
            pca_config.drift_threshold
        );
    }
}
