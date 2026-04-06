// OTLP gRPC transport for observability ingestion
//
// Implements an OTLP-compatible gRPC endpoint on port 4317 that accepts
// traces, metrics, and logs using the ProximaDB ObservabilityService proto
// definition. Incoming OTLP data is converted to internal types and forwarded
// to the ObservabilityService for storage.

use std::collections::HashMap;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::observability::ObservabilityService as ObsStorageService;
use crate::proto::proximadb_v1::{
    self, Severity, SpanKind, SpanStatus, SpanStatusCode, SqlValue, TraceData, sql_value,
};

use super::otlp::{OtlpAnyValue, OtlpKeyValue, OtlpResourceSpans, OtlpSpan};

/// OTLP gRPC service that wraps the ObservabilityService proto implementation
///
/// This service listens on port 4317 (standard OTLP gRPC port) and handles
/// ExportTraces, ExportMetrics, and ExportLogs by converting OTLP structures
/// into ProximaDB internal types and delegating to the ObservabilityService.
pub struct OtlpGrpcService {
    /// Shared observability storage service
    observability_service: Arc<ObsStorageService>,
    /// Target namespace for ingested data
    namespace: String,
}

impl OtlpGrpcService {
    /// Create a new OTLP gRPC service
    pub fn new(observability_service: Arc<ObsStorageService>, namespace: String) -> Self {
        Self {
            observability_service,
            namespace,
        }
    }

    /// Start the gRPC server on the given address
    pub async fn serve(self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        use crate::proto::proximadb_v1::observability_service_server::ObservabilityServiceServer;

        let service_impl = OtlpGrpcServiceImpl {
            observability_service: self.observability_service,
            namespace: self.namespace,
        };

        tracing::info!("OTLP gRPC adapter listening on {}", addr);

        tonic::transport::Server::builder()
            .add_service(ObservabilityServiceServer::new(service_impl))
            .serve(addr)
            .await
            .map_err(|e| anyhow::anyhow!("OTLP gRPC server error: {}", e))
    }

    /// Start the gRPC server with graceful shutdown support
    pub async fn serve_with_shutdown(
        self,
        addr: std::net::SocketAddr,
        shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        use crate::proto::proximadb_v1::observability_service_server::ObservabilityServiceServer;

        let service_impl = OtlpGrpcServiceImpl {
            observability_service: self.observability_service,
            namespace: self.namespace,
        };

        tracing::info!("OTLP gRPC adapter listening on {}", addr);

        tonic::transport::Server::builder()
            .add_service(ObservabilityServiceServer::new(service_impl))
            .serve_with_shutdown(addr, async move {
                shutdown_rx.await.ok();
                tracing::info!("OTLP gRPC adapter shutting down");
            })
            .await
            .map_err(|e| anyhow::anyhow!("OTLP gRPC server error: {}", e))
    }
}

/// Internal tonic service implementation that bridges OTLP gRPC to ObservabilityService
struct OtlpGrpcServiceImpl {
    observability_service: Arc<ObsStorageService>,
    namespace: String,
}

impl OtlpGrpcServiceImpl {
    /// Get the namespace, preferring the request namespace if non-empty
    fn resolve_namespace(&self, request_namespace: &str) -> String {
        if request_namespace.is_empty() {
            self.namespace.clone()
        } else {
            request_namespace.to_string()
        }
    }
}

// -- Proto conversion functions --

/// Convert an OTLP span kind integer (proto enum value) to internal SpanKind
pub fn convert_span_kind(kind: i32) -> SpanKind {
    // OTLP SpanKind enum: 0=UNSPECIFIED, 1=INTERNAL, 2=SERVER, 3=CLIENT, 4=PRODUCER, 5=CONSUMER
    match kind {
        1 => SpanKind::Internal,
        2 => SpanKind::Server,
        3 => SpanKind::Client,
        4 => SpanKind::Producer,
        5 => SpanKind::Consumer,
        _ => SpanKind::Internal,
    }
}

