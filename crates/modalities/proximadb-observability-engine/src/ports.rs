//! Dependency-inversion port for the observability storage layer.
//!
//! The observability facade (`ObservabilityService` / `Ingester` / `QueryEngine`)
//! lives in this crate; the concrete WAL-backed `ObservabilityStorage` lives in
//! the root `src/observability/storage/` (it carries root-storage up-edges:
//! `UnifiedWALReader/Writer`, `FlushParameters`). This port dissolves the
//! facade→storage up-edge: the facade holds `Arc<dyn ObservabilityStoragePort>`
//! and the root `ObservabilityStorage` impls it; the composition root injects
//! the concrete storage (coerced to the port) into the facade.
//!
//! The surface is exactly the methods the facade (and the root query-facade
//! strategy via `QueryEngine::storage()`) calls on storage — measured via
//! compile-time grep (the `graph_edge` CALLS view over-approximates via
//! trait-impl devirtualization).

use anyhow::Result;

use crate::model::{ObservabilityStorageStats, TraceSpan, TraceSummary};
use crate::proto::proximadb_v1::{LogEntry, MetricSample, ObservabilityNamespaceConfig};

/// Namespace-scoped observability storage the facade depends on.
#[async_trait::async_trait]
pub trait ObservabilityStoragePort: Send + Sync {
    /// Create + initialize storage for a namespace.
    async fn create_namespace(
        &self,
        name: &str,
        config: &ObservabilityNamespaceConfig,
    ) -> Result<()>;

    /// Delete a namespace and its data.
    async fn delete_namespace(&self, name: &str) -> Result<()>;

    /// Persist a log entry.
    async fn write_log(&self, namespace: &str, log: &LogEntry) -> Result<()>;

    /// Persist a metric sample.
    async fn write_metric(&self, namespace: &str, metric: &MetricSample) -> Result<()>;

    /// Persist a trace span.
    async fn write_span(&self, namespace: &str, span: &TraceSpan) -> Result<()>;

    /// Look up all spans for a single trace id.
    async fn query_trace(&self, namespace: &str, trace_id: &str) -> Result<Vec<TraceSpan>>;

    /// Summarize traces overlapping a time window (capped at `limit`).
    async fn query_traces_by_time(
        &self,
        namespace: &str,
        start_ns: i64,
        end_ns: i64,
        limit: usize,
    ) -> Result<Vec<TraceSummary>>;

    /// Per-namespace storage statistics.
    async fn stats(&self, namespace: &str) -> Result<ObservabilityStorageStats>;
}
