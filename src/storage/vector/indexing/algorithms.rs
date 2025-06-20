// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Vector Indexing Algorithms Collection
//!
//! This module provides implementations of various vector indexing algorithms
//! for the unified indexing system, including Flat, IVF, LSH, and others.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::super::types::*;
use super::manager::{IndexBuilder, BuildCostEstimate};
use crate::core::VectorId;

/// Flat (Brute Force) Index Implementation
/// Provides exact search with linear time complexity
pub struct FlatIndex {
    /// Vector storage
    vectors: Arc<RwLock<HashMap<VectorId, Vec<f32>>>>,
    
    /// Distance metric
    distance_metric: DistanceMetric,
    
    /// Index statistics
    stats: Arc<RwLock<FlatIndexStats>>,
}

/// Flat index statistics
#[derive(Debug, Default)]
pub struct FlatIndexStats {
    pub total_vectors: usize,
    pub search_count: u64,
    pub avg_search_time_ms: f64,
    pub total_comparisons: u64,
}

/// IVF (Inverted File) Index Implementation
/// Provides approximate search with clustering-based acceleration
pub struct IvfIndex {
    /// Cluster centroids
    centroids: Vec<Vec<f32>>,
    
    /// Inverted lists (cluster_id -> vector_ids)
    inverted_lists: HashMap<usize, Vec<VectorId>>,
    
    /// Vector storage
    vectors: Arc<RwLock<HashMap<VectorId, Vec<f32>>>>,
    
    /// Configuration
    config: IvfConfig,
    
    /// Distance metric
    distance_metric: DistanceMetric,
    
    /// Index statistics
    stats: Arc<RwLock<IvfIndexStats>>,
}

/// IVF configuration
#[derive(Debug, Clone)]
pub struct IvfConfig {
    pub n_lists: usize,    // Number of clusters
    pub n_probes: usize,   // Number of clusters to search
    pub train_size: usize, // Training set size for clustering
}

/// IVF index statistics
#[derive(Debug, Default)]
pub struct IvfIndexStats {
    pub total_vectors: usize,
    pub n_clusters: usize,
    pub avg_cluster_size: f64,
    pub search_count: u64,
    pub avg_search_time_ms: f64,
    pub avg_clusters_searched: f64,
}

/// LSH (Locality Sensitive Hashing) Index Implementation
/// Provides approximate search with hash-based acceleration
pub struct LshIndex {
    /// Hash tables
    hash_tables: Vec<LshHashTable>,
    
    /// Vector storage
    vectors: Arc<RwLock<HashMap<VectorId, Vec<f32>>>>,
    
    /// Configuration
    config: LshConfig,
    
    /// Random projection matrices
    projection_matrices: Vec<Vec<Vec<f32>>>,
    
    /// Index statistics
    stats: Arc<RwLock<LshIndexStats>>,
}

/// LSH hash table
#[derive(Debug)]
pub struct LshHashTable {
    /// Hash buckets (hash_value -> vector_ids)
    buckets: HashMap<u64, Vec<VectorId>>,
    
    /// Hash function parameters
    hash_params: LshHashParams,
}

/// LSH hash function parameters
#[derive(Debug, Clone)]
pub struct LshHashParams {
    pub projection_dim: usize,
    pub hash_width: f32,
}

/// LSH configuration
#[derive(Debug, Clone)]
pub struct LshConfig {
    pub n_tables: usize,      // Number of hash tables
    pub n_bits: usize,        // Number of bits per hash
    pub hash_width: f32,      // Hash function width
    pub projection_dim: usize, // Projection dimension
}

/// LSH index statistics
#[derive(Debug, Default)]
pub struct LshIndexStats {
    pub total_vectors: usize,
    pub n_hash_tables: usize,
    pub avg_bucket_size: f64,
    pub search_count: u64,
    pub avg_search_time_ms: f64,
    pub hash_collisions: u64,
}

// Implementation of FlatIndex

