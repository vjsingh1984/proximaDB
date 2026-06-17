// Time-series metric storage with downsampling
//
// Provides:
// - High-throughput metric ingestion
// - Automatic downsampling for older data
// - Label-based indexing
// - Aggregation queries
// - Progressive resolution tiering
// - Automatic resolution selection based on time range

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::rollup_persistence::{RollupPersistence, RollupPoint};
use proximadb_observability::metric_sample_to_proxima_record;
use proximadb_records::ProximaRecord;

use crate::proto::proximadb_v1::MetricSample;

/// Nanoseconds per second
const NANOS_PER_SEC: i64 = 1_000_000_000;
/// Nanoseconds per minute
const NANOS_PER_MIN: i64 = 60 * NANOS_PER_SEC;
/// Nanoseconds per hour
const NANOS_PER_HOUR: i64 = 60 * NANOS_PER_MIN;
/// Nanoseconds per day
const NANOS_PER_DAY: i64 = 24 * NANOS_PER_HOUR;

/// Time-series metric storage
pub struct MetricStorage {
    /// Base path for storage
    #[allow(dead_code)]
    base_path: String,
    /// Observability namespace used as the canonical tenant id.
    namespace: String,
    /// Metrics by series key (metric name + labels)
    series: RwLock<HashMap<String, MetricSeries>>,
    /// Series count
    series_count: AtomicU64,
    /// Total storage bytes (estimated)
    total_bytes: AtomicU64,
    /// Tiering policy configuration
    tiering_policy: MetricTieringPolicy,
    /// Optional rollup persistence layer for durable storage
    rollup_persistence: Option<Arc<dyn RollupPersistence>>,
}

/// Policy for metric data tiering and retention
///
/// Configures how long to keep data at each resolution level
/// and when to automatically select each resolution for queries.
#[derive(Debug, Clone)]
pub struct MetricTieringPolicy {
    /// How long to keep raw data (nanoseconds)
    pub raw_retention_ns: i64,
    /// How long to keep 1-minute aggregates (nanoseconds)
    pub minute_retention_ns: i64,
    /// How long to keep 5-minute aggregates (nanoseconds)
    pub five_minute_retention_ns: i64,
    /// How long to keep 1-hour aggregates (nanoseconds)
    pub hour_retention_ns: i64,
    /// Threshold for auto-selecting minute resolution (query span in ns)
    pub minute_threshold_ns: i64,
    /// Threshold for auto-selecting 5-minute resolution (query span in ns)
    pub five_minute_threshold_ns: i64,
    /// Threshold for auto-selecting hour resolution (query span in ns)
    pub hour_threshold_ns: i64,
}

impl Default for MetricTieringPolicy {
    fn default() -> Self {
        Self {
            // Raw data: keep for 2 hours
            raw_retention_ns: 2 * NANOS_PER_HOUR,
            // 1-minute aggregates: keep for 7 days
            minute_retention_ns: 7 * NANOS_PER_DAY,
            // 5-minute aggregates: keep for 30 days
            five_minute_retention_ns: 30 * NANOS_PER_DAY,
            // 1-hour aggregates: keep for 1 year
            hour_retention_ns: 365 * NANOS_PER_DAY,
            // Use raw for queries under 1 hour
            minute_threshold_ns: NANOS_PER_HOUR,
            // Use 1-minute for queries 1-6 hours
            five_minute_threshold_ns: 6 * NANOS_PER_HOUR,
            // Use 5-minute for queries 6-24 hours, hour for longer
            hour_threshold_ns: NANOS_PER_DAY,
        }
    }
}

impl MetricTieringPolicy {
    /// Create a new tiering policy with custom retention
    #[must_use]
    pub fn new(raw_hours: u64, minute_days: u64, five_minute_days: u64, hour_days: u64) -> Self {
        Self {
            raw_retention_ns: raw_hours as i64 * NANOS_PER_HOUR,
            minute_retention_ns: minute_days as i64 * NANOS_PER_DAY,
            five_minute_retention_ns: five_minute_days as i64 * NANOS_PER_DAY,
            hour_retention_ns: hour_days as i64 * NANOS_PER_DAY,
            ..Default::default()
        }
    }

    /// Get the optimal resolution for a time range
    pub fn optimal_resolution(&self, span_ns: i64) -> DownsampleResolution {
        if span_ns < self.minute_threshold_ns {
            DownsampleResolution::Raw
        } else if span_ns < self.five_minute_threshold_ns {
            DownsampleResolution::Minute
        } else if span_ns < self.hour_threshold_ns {
            DownsampleResolution::FiveMinute
        } else {
            DownsampleResolution::Hour
        }
    }
}

/// A single metric time series
///
/// Represents a unique metric identified by name and labels.
/// Stores raw data points and downsampled aggregates.
struct MetricSeries {
    /// Metric name
    name: String,
    /// Labels
    labels: HashMap<String, String>,
    /// Data points (timestamp -> value)
    points: RwLock<BTreeMap<i64, f64>>,
    /// Downsampled data (aggregated)
    downsampled: RwLock<DownsampledData>,
}

/// Downsampled aggregates
///
/// Stores pre-aggregated data at multiple time resolutions
/// for efficient querying of large time ranges.
struct DownsampledData {
    /// 1-minute aggregates
    minute: BTreeMap<i64, AggregatedPoint>,
    /// 5-minute aggregates
    five_minute: BTreeMap<i64, AggregatedPoint>,
    /// 1-hour aggregates
    hour: BTreeMap<i64, AggregatedPoint>,
}

