// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Comprehensive metrics collection and monitoring system for ProximaDB

pub mod collector;
pub mod exporters;
pub mod index_metrics;
pub mod query_metrics;
pub mod registry;
pub mod server_metrics;
pub mod storage_metrics;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

pub use collector::MetricsCollector;
pub use exporters::{JsonExporter, PrometheusExporter};
pub use registry::MetricsRegistry;

/// Core metric types supported by ProximaDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
}

/// Metric value with timestamp
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricValue {
    pub value: f64,
    pub timestamp: DateTime<Utc>,
    pub labels: HashMap<String, String>,
}

/// Metric definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub help: String,
    pub metric_type: MetricType,
    pub values: Vec<MetricValue>,
}

/// Histogram bucket for latency measurements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramBucket {
    pub upper_bound: f64,
    pub count: u64,
}

/// Histogram metric for measuring distributions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Histogram {
    pub sum: f64,
    pub count: u64,
    pub buckets: Vec<HistogramBucket>,
}

/// Summary metric with quantiles
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub sum: f64,
    pub count: u64,
    pub quantiles: Vec<(f64, f64)>, // (quantile, value) pairs
}

/// System-wide metrics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub timestamp: DateTime<Utc>,
    pub server: ServerMetrics,
    pub storage: StorageMetrics,
    pub index: IndexMetrics,
    pub query: QueryMetrics,
    pub custom: HashMap<String, f64>,
}

/// Server-level metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMetrics {
    pub uptime_seconds: f64,
    pub cpu_usage_percent: f64,
    pub memory_usage_bytes: u64,
    pub memory_available_bytes: u64,
    pub disk_usage_bytes: u64,
    pub disk_available_bytes: u64,
    pub network_bytes_in: u64,
    pub network_bytes_out: u64,
    pub open_connections: u32,
    pub active_requests: u32,
}

/// Storage-level metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetrics {
    pub total_vectors: u64,
    pub total_collections: u32,
    pub storage_size_bytes: u64,
    pub wal_size_bytes: u64,
    pub memtable_size_bytes: u64,
    pub compaction_operations: u64,
    pub flush_operations: u64,
    pub read_operations_per_second: f64,
    pub write_operations_per_second: f64,
    pub cache_hit_rate: f64,
}

/// Index-level metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetrics {
    pub total_indexes: u32,
    pub index_memory_usage_bytes: u64,
    pub index_build_operations: u64,
    pub index_rebuild_operations: u64,
    pub index_optimization_operations: u64,
    pub search_operations_per_second: f64,
    pub average_search_latency_ms: f64,
    pub p99_search_latency_ms: f64,
    pub vector_insertions_per_second: f64,
    pub vector_deletions_per_second: f64,
}

/// Query-level metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMetrics {
    pub total_queries: u64,
    pub successful_queries: u64,
    pub failed_queries: u64,
    pub average_query_latency_ms: f64,
    pub p50_query_latency_ms: f64,
    pub p90_query_latency_ms: f64,
    pub p99_query_latency_ms: f64,
    pub queries_per_second: f64,
    pub slow_queries: u64, // Queries above threshold
    pub timeout_queries: u64,
}

/// Alerts and thresholds configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertThresholds {
    pub max_cpu_usage_percent: f64,
    pub max_memory_usage_percent: f64,
    pub max_disk_usage_percent: f64,
    pub max_query_latency_ms: f64,
    pub min_cache_hit_rate: f64,
    pub max_error_rate: f64,
}

impl Default for AlertThresholds {
    fn default() -> Self {
        Self {
            max_cpu_usage_percent: 80.0,
            max_memory_usage_percent: 85.0,
            max_disk_usage_percent: 90.0,
            max_query_latency_ms: 100.0,
            min_cache_hit_rate: 0.8,
            max_error_rate: 0.05,
        }
    }
}

/// Alert level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

/// System alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub level: AlertLevel,
    pub message: String,
    pub metric_name: String,
    pub current_value: f64,
    pub threshold_value: f64,
    pub timestamp: DateTime<Utc>,
    pub acknowledged: bool,
}

/// Metrics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    pub collection_interval_seconds: u64,
    pub retention_hours: u64,
    pub enable_prometheus: bool,
    pub prometheus_port: u16,
    pub enable_detailed_logging: bool,
    pub alert_thresholds: AlertThresholds,
    pub histogram_buckets: Vec<f64>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            collection_interval_seconds: 30,
            retention_hours: 24 * 7, // 1 week
            enable_prometheus: true,
            prometheus_port: 9090,
            enable_detailed_logging: false,
            alert_thresholds: AlertThresholds::default(),
            histogram_buckets: vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ],
        }
    }
}

/// Rate limiter for metrics to prevent overload
pub struct MetricsRateLimiter {
    last_emit: Arc<RwLock<Instant>>,
    min_interval: Duration,
}

impl MetricsRateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            last_emit: Arc::new(RwLock::new(Instant::now())),
            min_interval,
        }
    }

    pub async fn should_emit(&self) -> bool {
        let now = Instant::now();
        let mut last_emit = self.last_emit.write().await;

        if now.duration_since(*last_emit) >= self.min_interval {
            *last_emit = now;
            true
        } else {
            false
        }
    }
}

