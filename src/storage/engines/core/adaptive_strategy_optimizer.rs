//! Adaptive Strategy Optimizer for intelligent threshold tuning
//!
//! This module provides automatic optimization of strategy parameters based on
//! observed workload patterns and performance metrics.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::read_strategy::ReadAccessStrategy;

/// Performance metrics for strategy evaluation
#[derive(Debug, Clone)]
pub struct StrategyMetrics {
    /// Number of cache hits
    pub cache_hits: u64,
    /// Number of cache misses
    pub cache_misses: u64,
    /// Total read operations
    pub total_reads: u64,
    /// Average read latency in microseconds
    pub avg_latency_us: f64,
    /// Total bytes read
    pub bytes_read: u64,
    /// Number of strategy switches
    pub strategy_switches: u64,
    /// Last measurement timestamp
    pub last_updated: Instant,
}

impl StrategyMetrics {
    pub fn new() -> Self {
        Self {
            cache_hits: 0,
            cache_misses: 0,
            total_reads: 0,
            avg_latency_us: 0.0,
            bytes_read: 0,
            strategy_switches: 0,
            last_updated: Instant::now(),
        }
    }

    /// Calculate cache hit rate (0.0 to 1.0)
    pub fn hit_rate(&self) -> f64 {
        if self.total_reads == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.total_reads as f64
        }
    }

    /// Calculate miss rate (0.0 to 1.0)
    pub fn miss_rate(&self) -> f64 {
        1.0 - self.hit_rate()
    }

    /// Calculate reads per second
    pub fn reads_per_second(&self) -> f64 {
        let duration = self.last_updated.elapsed().as_secs_f64();
        if duration > 0.0 {
            self.total_reads as f64 / duration
        } else {
            0.0
        }
    }

    /// Merge another metrics instance into this one
    pub fn merge(&mut self, other: &StrategyMetrics) {
        self.cache_hits += other.cache_hits;
        self.cache_misses += other.cache_misses;
        self.total_reads += other.total_reads;
        self.bytes_read += other.bytes_read;
        self.strategy_switches += other.strategy_switches;

        // Update weighted average latency
        if other.total_reads > 0 {
            let total_latency = (self.avg_latency_us * self.total_reads as f64) +
                               (other.avg_latency_us * other.total_reads as f64);
            self.avg_latency_us = total_latency / self.total_reads as f64;
        }

        self.last_updated = std::cmp::max(self.last_updated, other.last_updated);
    }
}

impl Default for StrategyMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Workload pattern classification
#[derive(Debug, Clone, PartialEq)]
pub enum WorkloadPattern {
    /// Sequential read pattern (full scans)
    Sequential,
    /// Random access pattern (point queries)
    Random,
    /// Search pattern (similarity queries)
    Search,
    /// Mixed pattern (combination of patterns)
    Mixed,
    /// Unknown pattern (insufficient data)
    Unknown,
}

/// Adaptive strategy configuration
#[derive(Debug, Clone)]
pub struct AdaptiveConfig {
    /// Minimum number of operations before optimization
    pub min_operations: u64,
    /// Time window for metric collection (seconds)
    pub collection_window_secs: u64,
    /// Cache hit rate threshold for cached strategies (0.0-1.0)
    pub cache_hit_threshold: f64,
    /// Latency threshold for strategy switching (microseconds)
    pub latency_threshold_us: f64,
    /// Minimum improvement required for strategy switch (percentage)
    pub min_improvement_pct: f64,
    /// Maximum number of strategy switches per hour
    pub max_switches_per_hour: u64,
    /// Minimum time between strategy switches (seconds)
    pub min_time_between_switches_secs: u64,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            min_operations: 100,
            collection_window_secs: 300, // 5 minutes
            cache_hit_threshold: 0.7,    // 70% hit rate
            latency_threshold_us: 1000.0, // 1ms
            min_improvement_pct: 10.0,   // 10% improvement
            max_switches_per_hour: 6,    // At most every 10 minutes
            min_time_between_switches_secs: 600, // 10 minutes
        }
    }
}

/// Optimizer for adaptive strategy tuning
pub struct AdaptiveStrategyOptimizer {
    /// Configuration parameters
    config: AdaptiveConfig,
    /// Metrics by collection ID
    metrics: Arc<RwLock<HashMap<String, StrategyMetrics>>>,
    /// Current strategies by collection ID
    strategies: Arc<RwLock<HashMap<String, ReadAccessStrategy>>>,
    /// Pattern detection history
    pattern_history: Arc<RwLock<HashMap<String, Vec<WorkloadPattern>>>>,
}

