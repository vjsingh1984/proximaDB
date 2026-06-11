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
    CandidateReadRoute, ReadFreshnessSla, ReadPolicyBoundary, RoutedReadPlan,
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
}

/// Short, stable label for a backend in EXPLAIN/telemetry output.
fn backend_label(backend: &ComputeBackend) -> String {
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
        match (shape.engages_relational, shape.parquet_backed) {
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
        }
    }

    /// Route a relational `SELECT` and materialize the typed read-route plan.
    pub fn route_select_plan(&self, shape: QueryShape) -> RoutedReadPlan {
        self.route_select(shape).routed_read_plan()
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
    fn explain_line_is_stable_and_readable() {
        let line = ComputeScheduler::new()
            .route_select(QueryShape {
                engages_relational: true,
                parquet_backed: false,
            })
            .explain_line();
        assert!(line.starts_with("Compute Route: Native(Volcano) (workload=Olap"));
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
