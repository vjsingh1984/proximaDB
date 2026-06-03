// Observability gRPC backend — `ObservabilityPort` implementation.
//
// The tonic `ObservabilityService` wire adapter lives in
// `crates/platform/proximadb-api/src/grpc/v1/observability.rs` and is the only
// type served (built by `GrpcServiceFactory`). This file holds the canonical
// port-side business logic that the adapter delegates to via
// `Arc<dyn proximadb_runtime::ObservabilityPort>`. TD-105 Phase B lifted the
// logic out of the former (dead) tonic `impl ObservabilityService` block so this
// type is now purely a port backend — no tonic service surface.

use std::sync::Arc;
use tracing::debug;

use crate::observability::ObservabilityService as ObsStorageService;
use crate::proto::proximadb_v1;
use proximadb_v1::{
    AggregateMetricsRequest, AggregateMetricsResponse, CreateObservabilityNamespaceRequest,
    CreateObservabilityNamespaceResponse, DeleteAlertRuleRequest, DeleteAlertRuleResponse,
    DeleteNamespaceRequest, DeleteNamespaceResponse, GetTraceRequest, GetTraceResponse,
    IngestLogsRequest, IngestLogsResponse, IngestMetricsRequest, IngestMetricsResponse,
    IngestTracesRequest, IngestTracesResponse, ListAlertsRequest, ListAlertsResponse,
    ListNamespacesRequest, ListNamespacesResponse, LogEntry, QueryLogsRequest, QueryLogsResponse,
    QueryMetricsRequest, QueryMetricsResponse, QueryTracesRequest, QueryTracesResponse,
    UpsertAlertRuleRequest, UpsertAlertRuleResponse,
};

/// Observability port backend — implements [`proximadb_runtime::ObservabilityPort`].
pub struct ObservabilityServiceImpl {
    observability_service: Arc<ObsStorageService>,
}

impl ObservabilityServiceImpl {
    /// Create a new observability port backend.
    pub fn new(observability_service: Arc<ObsStorageService>) -> Self {
        Self {
            observability_service,
        }
    }
}

#[async_trait::async_trait]
impl proximadb_runtime::ObservabilityPort for ObservabilityServiceImpl {
    async fn create_namespace(
        &self,
        request: CreateObservabilityNamespaceRequest,
    ) -> anyhow::Result<CreateObservabilityNamespaceResponse> {
        let config = request
            .config
            .ok_or_else(|| anyhow::anyhow!("Missing config"))?;

        match self.observability_service.create_namespace(config).await {
            Ok(name) => Ok(CreateObservabilityNamespaceResponse {
                namespace_id: name,
                success: true,
            }),
            Err(e) => Err(anyhow::anyhow!("Failed to create namespace: {}", e)),
        }
    }

    async fn list_namespaces(
        &self,
        _request: ListNamespacesRequest,
    ) -> anyhow::Result<ListNamespacesResponse> {
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

        Ok(ListNamespacesResponse {
            namespaces: namespace_infos,
        })
    }

