// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Workload Analysis and Pattern Detection

use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, info};

/// Workload pattern types
#[derive(Debug, Clone, PartialEq)]
pub enum WorkloadPattern {
    /// Primarily read operations with occasional writes
    ReadHeavy,
    /// Primarily write operations with occasional reads
    WriteHeavy,
    /// Balanced read and write operations
    Balanced,
    /// Batch processing with periodic bursts
    BatchProcessing,
    /// Real-time streaming with consistent load
    Streaming,
    /// Analytics workload with complex queries
    Analytics,
    /// Mixed workload with no clear pattern
    Mixed,
}

/// Time series data point
#[derive(Debug, Clone)]
pub struct TimeSeriesPoint {
    timestamp: Instant,
    value: f64,
}

/// Workload time series
#[derive(Debug, Clone)]
pub struct WorkloadTimeSeries {
    /// Read operations per second
    reads_per_sec: VecDeque<TimeSeriesPoint>,
    /// Write operations per second
    writes_per_sec: VecDeque<TimeSeriesPoint>,
    /// Query latency in milliseconds
    query_latency_ms: VecDeque<TimeSeriesPoint>,
    /// Memory usage in MB
    memory_usage_mb: VecDeque<TimeSeriesPoint>,
    /// CPU usage percentage
    cpu_usage_percent: VecDeque<TimeSeriesPoint>,
}

impl WorkloadTimeSeries {
    fn new() -> Self {
        Self {
            reads_per_sec: VecDeque::new(),
            writes_per_sec: VecDeque::new(),
            query_latency_ms: VecDeque::new(),
            memory_usage_mb: VecDeque::new(),
            cpu_usage_percent: VecDeque::new(),
        }
    }

    /// Add a data point to the appropriate time series
    fn add_point(&mut self, metric: &str, value: f64, max_points: usize) {
        let point = TimeSeriesPoint {
            timestamp: Instant::now(),
            value,
        };

        let series = match metric {
            "reads_per_sec" => &mut self.reads_per_sec,
            "writes_per_sec" => &mut self.writes_per_sec,
            "query_latency_ms" => &mut self.query_latency_ms,
            "memory_usage_mb" => &mut self.memory_usage_mb,
            "cpu_usage_percent" => &mut self.cpu_usage_percent,
            _ => return,
        };

        series.push_back(point);

        // Keep only recent data points
        while series.len() > max_points {
            series.pop_front();
        }
    }

    /// Calculate the trend (positive = increasing, negative = decreasing)
    fn calculate_trend(series: &VecDeque<TimeSeriesPoint>) -> f64 {
        if series.len() < 2 {
            return 0.0;
        }

        // Simple linear regression
        let n = series.len() as f64;
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_xy = 0.0;
        let mut sum_x2 = 0.0;

        for (i, point) in series.iter().enumerate() {
            let x = i as f64;
            let y = point.value;
            sum_x += x;
            sum_y += y;
            sum_xy += x * y;
            sum_x2 += x * x;
        }

        let slope = (n * sum_xy - sum_x * sum_y) / (n * sum_x2 - sum_x * sum_x);
        slope
    }

    /// Calculate the variance of a time series
    fn calculate_variance(series: &VecDeque<TimeSeriesPoint>) -> f64 {
        if series.is_empty() {
            return 0.0;
        }

        let mean = series.iter().map(|p| p.value).sum::<f64>() / series.len() as f64;
        let variance =
            series.iter().map(|p| (p.value - mean).powi(2)).sum::<f64>() / series.len() as f64;

        variance
    }
}

/// Workload statistics
#[derive(Debug, Clone)]
pub struct WorkloadStatistics {
    pub avg_reads_per_sec: f64,
    pub avg_writes_per_sec: f64,
    pub read_write_ratio: f64,
    pub query_latency_p50: f64,
    pub query_latency_p99: f64,
    pub burstiness_score: f64,
    pub pattern_stability: f64,
}