/// Aggregated data point
///
/// Represents aggregated statistics for a time bucket.
/// Tracks min, max, sum, and count for calculating averages.
#[derive(Debug, Clone)]
struct AggregatedPoint {
    /// Minimum value
    min: f64,
    /// Maximum value
    max: f64,
    /// Sum of values
    sum: f64,
    /// Count of values
    count: u64,
}

impl AggregatedPoint {
    fn new(value: f64) -> Self {
        Self {
            min: value,
            max: value,
            sum: value,
            count: 1,
        }
    }

    fn update(&mut self, value: f64) {
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        self.sum += value;
        self.count += 1;
    }

    fn avg(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }
}

impl MetricStorage {
    /// Create a new metric storage with default tiering policy
    pub fn new(base_path: &str) -> Result<Self> {
        Self::new_for_namespace(base_path, "default")
    }

    /// Create metric storage for a specific observability namespace.
    pub fn new_for_namespace(base_path: &str, namespace: &str) -> Result<Self> {
        Self::with_policy_for_namespace(base_path, namespace, MetricTieringPolicy::default())
    }

    /// Create a new metric storage with custom tiering policy
    pub fn with_policy(base_path: &str, policy: MetricTieringPolicy) -> Result<Self> {
        Self::with_policy_for_namespace(base_path, "default", policy)
    }

    /// Create metric storage with a custom tiering policy and namespace.
    pub fn with_policy_for_namespace(
        base_path: &str,
        namespace: &str,
        policy: MetricTieringPolicy,
    ) -> Result<Self> {
        Ok(Self {
            base_path: base_path.to_string(),
            namespace: namespace.to_string(),
            series: RwLock::new(HashMap::new()),
            series_count: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            tiering_policy: policy,
            rollup_persistence: None,
        })
    }

    /// Create a new metric storage with rollup persistence
    pub fn with_persistence(
        base_path: &str,
        policy: MetricTieringPolicy,
        persistence: Arc<dyn RollupPersistence>,
    ) -> Result<Self> {
        Self::with_persistence_for_namespace(base_path, "default", policy, persistence)
    }

    /// Create metric storage with rollup persistence for a specific namespace.
    pub fn with_persistence_for_namespace(
        base_path: &str,
        namespace: &str,
        policy: MetricTieringPolicy,
        persistence: Arc<dyn RollupPersistence>,
    ) -> Result<Self> {
        Ok(Self {
            base_path: base_path.to_string(),
            namespace: namespace.to_string(),
            series: RwLock::new(HashMap::new()),
            series_count: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            tiering_policy: policy,
            rollup_persistence: Some(persistence),
        })
    }

    /// Set rollup persistence layer
    pub fn set_rollup_persistence(&mut self, persistence: Arc<dyn RollupPersistence>) {
        self.rollup_persistence = Some(persistence);
    }

    /// Get the tiering policy
    pub fn tiering_policy(&self) -> &MetricTieringPolicy {
        &self.tiering_policy
    }

    /// Project a metric sample onto the canonical observability record shape.
    ///
    /// The projection remains rebuildable from the observability WAL and shares
    /// the modality crate mapping used by future PAX/record-store replay.
    pub fn sample_projection_record(&self, sample: &MetricSample) -> ProximaRecord {
        metric_sample_to_proxima_record(&self.namespace, sample)
    }

    /// Generate series key from name and labels
    fn series_key(name: &str, labels: &HashMap<String, String>) -> String {
        let mut sorted_labels: Vec<_> = labels.iter().collect();
        sorted_labels.sort_by_key(|(k, _)| *k);

        let label_str: String = sorted_labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");

        format!("{name}:{{{label_str}}}")
    }

    /// Write a metric sample
    pub async fn write(&self, sample: &MetricSample) -> Result<()> {
        let key = Self::series_key(&sample.name, &sample.labels);

        // Estimate sample size for storage tracking
        let sample_size = self.estimate_sample_size(sample);

        // Get or create series
        {
            let series = self.series.read().await;
            if let Some(s) = series.get(&key) {
                let mut points = s.points.write().await;
                points.insert(sample.timestamp_ns, sample.value);

                // Update downsampled data
                self.update_downsampled(s, sample.timestamp_ns, sample.value)
                    .await;

                // Track bytes
                self.total_bytes.fetch_add(sample_size, Ordering::Relaxed);
                return Ok(());
            }
        }

        // Create new series
        let sample_size = self.estimate_sample_size(sample);
        let new_series = MetricSeries {
            name: sample.name.clone(),
            labels: sample.labels.clone(),
            points: RwLock::new(BTreeMap::from([(sample.timestamp_ns, sample.value)])),
            downsampled: RwLock::new(DownsampledData {
                minute: BTreeMap::new(),
                five_minute: BTreeMap::new(),
                hour: BTreeMap::new(),
            }),
        };

        let mut series = self.series.write().await;
        series.insert(key, new_series);
        self.series_count.fetch_add(1, Ordering::Relaxed);
        self.total_bytes.fetch_add(sample_size, Ordering::Relaxed);

        Ok(())
    }

    /// Update downsampled aggregates
    async fn update_downsampled(&self, series: &MetricSeries, timestamp_ns: i64, value: f64) {
        let minute_key = (timestamp_ns / 60_000_000_000) * 60_000_000_000;
        let five_min_key = (timestamp_ns / 300_000_000_000) * 300_000_000_000;
        let hour_key = (timestamp_ns / 3_600_000_000_000) * 3_600_000_000_000;

        let mut downsampled = series.downsampled.write().await;

        // Update minute aggregate
        downsampled
            .minute
            .entry(minute_key)
            .and_modify(|p| p.update(value))
            .or_insert_with(|| AggregatedPoint::new(value));

        // Update 5-minute aggregate
        downsampled
            .five_minute
            .entry(five_min_key)
            .and_modify(|p| p.update(value))
            .or_insert_with(|| AggregatedPoint::new(value));

        // Update hour aggregate
        downsampled
            .hour
            .entry(hour_key)
            .and_modify(|p| p.update(value))
            .or_insert_with(|| AggregatedPoint::new(value));
    }

