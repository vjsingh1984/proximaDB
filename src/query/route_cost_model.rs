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
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Stable shape-class key the model aggregates under. Coarse on purpose at C4 —
/// it mirrors today's scheduler signals (`engages_relational` × `parquet_backed`);
/// finer classes (cardinality, partition count, point-lookup) arrive with the
/// §5.2 Phase-1 shape inputs and slot in here without changing the model.
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
    format!("{workload}/{base}")
}

/// Neutral relative weights for the route cost score. NOT dollars (see module
/// docs). They encode only the co-design cost ordering: a GET's fixed
/// first-byte latency dominates a MiB of bandwidth, which dominates a ms of CPU.
#[derive(Debug, Clone, Copy)]
pub struct CostWeights {
    /// Per ranged GET — the fixed-latency, per-request term (the dominant cost
    /// for object-store-served reads).
    pub per_get: f64,
    /// Per MiB read — the bandwidth term.
    pub per_mib_read: f64,
    /// Per compute-millisecond — the CPU term (smallest for I/O-bound DB work).
    pub per_compute_ms: f64,
}

impl Default for CostWeights {
    fn default() -> Self {
        // Illustrative neutral ordering (round-trip ≫ bandwidth ≫ CPU), tunable.
        Self {
            per_get: 20.0,
            per_mib_read: 5.0,
            per_compute_ms: 1.0,
        }
    }
}

/// One (shape-class, backend) cell: EWMA means of the cost-bearing quantities.
#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    range_gets: f64,
    bytes_read: f64,
    compute_ms: f64,
    samples: u64,
}

impl Cell {
    fn fold(&mut self, alpha: f64, range_gets: f64, bytes_read: f64, compute_ms: f64) {
        if self.samples == 0 {
            self.range_gets = range_gets;
            self.bytes_read = bytes_read;
            self.compute_ms = compute_ms;
        } else {
            self.range_gets = alpha * range_gets + (1.0 - alpha) * self.range_gets;
            self.bytes_read = alpha * bytes_read + (1.0 - alpha) * self.bytes_read;
            self.compute_ms = alpha * compute_ms + (1.0 - alpha) * self.compute_ms;
        }
        self.samples += 1;
    }
}

/// Learned cost estimate for serving a shape-class on one backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteCost {
    pub samples: u64,
    pub range_gets: f64,
    pub bytes_read: f64,
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
        }
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

    fn score(&self, range_gets: f64, bytes_read: f64, compute_ms: f64) -> f64 {
        self.weights.per_get * range_gets
            + self.weights.per_mib_read * (bytes_read / (1024.0 * 1024.0))
            + self.weights.per_compute_ms * compute_ms
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
        cells.entry(key).or_default().fold(
            self.alpha,
            snap.range_gets as f64,
            snap.bytes_read as f64,
            snap.total_compute_ms() as f64,
        );
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
            compute_ms: c.compute_ms,
            score: self.score(c.range_gets, c.bytes_read, c.compute_ms),
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
        assert_eq!(
            shape_class(&QueryShape {
                engages_relational: true,
                parquet_backed: true
            }),
            "olap/parquet"
        );
        assert_eq!(
            shape_class(&QueryShape {
                engages_relational: false,
                parquet_backed: false
            }),
            "oltp/native"
        );
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
}