impl AdaptiveStrategyOptimizer {
    /// Create a new adaptive strategy optimizer
    pub fn new(config: AdaptiveConfig) -> Self {
        Self {
            config,
            metrics: Arc::new(RwLock::new(HashMap::new())),
            strategies: Arc::new(RwLock::new(HashMap::new())),
            pattern_history: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a read operation metric
    pub async fn record_read(
        &self,
        collection_id: &str,
        cache_hit: bool,
        latency_us: u64,
        bytes_read: u64,
    ) {
        let mut metrics = self.metrics.write().await;
        let collection_metrics = metrics.entry(collection_id.to_string()).or_default();

        if cache_hit {
            collection_metrics.cache_hits += 1;
        } else {
            collection_metrics.cache_misses += 1;
        }

        collection_metrics.total_reads += 1;
        collection_metrics.bytes_read += bytes_read;

        // Update moving average latency
        let current_total_latency = collection_metrics.avg_latency_us * (collection_metrics.total_reads - 1) as f64;
        collection_metrics.avg_latency_us = (current_total_latency + latency_us as f64) / collection_metrics.total_reads as f64;

        collection_metrics.last_updated = Instant::now();
    }

    /// Record a strategy switch
    pub async fn record_strategy_switch(&self, collection_id: &str, new_strategy: ReadAccessStrategy) {
        let mut metrics = self.metrics.write().await;
        let collection_metrics = metrics.entry(collection_id.to_string()).or_default();
        collection_metrics.strategy_switches += 1;

        let mut strategies = self.strategies.write().await;
        strategies.insert(collection_id.to_string(), new_strategy);

        debug!("Strategy switch recorded for collection {}: switches = {}",
               collection_id, collection_metrics.strategy_switches);
    }

    /// Detect workload pattern based on recent metrics
    pub async fn detect_pattern(&self, collection_id: &str) -> WorkloadPattern {
        let metrics = self.metrics.read().await;
        let collection_metrics = match metrics.get(collection_id) {
            Some(metrics) => metrics,
            None => return WorkloadPattern::Unknown,
        };

        // Not enough data to determine pattern
        if collection_metrics.total_reads < self.config.min_operations {
            return WorkloadPattern::Unknown;
        }

        let hit_rate = collection_metrics.hit_rate();
        let avg_latency = collection_metrics.avg_latency_us;

        // Pattern classification logic
        let pattern = if hit_rate > 0.8 && avg_latency < 500.0 {
            // High hit rate, low latency = Search pattern
            WorkloadPattern::Search
        } else if hit_rate < 0.3 && avg_latency > 2000.0 {
            // Low hit rate, high latency = Sequential pattern
            WorkloadPattern::Sequential
        } else if hit_rate > 0.6 && avg_latency < 1000.0 {
            // Medium hit rate, low latency = Random access
            WorkloadPattern::Random
        } else {
            // Mixed characteristics
            WorkloadPattern::Mixed
        };

        // Update pattern history
        let mut history = self.pattern_history.write().await;
        let collection_history = history.entry(collection_id.to_string()).or_default();
        collection_history.push(pattern.clone());

        // Keep only recent history (last 10 detections)
        if collection_history.len() > 10 {
            collection_history.remove(0);
        }

        debug!("Detected pattern for {}: {:?} (hit_rate: {:.2}, latency: {:.2}μs)",
               collection_id, pattern, hit_rate, avg_latency);

        pattern
    }

    /// Optimize strategy for a collection based on current metrics
    pub async fn optimize_strategy(&self, collection_id: &str) -> Option<ReadAccessStrategy> {
        let pattern = self.detect_pattern(collection_id).await;
        let current_strategy = {
            let strategies = self.strategies.read().await;
            strategies.get(collection_id).cloned()
        };

        // Check if we should switch strategies based on pattern
        let recommended_strategy = match pattern {
            WorkloadPattern::Sequential => ReadAccessStrategy::DirectStream,
            WorkloadPattern::Random => ReadAccessStrategy::CachedSelective { filter: None },
            WorkloadPattern::Search => ReadAccessStrategy::CachedSearch { prefetch_metadata: true },
            WorkloadPattern::Mixed => ReadAccessStrategy::Adaptive {
                initial_strategy: Box::new(ReadAccessStrategy::CachedSearch { prefetch_metadata: true }),
                fallback_threshold: self.calculate_optimal_threshold(collection_id).await,
            },
            WorkloadPattern::Unknown => return None,
        };

        // Don't switch if already using the recommended strategy
        if let Some(ref current) = current_strategy {
            if std::mem::discriminant(current) == std::mem::discriminant(&recommended_strategy) {
                return None;
            }
        }

        // Check rate limiting for strategy switches
        if !self.should_allow_switch(collection_id).await {
            debug!("Rate limiting strategy switch for collection {}", collection_id);
            return None;
        }

        // Validate that the switch would provide sufficient improvement
        if self.would_improve_performance(collection_id, &recommended_strategy).await {
            info!("Recommending strategy switch for collection {} to {:?}",
                  collection_id, recommended_strategy);
            Some(recommended_strategy)
        } else {
            debug!("Strategy switch would not provide sufficient improvement for {}", collection_id);
            None
        }
    }

    /// Calculate optimal threshold for adaptive strategy
    async fn calculate_optimal_threshold(&self, collection_id: &str) -> usize {
        let metrics = self.metrics.read().await;
        let collection_metrics = match metrics.get(collection_id) {
            Some(metrics) => metrics,
            None => return 5, // Default threshold
        };

        let hit_rate = collection_metrics.hit_rate();

        // Adjust threshold based on cache hit rate
        if hit_rate > 0.8 {
            10 // High hit rate, allow more misses before switching
        } else if hit_rate > 0.5 {
            7  // Medium hit rate
        } else {
            3  // Low hit rate, switch quickly to direct reads
        }
    }

    /// Check if strategy switch should be allowed (rate limiting)
    async fn should_allow_switch(&self, collection_id: &str) -> bool {
        let metrics = self.metrics.read().await;
        let collection_metrics = match metrics.get(collection_id) {
            Some(metrics) => metrics,
            None => return true,
        };

        // Check if we've exceeded the maximum switches per hour
        let hour_ago = Instant::now() - Duration::from_secs(3600);
        if collection_metrics.last_updated > hour_ago {
            let switches_per_hour = collection_metrics.strategy_switches;
            if switches_per_hour >= self.config.max_switches_per_hour {
                return false;
            }
        }

        // Check minimum time since last update
        let min_time_between_switches = Duration::from_secs(self.config.min_time_between_switches_secs);
        collection_metrics.last_updated.elapsed() >= min_time_between_switches
    }

    /// Predict if strategy switch would improve performance
    async fn would_improve_performance(
        &self,
        collection_id: &str,
        new_strategy: &ReadAccessStrategy
    ) -> bool {
        let metrics = self.metrics.read().await;
        let collection_metrics = match metrics.get(collection_id) {
            Some(metrics) => metrics,
            None => return true, // No data, allow the switch
        };

        // Logic: evaluate if the new strategy is appropriate for current performance characteristics
        match new_strategy {
            ReadAccessStrategy::DirectStream => {
                // Switch to direct if cache hit rate is very low or latency is very high
                collection_metrics.hit_rate() < 0.3 || collection_metrics.avg_latency_us > 2000.0
            }
            ReadAccessStrategy::CachedSearch { .. } => {
                // Cached search is appropriate for high hit rates and low latencies (search workloads)
                collection_metrics.hit_rate() > 0.6 && collection_metrics.avg_latency_us < 1000.0
            }
            ReadAccessStrategy::CachedSelective { .. } => {
                // Cached selective is appropriate for medium hit rates
                collection_metrics.hit_rate() > 0.4 && collection_metrics.avg_latency_us < 1500.0
            }
            ReadAccessStrategy::Adaptive { .. } => {
                // Adaptive is always worth trying for mixed workloads
                true
            }
            ReadAccessStrategy::CachedMetadataOnly => {
                // Metadata-only caching for specific use cases
                collection_metrics.avg_latency_us > self.config.latency_threshold_us
            }
        }
    }

    /// Get current metrics for a collection
    pub async fn get_metrics(&self, collection_id: &str) -> Option<StrategyMetrics> {
        let metrics = self.metrics.read().await;
        metrics.get(collection_id).cloned()
    }

    /// Get current strategy for a collection
    pub async fn get_current_strategy(&self, collection_id: &str) -> Option<ReadAccessStrategy> {
        let strategies = self.strategies.read().await;
        strategies.get(collection_id).cloned()
    }

    /// Get pattern history for a collection
    pub async fn get_pattern_history(&self, collection_id: &str) -> Vec<WorkloadPattern> {
        let history = self.pattern_history.read().await;
        history.get(collection_id).cloned().unwrap_or_default()
    }

    /// Reset metrics for a collection (useful for testing)
    pub async fn reset_metrics(&self, collection_id: &str) {
        let mut metrics = self.metrics.write().await;
        metrics.remove(collection_id);

        let mut strategies = self.strategies.write().await;
        strategies.remove(collection_id);

        let mut history = self.pattern_history.write().await;
        history.remove(collection_id);
    }

    /// Get global optimization recommendations
    pub async fn get_global_recommendations(&self) -> HashMap<String, ReadAccessStrategy> {
        let mut recommendations = HashMap::new();

        let collection_ids: Vec<String> = {
            let metrics = self.metrics.read().await;
            metrics.keys().cloned().collect()
        };

        for collection_id in collection_ids {
            if let Some(strategy) = self.optimize_strategy(&collection_id).await {
                recommendations.insert(collection_id, strategy);
            }
        }

        recommendations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_recording() {
        let optimizer = AdaptiveStrategyOptimizer::new(AdaptiveConfig::default());

        // Record some operations
        optimizer.record_read("test_collection", true, 100, 1024).await;
        optimizer.record_read("test_collection", false, 200, 2048).await;
        optimizer.record_read("test_collection", true, 150, 1536).await;

        let metrics = optimizer.get_metrics("test_collection").await.unwrap();
        assert_eq!(metrics.total_reads, 3);
        assert_eq!(metrics.cache_hits, 2);
        assert_eq!(metrics.cache_misses, 1);
        assert_eq!(metrics.hit_rate(), 2.0/3.0);
    }

    #[tokio::test]
    async fn test_pattern_detection() {
        let optimizer = AdaptiveStrategyOptimizer::new(AdaptiveConfig {
            min_operations: 5,
            ..Default::default()
        });

        // Simulate search pattern (high hit rate, low latency)
        for _ in 0..10 {
            optimizer.record_read("search_collection", true, 50, 1024).await;
        }
        optimizer.record_read("search_collection", false, 100, 1024).await;

        let pattern = optimizer.detect_pattern("search_collection").await;
        assert_eq!(pattern, WorkloadPattern::Search);

        // Simulate sequential pattern (low hit rate, high latency)
        for _ in 0..10 {
            optimizer.record_read("seq_collection", false, 3000, 1024).await;
        }

        let pattern = optimizer.detect_pattern("seq_collection").await;
        assert_eq!(pattern, WorkloadPattern::Sequential);
    }

    #[tokio::test]
    async fn test_strategy_optimization() {
        let optimizer = AdaptiveStrategyOptimizer::new(AdaptiveConfig {
            min_operations: 5,
            max_switches_per_hour: 100, // Allow many switches for testing
            min_time_between_switches_secs: 0, // No time constraint for testing
            ..Default::default()
        });

        // Simulate search workload
        for _ in 0..10 {
            optimizer.record_read("test_collection", true, 50, 1024).await;
        }

        let recommended = optimizer.optimize_strategy("test_collection").await;
        assert!(matches!(recommended, Some(ReadAccessStrategy::CachedSearch { .. })));
    }

    #[test]
    fn test_metrics_merge() {
        let mut metrics1 = StrategyMetrics {
            cache_hits: 10,
            cache_misses: 5,
            total_reads: 15,
            avg_latency_us: 100.0,
            bytes_read: 1024,
            strategy_switches: 1,
            last_updated: Instant::now(),
        };

        let metrics2 = StrategyMetrics {
            cache_hits: 20,
            cache_misses: 10,
            total_reads: 30,
            avg_latency_us: 200.0,
            bytes_read: 2048,
            strategy_switches: 2,
            last_updated: Instant::now(),
        };

        metrics1.merge(&metrics2);

        assert_eq!(metrics1.cache_hits, 30);
        assert_eq!(metrics1.cache_misses, 15);
        assert_eq!(metrics1.total_reads, 45);
        assert_eq!(metrics1.bytes_read, 3072);
        assert_eq!(metrics1.strategy_switches, 3);
    }
}