    /// Query metrics by name and time range
    pub async fn query(&self, name: &str, start_ns: i64, end_ns: i64) -> Result<Vec<MetricSample>> {
        let series = self.series.read().await;

        let mut results = Vec::new();

        for s in series.values() {
            if s.name != name {
                continue;
            }

            let points = s.points.read().await;
            for (ts, value) in points.range(start_ns..=end_ns) {
                results.push(MetricSample {
                    name: s.name.clone(),
                    timestamp_ns: *ts,
                    value: *value,
                    labels: s.labels.clone(),
                });
            }
        }

        Ok(results)
    }

    /// Query with label filters
    pub async fn query_with_labels(
        &self,
        name: &str,
        start_ns: i64,
        end_ns: i64,
        label_filters: &HashMap<String, String>,
    ) -> Result<Vec<MetricSample>> {
        let series = self.series.read().await;

        let mut results = Vec::new();

        for s in series.values() {
            if s.name != name {
                continue;
            }

            // Check label filters
            let matches = label_filters
                .iter()
                .all(|(k, v)| s.labels.get(k).is_some_and(|sv| sv == v));

            if !matches {
                continue;
            }

            let points = s.points.read().await;
            for (ts, value) in points.range(start_ns..=end_ns) {
                results.push(MetricSample {
                    name: s.name.clone(),
                    timestamp_ns: *ts,
                    value: *value,
                    labels: s.labels.clone(),
                });
            }
        }

        Ok(results)
    }

    /// Query downsampled data
    pub async fn query_downsampled(
        &self,
        name: &str,
        start_ns: i64,
        end_ns: i64,
        resolution: DownsampleResolution,
    ) -> Result<Vec<AggregatedMetric>> {
        let series = self.series.read().await;

        let mut results = Vec::new();

        for s in series.values() {
            if s.name != name {
                continue;
            }

            // Raw resolution should use query() instead of query_downsampled()
            if resolution == DownsampleResolution::Raw {
                continue;
            }

            let downsampled = s.downsampled.read().await;
            let data = match resolution {
                DownsampleResolution::Raw => continue, // Already handled above
                DownsampleResolution::Minute => &downsampled.minute,
                DownsampleResolution::FiveMinute => &downsampled.five_minute,
                DownsampleResolution::Hour => &downsampled.hour,
            };

            for (ts, point) in data.range(start_ns..=end_ns) {
                results.push(AggregatedMetric {
                    name: s.name.clone(),
                    timestamp_ns: *ts,
                    min: point.min,
                    max: point.max,
                    avg: point.avg(),
                    sum: point.sum,
                    count: point.count,
                    labels: s.labels.clone(),
                });
            }
        }

        Ok(results)
    }

    /// Get series count
    pub async fn series_count(&self) -> u64 {
        self.series_count.load(Ordering::Relaxed)
    }

