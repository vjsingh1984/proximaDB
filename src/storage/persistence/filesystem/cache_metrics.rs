//! Cache Metrics for Unified Filesystem
//!
//! Comprehensive metrics tracking for all cache layers to monitor
//! performance, identify bottlenecks, and optimize cache behavior.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::RwLock;
use tracing::trace;

use crate::storage::persistence::filesystem::unified::CacheType;

/// Comprehensive cache metrics collector
pub struct CacheMetrics {
    /// Per-cache-type metrics
    cache_metrics: Arc<DashMap<CacheType, CacheTypeMetrics>>,

    /// Access patterns
    access_patterns: Arc<RwLock<AccessMetrics>>,

    /// Performance metrics
    performance: Arc<RwLock<PerformanceMetrics>>,

    /// Start time for uptime calculation
    start_time: Instant,
}

/// Metrics for a specific cache type
#[derive(Debug, Default, Clone)]
struct CacheTypeMetrics {
    hits: u64,
    misses: u64,
    evictions: u64,
    insertions: u64,
    bytes_served: u64,
    bytes_stored: u64,
}

/// Access pattern metrics
#[derive(Debug, Default)]
struct AccessMetrics {
    total_accesses: u64,
    unique_files_accessed: HashSet<String>,
    access_distribution: Vec<(String, u64)>, // Top accessed files
}

use std::collections::HashSet;

/// Performance metrics
#[derive(Debug, Default)]
struct PerformanceMetrics {
    avg_hit_latency_us: u64,
    avg_miss_latency_us: u64,
    p99_latency_us: u64,
    p95_latency_us: u64,
    latency_samples: Vec<u64>,
}

impl CacheMetrics {
    /// Create new metrics collector
    pub fn new() -> Self {
        Self {
            cache_metrics: Arc::new(DashMap::new()),
            access_patterns: Arc::new(RwLock::new(AccessMetrics::default())),
            performance: Arc::new(RwLock::new(PerformanceMetrics::default())),
            start_time: Instant::now(),
        }
    }

    /// Record a cache access
    pub fn record_access(&self) {
        // Use try_write to avoid blocking in async context
        if let Ok(mut patterns) = self.access_patterns.try_write() {
            patterns.total_accesses += 1;
        }
        // If we can't get the lock, skip recording this access
        // This is better than panicking in async context
    }

    /// Record a cache hit
    pub fn record_cache_hit(&self, cache_type: CacheType) {
        self.record_cache_hit_with_size(cache_type, 0);
    }

    /// Record a cache hit with size
    pub fn record_cache_hit_with_size(&self, cache_type: CacheType, bytes: usize) {
        let mut entry = self.cache_metrics.entry(cache_type.clone()).or_default();
        entry.hits += 1;
        entry.bytes_served += bytes as u64;

        trace!("Cache hit for {:?}, total hits: {}", cache_type, entry.hits);
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self, cache_type: CacheType) {
        let mut entry = self.cache_metrics.entry(cache_type.clone()).or_default();
        entry.misses += 1;

        trace!(
            "Cache miss for {:?}, total misses: {}",
            cache_type, entry.misses
        );
    }

    /// Record a cache eviction
    pub fn record_eviction(&self, cache_type: CacheType, count: u64) {
        let mut entry = self.cache_metrics.entry(cache_type).or_default();
        entry.evictions += count;
    }

    /// Record a cache insertion
    pub fn record_insertion(&self, cache_type: CacheType, bytes: usize) {
        let mut entry = self.cache_metrics.entry(cache_type).or_default();
        entry.insertions += 1;
        entry.bytes_stored += bytes as u64;
    }

    /// Record access latency
    pub async fn record_latency(&self, latency_us: u64, is_hit: bool) {
        let mut perf = self.performance.write().await;

        perf.latency_samples.push(latency_us);

        // Keep only last 1000 samples for percentile calculation
        if perf.latency_samples.len() > 1000 {
            perf.latency_samples.drain(0..500);
        }

        // Update averages
        if is_hit {
            let current_avg = perf.avg_hit_latency_us;
            perf.avg_hit_latency_us = (current_avg * 9 + latency_us) / 10; // Exponential moving average
        } else {
            let current_avg = perf.avg_miss_latency_us;
            perf.avg_miss_latency_us = (current_avg * 9 + latency_us) / 10;
        }

        // Update percentiles
        if perf.latency_samples.len() >= 100 {
            let mut sorted = perf.latency_samples.clone();
            sorted.sort_unstable();

            perf.p95_latency_us = sorted[sorted.len() * 95 / 100];
            perf.p99_latency_us = sorted[sorted.len() * 99 / 100];
        }
    }

    /// Record file access for pattern tracking
    pub fn record_file_access(&self, path: &str) {
        // Use try_write to avoid blocking in async context
        if let Ok(mut patterns) = self.access_patterns.try_write() {
            patterns.unique_files_accessed.insert(path.to_string());

            // Update access distribution
            if let Some(entry) = patterns
                .access_distribution
                .iter_mut()
                .find(|(p, _)| p == path)
            {
                entry.1 += 1;
            } else {
                patterns.access_distribution.push((path.to_string(), 1));
            }

            // Keep only top 100 accessed files
            if patterns.access_distribution.len() > 100 {
                patterns
                    .access_distribution
                    .sort_by_key(|(_, count)| std::cmp::Reverse(*count));
                patterns.access_distribution.truncate(100);
            }
        }
    }

