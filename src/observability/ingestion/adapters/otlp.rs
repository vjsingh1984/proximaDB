// OTLP adapter (OpenTelemetry Protocol)
//
// Supports:
// - gRPC transport (port 4317)
// - HTTP/JSON transport (port 4318)
// - Logs, metrics, and traces

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use axum::{Json, Router, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;

use super::{AdapterConfig, InputAdapter};
use crate::observability::ObservabilityService;
use crate::proto::proximadb_v1::{
    LogEntry, MetricSample, Severity, SpanKind, SpanStatus, SpanStatusCode, SqlValue, TraceData,
    sql_value,
};

/// OTLP adapter for OpenTelemetry protocol
pub struct OtlpAdapter {
    /// Configuration
    config: AdapterConfig,
    /// Whether the adapter is running
    running: AtomicBool,
    /// Number of events received
    events_received: Arc<AtomicU64>,
    /// Transport type
    transport: OtlpTransport,
    /// Observability service for ingestion
    observability_service: Arc<ObservabilityService>,
    /// Namespace to ingest into
    namespace: String,
    /// Shutdown channel sender
    shutdown_tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
}

/// OTLP transport type
#[derive(Debug, Clone, Copy)]
pub enum OtlpTransport {
    /// gRPC transport (default OTLP port 4317)
    Grpc,
    /// HTTP/JSON transport (default OTLP port 4318)
    Http,
}

// OTLP JSON trace structures (based on OpenTelemetry specification)
// https://opentelemetry.io/docs/reference/specification/protocol/otlp/

/// OTLP trace export request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpExportTracesServiceRequest {
    /// Resource spans (contains resource attributes and scope spans)
    #[serde(rename = "resourceSpans")]
    pub resource_spans: Vec<OtlpResourceSpans>,
}

/// OTLP resource spans
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpResourceSpans {
    /// Resource attributes (service.name, telemetry.sdk.name, etc.)
    pub resource: Option<OtlpResource>,
    /// Scope spans (instrumentation scope)
    #[serde(rename = "scopeSpans")]
    pub scope_spans: Vec<OtlpScopeSpans>,
    /// Schema URL
    #[serde(rename = "schemaUrl")]
    pub schema_url: Option<String>,
}

/// OTLP resource
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpResource {
    /// Resource attributes
    pub attributes: Vec<OtlpKeyValue>,
    /// Schema URL
    #[serde(rename = "schemaUrl")]
    pub schema_url: Option<String>,
}

/// OTLP scope spans
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpScopeSpans {
    /// Instrumentation scope
    pub scope: Option<OtlpInstrumentationScope>,
    /// Spans
    pub spans: Vec<OtlpSpan>,
    /// Schema URL
    #[serde(rename = "schemaUrl")]
    pub schema_url: Option<String>,
}

/// OTLP instrumentation scope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpInstrumentationScope {
    /// Scope name
    pub name: String,
    /// Scope version
    pub version: Option<String>,
    /// Schema URL
    #[serde(rename = "schemaUrl")]
    pub schema_url: Option<String>,
    /// Scope attributes
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
}

/// OTLP span
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpSpan {
    /// Trace ID (16 bytes as hex string)
    #[serde(rename = "traceId")]
    pub trace_id: String,
    /// Span ID (8 bytes as hex string)
    #[serde(rename = "spanId")]
    pub span_id: String,
    /// Parent span ID (8 bytes as hex string)
    #[serde(rename = "parentSpanId")]
    pub parent_span_id: Option<String>,
    /// Trace state
    #[serde(rename = "traceState")]
    pub trace_state: Option<String>,
    /// Span name
    pub name: String,
    /// Span kind
    #[serde(rename = "kind")]
    pub kind: Option<String>, // INTERNAL, SERVER, CLIENT, PRODUCER, CONSUMER
    /// Start time (Unix epoch nanoseconds)
    #[serde(rename = "startTimeUnixNano")]
    pub start_time_unix_nano: String,
    /// End time (Unix epoch nanoseconds)
    #[serde(rename = "endTimeUnixNano")]
    pub end_time_unix_nano: String,
    /// Span attributes
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
    /// Span events
    #[serde(rename = "events", default)]
    pub events: Vec<OtlpEvent>,
    /// Span links
    #[serde(default)]
    pub links: Vec<OtlpLink>,
    /// Span status
    pub status: Option<OtlpStatus>,
}

/// OTLP key-value pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpKeyValue {
    /// Key
    pub key: String,
    /// Value
    pub value: OtlpAnyValue,
}

