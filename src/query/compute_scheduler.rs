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

use crate::query::table_write_plan::ComputeBackend;
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
        if shape.engages_relational {
            // OLAP shape — the eventual DataFusion destination (P1). Until that
            // read path is live, the OLAP arm stays on Volcano.
            SelectRouteDecision {
                backend: ComputeBackend::Native,
                workload_profile: CatalogWorkloadProfile::Olap,
                reason: "OLAP shape (join/group-by/aggregate/set-op); DataFusion \
                         read path not yet wired (P1) — staying on Volcano"
                    .to_string(),
            }
        } else {
            SelectRouteDecision {
                backend: ComputeBackend::Native,
                workload_profile: CatalogWorkloadProfile::Oltp,
                reason: "OLTP shape (point/simple select) — Volcano".to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn olap_shape_classifies_olap_but_p0_stays_on_native() {
        let decision = ComputeScheduler::new().route_select(QueryShape {
            engages_relational: true,
        });
        // P0 invariant: always Volcano/Native, regardless of shape.
        assert_eq!(decision.backend, ComputeBackend::Native);
        assert_eq!(decision.workload_profile, CatalogWorkloadProfile::Olap);
        assert!(decision.reason.to_lowercase().contains("olap"));
    }

    #[test]
    fn oltp_shape_classifies_oltp_on_native() {
        let decision = ComputeScheduler::new().route_select(QueryShape {
            engages_relational: false,
        });
        assert_eq!(decision.backend, ComputeBackend::Native);
        assert_eq!(decision.workload_profile, CatalogWorkloadProfile::Oltp);
    }

    #[test]
    fn p0_never_routes_off_native() {
        // The whole point of P0: no behavior change. Both shapes stay Native.
        for engages in [true, false] {
            let d = ComputeScheduler::new().route_select(QueryShape {
                engages_relational: engages,
            });
            assert_eq!(d.backend, ComputeBackend::Native);
        }
    }

    #[test]
    fn explain_line_is_stable_and_readable() {
        let line = ComputeScheduler::new()
            .route_select(QueryShape {
                engages_relational: true,
            })
            .explain_line();
        assert!(line.starts_with("Compute Route: Native(Volcano) (workload=Olap"));
    }
}
