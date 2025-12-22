//! PCA Model Manager with versioning and drift detection
//!
//! This module provides sophisticated PCA model lifecycle management including
//! versioning, drift detection, and automatic retraining triggers for all
//! storage engines (SST, HELIX, SWIFT).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::config::PCAManagerConfig;
use super::model::{EnhancedPCAModel, ModelQuality};
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::persistence::filesystem::FileSystem;

/// Model version metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVersion {
    /// Version identifier
    pub version: u32,
    /// Model creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Number of samples used for training
    pub training_samples: usize,
    /// Variance explained by the model (0.0-1.0)
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// PCA Model Manager with versioning and lifecycle management
///
/// This manager is shared across all storage engines (SST, HELIX, SWIFT)
/// and provides per-collection PCA model management.
pub struct PCAModelManager {
    /// Configuration
    config: PCAManagerConfig,
    /// Collection identifier
    collection_id: String,
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
    /// Create a new PCA model manager for a collection
    ///
    /// # Arguments
    /// * `collection_id` - The collection this manager is for
    /// * `config` - Manager configuration
    /// * `filesystem` - Filesystem for persistence
    /// * `base_dir` - Base directory for the collection
    pub fn new(
        collection_id: String,
        config: PCAManagerConfig,
        filesystem: Arc<dyn FileSystem>,
        base_dir: PathBuf,
    ) -> Result<Self> {
        let model_dir = base_dir.join("__model");

        Ok(Self {
            config,
            collection_id,
            active_model: Arc::new(RwLock::new(None)),
            version_history: Arc::new(RwLock::new(Vec::new())),
            model_cache: Arc::new(RwLock::new(HashMap::new())),
            drift_metrics: Arc::new(RwLock::new(DriftMetrics::default())),
            filesystem,
            model_dir,
            next_version: Arc::new(RwLock::new(1)),
        })
    }

    /// Get the collection ID this manager is for
    pub fn collection_id(&self) -> &str {
        &self.collection_id
    }

    /// Get the model directory path
    pub fn model_dir(&self) -> &PathBuf {
        &self.model_dir
    }

    /// Initialize manager and load existing models from disk
    pub async fn initialize(&self) -> Result<()> {
        // Create model directory if it doesn't exist
        self.filesystem
            .create_dir_all(&self.model_dir.to_string_lossy())
            .await?;

        // Load version history from metadata file
        let metadata_path = self.model_dir.join("versions.json");
        if self
            .filesystem
            .exists(&metadata_path.to_string_lossy())
            .await?
        {
            let metadata_bytes = self
                .filesystem
                .read(&metadata_path.to_string_lossy())
                .await?;
            let versions: Vec<ModelVersion> = serde_json::from_slice(&metadata_bytes)?;

            // Load active model
            for version in &versions {
                if version.is_active {
                    match self.load_model(version.version).await {
                        Ok(model) => {
                            *self.active_model.write().await = Some(model);
                            info!(
                                "Loaded active PCA model version {} for collection {}",
                                version.version, self.collection_id
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Failed to load active PCA model version {}: {}",
                                version.version, e
                            );
                        }
                    }
                    break;
                }
            }

            *self.version_history.write().await = versions;

            // Set next version
            let max_version = self
                .version_history
                .read()
                .await
                .iter()
                .map(|v| v.version)
                .max()
                .unwrap_or(0);
            *self.next_version.write().await = max_version + 1;
        }

        Ok(())
    }

    /// Check if a model is already trained
    pub async fn has_model(&self) -> bool {
        self.active_model.read().await.is_some()
    }

    /// Train a new PCA model from vector records
    pub async fn train_model(&self, vectors: &[VectorRecord], n_components: usize) -> Result<u32> {
        if vectors.len() < self.config.min_training_samples {
            return Err(anyhow::anyhow!(
                "Insufficient training samples: {} < {}",
                vectors.len(),
                self.config.min_training_samples
            ));
        }

        info!(
            "Training new PCA model with {} vectors for collection {}",
            vectors.len(),
            self.collection_id
        );

        // Train the model
        let model = EnhancedPCAModel::train(vectors, n_components)?;

        // Calculate metrics
        let variance_explained = model.variance_explained();
        let avg_reconstruction_error = self.calculate_avg_reconstruction_error(&model, vectors)?;

        // Get version number
        let version = *self.next_version.read().await;
        *self.next_version.write().await = version + 1;

        // Save model to disk
        let model_filename = format!("pca_model_v{}.bin", version);
        let model_path = self.model_dir.join(&model_filename);
        let model_bytes = model.to_bytes()?;
        self.filesystem
            .write(&model_path.to_string_lossy(), &model_bytes, None)
            .await?;

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
        self.version_history.write().await.push(version_meta);

        // Cache the model
        self.model_cache.write().await.insert(version, model);

        // Save version history
        self.save_version_history().await?;

        info!(
            "Trained PCA model version {} with {:.2}% variance explained for collection {}",
            version,
            variance_explained * 100.0,
            self.collection_id
        );

        Ok(version)
    }

