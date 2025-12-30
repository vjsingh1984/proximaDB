// Observability gRPC service implementation
//
// Implements the ObservabilityService defined in proto/proximadb/v1/observability.proto
// This is a stub implementation that will be filled in as the observability module is completed.

use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

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
        // TODO: Implement when list_namespaces is available on ObservabilityService
        Ok(Response::new(proximadb_v1::ListNamespacesResponse {
            namespaces: Vec::new(),
        }))
    }

    async fn delete_namespace(
        &self,
        _request: Request<proximadb_v1::DeleteNamespaceRequest>,
    ) -> Result<Response<proximadb_v1::DeleteNamespaceResponse>, Status> {
        // TODO: Implement when delete_namespace is available on ObservabilityService
        Err(Status::unimplemented("Delete namespace not yet implemented"))
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

        match self.observability_service.query_logs(&req.namespace, params).await {
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
        _request: Request<proximadb_v1::QueryLogsRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        Err(Status::unimplemented("Log streaming not yet implemented"))
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
        _request: Request<proximadb_v1::QueryMetricsRequest>,
    ) -> Result<Response<proximadb_v1::QueryMetricsResponse>, Status> {
        // TODO: Implement when query_metrics is available on ObservabilityService
        Err(Status::unimplemented("Query metrics not yet implemented"))
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
            step_seconds: 60, // Default 1 minute
            label_filters: std::collections::HashMap::new(),
            group_by: req.group_by,
        };

        match self.observability_service.aggregate_metrics(&req.namespace, params).await {
            Ok(result) => {
                // Convert internal types to proto types
                let series: Vec<proximadb_v1::TimeSeriesResult> = result
                    .series
                    .into_iter()
                    .map(|s| proximadb_v1::TimeSeriesResult {
                        labels: s.labels,
                        points: s.points.into_iter().map(|p| proximadb_v1::DataPoint {
                            timestamp_ns: p.timestamp_ns,
                            value: p.value,
                        }).collect(),
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
        _request: Request<proximadb_v1::IngestTracesRequest>,
    ) -> Result<Response<proximadb_v1::IngestTracesResponse>, Status> {
        // TODO: Implement when ingest_traces is available on ObservabilityService
        Err(Status::unimplemented("Trace ingestion not yet implemented"))
    }

    async fn query_traces(
        &self,
        _request: Request<proximadb_v1::QueryTracesRequest>,
    ) -> Result<Response<proximadb_v1::QueryTracesResponse>, Status> {
        Err(Status::unimplemented("Trace query not yet implemented"))
    }

    async fn get_trace(
        &self,
        _request: Request<proximadb_v1::GetTraceRequest>,
    ) -> Result<Response<proximadb_v1::GetTraceResponse>, Status> {
        Err(Status::unimplemented("Get trace not yet implemented"))
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
