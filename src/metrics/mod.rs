// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Metrics Module - Observability and Performance Monitoring
//!
//! This module provides ProximaDB's comprehensive metrics and monitoring system for
//! production observability. It tracks performance metrics, resource usage, and provides
//! optimization hints based on real-time data characteristics.
//!
//! ## Role in ProximaDB Architecture
//!
//! The metrics system provides full observability:
//! ```text
//! System Components → Metrics Collectors → Aggregation Engine
//!                            ↓                    ↓
//!                    Persistent Storage    In-Memory Cache
//!                            ↓                    ↓
//!                    ┌────────────────────────────┐
//!                    │    Query Service API        │
//!                    ├────────────────────────────┤
//!                    │ REST │ Prometheus │ JSON   │
//!                    └────────────────────────────┘
//!                            ↓
//!                    Dashboards & Alerts
//! ```
//!
//! ## Key Features
//!
//! ### 1. **Comprehensive Metric Collection**
//! Track all aspects of system performance:
//! - **Operation Metrics**: Latency, throughput, error rates
//! - **Resource Metrics**: CPU, memory, disk, network usage
//! - **Storage Metrics**: Compression ratios, file counts, sizes
//! - **Index Metrics**: Build times, search efficiency, accuracy
//! - **Cache Metrics**: Hit rates, evictions, memory usage
//!
//! ### 2. **Multi-Level Aggregation**
//! Flexible time-based aggregations:
//! - **Real-time**: Last 1 minute with 1-second resolution
//! - **Recent**: Last hour with 1-minute resolution
//! - **Daily**: Last 7 days with hourly resolution
//! - **Historical**: Up to 30 days with daily resolution
//!
//! ### 3. **Query Optimization Hints**
//! Data-driven recommendations:
//! - Index selection based on query patterns
//! - Storage engine recommendations
//! - Compression algorithm suggestions
//! - Cache sizing optimization
//! - Parallelism tuning
//!
//! ### 4. **Non-Critical Path Design**
//! Metrics never block operations:
//! - Asynchronous collection and updates
//! - Graceful degradation on failures
//! - Bounded memory usage
//! - Automatic old data pruning
//!
//! ## Performance Characteristics
//!
//! - **Collection Overhead**: < 0.1% CPU usage
//! - **Memory Usage**: 512MB max (configurable)
//! - **Storage Overhead**: < 1% of data size
//! - **Query Latency**: < 10ms for recent metrics
//! - **Aggregation Speed**: 1M metrics/sec
//!
//! ## Module Organization
//!
//! - **`collectors/`**: Metric collection from components
//!   - `engine.rs`: Storage engine metrics
//!   - `access_pattern.rs`: Query pattern analysis
//!   - `filesystem.rs`: I/O and storage metrics
//!
//! - **`store.rs`**: Persistent metrics storage
//!   - Partitioned by collection for O(1) lookups
//!   - Cloud storage support (S3/GCS/Azure)
//!   - Automatic retention management
//!
//! - **`updater.rs`**: Internal metrics update API
//!   - Thread-safe metric updates
//!   - Batch processing for efficiency
//!   - Lock-free data structures
//!
//! - **`query_service.rs`**: Metrics query API
//!   - REST endpoints for dashboards
//!   - Time-range queries
//!   - Aggregation support
//!
//! - **`exporters/`**: Export formats
//!   - `prometheus.rs`: Prometheus format
//!   - `json.rs`: JSON export
//!   - `graphite.rs`: Graphite/Carbon
//!
//! ## Metric Types
//!
//! | Category | Metrics | Use Case |
//! |----------|---------|----------|
//! | **Latency** | p50, p95, p99, max | Performance monitoring |
//! | **Throughput** | ops/sec, bytes/sec | Capacity planning |
//! | **Errors** | Error rate, error types | Reliability tracking |
//! | **Resources** | CPU, memory, disk | Resource optimization |
//! | **Cache** | Hit rate, size, evictions | Cache tuning |
//! | **Storage** | Compression, file counts | Storage optimization |
//!
//! ## Configuration
//!
//! ```toml
//! [metrics]
//! enabled = true
//!
//! # Storage configuration
//! storage_path = "s3://bucket/metrics"
//! collection_partitions = 16
//! retention_days = 7
//!
//! # Update intervals
//! flush_interval_seconds = 30
//! snapshot_interval_seconds = 60
//!
//! # Resource limits
//! max_memory_mb = 512
//!
//! # Optimization thresholds
//! parallel_scan_threshold = 10
//! sparsity_threshold = 0.3
//! quantization_size_threshold = 104857600  # 100MB
//! ```
//!
//! ## REST API Endpoints
//!
//! - `GET /metrics` - Prometheus format metrics
//! - `GET /metrics/json` - JSON format metrics
//! - `GET /metrics/collections/{name}` - Per-collection metrics
//! - `GET /metrics/hints` - Optimization recommendations
//! - `GET /metrics/health` - Metrics system health
//!
//! ## Usage Example
//!
//! ```rust
//! use proximadb::metrics::{MetricsConfig, MetricsQueryService};
//!
//! // Initialize metrics system
//! let config = MetricsConfig::default();
//! let metrics = MetricsQueryService::new(config)?;
//!
//! // Query recent metrics
//! let recent = metrics.query_recent(
//!     "collection_name",
//!     Duration::from_secs(300)  // Last 5 minutes
//! ).await?;
//!
//! // Get optimization hints
//! let hints = metrics.get_optimization_hints("collection_name").await?;
//! if hints.recommend_index_rebuild {
//!     println!("Consider rebuilding index for better performance");
//! }
//! ```
//!
//! ## Prometheus Integration
//!
//! ```yaml
//! # prometheus.yml
//! scrape_configs:
//!   - job_name: 'proximadb'
//!     static_configs:
//!       - targets: ['localhost:5678']
//!     metrics_path: '/metrics'
//!     scrape_interval: 30s
//! ```
//!
//! ## Dashboard Support
//!
//! Pre-built dashboards available for:
//! - Grafana (JSON templates)
//! - DataDog (monitors and dashboards)
//! - CloudWatch (metric filters)
//! - Custom web UI at `/dashboard`

