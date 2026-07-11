//! # ProximaDB Observability Engine
//!
//! Runtime implementation of ProximaDB's observability surface — the per-query
//! I/O trace substrate (co-design C0), metering-event builders, the trace
//! batcher/digest/fingerprint/sampling/retention pipeline, route-explain,
//! predicate-diagnostics, embedding/rank precision metrics, and the alerting +
//! audit sub-systems.
//!
//! ## Extraction status (root-crate decomposition)
//!
//! This crate is **Slice 1** of the observability extraction from the
//! monolithic root crate: it holds the *foundation-pure* modules (no dependency
//! on root storage/query/services). The *coupled* core — `ingestion`, `query`,
//! `graph_linking`, and the top-level `mod.rs` service orchestration — remains
//! in the root `src/observability/` because it depends on the WAL-backed
//! `ObservabilityStorage` (which carries root-storage up-edges). Slice 2 will
//! land that core behind an `ObservabilityStoragePort` (dependency inversion),
//! at which point it joins this crate.
//!
//! The root `crate::observability` module re-exports this crate, so the
//! ~80 inbound callers (`crate::*`) are unchanged.
//!
//! ## Foundation
//!
//! The modules here depend only on foundation types (`proximadb-data-model`,
//! `proximadb-kernel`, `proximadb-records`, `proximadb-tenant`) plus horizontal
//! crates (serde, tokio, prometheus, opentelemetry). No upward edge into the
//! root.

/// Proto re-export so moved modules' `crate::proto::proximadb_v1::*` paths
/// resolve (mirrors the root `src/proto/mod.rs`). Add more re-exports as the
/// extracted surface grows.
pub mod proto {
    pub use proximadb_proto::proximadb_v1;
}

/// Alerting engine — rule-based evaluation, escalation, persistence, notifications.
pub mod alerting;
/// Audit logging for compliance and security event tracking.
pub mod audit;
/// High-throughput ingestion pipeline — foundation-pure leaves (`buffer`,
/// `parser`). The facade + format adapters remain in the root.
pub mod ingestion;
/// Per-query I/O trace bus (co-design C0 — physical-dimension trace substrate).
pub mod io_trace;
/// Metering event builder — SearchPlanTrace → operator metering-event JSON.
pub mod metering_event;
/// TD-161 external OTLP metering push (ADR-027 dual-sink push half). Feature
/// `otlp-metering` + runtime-gated by `PROXIMADB_OTLP_ENDPOINT`; no-ops otherwise.
pub mod metering_otlp;
/// `ObjectStore` decorator feeding the per-query io_trace (ADR-030/TD-158).
pub mod object_store_trace;
/// Embedding-precision metrics — Prometheus gauges/counters.
pub mod precision_metrics;
/// TD-064 predicate diagnostics bus — recall-shortfall events from deep search.
pub mod predicate_diagnostics;
/// Query engine for logs, metrics, and traces — foundation-pure leaves
/// (`promql`, `tantivy_log_index`, `metrics`). The facade + logs + traces
/// remain in the root.
pub mod query;
/// Route explain builder — human-readable explanation from a SearchPlanTrace.
pub mod route_explain;
/// SearchPlanTrace — per-query telemetry envelope (KRU billing + learned planner).
pub mod search_plan_trace;
/// Post-execution SearchPlanTrace builder.
pub mod search_plan_trace_builder;
/// Tenant Prometheus label resolver — bounded cardinality-safe label resolution.
pub mod tenant_label;
/// Trace batcher — bundles N traces into one POST for the async billing sink.
pub mod trace_batcher;
/// Trace digest — stable FNV-1a hash for billing dedup / idempotency keys.
pub mod trace_digest;
/// Trace fingerprint — shape-only hash for incident-triage grouping.
pub mod trace_fingerprint;
/// Trace retention policy — companion to trace_sampling; per-tier age windows.
pub mod trace_retention;
/// Trace sampling policy — LLD-anchored down-sampling by tier + load.
pub mod trace_sampling;
/// Workload mix detector — fingerprint counts → typed summary for tier hints.
pub mod workload_mix;