/// Convert an OTLP span kind string to internal SpanKind
pub fn convert_span_kind_str(kind: &str) -> SpanKind {
    match kind {
        "SPAN_KIND_INTERNAL" => SpanKind::Internal,
        "SPAN_KIND_SERVER" => SpanKind::Server,
        "SPAN_KIND_CLIENT" => SpanKind::Client,
        "SPAN_KIND_PRODUCER" => SpanKind::Producer,
        "SPAN_KIND_CONSUMER" => SpanKind::Consumer,
        _ => SpanKind::Internal,
    }
}

/// Convert OTLP status code integer to internal SpanStatusCode
pub fn convert_status_code(code: i32) -> SpanStatusCode {
    // OTLP StatusCode enum: 0=UNSET, 1=OK, 2=ERROR
    match code {
        0 => SpanStatusCode::Unset,
        1 => SpanStatusCode::Ok,
        2 => SpanStatusCode::Error,
        _ => SpanStatusCode::Unset,
    }
}

/// Convert OTLP status code string to internal SpanStatusCode
pub fn convert_status_code_str(code: &str) -> SpanStatusCode {
    match code {
        "STATUS_CODE_OK" => SpanStatusCode::Ok,
        "STATUS_CODE_ERROR" => SpanStatusCode::Error,
        _ => SpanStatusCode::Unset,
    }
}

/// Convert OTLP severity number to internal Severity
///
/// OTLP severity numbers: 1-4 = TRACE, 5-8 = DEBUG, 9-12 = INFO,
/// 13-16 = WARN, 17-20 = ERROR, 21-24 = FATAL
pub fn convert_severity(severity_number: i32) -> Severity {
    match severity_number {
        1..=4 => Severity::Debug,   // TRACE
        5..=8 => Severity::Debug,   // DEBUG
        9..=12 => Severity::Info,   // INFO
        13..=16 => Severity::Warn,  // WARN
        17..=20 => Severity::Error, // ERROR
        21..=24 => Severity::Fatal, // FATAL
        _ => Severity::Info,
    }
}

/// Convert OTLP AnyValue (JSON-style) to SqlValue
pub fn convert_any_value(value: &OtlpAnyValue) -> Option<SqlValue> {
    Some(SqlValue {
        value: Some(if let Some(s) = &value.string_value {
            sql_value::Value::StringValue(s.clone())
        } else if let Some(b) = value.bool_value {
            sql_value::Value::BoolValue(b)
        } else if let Some(i) = value.int_value {
            sql_value::Value::Int64Value(i)
        } else if let Some(f) = value.double_value {
            sql_value::Value::NumberValue(f)
        } else if let Some(b) = &value.bytes_value {
            sql_value::Value::StringValue(b.clone())
        } else if let Some(a) = &value.array_value {
            let json_str = serde_json::to_string(&a.values).ok()?;
            sql_value::Value::StringValue(json_str)
        } else if let Some(kvl) = &value.kvlist_value {
            let json_str = serde_json::to_string(&kvl.values).ok()?;
            sql_value::Value::StringValue(json_str)
        } else {
            return None;
        }),
    })
}

/// Convert OTLP key-value pairs to a HashMap of resource attributes (string values)
pub fn extract_resource_attributes(attributes: &[OtlpKeyValue]) -> HashMap<String, String> {
    attributes
        .iter()
        .filter_map(|kv| {
            let value = if let Some(s) = &kv.value.string_value {
                s.clone()
            } else if let Some(b) = kv.value.bool_value {
                b.to_string()
            } else if let Some(i) = kv.value.int_value {
                i.to_string()
            } else if let Some(f) = kv.value.double_value {
                f.to_string()
            } else if let Some(b) = &kv.value.bytes_value {
                b.clone()
            } else {
                return None;
            };
            Some((kv.key.clone(), value))
        })
        .collect()
}