impl FlatIndex {
    pub fn new(distance_metric: DistanceMetric) -> Self {
        Self {
            vectors: Arc::new(RwLock::new(HashMap::new())),
            distance_metric,
            stats: Arc::new(RwLock::new(FlatIndexStats::default())),
        }
    }
    
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
}

#[async_trait]
impl VectorIndex for FlatIndex {
    async fn add_vector(
        &mut self,
        vector_id: VectorId,
        vector: &[f32],
        _metadata: &serde_json::Value,
    ) -> Result<()> {
        let mut vectors = self.vectors.write().await;
        vectors.insert(vector_id, vector.to_vec());
        
        let mut stats = self.stats.write().await;
        stats.total_vectors = vectors.len();
        
        Ok(())
    }
    
    async fn remove_vector(&mut self, vector_id: VectorId) -> Result<()> {
        let mut vectors = self.vectors.write().await;
        vectors.remove(&vector_id);
        
        let mut stats = self.stats.write().await;
        stats.total_vectors = vectors.len();
        
        Ok(())
    }
    
    async fn search(
        &self,
        query: &[f32],
        k: usize,
        _filters: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        let search_start = std::time::Instant::now();
        let vectors = self.vectors.read().await;
        
        let mut distances: Vec<(VectorId, f32)> = Vec::new();
        let mut comparisons = 0;
        
        // Calculate distance to all vectors (brute force)
        for (vector_id, vector) in vectors.iter() {
            let distance = self.calculate_distance(query, vector);
            distances.push((vector_id.clone(), distance));
            comparisons += 1;
        }
        
        // Sort by distance and take top k
        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        distances.truncate(k);
        
        // Convert to SearchResult format
        let results = distances.into_iter()
            .enumerate()
            .map(|(rank, (vector_id, distance))| SearchResult {
                vector_id,
                score: 1.0 - distance.min(1.0), // Convert distance to similarity score
                vector: None,
                metadata: serde_json::Value::Null,
                debug_info: None,
                storage_info: None,
                rank: Some(rank),
                distance: Some(distance),
            })
            .collect();
        
        // Update statistics
        let search_time = search_start.elapsed().as_millis() as f64;
        {
            let mut stats = self.stats.write().await;
            stats.search_count += 1;
            stats.total_comparisons += comparisons;
            
            // Update rolling average
            let alpha = 0.1;
            stats.avg_search_time_ms = stats.avg_search_time_ms * (1.0 - alpha) + search_time * alpha;
        }
        
        Ok(results)
    }
    
    async fn optimize(&mut self) -> Result<()> {
        debug!("🔧 Optimizing Flat index (no-op for brute force)");
        Ok(())
    }
    
    async fn statistics(&self) -> Result<IndexStatistics> {
        let stats = self.stats.read().await;
        let vectors = self.vectors.read().await;
        
        Ok(IndexStatistics {
            vector_count: stats.total_vectors,
            index_size_bytes: (vectors.len() * 
                             vectors.values().next().map_or(0, |v| v.len() * 4)) as u64,
            build_time_ms: 0, // Flat index has no build time
            last_optimized: chrono::Utc::now(),
            search_accuracy: 1.0, // Flat search is exact
            avg_search_time_ms: stats.avg_search_time_ms,
        })
    }
    
    fn index_name(&self) -> &str {
        "Flat"
    }
    
    fn index_type(&self) -> IndexType {
        IndexType::Flat
    }
}

// Implementation of IvfIndex

