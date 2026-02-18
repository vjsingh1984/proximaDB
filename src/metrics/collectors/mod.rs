//! Real-time metrics collectors

use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

pub mod access_pattern;
pub mod document;
pub mod engine;
pub mod filesystem;
pub mod graph;
pub mod query;
pub mod storage;
pub mod system;

#[cfg(test)]
pub mod tests;

pub use access_pattern::AccessPatternMetricsCollector;
pub use document::DocumentMetricsCollector;
pub use engine::{EngineComparison, EngineMetricsCollector, EngineStatistics, OperationTimer};
pub use filesystem::FilesystemMetricsCollector;
pub use graph::{GraphMetricsCollector, PulsarMetricsCollector, QuasarMetricsCollector};
pub use query::QueryMetricsCollector;
pub use storage::StorageMetricsCollector;
pub use system::SystemMetricsCollector;

/// Trait for all metrics collectors
#[async_trait::async_trait]
pub trait MetricsCollector: Send + Sync {
    /// Collect current metrics
    async fn collect(&self) -> Result<MetricsSample>;

    /// Get collector name
    fn name(&self) -> &'static str;

    /// Get collection interval recommendation
    fn recommended_interval(&self) -> Duration;
}

/// A single metrics sample from a collector
#[derive(Debug, Clone)]
pub struct MetricsSample {
    pub timestamp: Instant,
    pub collector: String,
    pub values: std::collections::HashMap<String, f64>,
}

/// Unified collector that aggregates all collectors
pub struct UnifiedMetricsCollector {
    collectors: Vec<Arc<dyn MetricsCollector>>,
    last_collection: Arc<RwLock<Instant>>,
    current_metrics: Arc<RwLock<crate::metrics::SystemMetrics>>,
    active_alerts: Arc<RwLock<Vec<crate::metrics::Alert>>>,
    start_time: Instant,
}

impl UnifiedMetricsCollector {
    pub fn new() -> Self {
        Self {
            collectors: Vec::new(),
            last_collection: Arc::new(RwLock::new(Instant::now())),
            current_metrics: Arc::new(RwLock::new(crate::metrics::SystemMetrics::default())),
            active_alerts: Arc::new(RwLock::new(Vec::new())),
            start_time: Instant::now(),
        }
    }

    pub fn register(&mut self, collector: Arc<dyn MetricsCollector>) {
        self.collectors.push(collector);
    }

    pub async fn collect_all(&self) -> Result<Vec<MetricsSample>> {
        let mut samples = Vec::new();

        for collector in &self.collectors {
            match collector.collect().await {
                Ok(sample) => samples.push(sample),
                Err(e) => {
                    // Log error but continue - metrics are non-critical
                    tracing::warn!("Failed to collect metrics from {}: {}", collector.name(), e);
                }
            }
        }

        *self.last_collection.write().await = Instant::now();
        Ok(samples)
    }

    /// Get current metrics snapshot (for dashboard compatibility)
    pub async fn current_metrics(&self) -> crate::metrics::SystemMetrics {
        // Calculate uptime
        let uptime_seconds = self.start_time.elapsed().as_secs_f64();

        // Try to collect fresh system metrics
        let system_collector = SystemMetricsCollector::new();
        let storage_collector = StorageMetricsCollector::new();

        if let Ok(sample) = system_collector.collect().await {
            // Update current_metrics with real data
            let mut metrics = self.current_metrics.write().await;

            // Extract values from sample
            if let Some(&cpu) = sample.values.get("cpu_usage_percent") {
                metrics.cpu_usage = cpu as f32;
            }
            if let Some(&mem_used) = sample.values.get("memory_used_bytes") {
                metrics.memory_used_bytes = mem_used as u64;
            }
            if let Some(&mem_total) = sample.values.get("memory_total_bytes") {
                metrics.memory_total_bytes = mem_total as u64;
            }
            if let Some(&disk_used) = sample.values.get("disk_used_bytes") {
                metrics.disk_used_bytes = disk_used as u64;
            }
            if let Some(&disk_total) = sample.values.get("disk_total_bytes") {
                metrics.disk_total_bytes = disk_total as u64;
            }

            // Try to collect storage metrics
            if let Ok(storage_sample) = storage_collector.collect().await {
                if let Some(&total_vectors) = storage_sample.values.get("total_vectors") {
                    metrics.storage.total_vectors = total_vectors as u64;
                }
                if let Some(&total_collections) = storage_sample.values.get("total_collections") {
                    metrics.storage.total_collections = total_collections as u64;
                }
                if let Some(&storage_size) = storage_sample.values.get("storage_size_bytes") {
                    metrics.storage.storage_size_bytes = storage_size as u64;
                }
            }

            // Set uptime and server metrics
            metrics.uptime_seconds = uptime_seconds;
            metrics.server.uptime_seconds = uptime_seconds;

            // Update timestamp
            metrics.timestamp = chrono::Utc::now();

            // Return the updated metrics
            metrics.clone()
        } else {
            // Fallback to cached metrics with updated uptime
            let mut metrics = self.current_metrics.read().await.clone();
            metrics.uptime_seconds = uptime_seconds;
            metrics.server.uptime_seconds = uptime_seconds;
            metrics.timestamp = chrono::Utc::now();
            metrics
        }
    }

    /// Get metrics summary (for dashboard compatibility)
    pub async fn metrics_summary(&self) -> MetricsSummary {
        let metrics = self.current_metrics.read().await;
        MetricsSummary {
            system_health: 0.85,
            cpu_usage: metrics.cpu_usage,
            memory_usage_percent: metrics.memory_used_bytes as f64
                / metrics.memory_total_bytes.max(1) as f64
                * 100.0,
            cache_hit_rate: 0.75,
            queries_per_second: 100.0,
            query_latency_p99: 5.0,
            active_alerts_count: self.active_alerts.read().await.len(),
            critical_alerts_count: self
                .active_alerts
                .read()
                .await
                .iter()
                .filter(|a| matches!(a.level, crate::metrics::schema::AlertLevel::Critical))
                .count(),
        }
    }

    /// Get active alerts (for dashboard compatibility)
    pub async fn active_alerts(&self) -> Vec<crate::metrics::Alert> {
        self.active_alerts.read().await.clone()
    }
}

/// Metrics summary for dashboard
pub struct MetricsSummary {
    pub system_health: f64,
    pub cpu_usage: f32,
    pub memory_usage_percent: f64,
    pub cache_hit_rate: f64,
    pub queries_per_second: f64,
    pub query_latency_p99: f64,
    pub active_alerts_count: usize,
    pub critical_alerts_count: usize,
}
