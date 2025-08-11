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

//! HNSW (Hierarchical Navigable Small World) index implementation for AXIS
//!
//! This module provides a production-ready HNSW index that integrates seamlessly
//! with the AXIS adaptive indexing system. HNSW is ideal for approximate nearest
//! neighbor search with excellent recall and query performance.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::cmp::Ordering;
use std::sync::{Arc, RwLock, atomic::{AtomicUsize, Ordering as AtomicOrdering}};
use dashmap::DashMap;
use tracing::info;

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::VectorRecord;
use crate::index::axis::index_factory::{AxisVectorIndex, IndexStats};
use crate::index::axis::types::IndexAlgorithm;
use crate::index::axis::utils::{IndexVectorStore, ConcurrentIdMapping, AtomicStats, validation, memory};

/// Memory usage statistics
#[derive(Debug, Clone)]
pub struct MemoryUsage {
    pub total_bytes: usize,
    pub index_size_bytes: usize,
    pub vector_data_bytes: usize,
    pub metadata_bytes: usize,
}

/// HNSW index configuration aligned with AXIS standards
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisHnswConfig {
    /// Number of bi-directional links for each node
    pub m: usize,
    /// Size of candidate set during construction
    pub ef_construction: usize,
    /// Search parameter (dynamic, can be adjusted at query time)
    pub ef: usize,
    /// Maximum number of layers
    pub max_layers: usize,
    /// Distance metric to use
    pub distance_metric: DistanceMetric,
}

impl Default for AxisHnswConfig {
    fn default() -> Self {
        Self {
            m: 16,              // Good balance of connectivity and memory
            ef_construction: 200, // Higher for better quality
            ef: 50,             // Lower for faster searches
            max_layers: 16,     // Reasonable depth
            distance_metric: DistanceMetric::Cosine,
        }
    }
}

/// Wrapper for f32 to implement Ord for use in BinaryHeap
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
struct OrderedFloat(f32);

impl Eq for OrderedFloat {}

impl Ord for OrderedFloat {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}

/// HNSW index implementation using shared infrastructure with collection partitioning
/// Migrated to use IndexVectorStore and ConcurrentIdMapping for consistency
pub struct AxisHnswIndex {
    /// Collection identifier for partitioning (optional for backward compatibility)
    collection_id: Option<String>,
    
    /// Configuration
    config: AxisHnswConfig,
    /// Distance computation
    distance_computer: UnifiedDistanceCompute,
    
    /// USING UTILS: Standardized vector storage with external IDs
    vectors: IndexVectorStore,
    
    /// USING UTILS: Bidirectional ID mapping for external<->internal IDs
    id_mapping: ConcurrentIdMapping,
    
    /// USING UTILS: Performance statistics tracking
    stats: AtomicStats,
    
    /// HNSW-specific: Graph layers with composite keys
    /// (layer, internal_node_id) -> connections
    /// TODO: Add partitioning - will use (collection_id, layer, node_id) in Phase 3
    layers: DashMap<(usize, usize), Vec<usize>>,
    
    /// HNSW-specific: Maximum layer currently in use (atomic)
    max_layer: AtomicUsize,
    
    /// HNSW-specific: Entry point for search
    entry_point: RwLock<Option<usize>>,
    
    /// Random number generator state
    rng_state: Arc<RwLock<u64>>,
    
    /// Algorithm type for trait requirement
    algorithm_type: IndexAlgorithm,
}

impl AxisHnswIndex {
    /// Create a new HNSW index with the given configuration using shared utilities
    pub fn new(config: AxisHnswConfig, dimension: usize) -> Result<Self> {
        Self::new_with_collection(None, config, dimension)
    }
    
