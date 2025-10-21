//! # Cache Warming Strategies
//!
//! This module implements intelligent cache warming strategies for ProximaDB to optimize
//! cold start performance and improve overall system responsiveness.
//!
//! ## Warming Strategies:
//!
//! 1. **Popularity-Based Warming**: Cache most frequently accessed vectors
//! 2. **Collection-Based Warming**: Pre-load entire collections for high-priority tenants
//! 3. **Similarity-Based Warming**: Pre-load vectors similar to recently accessed ones
//! 4. **Time-Based Warming**: Cache vectors based on temporal access patterns
//!
//! ## Integration with Unified Metrics:
//!
//! - Uses existing access pattern data from CrossCacheOrchestrator
//! - Integrates with unified metrics framework for monitoring
//! - Provides dashboard-ready metrics for cache warming effectiveness

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{Duration, interval};
use tracing::{debug, info, warn};

use crate::storage::cache::orchestrator::{CacheType, CrossCacheOrchestrator};
use crate::storage::traits::UnifiedStorageEngine;
use crate::storage::traits::{MetricsOperationType, UnifiedMetricsCollector};

/// Cache warming strategies for different scenarios
#[derive(Debug, Clone)]
pub enum WarmingStrategy {
    /// Cache most popular vectors (by access frequency)
    PopularityBased {
        /// Number of top vectors to cache
        top_count: usize,
        /// Minimum access count threshold
        min_access_count: u64,
    },
    /// Cache entire collections for high-priority tenants
    CollectionBased {
        /// Collections to fully cache
        priority_collections: Vec<String>,
        /// Maximum vectors per collection
        max_vectors_per_collection: usize,
    },
    /// Cache vectors similar to recently accessed ones
    SimilarityBased {
        /// Number of similar vectors to cache per access
        similarity_count: usize,
        /// Similarity threshold for caching
        similarity_threshold: f32,
    },
    /// Cache vectors based on temporal patterns
    TimeBased {
        /// Time window for pattern analysis
        pattern_window_hours: u64,
        /// Number of vectors to pre-cache
        prefetch_count: usize,
    },
}

/// Cache warming orchestrator that integrates with unified metrics
pub struct CacheWarmer {
    /// Reference to global cache orchestrator
    cache_orchestrator: Arc<CrossCacheOrchestrator>,
    /// Storage engines for data loading
    storage_engines: HashMap<String, Arc<dyn UnifiedStorageEngine>>,
    /// Unified metrics collector for monitoring
    metrics_collector: Arc<UnifiedMetricsCollector>,
    /// Active warming strategies
    warming_strategies: Vec<WarmingStrategy>,
    /// Background warming interval
    warming_interval: Duration,
}

impl CacheWarmer {
    /// Create new cache warmer with unified metrics integration
    pub fn new(
        cache_orchestrator: Arc<CrossCacheOrchestrator>,
        metrics_collector: Arc<UnifiedMetricsCollector>,
    ) -> Self {
        Self {
            cache_orchestrator,
            storage_engines: HashMap::new(),
            metrics_collector,
            warming_strategies: vec![
                // Default strategies for optimal performance
                WarmingStrategy::PopularityBased {
                    top_count: 1000,
                    min_access_count: 5,
                },
                WarmingStrategy::TimeBased {
                    pattern_window_hours: 24,
                    prefetch_count: 500,
                },
            ],
            warming_interval: Duration::from_secs(300), // 5 minutes
        }
    }

    /// Register storage engine for cache warming
    pub fn register_engine(&mut self, engine_name: String, engine: Arc<dyn UnifiedStorageEngine>) {
        self.storage_engines.insert(engine_name, engine);
    }

    /// Add warming strategy
    pub fn add_strategy(&mut self, strategy: WarmingStrategy) {
        self.warming_strategies.push(strategy);
    }

    /// Start background cache warming process
    pub async fn start_warming(&self) -> Result<()> {
        let mut warming_interval = interval(self.warming_interval);

        info!(
            "🔥 Cache warming started with {} strategies",
            self.warming_strategies.len()
        );

        loop {
            warming_interval.tick().await;

            if let Err(e) = self.execute_warming_cycle().await {
                warn!("Cache warming cycle failed: {}", e);
                // Report error to unified metrics
                self.metrics_collector
                    .record(MetricsOperationType::Write, 0, false, None);
            }
        }
    }

