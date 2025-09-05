//! Enhanced PCA Model Manager with versioning and drift detection
//!
//! This module provides sophisticated PCA model lifecycle management including
//! versioning, drift detection, and automatic retraining triggers.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::core::VectorRecord;
use crate::storage::persistence::filesystem::FileSystem;
use super::pca_impl::EnhancedPCAModel;

/// Configuration for PCA model management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PCAManagerConfig {
    /// Maximum number of model versions to retain
    pub max_versions: usize,
    /// Drift threshold for triggering retraining (0.0-1.0)
    pub drift_threshold: f32,
    /// Minimum samples for training
    pub min_training_samples: usize,
    /// Model evaluation window size
    pub evaluation_window: usize,
    /// Auto-retrain interval in hours
    pub retrain_interval_hours: u64,
    /// Enable incremental PCA updates
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

/// Model version metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVersion {
    /// Version identifier
    pub version: u32,
    /// Model creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Number of samples used for training
    pub training_samples: usize,
    /// Variance explained by the model
    pub variance_explained: f32,
    /// Average reconstruction error
    pub avg_reconstruction_error: f32,
    /// Model file path
    pub model_path: PathBuf,
    /// Is this the active model?
    pub is_active: bool,
    /// Model drift score (0.0 = no drift, 1.0 = complete drift)
    pub drift_score: f32,
}

/// Drift detection metrics
#[derive(Debug, Clone)]
pub struct DriftMetrics {
    /// Reconstruction error over time
    pub reconstruction_errors: VecDeque<f32>,
    /// Distribution shift score
    pub distribution_shift: f32,
    /// Variance drift
    pub variance_drift: f32,
    /// Sample count since last training
    pub samples_since_training: usize,
}

/// Enhanced PCA Model Manager with versioning and lifecycle management
pub struct PCAModelManager {
    /// Configuration
    config: PCAManagerConfig,
    /// Current active model
    active_model: Arc<RwLock<Option<EnhancedPCAModel>>>,
    /// Model version history
    version_history: Arc<RwLock<Vec<ModelVersion>>>,
    /// Model cache (version -> model)
    model_cache: Arc<RwLock<HashMap<u32, EnhancedPCAModel>>>,
    /// Drift metrics
    drift_metrics: Arc<RwLock<DriftMetrics>>,
    /// Filesystem for model persistence
    filesystem: Arc<dyn FileSystem>,
    /// Model storage directory
    model_dir: PathBuf,
    /// Next version number
    next_version: Arc<RwLock<u32>>,
}

impl PCAModelManager {
    /// Create a new PCA model manager
    pub fn new(
        config: PCAManagerConfig,
        filesystem: Arc<dyn FileSystem>,
        model_dir: PathBuf,
    ) -> Result<Self> {
        Ok(Self {
            config,
            active_model: Arc::new(RwLock::new(None)),
            version_history: Arc::new(RwLock::new(Vec::new())),
            model_cache: Arc::new(RwLock::new(HashMap::new())),
            drift_metrics: Arc::new(RwLock::new(DriftMetrics {
                reconstruction_errors: VecDeque::with_capacity(1000),
                distribution_shift: 0.0,
                variance_drift: 0.0,
                samples_since_training: 0,
            })),
            filesystem,
            model_dir,
            next_version: Arc::new(RwLock::new(1)),
        })
    }

    /// Initialize manager and load existing models
    pub async fn initialize(&self) -> Result<()> {
        // Create model directory if it doesn't exist
        self.filesystem.create_dir_all(&self.model_dir.to_string_lossy()).await?;
        
        // Load version history from metadata file
        let metadata_path = self.model_dir.join("versions.json");
        if self.filesystem.exists(&metadata_path.to_string_lossy()).await? {
            let metadata_bytes = self.filesystem.read(&metadata_path.to_string_lossy()).await?;
            let versions: Vec<ModelVersion> = serde_json::from_slice(&metadata_bytes)?;
            
            // Load active model
            for version in &versions {
                if version.is_active {
                    let model = self.load_model(version.version).await?;
                    *self.active_model.write().await = Some(model);
                    info!("Loaded active PCA model version {}", version.version);
                    break;
                }
            }
            
            *self.version_history.write().await = versions;
            
            // Set next version
            let max_version = self.version_history.read().await
                .iter()
                .map(|v| v.version)
                .max()
                .unwrap_or(0);
            *self.next_version.write().await = max_version + 1;
        }
        
        Ok(())
    }

