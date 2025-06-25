// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Search Engine - Consolidates All Search Functionality
//!
//! 🔥 NEW: This replaces and consolidates the following obsolete engines:
//! - src/storage/search/mod.rs (SearchEngine trait + cost optimization)
//! - src/storage/viper/search_engine.rs (ViperProgressiveSearchEngine)  
//! - src/storage/viper/search_impl.rs (VIPER search implementation)
//!
//! This unified engine provides:
//! - Single point of search coordination
//! - Cost-based algorithm selection
//! - Progressive tier searching
//! - ML-guided optimizations
//! - Storage-agnostic interface

use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info};

use crate::core::CollectionId;

use super::super::types::*;

/// Unified search engine that consolidates all search functionality
/// Replaces: SearchEngine, ViperProgressiveSearchEngine, search implementations
pub struct UnifiedSearchEngine {
    /// Algorithm registry for pluggable search algorithms
    algorithms: Arc<RwLock<HashMap<String, Box<dyn SearchAlgorithm>>>>,
    
    /// Cost model for algorithm selection
    cost_model: Arc<SearchCostModel>,
    
    /// Result processor for post-processing and merging
    result_processor: Arc<ResultProcessor>,
    
    /// Storage tier managers for progressive search
    tier_managers: Arc<RwLock<HashMap<StorageTier, Box<dyn TierSearcher>>>>,
    
    /// Search statistics and optimization feedback
    stats_collector: Arc<RwLock<SearchStatsCollector>>,
    
    /// Feature selection cache for optimization
    feature_cache: Arc<RwLock<FeatureSelectionCache>>,
    
    /// Search concurrency limiter
    search_semaphore: Arc<Semaphore>,
    
    /// Configuration
    config: UnifiedSearchConfig,
}

/// Configuration for the unified search engine
#[derive(Debug, Clone)]
pub struct UnifiedSearchConfig {
    pub max_concurrent_searches: usize,
    pub default_timeout_ms: u64,
    pub enable_cost_optimization: bool,
    pub enable_progressive_search: bool,
    pub enable_feature_selection: bool,
    pub cache_size: usize,
    pub tier_search_parallelism: usize,
}

/// Cost model for search algorithm selection
pub struct SearchCostModel {
    /// Historical performance data per algorithm
    algorithm_performance: RwLock<HashMap<String, AlgorithmPerformanceData>>,
    
    /// Collection-specific cost factors
    collection_factors: RwLock<HashMap<CollectionId, CollectionCostFactors>>,
    
    /// Cost calculation weights
    cost_weights: CostWeights,
}

/// Algorithm performance tracking
#[derive(Debug, Clone)]
pub struct AlgorithmPerformanceData {
    pub avg_execution_time_ms: f64,
    pub avg_accuracy: f64,
    pub total_executions: u64,
    pub success_rate: f64,
    pub memory_usage_mb: f64,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// Collection-specific cost factors
#[derive(Debug, Clone)]
pub struct CollectionCostFactors {
    pub vector_count: usize,
    pub avg_vector_dimension: usize,
    pub data_distribution: DataDistribution,
    pub index_types: Vec<IndexType>,
    pub access_patterns: AccessPatterns,
}

/// Data distribution characteristics
#[derive(Debug, Clone)]
pub enum DataDistribution {
    Uniform,
    Normal { mean: f64, std_dev: f64 },
    Skewed { skew_factor: f64 },
    Clustered { cluster_count: usize, separation: f64 },
}

/// Access pattern analysis
#[derive(Debug, Clone)]
pub struct AccessPatterns {
    pub query_frequency: f64,
    pub typical_k_values: Vec<usize>,
    pub filter_usage_rate: f64,
    pub temporal_patterns: Vec<f64>,
}

/// Cost calculation weights
#[derive(Debug, Clone)]
pub struct CostWeights {
    pub cpu_weight: f64,
    pub io_weight: f64,
    pub memory_weight: f64,
    pub accuracy_weight: f64,
    pub latency_weight: f64,
}

/// Result processor for search result handling
pub struct ResultProcessor {
    /// Result merger for combining multiple sources
    merger: Arc<ResultMerger>,
    