    /// Get the total storage size in bytes
    pub async fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }

    /// Estimate the size of a metric sample in bytes
    fn estimate_sample_size(&self, sample: &MetricSample) -> u64 {
        let mut size = 100; // Base overhead

        // Add metric name
        size += sample.name.len() as u64;

        // Add timestamp
        size += 8;

        // Add value
        size += 8;

        // Add labels (HashMap<String, String>)
        for (key, val) in &sample.labels {
            size += key.len() as u64;
            size += val.len() as u64;
        }

        size
    }

    /// Compact old data
    pub async fn compact(&self, older_than_ns: i64) -> Result<usize> {
        let series = self.series.read().await;
        let mut compacted = 0;

        for s in series.values() {
            let mut points = s.points.write().await;
            let to_remove: Vec<_> = points
                .keys()
                .filter(|ts| **ts < older_than_ns)
                .cloned()
                .collect();

            compacted += to_remove.len();
            for ts in to_remove {
                points.remove(&ts);
            }
        }

        Ok(compacted)
    }

    /// Query with automatic resolution selection based on time range
    ///
    /// Automatically selects the optimal resolution based on the query span:
    /// - Raw data for short spans (< 1 hour)
    /// - 1-minute aggregates for medium spans (1-6 hours)
    /// - 5-minute aggregates for longer spans (6-24 hours)
    /// - 1-hour aggregates for very long spans (> 24 hours)
    pub async fn query_auto_resolution(
        &self,
        name: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<QueryAutoResult> {
        let span_ns = end_ns - start_ns;
        let resolution = self.tiering_policy.optimal_resolution(span_ns);

        debug!(
            "Auto-selected resolution {:?} for span {}ns ({:.1} hours)",
            resolution,
            span_ns,
            span_ns as f64 / NANOS_PER_HOUR as f64
        );

        match resolution {
            DownsampleResolution::Raw => {
                let samples = self.query(name, start_ns, end_ns).await?;
                Ok(QueryAutoResult {
                    resolution,
                    raw_samples: Some(samples),
                    aggregated: None,
                })
            }
            _ => {
                let aggregated = self
                    .query_downsampled(name, start_ns, end_ns, resolution)
                    .await?;
                Ok(QueryAutoResult {
                    resolution,
                    raw_samples: None,
                    aggregated: Some(aggregated),
                })
            }
        }
    }

    /// Query with automatic resolution and label filters
    pub async fn query_auto_resolution_with_labels(
        &self,
        name: &str,
        start_ns: i64,
        end_ns: i64,
        label_filters: &HashMap<String, String>,
    ) -> Result<QueryAutoResult> {
        let span_ns = end_ns - start_ns;
        let resolution = self.tiering_policy.optimal_resolution(span_ns);

        match resolution {
            DownsampleResolution::Raw => {
                let samples = self
                    .query_with_labels(name, start_ns, end_ns, label_filters)
                    .await?;
                Ok(QueryAutoResult {
                    resolution,
                    raw_samples: Some(samples),
                    aggregated: None,
                })
            }
            _ => {
                let aggregated = self
                    .query_downsampled_with_labels(
                        name,
                        start_ns,
                        end_ns,
                        resolution,
                        label_filters,
                    )
                    .await?;
                Ok(QueryAutoResult {
                    resolution,
                    raw_samples: None,
                    aggregated: Some(aggregated),
                })
            }
        }
    }

    /// Query downsampled data with label filters
    pub async fn query_downsampled_with_labels(
        &self,
        name: &str,
        start_ns: i64,
        end_ns: i64,
        resolution: DownsampleResolution,
        label_filters: &HashMap<String, String>,
    ) -> Result<Vec<AggregatedMetric>> {
        let series = self.series.read().await;
        let mut results = Vec::new();

        for s in series.values() {
            if s.name != name {
                continue;
            }

            // Check label filters
            let matches = label_filters
                .iter()
                .all(|(k, v)| s.labels.get(k).is_some_and(|sv| sv == v));

            if !matches {
                continue;
            }

            let downsampled = s.downsampled.read().await;
            let data = match resolution {
                DownsampleResolution::Raw => continue, // Should use query_with_labels instead
                DownsampleResolution::Minute => &downsampled.minute,
                DownsampleResolution::FiveMinute => &downsampled.five_minute,
                DownsampleResolution::Hour => &downsampled.hour,
            };

            for (ts, point) in data.range(start_ns..=end_ns) {
                results.push(AggregatedMetric {
                    name: s.name.clone(),
                    timestamp_ns: *ts,
                    min: point.min,
                    max: point.max,
                    avg: point.avg(),
                    sum: point.sum,
                    count: point.count,
                    labels: s.labels.clone(),
                });
            }
        }

        Ok(results)
    }

    /// Apply tiering policy to all series
    ///
    /// This method enforces retention limits for each tier:
    /// - Removes raw data older than raw_retention
    /// - Removes minute aggregates older than minute_retention
    /// - Removes 5-minute aggregates older than five_minute_retention
    /// - Removes hour aggregates older than hour_retention
    pub async fn apply_tiering_policy(&self, now_ns: i64) -> Result<TieringResult> {
        let series = self.series.read().await;
        let mut result = TieringResult::default();

        let raw_cutoff = now_ns - self.tiering_policy.raw_retention_ns;
        let minute_cutoff = now_ns - self.tiering_policy.minute_retention_ns;
        let five_minute_cutoff = now_ns - self.tiering_policy.five_minute_retention_ns;
        let hour_cutoff = now_ns - self.tiering_policy.hour_retention_ns;

        for s in series.values() {
            // Compact raw data
            {
                let mut points = s.points.write().await;
                let to_remove: Vec<_> = points
                    .keys()
                    .filter(|ts| **ts < raw_cutoff)
                    .cloned()
                    .collect();
                result.raw_removed += to_remove.len();
                for ts in to_remove {
                    points.remove(&ts);
                }
            }

            // Compact downsampled tiers
            {
                let mut downsampled = s.downsampled.write().await;

                // Minute tier
                let to_remove: Vec<_> = downsampled
                    .minute
                    .keys()
                    .filter(|ts| **ts < minute_cutoff)
                    .cloned()
                    .collect();
                result.minute_removed += to_remove.len();
                for ts in to_remove {
                    downsampled.minute.remove(&ts);
                }

                // 5-minute tier
                let to_remove: Vec<_> = downsampled
                    .five_minute
                    .keys()
                    .filter(|ts| **ts < five_minute_cutoff)
                    .cloned()
                    .collect();
                result.five_minute_removed += to_remove.len();
                for ts in to_remove {
                    downsampled.five_minute.remove(&ts);
                }

                // Hour tier
                let to_remove: Vec<_> = downsampled
                    .hour
                    .keys()
                    .filter(|ts| **ts < hour_cutoff)
                    .cloned()
                    .collect();
                result.hour_removed += to_remove.len();
                for ts in to_remove {
                    downsampled.hour.remove(&ts);
                }
            }
        }

        info!(
            "Tiering policy applied: raw={}, 1m={}, 5m={}, 1h={}",
            result.raw_removed,
            result.minute_removed,
            result.five_minute_removed,
            result.hour_removed
        );

        Ok(result)
    }

    /// Get tiering statistics for all series
    pub async fn tiering_stats(&self) -> ObservabilityTieringStats {
        let series = self.series.read().await;
        let mut stats = ObservabilityTieringStats::default();

        for s in series.values() {
            let points = s.points.read().await;
            stats.raw_points += points.len();

            let downsampled = s.downsampled.read().await;
            stats.minute_points += downsampled.minute.len();
            stats.five_minute_points += downsampled.five_minute.len();
            stats.hour_points += downsampled.hour.len();
        }

        stats.series_count = series.len();
        stats
    }

    /// Flush all in-memory rollups to persistent storage
    ///
    /// This method persists minute, five_minute, and hour aggregates to disk
    /// using the configured RollupPersistence implementation.
    pub async fn flush_rollups(&self) -> Result<RollupFlushResult> {
        let Some(ref persistence) = self.rollup_persistence else {
            return Ok(RollupFlushResult::default());
        };

        let series = self.series.read().await;
        let mut result = RollupFlushResult::default();

        for (series_key, s) in series.iter() {
            let downsampled = s.downsampled.read().await;

            // Convert AggregatedPoint to RollupPoint for minute tier
            let minute_rollups: BTreeMap<i64, RollupPoint> = downsampled
                .minute
                .iter()
                .map(|(ts, p)| {
                    (
                        *ts,
                        RollupPoint {
                            min: p.min,
                            max: p.max,
                            sum: p.sum,
                            count: p.count,
                            name: s.name.clone(),
                            labels: s.labels.clone(),
                        },
                    )
                })
                .collect();

            if !minute_rollups.is_empty() {
                match persistence
                    .flush_rollups(series_key, DownsampleResolution::Minute, &minute_rollups)
                    .await
                {
                    Ok(count) => result.minute_flushed += count,
                    Err(e) => warn!("Failed to flush minute rollups for {}: {}", series_key, e),
                }
            }

            // Convert for five_minute tier
            let five_minute_rollups: BTreeMap<i64, RollupPoint> = downsampled
                .five_minute
                .iter()
                .map(|(ts, p)| {
                    (
                        *ts,
                        RollupPoint {
                            min: p.min,
                            max: p.max,
                            sum: p.sum,
                            count: p.count,
                            name: s.name.clone(),
                            labels: s.labels.clone(),
                        },
                    )
                })
                .collect();

            if !five_minute_rollups.is_empty() {
                match persistence
                    .flush_rollups(
                        series_key,
                        DownsampleResolution::FiveMinute,
                        &five_minute_rollups,
                    )
                    .await
                {
                    Ok(count) => result.five_minute_flushed += count,
                    Err(e) => warn!(
                        "Failed to flush five_minute rollups for {}: {}",
                        series_key, e
                    ),
                }
            }

            // Convert for hour tier
            let hour_rollups: BTreeMap<i64, RollupPoint> = downsampled
                .hour
                .iter()
                .map(|(ts, p)| {
                    (
                        *ts,
                        RollupPoint {
                            min: p.min,
                            max: p.max,
                            sum: p.sum,
                            count: p.count,
                            name: s.name.clone(),
                            labels: s.labels.clone(),
                        },
                    )
                })
                .collect();

            if !hour_rollups.is_empty() {
                match persistence
                    .flush_rollups(series_key, DownsampleResolution::Hour, &hour_rollups)
                    .await
                {
                    Ok(count) => result.hour_flushed += count,
                    Err(e) => warn!("Failed to flush hour rollups for {}: {}", series_key, e),
                }
            }
        }

        result.success = true;
        info!(
            "Flushed rollups: 1m={}, 5m={}, 1h={}",
            result.minute_flushed, result.five_minute_flushed, result.hour_flushed
        );

        Ok(result)
    }

    /// Query downsampled data with persistence fallback
    ///
    /// This method first queries in-memory data, then falls back to
    /// persistent storage for older data that may have been evicted.
    pub async fn query_downsampled_with_persistence(
        &self,
        name: &str,
        start_ns: i64,
        end_ns: i64,
        resolution: DownsampleResolution,
    ) -> Result<Vec<AggregatedMetric>> {
        // First, get in-memory results
        let mut results = self
            .query_downsampled(name, start_ns, end_ns, resolution)
            .await?;

        // Check if we have persistence and the query spans potentially persisted data
        if let Some(ref persistence) = self.rollup_persistence {
            let series = self.series.read().await;

            for (series_key, s) in series.iter() {
                if s.name != name {
                    continue;
                }

                // Query persisted data
                match persistence
                    .load_rollups(series_key, resolution, start_ns, end_ns)
                    .await
                {
                    Ok(persisted) => {
                        // Merge persisted data (avoid duplicates by timestamp)
                        let existing_timestamps: std::collections::HashSet<i64> =
                            results.iter().map(|r| r.timestamp_ns).collect();

                        for metric in persisted {
                            if !existing_timestamps.contains(&metric.timestamp_ns) {
                                results.push(metric);
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Failed to load persisted rollups for {}: {}", series_key, e);
                    }
                }
            }
        }

        // Sort by timestamp
        results.sort_by_key(|r| r.timestamp_ns);

        Ok(results)
    }

    /// Apply tiering policy and flush to persistence
    ///
    /// This method applies the tiering policy to evict old data from memory
    /// while ensuring it's first persisted to disk.
    pub async fn apply_tiering_policy_with_persistence(
        &self,
        now_ns: i64,
    ) -> Result<TieringResult> {
        // First, flush rollups to persistence (if configured)
        if self.rollup_persistence.is_some() {
            self.flush_rollups().await?;
        }

        // Then apply the normal tiering policy
        self.apply_tiering_policy(now_ns).await
    }

    /// Delete old persisted rollups based on retention policy
    pub async fn delete_persisted_before(&self, now_ns: i64) -> Result<RollupDeleteResult> {
        let Some(ref persistence) = self.rollup_persistence else {
            return Ok(RollupDeleteResult::default());
        };

        let mut result = RollupDeleteResult::default();

        // Delete based on retention policy
        let minute_cutoff = now_ns - self.tiering_policy.minute_retention_ns;
        let five_minute_cutoff = now_ns - self.tiering_policy.five_minute_retention_ns;
        let hour_cutoff = now_ns - self.tiering_policy.hour_retention_ns;

        match persistence
            .delete_before(DownsampleResolution::Minute, minute_cutoff)
            .await
        {
            Ok(count) => result.minute_deleted = count,
            Err(e) => warn!("Failed to delete old minute rollups: {}", e),
        }

        match persistence
            .delete_before(DownsampleResolution::FiveMinute, five_minute_cutoff)
            .await
        {
            Ok(count) => result.five_minute_deleted = count,
            Err(e) => warn!("Failed to delete old five_minute rollups: {}", e),
        }

        match persistence
            .delete_before(DownsampleResolution::Hour, hour_cutoff)
            .await
        {
            Ok(count) => result.hour_deleted = count,
            Err(e) => warn!("Failed to delete old hour rollups: {}", e),
        }

        info!(
            "Deleted persisted rollups: 1m={}, 5m={}, 1h={}",
            result.minute_deleted, result.five_minute_deleted, result.hour_deleted
        );

        Ok(result)
    }
}

/// Result from flushing rollups to persistence
///
/// Contains statistics about a flush operation, including
/// the number of aggregates flushed at each resolution level.
#[derive(Debug, Default)]
pub struct RollupFlushResult {
    /// Number of minute aggregates flushed
    pub minute_flushed: usize,
    /// Number of 5-minute aggregates flushed
    pub five_minute_flushed: usize,
    /// Number of hour aggregates flushed
    pub hour_flushed: usize,
    /// Whether the operation was successful
    pub success: bool,
}

impl RollupFlushResult {
    /// Total points flushed across all resolutions
    #[must_use]
    pub fn total_flushed(&self) -> usize {
        self.minute_flushed + self.five_minute_flushed + self.hour_flushed
    }
}

/// Result from deleting old persisted rollups
///
/// Contains statistics about a deletion operation, including
/// the number of aggregates deleted at each resolution level.
#[derive(Debug, Default)]
pub struct RollupDeleteResult {
    /// Number of minute aggregates deleted
    pub minute_deleted: usize,
    /// Number of 5-minute aggregates deleted
    pub five_minute_deleted: usize,
    /// Number of hour aggregates deleted
    pub hour_deleted: usize,
}

impl RollupDeleteResult {
    /// Total points deleted across all resolutions
    #[must_use]
    pub fn total_deleted(&self) -> usize {
        self.minute_deleted + self.five_minute_deleted + self.hour_deleted
    }
}

/// Result from automatic resolution query
///
/// Contains the results of a query with automatic resolution selection.
/// May contain either raw samples or aggregated data depending on the
/// selected resolution.
#[derive(Debug)]
pub struct QueryAutoResult {
    /// Resolution that was used
    pub resolution: DownsampleResolution,
    /// Raw samples (if resolution was Raw)
    pub raw_samples: Option<Vec<MetricSample>>,
    /// Aggregated data (if resolution was not Raw)
    pub aggregated: Option<Vec<AggregatedMetric>>,
}

impl QueryAutoResult {
    /// Get the number of data points returned
    #[must_use]
    pub fn point_count(&self) -> usize {
        self.raw_samples.as_ref().map_or(0, |s| s.len())
            + self.aggregated.as_ref().map_or(0, |a| a.len())
    }

    /// Check if result is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.point_count() == 0
    }
}

/// Result from applying tiering policy
///
/// Contains statistics about a tiering policy application, including
/// the number of points removed at each resolution level.
#[derive(Debug, Default)]
pub struct TieringResult {
    /// Number of raw points removed
    pub raw_removed: usize,
    /// Number of minute aggregates removed
    pub minute_removed: usize,
    /// Number of 5-minute aggregates removed
    pub five_minute_removed: usize,
    /// Number of hour aggregates removed
    pub hour_removed: usize,
}

impl TieringResult {
    /// Total points removed across all tiers
    #[must_use]
    pub fn total_removed(&self) -> usize {
        self.raw_removed + self.minute_removed + self.five_minute_removed + self.hour_removed
    }
}

/// Statistics about tiering storage
///
/// Backwards-compat alias for [`ObservabilityTieringStats`].
pub type TieringStats = ObservabilityTieringStats;

/// Provides statistics about metric storage across all resolution tiers.
/// Used to monitor storage usage and compression ratios.
#[derive(Debug, Default)]
pub struct ObservabilityTieringStats {
    /// Number of series
    pub series_count: usize,
    /// Total raw data points
    pub raw_points: usize,
    /// Total 1-minute aggregate points
    pub minute_points: usize,
    /// Total 5-minute aggregate points
    pub five_minute_points: usize,
    /// Total 1-hour aggregate points
    pub hour_points: usize,
}

impl ObservabilityTieringStats {
    /// Total points across all tiers
    #[must_use]
    pub fn total_points(&self) -> usize {
        self.raw_points + self.minute_points + self.five_minute_points + self.hour_points
    }

    /// Compression ratio (raw points / aggregated points)
    #[must_use]
    pub fn compression_ratio(&self) -> f64 {
        let aggregated = self.minute_points + self.five_minute_points + self.hour_points;
        if aggregated == 0 {
            0.0
        } else {
            self.raw_points as f64 / aggregated as f64
        }
    }
}

/// Downsample resolution
///
/// Represents the time resolution of aggregated metric data.
/// Raw data has no aggregation, while other resolutions represent
/// time buckets of varying sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DownsampleResolution {
    /// Raw data points (no aggregation)
    Raw,
    /// 1-minute aggregates
    Minute,
    /// 5-minute aggregates
    FiveMinute,
    /// 1-hour aggregates
    Hour,
}