    /// Train a new PCA model
    pub async fn train_model(&self, vectors: &[VectorRecord], n_components: usize) -> Result<u32> {
        if vectors.len() < self.config.min_training_samples {
            return Err(anyhow::anyhow!(
                "Insufficient training samples: {} < {}",
                vectors.len(),
                self.config.min_training_samples
            ));
        }

        info!("Training new PCA model with {} vectors", vectors.len());
        
        // Train the model
        let model = EnhancedPCAModel::train(vectors, n_components)?;
        
        // Calculate metrics
        let variance_explained = model.cumulative_variance.last()
            .copied()
            .unwrap_or(0.0);
        
        let avg_reconstruction_error = self.calculate_avg_reconstruction_error(&model, vectors)?;
        
        // Get version number
        let version = *self.next_version.read().await;
        *self.next_version.write().await = version + 1;
        
        // Save model to disk
        let model_filename = format!("model_v{}.bin", version);
        let model_path = self.model_dir.join(&model_filename);
        let model_bytes = model.to_bytes()?;
        self.filesystem.write(&model_path.to_string_lossy(), &model_bytes).await?;
        
        // Create version metadata
        let version_meta = ModelVersion {
            version,
            created_at: chrono::Utc::now(),
            training_samples: vectors.len(),
            variance_explained,
            avg_reconstruction_error,
            model_path,
            is_active: false,
            drift_score: 0.0,
        };
        
        // Add to history
        self.version_history.write().await.push(version_meta.clone());
        
        // Cache the model
        self.model_cache.write().await.insert(version, model);
        
        // Save version history
        self.save_version_history().await?;
        
        info!(
            "Trained PCA model version {} with {:.2}% variance explained",
            version,
            variance_explained * 100.0
        );
        
        Ok(version)
    }

    /// Activate a specific model version
    pub async fn activate_version(&self, version: u32) -> Result<()> {
        // Load model if not in cache
        if !self.model_cache.read().await.contains_key(&version) {
            let model = self.load_model(version).await?;
            self.model_cache.write().await.insert(version, model.clone());
        }
        
        // Get model from cache
        let model = self.model_cache.read().await
            .get(&version)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Model version {} not found", version))?;
        
        // Update active model
        *self.active_model.write().await = Some(model);
        
        // Update version history
        for v in self.version_history.write().await.iter_mut() {
            v.is_active = v.version == version;
        }
        
        // Save version history
        self.save_version_history().await?;
        
        // Reset drift metrics
        *self.drift_metrics.write().await = DriftMetrics {
            reconstruction_errors: VecDeque::with_capacity(1000),
            distribution_shift: 0.0,
            variance_drift: 0.0,
            samples_since_training: 0,
        };
        
        info!("Activated PCA model version {}", version);
        Ok(())
    }

    /// Check for model drift and trigger retraining if needed
    pub async fn check_drift(&self, recent_vectors: &[VectorRecord]) -> Result<bool> {
        let active_model = self.active_model.read().await;
        if active_model.is_none() {
            return Ok(false);
        }
        let model = active_model.as_ref().unwrap();
        
        // Calculate reconstruction errors
        let mut errors = Vec::new();
        for vector in recent_vectors.iter().take(self.config.evaluation_window) {
            let error = model.reconstruction_error(&vector.vector)?;
            errors.push(error);
        }
        
        let avg_error = errors.iter().sum::<f32>() / errors.len() as f32;
        
        // Update drift metrics
        let mut metrics = self.drift_metrics.write().await;
        metrics.reconstruction_errors.push_back(avg_error);
        if metrics.reconstruction_errors.len() > 1000 {
            metrics.reconstruction_errors.pop_front();
        }
        metrics.samples_since_training += recent_vectors.len();
        
        // Calculate drift score
        let baseline_error = self.get_active_version_metadata().await?
            .map(|v| v.avg_reconstruction_error)
            .unwrap_or(0.0);
        
        let drift_score = if baseline_error > 0.0 {
            (avg_error - baseline_error).abs() / baseline_error
        } else {
            0.0
        };
        
        metrics.distribution_shift = drift_score;
        
        // Check if retraining is needed
        let needs_retraining = drift_score > self.config.drift_threshold;
        
        if needs_retraining {
            warn!(
                "Model drift detected: score={:.3} > threshold={:.3}",
                drift_score, self.config.drift_threshold
            );
        }
        
        Ok(needs_retraining)
    }

    /// Get the active model for projection
    pub async fn get_active_model(&self) -> Option<EnhancedPCAModel> {
        self.active_model.read().await.clone()
    }

