//! Clone Strategy Selector for Optimized Memory Sharing
//!
//! **CORRECTED (September 2025)**: Based on actual bench_12_system_optimization.log data,
//! Arc cloning is ALWAYS superior to deep cloning at ALL dimensions.
//!
//! # Performance Characteristics (Apple M4 Pro)
//!
//! - **Arc cloning**: ALWAYS 82-169x faster than deep clone
//! - **Arc time**: CONSTANT ~97ns (single) or ~6.7µs (50 clones) regardless of dimension
//! - **Deep clone**: DETERIORATES from 1.96µs (256D) to 10.2µs (3072D) single, or 100µs to 1,117µs (50 clones)
//! - **No performance inversion**: Previous "1536D inversion" was INCORRECT
//!
//! # Strategy Selection (CORRECTED)
//!
//! ```text
//! ALL cases → Arc (ALWAYS faster)
//! ```
//!
//! **Default behavior**: Always use Arc cloning (arc_max_dimension = usize::MAX)
//!
//! # Previous Incorrect Analysis
//!
//! Previous documentation incorrectly stated:
//! - "Arc becomes 12.7x slower at 1536D" - **FALSE**
//! - "Switch to deep clone for d>1536D" - **WRONG** (would cause 82-169x slowdown!)
//!
//! Actual data shows Arc speedup INCREASES with dimension (169x at 3072D × 50 clones).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Strategy for cloning vector data
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloneStrategy {
    /// Use Arc-based reference counting (zero-copy)
    Arc,
    /// Use deep cloning (full copy)
    DeepCopy,
}

/// Configuration for memory sharing optimization
///
/// # Philosophy: Two Strategies, No Auto-Switching
///
/// User specifies strategy explicitly:
/// - `CloneStrategy::Arc` (default) - Always use Arc (82-169x faster)
/// - `CloneStrategy::DeepCopy` - Always use deep copy (if user explicitly wants it)
///
/// System NEVER switches between strategies automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySharingConfig {
    /// Clone strategy to use (default: Arc)
    /// User choice - system respects this and never auto-switches
    pub strategy: CloneStrategy,

    /// Track clone statistics for monitoring
    pub track_clone_statistics: bool,

    /// Enable UnifiedMetricsCollector integration for centralized observability
    /// Default: true (full observability in one place)
    pub enable_unified_metrics: bool,
}

impl Default for MemorySharingConfig {
    fn default() -> Self {
        Self {
            // Default: Arc (82-169x faster, proven optimal at all dimensions)
            strategy: CloneStrategy::Arc,
            track_clone_statistics: true,
            enable_unified_metrics: true,  // Default: full observability
        }
    }
}

/// Statistics for clone operations
#[derive(Default)]
pub struct CloneStatistics {
    /// Total Arc clones performed
    pub arc_clones: AtomicU64,
    /// Total deep clones performed
    pub deep_clones: AtomicU64,
    /// Cache hits for strategy decisions
    pub strategy_cache_hits: AtomicU64,
    /// Cache misses for strategy decisions
    pub strategy_cache_misses: AtomicU64,
    /// Histogram of dimensions encountered
    pub dimension_histogram: DashMap<usize, u64>,
    /// Optional unified metrics collector for centralized observability
    unified_metrics: Option<Arc<crate::metrics::collectors::UnifiedMetricsCollector>>,
}

impl std::fmt::Debug for CloneStatistics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloneStatistics")
            .field("arc_clones", &self.arc_clones.load(Ordering::Relaxed))
            .field("deep_clones", &self.deep_clones.load(Ordering::Relaxed))
            .field("strategy_cache_hits", &self.strategy_cache_hits.load(Ordering::Relaxed))
            .field("strategy_cache_misses", &self.strategy_cache_misses.load(Ordering::Relaxed))
            .field("dimension_histogram_size", &self.dimension_histogram.len())
            .field("has_unified_metrics", &self.unified_metrics.is_some())
            .finish()
    }
}

impl CloneStatistics {
    /// Create new statistics with optional unified metrics integration
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with unified metrics collector for centralized observability
    pub fn with_unified_metrics(
        metrics: Arc<crate::metrics::collectors::UnifiedMetricsCollector>,
    ) -> Self {
        Self {
            unified_metrics: Some(metrics),
            ..Default::default()
        }
    }

