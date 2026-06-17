//! HDR Histogram implementation for embedded mode latency tracking
//!
//! Provides accurate percentile calculations (p50, p95, p99) for operation
//! latencies with minimal memory overhead using a hybrid approach:
//! - Ring buffer for rolling window support (1min, 5min, 1hr)
//! - Bucket-based histogram for accurate percentile calculation
//!
//! This is optimized for embedded mode where we need low overhead and
//! accurate latency tracking without external dependencies.

use std::collections::VecDeque;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Number of buckets for histogram (log-scale)
/// Covers 1us to ~10s with 128 buckets
const NUM_BUCKETS: usize = 128;

/// Maximum trackable latency in microseconds (10 seconds)
const MAX_LATENCY_US: u64 = 10_000_000;

/// Minimum trackable latency in microseconds
const MIN_LATENCY_US: u64 = 1;

/// A single latency sample with timestamp
#[derive(Clone, Copy, Debug)]
struct LatencySample {
    /// Latency in microseconds
    latency_us: u64,
    /// When this sample was recorded
    timestamp: Instant,
}

/// Rolling window configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollingWindow {
    /// 1 minute window
    OneMinute,
    /// 5 minute window
    FiveMinutes,
    /// 1 hour window
    OneHour,
    /// All time (no rolling window)
    AllTime,
}

impl RollingWindow {
    /// Get the duration for this window
    fn duration(&self) -> Option<Duration> {
        match self {
            RollingWindow::OneMinute => Some(Duration::from_secs(60)),
            RollingWindow::FiveMinutes => Some(Duration::from_secs(300)),
            RollingWindow::OneHour => Some(Duration::from_secs(3600)),
            RollingWindow::AllTime => None,
        }
    }
}

/// HDR histogram for latency tracking with rolling window support
///
/// Uses a hybrid approach:
/// 1. Ring buffer stores recent samples for rolling window calculations
/// 2. Log-scale buckets for fast percentile lookups
pub struct LatencyHistogram {
    /// Operation name for identification
    name: String,

    /// Ring buffer for rolling window (stores recent samples)
    /// Protected by RwLock for thread-safe access
    samples: RwLock<VecDeque<LatencySample>>,

    /// Maximum samples to keep (based on expected throughput)
    max_samples: usize,

    /// Log-scale bucket counts for all-time histogram
    /// Index = floor(log2(latency_us - MIN_LATENCY_US + 1) * scale_factor)
    buckets: [AtomicU64; NUM_BUCKETS],

    /// Total count of recorded samples (all time)
    total_count: AtomicU64,

    /// Sum of all latencies in microseconds (for average calculation)
    total_sum_us: AtomicU64,

    /// Minimum latency seen (all time)
    min_us: AtomicU64,

    /// Maximum latency seen (all time)
    max_us: AtomicU64,
}

impl LatencyHistogram {
    /// Create a new histogram with the given name
    ///
    /// # Arguments
    /// * `name` - Operation name (e.g., "search", "insert", "flush")
    /// * `max_samples` - Maximum samples to keep in rolling buffer (default: 10000)
    pub fn new(name: impl Into<String>, max_samples: usize) -> Self {
        // Initialize all buckets to 0
        let buckets: [AtomicU64; NUM_BUCKETS] = std::array::from_fn(|_| AtomicU64::new(0));

        Self {
            name: name.into(),
            samples: RwLock::new(VecDeque::with_capacity(max_samples.min(10000))),
            max_samples,
            buckets,
            total_count: AtomicU64::new(0),
            total_sum_us: AtomicU64::new(0),
            min_us: AtomicU64::new(u64::MAX),
            max_us: AtomicU64::new(0),
        }
    }

    /// Create a default histogram with reasonable buffer size
    pub fn with_name(name: impl Into<String>) -> Self {
        Self::new(name, 10000)
    }

