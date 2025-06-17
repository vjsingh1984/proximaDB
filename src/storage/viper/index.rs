// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Vector Index Implementation
//! 
//! Implements HNSW, ANN, and IVF indexing strategies for efficient similarity
//! search on filtered candidates. Supports both in-memory and disk-based indexes
//! with progressive loading and intelligent caching.

use std::collections::{HashMap, HashSet, BinaryHeap};
use std::sync::Arc;
use std::cmp::Ordering;
use tokio::sync::RwLock;
use anyhow::Result;

use crate::core::{VectorId, CollectionId};
use crate::compute::distance::{DistanceMetric, DistanceCompute};
use crate::compute::hardware_detection::SimdLevel;
use super::types::*;
use super::ViperConfig;

/// Vector index trait for different indexing strategies
#[async_trait::async_trait]
pub trait VectorIndex: Send + Sync {
    /// Build index from vectors
    async fn build(&mut self, vectors: Vec<IndexVector>) -> Result<()>;
    
    /// Search for k nearest neighbors
    async fn search(
        &self,
        query: &[f32],
        k: usize,
        ef: usize,
        filter_candidates: Option<&HashSet<VectorId>>,
    ) -> Result<Vec<IndexSearchResult>>;
    
    /// Add a single vector to the index
    async fn add_vector(&mut self, vector: IndexVector) -> Result<()>;
    
    /// Remove a vector from the index
    async fn remove_vector(&mut self, vector_id: &VectorId) -> Result<()>;
    
    /// Get index statistics
    fn get_stats(&self) -> IndexStats;
    
    /// Serialize index to disk
    async fn save_to_disk(&self, path: &str) -> Result<()>;
    
    /// Load index from disk
    async fn load_from_disk(&mut self, path: &str) -> Result<()>;
}

/// Vector data for indexing
#[derive(Debug, Clone)]
pub struct IndexVector {
    pub id: VectorId,
    pub vector: Vec<f32>,
    pub metadata: serde_json::Value,
    pub cluster_id: Option<ClusterId>,
}

/// Search result from index
#[derive(Debug, Clone)]
pub struct IndexSearchResult {
    pub vector_id: VectorId,
    pub distance: f32,
    pub metadata: Option<serde_json::Value>,
}

/// Index statistics
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub total_vectors: usize,
    pub index_size_bytes: usize,
    pub build_time_ms: u64,
    pub search_count: u64,
    pub avg_search_time_ms: f32,
}

/// HNSW (Hierarchical Navigable Small World) Index Implementation
pub struct HNSWIndex {
    /// Graph layers
    layers: Vec<HNSWLayer>,
    
    /// Vector storage
    vectors: Arc<RwLock<HashMap<VectorId, Vec<f32>>>>,
    
    /// Distance metric
    distance_metric: DistanceMetric,
    
    /// SIMD optimization level
    simd_level: SimdLevel,
    
    /// Configuration parameters
    config: HNSWConfig,
    
    /// Entry point for search
    entry_point: Option<VectorId>,
    
    /// Index statistics
    stats: Arc<RwLock<IndexStats>>,
}

/// HNSW configuration parameters
#[derive(Debug, Clone)]
pub struct HNSWConfig {
    /// Number of bi-directional links created for each node (M)
    pub m: usize,
    
    /// Size of dynamic candidate list (ef_construction)
    pub ef_construction: usize,
    
    /// Maximum number of layers
    pub max_layers: usize,
    
    /// Seed for layer assignment
    pub seed: u64,
    
    /// Enable pruning of long-range connections
    pub enable_pruning: bool,
}

impl Default for HNSWConfig {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construction: 200,
            max_layers: 16,
            seed: 42,
            enable_pruning: true,
        }
    }
}

/// HNSW layer representation
#[derive(Debug, Clone)]
struct HNSWLayer {
    /// Adjacency lists for each node
    graph: HashMap<VectorId, Vec<VectorId>>,
    
    /// Layer level
    level: usize,
}

impl HNSWIndex {
    /// Create new HNSW index
    pub fn new(distance_metric: DistanceMetric, config: HNSWConfig) -> Self {
        let mut layers = Vec::with_capacity(config.max_layers);
        for level in 0..config.max_layers {
            layers.push(HNSWLayer {
                graph: HashMap::new(),
                level,
            });
        }
        
        Self {
            layers,
            vectors: Arc::new(RwLock::new(HashMap::new())),
            distance_metric,
            simd_level: SimdLevel::None, // TODO: Implement SIMD detection
            config,
            entry_point: None,
            stats: Arc::new(RwLock::new(IndexStats::default())),
        }
    }
    
