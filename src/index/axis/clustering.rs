//! ML Clustering for AXIS
//!
//! This module provides ML-based clustering capabilities for the AXIS indexing system.
//! It supports various clustering algorithms and integrates with the adaptive indexing strategy.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::index::axis::types::{DataType, IndexAlgorithm, IndexSpecification};

use crate::compute::distance::DistanceMetric;
use crate::compute::unified_distance::UnifiedDistanceCompute;
use crate::core::VectorRecord;

/// Clustering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringConfig {
    /// Algorithm to use
    pub algorithm: ClusteringAlgorithm,
    /// Minimum vectors required before clustering
    pub min_vectors_for_clustering: usize,
    /// Maximum number of clusters
    pub max_clusters: usize,
    /// Distance metric for clustering
    pub distance_metric: DistanceMetric,
    /// Enable adaptive cluster count
    pub adaptive_cluster_count: bool,
    /// Recompute clusters after this many new vectors
    pub recompute_threshold: usize,
    /// Enable incremental updates
    pub enable_incremental: bool,
}

impl Default for ClusteringConfig {
    fn default() -> Self {
        Self {
            algorithm: ClusteringAlgorithm::KMeans(KMeansConfig::default()),
            min_vectors_for_clustering: 1000,
            max_clusters: 256,
            distance_metric: DistanceMetric::Cosine,
            adaptive_cluster_count: true,
            recompute_threshold: 10000,
            enable_incremental: true,
        }
    }
}

/// Clustering algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusteringAlgorithm {
    /// K-Means clustering
    KMeans(KMeansConfig),
    /// Hierarchical clustering
    Hierarchical(HierarchicalConfig),
    /// DBSCAN clustering
    DBSCAN(DBSCANConfig),
}

/// K-Means configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KMeansConfig {
    /// Number of clusters (k)
    pub k: usize,
    /// Maximum iterations
    pub max_iterations: usize,
    /// Convergence tolerance
    pub tolerance: f32,
    /// Number of initializations
    pub n_init: usize,
    /// Initialization method
    pub init_method: KMeansInit,
}

impl Default for KMeansConfig {
    fn default() -> Self {
        Self {
            k: 32,
            max_iterations: 100,
            tolerance: 1e-4,
            n_init: 3,
            init_method: KMeansInit::KMeansPlusPlus,
        }
    }
}

/// K-Means initialization methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KMeansInit {
    /// Random initialization
    Random,
    /// K-Means++ initialization
    KMeansPlusPlus,
    /// Custom seed centroids
    Custom(Vec<Vec<f32>>),
}

/// Hierarchical clustering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchicalConfig {
    /// Linkage criterion
    pub linkage: LinkageCriterion,
    /// Distance threshold for cutting dendrogram
    pub distance_threshold: f32,
}

/// Linkage criteria for hierarchical clustering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LinkageCriterion {
    Single,
    Complete,
    Average,
    Ward,
}

/// DBSCAN configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DBSCANConfig {
    /// Epsilon neighborhood distance
    pub eps: f32,
    /// Minimum points in neighborhood
    pub min_samples: usize,
}

/// Cluster assignment for a vector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterAssignment {
    /// Cluster ID
    pub cluster_id: u32,
    /// Distance to cluster centroid
    pub distance_to_centroid: f32,
    /// Confidence score (0-1)
    pub confidence: f32,
}

/// Clustering model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringModel {
    /// Algorithm used
    pub algorithm: ClusteringAlgorithm,
    /// Cluster centroids (for algorithms that use them)
    pub centroids: Vec<Vec<f32>>,
    /// Number of vectors per cluster
    pub cluster_sizes: Vec<usize>,
    /// Total vectors clustered
    pub total_vectors: usize,
    /// Model version
    pub version: Option<u32>,
    /// Model quality metrics
    pub metrics: ClusteringMetrics,
}

/// Clustering quality metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringMetrics {
    /// Silhouette score (-1 to 1, higher is better)
    pub silhouette_score: f32,
    /// Davies-Bouldin index (lower is better)
    pub davies_bouldin_index: f32,
    /// Calinski-Harabasz index (higher is better)
    pub calinski_harabasz_index: f32,
    /// Average intra-cluster distance
    pub avg_intra_cluster_distance: f32,
    /// Average inter-cluster distance
    pub avg_inter_cluster_distance: f32,
}

