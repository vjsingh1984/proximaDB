// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Progressive Search Engine
//! 
//! Storage-aware search implementation that progressively searches across tiers
//! with ML-guided cluster pruning and feature selection optimizations.

use std::collections::{HashMap, HashSet, BinaryHeap};
use std::cmp::Ordering;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::Duration;
use anyhow::Result;

use crate::core::CollectionId;
use super::types::*;
use super::{ViperConfig, TierLevel};

/// Progressive search engine for VIPER storage
pub struct ViperProgressiveSearchEngine {
    /// Configuration
    config: ViperConfig,
    
    /// Tier searchers optimized for each storage tier
    tier_searchers: Arc<RwLock<HashMap<TierLevel, Box<dyn TierSearcher>>>>,
    
    /// Search concurrency limiter
    search_semaphore: Arc<Semaphore>,
    
    /// Feature selection cache
    feature_cache: Arc<RwLock<FeatureSelectionCache>>,
    
    /// Search statistics
    stats: Arc<RwLock<SearchStats>>,
}

/// Trait for tier-specific search implementations
#[async_trait::async_trait]
pub trait TierSearcher: Send + Sync {
    /// Search within a specific tier
    async fn search_tier(
        &self,
        context: &ViperSearchContext,
        partitions: Vec<PartitionId>,
        important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>>;
    
    /// Get tier characteristics for optimization
    fn tier_characteristics(&self) -> TierCharacteristics;
}

/// Tier characteristics for search optimization
#[derive(Debug, Clone)]
pub struct TierCharacteristics {
    pub tier_level: TierLevel,
    pub avg_latency_ms: f32,
    pub throughput_mbps: f32,
    pub memory_resident: bool,
    pub supports_random_access: bool,
    pub compression_overhead: f32,
}

/// Feature selection cache for search optimization
#[derive(Debug)]
pub struct FeatureSelectionCache {
    /// Collection -> important features mapping
    feature_sets: HashMap<CollectionId, CachedFeatureSet>,
    
    /// Maximum cache size
    max_cache_size: usize,
    
    /// LRU tracking
    access_order: Vec<CollectionId>,
}

/// Cached feature set with metadata
#[derive(Debug, Clone)]
pub struct CachedFeatureSet {
    pub features: Vec<usize>,
    pub importance_scores: Vec<f32>,
    pub last_updated: chrono::DateTime<chrono::Utc>,
    pub hit_count: u64,
}

/// Search statistics tracking
#[derive(Debug, Default)]
pub struct SearchStats {
    pub total_searches: u64,
    pub tier_hits: HashMap<TierLevel, u64>,
    pub avg_latency_ms: f32,
    pub cluster_pruning_ratio: f32,
    pub feature_reduction_ratio: f32,
}

/// Search result with score for priority queue
#[derive(Debug, Clone)]
struct ScoredResult {
    result: ViperSearchResult,
    score: f32,
}

impl Eq for ScoredResult {}

impl PartialEq for ScoredResult {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.result.vector_id == other.result.vector_id
    }
}

