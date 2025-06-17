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

use crate::core::{CollectionId, VectorId};
use crate::storage::{WalManager, WalEntry};
use crate::compute::distance::DistanceMetric;
use super::types::*;
use super::ViperConfig;
use super::index::{VectorIndex, ViperIndexManager, IndexStrategy, IndexVector, IndexSearchResult};
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
    
    /// Vector index manager for ANN search
    index_manager: Arc<RwLock<HashMap<CollectionId, ViperIndexManager>>>,
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
            index_manager: Arc::new(RwLock::new(HashMap::new())),
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
        
        let tier_searchers = self.storage_searchers.read().await;
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
        let tier_searchers = self.storage_searchers.read().await;
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
            *stats.storage_hits.entry("storage_tier".to_string()).or_insert(0) += 1;
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
    
    /// Phase 5: Two-phase search integration (WAL + Storage) with result merging
    pub async fn search_two_phase(
        &self,
        context: ViperSearchContext,
        wal_manager: Option<Arc<WalManager>>,
    ) -> Result<Vec<ViperSearchResult>> {
        tracing::debug!("🔍 Starting two-phase search for collection: {}", context.collection_id);
        
        let start_time = std::time::Instant::now();
        
        // Phase 1: Search WAL (fresh data) - this is fast, in-memory
        let wal_results = if let Some(wal) = wal_manager {
            self.search_wal_phase(&context, &wal).await?
        } else {
            Vec::new()
        };
        
        tracing::debug!("📝 WAL search completed: {} results found", wal_results.len());
        
        // Phase 2: Search storage tiers (flushed data) - this uses the existing progressive search
        let storage_results = self.search_storage_phase(&context).await?;
        
        tracing::debug!("💾 Storage search completed: {} results found", storage_results.len());
        
        // Phase 3: Merge and deduplicate results with recency bias
        let merged_results = self.merge_two_phase_results(
            wal_results,
            storage_results,
            &context,
        ).await?;
        
        let elapsed = start_time.elapsed();
        tracing::debug!("✅ Two-phase search completed: {} total results in {:?}", 
                       merged_results.len(), elapsed);
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.total_searches += 1;
        stats.avg_latency_ms = (stats.avg_latency_ms + elapsed.as_millis() as f32) / 2.0;
        
        Ok(merged_results)
    }
    
    /// Search WAL for fresh vectors (Phase 1)
    async fn search_wal_phase(
        &self,
        context: &ViperSearchContext,
        wal_manager: &WalManager,
    ) -> Result<Vec<ViperSearchResult>> {
        let mut wal_results = Vec::new();
        
        // Read recent WAL entries for this collection
        let wal_entries = wal_manager
            .read_entries(&context.collection_id, 0, Some(10000)) // Recent 10k entries
            .await?;
        
        tracing::debug!("📖 Found {} WAL entries to search", wal_entries.len());
        
        // Extract vectors from WAL entries and compute similarity
        for entry in wal_entries {
            if let Some(wal_vector) = self.extract_vector_from_wal_entry(&entry).await? {
                // Apply metadata filters first (cheap operation)
                if let Some(filters) = &context.filters {
                    if !self.matches_metadata_filters(&wal_vector.metadata, filters) {
                        continue;
                    }
                }
                
                // Check TTL validity
                if !self.is_wal_vector_valid(&wal_vector).await {
                    continue;
                }
                
                // Compute similarity score
                let similarity = self.compute_vector_similarity(
                    &context.query_vector,
                    &wal_vector.vector,
                ).await?;
                
                // Apply similarity threshold
                if let Some(threshold) = context.threshold {
                    if similarity < threshold {
                        continue;
                    }
                }
                
                wal_results.push(ViperSearchResult {
                    vector_id: wal_vector.id,
                    score: similarity,
                    vector: Some(wal_vector.vector),
                    metadata: wal_vector.metadata,
                    cluster_id: 0, // WAL vectors don't have cluster assignments yet
                    partition_id: "wal".to_string(),
                    storage_url: "wal://memory".to_string(),
                });
            }
        }
        
        // Sort by similarity score (descending)
        wal_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        
        // Limit to reasonable number to avoid overwhelming merge phase
        let wal_limit = (context.k * 3).min(1000); // At most 3x requested or 1000
        wal_results.truncate(wal_limit);
        
        tracing::debug!("🎯 WAL search yielded {} filtered results", wal_results.len());
        
        Ok(wal_results)
    }
    
    /// Search storage tiers for flushed vectors (Phase 2)
    async fn search_storage_phase(
        &self,
        context: &ViperSearchContext,
    ) -> Result<Vec<ViperSearchResult>> {
        // Use existing progressive search implementation
        // This searches across all storage tiers (hot, warm, cold)
        let storage_results = self.search(context.clone(), None, vec![]).await?;
        
        tracing::debug!("🗄️ Storage phase found {} results", storage_results.len());
        
        Ok(storage_results)
    }
    
    /// Merge results from WAL and storage phases with intelligent deduplication
    async fn merge_two_phase_results(
        &self,
        mut wal_results: Vec<ViperSearchResult>,
        mut storage_results: Vec<ViperSearchResult>,
        context: &ViperSearchContext,
    ) -> Result<Vec<ViperSearchResult>> {
        let mut merged_results = Vec::new();
        let mut seen_vectors = HashSet::new();
        
        // Recency bias: WAL results are more recent, so prefer them
        let recency_boost = 0.05; // 5% boost for WAL results
        
        // Apply recency boost to WAL results
        for result in &mut wal_results {
            result.score += recency_boost;
        }
        
        // Create a combined result set for merge sorting
        let mut all_results = Vec::new();
        
        // Mark WAL results with source
        for mut result in wal_results {
            result.partition_id = format!("wal:{}", result.partition_id);
            all_results.push((result, true)); // true = from WAL
        }
        
        // Mark storage results with source
        for result in storage_results {
            all_results.push((result, false)); // false = from storage
        }
        
        // Sort all results by score (descending)
        all_results.sort_by(|a, b| b.0.score.partial_cmp(&a.0.score).unwrap_or(std::cmp::Ordering::Equal));
        
        // Track total candidate count before consuming all_results
        let total_candidates = all_results.len();
        
        // Merge with deduplication, preferring WAL results for duplicates
        for (result, from_wal) in all_results {
            if seen_vectors.contains(&result.vector_id) {
                // Duplicate found - keep the version we already have
                // Since we sorted by score and WAL has recency boost, 
                // the first occurrence is the best one
                continue;
            }
            
            seen_vectors.insert(result.vector_id.clone());
            merged_results.push(result);
            
            // Stop when we have enough results
            if merged_results.len() >= context.k {
                break;
            }
        }
        
        tracing::debug!("🔀 Merged {} unique results (from {} total candidates)", 
                       merged_results.len(), total_candidates);
        
        // Log merge statistics
        let wal_count = merged_results.iter().filter(|r| r.partition_id.starts_with("wal:")).count();
        let storage_count = merged_results.len() - wal_count;
        
        tracing::debug!("📊 Final result composition: {} from WAL, {} from storage", 
                       wal_count, storage_count);
        
        Ok(merged_results)
    }
    
    /// Extract vector record from WAL entry
    async fn extract_vector_from_wal_entry(
        &self,
        entry: &WalEntry,
    ) -> Result<Option<WalVectorRecord>> {
        match &entry.operation {
            crate::storage::WalOperation::Insert { record, .. } => {
                // Convert VectorRecord to WalVectorRecord
                Ok(Some(WalVectorRecord {
                    id: record.id.clone(),
                    vector: record.vector.clone(),
                    metadata: serde_json::to_value(&record.metadata).unwrap_or(serde_json::Value::Null),
                    timestamp: entry.timestamp,
                    operation_type: WalOperationType::Insert,
                }))
            },
            crate::storage::WalOperation::Update { record, .. } => {
                // Convert VectorRecord to WalVectorRecord
                Ok(Some(WalVectorRecord {
                    id: record.id.clone(),
                    vector: record.vector.clone(),
                    metadata: serde_json::to_value(&record.metadata).unwrap_or(serde_json::Value::Null),
                    timestamp: entry.timestamp,
                    operation_type: WalOperationType::Update,
                }))
            },
            crate::storage::WalOperation::Delete { vector_id, .. } => {
                // For deletes, create a tombstone record
                Ok(Some(WalVectorRecord {
                    id: vector_id.clone(),
                    vector: Vec::new(), // Empty vector for tombstone
                    metadata: serde_json::Value::Null,
                    timestamp: entry.timestamp,
                    operation_type: WalOperationType::Delete,
                }))
            },
            _ => Ok(None), // Other WAL entry types don't contain vectors
        }
    }
    
    /// Check if WAL vector is valid (not deleted, not expired)
    async fn is_wal_vector_valid(&self, wal_vector: &WalVectorRecord) -> bool {
        // Skip deleted vectors (tombstones)
        if wal_vector.operation_type == WalOperationType::Delete {
            return false;
        }
        
        // Check TTL if applicable (similar to storage TTL check)
        // For now, WAL vectors are considered always valid
        // Future: implement TTL check for WAL vectors
        
        true
    }
    
    /// Check if metadata matches the given filters
    fn matches_metadata_filters(
        &self,
        metadata: &serde_json::Value,
        filters: &HashMap<String, serde_json::Value>,
    ) -> bool {
        for (key, expected_value) in filters {
            if let Some(actual_value) = metadata.get(key) {
                if actual_value != expected_value {
                    return false;
                }
            } else {
                return false; // Missing key
            }
        }
        true
    }
    
    /// Compute vector similarity using the configured distance metric
    async fn compute_vector_similarity(
        &self,
        query_vector: &[f32],
        candidate_vector: &[f32],
    ) -> Result<f32> {
        // For now, use cosine similarity as default
        // Future: make this configurable based on collection settings
        let similarity = cosine_similarity(query_vector, candidate_vector);
        Ok(similarity)
    }
    
    /// VIPER Phase 6: Build HNSW index for a collection
    pub async fn build_hnsw_index(
        &self,
        collection_id: &CollectionId,
        vectors: Vec<(VectorId, Vec<f32>, serde_json::Value)>,
        distance_metric: DistanceMetric,
    ) -> Result<()> {
        tracing::info!("🏗️ Building HNSW index for collection {} with {} vectors", 
                      collection_id, vectors.len());
        
        // Convert to IndexVector format
        let index_vectors: Vec<IndexVector> = vectors.into_iter()
            .map(|(id, vector, metadata)| IndexVector {
                id,
                vector,
                metadata,
                cluster_id: None, // Will be assigned during clustering
            })
            .collect();
        
        // Create index manager with adaptive strategy
        let mut index_manager = ViperIndexManager::new(
            self.config.clone(),
            IndexStrategy::Adaptive { threshold: 1_000_000 }, // Use HNSW for <1M vectors
        );
        
        // Build the index
        index_manager.build_indexes(index_vectors, distance_metric).await?;
        
        // Store the index manager
        let mut managers = self.index_manager.write().await;
        managers.insert(collection_id.clone(), index_manager);
        
        tracing::info!("✅ HNSW index built successfully for collection {}", collection_id);
        
        Ok(())
    }
    
    /// VIPER Phase 6: Search using HNSW index with filtered candidates
    pub async fn search_with_hnsw(
        &self,
        context: ViperSearchContext,
        filtered_candidates: Option<HashSet<VectorId>>,
    ) -> Result<Vec<ViperSearchResult>> {
        tracing::debug!("🔍 HNSW search for collection: {}", context.collection_id);
        
        let managers = self.index_manager.read().await;
        
        if let Some(index_manager) = managers.get(&context.collection_id) {
            // Search using HNSW index with optional filtering
            let ef = context.k * 10; // Dynamic ef based on k
            let index_results = index_manager.search(
                &context.query_vector,
                context.k,
                ef,
                filtered_candidates.as_ref(),
            ).await?;
            
            // Convert IndexSearchResult to ViperSearchResult
            let mut results = Vec::new();
            for index_result in index_results {
                // Apply similarity threshold if specified
                if let Some(threshold) = context.threshold {
                    // Convert distance to similarity (assuming cosine distance)
                    let similarity = 1.0 - index_result.distance;
                    if similarity < threshold {
                        continue;
                    }
                }
                
                results.push(ViperSearchResult {
                    vector_id: index_result.vector_id,
                    score: 1.0 - index_result.distance, // Convert distance to similarity
                    vector: None, // Will be loaded on demand
                    metadata: index_result.metadata.unwrap_or(serde_json::Value::Null),
                    cluster_id: 0, // TODO: Track cluster assignments
                    partition_id: "hnsw_index".to_string(),
                    storage_url: "memory://hnsw".to_string(),
                });
            }
            
            tracing::debug!("🎯 HNSW search returned {} results", results.len());
            Ok(results)
        } else {
            tracing::warn!("⚠️ No HNSW index found for collection {}, falling back to scan", 
                         context.collection_id);
            // Fallback to regular search
            self.search(context, None, vec![]).await
        }
    }
    
    /// VIPER Phase 6: Combined search with metadata filtering + HNSW
    pub async fn search_filtered_with_hnsw(
        &self,
        context: ViperSearchContext,
    ) -> Result<Vec<ViperSearchResult>> {
        tracing::info!("🔍 Starting filtered HNSW search for collection: {}", context.collection_id);
        
        let start_time = std::time::Instant::now();
        
        // Step 1: Apply metadata filters to get candidate set
        let filtered_candidates = if let Some(filters) = &context.filters {
            tracing::debug!("📋 Applying metadata filters: {:?}", filters);
            let candidates = self.apply_metadata_filters(&context.collection_id, filters).await?;
            tracing::debug!("✅ Metadata filtering reduced to {} candidates", candidates.len());
            Some(candidates)
        } else {
            None
        };
        
        // Step 2: Use HNSW index on filtered candidates
        let hnsw_results = self.search_with_hnsw(context.clone(), filtered_candidates).await?;
        
        // Step 3: Load full vectors for final results if needed
        let mut final_results = Vec::new();
        for mut result in hnsw_results {
            // Load vector data if not already present
            if result.vector.is_none() {
                result.vector = self.load_vector_data(&result.vector_id).await?;
            }
            final_results.push(result);
        }
        
        let elapsed = start_time.elapsed();
        tracing::info!("✅ Filtered HNSW search completed: {} results in {:?}", 
                      final_results.len(), elapsed);
        
        // Update statistics
        let mut stats = self.stats.write().await;
        stats.total_searches += 1;
        stats.avg_latency_ms = (stats.avg_latency_ms + elapsed.as_millis() as f32) / 2.0;
        
        Ok(final_results)
    }
    
    /// Apply metadata filters to get candidate vector IDs
    async fn apply_metadata_filters(
        &self,
        collection_id: &CollectionId,
        filters: &HashMap<String, serde_json::Value>,
    ) -> Result<HashSet<VectorId>> {
        // This would integrate with the storage layer to efficiently
        // query vectors based on metadata filters
        // For now, return empty set as placeholder
        tracing::warn!("⚠️ Metadata filtering not yet fully implemented");
        Ok(HashSet::new())
    }
    
    /// Load vector data for a specific vector ID
    async fn load_vector_data(&self, vector_id: &VectorId) -> Result<Option<Vec<f32>>> {
        // This would load from storage
        // For now, return None as placeholder
        Ok(None)
    }
    
    /// Update HNSW index with new vectors (incremental indexing)
    pub async fn update_hnsw_index(
        &self,
        collection_id: &CollectionId,
        new_vectors: Vec<(VectorId, Vec<f32>, serde_json::Value)>,
    ) -> Result<()> {
        let managers = self.index_manager.read().await;
        
        if let Some(_index_manager) = managers.get(collection_id) {
            // In a real implementation, we would:
            // 1. Get the HNSW index from the manager
            // 2. Add new vectors incrementally
            // 3. Update the index structure
            
            tracing::info!("📝 Updating HNSW index with {} new vectors", new_vectors.len());
            
            // Placeholder for incremental update
            tracing::warn!("⚠️ Incremental HNSW update not yet implemented");
        } else {
            tracing::warn!("⚠️ No HNSW index found for collection {}", collection_id);
        }
        
        Ok(())
    }
}

/// WAL vector record for search operations
#[derive(Debug, Clone)]
struct WalVectorRecord {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: serde_json::Value,
    pub timestamp: DateTime<Utc>,
    pub operation_type: WalOperationType,
}

/// WAL operation type for tracking vector lifecycle
#[derive(Debug, Clone, PartialEq)]
enum WalOperationType {
    Insert,
    Update,
    Delete,
}

/// Cosine similarity implementation
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
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