    /// Post-processing filters
    post_processors: Vec<Box<dyn ResultPostProcessor>>,
    
    /// Score normalizers
    score_normalizers: HashMap<String, Box<dyn ScoreNormalizer>>,
}

/// Trait for storage tier-specific searching
#[async_trait]
pub trait TierSearcher: Send + Sync {
    /// Search within a specific storage tier
    async fn search_tier(
        &self,
        context: &SearchContext,
        tier_constraints: &TierConstraints,
    ) -> Result<Vec<SearchResult>>;
    
    /// Get tier characteristics for cost estimation
    fn tier_characteristics(&self) -> TierCharacteristics;
    
    /// Check if tier is available and healthy
    async fn health_check(&self) -> Result<bool>;
}

/// Constraints for tier-specific searching
#[derive(Debug, Clone)]
pub struct TierConstraints {
    pub max_results: usize,
    pub timeout_ms: u64,
    pub quality_threshold: f64,
    pub partition_hints: Option<Vec<String>>,
    pub cluster_hints: Option<Vec<u32>>,
}

/// Tier characteristics for optimization
#[derive(Debug, Clone)]
pub struct TierCharacteristics {
    pub tier: StorageTier,
    pub avg_latency_ms: f64,
    pub throughput_vectors_per_sec: f64,
    pub memory_resident: bool,
    pub supports_random_access: bool,
    pub compression_overhead: f64,
    pub index_types_supported: Vec<IndexType>,
}

/// Search statistics collector
#[derive(Debug, Default, Clone)]
pub struct SearchStatsCollector {
    pub total_searches: u64,
    pub algorithm_usage: HashMap<String, u64>,
    pub tier_usage: HashMap<StorageTier, u64>,
    pub avg_latency_ms: f64,
    pub accuracy_scores: Vec<f64>,
    pub cost_efficiency_scores: Vec<f64>,
}

/// Feature selection cache for optimization
#[derive(Debug)]
pub struct FeatureSelectionCache {
    /// Collection -> important features mapping
    feature_sets: HashMap<CollectionId, CachedFeatureSet>,
    
    /// Maximum cache entries
    max_size: usize,
    
    /// LRU tracking
    access_order: Vec<CollectionId>,
}

/// Cached feature set with importance scores
#[derive(Debug, Clone)]
pub struct CachedFeatureSet {
    pub features: Vec<usize>,
    pub importance_scores: Vec<f32>,
    pub confidence: f64,
    pub last_updated: chrono::DateTime<chrono::Utc>,
    pub hit_count: u64,
}

/// Result merger for combining search results from multiple sources
pub struct ResultMerger {
    /// Merge strategies
    strategies: HashMap<String, Box<dyn MergeStrategy>>,
    
    /// Default strategy
    default_strategy: String,
}

/// Trait for result merging strategies
#[async_trait]
pub trait MergeStrategy: Send + Sync {
    async fn merge_results(
        &self,
        results: Vec<Vec<SearchResult>>,
        context: &SearchContext,
    ) -> Result<Vec<SearchResult>>;
}

/// Trait for post-processing search results
#[async_trait]
pub trait ResultPostProcessor: Send + Sync {
    async fn process_results(
        &self,
        results: Vec<SearchResult>,
        context: &SearchContext,
    ) -> Result<Vec<SearchResult>>;
}

/// Trait for score normalization
pub trait ScoreNormalizer: Send + Sync {
    fn normalize_score(&self, score: f32, context: &SearchContext) -> f32;
}

impl ResultMerger {
    pub async fn merge_results(
        &self,
        results: Vec<Vec<SearchResult>>,
        context: &SearchContext,
    ) -> Result<Vec<SearchResult>> {
        let strategy = self.strategies.get(&self.default_strategy)
            .ok_or_else(|| anyhow::anyhow!("Default merge strategy not found"))?;
        strategy.merge_results(results, context).await
    }
}

// Implementation of UnifiedSearchEngine

impl UnifiedSearchEngine {
    /// Create new unified search engine
    pub async fn new(config: UnifiedSearchConfig) -> Result<Self> {
        let algorithms: HashMap<String, Box<dyn SearchAlgorithm>> = HashMap::new();
        
        // Register built-in algorithms
        // algorithms.insert("hnsw".to_string(), Box::new(HnswSearchAlgorithm::new()));
        // algorithms.insert("flat".to_string(), Box::new(FlatSearchAlgorithm::new()));
        // algorithms.insert("ivf".to_string(), Box::new(IvfSearchAlgorithm::new()));
        
        let cost_model = Arc::new(SearchCostModel::new());
        let result_processor = Arc::new(ResultProcessor::new());
        let stats_collector = Arc::new(RwLock::new(SearchStatsCollector::default()));
        let feature_cache = Arc::new(RwLock::new(FeatureSelectionCache::new(config.cache_size)));
        
        Ok(Self {
            algorithms: Arc::new(RwLock::new(algorithms)),
            cost_model,
            result_processor,
            tier_managers: Arc::new(RwLock::new(HashMap::new())),
            stats_collector,
            feature_cache,
            search_semaphore: Arc::new(Semaphore::new(config.max_concurrent_searches)),
            config,
        })
    }
    
