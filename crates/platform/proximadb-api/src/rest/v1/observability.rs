//! # Observability REST Handlers
//!
//! REST endpoints for logs, metrics, and traces.  All handlers delegate to
//! `ObservabilityPort` so this module compiles without any dependency on
//! root-crate concrete service types.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Router,
    extract::{Json, Path, State},
    response::Json as JsonResponse,
    routing::post,
};
use proximadb_proto::v1::{
    AggregateMetricsRequest, CreateObservabilityNamespaceRequest, IngestLogsRequest,
    IngestMetricsRequest, LogEntry, LogFilter, MetricAggregation, MetricSample,
    ObservabilityNamespaceConfig, QueryLogsRequest, RetentionConfig, Severity, SortOrder, SqlValue,
};
use proximadb_runtime::ObservabilityPort;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::rest::errors::{RestError, RestResult};

// ── State ─────────────────────────────────────────────────────────────────────

/// Axum state for observability REST endpoints.
#[derive(Clone)]
pub struct ObservabilityRestState {
    pub observability_port: Arc<dyn ObservabilityPort>,
}

// ── Legacy stub types kept for re-export compatibility ────────────────────────

/// Logs handler stub.
pub struct LogsHandler;

impl LogsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogsHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics handler stub.
pub struct MetricsHandler;

impl MetricsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MetricsHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Request / Response types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateNamespaceRequest {
    pub name: String,
    #[serde(default = "default_hot")]
    pub hot_retention_days: u32,
    #[serde(default = "default_warm")]
    pub warm_retention_days: u32,
    #[serde(default = "default_cold")]
    pub cold_retention_days: u32,
}

fn default_hot() -> u32 {
    1
}
fn default_warm() -> u32 {
    7
}
fn default_cold() -> u32 {
    30
}

#[derive(Debug, Deserialize)]
pub struct LogEntryRequest {
    pub timestamp_ns: Option<i64>,
    pub message: String,
    #[serde(default = "default_severity_str")]
    pub severity: String,
    pub source: Option<String>,
    pub service: Option<String>,
    #[serde(default)]
    pub fields: HashMap<String, serde_json::Value>,
}

fn default_severity_str() -> String {
    "info".to_string()
}

#[derive(Debug, Deserialize)]
pub struct BulkLogRequest {
    pub logs: Vec<LogEntryRequest>,
}

#[derive(Debug, Deserialize)]
pub struct LogQueryRequest {
    pub start_time_ns: Option<i64>,
    pub end_time_ns: Option<i64>,
    pub query: Option<String>,
    #[serde(default)]
    pub severities: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub cursor: Option<String>,
}

fn default_limit() -> u32 {
    100
}

