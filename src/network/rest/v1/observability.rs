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

/// Create namespace request
#[derive(Debug, Deserialize)]
pub struct CreateNamespaceRequest {
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
        .route("/namespaces/:namespace/logs/_bulk", post(ingest_logs))
        .route("/namespaces/:namespace/logs", post(ingest_log))
        // Log queries
        .route("/namespaces/:namespace/logs/_search", post(query_logs))
        // Metric ingestion
        .route("/namespaces/:namespace/metrics/_bulk", post(ingest_metrics))
        .route("/namespaces/:namespace/metrics", post(ingest_metric))
        // Metric queries
        .route(
            "/namespaces/:namespace/metrics/_aggregate",
            post(aggregate_metrics),
        )
}

/// Create a namespace
async fn create_namespace(
    State(state): State<ObservabilityApiState>,
    Json(request): Json<CreateNamespaceRequest>,
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
    let one_hour_ns = 3600_000_000_000i64;

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
}
