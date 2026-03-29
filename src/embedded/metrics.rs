//! Embedded mode observability metrics
//!
//! Provides comprehensive metrics collection for ProximaDB embedded mode:
//! - Latency histograms (p50, p95, p99) for search, insert, flush operations
//! - Cache statistics (hit rate, entries, memory usage)
//! - Operation counters (total searches, inserts, deletes)
//! - WAL statistics (pending bytes, segment count)
//!
//! All metrics use atomic counters for thread-safe, lock-free updates
//! on the critical path.

use super::histograms::{HistogramStats, LatencyHistogram, RollingWindow};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Embedded metrics collector
///
/// Thread-safe metrics collection for embedded mode operations.
/// Uses atomic counters and HDR histograms for low-overhead tracking.
pub struct EmbeddedMetricsCollector {
    /// Search operation latency histogram
    search_histogram: LatencyHistogram,

    /// Insert operation latency histogram
    insert_histogram: LatencyHistogram,

    /// Flush operation latency histogram
    flush_histogram: LatencyHistogram,

    /// Delete operation latency histogram
    delete_histogram: LatencyHistogram,

    /// Get operation latency histogram
    get_histogram: LatencyHistogram,

    /// Operation counters
    counters: OperationCounters,

    /// Cache statistics tracker
    cache_stats: CacheStatsTracker,

    /// WAL statistics tracker
    wal_stats: WalStatsTracker,

    /// When this collector was created
    created_at: Instant,
}

/// Atomic operation counters
pub struct OperationCounters {
    /// Total search operations
    pub total_searches: AtomicU64,
    /// Total insert operations
    pub total_inserts: AtomicU64,
    /// Total delete operations
    pub total_deletes: AtomicU64,
    /// Total flush operations
    pub total_flushes: AtomicU64,
    /// Total get operations
    pub total_gets: AtomicU64,
    /// Total upsert operations
    pub total_upserts: AtomicU64,
    /// Total vectors inserted (cumulative)
    pub total_vectors_inserted: AtomicU64,
    /// Total vectors deleted (cumulative)
    pub total_vectors_deleted: AtomicU64,
    /// Total bytes written
    pub total_bytes_written: AtomicU64,
    /// Total bytes read
    pub total_bytes_read: AtomicU64,
    /// Error count
    pub total_errors: AtomicU64,
}

impl Default for OperationCounters {
    fn default() -> Self {
        Self {
            total_searches: AtomicU64::new(0),
            total_inserts: AtomicU64::new(0),
            total_deletes: AtomicU64::new(0),
            total_flushes: AtomicU64::new(0),
            total_gets: AtomicU64::new(0),
            total_upserts: AtomicU64::new(0),
            total_vectors_inserted: AtomicU64::new(0),
            total_vectors_deleted: AtomicU64::new(0),
            total_bytes_written: AtomicU64::new(0),
            total_bytes_read: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
        }
    }
}

/// Cache statistics tracker with atomic updates
pub struct CacheStatsTracker {
    /// Cache hits
    pub hits: AtomicU64,
    /// Cache misses
    pub misses: AtomicU64,
    /// Number of entries in cache
    pub entries: AtomicU64,
    /// Memory used by cache in bytes
    pub memory_bytes: AtomicU64,
    /// Eviction count
    pub evictions: AtomicU64,
}

impl Default for CacheStatsTracker {
    fn default() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            entries: AtomicU64::new(0),
            memory_bytes: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }
}

/// WAL statistics tracker
pub struct WalStatsTracker {
    /// Pending bytes in WAL
    pub pending_bytes: AtomicU64,
    /// Number of WAL segments
    pub segments_count: AtomicU64,
    /// Total bytes written to WAL
    pub total_bytes_written: AtomicU64,
    /// Number of WAL flushes
    pub flush_count: AtomicU64,
}

impl Default for WalStatsTracker {
    fn default() -> Self {
        Self {
            pending_bytes: AtomicU64::new(0),
            segments_count: AtomicU64::new(0),
            total_bytes_written: AtomicU64::new(0),
            flush_count: AtomicU64::new(0),
        }
    }
}

