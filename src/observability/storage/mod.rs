// Observability storage layer
//
// Provides:
// - Time-partitioned storage for logs
// - Time-series storage for metrics with downsampling
// - Trace storage with span assembly
// - Hot/warm/cold tiering
// - WAL-backed durability for all writes
// - SST-based rollup persistence for metric aggregates

/// Time-series metric storage with downsampling and aggregation.
pub mod metrics;
/// Time-partitioned log storage with hot/warm/cold tiering.
pub mod partitioned;
/// Persistent storage for downsampled metric rollup aggregates.
pub mod rollup_persistence;
/// Distributed trace storage with span assembly and querying.
pub mod traces;

// Re-export tier types
pub use partitioned::{PartitionTier, TierFlushResult};

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, RwLock};
use tracing::info;

use self::traces::TraceSpan;
use crate::proto::proximadb_v1::{LogEntry, MetricSample, ObservabilityNamespaceConfig};
use crate::storage::persistence::write_ahead_log::unified_operations::{
    ObservabilityOperation, UnifiedWALOperation, UnifiedWALReader, UnifiedWALWriter,
};

/// Observability storage service
///
/// Manages storage for logs, metrics, and traces across multiple namespaces.
/// Features:
/// - WAL-backed durability for all writes
/// - Time-partitioned log storage
/// - Time-series metric storage with downsampling
/// - Trace storage with span assembly
/// - Namespace isolation
pub struct ObservabilityStorage {
    /// Namespace configurations
    namespaces: RwLock<HashMap<String, NamespaceStorage>>,
    /// Base path for storage
    base_path: String,
    /// WAL writer for durability
    wal_writer: Arc<Mutex<Option<UnifiedWALWriter>>>,
    /// WAL path
    wal_path: String,
}

/// Storage for a single namespace
///
/// Isolated storage for a single observability namespace.
/// Contains separate storage engines for logs, metrics, and traces.
struct NamespaceStorage {
    /// Configuration
    #[allow(dead_code)]
    config: ObservabilityNamespaceConfig,
    /// Partitioned log storage
    logs: partitioned::PartitionedStorage,
    /// Metric storage
    metrics: metrics::MetricStorage,
    /// Trace storage
    traces: traces::TraceStorage,
}

impl ObservabilityStorage {
    /// Create a new observability storage (without WAL)
    pub fn new(base_path: &str) -> Self {
        Self {
            namespaces: RwLock::new(HashMap::new()),
            base_path: base_path.to_string(),
            wal_writer: Arc::new(Mutex::new(None)),
            wal_path: String::new(),
        }
    }

    /// Create a new observability storage with WAL support
    pub async fn new_with_wal(base_path: &str) -> Result<Self> {
        let wal_path = format!("{}/observability_wal", base_path);
        let wal_writer = UnifiedWALWriter::new(wal_path.clone())
            .await
            .context("Failed to create observability WAL writer")?;

        let mut storage = Self {
            namespaces: RwLock::new(HashMap::new()),
            base_path: base_path.to_string(),
            wal_writer: Arc::new(Mutex::new(Some(wal_writer))),
            wal_path,
        };

        // Recover from WAL on startup
        storage.recover_from_wal().await?;

        Ok(storage)
    }

