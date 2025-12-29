//! # Rollup Materialized Views
//!
//! Pre-computed aggregations for fast metric queries.
//! Supports multiple rollup intervals (1min, 5min, 1hr, 1day).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Rollup interval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RollupInterval {
    /// Raw data (no rollup)
    Raw,
    /// 1 minute aggregation
    OneMinute,
    /// 5 minute aggregation
    FiveMinutes,
    /// 1 hour aggregation
    OneHour,
    /// 1 day aggregation
    OneDay,
}

impl RollupInterval {
    /// Get duration in seconds
    pub fn duration_secs(&self) -> i64 {
        match self {
            RollupInterval::Raw => 0,
            RollupInterval::OneMinute => 60,
            RollupInterval::FiveMinutes => 300,
            RollupInterval::OneHour => 3600,
            RollupInterval::OneDay => 86400,
        }
    }

    /// Get display name
    pub fn name(&self) -> &'static str {
        match self {
            RollupInterval::Raw => "raw",
            RollupInterval::OneMinute => "1m",
            RollupInterval::FiveMinutes => "5m",
            RollupInterval::OneHour => "1h",
            RollupInterval::OneDay => "1d",
        }
    }

    /// Get all standard intervals
    pub fn all() -> Vec<RollupInterval> {
        vec![
            RollupInterval::Raw,
            RollupInterval::OneMinute,
            RollupInterval::FiveMinutes,
            RollupInterval::OneHour,
            RollupInterval::OneDay,
        ]
    }

    /// Select appropriate interval for a query time range
    pub fn select_for_range(start_ns: i64, end_ns: i64) -> RollupInterval {
        let duration_secs = (end_ns - start_ns) / 1_000_000_000;

        match duration_secs {
            0..=300 => RollupInterval::Raw, // Up to 5 min -> raw
            301..=3600 => RollupInterval::OneMinute, // Up to 1 hour -> 1m
            3601..=86400 => RollupInterval::FiveMinutes, // Up to 1 day -> 5m
            86401..=604800 => RollupInterval::OneHour, // Up to 1 week -> 1h
            _ => RollupInterval::OneDay, // More than 1 week -> 1d
        }
    }
}

/// Aggregation function for rollups
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregationFunction {
    Sum,
    Count,
    Avg,
    Min,
    Max,
    P50,
    P90,
    P95,
    P99,
}

impl AggregationFunction {
    /// Get display name
    pub fn name(&self) -> &'static str {
        match self {
            AggregationFunction::Sum => "sum",
            AggregationFunction::Count => "count",
            AggregationFunction::Avg => "avg",
            AggregationFunction::Min => "min",
            AggregationFunction::Max => "max",
            AggregationFunction::P50 => "p50",
            AggregationFunction::P90 => "p90",
            AggregationFunction::P95 => "p95",
            AggregationFunction::P99 => "p99",
        }
    }
}

/// Configuration for rollup manager
#[derive(Debug, Clone)]
pub struct RollupConfig {
    /// Enabled rollup intervals
    pub intervals: Vec<RollupInterval>,
    /// Default aggregation functions
    pub default_functions: Vec<AggregationFunction>,
    /// Retention per interval (in seconds)
    pub retention: HashMap<RollupInterval, i64>,
    /// Batch size for rollup computation
    pub batch_size: usize,
    /// Enable async rollup computation
    pub async_computation: bool,
}

impl Default for RollupConfig {
    fn default() -> Self {
        let mut retention = HashMap::new();
        retention.insert(RollupInterval::Raw, 7 * 86400); // 7 days
        retention.insert(RollupInterval::OneMinute, 14 * 86400); // 14 days
        retention.insert(RollupInterval::FiveMinutes, 30 * 86400); // 30 days
        retention.insert(RollupInterval::OneHour, 90 * 86400); // 90 days
        retention.insert(RollupInterval::OneDay, 365 * 86400); // 1 year

        Self {
            intervals: vec![
                RollupInterval::OneMinute,
                RollupInterval::FiveMinutes,
                RollupInterval::OneHour,
                RollupInterval::OneDay,
            ],
            default_functions: vec![
                AggregationFunction::Sum,
                AggregationFunction::Count,
                AggregationFunction::Avg,
                AggregationFunction::Min,
                AggregationFunction::Max,
            ],
            retention,
            batch_size: 10000,
            async_computation: true,
        }
    }
}

/// A single rollup view
#[derive(Debug, Clone)]
pub struct RollupView {
    /// Metric name
    pub metric_name: String,
    /// Rollup interval
    pub interval: RollupInterval,
    /// Label set (for grouping)
    pub labels: HashMap<String, String>,
    /// Aggregated values per function
    pub values: HashMap<AggregationFunction, f64>,
    /// Start time of this rollup bucket
    pub bucket_start_ns: i64,
    /// End time of this rollup bucket
    pub bucket_end_ns: i64,
    /// Number of raw data points in this bucket
    pub sample_count: u64,
}

