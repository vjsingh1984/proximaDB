// Observability module for Cloud SIEM / Datadog-like capabilities
//
// # Status: BETA - Limited Production Support
//
// This module is in beta status with several production-ready features and
// some experimental capabilities still under development.
//
// ## Production Ready Features
// - Log ingestion via HTTP/JSON
// - Syslog parsing (RFC 3164/5424)
// - Basic metric storage and aggregation
// - CEF/LEEF parsing (ArcSight/IBM QRadar formats)
// - OCSF event parsing
// - Fluent adapter: FULLY IMPLEMENTED (MessagePack parsing complete)
// - Full-text search with Tantivy (BM25 ranking)
// - OTLP adapter: HTTP/JSON transport fully implemented with comprehensive tests
//
// ## Experimental Features (Use with Caution)
// - OTLP gRPC transport: Not implemented (use HTTP/JSON transport instead)
// - High-throughput ingestion: Not benchmarked for >10K logs/sec
// - Alerting engine: Framework only, no production-tested rules
// - Storage partitioning: Basic implementation, needs validation
// - Distributed tracing: Span assembly not production-optimized
//
// ## Recommended Production Setup
//
// For high-scale production observability:
//
// 1. **Ingestion Layer** (Mature tools):
//    - Vector (https://vector.dev): High-performance log/metric collection
//    - OpenTelemetry Collector: For OTLP and distributed tracing
//    - Fluent Bit: Forward logs via TCP/HTTP
//
// 2. **Storage** (ProximaDB):
//    - Use as vector store backend for semantic log search
//    - Store full-text indexed logs for fast pattern matching
//    - Store metrics for time-series queries
//
// 3. **Visualization**:
//    - Grafana: Dashboards and alerting
//    - Kibana: Log analysis
//    - Jaeger/Zipkin UI: Distributed tracing
//
// ## Performance Targets (When using mature ingestion tools)
// - Log ingestion: >10K logs/second (via HTTP/JSON)
// - Full-text search: <100ms for 1M logs
// - Metric queries: <50ms for aggregation over 10K samples
//
// ---
//
// ## Legacy Documentation (Archived)
//
// Provides high-throughput ingestion and querying for:
// - Logs (structured and unstructured)
// - Metrics (time-series with aggregation)
// - Traces (distributed tracing with span assembly)
//
// Ingestion formats:
// - OTLP (OpenTelemetry Protocol)
// - Syslog (RFC 3164/5424)
// - Fluent (Fluent Bit/Fluentd forward protocol)
// - CEF/LEEF (ArcSight and IBM formats)
// - OCSF (Open Cybersecurity Schema Framework)
// - JSON over HTTP

pub mod alerting;
pub mod audit;
pub mod ingestion;
pub mod query;
pub mod storage;

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::proto::proximadb_v1::{
    IngestionFormat, LogEntry, MetricSample, ObservabilityNamespaceConfig, Severity,
};

pub use self::ingestion::ObservabilityIngester;
pub use self::query::ObservabilityQueryEngine;
pub use self::storage::ObservabilityStorage;

/// Observability service - main entry point
pub struct ObservabilityService {
    /// Ingestion pipeline
    ingester: Arc<ObservabilityIngester>,
    /// Query engine
    query_engine: Arc<ObservabilityQueryEngine>,
    /// Storage layer
    storage: Arc<ObservabilityStorage>,
    /// Namespace configurations
    namespaces: RwLock<std::collections::HashMap<String, NamespaceState>>,
}

/// State for a single namespace
struct NamespaceState {
    /// Configuration
    config: ObservabilityNamespaceConfig,
    /// Created timestamp
    created_at_ns: i64,
    /// Last ingestion timestamp
    last_ingest_at_ns: Option<i64>,
    /// Total events ingested
    total_events: u64,
}

impl ObservabilityService {
    /// Create a new observability service
    pub async fn new(storage: Arc<ObservabilityStorage>) -> Result<Self> {
        let ingester = Arc::new(ObservabilityIngester::new(storage.clone()).await?);
        let query_engine = Arc::new(ObservabilityQueryEngine::new(storage.clone()));

        Ok(Self {
            ingester,
            query_engine,
            storage,
            namespaces: RwLock::new(std::collections::HashMap::new()),
        })
    }

