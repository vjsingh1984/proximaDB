/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! IVF (Inverted File) index implementation for AXIS
//!
//! This module provides a production-ready IVF index that integrates seamlessly
//! with the AXIS adaptive indexing system. IVF is ideal for large-scale vector
//! search where vectors are clustered and search is performed within relevant clusters.

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::VectorRecord;
use crate::index::axis::index_factory::{AxisVectorIndex, IndexStats};
use crate::index::axis::types::IndexAlgorithm;
use async_trait::async_trait;

/// IVF index configuration aligned with AXIS standards
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisIvfConfig {
    /// Number of clusters (Voronoi cells)
    pub n_clusters: usize,
    /// Number of clusters to search during query
    pub n_probe: usize,
    /// Training sample size (0 for auto)
    pub train_size: usize,
    /// Maximum iterations for k-means clustering
    pub max_iterations: usize,
    /// Distance metric to use
    pub distance_metric: DistanceMetric,
    /// Enable PQ quantization for centroids
    pub enable_pq: bool,
    /// Number of subquantizers for PQ (if enabled)
    pub pq_subquantizers: usize,
}

impl Default for AxisIvfConfig {
    fn default() -> Self {
        Self {
            n_clusters: 256,
            n_probe: 16,
            train_size: 0, // Auto-calculate based on n_clusters
            max_iterations: 20,
            distance_metric: DistanceMetric::Cosine,
            enable_pq: false,
            pq_subquantizers: 8,
        }
    }
}

/// Statistics for IVF index
#[derive(Debug, Clone, Default)]
pub struct IvfStats {
    pub vector_count: usize,
    pub cluster_count: usize,
    pub trained: bool,
    pub memory_usage_bytes: usize,
    pub avg_cluster_size: f32,
    pub max_cluster_size: usize,
    pub min_cluster_size: usize,
}

/// AXIS-integrated IVF index
pub struct AxisIvfIndex {
    /// Configuration
    config: AxisIvfConfig,
    /// Cluster centroids
    centroids: Vec<Vec<f32>>,
    /// Inverted lists mapping cluster ID to vector IDs
    inverted_lists: Arc<DashMap<usize, Vec<String>>>,
    /// Vector data storage
    vectors: Arc<DashMap<String, Arc<VectorRecord>>>,
    /// Dimension of vectors
    dimension: usize,
    /// Distance compute instance
    distance_compute: UnifiedDistanceCompute,
    /// Statistics
    vector_count: Arc<AtomicUsize>,
    /// Whether the index has been trained
    trained: bool,
    /// The algorithm specification
    algorithm: IndexAlgorithm,
}

impl AxisIvfIndex {
    /// Create a new IVF index
    pub fn new(config: AxisIvfConfig, dimension: usize) -> Self {
        info!(
            "Creating AXIS IVF index: {} clusters, {} probe, {} dim",
            config.n_clusters, config.n_probe, dimension
        );
        
        let distance_compute = UnifiedDistanceCompute::new(config.distance_metric);
        let algorithm = IndexAlgorithm::IVF {
            nlist: config.n_clusters as u32,
            nprobe: config.n_probe as u32,
            quantizer: if config.enable_pq { 
                Some(Box::new(IndexAlgorithm::PQ {
                    m: config.pq_subquantizers as u32,
                    nbits: 8,
                    train_size: 10000,
                }))
            } else { 
                None 
            },
        };
        
        Self {
            config,
            centroids: Vec::new(),
            inverted_lists: Arc::new(DashMap::new()),
            vectors: Arc::new(DashMap::new()),
            dimension,
            distance_compute,
            vector_count: Arc::new(AtomicUsize::new(0)),
            trained: false,
            algorithm,
        }
    }
    