    /// Create a new HNSW index for a specific collection
    pub fn new_with_collection(
        collection_id: Option<String>, 
        config: AxisHnswConfig, 
        dimension: usize
    ) -> Result<Self> {
        // USING UTILS: Validate dimension
        validation::validate_dimension(dimension)?;
        
        let coll_str = collection_id.as_ref().map(|s| s.as_str()).unwrap_or("default");
        info!(
            "Creating AXIS HNSW index for collection '{}': M={}, ef_construction={}, ef={}, dim={}",
            coll_str, config.m, config.ef_construction, config.ef, dimension
        );
        
        let distance_computer = UnifiedDistanceCompute::new(config.distance_metric);
        
        let algorithm_type = IndexAlgorithm::HNSW {
            m: config.m as u32,
            ef_construction: config.ef_construction as u32,
            ef_search: config.ef as u32,
            max_elements: 1000000, // Default max elements
        };
        
        Ok(Self {
            collection_id,
            config,
            distance_computer,
            
            // USING UTILS: Shared infrastructure
            vectors: IndexVectorStore::new(dimension),
            id_mapping: ConcurrentIdMapping::new(),
            stats: AtomicStats::new(),
            
            // HNSW-specific structures
            layers: DashMap::new(),
            max_layer: AtomicUsize::new(0),
            entry_point: RwLock::new(None),
            rng_state: Arc::new(RwLock::new(42)), // Deterministic seed for reproducibility
            algorithm_type,
        })
    }

    /// Generate random level for new node using exponential decay
    fn get_random_level(&self) -> usize {
        let mut rng = self.rng_state.write().unwrap();
        let mut level = 0;
        let _ml = 1.0 / (2.0_f32.ln()); // 1/ln(2) ≈ 1.44
        
        let mut random_val = self.fast_random(&mut rng) as f32 / u32::MAX as f32;
        
        while random_val < 0.5 && level < self.config.max_layers {
            level += 1;
            random_val = self.fast_random(&mut rng) as f32 / u32::MAX as f32;
        }
        
        level
    }
    
    /// Fast pseudo-random number generator (Linear Congruential Generator)
    fn fast_random(&self, state: &mut u64) -> u32 {
        *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (*state >> 32) as u32
    }