impl DownsampleResolution {
    /// Get the bucket size in nanoseconds
    #[must_use]
    pub fn bucket_size_ns(&self) -> i64 {
        match self {
            DownsampleResolution::Raw => 0, // No bucketing
            DownsampleResolution::Minute => NANOS_PER_MIN,
            DownsampleResolution::FiveMinute => 5 * NANOS_PER_MIN,
            DownsampleResolution::Hour => NANOS_PER_HOUR,
        }
    }

    /// Get display name
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            DownsampleResolution::Raw => "raw",
            DownsampleResolution::Minute => "1m",
            DownsampleResolution::FiveMinute => "5m",
            DownsampleResolution::Hour => "1h",
        }
    }
}

/// Aggregated metric result
///
/// Represents a single aggregated metric data point.
/// Contains min, max, avg, sum, and count for a time bucket.
#[derive(Debug, Clone)]
pub struct AggregatedMetric {
    /// Metric name
    pub name: String,
    /// Bucket timestamp
    pub timestamp_ns: i64,
    /// Minimum value
    pub min: f64,
    /// Maximum value
    pub max: f64,
    /// Average value
    pub avg: f64,
    /// Sum of values
    pub sum: f64,
    /// Count of values
    pub count: u64,
    /// Labels
    pub labels: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_observability::METRIC_LABEL;