/// Convert an OTLP span (JSON-style) to internal TraceData
pub fn convert_otlp_span(
    otlp_span: &OtlpSpan,
    resource_attributes: &HashMap<String, String>,
) -> TraceData {
    let start_time_ns = otlp_span.start_time_unix_nano.parse::<i64>().unwrap_or(0);
    let end_time_ns = otlp_span.end_time_unix_nano.parse::<i64>().unwrap_or(0);

    let kind = match otlp_span.kind.as_deref() {
        Some(s) => convert_span_kind_str(s),
        None => SpanKind::Internal,
    };

    let status = otlp_span.status.as_ref().map(|s| SpanStatus {
        code: convert_status_code_str(&s.code) as i32,
        message: if s.message.is_empty() {
            None
        } else {
            Some(s.message.clone())
        },
    });

    // Build attributes: resource attrs first, then span attrs
    let mut attributes = HashMap::new();
    for (k, v) in resource_attributes {
        attributes.insert(
            k.clone(),
            SqlValue {
                value: Some(sql_value::Value::StringValue(v.clone())),
            },
        );
    }
    for kv in &otlp_span.attributes {
        if let Some(value) = convert_any_value(&kv.value) {
            attributes.insert(kv.key.clone(), value);
        }
    }

    TraceData {
        trace_id: otlp_span.trace_id.clone(),
        span_id: otlp_span.span_id.clone(),
        parent_span_id: otlp_span.parent_span_id.clone(),
        name: otlp_span.name.clone(),
        kind: kind as i32,
        start_time_ns,
        end_time_ns,
        status,
        attributes,
        events: otlp_span
            .events
            .iter()
            .map(|e| crate::proto::proximadb_v1::SpanEvent {
                timestamp_ns: e.time_unix_nano.parse::<i64>().unwrap_or(0),
                name: e.name.clone(),
                attributes: e
                    .attributes
                    .iter()
                    .filter_map(|kv| convert_any_value(&kv.value).map(|v| (kv.key.clone(), v)))
                    .collect(),
            })
            .collect(),
        links: otlp_span
            .links
            .iter()
            .map(|l| crate::proto::proximadb_v1::SpanLink {
                trace_id: l.trace_id.clone(),
                span_id: l.span_id.clone(),
                attributes: l
                    .attributes
                    .iter()
                    .filter_map(|kv| convert_any_value(&kv.value).map(|v| (kv.key.clone(), v)))
                    .collect(),
            })
            .collect(),
    }
}

/// Convert OTLP ResourceSpans into a vector of internal TraceData
pub fn convert_resource_spans(resource_spans: &[OtlpResourceSpans]) -> Vec<TraceData> {
    let mut traces = Vec::new();
    for rs in resource_spans {
        let resource_attributes = rs
            .resource
            .as_ref()
            .map(|r| extract_resource_attributes(&r.attributes))
            .unwrap_or_default();

        for scope_span in &rs.scope_spans {
            for otlp_span in &scope_span.spans {
                traces.push(convert_otlp_span(otlp_span, &resource_attributes));
            }
        }
    }
    traces
}

// -- tonic::async_trait implementation for ObservabilityService --

use crate::proto::proximadb_v1::observability_service_server::ObservabilityService;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

#[tonic::async_trait]
impl ObservabilityService for OtlpGrpcServiceImpl {
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
                retention: None,
                log_count: ns.total_events,
                metric_count: 0,
                trace_count: 0,
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
        let namespace = self.resolve_namespace(&req.namespace);

        match self
            .observability_service
            .delete_namespace(&namespace)
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
                        namespace
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
        let namespace = self.resolve_namespace(&req.namespace);

        match self
            .observability_service
            .ingest_logs(&namespace, req.logs, None)
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
        let namespace = self.resolve_namespace(&req.namespace);

        let params = crate::observability::LogQueryParams {
            start_time_ns: req.start_time_ns,
            end_time_ns: req.end_time_ns,
            query: req.query,
            severities: Vec::new(),
            services: Vec::new(),
            sources: Vec::new(),
            limit: req.limit,
            cursor: req.cursor,
        };