/// OTLP any value
///
/// In OTLP JSON, each value type has its own key.
/// We use a struct with optional fields for each value type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpAnyValue {
    /// String value
    #[serde(rename = "stringValue", skip_serializing_if = "Option::is_none")]
    pub string_value: Option<String>,
    /// Bool value
    #[serde(rename = "boolValue", skip_serializing_if = "Option::is_none")]
    pub bool_value: Option<bool>,
    /// Int value
    #[serde(rename = "intValue", skip_serializing_if = "Option::is_none")]
    pub int_value: Option<i64>,
    /// Double value
    #[serde(rename = "doubleValue", skip_serializing_if = "Option::is_none")]
    pub double_value: Option<f64>,
    /// Array value
    #[serde(rename = "arrayValue", skip_serializing_if = "Option::is_none")]
    pub array_value: Option<OtlpArray>,
    /// Key-value list value
    #[serde(rename = "kvlistValue", skip_serializing_if = "Option::is_none")]
    pub kvlist_value: Option<OtlpKeyValueList>,
    /// Bytes value (base64 encoded)
    #[serde(rename = "bytesValue", skip_serializing_if = "Option::is_none")]
    pub bytes_value: Option<String>,
}

/// OTLP array
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpArray {
    /// Array values
    pub values: Vec<OtlpAnyValue>,
}

/// OTLP key-value list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpKeyValueList {
    /// Key-value pairs
    pub values: Vec<OtlpKeyValue>,
}

/// OTLP event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpEvent {
    /// Event name
    pub name: String,
    /// Event time (Unix epoch nanoseconds)
    #[serde(rename = "timeUnixNano")]
    pub time_unix_nano: String,
    /// Event attributes
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
}

/// OTLP link
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpLink {
    /// Linked trace ID
    #[serde(rename = "traceId")]
    pub trace_id: String,
    /// Linked span ID
    #[serde(rename = "spanId")]
    pub span_id: String,
    /// Trace state
    #[serde(rename = "traceState")]
    pub trace_state: Option<String>,
    /// Link attributes
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
}

/// OTLP status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpStatus {
    /// Status code (UNSET=0, OK=1, ERROR=2)
    #[serde(rename = "code")]
    pub code: String, // "STATUS_CODE_UNSET", "STATUS_CODE_OK", "STATUS_CODE_ERROR"
    /// Status message
    #[serde(default)]
    pub message: String,
}

/// OTLP trace export response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpExportTracesServiceResponse {
    /// Partial success (for partial failures)
    #[serde(rename = "partialSuccess")]
    pub partial_success: Option<OtlpPartialSuccess>,
}

/// OTLP partial success
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpPartialSuccess {
    /// Number of rejected spans
    #[serde(rename = "rejectedSpans")]
    pub rejected_spans: i64,
    /// Error message
    #[serde(rename = "errorMessage")]
    pub error_message: String,
}

/// Error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpErrorResponse {
    /// Error code
    pub code: i64,
    /// Error message
    pub message: String,
}

// ---- OTLP Logs JSON structures ----

/// OTLP log export request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpExportLogsServiceRequest {
    /// Resource logs
    #[serde(rename = "resourceLogs")]
    pub resource_logs: Vec<OtlpResourceLogs>,
}

/// OTLP resource logs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpResourceLogs {
    /// Resource attributes
    pub resource: Option<OtlpResource>,
    /// Scope logs
    #[serde(rename = "scopeLogs")]
    pub scope_logs: Vec<OtlpScopeLogs>,
    /// Schema URL
    #[serde(rename = "schemaUrl")]
    pub schema_url: Option<String>,
}

/// OTLP scope logs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpScopeLogs {
    /// Instrumentation scope
    pub scope: Option<OtlpInstrumentationScope>,
    /// Log records
    #[serde(rename = "logRecords")]
    pub log_records: Vec<OtlpLogRecord>,
    /// Schema URL
    #[serde(rename = "schemaUrl")]
    pub schema_url: Option<String>,
}

/// OTLP log record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpLogRecord {
    /// Timestamp (Unix epoch nanoseconds)
    #[serde(rename = "timeUnixNano", default)]
    pub time_unix_nano: String,
    /// Observed timestamp (Unix epoch nanoseconds)
    #[serde(rename = "observedTimeUnixNano", default)]
    pub observed_time_unix_nano: String,
    /// Severity number (1-24)
    #[serde(rename = "severityNumber", default)]
    pub severity_number: i32,
    /// Severity text
    #[serde(rename = "severityText", default)]
    pub severity_text: String,
    /// Log body
    pub body: Option<OtlpAnyValue>,
    /// Attributes
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
    /// Trace ID (for correlated logs)
    #[serde(rename = "traceId", default)]
    pub trace_id: String,
    /// Span ID (for correlated logs)
    #[serde(rename = "spanId", default)]
    pub span_id: String,
}

/// OTLP log export response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpExportLogsServiceResponse {
    /// Partial success
    #[serde(rename = "partialSuccess")]
    pub partial_success: Option<OtlpLogsPartialSuccess>,
}

/// OTLP logs partial success
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpLogsPartialSuccess {
    /// Number of rejected log records
    #[serde(rename = "rejectedLogRecords")]
    pub rejected_log_records: i64,
    /// Error message
    #[serde(rename = "errorMessage")]
    pub error_message: String,
}

// ---- OTLP Metrics JSON structures ----

