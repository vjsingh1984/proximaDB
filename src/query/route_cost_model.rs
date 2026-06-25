// Copyright 2026 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Trace-driven route cost model (co-design C4 — the keystone of the loop).
//!
//! The `ComputeScheduler` ([`crate::query::compute_scheduler`]) chooses a
//! physical engine from *static* shape heuristics. C4 closes the co-design loop
//! the C0 trace substrate was built for: feed the *measured* per-query
//! [`IoTraceSnapshot`] back so the scheduler can cost candidate routes from what
//! they *actually paid* (object-store GETs, bytes moved, compute-ms) rather than
//! a fixed rule — the Policy→RL evolution named in the course correction §5.2.
//!
//! ## How the loop works
//!
//! 1. A query runs under an `io_trace` scope (C0) and, at completion, yields an
//!    [`IoTraceSnapshot`] of the physical quantities it paid.
//! 2. [`RouteCostModel::observe`] folds that snapshot into a per-(shape-class,
//!    backend) EWMA — the running cost of serving *this kind of query* on *that
//!    engine*.
//! 3. At the next route decision for the same shape-class,
//!    [`RouteCostModel::recommend`] compares the learned cost of each candidate
//!    backend and names the cheapest. The scheduler surfaces this as an
//!    EXPLAIN/telemetry advisory (**observe-mode** — see
//!    `ComputeScheduler::route_select_advised`); flipping live routing onto the
//!    recommendation is a later, flag-gated slice.
//!
//! ## OSS boundary — neutral cost, not pricing
//!
//! The score combines *neutral relative I/O/compute units*, NOT currency. Per
//! the OSS/enterprise boundary, *pricing* (KSU/KRU/KEU $ weights, tenant
//! tiers) belongs to the commercial control plane; *routing* is OSS mechanism.
//! The default [`CostWeights`] only encode the co-design ordering — an
//! object-store round-trip dominates bandwidth, which dominates CPU (P5: the
//! dominant cost term for a cloud DB is I/O round-trips, not compute) — and are
//! tunable. They are deliberately *not* calibrated dollar figures.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::observability::io_trace::IoTraceSnapshot;
use crate::query::compute_scheduler::{QueryShape, backend_label};
use crate::query::table_write_plan::ComputeBackend;

/// Process-global trace-driven route cost model. Fed by completed routed queries
/// (via [`install_route_cost_observer`]) and consulted at the route decision.
pub static GLOBAL_ROUTE_COST_MODEL: LazyLock<RouteCostModel> = LazyLock::new(RouteCostModel::new);