/// AXIS clustering engine
pub struct AxisClusteringEngine {
    /// Configuration
    config: ClusteringConfig,
    /// Current models per collection
    models: Arc<RwLock<HashMap<String, ClusteringModel>>>,
    /// Pending vectors for incremental updates
    pending_vectors: Arc<RwLock<HashMap<String, Vec<VectorRecord>>>>,
    /// Distance computation
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Statistics
    stats: Arc<RwLock<ClusteringStats>>,
}

/// Clustering statistics
#[derive(Debug, Default)]
pub struct ClusteringStats {
    /// Total clustering operations
    pub total_clustering_ops: u64,
    /// Total vectors clustered
    pub total_vectors_clustered: u64,
    /// Average clustering time (ms)
    pub avg_clustering_time_ms: f64,
    /// Cache hit rate
    pub cache_hit_rate: f32,
}

impl AxisClusteringEngine {
    /// Create new clustering engine
    pub fn new(config: ClusteringConfig) -> Self {
        Self {
            config,
            models: Arc::new(RwLock::new(HashMap::new())),
            pending_vectors: Arc::new(RwLock::new(HashMap::new())),
            distance_compute: Arc::new(UnifiedDistanceCompute::default()),
            stats: Arc::new(RwLock::new(ClusteringStats::default())),
        }
    }

    /// Train clustering model for a collection
    pub async fn train_model(
        &self,
        collection_id: &str,
        vectors: Vec<VectorRecord>,
    ) -> Result<ClusteringModel> {
        let start_time = std::time::Instant::now();
        
        tracing::info!(
            "🎯 Training clustering model for collection {} with {} vectors",
            collection_id,
            vectors.len()
        );

        // Check minimum vectors requirement
        if vectors.len() < self.config.min_vectors_for_clustering {
            return Err(anyhow::anyhow!(
                "Not enough vectors for clustering: {} < {}",
                vectors.len(),
                self.config.min_vectors_for_clustering
            ));
        }

        // Extract vector data
        let vector_data: Vec<Vec<f32>> = vectors
            .iter()
            .map(|v| v.vector.clone())
            .collect();

        // Train based on algorithm
        let model = match &self.config.algorithm {
            ClusteringAlgorithm::KMeans(config) => {
                self.train_kmeans(config, vector_data).await?
            }
            ClusteringAlgorithm::Hierarchical(config) => {
                self.train_hierarchical(config, vector_data).await?
            }
            ClusteringAlgorithm::DBSCAN(config) => {
                self.train_dbscan(config, vector_data).await?
            }
        };

        // Update stats
        let elapsed = start_time.elapsed();
        {
            let mut stats = self.stats.write().await;
            stats.total_clustering_ops += 1;
            stats.total_vectors_clustered += vectors.len() as u64;
            stats.avg_clustering_time_ms = 
                (stats.avg_clustering_time_ms * (stats.total_clustering_ops - 1) as f64 
                + elapsed.as_millis() as f64) / stats.total_clustering_ops as f64;
        }

        // Store model
        {
            let mut models = self.models.write().await;
            models.insert(collection_id.to_string(), model.clone());
        }

        tracing::info!(
            "✅ Clustering model trained in {:?} - {} clusters created",
            elapsed,
            model.centroids.len()
        );

        Ok(model)
    }

    /// Assign vector to cluster
    pub async fn assign_vector(
        &self,
        collection_id: &str,
        vector: &[f32],
    ) -> Result<ClusterAssignment> {
        let models = self.models.read().await;
        let model = models
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No clustering model for collection {}", collection_id))?;

        // Find nearest centroid
        let mut best_cluster = 0;
        let mut best_distance = f32::MAX;
        
        for (idx, centroid) in model.centroids.iter().enumerate() {
            let similarity = self.distance_compute.calculate_distance(
                vector,
                centroid,
                &self.config.distance_metric,
            );
            
            if similarity.raw_value < best_distance {
                best_distance = similarity.raw_value;
                best_cluster = idx;
            }
        }

        // Calculate confidence based on distance
        let confidence = 1.0 / (1.0 + best_distance);

        Ok(ClusterAssignment {
            cluster_id: best_cluster as u32,
            distance_to_centroid: best_distance,
            confidence,
        })
    }