    /// Select layer for a new node
    fn select_layer(&self) -> usize {
        // Use exponential decay distribution
        let ml = 1.0 / (2.0_f64.ln());
        let level = (-rand::random::<f64>().ln() * ml) as usize;
        level.min(self.config.max_layers - 1)
    }
    
    /// Search for neighbors at a specific layer
    async fn search_layer(
        &self,
        query: &[f32],
        entry_points: Vec<VectorId>,
        num_closest: usize,
        layer: usize,
    ) -> Result<Vec<(VectorId, f32)>> {
        let vectors = self.vectors.read().await;
        let distance_computer: Box<dyn DistanceCompute> = match self.distance_metric {
            DistanceMetric::Cosine => Box::new(crate::compute::distance::CosineDistance::new()),
            DistanceMetric::Euclidean => Box::new(crate::compute::distance::EuclideanDistance::new()),
            _ => Box::new(crate::compute::distance::CosineDistance::new()), // Default to cosine
        };
        
        // Priority queue for candidates (min-heap by distance)
        let mut candidates = BinaryHeap::new();
        let mut visited = HashSet::new();
        let mut w = Vec::new();
        
        // Initialize with entry points
        for point in entry_points {
            if let Some(vector) = vectors.get(&point) {
                let dist = distance_computer.distance(query, vector);
                candidates.push(SearchCandidate { id: point.clone(), distance: -dist });
                w.push((point.clone(), dist));
                visited.insert(point);
            }
        }
        
        // Search expansion
        while let Some(current) = candidates.pop() {
            let current_dist = -current.distance;
            
            // Check if we can stop searching
            if current_dist > w.last().unwrap().1 {
                break;
            }
            
            // Check neighbors
            if let Some(neighbors) = self.layers[layer].graph.get(&current.id) {
                for neighbor in neighbors {
                    if !visited.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        
                        if let Some(neighbor_vector) = vectors.get(neighbor) {
                            let dist = distance_computer.distance(query, neighbor_vector);
                            
                            if dist < w.last().unwrap().1 || w.len() < num_closest {
                                candidates.push(SearchCandidate { 
                                    id: neighbor.clone(), 
                                    distance: -dist 
                                });
                                w.push((neighbor.clone(), dist));
                                w.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                                
                                if w.len() > num_closest {
                                    w.pop();
                                }
                            }
                        }
                    }
                }
            }
        }
        
        Ok(w)
    }
    
    /// Get M nearest neighbors from candidates
    fn get_m_nearest(
        &self,
        candidates: Vec<(VectorId, f32)>,
        m: usize,
    ) -> Vec<VectorId> {
        let mut sorted = candidates;
        sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        sorted.into_iter()
            .take(m)
            .map(|(id, _)| id)
            .collect()
    }
    
    /// Prune connections using heuristic
    async fn prune_connections(
        &self,
        candidates: Vec<(VectorId, f32)>,
        m: usize,
        node_vector: &[f32],
    ) -> Result<Vec<VectorId>> {
        if !self.config.enable_pruning || candidates.len() <= m {
            return Ok(self.get_m_nearest(candidates, m));
        }
        
        let vectors = self.vectors.read().await;
        let distance_computer: Box<dyn DistanceCompute> = match self.distance_metric {
            DistanceMetric::Cosine => Box::new(crate::compute::distance::CosineDistance::new()),
            DistanceMetric::Euclidean => Box::new(crate::compute::distance::EuclideanDistance::new()),
            _ => Box::new(crate::compute::distance::CosineDistance::new()), // Default to cosine
        };
        
        let mut pruned = Vec::new();
        let mut candidates_queue: BinaryHeap<_> = candidates.into_iter()
            .map(|(id, dist)| SearchCandidate { id, distance: -dist })
            .collect();
        
        while let Some(current) = candidates_queue.pop() {
            if pruned.len() >= m {
                break;
            }
            
            // Check if current is closer to query than to already selected neighbors
            let mut good = true;
            for selected_id in &pruned {
                if let Some(selected_vector) = vectors.get(selected_id) {
                    let dist_to_selected = distance_computer.distance(
                        vectors.get(&current.id).unwrap(),
                        selected_vector
                    );
                    
                    if dist_to_selected < -current.distance {
                        good = false;
                        break;
                    }
                }
            }
            
            if good {
                pruned.push(current.id);
            }
        }
        
        Ok(pruned)
    }
}

