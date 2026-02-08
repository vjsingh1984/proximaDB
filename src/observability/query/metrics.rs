// Metric query and aggregation engine
//
// Provides:
// - PromQL-like aggregation functions
// - Time bucketing
// - Rate calculations
// - Percentile computations

use std::collections::HashMap;

use crate::proto::proximadb_v1::MetricSample;

/// Metric query builder
///
/// Fluent builder for constructing metric queries with filters,
/// aggregations, and time bucketing.
pub struct MetricQueryBuilder {
    /// Metric name
    metric_name: Option<String>,
    /// Time range start
    start_time_ns: Option<i64>,
    /// Time range end
    end_time_ns: Option<i64>,
    /// Label filters
    labels: HashMap<String, String>,
    /// Aggregation function
    aggregation: Option<MetricAggregationFn>,
    /// Time bucket size
    bucket_size_ns: Option<i64>,
    /// Group by labels
    group_by: Vec<String>,
}

impl MetricQueryBuilder {
    /// Create a new query builder
    #[must_use]
    pub fn new() -> Self {
        Self {
            metric_name: None,
            start_time_ns: None,
            end_time_ns: None,
            labels: HashMap::new(),
            aggregation: None,
            bucket_size_ns: None,
            group_by: Vec::new(),
        }
    }

    /// Set metric name
    #[must_use]
    pub fn metric(mut self, name: &str) -> Self {
        self.metric_name = Some(name.to_string());
        self
    }

    /// Set time range
    #[must_use]
    pub fn time_range(mut self, start_ns: i64, end_ns: i64) -> Self {
        self.start_time_ns = Some(start_ns);
        self.end_time_ns = Some(end_ns);
        self
    }

    /// Add label filter
    #[must_use]
    pub fn label(mut self, key: &str, value: &str) -> Self {
        self.labels.insert(key.to_string(), value.to_string());
        self
    }

    /// Set aggregation function
    #[must_use]
    pub fn aggregate(mut self, agg: MetricAggregationFn) -> Self {
        self.aggregation = Some(agg);
        self
    }

    /// Set time bucket size
    #[must_use]
    pub fn bucket(mut self, size_ns: i64) -> Self {
        self.bucket_size_ns = Some(size_ns);
        self
    }

    /// Add group by label
    #[must_use]
    pub fn group_by(mut self, label: &str) -> Self {
        self.group_by.push(label.to_string());
        self
    }

    /// Build the query
    #[must_use]
    pub fn build(self) -> MetricQuery {
        MetricQuery {
            metric_name: self.metric_name.unwrap_or_default(),
            start_time_ns: self.start_time_ns.unwrap_or(0),
            end_time_ns: self.end_time_ns.unwrap_or(i64::MAX),
            labels: self.labels,
            aggregation: self.aggregation.unwrap_or(MetricAggregationFn::Avg),
            bucket_size_ns: self.bucket_size_ns.unwrap_or(60_000_000_000), // 1 minute default
            group_by: self.group_by,
        }
    }
}

impl Default for MetricQueryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Compiled metric query
///
/// A fully-specified metric query ready for execution.
/// Contains metric name, time range, filters, and aggregation settings.
#[derive(Debug, Clone)]
pub struct MetricQuery {
    /// Metric name
    pub metric_name: String,
    /// Time range start
    pub start_time_ns: i64,
    /// Time range end
    pub end_time_ns: i64,
    /// Label filters
    pub labels: HashMap<String, String>,
    /// Aggregation function
    pub aggregation: MetricAggregationFn,
    /// Time bucket size
    pub bucket_size_ns: i64,
    /// Group by labels
    pub group_by: Vec<String>,
}