impl RollupView {
    /// Create a new rollup view
    pub fn new(
        metric_name: String,
        interval: RollupInterval,
        labels: HashMap<String, String>,
        bucket_start_ns: i64,
    ) -> Self {
        let bucket_end_ns = bucket_start_ns + (interval.duration_secs() * 1_000_000_000);

        Self {
            metric_name,
            interval,
            labels,
            values: HashMap::new(),
            bucket_start_ns,
            bucket_end_ns,
            sample_count: 0,
        }
    }

    /// Set aggregated value
    pub fn set_value(&mut self, func: AggregationFunction, value: f64) {
        self.values.insert(func, value);
    }

    /// Get aggregated value
    pub fn get_value(&self, func: AggregationFunction) -> Option<f64> {
        self.values.get(&func).copied()
    }
}

/// Rollup manager handles materialized view computation
pub struct RollupManager {
    /// Namespace
    namespace: String,
    /// Configuration
    config: RollupConfig,
    /// Pending rollups (metric -> interval -> views)
    pending_rollups: RwLock<HashMap<String, HashMap<RollupInterval, Vec<RollupView>>>>,
    /// Last rollup timestamp per interval
    last_rollup_time: RwLock<HashMap<RollupInterval, i64>>,
    /// Total rollups computed
    total_rollups: std::sync::atomic::AtomicU64,
}