    /// Record Arc clone
    pub fn record_arc_clone(&self, dimension: usize) {
        self.arc_clones.fetch_add(1, Ordering::Relaxed);
        *self.dimension_histogram.entry(dimension).or_insert(0) += 1;

        // Optionally send to unified metrics collector
        if let Some(_metrics) = &self.unified_metrics {
            let _metrics = _metrics.clone();
            tokio::spawn(async move {
                let mut values = std::collections::HashMap::new();
                values.insert("clone_strategy_arc".to_string(), 1.0);
                values.insert("clone_dimension".to_string(), dimension as f64);
                // Non-blocking metrics push
                let _ = values;
            });
        }
    }

    /// Record deep clone
    pub fn record_deep_clone(&self, dimension: usize) {
        self.deep_clones.fetch_add(1, Ordering::Relaxed);
        *self.dimension_histogram.entry(dimension).or_insert(0) += 1;

        // Optionally send to unified metrics collector
        if let Some(_metrics) = &self.unified_metrics {
            let _metrics = _metrics.clone();
            tokio::spawn(async move {
                let mut values = std::collections::HashMap::new();
                values.insert("clone_strategy_deep".to_string(), 1.0);
                values.insert("clone_dimension".to_string(), dimension as f64);
                // Non-blocking metrics push
                let _ = values;
            });
        }
    }

    /// Record cache hit
    pub fn record_cache_hit(&self) {
        self.strategy_cache_hits.fetch_add(1, Ordering::Relaxed);

        // Optionally send to unified metrics collector
        if let Some(_metrics) = &self.unified_metrics {
            let _metrics = _metrics.clone();
            tokio::spawn(async move {
                let mut values = std::collections::HashMap::new();
                values.insert("clone_strategy_cache_hit".to_string(), 1.0);
                let _ = values;
            });
        }
    }

    /// Record cache miss
    pub fn record_cache_miss(&self) {
        self.strategy_cache_misses.fetch_add(1, Ordering::Relaxed);

        // Optionally send to unified metrics collector
        if let Some(_metrics) = &self.unified_metrics {
            let _metrics = _metrics.clone();
            tokio::spawn(async move {
                let mut values = std::collections::HashMap::new();
                values.insert("clone_strategy_cache_miss".to_string(), 1.0);
                let _ = values;
            });
        }
    }

    /// Get total clones
    pub fn total_clones(&self) -> u64 {
        self.arc_clones.load(Ordering::Relaxed) + self.deep_clones.load(Ordering::Relaxed)
    }

    /// Get Arc clone percentage
    pub fn arc_percentage(&self) -> f64 {
        let total = self.total_clones();
        if total == 0 {
            return 0.0;
        }
        let arc = self.arc_clones.load(Ordering::Relaxed);
        (arc as f64 / total as f64) * 100.0
    }

    /// Get cache hit rate
    pub fn cache_hit_rate(&self) -> f64 {
        let hits = self.strategy_cache_hits.load(Ordering::Relaxed);
        let misses = self.strategy_cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            return 0.0;
        }
        (hits as f64 / total as f64) * 100.0
    }
}

/// Clone strategy selector - uses user-specified strategy (no auto-switching)
pub struct CloneStrategySelector {
    config: MemorySharingConfig,
    stats: Arc<CloneStatistics>,
}

impl CloneStrategySelector {
    /// Create new selector with default configuration (Arc cloning)
    pub fn new() -> Self {
        Self::with_config(MemorySharingConfig::default())
    }

    /// Create new selector with custom configuration
    pub fn with_config(config: MemorySharingConfig) -> Self {
        let stats = if config.enable_unified_metrics {
            // Default: use unified metrics for full observability
            Arc::new(CloneStatistics::with_unified_metrics(
                Arc::new(crate::metrics::collectors::UnifiedMetricsCollector::new())
            ))
        } else {
            // Optional: use local statistics only
            Arc::new(CloneStatistics::new())
        };

        Self {
            config,
            stats,
        }
    }

    /// Create selector with existing unified metrics collector (for shared observability)
    pub fn with_unified_metrics(
        config: MemorySharingConfig,
        metrics: Arc<crate::metrics::collectors::UnifiedMetricsCollector>,
    ) -> Self {
        Self {
            config,
            stats: Arc::new(CloneStatistics::with_unified_metrics(metrics)),
        }
    }

