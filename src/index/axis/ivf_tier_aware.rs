/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Tier-aware IVF index implementation that integrates with shared infrastructure
//!
//! This module extends the basic IVF index with tier-aware posting list management,
//! allowing hot clusters to be promoted to memory while cold clusters remain on disk.

use anyhow::Result;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

use crate::common::adaptive_structures::{
    AdaptiveStore, AdaptiveStoreConfig, BackendType, StorageTier,
    UnifiedTierPolicy, EvictionPolicy, PromotionCriteria, DemotionCriteria,
};
use crate::common::tier_policy_engine::WorkloadPattern;
use crate::compute::distance_computation::DistanceMetric;
use crate::index::axis::ivf_index::{AxisIvfConfig, AxisIvfIndex};

/// Access statistics for a posting list
#[derive(Debug, Clone)]
pub struct PostingListStats {
    pub cluster_id: usize,
    pub access_count: u64,
    pub last_access: Instant,
    pub size_bytes: usize,
    pub tier: StorageTier,
}

/// Tier-aware posting list that can be promoted/demoted
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredPostingList {
    pub cluster_id: usize,
    pub vector_ids: Vec<String>,
    pub vectors: Option<Vec<Vec<f32>>>, // None if on disk
    pub tier_hint: StorageTier,
}

/// Configuration for tier-aware IVF
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredIvfConfig {
    pub base_config: AxisIvfConfig,
    pub hot_cluster_threshold: usize,  // Access count to consider "hot"
    pub promotion_interval: Duration,   // How often to check for promotion
    pub max_memory_clusters: usize,     // Max clusters to keep in memory
    pub enable_predictive_prefetch: bool,
}

/// Tier-aware IVF index with adaptive posting list management
pub struct TierAwareIvfIndex {
    /// Base IVF index
    base_index: AxisIvfIndex,
    
    /// Adaptive store for posting lists
    posting_list_store: Arc<AdaptiveStore<usize, TieredPostingList>>,
    
    /// Access statistics per cluster
    cluster_stats: Arc<DashMap<usize, PostingListStats>>,
    
    /// Global access counter for LRU
    global_access_counter: Arc<AtomicU64>,
    
    /// Configuration
    config: TieredIvfConfig,
    
    /// Last promotion check time
    last_promotion_check: Arc<parking_lot::Mutex<Instant>>,
}

impl TierAwareIvfIndex {
    pub fn new(config: TieredIvfConfig, dimension: usize) -> Result<Self> {
        // Create base IVF index
        let base_index = AxisIvfIndex::new(config.base_config.clone(), dimension);
        
        // Configure adaptive store for posting lists
        let store_config = AdaptiveStoreConfig {
            backend_type: BackendType::Index {
                algorithm: "ivf".to_string(),
                expected_qps: 1000,
            },
            collection_id: "ivf_posting_lists".to_string(),
            workload_pattern: WorkloadPattern::ReadHeavy,
            tier_policy: UnifiedTierPolicy {
                eviction_policy: EvictionPolicy::Lru,
                promotion_criteria: PromotionCriteria::AccessFrequency {
                    min_accesses: config.hot_cluster_threshold,
                    time_window: config.promotion_interval,
                },
                demotion_criteria: DemotionCriteria::Age {
                    max_age: Duration::from_secs(3600), // 1 hour
                },
                reload_strategy: crate::common::adaptive_structures::ReloadStrategy::LazyLoad,
            },
            memory_limit_mb: Some(1024), // 1GB for posting lists
            enable_metrics: true,
        };
        
        let posting_list_store = Arc::new(AdaptiveStore::new(store_config)?);
        
        Ok(Self {
            base_index,
            posting_list_store,
            cluster_stats: Arc::new(DashMap::new()),
            global_access_counter: Arc::new(AtomicU64::new(0)),
            config,
            last_promotion_check: Arc::new(parking_lot::Mutex::new(Instant::now())),
        })
    }
    