/// Metric aggregation functions
///
/// Supported aggregation functions for metric queries.
/// Includes basic stats (avg, sum, min, max), rate calculations,
/// percentiles, and standard deviation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetricAggregationFn {
    /// Average value
    Avg,
    /// Sum of values
    Sum,
    /// Minimum value
    Min,
    /// Maximum value
    Max,
    /// Count of samples
    Count,
    /// Rate of change per second
    Rate,
    /// 50th percentile
    P50,
    /// 90th percentile
    P90,
    /// 95th percentile
    P95,
    /// 99th percentile
    P99,
    /// Standard deviation
    StdDev,
}

impl MetricQuery {
    /// Execute the query on samples
    pub fn execute(&self, samples: Vec<MetricSample>) -> Vec<MetricResult> {
        // Filter samples
        let filtered: Vec<_> = samples
            .into_iter()
            .filter(|s| self.matches_sample(s))
            .collect();

        // Group by time buckets and labels
        let groups = self.group_samples(filtered);

        // Apply aggregation to each group
        groups
            .into_iter()
            .map(|(key, samples)| {
                let value = self.aggregate_samples(&samples);
                MetricResult {
                    timestamp_ns: key.timestamp_ns,
                    value,
                    labels: key.labels_as_map(),
                }
            })
            .collect()
    }

    /// Check if a sample matches the query filters
    fn matches_sample(&self, sample: &MetricSample) -> bool {
        // Name filter
        if sample.name != self.metric_name {
            return false;
        }

        // Time filter
        if sample.timestamp_ns < self.start_time_ns || sample.timestamp_ns > self.end_time_ns {
            return false;
        }

        // Label filters
        for (key, value) in &self.labels {
            match sample.labels.get(key) {
                Some(v) if v == value => {}
                _ => return false,
            }
        }

        true
    }

    /// Group samples by time bucket and labels
    fn group_samples(&self, samples: Vec<MetricSample>) -> HashMap<GroupKey, Vec<MetricSample>> {
        let mut groups: HashMap<GroupKey, Vec<MetricSample>> = HashMap::new();

        for sample in samples {
            let bucket_ts = (sample.timestamp_ns / self.bucket_size_ns) * self.bucket_size_ns;

            let group_labels: HashMap<String, String> = self
                .group_by
                .iter()
                .filter_map(|k| sample.labels.get(k).map(|v| (k.clone(), v.clone())))
                .collect();

            let key = GroupKey::new(bucket_ts, group_labels);

            groups.entry(key).or_insert_with(Vec::new).push(sample);
        }

        groups
    }

    /// Apply aggregation function to samples
    fn aggregate_samples(&self, samples: &[MetricSample]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }

        let values: Vec<f64> = samples.iter().map(|s| s.value).collect();

        match self.aggregation {
            MetricAggregationFn::Avg => values.iter().sum::<f64>() / values.len() as f64,
            MetricAggregationFn::Sum => values.iter().sum(),
            MetricAggregationFn::Min => values.iter().cloned().fold(f64::INFINITY, f64::min),
            MetricAggregationFn::Max => values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            MetricAggregationFn::Count => values.len() as f64,
            MetricAggregationFn::Rate => self.calculate_rate(samples),
            MetricAggregationFn::P50 => self.calculate_percentile(&values, 0.50),
            MetricAggregationFn::P90 => self.calculate_percentile(&values, 0.90),
            MetricAggregationFn::P95 => self.calculate_percentile(&values, 0.95),
            MetricAggregationFn::P99 => self.calculate_percentile(&values, 0.99),
            MetricAggregationFn::StdDev => self.calculate_stddev(&values),
        }
    }

    /// Calculate rate of change
    fn calculate_rate(&self, samples: &[MetricSample]) -> f64 {
        if samples.len() < 2 {
            return 0.0;
        }

        let mut sorted: Vec<_> = samples.iter().collect();
        sorted.sort_by_key(|s| s.timestamp_ns);

        let first = sorted.first().unwrap();
        let last = sorted.last().unwrap();

        let time_diff_s = (last.timestamp_ns - first.timestamp_ns) as f64 / 1_000_000_000.0;
        if time_diff_s <= 0.0 {
            return 0.0;
        }

        (last.value - first.value) / time_diff_s
    }

    /// Calculate percentile
    fn calculate_percentile(&self, values: &[f64], percentile: f64) -> f64 {
        if values.is_empty() {
            return 0.0;
        }

        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let index = (percentile * (sorted.len() - 1) as f64) as usize;
        sorted[index]
    }

    /// Calculate standard deviation
    fn calculate_stddev(&self, values: &[f64]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }

        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
        variance.sqrt()
    }
}

