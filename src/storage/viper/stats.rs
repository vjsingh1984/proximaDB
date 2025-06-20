//! VIPER Performance Statistics and Monitoring
//!
//! This module provides comprehensive statistics collection and monitoring
//! for VIPER operations, enabling performance analysis and optimization.

use std::collections::HashMap;
use std::time::Instant;

/// VIPER performance statistics
#[derive(Debug, Clone)]
pub struct ViperStats {
    pub total_entries_clustered: u64,
    pub clusters_created: usize,
    pub clustering_time_ms: u64,
    pub parquet_write_time_ms: u64,
    pub compression_ratio: f64,
    pub avg_vectors_per_cluster: f64,
}

impl Default for ViperStats {
    fn default() -> Self {
        Self {
            total_entries_clustered: 0,
            clusters_created: 0,
            clustering_time_ms: 0,
            parquet_write_time_ms: 0,
            compression_ratio: 1.0,
            avg_vectors_per_cluster: 0.0,
        }
    }
}

/// Detailed performance metrics for VIPER operations
#[derive(Debug, Clone)]
pub struct ViperPerformanceMetrics {
    pub operation_type: String,
    pub collection_id: String,
    pub records_processed: u64,
    pub schema_generation_time_ms: u64,
    pub preprocessing_time_ms: u64,
    pub conversion_time_ms: u64,
    pub postprocessing_time_ms: u64,
    pub parquet_write_time_ms: u64,
    pub total_time_ms: u64,
    pub bytes_written: u64,
    pub compression_ratio: f64,
    pub records_per_second: f64,
    pub bytes_per_second: f64,
}

/// Statistics collector for VIPER operations
#[derive(Debug)]
pub struct ViperStatsCollector {
    operation_start: Option<Instant>,
    phase_timers: HashMap<String, Instant>,
    metrics: ViperPerformanceMetrics,
}

impl ViperStatsCollector {
    pub fn new(operation_type: String, collection_id: String) -> Self {
        Self {
            operation_start: None,
            phase_timers: HashMap::new(),
            metrics: ViperPerformanceMetrics {
                operation_type,
                collection_id,
                records_processed: 0,
                schema_generation_time_ms: 0,
                preprocessing_time_ms: 0,
                conversion_time_ms: 0,
                postprocessing_time_ms: 0,
                parquet_write_time_ms: 0,
                total_time_ms: 0,
                bytes_written: 0,
                compression_ratio: 1.0,
                records_per_second: 0.0,
                bytes_per_second: 0.0,
            },
        }
    }

    /// Start timing the overall operation
    pub fn start_operation(&mut self) {
        self.operation_start = Some(Instant::now());
    }

    /// Start timing a specific phase
    pub fn start_phase(&mut self, phase_name: &str) {
        self.phase_timers
            .insert(phase_name.to_string(), Instant::now());
    }

    /// End timing a specific phase and record the duration
    pub fn end_phase(&mut self, phase_name: &str) {
        if let Some(start_time) = self.phase_timers.remove(phase_name) {
            let duration_ms = start_time.elapsed().as_millis() as u64;

            match phase_name {
                "schema_generation" => self.metrics.schema_generation_time_ms = duration_ms,
                "preprocessing" => self.metrics.preprocessing_time_ms = duration_ms,
                "conversion" => self.metrics.conversion_time_ms = duration_ms,
                "postprocessing" => self.metrics.postprocessing_time_ms = duration_ms,
                "parquet_write" => self.metrics.parquet_write_time_ms = duration_ms,
                _ => {} // Unknown phase
            }
        }
    }

    /// Record the number of records processed
    pub fn set_records_processed(&mut self, count: u64) {
        self.metrics.records_processed = count;
    }

    /// Record the number of bytes written
    pub fn set_bytes_written(&mut self, bytes: u64) {
        self.metrics.bytes_written = bytes;
    }

    /// Record the compression ratio
    pub fn set_compression_ratio(&mut self, ratio: f64) {
        self.metrics.compression_ratio = ratio;
    }

    /// Finalize the operation and calculate derived metrics
    pub fn finalize_operation(mut self) -> ViperPerformanceMetrics {
        if let Some(start_time) = self.operation_start {
            self.metrics.total_time_ms = start_time.elapsed().as_millis() as u64;

            // Calculate derived metrics
            if self.metrics.total_time_ms > 0 {
                self.metrics.records_per_second = (self.metrics.records_processed as f64 * 1000.0)
                    / self.metrics.total_time_ms as f64;
                self.metrics.bytes_per_second = (self.metrics.bytes_written as f64 * 1000.0)
                    / self.metrics.total_time_ms as f64;
            }
        }

        self.metrics
    }
}

/// Aggregated statistics across multiple operations
#[derive(Debug, Clone)]
pub struct ViperAggregateStats {
    pub total_operations: u64,
    pub total_records_processed: u64,
    pub total_bytes_written: u64,
    pub avg_compression_ratio: f64,
    pub avg_records_per_second: f64,
    pub avg_bytes_per_second: f64,
    pub avg_total_time_ms: f64,
    pub operations_by_type: HashMap<String, u64>,
    pub performance_by_collection: HashMap<String, CollectionPerformanceStats>,
}

#[derive(Debug, Clone)]
pub struct CollectionPerformanceStats {
    pub operations_count: u64,
    pub avg_records_per_operation: f64,
    pub avg_compression_ratio: f64,
    pub avg_throughput_records_per_sec: f64,
}