pub mod aggregator;
pub mod cache;
pub mod collectors;
pub mod compression;
pub mod exporters;
pub mod query_service;
pub mod schema;
pub mod store;
pub mod updater;

#[cfg(test)]
mod tests;

pub use aggregator::{AggregationWindow, MetricsAggregationEngine};
pub use cache::{CacheMetricsCollector, CacheMetricsSnapshot, CacheOptimizationHints};
pub use collectors::{
    EngineComparison, EngineMetricsCollector, EngineStatistics, UnifiedMetricsCollector,
};
pub use compression::{
    CompressionMetrics, CompressionMetricsTracker, CompressionResult, DecompressionResult,
};
pub use exporters::{ExportFormat, JsonExporter, MetricsExporter, PrometheusExporter};
pub use query_service::{MetricsQueryOptions, MetricsQueryService};
pub use schema::{CollectionMetrics, GlobalMetrics, QueryOptimizationHints};
pub use store::MetricsPersistenceLayer;
pub use updater::{
    CompactionMetricsUpdate, FlushMetricsUpdate, InternalMetricsUpdater, MetricsUpdate,
};

// Re-export common types for compatibility
pub use self::schema::Alert;
pub use exporters::{MetricsSnapshot, SystemMetrics};

use anyhow::Result;

/// Configuration for the metrics system
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Enable or disable the entire metrics system
    pub enabled: bool,

    /// Number of partitions for collection-based metrics storage
    pub collection_partitions: usize,

    /// Base path for metrics storage (e.g., "s3://bucket/metrics" or "file:///data/metrics")
    pub storage_path: String,

    /// Flush interval for metrics updates in seconds
    pub flush_interval_seconds: u64,

    /// Retention period in days (max: 30, default: 7)
    pub retention_days: u32,

    /// Threshold for parallel scan optimization (number of files)
    pub parallel_scan_threshold: usize,

    /// Sparsity threshold for compression decisions (% of zero/null values)
    pub sparsity_threshold: f32,

    /// Size threshold for quantization recommendations (bytes)
    pub quantization_size_threshold: u64,

    /// Snapshot interval for metrics aggregation in seconds
    pub snapshot_interval_seconds: u64,

    /// Maximum memory usage in MB for metrics cache
    pub max_memory_mb: usize,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            collection_partitions: 16,
            storage_path: "file:///data/proximadb/metrics".to_string(),
            flush_interval_seconds: 30, // 30 seconds
            retention_days: 7,
            parallel_scan_threshold: 10, // Suggest parallel scan if >10 files
            sparsity_threshold: 0.3,     // Consider sparse if >30% zeros
            quantization_size_threshold: 100 * 1024 * 1024, // 100MB
            snapshot_interval_seconds: 60, // 1 minute snapshots
            max_memory_mb: 512,          // 512MB max memory for metrics cache
        }
    }
}

impl MetricsConfig {
    /// Validate and adjust configuration to safe bounds
    pub fn validate(&mut self) -> Result<()> {
        // Enforce minimum flush interval
        if self.flush_interval_seconds < 10 {
            tracing::warn!(
                "Flush interval {} too low, setting to minimum 10 seconds",
                self.flush_interval_seconds
            );
            self.flush_interval_seconds = 10;
        }

        // Enforce maximum retention
        if self.retention_days > 30 {
            tracing::warn!(
                "Retention period {} days too high, setting to maximum 30 days",
                self.retention_days
            );
            self.retention_days = 30;
        }

        // Enforce minimum partitions
        if self.collection_partitions < 1 {
            tracing::warn!(
                "Collection partitions {} too low, setting to minimum 1",
                self.collection_partitions
            );
            self.collection_partitions = 1;
        }

        // Enforce maximum partitions
        if self.collection_partitions > 256 {
            tracing::warn!(
                "Collection partitions {} too high, setting to maximum 256",
                self.collection_partitions
            );
            self.collection_partitions = 256;
        }

        Ok(())
    }
}
