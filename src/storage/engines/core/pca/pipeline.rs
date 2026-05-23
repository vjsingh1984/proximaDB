//! Spatial Clustering Pipeline for Flush Operations
//!
//! This module provides a unified pipeline for spatial clustering during flush,
//! combining PCA dimensionality reduction with spatial curve encoding.
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::pca::pipeline::{
//!     SpatialClusteringPipeline, PcaClusteringConfig,
//! };
//! use proximadb::storage::engines::core::formats::proximablocks::spatial_traits::CurveType;
//!
//! // Create pipeline for SST (Z-order)
//! let config = PcaClusteringConfig::for_engine(CurveType::ZOrder);
//! let pipeline = SpatialClusteringPipeline::new(config, filesystem, collection_dir).await?;
//!
//! // During flush, cluster blocks
//! let clustered = pipeline.cluster_blocks(&blocks).await?;
//! // clustered.sorted_indices gives the order to write blocks
//! // clustered.spatial_codes gives the code for each block
//! ```

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

use super::config::PCAManagerConfig;
use super::manager::PCAModelManager;
use super::model::EnhancedPCAModel;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;
use crate::storage::engines::core::formats::proximablocks::spatial_traits::{
    CurveType, SpatialCurveEncoder, SpatialEncoderFactory,
};
use crate::storage::persistence::filesystem::FileSystem;

/// Backwards-compat alias for [`PcaClusteringConfig`].
pub type ClusteringConfig = PcaClusteringConfig;

/// Configuration for spatial clustering pipeline
#[derive(Debug, Clone)]
pub struct PcaClusteringConfig {
    /// Type of spatial curve to use
    pub curve_type: CurveType,
    /// Number of PCA dimensions (0 = auto-select)
    pub pca_dimensions: usize,
    /// Bits per dimension for spatial encoding
    pub bits_per_dim: usize,
    /// Minimum vectors required before enabling clustering
    pub min_vectors_for_clustering: usize,
    /// Target variance ratio for PCA (0.0-1.0)
    pub target_variance: f32,
    /// Enable spatial clustering
    pub enabled: bool,
}

impl Default for PcaClusteringConfig {
    fn default() -> Self {
        Self {
            curve_type: CurveType::ZOrder,
            pca_dimensions: 8,
            bits_per_dim: 8,
            min_vectors_for_clustering: 1000,
            target_variance: 0.95,
            enabled: true,
        }
    }
}

impl PcaClusteringConfig {
    /// Create configuration for a specific engine type
    pub fn for_engine(curve_type: CurveType) -> Self {
        match curve_type {
            CurveType::ZOrder => Self {
                curve_type: CurveType::ZOrder,
                pca_dimensions: 8,
                bits_per_dim: 8,
                min_vectors_for_clustering: 500,
                target_variance: 0.90,
                enabled: true,
            },
            CurveType::Hilbert => Self {
                curve_type: CurveType::Hilbert,
                pca_dimensions: 8,
                bits_per_dim: 8,
                min_vectors_for_clustering: 500,
                target_variance: 0.95,
                enabled: true,
            },
            CurveType::AdaCurve => Self {
                curve_type: CurveType::AdaCurve,
                pca_dimensions: 16,
                bits_per_dim: 8,
                min_vectors_for_clustering: 1000,
                target_variance: 0.92,
                enabled: true,
            },
        }
    }

    /// Disable clustering (for small datasets or testing)
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

/// Result of clustering operation
#[derive(Debug)]
pub struct ClusteringResult {
    /// Indices sorted by spatial code (order to write blocks)
    pub sorted_indices: Vec<usize>,
    /// Spatial codes for each block (in original order)
    pub spatial_codes: Vec<SpatialCode>,
    /// Block centroids (in original order)
    pub centroids: Vec<Vec<f32>>,
    /// PCA-projected centroids (in original order)
    pub pca_centroids: Vec<Vec<f32>>,
    /// Whether clustering was actually applied
    pub clustering_applied: bool,
    /// Curve type used
    pub curve_type: CurveType,
}

impl ClusteringResult {
    /// Create a passthrough result when clustering is disabled
    pub fn passthrough(num_blocks: usize) -> Self {
        Self {
            sorted_indices: (0..num_blocks).collect(),
            spatial_codes: vec![SpatialCode::Code64(0); num_blocks],
            centroids: Vec::new(),
            pca_centroids: Vec::new(),
            clustering_applied: false,
            curve_type: CurveType::ZOrder,
        }
    }
}

/// Block information for clustering
#[derive(Debug, Clone)]
pub struct BlockInfo {
    /// Block index
    pub index: usize,
    /// Vectors in this block
    pub vectors: Vec<Vec<f32>>,
    /// Pre-computed centroid (optional, will be computed if None)
    pub centroid: Option<Vec<f32>>,
}

impl BlockInfo {
    /// Create from vectors
    pub fn from_vectors(index: usize, vectors: Vec<Vec<f32>>) -> Self {
        Self {
            index,
            vectors,
            centroid: None,
        }
    }

    /// Create from VectorRecords
    pub fn from_records(index: usize, records: &[VectorRecord]) -> Self {
        let vectors: Vec<Vec<f32>> = records.iter().map(|r| r.vector.clone()).collect();
        Self {
            index,
            vectors,
            centroid: None,
        }
    }

    /// Create with pre-computed centroid (no vectors needed)
    /// Useful when centroids are already computed (e.g., from IndexEntry)
    pub fn from_centroid(index: usize, centroid: Vec<f32>) -> Self {
        Self {
            index,
            vectors: Vec::new(),
            centroid: Some(centroid),
        }
    }

    /// Compute centroid if not already set
    pub fn compute_centroid(&mut self) {
        if self.centroid.is_some() || self.vectors.is_empty() {
            return;
        }

        let dim = self.vectors[0].len();
        let mut centroid = vec![0.0f32; dim];

        for vec in &self.vectors {
            for (i, &v) in vec.iter().enumerate() {
                centroid[i] += v;
            }
        }

        let count = self.vectors.len() as f32;
        for c in &mut centroid {
            *c /= count;
        }

        self.centroid = Some(centroid);
    }

    /// Get centroid (computes if necessary)
    pub fn get_centroid(&mut self) -> &[f32] {
        self.compute_centroid();
        match self.centroid.as_ref() {
            Some(centroid) => centroid,
            None => &self.vectors[0],
        }
    }
}

/// Spatial clustering pipeline for flush operations
///
/// Combines PCA model management with spatial curve encoding to cluster
/// blocks for improved locality during writes.
pub struct SpatialClusteringPipeline {
    /// Configuration
    config: PcaClusteringConfig,
    /// PCA model manager (per-collection)
    pca_manager: Option<PCAModelManager>,
    /// Spatial encoder
    encoder: Box<dyn SpatialCurveEncoder>,
    /// Collection ID
    collection_id: String,
}

impl SpatialClusteringPipeline {
    /// Create a new spatial clustering pipeline
    ///
    /// # Arguments
    /// * `config` - Clustering configuration
    /// * `filesystem` - Filesystem for PCA model persistence
    /// * `collection_id` - Collection identifier
    /// * `collection_dir` - Base directory for collection data
    pub async fn new(
        config: PcaClusteringConfig,
        filesystem: Arc<dyn FileSystem>,
        collection_id: String,
        collection_dir: PathBuf,
    ) -> Result<Self> {
        let encoder = SpatialEncoderFactory::create(
            config.curve_type,
            config.pca_dimensions,
            config.bits_per_dim,
        );

        let pca_manager = if config.enabled {
            let pca_config = PCAManagerConfig {
                max_versions: 3,
                drift_threshold: 0.3,
                min_training_samples: config.min_vectors_for_clustering,
                evaluation_window: 1000,
                retrain_interval_hours: 24,
                enable_incremental: false,
            };

            let manager = PCAModelManager::new(
                collection_id.clone(),
                pca_config,
                filesystem,
                collection_dir,
            )?;
            manager.initialize().await?;
            Some(manager)
        } else {
            None
        };

        Ok(Self {
            config,
            pca_manager,
            encoder,
            collection_id,
        })
    }

    /// Create a lightweight pipeline without persistence (for testing)
    pub fn new_in_memory(config: PcaClusteringConfig) -> Self {
        let encoder = SpatialEncoderFactory::create(
            config.curve_type,
            config.pca_dimensions,
            config.bits_per_dim,
        );

        Self {
            config,
            pca_manager: None,
            encoder,
            collection_id: "in_memory".to_string(),
        }
    }

    /// Check if clustering is enabled and has a trained model
    pub async fn is_ready(&self) -> bool {
        if !self.config.enabled {
            return false;
        }

        if let Some(ref manager) = self.pca_manager {
            manager.has_model().await
        } else {
            false
        }
    }

    /// Train PCA model from vectors (call during first flush with enough data)
    pub async fn train_model(&self, vectors: &[VectorRecord]) -> Result<bool> {
        if !self.config.enabled {
            return Ok(false);
        }

        if vectors.len() < self.config.min_vectors_for_clustering {
            debug!(
                "Not enough vectors for clustering: {} < {}",
                vectors.len(),
                self.config.min_vectors_for_clustering
            );
            return Ok(false);
        }

        if let Some(ref manager) = self.pca_manager {
            // Check if we already have a model
            if manager.has_model().await {
                // Check for drift
                if manager.check_drift(vectors).await? {
                    info!(
                        "Retraining PCA model for collection {} due to drift",
                        self.collection_id
                    );
                    manager
                        .train_and_activate(vectors, self.config.pca_dimensions)
                        .await?;
                }
            } else {
                // Train initial model
                info!(
                    "Training initial PCA model for collection {} with {} vectors",
                    self.collection_id,
                    vectors.len()
                );
                manager
                    .train_and_activate(vectors, self.config.pca_dimensions)
                    .await?;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Train PCA model from raw vectors
    pub async fn train_model_from_vectors(&self, vectors: &[Vec<f32>]) -> Result<bool> {
        if !self.config.enabled {
            return Ok(false);
        }

        if vectors.len() < self.config.min_vectors_for_clustering {
            return Ok(false);
        }

        // Convert to VectorRecords for the manager
        let records: Vec<VectorRecord> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| VectorRecord {
                id: format!("train_{}", i),
                vector: v.clone(),
                metadata: std::collections::HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            })
            .collect();

        self.train_model(&records).await
    }

    /// Cluster blocks and return sorted order with spatial codes
    ///
    /// This is the main entry point for flush operations.
    pub async fn cluster_blocks(&self, blocks: &mut [BlockInfo]) -> Result<ClusteringResult> {
        if !self.config.enabled || blocks.is_empty() {
            return Ok(ClusteringResult::passthrough(blocks.len()));
        }

        // Compute centroids for all blocks
        for block in blocks.iter_mut() {
            block.compute_centroid();
        }

        let centroids: Vec<Vec<f32>> = blocks.iter().filter_map(|b| b.centroid.clone()).collect();

        if centroids.is_empty() {
            return Ok(ClusteringResult::passthrough(blocks.len()));
        }

        // Get PCA model
        let pca_model = if let Some(ref manager) = self.pca_manager {
            manager.get_active_model().await
        } else {
            None
        };

        // Project centroids through PCA
        let pca_centroids = if let Some(ref model) = pca_model {
            match model.project_batch(&centroids) {
                Ok(projected) => projected,
                Err(e) => {
                    debug!("PCA projection failed, using raw centroids: {}", e);
                    // Fall back to using first N dimensions
                    centroids
                        .iter()
                        .map(|c| c.iter().take(self.config.pca_dimensions).cloned().collect())
                        .collect()
                }
            }
        } else {
            // No PCA model, use first N dimensions directly
            centroids
                .iter()
                .map(|c| c.iter().take(self.config.pca_dimensions).cloned().collect())
                .collect()
        };

        // Normalize PCA coordinates to [0, 1] for spatial encoding
        let normalized = self.normalize_coordinates(&pca_centroids);

        // Encode to spatial codes
        let spatial_codes: Vec<SpatialCode> =
            normalized.iter().map(|c| self.encoder.encode(c)).collect();

        // Sort indices by spatial code
        let mut indexed_codes: Vec<(usize, &SpatialCode)> =
            spatial_codes.iter().enumerate().collect();
        indexed_codes.sort_by(|a, b| a.1.cmp(b.1));

        let sorted_indices: Vec<usize> = indexed_codes.iter().map(|(i, _)| *i).collect();

        info!(
            "Clustered {} blocks using {:?} curve for collection {}",
            blocks.len(),
            self.config.curve_type,
            self.collection_id
        );

        Ok(ClusteringResult {
            sorted_indices,
            spatial_codes,
            centroids,
            pca_centroids,
            clustering_applied: true,
            curve_type: self.config.curve_type,
        })
    }

    /// Cluster vectors directly (for engines that don't pre-create blocks)
    ///
    /// Returns the order in which vectors should be written.
    pub async fn cluster_vectors(&self, vectors: &[VectorRecord]) -> Result<Vec<usize>> {
        if !self.config.enabled || vectors.is_empty() {
            return Ok((0..vectors.len()).collect());
        }

        // Get PCA model
        let pca_model = if let Some(ref manager) = self.pca_manager {
            manager.get_active_model().await
        } else {
            None
        };

        // Project vectors through PCA
        let pca_vectors: Vec<Vec<f32>> = if let Some(ref model) = pca_model {
            vectors
                .iter()
                .filter_map(|v| model.project(&v.vector).ok())
                .collect()
        } else {
            // Use first N dimensions
            vectors
                .iter()
                .map(|v| {
                    v.vector
                        .iter()
                        .take(self.config.pca_dimensions)
                        .cloned()
                        .collect()
                })
                .collect()
        };

        if pca_vectors.len() != vectors.len() {
            // Projection failed for some vectors, return original order
            return Ok((0..vectors.len()).collect());
        }

        // Normalize and encode
        let normalized = self.normalize_coordinates(&pca_vectors);
        let spatial_codes: Vec<SpatialCode> =
            normalized.iter().map(|c| self.encoder.encode(c)).collect();

        // Sort indices by spatial code
        let mut indexed_codes: Vec<(usize, &SpatialCode)> =
            spatial_codes.iter().enumerate().collect();
        indexed_codes.sort_by(|a, b| a.1.cmp(b.1));

        Ok(indexed_codes.iter().map(|(i, _)| *i).collect())
    }

    /// Get the spatial encoder
    pub fn encoder(&self) -> &dyn SpatialCurveEncoder {
        self.encoder.as_ref()
    }

    /// Get the curve type
    pub fn curve_type(&self) -> CurveType {
        self.config.curve_type
    }

    /// Encode a single vector's PCA coordinates to spatial code
    pub async fn encode_vector(&self, vector: &[f32]) -> Result<SpatialCode> {
        let pca_coords = if let Some(ref manager) = self.pca_manager {
            if let Some(model) = manager.get_active_model().await {
                model.project(vector)?
            } else {
                vector
                    .iter()
                    .take(self.config.pca_dimensions)
                    .cloned()
                    .collect()
            }
        } else {
            vector
                .iter()
                .take(self.config.pca_dimensions)
                .cloned()
                .collect()
        };

        // Normalize single vector
        let normalized = self.normalize_single(&pca_coords);
        Ok(self.encoder.encode(&normalized))
    }

    // Helper: Normalize coordinates to [0, 1] range
    fn normalize_coordinates(&self, coords: &[Vec<f32>]) -> Vec<Vec<f32>> {
        if coords.is_empty() {
            return Vec::new();
        }

        let dim = coords[0].len();
        if dim == 0 {
            return coords.to_vec();
        }

        // Find min/max per dimension
        let mut mins = vec![f32::INFINITY; dim];
        let mut maxs = vec![f32::NEG_INFINITY; dim];

        for coord in coords {
            for (i, &v) in coord.iter().enumerate() {
                if v < mins[i] {
                    mins[i] = v;
                }
                if v > maxs[i] {
                    maxs[i] = v;
                }
            }
        }

        // Normalize to [0, 1]
        coords
            .iter()
            .map(|coord| {
                coord
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        let range = maxs[i] - mins[i];
                        if range > 1e-10 {
                            (v - mins[i]) / range
                        } else {
                            0.5 // Default to center if no range
                        }
                    })
                    .collect()
            })
            .collect()
    }

    // Helper: Normalize a single vector based on typical ranges
    fn normalize_single(&self, coords: &[f32]) -> Vec<f32> {
        // For single vectors, use a heuristic normalization
        // In production, would use statistics from the PCA model
        coords
            .iter()
            .map(|&v| {
                // Assume PCA coordinates are roughly in [-10, 10] range
                ((v + 10.0) / 20.0).clamp(0.0, 1.0)
            })
            .collect()
    }
}

// ============================================================================
// Synchronous clustering utilities for simple use cases (e.g., SST flush)
// ============================================================================

/// Result of synchronous block clustering
#[derive(Debug)]
pub struct SyncClusteringResult {
    /// Indices sorted by spatial code (order to write blocks)
    pub sorted_indices: Vec<usize>,
    /// Spatial codes for each block (in original order)
    pub spatial_codes: Vec<SpatialCode>,
}

/// Synchronously cluster blocks using PCA + spatial encoding
///
/// This is a simpler alternative to SpatialClusteringPipeline for use cases
/// that don't need async operations or persistent PCA models.
///
/// # Arguments
/// * `centroids` - Pre-computed block centroids
/// * `curve_type` - Type of space-filling curve (ZOrder, Hilbert, AdaCurve)
/// * `target_dims` - Target PCA dimensions (typically 8-32)
///
/// # Returns
/// SyncClusteringResult with sorted indices and spatial codes
pub fn cluster_blocks_sync(
    centroids: &[Vec<f32>],
    curve_type: CurveType,
    target_dims: usize,
) -> SyncClusteringResult {
    if centroids.is_empty() {
        return SyncClusteringResult {
            sorted_indices: Vec::new(),
            spatial_codes: Vec::new(),
        };
    }

    // Train PCA on centroids
    let pca_coords = if centroids.len() >= 10 && centroids[0].len() > target_dims {
        // Train PCA model directly from vectors
        match EnhancedPCAModel::train_from_vectors(centroids, target_dims) {
            Ok(model) => {
                // Project centroids through PCA
                centroids
                    .iter()
                    .filter_map(|c| model.project(c).ok())
                    .collect::<Vec<_>>()
            }
            Err(_) => {
                // Fallback: use first N dimensions
                centroids
                    .iter()
                    .map(|c| c.iter().take(target_dims).cloned().collect())
                    .collect()
            }
        }
    } else {
        // Not enough data or low dim, use directly
        centroids
            .iter()
            .map(|c| c.iter().take(target_dims).cloned().collect())
            .collect()
    };

    // Create encoder
    let encoder = SpatialEncoderFactory::create(curve_type, target_dims, 8);

    // Normalize to [0, 1]
    let normalized = normalize_coordinates_sync(&pca_coords);

    // Encode to spatial codes
    let spatial_codes: Vec<SpatialCode> = normalized.iter().map(|c| encoder.encode(c)).collect();

    // Sort indices by spatial code
    let mut indexed: Vec<(usize, &SpatialCode)> = spatial_codes.iter().enumerate().collect();
    indexed.sort_by(|a, b| a.1.cmp(b.1));
    let sorted_indices: Vec<usize> = indexed.iter().map(|(i, _)| *i).collect();

    SyncClusteringResult {
        sorted_indices,
        spatial_codes,
    }
}

/// Helper: Normalize coordinates to [0, 1] range (sync version)
fn normalize_coordinates_sync(coords: &[Vec<f32>]) -> Vec<Vec<f32>> {
    if coords.is_empty() {
        return Vec::new();
    }

    let dim = coords[0].len();
    if dim == 0 {
        return coords.to_vec();
    }

    // Find min/max per dimension
    let mut mins = vec![f32::INFINITY; dim];
    let mut maxs = vec![f32::NEG_INFINITY; dim];

    for coord in coords {
        for (i, &v) in coord.iter().enumerate() {
            if v < mins[i] {
                mins[i] = v;
            }
            if v > maxs[i] {
                maxs[i] = v;
            }
        }
    }

    // Normalize to [0, 1]
    coords
        .iter()
        .map(|coord| {
            coord
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let range = maxs[i] - mins[i];
                    if range > 1e-10 {
                        (v - mins[i]) / range
                    } else {
                        0.5
                    }
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_vectors(n: usize, dim: usize) -> Vec<Vec<f32>> {
        (0..n)
            .map(|i| {
                (0..dim)
                    .map(|j| ((i * dim + j) as f32) / (n * dim) as f32)
                    .collect()
            })
            .collect()
    }

    fn create_test_records(n: usize, dim: usize) -> Vec<VectorRecord> {
        create_test_vectors(n, dim)
            .into_iter()
            .enumerate()
            .map(|(i, vector)| VectorRecord {
                id: format!("vec_{}", i),
                vector,
                metadata: std::collections::HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            })
            .collect()
    }

    #[test]
    fn test_clustering_config() {
        let config = PcaClusteringConfig::for_engine(CurveType::ZOrder);
        assert_eq!(config.curve_type, CurveType::ZOrder);
        assert!(config.enabled);

        let config = PcaClusteringConfig::for_engine(CurveType::Hilbert);
        assert_eq!(config.curve_type, CurveType::Hilbert);

        let disabled = PcaClusteringConfig::disabled();
        assert!(!disabled.enabled);
    }

    #[test]
    fn test_block_info() {
        let vectors = create_test_vectors(10, 4);
        let mut block = BlockInfo::from_vectors(0, vectors);

        assert!(block.centroid.is_none());
        block.compute_centroid();
        assert!(block.centroid.is_some());

        let centroid = block.centroid.as_ref().unwrap();
        assert_eq!(centroid.len(), 4);
    }

    #[tokio::test]
    async fn test_in_memory_pipeline() {
        let config = PcaClusteringConfig::for_engine(CurveType::ZOrder);
        let pipeline = SpatialClusteringPipeline::new_in_memory(config);

        // Not ready without a trained model
        assert!(!pipeline.is_ready().await);

        // Create blocks
        let mut blocks: Vec<BlockInfo> = (0..5)
            .map(|i| {
                let vectors = create_test_vectors(10, 16);
                BlockInfo::from_vectors(i, vectors)
            })
            .collect();

        // Cluster blocks (will use fallback without trained model)
        let result = pipeline.cluster_blocks(&mut blocks).await.unwrap();

        assert_eq!(result.sorted_indices.len(), 5);
        assert_eq!(result.spatial_codes.len(), 5);
        assert!(result.clustering_applied);
    }

    #[tokio::test]
    async fn test_cluster_vectors() {
        let config = PcaClusteringConfig::for_engine(CurveType::Hilbert);
        let pipeline = SpatialClusteringPipeline::new_in_memory(config);

        let records = create_test_records(100, 32);
        let order = pipeline.cluster_vectors(&records).await.unwrap();

        assert_eq!(order.len(), 100);
        // All indices should be present
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, (0..100).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn test_disabled_pipeline() {
        let config = PcaClusteringConfig::disabled();
        let pipeline = SpatialClusteringPipeline::new_in_memory(config);

        let mut blocks: Vec<BlockInfo> = (0..3)
            .map(|i| BlockInfo::from_vectors(i, create_test_vectors(5, 8)))
            .collect();

        let result = pipeline.cluster_blocks(&mut blocks).await.unwrap();

        // Should return passthrough (no clustering)
        assert!(!result.clustering_applied);
        assert_eq!(result.sorted_indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_normalize_coordinates() {
        let config = PcaClusteringConfig::default();
        let pipeline = SpatialClusteringPipeline::new_in_memory(config);

        let coords = vec![vec![0.0, 10.0], vec![5.0, 20.0], vec![10.0, 30.0]];

        let normalized = pipeline.normalize_coordinates(&coords);

        // First should be [0, 0], last should be [1, 1]
        assert!((normalized[0][0] - 0.0).abs() < 0.001);
        assert!((normalized[0][1] - 0.0).abs() < 0.001);
        assert!((normalized[2][0] - 1.0).abs() < 0.001);
        assert!((normalized[2][1] - 1.0).abs() < 0.001);
    }
}
