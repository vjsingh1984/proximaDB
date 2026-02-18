//! Unified metrics exporters module combining Prometheus, JSON, and OpenTelemetry formats

pub mod json;
pub mod prometheus;

use anyhow::Result;
use std::collections::HashMap;

pub use json::JsonExporter;
pub use prometheus::PrometheusExporter;

/// Trait for all metric exporters
pub trait MetricsExporter: Send + Sync {
    /// Export metrics to the specific format
    fn export(&self, metrics: &MetricsSnapshot) -> Result<String>;

    /// Get the content type for HTTP responses
    fn content_type(&self) -> &'static str;

    /// Get the export format name
    fn format_name(&self) -> &'static str;
}

/// Unified metrics snapshot for export
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricsSnapshot {
    pub timestamp: i64,
    pub system: SystemMetrics,
    pub collections: HashMap<String, CollectionMetrics>,
    pub cache: CacheMetrics,
    pub compression: CompressionMetrics,
    pub custom: HashMap<String, f64>,
}

/// System-wide metrics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemMetrics {
    pub cpu_usage: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub uptime_seconds: f64,
    pub server: ServerMetrics,
    pub storage: StorageMetrics,
    pub query: QueryMetrics,
    pub index: IndexMetrics,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self {
            cpu_usage: 0.0,
            memory_used_bytes: 0,
            memory_total_bytes: 0,
            disk_used_bytes: 0,
            disk_total_bytes: 0,
            network_rx_bytes: 0,
            network_tx_bytes: 0,
            uptime_seconds: 0.0,
            server: ServerMetrics::default(),
            storage: StorageMetrics::default(),
            query: QueryMetrics::default(),
            index: IndexMetrics::default(),
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Server-specific metrics
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ServerMetrics {
    pub uptime_seconds: f64,
}

/// Storage-specific metrics
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StorageMetrics {
    pub total_vectors: u64,
    pub total_collections: u64,
    pub storage_size_bytes: u64,
}

/// Query-specific metrics
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct QueryMetrics {
    pub total_queries: u64,
    pub failed_queries: u64,
    pub p99_latency_ms: f64,
}

/// Index-specific metrics
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct IndexMetrics {
    pub total_indexes: u64,
    pub index_memory_usage_bytes: u64,
    pub search_operations_per_second: f64,
}

/// Per-collection metrics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CollectionMetrics {
    pub vector_count: u64,
    pub index_size_bytes: u64,
    pub search_qps: f64,
    pub insert_qps: f64,
    pub p99_latency_ms: f64,
    pub cache_hit_rate: f64,
}

/// Cache system metrics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheMetrics {
    #[allow(dead_code)]
    pub hit_rate: f64,
    #[allow(dead_code)]
    pub evictions_per_second: f64,
    #[allow(dead_code)]
    pub memory_used_bytes: u64,
    #[allow(dead_code)]
    pub entries_count: u64,
}

/// Compression metrics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompressionMetrics {
    pub compression_ratio: f64,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub compression_time_ms: f64,
}

/// Export format enum
#[derive(Debug, Clone, Copy)]
pub enum ExportFormat {
    Prometheus,
    Json,
    OpenTelemetry,
}

impl ExportFormat {
    pub fn create_exporter(&self) -> Box<dyn MetricsExporter> {
        match self {
            ExportFormat::Prometheus => Box::new(PrometheusExporter::new()),
            ExportFormat::Json => Box::new(JsonExporter::new()),
            ExportFormat::OpenTelemetry => Box::new(JsonExporter::new()), // Use JSON for now
        }
    }
}