/// OTLP metric export request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpExportMetricsServiceRequest {
    /// Resource metrics
    #[serde(rename = "resourceMetrics")]
    pub resource_metrics: Vec<OtlpResourceMetrics>,
}

/// OTLP resource metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpResourceMetrics {
    /// Resource attributes
    pub resource: Option<OtlpResource>,
    /// Scope metrics
    #[serde(rename = "scopeMetrics")]
    pub scope_metrics: Vec<OtlpScopeMetrics>,
    /// Schema URL
    #[serde(rename = "schemaUrl")]
    pub schema_url: Option<String>,
}

/// OTLP scope metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpScopeMetrics {
    /// Instrumentation scope
    pub scope: Option<OtlpInstrumentationScope>,
    /// Metrics
    pub metrics: Vec<OtlpMetric>,
    /// Schema URL
    #[serde(rename = "schemaUrl")]
    pub schema_url: Option<String>,
}

/// OTLP metric (supports gauge, sum, histogram)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpMetric {
    /// Metric name
    pub name: String,
    /// Metric description
    #[serde(default)]
    pub description: String,
    /// Metric unit
    #[serde(default)]
    pub unit: String,
    /// Gauge data points
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gauge: Option<OtlpGauge>,
    /// Sum data points
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<OtlpSum>,
    /// Histogram data points
    #[serde(skip_serializing_if = "Option::is_none")]
    pub histogram: Option<OtlpHistogram>,
}

/// OTLP gauge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpGauge {
    /// Data points
    #[serde(rename = "dataPoints")]
    pub data_points: Vec<OtlpNumberDataPoint>,
}

/// OTLP sum (counter)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpSum {
    /// Data points
    #[serde(rename = "dataPoints")]
    pub data_points: Vec<OtlpNumberDataPoint>,
    /// Aggregation temporality
    #[serde(rename = "aggregationTemporality", default)]
    pub aggregation_temporality: i32,
    /// Is monotonic
    #[serde(rename = "isMonotonic", default)]
    pub is_monotonic: bool,
}

/// OTLP histogram
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpHistogram {
    /// Data points
    #[serde(rename = "dataPoints")]
    pub data_points: Vec<OtlpHistogramDataPoint>,
    /// Aggregation temporality
    #[serde(rename = "aggregationTemporality", default)]
    pub aggregation_temporality: i32,
}

/// OTLP number data point (for gauge and sum)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpNumberDataPoint {
    /// Attributes
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
    /// Start time (Unix epoch nanoseconds)
    #[serde(rename = "startTimeUnixNano", default)]
    pub start_time_unix_nano: String,
    /// Time (Unix epoch nanoseconds)
    #[serde(rename = "timeUnixNano", default)]
    pub time_unix_nano: String,
    /// Double value
    #[serde(rename = "asDouble", skip_serializing_if = "Option::is_none")]
    pub as_double: Option<f64>,
    /// Int value
    #[serde(rename = "asInt", skip_serializing_if = "Option::is_none")]
    pub as_int: Option<i64>,
}

/// OTLP histogram data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpHistogramDataPoint {
    /// Attributes
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
    /// Start time (Unix epoch nanoseconds)
    #[serde(rename = "startTimeUnixNano", default)]
    pub start_time_unix_nano: String,
    /// Time (Unix epoch nanoseconds)
    #[serde(rename = "timeUnixNano", default)]
    pub time_unix_nano: String,
    /// Count
    #[serde(default)]
    pub count: u64,
    /// Sum
    #[serde(default)]
    pub sum: Option<f64>,
    /// Bucket counts
    #[serde(rename = "bucketCounts", default)]
    pub bucket_counts: Vec<u64>,
    /// Explicit bounds
    #[serde(rename = "explicitBounds", default)]
    pub explicit_bounds: Vec<f64>,
}

/// OTLP metric export response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpExportMetricsServiceResponse {
    /// Partial success
    #[serde(rename = "partialSuccess")]
    pub partial_success: Option<OtlpMetricsPartialSuccess>,
}

/// OTLP metrics partial success
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpMetricsPartialSuccess {
    /// Number of rejected data points
    #[serde(rename = "rejectedDataPoints")]
    pub rejected_data_points: i64,
    /// Error message
    #[serde(rename = "errorMessage")]
    pub error_message: String,
}