/// Utility functions for metrics
impl SystemMetrics {
    /// Create empty system metrics
    pub fn new() -> Self {
        Self {
            timestamp: Utc::now(),
            server: ServerMetrics::default(),
            storage: StorageMetrics::default(),
            index: IndexMetrics::default(),
            query: QueryMetrics::default(),
            custom: HashMap::new(),
        }
    }

    /// Check if any metrics exceed alert thresholds
    pub fn check_alerts(&self, thresholds: &AlertThresholds) -> Vec<Alert> {
        let mut alerts = Vec::new();

        // CPU usage alert
        if self.server.cpu_usage_percent > thresholds.max_cpu_usage_percent {
            alerts.push(Alert {
                id: format!("cpu_usage_{}", self.timestamp.timestamp()),
                level: AlertLevel::Warning,
                message: format!("High CPU usage: {:.1}%", self.server.cpu_usage_percent),
                metric_name: "cpu_usage_percent".to_string(),
                current_value: self.server.cpu_usage_percent,
                threshold_value: thresholds.max_cpu_usage_percent,
                timestamp: self.timestamp,
                acknowledged: false,
            });
        }

        // Memory usage alert
        let memory_usage_percent = if self.server.memory_available_bytes > 0 {
            ((self.server.memory_usage_bytes as f64) / (self.server.memory_available_bytes as f64))
                * 100.0
        } else {
            0.0
        };

        if memory_usage_percent > thresholds.max_memory_usage_percent {
            alerts.push(Alert {
                id: format!("memory_usage_{}", self.timestamp.timestamp()),
                level: AlertLevel::Warning,
                message: format!("High memory usage: {:.1}%", memory_usage_percent),
                metric_name: "memory_usage_percent".to_string(),
                current_value: memory_usage_percent,
                threshold_value: thresholds.max_memory_usage_percent,
                timestamp: self.timestamp,
                acknowledged: false,
            });
        }

        // Query latency alert
        if self.query.p99_query_latency_ms > thresholds.max_query_latency_ms {
            alerts.push(Alert {
                id: format!("query_latency_{}", self.timestamp.timestamp()),
                level: AlertLevel::Critical,
                message: format!(
                    "High query latency: {:.1}ms",
                    self.query.p99_query_latency_ms
                ),
                metric_name: "p99_query_latency_ms".to_string(),
                current_value: self.query.p99_query_latency_ms,
                threshold_value: thresholds.max_query_latency_ms,
                timestamp: self.timestamp,
                acknowledged: false,
            });
        }

        // Cache hit rate alert
        if self.storage.cache_hit_rate < thresholds.min_cache_hit_rate {
            alerts.push(Alert {
                id: format!("cache_hit_rate_{}", self.timestamp.timestamp()),
                level: AlertLevel::Warning,
                message: format!(
                    "Low cache hit rate: {:.1}%",
                    self.storage.cache_hit_rate * 100.0
                ),
                metric_name: "cache_hit_rate".to_string(),
                current_value: self.storage.cache_hit_rate,
                threshold_value: thresholds.min_cache_hit_rate,
                timestamp: self.timestamp,
                acknowledged: false,
            });
        }

        alerts
    }
}

impl Default for ServerMetrics {
    fn default() -> Self {
        Self {
            uptime_seconds: 0.0,
            cpu_usage_percent: 0.0,
            memory_usage_bytes: 0,
            memory_available_bytes: 0,
            disk_usage_bytes: 0,
            disk_available_bytes: 0,
            network_bytes_in: 0,
            network_bytes_out: 0,
            open_connections: 0,
            active_requests: 0,
        }
    }
}

impl Default for StorageMetrics {
    fn default() -> Self {
        Self {
            total_vectors: 0,
            total_collections: 0,
            storage_size_bytes: 0,
            wal_size_bytes: 0,
            memtable_size_bytes: 0,
            compaction_operations: 0,
            flush_operations: 0,
            read_operations_per_second: 0.0,
            write_operations_per_second: 0.0,
            cache_hit_rate: 0.0,
        }
    }
}

impl Default for IndexMetrics {
    fn default() -> Self {
        Self {
            total_indexes: 0,
            index_memory_usage_bytes: 0,
            index_build_operations: 0,
            index_rebuild_operations: 0,
            index_optimization_operations: 0,
            search_operations_per_second: 0.0,
            average_search_latency_ms: 0.0,
            p99_search_latency_ms: 0.0,
            vector_insertions_per_second: 0.0,
            vector_deletions_per_second: 0.0,
        }
    }
}

impl Default for QueryMetrics {
    fn default() -> Self {
        Self {
            total_queries: 0,
            successful_queries: 0,
            failed_queries: 0,
            average_query_latency_ms: 0.0,
            p50_query_latency_ms: 0.0,
            p90_query_latency_ms: 0.0,
            p99_query_latency_ms: 0.0,
            queries_per_second: 0.0,
            slow_queries: 0,
            timeout_queries: 0,
        }
    }
}
