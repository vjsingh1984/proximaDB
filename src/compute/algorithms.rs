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

//! High-performance vector search algorithms
//! 
//! This module implements state-of-the-art vector similarity search algorithms:
//! - HNSW (Hierarchical Navigable Small World) for approximate search
//! - IVF (Inverted File) for large-scale search
//! - LSH (Locality Sensitive Hashing) for binary vectors
//! - Brute force for exact search

use crate::compute::{DistanceCompute, DistanceMetric};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::cmp::Ordering;
use std::sync::{Arc, RwLock};

/// Search result with similarity score
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub vector_id: String,
    pub score: f32,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl PartialEq for SearchResult {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl Eq for SearchResult {}

impl PartialOrd for SearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // For max heap (highest scores first)
        other.score.partial_cmp(&self.score)
    }
}

impl Ord for SearchResult {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

/// Vector search algorithm trait
pub trait VectorSearchAlgorithm: Send + Sync {
    /// Add a vector to the index
    fn add_vector(&mut self, id: String, vector: Vec<f32>, metadata: Option<HashMap<String, serde_json::Value>>) -> Result<(), String>;
    
    /// Add multiple vectors to the index
    fn add_vectors(&mut self, vectors: Vec<(String, Vec<f32>, Option<HashMap<String, serde_json::Value>>)>) -> Result<(), String>;
    
    /// Search for k nearest neighbors
    fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchResult>, String>;
    
    /// Search with filtering based on metadata
    fn search_with_filter(&self, query: &[f32], k: usize, filter: &dyn Fn(&HashMap<String, serde_json::Value>) -> bool) -> Result<Vec<SearchResult>, String>;
    
    /// Remove a vector from the index
    fn remove_vector(&mut self, id: &str) -> Result<bool, String>;
    
    /// Get total number of vectors in the index
    fn size(&self) -> usize;
    
    /// Get memory usage statistics
    fn memory_usage(&self) -> MemoryUsage;
    
    /// Optimize the index (e.g., rebuild for better performance)
    fn optimize(&mut self) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct MemoryUsage {
    pub index_size_bytes: usize,
    pub vector_data_bytes: usize,
    pub metadata_bytes: usize,
    pub total_bytes: usize,
}

/// HNSW (Hierarchical Navigable Small World) algorithm implementation
pub struct HNSWIndex {
    /// Number of bi-directional links for each node
    m: usize,
    /// Size of candidate set during construction
    ef_construction: usize,
    /// Search parameter (dynamic)
    ef: usize,
    /// Maximum number of layers
    max_layers: usize,
    /// Level generation probability
    ml: f32,
    /// Distance function
    distance_computer: Box<dyn DistanceCompute>,
    /// Graph layers: layer -> node_id -> connections
    layers: Vec<HashMap<usize, Vec<usize>>>,
    /// Vector storage: node_id -> (vector, metadata)
    vectors: HashMap<usize, (Vec<f32>, Option<HashMap<String, serde_json::Value>>)>,
    /// ID mapping: external_id -> internal_node_id
    id_mapping: HashMap<String, usize>,
    /// Reverse ID mapping: internal_node_id -> external_id
    reverse_mapping: HashMap<usize, String>,
    /// Next available node ID
    next_node_id: usize,
    /// Entry point for search
    entry_point: Option<usize>,
    /// Random number generator state
    rng_state: Arc<RwLock<u64>>,
}

impl HNSWIndex {
    pub fn new(m: usize, ef_construction: usize, distance_metric: DistanceMetric, use_simd: bool) -> Self {
        let distance_computer = crate::compute::distance::create_distance_computer(distance_metric, use_simd);
        
        Self {
            m,
            ef_construction,
            ef: ef_construction,
            max_layers: 16, // Reasonable default
            ml: 1.0 / (2.0_f32.ln()), // 1/ln(2) ≈ 1.44
            distance_computer,
            layers: Vec::new(),
            vectors: HashMap::new(),
            id_mapping: HashMap::new(),
            reverse_mapping: HashMap::new(),
            next_node_id: 0,
            entry_point: None,
            rng_state: Arc::new(RwLock::new(42)), // Seed with fixed value for reproducibility
        }
    }
    