    /// Train the index on a set of vectors
    pub async fn train(&mut self, training_vectors: &[Vec<f32>]) -> Result<()> {
        if training_vectors.is_empty() {
            return Err(anyhow!("No training vectors provided"));
        }
        
        let actual_train_size = if self.config.train_size > 0 {
            self.config.train_size.min(training_vectors.len())
        } else {
            // Auto-calculate: 100 samples per cluster or all vectors
            (self.config.n_clusters * 100).min(training_vectors.len())
        };
        
        info!(
            "Training IVF index with {} vectors (out of {} available)",
            actual_train_size,
            training_vectors.len()
        );
        
        // Sample training vectors if needed
        let training_sample = if actual_train_size < training_vectors.len() {
            Self::sample_vectors(training_vectors, actual_train_size)
        } else {
            training_vectors.to_vec()
        };
        
        // Initialize centroids using k-means++
        self.centroids = self.kmeans_plusplus_init(&training_sample)?;
        
        // Run k-means clustering
        let mut iteration = 0;
        let mut prev_inertia = f32::MAX;
        
        while iteration < self.config.max_iterations {
            let (new_centroids, inertia) = self.kmeans_iteration(&training_sample)?;
            
            // Check convergence
            let inertia_change = (prev_inertia - inertia).abs() / prev_inertia;
            if inertia_change < 1e-4 {
                info!("K-means converged after {} iterations", iteration + 1);
                break;
            }
            
            self.centroids = new_centroids;
            prev_inertia = inertia;
            iteration += 1;
        }
        
        self.trained = true;
        info!("IVF index training completed");
        
        Ok(())
    }
    
    /// Add a vector to the index
    pub async fn add(&self, id: String, vector_record: Arc<VectorRecord>) -> Result<()> {
        if !self.trained {
            return Err(anyhow!("Index must be trained before adding vectors"));
        }
        
        if vector_record.vector.len() != self.dimension {
            return Err(anyhow!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimension,
                vector_record.vector.len()
            ));
        }
        
        // Find nearest centroid
        let cluster_id = self.find_nearest_centroid(&vector_record.vector)?;
        
        // Add to inverted list
        self.inverted_lists
            .entry(cluster_id)
            .or_insert_with(Vec::new)
            .push(id.clone());
        
        // Store vector
        self.vectors.insert(id, vector_record);
        self.vector_count.fetch_add(1, Ordering::Relaxed);
        