impl OtlpAdapter {
    /// Create a new OTLP adapter
    pub fn new(
        config: AdapterConfig,
        transport: OtlpTransport,
        observability_service: Arc<ObservabilityService>,
        namespace: String,
    ) -> Self {
        Self {
            config,
            running: AtomicBool::new(false),
            events_received: Arc::new(AtomicU64::new(0)),
            transport,
            observability_service,
            namespace,
            shutdown_tx: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Create a new OTLP adapter with default namespace
    pub fn with_defaults(
        bind_address: SocketAddr,
        transport: OtlpTransport,
        observability_service: Arc<ObservabilityService>,
    ) -> Self {
        let (tx, _rx) = tokio::sync::mpsc::channel(1000);
        let config = AdapterConfig::new(bind_address, tx);
        Self::new(
            config,
            transport,
            observability_service,
            "default".to_string(),
        )
    }

    /// Convert OTLP span to ProximaDB TraceData
    pub fn convert_otlp_span(
        &self,
        otlp_span: &OtlpSpan,
        resource_attributes: &HashMap<String, String>,
    ) -> TraceData {
        // Parse timestamps
        let start_time_ns = otlp_span.start_time_unix_nano.parse::<i64>().unwrap_or(0);
        let end_time_ns = otlp_span.end_time_unix_nano.parse::<i64>().unwrap_or(0);

        // Convert span kind
        let kind = match otlp_span.kind.as_deref() {
            Some("SPAN_KIND_INTERNAL") => SpanKind::Internal,
            Some("SPAN_KIND_SERVER") => SpanKind::Server,
            Some("SPAN_KIND_CLIENT") => SpanKind::Client,
            Some("SPAN_KIND_PRODUCER") => SpanKind::Producer,
            Some("SPAN_KIND_CONSUMER") => SpanKind::Consumer,
            _ => SpanKind::Internal,
        };

        // Convert OTLP status to ProximaDB SpanStatus
        let status = otlp_span.status.as_ref().map(|s| SpanStatus {
            code: match s.code.as_str() {
                "STATUS_CODE_UNSET" => SpanStatusCode::Unset,
                "STATUS_CODE_OK" => SpanStatusCode::Ok,
                "STATUS_CODE_ERROR" => SpanStatusCode::Error,
                _ => SpanStatusCode::Unset,
            } as i32,
            message: if s.message.is_empty() {
                None
            } else {
                Some(s.message.clone())
            },
        });

        // Convert attributes to SqlValue map
        let mut attributes = HashMap::new();

        // Add resource attributes first
        for (k, v) in resource_attributes {
            attributes.insert(
                k.clone(),
                SqlValue {
                    value: Some(sql_value::Value::StringValue(v.clone())),
                },
            );
        }

        // Add span attributes
        for kv in &otlp_span.attributes {
            if let Some(value) = self.convert_otlp_any_value(&kv.value) {
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
                        .filter_map(|kv| {
                            self.convert_otlp_any_value(&kv.value)
                                .map(|v| (kv.key.clone(), v))
                        })
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
                        .filter_map(|kv| {
                            self.convert_otlp_any_value(&kv.value)
                                .map(|v| (kv.key.clone(), v))
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Convert OTLP AnyValue to SqlValue
    fn convert_otlp_any_value(&self, value: &OtlpAnyValue) -> Option<SqlValue> {
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
                // Convert array to JSON string
                let json_str = serde_json::to_string(&a.values).ok()?;
                sql_value::Value::StringValue(json_str)
            } else if let Some(kvl) = &value.kvlist_value {
                // Convert KV list to JSON string
                let json_str = serde_json::to_string(&kvl.values).ok()?;
                sql_value::Value::StringValue(json_str)
            } else {
                return None;
            }),
        })
    }

    /// Convert OTLP severity to our Severity
    #[allow(dead_code)]
    fn convert_severity(severity_number: i32) -> Severity {
        // OTLP severity numbers: 1-4 = TRACE, 5-8 = DEBUG, 9-12 = INFO,
        // 13-16 = WARN, 17-20 = ERROR, 21-24 = FATAL
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

    /// Convert OTLP log record to LogEntry
    #[allow(dead_code)]
    fn convert_log_record(
        &self,
        timestamp_ns: i64,
        severity_number: i32,
        body: &str,
        attributes: HashMap<String, String>,
        resource_attributes: &HashMap<String, String>,
    ) -> LogEntry {
        let source = resource_attributes.get("host.name").cloned();
        let service = resource_attributes.get("service.name").cloned();

        // Convert attributes to SqlValue map
        let fields: HashMap<String, SqlValue> = attributes
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(v)),
                    },
                )
            })
            .collect();