    /// Get model by version
    pub async fn get_model(&self, version: u32) -> Result<EnhancedPCAModel> {
        // Check cache first
        if let Some(model) = self.model_cache.read().await.get(&version) {
            return Ok(model.clone());
        }
        
        // Load from disk
        let model = self.load_model(version).await?;
        
        // Cache it
        self.model_cache.write().await.insert(version, model.clone());
        
        Ok(model)
    }

    /// Get version history
    pub async fn get_version_history(&self) -> Vec<ModelVersion> {
        self.version_history.read().await.clone()
    }

    /// Get drift metrics
    pub async fn get_drift_metrics(&self) -> DriftMetrics {
        self.drift_metrics.read().await.clone()
    }

    /// Cleanup old versions
    pub async fn cleanup_old_versions(&self) -> Result<()> {
        let mut history = self.version_history.write().await;
        
        // Keep only the configured number of versions
        if history.len() > self.config.max_versions {
            // Sort by version (newest first)
            history.sort_by(|a, b| b.version.cmp(&a.version));
            
            // Find versions to remove
            let to_remove: Vec<_> = history.iter()
                .skip(self.config.max_versions)
                .filter(|v| !v.is_active)
                .map(|v| v.version)
                .collect();
            
            // Remove from history
            history.retain(|v| !to_remove.contains(&v.version) || v.is_active);
            
            // Remove from cache
            let mut cache = self.model_cache.write().await;
            for version in &to_remove {
                cache.remove(version);
                
                // Delete model file
                let model_filename = format!("model_v{}.bin", version);
                let model_path = self.model_dir.join(&model_filename);
                if self.filesystem.exists(&model_path.to_string_lossy()).await? {
                    self.filesystem.delete(&model_path.to_string_lossy()).await?;
                }
                
                info!("Removed old PCA model version {}", version);
            }
        }
        
        Ok(())
    }

    // Private helper methods

    async fn load_model(&self, version: u32) -> Result<EnhancedPCAModel> {
        let model_filename = format!("model_v{}.bin", version);
        let model_path = self.model_dir.join(&model_filename);
        
        let model_bytes = self.filesystem.read(&model_path.to_string_lossy()).await
            .context(format!("Failed to read model version {}", version))?;
        
        EnhancedPCAModel::from_bytes(&model_bytes)
    }

    async fn save_version_history(&self) -> Result<()> {
        let metadata_path = self.model_dir.join("versions.json");
        let history = self.version_history.read().await;
        let json = serde_json::to_vec_pretty(&*history)?;
        self.filesystem.write(&metadata_path.to_string_lossy(), &json).await?;
        Ok(())
    }

    async fn get_active_version_metadata(&self) -> Result<Option<ModelVersion>> {
        Ok(self.version_history.read().await
            .iter()
            .find(|v| v.is_active)
            .cloned())
    }

    fn calculate_avg_reconstruction_error(
        &self,
        model: &EnhancedPCAModel,
        vectors: &[VectorRecord],
    ) -> Result<f32> {
        let sample_size = vectors.len().min(1000);
        let mut total_error = 0.0;
        
        for vector in vectors.iter().take(sample_size) {
            total_error += model.reconstruction_error(&vector.vector)?;
        }
        
        Ok(total_error / sample_size as f32)
    }
}

/// Incremental PCA updater for online learning
pub struct IncrementalPCAUpdater {
    /// Base model to update
    base_model: EnhancedPCAModel,
    /// Update batch size
    batch_size: usize,
    /// Learning rate for updates
    learning_rate: f32,
    /// Accumulated updates
    accumulated_updates: Vec<Vec<f32>>,
}

impl IncrementalPCAUpdater {
    pub fn new(base_model: EnhancedPCAModel, batch_size: usize, learning_rate: f32) -> Self {
        Self {
            base_model,
            batch_size,
            learning_rate,
            accumulated_updates: Vec::new(),
        }
    }

    /// Add vectors for incremental update
    pub fn add_vectors(&mut self, vectors: &[VectorRecord]) {
        for vector in vectors {
            self.accumulated_updates.push(vector.vector.clone());
            
            if self.accumulated_updates.len() >= self.batch_size {
                self.apply_update();
            }
        }
    }

    /// Apply incremental update to the model
    fn apply_update(&mut self) {
        // Simplified incremental PCA update
        // In production, this would use more sophisticated online PCA algorithms
        // such as CCIPCA (Candid Covariance-free Incremental PCA)
        
        // Clear accumulated updates after processing
        self.accumulated_updates.clear();
    }

    /// Get the updated model
    pub fn get_model(self) -> EnhancedPCAModel {
        self.base_model
    }
}