    /// Get top-k nearest clusters
    pub async fn get_nearest_clusters(
        &self,
        collection_id: &str,
        vector: &[f32],
        k: usize,
    ) -> Result<Vec<(u32, f32)>> {
        let models = self.models.read().await;
        let model = models
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No clustering model for collection {}", collection_id))?;

        // Calculate distances to all centroids
        let mut cluster_distances: Vec<(u32, f32)> = model
            .centroids
            .iter()
            .enumerate()
            .map(|(idx, centroid)| {
                let similarity = self.distance_compute.calculate_distance(
                    vector,
                    centroid,
                    &self.config.distance_metric,
                );
                (idx as u32, similarity.raw_value)
            })
            .collect();

        // Sort by distance and take top-k
        cluster_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        cluster_distances.truncate(k);

        Ok(cluster_distances)
    }

    /// Add vector for incremental update
    pub async fn add_pending_vector(
        &self,
        collection_id: &str,
        vector: VectorRecord,
    ) -> Result<()> {
        let mut pending = self.pending_vectors.write().await;
        pending
            .entry(collection_id.to_string())
            .or_insert_with(Vec::new)
            .push(vector);

        // Check if we need to recompute
        let pending_count = pending.get(collection_id).map(|v| v.len()).unwrap_or(0);
        if pending_count >= self.config.recompute_threshold {
            // TODO: Trigger recomputation
            tracing::info!(
                "🔄 Recomputation threshold reached for collection {} ({} pending vectors)",
                collection_id,
                pending_count
            );
        }

        Ok(())
    }

    /// Get clustering model for collection
    pub async fn get_model(&self, collection_id: &str) -> Option<ClusteringModel> {
        let models = self.models.read().await;
        models.get(collection_id).cloned()
    }

    /// Train K-Means model
    async fn train_kmeans(
        &self,
        config: &KMeansConfig,
        vectors: Vec<Vec<f32>>,
    ) -> Result<ClusteringModel> {
        // Determine optimal K if adaptive
        let k = if self.config.adaptive_cluster_count {
            self.determine_optimal_k(&vectors, config.k).await?
        } else {
            config.k
        };

        tracing::debug!("Training K-Means with k={}", k);

        // Initialize centroids
        let mut centroids = self.initialize_kmeans_centroids(&vectors, k, &config.init_method)?;
        
        // Run K-Means iterations
        let mut cluster_assignments = vec![0usize; vectors.len()];
        let mut cluster_sizes = vec![0usize; k];
        
        for iteration in 0..config.max_iterations {
            let mut changed = false;
            
            // Assignment step
            for (idx, vector) in vectors.iter().enumerate() {
                let mut best_cluster = 0;
                let mut best_distance = f32::MAX;
                
                for (cluster_idx, centroid) in centroids.iter().enumerate() {
                    let similarity = self.distance_compute.calculate_distance(
                        vector,
                        centroid,
                        &self.config.distance_metric,
                    );
                    
                    if similarity.raw_value < best_distance {
                        best_distance = similarity.raw_value;
                        best_cluster = cluster_idx;
                    }
                }
                
                if cluster_assignments[idx] != best_cluster {
                    changed = true;
                    cluster_assignments[idx] = best_cluster;
                }
            }
            
            // Update step
            cluster_sizes.fill(0);
            centroids.iter_mut().for_each(|c| c.fill(0.0));
            
            for (idx, vector) in vectors.iter().enumerate() {
                let cluster = cluster_assignments[idx];
                cluster_sizes[cluster] += 1;
                
                for (i, val) in vector.iter().enumerate() {
                    centroids[cluster][i] += val;
                }
            }
            
            // Average centroids
            for (cluster_idx, size) in cluster_sizes.iter().enumerate() {
                if *size > 0 {
                    for val in centroids[cluster_idx].iter_mut() {
                        *val /= *size as f32;
                    }
                }
            }
            
            // Check convergence
            if !changed {
                tracing::debug!("K-Means converged at iteration {}", iteration);
                break;
            }
        }

        // Calculate metrics
        let metrics = self.calculate_clustering_metrics(&vectors, &centroids, &cluster_assignments)?;

        Ok(ClusteringModel {
            algorithm: ClusteringAlgorithm::KMeans(config.clone()),
            centroids,
            cluster_sizes,
            total_vectors: vectors.len(),
            version: Some(1),
            metrics,
        })
    }