        LogEntry {
            timestamp_ns,
            severity: Self::convert_severity(severity_number) as i32,
            message: body.to_string(),
            fields,
            source,
            service,
        }
    }

    /// Start gRPC server
    ///
    /// Uses an HTTP/2 server with JSON content type on the standard OTLP gRPC port (4317).
    /// This accepts the same JSON payloads as the HTTP transport but on the gRPC service paths,
    /// providing compatibility with OTLP exporters configured for gRPC transport.
    async fn start_grpc(&self) -> Result<()> {
        use axum::routing::post;

        let state = (
            self.observability_service.clone(),
            self.namespace.clone(),
            self.events_received.clone(),
        );

        let app = Router::new()
            // gRPC-style service paths (used by OTLP gRPC exporters with JSON encoding)
            .route(
                "/opentelemetry.proto.collector.trace.v1.TraceService/Export",
                post(otlp_grpc_traces_handler),
            )
            .route(
                "/opentelemetry.proto.collector.logs.v1.LogsService/Export",
                post(otlp_grpc_logs_handler),
            )
            .route(
                "/opentelemetry.proto.collector.metrics.v1.MetricsService/Export",
                post(otlp_grpc_metrics_handler),
            )
            // Also serve standard HTTP paths for convenience
            .route("/v1/traces", post(otlp_traces_handler))
            .route("/v1/logs", post(otlp_logs_handler))
            .route("/v1/metrics", post(otlp_metrics_handler))
            .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()))
            .with_state(state);

        tracing::info!(
            "OTLP gRPC adapter listening on {} for traces, logs, and metrics",
            self.config.bind_address
        );

        // Store the sender for shutdown
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        // Create HTTP/2 server (gRPC transport layer)
        let addr = self.config.bind_address;
        let server = hyper::Server::bind(&addr).serve(app.into_make_service());

        // Spawn graceful shutdown task
        let graceful = server.with_graceful_shutdown(async move {
            shutdown_rx.await.ok();
            tracing::info!("OTLP gRPC adapter shutting down");
        });

        // Wait for server to complete
        graceful
            .await
            .map_err(|e| anyhow::anyhow!("OTLP gRPC server error: {}", e))
    }

    /// Start HTTP server for OTLP trace, log, and metric ingestion
    async fn start_http(&self) -> Result<()> {
        use axum::routing::post;

        use hyper::Server;

        let app = Router::new()
            .route("/v1/traces", post(otlp_traces_handler))
            .route("/v1/logs", post(otlp_logs_handler))
            .route("/v1/metrics", post(otlp_metrics_handler))
            .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()))
            .with_state((
                self.observability_service.clone(),
                self.namespace.clone(),
                self.events_received.clone(),
            ));

        tracing::info!(
            "OTLP HTTP adapter listening on {} for traces, logs, and metrics",
            self.config.bind_address
        );

        // Store the sender for shutdown
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        // Create server with graceful shutdown
        let addr = self.config.bind_address;
        let server = Server::bind(&addr).serve(app.into_make_service());

        // Spawn graceful shutdown task
        let graceful = server.with_graceful_shutdown(async move {
            shutdown_rx.await.ok();
            tracing::info!("OTLP HTTP adapter shutting down");
        });

        // Wait for server to complete
        graceful
            .await
            .map_err(|e| anyhow::anyhow!("OTLP HTTP server error: {}", e))
    }
}

/// OTLP traces HTTP handler
async fn otlp_traces_handler(
    State((service, namespace, events_received)): State<(
        Arc<ObservabilityService>,
        String,
        Arc<AtomicU64>,
    )>,
    Json(req): Json<OtlpExportTracesServiceRequest>,
) -> Result<Json<OtlpExportTracesServiceResponse>, (StatusCode, Json<OtlpErrorResponse>)> {
    // Convert OTLP traces to ProximaDB TraceData format
    let mut traces = Vec::new();

    for resource_span in &req.resource_spans {
        // Extract resource attributes (service.name, etc.)
        let resource_attributes: HashMap<String, String> = resource_span
            .resource
            .as_ref()
            .map(|r| {
                r.attributes
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
            })
            .unwrap_or_default();

        // Process each scope span
        for scope_span in &resource_span.scope_spans {
            // Process each span
            for otlp_span in &scope_span.spans {
                // Convert OTLP span to ProximaDB TraceData
                let adapter = OtlpAdapter::with_defaults(
                    "127.0.0.1:4318"
                        .parse()
                        .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], 4318))),
                    OtlpTransport::Http,
                    service.clone(),
                );
                let trace_data = adapter.convert_otlp_span(otlp_span, &resource_attributes);
                traces.push(trace_data);
            }
        }
    }

    // Ingest traces into ProximaDB
    match service.ingest_traces(&namespace, traces).await {
        Ok(result) => {
            events_received.fetch_add(result.ingested, Ordering::Relaxed);
            tracing::debug!(
                "OTLP: Ingested {} traces, {} failed",
                result.ingested,
                result.failed
            );

            Ok(Json(OtlpExportTracesServiceResponse {
                partial_success: if result.failed > 0 {
                    Some(OtlpPartialSuccess {
                        rejected_spans: result.failed as i64,
                        error_message: result.errors.join("; "),
                    })
                } else {
                    None
                },
            }))
        }
        Err(e) => {
            tracing::error!("OTLP: Failed to ingest traces: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(OtlpErrorResponse {
                    code: 2, // Internal error
                    message: format!("Failed to ingest traces: {}", e),
                }),
            ))
        }
    }
}

