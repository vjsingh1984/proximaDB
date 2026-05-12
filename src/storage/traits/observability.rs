//! Observability storage operations trait and related types.
//!
//! This module provides Cloud SIEM-like capabilities for logs, metrics, and traces.
//! Implementations should use time-partitioned storage with hot/warm/cold tiering.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Observability storage operations trait (ISP: focused interface for observability).
///
/// This trait provides Cloud SIEM-like capabilities for logs, metrics, and traces.
/// Implementations should use time-partitioned storage with hot/warm/cold tiering.
#[async_trait]
pub trait ObservabilityStorageOperations: Send + Sync {
    /// Ingest logs in bulk.
    async fn ingest_logs(
        &self,
        namespace: &str,
        logs: Vec<crate::proto::proximadb_v1::LogEntry>,
    ) -> Result<IngestResult>;

    /// Ingest metrics in bulk.
    async fn ingest_metrics(
        &self,
        namespace: &str,
        metrics: Vec<crate::proto::proximadb_v1::MetricSample>,
    ) -> Result<IngestResult>;

    /// Ingest traces.
    async fn ingest_traces(
        &self,
        namespace: &str,
        traces: Vec<crate::proto::proximadb_v1::TraceData>,
    ) -> Result<IngestResult>;

    /// Query logs with time range and filters.
    async fn query_logs(
        &self,
        namespace: &str,
        start_time_ns: i64,
        end_time_ns: i64,
        filter: Option<crate::proto::proximadb_v1::LogFilter>,
        limit: u32,
    ) -> Result<LogQueryResult>;

    /// Aggregate metrics with PromQL-like semantics.
    async fn aggregate_metrics(
        &self,
        namespace: &str,
        params: MetricAggregationParams,
    ) -> Result<MetricAggregationResult>;

    /// Query traces by trace ID or filters.
    async fn query_traces(
        &self,
        namespace: &str,
        start_time_ns: i64,
        end_time_ns: i64,
        trace_id: Option<String>,
        service: Option<String>,
        limit: u32,
    ) -> Result<Vec<crate::proto::proximadb_v1::TraceData>>;

    /// Create an observability namespace with retention config.
    async fn create_namespace(
        &self,
        config: crate::proto::proximadb_v1::ObservabilityNamespaceConfig,
    ) -> Result<String>;

    /// List observability namespaces.
    async fn list_namespaces(&self) -> Result<Vec<NamespaceInfo>>;
}

/// Ingest result for bulk operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestResult {
    pub ingested: u64,
    pub failed: u64,
    pub errors: Vec<String>,
    pub processing_time_ms: u64,
}

/// Log query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQueryResult {
    pub logs: Vec<crate::proto::proximadb_v1::LogEntry>,
    pub next_cursor: Option<String>,
    pub total_matched: u64,
    pub query_time_ms: u64,
}

/// Metric aggregation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricAggregationParams {
    pub metric_name: String,
    pub start_time_ns: i64,
    pub end_time_ns: i64,
    pub aggregation: crate::proto::proximadb_v1::MetricAggregation,
    pub step_seconds: u32,
    pub label_filters: HashMap<String, String>,
    pub group_by: Vec<String>,
}

/// Metric aggregation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricAggregationResult {
    pub series: Vec<TimeSeriesData>,
    pub query_time_ms: u64,
}

/// Time series data point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesData {
    pub labels: HashMap<String, String>,
    pub points: Vec<DataPointValue>,
}

/// Individual data point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataPointValue {
    pub timestamp_ns: i64,
    pub value: f64,
}

/// Namespace info for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceInfo {
    pub name: String,
    pub log_count: u64,
    pub metric_count: u64,
    pub trace_count: u64,
    pub retention_config: Option<crate::proto::proximadb_v1::RetentionConfig>,
}

/// Unified multi-model storage trait combining all data model operations.
///
/// This trait follows the Composite pattern, aggregating specialized storage
/// traits into a single interface for engines that support multiple data models.
///
/// **SOLID Principles Applied:**
/// - **S (Single Responsibility)**: Each sub-trait handles one data model
/// - **O (Open/Closed)**: New data models can be added via new traits
/// - **L (Liskov Substitution)**: Any implementing engine works as UnifiedStorageEngine
/// - **I (Interface Segregation)**: Clients can depend on specific sub-traits
/// - **D (Dependency Inversion)**: Higher layers depend on these abstractions
///
/// Engines can implement this trait to provide multi-model storage capabilities
/// on top of their vector storage foundation.
#[async_trait]
pub trait MultiModelStorage:
    super::UnifiedStorageEngine + super::DocumentStorageOperations + ObservabilityStorageOperations
{
    /// Check which data models are supported by this engine.
    fn supported_models(&self) -> Vec<DataModel> {
        vec![
            DataModel::Vector,        // Always supported via UnifiedStorageEngine
            DataModel::Document,      // Via DocumentStorageOperations
            DataModel::Observability, // Via ObservabilityStorageOperations
        ]
    }

    /// Get unified storage statistics across all models.
    async fn get_multi_model_stats(&self) -> Result<MultiModelStats> {
        Ok(MultiModelStats::default())
    }
}

/// Supported data models — re-exported from the canonical definition.
pub use proximadb_data_model::DataModel;

/// Unified statistics across all data models.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MultiModelStats {
    pub vector_count: u64,
    pub document_count: u64,
    pub log_count: u64,
    pub metric_count: u64,
    pub trace_count: u64,
    pub graph_node_count: u64,
    pub graph_edge_count: u64,
    pub total_storage_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ingest_result_default() {
        let result = IngestResult::default();
        assert_eq!(result.ingested, 0);
        assert_eq!(result.failed, 0);
    }

    #[test]
    fn test_multi_model_stats_default() {
        let stats = MultiModelStats::default();
        assert_eq!(stats.vector_count, 0);
        assert_eq!(stats.document_count, 0);
    }
}
