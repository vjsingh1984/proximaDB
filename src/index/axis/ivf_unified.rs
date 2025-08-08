/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Unified IVF index with dual-store architecture
//!
//! This module provides a single IVF implementation that internally manages:
//! - Inelastic centroid store (always in memory)
//! - Elastic posting list store (tierable)
//! Both stores are properly partitioned by collection_id.

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::common::adaptive_structures::{
    AdaptiveStore, AdaptiveStoreConfig, BackendType, StorageTier,
    UnifiedTierPolicy, EvictionPolicy, PromotionCriteria, DemotionCriteria,
};
use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::VectorRecord;

/// Partitioned key for collection-aware storage
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct PartitionedKey<K> {
    pub collection_id: String,
    pub key: K,
}

impl<K> PartitionedKey<K> {
    pub fn new(collection_id: String, key: K) -> Self {
        Self { collection_id, key }
    }
}

/// Configuration for unified IVF index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedIvfConfig {
    /// Number of clusters
    pub n_clusters: usize,
    /// Number of clusters to probe during search
    pub n_probe: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Distance metric
    pub distance_metric: DistanceMetric,
    
    // Centroid store config (inelastic)
    pub centroid_config: CentroidConfig,
    
    // Posting list store config (elastic)
    pub posting_list_config: PostingListConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentroidConfig {
    /// Centroids are never evicted
    pub evictable: bool, // Always false
    /// Priority for memory allocation
    pub priority: MemoryPriority,
    /// Minimum memory guarantee
    pub min_memory_guarantee: bool, // Always true
}