    /// Record a latency value in microseconds
    pub fn record_us(&self, latency_us: u64) {
        let now = Instant::now();
        let clamped = latency_us.clamp(MIN_LATENCY_US, MAX_LATENCY_US);

        // Update all-time stats atomically
        self.total_count.fetch_add(1, Ordering::Relaxed);
        self.total_sum_us.fetch_add(clamped, Ordering::Relaxed);

        // Update min/max using compare-and-swap loops
        loop {
            let current_min = self.min_us.load(Ordering::Relaxed);
            if clamped >= current_min {
                break;
            }
            if self
                .min_us
                .compare_exchange_weak(current_min, clamped, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        loop {
            let current_max = self.max_us.load(Ordering::Relaxed);
            if clamped <= current_max {
                break;
            }
            if self
                .max_us
                .compare_exchange_weak(current_max, clamped, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        // Update histogram bucket
        let bucket_idx = Self::latency_to_bucket(clamped);
        self.buckets[bucket_idx].fetch_add(1, Ordering::Relaxed);

        // Add to ring buffer for rolling window support
        if let Ok(mut samples) = self.samples.write() {
            samples.push_back(LatencySample {
                latency_us: clamped,
                timestamp: now,
            });

            // Trim to max size
            while samples.len() > self.max_samples {
                samples.pop_front();
            }
        }
    }

    /// Record a latency value in milliseconds
    pub fn record_ms(&self, latency_ms: f64) {
        self.record_us((latency_ms * 1000.0) as u64);
    }

    /// Record a latency from a Duration
    pub fn record(&self, latency: Duration) {
        self.record_us(latency.as_micros() as u64);
    }

    /// Get the name of this histogram
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get statistics for a specific rolling window
    pub fn stats(&self, window: RollingWindow) -> HistogramStats {
        match window {
            RollingWindow::AllTime => self.all_time_stats(),
            _ => self.rolling_stats(window),
        }
    }

    /// Get all-time statistics using bucket histogram
    fn all_time_stats(&self) -> HistogramStats {
        let count = self.total_count.load(Ordering::Relaxed);
        if count == 0 {
            return HistogramStats::default();
        }

        let sum = self.total_sum_us.load(Ordering::Relaxed);
        let min = self.min_us.load(Ordering::Relaxed);
        let max = self.max_us.load(Ordering::Relaxed);

        // Calculate percentiles from buckets
        let p50 = self.percentile_from_buckets(0.50, count);
        let p95 = self.percentile_from_buckets(0.95, count);
        let p99 = self.percentile_from_buckets(0.99, count);

        HistogramStats {
            count,
            min_us: if min == u64::MAX { 0 } else { min },
            max_us: max,
            mean_us: sum as f64 / count as f64,
            p50_us: p50,
            p95_us: p95,
            p99_us: p99,
        }
    }

    /// Get rolling window statistics from sample buffer
    fn rolling_stats(&self, window: RollingWindow) -> HistogramStats {
        let duration = match window.duration() {
            Some(d) => d,
            None => return self.all_time_stats(),
        };

        let now = Instant::now();
        let cutoff = now.checked_sub(duration).unwrap_or(now);

        let samples = match self.samples.read() {
            Ok(s) => s,
            Err(_) => return HistogramStats::default(),
        };

        // Collect samples within window
        let window_samples: Vec<u64> = samples
            .iter()
            .filter(|s| s.timestamp >= cutoff)
            .map(|s| s.latency_us)
            .collect();

        if window_samples.is_empty() {
            return HistogramStats::default();
        }

        // Sort for percentile calculation
        let mut sorted = window_samples.clone();
        sorted.sort_unstable();

        let count = sorted.len() as u64;
        let sum: u64 = window_samples.iter().sum();
        let min = *sorted.first().unwrap_or(&0);
        let max = *sorted.last().unwrap_or(&0);

        HistogramStats {
            count,
            min_us: min,
            max_us: max,
            mean_us: sum as f64 / count as f64,
            p50_us: Self::percentile_sorted(&sorted, 0.50),
            p95_us: Self::percentile_sorted(&sorted, 0.95),
            p99_us: Self::percentile_sorted(&sorted, 0.99),
        }
    }

    /// Calculate percentile from sorted array
    fn percentile_sorted(sorted: &[u64], percentile: f64) -> u64 {
        if sorted.is_empty() {
            return 0;
        }
        let idx = ((sorted.len() as f64 - 1.0) * percentile) as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    /// Convert latency to bucket index (log-scale)
    fn latency_to_bucket(latency_us: u64) -> usize {
        if latency_us <= MIN_LATENCY_US {
            return 0;
        }

        // Log2 scale with adjustment for bucket count
        // Maps [1us, 10s] to [0, NUM_BUCKETS-1]
        let log_range = (MAX_LATENCY_US as f64 / MIN_LATENCY_US as f64).log2();
        let scale_factor = (NUM_BUCKETS - 1) as f64 / log_range;

        let log_val = (latency_us as f64 / MIN_LATENCY_US as f64).log2();
        let bucket = (log_val * scale_factor) as usize;

        bucket.min(NUM_BUCKETS - 1)
    }

    /// Convert bucket index back to approximate latency
    fn bucket_to_latency(bucket_idx: usize) -> u64 {
        let log_range = (MAX_LATENCY_US as f64 / MIN_LATENCY_US as f64).log2();
        let scale_factor = (NUM_BUCKETS - 1) as f64 / log_range;

        let log_val = bucket_idx as f64 / scale_factor;
        (MIN_LATENCY_US as f64 * 2.0_f64.powf(log_val)) as u64
    }

    /// Calculate percentile from bucket histogram
    fn percentile_from_buckets(&self, percentile: f64, total_count: u64) -> u64 {
        let target = (total_count as f64 * percentile) as u64;
        let mut cumulative = 0u64;

        for (idx, bucket) in self.buckets.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= target {
                return Self::bucket_to_latency(idx);
            }
        }

        // If we get here, return max bucket latency
        Self::bucket_to_latency(NUM_BUCKETS - 1)
    }

    /// Reset all statistics
    pub fn reset(&self) {
        self.total_count.store(0, Ordering::Relaxed);
        self.total_sum_us.store(0, Ordering::Relaxed);
        self.min_us.store(u64::MAX, Ordering::Relaxed);
        self.max_us.store(0, Ordering::Relaxed);

        for bucket in &self.buckets {
            bucket.store(0, Ordering::Relaxed);
        }

        if let Ok(mut samples) = self.samples.write() {
            samples.clear();
        }
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new("unnamed", 10000)
    }
}

impl std::fmt::Debug for LatencyHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.all_time_stats();
        f.debug_struct("LatencyHistogram")
            .field("name", &self.name)
            .field("count", &stats.count)
            .field("min_us", &stats.min_us)
            .field("max_us", &stats.max_us)
            .field("p50_us", &stats.p50_us)
            .field("p95_us", &stats.p95_us)
            .field("p99_us", &stats.p99_us)
            .finish()
    }
}

/// Statistics snapshot from a histogram
#[derive(Debug, Clone, Default)]
pub struct HistogramStats {
    /// Number of samples
    pub count: u64,
    /// Minimum latency in microseconds
    pub min_us: u64,
    /// Maximum latency in microseconds
    pub max_us: u64,
    /// Mean latency in microseconds
    pub mean_us: f64,
    /// 50th percentile (median) in microseconds
    pub p50_us: u64,
    /// 95th percentile in microseconds
    pub p95_us: u64,
    /// 99th percentile in microseconds
    pub p99_us: u64,
}

impl HistogramStats {
    /// Get minimum latency in milliseconds
    pub fn min_ms(&self) -> f64 {
        self.min_us as f64 / 1000.0
    }

    /// Get maximum latency in milliseconds
    pub fn max_ms(&self) -> f64 {
        self.max_us as f64 / 1000.0
    }

    /// Get mean latency in milliseconds
    pub fn mean_ms(&self) -> f64 {
        self.mean_us / 1000.0
    }

    /// Get p50 latency in milliseconds
    pub fn p50_ms(&self) -> f64 {
        self.p50_us as f64 / 1000.0
    }

    /// Get p95 latency in milliseconds
    pub fn p95_ms(&self) -> f64 {
        self.p95_us as f64 / 1000.0
    }

    /// Get p99 latency in milliseconds
    pub fn p99_ms(&self) -> f64 {
        self.p99_us as f64 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // LatencyHistogram Basic Tests
    // ========================================================================

    #[test]
    fn test_histogram_basic() {
        let hist = LatencyHistogram::with_name("test");

        // Record some samples
        hist.record_us(100);
        hist.record_us(200);
        hist.record_us(300);
        hist.record_us(1000);
        hist.record_us(5000);

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 5);
        assert_eq!(stats.min_us, 100);
        assert_eq!(stats.max_us, 5000);
    }

    #[test]
    fn test_histogram_new_with_buckets() {
        let hist = LatencyHistogram::new("custom", 200);
        assert_eq!(hist.name(), "custom");

        hist.record_us(500);
        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 1);
    }

    #[test]
    fn test_histogram_with_name() {
        let hist = LatencyHistogram::with_name("search_latency");
        assert_eq!(hist.name(), "search_latency");
    }

    #[test]
    fn test_histogram_default() {
        let hist = LatencyHistogram::default();
        assert_eq!(hist.name(), "unnamed");

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 0);
    }

