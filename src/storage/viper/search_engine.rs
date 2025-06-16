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
use super::ViperConfig;
use chrono::{DateTime, Utc};

/// Progressive search engine for VIPER storage
pub struct ViperProgressiveSearchEngine {
    /// Configuration
    config: ViperConfig,
    
    /// Tier searchers optimized for each storage tier
    storage_searchers: Arc<RwLock<HashMap<String, Box<dyn TierSearcher>>>>,
    
    /// Search concurrency limiter
    search_semaphore: Arc<Semaphore>,
    
    /// Feature selection cache
    feature_cache: Arc<RwLock<FeatureSelectionCache>>,
    
    /// Search statistics
    stats: Arc<RwLock<SearchStats>>,
}

/// Trait for storage-specific search implementations
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
    pub storage_url: String,
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
    pub storage_hits: HashMap<String, u64>,
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
        let max_concurrent_searches = 8; // Default value for now
        
        let mut storage_searchers: HashMap<String, Box<dyn TierSearcher>> = HashMap::new();
        
        // Initialize storage-specific searchers based on filesystem configuration
        // For now, use a default set of storage types - this can be made configurable later
        let default_storage_urls = vec![
            "file://hot".to_string(),
            "file://warm".to_string(),
            "s3://cold".to_string(),
        ];
        
        for storage_url in default_storage_urls {
            let searcher: Box<dyn TierSearcher> = if storage_url.starts_with("file://") {
                Box::new(LocalFileSearcher::new(storage_url.clone()))
            } else if storage_url.starts_with("s3://") {
                Box::new(S3Searcher::new(storage_url.clone()))
            } else {
                Box::new(GenericSearcher::new(storage_url.clone()))
            };
            storage_searchers.insert(storage_url, searcher);
        }
        
        Ok(Self {
            config,
            storage_searchers: Arc::new(RwLock::new(storage_searchers)),
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
        
        let storage_order = vec![
            "file://hot".to_string(),
            "file://warm".to_string(),
            "s3://cold".to_string(),
        ];
        
        for (storage_idx, storage_url) in storage_order.iter().enumerate() {
            if let Some(max_storage_locations) = context.max_storage_locations {
                if storage_idx >= max_storage_locations {
                    break;
                }
            }
            
            // Get partitions for this storage location
            let partitions = self.get_storage_partitions(
                &context.collection_id,
                storage_url,
                &cluster_hints,
            ).await?;
            
            if partitions.is_empty() {
                continue;
            }
            
            // Search this storage location
            let storage_results = self.search_single_storage(
                &context,
                storage_url,
                partitions,
                &important_features,
            ).await?;
            
            // Merge results with TTL filtering
            for result in storage_results {
                if !searched_vectors.contains(&result.vector_id) && self.is_vector_valid(&result).await {
                    searched_vectors.insert(result.vector_id.clone());
                    result_heap.push(ScoredResult {
                        score: result.score,
                        result,
                    });
                }
            }
            
            // Check if we have enough results for this storage location
            if storage_idx < tier_thresholds.len() && result_heap.len() >= tier_thresholds[storage_idx] {
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
    
    /// Search a single storage location
    async fn search_single_storage(
        &self,
        context: &ViperSearchContext,
        storage_url: &str,
        partitions: Vec<PartitionId>,
        important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        let storage_searchers = self.storage_searchers.read().await;
        
        if let Some(searcher) = storage_searchers.get(storage_url) {
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
    
    /// Check if a vector is valid (not expired based on TTL)
    async fn is_vector_valid(&self, result: &ViperSearchResult) -> bool {
        // Check if vector has expired based on TTL
        if let Some(expires_at) = self.extract_expires_at_from_metadata(&result.metadata).await {
            let now = Utc::now();
            if now > expires_at {
                tracing::debug!("🕒 Vector {} expired at {}, skipping", result.vector_id, expires_at);
                return false;
            }
        }
        true
    }
    
    /// Extract expires_at timestamp from metadata
    async fn extract_expires_at_from_metadata(&self, metadata: &serde_json::Value) -> Option<DateTime<Utc>> {
        // Try to extract expires_at from metadata if it exists
        if let Some(expires_at_value) = metadata.get("expires_at") {
            if let Some(timestamp_ms) = expires_at_value.as_i64() {
                return DateTime::from_timestamp_millis(timestamp_ms);
            }
        }
        None
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
    async fn get_storage_partitions(
        &self,
        _collection_id: &CollectionId,
        _storage_url: &str,
        _cluster_hints: &Option<Vec<ClusterId>>,
    ) -> Result<Vec<PartitionId>> {
        Ok(Vec::new())
    }
    
    async fn get_cluster_partitions(
        &self,
        _collection_id: &CollectionId,
        _storage_url: &str,
        _clusters: &[ClusterId],
    ) -> Result<Vec<PartitionId>> {
        Ok(Vec::new())
    }
    
    async fn get_all_partitions(
        &self,
        _collection_id: &CollectionId,
        _storage_url: &str,
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

/// Local file storage searcher (file:// URLs)
struct LocalFileSearcher {
    storage_url: String,
}

impl LocalFileSearcher {
    fn new(storage_url: String) -> Self {
        Self { storage_url }
    }
}

#[async_trait::async_trait]
impl TierSearcher for LocalFileSearcher {
    async fn search_tier(
        &self,
        _context: &ViperSearchContext,
        _partitions: Vec<PartitionId>,
        _important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        // Local file storage: Direct file access with optimal caching
        Ok(Vec::new())
    }
    
    fn tier_characteristics(&self) -> TierCharacteristics {
        TierCharacteristics {
            storage_url: self.storage_url.clone(),
            avg_latency_ms: 1.0,
            throughput_mbps: 5000.0,
            memory_resident: false,
            supports_random_access: true,
            compression_overhead: 0.1,
        }
    }
}

/// S3 storage searcher (s3:// URLs)
struct S3Searcher {
    storage_url: String,
}

impl S3Searcher {
    fn new(storage_url: String) -> Self {
        Self { storage_url }
    }
}

#[async_trait::async_trait]
impl TierSearcher for S3Searcher {
    async fn search_tier(
        &self,
        _context: &ViperSearchContext,
        _partitions: Vec<PartitionId>,
        _important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        // S3 storage: Network-optimized batch fetching
        Ok(Vec::new())
    }
    
    fn tier_characteristics(&self) -> TierCharacteristics {
        TierCharacteristics {
            storage_url: self.storage_url.clone(),
            avg_latency_ms: 100.0,
            throughput_mbps: 100.0,
            memory_resident: false,
            supports_random_access: false,
            compression_overhead: 0.5,
        }
    }
}

/// Generic storage searcher for other URLs
struct GenericSearcher {
    storage_url: String,
}

impl GenericSearcher {
    fn new(storage_url: String) -> Self {
        Self { storage_url }
    }
}

#[async_trait::async_trait]
impl TierSearcher for GenericSearcher {
    async fn search_tier(
        &self,
        _context: &ViperSearchContext,
        _partitions: Vec<PartitionId>,
        _important_features: &[usize],
    ) -> Result<Vec<ViperSearchResult>> {
        // Generic storage: Filesystem API-based access
        Ok(Vec::new())
    }
    
    fn tier_characteristics(&self) -> TierCharacteristics {
        TierCharacteristics {
            storage_url: self.storage_url.clone(),
            avg_latency_ms: 50.0,
            throughput_mbps: 500.0,
            memory_resident: false,
            supports_random_access: true,
            compression_overhead: 0.2,
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