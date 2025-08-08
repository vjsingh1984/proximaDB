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

//! AXIS-native Annoy (Approximate Nearest Neighbors Oh Yeah) implementation
//!
//! This module provides a pure Rust implementation of the Annoy algorithm,
//! originally created by Spotify. It builds a forest of random projection trees
//! for fast approximate nearest neighbor search.
//!
//! Key features:
//! - Multiple random projection trees for improved recall
//! - Memory-efficient binary tree structure
//! - Support for angular (cosine) and Euclidean distances
//! - Memory-mapped file support for large datasets
//! - Static index (built once, no dynamic updates)

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::VectorRecord;
use crate::index::axis::index_factory::{AxisVectorIndex, IndexStats};
use crate::index::axis::types::IndexAlgorithm;
use crate::index::axis::utils::{IndexVectorStore, AtomicStats, validation, memory};

/// Configuration for AXIS Annoy index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisAnnoyConfig {
    /// Number of trees to build
    pub n_trees: usize,
    /// Number of nodes to inspect during search (-1 = n_trees * n * 1.5)
    pub search_k: i32,
    /// Maximum number of descendants in a leaf node
    pub max_leaf_size: usize,
    /// Random seed for reproducibility
    pub seed: u64,
    /// Distance metric
    pub distance_metric: DistanceMetric,
}

impl Default for AxisAnnoyConfig {
    fn default() -> Self {
        Self {
            n_trees: 10,
            search_k: -1,
            max_leaf_size: 100,
            seed: 42,
            distance_metric: DistanceMetric::Cosine,
        }
    }
}

/// Node in an Annoy tree
#[derive(Debug, Clone, Serialize, Deserialize)]
enum AnnoyNode {
    /// Leaf node containing vector IDs
    Leaf {
        vector_ids: Vec<String>,
    },
    /// Split node with hyperplane
    Split {
        /// Normal vector to the splitting hyperplane
        hyperplane: Vec<f32>,
        /// Offset for the hyperplane
        offset: f32,
        /// Left child index
        left: usize,
        /// Right child index
        right: usize,
    },
}

/// A single Annoy tree
#[derive(Debug, Serialize, Deserialize)]
struct AnnoyTree {
    /// Tree nodes stored in a vector
    nodes: Vec<AnnoyNode>,
    /// Root node index
    root: usize,
    /// Vector dimension
    dimension: usize,
}

impl AnnoyTree {
    fn new(dimension: usize) -> Self {
        Self {
            nodes: Vec::new(),
            root: 0,
            dimension,
        }
    }

    /// Estimate memory usage of this tree
    fn estimate_memory(&self) -> usize {
        let node_memory: usize = self.nodes.iter().map(|node| {
            match node {
                AnnoyNode::Leaf { vector_ids } => {
                    std::mem::size_of::<AnnoyNode>() + vector_ids.len() * std::mem::size_of::<String>()
                }
                AnnoyNode::Split { hyperplane, .. } => {
                    std::mem::size_of::<AnnoyNode>() + hyperplane.len() * std::mem::size_of::<f32>()
                }
            }
        }).sum();
        
        node_memory + std::mem::size_of::<Self>()
    }