    async fn delete_namespace(
        &self,
        request: DeleteNamespaceRequest,
    ) -> anyhow::Result<DeleteNamespaceResponse> {
        if request.namespace.is_empty() {
            anyhow::bail!("Namespace name is required");
        }

        debug!("Deleting observability namespace: {}", request.namespace);

        match self
            .observability_service
            .delete_namespace(&request.namespace)
            .await
        {
            Ok(()) => Ok(DeleteNamespaceResponse { success: true }),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(anyhow::anyhow!(
                        "Namespace '{}' not found",
                        request.namespace
                    ))
                } else {
                    Err(anyhow::anyhow!("Failed to delete namespace: {}", e))
                }
            }
        }
    }

    async fn ingest_logs(&self, request: IngestLogsRequest) -> anyhow::Result<IngestLogsResponse> {
        match self
            .observability_service
            .ingest_logs(&request.namespace, request.logs, None)
            .await
        {
            Ok(result) => Ok(IngestLogsResponse {
                ingested: result.ingested,
                failed: result.failed,
                errors: result.errors,
                processing_time_ms: result.processing_time_ms,
            }),
            Err(e) => Err(anyhow::anyhow!("Failed to ingest logs: {}", e)),
        }
    }

    async fn query_logs(&self, request: QueryLogsRequest) -> anyhow::Result<QueryLogsResponse> {
        let params = crate::observability::LogQueryParams {
            start_time_ns: request.start_time_ns,
            end_time_ns: request.end_time_ns,
            query: request.query,
            severities: Vec::new(), // Deferred: Convert from proto severities
            services: Vec::new(),
            sources: Vec::new(),
            limit: request.limit,
            cursor: request.cursor,
        };

        match self
            .observability_service
            .query_logs(&request.namespace, params)
            .await
        {
            Ok(result) => Ok(QueryLogsResponse {
                logs: result.logs,
                next_cursor: result.next_cursor,
                total_matched: result.total_matched.unwrap_or(0),
                query_time_ms: result.query_time_ms,
            }),
            Err(e) => Err(anyhow::anyhow!("Failed to query logs: {}", e)),
        }
    }

    async fn stream_logs(&self, request: QueryLogsRequest) -> anyhow::Result<Vec<LogEntry>> {
        // The gRPC adapter wraps the returned Vec in a ReceiverStream; the port
        // surface is a plain batch query, so reuse `query_logs`.
        if request.namespace.is_empty() {
            anyhow::bail!("Namespace is required");
        }
        Ok(self.query_logs(request).await?.logs)
    }

    async fn ingest_metrics(
        &self,
        request: IngestMetricsRequest,
    ) -> anyhow::Result<IngestMetricsResponse> {
        match self
            .observability_service
            .ingest_metrics(&request.namespace, request.samples)
            .await
        {
            Ok(result) => Ok(IngestMetricsResponse {
                ingested: result.ingested,
                failed: result.failed,
                processing_time_ms: result.processing_time_ms,
            }),
            Err(e) => Err(anyhow::anyhow!("Failed to ingest metrics: {}", e)),
        }
    }

    async fn query_metrics(
        &self,
        request: QueryMetricsRequest,
    ) -> anyhow::Result<QueryMetricsResponse> {
        if request.namespace.is_empty() {
            anyhow::bail!("Namespace is required");
        }

        if request.metric_name.is_empty() {
            anyhow::bail!("Metric name is required");
        }

        debug!(
            "Querying metrics: namespace={}, metric={}, time_range=[{}, {}]",
            request.namespace, request.metric_name, request.start_time_ns, request.end_time_ns
        );

        match self
            .observability_service
            .query_metrics(
                &request.namespace,
                &request.metric_name,
                request.start_time_ns,
                request.end_time_ns,
                &request.labels,
                request.limit,
            )
            .await
        {
            Ok(result) => Ok(QueryMetricsResponse {
                samples: result.samples,
                query_time_ms: result.query_time_ms,
            }),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(anyhow::anyhow!(
                        "Namespace '{}' not found",
                        request.namespace
                    ))
                } else {
                    Err(anyhow::anyhow!("Failed to query metrics: {}", e))
                }
            }
        }
    }

    async fn aggregate_metrics(
        &self,
        request: AggregateMetricsRequest,
    ) -> anyhow::Result<AggregateMetricsResponse> {
        let params = crate::observability::MetricAggParams {
            metric_name: request.metric_name,
            start_time_ns: request.start_time_ns,
            end_time_ns: request.end_time_ns,
            aggregation: crate::observability::MetricAggregation::Avg, // Deferred: Convert from proto
            step_seconds: 60,                                          // Default 1 minute
            label_filters: std::collections::HashMap::new(),
            group_by: request.group_by,
        };

        match self
            .observability_service
            .aggregate_metrics(&request.namespace, params)
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

                Ok(AggregateMetricsResponse {
                    series,
                    query_time_ms: result.query_time_ms,
                })
            }
            Err(e) => Err(anyhow::anyhow!("Failed to aggregate metrics: {}", e)),
        }
    }

    async fn ingest_traces(
        &self,
        request: IngestTracesRequest,
    ) -> anyhow::Result<IngestTracesResponse> {
        if request.namespace.is_empty() {
            anyhow::bail!("Namespace is required");
        }

        if request.traces.is_empty() {
            anyhow::bail!("At least one trace span is required");
        }

        debug!(
            "Ingesting {} trace spans to namespace: {}",
            request.traces.len(),
            request.namespace
        );

        match self
            .observability_service
            .ingest_traces(&request.namespace, request.traces)
            .await
        {
            Ok(result) => Ok(IngestTracesResponse {
                ingested: result.ingested,
                failed: result.failed,
                processing_time_ms: result.processing_time_ms,
            }),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(anyhow::anyhow!(
                        "Namespace '{}' not found",
                        request.namespace
                    ))
                } else {
                    Err(anyhow::anyhow!("Failed to ingest traces: {}", e))
                }
            }
        }
    }

    async fn query_traces(
        &self,
        request: QueryTracesRequest,
    ) -> anyhow::Result<QueryTracesResponse> {
        if request.namespace.is_empty() {
            anyhow::bail!("Namespace is required");
        }

        debug!(
            "Querying traces: namespace={}, time_range=[{}, {}], trace_id={:?}, service={:?}",
            request.namespace,
            request.start_time_ns,
            request.end_time_ns,
            request.trace_id,
            request.service
        );

        let params = crate::observability::TraceQueryParams {
            start_time_ns: request.start_time_ns,
            end_time_ns: request.end_time_ns,
            trace_id: request.trace_id,
            service: request.service,
            operation: request.operation,
            min_duration_ns: request.min_duration_ns,
            status: request.status,
            limit: request.limit,
            cursor: request.cursor,
        };

        match self
            .observability_service
            .query_traces(&request.namespace, params)
            .await
        {
            Ok(result) => Ok(QueryTracesResponse {
                traces: result.traces,
                next_cursor: result.next_cursor,
                query_time_ms: result.query_time_ms,
            }),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(anyhow::anyhow!(
                        "Namespace '{}' not found",
                        request.namespace
                    ))
                } else {
                    Err(anyhow::anyhow!("Failed to query traces: {}", e))
                }
            }
        }
    }

    async fn get_trace(&self, request: GetTraceRequest) -> anyhow::Result<GetTraceResponse> {
        if request.namespace.is_empty() {
            anyhow::bail!("Namespace is required");
        }

        if request.trace_id.is_empty() {
            anyhow::bail!("Trace ID is required");
        }

        debug!(
            "Getting trace: namespace={}, trace_id={}",
            request.namespace, request.trace_id
        );

        match self
            .observability_service
            .get_trace(&request.namespace, &request.trace_id)
            .await
        {
            Ok(result) => Ok(GetTraceResponse {
                spans: result.spans,
                complete: result.complete,
            }),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    Err(anyhow::anyhow!(
                        "Namespace '{}' not found",
                        request.namespace
                    ))
                } else {
                    Err(anyhow::anyhow!("Failed to get trace: {}", e))
                }
            }
        }
    }

    async fn upsert_alert_rule(
        &self,
        _request: UpsertAlertRuleRequest,
    ) -> anyhow::Result<UpsertAlertRuleResponse> {
        Err(anyhow::anyhow!("Alert rules not yet implemented"))
    }

    async fn delete_alert_rule(
        &self,
        _request: DeleteAlertRuleRequest,
    ) -> anyhow::Result<DeleteAlertRuleResponse> {
        Err(anyhow::anyhow!("Alert rules not yet implemented"))
    }

    async fn list_alerts(&self, _request: ListAlertsRequest) -> anyhow::Result<ListAlertsResponse> {
        Err(anyhow::anyhow!("Alerts not yet implemented"))
    }
}