/// Wire the io_trace flush to feed [`GLOBAL_ROUTE_COST_MODEL`]: every completed
/// query that stamped a route (via `ComputeScheduler`) folds its measured
/// snapshot into the model. Call once at startup. This is the query-layer half
/// of the dependency-inversion seam — io_trace defines the observer type and
/// never depends on this module.
pub fn install_route_cost_observer() {
    crate::observability::io_trace::set_route_observer(Some(Box::new(
        |snap, shape_class, backend_label| {
            GLOBAL_ROUTE_COST_MODEL.observe_by_label(shape_class, backend_label, snap);
        },
    )));
    // Flag-gated live override (default OFF). When PROXIMADB_ROUTE_COST_OVERRIDE
    // is truthy, the warmed model may flip freshness-safe routes to the cheaper
    // backend (slice 4); otherwise the model stays observe-only.
    let on = std::env::var("PROXIMADB_ROUTE_COST_OVERRIDE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    GLOBAL_ROUTE_COST_MODEL.set_override_enabled(on);
}

// ── Per-Parquet-location shape stats (route-time classification) ────────────

/// One Parquet table's physical shape, warmed for free wherever the table is
/// opened and peeked at route time so the cost-model shape-class discriminates
/// OLAP-over-Parquet queries by fan-out / cardinality WITHOUT a cold footer
/// read on the route path (co-design P5: I/O round-trips, not CPU, dominate).
#[derive(Clone, Copy, Debug)]
pub struct TableShapeStat {
    /// Row-group / split count — the GET-fan-out a scan of this table pays.
    pub row_groups: u32,
    /// Estimated row count from footer statistics, when the footer carries it.
    pub rows: Option<u64>,
}

/// Process-global, bounded cache of per-Parquet-location [`TableShapeStat`]s.
/// Warmed as a free side-effect of opening a table for execution/EXPLAIN (the
/// opener already reads the footer) and consulted at the route decision so the
/// scheduler's [`QueryShape`] carries real `partition_fanout` / `cardinality`
/// for hot tables. A cold (never-scanned) table simply lacks an entry → the
/// shape stays coarse (`Unknown`), so a fresh table changes nothing until its
/// first scan warms the stat. Workload-adaptive, and adds zero I/O to routing.
static GLOBAL_TABLE_SHAPE_STATS: LazyLock<Mutex<HashMap<String, TableShapeStat>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Soft cap on cached locations. Locations are catalog-bounded in practice; this
/// only guards a runaway (e.g. a bug minting synthetic locations). Over-cap the
/// cache clears wholesale — stat data is advisory and re-warms on the next scan.
const TABLE_SHAPE_STAT_CAP: usize = 8192;

/// Record (or refresh) a table's shape stat. Called by the Parquet opener after
/// it has the footer, so this adds no I/O. Best-effort: a poisoned lock or an
/// over-cap cache is silently dropped (classification then falls back coarse).
pub fn record_table_shape_stat(location: &str, row_groups: u32, rows: Option<u64>) {
    let mut guard = GLOBAL_TABLE_SHAPE_STATS
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if guard.len() >= TABLE_SHAPE_STAT_CAP {
        guard.clear();
    }
    guard.insert(location.to_string(), TableShapeStat { row_groups, rows });
}

/// Aggregate the cached stats for `locations` into cost-model shape buckets.
///
/// Conservative: if *any* referenced location lacks a warmed stat, returns
/// `(Unknown, Unknown)` — never half-classify a multi-table query from partial
/// data, since an unknown table could dominate fan-out / cardinality. An empty
/// slice also yields `Unknown`. Sums row-groups (total GET fan-out) and rows
/// (total scan cardinality) across the tables, then buckets via the scheduler's
/// [`PartitionFanout`] / [`CardinalityClass`].
///
/// [`PartitionFanout`]: crate::query::compute_scheduler::PartitionFanout
/// [`CardinalityClass`]: crate::query::compute_scheduler::CardinalityClass
pub fn classify_table_shapes(
    locations: &[String],
) -> (
    crate::query::compute_scheduler::PartitionFanout,
    crate::query::compute_scheduler::CardinalityClass,
) {
    use crate::query::compute_scheduler::{CardinalityClass, PartitionFanout};
    if locations.is_empty() {
        return (PartitionFanout::Unknown, CardinalityClass::Unknown);
    }
    let guard = GLOBAL_TABLE_SHAPE_STATS
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let mut total_row_groups = 0u32;
    let mut total_rows: Option<u64> = Some(0);
    for loc in locations {
        let Some(stat) = guard.get(loc) else {
            return (PartitionFanout::Unknown, CardinalityClass::Unknown);
        };
        total_row_groups = total_row_groups.saturating_add(stat.row_groups);
        total_rows = match (total_rows, stat.rows) {
            (Some(acc), Some(r)) => Some(acc.saturating_add(r)),
            _ => None,
        };
    }
    (
        PartitionFanout::from_count(Some(total_row_groups)),
        CardinalityClass::from_estimate(total_rows),
    )
}

/// Per-tenant governance **tier-entitlement multiplier** resolver — the final
/// term of the §3 `Cost(q)` objective (Dimension 5, C5). Maps a `tenant_id` to a
/// neutral scalar applied to the *reported* cost.
type TierMultiplierFn = dyn Fn(&str) -> f64 + Send + Sync;

static TIER_MULTIPLIER_RESOLVER: Mutex<Option<Box<TierMultiplierFn>>> = Mutex::new(None);

/// Install (or clear with `None`) the per-tenant tier-multiplier resolver. This
/// is the OSS **mechanism** / DI seam (the `LimitsResolver` pattern): the OSS
/// core ships no resolver (default multiplier `1.0`, fully inert); the commercial
/// control plane (anvaiops) installs one at startup that maps a tenant to its
/// tier entitlement from claims/config. Replaceable in tests. No new authority —
/// the tenant→tier authority stays in the control plane; this only consults it.
pub fn set_tier_multiplier_resolver(resolver: Option<Box<TierMultiplierFn>>) {
    *TIER_MULTIPLIER_RESOLVER
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = resolver;
}

/// The tier-entitlement multiplier for `tenant_id`. Returns `1.0` (inert) when no
/// resolver is installed, the tenant is unknown (`None`), or the resolver returns
/// a non-finite / non-positive value — a non-positive multiplier could invert
/// cost ordering, so it is rejected fail-safe.
pub fn tier_entitlement_multiplier(tenant_id: Option<&str>) -> f64 {
    let Some(id) = tenant_id else {
        return 1.0;
    };
    let guard = TIER_MULTIPLIER_RESOLVER
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    match guard.as_ref() {
        Some(resolve) => {
            let m = resolve(id);
            if m.is_finite() && m > 0.0 { m } else { 1.0 }
        }
        None => 1.0,
    }
}

/// The §3 final per-tenant `Cost(q)`: the neutral base route score scaled by the
/// tenant's tier-entitlement multiplier — the last term of the unified cost
/// objective. This is deliberately **routing-neutral**: a single per-tenant
/// scalar scales every candidate equally, so it can never change *which* engine
/// is cheapest (entitlement that restricts the candidate *set* is the scheduler's
/// `override_candidates`, safe-by-construction). Its role is to make the reported
/// / billed cost the real per-tenant `Cost(q)` for EXPLAIN, chargeback, and a
/// future budget-aware admission decision — completing the objective the C0/C4
/// loop was built to minimize, while keeping pricing/policy out of the OSS core.
pub fn final_cost(base_score: f64, tenant_id: Option<&str>) -> f64 {
    base_score * tier_entitlement_multiplier(tenant_id)
}

/// Stable shape-class key the model aggregates under. The coarse base mirrors the
/// scheduler's binary signals (`engages_relational` × `parquet_backed`); C4
/// Phase-2b refines it with the §5.2 Phase-1 inputs — cardinality and partition
/// fan-out — but only when they are *known*. An `Unknown` signal contributes no
/// suffix, so a planner that cannot estimate them yields exactly the original
/// 2-part key (`olap/parquet`, `oltp/native`, …) — backward-compatible with
/// warmed cells and existing EXPLAIN output.
pub fn shape_class(shape: &QueryShape) -> String {
    let workload = if shape.engages_relational {
        "olap"
    } else {
        "oltp"
    };
    let base = if shape.parquet_backed {
        "parquet"
    } else {
        "native"
    };
    let mut class = format!("{workload}/{base}");
    for suffix in [
        shape.cardinality.class_suffix(),
        shape.partition_fanout.class_suffix(),
    ]
    .into_iter()
    .flatten()
    {
        class.push('/');
        class.push_str(suffix);
    }
    class
}

/// Neutral relative weights for the route cost score. NOT dollars (see module
/// docs). They encode only the co-design cost ordering of the §3 unified cost
/// objective `Cost(q)`: an object-store round-trip dominates cross-region egress,
/// which dominates same-region read bandwidth, which dominates a ms of CPU
/// (P5: for a cloud DB the dominant cost term is I/O round-trips + egress, not
/// compute — §2.2).
#[derive(Debug, Clone, Copy)]
pub struct CostWeights {
    /// Per ranged GET — the fixed-latency, per-request term (the dominant cost
    /// for object-store-served reads). §3 `GET_count·get_fee`.
    pub per_get: f64,
    /// Per MiB read — the same-region read-bandwidth term. §3 `bytes_read·bw_cost`.
    pub per_mib_read: f64,
    /// Per MiB moved cross-region / to the internet — the KEU **egress** term
    /// (§2.2, §3 `bytes_moved·locality_cost`). Weighted above same-region read
    /// bandwidth because cross-region (~$0.02/GB) / internet egress
    /// (~$0.09-0.12/GB by cloud) is frequently the dominant TCO term. Fed by the
    /// trace's `egress_bytes`, which is **zero on the free same-region path**, so
    /// this term is inert until a deployment actually moves bytes cross-region.
    pub per_mib_egress: f64,
    /// Per MiB written to object storage — the KIU ingest / storage-write term
    /// (§3 storage side). Zero for read-only SELECT routing, so inert today;
    /// present so a write-route cost (`table_write_plan`) folds in here too.
    pub per_mib_written: f64,
    /// Per compute-millisecond — the CPU term (smallest for I/O-bound DB work).
    /// §3 `engine_ms·rate`.
    pub per_compute_ms: f64,
}

impl Default for CostWeights {
    fn default() -> Self {
        // Illustrative neutral ordering (round-trip ≫ egress ≫ read-bw ≫ CPU),
        // tunable. `per_mib_written` mirrors `per_mib_read` (a PUT's bandwidth).
        Self {
            per_get: 20.0,
            per_mib_read: 5.0,
            per_mib_egress: 25.0,
            per_mib_written: 5.0,
            per_compute_ms: 1.0,
        }
    }
}

/// The cost-bearing physical quantities of one query, extracted from its
/// measured [`IoTraceSnapshot`]. Folded (EWMA) per (shape-class, backend) and
/// scored by [`CostWeights`] into the §3 `Cost(q)`. Keeping them in one struct
/// (rather than positional `f64` args) keeps `fold`/`score` self-documenting as
/// terms are added.
#[derive(Debug, Clone, Copy, Default)]
struct CostQuantities {
    range_gets: f64,
    bytes_read: f64,
    egress_bytes: f64,
    bytes_written: f64,
    compute_ms: f64,
}

impl CostQuantities {
    fn from_snapshot(snap: &IoTraceSnapshot) -> Self {
        Self {
            range_gets: snap.range_gets as f64,
            bytes_read: snap.bytes_read as f64,
            egress_bytes: snap.egress_bytes as f64,
            bytes_written: snap.bytes_written as f64,
            compute_ms: snap.total_compute_ms() as f64,
        }
    }
}

/// One (shape-class, backend) cell: EWMA means of the cost-bearing quantities.
#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    range_gets: f64,
    bytes_read: f64,
    egress_bytes: f64,
    bytes_written: f64,
    compute_ms: f64,
    samples: u64,
}