    /// Record access to a cluster for tier management
    fn record_cluster_access(&self, cluster_id: usize) {
        let access_num = self.global_access_counter.fetch_add(1, Ordering::Relaxed);
        
        self.cluster_stats
            .entry(cluster_id)
            .and_modify(|stats| {
                stats.access_count += 1;
                stats.last_access = Instant::now();
            })
            .or_insert_with(|| PostingListStats {
                cluster_id,
                access_count: 1,
                last_access: Instant::now(),
                size_bytes: 0,
                tier: StorageTier::Memory,
            });
        
        // Check if we should run promotion/demotion
        let mut last_check = self.last_promotion_check.lock();
        if last_check.elapsed() > self.config.promotion_interval {
            *last_check = Instant::now();
            drop(last_check); // Release lock before async operation
            
            // Trigger async tier rebalancing
            let stats = self.cluster_stats.clone();
            let store = self.posting_list_store.clone();
            let max_memory = self.config.max_memory_clusters;
            
            tokio::spawn(async move {
                Self::rebalance_tiers(stats, store, max_memory).await;
            });
        }
    }
    
    /// Rebalance clusters between tiers based on access patterns
    async fn rebalance_tiers(
        stats: Arc<DashMap<usize, PostingListStats>>,
        store: Arc<AdaptiveStore<usize, TieredPostingList>>,
        max_memory_clusters: usize,
    ) {
        // Collect all cluster stats
        let mut cluster_stats: Vec<PostingListStats> = stats
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        
        // Sort by access count (hot to cold)
        cluster_stats.sort_by(|a, b| b.access_count.cmp(&a.access_count));
        
        // Promote hot clusters to memory
        for (idx, stat) in cluster_stats.iter().enumerate() {
            if idx < max_memory_clusters {
                // This cluster should be in memory
                if !matches!(stat.tier, StorageTier::Memory) {
                    debug!("Promoting cluster {} to memory (access_count: {})", 
                           stat.cluster_id, stat.access_count);
                    
                    // The AdaptiveStore handles the actual promotion
                    if let Ok(Some(_)) = store.get(&stat.cluster_id).await {
                        // Access triggers promotion in AdaptiveStore
                    }
                }
            } else {
                // This cluster can be demoted
                if matches!(stat.tier, StorageTier::Memory) {
                    debug!("Demoting cluster {} from memory (access_count: {})",
                           stat.cluster_id, stat.access_count);
                    
                    // AdaptiveStore handles demotion based on policy
                    store.mark_for_demotion(&stat.cluster_id).await;
                }
            }
        }
    }
    
    /// Search with tier-aware posting list loading
    pub async fn search_tiered(
        &self,
        query: &[f32],
        k: usize,
        n_probe: Option<usize>,
    ) -> Result<Vec<(String, f32)>> {
        let n_probe = n_probe.unwrap_or(self.config.base_config.n_probe);
        
        // Step 1: Find nearest centroids (always in memory)
        let nearest_clusters = self.find_nearest_centroids(query, n_probe)?;
        
        // Step 2: Predictive prefetch if enabled
        if self.config.enable_predictive_prefetch {
            self.prefetch_correlated_clusters(&nearest_clusters).await;
        }
        
        // Step 3: Load and search posting lists (tier-aware)
        let mut all_candidates = Vec::new();
        
        for cluster_id in nearest_clusters {
            // Record access for tier management
            self.record_cluster_access(cluster_id);
            
            // Load posting list (from memory or disk via AdaptiveStore)
            match self.posting_list_store.get(&cluster_id).await {
                Ok(Some(posting_list)) => {
                    // Search within this posting list
                    for vector_id in &posting_list.vector_ids {
                        if let Some(vector_record) = self.base_index.get_vector(vector_id) {
                            let distance = self.base_index.compute_distance(query, &vector_record.vector);
                            all_candidates.push((vector_id.clone(), distance));
                        }
                    }
                }
                Ok(None) => {
                    debug!("Posting list {} not found, may need rebuild", cluster_id);
                }
                Err(e) => {
                    debug!("Error loading posting list {}: {}", cluster_id, e);
                }
            }
        }
        
        // Step 4: Sort and return top-k
        all_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_candidates.truncate(k);
        
        Ok(all_candidates)
    }
    
