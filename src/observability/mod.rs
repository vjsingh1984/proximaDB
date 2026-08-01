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

/// Alerting engine with rule-based evaluation and notification channels.
pub use proximadb_observability_engine::alerting;
/// Audit logging for compliance and security event tracking.
pub use proximadb_observability_engine::audit;
/// High-throughput ingestion pipeline with multi-format parsing.
pub mod ingestion;
/// Per-query I/O trace bus (C0 — co-design trace substrate). Task-local
/// accumulator of the physical-dimension quantities (object-store ops, bytes
/// moved, footer-cache outcomes, cross-AZ egress, compute-ms by engine) the
/// co-design cost model minimizes. See
/// `docs/12-design/CODESIGN_DIMENSIONAL_ARCHITECTURE_2026_06_19.adoc` §4.1.
pub use proximadb_observability_engine::io_trace;
/// TD-TRACE-2 S3 — the durable io_trace record envelope (header + modality payload).
pub use proximadb_observability_engine::trace_envelope;
/// Durable io_trace ETL sink (TD-TRACE-2 / ADR-066) — a separate, default-OFF
/// observer that spools each per-query snapshot to local JSONL+zstd segments (S1;
/// object-store dispatch is S2). Billing stays untouched/always-on (ADR-027).
pub mod io_trace_sink;
/// TD-TRACE-2 S4 — Iceberg-managed Parquet warehouse: an async compactor that
/// projects durable io_trace envelopes into a star schema (`trace_header` + modality
/// satellites), committed through Iceberg with a source-retirement watermark.
pub mod io_trace_warehouse;
/// Metering event builder — converts SearchPlanTrace → operator metering
/// event JSON shape so the data plane and operator pipelines can't drift.
pub use proximadb_observability_engine::metering_event;
/// TD-161 external OTLP metering push — ships per-tenant billing meters to a
/// standard OTLP collector (ADR-027 dual-sink push half). Feature `otlp-metering`
/// + runtime-gated by `PROXIMADB_OTLP_ENDPOINT`; compiles to no-ops otherwise.
pub use proximadb_observability_engine::metering_otlp;
/// [`ObjectStore`](object_store::ObjectStore) decorator feeding the per-query
/// io_trace (ADR-030/TD-158) — first consumer: the DataFusion Parquet leaf,
/// closing the "DataFusion route reports zero bytes" trace gap.
pub use proximadb_observability_engine::object_store_trace;
/// Embedding-precision metrics — Prometheus gauges/counters per
/// EMBEDDING_PRECISION_LLD_2026_05_22 §"Observability (Q11)" (PR 7b).
pub use proximadb_observability_engine::precision_metrics;
/// TD-064 predicate diagnostics bus — task-local channel that carries
/// recall-shortfall events from AxisManager-deep search paths to the
/// REST/gRPC handler that builds the SearchPlanTrace.
pub use proximadb_observability_engine::predicate_diagnostics;
/// Query engine for logs, metrics, and traces with PromQL support.
pub mod query;
/// Rank-pipeline metrics — Prometheus histograms/counters per
/// RANKING_FRAMEWORK_SPEC NFR-8 (R-7c.4d follow-up). Now lives in
/// `proximadb-observability-engine` (Slice 2a / TD-DECOMP-2); re-exported so the
/// `/metrics/prometheus` scrape and existing callers resolve unchanged.
pub use proximadb_observability_engine::rank_metrics;
/// Route explain builder — human-readable explanation derived from a
/// populated SearchPlanTrace for the LLD §1 debug=true response.
pub use proximadb_observability_engine::route_explain;
/// SearchPlanTrace — per-query telemetry envelope feeding KRU billing and the
/// learned planner v2 (LLD §10).
pub use proximadb_observability_engine::search_plan_trace;
/// Post-execution SearchPlanTrace builder.
pub use proximadb_observability_engine::search_plan_trace_builder;
/// Time-partitioned storage for observability data with WAL durability.
pub mod storage;
/// Tenant Prometheus label resolver — bundles tenant_id → bounded
/// label resolution with the LLD's cardinality-safety guardrail.
pub use proximadb_observability_engine::tenant_label;
/// Trace batcher — bundles N populated traces into one POST payload
/// for the async billing sink (digest-keyed dedup + fingerprint-aware).
pub use proximadb_observability_engine::trace_batcher;
/// Trace digest — stable FNV-1a hash for billing-event dedup +
/// idempotency keys on the async sink.
pub use proximadb_observability_engine::trace_digest;
/// Trace fingerprint — shape-only hash for incident-triage grouping.
pub use proximadb_observability_engine::trace_fingerprint;
/// Trace retention policy — companion to trace_sampling; per-tier age
/// windows + soft storage-budget shedding.
pub use proximadb_observability_engine::trace_retention;
/// Trace sampling policy — LLD-anchored down-sampling by tier + load.
pub use proximadb_observability_engine::trace_sampling;
/// Workload mix detector — aggregates fingerprint counts into a typed
/// summary for tier-recommendation hints and cache-warm targeting.
pub use proximadb_observability_engine::workload_mix;

// Facade types now live in `proximadb-observability-engine`; re-export so the
// existing `crate::observability::*` paths resolve unchanged.
pub use self::ingestion::ObservabilityIngester;
pub use self::query::ObservabilityQueryEngine;
pub use self::storage::ObservabilityStorage;
pub use proximadb_observability_engine::service::*;
pub use proximadb_observability_engine::{
    ObservabilityStoragePort, ObservabilityStorageStats, TraceSpan, TraceSummary,
};

#[cfg(test)]
mod query_tests;

use anyhow::Result;
use async_trait::async_trait;

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
        use crate::observability::TraceSpan;

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
            let (status_code, status_message) = trace_data.status.map_or((0, String::new()), |s| {
                (s.code, s.message.unwrap_or_default())
            }); // 0 = Unset

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

            match self.storage().write_span(namespace, &span).await {
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
            self.storage().query_trace(namespace, &tid).await?
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
        // `namespace_details()` replaces direct access to the private `namespaces`
        // map (the service moved to the engine crate).
        let details = self.namespace_details().await;

        let mut result = Vec::new();
        for (name, retention) in details {
            // Get storage stats for counts
            let stats = self.storage().stats(&name).await.ok();

            result.push(TraitNamespaceInfo {
                name,
                log_count: stats.as_ref().map_or(0, |s| s.log_count),
                metric_count: stats.as_ref().map_or(0, |s| s.metric_series_count),
                trace_count: stats.as_ref().map_or(0, |s| s.trace_count),
                retention_config: retention,
            });
        }

        Ok(result)
    }
}