        match self
            .observability_service
            .query_logs(&namespace, params)
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
        let namespace = self.resolve_namespace(&req.namespace);

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

        let query_result = self
            .observability_service
            .query_logs(&namespace, params)
            .await
            .map_err(|e| Status::internal(format!("Failed to query logs: {}", e)))?;

        let (tx, rx) = mpsc::channel(128);

        tokio::spawn(async move {
            for log in query_result.logs {
                if tx.send(Ok(log)).await.is_err() {
                    tracing::warn!("OTLP gRPC log stream receiver dropped");
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn ingest_metrics(
        &self,
        request: Request<proximadb_v1::IngestMetricsRequest>,
    ) -> Result<Response<proximadb_v1::IngestMetricsResponse>, Status> {
        let req = request.into_inner();
        let namespace = self.resolve_namespace(&req.namespace);

        match self
            .observability_service
            .ingest_metrics(&namespace, req.samples)
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
        let namespace = self.resolve_namespace(&req.namespace);

        match self
            .observability_service
            .query_metrics(
                &namespace,
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
                        namespace
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
        let namespace = self.resolve_namespace(&req.namespace);

        let params = crate::observability::MetricAggParams {
            metric_name: req.metric_name,
            start_time_ns: req.start_time_ns,
            end_time_ns: req.end_time_ns,
            aggregation: crate::observability::MetricAggregation::Avg,
            step_seconds: 60,
            label_filters: std::collections::HashMap::new(),
            group_by: req.group_by,
        };

        match self
            .observability_service
            .aggregate_metrics(&namespace, params)
            .await
        {
            Ok(result) => {
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
        let namespace = self.resolve_namespace(&req.namespace);

        if req.traces.is_empty() {
            return Err(Status::invalid_argument(
                "At least one trace span is required",
            ));
        }

        tracing::debug!(
            "OTLP gRPC: Ingesting {} trace spans to namespace: {}",
            req.traces.len(),
            namespace
        );

        match self
            .observability_service
            .ingest_traces(&namespace, req.traces)
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
                        namespace
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
        let namespace = self.resolve_namespace(&req.namespace);

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
            .query_traces(&namespace, params)
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
                        namespace
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
        let namespace = self.resolve_namespace(&req.namespace);

        if req.trace_id.is_empty() {
            return Err(Status::invalid_argument("Trace ID is required"));
        }

        match self
            .observability_service
            .get_trace(&namespace, &req.trace_id)
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
                        namespace
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_span_kind() {
        assert!(matches!(convert_span_kind(0), SpanKind::Internal));
        assert!(matches!(convert_span_kind(1), SpanKind::Internal));
        assert!(matches!(convert_span_kind(2), SpanKind::Server));
        assert!(matches!(convert_span_kind(3), SpanKind::Client));
        assert!(matches!(convert_span_kind(4), SpanKind::Producer));
        assert!(matches!(convert_span_kind(5), SpanKind::Consumer));
        assert!(matches!(convert_span_kind(99), SpanKind::Internal));
    }

    #[test]
    fn test_convert_span_kind_str() {
        assert!(matches!(
            convert_span_kind_str("SPAN_KIND_SERVER"),
            SpanKind::Server
        ));
        assert!(matches!(
            convert_span_kind_str("SPAN_KIND_CLIENT"),
            SpanKind::Client
        ));
        assert!(matches!(
            convert_span_kind_str("SPAN_KIND_PRODUCER"),
            SpanKind::Producer
        ));
        assert!(matches!(
            convert_span_kind_str("SPAN_KIND_CONSUMER"),
            SpanKind::Consumer
        ));
        assert!(matches!(
            convert_span_kind_str("SPAN_KIND_INTERNAL"),
            SpanKind::Internal
        ));
        assert!(matches!(
            convert_span_kind_str("unknown"),
            SpanKind::Internal
        ));
    }

    #[test]
    fn test_convert_status_code() {
        assert!(matches!(convert_status_code(0), SpanStatusCode::Unset));
        assert!(matches!(convert_status_code(1), SpanStatusCode::Ok));
        assert!(matches!(convert_status_code(2), SpanStatusCode::Error));
        assert!(matches!(convert_status_code(99), SpanStatusCode::Unset));
    }

    #[test]
    fn test_convert_status_code_str() {
        assert!(matches!(
            convert_status_code_str("STATUS_CODE_UNSET"),
            SpanStatusCode::Unset
        ));
        assert!(matches!(
            convert_status_code_str("STATUS_CODE_OK"),
            SpanStatusCode::Ok
        ));
        assert!(matches!(
            convert_status_code_str("STATUS_CODE_ERROR"),
            SpanStatusCode::Error
        ));
        assert!(matches!(
            convert_status_code_str("anything_else"),
            SpanStatusCode::Unset
        ));
    }

    #[test]
    fn test_convert_severity() {
        // TRACE range
        assert!(matches!(convert_severity(1), Severity::Debug));
        assert!(matches!(convert_severity(4), Severity::Debug));
        // DEBUG range
        assert!(matches!(convert_severity(5), Severity::Debug));
        assert!(matches!(convert_severity(8), Severity::Debug));
        // INFO range
        assert!(matches!(convert_severity(9), Severity::Info));
        assert!(matches!(convert_severity(12), Severity::Info));
        // WARN range
        assert!(matches!(convert_severity(13), Severity::Warn));
        assert!(matches!(convert_severity(16), Severity::Warn));
        // ERROR range
        assert!(matches!(convert_severity(17), Severity::Error));
        assert!(matches!(convert_severity(20), Severity::Error));
        // FATAL range
        assert!(matches!(convert_severity(21), Severity::Fatal));
        assert!(matches!(convert_severity(24), Severity::Fatal));
        // Default
        assert!(matches!(convert_severity(0), Severity::Info));
        assert!(matches!(convert_severity(99), Severity::Info));
    }

    #[test]
    fn test_convert_any_value_string() {
        let value = OtlpAnyValue {
            string_value: Some("hello".to_string()),
            bool_value: None,
            int_value: None,
            double_value: None,
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        let result = convert_any_value(&value);
        assert!(result.is_some());
        let sql_val = result.unwrap();
        assert!(matches!(
            sql_val.value,
            Some(sql_value::Value::StringValue(ref s)) if s == "hello"
        ));
    }

    #[test]
    fn test_convert_any_value_bool() {
        let value = OtlpAnyValue {
            string_value: None,
            bool_value: Some(true),
            int_value: None,
            double_value: None,
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        let result = convert_any_value(&value);
        assert!(result.is_some());
        let sql_val = result.unwrap();
        assert!(matches!(
            sql_val.value,
            Some(sql_value::Value::BoolValue(true))
        ));
    }

    #[test]
    fn test_convert_any_value_int() {
        let value = OtlpAnyValue {
            string_value: None,
            bool_value: None,
            int_value: Some(42),
            double_value: None,
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        let result = convert_any_value(&value);
        assert!(result.is_some());
        let sql_val = result.unwrap();
        assert!(matches!(
            sql_val.value,
            Some(sql_value::Value::Int64Value(42))
        ));
    }

    #[test]
    fn test_convert_any_value_double() {
        let value = OtlpAnyValue {
            string_value: None,
            bool_value: None,
            int_value: None,
            double_value: Some(3.14),
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        let result = convert_any_value(&value);
        assert!(result.is_some());
        let sql_val = result.unwrap();
        assert!(matches!(
            sql_val.value,
            Some(sql_value::Value::NumberValue(f)) if (f - 3.14).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn test_convert_any_value_none() {
        let value = OtlpAnyValue {
            string_value: None,
            bool_value: None,
            int_value: None,
            double_value: None,
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        let result = convert_any_value(&value);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_resource_attributes() {
        let attrs = vec![
            OtlpKeyValue {
                key: "service.name".to_string(),
                value: OtlpAnyValue {
                    string_value: Some("my-service".to_string()),
                    bool_value: None,
                    int_value: None,
                    double_value: None,
                    array_value: None,
                    kvlist_value: None,
                    bytes_value: None,
                },
            },
            OtlpKeyValue {
                key: "host.name".to_string(),
                value: OtlpAnyValue {
                    string_value: Some("localhost".to_string()),
                    bool_value: None,
                    int_value: None,
                    double_value: None,
                    array_value: None,
                    kvlist_value: None,
                    bytes_value: None,
                },
            },
            OtlpKeyValue {
                key: "count".to_string(),
                value: OtlpAnyValue {
                    string_value: None,
                    bool_value: None,
                    int_value: Some(5),
                    double_value: None,
                    array_value: None,
                    kvlist_value: None,
                    bytes_value: None,
                },
            },
        ];

        let result = extract_resource_attributes(&attrs);
        assert_eq!(result.len(), 3);
        assert_eq!(result.get("service.name").unwrap(), "my-service");
        assert_eq!(result.get("host.name").unwrap(), "localhost");
        assert_eq!(result.get("count").unwrap(), "5");
    }

    #[test]
    fn test_convert_otlp_span_basic() {
        use super::super::otlp::{OtlpSpan, OtlpStatus};

        let span = OtlpSpan {
            trace_id: "abc123".to_string(),
            span_id: "def456".to_string(),
            parent_span_id: Some("parent789".to_string()),
            trace_state: None,
            name: "test-operation".to_string(),
            kind: Some("SPAN_KIND_SERVER".to_string()),
            start_time_unix_nano: "1000000000".to_string(),
            end_time_unix_nano: "2000000000".to_string(),
            attributes: vec![],
            events: vec![],
            links: vec![],
            status: Some(OtlpStatus {
                code: "STATUS_CODE_OK".to_string(),
                message: String::new(),
            }),
        };

        let resource_attrs = HashMap::new();
        let trace_data = convert_otlp_span(&span, &resource_attrs);

        assert_eq!(trace_data.trace_id, "abc123");
        assert_eq!(trace_data.span_id, "def456");
        assert_eq!(trace_data.parent_span_id, Some("parent789".to_string()));
        assert_eq!(trace_data.name, "test-operation");
        assert_eq!(trace_data.kind, SpanKind::Server as i32);
        assert_eq!(trace_data.start_time_ns, 1_000_000_000);
        assert_eq!(trace_data.end_time_ns, 2_000_000_000);
        assert!(trace_data.status.is_some());
        assert_eq!(
            trace_data.status.as_ref().unwrap().code,
            SpanStatusCode::Ok as i32
        );
    }

    #[test]
    fn test_convert_otlp_span_with_resource_attributes() {
        use super::super::otlp::OtlpSpan;

        let span = OtlpSpan {
            trace_id: "trace1".to_string(),
            span_id: "span1".to_string(),
            parent_span_id: None,
            trace_state: None,
            name: "op".to_string(),
            kind: None,
            start_time_unix_nano: "0".to_string(),
            end_time_unix_nano: "0".to_string(),
            attributes: vec![OtlpKeyValue {
                key: "http.method".to_string(),
                value: OtlpAnyValue {
                    string_value: Some("GET".to_string()),
                    bool_value: None,
                    int_value: None,
                    double_value: None,
                    array_value: None,
                    kvlist_value: None,
                    bytes_value: None,
                },
            }],
            events: vec![],
            links: vec![],
            status: None,
        };

        let mut resource_attrs = HashMap::new();
        resource_attrs.insert("service.name".to_string(), "test-svc".to_string());

        let trace_data = convert_otlp_span(&span, &resource_attrs);

        // Should have both resource and span attributes
        assert_eq!(trace_data.attributes.len(), 2);
        assert!(trace_data.attributes.contains_key("service.name"));
        assert!(trace_data.attributes.contains_key("http.method"));
    }

    #[test]
    fn test_convert_resource_spans_empty() {
        let result = convert_resource_spans(&[]);
        assert!(result.is_empty());
    }
}
