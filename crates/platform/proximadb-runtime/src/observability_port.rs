//! Observability composition port trait for `proximadb-runtime`.
//!
//! `ObservabilityPort` is the stable contract that the gRPC
//! `ObservabilityService` in `proximadb-api` uses to call into the
//! observability subsystem without importing root-crate concrete types.
//!
//! Every method maps to a gRPC `ObservabilityService` RPC.  The streaming
//! `StreamLogs` RPC is expressed as a batch return (`Vec<LogEntry>`) so the
//! port stays protocol-neutral; the gRPC adapter wraps it in a tonic channel
//! stream.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_proto::v1::{
    AggregateMetricsRequest, AggregateMetricsResponse, CreateObservabilityNamespaceRequest,
    CreateObservabilityNamespaceResponse, DeleteAlertRuleRequest, DeleteAlertRuleResponse,
    DeleteNamespaceRequest, DeleteNamespaceResponse, GetTraceRequest, GetTraceResponse,
    IngestLogsRequest, IngestLogsResponse, IngestMetricsRequest, IngestMetricsResponse,
    IngestTracesRequest, IngestTracesResponse, ListAlertsRequest, ListAlertsResponse,
    ListNamespacesRequest, ListNamespacesResponse, LogEntry, QueryLogsRequest, QueryLogsResponse,
    QueryMetricsRequest, QueryMetricsResponse, QueryTracesRequest, QueryTracesResponse,
    UpsertAlertRuleRequest, UpsertAlertRuleResponse,
};

/// Port for observability operations (logs, metrics, traces, alerts).
///
/// Implemented by root-crate `ObservabilityService`.  When absent, the
/// protocol adapter returns a safe "not configured" status so the node
/// starts without an observability backend.
///
/// The streaming `StreamLogs` RPC is handled as a batch fetch — the gRPC
/// adapter converts `Vec<LogEntry>` into a tonic `ReceiverStream`.
#[async_trait]
pub trait ObservabilityPort: Send + Sync {
    // ── Namespace management ──────────────────────────────────────────────

    async fn create_namespace(
        &self,
        request: CreateObservabilityNamespaceRequest,
    ) -> Result<CreateObservabilityNamespaceResponse>;

    async fn list_namespaces(
        &self,
        request: ListNamespacesRequest,
    ) -> Result<ListNamespacesResponse>;

    async fn delete_namespace(
        &self,
        request: DeleteNamespaceRequest,
    ) -> Result<DeleteNamespaceResponse>;

    // ── Logs ──────────────────────────────────────────────────────────────

    async fn ingest_logs(&self, request: IngestLogsRequest) -> Result<IngestLogsResponse>;

    async fn query_logs(&self, request: QueryLogsRequest) -> Result<QueryLogsResponse>;

    /// Batch-fetch log entries for the streaming RPC.
    ///
    /// The gRPC adapter wraps the returned `Vec<LogEntry>` in a
    /// `tokio::sync::mpsc` channel stream.
    async fn stream_logs(&self, request: QueryLogsRequest) -> Result<Vec<LogEntry>>;

    // ── Metrics ───────────────────────────────────────────────────────────

    async fn ingest_metrics(&self, request: IngestMetricsRequest) -> Result<IngestMetricsResponse>;

    async fn query_metrics(&self, request: QueryMetricsRequest) -> Result<QueryMetricsResponse>;

    async fn aggregate_metrics(
        &self,
        request: AggregateMetricsRequest,
    ) -> Result<AggregateMetricsResponse>;

    // ── Traces ────────────────────────────────────────────────────────────

    async fn ingest_traces(&self, request: IngestTracesRequest) -> Result<IngestTracesResponse>;

    async fn query_traces(&self, request: QueryTracesRequest) -> Result<QueryTracesResponse>;

    async fn get_trace(&self, request: GetTraceRequest) -> Result<GetTraceResponse>;

    // ── Alerts ────────────────────────────────────────────────────────────

    async fn upsert_alert_rule(
        &self,
        request: UpsertAlertRuleRequest,
    ) -> Result<UpsertAlertRuleResponse>;

    async fn delete_alert_rule(
        &self,
        request: DeleteAlertRuleRequest,
    ) -> Result<DeleteAlertRuleResponse>;

    async fn list_alerts(&self, request: ListAlertsRequest) -> Result<ListAlertsResponse>;
}
