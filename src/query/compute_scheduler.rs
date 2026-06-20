// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Read-side compute routing — the `ComputeScheduler` (P0).
//!
//! Implements the planner-boundary half of the multi-engine routing contract in
//! `docs/12-design/DATA_WAREHOUSE_AND_ENGINEERING_COURSE_CORRECTION_2026_06_04.adoc`
//! §5 (Intelligent Multi-Engine Routing, Policy→RL). It selects ONE physical
//! execution engine per query plan and materializes the choice as a
//! [`SelectRouteDecision`] for telemetry and (once a SELECT EXPLAIN surface
//! exists) `EXPLAIN`.
//!
//! ## Convergence (Convergence Gate)
//! This is the READ/query-execution analog of the table-WRITE router's
//! [`crate::query::table_write_plan::RoutedExecutionPlan`] /
//! `RouteDecisionMetadata`. It deliberately REUSES the canonical engine enum
//! [`ComputeBackend`] and [`CatalogWorkloadProfile`] instead of introducing a
//! parallel vocabulary — there is one engine taxonomy across read and write.
//! It does not own durable authority.
//!
//! * `authority_mode`: control-plane routing decision; no durable authority.
//! * `policy_boundary`: exactly one engine chosen per query plan at the planner
//!   boundary (pgwire today), never per row.
//! * `freshness`: P0 always selects [`ComputeBackend::Native`] — the live
//!   Volcano executor over WAL+`RecordStorage`, i.e. strong freshness. The
//!   DataFusion/Polars destinations are declared but not yet wired (course
//!   correction §4 audit); P1+ flip the OLAP arm once a live DataFusion read
//!   path exists.
//!
//! P0 is purely additive: the chosen backend is ALWAYS `Native`, so nothing
//! about execution changes — the scheduler only makes the decision observable so
//! later phases have a single, contract-bound place to evolve.

use crate::query::read_route::{
    CandidateReadRoute, ReadFreshnessSla, ReadPolicyBoundary, ReadSplitSummary, RoutedReadPlan,
};
use crate::query::table_write_plan::ComputeBackend;
use proximadb_catalog::CatalogAuthorityMode;
use proximadb_catalog::CatalogWorkloadProfile;

/// Shape signals the scheduler routes on.
///
/// P0 uses only `engages_relational` — the existing join / `GROUP BY` /
/// aggregate / set-op gate, which is the OLAP-shape signal. P1 (§5.2 policy
/// inputs) adds cardinality, partition count, and point-lookup flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryShape {
    /// The query lowers against the relational algebra engine for a reason the
    /// single-table legacy path cannot serve (joins, `GROUP BY`, aggregates,
    /// `HAVING`, set-ops). Today this is the OLAP-candidate signal.
    pub engages_relational: bool,
    /// P1: every referenced table is Parquet-backed (object-store / open-format),
    /// so the OLAP arm can route to DataFusion. Set by the planner boundary
    /// (`try_run_select`) only when the `datafusion-integration` feature is
    /// compiled in, so the route is never advertised when the build can't honor it.
    pub parquet_backed: bool,
}

/// A materialized read-route decision: the engine, the workload it was
/// classified as, and a human-readable reason. Emitted to telemetry and, once a
/// SELECT `EXPLAIN` surface exists, surfaced there via [`Self::explain_line`].
#[derive(Debug, Clone)]
pub struct SelectRouteDecision {
    /// Selected physical engine (canonical [`ComputeBackend`]).
    pub backend: ComputeBackend,
    /// Workload classification this route was based on.
    pub workload_profile: CatalogWorkloadProfile,
    /// Human-readable reason for the choice.
    pub reason: String,
}

impl SelectRouteDecision {
    /// Stable one-line `EXPLAIN`/telemetry form, e.g.
    /// `Compute Route: Native(Volcano) (workload=Olap, reason="...")`.
    pub fn explain_line(&self) -> String {
        format!(
            "Compute Route: {} (workload={:?}, reason=\"{}\")",
            backend_label(&self.backend),
            self.workload_profile,
            self.reason
        )
    }

    /// Stable short backend label (e.g. `Native(Volcano)`, `DataFusionLocal`) for the
    /// structured `EXPLAIN` JSON surface. Single source for the engine name.
    pub fn compute_route_label(&self) -> String {
        backend_label(&self.backend)
    }