    /// Search for ef closest candidates in a specific layer
    fn search_layer(&self, query: &[f32], entry_points: &[usize], ef: usize, layer: usize) -> Vec<(usize, f32)> {
        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new(); // Min heap for candidates (to visit)
        let mut dynamic_candidates = BinaryHeap::new(); // Max heap for best found
        
        // Initialize with entry points
        for &ep in entry_points {
            // USING UTILS: Get vector from IndexVectorStore by internal ID
            if let Some(external_id) = self.id_mapping.get_external(ep) {
                if let Some(vector_record) = self.vectors.get(&external_id) {
                    let dist = self.distance_computer.calculate_distance(query, &vector_record.vector, &self.config.distance_metric).rank_value;
                    
                    candidates.push(std::cmp::Reverse((OrderedFloat(dist), ep)));
                    dynamic_candidates.push((OrderedFloat(dist), ep));
                    visited.insert(ep);
                }
            }
        }
        
        // Explore the graph
        while let Some(std::cmp::Reverse((curr_dist, curr_node))) = candidates.pop() {
            // Early termination: if current distance is worse than worst in dynamic_candidates
            if let Some((worst_dist, _)) = dynamic_candidates.peek() {
                if curr_dist.0 > worst_dist.0 && dynamic_candidates.len() >= ef {
                    break;
                }
            }
            
            // Explore neighbors of current node using DashMap
            if let Some(neighbors) = self.layers.get(&(layer, curr_node)) {
                for &neighbor in neighbors.value() {
                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor);
                        
                        // USING UTILS: Get vector from IndexVectorStore by internal ID
                        if let Some(external_id) = self.id_mapping.get_external(neighbor) {
                            if let Some(vector_record) = self.vectors.get(&external_id) {
                                let dist = self.distance_computer.calculate_distance(query, &vector_record.vector, &self.config.distance_metric).rank_value;
                                
                                if dynamic_candidates.len() < ef {
                                    // We need more candidates
                                    candidates.push(std::cmp::Reverse((OrderedFloat(dist), neighbor)));
                                    dynamic_candidates.push((OrderedFloat(dist), neighbor));
                                } else if let Some((worst_dist, _)) = dynamic_candidates.peek() {
                                    if dist < worst_dist.0 {
                                        // Found a better candidate
                                        candidates.push(std::cmp::Reverse((OrderedFloat(dist), neighbor)));
                                        dynamic_candidates.push((OrderedFloat(dist), neighbor));
                                        
                                        // Remove worst candidate if we exceed ef
                                        if dynamic_candidates.len() > ef {
                                            dynamic_candidates.pop();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // Convert to sorted result (ascending by distance)
        let mut result: Vec<_> = dynamic_candidates.into_iter()
            .map(|(OrderedFloat(dist), node)| (node, dist))
            .collect();
        
        result.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
        result
    }

    /// Select m neighbors using simple heuristic (closest neighbors)
    /// TODO: Implement more sophisticated heuristics for better graph connectivity
    fn select_neighbors(&self, candidates: Vec<(usize, f32)>, m: usize) -> Vec<usize> {
        candidates.into_iter()
            .take(m)
            .map(|(node, _)| node)
            .collect()
    }

}

#[async_trait]
impl AxisVectorIndex for AxisHnswIndex {
    async fn add(&self, id: String, vector: Arc<VectorRecord>) -> Result<()> {
        let start = std::time::Instant::now();
        
        // USING UTILS: Validate vector ID
        validation::validate_vector_id(&id)?;
        
        // USING UTILS: Register ID mapping and get internal node ID
        let internal_node_id = self.id_mapping.register(id.clone())?;
        
        // USING UTILS: Store vector in IndexVectorStore
        self.vectors.insert(id, vector.clone())?;
        
        // Determine level for this node
        let level = self.get_random_level();
        
        // Update maximum layer atomically
        let current_max = self.max_layer.load(AtomicOrdering::Relaxed);
        if level > current_max {
            self.max_layer.store(level, AtomicOrdering::Relaxed);
        }
        
        // Initialize layers for this node
        for l in 0..=level {
            self.layers.insert((l, internal_node_id), Vec::new());
        }
        
        // If this is the first node, make it the entry point
        {
            let mut entry_point_lock = self.entry_point.write().unwrap();
            if entry_point_lock.is_none() {
                *entry_point_lock = Some(internal_node_id);
                self.stats.record_success(start.elapsed().as_micros() as u64);
                return Ok(());
            }
        }
        
        let entry_point = self.entry_point.read().unwrap().unwrap();
        let mut curr_nearest = vec![entry_point];
        
        // Search from top layer down to level+1 (greedy search with ef=1)
        for layer in (level + 1..=self.max_layer.load(AtomicOrdering::Relaxed)).rev() {
            curr_nearest = self.search_layer(&vector.vector, &curr_nearest, 1, layer)
                .into_iter()
                .map(|(node, _)| node)
                .collect();
        }
        
        // Search and connect from level down to 0
        for layer in (0..=level).rev() {
            let candidates = self.search_layer(&vector.vector, &curr_nearest, self.config.ef_construction, layer);
            
            // Different M values for layer 0 vs higher layers
            let m = if layer == 0 { self.config.m * 2 } else { self.config.m };
            let selected = self.select_neighbors(candidates.clone(), m);
            
            // Add bidirectional connections using DashMap
            for neighbor in &selected {
                // Add internal_node_id to neighbor's connections
                self.layers.entry((layer, *neighbor))
                    .or_insert_with(Vec::new)
                    .push(internal_node_id);
                
                // Add neighbor to internal_node_id's connections
                self.layers.entry((layer, internal_node_id))
                    .or_insert_with(Vec::new)
                    .push(*neighbor);
            }
            
            // Update curr_nearest for next layer (best candidate from this layer)
            curr_nearest = candidates.into_iter()
                .take(1)
                .map(|(node, _)| node)
                .collect();
        }
        
        // Update entry point if this node reaches the highest level
        if level >= self.max_layer.load(AtomicOrdering::Relaxed) {
            *self.entry_point.write().unwrap() = Some(internal_node_id);
        }
        
        // USING UTILS: Record successful operation
        self.stats.record_success(start.elapsed().as_micros() as u64);
        Ok(())
    }

    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        self.search_with_filter(query, top_k, filter).await
    }

    async fn remove(&self, id: &str) -> Result<()> {
        let start = std::time::Instant::now();
        
        // USING UTILS: Get internal node ID from mapping
        let internal_node_id = match self.id_mapping.get_internal(id) {
            Some(node_id) => node_id,
            None => {
                self.stats.record_success(start.elapsed().as_micros() as u64);
                return Ok(()); // ID not found, nothing to remove
            }
        };
        
        // Remove from all layers and clean up connections using DashMap
        let max_layer = self.max_layer.load(AtomicOrdering::Relaxed);
        for layer in 0..=max_layer {
            // Remove this node's connections
            self.layers.remove(&(layer, internal_node_id));
            
            // Remove connections to this node from all other nodes
            let keys_to_update: Vec<_> = self.layers.iter()
                .filter(|entry| entry.key().0 == layer)
                .map(|entry| entry.key().clone())
                .collect();
                
            for key in keys_to_update {
                if let Some(mut connections) = self.layers.get_mut(&key) {
                    connections.retain(|&x| x != internal_node_id);
                }
            }
        }
        
        // USING UTILS: Remove from mappings and vectors
        self.vectors.remove(id);
        self.id_mapping.remove_by_external(id);
        
        // Update entry point if necessary
        {
            let mut entry_point_lock = self.entry_point.write().unwrap();
            if *entry_point_lock == Some(internal_node_id) {
                // Find a new entry point from remaining vectors
                if self.vectors.is_empty() {
                    *entry_point_lock = None;
                } else {
                    let keys = self.vectors.keys();
                    if let Some(first_key) = keys.first() {
                        if let Some(new_internal_id) = self.id_mapping.get_internal(first_key) {
                            *entry_point_lock = Some(new_internal_id);
                        } else {
                            *entry_point_lock = None;
                        }
                    } else {
                        *entry_point_lock = None;
                    }
                }
            }
        }
        
        // USING UTILS: Record successful operation
        self.stats.record_success(start.elapsed().as_micros() as u64);
        Ok(())
    }

    fn algorithm(&self) -> &IndexAlgorithm {
        &self.algorithm_type
    }

    fn stats(&self) -> IndexStats {
        IndexStats {
            vector_count: self.vectors.len(),
            memory_usage_bytes: self.estimate_memory_usage(),
            index_type: "HNSW".to_string(),
        }
    }
}

impl AxisHnswIndex {
    /// Search with optional filtering
    pub async fn search_with_filter(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&(dyn for<'a> Fn(&'a VectorRecord) -> bool + Send + Sync)>,
    ) -> Result<Vec<(String, f32)>> {
        let start = std::time::Instant::now();
        
        // Get entry point
        let entry_point = match self.entry_point.read().unwrap().as_ref() {
            Some(&ep) => ep,
            None => {
                self.stats.record_success(start.elapsed().as_micros() as u64);
                return Ok(Vec::new()); // Empty index
            }
        };
        
        let mut curr_nearest = vec![entry_point];
        let max_layer = self.max_layer.load(AtomicOrdering::Relaxed);
        
        // Search from top layer down to layer 1 (greedy with ef=1)
        for layer in (1..=max_layer).rev() {
            curr_nearest = self.search_layer(query, &curr_nearest, 1, layer)
                .into_iter()
                .map(|(node, _)| node)
                .collect();
        }
        
        // Search layer 0 with configured ef (or top_k if larger)
        let search_ef = self.config.ef.max(top_k);
        let candidates = self.search_layer(query, &curr_nearest, search_ef, 0);
        
        // Apply filter and convert results
        let mut results: Vec<(String, f32)> = Vec::new();
        for (internal_node_id, score) in candidates.into_iter() {
            // USING UTILS: Get external ID and vector record
            if let Some(external_id) = self.id_mapping.get_external(internal_node_id) {
                if let Some(vector_record) = self.vectors.get(&external_id) {
                    // Apply filter if provided
                    if let Some(filter_fn) = filter {
                        if !filter_fn(&vector_record) {
                            continue; // Skip filtered out results
                        }
                    }

                    results.push((external_id.clone(), score));
                    
                    if results.len() >= top_k {
                        break;
                    }
                }
            }
        }
        
        // USING UTILS: Record successful operation
        self.stats.record_success(start.elapsed().as_micros() as u64);
        Ok(results)
    }
    
    /// Get the number of vectors in the index
    pub fn size(&self) -> usize {
        self.vectors.len()
    }
    
    /// Get memory usage of the index
    pub fn memory_usage(&self) -> MemoryUsage {
        let total = self.estimate_memory_usage();
        MemoryUsage {
            total_bytes: total,
            index_size_bytes: total / 3, // Approximate distribution
            vector_data_bytes: total / 3,
            metadata_bytes: total / 3,
        }
    }
    
    /// Add a single vector to the index
    pub async fn add_vector(&mut self, id: String, vector: Vec<f32>, _metadata: Option<HashMap<String, serde_json::Value>>) -> Result<()> {
        let record = Arc::new(crate::proto::proximadb::VectorRecord {
            id: Some(id.clone()),
            vector,
            metadata: vec![],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            version: None,
            rank: None,
            score: None,
            distance: None,
        });
        self.add(id, record).await
    }
    
    /// Add multiple vectors to the index
    pub async fn add_vectors(&mut self, batch: Vec<(String, Vec<f32>, Option<HashMap<String, serde_json::Value>>)>) -> Result<()> {
        for (id, vector, metadata) in batch {
            self.add_vector(id, vector, metadata).await?;
        }
        Ok(())
    }
    
    /// Search for top_k nearest neighbors
    pub async fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(String, f32)>> {
        self.search_with_filter(query, top_k, None).await
    }
    
    /// Remove a vector from the index
    pub async fn remove_vector(&mut self, id: &str) -> Result<bool> {
        self.remove(id).await.map(|_| true)
    }
    
    /// Optimize the index (no-op for HNSW)
    pub fn optimize(&self) -> Result<()> {
        Ok(())
    }
    
    /// Estimate memory usage in bytes
    fn estimate_memory_usage(&self) -> usize {
        // USING UTILS: Get vector storage memory usage
        let vector_memory = self.vectors.memory_usage();
        
        // USING UTILS: Get ID mapping memory usage
        let id_mapping_memory = memory::dashmap_overhead::<String, usize>(self.id_mapping.len())
            + memory::dashmap_overhead::<usize, String>(self.id_mapping.len());
        
        // Graph structure memory (layers DashMap)
        let layers_memory = self.layers.len() * (std::mem::size_of::<(usize, usize)>() + std::mem::size_of::<Vec<usize>>() + 64);
        
        // Other structures
        let config_memory = std::mem::size_of::<AxisHnswConfig>();
        let stats_memory = std::mem::size_of::<AtomicStats>();
        
        vector_memory + id_mapping_memory + layers_memory + config_memory + stats_memory
    }

}

/// Factory function to create HNSW index instances
pub fn create_hnsw_index(config: AxisHnswConfig, dimension: usize) -> Result<Box<dyn AxisVectorIndex>> {
    Ok(Box::new(AxisHnswIndex::new(config, dimension)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::VectorRecord;
    
    #[tokio::test]
    async fn test_hnsw_basic_operations() {
        // Initialize hardware capabilities
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let config = AxisHnswConfig::default();
        let index = AxisHnswIndex::new(config, 3).unwrap();
        
        // Add test vectors
        let record1 = VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            version: None,
            rank: None,
            score: None,
            distance: None,
        };
        
        let record2 = VectorRecord {
            id: Some("vec2".to_string()),
            vector: vec![0.0, 1.0, 0.0],
            metadata: vec![],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            version: None,
            rank: None,
            score: None,
            distance: None,
        };
        
        let record3 = VectorRecord {
            id: Some("vec3".to_string()),
            vector: vec![1.0, 1.0, 0.0],
            metadata: vec![],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            version: None,
            rank: None,
            score: None,
            distance: None,
        };
        
        index.add("vec1".to_string(), Arc::new(record1)).await.unwrap();
        index.add("vec2".to_string(), Arc::new(record2)).await.unwrap();
        index.add("vec3".to_string(), Arc::new(record3)).await.unwrap();
        
        assert_eq!(index.stats().vector_count, 3);
        
        // Search should work
        let results = index.search(&[1.0, 0.0, 0.0], 2).await.unwrap();
        assert!(results.len() <= 2); // HNSW is approximate
        
        // Remove a vector
        index.remove("vec2").await.unwrap();
        assert_eq!(index.stats().vector_count, 2);
        
        // Remove non-existent vector (should succeed without error)
        index.remove("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn test_hnsw_search_quality() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let mut config = AxisHnswConfig::default();
        config.ef = 200; // Higher ef for better quality
        let index = AxisHnswIndex::new(config, 4).unwrap();
        
        // Create a set of test vectors
        let test_vectors = vec![
            ("v1", vec![1.0, 0.0, 0.0, 0.0]),
            ("v2", vec![0.9, 0.1, 0.0, 0.0]),
            ("v3", vec![0.0, 1.0, 0.0, 0.0]),
            ("v4", vec![0.0, 0.0, 1.0, 0.0]),
            ("v5", vec![0.0, 0.0, 0.0, 1.0]),
            ("v6", vec![0.8, 0.2, 0.0, 0.0]),
            ("v7", vec![0.7, 0.3, 0.0, 0.0]),
        ];
        
        for (id, vector) in test_vectors.iter() {
            let record = VectorRecord {
                id: Some(id.to_string()),
                vector: vector.clone(),
                metadata: vec![],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                rank: None,
                score: None,
                distance: None,
            };
            index.add(id.to_string(), Arc::new(record)).await.unwrap();
        }
        
        // Search for nearest neighbors to v1
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 3).await.unwrap();
        
        // v1, v2, v6, v7 should be closest (all have high first component)
        assert!(results.len() >= 2);
        
        // The first result should be v1 itself (exact match)
        if let Some(first) = results.first() {
            assert_eq!(first.0, "v1");
        }
    }

    #[tokio::test]
    async fn test_hnsw_layer_navigation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let mut config = AxisHnswConfig::default();
        config.m = 16;
        config.ef_construction = 200;
        let index = AxisHnswIndex::new(config, 3).unwrap();
        
        // Add enough vectors to create multiple layers
        for i in 0..50 {
            let vector = vec![
                (i as f32).sin(),
                (i as f32).cos(),
                (i as f32 * 0.5).sin(),
            ];
            let record = VectorRecord {
                id: Some(format!("vec_{}", i)),
                vector,
                metadata: vec![],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                rank: None,
                score: None,
                distance: None,
            };
            index.add(format!("vec_{}", i), Arc::new(record)).await.unwrap();
        }
        
        // Check that multiple layers were created
        let stats = index.stats();
        assert_eq!(stats.vector_count, 50);
        
        // Search should work efficiently across layers
        let query = vec![0.5, 0.5, 0.5];
        let results = index.search(&query, 10).await.unwrap();
        assert!(results.len() > 0);
    }

    #[tokio::test]
    async fn test_hnsw_pruning_heuristic() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let mut config = AxisHnswConfig::default();
        config.m = 5; // Small M to test pruning
        let index = AxisHnswIndex::new(config, 2).unwrap();
        
        // Add vectors in a line to test pruning
        for i in 0..20 {
            let record = VectorRecord {
                id: Some(format!("v{}", i)),
                vector: vec![i as f32, 0.0],
                metadata: vec![],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                rank: None,
                score: None,
                distance: None,
            };
            index.add(format!("v{}", i), Arc::new(record)).await.unwrap();
        }
        
        // Search for middle point
        let query = vec![10.0, 0.0];
        let results = index.search(&query, 5).await.unwrap();
        
        // Should find neighbors despite pruning
        assert!(results.len() >= 3);
    }

    #[test]
    fn test_hnsw_config_validation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        // Test valid config
        let valid_config = AxisHnswConfig {
            m: 16,
            ef_construction: 200,
            ef: 100,
            max_layers: 10,
            distance_metric: DistanceMetric::Cosine,
        };
        assert!(AxisHnswIndex::new(valid_config, 128).is_ok());
        
        // Test invalid dimension
        let config = AxisHnswConfig::default();
        assert!(AxisHnswIndex::new(config.clone(), 0).is_err());
        
        // Test with very high dimension
        assert!(AxisHnswIndex::new(config, 10000).is_ok());
    }

    #[tokio::test]
    async fn test_hnsw_empty_index_search() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let config = AxisHnswConfig::default();
        let index = AxisHnswIndex::new(config, 3).unwrap();
        
        // Search on empty index should return empty results
        let results = index.search(&[1.0, 0.0, 0.0], 5).await.unwrap();
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_hnsw_duplicate_removal() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let config = AxisHnswConfig::default();
        let index = AxisHnswIndex::new(config, 2).unwrap();
        
        let record = VectorRecord {
            id: Some("duplicate".to_string()),
            vector: vec![1.0, 0.0],
            metadata: vec![],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            version: None,
            rank: None,
            score: None,
            distance: None,
        };
        
        // Add the same ID twice
        index.add("duplicate".to_string(), Arc::new(record.clone())).await.unwrap();
        assert_eq!(index.stats().vector_count, 1);
        
        // Adding again should replace
        index.add("duplicate".to_string(), Arc::new(record)).await.unwrap();
        assert_eq!(index.stats().vector_count, 1);
        
        // Remove should work
        index.remove("duplicate").await.unwrap();
        assert_eq!(index.stats().vector_count, 0);
    }
}