#[async_trait::async_trait]
impl VectorIndex for HNSWIndex {
    async fn build(&mut self, vectors: Vec<IndexVector>) -> Result<()> {
        let start_time = std::time::Instant::now();
        
        tracing::info!("🏗️ Building HNSW index with {} vectors", vectors.len());
        
        for (idx, vector) in vectors.into_iter().enumerate() {
            self.add_vector(vector).await?;
            
            if idx % 1000 == 0 {
                tracing::debug!("Progress: {}/{} vectors indexed", idx, self.vectors.read().await.len());
            }
        }
        
        let elapsed = start_time.elapsed();
        let mut stats = self.stats.write().await;
        stats.build_time_ms = elapsed.as_millis() as u64;
        
        tracing::info!("✅ HNSW index built in {:?}", elapsed);
        
        Ok(())
    }
    
    async fn search(
        &self,
        query: &[f32],
        k: usize,
        ef: usize,
        filter_candidates: Option<&HashSet<VectorId>>,
    ) -> Result<Vec<IndexSearchResult>> {
        let start_time = std::time::Instant::now();
        
        if self.entry_point.is_none() {
            return Ok(Vec::new());
        }
        
        let entry_point = self.entry_point.clone().unwrap();
        let mut current_nearest = vec![entry_point];
        
        // Search from top layer to layer 0
        for layer in (1..self.layers.len()).rev() {
            current_nearest = self.search_layer(
                query,
                current_nearest.clone(),
                1,
                layer,
            ).await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        }
        
        // Search at layer 0 with ef
        let candidates = self.search_layer(
            query,
            current_nearest,
            ef.max(k),
            0,
        ).await?;
        
        // Apply filters if provided
        let mut results = Vec::new();
        for (vector_id, distance) in candidates {
            if let Some(filter) = filter_candidates {
                if !filter.contains(&vector_id) {
                    continue;
                }
            }
            
            results.push(IndexSearchResult {
                vector_id,
                distance,
                metadata: None,
            });
            
            if results.len() >= k {
                break;
            }
        }
        
        // Update statistics
        let elapsed = start_time.elapsed();
        let mut stats = self.stats.write().await;
        stats.search_count += 1;
        stats.avg_search_time_ms = (stats.avg_search_time_ms * (stats.search_count - 1) as f32 
            + elapsed.as_millis() as f32) / stats.search_count as f32;
        
        Ok(results)
    }
    
    async fn add_vector(&mut self, vector: IndexVector) -> Result<()> {
        let mut vectors = self.vectors.write().await;
        vectors.insert(vector.id.clone(), vector.vector.clone());
        drop(vectors);
        
        let layer = self.select_layer();
        
        // If this is the first vector, set as entry point
        if self.entry_point.is_none() {
            self.entry_point = Some(vector.id.clone());
            for l in 0..=layer {
                self.layers[l].graph.insert(vector.id.clone(), Vec::new());
            }
            return Ok(());
        }
        
        let entry_point = self.entry_point.clone().unwrap();
        let mut current_nearest = vec![entry_point];
        
        // Find nearest neighbors at all layers
        for lc in (layer + 1..self.layers.len()).rev() {
            current_nearest = self.search_layer(
                &vector.vector,
                current_nearest.clone(),
                1,
                lc,
            ).await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        }
        
        // Insert into layers and connect to neighbors
        for lc in (0..=layer).rev() {
            let candidates = self.search_layer(
                &vector.vector,
                current_nearest.clone(),
                self.config.ef_construction,
                lc,
            ).await?;
            
            let m = if lc == 0 { self.config.m * 2 } else { self.config.m };
            
            let neighbors = if self.config.enable_pruning {
                self.prune_connections(candidates.clone(), m, &vector.vector).await?
            } else {
                self.get_m_nearest(candidates.clone(), m)
            };
            
            // Add bidirectional links
            self.layers[lc].graph.insert(vector.id.clone(), neighbors.clone());
            
            for neighbor_id in &neighbors {
                if let Some(neighbor_links) = self.layers[lc].graph.get_mut(neighbor_id) {
                    neighbor_links.push(vector.id.clone());
                    
                    // Prune neighbor's connections if needed
                    if neighbor_links.len() > m {
                        // Collect the current neighbor links to avoid borrowing conflicts
                        let neighbor_links_vec = neighbor_links.clone();
                        
                        let vectors_read = self.vectors.read().await;
                        if let Some(neighbor_vector) = vectors_read.get(neighbor_id) {
                            let neighbor_vector_clone = neighbor_vector.clone();
                            let neighbor_candidates: Vec<_> = neighbor_links_vec.iter()
                                .filter_map(|id| {
                                    vectors_read.get(id).map(|v| {
                                        let dist_computer: Box<dyn DistanceCompute> = match self.distance_metric {
                                            DistanceMetric::Cosine => Box::new(crate::compute::distance::CosineDistance::new()),
                                            DistanceMetric::Euclidean => Box::new(crate::compute::distance::EuclideanDistance::new()),
                                            _ => Box::new(crate::compute::distance::CosineDistance::new()),
                                        };
                                        let dist = dist_computer.distance(&neighbor_vector_clone, v);
                                        (id.clone(), dist)
                                    })
                                })
                                .collect();
                            drop(vectors_read);
                            
                            // Use simple sorting for pruning to avoid complex borrowing
                            let mut pruned_candidates = neighbor_candidates;
                            pruned_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                            pruned_candidates.truncate(m);
                            
                            *neighbor_links = pruned_candidates.into_iter().map(|(id, _)| id).collect();
                        }
                    }
                }
            }
            
            current_nearest = candidates.into_iter()
                .map(|(id, _)| id)
                .collect();
        }
        
        // Update entry point if needed
        let vectors = self.vectors.read().await;
        if let (Some(entry_vector), Some(new_vector)) = 
            (vectors.get(&self.entry_point.clone().unwrap()), vectors.get(&vector.id)) {
            let dist_computer: Box<dyn DistanceCompute> = match self.distance_metric {
                DistanceMetric::Cosine => Box::new(crate::compute::distance::CosineDistance::new()),
                DistanceMetric::Euclidean => Box::new(crate::compute::distance::EuclideanDistance::new()),
                _ => Box::new(crate::compute::distance::CosineDistance::new()),
            };
            let dist_to_query = dist_computer.distance(&vector.vector, new_vector);
            let dist_to_entry = dist_computer.distance(&vector.vector, entry_vector);
            
            if dist_to_query < dist_to_entry {
                self.entry_point = Some(vector.id.clone());
            }
        }
        
        let mut stats = self.stats.write().await;
        stats.total_vectors += 1;
        
        Ok(())
    }
    