    /// Register a new search algorithm
    pub async fn register_algorithm(
        &self,
        name: String,
        algorithm: Box<dyn SearchAlgorithm>,
    ) -> Result<()> {
        let mut algorithms = self.algorithms.write().await;
        algorithms.insert(name, algorithm);
        Ok(())
    }
    
    /// Register a tier searcher
    pub async fn register_tier_searcher(
        &self,
        tier: StorageTier,
        searcher: Box<dyn TierSearcher>,
    ) -> Result<()> {
        let mut tier_managers = self.tier_managers.write().await;
        tier_managers.insert(tier, searcher);
        Ok(())
    }
    
    /// Execute unified search - main entry point
    pub async fn search(&self, context: SearchContext) -> Result<Vec<SearchResult>> {
        let _permit = self.search_semaphore.acquire().await?;
        let start_time = Instant::now();
        
        debug!("🔍 Starting unified search for collection: {}", context.collection_id);
        
        // Step 1: Select optimal algorithm based on context and cost model
        let selected_algorithm = self.select_algorithm(&context).await?;
        debug!("📊 Selected algorithm: {}", selected_algorithm);
        
        // Step 2: Execute search based on strategy
        let results = match context.strategy {
            SearchStrategy::Exact => {
                self.execute_exact_search(&context, &selected_algorithm).await?
            }
            SearchStrategy::Approximate { .. } => {
                self.execute_approximate_search(&context, &selected_algorithm).await?
            }
            SearchStrategy::Progressive { .. } => {
                self.execute_progressive_search(&context).await?
            }
            SearchStrategy::Hybrid { .. } => {
                self.execute_hybrid_search(&context, &selected_algorithm).await?
            }
            SearchStrategy::Adaptive { .. } => {
                self.execute_adaptive_search(&context).await?
            }
        };
        
        // Step 3: Post-process results
        let processed_results = self.result_processor.process_results(results, &context).await?;
        
        // Step 4: Update statistics
        let execution_time = start_time.elapsed().as_millis() as f64;
        self.update_statistics(&context, &selected_algorithm, execution_time, &processed_results).await?;
        
        info!("✅ Search completed in {:.2}ms, returned {} results", 
              execution_time, processed_results.len());
        
        Ok(processed_results)
    }
    
    /// Select optimal algorithm based on cost model
    async fn select_algorithm(&self, context: &SearchContext) -> Result<String> {
        if !self.config.enable_cost_optimization {
            return Ok("hnsw".to_string()); // Default fallback
        }
        
        let algorithms = self.algorithms.read().await;
        let cost_model = &self.cost_model;
        
        let mut best_algorithm = "hnsw".to_string();
        let mut best_cost = f64::INFINITY;
        
        for (name, algorithm) in algorithms.iter() {
            let estimated_cost = cost_model.estimate_cost(context, algorithm.as_ref()).await?;
            if estimated_cost.total_cost < best_cost {
                best_cost = estimated_cost.total_cost;
                best_algorithm = name.clone();
            }
        }
        
        Ok(best_algorithm)
    }
    