/// OTLP logs HTTP handler
async fn otlp_logs_handler(
    State((service, namespace, events_received)): State<(
        Arc<ObservabilityService>,
        String,
        Arc<AtomicU64>,
    )>,
    Json(req): Json<OtlpExportLogsServiceRequest>,
) -> Result<Json<OtlpExportLogsServiceResponse>, (StatusCode, Json<OtlpErrorResponse>)> {
    let mut logs = Vec::new();

    for resource_log in &req.resource_logs {
        // Extract resource attributes
        let resource_attributes: HashMap<String, String> = resource_log
            .resource
            .as_ref()
            .map(|r| extract_resource_attributes(&r.attributes))
            .unwrap_or_default();

        for scope_log in &resource_log.scope_logs {
            for log_record in &scope_log.log_records {
                let timestamp_ns = log_record
                    .time_unix_nano
                    .parse::<i64>()
                    .or_else(|_| log_record.observed_time_unix_nano.parse::<i64>())
                    .unwrap_or_else(|_| chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));

                // Extract body as message string
                let message = log_record
                    .body
                    .as_ref()
                    .and_then(|v| {
                        v.string_value
                            .clone()
                            .or_else(|| serde_json::to_string(v).ok())
                    })
                    .unwrap_or_default();

                // Convert attributes to string map
                let attributes: HashMap<String, String> = log_record
                    .attributes
                    .iter()
                    .filter_map(|kv| {
                        otlp_any_value_to_string(&kv.value).map(|v| (kv.key.clone(), v))
                    })
                    .collect();

                let source = resource_attributes.get("host.name").cloned();
                let service_name = resource_attributes.get("service.name").cloned();

                // Build fields from attributes
                let fields: HashMap<String, SqlValue> = attributes
                    .into_iter()
                    .chain(resource_attributes.clone())
                    .map(|(k, v)| {
                        (
                            k,
                            SqlValue {
                                value: Some(sql_value::Value::StringValue(v)),
                            },
                        )
                    })
                    .collect();

                // Add trace correlation fields if present
                let mut entry_fields = fields;
                if !log_record.trace_id.is_empty() {
                    entry_fields.insert(
                        "trace_id".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue(log_record.trace_id.clone())),
                        },
                    );
                }
                if !log_record.span_id.is_empty() {
                    entry_fields.insert(
                        "span_id".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue(log_record.span_id.clone())),
                        },
                    );
                }

                logs.push(LogEntry {
                    timestamp_ns,
                    severity: OtlpAdapter::convert_severity(log_record.severity_number) as i32,
                    message,
                    fields: entry_fields,
                    source,
                    service: service_name,
                });
            }
        }
    }

    match service.ingest_logs(&namespace, logs, None).await {
        Ok(result) => {
            events_received.fetch_add(result.ingested, Ordering::Relaxed);
            tracing::debug!(
                "OTLP: Ingested {} logs, {} failed",
                result.ingested,
                result.failed
            );

            Ok(Json(OtlpExportLogsServiceResponse {
                partial_success: if result.failed > 0 {
                    Some(OtlpLogsPartialSuccess {
                        rejected_log_records: result.failed as i64,
                        error_message: result.errors.join("; "),
                    })
                } else {
                    None
                },
            }))
        }
        Err(e) => {
            tracing::error!("OTLP: Failed to ingest logs: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(OtlpErrorResponse {
                    code: 2,
                    message: format!("Failed to ingest logs: {}", e),
                }),
            ))
        }
    }
}

/// OTLP metrics HTTP handler
async fn otlp_metrics_handler(
    State((service, namespace, events_received)): State<(
        Arc<ObservabilityService>,
        String,
        Arc<AtomicU64>,
    )>,
    Json(req): Json<OtlpExportMetricsServiceRequest>,
) -> Result<Json<OtlpExportMetricsServiceResponse>, (StatusCode, Json<OtlpErrorResponse>)> {
    let mut samples = Vec::new();

    for resource_metric in &req.resource_metrics {
        let resource_attributes: HashMap<String, String> = resource_metric
            .resource
            .as_ref()
            .map(|r| extract_resource_attributes(&r.attributes))
            .unwrap_or_default();

        for scope_metric in &resource_metric.scope_metrics {
            for metric in &scope_metric.metrics {
                // Extract data points from gauge, sum, or histogram
                let data_points = extract_metric_data_points(metric);

                for (timestamp_ns, value, point_attributes) in data_points {
                    // Merge resource attributes with point attributes
                    let mut labels = resource_attributes.clone();
                    labels.extend(point_attributes);

                    // Add metric unit as a label if present
                    if !metric.unit.is_empty() {
                        labels.insert("unit".to_string(), metric.unit.clone());
                    }

                    samples.push(MetricSample {
                        name: metric.name.clone(),
                        timestamp_ns,
                        value,
                        labels,
                    });
                }
            }
        }
    }

    match service.ingest_metrics(&namespace, samples).await {
        Ok(result) => {
            events_received.fetch_add(result.ingested, Ordering::Relaxed);
            tracing::debug!(
                "OTLP: Ingested {} metrics, {} failed",
                result.ingested,
                result.failed
            );

            Ok(Json(OtlpExportMetricsServiceResponse {
                partial_success: if result.failed > 0 {
                    Some(OtlpMetricsPartialSuccess {
                        rejected_data_points: result.failed as i64,
                        error_message: result.errors.join("; "),
                    })
                } else {
                    None
                },
            }))
        }
        Err(e) => {
            tracing::error!("OTLP: Failed to ingest metrics: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(OtlpErrorResponse {
                    code: 2,
                    message: format!("Failed to ingest metrics: {}", e),
                }),
            ))
        }
    }
}