    /// Materialize this scheduler decision into the typed read-route contract.
    ///
    /// This is intentionally conservative: until split planning is wired, routes
    /// use a whole-collection split summary and leave execution behavior unchanged.
    pub fn routed_read_plan(&self) -> RoutedReadPlan {
        let mut plan = RoutedReadPlan::native_whole_collection(self.workload_profile);
        plan.backend = self.backend.clone();
        plan.authority_mode = authority_mode_for_backend(&self.backend);
        plan.policy_boundary = policy_boundary_for_backend(&self.backend);
        plan.freshness_sla = freshness_for_backend(&self.backend);
        plan.candidate_routes = vec![CandidateReadRoute {
            backend: self.backend.clone(),
            access_method: access_method_for_backend(&self.backend).to_string(),
            reason: self.reason.clone(),
        }];
        plan
    }

    /// Like [`Self::routed_read_plan`] but carries a concrete split inventory
    /// (e.g. the Parquet row groups discovered for a `DataFusionLocal` route)
    /// instead of the conservative whole-collection placeholder, so EXPLAIN
    /// discloses the real partition count the executor fans out over.
    pub fn routed_read_plan_with_splits(&self, split_summary: ReadSplitSummary) -> RoutedReadPlan {
        self.routed_read_plan().with_split_summary(split_summary)
    }
}

/// Short, stable label for a backend in EXPLAIN/telemetry output. Also the
/// canonical key the trace-driven [`crate::query::route_cost_model`] aggregates
/// observations under, so labels and cost-model keys never diverge.
pub(crate) fn backend_label(backend: &ComputeBackend) -> String {
    match backend {
        ComputeBackend::Native => "Native(Volcano)".to_string(),
        ComputeBackend::DataFusionLocal => "DataFusionLocal".to_string(),
        ComputeBackend::DataFusionDistributed => "DataFusionDistributed".to_string(),
        ComputeBackend::PolarsLocal => "PolarsLocal".to_string(),
        ComputeBackend::DuckDbCompat => "DuckDbCompat".to_string(),
        ComputeBackend::ExternalDelegated(name) => format!("ExternalDelegated({name})"),
    }
}

fn authority_mode_for_backend(backend: &ComputeBackend) -> CatalogAuthorityMode {
    match backend {
        ComputeBackend::Native => CatalogAuthorityMode::ProximaAuthoritative,
        ComputeBackend::DataFusionLocal | ComputeBackend::DataFusionDistributed => {
            CatalogAuthorityMode::ProjectionPublication
        }
        ComputeBackend::ExternalDelegated(_) => CatalogAuthorityMode::FederatedRead,
        ComputeBackend::PolarsLocal | ComputeBackend::DuckDbCompat => {
            CatalogAuthorityMode::ProximaAuthoritative
        }
    }
}

fn policy_boundary_for_backend(backend: &ComputeBackend) -> ReadPolicyBoundary {
    match backend {
        ComputeBackend::Native => ReadPolicyBoundary::EngineEnforced,
        ComputeBackend::DataFusionLocal | ComputeBackend::DataFusionDistributed => {
            ReadPolicyBoundary::ConnectorEnforced
        }
        ComputeBackend::ExternalDelegated(_) => ReadPolicyBoundary::ExternalPolicy,
        ComputeBackend::PolarsLocal | ComputeBackend::DuckDbCompat => {
            ReadPolicyBoundary::EngineEnforced
        }
    }
}

fn freshness_for_backend(backend: &ComputeBackend) -> ReadFreshnessSla {
    match backend {
        ComputeBackend::Native => ReadFreshnessSla::Synchronous,
        ComputeBackend::DataFusionLocal | ComputeBackend::DataFusionDistributed => {
            ReadFreshnessSla::CatalogValue("base-snapshot".to_string())
        }
        ComputeBackend::ExternalDelegated(_) => {
            ReadFreshnessSla::CatalogValue("external-snapshot".to_string())
        }
        ComputeBackend::PolarsLocal | ComputeBackend::DuckDbCompat => ReadFreshnessSla::Synchronous,
    }
}

