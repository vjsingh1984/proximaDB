//! Query execution statistics and performance metrics.

use std::time::{Duration, Instant};

/// Query execution statistics
#[derive(Debug, Clone, Default)]
pub struct QueryStatistics {
    pub files_read: usize,
    pub row_groups_read: usize,
    pub row_groups_skipped: usize,
    pub records_read: usize,
    pub records_filtered: usize,
    pub columns_projected: usize,
    pub total_duration: Duration,
    pub io_duration: Duration,
    pub filter_duration: Duration,
    pub projection_duration: Duration,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub bytes_read: usize,
    pub bytes_from_cache: usize,
}

impl QueryStatistics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            self.cache_hits as f64 / total as f64
        }
    }

    pub fn row_group_skip_rate(&self) -> f64 {
        let total = self.row_groups_read + self.row_groups_skipped;
        if total == 0 {
            0.0
        } else {
            self.row_groups_skipped as f64 / total as f64
        }
    }

    pub fn filter_selectivity(&self) -> f64 {
        let total = self.records_read + self.records_filtered;
        if total == 0 {
            1.0
        } else {
            self.records_read as f64 / total as f64
        }
    }

    pub fn throughput_records_per_sec(&self) -> f64 {
        if self.total_duration.as_secs_f64() == 0.0 {
            0.0
        } else {
            self.records_read as f64 / self.total_duration.as_secs_f64()
        }
    }

    pub fn throughput_mb_per_sec(&self) -> f64 {
        if self.total_duration.as_secs_f64() == 0.0 {
            0.0
        } else {
            (self.bytes_read as f64 / (1024.0 * 1024.0)) / self.total_duration.as_secs_f64()
        }
    }

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
    pub fn new() -> Self {
        Self {
            stats: QueryStatistics::new(),
            start_time: None,
            io_timer: None,
            filter_timer: None,
            projection_timer: None,
        }
    }

    pub fn start(&mut self) {
        self.start_time = Some(Instant::now());
    }

    pub fn stop(&mut self) {
        if let Some(start) = self.start_time {
            self.stats.total_duration = start.elapsed();
        }
    }

    pub fn start_io(&mut self) {
        self.io_timer = Some(Instant::now());
    }

    pub fn stop_io(&mut self) {
        if let Some(start) = self.io_timer.take() {
            self.stats.io_duration += start.elapsed();
        }
    }

    pub fn start_filter(&mut self) {
        self.filter_timer = Some(Instant::now());
    }

    pub fn stop_filter(&mut self) {
        if let Some(start) = self.filter_timer.take() {
            self.stats.filter_duration += start.elapsed();
        }
    }

    pub fn start_projection(&mut self) {
        self.projection_timer = Some(Instant::now());
    }

    pub fn stop_projection(&mut self) {
        if let Some(start) = self.projection_timer.take() {
            self.stats.projection_duration += start.elapsed();
        }
    }

    pub fn record_file_read(&mut self) {
        self.stats.files_read += 1;
    }

    pub fn record_row_group_read(&mut self, count: usize) {
        self.stats.row_groups_read += count;
    }

    pub fn record_row_group_skip(&mut self, count: usize) {
        self.stats.row_groups_skipped += count;
    }

    pub fn record_records_read(&mut self, count: usize) {
        self.stats.records_read += count;
    }

    pub fn record_records_filtered(&mut self, count: usize) {
        self.stats.records_filtered += count;
    }

    pub fn record_cache_hit(&mut self, bytes: usize) {
        self.stats.cache_hits += 1;
        self.stats.bytes_from_cache += bytes;
    }

    pub fn record_cache_miss(&mut self, bytes: usize) {
        self.stats.cache_misses += 1;
        self.stats.bytes_read += bytes;
    }

    pub fn get_statistics(&self) -> QueryStatistics {
        self.stats.clone()
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_statistics_use_safe_zero_or_identity_rates() {
        let stats = QueryStatistics::new();

        assert_eq!(stats.cache_hit_rate(), 0.0);
        assert_eq!(stats.row_group_skip_rate(), 0.0);
        assert_eq!(stats.filter_selectivity(), 1.0);
        assert_eq!(stats.throughput_records_per_sec(), 0.0);
        assert_eq!(stats.throughput_mb_per_sec(), 0.0);
    }

    #[test]
    fn statistics_compute_rates_and_throughput() {
        let stats = QueryStatistics {
            row_groups_read: 3,
            row_groups_skipped: 7,
            records_read: 250,
            records_filtered: 750,
            total_duration: Duration::from_secs(2),
            cache_hits: 9,
            cache_misses: 1,
            bytes_read: 2 * 1024 * 1024,
            ..QueryStatistics::default()
        };

        assert_eq!(stats.cache_hit_rate(), 0.9);
        assert_eq!(stats.row_group_skip_rate(), 0.7);
        assert_eq!(stats.filter_selectivity(), 0.25);
        assert_eq!(stats.throughput_records_per_sec(), 125.0);
        assert_eq!(stats.throughput_mb_per_sec(), 1.0);
    }

    #[test]
    fn statistics_merge_accumulates_all_counters_and_durations() {
        let mut left = QueryStatistics {
            files_read: 1,
            row_groups_read: 2,
            row_groups_skipped: 3,
            records_read: 4,
            records_filtered: 5,
            columns_projected: 6,
            total_duration: Duration::from_secs(7),
            io_duration: Duration::from_secs(8),
            filter_duration: Duration::from_secs(9),
            projection_duration: Duration::from_secs(10),
            cache_hits: 11,
            cache_misses: 12,
            bytes_read: 13,
            bytes_from_cache: 14,
        };
        let right = QueryStatistics {
            files_read: 10,
            row_groups_read: 20,
            row_groups_skipped: 30,
            records_read: 40,
            records_filtered: 50,
            columns_projected: 60,
            total_duration: Duration::from_secs(70),
            io_duration: Duration::from_secs(80),
            filter_duration: Duration::from_secs(90),
            projection_duration: Duration::from_secs(100),
            cache_hits: 110,
            cache_misses: 120,
            bytes_read: 130,
            bytes_from_cache: 140,
        };

        left.merge(&right);

        assert_eq!(left.files_read, 11);
        assert_eq!(left.row_groups_read, 22);
        assert_eq!(left.row_groups_skipped, 33);
        assert_eq!(left.records_read, 44);
        assert_eq!(left.records_filtered, 55);
        assert_eq!(left.columns_projected, 66);
        assert_eq!(left.total_duration, Duration::from_secs(77));
        assert_eq!(left.io_duration, Duration::from_secs(88));
        assert_eq!(left.filter_duration, Duration::from_secs(99));
        assert_eq!(left.projection_duration, Duration::from_secs(110));
        assert_eq!(left.cache_hits, 121);
        assert_eq!(left.cache_misses, 132);
        assert_eq!(left.bytes_read, 143);
        assert_eq!(left.bytes_from_cache, 154);
    }

    #[test]
    fn statistics_collector_records_counters_bytes_and_reset() {
        let mut collector = StatisticsCollector::new();

        collector.record_file_read();
        collector.record_row_group_read(2);
        collector.record_row_group_skip(3);
        collector.record_records_read(5);
        collector.record_records_filtered(7);
        collector.record_cache_hit(11);
        collector.record_cache_miss(13);

        let stats = collector.get_statistics();
        assert_eq!(stats.files_read, 1);
        assert_eq!(stats.row_groups_read, 2);
        assert_eq!(stats.row_groups_skipped, 3);
        assert_eq!(stats.records_read, 5);
        assert_eq!(stats.records_filtered, 7);
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.cache_misses, 1);
        assert_eq!(stats.bytes_from_cache, 11);
        assert_eq!(stats.bytes_read, 13);

        collector.reset();
        assert_eq!(collector.get_statistics().files_read, 0);
    }

    #[test]
    fn statistics_summary_includes_core_workload_and_cache_fields() {
        let stats = QueryStatistics {
            files_read: 2,
            row_groups_read: 4,
            row_groups_skipped: 6,
            records_read: 100,
            records_filtered: 100,
            total_duration: Duration::from_secs(1),
            cache_hits: 3,
            cache_misses: 1,
            bytes_read: 1024 * 1024,
            ..QueryStatistics::default()
        };

        let summary = stats.summary();

        assert!(summary.contains("Files: 2"));
        assert!(summary.contains("4 read, 6 skipped"));
        assert!(summary.contains("100 read, 100 filtered"));
        assert!(summary.contains("3 hits, 1 misses"));
        assert!(summary.contains("100 records/sec"));
    }

    #[test]
    fn statistics_collector_timer_lifecycle_is_optional_and_accumulative() {
        let mut collector = StatisticsCollector::new();

        collector.stop();
        collector.stop_io();
        collector.stop_filter();
        collector.stop_projection();

        collector.start();
        collector.start_io();
        collector.stop_io();
        collector.start_filter();
        collector.stop_filter();
        collector.start_projection();
        collector.stop_projection();
        collector.stop();

        let stats = collector.get_statistics();
        assert!(stats.total_duration <= Duration::from_secs(1));
        assert!(stats.io_duration <= Duration::from_secs(1));
        assert!(stats.filter_duration <= Duration::from_secs(1));
        assert!(stats.projection_duration <= Duration::from_secs(1));
    }
}