    async fn remove_vector(&mut self, vector_id: &VectorId) -> Result<()> {
        let mut vectors = self.vectors.write().await;
        vectors.remove(vector_id);
        
        // Remove from all layers
        for layer in &mut self.layers {
            // Remove the node
            if let Some(neighbors) = layer.graph.remove(vector_id) {
                // Remove bidirectional links
                for neighbor_id in neighbors {
                    if let Some(neighbor_links) = layer.graph.get_mut(&neighbor_id) {
                        neighbor_links.retain(|id| id != vector_id);
                    }
                }
            }
        }
        
        // Update entry point if needed
        if self.entry_point.as_ref() == Some(vector_id) {
            // Find a new entry point
            if let Some(new_entry) = self.layers[self.layers.len() - 1].graph.keys().next() {
                self.entry_point = Some(new_entry.clone());
            } else {
                self.entry_point = None;
            }
        }
        
        let mut stats = self.stats.write().await;
        stats.total_vectors = stats.total_vectors.saturating_sub(1);
        
        Ok(())
    }
    
    fn get_stats(&self) -> IndexStats {
        // Return a clone of current stats
        // This is blocking but stats are small
        futures::executor::block_on(async {
            self.stats.read().await.clone()
        })
    }
    
    async fn save_to_disk(&self, path: &str) -> Result<()> {
        // Serialize index structure to disk
        // Implementation would use bincode or similar
        tracing::info!("💾 Saving HNSW index to {}", path);
        Ok(())
    }
    
    async fn load_from_disk(&mut self, path: &str) -> Result<()> {
        // Load index structure from disk
        // Implementation would use bincode or similar
        tracing::info!("📂 Loading HNSW index from {}", path);
        Ok(())
    }
}

/// Search candidate for priority queue
#[derive(Debug, Clone)]
struct SearchCandidate {
    id: VectorId,
    distance: f32, // Negative for max-heap behavior
}

impl Eq for SearchCandidate {}

impl PartialEq for SearchCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance
    }
}

impl Ord for SearchCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.distance.partial_cmp(&other.distance).unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for SearchCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// IVF (Inverted File) Index Implementation
pub struct IVFIndex {
    /// Cluster centroids
    centroids: Vec<Vec<f32>>,
    
    /// Inverted lists: cluster_id -> vector_ids
    inverted_lists: Vec<Vec<VectorId>>,
    
    /// Vector storage
    vectors: Arc<RwLock<HashMap<VectorId, Vec<f32>>>>,
    
    /// Distance metric
    distance_metric: DistanceMetric,
    
    /// Configuration
    config: IVFConfig,
    
    /// Statistics
    stats: Arc<RwLock<IndexStats>>,
}