    /// Build tree from vectors
    fn build(
        &mut self,
        vectors: &[(String, Vec<f32>)],
        config: &AxisAnnoyConfig,
        rng: &mut ChaCha20Rng,
        distance_compute: &UnifiedDistanceCompute,
    ) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }

        // Build tree recursively
        self.root = self.build_subtree(vectors, config, rng, distance_compute)?;
        Ok(())
    }

    /// Build a subtree recursively
    fn build_subtree(
        &mut self,
        vectors: &[(String, Vec<f32>)],
        config: &AxisAnnoyConfig,
        rng: &mut ChaCha20Rng,
        distance_compute: &UnifiedDistanceCompute,
    ) -> Result<usize> {
        // Base case: create leaf node
        if vectors.len() <= config.max_leaf_size {
            let node = AnnoyNode::Leaf {
                vector_ids: vectors.iter().map(|(id, _)| id.clone()).collect(),
            };
            let node_idx = self.nodes.len();
            self.nodes.push(node);
            return Ok(node_idx);
        }

        // Choose two random points
        let idx1 = rng.gen_range(0..vectors.len());
        let mut idx2 = rng.gen_range(0..vectors.len());
        while idx2 == idx1 {
            idx2 = rng.gen_range(0..vectors.len());
        }

        let v1 = &vectors[idx1].1;
        let v2 = &vectors[idx2].1;

        // Create hyperplane (normalized difference vector)
        let mut hyperplane = vec![0.0; self.dimension];
        let mut norm = 0.0;
        for i in 0..self.dimension {
            hyperplane[i] = v1[i] - v2[i];
            norm += hyperplane[i] * hyperplane[i];
        }
        norm = norm.sqrt();
        if norm > 0.0 {
            for h in &mut hyperplane {
                *h /= norm;
            }
        }

        // Calculate offset (midpoint projection)
        let mut offset = 0.0;
        for i in 0..self.dimension {
            offset += hyperplane[i] * (v1[i] + v2[i]) / 2.0;
        }

        // Split vectors
        let mut left_vectors = Vec::new();
        let mut right_vectors = Vec::new();

        for (id, vec) in vectors {
            let mut dot = 0.0;
            for i in 0..self.dimension {
                dot += hyperplane[i] * vec[i];
            }
            
            if dot <= offset {
                left_vectors.push((id.clone(), vec.clone()));
            } else {
                right_vectors.push((id.clone(), vec.clone()));
            }
        }

        // Handle edge case where all vectors go to one side
        if left_vectors.is_empty() || right_vectors.is_empty() {
            // Fall back to leaf node
            let node = AnnoyNode::Leaf {
                vector_ids: vectors.iter().map(|(id, _)| id.clone()).collect(),
            };
            let node_idx = self.nodes.len();
            self.nodes.push(node);
            return Ok(node_idx);
        }

        // Create split node (reserve space first)
        let node_idx = self.nodes.len();
        self.nodes.push(AnnoyNode::Leaf { vector_ids: vec![] }); // Placeholder

        // Build children
        let left_idx = self.build_subtree(&left_vectors, config, rng, distance_compute)?;
        let right_idx = self.build_subtree(&right_vectors, config, rng, distance_compute)?;

        // Update the split node
        self.nodes[node_idx] = AnnoyNode::Split {
            hyperplane,
            offset,
            left: left_idx,
            right: right_idx,
        };

        Ok(node_idx)
    }

    /// Search for nearest neighbors in this tree
    fn search(
        &self,
        query: &[f32],
        k: usize,
        candidates: &mut Vec<(String, f32)>,
        vectors: &HashMap<String, Arc<VectorRecord>>,
        distance_compute: &UnifiedDistanceCompute,
        nodes_to_search: usize,
    ) -> Result<()> {
        let mut stack = vec![(self.root, 0)];
        let mut nodes_searched = 0;

        while let Some((node_idx, _depth)) = stack.pop() {
            if nodes_searched >= nodes_to_search {
                break;
            }
            nodes_searched += 1;

            match &self.nodes[node_idx] {
                AnnoyNode::Leaf { vector_ids } => {
                    // Add all vectors in leaf to candidates
                    for id in vector_ids {
                        if let Some(record) = vectors.get(id) {
                            let distance_result = distance_compute.calculate_distance(query, &record.vector, &distance_compute.system_default());
                            candidates.push((id.clone(), distance_result.rank_value));
                        }
                    }
                }
                AnnoyNode::Split { hyperplane, offset, left, right } => {
                    // Calculate which side of hyperplane query is on
                    let mut dot = 0.0;
                    for i in 0..self.dimension {
                        dot += hyperplane[i] * query[i];
                    }

                    // Search both sides, but prioritize the side containing the query
                    if dot <= *offset {
                        stack.push((*right, 0)); // Push far side first (lower priority)
                        stack.push((*left, 0));  // Push near side (higher priority)
                    } else {
                        stack.push((*left, 0));  // Push far side first
                        stack.push((*right, 0)); // Push near side
                    }
                }
            }
        }

        Ok(())
    }
}

/// AXIS-native Annoy index implementation with improved concurrency
pub struct AxisAnnoyIndex {
    /// Configuration
    config: AxisAnnoyConfig,
    
    /// USING UTILS: Standardized vector storage
    vectors: IndexVectorStore,
    
    /// USING UTILS: Performance statistics
    stats: AtomicStats,
    
    /// Forest of trees (RwLock only for build process, then read-only)
    trees: RwLock<Vec<AnnoyTree>>,
    
    /// Whether index has been built (atomic for lock-free reads)
    is_built: AtomicBool,
    
    /// Distance computation
    distance_compute: UnifiedDistanceCompute,
    