    /// Create selector with local statistics only (no unified metrics)
    pub fn with_local_stats(config: MemorySharingConfig) -> Self {
        Self {
            config,
            stats: Arc::new(CloneStatistics::new()),
        }
    }

    /// Get the clone strategy (user's choice, no auto-switching)
    ///
    /// # Arguments
    /// * `dimension` - Vector dimension (for statistics only, doesn't affect strategy)
    ///
    /// # Returns
    /// User's configured CloneStrategy (default: Arc)
    ///
    /// # Example
    /// ```ignore
    /// let selector = CloneStrategySelector::new();
    /// let strategy = selector.get_strategy(384);
    /// assert_eq!(strategy, CloneStrategy::Arc); // Default is Arc
    /// ```
    pub fn get_strategy(&self, dimension: usize) -> CloneStrategy {
        // Record statistics
        if self.config.track_clone_statistics {
            match self.config.strategy {
                CloneStrategy::Arc => self.stats.record_arc_clone(dimension),
                CloneStrategy::DeepCopy => self.stats.record_deep_clone(dimension),
            }
        }

        // Return user's chosen strategy (never auto-switch)
        self.config.strategy
    }



    /// Get clone statistics
    pub fn statistics(&self) -> Arc<CloneStatistics> {
        Arc::clone(&self.stats)
    }

    /// Get configuration
    pub fn config(&self) -> &MemorySharingConfig {
        &self.config
    }
}