fn access_method_for_backend(backend: &ComputeBackend) -> &'static str {
    match backend {
        ComputeBackend::Native => "canonical-record-scan",
        ComputeBackend::DataFusionLocal => "datafusion-local-scan",
        ComputeBackend::DataFusionDistributed => "ballista-distributed-scan",
        ComputeBackend::PolarsLocal => "polars-local-scan",
        ComputeBackend::DuckDbCompat => "duckdb-compat-scan",
        ComputeBackend::ExternalDelegated(_) => "external-delegated-scan",
    }
}

/// Policy/heuristic read-route scheduler (course correction §5.2 Phase 1).
///
/// P0 ALWAYS returns [`ComputeBackend::Native`] (the live Volcano path) — purely
/// additive, no behavior change — but classifies the workload so the decision is
/// observable and P1 can flip the OLAP arm to `DataFusionLocal` in exactly one
/// place. The rule engine grows (cardinality/partition/point-lookup, then the
/// §5.2 Phase-2 `RLPlanner` learner) without moving this seam.
#[derive(Debug, Default, Clone, Copy)]
pub struct ComputeScheduler;

impl ComputeScheduler {
    /// Construct the (stateless, P0) scheduler.
    pub fn new() -> Self {
        Self
    }

    /// Route a relational `SELECT` plan to a physical engine.
    ///
    /// P0 invariant: the backend is ALWAYS `Native`. Only the workload
    /// classification and reason vary, so the contract is locked before any
    /// second physical executor exists.
    pub fn route_select(&self, shape: QueryShape) -> SelectRouteDecision {
        let decision = match (shape.engages_relational, shape.parquet_backed) {
            // P1: OLAP shape over Parquet-backed (object-store) table(s) → DataFusion.
            (true, true) => SelectRouteDecision {
                backend: ComputeBackend::DataFusionLocal,
                workload_profile: CatalogWorkloadProfile::Olap,
                reason: "OLAP shape over Parquet-backed table(s) — DataFusion over object storage"
                    .to_string(),
            },
            // OLAP shape on native storage — Volcano serves it from WAL+RecordStorage
            // until the relational base tier is Parquet/Iceberg (course-correction §6 P3).
            (true, false) => SelectRouteDecision {
                backend: ComputeBackend::Native,
                workload_profile: CatalogWorkloadProfile::Olap,
                reason: "OLAP shape (join/group-by/aggregate/set-op) on native storage — Volcano"
                    .to_string(),
            },
            (false, _) => SelectRouteDecision {
                backend: ComputeBackend::Native,
                workload_profile: CatalogWorkloadProfile::Oltp,
                reason: "OLTP shape (point/simple select) — Volcano".to_string(),
            },
        };
        // C4 ingestion: stamp the chosen route onto the active io_trace scope (a
        // no-op when unscoped, e.g. unit tests / EXPLAIN-only). The completed
        // query's measured snapshot then feeds the trace-driven cost model at
        // flush; empty traces are skipped, so EXPLAIN-only calls cost nothing.
        crate::observability::io_trace::record_route(
            &crate::query::route_cost_model::shape_class(&shape),
            &backend_label(&decision.backend),
        );
        decision
    }

    /// Route a relational `SELECT` and materialize the typed read-route plan.
    pub fn route_select_plan(&self, shape: QueryShape) -> RoutedReadPlan {
        self.route_select(shape).routed_read_plan()
    }