    /// Train hierarchical clustering model
    async fn train_hierarchical(
        &self,
        _config: &HierarchicalConfig,
        _vectors: Vec<Vec<f32>>,
    ) -> Result<ClusteringModel> {
        // TODO: Implement hierarchical clustering
        Err(anyhow::anyhow!("Hierarchical clustering not yet implemented"))
    }

    /// Train DBSCAN model
    async fn train_dbscan(
        &self,
        _config: &DBSCANConfig,
        _vectors: Vec<Vec<f32>>,
    ) -> Result<ClusteringModel> {
        // TODO: Implement DBSCAN
        Err(anyhow::anyhow!("DBSCAN clustering not yet implemented"))
    }

    /// Initialize K-Means centroids
    fn initialize_kmeans_centroids(
        &self,
        vectors: &[Vec<f32>],
        k: usize,
        method: &KMeansInit,
    ) -> Result<Vec<Vec<f32>>> {
        match method {
            KMeansInit::Random => {
                // Random initialization
                use rand::seq::SliceRandom;
                let mut rng = rand::thread_rng();
                let mut indices: Vec<usize> = (0..vectors.len()).collect();
                indices.shuffle(&mut rng);
                
                Ok(indices
                    .into_iter()
                    .take(k)
                    .map(|i| vectors[i].clone())
                    .collect())
            }
            KMeansInit::KMeansPlusPlus => {
                // K-Means++ initialization
                self.kmeans_plusplus_init(vectors, k)
            }
            KMeansInit::Custom(centroids) => {
                // Custom initialization
                if centroids.len() != k {
                    return Err(anyhow::anyhow!(
                        "Custom centroids count {} doesn't match k={}",
                        centroids.len(),
                        k
                    ));
                }
                Ok(centroids.clone())
            }
        }
    }

    /// K-Means++ initialization
    fn kmeans_plusplus_init(&self, vectors: &[Vec<f32>], k: usize) -> Result<Vec<Vec<f32>>> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut centroids = Vec::with_capacity(k);
        
        // Choose first centroid randomly
        let first_idx = rng.gen_range(0..vectors.len());
        centroids.push(vectors[first_idx].clone());
        
        // Choose remaining centroids
        for _ in 1..k {
            let mut distances = vec![f32::MAX; vectors.len()];
            
            // Calculate minimum distance to existing centroids
            for (idx, vector) in vectors.iter().enumerate() {
                for centroid in &centroids {
                    let similarity = self.distance_compute.calculate_distance(
                        vector,
                        centroid,
                        &self.config.distance_metric,
                    );
                    distances[idx] = distances[idx].min(similarity.raw_value);
                }
            }
            
            // Choose next centroid with probability proportional to squared distance
            let total_dist: f32 = distances.iter().map(|d| d * d).sum();
            let mut cumsum = 0.0;
            let target = rng.gen::<f32>() * total_dist;
            
            for (idx, dist) in distances.iter().enumerate() {
                cumsum += dist * dist;
                if cumsum >= target {
                    centroids.push(vectors[idx].clone());
                    break;
                }
            }
        }
        