    /// Create a new namespace for observability data
    pub async fn create_namespace(&self, config: ObservabilityNamespaceConfig) -> Result<String> {
        let name = config.name.clone();
        info!("Creating observability namespace: {}", name);

        // Check if namespace already exists
        {
            let namespaces = self.namespaces.read().await;
            if namespaces.contains_key(&name) {
                return Err(anyhow::anyhow!("Namespace '{}' already exists", name));
            }
        }

        // Initialize storage for namespace
        self.storage.create_namespace(&name, &config).await?;

        // Store namespace state
        {
            let mut namespaces = self.namespaces.write().await;
            namespaces.insert(
                name.clone(),
                NamespaceState {
                    config,
                    created_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                    last_ingest_at_ns: None,
                    total_events: 0,
                },
            );
        }

        info!("Created observability namespace: {}", name);
        Ok(name)
    }

    /// Ingest a batch of logs
    pub async fn ingest_logs(
        &self,
        namespace: &str,
        logs: Vec<LogEntry>,
        format: Option<IngestionFormat>,
    ) -> Result<IngestResult> {
        debug!("Ingesting {} logs to namespace {}", logs.len(), namespace);

        // Verify namespace exists
        {
            let namespaces = self.namespaces.read().await;
            if !namespaces.contains_key(namespace) {
                return Err(anyhow::anyhow!("Namespace '{}' not found", namespace));
            }
        }

        let result = self.ingester.ingest_logs(namespace, logs, format).await?;

        // Update namespace stats
        {
            let mut namespaces = self.namespaces.write().await;
            if let Some(state) = namespaces.get_mut(namespace) {
                state.last_ingest_at_ns =
                    Some(chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
                state.total_events += result.ingested;
            }
        }

        Ok(result)
    }

    /// Ingest a batch of metrics
    pub async fn ingest_metrics(
        &self,
        namespace: &str,
        metrics: Vec<MetricSample>,
    ) -> Result<IngestResult> {
        debug!(
            "Ingesting {} metrics to namespace {}",
            metrics.len(),
            namespace
        );

        // Verify namespace exists
        {
            let namespaces = self.namespaces.read().await;
            if !namespaces.contains_key(namespace) {
                return Err(anyhow::anyhow!("Namespace '{}' not found", namespace));
            }
        }

        let result = self.ingester.ingest_metrics(namespace, metrics).await?;

        // Update namespace stats
        {
            let mut namespaces = self.namespaces.write().await;
            if let Some(state) = namespaces.get_mut(namespace) {
                state.last_ingest_at_ns =
                    Some(chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
                state.total_events += result.ingested;
            }
        }

        Ok(result)
    }

    /// Query logs
    pub async fn query_logs(
        &self,
        namespace: &str,
        params: LogQueryParams,
    ) -> Result<LogQueryResult> {
        self.query_engine.query_logs(namespace, params).await
    }

    /// Aggregate metrics
    pub async fn aggregate_metrics(
        &self,
        namespace: &str,
        params: MetricAggParams,
    ) -> Result<MetricAggResult> {
        self.query_engine.aggregate_metrics(namespace, params).await
    }

    /// Get namespace stats
    pub async fn get_namespace_stats(&self, namespace: &str) -> Result<NamespaceStats> {
        let namespaces = self.namespaces.read().await;
        let state = namespaces
            .get(namespace)
            .ok_or_else(|| anyhow::anyhow!("Namespace '{}' not found", namespace))?;

        Ok(NamespaceStats {
            name: namespace.to_string(),
            total_events: state.total_events,
            created_at_ns: state.created_at_ns,
            last_ingest_at_ns: state.last_ingest_at_ns,
        })
    }

    /// Delete a namespace
    pub async fn delete_namespace(&self, namespace: &str) -> Result<()> {
        info!("Deleting observability namespace: {}", namespace);

        // Verify namespace exists
        {
            let namespaces = self.namespaces.read().await;
            if !namespaces.contains_key(namespace) {
                return Err(anyhow::anyhow!("Namespace '{}' not found", namespace));
            }
        }

        // Delete from storage (handles WAL write)
        self.storage.delete_namespace(namespace).await?;

        // Remove from in-memory state
        {
            let mut namespaces = self.namespaces.write().await;
            namespaces.remove(namespace);
        }

        info!("Deleted observability namespace: {}", namespace);
        Ok(())
    }

    /// List all namespaces
    pub async fn list_namespaces(&self) -> Vec<NamespaceInfo> {
        let namespaces = self.namespaces.read().await;
        namespaces
            .iter()
            .map(|(name, state)| NamespaceInfo {
                name: name.clone(),
                created_at_ns: state.created_at_ns,
                last_ingest_at_ns: state.last_ingest_at_ns,
                total_events: state.total_events,
            })
            .collect()
    }

    /// Query metrics with time range and label filters
    pub async fn query_metrics(
        &self,
        namespace: &str,
        metric_name: &str,
        start_time_ns: i64,
        end_time_ns: i64,
        labels: &std::collections::HashMap<String, String>,
        limit: u32,
    ) -> Result<MetricQueryResult> {
        let start = std::time::Instant::now();

        // Get raw metrics from storage
        let mut samples = self
            .storage
            .query_metrics(namespace, metric_name, start_time_ns, end_time_ns)
            .await?;

        // Apply label filters
        if !labels.is_empty() {
            samples.retain(|sample| {
                labels
                    .iter()
                    .all(|(k, v)| sample.labels.get(k).map_or(false, |sv| sv == v))
            });
        }

        // Apply limit
        if limit > 0 && samples.len() > limit as usize {
            samples.truncate(limit as usize);
        }

        let query_time_ms = start.elapsed().as_millis() as u64;

        Ok(MetricQueryResult {
            samples,
            query_time_ms,
        })
    }

    /// Ingest a batch of trace spans
    pub async fn ingest_traces(
        &self,
        namespace: &str,
        traces: Vec<crate::proto::proximadb_v1::TraceData>,
    ) -> Result<IngestResult> {
        use crate::observability::storage::traces::TraceSpan;

        debug!(
            "Ingesting {} traces to namespace {}",
            traces.len(),
            namespace
        );

        // Verify namespace exists
        {
            let namespaces = self.namespaces.read().await;
            if !namespaces.contains_key(namespace) {
                return Err(anyhow::anyhow!("Namespace '{}' not found", namespace));
            }
        }

        let start = std::time::Instant::now();
        let mut ingested = 0u64;
        let mut failed = 0u64;
        let mut errors = Vec::new();

        // TraceData in proto is a single span, convert and write
        for trace_data in traces {
            // Extract service name from attributes or use empty string
            let service_name = trace_data
                .attributes
                .get("service.name")
                .and_then(|v| v.value.as_ref())
                .and_then(|v| match v {
                    crate::proto::proximadb_v1::sql_value::Value::StringValue(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();

            // Convert SqlValue attributes to String attributes
            let attributes: std::collections::HashMap<String, String> = trace_data
                .attributes
                .iter()
                .filter_map(|(k, v)| {
                    v.value.as_ref().and_then(|val| match val {
                        crate::proto::proximadb_v1::sql_value::Value::StringValue(s) => {
                            Some((k.clone(), s.clone()))
                        }
                        crate::proto::proximadb_v1::sql_value::Value::Int64Value(i) => {
                            Some((k.clone(), i.to_string()))
                        }
                        crate::proto::proximadb_v1::sql_value::Value::NumberValue(f) => {
                            Some((k.clone(), f.to_string()))
                        }
                        crate::proto::proximadb_v1::sql_value::Value::BoolValue(b) => {
                            Some((k.clone(), b.to_string()))
                        }
                        _ => None,
                    })
                })
                .collect();

            // Extract status code from SpanStatus message
            let (status_code, status_message) = trace_data
                .status
                .map(|s| (s.code, s.message.unwrap_or_default()))
                .unwrap_or((0, String::new())); // 0 = Unset

            let span = TraceSpan {
                trace_id: trace_data.trace_id,
                span_id: trace_data.span_id,
                parent_span_id: trace_data.parent_span_id.unwrap_or_default(),
                name: trace_data.name,
                service_name,
                start_time_ns: trace_data.start_time_ns,
                end_time_ns: trace_data.end_time_ns,
                attributes,
                status: status_code,
                status_message,
            };

            match self.storage.write_span(namespace, &span).await {
                Ok(()) => ingested += 1,
                Err(e) => {
                    failed += 1;
                    errors.push(e.to_string());
                }
            }
        }

        // Update namespace stats
        {
            let mut namespaces = self.namespaces.write().await;
            if let Some(state) = namespaces.get_mut(namespace) {
                state.last_ingest_at_ns =
                    Some(chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
                state.total_events += ingested;
            }
        }

        Ok(IngestResult {
            ingested,
            failed,
            errors,
            processing_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Query traces with filters
    pub async fn query_traces(
        &self,
        namespace: &str,
        params: TraceQueryParams,
    ) -> Result<TraceQueryResult> {
        let start = std::time::Instant::now();

        // If trace_id is specified, fetch that specific trace
        if let Some(trace_id) = &params.trace_id {
            let spans = self.storage.query_trace(namespace, trace_id).await?;

            let query_time_ms = start.elapsed().as_millis() as u64;

            // Convert spans to TraceData
            let traces = spans.into_iter().map(Self::span_to_trace_data).collect();

            return Ok(TraceQueryResult {
                traces,
                next_cursor: None,
                query_time_ms,
            });
        }

        // Query by time range or service
        let summaries = if let Some(service) = &params.service {
            self.storage
                .query_traces_by_service(
                    namespace,
                    service,
                    params.start_time_ns,
                    params.end_time_ns,
                    params.limit as usize,
                )
                .await?
        } else {
            self.storage
                .query_traces_by_time(
                    namespace,
                    params.start_time_ns,
                    params.end_time_ns,
                    params.limit as usize,
                )
                .await?
        };

        // Apply additional filters and convert to TraceData
        let mut traces = Vec::new();

        for summary in summaries {
            // Apply operation filter
            if let Some(op) = &params.operation {
                if &summary.root_operation != op {
                    continue;
                }
            }

            // Apply min duration filter
            if let Some(min_dur) = params.min_duration_ns {
                if summary.duration_ns < min_dur {
                    continue;
                }
            }

            // Fetch all spans for this trace
            let spans = self
                .storage
                .query_trace(namespace, &summary.trace_id)
                .await?;

            // Apply status filter if specified
            if let Some(status) = params.status {
                let has_matching_status = spans.iter().any(|s| s.status == status);
                if !has_matching_status {
                    continue;
                }
            }

            // Convert spans to TraceData and add to results
            for span in spans {
                traces.push(Self::span_to_trace_data(span));
            }

            if traces.len() >= params.limit as usize {
                break;
            }
        }

        let query_time_ms = start.elapsed().as_millis() as u64;

        Ok(TraceQueryResult {
            traces,
            next_cursor: None,
            query_time_ms,
        })
    }

    /// Get a single trace by ID (all spans)
    pub async fn get_trace(&self, namespace: &str, trace_id: &str) -> Result<GetTraceResult> {
        let spans = self.storage.query_trace(namespace, trace_id).await?;

        let complete = !spans.is_empty() && spans.iter().any(|s| s.parent_span_id.is_empty()); // Has root span

        let traces = spans.into_iter().map(Self::span_to_trace_data).collect();

        Ok(GetTraceResult {
            spans: traces,
            complete,
        })
    }

    /// Convert internal TraceSpan to proto TraceData
    fn span_to_trace_data(
        span: crate::observability::storage::traces::TraceSpan,
    ) -> crate::proto::proximadb_v1::TraceData {
        // Convert String attributes back to SqlValue attributes
        let mut attributes = std::collections::HashMap::new();
        for (k, v) in span.attributes {
            attributes.insert(
                k,
                crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(v)),
                },
            );
        }

        // Add service.name attribute
        if !span.service_name.is_empty() {
            attributes.insert(
                "service.name".to_string(),
                crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        span.service_name,
                    )),
                },
            );
        }

        // Convert status to SpanStatus message
        let status = Some(crate::proto::proximadb_v1::SpanStatus {
            code: span.status,
            message: if span.status_message.is_empty() {
                None
            } else {
                Some(span.status_message)
            },
        });

        crate::proto::proximadb_v1::TraceData {
            trace_id: span.trace_id,
            span_id: span.span_id,
            parent_span_id: if span.parent_span_id.is_empty() {
                None
            } else {
                Some(span.parent_span_id)
            },
            name: span.name,
            kind: crate::proto::proximadb_v1::SpanKind::Unspecified as i32,
            start_time_ns: span.start_time_ns,
            end_time_ns: span.end_time_ns,
            status,
            attributes,
            events: vec![],
            links: vec![],
        }
    }
}

/// Parameters for trace queries
#[derive(Debug, Clone)]
pub struct TraceQueryParams {
    /// Start of time range (nanoseconds since epoch)
    pub start_time_ns: i64,
    /// End of time range (nanoseconds since epoch)
    pub end_time_ns: i64,
    /// Filter by trace ID
    pub trace_id: Option<String>,
    /// Filter by service
    pub service: Option<String>,
    /// Filter by operation name
    pub operation: Option<String>,
    /// Minimum duration filter
    pub min_duration_ns: Option<i64>,
    /// Status filter (SpanStatusCode as i32)
    pub status: Option<i32>,
    /// Maximum results
    pub limit: u32,
    /// Cursor for pagination
    pub cursor: Option<String>,
}

/// Result of a trace query
#[derive(Debug, Clone)]
pub struct TraceQueryResult {
    /// Matched traces (as spans)
    pub traces: Vec<crate::proto::proximadb_v1::TraceData>,
    /// Cursor for next page
    pub next_cursor: Option<String>,
    /// Query time in milliseconds
    pub query_time_ms: u64,
}

/// Result of getting a single trace
#[derive(Debug, Clone)]
pub struct GetTraceResult {
    /// All spans in the trace
    pub spans: Vec<crate::proto::proximadb_v1::TraceData>,
    /// Whether the trace is complete (has root span)
    pub complete: bool,
}

/// Result of an ingestion operation
#[derive(Debug, Clone, Default)]
pub struct IngestResult {
    /// Number of events successfully ingested
    pub ingested: u64,
    /// Number of events that failed
    pub failed: u64,
    /// Error messages for failed events
    pub errors: Vec<String>,
    /// Processing time in milliseconds
    pub processing_time_ms: u64,
}

/// Parameters for log queries
#[derive(Debug, Clone)]
pub struct LogQueryParams {
    /// Start of time range (nanoseconds since epoch)
    pub start_time_ns: i64,
    /// End of time range (nanoseconds since epoch)
    pub end_time_ns: i64,
    /// Filter query (Datadog-style or SQL-like)
    pub query: Option<String>,
    /// Severity filter
    pub severities: Vec<Severity>,
    /// Service filter
    pub services: Vec<String>,
    /// Source filter
    pub sources: Vec<String>,
    /// Maximum results
    pub limit: u32,
    /// Cursor for pagination
    pub cursor: Option<String>,
}

/// Result of a log query
#[derive(Debug, Clone)]
pub struct LogQueryResult {
    /// Matched log entries
    pub logs: Vec<LogEntry>,
    /// Cursor for next page
    pub next_cursor: Option<String>,
    /// Total matched (if countable)
    pub total_matched: Option<u64>,
    /// Query time in milliseconds
    pub query_time_ms: u64,
}

/// Parameters for metric aggregation
#[derive(Debug, Clone)]
pub struct MetricAggParams {
    /// Metric name
    pub metric_name: String,
    /// Start of time range (nanoseconds since epoch)
    pub start_time_ns: i64,
    /// End of time range (nanoseconds since epoch)
    pub end_time_ns: i64,
    /// Aggregation function
    pub aggregation: MetricAggregation,
    /// Step/resolution in seconds
    pub step_seconds: u32,
    /// Label filters
    pub label_filters: std::collections::HashMap<String, String>,
    /// Group by labels
    pub group_by: Vec<String>,
}

/// Metric aggregation function
#[derive(Debug, Clone, Copy)]
pub enum MetricAggregation {
    Avg,
    Sum,
    Min,
    Max,
    Count,
    Rate,
    P50,
    P90,
    P95,
    P99,
}

/// Result of a metric aggregation
#[derive(Debug, Clone)]
pub struct MetricAggResult {
    /// Time series results
    pub series: Vec<TimeSeriesResult>,
    /// Query time in milliseconds
    pub query_time_ms: u64,
}

/// Single time series result
#[derive(Debug, Clone)]
pub struct TimeSeriesResult {
    /// Labels identifying this series
    pub labels: std::collections::HashMap<String, String>,
    /// Data points
    pub points: Vec<DataPoint>,
}

/// Single data point
#[derive(Debug, Clone)]
pub struct DataPoint {
    /// Timestamp (nanoseconds since epoch)
    pub timestamp_ns: i64,
    /// Value
    pub value: f64,
}

/// Namespace statistics
#[derive(Debug, Clone)]
pub struct NamespaceStats {
    /// Namespace name
    pub name: String,
    /// Total events ingested
    pub total_events: u64,
    /// Created timestamp
    pub created_at_ns: i64,
    /// Last ingestion timestamp
    pub last_ingest_at_ns: Option<i64>,
}

/// Namespace info (for list_namespaces)
#[derive(Debug, Clone)]
pub struct NamespaceInfo {
    /// Namespace name
    pub name: String,
    /// Created timestamp (nanoseconds since epoch)
    pub created_at_ns: i64,
    /// Last ingestion timestamp (nanoseconds since epoch)
    pub last_ingest_at_ns: Option<i64>,
    /// Total events ingested
    pub total_events: u64,
}

/// Result of a metric query
#[derive(Debug, Clone)]
pub struct MetricQueryResult {
    /// Metric samples matching the query
    pub samples: Vec<MetricSample>,
    /// Query time in milliseconds
    pub query_time_ms: u64,
}

// =============================================================================
// TRAIT IMPLEMENTATION: ObservabilityStorageOperations
// =============================================================================
// This implements the SOLID-compliant trait interface for observability operations,
// bridging the existing ObservabilityService to the multi-model storage traits.

use crate::storage::traits::{
    DataPointValue as TraitDataPoint, IngestResult as TraitIngestResult,
    LogQueryResult as TraitLogQueryResult, MetricAggregationParams as TraitMetricAggParams,
    MetricAggregationResult as TraitMetricAggResult, NamespaceInfo as TraitNamespaceInfo,
    ObservabilityStorageOperations, TimeSeriesData as TraitTimeSeriesData,
};
use async_trait::async_trait;

/// Convert internal IngestResult to trait IngestResult
fn to_trait_ingest_result(result: &IngestResult) -> TraitIngestResult {
    TraitIngestResult {
        ingested: result.ingested,
        failed: result.failed,
        errors: result.errors.clone(),
        processing_time_ms: result.processing_time_ms,
    }
}

/// Convert internal LogQueryResult to trait LogQueryResult
fn to_trait_log_query_result(result: &LogQueryResult) -> TraitLogQueryResult {
    TraitLogQueryResult {
        logs: result.logs.clone(),
        next_cursor: result.next_cursor.clone(),
        total_matched: result.total_matched.unwrap_or(0),
        query_time_ms: result.query_time_ms,
    }
}

/// Convert internal MetricAggResult to trait MetricAggregationResult
fn to_trait_metric_agg_result(result: &MetricAggResult) -> TraitMetricAggResult {
    TraitMetricAggResult {
        series: result
            .series
            .iter()
            .map(|s| TraitTimeSeriesData {
                labels: s.labels.clone(),
                points: s
                    .points
                    .iter()
                    .map(|p| TraitDataPoint {
                        timestamp_ns: p.timestamp_ns,
                        value: p.value,
                    })
                    .collect(),
            })
            .collect(),
        query_time_ms: result.query_time_ms,
    }
}

#[async_trait]
impl ObservabilityStorageOperations for ObservabilityService {
    async fn ingest_logs(
        &self,
        namespace: &str,
        logs: Vec<crate::proto::proximadb_v1::LogEntry>,
    ) -> Result<TraitIngestResult> {
        let result = ObservabilityService::ingest_logs(self, namespace, logs, None).await?;
        Ok(to_trait_ingest_result(&result))
    }

    async fn ingest_metrics(
        &self,
        namespace: &str,
        metrics: Vec<crate::proto::proximadb_v1::MetricSample>,
    ) -> Result<TraitIngestResult> {
        let result = ObservabilityService::ingest_metrics(self, namespace, metrics).await?;
        Ok(to_trait_ingest_result(&result))
    }

    async fn ingest_traces(
        &self,
        namespace: &str,
        traces: Vec<crate::proto::proximadb_v1::TraceData>,
    ) -> Result<TraitIngestResult> {
        use crate::observability::storage::traces::TraceSpan;

        let start = std::time::Instant::now();
        let mut ingested = 0u64;
        let mut failed = 0u64;
        let mut errors = Vec::new();

        // TraceData in proto is a single span, convert and write
        for trace_data in traces {
            // Extract service name from attributes or use empty string
            let service_name = trace_data
                .attributes
                .get("service.name")
                .and_then(|v| v.value.as_ref())
                .and_then(|v| match v {
                    crate::proto::proximadb_v1::sql_value::Value::StringValue(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();

            // Convert SqlValue attributes to String attributes
            let attributes: std::collections::HashMap<String, String> = trace_data
                .attributes
                .iter()
                .filter_map(|(k, v)| {
                    v.value.as_ref().and_then(|val| match val {
                        crate::proto::proximadb_v1::sql_value::Value::StringValue(s) => {
                            Some((k.clone(), s.clone()))
                        }
                        crate::proto::proximadb_v1::sql_value::Value::Int64Value(i) => {
                            Some((k.clone(), i.to_string()))
                        }
                        crate::proto::proximadb_v1::sql_value::Value::NumberValue(f) => {
                            Some((k.clone(), f.to_string()))
                        }
                        crate::proto::proximadb_v1::sql_value::Value::BoolValue(b) => {
                            Some((k.clone(), b.to_string()))
                        }
                        _ => None,
                    })
                })
                .collect();

            // Extract status code from SpanStatus message
            let (status_code, status_message) = trace_data
                .status
                .map(|s| (s.code, s.message.unwrap_or_default()))
                .unwrap_or((0, String::new())); // 0 = Unset

            let span = TraceSpan {
                trace_id: trace_data.trace_id,
                span_id: trace_data.span_id,
                parent_span_id: trace_data.parent_span_id.unwrap_or_default(),
                name: trace_data.name,
                service_name,
                start_time_ns: trace_data.start_time_ns,
                end_time_ns: trace_data.end_time_ns,
                attributes,
                status: status_code,
                status_message,
            };

            match self.storage.write_span(namespace, &span).await {
                Ok(_) => ingested += 1,
                Err(e) => {
                    failed += 1;
                    errors.push(e.to_string());
                }
            }
        }

        Ok(TraitIngestResult {
            ingested,
            failed,
            errors,
            processing_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn query_logs(
        &self,
        namespace: &str,
        start_time_ns: i64,
        end_time_ns: i64,
        filter: Option<crate::proto::proximadb_v1::LogFilter>,
        limit: u32,
    ) -> Result<TraitLogQueryResult> {
        // Build query params from function parameters
        let params = LogQueryParams {
            start_time_ns,
            end_time_ns,
            query: None, // LogFilter doesn't have text_query, field_filters handle specific conditions
            severities: filter
                .as_ref()
                .map(|f| {
                    f.severities
                        .iter()
                        .filter_map(|s| crate::proto::proximadb_v1::Severity::try_from(*s).ok())
                        .collect()
                })
                .unwrap_or_default(),
            services: filter
                .as_ref()
                .map(|f| f.services.clone())
                .unwrap_or_default(),
            sources: filter
                .as_ref()
                .map(|f| f.sources.clone())
                .unwrap_or_default(),
            limit,
            cursor: None,
        };

        let result = ObservabilityService::query_logs(self, namespace, params).await?;
        Ok(to_trait_log_query_result(&result))
    }

    async fn aggregate_metrics(
        &self,
        namespace: &str,
        params: TraitMetricAggParams,
    ) -> Result<TraitMetricAggResult> {
        // Convert trait params to internal params
        let internal_params = MetricAggParams {
            metric_name: params.metric_name,
            start_time_ns: params.start_time_ns,
            end_time_ns: params.end_time_ns,
            aggregation: match params.aggregation {
                crate::proto::proximadb_v1::MetricAggregation::Avg => MetricAggregation::Avg,
                crate::proto::proximadb_v1::MetricAggregation::Sum => MetricAggregation::Sum,
                crate::proto::proximadb_v1::MetricAggregation::Min => MetricAggregation::Min,
                crate::proto::proximadb_v1::MetricAggregation::Max => MetricAggregation::Max,
                crate::proto::proximadb_v1::MetricAggregation::Count => MetricAggregation::Count,
                crate::proto::proximadb_v1::MetricAggregation::Rate => MetricAggregation::Rate,
                crate::proto::proximadb_v1::MetricAggregation::P50 => MetricAggregation::P50,
                crate::proto::proximadb_v1::MetricAggregation::P90 => MetricAggregation::P90,
                crate::proto::proximadb_v1::MetricAggregation::P95 => MetricAggregation::P95,
                crate::proto::proximadb_v1::MetricAggregation::P99 => MetricAggregation::P99,
                _ => MetricAggregation::Avg, // Default fallback
            },
            step_seconds: params.step_seconds,
            label_filters: params.label_filters,
            group_by: params.group_by,
        };

        let result =
            ObservabilityService::aggregate_metrics(self, namespace, internal_params).await?;
        Ok(to_trait_metric_agg_result(&result))
    }

    async fn query_traces(
        &self,
        namespace: &str,
        _start_time_ns: i64,
        _end_time_ns: i64,
        trace_id: Option<String>,
        _service: Option<String>,
        limit: u32,
    ) -> Result<Vec<crate::proto::proximadb_v1::TraceData>> {
        // Query spans from storage
        // TraceData in proto represents a single span, so we return spans as TraceData
        let spans = if let Some(tid) = trace_id {
            self.storage.query_trace(namespace, &tid).await?
        } else {
            // Query by time range not yet supported, return empty
            // Future enhancement: add time-range query to trace storage
            vec![]
        };

        // Convert TraceSpan to proto TraceData (each span becomes one TraceData)
        let results: Vec<crate::proto::proximadb_v1::TraceData> = spans
            .into_iter()
            .take(limit as usize)
            .map(|span| {
                // Convert String attributes back to SqlValue attributes
                let mut attributes = std::collections::HashMap::new();
                for (k, v) in span.attributes {
                    attributes.insert(
                        k,
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                v,
                            )),
                        },
                    );
                }

                // Add service.name attribute
                if !span.service_name.is_empty() {
                    attributes.insert(
                        "service.name".to_string(),
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                span.service_name,
                            )),
                        },
                    );
                }

                // Convert status i32 and message to SpanStatus message
                let status = Some(crate::proto::proximadb_v1::SpanStatus {
                    code: span.status,
                    message: if span.status_message.is_empty() {
                        None
                    } else {
                        Some(span.status_message)
                    },
                });

                crate::proto::proximadb_v1::TraceData {
                    trace_id: span.trace_id,
                    span_id: span.span_id,
                    parent_span_id: if span.parent_span_id.is_empty() {
                        None
                    } else {
                        Some(span.parent_span_id)
                    },
                    name: span.name,
                    kind: crate::proto::proximadb_v1::SpanKind::Unspecified as i32,
                    start_time_ns: span.start_time_ns,
                    end_time_ns: span.end_time_ns,
                    status,
                    attributes,
                    events: vec![],
                    links: vec![],
                }
            })
            .collect();

        Ok(results)
    }

    async fn create_namespace(
        &self,
        config: crate::proto::proximadb_v1::ObservabilityNamespaceConfig,
    ) -> Result<String> {
        ObservabilityService::create_namespace(self, config).await
    }

    async fn list_namespaces(&self) -> Result<Vec<TraitNamespaceInfo>> {
        let namespaces = self.namespaces.read().await;

        let mut result = Vec::new();
        for (name, state) in namespaces.iter() {
            // Get storage stats for counts
            let stats = self.storage.stats(name).await.ok();

            result.push(TraitNamespaceInfo {
                name: name.clone(),
                log_count: stats.as_ref().map(|s| s.log_count).unwrap_or(0),
                metric_count: stats.as_ref().map(|s| s.metric_series_count).unwrap_or(0),
                trace_count: stats.as_ref().map(|s| s.trace_count).unwrap_or(0),
                retention_config: state.config.retention.clone(),
            });
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ingest_result_default() {
        let result = IngestResult::default();
        assert_eq!(result.ingested, 0);
        assert_eq!(result.failed, 0);
    }
}