    /// Train and immediately activate a model
    pub async fn train_and_activate(&self, vectors: &[VectorRecord], n_components: usize) -> Result<u32> {
        let version = self.train_model(vectors, n_components).await?;
        self.activate_version(version).await?;
        Ok(version)
    }

    /// Activate a specific model version
    pub async fn activate_version(&self, version: u32) -> Result<()> {
        // Load model if not in cache
        if !self.model_cache.read().await.contains_key(&version) {
            let model = self.load_model(version).await?;
            self.model_cache.write().await.insert(version, model);
        }

        // Get model from cache
        let model = self
            .model_cache
            .read()
            .await
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
        *self.drift_metrics.write().await = DriftMetrics::default();

        info!(
            "Activated PCA model version {} for collection {}",
            version, self.collection_id
        );
        Ok(())
    }

    /// Get the active model for projection
    pub async fn get_active_model(&self) -> Option<EnhancedPCAModel> {
        self.active_model.read().await.clone()
    }

    /// Project a vector using the active model
    pub async fn project(&self, vector: &[f32]) -> Result<Option<Vec<f32>>> {
        if let Some(ref model) = *self.active_model.read().await {
            Ok(Some(model.project(vector)?))
        } else {
            Ok(None)
        }
    }

    /// Project multiple vectors using the active model
    pub async fn project_batch(&self, vectors: &[Vec<f32>]) -> Result<Option<Vec<Vec<f32>>>> {
        if let Some(ref model) = *self.active_model.read().await {
            Ok(Some(model.project_batch(vectors)?))
        } else {
            Ok(None)
        }
    }

    /// Check for model drift and return whether retraining is needed
    pub async fn check_drift(&self, recent_vectors: &[VectorRecord]) -> Result<bool> {
        let active_model = self.active_model.read().await;
        if active_model.is_none() {
            return Ok(false);
        }
        let model = active_model.as_ref().unwrap();

        // Calculate reconstruction errors
        let mut errors = Vec::new();
        for vector in recent_vectors.iter().take(self.config.evaluation_window) {
            if let Ok(error) = model.reconstruction_error(&vector.vector) {
                errors.push(error);
            }
        }

        if errors.is_empty() {
            return Ok(false);
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
        let baseline_error = self
            .get_active_version_metadata()
            .await?
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
                "Model drift detected for collection {}: score={:.3} > threshold={:.3}",
                self.collection_id, drift_score, self.config.drift_threshold
            );
        }

