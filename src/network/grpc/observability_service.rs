// Observability gRPC service implementation
//
// Implements the ObservabilityService defined in proto/proximadb/v1/observability.proto

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, warn};

use crate::observability::ObservabilityService as ObsStorageService;
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::observability_service_server::{
    ObservabilityService, ObservabilityServiceServer,
};

/// Observability gRPC service implementation
pub struct ObservabilityServiceImpl {
    observability_service: Arc<ObsStorageService>,
}

impl ObservabilityServiceImpl {
    /// Create a new observability service
    pub fn new(observability_service: Arc<ObsStorageService>) -> Self {
        Self {
            observability_service,
        }
    }

    /// Convert to tonic server
    pub fn into_server(self) -> ObservabilityServiceServer<Self> {
        ObservabilityServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl ObservabilityService for ObservabilityServiceImpl {
    async fn create_namespace(
        &self,
        request: Request<proximadb_v1::CreateObservabilityNamespaceRequest>,
    ) -> Result<Response<proximadb_v1::CreateObservabilityNamespaceResponse>, Status> {
        let req = request.into_inner();
        let config = req
            .config
            .ok_or_else(|| Status::invalid_argument("Missing config"))?;

        match self.observability_service.create_namespace(config).await {
            Ok(name) => Ok(Response::new(
                proximadb_v1::CreateObservabilityNamespaceResponse {
                    namespace_id: name,
                    success: true,
                },
            )),
            Err(e) => Err(Status::internal(format!(
                "Failed to create namespace: {}",
                e
            ))),
        }
    }

    async fn list_namespaces(
        &self,
        _request: Request<proximadb_v1::ListNamespacesRequest>,
    ) -> Result<Response<proximadb_v1::ListNamespacesResponse>, Status> {
        let namespaces = self.observability_service.list_namespaces().await;

        let namespace_infos: Vec<proximadb_v1::NamespaceInfo> = namespaces
            .into_iter()
            .map(|ns| proximadb_v1::NamespaceInfo {
                name: ns.name,
                retention: None, // Retention config not tracked in simple NamespaceInfo
                log_count: ns.total_events, // Approximate - using total_events
                metric_count: 0, // Not tracked separately
                trace_count: 0,  // Not tracked separately
            })
            .collect();

        Ok(Response::new(proximadb_v1::ListNamespacesResponse {
            namespaces: namespace_infos,
        }))
    }

    async fn delete_namespace(
        &self,
        request: Request<proximadb_v1::DeleteNamespaceRequest>,
    ) -> Result<Response<proximadb_v1::DeleteNamespaceResponse>, Status> {
        let req = request.into_inner();

        if req.namespace.is_empty() {
            return Err(Status::invalid_argument("Namespace name is required"));
        }

        debug!("Deleting observability namespace: {}", req.namespace);

        match self
            .observability_service
            .delete_namespace(&req.namespace)
            .await
        {
            Ok(()) => Ok(Response::new(proximadb_v1::DeleteNamespaceResponse {
                success: true,
            })),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(Status::not_found(format!(
                        "Namespace '{}' not found",
                        req.namespace
                    )))
                } else {
                    Err(Status::internal(format!(
                        "Failed to delete namespace: {}",
                        e
                    )))
                }
            }
        }
    }

    async fn ingest_logs(
        &self,
        request: Request<proximadb_v1::IngestLogsRequest>,
    ) -> Result<Response<proximadb_v1::IngestLogsResponse>, Status> {
        let req = request.into_inner();

        match self
            .observability_service
            .ingest_logs(&req.namespace, req.logs, None)
            .await
        {
            Ok(result) => Ok(Response::new(proximadb_v1::IngestLogsResponse {
                ingested: result.ingested,
                failed: result.failed,
                errors: result.errors,
                processing_time_ms: result.processing_time_ms,
            })),
            Err(e) => Err(Status::internal(format!("Failed to ingest logs: {}", e))),
        }
    }

    async fn query_logs(
        &self,
        request: Request<proximadb_v1::QueryLogsRequest>,
    ) -> Result<Response<proximadb_v1::QueryLogsResponse>, Status> {
        let req = request.into_inner();

        let params = crate::observability::LogQueryParams {
            start_time_ns: req.start_time_ns,
            end_time_ns: req.end_time_ns,
            query: req.query,
            severities: Vec::new(), // TODO: Convert from proto severities
            services: Vec::new(),
            sources: Vec::new(),
            limit: req.limit,
            cursor: req.cursor,
        };

        match self
            .observability_service
            .query_logs(&req.namespace, params)
            .await
        {
            Ok(result) => Ok(Response::new(proximadb_v1::QueryLogsResponse {
                logs: result.logs,
                next_cursor: result.next_cursor,
                total_matched: result.total_matched.unwrap_or(0),
                query_time_ms: result.query_time_ms,
            })),
            Err(e) => Err(Status::internal(format!("Failed to query logs: {}", e))),
        }
    }

