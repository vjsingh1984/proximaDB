//! # Observability Service (gRPC)
//!
//! gRPC implementation for logs, metrics, traces, and alerts.  Each RPC
//! delegates to the injected `ObservabilityPort`; when no port is provided
//! the service returns `UNIMPLEMENTED` so the server can start without an
//! observability backend configured.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::{
    observability_service_server::{ObservabilityService, ObservabilityServiceServer},
    *,
};
use proximadb_runtime::ObservabilityPort;

/// gRPC ObservabilityService backed by an `ObservabilityPort`.
pub struct ObservabilityServiceImpl {
    port: Option<Arc<dyn ObservabilityPort>>,
}

impl ObservabilityServiceImpl {
    /// Construct with a concrete observability port.
    pub fn new(port: Arc<dyn ObservabilityPort>) -> Self {
        Self { port: Some(port) }
    }

    /// Construct without an observability backend (all RPCs return UNIMPLEMENTED).
    pub fn without_backend() -> Self {
        Self { port: None }
    }

    /// Convert into a tonic gRPC server.
    pub fn into_server(self) -> ObservabilityServiceServer<Self> {
        ObservabilityServiceServer::new(self)
    }

    fn not_configured() -> Status {
        Status::unimplemented("Observability service not configured on this node")
    }

    fn port_err(e: anyhow::Error) -> Status {
        Status::internal(e.to_string())
    }
}

/// Streaming response type alias for `StreamLogs`.
pub type StreamLogsStream = ReceiverStream<Result<LogEntry, Status>>;

#[tonic::async_trait]
impl ObservabilityService for ObservabilityServiceImpl {
    // ── Namespace management ──────────────────────────────────────────────

    async fn create_namespace(
        &self,
        request: Request<CreateObservabilityNamespaceRequest>,
    ) -> Result<Response<CreateObservabilityNamespaceResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.create_namespace(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn list_namespaces(
        &self,
        request: Request<ListNamespacesRequest>,
    ) -> Result<Response<ListNamespacesResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.list_namespaces(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn delete_namespace(
        &self,
        request: Request<DeleteNamespaceRequest>,
    ) -> Result<Response<DeleteNamespaceResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.delete_namespace(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Logs ──────────────────────────────────────────────────────────────

    async fn ingest_logs(
        &self,
        request: Request<IngestLogsRequest>,
    ) -> Result<Response<IngestLogsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.ingest_logs(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn query_logs(
        &self,
        request: Request<QueryLogsRequest>,
    ) -> Result<Response<QueryLogsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.query_logs(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    type StreamLogsStream = StreamLogsStream;

    async fn stream_logs(
        &self,
        request: Request<QueryLogsRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        let entries = port
            .stream_logs(request.into_inner())
            .await
            .map_err(Self::port_err)?;

        let (tx, rx) = mpsc::channel(128);
        tokio::spawn(async move {
            for entry in entries {
                if tx.send(Ok(entry)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    // ── Metrics ───────────────────────────────────────────────────────────

    async fn ingest_metrics(
        &self,
        request: Request<IngestMetricsRequest>,
    ) -> Result<Response<IngestMetricsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.ingest_metrics(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn query_metrics(
        &self,
        request: Request<QueryMetricsRequest>,
    ) -> Result<Response<QueryMetricsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.query_metrics(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn aggregate_metrics(
        &self,
        request: Request<AggregateMetricsRequest>,
    ) -> Result<Response<AggregateMetricsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.aggregate_metrics(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Traces ────────────────────────────────────────────────────────────

    async fn ingest_traces(
        &self,
        request: Request<IngestTracesRequest>,
    ) -> Result<Response<IngestTracesResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.ingest_traces(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn query_traces(
        &self,
        request: Request<QueryTracesRequest>,
    ) -> Result<Response<QueryTracesResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.query_traces(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn get_trace(
        &self,
        request: Request<GetTraceRequest>,
    ) -> Result<Response<GetTraceResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_trace(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Alerts ────────────────────────────────────────────────────────────

    async fn upsert_alert_rule(
        &self,
        request: Request<UpsertAlertRuleRequest>,
    ) -> Result<Response<UpsertAlertRuleResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.upsert_alert_rule(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn delete_alert_rule(
        &self,
        request: Request<DeleteAlertRuleRequest>,
    ) -> Result<Response<DeleteAlertRuleResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.delete_alert_rule(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn list_alerts(
        &self,
        request: Request<ListAlertsRequest>,
    ) -> Result<Response<ListAlertsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.list_alerts(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }
}