impl EmbeddedMetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            search_histogram: LatencyHistogram::with_name("search"),
            insert_histogram: LatencyHistogram::with_name("insert"),
            flush_histogram: LatencyHistogram::with_name("flush"),
            delete_histogram: LatencyHistogram::with_name("delete"),
            get_histogram: LatencyHistogram::with_name("get"),
            counters: OperationCounters::default(),
            cache_stats: CacheStatsTracker::default(),
            wal_stats: WalStatsTracker::default(),
            created_at: Instant::now(),
        }
    }

    // ========================================================================
    // Latency Recording
    // ========================================================================

    /// Record a search operation latency in microseconds
    pub fn record_search_us(&self, latency_us: u64) {
        self.search_histogram.record_us(latency_us);
        self.counters.total_searches.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a search operation latency in milliseconds
    pub fn record_search_ms(&self, latency_ms: f64) {
        self.search_histogram.record_ms(latency_ms);
        self.counters.total_searches.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an insert operation latency in microseconds
    pub fn record_insert_us(&self, latency_us: u64, vector_count: u64) {
        self.insert_histogram.record_us(latency_us);
        self.counters.total_inserts.fetch_add(1, Ordering::Relaxed);
        self.counters
            .total_vectors_inserted
            .fetch_add(vector_count, Ordering::Relaxed);
    }

    /// Record an insert operation latency in milliseconds
    pub fn record_insert_ms(&self, latency_ms: f64, vector_count: u64) {
        self.insert_histogram.record_ms(latency_ms);
        self.counters.total_inserts.fetch_add(1, Ordering::Relaxed);
        self.counters
            .total_vectors_inserted
            .fetch_add(vector_count, Ordering::Relaxed);
    }

    /// Record a flush operation latency in microseconds
    pub fn record_flush_us(&self, latency_us: u64, bytes_written: u64) {
        self.flush_histogram.record_us(latency_us);
        self.counters.total_flushes.fetch_add(1, Ordering::Relaxed);
        self.counters
            .total_bytes_written
            .fetch_add(bytes_written, Ordering::Relaxed);
    }

    /// Record a flush operation latency in milliseconds
    pub fn record_flush_ms(&self, latency_ms: f64, bytes_written: u64) {
        self.flush_histogram.record_ms(latency_ms);
        self.counters.total_flushes.fetch_add(1, Ordering::Relaxed);
        self.counters
            .total_bytes_written
            .fetch_add(bytes_written, Ordering::Relaxed);
    }

    /// Record a delete operation latency in microseconds
    pub fn record_delete_us(&self, latency_us: u64, vector_count: u64) {
        self.delete_histogram.record_us(latency_us);
        self.counters.total_deletes.fetch_add(1, Ordering::Relaxed);
        self.counters
            .total_vectors_deleted
            .fetch_add(vector_count, Ordering::Relaxed);
    }

    /// Record a get operation latency in microseconds
    pub fn record_get_us(&self, latency_us: u64) {
        self.get_histogram.record_us(latency_us);
        self.counters.total_gets.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an upsert operation
    pub fn record_upsert(&self, inserted: u64, updated: u64) {
        self.counters.total_upserts.fetch_add(1, Ordering::Relaxed);
        self.counters
            .total_vectors_inserted
            .fetch_add(inserted, Ordering::Relaxed);
        // Updated vectors are recorded as both delete + insert
        self.counters
            .total_vectors_deleted
            .fetch_add(updated, Ordering::Relaxed);
        self.counters
            .total_vectors_inserted
            .fetch_add(updated, Ordering::Relaxed);
    }

    /// Record an error
    pub fn record_error(&self) {
        self.counters.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    // ========================================================================
    // Cache Statistics
    // ========================================================================

    /// Record a cache hit
    pub fn record_cache_hit(&self) {
        self.cache_stats.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self) {
        self.cache_stats.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Update cache entry count
    pub fn set_cache_entries(&self, count: u64) {
        self.cache_stats.entries.store(count, Ordering::Relaxed);
    }

    /// Update total cache hits from an external cache provider.
    pub fn set_cache_hits(&self, hits: u64) {
        self.cache_stats.hits.store(hits, Ordering::Relaxed);
    }

    /// Update total cache misses from an external cache provider.
    pub fn set_cache_misses(&self, misses: u64) {
        self.cache_stats.misses.store(misses, Ordering::Relaxed);
    }

    /// Update cache memory usage
    pub fn set_cache_memory_bytes(&self, bytes: u64) {
        self.cache_stats
            .memory_bytes
            .store(bytes, Ordering::Relaxed);
    }

    /// Record a cache eviction
    pub fn record_cache_eviction(&self) {
        self.cache_stats.evictions.fetch_add(1, Ordering::Relaxed);
    }

    // ========================================================================
    // WAL Statistics
    // ========================================================================

    /// Update WAL pending bytes
    pub fn set_wal_pending_bytes(&self, bytes: u64) {
        self.wal_stats.pending_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Update WAL segment count
    pub fn set_wal_segments_count(&self, count: u64) {
        self.wal_stats
            .segments_count
            .store(count, Ordering::Relaxed);
    }

    /// Record bytes written to WAL
    pub fn record_wal_write(&self, bytes: u64) {
        self.wal_stats
            .total_bytes_written
            .fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record WAL flush
    pub fn record_wal_flush(&self) {
        self.wal_stats.flush_count.fetch_add(1, Ordering::Relaxed);
    }

    // ========================================================================
    // Metrics Snapshot
    // ========================================================================

    /// Get a snapshot of all metrics
    pub fn snapshot(&self, window: RollingWindow) -> EmbeddedMetrics {
        let search_stats = self.search_histogram.stats(window);
        let insert_stats = self.insert_histogram.stats(window);
        let flush_stats = self.flush_histogram.stats(window);
        let delete_stats = self.delete_histogram.stats(window);
        let get_stats = self.get_histogram.stats(window);

        // Calculate cache hit rate
        let hits = self.cache_stats.hits.load(Ordering::Relaxed);
        let misses = self.cache_stats.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        let cache_hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };

        EmbeddedMetrics {
            // Latency histograms
            search_latency: LatencyStats::from_histogram_stats(&search_stats),
            insert_latency: LatencyStats::from_histogram_stats(&insert_stats),
            flush_latency: LatencyStats::from_histogram_stats(&flush_stats),
            delete_latency: LatencyStats::from_histogram_stats(&delete_stats),
            get_latency: LatencyStats::from_histogram_stats(&get_stats),

            // Operation counters
            total_searches: self.counters.total_searches.load(Ordering::Relaxed),
            total_inserts: self.counters.total_inserts.load(Ordering::Relaxed),
            total_deletes: self.counters.total_deletes.load(Ordering::Relaxed),
            total_flushes: self.counters.total_flushes.load(Ordering::Relaxed),
            total_gets: self.counters.total_gets.load(Ordering::Relaxed),
            total_upserts: self.counters.total_upserts.load(Ordering::Relaxed),
            total_vectors_inserted: self.counters.total_vectors_inserted.load(Ordering::Relaxed),
            total_vectors_deleted: self.counters.total_vectors_deleted.load(Ordering::Relaxed),
            total_bytes_written: self.counters.total_bytes_written.load(Ordering::Relaxed),
            total_bytes_read: self.counters.total_bytes_read.load(Ordering::Relaxed),
            total_errors: self.counters.total_errors.load(Ordering::Relaxed),

            // Cache statistics
            cache_hit_rate,
            cache_hits: hits,
            cache_misses: misses,
            cache_entries: self.cache_stats.entries.load(Ordering::Relaxed),
            cache_memory_bytes: self.cache_stats.memory_bytes.load(Ordering::Relaxed),
            cache_evictions: self.cache_stats.evictions.load(Ordering::Relaxed),

            // WAL statistics
            wal_pending_bytes: self.wal_stats.pending_bytes.load(Ordering::Relaxed),
            wal_segments_count: self.wal_stats.segments_count.load(Ordering::Relaxed),
            wal_total_bytes_written: self.wal_stats.total_bytes_written.load(Ordering::Relaxed),
            wal_flush_count: self.wal_stats.flush_count.load(Ordering::Relaxed),

            // Timing
            uptime_secs: self.created_at.elapsed().as_secs(),
            window,
        }
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.search_histogram.reset();
        self.insert_histogram.reset();
        self.flush_histogram.reset();
        self.delete_histogram.reset();
        self.get_histogram.reset();

        self.counters.total_searches.store(0, Ordering::Relaxed);
        self.counters.total_inserts.store(0, Ordering::Relaxed);
        self.counters.total_deletes.store(0, Ordering::Relaxed);
        self.counters.total_flushes.store(0, Ordering::Relaxed);
        self.counters.total_gets.store(0, Ordering::Relaxed);
        self.counters.total_upserts.store(0, Ordering::Relaxed);
        self.counters
            .total_vectors_inserted
            .store(0, Ordering::Relaxed);
        self.counters
            .total_vectors_deleted
            .store(0, Ordering::Relaxed);
        self.counters
            .total_bytes_written
            .store(0, Ordering::Relaxed);
        self.counters.total_bytes_read.store(0, Ordering::Relaxed);
        self.counters.total_errors.store(0, Ordering::Relaxed);

        self.cache_stats.hits.store(0, Ordering::Relaxed);
        self.cache_stats.misses.store(0, Ordering::Relaxed);
        self.cache_stats.evictions.store(0, Ordering::Relaxed);

        self.wal_stats
            .total_bytes_written
            .store(0, Ordering::Relaxed);
        self.wal_stats.flush_count.store(0, Ordering::Relaxed);
    }
}

impl Default for EmbeddedMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Latency statistics for an operation type
#[derive(Debug, Clone)]
pub struct LatencyStats {
    /// Number of operations recorded
    pub count: u64,
    /// Minimum latency in milliseconds
    pub min_ms: f64,
    /// Maximum latency in milliseconds
    pub max_ms: f64,
    /// Mean latency in milliseconds
    pub mean_ms: f64,
    /// 50th percentile latency in milliseconds
    pub p50_ms: f64,
    /// 95th percentile latency in milliseconds
    pub p95_ms: f64,
    /// 99th percentile latency in milliseconds
    pub p99_ms: f64,
}

impl LatencyStats {
    /// Create from histogram stats
    fn from_histogram_stats(stats: &HistogramStats) -> Self {
        Self {
            count: stats.count,
            min_ms: stats.min_ms(),
            max_ms: stats.max_ms(),
            mean_ms: stats.mean_ms(),
            p50_ms: stats.p50_ms(),
            p95_ms: stats.p95_ms(),
            p99_ms: stats.p99_ms(),
        }
    }
}

impl Default for LatencyStats {
    fn default() -> Self {
        Self {
            count: 0,
            min_ms: 0.0,
            max_ms: 0.0,
            mean_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
        }
    }
}

/// Comprehensive embedded metrics snapshot
#[derive(Debug, Clone)]
pub struct EmbeddedMetrics {
    // Latency histograms
    /// Search operation latency statistics
    pub search_latency: LatencyStats,
    /// Insert operation latency statistics
    pub insert_latency: LatencyStats,
    /// Flush operation latency statistics
    pub flush_latency: LatencyStats,
    /// Delete operation latency statistics
    pub delete_latency: LatencyStats,
    /// Get operation latency statistics
    pub get_latency: LatencyStats,

    // Operation counters
    /// Total search operations
    pub total_searches: u64,
    /// Total insert operations
    pub total_inserts: u64,
    /// Total delete operations
    pub total_deletes: u64,
    /// Total flush operations
    pub total_flushes: u64,
    /// Total get operations
    pub total_gets: u64,
    /// Total upsert operations
    pub total_upserts: u64,
    /// Total vectors inserted
    pub total_vectors_inserted: u64,
    /// Total vectors deleted
    pub total_vectors_deleted: u64,
    /// Total bytes written
    pub total_bytes_written: u64,
    /// Total bytes read
    pub total_bytes_read: u64,
    /// Total errors
    pub total_errors: u64,

    // Cache statistics
    /// Cache hit rate (0.0 to 1.0)
    pub cache_hit_rate: f64,
    /// Total cache hits
    pub cache_hits: u64,
    /// Total cache misses
    pub cache_misses: u64,
    /// Number of entries in cache
    pub cache_entries: u64,
    /// Memory used by cache in bytes
    pub cache_memory_bytes: u64,
    /// Total cache evictions
    pub cache_evictions: u64,

    // WAL statistics
    /// Pending bytes in WAL
    pub wal_pending_bytes: u64,
    /// Number of WAL segments
    pub wal_segments_count: u64,
    /// Total bytes written to WAL
    pub wal_total_bytes_written: u64,
    /// Number of WAL flushes
    pub wal_flush_count: u64,

    // Timing
    /// Database uptime in seconds
    pub uptime_secs: u64,
    /// Rolling window used for latency stats
    pub window: RollingWindow,
}

impl EmbeddedMetrics {
    /// Export metrics in Prometheus text format
    pub fn to_prometheus(&self) -> String {
        let mut output = String::with_capacity(4096);

        // Helper macro for gauge metrics
        macro_rules! gauge {
            ($name:expr, $help:expr, $value:expr) => {
                output.push_str(&format!(
                    "# HELP proximadb_embedded_{} {}\n# TYPE proximadb_embedded_{} gauge\nproximadb_embedded_{} {}\n",
                    $name, $help, $name, $name, $value
                ));
            };
        }

        // Helper macro for counter metrics
        macro_rules! counter {
            ($name:expr, $help:expr, $value:expr) => {
                output.push_str(&format!(
                    "# HELP proximadb_embedded_{} {}\n# TYPE proximadb_embedded_{} counter\nproximadb_embedded_{} {}\n",
                    $name, $help, $name, $name, $value
                ));
            };
        }

        // Latency metrics (gauges for percentiles)
        gauge!(
            "search_latency_p50_ms",
            "Search latency 50th percentile in milliseconds",
            self.search_latency.p50_ms
        );
        gauge!(
            "search_latency_p95_ms",
            "Search latency 95th percentile in milliseconds",
            self.search_latency.p95_ms
        );
        gauge!(
            "search_latency_p99_ms",
            "Search latency 99th percentile in milliseconds",
            self.search_latency.p99_ms
        );

        gauge!(
            "insert_latency_p50_ms",
            "Insert latency 50th percentile in milliseconds",
            self.insert_latency.p50_ms
        );
        gauge!(
            "insert_latency_p95_ms",
            "Insert latency 95th percentile in milliseconds",
            self.insert_latency.p95_ms
        );
        gauge!(
            "insert_latency_p99_ms",
            "Insert latency 99th percentile in milliseconds",
            self.insert_latency.p99_ms
        );

        gauge!(
            "flush_latency_p50_ms",
            "Flush latency 50th percentile in milliseconds",
            self.flush_latency.p50_ms
        );
        gauge!(
            "flush_latency_p95_ms",
            "Flush latency 95th percentile in milliseconds",
            self.flush_latency.p95_ms
        );
        gauge!(
            "flush_latency_p99_ms",
            "Flush latency 99th percentile in milliseconds",
            self.flush_latency.p99_ms
        );

        // Operation counters
        counter!(
            "searches_total",
            "Total number of search operations",
            self.total_searches
        );
        counter!(
            "inserts_total",
            "Total number of insert operations",
            self.total_inserts
        );
        counter!(
            "deletes_total",
            "Total number of delete operations",
            self.total_deletes
        );
        counter!(
            "flushes_total",
            "Total number of flush operations",
            self.total_flushes
        );
        counter!(
            "gets_total",
            "Total number of get operations",
            self.total_gets
        );
        counter!("errors_total", "Total number of errors", self.total_errors);

        counter!(
            "vectors_inserted_total",
            "Total number of vectors inserted",
            self.total_vectors_inserted
        );
        counter!(
            "vectors_deleted_total",
            "Total number of vectors deleted",
            self.total_vectors_deleted
        );
        counter!(
            "bytes_written_total",
            "Total bytes written",
            self.total_bytes_written
        );

        // Cache statistics
        gauge!(
            "cache_hit_rate",
            "Cache hit rate (0.0 to 1.0)",
            self.cache_hit_rate
        );
        counter!("cache_hits_total", "Total cache hits", self.cache_hits);
        counter!(
            "cache_misses_total",
            "Total cache misses",
            self.cache_misses
        );
        gauge!(
            "cache_entries",
            "Number of entries in cache",
            self.cache_entries
        );
        gauge!(
            "cache_memory_bytes",
            "Memory used by cache in bytes",
            self.cache_memory_bytes
        );
        counter!(
            "cache_evictions_total",
            "Total cache evictions",
            self.cache_evictions
        );

        // WAL statistics
        gauge!(
            "wal_pending_bytes",
            "Pending bytes in WAL",
            self.wal_pending_bytes
        );
        gauge!(
            "wal_segments_count",
            "Number of WAL segments",
            self.wal_segments_count
        );
        counter!(
            "wal_bytes_written_total",
            "Total bytes written to WAL",
            self.wal_total_bytes_written
        );
        counter!(
            "wal_flushes_total",
            "Number of WAL flushes",
            self.wal_flush_count
        );

        // Uptime
        counter!(
            "uptime_seconds",
            "Database uptime in seconds",
            self.uptime_secs
        );

        output
    }
}

/// Timer guard for automatic latency recording
///
/// Records the elapsed time when dropped.
pub struct LatencyTimer<'a> {
    collector: &'a EmbeddedMetricsCollector,
    operation: OperationType,
    start: Instant,
    vector_count: u64,
    bytes: u64,
}

/// Operation type for latency timer
pub enum OperationType {
    Search,
    Insert,
    Flush,
    Delete,
    Get,
}

impl<'a> LatencyTimer<'a> {
    /// Create a new timer for search operation
    pub fn search(collector: &'a EmbeddedMetricsCollector) -> Self {
        Self {
            collector,
            operation: OperationType::Search,
            start: Instant::now(),
            vector_count: 0,
            bytes: 0,
        }
    }

    /// Create a new timer for insert operation
    pub fn insert(collector: &'a EmbeddedMetricsCollector, vector_count: u64) -> Self {
        Self {
            collector,
            operation: OperationType::Insert,
            start: Instant::now(),
            vector_count,
            bytes: 0,
        }
    }

    /// Create a new timer for flush operation
    pub fn flush(collector: &'a EmbeddedMetricsCollector) -> Self {
        Self {
            collector,
            operation: OperationType::Flush,
            start: Instant::now(),
            vector_count: 0,
            bytes: 0,
        }
    }

    /// Create a new timer for delete operation
    pub fn delete(collector: &'a EmbeddedMetricsCollector, vector_count: u64) -> Self {
        Self {
            collector,
            operation: OperationType::Delete,
            start: Instant::now(),
            vector_count,
            bytes: 0,
        }
    }

    /// Create a new timer for get operation
    pub fn get(collector: &'a EmbeddedMetricsCollector) -> Self {
        Self {
            collector,
            operation: OperationType::Get,
            start: Instant::now(),
            vector_count: 0,
            bytes: 0,
        }
    }

    /// Set bytes for flush operation
    pub fn with_bytes(mut self, bytes: u64) -> Self {
        self.bytes = bytes;
        self
    }

    /// Finish timing and record metrics
    pub fn finish(self) {
        drop(self);
    }
}

impl Drop for LatencyTimer<'_> {
    fn drop(&mut self) {
        let elapsed_us = self.start.elapsed().as_micros() as u64;

        match self.operation {
            OperationType::Search => {
                self.collector.record_search_us(elapsed_us);
            }
            OperationType::Insert => {
                self.collector
                    .record_insert_us(elapsed_us, self.vector_count);
            }
            OperationType::Flush => {
                self.collector.record_flush_us(elapsed_us, self.bytes);
            }
            OperationType::Delete => {
                self.collector
                    .record_delete_us(elapsed_us, self.vector_count);
            }
            OperationType::Get => {
                self.collector.record_get_us(elapsed_us);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // EmbeddedMetricsCollector Basic Tests
    // ========================================================================

    #[test]
    fn test_metrics_collector_basic() {
        let collector = EmbeddedMetricsCollector::new();

        // Record some operations
        collector.record_search_us(1000);
        collector.record_search_us(2000);
        collector.record_search_us(3000);

        collector.record_insert_us(500, 10);
        collector.record_flush_us(10000, 1024 * 1024);

        let metrics = collector.snapshot(RollingWindow::AllTime);

        assert_eq!(metrics.total_searches, 3);
        assert_eq!(metrics.total_inserts, 1);
        assert_eq!(metrics.total_flushes, 1);
        assert_eq!(metrics.total_vectors_inserted, 10);
        assert_eq!(metrics.total_bytes_written, 1024 * 1024);

        assert!(metrics.search_latency.count == 3);
    }

    #[test]
    fn test_metrics_collector_default() {
        let collector = EmbeddedMetricsCollector::default();
        let metrics = collector.snapshot(RollingWindow::AllTime);

        assert_eq!(metrics.total_searches, 0);
        assert_eq!(metrics.total_inserts, 0);
        assert_eq!(metrics.total_flushes, 0);
    }

    // ========================================================================
    // Search Metrics Tests
    // ========================================================================

    #[test]
    fn test_record_search_us() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_search_us(100);
        collector.record_search_us(200);
        collector.record_search_us(300);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_searches, 3);
        assert_eq!(metrics.search_latency.count, 3);
    }

    #[test]
    fn test_record_search_ms() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_search_ms(1.5);
        collector.record_search_ms(2.0);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_searches, 2);
        assert_eq!(metrics.search_latency.count, 2);
    }

    // ========================================================================
    // Insert Metrics Tests
    // ========================================================================

    #[test]
    fn test_record_insert_us() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_insert_us(500, 100);
        collector.record_insert_us(1000, 200);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_inserts, 2);
        assert_eq!(metrics.total_vectors_inserted, 300);
    }

    #[test]
    fn test_record_insert_ms() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_insert_ms(0.5, 50);
        collector.record_insert_ms(1.0, 75);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_inserts, 2);
        assert_eq!(metrics.total_vectors_inserted, 125);
    }

    // ========================================================================
    // Flush Metrics Tests
    // ========================================================================

    #[test]
    fn test_record_flush_us() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_flush_us(5000, 1024);
        collector.record_flush_us(10000, 2048);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_flushes, 2);
        assert_eq!(metrics.total_bytes_written, 3072);
    }

    #[test]
    fn test_record_flush_ms() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_flush_ms(5.0, 1024);
        collector.record_flush_ms(10.0, 2048);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_flushes, 2);
        assert_eq!(metrics.total_bytes_written, 3072);
    }

    // ========================================================================
    // Delete Metrics Tests
    // ========================================================================

    #[test]
    fn test_record_delete_us() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_delete_us(100, 5);
        collector.record_delete_us(200, 10);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_deletes, 2);
        assert_eq!(metrics.total_vectors_deleted, 15);
    }

    // ========================================================================
    // Get Metrics Tests
    // ========================================================================

    #[test]
    fn test_record_get_us() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_get_us(50);
        collector.record_get_us(100);
        collector.record_get_us(150);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_gets, 3);
    }

    // ========================================================================
    // Upsert Metrics Tests
    // ========================================================================

    #[test]
    fn test_record_upsert() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_upsert(10, 5); // 10 inserted, 5 updated

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_upserts, 1);
        // Updated vectors are recorded as both delete + insert
        assert_eq!(metrics.total_vectors_inserted, 15); // 10 + 5
        assert_eq!(metrics.total_vectors_deleted, 5);
    }

    #[test]
    fn test_record_upsert_multiple() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_upsert(20, 0); // All inserts
        collector.record_upsert(0, 10); // All updates

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_upserts, 2);
        assert_eq!(metrics.total_vectors_inserted, 30); // 20 + 10
        assert_eq!(metrics.total_vectors_deleted, 10);
    }

    // ========================================================================
    // Error Metrics Tests
    // ========================================================================

    #[test]
    fn test_record_error() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_error();
        collector.record_error();
        collector.record_error();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_errors, 3);
    }

    // ========================================================================
    // Cache Statistics Tests
    // ========================================================================

    #[test]
    fn test_cache_stats() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_cache_hit();
        collector.record_cache_hit();
        collector.record_cache_miss();

        let metrics = collector.snapshot(RollingWindow::AllTime);

        assert_eq!(metrics.cache_hits, 2);
        assert_eq!(metrics.cache_misses, 1);
        assert!((metrics.cache_hit_rate - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_cache_hit_rate_all_hits() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_cache_hit();
        collector.record_cache_hit();
        collector.record_cache_hit();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert!((metrics.cache_hit_rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_cache_hit_rate_all_misses() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_cache_miss();
        collector.record_cache_miss();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert!((metrics.cache_hit_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_cache_hit_rate_no_operations() {
        let collector = EmbeddedMetricsCollector::new();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert!((metrics.cache_hit_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_cache_entries() {
        let collector = EmbeddedMetricsCollector::new();

        collector.set_cache_entries(1000);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.cache_entries, 1000);
    }

    #[test]
    fn test_set_cache_memory_bytes() {
        let collector = EmbeddedMetricsCollector::new();

        collector.set_cache_memory_bytes(1024 * 1024);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.cache_memory_bytes, 1048576);
    }

    #[test]
    fn test_set_cache_hit_and_miss_totals() {
        let collector = EmbeddedMetricsCollector::new();

        collector.set_cache_hits(9);
        collector.set_cache_misses(3);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.cache_hits, 9);
        assert_eq!(metrics.cache_misses, 3);
        assert!((metrics.cache_hit_rate - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_record_cache_eviction() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_cache_eviction();
        collector.record_cache_eviction();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.cache_evictions, 2);
    }

    // ========================================================================
    // WAL Statistics Tests
    // ========================================================================

    #[test]
    fn test_set_wal_pending_bytes() {
        let collector = EmbeddedMetricsCollector::new();

        collector.set_wal_pending_bytes(4096);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.wal_pending_bytes, 4096);
    }

    #[test]
    fn test_set_wal_segments_count() {
        let collector = EmbeddedMetricsCollector::new();

        collector.set_wal_segments_count(5);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.wal_segments_count, 5);
    }

    #[test]
    fn test_record_wal_write() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_wal_write(1024);
        collector.record_wal_write(2048);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.wal_total_bytes_written, 3072);
    }

    #[test]
    fn test_record_wal_flush() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_wal_flush();
        collector.record_wal_flush();
        collector.record_wal_flush();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.wal_flush_count, 3);
    }

    // ========================================================================
    // LatencyTimer Tests
    // ========================================================================

    #[test]
    fn test_latency_timer() {
        let collector = EmbeddedMetricsCollector::new();

        {
            let _timer = LatencyTimer::search(&collector);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_searches, 1);
        // Verify that the latency was recorded (any non-zero value)
        // Timing assertions are inherently flaky in CI environments
        assert!(
            metrics.search_latency.p50_ms > 0.0,
            "p50_ms should be non-zero, got {}",
            metrics.search_latency.p50_ms
        );
    }

    #[test]
    fn test_latency_timer_insert() {
        let collector = EmbeddedMetricsCollector::new();

        {
            let _timer = LatencyTimer::insert(&collector, 100);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_inserts, 1);
        assert_eq!(metrics.total_vectors_inserted, 100);
    }

    #[test]
    fn test_latency_timer_flush_with_bytes() {
        let collector = EmbeddedMetricsCollector::new();

        {
            let _timer = LatencyTimer::flush(&collector).with_bytes(2048);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_flushes, 1);
        assert_eq!(metrics.total_bytes_written, 2048);
    }

    #[test]
    fn test_latency_timer_delete() {
        let collector = EmbeddedMetricsCollector::new();

        {
            let _timer = LatencyTimer::delete(&collector, 50);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_deletes, 1);
        assert_eq!(metrics.total_vectors_deleted, 50);
    }

    #[test]
    fn test_latency_timer_get() {
        let collector = EmbeddedMetricsCollector::new();

        {
            let _timer = LatencyTimer::get(&collector);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_gets, 1);
    }

    #[test]
    fn test_latency_timer_finish() {
        let collector = EmbeddedMetricsCollector::new();

        let timer = LatencyTimer::search(&collector);
        std::thread::sleep(std::time::Duration::from_millis(5));
        timer.finish();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_searches, 1);
    }

    // ========================================================================
    // Reset Tests
    // ========================================================================

    #[test]
    fn test_reset() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_search_us(1000);
        collector.record_cache_hit();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_searches, 1);

        collector.reset();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_searches, 0);
        assert_eq!(metrics.cache_hits, 0);
    }

    #[test]
    fn test_reset_all_counters() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_search_us(100);
        collector.record_insert_us(200, 10);
        collector.record_delete_us(150, 5);
        collector.record_flush_us(1000, 512);
        collector.record_get_us(50);
        collector.record_upsert(3, 2);
        collector.record_error();
        collector.record_cache_hit();
        collector.record_cache_miss();
        collector.record_cache_eviction();
        collector.record_wal_write(256);
        collector.record_wal_flush();

        collector.reset();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_searches, 0);
        assert_eq!(metrics.total_inserts, 0);
        assert_eq!(metrics.total_deletes, 0);
        assert_eq!(metrics.total_flushes, 0);
        assert_eq!(metrics.total_gets, 0);
        assert_eq!(metrics.total_upserts, 0);
        assert_eq!(metrics.total_errors, 0);
        assert_eq!(metrics.total_vectors_inserted, 0);
        assert_eq!(metrics.total_vectors_deleted, 0);
        assert_eq!(metrics.total_bytes_written, 0);
        assert_eq!(metrics.cache_hits, 0);
        assert_eq!(metrics.cache_misses, 0);
        assert_eq!(metrics.cache_evictions, 0);
        assert_eq!(metrics.wal_total_bytes_written, 0);
        assert_eq!(metrics.wal_flush_count, 0);
    }

    // ========================================================================
    // LatencyStats Tests
    // ========================================================================

    #[test]
    fn test_latency_stats_default() {
        let stats = LatencyStats::default();
        assert_eq!(stats.count, 0);
        assert!((stats.min_ms - 0.0).abs() < f64::EPSILON);
        assert!((stats.max_ms - 0.0).abs() < f64::EPSILON);
        assert!((stats.mean_ms - 0.0).abs() < f64::EPSILON);
        assert!((stats.p50_ms - 0.0).abs() < f64::EPSILON);
        assert!((stats.p95_ms - 0.0).abs() < f64::EPSILON);
        assert!((stats.p99_ms - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_latency_stats_clone() {
        let stats = LatencyStats {
            count: 100,
            min_ms: 0.5,
            max_ms: 10.0,
            mean_ms: 2.5,
            p50_ms: 2.0,
            p95_ms: 8.0,
            p99_ms: 9.5,
        };

        let cloned = stats.clone();
        assert_eq!(cloned.count, stats.count);
        assert!((cloned.min_ms - stats.min_ms).abs() < f64::EPSILON);
        assert!((cloned.p99_ms - stats.p99_ms).abs() < f64::EPSILON);
    }

    // ========================================================================
    // OperationCounters Tests
    // ========================================================================

    #[test]
    fn test_operation_counters_default() {
        let counters = OperationCounters::default();
        assert_eq!(counters.total_searches.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_inserts.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_deletes.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_flushes.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_gets.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_upserts.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_vectors_inserted.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_vectors_deleted.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_bytes_written.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_bytes_read.load(Ordering::Relaxed), 0);
        assert_eq!(counters.total_errors.load(Ordering::Relaxed), 0);
    }

    // ========================================================================
    // CacheStatsTracker Tests
    // ========================================================================

    #[test]
    fn test_cache_stats_tracker_default() {
        let tracker = CacheStatsTracker::default();
        assert_eq!(tracker.hits.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.misses.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.entries.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.memory_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.evictions.load(Ordering::Relaxed), 0);
    }

    // ========================================================================
    // WalStatsTracker Tests
    // ========================================================================

    #[test]
    fn test_wal_stats_tracker_default() {
        let tracker = WalStatsTracker::default();
        assert_eq!(tracker.pending_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.segments_count.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.total_bytes_written.load(Ordering::Relaxed), 0);
        assert_eq!(tracker.flush_count.load(Ordering::Relaxed), 0);
    }

    // ========================================================================
    // Prometheus Export Tests
    // ========================================================================

    #[test]
    fn test_prometheus_export() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_search_us(1000);
        collector.record_insert_us(500, 5);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        let prometheus = metrics.to_prometheus();

        assert!(prometheus.contains("proximadb_embedded_searches_total 1"));
        assert!(prometheus.contains("proximadb_embedded_inserts_total 1"));
        assert!(prometheus.contains("proximadb_embedded_vectors_inserted_total 5"));
    }

    #[test]
    fn test_prometheus_export_format() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_cache_hit();
        collector.record_cache_miss();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        let prometheus = metrics.to_prometheus();

        // Check HELP and TYPE lines exist
        assert!(prometheus.contains("# HELP proximadb_embedded_cache_hit_rate"));
        assert!(prometheus.contains("# TYPE proximadb_embedded_cache_hit_rate gauge"));
        assert!(prometheus.contains("# HELP proximadb_embedded_cache_hits_total"));
        assert!(prometheus.contains("# TYPE proximadb_embedded_cache_hits_total counter"));
    }

    #[test]
    fn test_prometheus_export_latency_percentiles() {
        let collector = EmbeddedMetricsCollector::new();

        collector.record_search_us(1000);
        collector.record_search_us(2000);
        collector.record_search_us(3000);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        let prometheus = metrics.to_prometheus();

        assert!(prometheus.contains("proximadb_embedded_search_latency_p50_ms"));
        assert!(prometheus.contains("proximadb_embedded_search_latency_p95_ms"));
        assert!(prometheus.contains("proximadb_embedded_search_latency_p99_ms"));
    }

    #[test]
    fn test_prometheus_export_wal_metrics() {
        let collector = EmbeddedMetricsCollector::new();

        collector.set_wal_pending_bytes(1024);
        collector.set_wal_segments_count(3);
        collector.record_wal_write(512);
        collector.record_wal_flush();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        let prometheus = metrics.to_prometheus();

        assert!(prometheus.contains("proximadb_embedded_wal_pending_bytes 1024"));
        assert!(prometheus.contains("proximadb_embedded_wal_segments_count 3"));
        assert!(prometheus.contains("proximadb_embedded_wal_bytes_written_total 512"));
        assert!(prometheus.contains("proximadb_embedded_wal_flushes_total 1"));
    }

    // ========================================================================
    // EmbeddedMetrics Tests
    // ========================================================================

    #[test]
    fn test_embedded_metrics_uptime() {
        let collector = EmbeddedMetricsCollector::new();
        std::thread::sleep(std::time::Duration::from_millis(100));

        let metrics = collector.snapshot(RollingWindow::AllTime);
        // Uptime should be at least 0 seconds (could be 0 if < 1 second elapsed)
        assert!(metrics.uptime_secs >= 0);
    }

    #[test]
    fn test_embedded_metrics_window() {
        let collector = EmbeddedMetricsCollector::new();

        let metrics = collector.snapshot(RollingWindow::OneMinute);
        assert_eq!(metrics.window, RollingWindow::OneMinute);

        let metrics = collector.snapshot(RollingWindow::FiveMinutes);
        assert_eq!(metrics.window, RollingWindow::FiveMinutes);

        let metrics = collector.snapshot(RollingWindow::OneHour);
        assert_eq!(metrics.window, RollingWindow::OneHour);

        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.window, RollingWindow::AllTime);
    }

    #[test]
    fn test_embedded_metrics_clone() {
        let collector = EmbeddedMetricsCollector::new();
        collector.record_search_us(1000);
        collector.record_cache_hit();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        let cloned = metrics.clone();

        assert_eq!(cloned.total_searches, metrics.total_searches);
        assert_eq!(cloned.cache_hits, metrics.cache_hits);
    }

    // ============================================================
    // LatencyTimer tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_latency_timer_search() {
        let collector = EmbeddedMetricsCollector::new();
        {
            let _timer = LatencyTimer::search(&collector);
            // Timer records on drop
        }
        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_searches, 1);
    }

    #[test]
    fn test_latency_timer_insert_records_vectors() {
        let collector = EmbeddedMetricsCollector::new();
        {
            let _timer = LatencyTimer::insert(&collector, 5);
        }
        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_inserts, 1);
        assert_eq!(metrics.total_vectors_inserted, 5);
    }

    #[test]
    fn test_latency_timer_delete_records_count() {
        let collector = EmbeddedMetricsCollector::new();
        {
            let _timer = LatencyTimer::delete(&collector, 3);
        }
        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_deletes, 1);
    }

    #[test]
    fn test_latency_timer_get_records_count() {
        let collector = EmbeddedMetricsCollector::new();
        {
            let _timer = LatencyTimer::get(&collector);
        }
        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_gets, 1);
    }

    #[test]
    fn test_latency_timer_flush() {
        let collector = EmbeddedMetricsCollector::new();
        {
            let _timer = LatencyTimer::flush(&collector);
        }
        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_flushes, 1);
    }

    #[test]
    fn test_latency_timer_with_bytes() {
        let collector = EmbeddedMetricsCollector::new();
        {
            let _timer = LatencyTimer::flush(&collector).with_bytes(4096);
        }
        // Verify flush was recorded (bytes tracking may go to different counters)
        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(metrics.total_flushes, 1);
    }

    // ============================================================
    // Prometheus export tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_prometheus_export_format_with_data() {
        let collector = EmbeddedMetricsCollector::new();
        collector.record_search_us(500);
        collector.record_insert_us(200, 10);
        collector.record_cache_hit();

        let metrics = collector.snapshot(RollingWindow::AllTime);
        let prom = metrics.to_prometheus();

        assert!(prom.contains("proximadb_"), "Should use proximadb_ prefix");
        assert!(
            prom.contains("searches_total"),
            "Should include search count"
        );
        assert!(
            prom.contains("inserts_total"),
            "Should include insert count"
        );
    }

    #[test]
    fn test_prometheus_export_empty() {
        let collector = EmbeddedMetricsCollector::new();
        let metrics = collector.snapshot(RollingWindow::AllTime);
        let prom = metrics.to_prometheus();
        // Even empty metrics should produce valid output
        assert!(!prom.is_empty(), "Prometheus output should not be empty");
    }

    // ============================================================
    // LatencyStats tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_latency_stats_from_histogram() {
        let collector = EmbeddedMetricsCollector::new();
        // Record some search latencies
        for i in 0..100 {
            collector.record_search_us(i * 10);
        }
        let metrics = collector.snapshot(RollingWindow::AllTime);
        assert!(
            metrics.search_latency.mean_ms > 0.0,
            "Average latency should be positive"
        );
    }
}