/// Workload analyzer for pattern detection
pub struct WorkloadAnalyzer {
    /// Time series data by collection
    time_series: Arc<RwLock<HashMap<String, WorkloadTimeSeries>>>,
    /// Detected patterns by collection
    patterns: Arc<RwLock<HashMap<String, WorkloadPattern>>>,
    /// Pattern detection configuration
    config: WorkloadAnalyzerConfig,
    /// Monitoring handle
    monitor_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
    /// Shutdown channel
    shutdown_tx: Arc<RwLock<Option<tokio::sync::mpsc::Sender<()>>>>,
}

/// Workload analyzer configuration
#[derive(Debug, Clone)]
pub struct WorkloadAnalyzerConfig {
    /// Maximum number of data points to keep
    pub max_data_points: usize,
    /// Analysis interval in seconds
    pub analysis_interval_secs: u64,
    /// Minimum data points for pattern detection
    pub min_data_points: usize,
    /// Pattern stability threshold (0.0 - 1.0)
    pub stability_threshold: f64,
}

impl Default for WorkloadAnalyzerConfig {
    fn default() -> Self {
        Self {
            max_data_points: 1000,
            analysis_interval_secs: 60, // Analyze every minute
            min_data_points: 10,
            stability_threshold: 0.7,
        }
    }
}

impl WorkloadAnalyzer {
    /// Create a new workload analyzer
    pub async fn new() -> Result<Self> {
        Self::with_config(WorkloadAnalyzerConfig::default()).await
    }

    /// Create a new workload analyzer with custom configuration
    pub async fn with_config(config: WorkloadAnalyzerConfig) -> Result<Self> {
        Ok(Self {
            time_series: Arc::new(RwLock::new(HashMap::new())),
            patterns: Arc::new(RwLock::new(HashMap::new())),
            config,
            monitor_handle: Arc::new(RwLock::new(None)),
            shutdown_tx: Arc::new(RwLock::new(None)),
        })
    }