impl Default for CentroidConfig {
    fn default() -> Self {
        Self {
            evictable: false,
            priority: MemoryPriority::Critical,
            min_memory_guarantee: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostingListConfig {
    /// Posting lists can be evicted
    pub evictable: bool, // Always true
    /// Access count to be considered hot
    pub promotion_threshold: usize,
    /// Seconds idle before demotion
    pub demotion_threshold: u64,
    /// Maximum memory for posting lists (MB)
    pub max_memory_mb: usize,
    /// Enable predictive prefetch
    pub enable_prefetch: bool,
}

impl Default for PostingListConfig {
    fn default() -> Self {
        Self {
            evictable: true,
            promotion_threshold: 100,
            demotion_threshold: 3600,
            max_memory_mb: 1500,
            enable_prefetch: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryPriority {
    Critical,  // Never evict (centroids)
    High,      // Evict last (hot posting lists)
    Normal,    // Standard eviction (warm posting lists)
    Low,       // Evict first (cold posting lists)
}

/// Inelastic centroid store - always in memory
struct CentroidStore {
    centroids: Arc<Vec<Vec<f32>>>,
    dimension: usize,
    trained: bool,
    
    // Small metadata always in memory
    cluster_sizes: Vec<AtomicUsize>,
    cluster_stats: Vec<ClusterStats>,
}

#[derive(Debug, Clone, Default)]
struct ClusterStats {
    pub vector_count: usize,
    pub last_updated: Option<Instant>,
    pub variance: f32,
}

impl CentroidStore {
    fn new(n_clusters: usize, dimension: usize) -> Self {
        Self {
            centroids: Arc::new(Vec::with_capacity(n_clusters)),
            dimension,
            trained: false,
            cluster_sizes: (0..n_clusters).map(|_| AtomicUsize::new(0)).collect(),
            cluster_stats: vec![ClusterStats::default(); n_clusters],
        }
    }
    
    fn is_trained(&self) -> bool {
        self.trained
    }
    
    fn train(&mut self, training_vectors: &[Vec<f32>]) -> Result<()> {
        // K-means clustering for centroid training
        info!("Training IVF centroids with {} vectors", training_vectors.len());
        
        // Implementation would do actual k-means here
        // For now, placeholder
        self.trained = true;
        Ok(())
    }
    
    fn find_nearest_centroid(&self, vector: &[f32], distance_compute: &UnifiedDistanceCompute) -> usize {
        let mut min_dist = f32::MAX;
        let mut nearest = 0;
        
        for (idx, centroid) in self.centroids.iter().enumerate() {
            let dist = distance_compute.distance(vector, centroid);
            if dist < min_dist {
                min_dist = dist;
                nearest = idx;
            }
        }
        
        nearest
    }
    
    fn find_nearest_centroids(&self, vector: &[f32], n: usize, distance_compute: &UnifiedDistanceCompute) -> Vec<(usize, f32)> {
        let mut distances: Vec<(usize, f32)> = self.centroids
            .iter()
            .enumerate()
            .map(|(idx, centroid)| {
                let dist = distance_compute.distance(vector, centroid);
                (idx, dist)
            })
            .collect();
        
        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        distances.truncate(n);
        distances
    }
    
    fn memory_usage_bytes(&self) -> usize {
        let centroids_size = self.centroids.len() * self.dimension * std::mem::size_of::<f32>();
        let metadata_size = self.cluster_sizes.len() * std::mem::size_of::<AtomicUsize>()
            + self.cluster_stats.len() * std::mem::size_of::<ClusterStats>();
        
        centroids_size + metadata_size
    }
}

/// Posting list that can be tiered
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostingList {
    pub cluster_id: usize,
    pub vector_ids: Vec<String>,
    pub vectors: Option<Vec<Vec<f32>>>, // None when on disk
    pub last_access: u64, // Unix timestamp
    pub access_count: u64,
}

/// Unified IVF index with dual stores
pub struct UnifiedIvfIndex {
    /// Collection identifier for partitioning
    collection_id: String,
    
    /// INELASTIC: Centroid store (always in memory)
    centroids: CentroidStore,
    
    /// ELASTIC: Posting list store (tierable)
    posting_lists: Arc<AdaptiveStore<PartitionedKey<usize>, PostingList>>,
    
    /// Vector storage (separate from posting lists for flexibility)
    vectors: Arc<DashMap<PartitionedKey<String>, Arc<VectorRecord>>>,
    
    /// Distance computation
    distance_compute: UnifiedDistanceCompute,
    
    /// Configuration
    config: UnifiedIvfConfig,
    
    /// Global statistics
    vector_count: Arc<AtomicUsize>,
    search_count: Arc<AtomicU64>,
    
    /// Access pattern tracking for prefetch
    access_correlations: Arc<DashMap<usize, Vec<(usize, f32)>>>,
}

impl UnifiedIvfIndex {
    pub fn new(collection_id: String, config: UnifiedIvfConfig) -> Result<Self> {
        info!(
            "Creating unified IVF index for collection '{}': {} clusters, {} probe",
            collection_id, config.n_clusters, config.n_probe
        );
        
        // Create inelastic centroid store
        let centroids = CentroidStore::new(config.n_clusters, config.dimension);
        
        // Create elastic posting list store with collection partitioning
        let posting_store_config = AdaptiveStoreConfig {
            backend_type: BackendType::Index {
                algorithm: "ivf".to_string(),
                expected_qps: 1000,
            },
            collection_id: collection_id.clone(),
            workload_pattern: crate::common::tier_policy_engine::WorkloadPattern::ReadHeavy,
            tier_policy: UnifiedTierPolicy {
                eviction_policy: EvictionPolicy::Lru,
                promotion_criteria: PromotionCriteria::AccessFrequency {
                    min_accesses: config.posting_list_config.promotion_threshold,
                    time_window: Duration::from_secs(300), // 5 minute window
                },
                demotion_criteria: DemotionCriteria::Age {
                    max_age: Duration::from_secs(config.posting_list_config.demotion_threshold),
                },
                reload_strategy: crate::common::adaptive_structures::ReloadStrategy::LazyLoad,
            },
            memory_limit_mb: Some(config.posting_list_config.max_memory_mb),
            enable_metrics: true,
        };
        
        let posting_lists = Arc::new(AdaptiveStore::new(posting_store_config)?);
        
        // Create distance compute
        let distance_compute = UnifiedDistanceCompute::new(config.distance_metric);
        
        Ok(Self {
            collection_id,
            centroids,
            posting_lists,
            vectors: Arc::new(DashMap::new()),
            distance_compute,
            config,
            vector_count: Arc::new(AtomicUsize::new(0)),
            search_count: Arc::new(AtomicU64::new(0)),
            access_correlations: Arc::new(DashMap::new()),
        })
    }
    
    /// Train the index with sample vectors
    pub fn train(&mut self, training_vectors: Vec<Vec<f32>>) -> Result<()> {
        if self.centroids.is_trained() {
            return Err(anyhow!("Index already trained"));
        }
        
        self.centroids.train(&training_vectors)?;
        
        // Initialize empty posting lists for each cluster
        for cluster_id in 0..self.config.n_clusters {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
            let posting_list = PostingList {
                cluster_id,
                vector_ids: Vec::new(),
                vectors: Some(Vec::new()), // Start in memory
                last_access: 0,
                access_count: 0,
            };
            
            tokio::runtime::Handle::current().block_on(async {
                self.posting_lists.insert(key, posting_list).await
            })?;
        }
        
        Ok(())
    }
    
    /// Add a vector to the index
    pub async fn add_vector(
        &self,
        id: String,
        vector: Vec<f32>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<()> {
        if !self.centroids.is_trained() {
            return Err(anyhow!("Index must be trained before adding vectors"));
        }
        
        // Find nearest centroid
        let cluster_id = self.centroids.find_nearest_centroid(&vector, &self.distance_compute);
        
        // Update posting list
        let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
        
        // Get or create posting list
        let mut posting_list = self.posting_lists
            .get(&key)
            .await?
            .unwrap_or_else(|| PostingList {
                cluster_id,
                vector_ids: Vec::new(),
                vectors: Some(Vec::new()),
                last_access: 0,
                access_count: 0,
            });
        
        // Add vector ID to posting list
        posting_list.vector_ids.push(id.clone());
        
        // If vectors are stored in posting list (for small clusters)
        if let Some(ref mut vectors) = posting_list.vectors {
            if vectors.len() < 1000 { // Keep small clusters in posting list
                vectors.push(vector.clone());
            } else {
                // Large clusters: store vectors separately
                posting_list.vectors = None;
            }
        }
        
        // Update posting list
        self.posting_lists.insert(key, posting_list).await?;
        
        // Store vector separately (for efficient random access)
        let vector_key = PartitionedKey::new(self.collection_id.clone(), id.clone());
        let vector_record = Arc::new(VectorRecord {
            id: Some(id),
            vector,
            metadata: metadata.unwrap_or_default(),
            version: 0,
        });
        self.vectors.insert(vector_key, vector_record);
        
        // Update statistics
        self.vector_count.fetch_add(1, Ordering::Relaxed);
        self.centroids.cluster_sizes[cluster_id].fetch_add(1, Ordering::Relaxed);
        
        Ok(())
    }
    
    /// Search for nearest neighbors
    pub async fn search(
        &self,
        query: &[f32],
        k: usize,
        n_probe: Option<usize>,
    ) -> Result<Vec<(String, f32)>> {
        if !self.centroids.is_trained() {
            return Err(anyhow!("Index must be trained before searching"));
        }
        
        let n_probe = n_probe.unwrap_or(self.config.n_probe);
        self.search_count.fetch_add(1, Ordering::Relaxed);
        
        // Step 1: Find nearest centroids (always in memory - fast)
        let nearest_clusters = self.centroids.find_nearest_centroids(query, n_probe, &self.distance_compute);
        
        // Step 2: Record access pattern for correlation learning
        self.record_access_pattern(&nearest_clusters).await;
        
        // Step 3: Predictive prefetch if enabled
        if self.config.posting_list_config.enable_prefetch {
            self.prefetch_correlated_clusters(&nearest_clusters).await;
        }
        
        // Step 4: Search posting lists (may trigger tier promotion)
        let mut candidates = Vec::new();
        
        for (cluster_id, _centroid_dist) in nearest_clusters {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
            
            // This access may promote the posting list to memory
            if let Ok(Some(posting_list)) = self.posting_lists.get(&key).await {
                // Search within posting list
                for vector_id in &posting_list.vector_ids {
                    let vector_key = PartitionedKey::new(self.collection_id.clone(), vector_id.clone());
                    
                    if let Some(entry) = self.vectors.get(&vector_key) {
                        let distance = self.distance_compute.distance(query, &entry.vector);
                        candidates.push((vector_id.clone(), distance));
                    }
                }
            }
        }
        
        // Step 5: Sort and return top-k
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        candidates.truncate(k);
        
        Ok(candidates)
    }
    
    /// Record access pattern for correlation learning
    async fn record_access_pattern(&self, clusters: &[(usize, f32)]) {
        if clusters.len() < 2 {
            return;
        }
        
        // Update correlation matrix
        for i in 0..clusters.len() {
            for j in i+1..clusters.len() {
                let cluster_i = clusters[i].0;
                let cluster_j = clusters[j].0;
                
                // Update correlation score
                self.access_correlations
                    .entry(cluster_i)
                    .or_insert_with(Vec::new)
                    .push((cluster_j, 0.9)); // Decay over time
                
                self.access_correlations
                    .entry(cluster_j)
                    .or_insert_with(Vec::new)
                    .push((cluster_i, 0.9));
            }
        }
    }
    
    /// Prefetch correlated clusters
    async fn prefetch_correlated_clusters(&self, clusters: &[(usize, f32)]) {
        for (cluster_id, _) in clusters {
            if let Some(correlations) = self.access_correlations.get(cluster_id) {
                for (corr_cluster, score) in correlations.value() {
                    if *score > 0.7 { // High correlation threshold
                        let key = PartitionedKey::new(self.collection_id.clone(), *corr_cluster);
                        
                        // Trigger async prefetch
                        let store = self.posting_lists.clone();
                        tokio::spawn(async move {
                            let _ = store.get(&key).await;
                        });
                    }
                }
            }
        }
    }
    
    /// Get index statistics
    pub fn stats(&self) -> IvfStats {
        IvfStats {
            collection_id: self.collection_id.clone(),
            vector_count: self.vector_count.load(Ordering::Relaxed),
            cluster_count: self.config.n_clusters,
            trained: self.centroids.is_trained(),
            search_count: self.search_count.load(Ordering::Relaxed),
            centroid_memory_bytes: self.centroids.memory_usage_bytes(),
            posting_list_memory_bytes: 0, // Would query AdaptiveStore
            total_memory_bytes: self.centroids.memory_usage_bytes(),
        }
    }
    
    /// Clear all data for this collection
    pub async fn clear_collection(&self) -> Result<()> {
        // Clear posting lists
        for cluster_id in 0..self.config.n_clusters {
            let key = PartitionedKey::new(self.collection_id.clone(), cluster_id);
            self.posting_lists.remove(&key).await?;
        }
        
        // Clear vectors
        self.vectors.clear();
        
        // Reset counters
        self.vector_count.store(0, Ordering::Relaxed);
        
        info!("Cleared all data for collection '{}'", self.collection_id);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IvfStats {
    pub collection_id: String,
    pub vector_count: usize,
    pub cluster_count: usize,
    pub trained: bool,
    pub search_count: u64,
    pub centroid_memory_bytes: usize,
    pub posting_list_memory_bytes: usize,
    pub total_memory_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_unified_ivf_basic() {
        let config = UnifiedIvfConfig {
            n_clusters: 10,
            n_probe: 2,
            dimension: 4,
            distance_metric: DistanceMetric::Euclidean,
            centroid_config: CentroidConfig::default(),
            posting_list_config: PostingListConfig::default(),
        };
        
        let mut index = UnifiedIvfIndex::new("test_collection".to_string(), config).unwrap();
        
        // Train with sample vectors
        let training_vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0, 1.0],
        ];
        
        index.train(training_vectors).unwrap();
        
        // Add vectors
        index.add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0, 0.0], None).await.unwrap();
        index.add_vector("vec2".to_string(), vec![0.0, 1.0, 0.0, 0.0], None).await.unwrap();
        
        // Search
        let results = index.search(&[1.0, 0.0, 0.0, 0.0], 2, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "vec1");
    }
    
    #[test]
    fn test_partitioned_key() {
        let key1 = PartitionedKey::new("collection1".to_string(), 42);
        let key2 = PartitionedKey::new("collection2".to_string(), 42);
        
        assert_ne!(key1, key2); // Different collections
        
        let key3 = PartitionedKey::new("collection1".to_string(), 43);
        assert_ne!(key1, key3); // Different keys
        
        let key4 = PartitionedKey::new("collection1".to_string(), 42);
        assert_eq!(key1, key4); // Same collection and key
    }
}