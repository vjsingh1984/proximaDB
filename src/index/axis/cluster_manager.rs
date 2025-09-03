use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::clustering::{ClusteringAlgorithm, ClusteringConfig, KMeansConfig};
use super::types::ClusterAssignment;
use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;

pub struct ClusterManager {
    config: ClusteringConfig,
    centroids: Vec<Vec<f32>>,
    cluster_sizes: Vec<usize>,
    global_centroid: Vec<f32>,
    distance_calculator: Arc<UnifiedDistanceCompute>,
    iteration_count: usize,
}

impl ClusterManager {
    pub async fn new(config: ClusteringConfig) -> Result<Self> {
        // Use DistanceMetric directly, not UnifiedDistanceConfig
        let distance_calculator = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        Ok(Self {
            config,
            centroids: Vec::new(),
            cluster_sizes: Vec::new(),
            global_centroid: Vec::new(),
            distance_calculator,
            iteration_count: 0,
        })
    }

    pub async fn cluster_vectors(
        &mut self,
        vectors: &[Vec<f32>],
    ) -> Result<Vec<ClusterAssignment>> {
        if vectors.is_empty() {
            return Ok(Vec::new());
        }

        let dimension = vectors[0].len();

        // Determine optimal cluster count
        let k = if self.config.adaptive_cluster_count {
            Self::determine_optimal_k(vectors.len(), self.config.max_clusters)
        } else {
            match &self.config.algorithm {
                ClusteringAlgorithm::KMeans(kmeans_config) => kmeans_config.k,
                _ => 32, // Default
            }
        };

        // Initialize centroids if needed
        if self.centroids.is_empty() || self.centroids.len() != k {
            self.initialize_centroids(vectors, k)?;
        }

        // Run clustering algorithm
        let kmeans_config = match &self.config.algorithm {
            ClusteringAlgorithm::KMeans(kmeans_config) => kmeans_config.clone(),
            _ => KMeansConfig::default(),
        };

        self.run_kmeans(vectors, &kmeans_config).await?;

        // Update global centroid
        self.update_global_centroid(vectors);

        // Assign vectors to clusters
        let mut assignments = Vec::new();
        for (i, vector) in vectors.iter().enumerate() {
            let (cluster_id, distance) = self.find_nearest_centroid(vector).await?;
            assignments.push(ClusterAssignment {
                vector_id: i as u32,
                cluster_id,
                similarity: -distance, // Convert distance to similarity (negative distance)
            });
        }

        Ok(assignments)
    }

    async fn run_kmeans(&mut self, vectors: &[Vec<f32>], config: &KMeansConfig) -> Result<()> {
        let mut prev_assignments = vec![0u32; vectors.len()];

        for iteration in 0..config.max_iterations {
            // Assign vectors to nearest centroids
            let mut new_assignments = Vec::new();
            let mut cluster_vectors: Vec<Vec<Vec<f32>>> = vec![Vec::new(); self.centroids.len()];

            for vector in vectors {
                let (cluster_id, _) = self.find_nearest_centroid(vector).await?;
                new_assignments.push(cluster_id);
                cluster_vectors[cluster_id as usize].push(vector.clone());
            }

            // Check for convergence
            let changed = new_assignments
                .iter()
                .zip(prev_assignments.iter())
                .filter(|(a, b)| a != b)
                .count();

            if changed == 0 || (changed as f32 / vectors.len() as f32) < config.tolerance {
                break;
            }

            // Update centroids
            for (i, cluster) in cluster_vectors.iter().enumerate() {
                if !cluster.is_empty() {
                    self.centroids[i] = Self::compute_centroid(cluster);
                    self.cluster_sizes[i] = cluster.len();
                }
            }

            prev_assignments = new_assignments;
            self.iteration_count = iteration + 1;
        }

        Ok(())
    }

    fn initialize_centroids(&mut self, vectors: &[Vec<f32>], k: usize) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }

        let dimension = vectors[0].len();
        self.centroids.clear();
        self.cluster_sizes = vec![0; k];

        // K-means++ initialization
        // Choose first centroid randomly
        self.centroids.push(vectors[0].clone());

        // Choose remaining centroids with probability proportional to squared distance
        for _ in 1..k {
            let mut distances = Vec::new();
            for vector in vectors {
                let min_dist = self
                    .centroids
                    .iter()
                    .map(|c| Self::euclidean_distance(vector, c))
                    .min_by(|a, b| a.partial_cmp(b).unwrap());
                if let Some(dist) = min_dist {
                    distances.push(dist * dist);
                } else {
                    distances.push(0.0);
                }
            }

            // Choose next centroid with weighted probability
            let sum: f32 = distances.iter().sum();
            let mut cumulative = 0.0;
            let threshold = rand::random::<f32>() * sum;

            for (i, dist) in distances.iter().enumerate() {
                cumulative += dist;
                if cumulative >= threshold {
                    self.centroids.push(vectors[i].clone());
                    break;
                }
            }
        }

        Ok(())
    }

    async fn find_nearest_centroid(&self, vector: &[f32]) -> Result<(u32, f32)> {
        let mut min_distance = f32::MAX;
        let mut nearest_cluster = 0u32;

        for (i, centroid) in self.centroids.iter().enumerate() {
            let distance_result = self.distance_calculator.calculate_distance(
                vector,
                centroid,
                &self.config.distance_metric,
            );
            let distance = distance_result.raw_value;

            if distance < min_distance {
                min_distance = distance;
                nearest_cluster = i as u32;
            }
        }

        Ok((nearest_cluster, min_distance))
    }

    pub async fn find_nearest_clusters(&self, query: &[f32], k: usize) -> Result<Vec<u32>> {
        let mut cluster_distances = Vec::new();

        for (i, centroid) in self.centroids.iter().enumerate() {
            let distance = self.distance_calculator.calculate_distance(
                query,
                centroid,
                &self.config.distance_metric,
            );
            cluster_distances.push((i as u32, distance));
        }

        cluster_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        Ok(cluster_distances
            .into_iter()
            .take(k)
            .map(|(id, _)| id)
            .collect())
    }

    pub async fn get_global_centroid(&self) -> Result<Vec<f32>> {
        Ok(self.global_centroid.clone())
    }

    fn update_global_centroid(&mut self, vectors: &[Vec<f32>]) {
        if vectors.is_empty() {
            return;
        }

        self.global_centroid = Self::compute_centroid(vectors);
    }

    fn compute_centroid(vectors: &[Vec<f32>]) -> Vec<f32> {
        if vectors.is_empty() {
            return Vec::new();
        }

        let dimension = vectors[0].len();
        let mut centroid = vec![0.0; dimension];

        for vector in vectors {
            for (i, val) in vector.iter().enumerate() {
                centroid[i] += val;
            }
        }

        for val in &mut centroid {
            *val /= vectors.len() as f32;
        }

        centroid
    }

    fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt()
    }

    fn determine_optimal_k(n_vectors: usize, max_k: usize) -> usize {
        // Simple heuristic: sqrt(n/2) bounded by max_k
        let optimal = ((n_vectors as f32 / 2.0).sqrt() as usize).max(2);
        optimal.min(max_k)
    }
}
