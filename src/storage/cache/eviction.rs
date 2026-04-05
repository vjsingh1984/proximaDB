//! # Cache Eviction Policies
//!
//! This module implements intelligent cache eviction policies for ProximaDB based on
//! access patterns, frequency, and recency to optimize memory usage and cache hit rates.
//!
//! ## Eviction Policies:
//!
//! 1. **LRU (Least Recently Used)**: Evict least recently accessed vectors
//! 2. **LFU (Least Frequently Used)**: Evict least frequently accessed vectors
//! 3. **ARC (Adaptive Replacement Cache)**: Adaptive algorithm that balances recency and frequency
//! 4. **TTL (Time To Live)**: Evict vectors based on age
//! 5. **Access Pattern Based**: Evict based on predicted future access patterns
//!
//! ## Integration with Unified Metrics:
//!
//! - Tracks eviction effectiveness and cache performance
//! - Provides dashboard-ready metrics for cache optimization
//! - Integrates with existing access pattern tracking

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time::{Instant, interval};
use tracing::{debug, info, warn};

use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
use crate::storage::traits::{MetricsOperationType, UnifiedMetricsCollector};

/// Cache eviction policies for different memory management strategies
#[derive(Debug, Clone)]
pub enum EvictionPolicy {
    /// Least Recently Used - evict oldest accessed items
    LRU {
        /// Maximum cache size in number of items
        max_items: usize,
        /// Eviction batch size (for efficiency)
        batch_size: usize,
    },
    /// Least Frequently Used - evict least accessed items
    LFU {
        /// Maximum cache size
        max_items: usize,
        /// Minimum access count to keep in cache
        min_access_count: u64,
        /// Time window for frequency calculation (hours)
        frequency_window_hours: u64,
    },
    /// Adaptive Replacement Cache - balances recency and frequency
    ARC {
        /// Target cache size
        target_size: usize,
        /// Recent list size (c1)
        recent_size: usize,
        /// Frequent list size (c2)
        frequent_size: usize,
    },
    /// Time To Live - evict based on age
    TTL {
        /// Maximum time to keep items in cache
        max_age_seconds: u64,
        /// Check interval for TTL cleanup
        cleanup_interval_seconds: u64,
    },
    /// Access Pattern Based - evict based on predicted access patterns
    PatternBased {
        /// Use machine learning predictions
        use_ml_predictions: bool,
        /// Historical pattern window (hours)
        pattern_window_hours: u64,
        /// Eviction threshold score
        eviction_threshold: f64,
    },
}

/// Cache eviction manager with unified metrics integration
pub struct CacheEvictor {
    /// Reference to global cache orchestrator
    cache_orchestrator: Arc<CrossCacheOrchestrator>,
    /// Unified metrics collector
    metrics_collector: Arc<UnifiedMetricsCollector>,
    /// Active eviction policies
    eviction_policies: Vec<EvictionPolicy>,
    /// Eviction check interval
    check_interval: Duration,
    /// Cache access tracking for eviction decisions
    access_tracker: Arc<AccessTracker>,
}

/// Tracks cache access patterns for eviction decisions
pub struct AccessTracker {
    /// Item access times (cache_key -> last_access_time)
    access_times: tokio::sync::RwLock<HashMap<String, SystemTime>>,
    /// Item access counts (cache_key -> count)
    access_counts: tokio::sync::RwLock<HashMap<String, u64>>,
    /// Item creation times (cache_key -> created_time)
    creation_times: tokio::sync::RwLock<HashMap<String, SystemTime>>,
}