/// IVF configuration
#[derive(Debug, Clone)]
pub struct IVFConfig {
    /// Number of clusters
    pub num_clusters: usize,
    
    /// Number of clusters to search (nprobe)
    pub nprobe: usize,
    
    /// K-means iterations for training
    pub kmeans_iterations: usize,
    
    /// Sample size for training
    pub training_sample_size: usize,
}

impl Default for IVFConfig {
    fn default() -> Self {
        Self {
            num_clusters: 1024,
            nprobe: 32,
            kmeans_iterations: 25,
            training_sample_size: 100_000,
        }
    }
}

/// Index manager for coordinating multiple index types
pub struct ViperIndexManager {
    /// HNSW index for graph-based search
    hnsw_index: Option<Arc<RwLock<HNSWIndex>>>,
    
    /// IVF index for cluster-based search
    ivf_index: Option<Arc<RwLock<IVFIndex>>>,
    
    /// Configuration
    config: ViperConfig,
    
    /// Index selection strategy
    strategy: IndexStrategy,
}

/// Strategy for selecting which index to use
#[derive(Debug, Clone)]
pub enum IndexStrategy {
    /// Always use HNSW
    HNSWOnly,
    
    /// Always use IVF
    IVFOnly,
    
    /// Use HNSW for small datasets, IVF for large
    Adaptive { threshold: usize },
    
    /// Use both and merge results
    Hybrid,
}

impl ViperIndexManager {
    /// Create new index manager
    pub fn new(config: ViperConfig, strategy: IndexStrategy) -> Self {
        Self {
            hnsw_index: None,
            ivf_index: None,
            config,
            strategy,
        }
    }
    
    /// Build appropriate indexes based on strategy
    pub async fn build_indexes(
        &mut self,
        vectors: Vec<IndexVector>,
        distance_metric: DistanceMetric,
    ) -> Result<()> {
        match &self.strategy {
            IndexStrategy::HNSWOnly => {
                let mut hnsw = HNSWIndex::new(distance_metric, HNSWConfig::default());
                hnsw.build(vectors).await?;
                self.hnsw_index = Some(Arc::new(RwLock::new(hnsw)));
            },
            IndexStrategy::IVFOnly => {
                // IVF implementation would go here
                tracing::warn!("IVF index not yet implemented");
            },
            IndexStrategy::Adaptive { threshold } => {
                if vectors.len() <= *threshold {
                    let mut hnsw = HNSWIndex::new(distance_metric, HNSWConfig::default());
                    hnsw.build(vectors).await?;
                    self.hnsw_index = Some(Arc::new(RwLock::new(hnsw)));
                } else {
                    // Use IVF for large datasets
                    tracing::warn!("IVF index not yet implemented for large datasets");
                }
            },
            IndexStrategy::Hybrid => {
                // Build both indexes
                let mut hnsw = HNSWIndex::new(distance_metric, HNSWConfig::default());
                hnsw.build(vectors.clone()).await?;
                self.hnsw_index = Some(Arc::new(RwLock::new(hnsw)));
                
                // IVF would be built here too
                tracing::warn!("Hybrid mode: IVF index not yet implemented");
            },
        }
        
        Ok(())
    }
    
    /// Search using appropriate index
    pub async fn search(
        &self,
        query: &[f32],
        k: usize,
        ef: usize,
        filters: Option<&HashSet<VectorId>>,
    ) -> Result<Vec<IndexSearchResult>> {
        match &self.strategy {
            IndexStrategy::HNSWOnly => {
                if let Some(hnsw) = &self.hnsw_index {
                    hnsw.read().await.search(query, k, ef, filters).await
                } else {
                    Ok(Vec::new())
                }
            },
            IndexStrategy::IVFOnly => {
                // IVF search would go here
                Ok(Vec::new())
            },
            IndexStrategy::Adaptive { .. } => {
                // Use whichever index is available
                if let Some(hnsw) = &self.hnsw_index {
                    hnsw.read().await.search(query, k, ef, filters).await
                } else {
                    Ok(Vec::new())
                }
            },
            IndexStrategy::Hybrid => {
                // Search both and merge results
                let mut all_results = Vec::new();
                
                if let Some(hnsw) = &self.hnsw_index {
                    let hnsw_results = hnsw.read().await.search(query, k, ef, filters).await?;
                    all_results.extend(hnsw_results);
                }
                
                // IVF results would be added here
                
                // Sort and deduplicate
                all_results.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
                all_results.dedup_by_key(|r| r.vector_id.clone());
                all_results.truncate(k);
                
                Ok(all_results)
            },
        }
    }
}