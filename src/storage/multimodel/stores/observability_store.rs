//! # Observability Store
//!
//! Wraps the existing ObservabilityService for logs, metrics, and traces.
//!
//! ## Storage Strategy (from `/src/observability/`)
//!
//! - **Time-partitioned storage** for logs (hourly/daily partitions)
//! - **Time-series storage** for metrics with downsampling
//! - **Trace storage** with span assembly
//! - **WAL-backed durability** for all writes
//! - **SST-based rollup persistence** for metric aggregates

use std::sync::Arc;
use async_trait::async_trait;
use anyhow::Result;

use crate::storage::traits::{
    IngestResult, LogQueryResult, MetricAggregationParams, MetricAggregationResult,
    NamespaceInfo, ObservabilityStorageOperations,
};
use crate::proto::proximadb_v1::{LogEntry, LogFilter, MetricSample, ObservabilityNamespaceConfig, TraceData};

use super::super::traits::{ModelType, StoreCapabilities};

/// Configuration for the observability store
#[derive(Debug, Clone)]
pub struct ObservabilityStoreConfig {
    /// Base path for storage
    pub base_path: String,
    /// Enable WAL for durability
    pub enable_wal: bool,
    /// Default retention period in seconds
    pub default_retention_seconds: i64,
    /// Enable rollup aggregation
    pub enable_rollups: bool,
    /// High cardinality label limit
    pub label_cardinality_limit: u32,
}

impl Default for ObservabilityStoreConfig {
    fn default() -> Self {
        Self {
            base_path: "/tmp/proximadb/observability".to_string(),
            enable_wal: true,
            default_retention_seconds: 7 * 24 * 3600, // 7 days
            enable_rollups: true,
            label_cardinality_limit: 10_000,
        }
    }
}

/// ObservabilityStore wraps the ObservabilityService for multi-model integration
///
/// ## Architecture
///
/// ```text
/// ┌─────────────────────────────────────────────────────────┐
/// │                ObservabilityStore                        │
/// │  ┌───────────────────────────────────────────────────┐  │
/// │  │            ObservabilityService                    │  │
/// │  │  (existing implementation)                         │  │
/// │  └───────────────────────────────────────────────────┘  │
/// │              │              │              │             │
/// │    ┌─────────▼────┐ ┌──────▼─────┐ ┌─────▼──────┐      │
/// │    │    Logs      │ │  Metrics   │ │   Traces   │      │
/// │    │ (Partitioned)│ │(TimeSeries)│ │(SpanAssemb)│      │
/// │    └──────────────┘ └────────────┘ └────────────┘      │
/// │              │                                          │
/// │    ┌─────────▼───────────────────────────────────┐     │
/// │    │              WAL + SST Rollups               │     │
/// │    │  (Durability + Aggregate Persistence)        │     │
/// │    └──────────────────────────────────────────────┘     │
/// └─────────────────────────────────────────────────────────┘
/// ```
pub struct ObservabilityStore {
    /// The underlying observability storage operations service
    service: Option<Arc<dyn ObservabilityStorageOperations>>,
    /// Configuration
    config: ObservabilityStoreConfig,
}

impl ObservabilityStore {
    /// Create a new ObservabilityStore with the given configuration
    pub fn new(config: ObservabilityStoreConfig) -> Self {
        Self {
            service: None,
            config,
        }
    }

    /// Set the underlying observability service
    pub fn with_service(mut self, service: Arc<dyn ObservabilityStorageOperations>) -> Self {
        self.service = Some(service);
        self
    }

    /// Get store capabilities
    pub fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            model_type: ModelType::Observability,
            supports_transactions: false,
            supports_secondary_indexes: true, // Label indexes
            supports_acid: false,
            supports_streaming: true, // Streaming ingestion
            max_recommended_records: None, // Virtually unlimited with retention
            description: "Observability storage: time-partitioned logs, time-series metrics, trace spans".to_string(),
        }
    }

    /// Get the underlying service
    pub fn service(&self) -> Option<&Arc<dyn ObservabilityStorageOperations>> {
        self.service.as_ref()
    }

    /// Get configuration
    pub fn config(&self) -> &ObservabilityStoreConfig {
        &self.config
    }

    /// Check if store is operational
    pub fn is_operational(&self) -> bool {
        self.service.is_some()
    }
}

#[async_trait]
impl ObservabilityStorageOperations for ObservabilityStore {
    async fn ingest_logs(
        &self,
        namespace: &str,
        logs: Vec<LogEntry>,
    ) -> Result<IngestResult> {
        let service = self.service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Observability service not configured"))?;
        service.ingest_logs(namespace, logs).await
    }

    async fn ingest_metrics(
        &self,
        namespace: &str,
        metrics: Vec<MetricSample>,
    ) -> Result<IngestResult> {
        let service = self.service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Observability service not configured"))?;
        service.ingest_metrics(namespace, metrics).await
    }

    async fn ingest_traces(
        &self,
        namespace: &str,
        traces: Vec<TraceData>,
    ) -> Result<IngestResult> {
        let service = self.service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Observability service not configured"))?;
        service.ingest_traces(namespace, traces).await
    }

    async fn query_logs(
        &self,
        namespace: &str,
        start_time_ns: i64,
        end_time_ns: i64,
        filter: Option<LogFilter>,
        limit: u32,
    ) -> Result<LogQueryResult> {
        let service = self.service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Observability service not configured"))?;
        service.query_logs(namespace, start_time_ns, end_time_ns, filter, limit).await
    }

    async fn aggregate_metrics(
        &self,
        namespace: &str,
        params: MetricAggregationParams,
    ) -> Result<MetricAggregationResult> {
        let service = self.service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Observability service not configured"))?;
        service.aggregate_metrics(namespace, params).await
    }

    async fn query_traces(
        &self,
        namespace: &str,
        start_time_ns: i64,
        end_time_ns: i64,
        trace_id: Option<String>,
        service_name: Option<String>,
        limit: u32,
    ) -> Result<Vec<TraceData>> {
        let service = self.service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Observability service not configured"))?;
        service.query_traces(namespace, start_time_ns, end_time_ns, trace_id, service_name, limit).await
    }

    async fn create_namespace(
        &self,
        config: ObservabilityNamespaceConfig,
    ) -> Result<String> {
        let service = self.service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Observability service not configured"))?;
        service.create_namespace(config).await
    }

    async fn list_namespaces(&self) -> Result<Vec<NamespaceInfo>> {
        let service = self.service.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Observability service not configured"))?;
        service.list_namespaces().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_observability_store_config_default() {
        let config = ObservabilityStoreConfig::default();
        assert!(config.enable_wal);
        assert!(config.enable_rollups);
        assert_eq!(config.default_retention_seconds, 7 * 24 * 3600);
    }

    #[test]
    fn test_observability_store_capabilities() {
        let store = ObservabilityStore::new(ObservabilityStoreConfig::default());
        let caps = store.capabilities();

        assert_eq!(caps.model_type, ModelType::Observability);
        assert!(caps.supports_streaming);
    }
}