        Ok(centroids)
    }

    /// Determine optimal number of clusters
    async fn determine_optimal_k(&self, vectors: &[Vec<f32>], max_k: usize) -> Result<usize> {
        // Simple heuristic: sqrt(n/2)
        let n = vectors.len();
        let optimal_k = ((n as f32 / 2.0).sqrt() as usize).clamp(2, max_k);
        
        tracing::debug!(
            "Determined optimal k={} for {} vectors (max_k={})",
            optimal_k,
            n,
            max_k
        );
        
        Ok(optimal_k)
    }

    /// Calculate clustering quality metrics
    fn calculate_clustering_metrics(
        &self,
        vectors: &[Vec<f32>],
        centroids: &[Vec<f32>],
        assignments: &[usize],
    ) -> Result<ClusteringMetrics> {
        // Calculate average intra-cluster distance
        let mut intra_distances = Vec::new();
        for (idx, vector) in vectors.iter().enumerate() {
            let cluster = assignments[idx];
            let similarity = self.distance_compute.calculate_distance(
                vector,
                &centroids[cluster],
                &self.config.distance_metric,
            );
            intra_distances.push(similarity.raw_value);
        }
        let avg_intra_cluster_distance = intra_distances.iter().sum::<f32>() / intra_distances.len() as f32;

        // Calculate average inter-cluster distance
        let mut inter_distances = Vec::new();
        for i in 0..centroids.len() {
            for j in (i + 1)..centroids.len() {
                let similarity = self.distance_compute.calculate_distance(
                    &centroids[i],
                    &centroids[j],
                    &self.config.distance_metric,
                );
                inter_distances.push(similarity.raw_value);
            }
        }
        let avg_inter_cluster_distance = if !inter_distances.is_empty() {
            inter_distances.iter().sum::<f32>() / inter_distances.len() as f32
        } else {
            0.0
        };

        // Simple silhouette approximation
        let silhouette_score = if avg_inter_cluster_distance > 0.0 {
            (avg_inter_cluster_distance - avg_intra_cluster_distance) 
                / avg_inter_cluster_distance.max(avg_intra_cluster_distance)
        } else {
            0.0
        };

        Ok(ClusteringMetrics {
            silhouette_score,
            davies_bouldin_index: avg_intra_cluster_distance / avg_inter_cluster_distance.max(0.001),
            calinski_harabasz_index: avg_inter_cluster_distance / avg_intra_cluster_distance.max(0.001),
            avg_intra_cluster_distance,
            avg_inter_cluster_distance,
        })
    }
}

/// Extension methods for clustering support
impl AxisClusteringEngine {
    /// Check if an index specification supports clustering
    pub fn index_supports_clustering(spec: &IndexSpecification) -> bool {
        // Clustering only makes sense for vector data types
        let is_vector_data = matches!(
            spec.data_type, 
            DataType::DenseVector { .. } | DataType::SparseVector { .. }
        );
        
        // And for algorithms that can benefit from clustering
        let supports_clustering_algo = matches!(
            spec.algorithm,
            IndexAlgorithm::HNSW { .. } |
            IndexAlgorithm::IVF { .. } |  // IVF already has clustering built-in
            IndexAlgorithm::PQ { .. } |
            IndexAlgorithm::LSH { .. }
        );
        
        is_vector_data && supports_clustering_algo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_kmeans_clustering() {
        let config = ClusteringConfig {
            algorithm: ClusteringAlgorithm::KMeans(KMeansConfig {
                k: 3,
                ..Default::default()
            }),
            min_vectors_for_clustering: 3,
            adaptive_cluster_count: false,  // Disable adaptive to use exact k=3
            ..Default::default()
        };

        let engine = AxisClusteringEngine::new(config);
        
        // Create test vectors
        let vectors = vec![
            VectorRecord {
                id: Some("1".to_string()),
                vector: vec![1.0, 0.0],
                metadata: vec![],
                timestamp: 0,
                updated_at: Some(0),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            
        },
            VectorRecord {
                id: Some("2".to_string()),
                vector: vec![0.0, 1.0],
                metadata: vec![],
                timestamp: 0,
                updated_at: Some(0),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            
        },
            VectorRecord {
                id: Some("3".to_string()),
                vector: vec![-1.0, 0.0],
                metadata: vec![],
                timestamp: 0,
                updated_at: Some(0),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            
        },
        ];

        let model = engine.train_model("test", vectors).await.unwrap();
        assert_eq!(model.centroids.len(), 3);
        assert_eq!(model.total_vectors, 3);
        
        // Test assignment
        let assignment = engine.assign_vector("test", &[0.9, 0.1]).await.unwrap();
        assert!(assignment.confidence > 0.0);
    }
}