//! Query Statistics and Performance Metrics
//!
//! This module tracks query performance metrics and statistics
//! for optimization and monitoring purposes.

use std::time::{Duration, Instant};

/// Query execution statistics
#[derive(Debug, Clone, Default)]
pub struct QueryStatistics {
    /// Number of files read
    pub files_read: usize,

    /// Number of row groups read
    pub row_groups_read: usize,

    /// Number of row groups skipped by statistics
    pub row_groups_skipped: usize,

    /// Number of records read
    pub records_read: usize,

    /// Number of records filtered
    pub records_filtered: usize,

    /// Number of columns projected
    pub columns_projected: usize,

    /// Total query time
    pub total_duration: Duration,

    /// Time spent in I/O
    pub io_duration: Duration,

    /// Time spent in filtering
    pub filter_duration: Duration,

    /// Time spent in projection
    pub projection_duration: Duration,

    /// Cache hits
    pub cache_hits: usize,

    /// Cache misses
    pub cache_misses: usize,

    /// Bytes read from disk
    pub bytes_read: usize,

    /// Bytes read from cache
    pub bytes_from_cache: usize,
}

impl QueryStatistics {
    /// Create new statistics instance
    pub fn new() -> Self {
        Self::default()
    }

    /// Get cache hit rate
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            self.cache_hits as f64 / total as f64
        }
    }

    /// Get row group skip rate
    pub fn row_group_skip_rate(&self) -> f64 {
        let total = self.row_groups_read + self.row_groups_skipped;
        if total == 0 {
            0.0
        } else {
            self.row_groups_skipped as f64 / total as f64
        }
    }

    /// Get filter selectivity
    pub fn filter_selectivity(&self) -> f64 {
        let total = self.records_read + self.records_filtered;
        if total == 0 {
            1.0
        } else {
            self.records_read as f64 / total as f64
        }
    }

    /// Get throughput in records per second
    pub fn throughput_records_per_sec(&self) -> f64 {
        if self.total_duration.as_secs_f64() == 0.0 {
            0.0
        } else {
            self.records_read as f64 / self.total_duration.as_secs_f64()
        }
    }

    /// Get throughput in MB per second
    pub fn throughput_mb_per_sec(&self) -> f64 {
        if self.total_duration.as_secs_f64() == 0.0 {
            0.0
        } else {
            (self.bytes_read as f64 / (1024.0 * 1024.0)) / self.total_duration.as_secs_f64()
        }
    }

    /// Merge statistics from another instance
    pub fn merge(&mut self, other: &QueryStatistics) {
        self.files_read += other.files_read;
        self.row_groups_read += other.row_groups_read;
        self.row_groups_skipped += other.row_groups_skipped;
        self.records_read += other.records_read;
        self.records_filtered += other.records_filtered;
        self.columns_projected += other.columns_projected;
        self.total_duration += other.total_duration;
        self.io_duration += other.io_duration;
        self.filter_duration += other.filter_duration;
        self.projection_duration += other.projection_duration;
        self.cache_hits += other.cache_hits;
        self.cache_misses += other.cache_misses;
        self.bytes_read += other.bytes_read;
        self.bytes_from_cache += other.bytes_from_cache;
    }

    /// Get summary string
    pub fn summary(&self) -> String {
        format!(
            "Query Statistics:\n\
             Files: {}, Row Groups: {} read, {} skipped ({:.1}% skip rate)\n\
             Records: {} read, {} filtered ({:.1}% selectivity)\n\
             Cache: {} hits, {} misses ({:.1}% hit rate)\n\
             Performance: {:.0} records/sec, {:.1} MB/sec\n\
             Time: {:.2}s total (I/O: {:.2}s, Filter: {:.2}s, Projection: {:.2}s)",
            self.files_read,
            self.row_groups_read,
            self.row_groups_skipped,
            self.row_group_skip_rate() * 100.0,
            self.records_read,
            self.records_filtered,
            self.filter_selectivity() * 100.0,
            self.cache_hits,
            self.cache_misses,
            self.cache_hit_rate() * 100.0,
            self.throughput_records_per_sec(),
            self.throughput_mb_per_sec(),
            self.total_duration.as_secs_f64(),
            self.io_duration.as_secs_f64(),
            self.filter_duration.as_secs_f64(),
            self.projection_duration.as_secs_f64(),
        )
    }
}

/// Statistics collector for tracking query performance
pub struct StatisticsCollector {
    stats: QueryStatistics,
    start_time: Option<Instant>,
    io_timer: Option<Instant>,
    filter_timer: Option<Instant>,
    projection_timer: Option<Instant>,
}

impl StatisticsCollector {
    /// Create new collector
    pub fn new() -> Self {
        Self {
            stats: QueryStatistics::new(),
            start_time: None,
            io_timer: None,
            filter_timer: None,
            projection_timer: None,
        }
    }

    /// Start overall timing
    pub fn start(&mut self) {
        self.start_time = Some(Instant::now());
    }

    /// Stop overall timing
    pub fn stop(&mut self) {
        if let Some(start) = self.start_time {
            self.stats.total_duration = start.elapsed();
        }
    }

    /// Start I/O timing
    pub fn start_io(&mut self) {
        self.io_timer = Some(Instant::now());
    }

    /// Stop I/O timing
    pub fn stop_io(&mut self) {
        if let Some(start) = self.io_timer.take() {
            self.stats.io_duration += start.elapsed();
        }
    }

    /// Start filter timing
    pub fn start_filter(&mut self) {
        self.filter_timer = Some(Instant::now());
    }

    /// Stop filter timing
    pub fn stop_filter(&mut self) {
        if let Some(start) = self.filter_timer.take() {
            self.stats.filter_duration += start.elapsed();
        }
    }

    /// Start projection timing
    pub fn start_projection(&mut self) {
        self.projection_timer = Some(Instant::now());
    }

    /// Stop projection timing
    pub fn stop_projection(&mut self) {
        if let Some(start) = self.projection_timer.take() {
            self.stats.projection_duration += start.elapsed();
        }
    }

    /// Record file read
    pub fn record_file_read(&mut self) {
        self.stats.files_read += 1;
    }

    /// Record row group read
    pub fn record_row_group_read(&mut self, count: usize) {
        self.stats.row_groups_read += count;
    }

    /// Record row group skip
    pub fn record_row_group_skip(&mut self, count: usize) {
        self.stats.row_groups_skipped += count;
    }

    /// Record records read
    pub fn record_records_read(&mut self, count: usize) {
        self.stats.records_read += count;
    }

    /// Record records filtered
    pub fn record_records_filtered(&mut self, count: usize) {
        self.stats.records_filtered += count;
    }

    /// Record cache hit
    pub fn record_cache_hit(&mut self, bytes: usize) {
        self.stats.cache_hits += 1;
        self.stats.bytes_from_cache += bytes;
    }

    /// Record cache miss
    pub fn record_cache_miss(&mut self, bytes: usize) {
        self.stats.cache_misses += 1;
        self.stats.bytes_read += bytes;
    }

    /// Get current statistics
    pub fn get_statistics(&self) -> QueryStatistics {
        self.stats.clone()
    }

    /// Reset statistics
    pub fn reset(&mut self) {
        self.stats = QueryStatistics::new();
        self.start_time = None;
        self.io_timer = None;
        self.filter_timer = None;
        self.projection_timer = None;
    }
}

impl Default for StatisticsCollector {
    fn default() -> Self {
        Self::new()
    }
}
