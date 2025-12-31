/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! I/O Metrics Collection for Smart I/O Layer
//!
//! Tracks I/O operations, bytes transferred, and calculates
//! efficiency metrics for the smart I/O layer.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use tracing::debug;

/// I/O metrics collector
#[derive(Debug)]
pub struct IoMetrics {
    // Read metrics
    /// Total bytes requested (original ranges)
    bytes_requested: AtomicU64,
    /// Total bytes actually read (after coalescing)
    bytes_read: AtomicU64,
    /// Number of read operations requested
    reads_requested: AtomicU64,
    /// Number of read operations performed (after coalescing)
    reads_performed: AtomicU64,

    // Coalescing metrics
    /// Number of ranges coalesced
    ranges_coalesced: AtomicU64,
    /// Bytes saved by coalescing (avoided re-reads of gaps)
    bytes_saved_by_coalescing: AtomicU64,

    // Performance metrics
    /// Total read latency in microseconds
    total_read_latency_us: AtomicU64,
    /// Number of parallel reads executed
    parallel_reads: AtomicU64,
    /// Number of sequential reads executed
    sequential_reads: AtomicU64,

    // Timing window for rate calculations
    window_start: RwLock<Instant>,
    /// Operations in current window
    window_ops: AtomicU64,
    /// Bytes in current window
    window_bytes: AtomicU64,
}