    /// Find nearest centroids to query vector
    fn find_nearest_centroids(&self, query: &[f32], n_probe: usize) -> Result<Vec<usize>> {
        let mut distances: Vec<(usize, f32)> = self.base_index
            .get_centroids()
            .iter()
            .enumerate()
            .map(|(idx, centroid)| {
                let dist = self.base_index.compute_distance(query, centroid);
                (idx, dist)
            })
            .collect();
        
        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        distances.truncate(n_probe);
        
        Ok(distances.into_iter().map(|(idx, _)| idx).collect())
    }
    
    /// Prefetch correlated clusters based on access patterns
    async fn prefetch_correlated_clusters(&self, clusters: &[usize]) {
        // This would use the AccessPatternTracker from orchestrator
        // to predict which other clusters are likely to be accessed
        
        for &cluster_id in clusters {
            // Check correlation matrix for related clusters
            let correlated = self.get_correlated_clusters(cluster_id);
            
            for corr_cluster in correlated {
                // Trigger background load (non-blocking)
                let store = self.posting_list_store.clone();
                tokio::spawn(async move {
                    let _ = store.get(&corr_cluster).await;
                });
            }
        }
    }
    
    /// Get clusters that are frequently accessed together
    fn get_correlated_clusters(&self, cluster_id: usize) -> Vec<usize> {
        // This would integrate with AccessPatternTracker
        // For now, return adjacent clusters as a heuristic
        let mut correlated = Vec::new();
        
        if cluster_id > 0 {
            correlated.push(cluster_id - 1);
        }
        if cluster_id + 1 < self.config.base_config.n_clusters {
            correlated.push(cluster_id + 1);
        }
        
        correlated
    }
    
    /// Add a vector and update tier statistics
    pub async fn add_vector_tiered(
        &mut self,
        id: String,
        vector: Vec<f32>,
    ) -> Result<()> {
        // Find nearest centroid
        let cluster_id = self.find_nearest_centroids(&vector, 1)?[0];
        
        // Update posting list in adaptive store
        let mut posting_list = self.posting_list_store
            .get(&cluster_id)
            .await?
            .unwrap_or_else(|| TieredPostingList {
                cluster_id,
                vector_ids: Vec::new(),
                vectors: None,
                tier_hint: StorageTier::Memory,
            });
        
        posting_list.vector_ids.push(id.clone());
        
        // Store back with updated posting list
        self.posting_list_store.insert(cluster_id, posting_list).await?;
        
        // Update base index
        self.base_index.add_vector(id, vector, None)?;
        
        Ok(())
    }
    
    /// Get current tier distribution statistics
    pub fn get_tier_distribution(&self) -> HashMap<StorageTier, usize> {
        use std::collections::HashMap;
        
        let mut distribution = HashMap::new();
        
        for entry in self.cluster_stats.iter() {
            *distribution.entry(entry.tier.clone()).or_insert(0) += 1;
        }
        
        distribution
    }
    
    /// Force promotion of specific clusters (for testing/debugging)
    pub async fn force_promote_clusters(&self, cluster_ids: &[usize]) -> Result<()> {
        for &cluster_id in cluster_ids {
            if let Ok(Some(posting_list)) = self.posting_list_store.get(&cluster_id).await {
                // Access will trigger promotion based on policy
                info!("Force promoted cluster {} to memory tier", cluster_id);
            }
        }
        Ok(())
    }
}

/// Extension trait for base IVF index
impl AxisIvfIndex {
    /// Get vector by ID (helper method)
    pub fn get_vector(&self, id: &str) -> Option<Arc<VectorRecord>> {
        self.vectors.get(id).map(|entry| entry.value().clone())
    }
    
    /// Compute distance between two vectors
    pub fn compute_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        self.distance_compute.distance(a, b)
    }
    
    /// Get centroids
    pub fn get_centroids(&self) -> &[Vec<f32>] {
        &self.centroids
    }
}