    /// Execute exact search
    async fn execute_exact_search(
        &self,
        context: &SearchContext,
        algorithm_name: &str,
    ) -> Result<Vec<SearchResult>> {
        let algorithms = self.algorithms.read().await;
        let algorithm = algorithms.get(algorithm_name)
            .ok_or_else(|| anyhow::anyhow!("Algorithm {} not found", algorithm_name))?;
        
        algorithm.search(context).await
    }
    
    /// Execute approximate search
    async fn execute_approximate_search(
        &self,
        context: &SearchContext,
        algorithm_name: &str,
    ) -> Result<Vec<SearchResult>> {
        // Similar to exact search but with different parameters
        self.execute_exact_search(context, algorithm_name).await
    }
    
    /// Execute progressive search across storage tiers
    async fn execute_progressive_search(&self, context: &SearchContext) -> Result<Vec<SearchResult>> {
        if let SearchStrategy::Progressive { tier_limits, early_termination_threshold } = &context.strategy {
            let tier_managers = self.tier_managers.read().await;
            let mut all_results = Vec::new();
            let mut total_found = 0;
            
            // Search tiers in priority order
            for (tier_index, (tier, searcher)) in tier_managers.iter().enumerate() {
                if tier_index >= tier_limits.len() {
                    break;
                }
                
                let tier_limit = tier_limits[tier_index];
                let tier_constraints = TierConstraints {
                    max_results: tier_limit,
                    timeout_ms: context.timeout_ms.unwrap_or(10000),
                    quality_threshold: early_termination_threshold.unwrap_or(0.8) as f64,
                    partition_hints: None,
                    cluster_hints: None,
                };
                
                debug!("🔍 Searching tier {:?} with limit {}", tier, tier_limit);
                
                let tier_results = searcher.search_tier(context, &tier_constraints).await?;
                total_found += tier_results.len();
                all_results.extend(tier_results);
                
                // Early termination check
                if let Some(threshold) = early_termination_threshold {
                    if total_found >= (context.k as f32 * (1.0 + threshold)) as usize {
                        debug!("🚀 Early termination triggered at tier {:?}", tier);
                        break;
                    }
                }
            }
            
            // Merge and rank all results
            self.result_processor.merger.merge_results(vec![all_results], context).await
        } else {
            Err(anyhow::anyhow!("Invalid strategy for progressive search"))
        }
    }
    
    /// Execute hybrid search (filter + similarity)
    async fn execute_hybrid_search(
        &self,
        context: &SearchContext,
        algorithm_name: &str,
    ) -> Result<Vec<SearchResult>> {
        // For now, delegate to regular search with filters
        // TODO: Implement filter-first vs search-first optimization
        self.execute_exact_search(context, algorithm_name).await
    }
    
    /// Execute adaptive search
    async fn execute_adaptive_search(&self, context: &SearchContext) -> Result<Vec<SearchResult>> {
        if let SearchStrategy::Adaptive { query_complexity_score, time_budget_ms, accuracy_preference } = &context.strategy {
            // Choose strategy based on context
            let adaptive_strategy = if *accuracy_preference > 0.8 {
                SearchStrategy::Exact
            } else if *time_budget_ms < 100 {
                SearchStrategy::Approximate { 
                    target_recall: 0.7, 
                    max_candidates: Some(context.k * 10) 
                }
            } else {
                SearchStrategy::Progressive { 
                    tier_limits: vec![context.k / 2, context.k], 
                    early_termination_threshold: Some(0.9) 
                }
            };
            
            let adaptive_context = SearchContext {
                strategy: adaptive_strategy,
                ..context.clone()
            };
            
            Box::pin(self.search(adaptive_context)).await
        } else {
            Err(anyhow::anyhow!("Invalid strategy for adaptive search"))
        }
    }
    