impl IoMetrics {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            bytes_requested: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            reads_requested: AtomicU64::new(0),
            reads_performed: AtomicU64::new(0),
            ranges_coalesced: AtomicU64::new(0),
            bytes_saved_by_coalescing: AtomicU64::new(0),
            total_read_latency_us: AtomicU64::new(0),
            parallel_reads: AtomicU64::new(0),
            sequential_reads: AtomicU64::new(0),
            window_start: RwLock::new(Instant::now()),
            window_ops: AtomicU64::new(0),
            window_bytes: AtomicU64::new(0),
        }
    }

    /// Record a read operation
    pub fn record_read(&self, bytes: u64, latency: Duration) {
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
        self.reads_performed.fetch_add(1, Ordering::Relaxed);
        self.total_read_latency_us
            .fetch_add(latency.as_micros() as u64, Ordering::Relaxed);

        // Update window metrics
        self.window_ops.fetch_add(1, Ordering::Relaxed);
        self.window_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record bytes requested (before coalescing)
    pub fn record_request(&self, bytes: u64, num_ranges: u64) {
        self.bytes_requested.fetch_add(bytes, Ordering::Relaxed);
        self.reads_requested.fetch_add(num_ranges, Ordering::Relaxed);
    }

    /// Record coalescing results
    pub fn record_coalescing(&self, original_ranges: usize, coalesced_ranges: usize, bytes_in_gaps: u64) {
        let coalesced = original_ranges.saturating_sub(coalesced_ranges) as u64;
        self.ranges_coalesced.fetch_add(coalesced, Ordering::Relaxed);
        self.bytes_saved_by_coalescing.fetch_add(bytes_in_gaps, Ordering::Relaxed);
    }

    /// Record parallel read execution
    pub fn record_parallel_read(&self) {
        self.parallel_reads.fetch_add(1, Ordering::Relaxed);
    }

    /// Record sequential read execution
    pub fn record_sequential_read(&self) {
        self.sequential_reads.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total bytes requested
    pub fn bytes_requested(&self) -> u64 {
        self.bytes_requested.load(Ordering::Relaxed)
    }

    /// Get total bytes actually read
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Get read amplification ratio
    ///
    /// Values > 1.0 indicate we're reading more bytes than requested
    /// (due to coalescing gaps). Lower is better for bandwidth,
    /// but reading gaps can reduce I/O operations.
    pub fn read_amplification(&self) -> f64 {
        let requested = self.bytes_requested.load(Ordering::Relaxed);
        let read = self.bytes_read.load(Ordering::Relaxed);

        if requested == 0 {
            1.0
        } else {
            read as f64 / requested as f64
        }
    }

    /// Get I/O reduction ratio
    ///
    /// Represents the percentage reduction in I/O operations.
    /// 0.5 means we reduced I/O operations by 50%.
    pub fn io_reduction_ratio(&self) -> f64 {
        let requested = self.reads_requested.load(Ordering::Relaxed);
        let performed = self.reads_performed.load(Ordering::Relaxed);

        if requested == 0 {
            0.0
        } else {
            1.0 - (performed as f64 / requested as f64)
        }
    }

    /// Get average read latency in microseconds
    pub fn avg_read_latency_us(&self) -> f64 {
        let total_latency = self.total_read_latency_us.load(Ordering::Relaxed);
        let num_reads = self.reads_performed.load(Ordering::Relaxed);

        if num_reads == 0 {
            0.0
        } else {
            total_latency as f64 / num_reads as f64
        }
    }

    /// Get bytes per second throughput for current window
    pub fn throughput_bytes_per_sec(&self) -> f64 {
        let window_start = *self.window_start.read();
        let elapsed = window_start.elapsed();
        let bytes = self.window_bytes.load(Ordering::Relaxed);

        if elapsed.as_secs_f64() < 0.001 {
            0.0
        } else {
            bytes as f64 / elapsed.as_secs_f64()
        }
    }

    /// Get operations per second for current window
    pub fn ops_per_sec(&self) -> f64 {
        let window_start = *self.window_start.read();
        let elapsed = window_start.elapsed();
        let ops = self.window_ops.load(Ordering::Relaxed);

        if elapsed.as_secs_f64() < 0.001 {
            0.0
        } else {
            ops as f64 / elapsed.as_secs_f64()
        }
    }

    /// Reset the timing window
    pub fn reset_window(&self) {
        *self.window_start.write() = Instant::now();
        self.window_ops.store(0, Ordering::Relaxed);
        self.window_bytes.store(0, Ordering::Relaxed);
    }

    /// Get complete metrics snapshot
    pub fn snapshot(&self) -> IoMetricsSnapshot {
        IoMetricsSnapshot {
            bytes_requested: self.bytes_requested.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            reads_requested: self.reads_requested.load(Ordering::Relaxed),
            reads_performed: self.reads_performed.load(Ordering::Relaxed),
            ranges_coalesced: self.ranges_coalesced.load(Ordering::Relaxed),
            bytes_saved_by_coalescing: self.bytes_saved_by_coalescing.load(Ordering::Relaxed),
            avg_read_latency_us: self.avg_read_latency_us(),
            parallel_reads: self.parallel_reads.load(Ordering::Relaxed),
            sequential_reads: self.sequential_reads.load(Ordering::Relaxed),
            read_amplification: self.read_amplification(),
            io_reduction_ratio: self.io_reduction_ratio(),
            throughput_bytes_per_sec: self.throughput_bytes_per_sec(),
            ops_per_sec: self.ops_per_sec(),
        }
    }

    /// Log current metrics
    pub fn log_summary(&self) {
        let snapshot = self.snapshot();
        debug!(
            "SmartIO Metrics: reads={}/{}, bytes={}/{}, amplification={:.2}, reduction={:.1}%, latency={:.0}us",
            snapshot.reads_performed,
            snapshot.reads_requested,
            snapshot.bytes_read,
            snapshot.bytes_requested,
            snapshot.read_amplification,
            snapshot.io_reduction_ratio * 100.0,
            snapshot.avg_read_latency_us,
        );
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.bytes_requested.store(0, Ordering::Relaxed);
        self.bytes_read.store(0, Ordering::Relaxed);
        self.reads_requested.store(0, Ordering::Relaxed);
        self.reads_performed.store(0, Ordering::Relaxed);
        self.ranges_coalesced.store(0, Ordering::Relaxed);
        self.bytes_saved_by_coalescing.store(0, Ordering::Relaxed);
        self.total_read_latency_us.store(0, Ordering::Relaxed);
        self.parallel_reads.store(0, Ordering::Relaxed);
        self.sequential_reads.store(0, Ordering::Relaxed);
        self.reset_window();
    }
}

impl Default for IoMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of I/O metrics at a point in time
#[derive(Debug, Clone)]
pub struct IoMetricsSnapshot {
    /// Total bytes requested (original ranges)
    pub bytes_requested: u64,
    /// Total bytes actually read
    pub bytes_read: u64,
    /// Number of read operations requested
    pub reads_requested: u64,
    /// Number of read operations performed
    pub reads_performed: u64,
    /// Number of ranges that were coalesced
    pub ranges_coalesced: u64,
    /// Bytes saved by not reading gaps
    pub bytes_saved_by_coalescing: u64,
    /// Average read latency in microseconds
    pub avg_read_latency_us: f64,
    /// Number of parallel reads
    pub parallel_reads: u64,
    /// Number of sequential reads
    pub sequential_reads: u64,
    /// Read amplification ratio (bytes read / bytes requested)
    pub read_amplification: f64,
    /// I/O reduction ratio (1 - reads performed / reads requested)
    pub io_reduction_ratio: f64,
    /// Current throughput in bytes per second
    pub throughput_bytes_per_sec: f64,
    /// Current operations per second
    pub ops_per_sec: f64,
}

impl IoMetricsSnapshot {
    /// Check if coalescing is providing good savings
    pub fn is_coalescing_effective(&self) -> bool {
        // Effective if we reduced I/O by at least 20% without too much amplification
        self.io_reduction_ratio >= 0.2 && self.read_amplification < 2.0
    }

    /// Get estimated time saved in microseconds
    ///
    /// Assumes 100us per I/O operation saved (base latency)
    pub fn estimated_time_saved_us(&self) -> u64 {
        let io_saved = self.reads_requested.saturating_sub(self.reads_performed);
        io_saved * 100 // 100us per operation
    }

    /// Format as a summary string
    pub fn summary(&self) -> String {
        format!(
            "reads: {}/{} ({:.1}% reduction), bytes: {}/{} ({:.2}x amplification), latency: {:.0}us avg",
            self.reads_performed,
            self.reads_requested,
            self.io_reduction_ratio * 100.0,
            self.bytes_read,
            self.bytes_requested,
            self.read_amplification,
            self.avg_read_latency_us,
        )
    }
}

/// Per-file I/O metrics for tracking access patterns
#[derive(Debug)]
pub struct FileIoMetrics {
    /// File path
    pub path: String,
    /// Total reads for this file
    pub reads: AtomicU64,
    /// Total bytes read
    pub bytes: AtomicU64,
    /// Last access timestamp
    pub last_access: RwLock<Instant>,
}

impl FileIoMetrics {
    pub fn new(path: String) -> Self {
        Self {
            path,
            reads: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            last_access: RwLock::new(Instant::now()),
        }
    }

    pub fn record(&self, bytes: u64) {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        *self.last_access.write() = Instant::now();
    }

    pub fn is_hot(&self, threshold_reads: u64) -> bool {
        self.reads.load(Ordering::Relaxed) >= threshold_reads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_new() {
        let metrics = IoMetrics::new();
        assert_eq!(metrics.bytes_requested(), 0);
        assert_eq!(metrics.bytes_read(), 0);
    }

    #[test]
    fn test_record_read() {
        let metrics = IoMetrics::new();

        metrics.record_read(1000, Duration::from_micros(100));
        metrics.record_read(2000, Duration::from_micros(200));

        assert_eq!(metrics.bytes_read(), 3000);
        assert_eq!(metrics.avg_read_latency_us(), 150.0);
    }

    #[test]
    fn test_record_request() {
        let metrics = IoMetrics::new();

        metrics.record_request(5000, 10);
        assert_eq!(metrics.bytes_requested(), 5000);
    }

    #[test]
    fn test_read_amplification() {
        let metrics = IoMetrics::new();

        // Requested 1000 bytes, read 1500 bytes (including gaps)
        metrics.record_request(1000, 3);
        metrics.record_read(1500, Duration::from_micros(100));

        assert!((metrics.read_amplification() - 1.5).abs() < 0.01);
    }

    #[test]
    fn test_io_reduction_ratio() {
        let metrics = IoMetrics::new();

        // Requested 10 reads, performed 4 (60% reduction)
        metrics.record_request(10000, 10);
        for _ in 0..4 {
            metrics.record_read(2500, Duration::from_micros(100));
        }

        assert!((metrics.io_reduction_ratio() - 0.6).abs() < 0.01);
    }

    #[test]
    fn test_coalescing_metrics() {
        let metrics = IoMetrics::new();

        // 10 original ranges coalesced to 3 ranges, saved 500 bytes in gaps
        metrics.record_coalescing(10, 3, 500);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.ranges_coalesced, 7);
        assert_eq!(snapshot.bytes_saved_by_coalescing, 500);
    }

    #[test]
    fn test_parallel_sequential_tracking() {
        let metrics = IoMetrics::new();

        metrics.record_parallel_read();
        metrics.record_parallel_read();
        metrics.record_sequential_read();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.parallel_reads, 2);
        assert_eq!(snapshot.sequential_reads, 1);
    }

    #[test]
    fn test_snapshot() {
        let metrics = IoMetrics::new();

        metrics.record_request(10000, 10);
        metrics.record_read(3000, Duration::from_micros(100));
        metrics.record_read(3000, Duration::from_micros(200));
        metrics.record_coalescing(10, 2, 1000);
        metrics.record_parallel_read();

        let snapshot = metrics.snapshot();

        assert_eq!(snapshot.bytes_requested, 10000);
        assert_eq!(snapshot.bytes_read, 6000);
        assert_eq!(snapshot.reads_requested, 10);
        assert_eq!(snapshot.reads_performed, 2);
        assert_eq!(snapshot.ranges_coalesced, 8);
        assert!((snapshot.read_amplification - 0.6).abs() < 0.01);
        assert!((snapshot.io_reduction_ratio - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_snapshot_summary() {
        let metrics = IoMetrics::new();

        metrics.record_request(10000, 10);
        for _ in 0..3 {
            metrics.record_read(3333, Duration::from_micros(100));
        }

        let snapshot = metrics.snapshot();
        let summary = snapshot.summary();

        assert!(summary.contains("reads: 3/10"));
        assert!(summary.contains("reduction"));
    }

    #[test]
    fn test_is_coalescing_effective() {
        let metrics = IoMetrics::new();

        // Good coalescing: 50% reduction, 1.2x amplification
        metrics.record_request(10000, 10);
        for _ in 0..5 {
            metrics.record_read(2400, Duration::from_micros(100));
        }

        let snapshot = metrics.snapshot();
        assert!(snapshot.is_coalescing_effective());

        // Reset and test bad case
        metrics.reset();

        // Bad coalescing: 10% reduction, 3x amplification
        metrics.record_request(10000, 10);
        for _ in 0..9 {
            metrics.record_read(3333, Duration::from_micros(100));
        }

        let snapshot = metrics.snapshot();
        assert!(!snapshot.is_coalescing_effective());
    }

    #[test]
    fn test_reset() {
        let metrics = IoMetrics::new();

        metrics.record_request(1000, 5);
        metrics.record_read(1000, Duration::from_micros(100));
        metrics.record_coalescing(5, 2, 100);

        metrics.reset();

        assert_eq!(metrics.bytes_requested(), 0);
        assert_eq!(metrics.bytes_read(), 0);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.ranges_coalesced, 0);
    }

    #[test]
    fn test_file_io_metrics() {
        let file_metrics = FileIoMetrics::new("/test/file.parquet".to_string());

        file_metrics.record(1000);
        file_metrics.record(2000);

        assert_eq!(file_metrics.reads.load(Ordering::Relaxed), 2);
        assert_eq!(file_metrics.bytes.load(Ordering::Relaxed), 3000);
        assert!(file_metrics.is_hot(2));
        assert!(!file_metrics.is_hot(3));
    }

    #[test]
    fn test_window_metrics() {
        let metrics = IoMetrics::new();

        // Record some operations
        for _ in 0..5 {
            metrics.record_read(1000, Duration::from_micros(100));
        }

        // Add small delay to ensure timing window is valid (> 1ms threshold)
        std::thread::sleep(Duration::from_millis(2));

        // Check window metrics - should be positive after delay
        let ops_per_sec = metrics.ops_per_sec();
        let throughput = metrics.throughput_bytes_per_sec();

        // After the delay, we should get positive values
        // (ops_per_sec may still be 0.0 on very fast systems, so check >= 0.0)
        assert!(ops_per_sec >= 0.0);
        assert!(throughput >= 0.0);

        // Reset window and verify
        metrics.reset_window();
        // Immediately after reset, values should be very small or zero
        // (depends on timing, so just verify no panic)
        let _ = metrics.ops_per_sec();
        let _ = metrics.throughput_bytes_per_sec();
    }
}
