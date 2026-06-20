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
//! [`crate::observability::predicate_diagnostics`] — so any depth of the
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
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// `tracing` target for emitted per-query I/O trace events. Kept distinct so
/// operators can route/sample it independently (and the future OTLP layer can
/// map it to a span).
pub const TARGET: &str = "proximadb::io_trace";

tokio::task_local! {
    /// Active per-query I/O trace for the current task. Bound by [`scope`] /
    /// [`instrument`].
    static IO_TRACE: IoTrace;
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
    /// Bytes written to object storage (ingest/flush — KIU).
    bytes_written: AtomicU64,
    /// Footer/metadata cache outcomes (Dimension 3 — the highest-ROI cache).
    footer_hits: AtomicU64,
    footer_misses: AtomicU64,
    /// Bytes moved across an availability zone (Dimension 2 — KEU, the egress
    /// term that is currently unmetered and the spec's named gap).
    bytes_cross_az: AtomicU64,
    /// Compute milliseconds attributed by engine (Dimension 4 — KRU/KIU). Kept
    /// in a small map so a single query that touches multiple engines (e.g. a
    /// Volcano point lookup plus a DataFusion aggregate) attributes each.
    compute_ms: Mutex<BTreeMap<String, u64>>,
    /// The route this query was served on, as `(shape_class, backend_label)`
    /// strings (co-design C4). Stamped by the `ComputeScheduler` so the flush can
    /// feed the trace-driven cost model the cost of *this kind of query on that
    /// engine* — without io_trace depending on any query-layer type. `None` until
    /// a route is chosen.
    route: Mutex<Option<(String, String)>>,
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

    /// Add to the count of ranged GET requests issued.
    pub fn record_range_gets(&self, gets: u64) {
        self.range_gets.fetch_add(gets, Ordering::Relaxed);
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

    /// Add to bytes moved across an availability zone (KEU / egress).
    pub fn record_cross_az_bytes(&self, bytes: u64) {
        self.bytes_cross_az.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Attribute compute milliseconds to a named engine.
    pub fn record_compute_ms(&self, engine: &str, ms: u64) {
        let mut g = self.compute_ms.lock().unwrap_or_else(|p| p.into_inner());
        *g.entry(engine.to_string()).or_insert(0) += ms;
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
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            footer_hits: self.footer_hits.load(Ordering::Relaxed),
            footer_misses: self.footer_misses.load(Ordering::Relaxed),
            bytes_cross_az: self.bytes_cross_az.load(Ordering::Relaxed),
            compute_ms: self
                .compute_ms
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
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
    pub bytes_written: u64,
    pub footer_hits: u64,
    pub footer_misses: u64,
    pub bytes_cross_az: u64,
    pub compute_ms: BTreeMap<String, u64>,
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
            && self.bytes_cross_az == 0
            && self.compute_ms.is_empty()
    }

    /// Emit this snapshot as a structured `tracing` event under [`TARGET`].
    /// No-op when empty. `tenant_id` and `route` label the query; all physical
    /// quantities become event fields the OTLP layer (§4.4) maps to a span.
    pub fn emit(&self, tenant_id: Option<&str>, route: &str) {
        if self.is_empty() {
            return;
        }
        tracing::info!(
            target: TARGET,
            tenant_id = tenant_id.unwrap_or("default"),
            route = route,
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
            bytes_cross_az = self.bytes_cross_az,
            compute_ms_total = self.total_compute_ms(),
            compute_ms_by_engine = ?self.compute_ms,
            "per-query I/O trace"
        );
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

/// Add to the count of ranged GET requests for the active query.
pub fn record_range_gets(gets: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_range_gets(gets));
}

/// Add to bytes written to object storage for the active query.
pub fn record_bytes_written(bytes: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_bytes_written(bytes));
}

/// Record a footer/metadata cache outcome for the active query.
pub fn record_footer(hit: bool) {
    let _ = IO_TRACE.try_with(|t| t.record_footer(hit));
}

/// Record a batch of footer/metadata cache outcomes for the active query.
pub fn record_footers(hits: u64, misses: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_footers(hits, misses));
}

/// Record cross-AZ bytes (KEU/egress) for the active query.
pub fn record_cross_az_bytes(bytes: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_cross_az_bytes(bytes));
}

/// Attribute compute milliseconds to `engine` for the active query.
pub fn record_compute_ms(engine: &str, ms: u64) {
    let _ = IO_TRACE.try_with(|t| t.record_compute_ms(engine, ms));
}

/// Stamp the route (`shape_class`, `backend_label`) onto the active query trace.
/// Silently no-ops outside an active scope. Neutral strings — io_trace never
/// depends on a query-layer type.
pub fn record_route(shape_class: &str, backend_label: &str) {
    let _ = IO_TRACE.try_with(|t| t.record_route(shape_class, backend_label));
}

/// Snapshot the active query trace, if any.
pub fn snapshot() -> Option<IoTraceSnapshot> {
    IO_TRACE.try_with(|t| t.snapshot()).ok()
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

/// Bind a fresh [`IoTrace`] to `future` and await it. Lower-level than
/// [`instrument`]; use when the caller wants to read the snapshot itself before
/// the scope ends.
pub async fn scope<F: std::future::Future>(future: F) -> F::Output {
    IO_TRACE.scope(IoTrace::new(), future).await
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
        .scope(IoTrace::new(), async move {
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
            }
            out
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

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
        t.record_cross_az_bytes(2_048);
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
        assert_eq!(s.bytes_cross_az, 2_048);
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
        assert_eq!(got.len(), 1, "observer fired exactly once (only the routed query)");
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
            bytes_cross_az: 4_096,
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
}