    type StreamLogsStream = ReceiverStream<Result<proximadb_v1::LogEntry, Status>>;

    async fn stream_logs(
        &self,
        request: Request<proximadb_v1::QueryLogsRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        let req = request.into_inner();

        if req.namespace.is_empty() {
            return Err(Status::invalid_argument("Namespace is required"));
        }

        debug!(
            "Starting log stream for namespace: {}, limit: {}",
            req.namespace, req.limit
        );

        // Build query params
        let params = crate::observability::LogQueryParams {
            start_time_ns: req.start_time_ns,
            end_time_ns: req.end_time_ns,
            query: req.query.clone(),
            severities: Vec::new(),
            services: Vec::new(),
            sources: Vec::new(),
            limit: req.limit,
            cursor: req.cursor.clone(),
        };

        // Query logs from the observability service
        let query_result = self
            .observability_service
            .query_logs(&req.namespace, params)
            .await
            .map_err(|e| Status::internal(format!("Failed to query logs: {}", e)))?;

        // Create a channel for streaming
        let (tx, rx) = mpsc::channel(128);

        // Spawn a task to send logs through the channel
        tokio::spawn(async move {
            for log in query_result.logs {
                if tx.send(Ok(log)).await.is_err() {
                    // Receiver dropped, stop sending
                    warn!("Log stream receiver dropped, stopping stream");
                    break;
                }
            }
            // Channel will be closed when tx is dropped
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn ingest_metrics(
        &self,
        request: Request<proximadb_v1::IngestMetricsRequest>,
    ) -> Result<Response<proximadb_v1::IngestMetricsResponse>, Status> {
        let req = request.into_inner();

        match self
            .observability_service
            .ingest_metrics(&req.namespace, req.samples)
            .await
        {
            Ok(result) => Ok(Response::new(proximadb_v1::IngestMetricsResponse {
                ingested: result.ingested,
                failed: result.failed,
                processing_time_ms: result.processing_time_ms,
            })),
            Err(e) => Err(Status::internal(format!("Failed to ingest metrics: {}", e))),
        }
    }

    async fn query_metrics(
        &self,
        request: Request<proximadb_v1::QueryMetricsRequest>,
    ) -> Result<Response<proximadb_v1::QueryMetricsResponse>, Status> {
        let req = request.into_inner();

        if req.namespace.is_empty() {
            return Err(Status::invalid_argument("Namespace is required"));
        }

        if req.metric_name.is_empty() {
            return Err(Status::invalid_argument("Metric name is required"));
        }

        debug!(
            "Querying metrics: namespace={}, metric={}, time_range=[{}, {}]",
            req.namespace, req.metric_name, req.start_time_ns, req.end_time_ns
        );

        match self
            .observability_service
            .query_metrics(
                &req.namespace,
                &req.metric_name,
                req.start_time_ns,
                req.end_time_ns,
                &req.labels,
                req.limit,
            )
            .await
        {
            Ok(result) => Ok(Response::new(proximadb_v1::QueryMetricsResponse {
                samples: result.samples,
                query_time_ms: result.query_time_ms,
            })),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(Status::not_found(format!(
                        "Namespace '{}' not found",
                        req.namespace
                    )))
                } else {
                    Err(Status::internal(format!("Failed to query metrics: {}", e)))
                }
            }
        }
    }

    async fn aggregate_metrics(
        &self,
        request: Request<proximadb_v1::AggregateMetricsRequest>,
    ) -> Result<Response<proximadb_v1::AggregateMetricsResponse>, Status> {
        let req = request.into_inner();

        let params = crate::observability::MetricAggParams {
            metric_name: req.metric_name,
            start_time_ns: req.start_time_ns,
            end_time_ns: req.end_time_ns,
            aggregation: crate::observability::MetricAggregation::Avg, // TODO: Convert from proto
            step_seconds: 60,                                          // Default 1 minute
            label_filters: std::collections::HashMap::new(),
            group_by: req.group_by,
        };

        match self
            .observability_service
            .aggregate_metrics(&req.namespace, params)
            .await
        {
            Ok(result) => {
                // Convert internal types to proto types
                let series: Vec<proximadb_v1::TimeSeriesResult> = result
                    .series
                    .into_iter()
                    .map(|s| proximadb_v1::TimeSeriesResult {
                        labels: s.labels,
                        points: s
                            .points
                            .into_iter()
                            .map(|p| proximadb_v1::DataPoint {
                                timestamp_ns: p.timestamp_ns,
                                value: p.value,
                            })
                            .collect(),
                    })
                    .collect();

                Ok(Response::new(proximadb_v1::AggregateMetricsResponse {
                    series,
                    query_time_ms: result.query_time_ms,
                }))
            }
            Err(e) => Err(Status::internal(format!(
                "Failed to aggregate metrics: {}",
                e
            ))),
        }
    }

    async fn ingest_traces(
        &self,
        request: Request<proximadb_v1::IngestTracesRequest>,
    ) -> Result<Response<proximadb_v1::IngestTracesResponse>, Status> {
        let req = request.into_inner();

        if req.namespace.is_empty() {
            return Err(Status::invalid_argument("Namespace is required"));
        }

        if req.traces.is_empty() {
            return Err(Status::invalid_argument(
                "At least one trace span is required",
            ));
        }

        debug!(
            "Ingesting {} trace spans to namespace: {}",
            req.traces.len(),
            req.namespace
        );

        match self
            .observability_service
            .ingest_traces(&req.namespace, req.traces)
            .await
        {
            Ok(result) => Ok(Response::new(proximadb_v1::IngestTracesResponse {
                ingested: result.ingested,
                failed: result.failed,
                processing_time_ms: result.processing_time_ms,
            })),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(Status::not_found(format!(
                        "Namespace '{}' not found",
                        req.namespace
                    )))
                } else {
                    Err(Status::internal(format!("Failed to ingest traces: {}", e)))
                }
            }
        }
    }

    async fn query_traces(
        &self,
        request: Request<proximadb_v1::QueryTracesRequest>,
    ) -> Result<Response<proximadb_v1::QueryTracesResponse>, Status> {
        let req = request.into_inner();

        if req.namespace.is_empty() {
            return Err(Status::invalid_argument("Namespace is required"));
        }

        debug!(
            "Querying traces: namespace={}, time_range=[{}, {}], trace_id={:?}, service={:?}",
            req.namespace, req.start_time_ns, req.end_time_ns, req.trace_id, req.service
        );

        let params = crate::observability::TraceQueryParams {
            start_time_ns: req.start_time_ns,
            end_time_ns: req.end_time_ns,
            trace_id: req.trace_id,
            service: req.service,
            operation: req.operation,
            min_duration_ns: req.min_duration_ns,
            status: req.status,
            limit: req.limit,
            cursor: req.cursor,
        };

        match self
            .observability_service
            .query_traces(&req.namespace, params)
            .await
        {
            Ok(result) => Ok(Response::new(proximadb_v1::QueryTracesResponse {
                traces: result.traces,
                next_cursor: result.next_cursor,
                query_time_ms: result.query_time_ms,
            })),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(Status::not_found(format!(
                        "Namespace '{}' not found",
                        req.namespace
                    )))
                } else {
                    Err(Status::internal(format!("Failed to query traces: {}", e)))
                }
            }
        }
    }

    async fn get_trace(
        &self,
        request: Request<proximadb_v1::GetTraceRequest>,
    ) -> Result<Response<proximadb_v1::GetTraceResponse>, Status> {
        let req = request.into_inner();

        if req.namespace.is_empty() {
            return Err(Status::invalid_argument("Namespace is required"));
        }

        if req.trace_id.is_empty() {
            return Err(Status::invalid_argument("Trace ID is required"));
        }

        debug!(
            "Getting trace: namespace={}, trace_id={}",
            req.namespace, req.trace_id
        );

        match self
            .observability_service
            .get_trace(&req.namespace, &req.trace_id)
            .await
        {
            Ok(result) => Ok(Response::new(proximadb_v1::GetTraceResponse {
                spans: result.spans,
                complete: result.complete,
            })),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(Status::not_found(format!(
                        "Namespace '{}' not found",
                        req.namespace
                    )))
                } else {
                    Err(Status::internal(format!("Failed to get trace: {}", e)))
                }
            }
        }
    }

    async fn upsert_alert_rule(
        &self,
        _request: Request<proximadb_v1::UpsertAlertRuleRequest>,
    ) -> Result<Response<proximadb_v1::UpsertAlertRuleResponse>, Status> {
        Err(Status::unimplemented("Alert rules not yet implemented"))
    }

    async fn delete_alert_rule(
        &self,
        _request: Request<proximadb_v1::DeleteAlertRuleRequest>,
    ) -> Result<Response<proximadb_v1::DeleteAlertRuleResponse>, Status> {
        Err(Status::unimplemented("Alert rules not yet implemented"))
    }

    async fn list_alerts(
        &self,
        _request: Request<proximadb_v1::ListAlertsRequest>,
    ) -> Result<Response<proximadb_v1::ListAlertsResponse>, Status> {
        Err(Status::unimplemented("Alerts not yet implemented"))
    }
}