/// Group key for aggregation
///
/// Internal key used for grouping metric data points
/// by time bucket and label set.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupKey {
    /// Bucket timestamp
    timestamp_ns: i64,
    /// Group labels (sorted keys for consistent hashing)
    labels: Vec<(String, String)>,
}

impl std::hash::Hash for GroupKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.timestamp_ns.hash(state);
        // Labels are stored as sorted Vec for consistent hashing
        for (k, v) in &self.labels {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl GroupKey {
    /// Create a new GroupKey from timestamp and labels
    fn new(timestamp_ns: i64, labels: HashMap<String, String>) -> Self {
        // Convert to sorted Vec for consistent hashing
        let mut sorted_labels: Vec<(String, String)> = labels.into_iter().collect();
        sorted_labels.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            timestamp_ns,
            labels: sorted_labels,
        }
    }

    /// Convert labels back to HashMap
    fn labels_as_map(&self) -> HashMap<String, String> {
        self.labels.iter().cloned().collect()
    }
}

/// Metric query result
///
/// A single aggregated metric data point resulting from a query.
/// Contains timestamp, value, and labels.
#[derive(Debug, Clone)]
pub struct MetricResult {
    /// Timestamp (bucket start)
    pub timestamp_ns: i64,
    /// Aggregated value
    pub value: f64,
    /// Group labels
    pub labels: HashMap<String, String>,
}

/// Time series result
///
/// A time series consisting of multiple metric data points
/// with the same label set.
#[derive(Debug, Clone)]
pub struct TimeSeries {
    /// Series labels
    pub labels: HashMap<String, String>,
    /// Data points
    pub points: Vec<MetricResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample(name: &str, timestamp_ns: i64, value: f64) -> MetricSample {
        MetricSample {
            name: name.to_string(),
            timestamp_ns,
            value,
            labels: HashMap::new(),
        }
    }

    #[test]
    fn test_query_builder() {
        let query = MetricQueryBuilder::new()
            .metric("cpu_usage")
            .aggregate(MetricAggregationFn::Avg)
            .bucket(60_000_000_000)
            .build();

        assert_eq!(query.metric_name, "cpu_usage");
        assert_eq!(query.aggregation, MetricAggregationFn::Avg);
    }

    #[test]
    fn test_aggregation_avg() {
        let query = MetricQueryBuilder::new()
            .metric("test")
            .aggregate(MetricAggregationFn::Avg)
            .bucket(100_000_000_000)
            .build();

        let samples = vec![
            make_sample("test", 1, 10.0),
            make_sample("test", 2, 20.0),
            make_sample("test", 3, 30.0),
        ];

        let results = query.execute(samples);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 20.0);
    }

    #[test]
    fn test_aggregation_sum() {
        let query = MetricQueryBuilder::new()
            .metric("test")
            .aggregate(MetricAggregationFn::Sum)
            .bucket(100_000_000_000)
            .build();

        let samples = vec![
            make_sample("test", 1, 10.0),
            make_sample("test", 2, 20.0),
            make_sample("test", 3, 30.0),
        ];

        let results = query.execute(samples);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 60.0);
    }

    #[test]
    fn test_percentile() {
        let query = MetricQueryBuilder::new().metric("test").build();

        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let p50 = query.calculate_percentile(&values, 0.50);
        assert!(p50 >= 5.0 && p50 <= 6.0);
    }
}