    /// Execute one warming cycle
    async fn execute_warming_cycle(&self) -> Result<()> {
        let cycle_start = std::time::Instant::now();
        let mut total_warmed = 0u64;

        debug!("🔥 Starting cache warming cycle");

        for strategy in &self.warming_strategies {
            let warmed_count = self.execute_strategy(strategy).await?;
            total_warmed += warmed_count;

            // Report strategy effectiveness to unified metrics
            self.metrics_collector.record(
                MetricsOperationType::Write,
                0,
                true,
                Some(warmed_count as usize),
            );
        }

        let cycle_duration = cycle_start.elapsed();

        // Report cycle metrics to unified framework
        self.metrics_collector.record(
            MetricsOperationType::Write,
            cycle_duration.as_millis() as u64,
            true,
            Some(total_warmed as usize),
        );

        info!(
            "🔥 Cache warming cycle completed: {} vectors warmed in {:?}",
            total_warmed, cycle_duration
        );
        Ok(())
    }

    /// Execute specific warming strategy
    async fn execute_strategy(&self, strategy: &WarmingStrategy) -> Result<u64> {
        match strategy {
            WarmingStrategy::PopularityBased {
                top_count,
                min_access_count,
            } => {
                self.warm_popular_vectors(*top_count, *min_access_count)
                    .await
            }
            WarmingStrategy::CollectionBased {
                priority_collections,
                max_vectors_per_collection,
            } => {
                self.warm_priority_collections(priority_collections, *max_vectors_per_collection)
                    .await
            }
            WarmingStrategy::SimilarityBased {
                similarity_count,
                similarity_threshold,
            } => {
                self.warm_similar_vectors(*similarity_count, *similarity_threshold)
                    .await
            }
            WarmingStrategy::TimeBased {
                pattern_window_hours,
                prefetch_count,
            } => {
                self.warm_temporal_patterns(*pattern_window_hours, *prefetch_count)
                    .await
            }
        }
    }

    /// Warm most popular vectors based on access patterns
    async fn warm_popular_vectors(&self, top_count: usize, min_access_count: u64) -> Result<u64> {
        debug!(
            "🔥 Warming {} popular vectors (min access: {})",
            top_count, min_access_count
        );

        // Get access patterns from orchestrator
        let access_patterns = self
            .cache_orchestrator
            .pattern_tracker()
            .get_popular_keys(top_count, min_access_count);
        let mut warmed_count = 0u64;

        for (cache_key, access_count) in access_patterns {
            // Parse cache key: "vector:collection_id:vector_id" (consistent format across all engines)
            if let Some((collection_id, vector_id)) = self.parse_vector_cache_key(&cache_key) {
                // Check if already cached in VectorCache (not QueryCache)
                if let Some(vector_cache) = self.cache_orchestrator.get_vector_cache() {
                    if vector_cache.get(&cache_key).await.is_some() {
                        continue; // Already cached
                    }
                }

                // Load from storage and cache
                if let Some(engine) = self.get_best_engine_for_collection(&collection_id) {
                    // We need base_path for storage engines - get from collection metadata
                    if let Ok(Some(vector)) = self
                        .load_vector_with_base_path(&engine, &collection_id, &vector_id)
                        .await
                    {
                        if let Some(vector_cache) = self.cache_orchestrator.get_vector_cache() {
                            let _ = vector_cache.put(cache_key.clone(), vector).await;
                            warmed_count += 1;

                            // Track warming success
                            self.metrics_collector.record(
                                MetricsOperationType::Write,
                                0,
                                true,
                                Some(1),
                            );
                        }
                    }
                }
            }
        }

        debug!(
            "🔥 Popularity warming completed: {} vectors cached",
            warmed_count
        );
        Ok(warmed_count)
    }