        Ok(needs_retraining)
    }

    /// Get the number of components in the active model
    pub async fn n_components(&self) -> Option<usize> {
        self.active_model
            .read()
            .await
            .as_ref()
            .map(|m| m.n_components)
    }

    /// Get model quality metrics for the active version
    pub async fn get_quality_metrics(&self) -> Option<ModelQuality> {
        let guard = self.active_model.read().await;
        let model = guard.as_ref()?;

        let version_meta = self.version_history
            .read()
            .await
            .iter()
            .find(|v| v.is_active)
            .cloned();

        version_meta.map(|v| ModelQuality {
            version: v.version,
            avg_reconstruction_error: v.avg_reconstruction_error,
            variance_explained: v.variance_explained,
            training_samples: v.training_samples,
            created_at: v.created_at,
        })
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

    /// Cleanup old versions to save disk space
    pub async fn cleanup_old_versions(&self) -> Result<()> {
        let mut history = self.version_history.write().await;

        if history.len() <= self.config.max_versions {
            return Ok(());
        }

        // Sort by version (newest first)
        history.sort_by(|a, b| b.version.cmp(&a.version));

        // Find versions to remove
        let to_remove: Vec<u32> = history
            .iter()
            .skip(self.config.max_versions)
            .filter(|v| !v.is_active)
            .map(|v| v.version)
            .collect();

        // Remove from history
        history.retain(|v| !to_remove.contains(&v.version) || v.is_active);
        drop(history); // Release lock before file operations

        // Remove from cache and delete files
        for version in &to_remove {
            self.model_cache.write().await.remove(version);

            let model_filename = format!("pca_model_v{}.bin", version);
            let model_path = self.model_dir.join(&model_filename);
            if self
                .filesystem
                .exists(&model_path.to_string_lossy())
                .await?
            {
                self.filesystem
                    .delete(&model_path.to_string_lossy())
                    .await?;
            }

            info!(
                "Removed old PCA model version {} for collection {}",
                version, self.collection_id
            );
        }

        // Save updated history
        self.save_version_history().await?;

        Ok(())
    }

    // Private helper methods

    async fn load_model(&self, version: u32) -> Result<EnhancedPCAModel> {
        let model_filename = format!("pca_model_v{}.bin", version);
        let model_path = self.model_dir.join(&model_filename);

        let model_bytes = self
            .filesystem
            .read(&model_path.to_string_lossy())
            .await
            .context(format!(
                "Failed to read model version {} for collection {}",
                version, self.collection_id
            ))?;

        EnhancedPCAModel::from_bytes(&model_bytes)
    }

    async fn save_version_history(&self) -> Result<()> {
        let metadata_path = self.model_dir.join("versions.json");
        let history = self.version_history.read().await;
        let json = serde_json::to_vec_pretty(&*history)?;
        self.filesystem
            .write(&metadata_path.to_string_lossy(), &json, None)
            .await?;
        Ok(())
    }

    async fn get_active_version_metadata(&self) -> Result<Option<ModelVersion>> {
        Ok(self
            .version_history
            .read()
            .await
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
        let mut count = 0;

        for vector in vectors.iter().take(sample_size) {
            if let Ok(error) = model.reconstruction_error(&vector.vector) {
                total_error += error;
                count += 1;
            }
        }

        if count == 0 {
            return Ok(0.0);
        }

        Ok(total_error / count as f32)
    }
}

/// Lightweight in-memory PCA manager for cases where persistence is not needed
///
/// This is useful for short-lived operations or testing.
#[derive(Debug)]
pub struct InMemoryPCAManager {
    /// Active model
    active_model: Option<EnhancedPCAModel>,
    /// Model version history
    model_history: Vec<EnhancedPCAModel>,
    /// Maximum history size
    max_history: usize,
    /// Quality metrics
    quality_metrics: HashMap<u32, ModelQuality>,
}

impl InMemoryPCAManager {
    /// Create a new in-memory PCA manager
    pub fn new(max_history: usize) -> Self {
        Self {
            active_model: None,
            model_history: Vec::new(),
            max_history,
            quality_metrics: HashMap::new(),
        }
    }

    /// Train and activate a new model
    pub fn train_new_model(&mut self, records: &[VectorRecord], n_components: usize) -> Result<()> {
        let model = EnhancedPCAModel::train(records, n_components)?;

        // Calculate quality metrics
        let mut total_error = 0.0;
        let sample_size = records.len().min(100);

        for record in &records[..sample_size] {
            total_error += model.reconstruction_error(&record.vector)?;
        }

        let quality = ModelQuality {
            version: model.version,
            avg_reconstruction_error: total_error / sample_size as f32,
            variance_explained: model.variance_explained(),
            training_samples: model.training_samples,
            created_at: chrono::Utc::now(),
        };

        self.quality_metrics.insert(model.version, quality);

        // Archive current model if exists
        if let Some(current) = self.active_model.take() {
            self.model_history.push(current);
            if self.model_history.len() > self.max_history {
                self.model_history.remove(0);
            }
        }

        self.active_model = Some(model);

        Ok(())
    }

    /// Get the active model
    pub fn get_active_model(&self) -> Option<&EnhancedPCAModel> {
        self.active_model.as_ref()
    }

    /// Project a vector
    pub fn project(&self, vector: &[f32]) -> Result<Option<Vec<f32>>> {
        if let Some(ref model) = self.active_model {
            Ok(Some(model.project(vector)?))
        } else {
            Ok(None)
        }
    }

    /// Check if retraining is needed based on drift
    pub fn needs_retraining(&self, new_samples: &[VectorRecord], threshold: f32) -> bool {
        if let Some(ref model) = self.active_model {
            // Check reconstruction error on new samples
            let sample_size = new_samples.len().min(50);
            if sample_size == 0 {
                return false;
            }

            let mut total_error = 0.0;
            for record in &new_samples[..sample_size] {
                if let Ok(error) = model.reconstruction_error(&record.vector) {
                    total_error += error;
                }
            }

            let avg_error = total_error / sample_size as f32;

            // Retrain if error exceeds threshold * baseline
            if let Some(quality) = self.quality_metrics.get(&model.version) {
                return avg_error > quality.avg_reconstruction_error * (1.0 + threshold);
            }
        }

        false
    }
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
                    metadata: std::collections::HashMap::new(),
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
    fn test_in_memory_manager_train() {
        let mut manager = InMemoryPCAManager::new(3);
        let records = create_test_records(100, 16);

        manager.train_new_model(&records, 4).unwrap();

        assert!(manager.active_model.is_some());
        assert_eq!(manager.quality_metrics.len(), 1);
    }

    #[test]
    fn test_in_memory_manager_project() {
        let mut manager = InMemoryPCAManager::new(3);
        let records = create_test_records(100, 16);

        manager.train_new_model(&records, 4).unwrap();

        let test_vec: Vec<f32> = (0..16).map(|i| i as f32 / 10.0).collect();
        let projected = manager.project(&test_vec).unwrap();

        assert!(projected.is_some());
        assert_eq!(projected.unwrap().len(), 4);
    }

    #[test]
    fn test_in_memory_manager_history() {
        let mut manager = InMemoryPCAManager::new(2);
        let records = create_test_records(100, 16);

        // Train multiple models
        manager.train_new_model(&records, 4).unwrap();
        manager.train_new_model(&records, 4).unwrap();
        manager.train_new_model(&records, 4).unwrap();

        // Should only keep 2 in history
        assert_eq!(manager.model_history.len(), 2);
    }
}