    /// Index algorithm representation
    algorithm: IndexAlgorithm,
}

impl AxisAnnoyIndex {
    /// Create new Annoy index using standardized utilities
    pub fn new(config: AxisAnnoyConfig, dimension: usize) -> Result<Self> {
        // USING UTILS: Validate configuration
        validation::validate_dimension(dimension)?;
        
        info!(
            "Creating AXIS Annoy index: {} trees, search_k={}, dim={}",
            config.n_trees, config.search_k, dimension
        );
        
        let distance_compute = UnifiedDistanceCompute::new(config.distance_metric);
        
        let algorithm = IndexAlgorithm::Annoy {
            n_trees: config.n_trees as u32,
            search_k: config.search_k,
            max_leaf_size: config.max_leaf_size as u32,
        };
        
        Ok(Self {
            config,
            // USING UTILS: Standardized vector storage
            vectors: IndexVectorStore::new(dimension),
            // USING UTILS: Performance statistics
            stats: AtomicStats::new(),
            
            trees: RwLock::new(Vec::new()), // Empty until built
            is_built: AtomicBool::new(false),
            distance_compute,
            algorithm,
        })
    }

    /// Build the index from current vectors using improved concurrency
    pub async fn build(&self) -> Result<()> {
        
        // Check if already built
        if self.is_built.load(Ordering::Relaxed) {
            return Err(anyhow!("Index is already built"));
        }
        
        if self.vectors.is_empty() {
            // Mark as built even for empty index
            self.is_built.store(true, Ordering::Relaxed);
            return Ok(());
        }

        info!(
            "Building Annoy index with {} trees for {} vectors",
            self.config.n_trees,
            self.vectors.len()
        );

        // USING UTILS: Prepare vector data from IndexVectorStore
        let vector_data: Vec<(String, Vec<f32>)> = self.vectors.keys()
            .into_iter()
            .filter_map(|id| {
                self.vectors.get(&id).map(|record| (id, record.vector.clone()))
            })
            .collect();

        // Build trees in parallel
        let mut trees = Vec::new();
        let mut rng = ChaCha20Rng::seed_from_u64(self.config.seed);

        for tree_idx in 0..self.config.n_trees {
            debug!("Building tree {}/{}", tree_idx + 1, self.config.n_trees);
            
            let mut tree = AnnoyTree::new(self.vectors.dimension());
            let tree_seed = rng.gen();
            let mut tree_rng = ChaCha20Rng::seed_from_u64(tree_seed);
            
            tree.build(&vector_data, &self.config, &mut tree_rng, &self.distance_compute)?;
            trees.push(tree);
        }

        // Update trees
        *self.trees.write().unwrap() = trees;
        
        // Mark as built atomically  
        self.is_built.store(true, Ordering::Release);

        info!("Annoy index built successfully");
        Ok(())
    }

    /// Get search_k value
    fn get_search_k(&self, n_vectors: usize) -> usize {
        if self.config.search_k < 0 {
            // Default: n_trees * sqrt(n) * 1.5
            let default_k = (self.config.n_trees as f64 * (n_vectors as f64).sqrt() * 1.5) as usize;
            default_k.max(self.config.n_trees * 10)
        } else {
            self.config.search_k as usize
        }
    }

    /// Get statistics
    pub fn stats(&self) -> AnnoyStats {
        let vectors = self.vectors.read();
        let trees = self.trees.read();
        
        let total_nodes = trees.iter().map(|t| t.nodes.len()).sum();
        let avg_tree_depth = if !trees.is_empty() {
            self.estimate_avg_depth(&trees)
        } else {
            0.0
        };

        AnnoyStats {
            vector_count: vectors.len(),
            tree_count: trees.len(),
            total_nodes,
            avg_tree_depth,
            is_built: *self.is_built.read(),
        }
    }

    /// Estimate average tree depth
    fn estimate_avg_depth(&self, trees: &[AnnoyTree]) -> f32 {
        if trees.is_empty() {
            return 0.0;
        }

        let total_depth: usize = trees.iter()
            .map(|tree| self.calculate_max_depth(tree, tree.root, 0))
            .sum();

        total_depth as f32 / trees.len() as f32
    }