impl IvfIndex {
    pub fn new(config: IvfConfig, distance_metric: DistanceMetric) -> Self {
        Self {
            centroids: Vec::new(),
            inverted_lists: HashMap::new(),
            vectors: Arc::new(RwLock::new(HashMap::new())),
            config,
            distance_metric,
            stats: Arc::new(RwLock::new(IvfIndexStats::default())),
        }
    }
    
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
            _ => {
                a.iter().zip(b.iter())
                    .map(|(x, y)| (x - y).powi(2))
                    .sum::<f32>()
                    .sqrt()
            }
        }
    }
    
    /// Find nearest centroid
    fn find_nearest_centroid(&self, vector: &[f32]) -> usize {
        self.centroids.iter()
            .enumerate()
            .map(|(i, centroid)| (i, self.calculate_distance(vector, centroid)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
    
    /// Train centroids using k-means clustering (simplified)
    async fn train_centroids(&mut self, training_vectors: &[(VectorId, Vec<f32>)]) -> Result<()> {
        if training_vectors.is_empty() {
            return Ok(());
        }
        
        let dimension = training_vectors[0].1.len();
        
        // Initialize centroids randomly
        self.centroids.clear();
        for i in 0..self.config.n_lists {
            if i < training_vectors.len() {
                self.centroids.push(training_vectors[i].1.clone());
            } else {
                // Random initialization
                let centroid: Vec<f32> = (0..dimension)
                    .map(|_| rand::random::<f32>() - 0.5)
                    .collect();
                self.centroids.push(centroid);
            }
        }
        
        // Simple k-means (few iterations for performance)
        for _iteration in 0..10 {
            let mut cluster_assignments: Vec<Vec<usize>> = vec![Vec::new(); self.config.n_lists];
            
            // Assign vectors to nearest centroids
            for (idx, (_, vector)) in training_vectors.iter().enumerate() {
                let nearest_centroid = self.find_nearest_centroid(vector);
                cluster_assignments[nearest_centroid].push(idx);
            }
            
            // Update centroids
            for (cluster_id, assignments) in cluster_assignments.iter().enumerate() {
                if !assignments.is_empty() {
                    let mut new_centroid = vec![0.0; dimension];
                    for &vector_idx in assignments {
                        for (i, &value) in training_vectors[vector_idx].1.iter().enumerate() {
                            new_centroid[i] += value;
                        }
                    }
                    for value in &mut new_centroid {
                        *value /= assignments.len() as f32;
                    }
                    self.centroids[cluster_id] = new_centroid;
                }
            }
        }
        
        Ok(())
    }
}

#[async_trait]
impl VectorIndex for IvfIndex {
    async fn add_vector(
        &mut self,
        vector_id: VectorId,
        vector: &[f32],
        _metadata: &serde_json::Value,
    ) -> Result<()> {
        // Store vector
        {
            let mut vectors = self.vectors.write().await;
            vectors.insert(vector_id.clone(), vector.to_vec());
        }
        
        // If centroids are not trained, we need to train them first
        if self.centroids.is_empty() {
            let vectors = self.vectors.read().await;
            let training_data: Vec<_> = vectors.iter()
                .take(self.config.train_size)
                .map(|(id, vec)| (id.clone(), vec.clone()))
                .collect();
            drop(vectors);
            
            if !training_data.is_empty() {
                self.train_centroids(&training_data).await?;
            }
        }
        
        // Assign to nearest centroid
        if !self.centroids.is_empty() {
            let nearest_centroid = self.find_nearest_centroid(vector);
            self.inverted_lists.entry(nearest_centroid)
                .or_insert_with(Vec::new)
                .push(vector_id);
        }
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_vectors += 1;
            stats.n_clusters = self.centroids.len();
            if stats.n_clusters > 0 {
                stats.avg_cluster_size = stats.total_vectors as f64 / stats.n_clusters as f64;
            }
        }
        
        Ok(())
    }
    
    async fn remove_vector(&mut self, vector_id: VectorId) -> Result<()> {
        // Remove from vector storage
        {
            let mut vectors = self.vectors.write().await;
            vectors.remove(&vector_id);
        }
        
        // Remove from inverted lists
        for (_, vector_list) in self.inverted_lists.iter_mut() {
            vector_list.retain(|id| id != &vector_id);
        }
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_vectors = stats.total_vectors.saturating_sub(1);
            if stats.n_clusters > 0 {
                stats.avg_cluster_size = stats.total_vectors as f64 / stats.n_clusters as f64;
            }
        }
        
        Ok(())
    }
    
    async fn search(
        &self,
        query: &[f32],
        k: usize,
        _filters: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        let search_start = std::time::Instant::now();
        
        if self.centroids.is_empty() {
            return Ok(Vec::new());
        }
        
        // Find nearest centroids to search
        let mut centroid_distances: Vec<(usize, f32)> = self.centroids.iter()
            .enumerate()
            .map(|(i, centroid)| (i, self.calculate_distance(query, centroid)))
            .collect();
        
        centroid_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        centroid_distances.truncate(self.config.n_probes);
        
        let vectors = self.vectors.read().await;
        let mut distances: Vec<(VectorId, f32)> = Vec::new();
        
        // Search in selected clusters
        for (cluster_id, _) in centroid_distances {
            if let Some(vector_list) = self.inverted_lists.get(&cluster_id) {
                for vector_id in vector_list {
                    if let Some(vector) = vectors.get(vector_id) {
                        let distance = self.calculate_distance(query, vector);
                        distances.push((vector_id.clone(), distance));
                    }
                }
            }
        }
        
        // Sort and take top k
        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        distances.truncate(k);
        
        // Convert to SearchResult format
        let results = distances.into_iter()
            .enumerate()
            .map(|(rank, (vector_id, distance))| SearchResult {
                vector_id,
                score: 1.0 - distance.min(1.0),
                vector: None,
                metadata: serde_json::Value::Null,
                debug_info: None,
                storage_info: None,
                rank: Some(rank),
                distance: Some(distance),
            })
            .collect();
        
        // Update statistics
        let search_time = search_start.elapsed().as_millis() as f64;
        {
            let mut stats = self.stats.write().await;
            stats.search_count += 1;
            stats.avg_clusters_searched = self.config.n_probes as f64;
            
            let alpha = 0.1;
            stats.avg_search_time_ms = stats.avg_search_time_ms * (1.0 - alpha) + search_time * alpha;
        }
        
        Ok(results)
    }
    
    async fn optimize(&mut self) -> Result<()> {
        debug!("🔧 Optimizing IVF index");
        
        // Retrain centroids with current data
        let vectors = self.vectors.read().await;
        let training_data: Vec<_> = vectors.iter()
            .take(self.config.train_size)
            .map(|(id, vec)| (id.clone(), vec.clone()))
            .collect();
        drop(vectors);
        
        if !training_data.is_empty() {
            self.train_centroids(&training_data).await?;
            
            // Reassign all vectors to new centroids
            self.inverted_lists.clear();
            let vectors = self.vectors.read().await;
            for (vector_id, vector) in vectors.iter() {
                let nearest_centroid = self.find_nearest_centroid(vector);
                self.inverted_lists.entry(nearest_centroid)
                    .or_insert_with(Vec::new)
                    .push(vector_id.clone());
            }
        }
        
        info!("✅ IVF index optimization completed");
        Ok(())
    }
    
    async fn statistics(&self) -> Result<IndexStatistics> {
        let stats = self.stats.read().await;
        let vectors = self.vectors.read().await;
        
        Ok(IndexStatistics {
            vector_count: stats.total_vectors,
            index_size_bytes: (vectors.len() * 
                             vectors.values().next().map_or(0, |v| v.len() * 4) +
                             self.centroids.len() * 
                             self.centroids.get(0).map_or(0, |c| c.len() * 4)) as u64,
            build_time_ms: 0, // Training time would be tracked here
            last_optimized: chrono::Utc::now(),
            search_accuracy: 0.85, // Approximate search
            avg_search_time_ms: stats.avg_search_time_ms,
        })
    }
    
    fn index_name(&self) -> &str {
        "IVF"
    }
    
    fn index_type(&self) -> IndexType {
        IndexType::IVF {
            n_lists: self.config.n_lists,
            n_probes: self.config.n_probes,
        }
    }
}