    /// Route a `SELECT`, then — if a trace-driven cost model is supplied —
    /// **observe-mode** advise: consult the measured per-(shape-class, backend)
    /// cost (co-design C4) and fold its recommendation into the decision's
    /// `reason` for EXPLAIN/telemetry, *without changing the backend*.
    ///
    /// This closes the co-design loop (C0 trace → cost model → router). By
    /// default it is **observe-mode**: the scheduler discloses what the trace
    /// *would* recommend and whether it diverges from the static rule, without
    /// changing the backend. When the model's live override is enabled (slice 4,
    /// flag-gated via `PROXIMADB_ROUTE_COST_OVERRIDE`) and a freshness-safe
    /// challenger is confidently cheaper, the route is *flipped* to it. With no
    /// warmed history the static decision is returned unchanged.
    pub fn route_select_advised(
        &self,
        shape: QueryShape,
        model: Option<&crate::query::route_cost_model::RouteCostModel>,
    ) -> SelectRouteDecision {
        let mut decision = self.route_select(shape);
        let Some(model) = model else {
            return decision;
        };
        let class = crate::query::route_cost_model::shape_class(&shape);

        // Live override (flag-gated, confidence-gated). Only consider backends
        // that are freshness-SAFE for this shape (see `override_candidates`), so
        // the cost model can never flip a query onto an engine that would serve
        // it incorrectly (e.g. a point/OLTP query onto a stale base snapshot).
        if model.override_active() {
            let safe = override_candidates(shape, &decision.backend);
            // Exploration (Phase 2) first: warm an under-explored freshness-safe
            // candidate so it accrues history the override can later act on.
            // Bounded + rate-limited + freshness-safe (see `exploration_choice`).
            if let Some(explore) = model.exploration_choice(&class, &safe)
                && backend_label(&explore) != backend_label(&decision.backend)
            {
                let prev = backend_label(&decision.backend);
                decision.reason = format!(
                    "{} | cost-model EXPLORE {prev}→{} (gathering cost history)",
                    decision.reason,
                    backend_label(&explore)
                );
                decision.backend = explore;
                return decision;
            }
            // Exploitation: override to the confidently-cheaper backend.
            if let Some(rec) = model.recommend_override(&class, &decision.backend, &safe) {
                let prev = backend_label(&decision.backend);
                decision.reason = format!(
                    "{} | cost-model OVERRIDE {prev}→{} ({})",
                    decision.reason,
                    backend_label(&rec.backend),
                    rec.reason()
                );
                decision.backend = rec.backend;
                return decision;
            }
        }

        // Observe-mode advisory (no behavior change): disclose the recommendation.
        let candidates = [ComputeBackend::Native, ComputeBackend::DataFusionLocal];
        if let Some(rec) = model.recommend(&class, &candidates) {
            let advisory = if rec.backend == decision.backend {
                format!("cost-model concurs ({})", rec.reason())
            } else {
                format!(
                    "cost-model would prefer {} ({}) — observe-mode, route unchanged",
                    backend_label(&rec.backend),
                    rec.reason()
                )
            };
            decision.reason = format!("{} | {advisory}", decision.reason);
        }
        decision
    }
}