    /// Recover state from WAL on startup
    async fn recover_from_wal(&mut self) -> Result<()> {
        info!(
            "Recovering observability storage from WAL at: {}",
            self.wal_path
        );
        let reader = UnifiedWALReader::new(self.wal_path.clone()).await?;
        let entries = reader.read_all().await?;

        let mut recovered_logs = 0u64;
        let mut recovered_metrics = 0u64;
        let mut recovered_namespaces = 0u64;

        for entry in entries {
            if entry.is_observability_operation()
                && let UnifiedWALOperation::ObservabilityOp(op) = entry.operation {
                    match op {
                        ObservabilityOperation::CreateNamespace {
                            namespace,
                            config_json,
                        } => {
                            if let Ok(config) =
                                serde_json::from_str::<ObservabilityNamespaceConfig>(&config_json)
                            {
                                // Replay namespace creation (without WAL write)
                                let namespace_path =
                                    format!("{}/observability/{}", self.base_path, namespace);
                                let namespace_storage = NamespaceStorage {
                                    config: config.clone(),
                                    logs: partitioned::PartitionedStorage::new(&format!(
                                        "{}/logs",
                                        namespace_path
                                    ))?,
                                    metrics: metrics::MetricStorage::new(&format!(
                                        "{}/metrics",
                                        namespace_path
                                    ))?,
                                    traces: traces::TraceStorage::new(&format!(
                                        "{}/traces",
                                        namespace_path
                                    ))?,
                                };
                                let mut namespaces = self.namespaces.write().await;
                                namespaces.insert(namespace, namespace_storage);
                                recovered_namespaces += 1;
                            }
                        }
                        ObservabilityOperation::WriteLog { namespace, log } => {
                            let namespaces = self.namespaces.read().await;
                            if let Some(ns) = namespaces.get(&namespace) {
                                let _ = ns.logs.write(&log).await;
                                recovered_logs += 1;
                            }
                        }
                        ObservabilityOperation::WriteLogs { namespace, logs } => {
                            let namespaces = self.namespaces.read().await;
                            if let Some(ns) = namespaces.get(&namespace) {
                                for log in logs {
                                    let _ = ns.logs.write(&log).await;
                                    recovered_logs += 1;
                                }
                            }
                        }
                        ObservabilityOperation::WriteMetric { namespace, metric } => {
                            let namespaces = self.namespaces.read().await;
                            if let Some(ns) = namespaces.get(&namespace) {
                                let _ = ns.metrics.write(&metric).await;
                                recovered_metrics += 1;
                            }
                        }
                        ObservabilityOperation::WriteMetrics { namespace, metrics } => {
                            let namespaces = self.namespaces.read().await;
                            if let Some(ns) = namespaces.get(&namespace) {
                                for metric in metrics {
                                    let _ = ns.metrics.write(&metric).await;
                                    recovered_metrics += 1;
                                }
                            }
                        }
                        ObservabilityOperation::WriteSpan {
                            namespace,
                            span_json,
                        } => {
                            let namespaces = self.namespaces.read().await;
                            if let Some(ns) = namespaces.get(&namespace)
                                && let Ok(span) = serde_json::from_str::<TraceSpan>(&span_json) {
                                    let _ = ns.traces.write(&span).await;
                                }
                        }
                        ObservabilityOperation::DeleteNamespace { namespace } => {
                            let mut namespaces = self.namespaces.write().await;
                            namespaces.remove(&namespace);
                        }
                    }
                }
        }

        info!(
            "WAL recovery complete: {} namespaces, {} logs, {} metrics recovered",
            recovered_namespaces, recovered_logs, recovered_metrics
        );
        Ok(())
    }

    /// Write operation to WAL (if enabled)
    async fn write_to_wal(&self, operation: ObservabilityOperation) -> Result<()> {
        let mut wal_guard = self.wal_writer.lock().await;
        if let Some(ref mut writer) = *wal_guard {
            let wal_op = UnifiedWALOperation::ObservabilityOp(operation);
            writer.append(wal_op).await?;
        }
        Ok(())
    }

    /// Flush WAL to disk
    pub async fn flush_wal(&self) -> Result<()> {
        let mut wal_guard = self.wal_writer.lock().await;
        if let Some(ref mut writer) = *wal_guard {
            writer.flush().await?;
        }
        Ok(())
    }

    /// Create a new namespace
    pub async fn create_namespace(
        &self,
        name: &str,
        config: &ObservabilityNamespaceConfig,
    ) -> Result<()> {
        info!("Creating observability namespace: {}", name);

        // Write to WAL first
        let config_json =
            serde_json::to_string(config).context("Failed to serialize namespace config")?;
        self.write_to_wal(ObservabilityOperation::CreateNamespace {
            namespace: name.to_string(),
            config_json,
        })
        .await?;

        let namespace_path = format!("{}/observability/{}", self.base_path, name);

        let namespace_storage = NamespaceStorage {
            config: config.clone(),
            logs: partitioned::PartitionedStorage::new(&format!("{}/logs", namespace_path))?,
            metrics: metrics::MetricStorage::new(&format!("{}/metrics", namespace_path))?,
            traces: traces::TraceStorage::new(&format!("{}/traces", namespace_path))?,
        };

        let mut namespaces = self.namespaces.write().await;
        namespaces.insert(name.to_string(), namespace_storage);

        Ok(())
    }