impl RollupManager {
    /// Create a new rollup manager
    pub fn new(namespace: String, config: RollupConfig) -> Self {
        Self {
            namespace,
            config,
            pending_rollups: RwLock::new(HashMap::new()),
            last_rollup_time: RwLock::new(HashMap::new()),
            total_rollups: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Add a data point to be rolled up
    pub async fn add_data_point(
        &self,
        metric_name: &str,
        labels: HashMap<String, String>,
        timestamp_ns: i64,
        value: f64,
    ) -> Result<()> {
        for interval in &self.config.intervals {
            let bucket_duration_ns = interval.duration_secs() * 1_000_000_000;
            let bucket_start_ns = (timestamp_ns / bucket_duration_ns) * bucket_duration_ns;

            // Create or update rollup view
            let mut pending = self.pending_rollups.write().await;
            let metric_rollups = pending.entry(metric_name.to_string()).or_default();
            let interval_rollups = metric_rollups.entry(*interval).or_default();

            // Find or create view for this bucket and labels
            if let Some(view) = interval_rollups.iter_mut().find(|v| {
                v.bucket_start_ns == bucket_start_ns && v.labels == labels
            }) {
                // Update existing view (simplified - real impl would maintain running aggregates)
                view.sample_count += 1;

                // Update aggregations (simplified)
                if let Some(current_sum) = view.values.get(&AggregationFunction::Sum) {
                    view.values.insert(AggregationFunction::Sum, current_sum + value);
                } else {
                    view.values.insert(AggregationFunction::Sum, value);
                }

                view.values.insert(AggregationFunction::Count, view.sample_count as f64);

                if let Some(&current_min) = view.values.get(&AggregationFunction::Min) {
                    view.values.insert(AggregationFunction::Min, current_min.min(value));
                } else {
                    view.values.insert(AggregationFunction::Min, value);
                }

                if let Some(&current_max) = view.values.get(&AggregationFunction::Max) {
                    view.values.insert(AggregationFunction::Max, current_max.max(value));
                } else {
                    view.values.insert(AggregationFunction::Max, value);
                }
            } else {
                // Create new view
                let mut view = RollupView::new(
                    metric_name.to_string(),
                    *interval,
                    labels.clone(),
                    bucket_start_ns,
                );
                view.sample_count = 1;
                view.values.insert(AggregationFunction::Sum, value);
                view.values.insert(AggregationFunction::Count, 1.0);
                view.values.insert(AggregationFunction::Min, value);
                view.values.insert(AggregationFunction::Max, value);

                interval_rollups.push(view);
            }
        }

        self.total_rollups.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Get rollups for a metric and time range
    pub async fn query_rollups(
        &self,
        metric_name: &str,
        start_ns: i64,
        end_ns: i64,
        labels: Option<&HashMap<String, String>>,
    ) -> Vec<RollupView> {
        // Select appropriate interval
        let interval = RollupInterval::select_for_range(start_ns, end_ns);

        let pending = self.pending_rollups.read().await;

        if let Some(metric_rollups) = pending.get(metric_name) {
            // Try selected interval first, fall back to finer granularity
            for check_interval in [
                interval,
                RollupInterval::FiveMinutes,
                RollupInterval::OneMinute,
                RollupInterval::Raw,
            ] {
                if let Some(views) = metric_rollups.get(&check_interval) {
                    let mut result: Vec<RollupView> = views
                        .iter()
                        .filter(|view| {
                            view.bucket_start_ns <= end_ns && view.bucket_end_ns >= start_ns
                        })
                        .filter(|view| {
                            if let Some(filter_labels) = labels {
                                filter_labels.iter().all(|(k, filter_val)| {
                                    view.labels.get(k) == Some(filter_val)
                                })
                            } else {
                                true
                            }
                        })
                        .cloned()
                        .collect();

                    if !result.is_empty() {
                        result.sort_by_key(|v| v.bucket_start_ns);
                        return result;
                    }
                }
            }
        }

        Vec::new()
    }

    /// Flush completed rollups to storage
    pub async fn flush(&self) -> Result<usize> {
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;

        let mut total_flushed = 0;

        for interval in &self.config.intervals {
            let bucket_duration_ns = interval.duration_secs() * 1_000_000_000;
            let current_bucket_start = (now_ns / bucket_duration_ns) * bucket_duration_ns;

            // Flush all completed buckets (not the current one)
            let mut pending = self.pending_rollups.write().await;

            for (_, metric_rollups) in pending.iter_mut() {
                if let Some(views) = metric_rollups.get_mut(interval) {
                    // Remove completed buckets
                    let initial_len = views.len();
                    views.retain(|v| v.bucket_start_ns >= current_bucket_start);
                    total_flushed += initial_len - views.len();
                }
            }
        }

        if total_flushed > 0 {
            debug!("Flushed {} rollup buckets for namespace {}", total_flushed, self.namespace);
        }

        Ok(total_flushed)
    }

    /// Get configuration
    pub fn config(&self) -> &RollupConfig {
        &self.config
    }

    /// Get total rollups processed
    pub fn total_rollups(&self) -> u64 {
        self.total_rollups.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get namespace
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rollup_interval_duration() {
        assert_eq!(RollupInterval::OneMinute.duration_secs(), 60);
        assert_eq!(RollupInterval::OneHour.duration_secs(), 3600);
        assert_eq!(RollupInterval::OneDay.duration_secs(), 86400);
    }

    #[test]
    fn test_rollup_interval_selection() {
        let ns_per_sec = 1_000_000_000i64;

        // 1 minute range -> raw
        assert_eq!(
            RollupInterval::select_for_range(0, 60 * ns_per_sec),
            RollupInterval::Raw
        );

        // 30 minute range -> 1m
        assert_eq!(
            RollupInterval::select_for_range(0, 30 * 60 * ns_per_sec),
            RollupInterval::OneMinute
        );

        // 12 hour range -> 5m
        assert_eq!(
            RollupInterval::select_for_range(0, 12 * 3600 * ns_per_sec),
            RollupInterval::FiveMinutes
        );

        // 3 day range -> 1h
        assert_eq!(
            RollupInterval::select_for_range(0, 3 * 86400 * ns_per_sec),
            RollupInterval::OneHour
        );

        // 30 day range -> 1d
        assert_eq!(
            RollupInterval::select_for_range(0, 30 * 86400 * ns_per_sec),
            RollupInterval::OneDay
        );
    }

    #[test]
    fn test_rollup_config_default() {
        let config = RollupConfig::default();
        assert!(!config.intervals.is_empty());
        assert!(config.retention.contains_key(&RollupInterval::OneHour));
    }

    #[tokio::test]
    async fn test_add_data_point() {
        let manager = RollupManager::new("test".to_string(), RollupConfig::default());

        let labels = HashMap::from([
            ("host".to_string(), "server1".to_string()),
        ]);

        manager.add_data_point("cpu_usage", labels.clone(), 0, 50.0).await.unwrap();
        manager.add_data_point("cpu_usage", labels.clone(), 1_000_000, 60.0).await.unwrap();

        assert_eq!(manager.total_rollups(), 2);
    }

    #[tokio::test]
    async fn test_query_rollups() {
        let manager = RollupManager::new("test".to_string(), RollupConfig::default());

        let labels = HashMap::from([
            ("host".to_string(), "server1".to_string()),
        ]);

        // Add some data points
        for i in 0..10 {
            manager
                .add_data_point("memory", labels.clone(), i * 1_000_000_000, (i as f64) * 10.0)
                .await
                .unwrap();
        }

        // Query rollups
        let rollups = manager.query_rollups("memory", 0, 10 * 1_000_000_000, Some(&labels)).await;

        // Should have at least one rollup
        assert!(!rollups.is_empty());
    }

    #[test]
    fn test_rollup_view_creation() {
        let labels = HashMap::from([
            ("env".to_string(), "prod".to_string()),
        ]);

        let mut view = RollupView::new(
            "requests".to_string(),
            RollupInterval::OneMinute,
            labels,
            60_000_000_000, // 1 minute in ns
        );

        view.set_value(AggregationFunction::Sum, 100.0);
        view.set_value(AggregationFunction::Count, 10.0);

        assert_eq!(view.get_value(AggregationFunction::Sum), Some(100.0));
        assert_eq!(view.get_value(AggregationFunction::Count), Some(10.0));
        assert_eq!(view.get_value(AggregationFunction::Avg), None);
    }
}