    /// Get comprehensive metrics report
    pub async fn get_report(&self) -> MetricsReport {
        let mut report = MetricsReport {
            uptime: self.start_time.elapsed(),
            cache_metrics: Vec::new(),
            total_hits: 0,
            total_misses: 0,
            overall_hit_rate: 0.0,
            total_bytes_served: 0,
            total_bytes_stored: 0,
            unique_files: 0,
            total_accesses: 0,
            top_accessed_files: Vec::new(),
            avg_hit_latency_us: 0,
            avg_miss_latency_us: 0,
            p95_latency_us: 0,
            p99_latency_us: 0,
        };

        // Collect cache metrics
        for entry in self.cache_metrics.iter() {
            let cache_type = entry.key().clone();
            let metrics = entry.value().clone();

            report.total_hits += metrics.hits;
            report.total_misses += metrics.misses;
            report.total_bytes_served += metrics.bytes_served;
            report.total_bytes_stored += metrics.bytes_stored;

            report.cache_metrics.push(CacheMetricDetail {
                cache_type: format!("{:?}", cache_type),
                hits: metrics.hits,
                misses: metrics.misses,
                hit_rate: Self::calculate_hit_rate(metrics.hits, metrics.misses),
                evictions: metrics.evictions,
                insertions: metrics.insertions,
                bytes_served: metrics.bytes_served,
                bytes_stored: metrics.bytes_stored,
            });
        }

        // Calculate overall hit rate
        report.overall_hit_rate = Self::calculate_hit_rate(report.total_hits, report.total_misses);

        // Add access patterns
        let patterns = self.access_patterns.read().await;
        report.unique_files = patterns.unique_files_accessed.len();
        report.total_accesses = patterns.total_accesses;
        report.top_accessed_files = patterns
            .access_distribution
            .iter()
            .take(10)
            .map(|(path, count)| (path.clone(), *count))
            .collect();

        // Add performance metrics
        let perf = self.performance.read().await;
        report.avg_hit_latency_us = perf.avg_hit_latency_us;
        report.avg_miss_latency_us = perf.avg_miss_latency_us;
        report.p95_latency_us = perf.p95_latency_us;
        report.p99_latency_us = perf.p99_latency_us;

        report
    }

    /// Calculate hit rate
    fn calculate_hit_rate(hits: u64, misses: u64) -> f64 {
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Reset all metrics
    pub async fn reset(&self) {
        self.cache_metrics.clear();
        *self.access_patterns.write().await = AccessMetrics::default();
        *self.performance.write().await = PerformanceMetrics::default();
    }
}

impl Default for CacheMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Comprehensive metrics report
#[derive(Debug, Clone)]
pub struct MetricsReport {
    pub uptime: Duration,
    pub cache_metrics: Vec<CacheMetricDetail>,
    pub total_hits: u64,
    pub total_misses: u64,
    pub overall_hit_rate: f64,
    pub total_bytes_served: u64,
    pub total_bytes_stored: u64,
    pub unique_files: usize,
    pub total_accesses: u64,
    pub top_accessed_files: Vec<(String, u64)>,
    pub avg_hit_latency_us: u64,
    pub avg_miss_latency_us: u64,
    pub p95_latency_us: u64,
    pub p99_latency_us: u64,
}

/// Cache-specific metric details
#[derive(Debug, Clone)]
pub struct CacheMetricDetail {
    pub cache_type: String,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub evictions: u64,
    pub insertions: u64,
    pub bytes_served: u64,
    pub bytes_stored: u64,
}

impl MetricsReport {
    /// Format report as string
    pub fn format(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!("=== Cache Metrics Report ===\n"));
        output.push_str(&format!("Uptime: {:?}\n", self.uptime));
        output.push_str(&format!(
            "Overall Hit Rate: {:.2}%\n",
            self.overall_hit_rate * 100.0
        ));
        output.push_str(&format!(
            "Total Hits: {} | Misses: {}\n",
            self.total_hits, self.total_misses
        ));
        output.push_str(&format!(
            "Bytes Served: {} | Stored: {}\n",
            format_bytes(self.total_bytes_served),
            format_bytes(self.total_bytes_stored)
        ));
        output.push_str(&format!(
            "Unique Files: {} | Total Accesses: {}\n",
            self.unique_files, self.total_accesses
        ));
        output.push_str(&format!(
            "\nLatency (μs): Hit={} Miss={} P95={} P99={}\n",
            self.avg_hit_latency_us,
            self.avg_miss_latency_us,
            self.p95_latency_us,
            self.p99_latency_us
        ));

        output.push_str("\nPer-Cache Metrics:\n");
        for metric in &self.cache_metrics {
            output.push_str(&format!(
                "  {}: Hit Rate={:.2}% Hits={} Misses={} Evictions={}\n",
                metric.cache_type,
                metric.hit_rate * 100.0,
                metric.hits,
                metric.misses,
                metric.evictions
            ));
        }

        if !self.top_accessed_files.is_empty() {
            output.push_str("\nTop Accessed Files:\n");
            for (path, count) in self.top_accessed_files.iter().take(5) {
                output.push_str(&format!("  {} - {} accesses\n", path, count));
            }
        }

        output
    }
}

/// Format bytes to human-readable string
fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_recording() {
        let metrics = CacheMetrics::new();

        metrics.record_cache_hit(CacheType::Metadata);
        metrics.record_cache_hit(CacheType::Disk);
        metrics.record_cache_miss(CacheType::Metadata);

        let report = metrics.get_report().await;

        assert_eq!(report.total_hits, 2);
        assert_eq!(report.total_misses, 1);
        assert!(report.overall_hit_rate > 0.6);
    }

    #[tokio::test]
    async fn test_latency_tracking() {
        let metrics = CacheMetrics::new();

        metrics.record_latency(100, true).await;
        metrics.record_latency(200, true).await;
        metrics.record_latency(1000, false).await;

        let report = metrics.get_report().await;

        assert!(report.avg_hit_latency_us > 0);
        assert!(report.avg_miss_latency_us > 0);
    }
}