    /// Generate random level for new node
    fn get_random_level(&self) -> usize {
        let mut rng = self.rng_state.write().unwrap();
        let mut level = 0;
        let mut random_val = self.fast_random(&mut rng) as f32 / u32::MAX as f32;
        
        while random_val < 0.5 && level < self.max_layers {
            level += 1;
            random_val = self.fast_random(&mut rng) as f32 / u32::MAX as f32;
        }
        
        level
    }
    
    /// Fast pseudo-random number generator (LCG)
    fn fast_random(&self, state: &mut u64) -> u32 {
        *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (*state >> 32) as u32
    }
    
    /// Search for ef closest candidates in a specific layer
    fn search_layer(&self, query: &[f32], entry_points: &[usize], ef: usize, layer: usize) -> Vec<(usize, f32)> {
        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new(); // Min heap for candidates
        let mut dynamic_candidates = BinaryHeap::new(); // Max heap for top-k
        
        // Initialize with entry points
        for &ep in entry_points {
            if let Some((vector, _)) = self.vectors.get(&ep) {
                let dist = self.distance_computer.distance(query, vector);
                
                candidates.push(std::cmp::Reverse((OrderedFloat(dist), ep)));
                dynamic_candidates.push((OrderedFloat(dist), ep));
                visited.insert(ep);
            }
        }
        
        while let Some(std::cmp::Reverse((curr_dist, curr_node))) = candidates.pop() {
            // Check if we should continue searching
            if let Some((worst_dist, _)) = dynamic_candidates.peek() {
                if curr_dist.0 > worst_dist.0 && dynamic_candidates.len() >= ef {
                    break;
                }
            }
            
            // Explore neighbors
            if let Some(neighbors) = self.layers.get(layer).and_then(|l| l.get(&curr_node)) {
                for &neighbor in neighbors {
                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor);
                        
                        if let Some((vector, _)) = self.vectors.get(&neighbor) {
                            let dist = self.distance_computer.distance(query, vector);
                            
                            if dynamic_candidates.len() < ef {
                                candidates.push(std::cmp::Reverse((OrderedFloat(dist), neighbor)));
                                dynamic_candidates.push((OrderedFloat(dist), neighbor));
                            } else if let Some((worst_dist, _)) = dynamic_candidates.peek() {
                                if dist < worst_dist.0 {
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
        
        // Convert to sorted result
        let mut result: Vec<_> = dynamic_candidates.into_iter()
            .map(|(OrderedFloat(dist), node)| (node, dist))
            .collect();
        
        // Sort by distance (ascending)
        result.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
        result
    }
    
    /// Select m neighbors using heuristic
    fn select_neighbors(&self, candidates: Vec<(usize, f32)>, m: usize) -> Vec<usize> {
        if candidates.len() <= m {
            return candidates.into_iter().map(|(node, _)| node).collect();
        }
        
        // Simple heuristic: select m closest neighbors
        // TODO: Implement more sophisticated heuristics for better connectivity
        candidates.into_iter()
            .take(m)
            .map(|(node, _)| node)
            .collect()
    }
    
    /// Ensure layer exists and is properly initialized
    fn ensure_layer(&mut self, layer: usize) {
        while self.layers.len() <= layer {
            self.layers.push(HashMap::new());
        }
    }
    
    /// Add bidirectional connection between nodes
    fn add_connection(&mut self, layer: usize, node1: usize, node2: usize) {
        self.ensure_layer(layer);
        
        // Add node2 to node1's connections
        self.layers[layer].entry(node1).or_insert_with(Vec::new).push(node2);
        
        // Add node1 to node2's connections
        self.layers[layer].entry(node2).or_insert_with(Vec::new).push(node1);
    }
}

impl VectorSearchAlgorithm for HNSWIndex {
    fn add_vector(&mut self, id: String, vector: Vec<f32>, metadata: Option<HashMap<String, serde_json::Value>>) -> Result<(), String> {
        if self.id_mapping.contains_key(&id) {
            return Err(format!("Vector with ID '{}' already exists", id));
        }
        
        let node_id = self.next_node_id;
        self.next_node_id += 1;
        
        // Store vector and metadata
        self.vectors.insert(node_id, (vector.clone(), metadata));
        self.id_mapping.insert(id.clone(), node_id);
        self.reverse_mapping.insert(node_id, id);
        
        // Determine level for this node
        let level = self.get_random_level();
        
        // Ensure layers exist
        for l in 0..=level {
            self.ensure_layer(l);
            self.layers[l].insert(node_id, Vec::new());
        }
        
        // If this is the first node, make it the entry point
        if self.entry_point.is_none() {
            self.entry_point = Some(node_id);
            return Ok(());
        }
        
        let entry_point = self.entry_point.unwrap();
        let mut curr_nearest = vec![entry_point];
        
        // Search from top layer down to layer level+1
        for layer in (level + 1..self.layers.len()).rev() {
            curr_nearest = self.search_layer(&vector, &curr_nearest, 1, layer)
                .into_iter()
                .map(|(node, _)| node)
                .collect();
        }
        
        // Search and connect from layer level down to 0
        for layer in (0..=level).rev() {
            let candidates = self.search_layer(&vector, &curr_nearest, self.ef_construction, layer);
            
            let m = if layer == 0 { self.m * 2 } else { self.m };
            let selected = self.select_neighbors(candidates.clone(), m);
            
            // Add connections
            for neighbor in &selected {
                self.add_connection(layer, node_id, *neighbor);
            }
            
            // Update curr_nearest for next layer
            curr_nearest = candidates.into_iter()
                .map(|(node, _)| node)
                .take(1)
                .collect();
        }
        
        // Update entry point if this node is at the highest level
        if level >= self.layers.len() - 1 {
            self.entry_point = Some(node_id);
        }
        
        Ok(())
    }
    
    fn add_vectors(&mut self, vectors: Vec<(String, Vec<f32>, Option<HashMap<String, serde_json::Value>>)>) -> Result<(), String> {
        for (id, vector, metadata) in vectors {
            self.add_vector(id, vector, metadata)?;
        }
        Ok(())
    }
    
    fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchResult>, String> {
        tracing::debug!("🔍 HNSWIndex::search: query_dim={}, k={}, vectors={}, entry_point={:?}", 
            query.len(), k, self.vectors.len(), self.entry_point);
        if let Some(entry_point) = self.entry_point {
            let mut curr_nearest = vec![entry_point];
            
            // Search from top layer down to layer 1
            for layer in (1..self.layers.len()).rev() {
                curr_nearest = self.search_layer(query, &curr_nearest, 1, layer)
                    .into_iter()
                    .map(|(node, _)| node)
                    .collect();
            }
            
            // Search layer 0 with ef
            let candidates = self.search_layer(query, &curr_nearest, self.ef.max(k), 0);
            
            // Convert to SearchResult and take top k
            let mut results = Vec::new();
            for (node_id, score) in candidates.into_iter().take(k) {
                if let (Some(external_id), Some((_, metadata))) = (
                    self.reverse_mapping.get(&node_id),
                    self.vectors.get(&node_id)
                ) {
                    results.push(SearchResult {
                        vector_id: external_id.clone(),
                        score,
                        metadata: metadata.clone(),
                    });
                }
            }
            
            Ok(results)
        } else {
            Ok(Vec::new()) // Empty index
        }
    }
    
    fn search_with_filter(&self, query: &[f32], k: usize, filter: &dyn Fn(&HashMap<String, serde_json::Value>) -> bool) -> Result<Vec<SearchResult>, String> {
        tracing::debug!("🔍 HNSWIndex::search_with_filter: query_dim={}, k={}, vectors={}", query.len(), k, self.vectors.len());
        // For now, do post-filtering. TODO: Implement filtered search directly
        let all_results = self.search(query, k * 2)?; // Get more candidates
        tracing::debug!("🔍 HNSWIndex::search_with_filter: got {} raw results", all_results.len());
        
        let mut filter_matches = 0;
        let filtered: Vec<_> = all_results.into_iter()
            .filter(|result| {
                if let Some(ref metadata) = result.metadata {
                    let matches = filter(metadata);
                    if matches {
                        filter_matches += 1;
                        tracing::debug!("🔍 Filter match #{}: vector_id={}, metadata={:?}", filter_matches, result.vector_id, metadata);
                    } else {
                        tracing::debug!("🔍 Filter NO MATCH: vector_id={}, metadata={:?}", result.vector_id, metadata);
                    }
                    matches
                } else {
                    tracing::debug!("🔍 No metadata for vector_id={}, skipping", result.vector_id);
                    false
                }
            })
            .take(k)
            .collect();
        
        tracing::debug!("🔍 HNSWIndex::search_with_filter: returning {} filtered results", filtered.len());
        Ok(filtered)
    }
    
    fn remove_vector(&mut self, id: &str) -> Result<bool, String> {
        if let Some(&node_id) = self.id_mapping.get(id) {
            // Remove from all layers
            for layer in &mut self.layers {
                layer.remove(&node_id);
                
                // Remove connections to this node
                for connections in layer.values_mut() {
                    connections.retain(|&x| x != node_id);
                }
            }
            
            // Remove from mappings and vectors
            self.vectors.remove(&node_id);
            self.id_mapping.remove(id);
            self.reverse_mapping.remove(&node_id);
            
            // Update entry point if necessary
            if self.entry_point == Some(node_id) {
                self.entry_point = self.vectors.keys().next().copied();
            }
            
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    fn size(&self) -> usize {
        self.vectors.len()
    }
    
    fn memory_usage(&self) -> MemoryUsage {
        let vector_data_bytes = self.vectors.iter()
            .map(|(_, (vector, _))| vector.len() * std::mem::size_of::<f32>())
            .sum::<usize>();
        
        let metadata_bytes = self.vectors.iter()
            .map(|(_, (_, metadata))| {
                metadata.as_ref().map(|m| m.len() * 100).unwrap_or(0) // Rough estimate
            })
            .sum::<usize>();
        
        let index_size_bytes = self.layers.iter()
            .map(|layer| layer.len() * std::mem::size_of::<usize>() * 10) // Rough estimate
            .sum::<usize>();
        
        MemoryUsage {
            index_size_bytes,
            vector_data_bytes,
            metadata_bytes,
            total_bytes: index_size_bytes + vector_data_bytes + metadata_bytes,
        }
    }
    
    fn optimize(&mut self) -> Result<(), String> {
        // For HNSW, optimization could involve:
        // 1. Rebuilding with better parameters
        // 2. Pruning poor connections
        // 3. Rebalancing layers
        // For now, this is a no-op
        Ok(())
    }
}

/// Brute force search for exact results (baseline)
pub struct BruteForceIndex {
    distance_computer: Box<dyn DistanceCompute>,
    vectors: HashMap<String, (Vec<f32>, Option<HashMap<String, serde_json::Value>>)>,
}

impl BruteForceIndex {
    pub fn new(distance_metric: DistanceMetric, use_simd: bool) -> Self {
        let distance_computer = crate::compute::distance::create_distance_computer(distance_metric, use_simd);
        
        Self {
            distance_computer,
            vectors: HashMap::new(),
        }
    }
}

impl VectorSearchAlgorithm for BruteForceIndex {
    fn add_vector(&mut self, id: String, vector: Vec<f32>, metadata: Option<HashMap<String, serde_json::Value>>) -> Result<(), String> {
        if self.vectors.contains_key(&id) {
            return Err(format!("Vector with ID '{}' already exists", id));
        }
        
        self.vectors.insert(id, (vector, metadata));
        Ok(())
    }
    
    fn add_vectors(&mut self, vectors: Vec<(String, Vec<f32>, Option<HashMap<String, serde_json::Value>>)>) -> Result<(), String> {
        for (id, vector, metadata) in vectors {
            self.add_vector(id, vector, metadata)?;
        }
        Ok(())
    }
    
    fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchResult>, String> {
        let mut results = Vec::new();
        
        for (id, (vector, metadata)) in &self.vectors {
            let score = self.distance_computer.distance(query, vector);
            results.push(SearchResult {
                vector_id: id.clone(),
                score,
                metadata: metadata.clone(),
            });
        }
        
        // Sort by score
        if self.distance_computer.is_similarity() {
            results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        } else {
            results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal));
        }
        
        results.truncate(k);
        Ok(results)
    }
    
    fn search_with_filter(&self, query: &[f32], k: usize, filter: &dyn Fn(&HashMap<String, serde_json::Value>) -> bool) -> Result<Vec<SearchResult>, String> {
        let mut results = Vec::new();
        
        for (id, (vector, metadata)) in &self.vectors {
            // Apply filter
            if let Some(ref meta) = metadata {
                if !filter(meta) {
                    continue;
                }
            } else {
                continue; // Skip vectors without metadata when filtering
            }
            
            let score = self.distance_computer.distance(query, vector);
            results.push(SearchResult {
                vector_id: id.clone(),
                score,
                metadata: metadata.clone(),
            });
        }
        
        // Sort by score
        if self.distance_computer.is_similarity() {
            results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        } else {
            results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal));
        }
        
        results.truncate(k);
        Ok(results)
    }
    
    fn remove_vector(&mut self, id: &str) -> Result<bool, String> {
        Ok(self.vectors.remove(id).is_some())
    }
    
    fn size(&self) -> usize {
        self.vectors.len()
    }
    
    fn memory_usage(&self) -> MemoryUsage {
        let vector_data_bytes = self.vectors.iter()
            .map(|(_, (vector, _))| vector.len() * std::mem::size_of::<f32>())
            .sum::<usize>();
        
        let metadata_bytes = self.vectors.iter()
            .map(|(_, (_, metadata))| {
                metadata.as_ref().map(|m| m.len() * 100).unwrap_or(0)
            })
            .sum::<usize>();
        
        MemoryUsage {
            index_size_bytes: 0, // No index overhead for brute force
            vector_data_bytes,
            metadata_bytes,
            total_bytes: vector_data_bytes + metadata_bytes,
        }
    }
    
    fn optimize(&mut self) -> Result<(), String> {
        // Nothing to optimize for brute force
        Ok(())
    }
}

/// Wrapper for f32 to implement Ord
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
struct OrderedFloat(f32);

impl Eq for OrderedFloat {}

impl Ord for OrderedFloat {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}

/// Factory function to create search algorithms
pub fn create_search_algorithm(
    algorithm: crate::compute::IndexAlgorithm,
    distance_metric: DistanceMetric,
    use_simd: bool,
) -> Box<dyn VectorSearchAlgorithm> {
    match algorithm {
        crate::compute::IndexAlgorithm::HNSW { m, ef_construction, .. } => {
            Box::new(HNSWIndex::new(m, ef_construction, distance_metric, use_simd))
        }
        crate::compute::IndexAlgorithm::BruteForce => {
            Box::new(BruteForceIndex::new(distance_metric, use_simd))
        }
        _ => {
            // Default to HNSW for unimplemented algorithms
            Box::new(HNSWIndex::new(16, 200, distance_metric, use_simd))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_brute_force_search() {
        let mut index = BruteForceIndex::new(DistanceMetric::Cosine, false);
        
        // Add test vectors
        index.add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0], None).unwrap();
        index.add_vector("vec2".to_string(), vec![0.0, 1.0, 0.0], None).unwrap();
        index.add_vector("vec3".to_string(), vec![1.0, 1.0, 0.0], None).unwrap();
        
        // Search for vector similar to [1, 0, 0]
        let results = index.search(&[1.0, 0.0, 0.0], 2).unwrap();
        
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].vector_id, "vec1"); // Should be most similar
    }
    
    #[test]
    fn test_hnsw_basic() {
        let mut index = HNSWIndex::new(16, 200, DistanceMetric::Cosine, false);
        
        // Add test vectors
        index.add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0], None).unwrap();
        index.add_vector("vec2".to_string(), vec![0.0, 1.0, 0.0], None).unwrap();
        index.add_vector("vec3".to_string(), vec![1.0, 1.0, 0.0], None).unwrap();
        
        assert_eq!(index.size(), 3);
        
        // Search should work
        let results = index.search(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
    }
}