impl AccessTracker {
    /// Create new access tracker
    pub fn new() -> Self {
        Self {
            access_times: tokio::sync::RwLock::new(HashMap::new()),
            access_counts: tokio::sync::RwLock::new(HashMap::new()),
            creation_times: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Track cache access
    pub async fn track_access(&self, cache_key: String) {
        let now = SystemTime::now();

        // Update access time
        {
            let mut access_times = self.access_times.write().await;
            access_times.insert(cache_key.clone(), now);
        }

        // Increment access count
        {
            let mut access_counts = self.access_counts.write().await;
            *access_counts.entry(cache_key.clone()).or_insert(0) += 1;
        }

        // Set creation time if new item
        {
            let mut creation_times = self.creation_times.write().await;
            creation_times.entry(cache_key).or_insert(now);
        }
    }

    /// Track cache item creation
    pub async fn track_creation(&self, cache_key: String) {
        let now = SystemTime::now();

        let mut creation_times = self.creation_times.write().await;
        creation_times.insert(cache_key, now);
    }

    /// Get least recently used items
    pub async fn get_lru_items(&self, count: usize) -> Vec<String> {
        let access_times = self.access_times.read().await;
        let mut items: Vec<_> = access_times.iter().collect();

        // Sort by access time (oldest first)
        items.sort_by_key(|(_, time)| *time);

        items
            .into_iter()
            .take(count)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Get least frequently used items
    pub async fn get_lfu_items(&self, count: usize, min_access_count: u64) -> Vec<String> {
        let access_counts = self.access_counts.read().await;
        let mut items: Vec<_> = access_counts
            .iter()
            .filter(|(_, count)| **count < min_access_count)
            .collect();

        // Sort by access count (lowest first)
        items.sort_by_key(|(_, count)| **count);

        items
            .into_iter()
            .take(count)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Get expired items based on TTL
    pub async fn get_expired_items(&self, max_age: Duration) -> Vec<String> {
        let creation_times = self.creation_times.read().await;
        let now = SystemTime::now();

        creation_times
            .iter()
            .filter_map(|(key, &created_time)| {
                if let Ok(age) = now.duration_since(created_time) {
                    if age > max_age {
                        Some(key.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    /// Remove tracking for evicted items
    pub async fn remove_tracking(&self, cache_keys: &[String]) {
        {
            let mut access_times = self.access_times.write().await;
            for key in cache_keys {
                access_times.remove(key);
            }
        }

        {
            let mut access_counts = self.access_counts.write().await;
            for key in cache_keys {
                access_counts.remove(key);
            }
        }

        {
            let mut creation_times = self.creation_times.write().await;
            for key in cache_keys {
                creation_times.remove(key);
            }
        }
    }
}

impl Default for AccessTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheEvictor {
    /// Create new cache evictor with unified metrics integration
    pub fn new(
        cache_orchestrator: Arc<CrossCacheOrchestrator>,
        metrics_collector: Arc<UnifiedMetricsCollector>,
    ) -> Self {
        Self {
            cache_orchestrator,
            metrics_collector,
            eviction_policies: vec![
                // Default policies for optimal memory management
                EvictionPolicy::LRU {
                    max_items: 10000,
                    batch_size: 100,
                },
                EvictionPolicy::TTL {
                    max_age_seconds: 3600,         // 1 hour
                    cleanup_interval_seconds: 300, // 5 minutes
                },
            ],
            check_interval: Duration::from_secs(60), // 1 minute
            access_tracker: Arc::new(AccessTracker::new()),
        }
    }

    /// Add eviction policy
    pub fn add_policy(&mut self, policy: EvictionPolicy) {
        self.eviction_policies.push(policy);
    }

    /// Start background cache eviction process
    pub async fn start_eviction(&self) -> Result<()> {
        let mut eviction_interval = interval(self.check_interval);

        info!(
            "🗑️ Cache eviction started with {} policies",
            self.eviction_policies.len()
        );

        loop {
            eviction_interval.tick().await;

            if let Err(e) = self.execute_eviction_cycle().await {
                warn!("Cache eviction cycle failed: {}", e);
                // Report error to unified metrics
                self.metrics_collector.record(
                    MetricsOperationType::Delete, // Use Delete as closest operation type
                    0,                            // duration not applicable for error
                    false,                        // success = false for error
                    None,
                );
            }
        }
    }

    /// Execute one eviction cycle
    async fn execute_eviction_cycle(&self) -> Result<()> {
        let cycle_start = Instant::now();
        let mut total_evicted = 0u64;

        debug!("🗑️ Starting cache eviction cycle");

        for policy in &self.eviction_policies {
            let evicted_count = self.execute_policy(policy).await?;
            total_evicted += evicted_count;

            // Report policy effectiveness to unified metrics
            self.metrics_collector.record(
                MetricsOperationType::Delete,
                0,
                true,
                Some(evicted_count as usize),
            );
        }

        let cycle_duration = cycle_start.elapsed();

        // Report cycle metrics to unified framework
        self.metrics_collector.record(
            MetricsOperationType::Delete,
            cycle_duration.as_millis() as u64,
            true,
            Some(total_evicted as usize),
        );

        if total_evicted > 0 {
            info!(
                "🗑️ Cache eviction cycle completed: {} items evicted in {:?}",
                total_evicted, cycle_duration
            );
        }

        Ok(())
    }

    /// Execute specific eviction policy
    async fn execute_policy(&self, policy: &EvictionPolicy) -> Result<u64> {
        match policy {
            EvictionPolicy::LRU {
                max_items,
                batch_size,
            } => self.evict_lru(*max_items, *batch_size).await,
            EvictionPolicy::LFU {
                max_items,
                min_access_count,
                frequency_window_hours,
            } => {
                self.evict_lfu(*max_items, *min_access_count, *frequency_window_hours)
                    .await
            }
            EvictionPolicy::ARC {
                target_size,
                recent_size,
                frequent_size,
            } => {
                self.evict_arc(*target_size, *recent_size, *frequent_size)
                    .await
            }
            EvictionPolicy::TTL {
                max_age_seconds,
                cleanup_interval_seconds: _,
            } => self.evict_ttl(*max_age_seconds).await,
            EvictionPolicy::PatternBased {
                use_ml_predictions,
                pattern_window_hours,
                eviction_threshold,
            } => {
                self.evict_pattern_based(
                    *use_ml_predictions,
                    *pattern_window_hours,
                    *eviction_threshold,
                )
                .await
            }
        }
    }

    /// Evict using LRU policy
    async fn evict_lru(&self, max_items: usize, batch_size: usize) -> Result<u64> {
        if let Some(query_cache) = self.cache_orchestrator.get_query_cache() {
            let current_size = query_cache.size().await;

            if current_size > max_items {
                let to_evict = (current_size - max_items).min(batch_size);
                let lru_items = self.access_tracker.get_lru_items(to_evict).await;

                for item in &lru_items {
                    let _ = query_cache.remove_by_string(item).await;
                }

                self.access_tracker.remove_tracking(&lru_items).await;

                debug!("🗑️ LRU eviction: {} items removed", lru_items.len());
                return Ok(lru_items.len() as u64);
            }
        }

        Ok(0)
    }

    /// Evict using LFU policy
    async fn evict_lfu(
        &self,
        max_items: usize,
        min_access_count: u64,
        _window_hours: u64,
    ) -> Result<u64> {
        if let Some(query_cache) = self.cache_orchestrator.get_query_cache() {
            let current_size = query_cache.size().await;

            if current_size > max_items {
                let to_evict = current_size - max_items;
                let lfu_items = self
                    .access_tracker
                    .get_lfu_items(to_evict, min_access_count)
                    .await;

                for item in &lfu_items {
                    let _ = query_cache.remove_by_string(item).await;
                }

                self.access_tracker.remove_tracking(&lfu_items).await;

                debug!("🗑️ LFU eviction: {} items removed", lfu_items.len());
                return Ok(lfu_items.len() as u64);
            }
        }

        Ok(0)
    }

    /// Evict using ARC policy (simplified implementation)
    async fn evict_arc(
        &self,
        target_size: usize,
        _recent_size: usize,
        _frequent_size: usize,
    ) -> Result<u64> {
        // Deferred: Implement full ARC algorithm
        // For now, use LRU as fallback
        self.evict_lru(target_size, target_size / 10).await
    }

    /// Evict using TTL policy
    async fn evict_ttl(&self, max_age_seconds: u64) -> Result<u64> {
        let max_age = Duration::from_secs(max_age_seconds);
        let expired_items = self.access_tracker.get_expired_items(max_age).await;

        if !expired_items.is_empty() {
            if let Some(query_cache) = self.cache_orchestrator.get_query_cache() {
                for item in &expired_items {
                    let _ = query_cache.remove_by_string(item).await;
                }
            }

            self.access_tracker.remove_tracking(&expired_items).await;

            debug!(
                "🗑️ TTL eviction: {} expired items removed",
                expired_items.len()
            );
            return Ok(expired_items.len() as u64);
        }

        Ok(0)
    }

    /// Evict using pattern-based predictions
    async fn evict_pattern_based(
        &self,
        _use_ml: bool,
        _window_hours: u64,
        _threshold: f64,
    ) -> Result<u64> {
        // Deferred: Implement pattern-based eviction using access pattern analysis
        // This would require:
        // 1. Analyze historical access patterns
        // 2. Predict future access probability
        // 3. Evict items with low predicted access probability

        // For now, return 0 as placeholder
        Ok(0)
    }

    /// Get access tracker for external use
    pub fn access_tracker(&self) -> Arc<AccessTracker> {
        self.access_tracker.clone()
    }

    /// Trigger immediate cache eviction (called when memory pressure detected)
    pub async fn trigger_immediate_eviction(&self) -> Result<()> {
        tracing::info!("Triggering immediate cache eviction due to memory pressure");

        let mut total_evicted = 0u64;

        // Execute all configured eviction policies immediately
        for policy in &self.eviction_policies {
            match self.execute_policy(policy).await {
                Ok(evicted) => {
                    total_evicted += evicted;
                    tracing::debug!("Evicted {} items using policy {:?}", evicted, policy);
                }
                Err(e) => {
                    tracing::warn!("Failed to execute eviction policy {:?}: {:?}", policy, e);
                }
            }
        }

        tracing::info!(
            "Immediate eviction completed: {} items evicted",
            total_evicted
        );
        Ok(())
    }
}

/// Cache eviction configuration
#[derive(Debug, Clone)]
pub struct CacheEvictionConfig {
    /// Enable cache eviction
    pub enabled: bool,
    /// Eviction check interval in seconds
    pub check_interval_seconds: u64,
    /// Maximum cache size before eviction
    pub max_cache_size: usize,
    /// Eviction policies to use
    pub policies: Vec<EvictionPolicy>,
}

impl Default for CacheEvictionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_seconds: 60, // 1 minute
            max_cache_size: 10000,
            policies: vec![
                EvictionPolicy::LRU {
                    max_items: 10000,
                    batch_size: 100,
                },
                EvictionPolicy::TTL {
                    max_age_seconds: 3600,         // 1 hour
                    cleanup_interval_seconds: 300, // 5 minutes
                },
            ],
        }
    }
}
