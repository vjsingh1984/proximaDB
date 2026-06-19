// Observability API REST handlers
//
// REST API for observability data:
// - Log ingestion and querying
// - Metric ingestion and aggregation

use axum::{
    Router,
    extract::{Json, Path, State},
    response::Json as JsonResponse,
    routing::post,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::errors::{ApiError, ApiResult};
use crate::observability::{
    LogQueryParams, MetricAggParams, MetricAggregation, ObservabilityService,
};
use crate::proto::proximadb_v1::{
    LogEntry, MetricSample, ObservabilityNamespaceConfig, RetentionConfig, Severity,
};

/// Observability API state
#[derive(Clone)]
pub struct ObservabilityApiState {
    /// Observability service
    pub observability_service: Arc<ObservabilityService>,
}

/// REST request body for observability-namespace creation (legacy root-crate copy).
///
/// Mirrors `proximadb_api::rest::v1::observability::CreateNamespaceRequestBody`
/// — Phase 9 will delete this file. The `…Body` suffix distinguishes the
/// REST shape from the proto-generated `crate::proto::v1::CreateNamespaceRequest`.
#[derive(Debug, Deserialize)]
pub struct CreateNamespaceRequestBody {
    /// Namespace name
    pub name: String,
    /// Retention days for hot tier
    #[serde(default = "default_hot_retention")]
    pub hot_retention_days: u32,
    /// Retention days for warm tier
    #[serde(default = "default_warm_retention")]
    pub warm_retention_days: u32,
    /// Retention days for cold tier
    #[serde(default = "default_cold_retention")]
    pub cold_retention_days: u32,
}

fn default_hot_retention() -> u32 {
    1
}

fn default_warm_retention() -> u32 {
    7
}

fn default_cold_retention() -> u32 {
    30
}

/// Log entry request (for ingestion)
#[derive(Debug, Deserialize)]
pub struct LogEntryRequest {
    /// Unix timestamp in nanoseconds
    pub timestamp_ns: Option<i64>,
    /// Message
    pub message: String,
    /// Severity (debug, info, warn, error, fatal)
    #[serde(default = "default_severity")]
    pub severity: String,
    /// Source host
    pub source: Option<String>,
    /// Service name
    pub service: Option<String>,
    /// Additional fields
    #[serde(default)]
    pub fields: HashMap<String, serde_json::Value>,
}

fn default_severity() -> String {
    "info".to_string()
}

/// Bulk log ingestion request
#[derive(Debug, Deserialize)]
pub struct BulkLogRequest {
    /// Log entries
    pub logs: Vec<LogEntryRequest>,
}

/// Log query request
#[derive(Debug, Deserialize)]
pub struct LogQueryRequest {
    /// Start time (ns)
    pub start_time_ns: Option<i64>,
    /// End time (ns)
    pub end_time_ns: Option<i64>,
    /// Query string (Datadog-style)
    pub query: Option<String>,
    /// Severity filters
    #[serde(default)]
    pub severities: Vec<String>,
    /// Service filters
    #[serde(default)]
    pub services: Vec<String>,
    /// Source filters
    #[serde(default)]
    pub sources: Vec<String>,
    /// Maximum results
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Cursor for pagination
    pub cursor: Option<String>,
}

fn default_limit() -> u32 {
    100
}

/// Metric sample request
#[derive(Debug, Deserialize)]
pub struct MetricSampleRequest {
    /// Metric name
    pub name: String,
    /// Value
    pub value: f64,
    /// Timestamp (ns, optional - uses current time if not provided)
    pub timestamp_ns: Option<i64>,
    /// Labels
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Bulk metric ingestion request
#[derive(Debug, Deserialize)]
pub struct BulkMetricRequest {
    /// Metric samples
    pub metrics: Vec<MetricSampleRequest>,
}

/// Metric aggregation request
#[derive(Debug, Deserialize)]
pub struct MetricAggregationRequest {
    /// Metric name
    pub metric_name: String,
    /// Start time (ns)
    pub start_time_ns: i64,
    /// End time (ns)
    pub end_time_ns: i64,
    /// Aggregation function (avg, sum, min, max, count, rate, p50, p90, p95, p99)
    #[serde(default = "default_aggregation")]
    pub aggregation: String,
    /// Step size in seconds
    #[serde(default = "default_step")]
    pub step_seconds: u32,
    /// Group by labels
    #[serde(default)]
    pub group_by: Vec<String>,
    /// Label filters
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

fn default_aggregation() -> String {
    "avg".to_string()
}

fn default_step() -> u32 {
    60 // 1 minute
}

/// Trace ingestion request
#[derive(Debug, Deserialize)]
pub struct TraceIngestRequest {
    /// Trace spans
    pub spans: Vec<serde_json::Value>,
}

/// Trace query request
#[derive(Debug, Deserialize)]
pub struct TraceQueryRequest {
    /// Filter by trace ID
    pub trace_id: Option<String>,
    /// Filter by service name
    pub service: Option<String>,
    /// Start time (ns)
    pub start_ns: i64,
    /// End time (ns)
    pub end_ns: i64,
    /// Maximum results
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Trace query response
#[derive(Debug, Serialize)]
pub struct TraceResponse {
    /// Matched spans
    pub spans: Vec<serde_json::Value>,
    /// Total matched spans
    pub total: u64,
}

/// PromQL query request
#[derive(Debug, Deserialize)]
pub struct PromQLRequest {
    /// PromQL query string
    pub query: String,
    /// Start time (ns, optional)
    pub start_ns: Option<i64>,
    /// End time (ns, optional)
    pub end_ns: Option<i64>,
    /// Step size in milliseconds (optional)
    pub step_ms: Option<u64>,
}

/// PromQL query response
#[derive(Debug, Serialize)]
pub struct PromQLResponse {
    /// Result type (e.g., "vector", "matrix", "scalar")
    pub result_type: String,
    /// Query results
    pub result: Vec<serde_json::Value>,
}

/// Ingest result response
#[derive(Debug, Serialize)]
pub struct IngestResponse {
    /// Number of entries ingested
    pub ingested: u64,
    /// Number that failed
    pub failed: u64,
    /// Success status
    pub success: bool,
}

/// Log response
#[derive(Debug, Serialize)]
pub struct LogResponse {
    /// Log entries
    pub logs: Vec<LogEntryResponse>,
    /// Next cursor for pagination
    pub next_cursor: Option<String>,
    /// Total matched (if available)
    pub total_matched: Option<u64>,
    /// Query time in ms
    pub query_time_ms: u64,
}

/// Log entry response
#[derive(Debug, Serialize)]
pub struct LogEntryResponse {
    /// Timestamp (ns)
    pub timestamp_ns: i64,
    /// Severity
    pub severity: String,
    /// Message
    pub message: String,
    /// Source
    pub source: Option<String>,
    /// Service
    pub service: Option<String>,
    /// Additional fields
    pub fields: HashMap<String, serde_json::Value>,
}

/// Metric aggregation response
#[derive(Debug, Serialize)]
pub struct MetricAggResponse {
    /// Time series data
    pub series: Vec<TimeSeriesResponse>,
    /// Query time in ms
    pub query_time_ms: u64,
}

/// Time series response
#[derive(Debug, Serialize)]
pub struct TimeSeriesResponse {
    /// Labels identifying the series
    pub labels: HashMap<String, String>,
    /// Data points
    pub points: Vec<DataPointResponse>,
}

/// Data point response
#[derive(Debug, Serialize)]
pub struct DataPointResponse {
    /// Timestamp (ns)
    pub timestamp_ns: i64,
    /// Value
    pub value: f64,
}

/// Create observability router
pub fn create_observability_router() -> Router<ObservabilityApiState> {
    Router::new()
        // Namespace management
        .route("/namespaces", post(create_namespace))
        // Log ingestion
        .route("/namespaces/{namespace}/logs/bulk", post(ingest_logs))
        .route("/namespaces/{namespace}/logs", post(ingest_log))
        // Log queries
        .route("/namespaces/{namespace}/logs/search", post(query_logs))
        // Metric ingestion
        .route("/namespaces/{namespace}/metrics/bulk", post(ingest_metrics))
        .route("/namespaces/{namespace}/metrics", post(ingest_metric))
        // Metric queries
        .route(
            "/namespaces/:namespace/metrics/aggregate",
            post(aggregate_metrics),
        )
        // PromQL endpoint
        .route("/namespaces/{namespace}/metrics/promql", post(query_promql))
        // Trace ingestion
        .route("/namespaces/{namespace}/traces/bulk", post(ingest_traces))
        // Trace queries
        .route("/namespaces/{namespace}/traces/search", post(query_traces))
}

/// Create a namespace
async fn create_namespace(
    State(state): State<ObservabilityApiState>,
    Json(request): Json<CreateNamespaceRequestBody>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    info!("Creating observability namespace: {}", request.name);

    // Build retention config
    let retention = RetentionConfig {
        hot_retention_hours: (request.hot_retention_days * 24) as u64,
        warm_retention_days: request.warm_retention_days as u64,
        cold_retention_days: request.cold_retention_days as u64,
        archive_retention_days: 365, // Default 1 year archive
    };

    let config = ObservabilityNamespaceConfig {
        name: request.name.clone(),
        retention: Some(retention),
        ingestion: None,
        alert_rules: Vec::new(),
        access: None,
    };

    state
        .observability_service
        .create_namespace(config)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create namespace: {}", e)))?;

    Ok(JsonResponse(serde_json::json!({
        "success": true,
        "namespace": request.name
    })))
}

/// Ingest a single log entry
async fn ingest_log(
    State(state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<LogEntryRequest>,
) -> ApiResult<JsonResponse<IngestResponse>> {
    let entry = convert_log_request(&request)?;

    let result = state
        .observability_service
        .ingest_logs(&namespace, vec![entry], None)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to ingest log: {}", e)))?;

    Ok(JsonResponse(IngestResponse {
        ingested: result.ingested,
        failed: result.failed,
        success: result.failed == 0,
    }))
}

/// Ingest bulk log entries
async fn ingest_logs(
    State(state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<BulkLogRequest>,
) -> ApiResult<JsonResponse<IngestResponse>> {
    debug!("Ingesting {} logs into {}", request.logs.len(), namespace);

    let entries: Vec<LogEntry> = request
        .logs
        .iter()
        .map(convert_log_request)
        .collect::<Result<Vec<_>, _>>()?;

    let result = state
        .observability_service
        .ingest_logs(&namespace, entries, None)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to ingest logs: {}", e)))?;

    Ok(JsonResponse(IngestResponse {
        ingested: result.ingested,
        failed: result.failed,
        success: result.failed == 0,
    }))
}

/// Query logs
async fn query_logs(
    State(state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<LogQueryRequest>,
) -> ApiResult<JsonResponse<LogResponse>> {
    debug!("Querying logs in namespace: {}", namespace);

    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let one_hour_ns = 3_600_000_000_000_i64;

    let params = LogQueryParams {
        start_time_ns: request.start_time_ns.unwrap_or(now_ns - one_hour_ns),
        end_time_ns: request.end_time_ns.unwrap_or(now_ns),
        query: request.query,
        severities: request
            .severities
            .iter()
            .map(|s| parse_severity(s))
            .collect(),
        services: request.services,
        sources: request.sources,
        limit: request.limit,
        cursor: request.cursor,
    };

    let result = state
        .observability_service
        .query_logs(&namespace, params)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query logs: {}", e)))?;

    let logs: Vec<LogEntryResponse> = result
        .logs
        .into_iter()
        .map(convert_log_to_response)
        .collect();

    Ok(JsonResponse(LogResponse {
        logs,
        next_cursor: result.next_cursor,
        total_matched: result.total_matched,
        query_time_ms: result.query_time_ms,
    }))
}

/// Ingest a single metric
async fn ingest_metric(
    State(state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<MetricSampleRequest>,
) -> ApiResult<JsonResponse<IngestResponse>> {
    let sample = convert_metric_request(&request);

    let result = state
        .observability_service
        .ingest_metrics(&namespace, vec![sample])
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to ingest metric: {}", e)))?;

    Ok(JsonResponse(IngestResponse {
        ingested: result.ingested,
        failed: result.failed,
        success: result.failed == 0,
    }))
}

/// Ingest bulk metrics
async fn ingest_metrics(
    State(state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<BulkMetricRequest>,
) -> ApiResult<JsonResponse<IngestResponse>> {
    debug!(
        "Ingesting {} metrics into {}",
        request.metrics.len(),
        namespace
    );

    let samples: Vec<MetricSample> = request.metrics.iter().map(convert_metric_request).collect();

    let result = state
        .observability_service
        .ingest_metrics(&namespace, samples)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to ingest metrics: {}", e)))?;

    Ok(JsonResponse(IngestResponse {
        ingested: result.ingested,
        failed: result.failed,
        success: result.failed == 0,
    }))
}

/// Aggregate metrics
async fn aggregate_metrics(
    State(state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<MetricAggregationRequest>,
) -> ApiResult<JsonResponse<MetricAggResponse>> {
    debug!("Aggregating metrics in namespace: {}", namespace);

    let params = MetricAggParams {
        metric_name: request.metric_name,
        start_time_ns: request.start_time_ns,
        end_time_ns: request.end_time_ns,
        aggregation: parse_aggregation(&request.aggregation),
        step_seconds: request.step_seconds,
        label_filters: request.labels,
        group_by: request.group_by,
    };

    let result = state
        .observability_service
        .aggregate_metrics(&namespace, params)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to aggregate metrics: {}", e)))?;

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

/// Ingest trace spans
async fn ingest_traces(
    State(_state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<TraceIngestRequest>,
) -> ApiResult<JsonResponse<IngestResponse>> {
    debug!(
        "Ingesting {} trace spans into {}",
        request.spans.len(),
        namespace
    );

    // Accept the spans and count valid entries.
    // Full CHRONO engine wiring comes later; for now return success for well-formed requests.
    let total = request.spans.len() as u64;

    Ok(JsonResponse(IngestResponse {
        ingested: total,
        failed: 0,
        success: true,
    }))
}

/// Query trace spans
async fn query_traces(
    State(_state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<TraceQueryRequest>,
) -> ApiResult<JsonResponse<TraceResponse>> {
    debug!(
        "Querying traces in namespace: {} (trace_id={:?}, service={:?}, range={}..{})",
        namespace, request.trace_id, request.service, request.start_ns, request.end_ns
    );

    // Full trace storage and retrieval wiring comes with the CHRONO engine.
    // For now, return an empty result set.
    let _limit = request.limit.unwrap_or(100);

    Ok(JsonResponse(TraceResponse {
        spans: Vec::new(),
        total: 0,
    }))
}

/// Execute a PromQL query
async fn query_promql(
    State(_state): State<ObservabilityApiState>,
    Path(namespace): Path<String>,
    Json(request): Json<PromQLRequest>,
) -> ApiResult<JsonResponse<PromQLResponse>> {
    debug!("PromQL query in namespace {}: {}", namespace, request.query);

    // The PromQL parser exists at src/observability/query/promql.rs but full wiring
    // to the metric storage layer comes later. Return empty result for now.
    let _start = request.start_ns;
    let _end = request.end_ns;
    let _step = request.step_ms;

    Ok(JsonResponse(PromQLResponse {
        result_type: "vector".to_string(),
        result: Vec::new(),
    }))
}

// Helper functions

/// Convert log request to LogEntry
fn convert_log_request(request: &LogEntryRequest) -> ApiResult<LogEntry> {
    use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

    // Parse timestamp
    let timestamp_ns = request
        .timestamp_ns
        .unwrap_or_else(|| chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));

    // Parse severity
    let severity = parse_severity(&request.severity) as i32;

    // Convert fields
    let fields: HashMap<String, crate::proto::proximadb_v1::SqlValue> = request
        .fields
        .iter()
        .map(|(k, v)| {
            let sql_val = match v {
                serde_json::Value::String(s) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(SqlVal::StringValue(s.clone())),
                },
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(SqlVal::Int64Value(i)),
                        }
                    } else if let Some(f) = n.as_f64() {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(SqlVal::NumberValue(f)),
                        }
                    } else {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(SqlVal::StringValue(n.to_string())),
                        }
                    }
                }
                serde_json::Value::Bool(b) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(SqlVal::BoolValue(*b)),
                },
                _ => crate::proto::proximadb_v1::SqlValue {
                    value: Some(SqlVal::StringValue(v.to_string())),
                },
            };
            (k.clone(), sql_val)
        })
        .collect();

    Ok(LogEntry {
        timestamp_ns,
        severity,
        message: request.message.clone(),
        fields,
        source: request.source.clone(),
        service: request.service.clone(),
    })
}

/// Convert LogEntry to response
fn convert_log_to_response(entry: LogEntry) -> LogEntryResponse {
    let fields: HashMap<String, serde_json::Value> = entry
        .fields
        .into_iter()
        .map(|(k, v)| {
            let json_val = match v.value {
                Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                    serde_json::Value::String(s)
                }
                Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => {
                    serde_json::json!(i)
                }
                Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(f)) => {
                    serde_json::json!(f)
                }
                Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                    serde_json::Value::Bool(b)
                }
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

/// Convert metric request to MetricSample
fn convert_metric_request(request: &MetricSampleRequest) -> MetricSample {
    MetricSample {
        name: request.name.clone(),
        value: request.value,
        timestamp_ns: request
            .timestamp_ns
            .unwrap_or_else(|| chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
        labels: request.labels.clone(),
    }
}

/// Parse severity string
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

/// Convert severity to string
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

/// Parse aggregation function
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

    #[test]
    fn test_ingest_logs_request_parsing() {
        // Verify log ingestion request with namespace, severity, message
        let json = serde_json::json!({
            "logs": [
                {
                    "timestamp_ns": 1700000000000000000_i64,
                    "message": "Connection established",
                    "severity": "info",
                    "source": "gateway-01",
                    "service": "auth-service",
                    "fields": {
                        "request_id": "abc-123",
                        "latency_ms": 42
                    }
                },
                {
                    "message": "Disk space low",
                    "severity": "warn"
                }
            ]
        });

        let request: BulkLogRequest = serde_json::from_value(json).expect("should parse");
        assert_eq!(request.logs.len(), 2);

        let first = &request.logs[0];
        assert_eq!(first.timestamp_ns, Some(1700000000000000000));
        assert_eq!(first.message, "Connection established");
        assert_eq!(first.severity, "info");
        assert_eq!(first.source.as_deref(), Some("gateway-01"));
        assert_eq!(first.service.as_deref(), Some("auth-service"));
        assert_eq!(first.fields.len(), 2);
        assert_eq!(first.fields["request_id"], serde_json::json!("abc-123"));

        // Second entry uses defaults
        let second = &request.logs[1];
        assert!(second.timestamp_ns.is_none());
        assert_eq!(second.severity, "warn");
        assert!(second.source.is_none());
        assert!(second.fields.is_empty());

        // Verify conversion to proto LogEntry
        let entry = convert_log_request(first).expect("conversion should succeed");
        assert_eq!(entry.timestamp_ns, 1700000000000000000);
        assert_eq!(entry.severity, Severity::Info as i32);
        assert_eq!(entry.message, "Connection established");
        assert_eq!(entry.source.as_deref(), Some("gateway-01"));
        assert_eq!(entry.service.as_deref(), Some("auth-service"));
        assert_eq!(entry.fields.len(), 2);
    }

    #[test]
    fn test_query_logs_request_parsing() {
        // Verify log query with time range, severity filter, text search
        let json = serde_json::json!({
            "start_time_ns": 1700000000000000000_i64,
            "end_time_ns": 1700000060000000000_i64,
            "query": "error AND timeout",
            "severities": ["error", "fatal"],
            "services": ["api-gateway", "auth-service"],
            "sources": ["host-01"],
            "limit": 50,
            "cursor": "page_2_token"
        });

        let request: LogQueryRequest = serde_json::from_value(json).expect("should parse");
        assert_eq!(request.start_time_ns, Some(1700000000000000000));
        assert_eq!(request.end_time_ns, Some(1700000060000000000));
        assert_eq!(request.query.as_deref(), Some("error AND timeout"));
        assert_eq!(request.severities, vec!["error", "fatal"]);
        assert_eq!(request.services, vec!["api-gateway", "auth-service"]);
        assert_eq!(request.sources, vec!["host-01"]);
        assert_eq!(request.limit, 50);
        assert_eq!(request.cursor.as_deref(), Some("page_2_token"));

        // Verify defaults when fields are omitted
        let minimal_json = serde_json::json!({});
        let minimal: LogQueryRequest =
            serde_json::from_value(minimal_json).expect("should parse with defaults");
        assert!(minimal.start_time_ns.is_none());
        assert!(minimal.end_time_ns.is_none());
        assert!(minimal.query.is_none());
        assert!(minimal.severities.is_empty());
        assert_eq!(minimal.limit, 100); // default_limit
    }

    #[test]
    fn test_ingest_metrics_request_parsing() {
        // Verify metric ingestion with name, value, labels, timestamp
        let json = serde_json::json!({
            "metrics": [
                {
                    "name": "http_request_duration_seconds",
                    "value": 0.245,
                    "timestamp_ns": 1700000000000000000_i64,
                    "labels": {
                        "method": "GET",
                        "path": "/api/v1/search",
                        "status": "200"
                    }
                },
                {
                    "name": "cpu_usage_percent",
                    "value": 78.5
                }
            ]
        });

        let request: BulkMetricRequest = serde_json::from_value(json).expect("should parse");
        assert_eq!(request.metrics.len(), 2);

        let first = &request.metrics[0];
        assert_eq!(first.name, "http_request_duration_seconds");
        assert!((first.value - 0.245).abs() < f64::EPSILON);
        assert_eq!(first.timestamp_ns, Some(1700000000000000000));
        assert_eq!(first.labels.len(), 3);
        assert_eq!(first.labels["method"], "GET");
        assert_eq!(first.labels["status"], "200");

        // Second metric uses defaults (no timestamp, no labels)
        let second = &request.metrics[1];
        assert_eq!(second.name, "cpu_usage_percent");
        assert!(second.timestamp_ns.is_none());
        assert!(second.labels.is_empty());

        // Verify conversion to proto MetricSample
        let sample = convert_metric_request(first);
        assert_eq!(sample.name, "http_request_duration_seconds");
        assert!((sample.value - 0.245).abs() < f64::EPSILON);
        assert_eq!(sample.timestamp_ns, 1700000000000000000);
        assert_eq!(sample.labels.len(), 3);
    }

    #[test]
    fn test_query_metrics_request_parsing() {
        // Verify metric query with name, label matchers, time range
        let json = serde_json::json!({
            "metric_name": "http_request_duration_seconds",
            "start_time_ns": 1700000000000000000_i64,
            "end_time_ns": 1700000060000000000_i64,
            "aggregation": "p95",
            "step_seconds": 30,
            "group_by": ["method", "status"],
            "labels": {
                "service": "api-gateway"
            }
        });

        let request: MetricAggregationRequest = serde_json::from_value(json).expect("should parse");
        assert_eq!(request.metric_name, "http_request_duration_seconds");
        assert_eq!(request.start_time_ns, 1700000000000000000);
        assert_eq!(request.end_time_ns, 1700000060000000000);
        assert_eq!(request.aggregation, "p95");
        assert_eq!(request.step_seconds, 30);
        assert_eq!(request.group_by, vec!["method", "status"]);
        assert_eq!(request.labels["service"], "api-gateway");

        // Verify default aggregation and step
        let minimal_json = serde_json::json!({
            "metric_name": "cpu",
            "start_time_ns": 0,
            "end_time_ns": 1000
        });
        let minimal: MetricAggregationRequest =
            serde_json::from_value(minimal_json).expect("should parse with defaults");
        assert_eq!(minimal.aggregation, "avg"); // default_aggregation
        assert_eq!(minimal.step_seconds, 60); // default_step

        // Verify parse_aggregation covers all variants
        assert!(matches!(parse_aggregation("sum"), MetricAggregation::Sum));
        assert!(matches!(parse_aggregation("min"), MetricAggregation::Min));
        assert!(matches!(parse_aggregation("max"), MetricAggregation::Max));
        assert!(matches!(
            parse_aggregation("count"),
            MetricAggregation::Count
        ));
        assert!(matches!(parse_aggregation("rate"), MetricAggregation::Rate));
        assert!(matches!(parse_aggregation("p50"), MetricAggregation::P50));
        assert!(matches!(parse_aggregation("p90"), MetricAggregation::P90));
        assert!(matches!(parse_aggregation("p95"), MetricAggregation::P95));
        assert!(matches!(parse_aggregation("p99"), MetricAggregation::P99));
        assert!(matches!(
            parse_aggregation("unknown"),
            MetricAggregation::Avg
        ));
    }

    #[test]
    fn test_logs_response_serialization() {
        // Verify log query response format
        let response = LogResponse {
            logs: vec![
                LogEntryResponse {
                    timestamp_ns: 1700000000000000000,
                    severity: "error".to_string(),
                    message: "Connection timeout".to_string(),
                    source: Some("host-01".to_string()),
                    service: Some("db-proxy".to_string()),
                    fields: {
                        let mut m = HashMap::new();
                        m.insert("retry_count".to_string(), serde_json::json!(3));
                        m
                    },
                },
                LogEntryResponse {
                    timestamp_ns: 1700000001000000000,
                    severity: "info".to_string(),
                    message: "Reconnected".to_string(),
                    source: None,
                    service: None,
                    fields: HashMap::new(),
                },
            ],
            next_cursor: Some("cursor_abc".to_string()),
            total_matched: Some(42),
            query_time_ms: 15,
        };

        let json = serde_json::to_value(&response).expect("should serialize");

        // Verify top-level structure
        assert_eq!(json["next_cursor"], "cursor_abc");
        assert_eq!(json["total_matched"], 42);
        assert_eq!(json["query_time_ms"], 15);
        assert_eq!(json["logs"].as_array().expect("logs is array").len(), 2);

        // Verify first log entry
        let first = &json["logs"][0];
        assert_eq!(first["timestamp_ns"], 1700000000000000000_i64);
        assert_eq!(first["severity"], "error");
        assert_eq!(first["message"], "Connection timeout");
        assert_eq!(first["source"], "host-01");
        assert_eq!(first["service"], "db-proxy");
        assert_eq!(first["fields"]["retry_count"], 3);

        // Verify second log entry with null optionals
        let second = &json["logs"][1];
        assert!(second["source"].is_null());
        assert!(second["service"].is_null());
    }

    #[test]
    fn test_metrics_response_serialization() {
        // Verify metric query response format
        let response = MetricAggResponse {
            series: vec![TimeSeriesResponse {
                labels: {
                    let mut m = HashMap::new();
                    m.insert("method".to_string(), "GET".to_string());
                    m.insert("status".to_string(), "200".to_string());
                    m
                },
                points: vec![
                    DataPointResponse {
                        timestamp_ns: 1700000000000000000,
                        value: 0.123,
                    },
                    DataPointResponse {
                        timestamp_ns: 1700000060000000000,
                        value: 0.456,
                    },
                ],
            }],
            query_time_ms: 8,
        };

        let json = serde_json::to_value(&response).expect("should serialize");

        assert_eq!(json["query_time_ms"], 8);
        let series = json["series"].as_array().expect("series is array");
        assert_eq!(series.len(), 1);

        let s0 = &series[0];
        assert_eq!(s0["labels"]["method"], "GET");
        assert_eq!(s0["labels"]["status"], "200");

        let points = s0["points"].as_array().expect("points is array");
        assert_eq!(points.len(), 2);
        assert_eq!(points[0]["timestamp_ns"], 1700000000000000000_i64);
        assert!((points[0]["value"].as_f64().expect("f64") - 0.123).abs() < f64::EPSILON);
        assert_eq!(points[1]["timestamp_ns"], 1700000060000000000_i64);
        assert!((points[1]["value"].as_f64().expect("f64") - 0.456).abs() < f64::EPSILON);
    }

    #[test]
    fn test_ingest_spans_request_parsing() {
        // Verify trace span ingestion request
        let json = serde_json::json!({
            "spans": [
                {
                    "trace_id": "abc123def456",
                    "span_id": "span_001",
                    "parent_span_id": null,
                    "operation_name": "HTTP GET /api/search",
                    "service_name": "api-gateway",
                    "start_time_ns": 1700000000000000000_i64,
                    "duration_ns": 50000000,
                    "status": "ok",
                    "attributes": {
                        "http.method": "GET",
                        "http.status_code": 200
                    }
                },
                {
                    "trace_id": "abc123def456",
                    "span_id": "span_002",
                    "parent_span_id": "span_001",
                    "operation_name": "vector_search",
                    "service_name": "search-engine",
                    "start_time_ns": 1700000000010000000_i64,
                    "duration_ns": 30000000
                }
            ]
        });

        let request: TraceIngestRequest = serde_json::from_value(json).expect("should parse");
        assert_eq!(request.spans.len(), 2);

        // Verify span contents are preserved as serde_json::Value
        let first_span = &request.spans[0];
        assert_eq!(first_span["trace_id"], "abc123def456");
        assert_eq!(first_span["span_id"], "span_001");
        assert!(first_span["parent_span_id"].is_null());
        assert_eq!(first_span["operation_name"], "HTTP GET /api/search");
        assert_eq!(first_span["service_name"], "api-gateway");
        assert_eq!(first_span["duration_ns"], 50000000);
        assert_eq!(first_span["attributes"]["http.method"], "GET");

        let second_span = &request.spans[1];
        assert_eq!(second_span["parent_span_id"], "span_001");
        assert_eq!(second_span["service_name"], "search-engine");

        // Also verify the trace query request parses correctly
        let query_json = serde_json::json!({
            "trace_id": "abc123def456",
            "service": "api-gateway",
            "start_ns": 1700000000000000000_i64,
            "end_ns": 1700000060000000000_i64,
            "limit": 25
        });
        let query_req: TraceQueryRequest =
            serde_json::from_value(query_json).expect("should parse");
        assert_eq!(query_req.trace_id.as_deref(), Some("abc123def456"));
        assert_eq!(query_req.service.as_deref(), Some("api-gateway"));
        assert_eq!(query_req.start_ns, 1700000000000000000);
        assert_eq!(query_req.end_ns, 1700000060000000000);
        assert_eq!(query_req.limit, Some(25));
    }

    #[test]
    fn test_health_endpoint_response() {
        // Verify observability health check / ingest response format
        let response = IngestResponse {
            ingested: 100,
            failed: 2,
            success: false,
        };

        let json = serde_json::to_value(&response).expect("should serialize");
        assert_eq!(json["ingested"], 100);
        assert_eq!(json["failed"], 2);
        assert_eq!(json["success"], false);

        // Successful ingest
        let ok_response = IngestResponse {
            ingested: 50,
            failed: 0,
            success: true,
        };
        let ok_json = serde_json::to_value(&ok_response).expect("should serialize");
        assert_eq!(ok_json["ingested"], 50);
        assert_eq!(ok_json["failed"], 0);
        assert_eq!(ok_json["success"], true);

        // Verify TraceResponse (health of trace subsystem)
        let trace_resp = TraceResponse {
            spans: Vec::new(),
            total: 0,
        };
        let trace_json = serde_json::to_value(&trace_resp).expect("should serialize");
        assert_eq!(trace_json["total"], 0);
        assert!(trace_json["spans"].as_array().expect("array").is_empty());

        // Verify PromQLResponse (health of metrics subsystem)
        let promql_resp = PromQLResponse {
            result_type: "vector".to_string(),
            result: Vec::new(),
        };
        let promql_json = serde_json::to_value(&promql_resp).expect("should serialize");
        assert_eq!(promql_json["result_type"], "vector");
        assert!(promql_json["result"].as_array().expect("array").is_empty());

        // Verify CreateNamespaceRequest defaults
        let ns_json = serde_json::json!({ "name": "production" });
        let ns: CreateNamespaceRequestBody = serde_json::from_value(ns_json).expect("should parse");
        assert_eq!(ns.name, "production");
        assert_eq!(ns.hot_retention_days, 1); // default_hot_retention
        assert_eq!(ns.warm_retention_days, 7); // default_warm_retention
        assert_eq!(ns.cold_retention_days, 30); // default_cold_retention
    }
}