/// The freshness-SAFE backend set a live override may choose among for `shape`.
///
/// Only OLAP-over-Parquet has more than one freshness-compatible engine: both
/// Native's strong-freshness scan (WAL+RecordStorage) and DataFusion's
/// base-snapshot scan correctly answer an analytic query, and both are already
/// used by the static rule for this shape — so flipping between them is safe.
/// OLTP (point/simple) is freshness-critical → never override off Native; OLAP
/// on native storage has no Parquet base → DataFusion cannot serve it. Those
/// keep only the static backend, so `recommend_override` can never flip them.
fn override_candidates(shape: QueryShape, static_backend: &ComputeBackend) -> Vec<ComputeBackend> {
    if shape.engages_relational && shape.parquet_backed {
        vec![ComputeBackend::Native, ComputeBackend::DataFusionLocal]
    } else {
        vec![static_backend.clone()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn olap_shape_on_native_storage_stays_on_volcano() {
        let decision = ComputeScheduler::new().route_select(QueryShape {
            engages_relational: true,
            parquet_backed: false,
        });
        // OLAP over native storage stays on Volcano (no Parquet base tier yet).
        assert_eq!(decision.backend, ComputeBackend::Native);
        assert_eq!(decision.workload_profile, CatalogWorkloadProfile::Olap);
        assert!(decision.reason.to_lowercase().contains("olap"));
    }

    #[test]
    fn oltp_shape_classifies_oltp_on_native() {
        let decision = ComputeScheduler::new().route_select(QueryShape {
            engages_relational: false,
            parquet_backed: false,
        });
        assert_eq!(decision.backend, ComputeBackend::Native);
        assert_eq!(decision.workload_profile, CatalogWorkloadProfile::Oltp);
    }

    #[test]
    fn p1_olap_over_parquet_routes_to_datafusion() {
        let d = ComputeScheduler::new().route_select(QueryShape {
            engages_relational: true,
            parquet_backed: true,
        });
        assert_eq!(d.backend, ComputeBackend::DataFusionLocal);
        assert_eq!(d.workload_profile, CatalogWorkloadProfile::Olap);
        assert_eq!(d.compute_route_label(), "DataFusionLocal");
    }

    #[test]
    fn oltp_never_routes_off_native_even_if_parquet() {
        // Point/simple selects stay on Volcano (strong freshness) regardless of format.
        let d = ComputeScheduler::new().route_select(QueryShape {
            engages_relational: false,
            parquet_backed: true,
        });
        assert_eq!(d.backend, ComputeBackend::Native);
    }

    #[test]
    fn datafusion_route_carries_concrete_row_group_split_inventory() {
        let plan = ComputeScheduler::new()
            .route_select(QueryShape {
                engages_relational: true,
                parquet_backed: true,
            })
            .routed_read_plan_with_splits(ReadSplitSummary::row_groups(
                8,
                Some(50_000),
                Some(1 << 20),
            ));

        assert_eq!(plan.backend, ComputeBackend::DataFusionLocal);
        let explain = plan.route_explanation();
        assert_eq!(explain.selected_backend, "DataFusionLocal");
        // Real row-group inventory, not the whole-collection `1`-partition default.
        assert_eq!(explain.split_strategy, "row_group");
        assert_eq!(explain.partition_count, 8);
        assert_eq!(explain.estimated_rows, Some(50_000));
    }

    #[test]
    fn explain_line_is_stable_and_readable() {
        let line = ComputeScheduler::new()
            .route_select(QueryShape {
                engages_relational: true,
                parquet_backed: false,
            })
            .explain_line();
        assert!(line.starts_with("Compute Route: Native(Volcano) (workload=Olap"));
    }

    fn io_snap(range_gets: u64, bytes_read: u64) -> crate::observability::io_trace::IoTraceSnapshot {
        crate::observability::io_trace::IoTraceSnapshot {
            range_gets,
            bytes_read,
            ..Default::default()
        }
    }

    #[test]
    fn advised_without_model_is_identical_to_static() {
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: false,
        };
        let s = ComputeScheduler::new();
        let plain = s.route_select(shape);
        let advised = s.route_select_advised(shape, None);
        assert_eq!(advised.backend, plain.backend);
        assert_eq!(advised.reason, plain.reason);
    }

    #[test]
    fn advised_is_observe_mode_never_overrides_backend() {
        use crate::query::route_cost_model::RouteCostModel;
        // OLAP-on-native shape: static rule => Native(Volcano).
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: false,
        };
        let model = RouteCostModel::new().with_min_samples(1);
        // Teach the model that DataFusion is far cheaper for this shape-class
        // (few coalesced GETs vs Native's many small GETs).
        for _ in 0..4 {
            model.observe(
                "olap/native",
                &ComputeBackend::DataFusionLocal,
                &io_snap(3, 16 << 20),
            );
            model.observe(
                "olap/native",
                &ComputeBackend::Native,
                &io_snap(300, 16 << 20),
            );
        }
        let advised = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        // Backend is UNCHANGED (observe-mode) ...
        assert_eq!(advised.backend, ComputeBackend::Native);
        // ... but the divergence is disclosed for EXPLAIN/validation.
        assert!(advised.reason.contains("would prefer DataFusionLocal"));
        assert!(advised.reason.contains("observe-mode"));
    }

    #[test]
    fn advised_notes_when_cost_model_concurs() {
        use crate::query::route_cost_model::RouteCostModel;
        let shape = QueryShape {
            engages_relational: false,
            parquet_backed: false,
        };
        let model = RouteCostModel::new().with_min_samples(1);
        // Only Native has history for this shape-class → it is the min, concurring
        // with the static OLTP→Native rule.
        for _ in 0..3 {
            model.observe("oltp/native", &ComputeBackend::Native, &io_snap(2, 8192));
        }
        let advised = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        assert_eq!(advised.backend, ComputeBackend::Native);
        assert!(advised.reason.contains("cost-model concurs"));
    }

    #[test]
    fn override_off_keeps_static_backend_even_when_model_disagrees() {
        use crate::query::route_cost_model::RouteCostModel;
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: true,
        }; // static => DataFusionLocal
        let model = RouteCostModel::new().with_min_samples(1); // override OFF (default)
        for _ in 0..5 {
            model.observe("olap/parquet", &ComputeBackend::Native, &io_snap(3, 16 << 20));
            model.observe(
                "olap/parquet",
                &ComputeBackend::DataFusionLocal,
                &io_snap(300, 16 << 20),
            );
        }
        let d = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        // Native is far cheaper, but override is off → static DataFusion holds.
        assert_eq!(d.backend, ComputeBackend::DataFusionLocal);
        assert!(d.reason.contains("would prefer Native") && d.reason.contains("observe-mode"));
    }

    #[test]
    fn override_on_flips_olap_parquet_to_the_cheaper_backend() {
        use crate::query::route_cost_model::RouteCostModel;
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: true,
        }; // static => DataFusionLocal
        let model = RouteCostModel::new().with_min_samples(1);
        model.set_override_enabled(true);
        for _ in 0..5 {
            model.observe("olap/parquet", &ComputeBackend::Native, &io_snap(3, 16 << 20));
            model.observe(
                "olap/parquet",
                &ComputeBackend::DataFusionLocal,
                &io_snap(300, 16 << 20),
            );
        }
        let d = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        // Override fires: the route is flipped to the measured-cheaper engine.
        assert_eq!(d.backend, ComputeBackend::Native);
        assert!(d.reason.contains("OVERRIDE"));
    }

    #[test]
    fn override_on_explores_under_explored_olap_parquet_candidate() {
        use crate::query::route_cost_model::RouteCostModel;
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: true,
        }; // static => DataFusionLocal
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_exploration_interval(1);
        model.set_override_enabled(true);
        // DataFusion (the static route) is warm; Native is unexplored → explore it.
        for _ in 0..5 {
            model.observe(
                "olap/parquet",
                &ComputeBackend::DataFusionLocal,
                &io_snap(4, 8192),
            );
        }
        let d = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        assert_eq!(
            d.backend,
            ComputeBackend::Native,
            "exploration routes to the under-explored freshness-safe candidate"
        );
        assert!(d.reason.contains("EXPLORE"));
    }

    #[test]
    fn exploration_never_targets_freshness_critical_oltp() {
        use crate::query::route_cost_model::RouteCostModel;
        let shape = QueryShape {
            engages_relational: false,
            parquet_backed: true,
        }; // static => Native (OLTP)
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_exploration_interval(1);
        model.set_override_enabled(true);
        // Even with DataFusion history for this class, OLTP's freshness-safe set
        // is just [Native], so exploration has nothing to probe.
        for _ in 0..5 {
            model.observe(
                "oltp/parquet",
                &ComputeBackend::DataFusionLocal,
                &io_snap(1, 4096),
            );
        }
        let d = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        assert_eq!(d.backend, ComputeBackend::Native);
        assert!(!d.reason.contains("EXPLORE"));
    }

    #[test]
    fn override_never_flips_freshness_critical_oltp() {
        use crate::query::route_cost_model::RouteCostModel;
        // Point/OLTP shape: static => Native, MUST stay Native (strong freshness)
        // no matter what the cost model says.
        let shape = QueryShape {
            engages_relational: false,
            parquet_backed: true,
        };
        let model = RouteCostModel::new().with_min_samples(1);
        model.set_override_enabled(true);
        // Even if DataFusion looks absurdly cheap for this class, it is not a
        // freshness-safe candidate for OLTP, so it can never be chosen.
        for _ in 0..5 {
            model.observe("oltp/parquet", &ComputeBackend::Native, &io_snap(500, 16 << 20));
            model.observe(
                "oltp/parquet",
                &ComputeBackend::DataFusionLocal,
                &io_snap(1, 4096),
            );
        }
        let d = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        assert_eq!(
            d.backend,
            ComputeBackend::Native,
            "OLTP must never be overridden off Native"
        );
        assert!(!d.reason.contains("OVERRIDE"));
    }

    #[test]
    fn scheduler_materializes_routed_read_plan() {
        let plan = ComputeScheduler::new().route_select_plan(QueryShape {
            engages_relational: true,
            parquet_backed: true,
        });

        assert_eq!(plan.backend, ComputeBackend::DataFusionLocal);
        assert_eq!(plan.workload_profile, CatalogWorkloadProfile::Olap);
        assert_eq!(plan.route_explanation().selected_backend, "DataFusionLocal");
        assert_eq!(
            plan.route_explanation().policy_boundary,
            "connector-enforced"
        );
    }
}