    /// Calculate maximum depth of a tree
    fn calculate_max_depth(&self, tree: &AnnoyTree, node_idx: usize, current_depth: usize) -> usize {
        match &tree.nodes[node_idx] {
            AnnoyNode::Leaf { .. } => current_depth,
            AnnoyNode::Split { left, right, .. } => {
                let left_depth = self.calculate_max_depth(tree, *left, current_depth + 1);
                let right_depth = self.calculate_max_depth(tree, *right, current_depth + 1);
                left_depth.max(right_depth)
            }
        }
    }
}

#[async_trait]
impl AxisVectorIndex for AxisAnnoyIndex {
    async fn add(&self, id: String, vector: Arc<VectorRecord>) -> Result<()> {
        let start = std::time::Instant::now();
        
        // Check if already built (lock-free atomic read)
        if self.is_built.load(Ordering::Relaxed) {
            self.stats.record_failure(start.elapsed().as_micros() as u64);
            return Err(anyhow!("Annoy index is static and cannot be modified after building"));
        }

        // USING UTILS: Validate vector ID and insert with automatic validation
        validation::validate_vector_id(&id)?;
        self.vectors.insert(id, vector)?;
        
        // USING UTILS: Record successful operation
        self.stats.record_success(start.elapsed().as_micros() as u64);
        Ok(())
    }

    async fn search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        let start = std::time::Instant::now();
        
        // Check if built (lock-free atomic read)
        if !self.is_built.load(Ordering::Relaxed) {
            self.stats.record_failure(start.elapsed().as_micros() as u64);
            return Err(anyhow!("Index must be built before searching"));
        }

        // USING UTILS: Validate k parameter
        let k = validation::validate_k(k, 10000)?;
        
        // Validate query dimension against stored dimension
        if query.len() != self.vectors.dimension() {
            self.stats.record_failure(start.elapsed().as_micros() as u64);
            return Err(anyhow!(
                "Query dimension {} doesn't match index dimension {}",
                query.len(),
                self.vectors.dimension()
            ));
        }

        // Get trees read lock
        let trees = self.trees.read();
        
        // Return empty results for empty index (valid case)
        if self.vectors.is_empty() {
            self.stats.record_success(start.elapsed().as_micros() as u64);
            return Ok(vec![]);
        }

        // Determine number of nodes to search
        let search_k = self.get_search_k(self.vectors.len());
        let nodes_per_tree = search_k / self.config.n_trees;

        // Collect candidates from all trees
        let mut all_candidates = Vec::new();
        
        // Create a temporary HashMap for tree search compatibility
        // TODO: Update tree search to work with IndexVectorStore directly
        let vector_map: HashMap<String, Arc<VectorRecord>> = self.vectors.keys()
            .into_iter()
            .filter_map(|key| self.vectors.get(&key).map(|v| (key, v)))
            .collect();
        
        for tree in trees.iter() {
            tree.search(
                query,
                k,
                &mut all_candidates,
                &vector_map,
                &self.distance_compute,
                nodes_per_tree,
            )?;
        }

        // Apply filter if provided
        if let Some(filter_fn) = filter {
            all_candidates.retain(|(id, _)| {
                self.vectors.get(id)
                    .map(|record| filter_fn(&record))
                    .unwrap_or(false)
            });
        }

        // Sort by distance and take top k
        all_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_candidates.truncate(k);

        // USING UTILS: Record successful search
        self.stats.record_success(start.elapsed().as_micros() as u64);
        Ok(all_candidates)
    }

    async fn remove(&self, _id: &str) -> Result<()> {
        Err(anyhow!("Annoy index is static and does not support removal"))
    }

    fn algorithm(&self) -> &IndexAlgorithm {
        &self.algorithm
    }

    fn stats(&self) -> IndexStats {
        // USING UTILS: Get standardized memory usage
        let vector_memory = self.vectors.memory_usage();
        let tree_memory = memory::vec_memory::<AnnoyTree>(self.trees.len()) 
            + self.trees.iter().map(|tree| tree.estimate_memory()).sum::<usize>();
        
        let total_memory = std::mem::size_of::<Self>() + vector_memory + tree_memory;

        IndexStats {
            vector_count: self.vectors.len(),
            memory_usage_bytes: total_memory,
            index_type: "Annoy".to_string(),
        }
    }
}

/// Annoy-specific statistics
#[derive(Debug, Clone)]
pub struct AnnoyStats {
    pub vector_count: usize,
    pub tree_count: usize,
    pub total_nodes: usize,
    pub avg_tree_depth: f32,
    pub is_built: bool,
}

#[cfg(test)]
#[path = "annoy_index_tests.rs"]
mod annoy_index_tests;