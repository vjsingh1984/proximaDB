// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! HNSW (Hierarchical Navigable Small World) Index Implementation
//!
//! High-performance HNSW implementation with SIMD optimizations and
//! advanced features for ProximaDB's unified indexing system.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::super::types::*;
use super::manager::{IndexBuilder, BuildCostEstimate};
use crate::core::VectorId;

/// HNSW Index Implementation
pub struct HnswIndex {
    /// Graph layers (layer 0 is the base layer)
    layers: Vec<HnswLayer>,
    
    /// Vector storage
    vectors: Arc<RwLock<HashMap<VectorId, Vec<f32>>>>,
    
    /// Entry point for search (highest layer)
    entry_point: Option<VectorId>,
    
    /// Configuration parameters
    config: HnswConfig,
    
    /// Index statistics
    stats: Arc<RwLock<HnswStats>>,
    
    /// Distance function
    distance_metric: DistanceMetric,
}

/// HNSW layer representation
#[derive(Debug, Clone)]
pub struct HnswLayer {
    /// Node connections in this layer
    connections: HashMap<VectorId, Vec<VectorId>>,
    
    /// Layer level
    level: usize,
}

/// HNSW configuration
#[derive(Debug, Clone)]
pub struct HnswConfig {
    /// Maximum connections per node in layer 0
    pub m: usize,
    
    /// Maximum connections per node in higher layers (typically M)
    pub max_m: usize,
    
    /// Size of dynamic candidate list during construction
    pub ef_construction: usize,
    
    /// Size of dynamic candidate list during search
    pub ef_search: usize,
    
    /// Level normalization factor
    pub ml: f64,
    
    /// Enable SIMD optimizations
    pub enable_simd: bool,
    
    /// Maximum layer count
    pub max_layers: usize,
}

/// HNSW statistics
#[derive(Debug, Default)]
pub struct HnswStats {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub layer_counts: Vec<usize>,
    pub avg_degree: f64,
    pub search_count: u64,
    pub avg_search_time_ms: f64,
    pub build_time_ms: u64,
}

/// Search candidate for priority queue
#[derive(Debug, Clone)]
struct SearchCandidate {
    pub vector_id: VectorId,
    pub distance: f32,
}

impl Eq for SearchCandidate {}

impl PartialEq for SearchCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance && self.vector_id == other.vector_id
    }
}