    /// Start monitoring workload
    pub async fn start_monitoring(&self) -> Result<()> {
        info!("Starting workload monitoring");

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

        let analyzer = self.clone();
        let handle = tokio::spawn(async move {
            let mut analysis_interval =
                interval(Duration::from_secs(analyzer.config.analysis_interval_secs));

            loop {
                tokio::select! {
                    _ = analysis_interval.tick() => {
                        if let Err(e) = analyzer.analyze_all_workloads().await {
                            tracing::warn!("Workload analysis failed: {}", e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Workload monitoring shutting down");
                        break;
                    }
                }
            }
        });

        // Store shutdown channel and handle
        *self.shutdown_tx.write().await = Some(shutdown_tx);
        *self.monitor_handle.write().await = Some(handle);

        Ok(())
    }

    /// Stop monitoring workload
    pub async fn stop_monitoring(&self) -> Result<()> {
        info!("Stopping workload monitoring");

        // Send shutdown signal
        if let Some(tx) = &*self.shutdown_tx.read().await {
            let _ = tx.send(()).await;
        }

        // Wait for monitoring task to complete with timeout
        if let Some(handle) = self.monitor_handle.write().await.take() {
            match tokio::time::timeout(Duration::from_secs(30), handle).await {
                Ok(Ok(())) => {
                    info!("Workload monitoring stopped successfully");
                }
                Ok(Err(e)) => {
                    tracing::warn!("Workload monitoring task panicked: {:?}", e);
                }
                Err(_) => {
                    tracing::warn!("Workload monitoring shutdown timed out after 30s");
                }
            }
        }

        // Clear shutdown channel
        *self.shutdown_tx.write().await = None;

        Ok(())
    }

    /// Record a workload metric
    pub async fn record_metric(&self, collection_id: &str, metric: &str, value: f64) -> Result<()> {
        let mut time_series = self.time_series.write().await;
        let series = time_series
            .entry(collection_id.to_string())
            .or_insert_with(WorkloadTimeSeries::new);

        series.add_point(metric, value, self.config.max_data_points);

        Ok(())
    }

    /// Analyze workload for a specific collection
    pub async fn analyze_workload(&self, collection_id: &str) -> Result<WorkloadPattern> {
        let time_series = self.time_series.read().await;
        let series = time_series
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No time series data for collection"))?;

        // Calculate statistics
        let stats = self.calculate_statistics(series)?;

        // Detect pattern
        let pattern = self.detect_pattern(&stats, series)?;

        // Update pattern cache
        {
            let mut patterns = self.patterns.write().await;
            patterns.insert(collection_id.to_string(), pattern.clone());
        }

        Ok(pattern)
    }

    /// Analyze all workloads
    async fn analyze_all_workloads(&self) -> Result<()> {
        let collection_ids: Vec<String> = {
            let time_series = self.time_series.read().await;
            time_series.keys().cloned().collect()
        };

        for collection_id in collection_ids {
            if let Err(e) = self.analyze_workload(&collection_id).await {
                debug!("Failed to analyze workload for {}: {}", collection_id, e);
            }
        }

        Ok(())
    }

    /// Calculate workload statistics
    fn calculate_statistics(&self, series: &WorkloadTimeSeries) -> Result<WorkloadStatistics> {
        // Calculate average reads and writes
        let avg_reads = if !series.reads_per_sec.is_empty() {
            series.reads_per_sec.iter().map(|p| p.value).sum::<f64>()
                / series.reads_per_sec.len() as f64
        } else {
            0.0
        };

        let avg_writes = if !series.writes_per_sec.is_empty() {
            series.writes_per_sec.iter().map(|p| p.value).sum::<f64>()
                / series.writes_per_sec.len() as f64
        } else {
            0.0
        };

        // Calculate read/write ratio
        let read_write_ratio = if avg_writes > 0.0 {
            avg_reads / avg_writes
        } else {
            f64::INFINITY
        };

        // Calculate latency percentiles
        let mut latencies: Vec<f64> = series.query_latency_ms.iter().map(|p| p.value).collect();
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let p50 = if !latencies.is_empty() {
            latencies[latencies.len() / 2]
        } else {
            0.0
        };

        let p99 = if !latencies.is_empty() {
            latencies[((latencies.len() as f64) * 0.99) as usize]
        } else {
            0.0
        };

        // Calculate burstiness (variance in throughput)
        let read_variance = WorkloadTimeSeries::calculate_variance(&series.reads_per_sec);
        let write_variance = WorkloadTimeSeries::calculate_variance(&series.writes_per_sec);
        let burstiness = (read_variance + write_variance).sqrt();

        // Calculate pattern stability (inverse of trend magnitude)
        let read_trend = WorkloadTimeSeries::calculate_trend(&series.reads_per_sec).abs();
        let write_trend = WorkloadTimeSeries::calculate_trend(&series.writes_per_sec).abs();
        let stability = 1.0 / (1.0 + read_trend + write_trend);

        Ok(WorkloadStatistics {
            avg_reads_per_sec: avg_reads,
            avg_writes_per_sec: avg_writes,
            read_write_ratio,
            query_latency_p50: p50,
            query_latency_p99: p99,
            burstiness_score: burstiness,
            pattern_stability: stability,
        })
    }

    /// Detect workload pattern based on statistics
    fn detect_pattern(
        &self,
        stats: &WorkloadStatistics,
        series: &WorkloadTimeSeries,
    ) -> Result<WorkloadPattern> {
        // Check if we have enough data
        if series.reads_per_sec.len() < self.config.min_data_points {
            return Ok(WorkloadPattern::Mixed);
        }

        // Detect pattern based on characteristics
        let pattern = if stats.read_write_ratio > 10.0 {
            WorkloadPattern::ReadHeavy
        } else if stats.read_write_ratio < 0.1 {
            WorkloadPattern::WriteHeavy
        } else if (0.5..=2.0).contains(&stats.read_write_ratio) {
            // Check for balanced workload before burstiness
            WorkloadPattern::Balanced
        } else if stats.burstiness_score > 100.0 {
            WorkloadPattern::BatchProcessing
        } else if stats.pattern_stability > self.config.stability_threshold
            && stats.burstiness_score < 10.0
        {
            WorkloadPattern::Streaming
        } else if stats.query_latency_p99 > 1000.0 && stats.avg_reads_per_sec < 100.0 {
            WorkloadPattern::Analytics
        } else {
            WorkloadPattern::Mixed
        };

        Ok(pattern)
    }

    /// Get current pattern for a collection
    pub async fn get_pattern(&self, collection_id: &str) -> Option<WorkloadPattern> {
        let patterns = self.patterns.read().await;
        patterns.get(collection_id).cloned()
    }

    /// Get workload statistics for a collection
    pub async fn get_statistics(&self, collection_id: &str) -> Result<WorkloadStatistics> {
        let time_series = self.time_series.read().await;
        let series = time_series
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No time series data for collection"))?;

        self.calculate_statistics(series)
    }

    /// Predict future workload pattern
    pub async fn predict_pattern(
        &self,
        collection_id: &str,
        horizon_secs: u64,
    ) -> Result<WorkloadPattern> {
        // Simple prediction based on current trend
        let time_series = self.time_series.read().await;
        let series = time_series
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No time series data for collection"))?;

        let read_trend = WorkloadTimeSeries::calculate_trend(&series.reads_per_sec);
        let write_trend = WorkloadTimeSeries::calculate_trend(&series.writes_per_sec);

        let current_stats = self.calculate_statistics(series)?;

        // Project future values based on trend
        let future_reads = current_stats.avg_reads_per_sec + (read_trend * horizon_secs as f64);
        let future_writes = current_stats.avg_writes_per_sec + (write_trend * horizon_secs as f64);

        let future_ratio = if future_writes > 0.0 {
            future_reads / future_writes
        } else {
            f64::INFINITY
        };

        // Predict pattern based on projected values
        let predicted_pattern = if future_ratio > 10.0 {
            WorkloadPattern::ReadHeavy
        } else if future_ratio < 0.1 {
            WorkloadPattern::WriteHeavy
        } else if (0.5..=2.0).contains(&future_ratio) {
            WorkloadPattern::Balanced
        } else {
            WorkloadPattern::Mixed
        };

        Ok(predicted_pattern)
    }
}

impl Clone for WorkloadAnalyzer {
    fn clone(&self) -> Self {
        Self {
            time_series: self.time_series.clone(),
            patterns: self.patterns.clone(),
            config: self.config.clone(),
            monitor_handle: Arc::new(RwLock::new(None)), // Don't clone handle
            shutdown_tx: Arc::new(RwLock::new(None)),    // Don't clone shutdown channel
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_workload_pattern_detection() {
        let analyzer = WorkloadAnalyzer::new().await.unwrap();

        // Simulate read-heavy workload
        for i in 0..20 {
            analyzer
                .record_metric("test_collection", "reads_per_sec", 1000.0 + i as f64)
                .await
                .unwrap();
            analyzer
                .record_metric("test_collection", "writes_per_sec", 10.0)
                .await
                .unwrap();
            analyzer
                .record_metric("test_collection", "query_latency_ms", 5.0)
                .await
                .unwrap();
        }

        let pattern = analyzer.analyze_workload("test_collection").await.unwrap();
        assert_eq!(pattern, WorkloadPattern::ReadHeavy);
    }

    #[tokio::test]
    async fn test_workload_statistics() {
        let analyzer = WorkloadAnalyzer::new().await.unwrap();

        // Record metrics
        for i in 0..10 {
            analyzer
                .record_metric("test_collection", "reads_per_sec", 100.0 * (i + 1) as f64)
                .await
                .unwrap();
            analyzer
                .record_metric("test_collection", "writes_per_sec", 50.0)
                .await
                .unwrap();
            analyzer
                .record_metric("test_collection", "query_latency_ms", 10.0 + i as f64)
                .await
                .unwrap();
        }

        let stats = analyzer.get_statistics("test_collection").await.unwrap();
        assert!(stats.avg_reads_per_sec > 0.0);
        assert!(stats.read_write_ratio > 1.0);
        assert!(stats.query_latency_p50 > 0.0);
    }
}