// Index Builders

/// Flat Index Builder
pub struct FlatIndexBuilder {
    distance_metric: DistanceMetric,
}

impl FlatIndexBuilder {
    pub fn new(distance_metric: DistanceMetric) -> Self {
        Self { distance_metric }
    }
}

#[async_trait]
impl IndexBuilder for FlatIndexBuilder {
    async fn build_index(
        &self,
        vectors: Vec<(VectorId, Vec<f32>)>,
        _config: &serde_json::Value,
    ) -> Result<Box<dyn VectorIndex>> {
        let mut index = FlatIndex::new(self.distance_metric.clone());
        
        info!("🔧 Building Flat index with {} vectors", vectors.len());
        
        for (vector_id, vector) in vectors {
            index.add_vector(vector_id, &vector, &serde_json::Value::Null).await?;
        }
        
        info!("✅ Flat index built");
        Ok(Box::new(index))
    }
    
    fn builder_name(&self) -> &'static str {
        "Flat"
    }
    
    fn index_type(&self) -> IndexType {
        IndexType::Flat
    }
    
    async fn estimate_build_cost(
        &self,
        vector_count: usize,
        dimension: usize,
        _config: &serde_json::Value,
    ) -> Result<BuildCostEstimate> {
        Ok(BuildCostEstimate {
            estimated_time_ms: 10, // Very fast to build
            memory_requirement_mb: (vector_count * dimension * 4) / (1024 * 1024),
            cpu_intensity: 0.1,
            io_requirement: 0.1,
            disk_space_mb: (vector_count * dimension * 4) / (1024 * 1024),
        })
    }
}