    /// Warm entire priority collections
    async fn warm_priority_collections(
        &self,
        collections: &[String],
        max_per_collection: usize,
    ) -> Result<u64> {
        debug!(
            "🔥 Warming {} priority collections (max {} per collection)",
            collections.len(),
            max_per_collection
        );

        let mut total_warmed = 0u64;

        for collection_id in collections {
            let warmed = self
                .warm_collection(collection_id, max_per_collection)
                .await?;
            total_warmed += warmed;

            // Report per-collection metrics
            self.metrics_collector.record(
                MetricsOperationType::Write,
                0,
                true,
                Some(warmed as usize),
            );
        }

        Ok(total_warmed)
    }

    /// Warm vectors similar to recently accessed ones
    async fn warm_similar_vectors(
        &self,
        similarity_count: usize,
        similarity_threshold: f32,
    ) -> Result<u64> {
        debug!(
            "🔥 Warming {} similar vectors (threshold: {})",
            similarity_count, similarity_threshold
        );

        // TODO: Implement similarity-based warming using vector embeddings
        // This would require:
        // 1. Get recently accessed vectors
        // 2. Find similar vectors using vector search
        // 3. Pre-cache the similar vectors

        // For now, return 0 as placeholder
        Ok(0)
    }

    /// Warm vectors based on temporal access patterns
    async fn warm_temporal_patterns(
        &self,
        window_hours: u64,
        prefetch_count: usize,
    ) -> Result<u64> {
        debug!(
            "🔥 Warming {} vectors based on {}h temporal patterns",
            prefetch_count, window_hours
        );

        // TODO: Implement temporal pattern analysis
        // This would require:
        // 1. Analyze access patterns within time window
        // 2. Predict likely next accesses
        // 3. Pre-cache predicted vectors

        // For now, return 0 as placeholder
        Ok(0)
    }

    /// Helper: Parse vector cache key
    fn parse_vector_cache_key(&self, cache_key: &str) -> Option<(String, String)> {
        let parts: Vec<&str> = cache_key.split(':').collect();
        if parts.len() == 3 && parts[0] == "vector" {
            Some((parts[1].to_string(), parts[2].to_string()))
        } else {
            None
        }
    }

    /// Helper: Get best storage engine for collection
    fn get_best_engine_for_collection(
        &self,
        _collection_id: &str,
    ) -> Option<&Arc<dyn UnifiedStorageEngine>> {
        // For now, return first available engine
        // TODO: Implement engine selection based on collection metadata
        self.storage_engines.values().next()
    }

    /// Helper: Load vector with base path resolution
    async fn load_vector_with_base_path(
        &self,
        engine: &Arc<dyn UnifiedStorageEngine>,
        collection_id: &str,
        vector_id: &str,
    ) -> Result<Option<crate::proto::proximadb_v1::VectorRecord>> {
        // TODO: Get base_path from collection metadata service
        // For now, use default path
        let base_path = "/data/collections";
        engine
            .vector_by_id(collection_id, base_path, vector_id)
            .await
    }

    /// Helper: Warm specific collection
    async fn warm_collection(&self, collection_id: &str, max_vectors: usize) -> Result<u64> {
        // TODO: Implement collection warming by:
        // 1. List all vectors in collection
        // 2. Load up to max_vectors
        // 3. Cache them

        // For now, return 0 as placeholder
        Ok(0)
    }
}

/// Cache warming configuration for integration with existing config system
#[derive(Debug, Clone)]
pub struct CacheWarmingConfig {
    /// Enable cache warming
    pub enabled: bool,
    /// Warming interval in seconds
    pub interval_seconds: u64,
    /// Maximum vectors to warm per cycle
    pub max_vectors_per_cycle: usize,
    /// Warming strategies to use
    pub strategies: Vec<WarmingStrategy>,
}

impl Default for CacheWarmingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 300, // 5 minutes
            max_vectors_per_cycle: 2000,
            strategies: vec![
                WarmingStrategy::PopularityBased {
                    top_count: 1000,
                    min_access_count: 3,
                },
                WarmingStrategy::TimeBased {
                    pattern_window_hours: 24,
                    prefetch_count: 500,
                },
            ],
        }
    }
}