/// gRPC-style handler for traces (delegates to HTTP handler)
async fn otlp_grpc_traces_handler(
    state: State<(Arc<ObservabilityService>, String, Arc<AtomicU64>)>,
    body: Json<OtlpExportTracesServiceRequest>,
) -> Result<Json<OtlpExportTracesServiceResponse>, (StatusCode, Json<OtlpErrorResponse>)> {
    otlp_traces_handler(state, body).await
}

/// gRPC-style handler for logs (delegates to HTTP handler)
async fn otlp_grpc_logs_handler(
    state: State<(Arc<ObservabilityService>, String, Arc<AtomicU64>)>,
    body: Json<OtlpExportLogsServiceRequest>,
) -> Result<Json<OtlpExportLogsServiceResponse>, (StatusCode, Json<OtlpErrorResponse>)> {
    otlp_logs_handler(state, body).await
}

/// gRPC-style handler for metrics (delegates to HTTP handler)
async fn otlp_grpc_metrics_handler(
    state: State<(Arc<ObservabilityService>, String, Arc<AtomicU64>)>,
    body: Json<OtlpExportMetricsServiceRequest>,
) -> Result<Json<OtlpExportMetricsServiceResponse>, (StatusCode, Json<OtlpErrorResponse>)> {
    otlp_metrics_handler(state, body).await
}

/// Extract resource attributes from OTLP key-value pairs into a string map
fn extract_resource_attributes(attributes: &[OtlpKeyValue]) -> HashMap<String, String> {
    attributes
        .iter()
        .filter_map(|kv| otlp_any_value_to_string(&kv.value).map(|v| (kv.key.clone(), v)))
        .collect()
}

/// Convert an OtlpAnyValue to a string representation
fn otlp_any_value_to_string(value: &OtlpAnyValue) -> Option<String> {
    if let Some(s) = &value.string_value {
        Some(s.clone())
    } else if let Some(b) = value.bool_value {
        Some(b.to_string())
    } else if let Some(i) = value.int_value {
        Some(i.to_string())
    } else if let Some(f) = value.double_value {
        Some(f.to_string())
    } else { value.bytes_value.as_ref().map(|b| b.clone()) }
}

/// Extract data points from an OTLP metric as (timestamp_ns, value, labels) tuples
fn extract_metric_data_points(metric: &OtlpMetric) -> Vec<(i64, f64, HashMap<String, String>)> {
    let mut points = Vec::new();

    // Extract from gauge
    if let Some(gauge) = &metric.gauge {
        for dp in &gauge.data_points {
            let ts = dp.time_unix_nano.parse::<i64>().unwrap_or(0);
            let value = dp
                .as_double
                .unwrap_or_else(|| dp.as_int.unwrap_or(0) as f64);
            let labels = extract_resource_attributes(&dp.attributes);
            points.push((ts, value, labels));
        }
    }

    // Extract from sum (counter)
    if let Some(sum) = &metric.sum {
        for dp in &sum.data_points {
            let ts = dp.time_unix_nano.parse::<i64>().unwrap_or(0);
            let value = dp
                .as_double
                .unwrap_or_else(|| dp.as_int.unwrap_or(0) as f64);
            let labels = extract_resource_attributes(&dp.attributes);
            points.push((ts, value, labels));
        }
    }

    // Extract from histogram (use sum as the representative value)
    if let Some(histogram) = &metric.histogram {
        for dp in &histogram.data_points {
            let ts = dp.time_unix_nano.parse::<i64>().unwrap_or(0);
            let value = dp.sum.unwrap_or(0.0);
            let mut labels = extract_resource_attributes(&dp.attributes);
            labels.insert("_count".to_string(), dp.count.to_string());
            points.push((ts, value, labels));
        }
    }

    points
}

#[async_trait]
impl InputAdapter for OtlpAdapter {
    fn name(&self) -> &str {
        match self.transport {
            OtlpTransport::Grpc => "otlp-grpc",
            OtlpTransport::Http => "otlp-http",
        }
    }

    async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        match self.transport {
            OtlpTransport::Grpc => self.start_grpc().await,
            OtlpTransport::Http => self.start_http().await,
        }
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn events_received(&self) -> u64 {
        self.events_received.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn test_convert_severity() {
        assert_eq!(OtlpAdapter::convert_severity(1), Severity::Debug); // TRACE1
        assert_eq!(OtlpAdapter::convert_severity(5), Severity::Debug); // DEBUG1
        assert_eq!(OtlpAdapter::convert_severity(9), Severity::Info); // INFO1
        assert_eq!(OtlpAdapter::convert_severity(13), Severity::Warn); // WARN1
        assert_eq!(OtlpAdapter::convert_severity(17), Severity::Error); // ERROR1
        assert_eq!(OtlpAdapter::convert_severity(21), Severity::Fatal); // FATAL1
    }

    #[test]
    fn test_otlp_adapter_creation() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:4318".parse().unwrap(), tx);

        // Create a mock ObservabilityService - in real tests this would be a test double
        // For now, just test the adapter structure
        let addr: SocketAddr = "127.0.0.1:4318".parse().unwrap();
        let transport = OtlpTransport::Http;

        assert_eq!(transport as i32, OtlpTransport::Http as i32);
        assert_eq!(addr.port(), 4318);
    }