impl Cell {
    fn fold(&mut self, alpha: f64, q: CostQuantities) {
        if self.samples == 0 {
            self.range_gets = q.range_gets;
            self.bytes_read = q.bytes_read;
            self.egress_bytes = q.egress_bytes;
            self.bytes_written = q.bytes_written;
            self.compute_ms = q.compute_ms;
        } else {
            let blend = |old: f64, new: f64| alpha * new + (1.0 - alpha) * old;
            self.range_gets = blend(self.range_gets, q.range_gets);
            self.bytes_read = blend(self.bytes_read, q.bytes_read);
            self.egress_bytes = blend(self.egress_bytes, q.egress_bytes);
            self.bytes_written = blend(self.bytes_written, q.bytes_written);
            self.compute_ms = blend(self.compute_ms, q.compute_ms);
        }
        self.samples += 1;
    }

    fn quantities(&self) -> CostQuantities {
        CostQuantities {
            range_gets: self.range_gets,
            bytes_read: self.bytes_read,
            egress_bytes: self.egress_bytes,
            bytes_written: self.bytes_written,
            compute_ms: self.compute_ms,
        }
    }
}

/// Learned cost estimate for serving a shape-class on one backend — the EWMA of
/// each §3 `Cost(q)` quantity plus the weighted neutral score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteCost {
    pub samples: u64,
    pub range_gets: f64,
    pub bytes_read: f64,
    /// EWMA cross-region / internet egress bytes (KEU) — zero on the free same-region path.
    pub egress_bytes: f64,
    /// EWMA bytes written to object storage (KIU) — zero for read-only routes.
    pub bytes_written: f64,
    pub compute_ms: f64,
    /// Weighted neutral score (lower is cheaper).
    pub score: f64,
}

/// The scheduler-facing recommendation: the cheapest candidate that has enough
/// history, plus the runners-up for EXPLAIN.
#[derive(Debug, Clone)]
pub struct RouteRecommendation {
    pub backend: ComputeBackend,
    pub score: f64,
    pub samples: u64,
    /// `(backend_label, score)` for every candidate with history, cheapest first.
    pub ranked: Vec<(String, f64)>,
}

impl RouteRecommendation {
    /// Compact reason string for the EXPLAIN/telemetry advisory.
    pub fn reason(&self) -> String {
        let ranked = self
            .ranked
            .iter()
            .map(|(b, s)| format!("{b}={s:.1}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "min-cost over {} sample(s): {} [{}]",
            self.samples,
            backend_label(&self.backend),
            ranked
        )
    }
}

/// Trace-driven, thread-safe route cost model. One instance is consulted at the
/// route decision and fed by completed-query traces.
#[derive(Debug)]
pub struct RouteCostModel {
    cells: Mutex<HashMap<(String, String), Cell>>,
    alpha: f64,
    weights: CostWeights,
    /// Minimum samples before a backend's estimate is trusted enough to compare.
    min_samples: u64,
    /// Live route override (flag-gated; default OFF → observe-only). Interior
    /// mutability so the process-global model can be toggled at startup.
    override_enabled: AtomicBool,
    /// Minimum fractional cost advantage the recommended backend must beat the
    /// static choice by before a live override fires — guards against flapping
    /// on marginal/noisy differences.
    min_advantage: f64,
    /// Phase-2 exploration: counter rate-limiting how often an under-explored
    /// freshness-safe candidate is sampled (~1 in `exploration_interval`).
    explore_tick: AtomicU64,
    exploration_interval: u64,
}

impl Default for RouteCostModel {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteCostModel {
    /// EWMA α=0.2, default neutral weights, 3-sample warmup, override OFF,
    /// 15% min advantage before an override fires.
    pub fn new() -> Self {
        Self {
            cells: Mutex::new(HashMap::new()),
            alpha: 0.2,
            weights: CostWeights::default(),
            min_samples: 3,
            override_enabled: AtomicBool::new(false),
            min_advantage: 0.15,
            explore_tick: AtomicU64::new(0),
            exploration_interval: 16,
        }
    }