    /// Update search statistics
    async fn update_statistics(
        &self,
        context: &SearchContext,
        algorithm_name: &str,
        execution_time_ms: f64,
        results: &[SearchResult],
    ) -> Result<()> {
        let mut stats = self.stats_collector.write().await;
        stats.total_searches += 1;
        
        *stats.algorithm_usage.entry(algorithm_name.to_string()).or_insert(0) += 1;
        
        // Update rolling average
        let alpha = 0.1; // Smoothing factor
        stats.avg_latency_ms = stats.avg_latency_ms * (1.0 - alpha) + execution_time_ms * alpha;
        
        debug!("📈 Updated stats: {} searches, {:.2}ms avg latency", 
               stats.total_searches, stats.avg_latency_ms);
        
        Ok(())
    }
    
    /// Get search statistics
    pub async fn get_statistics(&self) -> Result<SearchStatsCollector> {
        let stats = self.stats_collector.read().await;
        Ok(stats.clone())
    }
}

// Implementation of supporting structures

impl SearchCostModel {
    pub fn new() -> Self {
        Self {
            algorithm_performance: RwLock::new(HashMap::new()),
            collection_factors: RwLock::new(HashMap::new()),
            cost_weights: CostWeights {
                cpu_weight: 0.3,
                io_weight: 0.3,
                memory_weight: 0.2,
                accuracy_weight: 0.1,
                latency_weight: 0.1,
            },
        }
    }
    
    pub async fn estimate_cost(
        &self,
        context: &SearchContext,
        algorithm: &dyn SearchAlgorithm,
    ) -> Result<CostBreakdown> {
        // Simple cost estimation - can be made more sophisticated
        let base_cost = match algorithm.algorithm_type() {
            IndexType::HNSW { .. } => 0.5,
            IndexType::IVF { .. } => 0.7,
            IndexType::Flat => 1.0,
            _ => 0.6,
        };
        
        let k_factor = (context.k as f64).log2() / 10.0;
        let filter_penalty = if context.filters.is_some() { 1.2 } else { 1.0 };
        
        let total_cost = base_cost * k_factor * filter_penalty;
        
        Ok(CostBreakdown {
            cpu_cost: total_cost * 0.4,
            io_cost: total_cost * 0.3,
            memory_cost: total_cost * 0.2,
            network_cost: total_cost * 0.1,
            total_cost,
            cost_efficiency: 1.0 / total_cost,
        })
    }
}

impl ResultProcessor {
    pub fn new() -> Self {
        let mut strategies: HashMap<String, Box<dyn MergeStrategy>> = HashMap::new();
        strategies.insert("score_based".to_string(), Box::new(ScoreBasedMerger));
        
        Self {
            merger: Arc::new(ResultMerger {
                strategies,
                default_strategy: "score_based".to_string(),
            }),
            post_processors: Vec::new(),
            score_normalizers: HashMap::new(),
        }
    }
    
    pub async fn process_results(
        &self,
        results: Vec<SearchResult>,
        context: &SearchContext,
    ) -> Result<Vec<SearchResult>> {
        let mut processed = results;
        
        // Apply post-processors
        for processor in &self.post_processors {
            processed = processor.process_results(processed, context).await?;
        }
        
        // Limit to requested k
        processed.truncate(context.k);
        
        Ok(processed)
    }
}

impl FeatureSelectionCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            feature_sets: HashMap::new(),
            max_size,
            access_order: Vec::new(),
        }
    }
}

// Simple merge strategy implementation
pub struct ScoreBasedMerger;

#[async_trait]
impl MergeStrategy for ScoreBasedMerger {
    async fn merge_results(
        &self,
        results: Vec<Vec<SearchResult>>,
        context: &SearchContext,
    ) -> Result<Vec<SearchResult>> {
        let mut all_results: Vec<SearchResult> = results.into_iter().flatten().collect();
        
        // Sort by score (descending)
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        
        // Remove duplicates based on vector_id
        let mut seen_ids = HashSet::new();
        all_results.retain(|result| seen_ids.insert(result.vector_id.clone()));
        
        // Limit to k results
        all_results.truncate(context.k);
        
        Ok(all_results)
    }
}

impl Default for UnifiedSearchConfig {
    fn default() -> Self {
        Self {
            max_concurrent_searches: 8,
            default_timeout_ms: 10000,
            enable_cost_optimization: true,
            enable_progressive_search: true,
            enable_feature_selection: true,
            cache_size: 1000,
            tier_search_parallelism: 4,
        }
    }
}