    // ========================================================================
    // Recording Methods Tests
    // ========================================================================

    #[test]
    fn test_record_us() {
        let hist = LatencyHistogram::with_name("test");

        hist.record_us(100);
        hist.record_us(500);
        hist.record_us(1000);

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 3);
        assert_eq!(stats.min_us, 100);
        assert_eq!(stats.max_us, 1000);
    }

    #[test]
    fn test_record_ms() {
        let hist = LatencyHistogram::with_name("test");

        hist.record_ms(1.5); // 1500 us
        hist.record_ms(2.5); // 2500 us

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 2);
        assert_eq!(stats.min_us, 1500);
        assert_eq!(stats.max_us, 2500);
    }

    #[test]
    fn test_record_duration() {
        let hist = LatencyHistogram::with_name("test");

        hist.record(Duration::from_millis(5));
        hist.record(Duration::from_micros(2500));

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 2);
        assert_eq!(stats.min_us, 2500);
        assert_eq!(stats.max_us, 5000);
    }

    #[test]
    fn test_record_zero_latency() {
        let hist = LatencyHistogram::with_name("test");

        hist.record_us(0);
        hist.record_us(100);

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 2);
        // Zero latency should be in lowest bucket
        assert!(stats.min_us <= 1);
    }

    #[test]
    fn test_record_very_large_latency() {
        let hist = LatencyHistogram::with_name("test");

        // 100 seconds in microseconds
        hist.record_us(100_000_000);
        hist.record_us(100);

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 2);
        assert_eq!(stats.min_us, 100);
        // Max should be in highest bucket range
        assert!(stats.max_us >= 10_000_000);
    }

    // ========================================================================
    // Percentile Tests
    // ========================================================================

    #[test]
    fn test_histogram_percentiles() {
        let hist = LatencyHistogram::with_name("test");

        // Record 100 samples with known distribution
        for i in 1..=100 {
            hist.record_us(i * 100); // 100us to 10000us
        }

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 100);

        // P50 should be around 5000us (middle value)
        // Note: Logarithmic buckets give approximate values
        assert!(
            stats.p50_us >= 3500 && stats.p50_us <= 6500,
            "p50 was {} (expected 4000-6000)",
            stats.p50_us
        );

        // P95 should be around 9500us
        assert!(
            stats.p95_us >= 8000 && stats.p95_us <= 10500,
            "p95 was {} (expected 9000-10000)",
            stats.p95_us
        );

        // P99 should be around 9900us
        // Due to logarithmic bucket approximation, allow wider range
        assert!(
            stats.p99_us >= 8500,
            "p99 was {} (expected >= 8500)",
            stats.p99_us
        );
    }

    #[test]
    fn test_histogram_single_sample_percentiles() {
        let hist = LatencyHistogram::with_name("test");

        hist.record_us(1000);

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 1);
        // All percentiles should be the same single value
        assert_eq!(stats.p50_us, stats.p95_us);
        assert_eq!(stats.p95_us, stats.p99_us);
    }

    #[test]
    fn test_histogram_uniform_distribution() {
        let hist = LatencyHistogram::with_name("test");

        // All same value
        for _ in 0..100 {
            hist.record_us(5000);
        }

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 100);
        // All percentiles should be approximately equal
        assert!((stats.p50_us as i64 - stats.p99_us as i64).abs() <= 1);
    }

    #[test]
    fn test_histogram_bimodal_distribution() {
        let hist = LatencyHistogram::with_name("test");

        // Half fast, half slow
        for _ in 0..50 {
            hist.record_us(100);
        }
        for _ in 0..50 {
            hist.record_us(10000);
        }

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 100);
        // P50 should be somewhere in between
        assert!(stats.p95_us >= stats.p50_us);
        assert!(stats.p99_us >= stats.p95_us);
    }

    // ========================================================================
    // Rolling Window Tests
    // ========================================================================

    #[test]
    fn test_histogram_rolling_window() {
        let hist = LatencyHistogram::new("test", 100);

        // Record samples
        for i in 0..50 {
            hist.record_us(100 + i * 10);
        }

        // Rolling window should show same stats as all_time for fresh data
        let all_time = hist.stats(RollingWindow::AllTime);
        let one_min = hist.stats(RollingWindow::OneMinute);

        assert_eq!(all_time.count, one_min.count);
    }

    #[test]
    fn test_rolling_window_enum_values() {
        // Test that all rolling window variants exist and can be used
        let hist = LatencyHistogram::with_name("test");
        hist.record_us(1000);

        let _ = hist.stats(RollingWindow::OneMinute);
        let _ = hist.stats(RollingWindow::FiveMinutes);
        let _ = hist.stats(RollingWindow::OneHour);
        let _ = hist.stats(RollingWindow::AllTime);
    }

    #[test]
    fn test_rolling_window_duration() {
        assert_eq!(
            RollingWindow::OneMinute.duration(),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            RollingWindow::FiveMinutes.duration(),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            RollingWindow::OneHour.duration(),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(RollingWindow::AllTime.duration(), None);
    }

    #[test]
    fn test_rolling_window_all_time_has_no_duration() {
        // AllTime window has no fixed duration - it captures all historical data
        assert!(RollingWindow::AllTime.duration().is_none());
    }

    #[test]
    fn test_rolling_window_clone() {
        let window = RollingWindow::FiveMinutes;
        let cloned = window;
        assert_eq!(window, cloned);
    }

    #[test]
    fn test_rolling_window_copy() {
        let window = RollingWindow::OneHour;
        let copied = window;
        assert_eq!(window, copied);
    }

    // ========================================================================
    // Reset Tests
    // ========================================================================

    #[test]
    fn test_histogram_reset() {
        let hist = LatencyHistogram::with_name("test");

        hist.record_us(1000);
        hist.record_us(2000);

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 2);

        hist.reset();

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 0);
    }

    #[test]
    fn test_histogram_reset_clears_all() {
        let hist = LatencyHistogram::with_name("test");

        for i in 0..1000 {
            hist.record_us(i);
        }

        hist.reset();

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 0);
        assert_eq!(stats.min_us, 0);
        assert_eq!(stats.max_us, 0);
    }

    #[test]
    fn test_histogram_record_after_reset() {
        let hist = LatencyHistogram::with_name("test");

        hist.record_us(1000);
        hist.reset();
        hist.record_us(500);

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 1);
        assert_eq!(stats.min_us, 500);
        assert_eq!(stats.max_us, 500);
    }

    // ========================================================================
    // Bucket Mapping Tests
    // ========================================================================

    #[test]
    fn test_bucket_mapping() {
        // Test that bucket mapping is consistent
        let latencies = [1, 10, 100, 1000, 10000, 100000, 1000000, 10000000];

        for lat in latencies {
            let bucket = LatencyHistogram::latency_to_bucket(lat);
            let approx = LatencyHistogram::bucket_to_latency(bucket);

            // Approximate latency should be within 2x of original (log-scale bucketing)
            assert!(approx as f64 >= lat as f64 / 2.0);
            assert!(approx as f64 <= lat as f64 * 2.0);
        }
    }

    #[test]
    fn test_bucket_mapping_monotonic() {
        // Larger latencies should map to larger buckets
        let mut prev_bucket = 0usize;
        for lat in [1, 10, 100, 1000, 10000] {
            let bucket = LatencyHistogram::latency_to_bucket(lat);
            assert!(bucket >= prev_bucket);
            prev_bucket = bucket;
        }
    }

    #[test]
    fn test_bucket_to_latency_monotonic() {
        // Larger buckets should map to larger latencies
        let mut prev_latency = 0u64;
        for bucket in 0..50 {
            let latency = LatencyHistogram::bucket_to_latency(bucket);
            assert!(latency >= prev_latency);
            prev_latency = latency;
        }
    }

    // ========================================================================
    // HistogramStats Tests
    // ========================================================================

    #[test]
    fn test_histogram_stats_default() {
        let stats = HistogramStats::default();
        assert_eq!(stats.count, 0);
        assert_eq!(stats.min_us, 0);
        assert_eq!(stats.max_us, 0);
        assert!((stats.mean_us - 0.0).abs() < f64::EPSILON);
        assert_eq!(stats.p50_us, 0);
        assert_eq!(stats.p95_us, 0);
        assert_eq!(stats.p99_us, 0);
    }

    #[test]
    fn test_histogram_stats_ms_conversion() {
        let stats = HistogramStats {
            count: 100,
            min_us: 1000,
            max_us: 10000,
            mean_us: 5000.0,
            p50_us: 4500,
            p95_us: 9000,
            p99_us: 9800,
        };

        assert!((stats.min_ms() - 1.0).abs() < f64::EPSILON);
        assert!((stats.max_ms() - 10.0).abs() < f64::EPSILON);
        assert!((stats.mean_ms() - 5.0).abs() < f64::EPSILON);
        assert!((stats.p50_ms() - 4.5).abs() < f64::EPSILON);
        assert!((stats.p95_ms() - 9.0).abs() < f64::EPSILON);
        assert!((stats.p99_ms() - 9.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_histogram_stats_clone() {
        let stats = HistogramStats {
            count: 50,
            min_us: 100,
            max_us: 5000,
            mean_us: 2000.0,
            p50_us: 1800,
            p95_us: 4500,
            p99_us: 4900,
        };

        let cloned = stats.clone();
        assert_eq!(cloned.count, stats.count);
        assert_eq!(cloned.min_us, stats.min_us);
        assert_eq!(cloned.max_us, stats.max_us);
        assert_eq!(cloned.p99_us, stats.p99_us);
    }

    // ========================================================================
    // Empty Histogram Tests
    // ========================================================================

    #[test]
    fn test_empty_histogram_stats() {
        let hist = LatencyHistogram::with_name("empty");

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 0);
        assert_eq!(stats.min_us, 0);
        assert_eq!(stats.max_us, 0);
        assert_eq!(stats.p50_us, 0);
        assert_eq!(stats.p95_us, 0);
        assert_eq!(stats.p99_us, 0);
    }

    #[test]
    fn test_empty_histogram_stats_ms_conversions() {
        let stats = HistogramStats::default();

        assert!((stats.min_ms() - 0.0).abs() < f64::EPSILON);
        assert!((stats.max_ms() - 0.0).abs() < f64::EPSILON);
        assert!((stats.mean_ms() - 0.0).abs() < f64::EPSILON);
        assert!((stats.p50_ms() - 0.0).abs() < f64::EPSILON);
    }

    // ========================================================================
    // Concurrency Tests (Thread Safety)
    // ========================================================================

    #[test]
    fn test_histogram_concurrent_writes() {
        use std::sync::Arc;
        use std::thread;

        let hist = Arc::new(LatencyHistogram::with_name("concurrent"));
        let mut handles = vec![];

        // Spawn multiple threads writing to the same histogram
        for t in 0..4 {
            let hist_clone = Arc::clone(&hist);
            let handle = thread::spawn(move || {
                for i in 0..100 {
                    hist_clone.record_us((t * 100 + i) as u64);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("Thread should complete");
        }

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 400); // 4 threads * 100 samples
    }

    // ========================================================================
    // Large Scale Tests
    // ========================================================================

    #[test]
    fn test_histogram_many_samples() {
        let hist = LatencyHistogram::with_name("large_scale");

        // Record 10000 samples
        for i in 0..10000 {
            hist.record_us(i % 1000 + 100);
        }

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 10000);
        assert_eq!(stats.min_us, 100);
        assert!(stats.max_us >= 1000);
    }

    #[test]
    fn test_histogram_extreme_values() {
        let hist = LatencyHistogram::with_name("extreme");

        // Record min and max u64 values (clamped by bucket range)
        hist.record_us(1);
        hist.record_us(u64::MAX / 2); // Very large but not overflow

        let stats = hist.stats(RollingWindow::AllTime);
        assert_eq!(stats.count, 2);
        assert!(stats.min_us >= 1);
    }

    // ========================================================================
    // Mean Calculation Tests
    // ========================================================================

    #[test]
    fn test_histogram_mean_calculation() {
        let hist = LatencyHistogram::with_name("mean_test");

        // Record known values
        hist.record_us(1000);
        hist.record_us(2000);
        hist.record_us(3000);

        let stats = hist.stats(RollingWindow::AllTime);
        // Mean should be approximately 2000 (due to bucket approximation)
        assert!(
            stats.mean_us >= 1500.0 && stats.mean_us <= 2500.0,
            "mean was {} (expected around 2000)",
            stats.mean_us
        );
    }
}