    fn make_sample(name: &str, timestamp_ns: i64, value: f64) -> MetricSample {
        MetricSample {
            name: name.to_string(),
            timestamp_ns,
            value,
            labels: HashMap::new(),
        }
    }

    #[test]
    fn projection_record_uses_observability_canonical_shape() {
        let storage = MetricStorage::new_for_namespace("/tmp/test_metric_projection", "tenant-a")
            .expect("Failed to create MetricStorage");
        let mut sample = make_sample("cpu", 42, 0.5);
        sample
            .labels
            .insert("host".to_string(), "api-1".to_string());

        let record = storage.sample_projection_record(&sample);

        assert_eq!(record.oid, "obs://tenant-a/metric/cpu:42:host=api-1");
        assert_eq!(record.tenant_id, "tenant-a");
        assert!(record.labels.iter().any(|label| label == METRIC_LABEL));
        assert!(record.props.contains_key("labels"));
        assert_eq!(record.created_at_ns, 42);
        assert_eq!(record.updated_at_ns, 42);
    }

    #[tokio::test]
    async fn test_write_and_query() {
        let storage = MetricStorage::new("/tmp/test").expect("Failed to create MetricStorage");

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        storage
            .write(&make_sample("cpu", now, 0.5))
            .await
            .expect("Failed to write cpu metric sample 1");
        storage
            .write(&make_sample("cpu", now + 1000, 0.6))
            .await
            .expect("Failed to write cpu metric sample 2");
        storage
            .write(&make_sample("memory", now, 0.7))
            .await
            .expect("Failed to write memory metric sample");

        let results = storage
            .query("cpu", now - 1000, now + 2000)
            .await
            .expect("Failed to query cpu metrics");
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_series_count() {
        let storage = MetricStorage::new("/tmp/test").expect("Failed to create MetricStorage");

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        storage
            .write(&make_sample("cpu", now, 0.5))
            .await
            .expect("Failed to write cpu metric");
        storage
            .write(&make_sample("memory", now, 0.7))
            .await
            .expect("Failed to write memory metric");
        storage
            .write(&make_sample("disk", now, 0.3))
            .await
            .expect("Failed to write disk metric");

        assert_eq!(storage.series_count().await, 3);
    }

    #[test]
    fn test_aggregated_point() {
        let mut point = AggregatedPoint::new(5.0);
        assert_eq!(point.min, 5.0);
        assert_eq!(point.max, 5.0);
        assert_eq!(point.avg(), 5.0);

        point.update(3.0);
        point.update(7.0);
        assert_eq!(point.min, 3.0);
        assert_eq!(point.max, 7.0);
        assert_eq!(point.avg(), 5.0); // (5 + 3 + 7) / 3
    }

    #[test]
    fn test_tiering_policy_optimal_resolution() {
        let policy = MetricTieringPolicy::default();

        // Under 1 hour -> Raw
        assert_eq!(
            policy.optimal_resolution(30 * NANOS_PER_MIN),
            DownsampleResolution::Raw
        );

        // 1-6 hours -> Minute
        assert_eq!(
            policy.optimal_resolution(2 * NANOS_PER_HOUR),
            DownsampleResolution::Minute
        );

        // 6-24 hours -> FiveMinute
        assert_eq!(
            policy.optimal_resolution(12 * NANOS_PER_HOUR),
            DownsampleResolution::FiveMinute
        );

        // Over 24 hours -> Hour
        assert_eq!(
            policy.optimal_resolution(2 * NANOS_PER_DAY),
            DownsampleResolution::Hour
        );
    }

    #[test]
    fn test_tiering_policy_custom() {
        let policy = MetricTieringPolicy::new(
            4,   // raw: 4 hours
            14,  // minute: 14 days
            60,  // five_minute: 60 days
            730, // hour: 2 years
        );

        assert_eq!(policy.raw_retention_ns, 4 * NANOS_PER_HOUR);
        assert_eq!(policy.minute_retention_ns, 14 * NANOS_PER_DAY);
        assert_eq!(policy.five_minute_retention_ns, 60 * NANOS_PER_DAY);
        assert_eq!(policy.hour_retention_ns, 730 * NANOS_PER_DAY);
    }

    #[test]
    fn test_downsample_resolution_bucket_size() {
        assert_eq!(DownsampleResolution::Raw.bucket_size_ns(), 0);
        assert_eq!(DownsampleResolution::Minute.bucket_size_ns(), NANOS_PER_MIN);
        assert_eq!(
            DownsampleResolution::FiveMinute.bucket_size_ns(),
            5 * NANOS_PER_MIN
        );
        assert_eq!(DownsampleResolution::Hour.bucket_size_ns(), NANOS_PER_HOUR);
    }

    #[tokio::test]
    async fn test_query_auto_resolution_raw() {
        let storage = MetricStorage::new("/tmp/test").expect("Failed to create MetricStorage");
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        // Insert some data
        storage
            .write(&make_sample("cpu", now, 0.5))
            .await
            .expect("Failed to write cpu metric sample 1");
        storage
            .write(&make_sample("cpu", now + 1000, 0.6))
            .await
            .expect("Failed to write cpu metric sample 2");

        // Query for 30 minutes (should use raw)
        let result = storage
            .query_auto_resolution("cpu", now - 1000, now + 30 * NANOS_PER_MIN)
            .await
            .expect("Failed to query auto resolution");

        assert_eq!(result.resolution, DownsampleResolution::Raw);
        assert!(result.raw_samples.is_some());
        assert!(result.aggregated.is_none());
        assert_eq!(result.point_count(), 2);
    }

    #[tokio::test]
    async fn test_query_auto_resolution_minute() {
        let storage = MetricStorage::new("/tmp/test").expect("Failed to create MetricStorage");
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        // Insert some data
        storage
            .write(&make_sample("cpu", now, 0.5))
            .await
            .expect("Failed to write cpu metric sample 1");
        storage
            .write(&make_sample("cpu", now + NANOS_PER_MIN, 0.6))
            .await
            .expect("Failed to write cpu metric sample 2");

        // Query for 3 hours (should use minute aggregates)
        let result = storage
            .query_auto_resolution("cpu", now - 1000, now + 3 * NANOS_PER_HOUR)
            .await
            .expect("Failed to query auto resolution");

        assert_eq!(result.resolution, DownsampleResolution::Minute);
        assert!(result.raw_samples.is_none());
        assert!(result.aggregated.is_some());
    }

    #[tokio::test]
    async fn test_tiering_stats() {
        let storage = MetricStorage::new("/tmp/test").expect("Failed to create MetricStorage");
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        // Insert data for multiple metrics
        for i in 0..5 {
            storage
                .write(&make_sample("cpu", now + i * 1000, 0.5 + i as f64 * 0.1))
                .await
                .expect("Failed to write cpu metric sample");
        }

        let stats = storage.tiering_stats().await;
        assert_eq!(stats.series_count, 1);
        assert_eq!(stats.raw_points, 5);
        // Downsampled points should exist (all in same minute bucket)
        assert!(stats.minute_points >= 1);
    }

    #[tokio::test]
    async fn test_apply_tiering_policy() {
        // Create storage with very short retention for testing
        let policy = MetricTieringPolicy {
            raw_retention_ns: NANOS_PER_MIN,         // 1 minute
            minute_retention_ns: NANOS_PER_HOUR,     // 1 hour
            five_minute_retention_ns: NANOS_PER_DAY, // 1 day
            hour_retention_ns: 7 * NANOS_PER_DAY,    // 1 week
            ..Default::default()
        };
        let storage = MetricStorage::with_policy("/tmp/test", policy)
            .expect("Failed to create MetricStorage with policy");

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let old_time = now - 2 * NANOS_PER_MIN; // 2 minutes ago (older than retention)

        // Insert old data
        storage
            .write(&make_sample("cpu", old_time, 0.5))
            .await
            .expect("Failed to write old cpu metric");
        // Insert recent data
        storage
            .write(&make_sample("cpu", now, 0.6))
            .await
            .expect("Failed to write recent cpu metric");

        // Verify both points exist
        let before = storage.tiering_stats().await;
        assert_eq!(before.raw_points, 2);

        // Apply tiering policy
        let result = storage
            .apply_tiering_policy(now)
            .await
            .expect("Failed to apply tiering policy");
        assert_eq!(result.raw_removed, 1); // Old point removed

        // Verify only recent data remains
        let after = storage.tiering_stats().await;
        assert_eq!(after.raw_points, 1);
    }

    #[tokio::test]
    async fn test_query_result_helpers() {
        let storage = MetricStorage::new("/tmp/test").expect("Failed to create MetricStorage");
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        // Query empty storage
        let result = storage
            .query_auto_resolution("nonexistent", now - 1000, now + 1000)
            .await
            .expect("Failed to query auto resolution");

        assert!(result.is_empty());
        assert_eq!(result.point_count(), 0);
    }

    #[test]
    fn test_tiering_result_total() {
        let result = TieringResult {
            raw_removed: 10,
            minute_removed: 5,
            five_minute_removed: 2,
            hour_removed: 1,
        };
        assert_eq!(result.total_removed(), 18);
    }

    #[test]
    fn sample_projection_record_uses_observability_canonical_shape() {
        let storage = MetricStorage::new_for_namespace("/tmp/test_metric_projection", "tenant-a")
            .expect("Failed to create namespaced MetricStorage");
        let mut labels = HashMap::new();
        labels.insert("host".to_string(), "api-1".to_string());
        let sample = MetricSample {
            name: "cpu_usage".to_string(),
            timestamp_ns: 42,
            value: 0.75,
            labels,
        };

        let record = storage.sample_projection_record(&sample);

        assert_eq!(record.tenant_id, "tenant-a");
        assert!(record.oid.starts_with("obs://tenant-a/metric/cpu_usage:42"));
        assert!(
            record
                .labels
                .contains(proximadb_observability::METRIC_LABEL)
        );
        assert!(record.props.contains_key("labels"));
    }

    #[test]
    fn test_tiering_stats_compression_ratio() {
        let stats = ObservabilityTieringStats {
            series_count: 1,
            raw_points: 1000,
            minute_points: 100,
            five_minute_points: 20,
            hour_points: 5,
        };
        // 1000 / (100 + 20 + 5) = 8.0
        assert!((stats.compression_ratio() - 8.0).abs() < 0.01);
        assert_eq!(stats.total_points(), 1125);
    }
}