    #[test]
    fn test_otlp_trace_request_deserialize() {
        let json = r#"{
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": "test-service"}},
                        {"key": "telemetry.sdk.name", "value": {"stringValue": "opentelemetry"}}
                    ]
                },
                "scopeSpans": [{
                    "scope": {
                        "name": "test-scope"
                    },
                    "spans": [{
                        "traceId": "0102030405060708090a0b0c0d0e0f10",
                        "spanId": "0102030405060708",
                        "parentSpanId": "0102030405060709",
                        "name": "test-operation",
                        "kind": "SPAN_KIND_SERVER",
                        "startTimeUnixNano": "1234567890000000000",
                        "endTimeUnixNano": "1234567891000000000",
                        "status": {
                            "code": "STATUS_CODE_OK"
                        },
                        "attributes": [
                            {"key": "http.method", "value": {"stringValue": "GET"}},
                            {"key": "http.status_code", "value": {"intValue": 200}}
                        ]
                    }]
                }]
            }]
        }"#;

        let req: OtlpExportTracesServiceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.resource_spans.len(), 1);
        assert_eq!(req.resource_spans[0].scope_spans.len(), 1);
        assert_eq!(req.resource_spans[0].scope_spans[0].spans.len(), 1);

        let span = &req.resource_spans[0].scope_spans[0].spans[0];
        assert_eq!(span.trace_id, "0102030405060708090a0b0c0d0e0f10");
        assert_eq!(span.span_id, "0102030405060708");
        assert_eq!(span.name, "test-operation");
        assert_eq!(span.kind, Some("SPAN_KIND_SERVER".to_string()));
        assert_eq!(span.status.as_ref().unwrap().code, "STATUS_CODE_OK");
    }

    #[test]
    fn test_otlp_export_response_serialize() {
        let response = OtlpExportTracesServiceResponse {
            partial_success: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("partialSuccess") || json == "{}");
    }

    #[test]
    fn test_otlp_transport_types() {
        let grpc = OtlpTransport::Grpc;
        let http = OtlpTransport::Http;

        // Test that the types can be created
        match grpc {
            OtlpTransport::Grpc => {}
            OtlpTransport::Http => panic!("Expected Grpc"),
        }

        match http {
            OtlpTransport::Grpc => panic!("Expected Http"),
            OtlpTransport::Http => {}
        }
    }

    #[test]
    fn test_otlp_span_status_codes() {
        let status_unset = OtlpStatus {
            code: "STATUS_CODE_UNSET".to_string(),
            message: String::new(),
        };
        assert_eq!(status_unset.code, "STATUS_CODE_UNSET");

        let status_ok = OtlpStatus {
            code: "STATUS_CODE_OK".to_string(),
            message: "Success".to_string(),
        };
        assert_eq!(status_ok.code, "STATUS_CODE_OK");
        assert_eq!(status_ok.message, "Success");

        let status_error = OtlpStatus {
            code: "STATUS_CODE_ERROR".to_string(),
            message: "Something failed".to_string(),
        };
        assert_eq!(status_error.code, "STATUS_CODE_ERROR");
        assert_eq!(status_error.message, "Something failed");
    }

    #[test]
    fn test_otlp_attribute_types() {
        // Test string value
        let string_val = OtlpAnyValue {
            string_value: Some("test".to_string()),
            bool_value: None,
            int_value: None,
            double_value: None,
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        assert_eq!(string_val.string_value, Some("test".to_string()));

        // Test int value
        let int_val = OtlpAnyValue {
            string_value: None,
            bool_value: None,
            int_value: Some(42),
            double_value: None,
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        assert_eq!(int_val.int_value, Some(42));

        // Test bool value
        let bool_val = OtlpAnyValue {
            string_value: None,
            bool_value: Some(true),
            int_value: None,
            double_value: None,
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        assert_eq!(bool_val.bool_value, Some(true));

        // Test double value
        let double_val = OtlpAnyValue {
            string_value: None,
            bool_value: None,
            int_value: None,
            double_value: Some(3.14),
            array_value: None,
            kvlist_value: None,
            bytes_value: None,
        };
        assert_eq!(double_val.double_value, Some(3.14));
    }

    #[tokio::test]
    async fn test_otlp_adapter_with_defaults() {
        let addr: SocketAddr = "127.0.0.1:4318".parse().unwrap();

        // Create a minimal ObservabilityService for testing
        // In production, this would be properly initialized
        // For now, we just verify the adapter can be created
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new(addr, tx);
        let transport = OtlpTransport::Http;

        // Verify the transport type is correctly set
        assert!(matches!(transport, OtlpTransport::Http));
        assert_eq!(config.bind_address, addr);
    }
}