    /// Delete a namespace
    pub async fn delete_namespace(&self, name: &str) -> Result<()> {
        info!("Deleting observability namespace: {}", name);

        // Write to WAL first
        self.write_to_wal(ObservabilityOperation::DeleteNamespace {
            namespace: name.to_string(),
        })
        .await?;

        // Remove from in-memory map
        let mut namespaces = self.namespaces.write().await;
        namespaces
            .remove(name)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", name))?;

        info!("Deleted observability namespace: {}", name);
        Ok(())
    }

    /// Write a log entry
    pub async fn write_log(&self, namespace: &str, log: &LogEntry) -> Result<()> {
        // Write to WAL first
        self.write_to_wal(ObservabilityOperation::WriteLog {
            namespace: namespace.to_string(),
            log: log.clone(),
        })
        .await?;

        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        ns.logs.write(log).await
    }

    /// Write a metric sample
    pub async fn write_metric(&self, namespace: &str, metric: &MetricSample) -> Result<()> {
        // Write to WAL first
        self.write_to_wal(ObservabilityOperation::WriteMetric {
            namespace: namespace.to_string(),
            metric: metric.clone(),
        })
        .await?;

        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        ns.metrics.write(metric).await
    }

    /// Write a trace span
    pub async fn write_span(&self, namespace: &str, span: &TraceSpan) -> Result<()> {
        // Write to WAL first (serialize span to JSON)
        let span_json = serde_json::to_string(span).context("Failed to serialize trace span")?;
        self.write_to_wal(ObservabilityOperation::WriteSpan {
            namespace: namespace.to_string(),
            span_json,
        })
        .await?;

        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        ns.traces.write(span).await
    }

    /// Query logs
    pub async fn query_logs(
        &self,
        namespace: &str,
        start_ns: i64,
        end_ns: i64,
        limit: usize,
    ) -> Result<Vec<LogEntry>> {
        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        ns.logs.query(start_ns, end_ns, limit).await
    }

    /// Query metrics
    pub async fn query_metrics(
        &self,
        namespace: &str,
        metric_name: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<MetricSample>> {
        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        ns.metrics.query(metric_name, start_ns, end_ns).await
    }

    /// Query trace by ID
    pub async fn query_trace(&self, namespace: &str, trace_id: &str) -> Result<Vec<TraceSpan>> {
        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        ns.traces.query_by_trace_id(trace_id).await
    }

    /// Query traces by time range
    pub async fn query_traces_by_time(
        &self,
        namespace: &str,
        start_ns: i64,
        end_ns: i64,
        limit: usize,
    ) -> Result<Vec<traces::TraceSummary>> {
        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        ns.traces.query_by_time(start_ns, end_ns, limit).await
    }

    /// Query traces by service
    pub async fn query_traces_by_service(
        &self,
        namespace: &str,
        service: &str,
        start_ns: i64,
        end_ns: i64,
        limit: usize,
    ) -> Result<Vec<traces::TraceSummary>> {
        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        ns.traces
            .query_by_service(service, start_ns, end_ns, limit)
            .await
    }

    /// Get storage statistics for a namespace
    pub async fn stats(&self, namespace: &str) -> Result<StorageStats> {
        let namespaces = self.namespaces.read().await;
        let ns = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        // Calculate total bytes from logs, metrics, and traces
        let log_bytes = ns.logs.total_bytes().await;
        let metric_bytes = ns.metrics.total_bytes().await;
        let trace_bytes = ns.traces.total_bytes().await;

        Ok(StorageStats {
            log_count: ns.logs.count().await,
            metric_series_count: ns.metrics.series_count().await,
            trace_count: ns.traces.count().await,
            total_bytes: log_bytes + metric_bytes + trace_bytes,
        })
    }
}

/// Storage statistics
///
/// Provides statistics about a namespace's storage usage,
/// including counts of logs, metric series, and traces.
#[derive(Debug, Clone)]
pub struct StorageStats {
    /// Number of log entries
    pub log_count: u64,
    /// Number of metric series
    pub metric_series_count: u64,
    /// Number of traces
    pub trace_count: u64,
    /// Total storage bytes
    pub total_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_stats() {
        let stats = StorageStats {
            log_count: 1000,
            metric_series_count: 50,
            trace_count: 100,
            total_bytes: 1024 * 1024,
        };
        assert_eq!(stats.log_count, 1000);
    }
}
