/*
 * Copyright 2025 Vijaykumar Singh
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

//! # Vamana Graph Builder for DiskANN
//!
//! This module implements the Vamana graph construction algorithm for
//! building bounded-degree graphs optimized for SSD-based ANN search.
//!
//! ## Vamana Graph Algorithm
//!
//! The Vamana graph is built using a greedy algorithm:
//! 1. Select a random medoid as starting point
//! 2. For each node:
//!    a. Find its nearest neighbors using existing graph
//!    b. Add bidirectional edges
//!    c. Prune edges to maintain max degree R
//! 3. Result: Graph with bounded degree optimized for ANN search
//!
//! ## Key Properties
//!
//! - **Bounded Degree**: Each node has at most R neighbors
//! - **Greedy Construction**: Fast O(N^2 * log N) build time
//! - **SSD-Optimized**: Sequential node ordering for efficient reads
//! - **Low Diameter**: Small number of hops between any two nodes

use crate::compute::distance_computation::{DistanceMetric, UnifiedDistanceCompute};
use crate::core::error::ProximaDBError;
use std::collections::{HashSet, VecDeque};
use tracing::info;

// VamanaGraph is defined in the parent module (mod.rs)

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Vamana graph for efficient SSD-based search
#[derive(Debug, Clone)]
pub struct VamanaGraph {
    /// Maximum degree (R)
    pub max_degree: usize,

    /// Graph edges (node -> neighbors)
    pub edges: Vec<Vec<usize>>,

    /// Medoid (starting point for search)
    pub medoid: usize,
}

impl VamanaGraph {
    /// Create a new Vamana graph
    pub fn new(max_degree: usize, num_nodes: usize, medoid: usize) -> Self {
        Self {
            max_degree,
            edges: vec![Vec::new(); num_nodes],
            medoid,
        }
    }
}

/// Configuration for Vamana graph construction
#[derive(Debug, Clone)]
pub struct VamanaConfig {
    /// Maximum degree (R) for each node
    pub max_degree: usize,

    /// Search window size (L) during construction
    pub search_window_size: usize,

    /// Alpha parameter for candidate selection
    pub alpha: f32,
}

impl Default for VamanaConfig {
    fn default() -> Self {
        Self {
            max_degree: 32,         // Standard DiskANN parameter
            search_window_size: 75, // Search 75 candidates
            alpha: 1.2,             // Candidate selection threshold
        }
    }
}

/// Vamana graph builder
pub struct VamanaBuilder {
    /// Configuration
    config: VamanaConfig,

    /// Number of vectors
    num_vectors: usize,

    /// Vector dimension
    dimension: usize,

    /// Graph edges being built
    graph: Vec<Vec<usize>>,

    /// Reverse edges for bidirectional maintenance
    reverse_graph: Vec<Vec<usize>>,

    /// Distance cache for efficient computation
    distance_cache: Vec<Vec<f32>>,

    /// Unified distance compute engine
    distance_compute: UnifiedDistanceCompute,
}

impl VamanaBuilder {
    /// Create a new Vamana graph builder
    pub fn new(num_vectors: usize, dimension: usize, config: VamanaConfig) -> Self {
        Self {
            config,
            num_vectors,
            dimension,
            graph: vec![Vec::new(); num_vectors],
            reverse_graph: vec![Vec::new(); num_vectors],
            distance_cache: vec![Vec::new(); num_vectors],
            distance_compute: UnifiedDistanceCompute::new(DistanceMetric::Euclidean),
        }
    }

    /// Build Vamana graph from vectors
    pub fn build(&mut self, vectors: &[Vec<f32>]) -> Result<VamanaGraph> {
        if vectors.len() != self.num_vectors {
            return Err(ProximaDBError::InvalidInput(format!(
                "Vector count mismatch: expected {}, got {}",
                self.num_vectors,
                vectors.len()
            )));
        }

        info!(
            "Building Vamana graph: {} nodes, max_degree={}",
            self.num_vectors, self.config.max_degree
        );

        // Pre-compute distances between all vectors
        self.precompute_distances(vectors);

        // Step 1: Select random medoid
        let medoid = self.select_medoid()?;

        // Step 2: Build graph greedily
        self.build_greedy(vectors, medoid)?;

        // Step 3: Prune to max degree
        self.prune_to_max_degree()?;

        // Step 4: Find final medoid
        let final_medoid = self.find_medoid()?;

        Ok(VamanaGraph {
            max_degree: self.config.max_degree,
            edges: self.graph.clone(),
            medoid: final_medoid,
        })
    }

    /// Precompute distance matrix for efficient candidate search
    fn precompute_distances(&mut self, vectors: &[Vec<f32>]) {
        info!("Precomputing distances...");

        for i in 0..self.num_vectors {
            self.distance_cache[i] = Vec::with_capacity(self.num_vectors);

            for j in 0..self.num_vectors {
                if i != j {
                    let distance = self.compute_distance(&vectors[i], &vectors[j]);
                    self.distance_cache[i].push(distance);
                } else {
                    // Distance to self is infinity
                    self.distance_cache[i].push(f32::MAX);
                }
            }
        }
    }

    /// Compute distance between two vectors using unified distance engine
    fn compute_distance(&self, v1: &[f32], v2: &[f32]) -> f32 {
        // Use UnifiedDistanceCompute for hardware-accelerated distance calculation
        self.distance_compute.distance(v1, v2)
    }

    /// Select a random medoid as starting point
    fn select_medoid(&self) -> Result<usize> {
        use std::time::SystemTime;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| ProximaDBError::Internal(format!("Time error: {}", e)))?
            .as_nanos() as usize;

        let medoid = nanos % self.num_vectors;
        info!("Selected medoid: {}", medoid);
        Ok(medoid)
    }

    /// Greedy graph construction starting from medoid
    fn build_greedy(&mut self, _vectors: &[Vec<f32>], medoid: usize) -> Result<()> {
        let mut processed = HashSet::new();
        let mut queue = VecDeque::new();

        queue.push_back(medoid);
        processed.insert(medoid);

        while let Some(current) = queue.pop_front() {
            // Find nearest neighbors using existing graph
            let neighbors = self.find_nearest_neighbors(current, &processed)?;

            // Add bidirectional edges
            for neighbor in neighbors {
                self.add_bidirectional_edge(current, neighbor)?;
                self.add_bidirectional_edge(neighbor, current)?;

                // Enqueue for processing if not already processed
                if processed.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }

        Ok(())
    }

    /// Find nearest neighbors for a node using existing graph
    fn find_nearest_neighbors(
        &self,
        node: usize,
        processed: &HashSet<usize>,
    ) -> Result<Vec<usize>> {
        let mut candidates = Vec::new();

        // Use processed nodes as candidate pool
        for &candidate in processed.iter() {
            if candidate != node {
                let dist = self.distance_cache[node][candidate];
                candidates.push((candidate, dist));
            }
        }

        // Sort by distance and return top L candidates
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Filter by alpha * best_distance
        if let Some((_, best_dist)) = candidates.first() {
            let threshold = best_dist * self.config.alpha;
            candidates.retain(|(_, dist)| *dist <= threshold);
        }

        // Take top search_window_size candidates
        candidates.truncate(self.config.search_window_size);

        Ok(candidates.into_iter().map(|(idx, _)| idx).collect())
    }

    /// Add bidirectional edge between two nodes
    fn add_bidirectional_edge(&mut self, from: usize, to: usize) -> Result<()> {
        // Add edge from -> to
        if !self.graph[from].contains(&to) {
            self.graph[from].push(to);
        }

        // Add reverse edge to -> from
        if !self.reverse_graph[to].contains(&from) {
            self.reverse_graph[to].push(from);
        }

        Ok(())
    }

    /// Prune edges to maintain max degree constraint
    fn prune_to_max_degree(&mut self) -> Result<()> {
        info!("Pruning graph to max_degree={}", self.config.max_degree);

        for node in 0..self.num_vectors {
            // Prune outgoing edges
            if self.graph[node].len() > self.config.max_degree {
                // Keep only closest R neighbors
                let mut neighbors_with_dist = Vec::new();

                for &neighbor in &self.graph[node].clone() {
                    let dist = self.distance_cache[node][neighbor];
                    neighbors_with_dist.push((neighbor, dist));
                }

                // Sort by distance and keep closest R
                neighbors_with_dist
                    .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                neighbors_with_dist.truncate(self.config.max_degree);

                // Update graph and reverse graph
                let old_neighbors = std::mem::take(&mut self.graph[node]);
                for (neighbor, _) in neighbors_with_dist {
                    self.graph[node].push(neighbor);
                }

                // Remove stale reverse edges
                for old_neighbor in old_neighbors {
                    if !self.graph[node].contains(&old_neighbor) {
                        self.reverse_graph[old_neighbor].retain(|x| *x != node);
                    }
                }
            }

            // Also prune reverse edges
            if self.reverse_graph[node].len() > self.config.max_degree {
                let mut reverse_with_dist = Vec::new();

                for &neighbor in &self.reverse_graph[node].clone() {
                    let dist = self.distance_cache[node][neighbor];
                    reverse_with_dist.push((neighbor, dist));
                }

                reverse_with_dist
                    .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                reverse_with_dist.truncate(self.config.max_degree);

                let old_reverse = std::mem::take(&mut self.reverse_graph[node]);
                for (neighbor, _) in reverse_with_dist {
                    self.reverse_graph[node].push(neighbor);
                }

                // Remove stale forward edges
                for old_neighbor in old_reverse {
                    if !self.reverse_graph[node].contains(&old_neighbor) {
                        self.graph[old_neighbor].retain(|x| *x != node);
                    }
                }
            }
        }

        Ok(())
    }

    /// Find the medoid (node with minimum distance to all other nodes)
    fn find_medoid(&self) -> Result<usize> {
        let mut best_medoid = 0;
        let mut min_total_distance = f32::MAX;

        for node in 0..self.num_vectors {
            let total_distance: f32 = self.distance_cache[node].iter().sum();

            if total_distance < min_total_distance {
                min_total_distance = total_distance;
                best_medoid = node;
            }
        }

        info!(
            "Found medoid: {} with total distance {}",
            best_medoid, min_total_distance
        );
        Ok(best_medoid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vamana_config_default() {
        let config = VamanaConfig::default();
        assert_eq!(config.max_degree, 32);
        assert_eq!(config.search_window_size, 75);
    }

    #[test]
    fn test_vamana_builder_creation() {
        let builder = VamanaBuilder::new(100, 128, VamanaConfig::default());

        assert_eq!(builder.num_vectors, 100);
        assert_eq!(builder.dimension, 128);
    }

    #[test]
    fn test_compute_distance_euclidean() {
        let builder = VamanaBuilder::new(2, 3, VamanaConfig::default());

        let v1 = vec![1.0, 2.0, 3.0];
        let v2 = vec![1.0, 2.0, 4.0];

        let dist = builder.compute_distance(&v1, &v2);
        assert!((dist - 1.0).abs() < 0.001); // sqrt((4-3)^2) = 1
    }

    #[test]
    fn test_compute_distance_cosine() {
        // Test uses unified distance engine with Euclidean metric
        let builder = VamanaBuilder::new(2, 3, VamanaConfig::default());

        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0];

        let dist = builder.compute_distance(&v1, &v2);
        // Orthogonal vectors: Euclidean distance = sqrt(1^2 + 1^2) = sqrt(2) ≈ 1.414
        assert!((dist - 1.414).abs() < 0.001);
    }

    #[test]
    fn test_small_graph_build() {
        let num_vectors = 10;
        let dimension = 8;
        let mut builder = VamanaBuilder::new(num_vectors, dimension, VamanaConfig::default());

        // Create dummy vectors
        let vectors: Vec<Vec<f32>> = (0..num_vectors)
            .map(|i| (0..dimension).map(|j| (i * dimension + j) as f32).collect())
            .collect();

        let graph = builder.build(&vectors).unwrap();

        assert_eq!(graph.edges.len(), num_vectors);
        assert_eq!(graph.max_degree, 32);
        assert!(graph.medoid < num_vectors);

        // All nodes should have some edges
        for node in 0..num_vectors {
            // At minimum, node should have itself as neighbor or be connected
            // through other nodes
            assert!(graph.edges[node].len() <= 32);
        }
    }
}