    /// Tune how often exploration samples an under-explored candidate (~1 in N
    /// eligible decisions). Lower = warms faster but costs more; min 1.
    pub fn with_exploration_interval(mut self, n: u64) -> Self {
        self.exploration_interval = n.max(1);
        self
    }

    /// Override the warmup threshold (samples required before a cell is trusted).
    pub fn with_min_samples(mut self, n: u64) -> Self {
        self.min_samples = n.max(1);
        self
    }

    /// Tune the minimum cost advantage (fraction in [0,1)) required to override.
    pub fn with_min_advantage(mut self, frac: f64) -> Self {
        self.min_advantage = frac.clamp(0.0, 0.99);
        self
    }

    /// Enable/disable live route override (flag-gated; default OFF).
    pub fn set_override_enabled(&self, on: bool) {
        self.override_enabled.store(on, Ordering::Relaxed);
    }

    /// Whether live override is currently enabled.
    pub fn override_active(&self) -> bool {
        self.override_enabled.load(Ordering::Relaxed)
    }

    /// The §3 unified `Cost(q)` in neutral units: read (GETs + bandwidth) +
    /// egress (KEU) + ingest/storage-write (KIU) + compute. The storage·time
    /// (KSU) term and the tenant tier multiplier are *not* per-query routing
    /// discriminators — KSU is a per-tenant aggregate the consumption meter owns,
    /// and a scalar tier multiplier cancels across candidates (slice 4 applies it
    /// only to the *reported* cost). The cache-hit offset is already captured: a
    /// footer-cache hit shows up as fewer GETs / fewer bytes in the trace.
    fn score(&self, q: CostQuantities) -> f64 {
        let mib = |bytes: f64| bytes / (1024.0 * 1024.0);
        self.weights.per_get * q.range_gets
            + self.weights.per_mib_read * mib(q.bytes_read)
            + self.weights.per_mib_egress * mib(q.egress_bytes)
            + self.weights.per_mib_written * mib(q.bytes_written)
            + self.weights.per_compute_ms * q.compute_ms
    }

    /// Fold a completed query's measured trace into the (shape-class, backend)
    /// cell. `compute_ms` is the snapshot's total across engines (the query was
    /// served by `backend`, so its compute is attributed there).
    pub fn observe(&self, shape_class: &str, backend: &ComputeBackend, snap: &IoTraceSnapshot) {
        self.observe_by_label(shape_class, &backend_label(backend), snap);
    }

    /// Like [`Self::observe`] but keyed by the backend's label string directly —
    /// the form the io_trace ingestion observer uses, since io_trace stamps a
    /// neutral label, not a `ComputeBackend` (no layer-up dependency).
    pub fn observe_by_label(&self, shape_class: &str, backend_label: &str, snap: &IoTraceSnapshot) {
        let key = (shape_class.to_string(), backend_label.to_string());
        let mut cells = self.cells.lock().unwrap_or_else(|p| p.into_inner());
        cells
            .entry(key)
            .or_default()
            .fold(self.alpha, CostQuantities::from_snapshot(snap));
    }

    /// Current learned estimate for one (shape-class, backend), if any history.
    pub fn estimate(&self, shape_class: &str, backend: &ComputeBackend) -> Option<RouteCost> {
        let key = (shape_class.to_string(), backend_label(backend));
        let cells = self.cells.lock().unwrap_or_else(|p| p.into_inner());
        let c = cells.get(&key)?;
        if c.samples == 0 {
            return None;
        }
        Some(RouteCost {
            samples: c.samples,
            range_gets: c.range_gets,
            bytes_read: c.bytes_read,
            egress_bytes: c.egress_bytes,
            bytes_written: c.bytes_written,
            compute_ms: c.compute_ms,
            score: self.score(c.quantities()),
        })
    }

