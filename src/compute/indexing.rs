/*
 * Copyright 2024 Vijaykumar Singh
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

//! Advanced indexing algorithms for ProximaDB
//!
//! This module provides implementations of various vector indexing algorithms:
//! - IVF (Inverted File Index) for large-scale search
//! - LSH (Locality Sensitive Hashing) for binary vectors
//! - Disk-based indexing for massive datasets

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use dashmap::DashMap;

use crate::compute::distance::DistanceMetric;
use crate::compute::unified_distance::UnifiedDistanceCompute;
use crate::core::VectorRecord;

/// IVF (Inverted File) index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IvfConfig {
    /// Number of clusters (Voronoi cells)
    pub n_clusters: usize,
    /// Number of clusters to search during query
    pub n_probe: usize,
    /// Training sample size
    pub train_size: usize,
    /// Maximum iterations for k-means clustering
    pub max_iterations: usize,
    /// Distance metric to use
    pub distance_metric: DistanceMetric,
}

impl Default for IvfConfig {
    fn default() -> Self {
        Self {
            n_clusters: 256,
            n_probe: 16,
            train_size: 10000,
            max_iterations: 100,
            distance_metric: DistanceMetric::Euclidean,
        }
    }
}

/// IVF index structure
pub struct IvfIndex {
    /// Configuration
    config: IvfConfig,
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
}

impl IvfIndex {
    /// Create a new IVF index
    pub fn new(config: IvfConfig, dimension: usize) -> Self {
        let distance_compute = UnifiedDistanceCompute::new(config.distance_metric);
        Self {
            config,
            centroids: Vec::new(),
            inverted_lists: Arc::new(DashMap::new()),
            vectors: Arc::new(DashMap::new()),
            dimension,
            distance_compute,
        }
    }
    
    /// Compute distance between two vectors using configured metric
    fn compute_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        let result = self.distance_compute.calculate_distance(a, b, &self.config.distance_metric);
        result.raw_value
    }
    
    /// Train the index on a set of vectors
    pub fn train(&mut self, training_vectors: &[Vec<f32>]) -> Result<()> {
        if training_vectors.is_empty() {
            return Err(anyhow::anyhow!("No training vectors provided"));
        }
        
        // Initialize centroids using k-means++
        self.centroids = self.kmeans_plusplus_init(training_vectors)?;
        
        // Run k-means clustering
        for iteration in 0..self.config.max_iterations {
            let mut new_centroids = vec![vec![0.0; self.dimension]; self.config.n_clusters];
            let mut cluster_counts = vec![0usize; self.config.n_clusters];
            
            // Assign vectors to nearest centroids
            for vector in training_vectors {
                let cluster_id = self.find_nearest_centroid(vector)?;
                for (i, &val) in vector.iter().enumerate() {
                    new_centroids[cluster_id][i] += val;
                }
                cluster_counts[cluster_id] += 1;
            }
            
            // Update centroids
            let mut changed = false;
            for (i, count) in cluster_counts.iter().enumerate() {
                if *count > 0 {
                    for j in 0..self.dimension {
                        new_centroids[i][j] /= *count as f32;
                        if (new_centroids[i][j] - self.centroids[i][j]).abs() > 1e-6 {
                            changed = true;
                        }
                    }
                }
            }
            
            self.centroids = new_centroids;
            
            // Early stopping if centroids haven't changed
            if !changed {
                break;
            }
        }
        
        Ok(())
    }
    
    /// Add a vector to the index
    pub fn add(&self, id: String, vector_record: Arc<VectorRecord>) -> Result<()> {
        if vector_record.vector.len() != self.dimension {
            return Err(anyhow::anyhow!(
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
        
        Ok(())
    }
    
    /// Search for k nearest neighbors
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        if query.len() != self.dimension {
            return Err(anyhow::anyhow!(
                "Query dimension mismatch: expected {}, got {}",
                self.dimension,
                query.len()
            ));
        }
        
        // Find n_probe nearest centroids
        let mut centroid_distances: Vec<(usize, f32)> = self.centroids
            .iter()
            .enumerate()
            .map(|(i, centroid)| {
                let dist = self.compute_distance(query, centroid);
                (i, dist)
            })
            .collect();
        
        centroid_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        centroid_distances.truncate(self.config.n_probe);
        
        // Search within selected clusters
        let mut candidates = Vec::new();
        for (cluster_id, _) in centroid_distances {
            if let Some(vector_ids) = self.inverted_lists.get(&cluster_id) {
                for vector_id in vector_ids.iter() {
                    if let Some(vector_record) = self.vectors.get(vector_id) {
                        let dist = self.compute_distance(
                            query,
                            &vector_record.vector
                        );
                        candidates.push((vector_id.clone(), dist));
                    }
                }
            }
        }
        
        // Sort by distance and return top-k
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        candidates.truncate(k);
        
        Ok(candidates)
    }
    
    /// Find the nearest centroid for a vector
    fn find_nearest_centroid(&self, vector: &[f32]) -> Result<usize> {
        if self.centroids.is_empty() {
            return Err(anyhow::anyhow!("Index not trained"));
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
    
    /// Initialize centroids using k-means++ algorithm
    fn kmeans_plusplus_init(&self, vectors: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut centroids = Vec::new();
        
        // Choose first centroid randomly
        let first_idx = rng.gen_range(0..vectors.len());
        centroids.push(vectors[first_idx].clone());
        
        // Choose remaining centroids
        for _ in 1..self.config.n_clusters {
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
        
        Ok(centroids)
    }
}

/// LSH (Locality Sensitive Hashing) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LshConfig {
    /// Number of hash tables
    pub n_tables: usize,
    /// Number of hash functions per table
    pub n_hashes: usize,
    /// Random seed for reproducibility
    pub seed: u64,
}

impl Default for LshConfig {
    fn default() -> Self {
        Self {
            n_tables: 20,
            n_hashes: 10,
            seed: 42,
        }
    }
}

/// LSH index for binary vectors
pub struct LshIndex {
    /// Configuration
    config: LshConfig,
    /// Hash tables mapping hash values to vector IDs
    hash_tables: Vec<DashMap<u64, Vec<String>>>,
    /// Random projection matrices for each table
    projections: Vec<Vec<Vec<f32>>>,
    /// Vector data storage
    vectors: Arc<DashMap<String, Arc<VectorRecord>>>,
    /// Dimension
    dimension: usize,
    /// Distance compute instance (LSH typically uses Cosine)
    distance_compute: UnifiedDistanceCompute,
}

impl LshIndex {
    /// Create a new LSH index
    pub fn new(config: LshConfig, dimension: usize) -> Self {
        use rand::{Rng, SeedableRng};
        use rand::rngs::StdRng;
        
        let mut rng = StdRng::seed_from_u64(config.seed);
        let mut projections = Vec::new();
        
        // Generate random projections for each hash table
        let n_tables = config.n_tables;
        let n_hashes = config.n_hashes;
        for _ in 0..n_tables {
            let mut table_projections = Vec::new();
            for _ in 0..n_hashes {
                let projection: Vec<f32> = (0..dimension)
                    .map(|_| if rng.gen::<f32>() > 0.5 { 1.0 } else { -1.0 })
                    .collect();
                table_projections.push(projection);
            }
            projections.push(table_projections);
        }
        
        let distance_compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
        Self {
            config,
            hash_tables: (0..n_tables).map(|_| DashMap::new()).collect(),
            projections,
            vectors: Arc::new(DashMap::new()),
            dimension,
            distance_compute,
        }
    }
    
    /// Add a vector to the LSH index
    pub fn add(&self, id: String, vector_record: Arc<VectorRecord>) -> Result<()> {
        if vector_record.vector.len() != self.dimension {
            return Err(anyhow::anyhow!("Dimension mismatch"));
        }
        
        // Compute hash for each table
        for (table_idx, table) in self.hash_tables.iter().enumerate() {
            let hash = self.compute_hash(&vector_record.vector, table_idx);
            table.entry(hash).or_insert_with(Vec::new).push(id.clone());
        }
        
        self.vectors.insert(id, vector_record);
        Ok(())
    }
    
    /// Search for approximate nearest neighbors
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        let mut candidates = HashMap::new();
        
        // Find candidates from all hash tables
        for (table_idx, table) in self.hash_tables.iter().enumerate() {
            let hash = self.compute_hash(query, table_idx);
            
            if let Some(vector_ids) = table.get(&hash) {
                for vector_id in vector_ids.iter() {
                    candidates.insert(vector_id.clone(), ());
                }
            }
        }
        
        // Compute actual distances for candidates
        let mut results = Vec::new();
        for (vector_id, _) in candidates {
            if let Some(vector_record) = self.vectors.get(&vector_id) {
                let result = self.distance_compute.calculate_distance(
                    query,
                    &vector_record.vector,
                    &DistanceMetric::Cosine
                );
                let dist = result.raw_value;
                results.push((vector_id, dist));
            }
        }
        
        // Sort by distance and return top-k
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        results.truncate(k);
        
        Ok(results)
    }
    
    /// Compute hash value for a vector in a specific table
    fn compute_hash(&self, vector: &[f32], table_idx: usize) -> u64 {
        let mut hash = 0u64;
        
        for (i, projection) in self.projections[table_idx].iter().enumerate() {
            let dot_product: f32 = vector.iter()
                .zip(projection.iter())
                .map(|(a, b)| a * b)
                .sum();
            
            if dot_product > 0.0 {
                hash |= 1u64 << (i % 64);
            }
        }
        
        hash
    }
}

#[cfg(test)]
#[path = "indexing_tests.rs"]
mod tests;