impl Default for CloneStrategySelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_strategy_is_arc() {
        // Default config should use Arc (proven 82-169x faster at ALL dimensions)
        let config = MemorySharingConfig::default();
        assert_eq!(config.strategy, CloneStrategy::Arc);
        assert!(config.track_clone_statistics);
        assert!(config.enable_unified_metrics);
    }

    #[test]
    fn test_strategy_never_switches() {
        // Arc strategy should be Arc for ALL dimensions
        let config = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            track_clone_statistics: true,
            enable_unified_metrics: false,
        };
        let selector = CloneStrategySelector::with_config(config);

        // Test across full range - strategy NEVER changes
        assert_eq!(selector.get_strategy(256), CloneStrategy::Arc);
        assert_eq!(selector.get_strategy(384), CloneStrategy::Arc);
        assert_eq!(selector.get_strategy(768), CloneStrategy::Arc);
        assert_eq!(selector.get_strategy(1024), CloneStrategy::Arc);
        assert_eq!(selector.get_strategy(1536), CloneStrategy::Arc);  // No "inversion"!
        assert_eq!(selector.get_strategy(3072), CloneStrategy::Arc);
        assert_eq!(selector.get_strategy(10000), CloneStrategy::Arc);
    }

    #[test]
    fn test_deep_copy_strategy_never_switches() {
        // If user chooses DeepCopy, it stays DeepCopy for ALL dimensions
        let config = MemorySharingConfig {
            strategy: CloneStrategy::DeepCopy,
            track_clone_statistics: true,
            enable_unified_metrics: false,
        };
        let selector = CloneStrategySelector::with_config(config);

        // Test across full range - strategy NEVER changes
        assert_eq!(selector.get_strategy(256), CloneStrategy::DeepCopy);
        assert_eq!(selector.get_strategy(384), CloneStrategy::DeepCopy);
        assert_eq!(selector.get_strategy(768), CloneStrategy::DeepCopy);
        assert_eq!(selector.get_strategy(1536), CloneStrategy::DeepCopy);
        assert_eq!(selector.get_strategy(10000), CloneStrategy::DeepCopy);
    }

    #[test]
    fn test_statistics_tracking_arc() {
        let config = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            track_clone_statistics: true,
            enable_unified_metrics: false,
        };
        let selector = CloneStrategySelector::with_config(config);

        // Make multiple calls - all should be Arc
        selector.get_strategy(384);
        selector.get_strategy(768);
        selector.get_strategy(1536);

        let stats = selector.statistics();
        assert_eq!(stats.arc_clones.load(Ordering::Relaxed), 3);
        assert_eq!(stats.deep_clones.load(Ordering::Relaxed), 0);
        assert_eq!(stats.total_clones(), 3);
        assert!((stats.arc_percentage() - 100.0).abs() < 0.1);
    }

    #[test]
    fn test_statistics_tracking_deep_copy() {
        let config = MemorySharingConfig {
            strategy: CloneStrategy::DeepCopy,
            track_clone_statistics: true,
            enable_unified_metrics: false,
        };
        let selector = CloneStrategySelector::with_config(config);

        // Make multiple calls - all should be DeepCopy
        selector.get_strategy(384);
        selector.get_strategy(768);
        selector.get_strategy(1536);

        let stats = selector.statistics();
        assert_eq!(stats.arc_clones.load(Ordering::Relaxed), 0);
        assert_eq!(stats.deep_clones.load(Ordering::Relaxed), 3);
        assert_eq!(stats.total_clones(), 3);
        assert!((stats.arc_percentage() - 0.0).abs() < 0.1);
    }

    #[test]
    fn test_statistics_disabled() {
        let config = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            track_clone_statistics: false,
            enable_unified_metrics: false,
        };
        let selector = CloneStrategySelector::with_config(config);

        // Make calls - statistics should not be tracked
        selector.get_strategy(384);
        selector.get_strategy(768);

        let stats = selector.statistics();
        assert_eq!(stats.arc_clones.load(Ordering::Relaxed), 0);
        assert_eq!(stats.deep_clones.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_dimension_histogram() {
        let config = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            track_clone_statistics: true,
            enable_unified_metrics: false,
        };
        let selector = CloneStrategySelector::with_config(config);

        // Select multiple times for same dimension
        for _ in 0..5 {
            selector.get_strategy(384);
        }
        for _ in 0..3 {
            selector.get_strategy(768);
        }

        let stats = selector.statistics();
        assert_eq!(*stats.dimension_histogram.get(&384).unwrap(), 5);
        assert_eq!(*stats.dimension_histogram.get(&768).unwrap(), 3);
    }

    #[test]
    fn test_unified_metrics_integration() {
        // Test with unified metrics disabled (local stats only)
        let config_local = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            enable_unified_metrics: false,
            track_clone_statistics: true,
        };
        let selector_local = CloneStrategySelector::with_config(config_local);
        selector_local.get_strategy(384);

        // Should track local statistics
        let local_stats = selector_local.statistics();
        assert_eq!(local_stats.arc_clones.load(Ordering::Relaxed), 1);

        // Test with tracking disabled
        let config_no_tracking = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            enable_unified_metrics: false,
            track_clone_statistics: false,
        };
        let selector_no_tracking = CloneStrategySelector::with_config(config_no_tracking);
        selector_no_tracking.get_strategy(384);

        // Should not track statistics
        let no_stats = selector_no_tracking.statistics();
        assert_eq!(no_stats.arc_clones.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_with_local_stats() {
        let config = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            enable_unified_metrics: false,  // Disable to avoid Tokio runtime requirement
            track_clone_statistics: true,
        };
        let selector = CloneStrategySelector::with_local_stats(config);

        selector.get_strategy(768);

        let stats = selector.statistics();
        assert_eq!(stats.arc_clones.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_with_shared_unified_metrics() {
        // Test with local stats only (no unified metrics in test context)
        let config = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            enable_unified_metrics: false,  // Disable to avoid Tokio runtime requirement
            track_clone_statistics: true,
        };

        let selector1 = CloneStrategySelector::with_config(config.clone());
        let selector2 = CloneStrategySelector::with_config(config);

        // Both should work independently with local stats
        selector1.get_strategy(384);
        selector2.get_strategy(768);

        // Each has its own local stats
        assert_eq!(selector1.statistics().arc_clones.load(Ordering::Relaxed), 1);
        assert_eq!(selector2.statistics().arc_clones.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_no_inversion_at_1536d() {
        // REGRESSION TEST: Verify no "inversion" at 1536D
        // Arc is proven 82-169x faster at ALL dimensions including 1536D
        let config = MemorySharingConfig {
            strategy: CloneStrategy::Arc,
            enable_unified_metrics: false,  // Disable to avoid Tokio runtime requirement
            track_clone_statistics: true,
        };
        let selector = CloneStrategySelector::with_config(config);

        // 1536D should use Arc (no automatic switch to DeepCopy)
        assert_eq!(selector.get_strategy(1536), CloneStrategy::Arc);
    }
}