impl Default for ViperAggregateStats {
    fn default() -> Self {
        Self {
            total_operations: 0,
            total_records_processed: 0,
            total_bytes_written: 0,
            avg_compression_ratio: 1.0,
            avg_records_per_second: 0.0,
            avg_bytes_per_second: 0.0,
            avg_total_time_ms: 0.0,
            operations_by_type: HashMap::new(),
            performance_by_collection: HashMap::new(),
        }
    }
}

/// Statistics aggregator for collecting metrics across operations
#[derive(Debug)]
pub struct ViperStatsAggregator {
    aggregate_stats: ViperAggregateStats,
    recent_metrics: Vec<ViperPerformanceMetrics>,
    max_recent_metrics: usize,
}

impl ViperStatsAggregator {
    pub fn new(max_recent_metrics: usize) -> Self {
        Self {
            aggregate_stats: ViperAggregateStats::default(),
            recent_metrics: Vec::new(),
            max_recent_metrics,
        }
    }

    /// Add metrics from a completed operation
    pub fn add_metrics(&mut self, metrics: ViperPerformanceMetrics) {
        // Update aggregate stats
        self.aggregate_stats.total_operations += 1;
        self.aggregate_stats.total_records_processed += metrics.records_processed;
        self.aggregate_stats.total_bytes_written += metrics.bytes_written;

        // Update operation type counts
        *self
            .aggregate_stats
            .operations_by_type
            .entry(metrics.operation_type.clone())
            .or_insert(0) += 1;

        // Update collection-specific stats
        let collection_stats = self
            .aggregate_stats
            .performance_by_collection
            .entry(metrics.collection_id.clone())
            .or_insert(CollectionPerformanceStats {
                operations_count: 0,
                avg_records_per_operation: 0.0,
                avg_compression_ratio: 0.0,
                avg_throughput_records_per_sec: 0.0,
            });

        collection_stats.operations_count += 1;
        collection_stats.avg_records_per_operation = (collection_stats.avg_records_per_operation
            * (collection_stats.operations_count - 1) as f64
            + metrics.records_processed as f64)
            / collection_stats.operations_count as f64;
        collection_stats.avg_compression_ratio = (collection_stats.avg_compression_ratio
            * (collection_stats.operations_count - 1) as f64
            + metrics.compression_ratio)
            / collection_stats.operations_count as f64;
        collection_stats.avg_throughput_records_per_sec = (collection_stats
            .avg_throughput_records_per_sec
            * (collection_stats.operations_count - 1) as f64
            + metrics.records_per_second)
            / collection_stats.operations_count as f64;

        // Recalculate averages
        self.recalculate_averages();

        // Store recent metrics (with rotation)
        self.recent_metrics.push(metrics);
        if self.recent_metrics.len() > self.max_recent_metrics {
            self.recent_metrics.remove(0);
        }
    }

    /// Get current aggregate statistics
    pub fn get_aggregate_stats(&self) -> &ViperAggregateStats {
        &self.aggregate_stats
    }

    /// Get recent performance metrics
    pub fn get_recent_metrics(&self) -> &[ViperPerformanceMetrics] {
        &self.recent_metrics
    }

    /// Get performance trend analysis
    pub fn get_performance_trend(&self) -> Option<PerformanceTrend> {
        if self.recent_metrics.len() < 2 {
            return None;
        }

        let recent_half = &self.recent_metrics[self.recent_metrics.len() / 2..];
        let earlier_half = &self.recent_metrics[..self.recent_metrics.len() / 2];

        let recent_avg_throughput: f64 = recent_half
            .iter()
            .map(|m| m.records_per_second)
            .sum::<f64>()
            / recent_half.len() as f64;

        let earlier_avg_throughput: f64 = earlier_half
            .iter()
            .map(|m| m.records_per_second)
            .sum::<f64>()
            / earlier_half.len() as f64;

        let throughput_change = recent_avg_throughput - earlier_avg_throughput;
        let throughput_change_percent = if earlier_avg_throughput > 0.0 {
            (throughput_change / earlier_avg_throughput) * 100.0
        } else {
            0.0
        };

        Some(PerformanceTrend {
            throughput_change_percent,
            is_improving: throughput_change > 0.0,
            confidence: if self.recent_metrics.len() >= 10 {
                ConfidenceLevel::High
            } else {
                ConfidenceLevel::Low
            },
        })
    }

    fn recalculate_averages(&mut self) {
        if self.aggregate_stats.total_operations > 0 {
            self.aggregate_stats.avg_compression_ratio = self
                .aggregate_stats
                .performance_by_collection
                .values()
                .map(|stats| stats.avg_compression_ratio)
                .sum::<f64>()
                / self.aggregate_stats.performance_by_collection.len() as f64;

            self.aggregate_stats.avg_records_per_second = self
                .aggregate_stats
                .performance_by_collection
                .values()
                .map(|stats| stats.avg_throughput_records_per_sec)
                .sum::<f64>()
                / self.aggregate_stats.performance_by_collection.len() as f64;
        }
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceTrend {
    pub throughput_change_percent: f64,
    pub is_improving: bool,
    pub confidence: ConfidenceLevel,
}

#[derive(Debug, Clone)]
pub enum ConfidenceLevel {
    Low,
    Medium,
    High,
}