/// IVF Index Builder
pub struct IvfIndexBuilder {
    config: IvfConfig,
    distance_metric: DistanceMetric,
}

impl IvfIndexBuilder {
    pub fn new(config: IvfConfig, distance_metric: DistanceMetric) -> Self {
        Self { config, distance_metric }
    }
}

#[async_trait]
impl IndexBuilder for IvfIndexBuilder {
    async fn build_index(
        &self,
        vectors: Vec<(VectorId, Vec<f32>)>,
        config: &serde_json::Value,
    ) -> Result<Box<dyn VectorIndex>> {
        let mut ivf_config = self.config.clone();
        
        // Parse additional config
        if let Some(n_lists) = config.get("n_lists").and_then(|v| v.as_u64()) {
            ivf_config.n_lists = n_lists as usize;
        }
        if let Some(n_probes) = config.get("n_probes").and_then(|v| v.as_u64()) {
            ivf_config.n_probes = n_probes as usize;
        }
        
        let mut index = IvfIndex::new(ivf_config, self.distance_metric.clone());
        
        info!("🔧 Building IVF index with {} vectors", vectors.len());
        
        for (vector_id, vector) in vectors {
            index.add_vector(vector_id, &vector, &serde_json::Value::Null).await?;
        }
        
        info!("✅ IVF index built");
        Ok(Box::new(index))
    }
    
    fn builder_name(&self) -> &'static str {
        "IVF"
    }
    
    fn index_type(&self) -> IndexType {
        IndexType::IVF {
            n_lists: self.config.n_lists,
            n_probes: self.config.n_probes,
        }
    }
    
    async fn estimate_build_cost(
        &self,
        vector_count: usize,
        dimension: usize,
        _config: &serde_json::Value,
    ) -> Result<BuildCostEstimate> {
        let training_cost = self.config.n_lists * dimension * 10; // k-means iterations
        let estimated_time_ms = (training_cost + vector_count * self.config.n_lists) as u64 / 1000;
        
        Ok(BuildCostEstimate {
            estimated_time_ms,
            memory_requirement_mb: (vector_count * dimension * 4 + 
                                  self.config.n_lists * dimension * 4) / (1024 * 1024),
            cpu_intensity: 0.6,
            io_requirement: 0.3,
            disk_space_mb: (vector_count * dimension * 4) / (1024 * 1024),
        })
    }
}

impl Default for IvfConfig {
    fn default() -> Self {
        Self {
            n_lists: 100,
            n_probes: 10,
            train_size: 10000,
        }
    }
}

impl Default for LshConfig {
    fn default() -> Self {
        Self {
            n_tables: 8,
            n_bits: 12,
            hash_width: 1.0,
            projection_dim: 64,
        }
    }
}