/*
 * Copyright 2026 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Per-query I/O trace bus (C0 — co-design trace substrate)
//!
//! This is the foundational deliverable of the co-design mandate
//! (`docs/12-design/CODESIGN_DIMENSIONAL_ARCHITECTURE_2026_06_19.adoc`, §4.1):
//! *you cannot co-design against a trace distribution you do not capture.*
//!
//! The existing `consumption_metrics` counters meter four per-tenant
//! **aggregates** (object-store ops, storage byte-seconds, task time, cache
//! stats) — excellent for billing, but they cannot answer the questions
//! co-design actually asks: *for this one query, how many GETs did we pay, how
//! many bytes did we move, did the footer cache hit, and which engine burned
//! the compute?* Those are the quantities the `ComputeScheduler` cost model
//! (§3 of the spec) must minimize, and they are per-query, not per-tenant.
//!
//! `IoTrace` captures them. It is bound to the request future as a
//! [`tokio::task_local!`] — exactly like
//! [`crate::predicate_diagnostics`] — so any depth of the
//! storage/engine call stack can record into it without threading a new
//! parameter through dozens of signatures (the dominant cost of doing this any
//! other way). At the request boundary the handler wraps the query in
//! [`instrument`] (or [`scope`]); downstream I/O sites call the free
//! [`record_op`] / [`record_bytes_read`] / [`record_footer`] helpers, which
//! **silently no-op outside an active scope** (so direct service/test callers
//! keep working and the Prometheus counters remain the operator-visible signal).
//!
//! On completion the snapshot is emitted as a structured `tracing` event under
//! the [`TARGET`] target. Once OpenTelemetry export is wired (§4.4) these
//! events become spans on the trace backend; today they are already
//! grep-able structured logs. This module adds **no new billing authority** —
//! the per-tenant counters stay the source of truth for chargeback; this is the
//! finer, per-query *source* the spec calls for.
//!
//! ## OpenTelemetry export (§4.4 — dependency-gated follow-up)
//!
//! Today the snapshot is emitted as a structured [`tracing`] event under
//! [`TARGET`] and is routable by any subscriber layer. Full export to an OTLP
//! collector (so these become persisted, queryable spans on a trace backend) is
//! a deliberate **dependency decision** not yet taken: the tree has
//! `tracing-subscriber` + `tracing-appender` but none of `opentelemetry`,
//! `opentelemetry_sdk`, `opentelemetry-otlp`, or `tracing-opentelemetry`. When
//! that stack is approved the wiring is small and already enabled here:
//!
//! 1. [`IoTraceSnapshot`] is `serde`-serializable, so it maps directly to OTLP
//!    span attributes (or a JSON sink) with no further plumbing.
//! 2. Attach a `tracing_opentelemetry::layer()` to the subscriber registry in
//!    `src/bin/server.rs` (beside the existing console/file `fmt` layers, at the
//!    `tracing_subscriber::registry()` call), gated on `OTEL_EXPORTER_OTLP_ENDPOINT`.
//! 3. Point the existing `crate::monitoring::opentelemetry` config's
//!    `otlp_endpoint` at the collector.
//!
//! Until then, persistence is available with zero new deps by routing [`TARGET`]
//! to a `tracing-appender` JSON file sink.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// `tracing` target for emitted per-query I/O trace events. Kept distinct so
/// operators can route/sample it independently (and the future OTLP layer can
/// map it to a span).
pub const TARGET: &str = "proximadb::io_trace";

tokio::task_local! {
    /// Active per-query I/O trace for the current task. Bound by [`scope`] /
    /// [`instrument`].
    ///
    /// Held behind an `Arc` so a handle can be cloned out via
    /// [`current_handle`] and captured by components (e.g. `TracingObjectStore`,
    /// `ProximaScanExec`) whose I/O later runs on DataFusion-**spawned** tokio
    /// tasks — a `task_local!` does not cross `tokio::spawn`, so those tasks
    /// must record through a captured handle rather than this task-local
    /// (TD-OLAP-3). All counters are atomic, so concurrent spawned readers
    /// aggregate into the one shared trace correctly.
    static IO_TRACE: Arc<IoTrace>;
}

/// Classification of an object-store operation by its cost shape. The universe
/// prices GET, PUT, LIST and DELETE differently (LIST and GET dominate
/// scan-heavy ANN/OLAP; PUT dominates ingest), so the trace keeps them apart
/// rather than collapsing to a single "ops" count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoOp {
    Get,
    Put,
    List,
    Delete,
}

impl IoOp {
    /// Best-effort classification of the operation strings already passed to
    /// `consumption_metrics::record_object_store_op` (e.g. `"fetch_pax"`,
    /// `"list_parquet"`, `"write_parquet"`) so existing call sites can feed the
    /// trace with a one-line addition. Unknown verbs map to [`IoOp::Get`] (the
    /// conservative read default) — the verb is still preserved by the caller's
    /// Prometheus label.
    pub fn classify(operation: &str) -> Self {
        if operation.starts_with("list") {
            IoOp::List
        } else if operation.starts_with("write")
            || operation.starts_with("put")
            || operation.starts_with("delete")
        {
            if operation.starts_with("delete") {
                IoOp::Delete
            } else {
                IoOp::Put
            }
        } else {
            // read_parquet, fetch_pax, fetch_pax_ranged, get, ...
            IoOp::Get
        }
    }
}

/// Per-query accumulator for the physical-dimension quantities the co-design
/// cost model consumes. Held inside a [`tokio::task_local!`]; all counters are
/// atomic so concurrent segment reads within one query aggregate correctly.
#[derive(Debug, Default)]
pub struct IoTrace {
    get_ops: AtomicU64,
    put_ops: AtomicU64,
    list_ops: AtomicU64,
    delete_ops: AtomicU64,
    /// Bytes fetched from object storage (Dimension 1 — the read term of KRU).
    bytes_read: AtomicU64,
    /// Number of ranged GET requests issued. With `bytes_read` yields the
    /// average GET size — the read-granularity signal (§2.1): coalesced toward
    /// the ~8-16 MiB S3 optimum, or fragmented into per-request-fee-dominated
    /// small GETs?
    range_gets: AtomicU64,
    /// Split-pruning outcome (TD-OLAP-3): candidate row-group splits
    /// considered by scans, and how many were skipped BEFORE fetch.
    /// `splits_pruned / splits_total` is the runtime-filter skip ratio the
    /// promotion gate consumes.
    splits_total: AtomicU64,
    splits_pruned: AtomicU64,
    /// PAX cascade centroid block-prune outcome (TD-RDSTRAT-5 S3): `centroid_pruned_blocks`
    /// of `centroid_total_blocks` skipped by the VOE-directory centroid probe before
    /// scanning. Distinct from `splits_*` (row-group runtime filter) so the ratios
    /// don't cross-contaminate; the recall gate asserts this engaged
    /// (`centroid_pruned_blocks > 0`) rather than silently falling back to a full scan.
    centroid_total_blocks: AtomicU64,
    centroid_pruned_blocks: AtomicU64,
    /// Runtime-filter wait outcomes (ADR-056 AQE-S11): how often the probe scan's
    /// `wait_complete()` rendezvous resolved with the filter arrived (pruning
    /// enabled) vs timed out (filterless, conservative), plus the wall ms spent
    /// waiting. The route cost model consumes the arrived/(arrived+timed_out)
    /// ratio to learn per-workload whether the wait budget pays.
    runtime_filter_arrived: AtomicU64,
    runtime_filter_timed_out: AtomicU64,
    runtime_filter_wait_ms: AtomicU64,
    /// Bytes written to object storage (ingest/flush — KIU).
    bytes_written: AtomicU64,
    /// Footer/metadata cache outcomes (Dimension 3 — the highest-ROI cache).
    footer_hits: AtomicU64,
    footer_misses: AtomicU64,
    /// Chargeable egress bytes — moved cross-region / to the internet off the
    /// free same-region path (Dimension 2 — KEU). Recorded only for chargeable
    /// localities; the route cost model's egress weight consumes this.
    egress_bytes: AtomicU64,
    /// PAX RaBitQ cascade **logical** striped-read projection (ADR-057 / TD-RDSTRAT-3):
    /// the bytes + ranged GETs a *selective* striped read WOULD move (Stage-1 codes
    /// stripe + Stage-2 candidate rerank rows, with the real ranked candidate set),
    /// kept DISTINCT from the physical `bytes_read`/`range_gets` (which today reflect
    /// the whole-segment `fs.read`). The pair makes the striped-vs-whole headroom
    /// observable per query on real candidate scatter — the co-design "trace before
    /// you tune" signal the flip gates on. Recording here is projection-only; it
    /// moves no bytes and changes no read path.
    logical_striped_bytes: AtomicU64,
    logical_striped_gets: AtomicU64,
    /// Embedding API calls (Dimension 5 — KEU, Kilo-Embedding-Units). A neutral
    /// counter for code-embedding operations; pricing happens downstream in
    /// AnvaiOps. Distinct from general read KRU.
    embedding_calls: AtomicU64,
    /// Total input tokens consumed by embedding operations.
    embedding_input_tokens: AtomicU64,
    /// Total output tokens (or equivalently, vector count) generated by embedding operations.
    embedding_output_tokens: AtomicU64,
    /// Compute milliseconds attributed by engine (Dimension 4 — KRU/KIU). Kept
    /// in a small map so a single query that touches multiple engines (e.g. a
    /// Volcano point lookup plus a DataFusion aggregate) attributes each.
    compute_ms: Mutex<BTreeMap<String, u64>>,
    /// pgwire relational-pipeline SETUP wall milliseconds (TD-OLAP-4): per-query
    /// pre-execution cost in `try_run_select` BEFORE the engine runs — table-name
    /// collection + xCatalog schema pre-resolution + route classification. Paid
    /// only on the DataFusion (path-2) route, not the native early-return path.
    setup_ms: AtomicU64,
    /// pgwire result EMIT wall milliseconds (TD-OLAP-4): encoding the materialized
    /// rows to `RowDescription` + `DataRow` frames and writing them to the socket,
    /// AFTER execution — the last unmeasured span in the per-query wall floor.
    emit_ms: AtomicU64,
    /// SessionContext build wall milliseconds (TD-OLAP-4): per-query cost of
    /// creating a fresh DataFusion `SessionContext` and re-registering all
    /// UDFs/UDAFs — paid before table open, reusable across queries.
    session_ms: AtomicU64,
    /// Table-OPEN wall milliseconds (TD-OLAP-4): the per-query fixed cost of
    /// discovering + opening the parquet base (LIST + HEAD-per-file + footer
    /// read) BEFORE execution. Distinct from `compute_ms` (execution) — a
    /// footer-only query does ~0 compute yet pays this floor. Drops to ~0 on a
    /// warm table-open cache hit, so it is the direct signal for that lever.
    open_ms: AtomicU64,
    /// Query lowering + logical/physical planning wall milliseconds (TD-OLAP-4),
    /// the other half of the per-query floor separate from execution.
    plan_ms: AtomicU64,
    /// Table-OPEN cache outcomes (TD-OLAP-4): a hit skips the LIST+HEAD+footer
    /// discovery and reuses cached schema/splits/file-sizes.
    table_open_hits: AtomicU64,
    table_open_misses: AtomicU64,
    /// Plan-geometry vector (TD-EXEC-2 Slice 1, observe-only): the pre-execution
    /// geometric summary of the served physical plan — depth, node/leaf counts,
    /// fan-out, blocking-operator count — recorded at the plan→execute seam as
    /// neutral scalars (io_trace never depends on a query-layer type). One
    /// measured vector feeds three resource laws: stack sizing, engine routing,
    /// parallelism. Max semantics so a multi-statement scope keeps its deepest plan.
    plan_depth: AtomicU64,
    plan_nodes: AtomicU64,
    plan_leaves: AtomicU64,
    plan_fanout: AtomicU64,
    plan_blocking: AtomicU64,
    /// Per-operator-kind counts of the served plan (TD-EXEC-2) — the histogram
    /// half of the geometry vector, keyed by neutral op-kind labels.
    plan_ops: Mutex<BTreeMap<String, u64>>,
    /// Measured stack high-water mark (bytes) of the plan-tree recursions
    /// (planner lowering + executor build), sampled via the planner's
    /// `stack_probe` (TD-EXEC-2 Slice 1). Max across recordings — the binding
    /// per-query figure that calibrates `frame_bytes[op_kind]`.
    stack_hwm_bytes: AtomicU64,
    /// The route this query was served on, as `(shape_class, backend_label)`
    /// strings (co-design C4). Stamped by the `ComputeScheduler` so the flush can
    /// feed the trace-driven cost model the cost of *this kind of query on that
    /// engine* — without io_trace depending on any query-layer type. `None` until
    /// a route is chosen.
    route: Mutex<Option<(String, String)>>,
    /// The resolved per-collection storage profile (`append_bulk`/`churn`) this
    /// query read under (ADR-061 D6 / TD-WLP-6), stamped at the search boundary
    /// so a query's projection strategy is observable per-tenant. `None` until
    /// stamped; io_trace never depends on the `StorageProfile` type (neutral
    /// label string only).
    storage_profile: Mutex<Option<String>>,
}

impl IoTrace {
    /// Create an empty trace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamp the route this query is served on (`shape_class`, `backend_label`).
    /// Last write wins (a query is served by one engine). Neutral strings only.
    pub fn record_route(&self, shape_class: &str, backend_label: &str) {
        *self.route.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((shape_class.to_string(), backend_label.to_string()));
    }

    /// The stamped route, if any.
    pub fn route(&self) -> Option<(String, String)> {
        self.route.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Stamp the resolved storage profile this query read under (neutral label,
    /// e.g. `append_bulk`/`churn`). Last write wins. TD-WLP-6.
    pub fn record_storage_profile(&self, profile: &str) {
        *self
            .storage_profile
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(profile.to_string());
    }

    /// The stamped storage-profile label, if any.
    pub fn storage_profile(&self) -> Option<String> {
        self.storage_profile
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Record one classified object-store operation.
    pub fn record_op(&self, op: IoOp) {
        let counter = match op {
            IoOp::Get => &self.get_ops,
            IoOp::Put => &self.put_ops,
            IoOp::List => &self.list_ops,
            IoOp::Delete => &self.delete_ops,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Add to bytes fetched from object storage.
    pub fn record_bytes_read(&self, bytes: u64) {
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Add to the PAX cascade's **logical** striped-read projection (bytes + GETs)
    /// — the selective read a striped path WOULD issue. Kept distinct from the
    /// physical counters (ADR-057 / TD-RDSTRAT-3).
    pub fn record_logical_striped(&self, bytes: u64, gets: u64) {
        self.logical_striped_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        self.logical_striped_gets.fetch_add(gets, Ordering::Relaxed);
    }

    /// Add to the count of ranged GET requests issued.
    pub fn record_range_gets(&self, gets: u64) {
        self.range_gets.fetch_add(gets, Ordering::Relaxed);
    }

    /// Record a split-pruning outcome (TD-OLAP-3): `total` candidate
    /// row-group splits considered by a scan, of which `pruned` were skipped
    /// before fetch.
    pub fn record_splits(&self, total: u64, pruned: u64) {
        self.splits_total.fetch_add(total, Ordering::Relaxed);
        self.splits_pruned.fetch_add(pruned, Ordering::Relaxed);
    }

    /// Record a PAX cascade centroid block-prune outcome (TD-RDSTRAT-5 S3): of
    /// `total` blocks in the segment, `pruned` were skipped by the centroid probe.
    pub fn record_centroid_prune(&self, total: u64, pruned: u64) {
        self.centroid_total_blocks
            .fetch_add(total, Ordering::Relaxed);
        self.centroid_pruned_blocks
            .fetch_add(pruned, Ordering::Relaxed);
    }

    /// Record a runtime-filter wait outcome (ADR-056 AQE-S11): `arrived` = the
    /// filter completed within the wait budget (pruning enabled); otherwise it
    /// timed out (splits read filterless, conservative). `waited_ms` = the wall
    /// ms actually spent at the rendezvous.
    pub fn record_runtime_filter_wait(&self, arrived: bool, waited_ms: u64) {
        if arrived {
            self.runtime_filter_arrived.fetch_add(1, Ordering::Relaxed);
        } else {
            self.runtime_filter_timed_out
                .fetch_add(1, Ordering::Relaxed);
        }
        self.runtime_filter_wait_ms
            .fetch_add(waited_ms, Ordering::Relaxed);
    }

    /// Add to bytes written to object storage.
    pub fn record_bytes_written(&self, bytes: u64) {
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record a footer/metadata cache outcome.
    pub fn record_footer(&self, hit: bool) {
        let counter = if hit {
            &self.footer_hits
        } else {
            &self.footer_misses
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a batch of footer/metadata cache outcomes at once — convenient for
    /// forwarding a `RangedSegmentReader`'s per-open `SegmentReadStats`.
    pub fn record_footers(&self, hits: u64, misses: u64) {
        self.footer_hits.fetch_add(hits, Ordering::Relaxed);
        self.footer_misses.fetch_add(misses, Ordering::Relaxed);
    }

    /// Add to chargeable egress bytes (cross-region / internet — KEU).
    pub fn record_egress_bytes(&self, bytes: u64) {
        self.egress_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Attribute compute milliseconds to a named engine.
    pub fn record_compute_ms(&self, engine: &str, ms: u64) {
        let mut g = self.compute_ms.lock().unwrap_or_else(|p| p.into_inner());
        *g.entry(engine.to_string()).or_insert(0) += ms;
    }

    /// Add to the pgwire result-emit wall milliseconds (row encode + socket write).
    pub fn record_emit_ms(&self, ms: u64) {
        self.emit_ms.fetch_add(ms, Ordering::Relaxed);
    }

    /// Add to the pgwire relational-pipeline setup wall milliseconds.
    pub fn record_setup_ms(&self, ms: u64) {
        self.setup_ms.fetch_add(ms, Ordering::Relaxed);
    }

    /// Add to the SessionContext build wall milliseconds.
    pub fn record_session_ms(&self, ms: u64) {
        self.session_ms.fetch_add(ms, Ordering::Relaxed);
    }

    /// Add to the table-OPEN wall milliseconds (discovery + footer open before
    /// execution). Additive so multiple registered tables accumulate.
    pub fn record_open_ms(&self, ms: u64) {
        self.open_ms.fetch_add(ms, Ordering::Relaxed);
    }

    /// Add to the lowering + planning wall milliseconds.
    pub fn record_plan_ms(&self, ms: u64) {
        self.plan_ms.fetch_add(ms, Ordering::Relaxed);
    }

    /// Record the served plan's geometry vector (TD-EXEC-2 Slice 1, observe-only):
    /// neutral scalars measured from the physical plan at the plan→execute seam,
    /// plus the per-op-kind histogram as `(label, count)` pairs. Scalars keep the
    /// max so a multi-statement scope reflects its deepest plan; histogram counts
    /// accumulate.
    pub fn record_plan_geometry(
        &self,
        depth: u64,
        nodes: u64,
        leaves: u64,
        fanout: u64,
        blocking: u64,
        ops: &[(&str, u64)],
    ) {
        self.plan_depth.fetch_max(depth, Ordering::Relaxed);
        self.plan_nodes.fetch_max(nodes, Ordering::Relaxed);
        self.plan_leaves.fetch_max(leaves, Ordering::Relaxed);
        self.plan_fanout.fetch_max(fanout, Ordering::Relaxed);
        self.plan_blocking.fetch_max(blocking, Ordering::Relaxed);
        let mut g = self.plan_ops.lock().unwrap_or_else(|p| p.into_inner());
        for (kind, count) in ops {
            *g.entry((*kind).to_string()).or_insert(0) += count;
        }
    }

    /// Record a measured stack high-water mark (bytes) for one of the plan-tree
    /// recursions (TD-EXEC-2 Slice 1). Max semantics — the binding figure wins.
    pub fn record_stack_hwm(&self, bytes: u64) {
        self.stack_hwm_bytes.fetch_max(bytes, Ordering::Relaxed);
    }

    /// Record a table-OPEN cache outcome: `true` = hit (discovery reused, no
    /// LIST/HEAD/footer I/O), `false` = miss (cold open).
    pub fn record_table_open(&self, hit: bool) {
        let counter = if hit {
            &self.table_open_hits
        } else {
            &self.table_open_misses
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an embedding API call (KEU — Kilo-Embedding-Units metering).
    pub fn record_embedding_calls(&self, calls: u64) {
        self.embedding_calls.fetch_add(calls, Ordering::Relaxed);
    }

    /// Record input tokens consumed by embedding operations.
    pub fn record_embedding_input_tokens(&self, tokens: u64) {
        self.embedding_input_tokens
            .fetch_add(tokens, Ordering::Relaxed);
    }

    /// Record output tokens (or vector count) generated by embedding operations.
    pub fn record_embedding_output_tokens(&self, tokens: u64) {
        self.embedding_output_tokens
            .fetch_add(tokens, Ordering::Relaxed);
    }

    /// Record a complete embedding operation with input/output token counts.
    pub fn record_embedding(&self, input_tokens: u64, output_tokens: u64) {
        self.embedding_calls.fetch_add(1, Ordering::Relaxed);
        self.embedding_input_tokens
            .fetch_add(input_tokens, Ordering::Relaxed);
        self.embedding_output_tokens
            .fetch_add(output_tokens, Ordering::Relaxed);
    }

    /// Take a plain-value snapshot for emission/inspection.
    pub fn snapshot(&self) -> IoTraceSnapshot {
        IoTraceSnapshot {
            get_ops: self.get_ops.load(Ordering::Relaxed),
            put_ops: self.put_ops.load(Ordering::Relaxed),
            list_ops: self.list_ops.load(Ordering::Relaxed),
            delete_ops: self.delete_ops.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            range_gets: self.range_gets.load(Ordering::Relaxed),
            logical_striped_bytes: self.logical_striped_bytes.load(Ordering::Relaxed),
            logical_striped_gets: self.logical_striped_gets.load(Ordering::Relaxed),
            splits_total: self.splits_total.load(Ordering::Relaxed),
            splits_pruned: self.splits_pruned.load(Ordering::Relaxed),
            centroid_total_blocks: self.centroid_total_blocks.load(Ordering::Relaxed),
            centroid_pruned_blocks: self.centroid_pruned_blocks.load(Ordering::Relaxed),
            runtime_filter_arrived: self.runtime_filter_arrived.load(Ordering::Relaxed),
            runtime_filter_timed_out: self.runtime_filter_timed_out.load(Ordering::Relaxed),
            runtime_filter_wait_ms: self.runtime_filter_wait_ms.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            footer_hits: self.footer_hits.load(Ordering::Relaxed),
            footer_misses: self.footer_misses.load(Ordering::Relaxed),
            egress_bytes: self.egress_bytes.load(Ordering::Relaxed),
            embedding_calls: self.embedding_calls.load(Ordering::Relaxed),
            embedding_input_tokens: self.embedding_input_tokens.load(Ordering::Relaxed),
            embedding_output_tokens: self.embedding_output_tokens.load(Ordering::Relaxed),
            compute_ms: self
                .compute_ms
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
            setup_ms: self.setup_ms.load(Ordering::Relaxed),
            emit_ms: self.emit_ms.load(Ordering::Relaxed),
            session_ms: self.session_ms.load(Ordering::Relaxed),
            open_ms: self.open_ms.load(Ordering::Relaxed),
            plan_ms: self.plan_ms.load(Ordering::Relaxed),
            table_open_hits: self.table_open_hits.load(Ordering::Relaxed),
            table_open_misses: self.table_open_misses.load(Ordering::Relaxed),
            plan_depth: self.plan_depth.load(Ordering::Relaxed),
            plan_nodes: self.plan_nodes.load(Ordering::Relaxed),
            plan_leaves: self.plan_leaves.load(Ordering::Relaxed),
            plan_fanout: self.plan_fanout.load(Ordering::Relaxed),
            plan_blocking: self.plan_blocking.load(Ordering::Relaxed),
            plan_ops: self
                .plan_ops
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
            stack_hwm_bytes: self.stack_hwm_bytes.load(Ordering::Relaxed),
            route: self.route(),
            storage_profile: self.storage_profile(),
        }
    }
}

/// Immutable, plain-value view of an [`IoTrace`] at a point in time — what gets
/// emitted as a `tracing` event and what a future cost-model reader consumes.
///
/// Serde-serializable so an export layer (OTLP span attributes, a JSON sink, or
/// a cost-model reader) can consume the snapshot directly — the data-shape half
/// of §4.4. See the module docs for the OTLP-collector wiring plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IoTraceSnapshot {
    pub get_ops: u64,
    pub put_ops: u64,
    pub list_ops: u64,
    pub delete_ops: u64,
    pub bytes_read: u64,
    pub range_gets: u64,
    /// PAX cascade logical striped-read projection: bytes + ranged GETs a selective
    /// striped read would move (ADR-057 / TD-RDSTRAT-3). With `bytes_read`
    /// (whole-segment physical) this yields the measured striped-vs-whole headroom.
    #[serde(default)]
    pub logical_striped_bytes: u64,
    #[serde(default)]
    pub logical_striped_gets: u64,
    /// Candidate row-group splits considered by scans (TD-OLAP-3).
    #[serde(default)]
    pub splits_total: u64,
    /// Splits skipped before fetch — with `splits_total`, the skip ratio.
    #[serde(default)]
    pub splits_pruned: u64,
    /// PAX cascade centroid block-prune (TD-RDSTRAT-5 S3): total blocks in the
    /// segment and how many the centroid probe skipped. `centroid_pruned_blocks > 0`
    /// proves the prune engaged (vs a silent full-scan fallback).
    #[serde(default)]
    pub centroid_total_blocks: u64,
    #[serde(default)]
    pub centroid_pruned_blocks: u64,
    /// Runtime-filter wait outcomes (ADR-056 AQE-S11): arrived vs timed-out +
    /// the wall ms spent waiting. `arrived / (arrived + timed_out)` is the
    /// per-workload signal the route cost model learns to tune the wait budget.
    #[serde(default)]
    pub runtime_filter_arrived: u64,
    #[serde(default)]
    pub runtime_filter_timed_out: u64,
    #[serde(default)]
    pub runtime_filter_wait_ms: u64,
    pub bytes_written: u64,
    pub footer_hits: u64,
    pub footer_misses: u64,
    pub egress_bytes: u64,
    pub embedding_calls: u64,
    pub embedding_input_tokens: u64,
    pub embedding_output_tokens: u64,
    /// Compute wall ms attributed **per engine** — the engine dimension of the
    /// geometry-dependent dispatch (TD-OLAP-4). Keys distinguish `datafusion`,
    /// `native`/`native-vectorized`, and `volcano`, so the route cost model can
    /// learn which engine wins per query shape.
    pub compute_ms: BTreeMap<String, u64>,
    /// The route this query was served on, `(shape_class, backend_label)` — the
    /// top dispatch dimension (which engine). `None` if no route was stamped.
    #[serde(default)]
    pub route: Option<(String, String)>,
    /// The resolved per-collection storage profile (`append_bulk`/`churn`) this
    /// query read under (ADR-061 D6 / TD-WLP-6). `None` if unstamped.
    #[serde(default)]
    pub storage_profile: Option<String>,
    /// pgwire relational-pipeline setup wall ms — pre-execution xCatalog schema
    /// resolution + route classification, DataFusion route only (TD-OLAP-4).
    #[serde(default)]
    pub setup_ms: u64,
    /// pgwire result-emit wall ms — row encode + socket write, post-execution
    /// (TD-OLAP-4).
    #[serde(default)]
    pub emit_ms: u64,
    /// SessionContext build wall ms — per-query context+UDF setup (TD-OLAP-4).
    #[serde(default)]
    pub session_ms: u64,
    /// Table-OPEN wall ms — the per-query discovery+footer floor (TD-OLAP-4).
    #[serde(default)]
    pub open_ms: u64,
    /// Lowering + planning wall ms — the other half of the floor (TD-OLAP-4).
    #[serde(default)]
    pub plan_ms: u64,
    /// Table-OPEN cache hits (discovery reused, no LIST/HEAD/footer I/O).
    #[serde(default)]
    pub table_open_hits: u64,
    /// Table-OPEN cache misses (cold open).
    #[serde(default)]
    pub table_open_misses: u64,
    /// Served plan's longest root→leaf path (TD-EXEC-2 geometry; 0 = not recorded).
    #[serde(default)]
    pub plan_depth: u64,
    /// Served plan's total operator count (TD-EXEC-2 geometry).
    #[serde(default)]
    pub plan_nodes: u64,
    /// Served plan's leaf (scan/values) count — source fan-in (TD-EXEC-2 geometry).
    #[serde(default)]
    pub plan_leaves: u64,
    /// Served plan's widest sibling set — parallelism signal (TD-EXEC-2 geometry).
    #[serde(default)]
    pub plan_fanout: u64,
    /// Served plan's pipeline-breaker count (joins+sorts+aggregates) —
    /// memory/spill signal (TD-EXEC-2 geometry).
    #[serde(default)]
    pub plan_blocking: u64,
    /// Served plan's per-operator-kind histogram (TD-EXEC-2 geometry).
    #[serde(default)]
    pub plan_ops: BTreeMap<String, u64>,
    /// Measured stack high-water mark (bytes) of the plan-tree recursions,
    /// via the planner's `stack_probe` (TD-EXEC-2 Slice 1). The calibration
    /// figure that resolves `frame_bytes[op_kind]`; 0 = not measured.
    #[serde(default)]
    pub stack_hwm_bytes: u64,
}

impl IoTraceSnapshot {
    /// Total object-store operations across all verbs.
    pub fn total_ops(&self) -> u64 {
        self.get_ops + self.put_ops + self.list_ops + self.delete_ops
    }

    /// Footer-cache hit ratio in `[0, 1]`; `None` when the footer cache was not
    /// consulted (no hits or misses recorded).
    pub fn footer_hit_ratio(&self) -> Option<f64> {
        let total = self.footer_hits + self.footer_misses;
        if total == 0 {
            None
        } else {
            Some(self.footer_hits as f64 / total as f64)
        }
    }

    /// Total attributed compute milliseconds across engines.
    pub fn total_compute_ms(&self) -> u64 {
        self.compute_ms.values().copied().sum()
    }

    /// Average bytes per ranged GET — the read-granularity signal (§2.1).
    /// `None` when no ranged GETs were issued. A value far below the ~8-16 MiB
    /// S3 cost-throughput optimum means reads are fragmented and the per-GET fee
    /// dominates; this is the number the storage co-design lever moves.
    pub fn avg_get_bytes(&self) -> Option<f64> {
        if self.range_gets == 0 {
            None
        } else {
            Some(self.bytes_read as f64 / self.range_gets as f64)
        }
    }

    /// `true` when nothing was recorded — used to suppress empty trace events.
    pub fn is_empty(&self) -> bool {
        self.total_ops() == 0
            && self.bytes_read == 0
            && self.range_gets == 0
            && self.bytes_written == 0
            && self.footer_hits == 0
            && self.footer_misses == 0
            && self.egress_bytes == 0
            && self.embedding_calls == 0
            && self.embedding_input_tokens == 0
            && self.embedding_output_tokens == 0
            && self.compute_ms.is_empty()
    }

    /// Total embedding tokens (input + output).
    pub fn total_embedding_tokens(&self) -> u64 {
        self.embedding_input_tokens + self.embedding_output_tokens
    }

    /// Emit this snapshot as a structured `tracing` event under [`TARGET`].
    /// No-op when empty. `tenant_id` and `route` label the query; all physical
    /// quantities become event fields the OTLP layer (§4.4) maps to a span.
    /// Emit the per-query I/O trace as a structured `tracing` event.
    ///
    /// TD-160: this is the **perf-class emission** — gated behind the `io-trace`
    /// cargo feature (default OFF) so it costs nothing in normal operation. When
    /// the feature is off this is a near-no-op; the cheap core counters and the
    /// route/cache/**billing** observers in [`instrument`] are unaffected and stay
    /// always-on (billing is never gated — ADR-027 non-entanglement).
    pub fn emit(&self, tenant_id: Option<&str>, route: &str) {
        if self.is_empty() {
            return;
        }
        #[cfg(feature = "io-trace")]
        tracing::info!(
            target: TARGET,
            tenant_id = tenant_id.unwrap_or("default"),
            route = route,
            storage_profile = self.storage_profile.as_deref().unwrap_or("unset"),
            get_ops = self.get_ops,
            put_ops = self.put_ops,
            list_ops = self.list_ops,
            delete_ops = self.delete_ops,
            bytes_read = self.bytes_read,
            range_gets = self.range_gets,
            avg_get_bytes = self.avg_get_bytes().unwrap_or(0.0),
            bytes_written = self.bytes_written,
            footer_hits = self.footer_hits,
            footer_misses = self.footer_misses,
            footer_hit_ratio = self.footer_hit_ratio().unwrap_or(f64::NAN),
            egress_bytes = self.egress_bytes,
            embedding_calls = self.embedding_calls,
            embedding_input_tokens = self.embedding_input_tokens,
            embedding_output_tokens = self.embedding_output_tokens,
            embedding_tokens_total = self.total_embedding_tokens(),
            compute_ms_total = self.total_compute_ms(),
            compute_ms_by_engine = ?self.compute_ms,
            "per-query I/O trace"
        );
        // Perf emission compiled out (default): consume the args so the signature
        // is stable and no unused-var warning fires in the feature-off build.
        #[cfg(not(feature = "io-trace"))]
        let _ = (tenant_id, route);
    }
}

// ---- Free helpers: record into the active scope, or silently no-op. ----

/// Record one object-store operation into the active query trace. Silently
/// no-ops outside an active [`scope`]/[`instrument`].
pub fn record_op(op: IoOp) {
    let _ = IO_TRACE.try_with(|t| t.record_op(op));
}

/// Classify an operation verb (as used by `consumption_metrics`) and record it.
pub fn record_op_str(operation: &str) {
    record_op(IoOp::classify(operation));
}

/// Add to bytes fetched from object storage for the active query.
pub fn record_bytes_read(bytes: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_bytes_read(bytes));
}

/// Record the PAX cascade's logical striped-read projection (bytes + GETs a
/// selective read would move) into the active query trace — distinct from the
/// physical `bytes_read`/`range_gets` (ADR-057 / TD-RDSTRAT-3). Silently no-ops
/// outside an active scope; a core counter (always-on, like `record_bytes_read`).
pub fn record_logical_striped(bytes: u64, gets: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_logical_striped(bytes, gets));
}

/// Add to the count of ranged GET requests for the active query.
///
/// ADR-030 **core counter — always-on** (deliberately NOT behind `io-trace`,
/// alongside `record_bytes_read`/`record_compute_ms`): the billing observer drains
/// `range_gets` to compute `avg_get_size = bytes_read / range_gets`, and the route
/// cost model prices `per_get`, so this must read real values even when the
/// perf-emission class is compiled out. It is one task-local atomic increment per
/// physical GET — the ~free accumulator core, never the cost the `io-trace` gate
/// exists to remove. Silently no-ops outside an active scope.
pub fn record_range_gets(gets: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_range_gets(gets));
}

/// Record a scan's split-pruning outcome for the active query (TD-OLAP-3).
/// Core counter (always-on, like `record_range_gets`): the runtime-filter
/// promotion gate reads `splits_pruned / splits_total` from the billing
/// snapshot. Silently no-ops outside an active scope.
pub fn record_splits(total: u64, pruned: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_splits(total, pruned));
}

/// Record a PAX cascade centroid block-prune outcome for the active query
/// (TD-RDSTRAT-5 S3): of `total` blocks, `pruned` were skipped by the centroid
/// probe. Silently no-ops outside an active scope.
pub fn record_centroid_prune(total: u64, pruned: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_centroid_prune(total, pruned));
}

/// Record a runtime-filter wait outcome for the active query (ADR-056 AQE-S11).
pub fn record_runtime_filter_wait(arrived: bool, waited_ms: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_runtime_filter_wait(arrived, waited_ms));
}

/// Add to bytes written to object storage for the active query.
pub fn record_bytes_written(bytes: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_bytes_written(bytes));
}

/// Record a footer/metadata cache outcome for the active query. (TD-160:
/// perf/geometry trace class — compile-time gated.)
#[cfg(feature = "io-trace")]
pub fn record_footer(hit: bool) {
    let _ = IO_TRACE.try_with(|t| t.record_footer(hit));
}
#[cfg(not(feature = "io-trace"))]
#[inline(always)]
pub fn record_footer(_hit: bool) {}

/// Record a batch of footer/metadata cache outcomes for the active query.
/// (TD-160: perf/geometry trace class — compile-time gated.)
#[cfg(feature = "io-trace")]
pub fn record_footers(hits: u64, misses: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_footers(hits, misses));
}
#[cfg(not(feature = "io-trace"))]
#[inline(always)]
pub fn record_footers(_hits: u64, _misses: u64) {}

/// Record chargeable egress bytes (cross-region / internet — KEU) for the
/// active query.
pub fn record_egress_bytes(bytes: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_egress_bytes(bytes));
}

/// Attribute compute milliseconds to `engine` for the active query.
pub fn record_compute_ms(engine: &str, ms: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_compute_ms(engine, ms));
}

/// Add table-OPEN wall ms (discovery + footer open) to the active query trace.
pub fn record_open_ms(ms: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_open_ms(ms));
}

/// Add pgwire relational-pipeline setup wall ms to the active query trace.
pub fn record_setup_ms(ms: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_setup_ms(ms));
}

/// Record the served plan's geometry vector for the active query (TD-EXEC-2
/// Slice 1, observe-only). Core counter (always-on, like `record_plan_ms`):
/// the route cost model's geometry tier and the stack calibration fit both
/// consume it from the ledger. Silently no-ops outside an active scope.
pub fn record_plan_geometry(
    depth: u64,
    nodes: u64,
    leaves: u64,
    fanout: u64,
    blocking: u64,
    ops: &[(&str, u64)],
) {
    let _ =
        IO_TRACE.try_with(|t| t.record_plan_geometry(depth, nodes, leaves, fanout, blocking, ops));
}

/// Record a measured plan-recursion stack high-water mark (bytes) for the
/// active query (TD-EXEC-2 Slice 1). Silently no-ops outside an active scope.
pub fn record_stack_hwm(bytes: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_stack_hwm(bytes));
}

/// Add pgwire result-emit wall ms (row encode + socket write) to the active trace.
pub fn record_emit_ms(ms: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_emit_ms(ms));
}

/// Add SessionContext build wall ms to the active query trace.
pub fn record_session_ms(ms: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_session_ms(ms));
}

/// Add lowering + planning wall ms to the active query trace.
pub fn record_plan_ms(ms: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_plan_ms(ms));
}

/// Record a table-OPEN cache outcome on the active query trace.
pub fn record_table_open(hit: bool) {
    let _ = IO_TRACE.try_with(|t| t.record_table_open(hit));
}

/// Stamp the route (`shape_class`, `backend_label`) onto the active query trace.
/// Silently no-ops outside an active scope. Neutral strings — io_trace never
/// depends on a query-layer type.
pub fn record_route(shape_class: &str, backend_label: &str) {
    let _ = IO_TRACE.try_with(|t| t.record_route(shape_class, backend_label));
}

/// Stamp the resolved storage profile (`append_bulk`/`churn`) onto the active
/// query trace (ADR-061 D6 / TD-WLP-6). Silently no-ops outside an active
/// scope. Neutral label string — io_trace never depends on `StorageProfile`.
pub fn record_storage_profile(profile: &str) {
    let _ = IO_TRACE.try_with(|t| t.record_storage_profile(profile));
}

/// Record embedding API calls for the active query (KEU metering).
pub fn record_embedding_calls(calls: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_embedding_calls(calls));
}

/// Record input tokens consumed by embedding operations for the active query.
pub fn record_embedding_input_tokens(tokens: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_embedding_input_tokens(tokens));
}

/// Record output tokens generated by embedding operations for the active query.
pub fn record_embedding_output_tokens(tokens: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_embedding_output_tokens(tokens));
}

/// Record a complete embedding operation (input + output tokens) for the active query.
pub fn record_embedding(input_tokens: u64, output_tokens: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_embedding(input_tokens, output_tokens));
}

/// Snapshot the active query trace, if any.
pub fn snapshot() -> Option<IoTraceSnapshot> {
    IO_TRACE.try_with(|t| t.snapshot()).ok()
}

/// Clone out an `Arc` handle to the active query trace, if any.
///
/// Components whose I/O later runs on DataFusion-spawned tokio tasks (where the
/// `IO_TRACE` task-local is absent) capture this handle **while still in the
/// query scope** — table open / physical planning — and record through it, so
/// their reads attribute to the correct per-query trace (TD-OLAP-3). Returns
/// `None` outside any [`scope`]/[`instrument`].
pub fn current_handle() -> Option<Arc<IoTrace>> {
    IO_TRACE.try_with(|t| t.clone()).ok()
}

/// Observer invoked at trace flush with `(snapshot, shape_class, backend_label)`
/// when the query stamped a route. This is the dependency-inversion seam: the
/// query layer registers a sink that feeds the trace-driven route cost model
/// (C4), so io_trace ingests into routing *without depending on it*.
type RouteObserver = dyn Fn(&IoTraceSnapshot, &str, &str) + Send + Sync;

static ROUTE_OBSERVER: Mutex<Option<Box<RouteObserver>>> = Mutex::new(None);

/// Install (or clear with `None`) the route-trace observer. Called once at
/// startup by the query layer; replaceable in tests.
pub fn set_route_observer(observer: Option<Box<RouteObserver>>) {
    *ROUTE_OBSERVER.lock().unwrap_or_else(|p| p.into_inner()) = observer;
}

/// Feed the registered observer, if any, with a completed query's route trace.
fn notify_route_observer(snap: &IoTraceSnapshot, shape_class: &str, backend_label: &str) {
    if let Some(obs) = ROUTE_OBSERVER
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        obs(snap, shape_class, backend_label);
    }
}

/// Observer invoked at trace flush with `snapshot` for cache sizing feedback (T2.2).
/// The cache orchestrator registers to receive footer hit-rate and avg_get_bytes signals.
/// This is the dependency-inversion seam: io_trace feeds cache sizing *without depending on it*.
type CacheObserver = dyn Fn(&IoTraceSnapshot) + Send + Sync;

static CACHE_OBSERVER: Mutex<Option<Box<CacheObserver>>> = Mutex::new(None);

/// Install (or clear with `None`) the cache-trace observer. Called once at startup
/// by the cache orchestrator; replaceable in tests.
pub fn set_cache_observer(observer: Option<Box<CacheObserver>>) {
    *CACHE_OBSERVER.lock().unwrap_or_else(|p| p.into_inner()) = observer;
}

/// Feed the registered cache observer, if any, with a completed query's trace.
/// Called after route observer; both observers can be active simultaneously.
fn notify_cache_observer(snap: &IoTraceSnapshot) {
    if let Some(obs) = CACHE_OBSERVER
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        obs(snap);
    }
}

/// Observer invoked at trace flush with `snapshot` + `tenant_id` for **billing**
/// (ADR-030). The metering layer registers a sink that emits the always-on
/// per-tenant consumption meters (KRU read-compute, from `compute_ms`) from the
/// same measured snapshot the perf observers read — so cost-model bytes and
/// billed bytes cannot diverge. Dependency-inversion seam: io_trace feeds billing
/// *without depending on it*.
///
/// Unlike the route/cache observers this is the **billing class** — always-on,
/// never wrapped in the perf `io-trace` feature gate (ADR-027 non-entanglement;
/// CI-guarded).
type BillingObserver = dyn Fn(&IoTraceSnapshot, Option<&str>) + Send + Sync;

static BILLING_OBSERVER: Mutex<Option<Box<BillingObserver>>> = Mutex::new(None);

/// Install (or clear with `None`) the billing-trace observer. Called once at
/// startup by the metering layer; replaceable in tests.
pub fn set_billing_observer(observer: Option<Box<BillingObserver>>) {
    *BILLING_OBSERVER.lock().unwrap_or_else(|p| p.into_inner()) = observer;
}

/// Feed the registered billing observer, if any, with a completed query's trace
/// and the owning tenant. Called after the route + cache observers at scope close.
fn notify_billing_observer(snap: &IoTraceSnapshot, tenant_id: Option<&str>) {
    if let Some(obs) = BILLING_OBSERVER
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        obs(snap, tenant_id);
    }
}

/// Bind a fresh [`IoTrace`] to `future` and await it. Lower-level than
/// [`instrument`]; use when the caller wants to read the snapshot itself before
/// the scope ends.
pub async fn scope<F: std::future::Future>(future: F) -> F::Output {
    IO_TRACE.scope(Arc::new(IoTrace::new()), future).await
}

/// Wrap a query future in a fresh trace, run it, then emit the captured
/// snapshot as a [`TARGET`] event labelled by `tenant_id`/`route`. This is the
/// one call a request handler adds at the query boundary — co-locate it with
/// the existing `predicate_diagnostics::scope`.
pub async fn instrument<F>(
    tenant_id: Option<String>,
    route: impl Into<String>,
    future: F,
) -> F::Output
where
    F: std::future::Future,
{
    let route = route.into();
    IO_TRACE
        .scope(Arc::new(IoTrace::new()), async move {
            let out = future.await;
            // Still inside the scope: read and emit before the binding drops.
            if let Ok((snap, stamped_route)) = IO_TRACE.try_with(|t| (t.snapshot(), t.route())) {
                snap.emit(tenant_id.as_deref(), &route);
                // C4 ingestion: if a route was stamped, feed the cost model the
                // measured cost of this (shape-class, backend). Skip empty traces.
                if !snap.is_empty()
                    && let Some((shape_class, backend_label)) = stamped_route
                {
                    notify_route_observer(&snap, &shape_class, &backend_label);
                }
                // T2.2 ingestion: feed the cache orchestrator for trace-driven sizing.
                // Skip empty traces to avoid wasteful cache budget updates.
                if !snap.is_empty() {
                    notify_cache_observer(&snap);
                }
                // ADR-030 billing fan-out (always-on): emit the per-tenant
                // consumption meters (KRU) from the same snapshot. Tenant + the
                // per-engine compute_ms are both in hand here.
                if !snap.is_empty() {
                    notify_billing_observer(&snap, tenant_id.as_deref());
                }
            }
            out
        })
        .await
}

#[cfg(all(test, feature = "io-trace"))]
mod tests {
    use super::*;

    /// ADR-030: the always-on billing observer fires at scope close with the
    /// owning tenant and the same accumulated snapshot the perf observers see —
    /// so KRU is emitted from `compute_ms` and cannot diverge from the cost
    /// model's view. Drives the real `instrument` drain path end to end.
    #[tokio::test]
    #[allow(clippy::type_complexity)] // test-only capture cell for the billing observer
    async fn billing_observer_receives_tenant_and_accumulated_compute_ms() {
        use std::sync::{Arc, Mutex};
        let captured: Arc<Mutex<Option<(Option<String>, u64, u64)>>> = Arc::new(Mutex::new(None));
        let sink = captured.clone();
        set_billing_observer(Some(Box::new(move |snap, tenant| {
            *sink.lock().unwrap_or_else(|p| p.into_inner()) = Some((
                tenant.map(String::from),
                snap.total_compute_ms(),
                snap.bytes_read,
            ));
        })));

        instrument(Some("acme".to_string()), "test-route", async {
            record_compute_ms("native", 4);
            record_compute_ms("native", 3);
            record_bytes_read(100);
        })
        .await;

        set_billing_observer(None); // restore global state for other tests

        let got = captured.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert_eq!(
            got,
            Some((Some("acme".to_string()), 7, 100)),
            "billing observer must receive the tenant + summed compute_ms (4+3) + bytes_read from the same snapshot"
        );
    }

    #[test]
    fn classify_maps_known_verbs() {
        assert_eq!(IoOp::classify("list_pax"), IoOp::List);
        assert_eq!(IoOp::classify("list_parquet"), IoOp::List);
        assert_eq!(IoOp::classify("read_parquet"), IoOp::Get);
        assert_eq!(IoOp::classify("fetch_pax_ranged"), IoOp::Get);
        assert_eq!(IoOp::classify("write_parquet"), IoOp::Put);
        assert_eq!(IoOp::classify("delete_segment"), IoOp::Delete);
        // Unknown verb is the conservative read default.
        assert_eq!(IoOp::classify("mystery"), IoOp::Get);
    }

    #[test]
    fn snapshot_aggregates_and_derives() {
        let t = IoTrace::new();
        t.record_op(IoOp::List);
        t.record_op(IoOp::Get);
        t.record_op(IoOp::Get);
        t.record_bytes_read(1_024);
        t.record_bytes_read(512);
        t.record_range_gets(3);
        t.record_footer(true);
        t.record_footer(true);
        t.record_footer(false);
        t.record_egress_bytes(2_048);
        t.record_compute_ms("volcano", 3);
        t.record_compute_ms("volcano", 4);
        t.record_compute_ms("datafusion", 10);

        let s = t.snapshot();
        assert_eq!(s.get_ops, 2);
        assert_eq!(s.list_ops, 1);
        assert_eq!(s.total_ops(), 3);
        assert_eq!(s.bytes_read, 1_536);
        assert_eq!(s.range_gets, 3);
        assert_eq!(s.avg_get_bytes(), Some(1_536.0 / 3.0));
        assert_eq!(s.egress_bytes, 2_048);
        assert_eq!(s.footer_hit_ratio(), Some(2.0 / 3.0));
        assert_eq!(s.total_compute_ms(), 17);
        assert_eq!(s.compute_ms.get("volcano"), Some(&7));
        assert!(!s.is_empty());
    }

    #[test]
    fn empty_snapshot_is_empty() {
        assert!(IoTrace::new().snapshot().is_empty());
        assert_eq!(IoTraceSnapshot::default().footer_hit_ratio(), None);
        assert_eq!(IoTraceSnapshot::default().avg_get_bytes(), None);
    }

    #[tokio::test]
    async fn free_helpers_record_into_active_scope() {
        let captured = scope(async {
            record_op_str("fetch_pax");
            record_op_str("list_pax");
            record_bytes_read(4_096);
            record_footer(false);
            record_compute_ms("sst", 5);
            snapshot()
        })
        .await;

        let s = captured.expect("snapshot inside scope");
        assert_eq!(s.get_ops, 1);
        assert_eq!(s.list_ops, 1);
        assert_eq!(s.bytes_read, 4_096);
        assert_eq!(s.footer_misses, 1);
        assert_eq!(s.compute_ms.get("sst"), Some(&5));
    }

    #[tokio::test]
    async fn free_helpers_noop_outside_scope() {
        // No active scope: every helper must silently no-op, never panic, and
        // snapshot() returns None.
        record_op(IoOp::Get);
        record_bytes_read(999);
        record_footer(true);
        record_compute_ms("sst", 1);
        assert!(snapshot().is_none());
    }

    #[tokio::test]
    async fn record_footers_batches_outcomes() {
        let s = scope(async {
            record_footers(7, 3);
            record_footer(true); // one more hit on top
            snapshot()
        })
        .await
        .expect("snapshot inside scope");
        assert_eq!(s.footer_hits, 8);
        assert_eq!(s.footer_misses, 3);
        assert_eq!(s.footer_hit_ratio(), Some(8.0 / 11.0));
    }

    #[tokio::test]
    async fn record_route_round_trips_in_scope_and_noops_outside() {
        record_route("olap/parquet", "DataFusionLocal"); // outside scope: no-op
        let r = scope(async {
            record_route("olap/parquet", "DataFusionLocal");
            IO_TRACE.try_with(|t| t.route()).ok().flatten()
        })
        .await;
        assert_eq!(
            r,
            Some(("olap/parquet".to_string(), "DataFusionLocal".to_string()))
        );
    }

    /// TD-WLP-6: the storage-profile stamp round-trips in scope (and into the
    /// snapshot), no-ops outside a scope.
    #[tokio::test]
    async fn record_storage_profile_round_trips_and_reaches_snapshot() {
        record_storage_profile("churn"); // outside scope: no-op
        let snap = scope(async {
            record_storage_profile("churn");
            IO_TRACE.try_with(|t| t.snapshot()).unwrap()
        })
        .await;
        assert_eq!(snap.storage_profile.as_deref(), Some("churn"));
        // A fresh trace has no profile stamp.
        assert_eq!(IoTrace::new().snapshot().storage_profile, None);
    }

    // Both flush→observer cases live in ONE test: the route observer is a
    // process-global, so running the set/clear cases sequentially here avoids a
    // race with a parallel sibling test. (Other instrument tests stamp no route,
    // so they never invoke the observer regardless of install order.)
    #[tokio::test]
    async fn flush_feeds_route_observer_only_when_route_is_stamped() {
        use std::sync::Arc;
        let seen: Arc<Mutex<Vec<(IoTraceSnapshot, String, String)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        set_route_observer(Some(Box::new(move |snap, class, backend| {
            sink.lock()
                .unwrap()
                .push((snap.clone(), class.to_string(), backend.to_string()));
        })));

        // (1) Stamped route → observer fires with the measured snapshot.
        instrument(Some("t".to_string()), "pgwire.query", async {
            record_op_str("fetch_pax_ranged");
            record_bytes_read(8 << 20);
            record_range_gets(2);
            record_route("olap/parquet", "DataFusionLocal");
        })
        .await;

        // (2) I/O but NO route stamped → observer must NOT fire.
        instrument(None, "rest.v2.records.search", async {
            record_op_str("fetch_pax");
        })
        .await;

        set_route_observer(None); // reset global so other tests are unaffected

        let got = seen.lock().unwrap();
        assert_eq!(
            got.len(),
            1,
            "observer fired exactly once (only the routed query)"
        );
        let (snap, class, backend) = &got[0];
        assert_eq!(class, "olap/parquet");
        assert_eq!(backend, "DataFusionLocal");
        assert_eq!(snap.range_gets, 2);
        assert_eq!(snap.bytes_read, 8 << 20);
    }

    #[test]
    fn snapshot_json_round_trips_for_export() {
        // §4.4 enabler: the snapshot is the export payload (OTLP attrs / JSON
        // sink / cost-model reader), so it must serde round-trip losslessly.
        let mut compute = BTreeMap::new();
        compute.insert("datafusion".to_string(), 12u64);
        let s = IoTraceSnapshot {
            get_ops: 5,
            list_ops: 1,
            bytes_read: 8_388_608,
            range_gets: 2,
            footer_hits: 3,
            footer_misses: 1,
            egress_bytes: 4_096,
            compute_ms: compute,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).expect("serialize");
        let back: IoTraceSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
        // Spot-check a derived field survives the data it depends on.
        assert_eq!(back.avg_get_bytes(), Some(8_388_608.0 / 2.0));
    }

    #[tokio::test]
    async fn instrument_runs_and_returns_output() {
        let out = instrument(
            Some("tenant-a".to_string()),
            "rest.v2.records.scan",
            async {
                record_op_str("fetch_pax");
                record_bytes_read(2_048);
                42
            },
        )
        .await;
        assert_eq!(out, 42);
    }

    #[test]
    fn embedding_recording_aggregates_correctly() {
        let t = IoTrace::new();
        t.record_embedding_calls(5);
        t.record_embedding_input_tokens(1000);
        t.record_embedding_output_tokens(2000);

        // Record individual tokens
        t.record_embedding_input_tokens(500);
        t.record_embedding_output_tokens(1000);

        let s = t.snapshot();
        assert_eq!(s.embedding_calls, 5);
        assert_eq!(s.embedding_input_tokens, 1500);
        assert_eq!(s.embedding_output_tokens, 3000);
        assert_eq!(s.total_embedding_tokens(), 4500);
    }

    #[test]
    fn embedding_records_complete_operation() {
        let t = IoTrace::new();
        t.record_embedding(100, 200); // input, output
        t.record_embedding(50, 100);

        let s = t.snapshot();
        assert_eq!(s.embedding_calls, 2);
        assert_eq!(s.embedding_input_tokens, 150);
        assert_eq!(s.embedding_output_tokens, 300);
        assert_eq!(s.total_embedding_tokens(), 450);
    }

    #[tokio::test]
    async fn free_helpers_record_embedding_in_scope() {
        let captured = scope(async {
            record_embedding_calls(3);
            record_embedding_input_tokens(100);
            record_embedding_output_tokens(200);
            record_embedding(50, 75); // +1 call, +50 input, +75 output
            snapshot()
        })
        .await;

        let s = captured.expect("snapshot inside scope");
        assert_eq!(s.embedding_calls, 4); // 3 + 1
        assert_eq!(s.embedding_input_tokens, 150); // 100 + 50
        assert_eq!(s.embedding_output_tokens, 275); // 200 + 75
        assert_eq!(s.total_embedding_tokens(), 425);
    }

    #[tokio::test]
    async fn empty_snapshot_includes_embedding_fields() {
        let s = IoTrace::new().snapshot();
        assert_eq!(s.embedding_calls, 0);
        assert_eq!(s.embedding_input_tokens, 0);
        assert_eq!(s.embedding_output_tokens, 0);
        assert_eq!(s.total_embedding_tokens(), 0);
    }

    #[tokio::test]
    async fn snapshot_json_includes_embedding_fields() {
        let mut compute = BTreeMap::new();
        compute.insert("datafusion".to_string(), 12u64);
        let s = IoTraceSnapshot {
            get_ops: 5,
            list_ops: 1,
            bytes_read: 8_388_608,
            range_gets: 2,
            footer_hits: 3,
            footer_misses: 1,
            egress_bytes: 4_096,
            embedding_calls: 10,
            embedding_input_tokens: 1000,
            embedding_output_tokens: 2000,
            compute_ms: compute,
            ..Default::default()
        };

        let json = serde_json::to_string(&s).expect("serialize");
        let back: IoTraceSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
        assert_eq!(back.total_embedding_tokens(), 3000);
    }
}