impl Ord for ScoredResult {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse order for max heap (highest scores first)
        other.score.partial_cmp(&self.score).unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for ScoredResult {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl ViperProgressiveSearchEngine {
    /// Create new progressive search engine
    pub async fn new(config: ViperConfig) -> Result<Self> {
        let max_concurrent_searches = config.performance_config.parallel_config.io_thread_count;
        
        let mut tier_searchers: HashMap<TierLevel, Box<dyn TierSearcher>> = HashMap::new();
        
        // Initialize tier-specific searchers
        for (_, tier_def) in &config.tier_config.tiers {
            let searcher: Box<dyn TierSearcher> = match tier_def.level {
                TierLevel::UltraHot => {
                    Box::new(UltraHotTierSearcher::new(tier_def.clone()))
                }
                TierLevel::Hot => {
                    Box::new(HotTierSearcher::new(tier_def.clone()))
                }
                TierLevel::Warm => {
                    Box::new(WarmTierSearcher::new(tier_def.clone()))
                }
                TierLevel::Cold => {
                    Box::new(ColdTierSearcher::new(tier_def.clone()))
                }
                TierLevel::Archive => {
                    Box::new(ArchiveTierSearcher::new(tier_def.clone()))
                }
            };
            tier_searchers.insert(tier_def.level.clone(), searcher);
        }
        
        Ok(Self {
            config,
            tier_searchers: Arc::new(RwLock::new(tier_searchers)),
            search_semaphore: Arc::new(Semaphore::new(max_concurrent_searches)),
            feature_cache: Arc::new(RwLock::new(FeatureSelectionCache::new(1000))),
            stats: Arc::new(RwLock::new(SearchStats::default())),
        })
    }
    
    /// Execute progressive search across storage tiers
    pub async fn search(
        &self,
        context: ViperSearchContext,
        cluster_hints: Option<Vec<ClusterId>>,
        important_features: Vec<usize>,
    ) -> Result<Vec<ViperSearchResult>> {
        let _permit = self.search_semaphore.acquire().await?;
        let start_time = std::time::Instant::now();
        
        // Update feature cache
        self.update_feature_cache(&context.collection_id, &important_features).await?;
        
        let results = match context.search_strategy.clone() {
            SearchStrategy::Progressive { tier_result_thresholds } => {
                self.progressive_tier_search(
                    context,
                    cluster_hints,
                    important_features,
                    tier_result_thresholds,
                ).await?
            }
            
            SearchStrategy::ClusterPruned { max_clusters, confidence_threshold } => {
                self.cluster_pruned_search(
                    context,
                    cluster_hints,
                    important_features,
                    max_clusters,
                    confidence_threshold,
                ).await?
            }
            
            SearchStrategy::Exhaustive => {
                self.exhaustive_search(context, cluster_hints, important_features).await?
            }
            
            SearchStrategy::Adaptive { query_complexity_score, time_budget_ms } => {
                self.adaptive_search(
                    context,
                    cluster_hints,
                    important_features,
                    query_complexity_score,
                    time_budget_ms,
                ).await?
            }
        };
        
        // Update statistics
        let elapsed_ms = start_time.elapsed().as_millis() as f32;
        self.update_search_stats(elapsed_ms, &results).await?;
        
        Ok(results)
    }
    
    /// Progressive search across tiers with early stopping
    async fn progressive_tier_search(
        &self,
        context: ViperSearchContext,
        cluster_hints: Option<Vec<ClusterId>>,
        important_features: Vec<usize>,
        tier_thresholds: Vec<usize>,
    ) -> Result<Vec<ViperSearchResult>> {
        let mut result_heap = BinaryHeap::new();
        let mut searched_vectors = HashSet::new();
        
        let tier_order = vec![
            TierLevel::UltraHot,
            TierLevel::Hot,
            TierLevel::Warm,
            TierLevel::Cold,
            TierLevel::Archive,
        ];
        
        for (tier_idx, tier_level) in tier_order.iter().enumerate() {
            if let Some(max_tiers) = context.max_tiers {
                if tier_idx >= max_tiers {
                    break;
                }
            }
            
            // Get partitions for this tier
            let partitions = self.get_tier_partitions(
                &context.collection_id,
                tier_level,
                &cluster_hints,
            ).await?;
            
            if partitions.is_empty() {
                continue;
            }
            
            // Search this tier
            let tier_results = self.search_single_tier(
                &context,
                tier_level,
                partitions,
                &important_features,
            ).await?;
            
            // Merge results
            for result in tier_results {
                if !searched_vectors.contains(&result.vector_id) {
                    searched_vectors.insert(result.vector_id.clone());
                    result_heap.push(ScoredResult {
                        score: result.score,
                        result,
                    });
                }
            }
            
            // Check if we have enough results for this tier
            if tier_idx < tier_thresholds.len() && result_heap.len() >= tier_thresholds[tier_idx] {
                break; // Early stopping
            }
        }
        
        // Extract top-k results
        let mut results = Vec::new();
        for _ in 0..context.k {
            if let Some(scored) = result_heap.pop() {
                results.push(scored.result);
            } else {
                break;
            }
        }
        
        Ok(results)
    }
    
    /// Cluster-pruned search using ML predictions
    async fn cluster_pruned_search(
        &self,
        context: ViperSearchContext,
        cluster_hints: Option<Vec<ClusterId>>,
        important_features: Vec<usize>,
        max_clusters: usize,
        confidence_threshold: f32,
    ) -> Result<Vec<ViperSearchResult>> {
        // Get cluster predictions if not provided
        let clusters = if let Some(hints) = cluster_hints {
            hints
        } else {
            self.predict_search_clusters(&context, confidence_threshold).await?
        };
        
        // Limit to max clusters
        let search_clusters: Vec<ClusterId> = clusters.into_iter()
            .take(max_clusters)
            .collect();
        
        // Search across all tiers but only in selected clusters
        let mut all_results = Vec::new();
        
        let tier_searchers = self.tier_searchers.read().await;
        for (tier_level, searcher) in tier_searchers.iter() {
            let partitions = self.get_cluster_partitions(
                &context.collection_id,
                tier_level,
                &search_clusters,
            ).await?;
            
            if !partitions.is_empty() {
                let tier_results = searcher.search_tier(
                    &context,
                    partitions,
                    &important_features,
                ).await?;
                
                all_results.extend(tier_results);
            }
        }
        
        // Sort and take top-k
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        all_results.truncate(context.k);
        
        Ok(all_results)
    }
    
    /// Exhaustive search across all tiers and partitions
    async fn exhaustive_search(
        &self,
        context: ViperSearchContext,
        _cluster_hints: Option<Vec<ClusterId>>,
        important_features: Vec<usize>,
    ) -> Result<Vec<ViperSearchResult>> {
        let mut all_results = Vec::new();
        
        // Search all tiers in parallel
        let tier_searchers = self.tier_searchers.read().await;
        // Process tiers sequentially for now (could be parallelized)
        for (tier_level, searcher) in tier_searchers.iter() {
            let partitions = self.get_all_partitions(
                &context.collection_id,
                tier_level,
            ).await?;
            
            if partitions.is_empty() {
                continue;
            }
            
            match searcher.search_tier(&context, partitions, &important_features).await {
                Ok(tier_results) => all_results.extend(tier_results),
                Err(e) => eprintln!("Tier search error: {}", e),
            }
        }
        
        // Sort and take top-k
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        all_results.truncate(context.k);
        
        Ok(all_results)
    }
    
    /// Adaptive search based on query complexity and time budget
    async fn adaptive_search(
        &self,
        context: ViperSearchContext,
        cluster_hints: Option<Vec<ClusterId>>,
        important_features: Vec<usize>,
        query_complexity: f32,
        time_budget_ms: u64,
    ) -> Result<Vec<ViperSearchResult>> {
        let _deadline = Duration::from_millis(time_budget_ms);
        
        // Determine strategy based on complexity
        let strategy = if query_complexity < 0.3 {
            // Simple query - aggressive pruning
            SearchStrategy::ClusterPruned {
                max_clusters: 3,
                confidence_threshold: 0.8,
            }
        } else if query_complexity < 0.7 {
            // Medium complexity - progressive search
            SearchStrategy::Progressive {
                tier_result_thresholds: vec![
                    context.k * 2,  // Ultra-hot
                    context.k * 3,  // Hot
                    context.k * 5,  // Warm
                ],
            }
        } else {
            // Complex query - more exhaustive
            SearchStrategy::ClusterPruned {
                max_clusters: 10,
                confidence_threshold: 0.5,
            }
        };
        
        // Execute search with timeout (simplified to avoid recursion)
        let mut search_context = context;
        search_context.search_strategy = strategy;
        
        // Use the specific search strategy directly to avoid recursion
        let strategy_clone = search_context.search_strategy.clone();
        match strategy_clone {
            SearchStrategy::ClusterPruned { max_clusters, confidence_threshold } => {
                self.cluster_pruned_search(
                    search_context,
                    cluster_hints,
                    important_features,
                    max_clusters,
                    confidence_threshold,
                ).await
            }
            SearchStrategy::Progressive { tier_result_thresholds } => {
                self.progressive_tier_search(
                    search_context,
                    cluster_hints,
                    important_features,
                    tier_result_thresholds,
                ).await
            }
            _ => {
                // Fallback to cluster pruned search
                self.cluster_pruned_search(
                    search_context,
                    cluster_hints,
                    important_features,
                    5,
                    0.5,
                ).await
            }
        }
    }
    
    /// Search a single tier
    async fn search_single_tier(
        &self,
        context: &ViperSearchContext,
        tier_level: &TierLevel,
        partitions: Vec<PartitionId>,
        important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        let tier_searchers = self.tier_searchers.read().await;
        
        if let Some(searcher) = tier_searchers.get(tier_level) {
            searcher.search_tier(context, partitions, important_features).await
        } else {
            Ok(Vec::new())
        }
    }
    
    /// Update feature cache
    async fn update_feature_cache(
        &self,
        collection_id: &CollectionId,
        features: &[usize],
    ) -> Result<()> {
        let mut cache = self.feature_cache.write().await;
        cache.update(collection_id.clone(), features.to_vec());
        Ok(())
    }
    
    /// Update search statistics
    async fn update_search_stats(
        &self,
        elapsed_ms: f32,
        results: &[ViperSearchResult],
    ) -> Result<()> {
        let mut stats = self.stats.write().await;
        stats.total_searches += 1;
        
        // Update average latency (exponential moving average)
        let alpha = 0.1;
        stats.avg_latency_ms = stats.avg_latency_ms * (1.0 - alpha) + elapsed_ms * alpha;
        
        // Update tier hits
        for result in results {
            *stats.tier_hits.entry(result.tier_level.clone()).or_insert(0) += 1;
        }
        
        Ok(())
    }
    
    // Placeholder methods for partition management
    async fn get_tier_partitions(
        &self,
        _collection_id: &CollectionId,
        _tier_level: &TierLevel,
        _cluster_hints: &Option<Vec<ClusterId>>,
    ) -> Result<Vec<PartitionId>> {
        Ok(Vec::new())
    }
    
    async fn get_cluster_partitions(
        &self,
        _collection_id: &CollectionId,
        _tier_level: &TierLevel,
        _clusters: &[ClusterId],
    ) -> Result<Vec<PartitionId>> {
        Ok(Vec::new())
    }
    
    async fn get_all_partitions(
        &self,
        _collection_id: &CollectionId,
        _tier_level: &TierLevel,
    ) -> Result<Vec<PartitionId>> {
        Ok(Vec::new())
    }
    
    async fn predict_search_clusters(
        &self,
        _context: &ViperSearchContext,
        _confidence_threshold: f32,
    ) -> Result<Vec<ClusterId>> {
        Ok(Vec::new())
    }
}

/// Ultra-hot tier searcher (MMAP + OS cache)
struct UltraHotTierSearcher {
    tier_def: super::TierDefinition,
}

impl UltraHotTierSearcher {
    fn new(tier_def: super::TierDefinition) -> Self {
        Self { tier_def }
    }
}

#[async_trait::async_trait]
impl TierSearcher for UltraHotTierSearcher {
    async fn search_tier(
        &self,
        _context: &ViperSearchContext,
        _partitions: Vec<PartitionId>,
        _important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        // Ultra-hot tier: Direct memory access, no decompression needed
        // Implementation would use memory-mapped Parquet files
        Ok(Vec::new())
    }
    
    fn tier_characteristics(&self) -> TierCharacteristics {
        TierCharacteristics {
            tier_level: TierLevel::UltraHot,
            avg_latency_ms: 0.1,
            throughput_mbps: 10000.0,
            memory_resident: true,
            supports_random_access: true,
            compression_overhead: 0.0,
        }
    }
}

/// Hot tier searcher (Local SSD)
struct HotTierSearcher {
    tier_def: super::TierDefinition,
}

impl HotTierSearcher {
    fn new(tier_def: super::TierDefinition) -> Self {
        Self { tier_def }
    }
}

#[async_trait::async_trait]
impl TierSearcher for HotTierSearcher {
    async fn search_tier(
        &self,
        _context: &ViperSearchContext,
        _partitions: Vec<PartitionId>,
        _important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        // Hot tier: SSD-optimized search with LZ4 decompression
        Ok(Vec::new())
    }
    
    fn tier_characteristics(&self) -> TierCharacteristics {
        TierCharacteristics {
            tier_level: TierLevel::Hot,
            avg_latency_ms: 1.0,
            throughput_mbps: 5000.0,
            memory_resident: false,
            supports_random_access: true,
            compression_overhead: 0.1,
        }
    }
}

/// Warm tier searcher (Local HDD)
struct WarmTierSearcher {
    tier_def: super::TierDefinition,
}

impl WarmTierSearcher {
    fn new(tier_def: super::TierDefinition) -> Self {
        Self { tier_def }
    }
}

#[async_trait::async_trait]
impl TierSearcher for WarmTierSearcher {
    async fn search_tier(
        &self,
        _context: &ViperSearchContext,
        _partitions: Vec<PartitionId>,
        _important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        // Warm tier: Sequential access optimized with Zstd decompression
        Ok(Vec::new())
    }
    
    fn tier_characteristics(&self) -> TierCharacteristics {
        TierCharacteristics {
            tier_level: TierLevel::Warm,
            avg_latency_ms: 10.0,
            throughput_mbps: 200.0,
            memory_resident: false,
            supports_random_access: false,
            compression_overhead: 0.3,
        }
    }
}

/// Cold tier searcher (Object storage)
struct ColdTierSearcher {
    tier_def: super::TierDefinition,
}

impl ColdTierSearcher {
    fn new(tier_def: super::TierDefinition) -> Self {
        Self { tier_def }
    }
}

#[async_trait::async_trait]
impl TierSearcher for ColdTierSearcher {
    async fn search_tier(
        &self,
        _context: &ViperSearchContext,
        _partitions: Vec<PartitionId>,
        _important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        // Cold tier: Network-optimized batch fetching
        Ok(Vec::new())
    }
    
    fn tier_characteristics(&self) -> TierCharacteristics {
        TierCharacteristics {
            tier_level: TierLevel::Cold,
            avg_latency_ms: 100.0,
            throughput_mbps: 100.0,
            memory_resident: false,
            supports_random_access: false,
            compression_overhead: 0.5,
        }
    }
}

/// Archive tier searcher (Deep archive)
struct ArchiveTierSearcher {
    tier_def: super::TierDefinition,
}

impl ArchiveTierSearcher {
    fn new(tier_def: super::TierDefinition) -> Self {
        Self { tier_def }
    }
}

#[async_trait::async_trait]
impl TierSearcher for ArchiveTierSearcher {
    async fn search_tier(
        &self,
        _context: &ViperSearchContext,
        _partitions: Vec<PartitionId>,
        _important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        // Archive tier: High-latency retrieval
        Ok(Vec::new())
    }
    
    fn tier_characteristics(&self) -> TierCharacteristics {
        TierCharacteristics {
            tier_level: TierLevel::Archive,
            avg_latency_ms: 1000.0,
            throughput_mbps: 50.0,
            memory_resident: false,
            supports_random_access: false,
            compression_overhead: 0.7,
        }
    }
}

impl FeatureSelectionCache {
    fn new(max_size: usize) -> Self {
        Self {
            feature_sets: HashMap::new(),
            max_cache_size: max_size,
            access_order: Vec::new(),
        }
    }
    
    fn update(&mut self, collection_id: CollectionId, features: Vec<usize>) {
        // Update access order (LRU)
        self.access_order.retain(|id| id != &collection_id);
        self.access_order.push(collection_id.clone());
        
        // Evict if necessary
        if self.feature_sets.len() >= self.max_cache_size && !self.feature_sets.contains_key(&collection_id) {
            if let Some(oldest) = self.access_order.first() {
                let oldest_id = oldest.clone();
                self.feature_sets.remove(&oldest_id);
                self.access_order.remove(0);
            }
        }
        
        // Update or insert
        let entry = self.feature_sets.entry(collection_id).or_insert(CachedFeatureSet {
            features: features.clone(),
            importance_scores: vec![1.0; features.len()],
            last_updated: chrono::Utc::now(),
            hit_count: 0,
        });
        
        entry.features = features;
        entry.last_updated = chrono::Utc::now();
        entry.hit_count += 1;
    }
}