        Ok(())
    }
    
    /// Search for k nearest neighbors
    pub async fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        if !self.trained {
            return Err(anyhow!("Index must be trained before searching"));
        }
        
        if query.len() != self.dimension {
            return Err(anyhow!(
                "Query dimension mismatch: expected {}, got {}",
                self.dimension,
                query.len()
            ));
        }
        
        // Find n_probe nearest centroids
        let probe_clusters = self.find_probe_clusters(query)?;
        
        // Search within selected clusters
        let mut candidates = Vec::new();
        for cluster_id in probe_clusters {
            if let Some(vector_ids) = self.inverted_lists.get(&cluster_id) {
                for vector_id in vector_ids.iter() {
                    if let Some(vector_record) = self.vectors.get(vector_id) {
                        // Apply filter if provided
                        if let Some(f) = filter {
                            if !f(&vector_record) {
                                continue;
                            }
                        }
                        
                        let dist = self.compute_distance(query, &vector_record.vector);
                        candidates.push((vector_id.clone(), dist));
                    }
                }
            }
        }
        
        // Sort by distance and return top-k
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        candidates.truncate(k);
        
        debug!(
            "IVF search: examined {} candidates, returned {} results",
            candidates.len(),
            candidates.len().min(k)
        );
        
        Ok(candidates)
    }
    
    /// Remove a vector from the index
    pub async fn remove(&self, id: &str) -> Result<()> {
        if let Some((_, vector_record)) = self.vectors.remove(id) {
            // Find which cluster it belonged to
            let cluster_id = self.find_nearest_centroid(&vector_record.vector)?;
            
            // Remove from inverted list
            if let Some(mut list) = self.inverted_lists.get_mut(&cluster_id) {
                list.retain(|vid| vid != id);
            }
            
            self.vector_count.fetch_sub(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(anyhow!("Vector {} not found in index", id))
        }
    }
    
    /// Get index statistics
    pub fn stats(&self) -> IvfStats {
        let vector_count = self.vector_count.load(Ordering::Relaxed);
        
        let mut cluster_sizes = vec![0usize; self.config.n_clusters];
        for entry in self.inverted_lists.iter() {
            cluster_sizes[*entry.key()] = entry.value().len();
        }
        
        let max_cluster_size = cluster_sizes.iter().max().copied().unwrap_or(0);
        let min_cluster_size = cluster_sizes.iter().min().copied().unwrap_or(0);
        let avg_cluster_size = if self.config.n_clusters > 0 {
            vector_count as f32 / self.config.n_clusters as f32
        } else {
            0.0
        };
        
        // Estimate memory usage
        let centroid_memory = self.centroids.len() * self.dimension * 4; // f32
        let vector_memory = vector_count * self.dimension * 4;
        let index_overhead = vector_count * 64; // Rough estimate for hashmaps
        
        IvfStats {
            vector_count,
            cluster_count: self.config.n_clusters,
            trained: self.trained,
            memory_usage_bytes: centroid_memory + vector_memory + index_overhead,
            avg_cluster_size,
            max_cluster_size,
            min_cluster_size,
        }
    }
    
    // Private helper methods
    
    fn compute_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        let result = self.distance_compute.calculate_distance(a, b, &self.config.distance_metric);
        result.raw_value
    }
    
    fn find_nearest_centroid(&self, vector: &[f32]) -> Result<usize> {
        if self.centroids.is_empty() {
            return Err(anyhow!("No centroids available"));
        }
        
        let mut min_dist = f32::MAX;
        let mut nearest_id = 0;
        
        for (i, centroid) in self.centroids.iter().enumerate() {
            let dist = self.compute_distance(vector, centroid);
            if dist < min_dist {
                min_dist = dist;
                nearest_id = i;
            }
        }
        
        Ok(nearest_id)
    }
    
    fn find_probe_clusters(&self, query: &[f32]) -> Result<Vec<usize>> {
        let mut centroid_distances: Vec<(usize, f32)> = self.centroids
            .iter()
            .enumerate()
            .map(|(i, centroid)| {
                let dist = self.compute_distance(query, centroid);
                (i, dist)
            })
            .collect();
        
        centroid_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        
        let n_probe = self.config.n_probe.min(centroid_distances.len());
        Ok(centroid_distances[..n_probe].iter().map(|(i, _)| *i).collect())
    }
    
    fn kmeans_plusplus_init(&self, vectors: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut centroids = Vec::new();
        
        // Choose first centroid randomly
        let first_idx = rng.gen_range(0..vectors.len());
        centroids.push(vectors[first_idx].clone());
        
        // Choose remaining centroids
        for _ in 1..self.config.n_clusters.min(vectors.len()) {
            let mut distances = vec![f32::MAX; vectors.len()];
            
            // Compute distance to nearest centroid for each vector
            for (i, vector) in vectors.iter().enumerate() {
                for centroid in &centroids {
                    let dist = self.compute_distance(vector, centroid);
                    distances[i] = distances[i].min(dist);
                }
            }
            
            // Choose next centroid with probability proportional to squared distance
            let total_dist: f32 = distances.iter().map(|d| d * d).sum();
            if total_dist == 0.0 {
                warn!("All remaining vectors are identical to existing centroids");
                break;
            }
            
            let mut cumsum = 0.0;
            let target = rng.gen::<f32>() * total_dist;
            
            for (i, &dist) in distances.iter().enumerate() {
                cumsum += dist * dist;
                if cumsum >= target {
                    centroids.push(vectors[i].clone());
                    break;
                }
            }
        }
        
        // If we couldn't initialize enough centroids, use random vectors
        while centroids.len() < self.config.n_clusters && centroids.len() < vectors.len() {
            let idx = rng.gen_range(0..vectors.len());
            if !centroids.iter().any(|c| c == &vectors[idx]) {
                centroids.push(vectors[idx].clone());
            }
        }
        
        Ok(centroids)
    }
    
    fn kmeans_iteration(&self, vectors: &[Vec<f32>]) -> Result<(Vec<Vec<f32>>, f32)> {
        let mut new_centroids = vec![vec![0.0; self.dimension]; self.config.n_clusters];
        let mut cluster_counts = vec![0usize; self.config.n_clusters];
        let mut inertia = 0.0;
        
        // Assign vectors to nearest centroids
        for vector in vectors {
            let cluster_id = self.find_nearest_centroid(vector)?;
            let dist = self.compute_distance(vector, &self.centroids[cluster_id]);
            inertia += dist * dist;
            
            for (i, &val) in vector.iter().enumerate() {
                new_centroids[cluster_id][i] += val;
            }
            cluster_counts[cluster_id] += 1;
        }
        
        // Update centroids
        for (i, count) in cluster_counts.iter().enumerate() {
            if *count > 0 {
                for j in 0..self.dimension {
                    new_centroids[i][j] /= *count as f32;
                }
            } else {
                // Empty cluster - reinitialize with random vector
                warn!("Empty cluster {} detected, reinitializing", i);
                let random_idx = rand::random::<usize>() % vectors.len();
                new_centroids[i] = vectors[random_idx].clone();
            }
        }
        
        Ok((new_centroids, inertia))
    }
    
    fn sample_vectors(vectors: &[Vec<f32>], sample_size: usize) -> Vec<Vec<f32>> {
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();
        
        let mut indices: Vec<usize> = (0..vectors.len()).collect();
        indices.shuffle(&mut rng);
        indices.truncate(sample_size);
        
        indices.into_iter().map(|i| vectors[i].clone()).collect()
    }
}