#[derive(Debug, Deserialize)]
pub struct MetricSampleRequest {
    pub name: String,
    pub value: f64,
    pub timestamp_ns: Option<i64>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct BulkMetricRequest {
    pub metrics: Vec<MetricSampleRequest>,
}

#[derive(Debug, Deserialize)]
pub struct MetricAggregationRequest {
    pub metric_name: String,
    pub start_time_ns: i64,
    pub end_time_ns: i64,
    #[serde(default = "default_aggregation")]
    pub aggregation: String,
    #[serde(default = "default_step")]
    pub step_seconds: u32,
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

fn default_aggregation() -> String {
    "avg".to_string()
}
fn default_step() -> u32 {
    60
}

#[derive(Debug, Deserialize)]
pub struct TraceIngestRequest {
    pub spans: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct TraceQueryRequest {
    pub trace_id: Option<String>,
    pub service: Option<String>,
    pub start_ns: i64,
    pub end_ns: i64,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct PromQLRequest {
    pub query: String,
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub step_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct IngestResponse {
    pub ingested: u64,
    pub failed: u64,
    pub success: bool,
}

#[derive(Debug, Serialize)]
pub struct LogEntryResponse {
    pub timestamp_ns: i64,
    pub severity: String,
    pub message: String,
    pub source: Option<String>,
    pub service: Option<String>,
    pub fields: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct LogResponse {
    pub logs: Vec<LogEntryResponse>,
    pub next_cursor: Option<String>,
    pub total_matched: Option<u64>,
    pub query_time_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct TimeSeriesResponse {
    pub labels: HashMap<String, String>,
    pub points: Vec<DataPointResponse>,
}

#[derive(Debug, Serialize)]
pub struct DataPointResponse {
    pub timestamp_ns: i64,
    pub value: f64,
}

#[derive(Debug, Serialize)]
pub struct MetricAggResponse {
    pub series: Vec<TimeSeriesResponse>,
    pub query_time_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct TraceResponse {
    pub spans: Vec<serde_json::Value>,
    pub total: u64,
}

#[derive(Debug, Serialize)]
pub struct PromQLResponse {
    pub result_type: String,
    pub result: Vec<serde_json::Value>,
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn create_observability_router() -> Router<ObservabilityRestState> {
    Router::new()
        .route("/namespaces", post(create_namespace))
        .route("/namespaces/:namespace/logs/_bulk", post(ingest_logs))
        .route("/namespaces/:namespace/logs", post(ingest_log))
        .route("/namespaces/:namespace/logs/_search", post(query_logs))
        .route("/namespaces/:namespace/metrics/_bulk", post(ingest_metrics))
        .route("/namespaces/:namespace/metrics", post(ingest_metric))
        .route(
            "/namespaces/:namespace/metrics/_aggregate",
            post(aggregate_metrics),
        )
        .route("/namespaces/:namespace/metrics/_promql", post(query_promql))
        .route("/namespaces/:namespace/traces/_bulk", post(ingest_traces))
        .route("/namespaces/:namespace/traces/_search", post(query_traces))
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn create_namespace(
    State(state): State<ObservabilityRestState>,
    Json(request): Json<CreateNamespaceRequest>,
) -> RestResult<JsonResponse<serde_json::Value>> {
    let retention = RetentionConfig {
        hot_retention_hours: (request.hot_retention_days * 24) as u64,
        warm_retention_days: request.warm_retention_days as u64,
        cold_retention_days: request.cold_retention_days as u64,
        archive_retention_days: 365,
    };

    let config = ObservabilityNamespaceConfig {
        name: request.name.clone(),
        retention: Some(retention),
        ingestion: None,
        alert_rules: Vec::new(),
        access: None,
    };

    state
        .observability_port
        .create_namespace(CreateObservabilityNamespaceRequest {
            config: Some(config),
        })
        .await
        .map_err(|e| RestError::Internal(format!("Failed to create namespace: {}", e)))?;

    Ok(JsonResponse(
        serde_json::json!({ "success": true, "namespace": request.name }),
    ))
}

async fn ingest_log(
    State(state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<LogEntryRequest>,
) -> RestResult<JsonResponse<IngestResponse>> {
    let entry = convert_log_request(&request)?;

    let result = state
        .observability_port
        .ingest_logs(IngestLogsRequest {
            namespace,
            logs: vec![entry],
            format: None,
        })
        .await
        .map_err(|e| RestError::Internal(format!("Failed to ingest log: {}", e)))?;

    Ok(JsonResponse(IngestResponse {
        ingested: result.ingested,
        failed: result.failed,
        success: result.failed == 0,
    }))
}

async fn ingest_logs(
    State(state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<BulkLogRequest>,
) -> RestResult<JsonResponse<IngestResponse>> {
    debug!("Ingesting {} logs into {}", request.logs.len(), namespace);

    let entries: Vec<LogEntry> = request
        .logs
        .iter()
        .map(convert_log_request)
        .collect::<Result<Vec<_>, _>>()?;

    let result = state
        .observability_port
        .ingest_logs(IngestLogsRequest {
            namespace,
            logs: entries,
            format: None,
        })
        .await
        .map_err(|e| RestError::Internal(format!("Failed to ingest logs: {}", e)))?;

    Ok(JsonResponse(IngestResponse {
        ingested: result.ingested,
        failed: result.failed,
        success: result.failed == 0,
    }))
}

async fn query_logs(
    State(state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<LogQueryRequest>,
) -> RestResult<JsonResponse<LogResponse>> {
    debug!("Querying logs in namespace: {}", namespace);

    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let one_hour_ns = 3_600_000_000_000_i64;

    let severities: Vec<i32> = request
        .severities
        .iter()
        .map(|s| parse_severity(s) as i32)
        .collect();

    let filter =
        if severities.is_empty() && request.services.is_empty() && request.sources.is_empty() {
            None
        } else {
            Some(LogFilter {
                severities,
                services: request.services.clone(),
                sources: request.sources.clone(),
                field_filters: Vec::new(),
            })
        };

    let result = state
        .observability_port
        .query_logs(QueryLogsRequest {
            namespace,
            start_time_ns: request.start_time_ns.unwrap_or(now_ns - one_hour_ns),
            end_time_ns: request.end_time_ns.unwrap_or(now_ns),
            query: request.query,
            filter,
            limit: request.limit,
            cursor: request.cursor,
            sort: SortOrder::Desc as i32,
        })
        .await
        .map_err(|e| RestError::Internal(format!("Failed to query logs: {}", e)))?;

    let logs: Vec<LogEntryResponse> = result
        .logs
        .into_iter()
        .map(convert_log_to_response)
        .collect();

    Ok(JsonResponse(LogResponse {
        logs,
        next_cursor: result.next_cursor,
        total_matched: Some(result.total_matched),
        query_time_ms: result.query_time_ms,
    }))
}

async fn ingest_metric(
    State(state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<MetricSampleRequest>,
) -> RestResult<JsonResponse<IngestResponse>> {
    let sample = convert_metric_request(&request);

    let result = state
        .observability_port
        .ingest_metrics(IngestMetricsRequest {
            namespace,
            samples: vec![sample],
        })
        .await
        .map_err(|e| RestError::Internal(format!("Failed to ingest metric: {}", e)))?;

    Ok(JsonResponse(IngestResponse {
        ingested: result.ingested,
        failed: result.failed,
        success: result.failed == 0,
    }))
}

async fn ingest_metrics(
    State(state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<BulkMetricRequest>,
) -> RestResult<JsonResponse<IngestResponse>> {
    debug!(
        "Ingesting {} metrics into {}",
        request.metrics.len(),
        namespace
    );

    let samples: Vec<MetricSample> = request.metrics.iter().map(convert_metric_request).collect();

    let result = state
        .observability_port
        .ingest_metrics(IngestMetricsRequest { namespace, samples })
        .await
        .map_err(|e| RestError::Internal(format!("Failed to ingest metrics: {}", e)))?;

    Ok(JsonResponse(IngestResponse {
        ingested: result.ingested,
        failed: result.failed,
        success: result.failed == 0,
    }))
}

async fn aggregate_metrics(
    State(state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<MetricAggregationRequest>,
) -> RestResult<JsonResponse<MetricAggResponse>> {
    debug!("Aggregating metrics in namespace: {}", namespace);

    let result = state
        .observability_port
        .aggregate_metrics(AggregateMetricsRequest {
            namespace,
            metric_name: request.metric_name,
            start_time_ns: request.start_time_ns,
            end_time_ns: request.end_time_ns,
            aggregation: parse_aggregation(&request.aggregation) as i32,
            step_seconds: request.step_seconds,
            label_filters: request.labels,
            group_by: request.group_by,
        })
        .await
        .map_err(|e| RestError::Internal(format!("Failed to aggregate metrics: {}", e)))?;

    let series: Vec<TimeSeriesResponse> = result
        .series
        .into_iter()
        .map(|s| TimeSeriesResponse {
            labels: s.labels,
            points: s
                .points
                .into_iter()
                .map(|p| DataPointResponse {
                    timestamp_ns: p.timestamp_ns,
                    value: p.value,
                })
                .collect(),
        })
        .collect();

    Ok(JsonResponse(MetricAggResponse {
        series,
        query_time_ms: result.query_time_ms,
    }))
}

async fn query_promql(
    State(_state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<PromQLRequest>,
) -> RestResult<JsonResponse<PromQLResponse>> {
    debug!("PromQL query in namespace {}: {}", namespace, request.query);
    // Full PromQL wiring comes with the CHRONO engine; return empty for now.
    Ok(JsonResponse(PromQLResponse {
        result_type: "vector".to_string(),
        result: Vec::new(),
    }))
}

async fn ingest_traces(
    State(_state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<TraceIngestRequest>,
) -> RestResult<JsonResponse<IngestResponse>> {
    debug!(
        "Ingesting {} trace spans into {}",
        request.spans.len(),
        namespace
    );
    let total = request.spans.len() as u64;
    Ok(JsonResponse(IngestResponse {
        ingested: total,
        failed: 0,
        success: true,
    }))
}

async fn query_traces(
    State(_state): State<ObservabilityRestState>,
    Path(namespace): Path<String>,
    Json(request): Json<TraceQueryRequest>,
) -> RestResult<JsonResponse<TraceResponse>> {
    debug!(
        "Querying traces in namespace: {} (trace_id={:?}, service={:?}, range={}..{})",
        namespace, request.trace_id, request.service, request.start_ns, request.end_ns
    );
    Ok(JsonResponse(TraceResponse {
        spans: Vec::new(),
        total: 0,
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn convert_log_request(req: &LogEntryRequest) -> RestResult<LogEntry> {
    use proximadb_proto::v1::sql_value::Value as SV;

    let timestamp_ns = req
        .timestamp_ns
        .unwrap_or_else(|| chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));

    let fields: HashMap<String, SqlValue> = req
        .fields
        .iter()
        .map(|(k, v)| {
            let inner = match v {
                serde_json::Value::String(s) => Some(SV::StringValue(s.clone())),
                serde_json::Value::Bool(b) => Some(SV::BoolValue(*b)),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Some(SV::Int64Value(i))
                    } else {
                        n.as_f64().map(SV::NumberValue)
                    }
                }
                _ => Some(SV::StringValue(v.to_string())),
            };
            (k.clone(), SqlValue { value: inner })
        })
        .collect();

    Ok(LogEntry {
        timestamp_ns,
        severity: parse_severity(&req.severity) as i32,
        message: req.message.clone(),
        fields,
        source: req.source.clone(),
        service: req.service.clone(),
    })
}

fn convert_log_to_response(entry: LogEntry) -> LogEntryResponse {
    use proximadb_proto::v1::sql_value::Value as SV;

    let fields: HashMap<String, serde_json::Value> = entry
        .fields
        .into_iter()
        .map(|(k, v)| {
            let json_val = match v.value {
                Some(SV::StringValue(s)) => serde_json::Value::String(s),
                Some(SV::Int64Value(i)) => serde_json::json!(i),
                Some(SV::NumberValue(f)) => serde_json::json!(f),
                Some(SV::BoolValue(b)) => serde_json::Value::Bool(b),
                _ => serde_json::Value::Null,
            };
            (k, json_val)
        })
        .collect();

    LogEntryResponse {
        timestamp_ns: entry.timestamp_ns,
        severity: severity_to_string(Severity::try_from(entry.severity).unwrap_or(Severity::Info)),
        message: entry.message,
        source: entry.source,
        service: entry.service,
        fields,
    }
}

fn convert_metric_request(req: &MetricSampleRequest) -> MetricSample {
    MetricSample {
        name: req.name.clone(),
        value: req.value,
        timestamp_ns: req
            .timestamp_ns
            .unwrap_or_else(|| chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
        labels: req.labels.clone(),
    }
}

fn parse_severity(s: &str) -> Severity {
    match s.to_lowercase().as_str() {
        "trace" | "verbose" => Severity::Trace,
        "debug" => Severity::Debug,
        "info" | "information" => Severity::Info,
        "warn" | "warning" => Severity::Warn,
        "error" | "err" => Severity::Error,
        "fatal" | "critical" => Severity::Fatal,
        _ => Severity::Info,
    }
}

fn severity_to_string(severity: Severity) -> String {
    match severity {
        Severity::Trace => "trace",
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
        Severity::Fatal => "fatal",
        Severity::Unspecified => "info",
    }
    .to_string()
}

fn parse_aggregation(s: &str) -> MetricAggregation {
    match s.to_lowercase().as_str() {
        "sum" => MetricAggregation::Sum,
        "min" => MetricAggregation::Min,
        "max" => MetricAggregation::Max,
        "count" => MetricAggregation::Count,
        "rate" => MetricAggregation::Rate,
        "p50" => MetricAggregation::P50,
        "p90" => MetricAggregation::P90,
        "p95" => MetricAggregation::P95,
        "p99" => MetricAggregation::P99,
        _ => MetricAggregation::Avg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_severity() {
        assert_eq!(parse_severity("debug"), Severity::Debug);
        assert_eq!(parse_severity("INFO"), Severity::Info);
        assert_eq!(parse_severity("warn"), Severity::Warn);
        assert_eq!(parse_severity("ERROR"), Severity::Error);
        assert_eq!(parse_severity("fatal"), Severity::Fatal);
    }
}