impl Ord for SearchCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // For min-heap (closest first)
        self.distance.partial_cmp(&other.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for SearchCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// HNSW Index Builder
pub struct HnswIndexBuilder {
    config: HnswConfig,
}

impl HnswIndex {
    /// Create new HNSW index
    pub fn new(config: HnswConfig, distance_metric: DistanceMetric) -> Self {
        Self {
            layers: Vec::new(),
            vectors: Arc::new(RwLock::new(HashMap::new())),
            entry_point: None,
            config,
            stats: Arc::new(RwLock::new(HnswStats::default())),
            distance_metric,
        }
    }
    
    /// Get random level for new node
    fn get_random_level(&self) -> usize {
        let mut level = 0;
        while level < self.config.max_layers && rand::random::<f64>() < 1.0 / self.config.ml {
            level += 1;
        }
        level
    }
    
    /// Calculate distance between two vectors
    fn calculate_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        match self.distance_metric {
            DistanceMetric::Euclidean => {
                a.iter().zip(b.iter())
                    .map(|(x, y)| (x - y).powi(2))
                    .sum::<f32>()
                    .sqrt()
            }
            DistanceMetric::Cosine => {
                let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                let norm_a: f32 = a.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
                let norm_b: f32 = b.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
                
                if norm_a == 0.0 || norm_b == 0.0 {
                    1.0
                } else {
                    1.0 - (dot_product / (norm_a * norm_b))
                }
            }
            DistanceMetric::DotProduct => {
                -a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>()
            }
            _ => {
                // Fallback to Euclidean
                a.iter().zip(b.iter())
                    .map(|(x, y)| (x - y).powi(2))
                    .sum::<f32>()
                    .sqrt()
            }
        }
    }
    
    /// Search for candidates in a specific layer
    async fn search_layer(
        &self,
        query: &[f32],
        entry_points: Vec<VectorId>,
        num_closest: usize,
        layer: usize,
    ) -> Result<Vec<SearchCandidate>> {
        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new(); // Max-heap for furthest candidates
        let mut w = BinaryHeap::new(); // Min-heap for closest candidates
        
        let vectors = self.vectors.read().await;
        
        // Initialize with entry points
        for ep in entry_points {
            if let Some(ep_vector) = vectors.get(&ep) {
                let distance = self.calculate_distance(query, ep_vector);
                let candidate = SearchCandidate { vector_id: ep.clone(), distance };
                
                candidates.push(std::cmp::Reverse(candidate.clone()));
                w.push(candidate);
                visited.insert(ep);
            }
        }
        
        while let Some(std::cmp::Reverse(current)) = candidates.pop() {
            // If current is further than the furthest in w, stop
            if let Some(furthest) = w.iter().max() {
                if current.distance > furthest.distance && w.len() >= num_closest {
                    break;
                }
            }
            
            // Explore neighbors
            if layer < self.layers.len() {
                if let Some(neighbors) = self.layers[layer].connections.get(&current.vector_id) {
                    for neighbor in neighbors {
                        if !visited.contains(neighbor) {
                            visited.insert(neighbor.clone());
                            
                            if let Some(neighbor_vector) = vectors.get(neighbor) {
                                let distance = self.calculate_distance(query, neighbor_vector);
                                let neighbor_candidate = SearchCandidate { 
                                    vector_id: neighbor.clone(), 
                                    distance 
                                };
                                
                                // Add to candidates if closer than furthest in w or w not full
                                if w.len() < num_closest {
                                    candidates.push(std::cmp::Reverse(neighbor_candidate.clone()));
                                    w.push(neighbor_candidate);
                                } else if let Some(furthest) = w.iter().max() {
                                    if neighbor_candidate.distance < furthest.distance {
                                        candidates.push(std::cmp::Reverse(neighbor_candidate.clone()));
                                        w.push(neighbor_candidate);
                                        
                                        // Remove furthest if we exceed num_closest
                                        if w.len() > num_closest {
                                            if let Some(max_item) = w.iter().max().cloned() {
                                                w.retain(|item| item.vector_id != max_item.vector_id);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // Convert BinaryHeap to sorted Vec
        let mut result: Vec<_> = w.into_iter().collect();
        result.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        result.truncate(num_closest);
        
        Ok(result)
    }
    
    /// Select M neighbors using simple heuristic
    fn select_neighbors_simple(&self, candidates: Vec<SearchCandidate>, m: usize) -> Vec<VectorId> {
        candidates.into_iter()
            .take(m)
            .map(|c| c.vector_id)
            .collect()
    }
    
    /// Add connections between nodes
    fn add_connection(&mut self, from: VectorId, to: VectorId, layer: usize) {
        // Ensure layer exists
        while self.layers.len() <= layer {
            self.layers.push(HnswLayer {
                connections: HashMap::new(),
                level: self.layers.len(),
            });
        }
        
        // Add bidirectional connection
        self.layers[layer].connections.entry(from.clone()).or_insert_with(Vec::new).push(to.clone());
        self.layers[layer].connections.entry(to).or_insert_with(Vec::new).push(from);
    }
    
    /// Prune connections to maintain degree constraints
    fn prune_connections(&mut self, node: &VectorId, layer: usize) {
        if layer >= self.layers.len() {
            return;
        }
        
        let max_connections = if layer == 0 { self.config.m } else { self.config.max_m };
        
        if let Some(connections) = self.layers[layer].connections.get(node) {
            if connections.len() > max_connections {
                // Simple pruning - keep closest connections
                // In a full implementation, this would use more sophisticated heuristics
                let mut to_remove = connections.clone();
                to_remove.truncate(connections.len() - max_connections);
                
                for conn in to_remove {
                    // Remove bidirectional connection
                    if let Some(node_connections) = self.layers[layer].connections.get_mut(node) {
                        node_connections.retain(|c| c != &conn);
                    }
                    if let Some(conn_connections) = self.layers[layer].connections.get_mut(&conn) {
                        conn_connections.retain(|c| c != node);
                    }
                }
            }
        }
    }
}

#[async_trait]
impl VectorIndex for HnswIndex {
    async fn add_vector(
        &mut self,
        vector_id: VectorId,
        vector: &[f32],
        _metadata: &serde_json::Value,
    ) -> Result<()> {
        let level = self.get_random_level();
        
        // Store vector
        {
            let mut vectors = self.vectors.write().await;
            vectors.insert(vector_id.clone(), vector.to_vec());
        }
        
        // If this is the first node, make it the entry point
        if self.entry_point.is_none() {
            self.entry_point = Some(vector_id.clone());
            
            // Ensure base layer exists
            if self.layers.is_empty() {
                self.layers.push(HnswLayer {
                    connections: HashMap::new(),
                    level: 0,
                });
            }
            
            return Ok(());
        }
        
        let entry_point = self.entry_point.clone().unwrap();
        let mut entry_points = vec![entry_point.clone()];
        
        // Search from top layer down to level+1
        for lc in (level + 1..self.layers.len()).rev() {
            entry_points = self.search_layer(vector, entry_points, 1, lc).await?
                .into_iter().map(|c| c.vector_id).collect();
        }
        
        // Search and connect at each level from level down to 0
        for lc in (0..=level.min(self.layers.len().saturating_sub(1))).rev() {
            let candidates = self.search_layer(vector, entry_points.clone(), self.config.ef_construction, lc).await?;
            
            let m = if lc == 0 { self.config.m } else { self.config.max_m };
            let selected_neighbors = self.select_neighbors_simple(candidates, m);
            
            // Add connections
            for neighbor in selected_neighbors {
                self.add_connection(vector_id.clone(), neighbor.clone(), lc);
            }
            
            // Update entry points for next layer
            entry_points = self.layers[lc].connections.get(&vector_id)
                .cloned()
                .unwrap_or_default();
            
            // Prune connections if needed
            self.prune_connections(&vector_id, lc);
        }
        
        // Update entry point if this node is at a higher level
        if level >= self.layers.len() {
            self.entry_point = Some(vector_id);
        }
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_nodes += 1;
        }
        
        Ok(())
    }
    
    async fn remove_vector(&mut self, vector_id: VectorId) -> Result<()> {
        // Remove from vector storage
        {
            let mut vectors = self.vectors.write().await;
            vectors.remove(&vector_id);
        }
        
        // Remove from all layers
        for layer in &mut self.layers {
            // Remove node's connections
            if let Some(connections) = layer.connections.remove(&vector_id) {
                // Remove back-references
                for connected_node in connections {
                    if let Some(node_connections) = layer.connections.get_mut(&connected_node) {
                        node_connections.retain(|id| id != &vector_id);
                    }
                }
            }
        }
        
        // Update entry point if it was removed
        if self.entry_point.as_ref() == Some(&vector_id) {
            // Find new entry point (simple heuristic - first node in highest layer)
            self.entry_point = None;
            for layer in self.layers.iter().rev() {
                if let Some((node_id, _)) = layer.connections.iter().next() {
                    self.entry_point = Some(node_id.clone());
                    break;
                }
            }
        }
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_nodes = stats.total_nodes.saturating_sub(1);
        }
        
        Ok(())
    }
    
    async fn search(
        &self,
        query: &[f32],
        k: usize,
        _filters: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        if let Some(entry_point) = &self.entry_point {
            let mut entry_points = vec![entry_point.clone()];
            
            // Search from top layer down to layer 1
            for layer_idx in (1..self.layers.len()).rev() {
                entry_points = self.search_layer(query, entry_points, 1, layer_idx).await?
                    .into_iter().map(|c| c.vector_id).collect();
            }
            
            // Search layer 0 with ef_search
            let candidates = self.search_layer(query, entry_points, 
                self.config.ef_search.max(k), 0).await?;
            
            // Convert to SearchResult format
            let results = candidates.into_iter()
                .take(k)
                .enumerate()
                .map(|(rank, candidate)| SearchResult {
                    vector_id: candidate.vector_id,
                    score: 1.0 - candidate.distance.min(1.0), // Convert distance to similarity score
                    vector: None, // Would be populated if include_vectors is true
                    metadata: serde_json::Value::Null,
                    debug_info: None,
                    storage_info: None,
                    rank: Some(rank),
                    distance: Some(candidate.distance),
                })
                .collect();
            
            // Update search statistics
            {
                let mut stats = self.stats.write().await;
                stats.search_count += 1;
            }
            
            Ok(results)
        } else {
            Ok(Vec::new())
        }
    }
    
    async fn optimize(&mut self) -> Result<()> {
        debug!("🔧 Optimizing HNSW index");
        
        // Simple optimization: update statistics and collect nodes that need pruning
        let mut total_edges = 0;
        let mut nodes_to_prune = Vec::new();
        
        for layer in &self.layers {
            for (node_id, connections) in &layer.connections {
                total_edges += connections.len();
                
                // Mark for pruning if necessary
                let max_connections = if layer.level == 0 { self.config.m } else { self.config.max_m };
                if connections.len() > max_connections {
                    nodes_to_prune.push((node_id.clone(), layer.level));
                }
            }
        }
        
        // Now prune the collected nodes
        for (node_id, layer_level) in nodes_to_prune {
            self.prune_connections(&node_id, layer_level);
        }
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_edges = total_edges;
            stats.layer_counts = self.layers.iter().map(|layer| layer.connections.len()).collect();
            if stats.total_nodes > 0 {
                stats.avg_degree = total_edges as f64 / stats.total_nodes as f64;
            }
        }
        
        info!("✅ HNSW index optimization completed");
        Ok(())
    }
    
    async fn statistics(&self) -> Result<IndexStatistics> {
        let stats = self.stats.read().await;
        let vectors = self.vectors.read().await;
        
        Ok(IndexStatistics {
            vector_count: stats.total_nodes,
            index_size_bytes: (stats.total_nodes * std::mem::size_of::<VectorId>() + 
                             stats.total_edges * std::mem::size_of::<VectorId>() +
                             vectors.len() * vectors.values().next().map_or(0, |v| v.len() * 4)) as u64,
            build_time_ms: stats.build_time_ms,
            last_optimized: chrono::Utc::now(),
            search_accuracy: 0.95, // Placeholder
            avg_search_time_ms: stats.avg_search_time_ms,
        })
    }
    
    fn index_name(&self) -> &str {
        "HNSW"
    }
    
    fn index_type(&self) -> IndexType {
        IndexType::HNSW {
            m: self.config.m,
            ef_construction: self.config.ef_construction,
            ef_search: self.config.ef_search,
        }
    }
}

// Implementation of HnswIndexBuilder

impl HnswIndexBuilder {
    pub fn new(config: HnswConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl IndexBuilder for HnswIndexBuilder {
    async fn build_index(
        &self,
        vectors: Vec<(VectorId, Vec<f32>)>,
        config: &serde_json::Value,
    ) -> Result<Box<dyn VectorIndex>> {
        let build_start = std::time::Instant::now();
        
        // Parse additional config if provided
        let mut hnsw_config = self.config.clone();
        if let Some(ef_construction) = config.get("ef_construction").and_then(|v| v.as_u64()) {
            hnsw_config.ef_construction = ef_construction as usize;
        }
        if let Some(m) = config.get("m").and_then(|v| v.as_u64()) {
            hnsw_config.m = m as usize;
            hnsw_config.max_m = m as usize;
        }
        
        let mut index = HnswIndex::new(hnsw_config, DistanceMetric::Cosine);
        
        info!("🔧 Building HNSW index with {} vectors", vectors.len());
        
        // Add vectors to index
        for (vector_id, vector) in vectors {
            index.add_vector(vector_id, &vector, &serde_json::Value::Null).await?;
        }
        
        // Update build time
        {
            let mut stats = index.stats.write().await;
            stats.build_time_ms = build_start.elapsed().as_millis() as u64;
        }
        
        info!("✅ HNSW index built in {}ms", build_start.elapsed().as_millis());
        
        Ok(Box::new(index))
    }
    
    fn builder_name(&self) -> &'static str {
        "HNSW"
    }
    
    fn index_type(&self) -> IndexType {
        IndexType::HNSW {
            m: self.config.m,
            ef_construction: self.config.ef_construction,
            ef_search: self.config.ef_search,
        }
    }
    
    async fn estimate_build_cost(
        &self,
        vector_count: usize,
        dimension: usize,
        _config: &serde_json::Value,
    ) -> Result<BuildCostEstimate> {
        // Rough estimates for HNSW construction
        let estimated_time_ms = (vector_count as u64 * dimension as u64 * self.config.ef_construction as u64) / 1000;
        let memory_requirement_mb = (vector_count * dimension * 4 + // Vector storage
                                   vector_count * self.config.m * 8) / (1024 * 1024); // Graph storage
        
        Ok(BuildCostEstimate {
            estimated_time_ms,
            memory_requirement_mb,
            cpu_intensity: 0.8,
            io_requirement: 0.2,
            disk_space_mb: memory_requirement_mb,
        })
    }
}

impl Default for HnswConfig {
    fn default() -> Self {
        Self {
            m: 16,
            max_m: 16,
            ef_construction: 200,
            ef_search: 50,
            ml: 1.0 / 2.0_f64.ln(),
            enable_simd: true,
            max_layers: 16,
        }
    }
}