    /// Among `candidates`, recommend the cheapest backend whose cell has reached
    /// the warmup threshold. Returns `None` when no candidate has enough history
    /// (the scheduler then keeps its static decision). Ties keep candidate order.
    pub fn recommend(
        &self,
        shape_class: &str,
        candidates: &[ComputeBackend],
    ) -> Option<RouteRecommendation> {
        let mut scored: Vec<(ComputeBackend, RouteCost)> = candidates
            .iter()
            .filter_map(|b| {
                self.estimate(shape_class, b)
                    .filter(|c| c.samples >= self.min_samples)
                    .map(|c| (b.clone(), c))
            })
            .collect();
        if scored.is_empty() {
            return None;
        }
        // Stable: sort by score, ties keep input order (sort_by is stable).
        scored.sort_by(|a, b| {
            a.1.score
                .partial_cmp(&b.1.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let ranked = scored
            .iter()
            .map(|(b, c)| (backend_label(b), c.score))
            .collect::<Vec<_>>();
        let (backend, cost) = scored.into_iter().next()?;
        Some(RouteRecommendation {
            backend,
            score: cost.score,
            samples: cost.samples,
            ranked,
        })
    }

    /// Live-override recommendation. Returns `Some(better)` ONLY when ALL hold:
    /// override is enabled; a candidate other than `static_backend` is the
    /// cheapest with warmed history; the static choice itself has warmed history
    /// (so we have evidence to beat, not a cold guess); and the challenger is
    /// cheaper by at least `min_advantage`. Otherwise `None` — the scheduler
    /// keeps its static route. The freshness-safe `candidates` set is chosen by
    /// the scheduler; this method only ranks cost, so it can never propose a
    /// backend the caller didn't deem correct for the query.
    pub fn recommend_override(
        &self,
        shape_class: &str,
        static_backend: &ComputeBackend,
        candidates: &[ComputeBackend],
    ) -> Option<RouteRecommendation> {
        if !self.override_active() {
            return None;
        }
        let rec = self.recommend(shape_class, candidates)?;
        if backend_label(&rec.backend) == backend_label(static_backend) {
            return None; // the model already agrees with the static choice
        }
        let static_cost = self.estimate(shape_class, static_backend)?;
        if static_cost.samples < self.min_samples {
            return None; // not enough evidence about the static route to beat it
        }
        if rec.score < static_cost.score * (1.0 - self.min_advantage) {
            Some(rec)
        } else {
            None // advantage too small — don't flap
        }
    }

    /// Phase-2 exploration pick (warm-up). When override is enabled and a
    /// freshness-safe candidate is still under-explored (`< min_samples`),
    /// occasionally route to the LEAST-sampled candidate so it accrues cost
    /// history — otherwise the static rule always picks one engine, the other
    /// never warms, and `recommend_override` could never fire. Properties:
    ///
    /// * **Bounded** — returns `None` once every candidate is warm (exploration
    ///   then yields to exploitation).
    /// * **Rate-limited** — fires ~1 in `exploration_interval` eligible
    ///   decisions, so the cost of probing a non-optimal engine stays small.
    /// * **Freshness-safe** — the caller passes the freshness-safe candidate set,
    ///   so exploration can never target an engine that would serve the query
    ///   incorrectly (OLTP gets a single-candidate set → always `None`).
    /// * **Flag-gated** — `None` unless override is enabled.
    pub fn exploration_choice(
        &self,
        shape_class: &str,
        candidates: &[ComputeBackend],
    ) -> Option<ComputeBackend> {
        if !self.override_active() || candidates.len() < 2 {
            return None;
        }
        let (cand, samples) = candidates
            .iter()
            .map(|c| {
                let s = self
                    .estimate(shape_class, c)
                    .map(|e| e.samples)
                    .unwrap_or(0);
                (c.clone(), s)
            })
            .min_by_key(|(_, s)| *s)?;
        if samples >= self.min_samples {
            return None; // all candidates warm — exploit, don't explore
        }
        let tick = self.explore_tick.fetch_add(1, Ordering::Relaxed);
        if tick.is_multiple_of(self.exploration_interval) {
            Some(cand)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(range_gets: u64, bytes_read: u64, compute_ms: u64) -> IoTraceSnapshot {
        let mut s = IoTraceSnapshot {
            range_gets,
            bytes_read,
            ..Default::default()
        };
        if compute_ms > 0 {
            s.compute_ms.insert("engine".to_string(), compute_ms);
        }
        s
    }

    #[test]
    fn shape_class_mirrors_scheduler_signals() {
        // Unknown finer signals (the default) keep the coarse 2-part class.
        assert_eq!(
            shape_class(&QueryShape {
                engages_relational: true,
                parquet_backed: true,
                ..Default::default()
            }),
            "olap/parquet"
        );
        assert_eq!(
            shape_class(&QueryShape {
                engages_relational: false,
                parquet_backed: false,
                ..Default::default()
            }),
            "oltp/native"
        );
    }

    #[test]
    fn shape_class_refines_with_known_cardinality_and_partition_signals() {
        use crate::query::compute_scheduler::{CardinalityClass, PartitionFanout};
        // Known finer signals append stable suffixes (cardinality then partition).
        let class = shape_class(&QueryShape {
            engages_relational: true,
            parquet_backed: true,
            cardinality: CardinalityClass::Large,
            partition_fanout: PartitionFanout::Many,
        });
        assert_eq!(class, "olap/parquet/card=l/part=m");
        // A partially-known shape only appends the known suffix.
        let partial = shape_class(&QueryShape {
            engages_relational: false,
            parquet_backed: false,
            cardinality: CardinalityClass::Small,
            ..Default::default()
        });
        assert_eq!(partial, "oltp/native/card=s");
    }

    #[test]
    fn no_history_yields_no_estimate_or_recommendation() {
        let m = RouteCostModel::new();
        assert!(
            m.estimate("olap/parquet", &ComputeBackend::Native)
                .is_none()
        );
        assert!(
            m.recommend(
                "olap/parquet",
                &[ComputeBackend::Native, ComputeBackend::DataFusionLocal]
            )
            .is_none()
        );
    }

    #[test]
    fn warmup_threshold_gates_recommendation() {
        let m = RouteCostModel::new().with_min_samples(3);
        // Two observations — below the 3-sample warmup → not yet trusted.
        m.observe(
            "olap/parquet",
            &ComputeBackend::Native,
            &snap(100, 1 << 20, 5),
        );
        m.observe(
            "olap/parquet",
            &ComputeBackend::Native,
            &snap(100, 1 << 20, 5),
        );
        assert!(
            m.recommend("olap/parquet", &[ComputeBackend::Native])
                .is_none()
        );
        m.observe(
            "olap/parquet",
            &ComputeBackend::Native,
            &snap(100, 1 << 20, 5),
        );
        assert!(
            m.recommend("olap/parquet", &[ComputeBackend::Native])
                .is_some()
        );
    }

    #[test]
    fn recommends_the_cheaper_backend_from_measured_traces() {
        let m = RouteCostModel::new().with_min_samples(1);
        // DataFusion route pays FEW big coalesced GETs; Native pays MANY small
        // GETs for the same shape — the trace says DataFusion is cheaper here.
        for _ in 0..5 {
            m.observe(
                "olap/parquet",
                &ComputeBackend::DataFusionLocal,
                &snap(4, 16 << 20, 50),
            );
            m.observe(
                "olap/parquet",
                &ComputeBackend::Native,
                &snap(400, 16 << 20, 20),
            );
        }
        let rec = m
            .recommend(
                "olap/parquet",
                &[ComputeBackend::Native, ComputeBackend::DataFusionLocal],
            )
            .expect("history exists");
        // GET-dominated score → DataFusion (4 GETs) beats Native (400 GETs).
        assert_eq!(rec.backend, ComputeBackend::DataFusionLocal);
        assert_eq!(rec.ranked.len(), 2);
        assert_eq!(rec.ranked[0].0, "DataFusionLocal");
        // Cheapest-first ordering.
        assert!(rec.ranked[0].1 < rec.ranked[1].1);
    }

    #[test]
    fn egress_term_is_inert_on_the_free_same_region_path() {
        // A read-only route with NO cross-region bytes must score exactly the read +
        // compute terms — i.e. adding the egress dimension changes nothing on the
        // free path (default-OFF behavior; KEU is zero until bytes move cross-region).
        let m = RouteCostModel::new().with_min_samples(1);
        m.observe(
            "olap/parquet",
            &ComputeBackend::Native,
            &snap(10, 4 << 20, 7),
        );
        let est = m
            .estimate("olap/parquet", &ComputeBackend::Native)
            .expect("history");
        let w = CostWeights::default();
        let expected = w.per_get * 10.0 + w.per_mib_read * 4.0 + w.per_compute_ms * 7.0;
        assert_eq!(est.egress_bytes, 0.0);
        assert!((est.score - expected).abs() < 1e-6);
    }

    #[test]
    fn cross_region_egress_raises_the_route_cost() {
        // Two routes identical in GETs/bytes-read/compute; one also moves bytes
        // cross-region. The egress (KEU) term must make that route cost more —
        // the whole point of metering Dimension 2 into Cost(q).
        let m = RouteCostModel::new().with_min_samples(1);
        // Identical in every term except egress, so the score delta isolates it.
        let same_region = snap(4, 8 << 20, 0);
        let cross_region = IoTraceSnapshot {
            range_gets: 4,
            bytes_read: 8 << 20,
            egress_bytes: 8 << 20,
            ..Default::default()
        };
        m.observe(
            "olap/parquet",
            &ComputeBackend::DataFusionLocal,
            &same_region,
        );
        m.observe("olap/parquet", &ComputeBackend::Native, &cross_region);
        let cheap = m
            .estimate("olap/parquet", &ComputeBackend::DataFusionLocal)
            .expect("history");
        let dear = m
            .estimate("olap/parquet", &ComputeBackend::Native)
            .expect("history");
        assert!(dear.egress_bytes > 0.0 && cheap.egress_bytes == 0.0);
        // The cross-region route is dearer by exactly the egress weight × 8 MiB.
        let delta = dear.score - cheap.score;
        assert!((delta - CostWeights::default().per_mib_egress * 8.0).abs() < 1e-6);
        // And the trace-driven recommendation prefers the same-region route.
        let rec = m
            .recommend(
                "olap/parquet",
                &[ComputeBackend::Native, ComputeBackend::DataFusionLocal],
            )
            .expect("history");
        assert_eq!(rec.backend, ComputeBackend::DataFusionLocal);
    }

    #[test]
    fn ingest_write_term_costs_storage_writes() {
        // The KIU storage-write term folds bytes_written into Cost(q) (inert for
        // read-only routes, active once a route induces writes).
        let m = RouteCostModel::new().with_min_samples(1);
        let written = IoTraceSnapshot {
            range_gets: 0,
            bytes_written: 2 << 20,
            ..Default::default()
        };
        m.observe("oltp/native", &ComputeBackend::Native, &written);
        let est = m
            .estimate("oltp/native", &ComputeBackend::Native)
            .expect("history");
        assert!((est.bytes_written - (2 << 20) as f64).abs() < 1.0);
        assert!((est.score - CostWeights::default().per_mib_written * 2.0).abs() < 1e-6);
    }

    #[test]
    fn estimate_reflects_ewma_of_observations() {
        let m = RouteCostModel::new();
        for _ in 0..10 {
            m.observe("oltp/native", &ComputeBackend::Native, &snap(2, 8192, 1));
        }
        let est = m
            .estimate("oltp/native", &ComputeBackend::Native)
            .expect("history");
        assert_eq!(est.samples, 10);
        // EWMA of a constant series converges to that constant.
        assert!((est.range_gets - 2.0).abs() < 1e-6);
        assert!(est.score > 0.0);
    }

    #[test]
    fn observe_by_label_matches_typed_observe() {
        // The label-keyed ingestion form (used by the io_trace observer) lands in
        // the same cell as the typed observe.
        let m = RouteCostModel::new();
        m.observe_by_label("olap/parquet", "DataFusionLocal", &snap(4, 16 << 20, 50));
        let by_label = m
            .estimate("olap/parquet", &ComputeBackend::DataFusionLocal)
            .expect("label-keyed observation is visible to typed estimate");
        assert_eq!(by_label.samples, 1);
        assert!((by_label.range_gets - 4.0).abs() < 1e-6);
    }

    #[test]
    fn global_model_is_usable() {
        // Feed the process-global model directly under a unique key (avoids the
        // observer global, so no cross-test race) and read it back.
        let key = "test/global-usable-unique";
        for _ in 0..3 {
            GLOBAL_ROUTE_COST_MODEL.observe_by_label(key, "Native(Volcano)", &snap(2, 4096, 1));
        }
        assert!(
            GLOBAL_ROUTE_COST_MODEL
                .estimate(key, &ComputeBackend::Native)
                .is_some()
        );
    }

    /// Warm a model so DataFusion is much cheaper than Native for "olap/parquet".
    fn warm_df_cheaper(m: &RouteCostModel) {
        for _ in 0..5 {
            m.observe_by_label("olap/parquet", "DataFusionLocal", &snap(4, 16 << 20, 50));
            m.observe_by_label("olap/parquet", "Native(Volcano)", &snap(400, 16 << 20, 20));
        }
    }

    #[test]
    fn override_respects_the_enable_flag() {
        let m = RouteCostModel::new().with_min_samples(1);
        warm_df_cheaper(&m);
        let cands = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        // Default OFF → no override even with a huge advantage.
        assert!(
            m.recommend_override("olap/parquet", &ComputeBackend::Native, &cands)
                .is_none()
        );
        m.set_override_enabled(true);
        let rec = m
            .recommend_override("olap/parquet", &ComputeBackend::Native, &cands)
            .expect("confident, enabled → override");
        assert_eq!(rec.backend, ComputeBackend::DataFusionLocal);
    }

    #[test]
    fn override_requires_min_advantage() {
        // DataFusion only ~5% cheaper than Native → below the 15% gate → no flip.
        let m = RouteCostModel::new()
            .with_min_samples(1)
            .with_min_advantage(0.15);
        m.set_override_enabled(true);
        for _ in 0..5 {
            m.observe_by_label("olap/parquet", "DataFusionLocal", &snap(95, 1 << 20, 10));
            m.observe_by_label("olap/parquet", "Native(Volcano)", &snap(100, 1 << 20, 10));
        }
        let cands = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        assert!(
            m.recommend_override("olap/parquet", &ComputeBackend::Native, &cands)
                .is_none(),
            "marginal advantage must not flip the route"
        );
    }

    #[test]
    fn override_none_when_static_is_already_cheapest() {
        let m = RouteCostModel::new().with_min_samples(1);
        m.set_override_enabled(true);
        warm_df_cheaper(&m); // DataFusion cheapest
        let cands = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        // Static already DataFusion → nothing cheaper to flip to.
        assert!(
            m.recommend_override("olap/parquet", &ComputeBackend::DataFusionLocal, &cands)
                .is_none()
        );
    }

    #[test]
    fn override_needs_warmed_history_for_the_static_route() {
        // Challenger has history but the static route does not → can't prove the
        // override beats it, so no flip.
        let m = RouteCostModel::new().with_min_samples(3);
        m.set_override_enabled(true);
        for _ in 0..5 {
            m.observe_by_label("olap/parquet", "DataFusionLocal", &snap(4, 1 << 20, 5));
        }
        let cands = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        assert!(
            m.recommend_override("olap/parquet", &ComputeBackend::Native, &cands)
                .is_none()
        );
    }

    #[test]
    fn exploration_is_off_unless_override_enabled() {
        let m = RouteCostModel::new().with_exploration_interval(1);
        // Native warm, DataFusion unexplored — but override flag is OFF.
        for _ in 0..5 {
            m.observe_by_label("olap/parquet", "Native(Volcano)", &snap(2, 4096, 1));
        }
        let cands = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        assert!(m.exploration_choice("olap/parquet", &cands).is_none());
    }

    #[test]
    fn exploration_targets_the_least_sampled_candidate() {
        let m = RouteCostModel::new().with_exploration_interval(1);
        m.set_override_enabled(true);
        // Native warm; DataFusion has no history → it is the under-explored one.
        for _ in 0..5 {
            m.observe_by_label("olap/parquet", "Native(Volcano)", &snap(2, 4096, 1));
        }
        let cands = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        assert_eq!(
            m.exploration_choice("olap/parquet", &cands),
            Some(ComputeBackend::DataFusionLocal)
        );
    }

    #[test]
    fn exploration_stops_once_every_candidate_is_warm() {
        let m = RouteCostModel::new().with_exploration_interval(1);
        m.set_override_enabled(true);
        for _ in 0..5 {
            m.observe_by_label("olap/parquet", "Native(Volcano)", &snap(2, 4096, 1));
            m.observe_by_label("olap/parquet", "DataFusionLocal", &snap(4, 8192, 2));
        }
        let cands = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        assert!(m.exploration_choice("olap/parquet", &cands).is_none());
    }

    #[test]
    fn exploration_is_rate_limited_by_interval() {
        let m = RouteCostModel::new().with_exploration_interval(3);
        m.set_override_enabled(true);
        for _ in 0..5 {
            m.observe_by_label("olap/parquet", "Native(Volcano)", &snap(2, 4096, 1));
        }
        let cands = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        // ticks 0,3 explore; 1,2,4,5 do not (1-in-3).
        let fired: Vec<bool> = (0..6)
            .map(|_| m.exploration_choice("olap/parquet", &cands).is_some())
            .collect();
        assert_eq!(fired, vec![true, false, false, true, false, false]);
    }

    #[test]
    fn tier_multiplier_defaults_to_inert_one() {
        // No resolver installed → multiplier 1.0, and None tenant is always 1.0.
        assert_eq!(tier_entitlement_multiplier(Some("anyone")), 1.0);
        assert_eq!(tier_entitlement_multiplier(None), 1.0);
        // final_cost is then the unscaled base score.
        assert_eq!(final_cost(42.0, Some("anyone")), 42.0);
    }

    #[test]
    fn tier_multiplier_resolver_scales_reported_cost_but_is_routing_neutral() {
        // Use a unique resolver, then clear it, to avoid racing sibling tests on
        // the process-global (mirrors the io_trace observer test discipline).
        set_tier_multiplier_resolver(Some(Box::new(|tenant: &str| match tenant {
            "premium" => 0.5,   // entitled to a discounted reported cost
            "throttled" => 4.0, // surcharged
            "bad" => -1.0,      // invalid → must be rejected fail-safe to 1.0
            _ => 1.0,
        })));

        assert_eq!(tier_entitlement_multiplier(Some("premium")), 0.5);
        assert_eq!(tier_entitlement_multiplier(Some("throttled")), 4.0);
        // Non-positive multiplier is rejected (could invert cost ordering).
        assert_eq!(tier_entitlement_multiplier(Some("bad")), 1.0);
        // Unknown tenant falls through the resolver's own default.
        assert_eq!(tier_entitlement_multiplier(Some("someone-else")), 1.0);

        // final_cost applies the multiplier to the reported cost...
        assert_eq!(final_cost(100.0, Some("premium")), 50.0);
        assert_eq!(final_cost(100.0, Some("throttled")), 400.0);

        // ...but it is routing-neutral: scaling two candidates by the SAME
        // per-tenant multiplier preserves which one is cheaper.
        let (a, b) = (120.0_f64, 200.0_f64);
        for tenant in ["premium", "throttled", "someone-else"] {
            let fa = final_cost(a, Some(tenant));
            let fb = final_cost(b, Some(tenant));
            assert_eq!(a < b, fa < fb, "ordering preserved under tier {tenant}");
        }

        set_tier_multiplier_resolver(None); // reset global for other tests
        assert_eq!(tier_entitlement_multiplier(Some("premium")), 1.0);
    }

    #[test]
    fn exploration_never_fires_with_a_single_candidate() {
        // OLTP gets a single freshness-safe candidate → nothing to explore.
        let m = RouteCostModel::new().with_exploration_interval(1);
        m.set_override_enabled(true);
        assert!(
            m.exploration_choice("oltp/native", &[ComputeBackend::Native])
                .is_none()
        );
    }

    // ── C4 Phase-2b: route-time shape-stat cache (T2.1 wiring / T3.3 ratchet) ─

    #[test]
    fn classify_table_shapes_empty_and_missing_is_unknown() {
        use crate::query::compute_scheduler::{CardinalityClass, PartitionFanout};
        // Empty slice → Unknown.
        let (pf, card) = classify_table_shapes(&[]);
        assert_eq!(
            (pf, card),
            (PartitionFanout::Unknown, CardinalityClass::Unknown)
        );
        // A location with no warmed stat → Unknown (conservative: never
        // half-classify — an unknown table could dominate fan-out/cardinality).
        let (pf, card) = classify_table_shapes(&["test://route-shape/missing".to_string()]);
        assert_eq!(
            (pf, card),
            (PartitionFanout::Unknown, CardinalityClass::Unknown)
        );
    }

    #[test]
    fn classify_table_shapes_buckets_and_sums_warmed_stats() {
        use crate::query::compute_scheduler::{CardinalityClass, PartitionFanout};
        // Small / single-row-group table.
        record_table_shape_stat("test://route-shape/small", 1, Some(500));
        let (pf, card) = classify_table_shapes(&["test://route-shape/small".to_string()]);
        assert_eq!(pf, PartitionFanout::Single);
        assert_eq!(card, CardinalityClass::Small);

        // Large / many-row-group table.
        record_table_shape_stat("test://route-shape/large", 5_000, Some(50_000_000));
        let (pf, card) = classify_table_shapes(&["test://route-shape/large".to_string()]);
        assert_eq!(pf, PartitionFanout::Many);
        assert_eq!(card, CardinalityClass::Large);

        // Two warmed tables: row-groups and rows SUM (total GET fan-out / scan
        // cardinality). 12 row-groups → Many; 1.3M rows → Large.
        record_table_shape_stat("test://route-shape/sum-a", 6, Some(600_000));
        record_table_shape_stat("test://route-shape/sum-b", 6, Some(700_000));
        let (pf, card) = classify_table_shapes(&[
            "test://route-shape/sum-a".to_string(),
            "test://route-shape/sum-b".to_string(),
        ]);
        assert_eq!(pf, PartitionFanout::Many);
        assert_eq!(card, CardinalityClass::Large);

        // Conservative: a cold location among warmed ones → Unknown.
        let (pf, card) = classify_table_shapes(&[
            "test://route-shape/sum-a".to_string(),
            "test://route-shape/cold".to_string(),
        ]);
        assert_eq!(
            (pf, card),
            (PartitionFanout::Unknown, CardinalityClass::Unknown)
        );
    }

    #[test]
    fn shape_class_discriminates_fine_parquet_signals() {
        use crate::query::compute_scheduler::{CardinalityClass, PartitionFanout, QueryShape};
        // Coarse (no fine signals) — the backward-compatible 2-part key.
        let coarse = shape_class(&QueryShape {
            engages_relational: true,
            parquet_backed: true,
            ..Default::default()
        });
        assert_eq!(coarse, "olap/parquet");
        // Refined — distinct keys per fine signal, so the model accrues
        // per-(cardinality, fan-out) cells instead of one noisy bucket.
        let big = shape_class(&QueryShape {
            engages_relational: true,
            parquet_backed: true,
            cardinality: CardinalityClass::Large,
            partition_fanout: PartitionFanout::Many,
        });
        let small = shape_class(&QueryShape {
            engages_relational: true,
            parquet_backed: true,
            cardinality: CardinalityClass::Small,
            partition_fanout: PartitionFanout::Single,
        });
        assert_ne!(big, coarse);
        assert_ne!(small, coarse);
        assert_ne!(big, small);
    }

    #[test]
    fn warmed_fine_class_lets_cost_override_flip_route() {
        // T2.1 keystone (T3.3 ratchet): with real fine signals the model accrues
        // trustworthy per-(cardinality, fan-out) cost; with override enabled it
        // flips a freshness-safe OLAP-Parquet route to the confidently-cheaper
        // backend. This is the co-design loop actually closing — observe-mode
        // promoted to act-mode on reliable per-class evidence.
        use crate::query::compute_scheduler::{
            CardinalityClass, ComputeScheduler, PartitionFanout, QueryShape,
        };
        let m = RouteCostModel::new()
            .with_min_samples(1)
            .with_min_advantage(0.1);
        m.set_override_enabled(true);
        let big = QueryShape {
            engages_relational: true,
            parquet_backed: true,
            cardinality: CardinalityClass::Large,
            partition_fanout: PartitionFanout::Many,
        };
        let class = shape_class(&big);
        // Observe Native as confidently cheaper than DataFusion for THIS fine
        // class (far fewer object-store round-trips). Both backends get ≥
        // min_samples → exploration is satisfied (warm), so the exploit override
        // runs deterministically rather than probing.
        for _ in 0..3 {
            m.observe(&class, &ComputeBackend::Native, &snap(2, 8192, 1));
            m.observe(
                &class,
                &ComputeBackend::DataFusionLocal,
                &snap(40, 1 << 20, 8),
            );
        }
        let decision = ComputeScheduler::new().route_select_advised(big, Some(&m));
        // Static rule picks DataFusion for OLAP-Parquet; the warmed model
        // overrides to freshness-safe, cheaper Native.
        assert_eq!(decision.backend, ComputeBackend::Native);
        assert!(
            decision.reason.contains("OVERRIDE"),
            "expected override reason, got: {}",
            decision.reason
        );
    }
}