// Implement AxisVectorIndex trait for deep AXIS integration
#[async_trait]
impl AxisVectorIndex for AxisIvfIndex {
    async fn add(&self, id: String, vector: Arc<VectorRecord>) -> Result<()> {
        AxisIvfIndex::add(self, id, vector).await
    }
    
    async fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        AxisIvfIndex::search(self, query, k, filter).await
    }
    
    async fn remove(&self, id: &str) -> Result<()> {
        AxisIvfIndex::remove(self, id).await
    }
    
    fn algorithm(&self) -> &IndexAlgorithm {
        &self.algorithm
    }
    
    fn stats(&self) -> IndexStats {
        let ivf_stats = self.stats();
        IndexStats {
            vector_count: ivf_stats.vector_count,
            memory_usage_bytes: ivf_stats.memory_usage_bytes,
            index_type: "IVF".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_ivf_basic_operations() {
        let config = AxisIvfConfig {
            n_clusters: 4,
            n_probe: 2,
            ..Default::default()
        };
        
        let mut index = AxisIvfIndex::new(config, 4);
        
        // Create training vectors
        let training_vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0, 1.0],
            vec![0.5, 0.5, 0.0, 0.0],
            vec![0.0, 0.5, 0.5, 0.0],
            vec![0.0, 0.0, 0.5, 0.5],
            vec![0.5, 0.0, 0.0, 0.5],
        ];
        
        // Train the index
        index.train(&training_vectors).await.unwrap();
        assert!(index.trained);
        
        // Add vectors
        for (i, vector) in training_vectors.iter().enumerate() {
            let record = VectorRecord {
                id: Some(format!("vec_{}", i)),
                vector: vector.clone(),
                metadata: vec![],
                timestamp: 0,
                updated_at: Some(0),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            };
            index.add(format!("vec_{}", i), Arc::new(record)).await.unwrap();
        }
        
        // Search
        let query = vec![0.9, 0.1, 0.0, 0.0];
        let results = index.search(&query, 3, None).await.unwrap();
        
        // IVF is approximate, might not return all vectors
        assert!(results.len() >= 2 && results.len() <= 3, "Expected 2-3 results, got {}", results.len());
        assert_eq!(results[0].0, "vec_0");
    }
}