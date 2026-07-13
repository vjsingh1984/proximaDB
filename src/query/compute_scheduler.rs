// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Read-side compute routing — the `ComputeScheduler`.
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
//! * `freshness`: the static rule keeps strong freshness on
//!   [`ComputeBackend::Native`] — the live Volcano executor over
//!   WAL+`RecordStorage`. Only OLAP-shape queries over Parquet-backed tables
//!   route to `DataFusionLocal` (P1, live behind the default-on
//!   `datafusion-integration` feature).
//!
//! Routing is three tiers behind this one seam: the static shape rule
//! ([`ComputeScheduler::route_select`]), the trace-driven cost-model consult
//! ([`ComputeScheduler::route_select_advised`] — observe-mode advisory on every
//! pgwire relational `SELECT`), and the flag-gated live override
//! (`PROXIMADB_ROUTE_COST_OVERRIDE`, default OFF — explore/exploit over
//! freshness-safe candidates with the TD-170 round-trip hard gate; enablement
//! gated by TD-ROUTE-1's capability-aware candidate fix). The rule engine grows
//! without moving the seam.

use crate::query::read_route::{
    CandidateReadRoute, ReadFreshnessSla, ReadPolicyBoundary, ReadSplitSummary, RoutedReadPlan,
};
use crate::query::table_write_plan::ComputeBackend;
use proximadb_catalog::CatalogAuthorityMode;
use proximadb_catalog::CatalogWorkloadProfile;

/// Estimated cardinality bucket of a query's scan/result — a §5.2 Phase-1 shape
/// input. Bucketed (not raw counts) so it refines the cost-model shape-class
/// without exploding the key space. `Unknown` (the default) keeps the coarse
/// 2-part class, so a planner that cannot estimate rows changes nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CardinalityClass {
    /// No estimate available — class stays coarse (backward-compatible).
    #[default]
    Unknown,
    /// Point / very small result (≲ 1k rows) — favors the low-latency row path.
    Small,
    /// Mid-size scan/result (≲ 1M rows).
    Medium,
    /// Large scan/result — favors the vectorized columnar engine.
    Large,
}

impl CardinalityClass {
    /// Bucket an estimated row count; `None` → [`CardinalityClass::Unknown`].
    pub fn from_estimate(rows: Option<u64>) -> Self {
        match rows {
            None => CardinalityClass::Unknown,
            Some(n) if n <= 1_000 => CardinalityClass::Small,
            Some(n) if n <= 1_000_000 => CardinalityClass::Medium,
            Some(_) => CardinalityClass::Large,
        }
    }

    /// Stable shape-class suffix, or `None` when unknown (omitted from the key).
    pub(crate) fn class_suffix(self) -> Option<&'static str> {
        match self {
            CardinalityClass::Unknown => None,
            CardinalityClass::Small => Some("card=s"),
            CardinalityClass::Medium => Some("card=m"),
            CardinalityClass::Large => Some("card=l"),
        }
    }
}

/// Bucketed count of partitions / row-groups / segments a route fans out over —
/// a §5.2 Phase-1 shape input. High fan-out is GET-round-trip-dominated (the
/// dominant cost term), so it discriminates routes the binary signals cannot.
/// `Unknown` (the default) keeps the coarse class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PartitionFanout {
    /// No partition count available — class stays coarse.
    #[default]
    Unknown,
    /// A single partition / segment.
    Single,
    /// A handful (≤ 8) — bounded fan-out.
    Few,
    /// Many partitions — fan-out / GET-count dominated.
    Many,
}

impl PartitionFanout {
    /// Bucket a partition/segment count; `None` → [`PartitionFanout::Unknown`].
    pub fn from_count(count: Option<u32>) -> Self {
        match count {
            None => PartitionFanout::Unknown,
            Some(0) | Some(1) => PartitionFanout::Single,
            Some(n) if n <= 8 => PartitionFanout::Few,
            Some(_) => PartitionFanout::Many,
        }
    }

    /// Stable shape-class suffix, or `None` when unknown (omitted from the key).
    pub(crate) fn class_suffix(self) -> Option<&'static str> {
        match self {
            PartitionFanout::Unknown => None,
            PartitionFanout::Single => Some("part=1"),
            PartitionFanout::Few => Some("part=f"),
            PartitionFanout::Many => Some("part=m"),
        }
    }
}

/// TD-OLAP-4 operation dimension: the OLAP operation class, so the cost model
/// keys its per-engine samples by *what the query does* (the geometry the shadow
/// ledger showed engines win/lose on), not just cardinality/partition. Default
/// `Unknown` preserves the coarse class + warmed cells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OperationClass {
    #[default]
    Unknown,
    /// Unfiltered scalar aggregate answerable from footer stats — `COUNT(*)`,
    /// `MIN`/`MAX`. Native wins (metadata elision, flat cost).
    MetadataElidable,
    /// Ungrouped scalar aggregate over column data — `SUM`/`AVG`/filtered `COUNT`.
    /// Native-competitive once vectorized + morsel-parallel (TD-OLAP-12).
    ScalarAggregate,
    /// `GROUP BY` aggregate. High-cardinality → DataFusion (native has no spilling).
    Grouped,
    /// String-heavy predicate/projection — `LIKE`/regex/string funcs. DataFusion
    /// (native has no predicate pushdown / wide-string kernels yet).
    StringHeavy,
    /// Anything else — joins, window, subquery, plain projection.
    Other,
}

impl OperationClass {
    pub(crate) fn class_suffix(self) -> Option<&'static str> {
        match self {
            OperationClass::Unknown => None,
            OperationClass::MetadataElidable => Some("op=meta"),
            OperationClass::ScalarAggregate => Some("op=agg"),
            OperationClass::Grouped => Some("op=grp"),
            OperationClass::StringHeavy => Some("op=str"),
            OperationClass::Other => Some("op=other"),
        }
    }
}

/// Bucketed estimated plan depth — half of the TD-EXEC-2 geometry tier.
///
/// Seed cutoffs: `Shallow` (≤ 6) covers the maximal single-table operator chain
/// (scan→filter→aggregate→having→project→sort — the ClickBench posture native
/// wins per TD-OLAP-15); `Mid` (7–12) a few joins; `Deep` (> 12) join trees /
/// nested subqueries (the TPC-H posture DataFusion wins). The ledger records the
/// *measured* `plan_depth` per query (TD-EXEC-2 Slice 1), so these bands are
/// refittable from evidence, not asserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthBand {
    /// Estimated depth ≤ 6 — a single-table operator chain.
    Shallow,
    /// Estimated depth 7–12 — a handful of joins / one nesting level.
    Mid,
    /// Estimated depth > 12 — join trees, deep subquery nesting.
    Deep,
}

impl DepthBand {
    /// Bucket an estimated plan depth (operator count on the longest root→leaf path).
    pub fn from_depth(depth: u32) -> Self {
        match depth {
            0..=6 => DepthBand::Shallow,
            7..=12 => DepthBand::Mid,
            _ => DepthBand::Deep,
        }
    }
}

/// Bucketed pipeline-breaker count (joins + sorts + aggregates — the ops
/// `plan_geometry::OpKind::is_blocking` flags) — the other half of the geometry
/// tier. Blocking count is the strongest engine discriminator TD-OLAP-15
/// measured: zero/low = streaming scan shapes (native-competitive), high =
/// join/agg-bound shapes (DataFusion's surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockingBand {
    /// No pipeline breakers — pure scan/filter/project/limit, streams through.
    Zero,
    /// 1–2 breakers — a single aggregate and/or sort (scan-heavy analytics).
    Low,
    /// ≥ 3 breakers — join-bearing / multi-breaker plans.
    High,
}

impl BlockingBand {
    /// Bucket an estimated pipeline-breaker count.
    pub fn from_count(count: u32) -> Self {
        match count {
            0 => BlockingBand::Zero,
            1..=2 => BlockingBand::Low,
            _ => BlockingBand::High,
        }
    }
}

/// TD-EXEC-2 Slice 3: the plan-geometry tier of the cost-model shape-class —
/// bucketed estimated depth × pipeline-breaker bands, so the EWMA cost cells
/// accumulate per shape⊕geometry class and `route_select_advised` compares
/// engines on the geometry they measurably win/lose on (TD-OLAP-15). Estimated
/// from the AST *before* lowering (the route decision precedes the physical
/// plan); because `finalize_route` stamps this class into the io_trace, the
/// consult key and the EWMA fold key are the same by construction. Default
/// `Unknown` contributes no suffix — the coarse class and warmed cells are
/// preserved (backward-compatible, like every other signal here).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GeometryClass {
    /// No geometry estimate available — class stays coarse.
    #[default]
    Unknown,
    /// Bucketed estimate: depth band × blocking band.
    Known {
        /// Bucketed estimated plan depth.
        depth: DepthBand,
        /// Bucketed estimated pipeline-breaker count.
        blocking: BlockingBand,
    },
}

impl GeometryClass {
    /// Bucket an estimated (depth, pipeline-breaker count) pair.
    pub fn from_estimate(depth: u32, blocking: u32) -> Self {
        GeometryClass::Known {
            depth: DepthBand::from_depth(depth),
            blocking: BlockingBand::from_count(blocking),
        }
    }

    /// Stable shape-class suffix `geom=<depth-band>x<blocking-band>`, or `None`
    /// when unknown (omitted from the key).
    pub(crate) fn class_suffix(self) -> Option<&'static str> {
        let GeometryClass::Known { depth, blocking } = self else {
            return None;
        };
        Some(match (depth, blocking) {
            (DepthBand::Shallow, BlockingBand::Zero) => "geom=sx0",
            (DepthBand::Shallow, BlockingBand::Low) => "geom=sxlo",
            (DepthBand::Shallow, BlockingBand::High) => "geom=sxhi",
            (DepthBand::Mid, BlockingBand::Zero) => "geom=mx0",
            (DepthBand::Mid, BlockingBand::Low) => "geom=mxlo",
            (DepthBand::Mid, BlockingBand::High) => "geom=mxhi",
            (DepthBand::Deep, BlockingBand::Zero) => "geom=dx0",
            (DepthBand::Deep, BlockingBand::Low) => "geom=dxlo",
            (DepthBand::Deep, BlockingBand::High) => "geom=dxhi",
        })
    }
}

/// Shape signals the scheduler routes on.
///
/// P0 used only `engages_relational` — the join / `GROUP BY` / aggregate / set-op
/// gate (the OLAP-shape signal). P1 added `parquet_backed`. C4 Phase-2b adds the
/// §5.2 Phase-1 inputs — `cardinality` and `partition_fanout` — which refine the
/// cost-model shape-class so routing and exploration discriminate beyond
/// `olap/oltp × native/parquet`. TD-OLAP-4 adds `operation_class`; TD-EXEC-2
/// Slice 3 adds `geometry`. All default to `Unknown`, preserving the coarse
/// class (and warmed cells).
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
    /// Estimated scan/result cardinality bucket (§5.2 Phase-1). `Unknown` default.
    pub cardinality: CardinalityClass,
    /// Bucketed partition/row-group fan-out (§5.2 Phase-1). `Unknown` default.
    pub partition_fanout: PartitionFanout,
    /// TD-OLAP-1 slice 2: tables backed by PAX segments (not Parquet). When
    /// true + `pax_reader_enabled()`, routes to DataFusion via PaxSplitReader.
    pub pax_backed: bool,
    /// TD-OLAP-4: the OLAP operation class (metadata-elidable / scalar-aggregate /
    /// grouped / string-heavy) — the geometry dimension the engines win/lose on.
    pub operation_class: OperationClass,
    /// TD-EXEC-2 Slice 3: bucketed plan-geometry estimate (depth band ×
    /// pipeline-breaker band) refining the cost-model class. `Unknown` default.
    pub geometry: GeometryClass,
}

/// TD-OLAP-1 slice 2: PAX-native OLAP scan gate (inline — not imported from
/// `pax_adapter` so compute_scheduler compiles without `datafusion-integration`).
fn pax_reader_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("PROXIMADB_DF_PAX_READER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// What produced a [`SelectRouteDecision`] — a `route_decisions_total` metric
/// label (co-design C4 operator surface). `Static` is the policy rule; the two
/// override variants only occur when `PROXIMADB_ROUTE_COST_OVERRIDE` is enabled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RouteSource {
    /// The static shape-rule decision (no cost-model override fired).
    #[default]
    Static,
    /// Live cost-model override to a confidently-cheaper freshness-safe backend.
    OverrideExploit,
    /// Rate-limited exploration flip to a cold freshness-safe backend (warm-up).
    OverrideExplore,
}

impl RouteSource {
    /// Stable metric-label form for `route_decisions_total{source}`.
    pub fn as_str(&self) -> &'static str {
        match self {
            RouteSource::Static => "static",
            RouteSource::OverrideExploit => "override_exploit",
            RouteSource::OverrideExplore => "override_explore",
        }
    }
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
    /// What produced this decision (static rule vs cost-model override/explore).
    pub source: RouteSource,
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

/// Policy/heuristic read-route scheduler (course correction §5.2, P1 live).
///
/// The static rule routes OLAP-shape queries over Parquet-backed tables to
/// [`ComputeBackend::DataFusionLocal`] and everything else to `Native` (the live
/// Volcano path) in exactly one place ([`Self::route_select`]).
/// [`Self::route_select_advised`] layers the trace-driven cost model on top:
/// observe-mode advisory by default, with a flag-gated live override
/// (`PROXIMADB_ROUTE_COST_OVERRIDE`, default OFF) that only flips between
/// freshness-safe candidates. The rule engine grows
/// (cardinality/partition/point-lookup, then the §5.2 Phase-2 `RLPlanner`
/// learner) without moving this seam.
#[derive(Debug, Default, Clone, Copy)]
pub struct ComputeScheduler;

impl ComputeScheduler {
    /// Construct the (stateless) scheduler.
    pub fn new() -> Self {
        Self
    }

    /// Route a relational `SELECT` plan to a physical engine.
    ///
    /// Public entry: applies the static shape rule, then records the decision
    /// metric and stamps the route onto the io_trace scope via [`finalize_route`].
    pub fn route_select(&self, shape: QueryShape) -> SelectRouteDecision {
        finalize_route(self.route_select_inner(shape), shape)
    }

    /// The static shape rule ONLY — no metric, no io_trace stamp. The pure
    /// policy core; [`Self::route_select`] and [`Self::route_select_advised`]
    /// wrap it (the latter may override the backend before stamping, so the
    /// stamp reflects the FINAL engine — see [`finalize_route`]).
    fn route_select_inner(&self, shape: QueryShape) -> SelectRouteDecision {
        match (shape.engages_relational, shape.parquet_backed) {
            // P1: OLAP shape over Parquet-backed (object-store) table(s) → DataFusion.
            (true, true) => SelectRouteDecision {
                backend: ComputeBackend::DataFusionLocal,
                workload_profile: CatalogWorkloadProfile::Olap,
                reason: "OLAP shape over Parquet-backed table(s) — DataFusion over object storage"
                    .to_string(),
                source: RouteSource::Static,
            },
            // OLAP shape on native storage — Volcano serves it from WAL+RecordStorage
            // until the relational base tier is Parquet/Iceberg (course-correction §6 P3).
            (true, false) => {
                // TD-OLAP-1 slice 2: PAX-backed analytical → DataFusion via
                // PaxSplitReader (flag-gated, default OFF per ADR-052).
                if shape.pax_backed && pax_reader_enabled() {
                    SelectRouteDecision {
                        backend: ComputeBackend::DataFusionLocal,
                        workload_profile: CatalogWorkloadProfile::Olap,
                        reason: "OLAP shape on PAX-backed table(s) — DataFusion via PaxSplitReader"
                            .to_string(),
                        source: RouteSource::Static,
                    }
                } else {
                    SelectRouteDecision {
                        backend: ComputeBackend::Native,
                        workload_profile: CatalogWorkloadProfile::Olap,
                        reason:
                            "OLAP shape (join/group-by/aggregate/set-op) on native storage — Volcano"
                                .to_string(),
                        source: RouteSource::Static,
                    }
                }
            }
            (false, _) => SelectRouteDecision {
                backend: ComputeBackend::Native,
                workload_profile: CatalogWorkloadProfile::Oltp,
                reason: "OLTP shape (point/simple select) — Volcano".to_string(),
                source: RouteSource::Static,
            },
        }
    }

    /// Route a relational `SELECT` and materialize the typed read-route plan.
    pub fn route_select_plan(&self, shape: QueryShape) -> RoutedReadPlan {
        self.route_select(shape).routed_read_plan()
    }

    /// Route a `SELECT`, then — if a trace-driven cost model is supplied —
    /// consult its frozen recommendation table (a lock-free `ArcSwap` load +
    /// `HashMap` get, never the learn-path mutex) and either disclose an advisory
    /// (observe-mode) or flip to a freshness-safe challenger (live override,
    /// flag-gated via `PROXIMADB_ROUTE_COST_OVERRIDE`). With override OFF (the
    /// default) or no warmed history, the static decision is returned unchanged.
    ///
    /// This closes the co-design loop (C0 trace → cost model → router) while
    /// keeping the decision lock-free and O(1): freshness-safety is baked into
    /// the frozen table (only `olap/parquet` offers two safe engines), so the
    /// consult can never flip a query onto an engine that would serve it
    /// incorrectly (e.g. a point/OLTP query onto a stale base snapshot).
    pub fn route_select_advised(
        &self,
        shape: QueryShape,
        model: Option<&crate::query::route_cost_model::RouteCostModel>,
    ) -> SelectRouteDecision {
        use crate::query::route_cost_model::RouteConsult;

        let mut decision = self.route_select_inner(shape);
        let Some(model) = model else {
            return finalize_route(decision, shape);
        };
        let class = crate::query::route_cost_model::shape_class(&shape);

        // Lock-free hierarchical consult: the refined class first, then each
        // coarser ancestor — so a key-refinement tier (geom/op/card) never
        // orphans warmed cells (ADR-058 D3 density across generations). `None`
        // at EVERY level → keep the static route unchanged, and count the miss
        // (a hot class that keeps missing after warm-up means the key
        // fragmented faster than cells warm).
        let consulted = model.consult_with_fallback(&class);
        if consulted.is_none() {
            crate::metrics::route_metrics::record_consult_miss(&class);
        }
        let (consult, consult_class) = match consulted {
            Some((rec, level)) => (Some(rec), Some(level)),
            None => (None, None),
        };
        // Disclose when an ancestor's cells served the decision (refined cell
        // still cold) — visible in EXPLAIN/telemetry, zero behavior change.
        let cells_note = match consult_class.as_deref() {
            Some(level) if level != class => format!(" [cells: {level}]"),
            _ => String::new(),
        };

        if model.override_active()
            && let Some(consult) = consult.as_ref()
        {
            // Exploration (warm-up) first: rate-limited flip to the baked
            // least-sampled freshness-safe candidate so it accrues history the
            // exploit override can later act on. Bounded + freshness-safe.
            if let Some(target) = consult
                .explore_target()
                // Staleness guard: the frozen target may have warmed since the
                // last recompute; re-check against the frozen ranked samples (a
                // target absent from `ranked` was never observed → still cold).
                .filter(|t| {
                    backend_label(t) != backend_label(&decision.backend)
                        && consult
                            .find(t)
                            .is_none_or(|c| c.samples < model.min_samples())
                })
                && model
                    .next_exploration_tick()
                    .is_multiple_of(model.exploration_interval())
            {
                let prev = backend_label(&decision.backend);
                decision.reason = format!(
                    "{} | cost-model EXPLORE {prev}→{} (gathering cost history){cells_note}",
                    decision.reason,
                    backend_label(target)
                );
                decision.backend = target.clone();
                decision.source = RouteSource::OverrideExplore;
                return finalize_route(decision, shape);
            }

            // Exploit override — TD-170 hard round-trip gate, then the soft
            // min_advantage byte-cost gate — both resolved from frozen data.
            if let Some(static_choice) = consult.find(&decision.backend)
                && static_choice.samples >= model.min_samples()
            {
                // Hard gate: a backend over the round-trip budget loses even
                // when it moves fewer bytes (latency = depth × RTT). Flip to the
                // cheapest freshness-safe candidate WITHIN the budget.
                if let Some(budget) = model.rtt_budget()
                    && static_choice.range_gets > budget
                    && let Some(within) = consult
                        .ranked()
                        .iter()
                        .filter(|c| c.range_gets <= budget)
                        .min_by(|a, b| {
                            a.score
                                .partial_cmp(&b.score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .filter(|c| backend_label(&c.backend) != backend_label(&decision.backend))
                {
                    let prev = backend_label(&decision.backend);
                    decision.reason = format!(
                        "{} | cost-model OVERRIDE {prev}→{} (RTT budget {budget:.0} exceeded){cells_note}",
                        decision.reason,
                        backend_label(&within.backend)
                    );
                    decision.backend = within.backend.clone();
                    decision.source = RouteSource::OverrideExploit;
                    return finalize_route(decision, shape);
                }

                // Soft gate: flip to the cheapest warmed candidate if it beats
                // the static route by at least `min_advantage`.
                if let Some(challenger) = consult
                    .ranked()
                    .iter()
                    .filter(|c| c.samples >= model.min_samples())
                    .find(|c| backend_label(&c.backend) != backend_label(&decision.backend))
                    .filter(|c| c.score < static_choice.score * (1.0 - model.min_advantage()))
                {
                    let prev = backend_label(&decision.backend);
                    decision.reason = format!(
                        "{} | cost-model OVERRIDE {prev}→{} (score {:.1} vs {:.1}){cells_note}",
                        decision.reason,
                        backend_label(&challenger.backend),
                        challenger.score,
                        static_choice.score
                    );
                    decision.backend = challenger.backend.clone();
                    decision.source = RouteSource::OverrideExploit;
                    return finalize_route(decision, shape);
                }
            }
        }

        // Observe-mode advisory (no behavior change): disclose the cheapest
        // warmed freshness-safe candidate from the frozen table.
        if let Some(consult) = consult.as_ref()
            && let Some(rec) = consult
                .ranked()
                .iter()
                .find(|c| c.samples >= model.min_samples())
        {
            let advisory = if backend_label(&rec.backend) == backend_label(&decision.backend) {
                format!("cost-model concurs (score {:.1})", rec.score)
            } else {
                format!(
                    "cost-model would prefer {} (score {:.1}) — observe-mode, route unchanged",
                    backend_label(&rec.backend),
                    rec.score
                )
            };
            decision.reason = format!("{} | {advisory}{cells_note}", decision.reason);
        }

        finalize_route(decision, shape)
    }
}

/// Record the decision metric (and the OLAP-on-Native nudge), then stamp the
/// FINAL route onto the active io_trace scope. Called once per route decision,
/// AFTER any override has settled the backend — so the learn loop attributes a
/// query's measured cost to the engine that actually served it (fixing the prior
/// bug where the static backend was stamped before an override could flip it).
/// Stamping is a no-op when no io_trace scope is active (unit tests / EXPLAIN).
fn finalize_route(decision: SelectRouteDecision, shape: QueryShape) -> SelectRouteDecision {
    let class = crate::query::route_cost_model::shape_class(&shape);
    let backend = backend_label(&decision.backend);
    crate::metrics::route_metrics::record_decision(&backend, &class, decision.source.as_str());
    if decision.backend == ComputeBackend::Native && class.starts_with("olap/") {
        crate::metrics::route_metrics::record_olap_on_native(&class);
    }
    crate::observability::io_trace::record_route(&class, &backend);
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn olap_shape_on_native_storage_stays_on_volcano() {
        let decision = ComputeScheduler::new().route_select(QueryShape {
            engages_relational: true,
            parquet_backed: false,
            ..Default::default()
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
            ..Default::default()
        });
        assert_eq!(decision.backend, ComputeBackend::Native);
        assert_eq!(decision.workload_profile, CatalogWorkloadProfile::Oltp);
    }

    #[test]
    fn p1_olap_over_parquet_routes_to_datafusion() {
        let d = ComputeScheduler::new().route_select(QueryShape {
            engages_relational: true,
            parquet_backed: true,
            ..Default::default()
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
            ..Default::default()
        });
        assert_eq!(d.backend, ComputeBackend::Native);
    }

    #[test]
    fn datafusion_route_carries_concrete_row_group_split_inventory() {
        let plan = ComputeScheduler::new()
            .route_select(QueryShape {
                engages_relational: true,
                parquet_backed: true,
                ..Default::default()
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
                ..Default::default()
            })
            .explain_line();
        assert!(line.starts_with("Compute Route: Native(Volcano) (workload=Olap"));
    }

    fn io_snap(
        range_gets: u64,
        bytes_read: u64,
    ) -> crate::observability::io_trace::IoTraceSnapshot {
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
            ..Default::default()
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
        // OLAP-over-Parquet: static rule => DataFusionLocal. Warm Native as far
        // cheaper (few coalesced GETs vs DataFusion's many) so observe-mode
        // DISCLOSES the preference without flipping the backend. (The only class
        // with two freshness-safe engines — the only one an advisory can prefer.)
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: true,
            ..Default::default()
        };
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_recompute_every(1);
        for _ in 0..4 {
            model.observe(
                "olap/parquet",
                &ComputeBackend::Native,
                &io_snap(3, 16 << 20),
            );
            model.observe(
                "olap/parquet",
                &ComputeBackend::DataFusionLocal,
                &io_snap(300, 16 << 20),
            );
        }
        let advised = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        // Backend is UNCHANGED (observe-mode) ...
        assert_eq!(advised.backend, ComputeBackend::DataFusionLocal);
        // ... but the divergence is disclosed for EXPLAIN/validation.
        assert!(advised.reason.contains("would prefer Native"));
        assert!(advised.reason.contains("observe-mode"));
    }

    #[test]
    fn advised_notes_when_cost_model_concurs() {
        use crate::query::route_cost_model::RouteCostModel;
        let shape = QueryShape {
            engages_relational: false,
            parquet_backed: false,
            ..Default::default()
        };
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_recompute_every(1);
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
            ..Default::default()
        }; // static => DataFusionLocal
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_recompute_every(1); // override OFF (default)
        for _ in 0..5 {
            model.observe(
                "olap/parquet",
                &ComputeBackend::Native,
                &io_snap(3, 16 << 20),
            );
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
            ..Default::default()
        }; // static => DataFusionLocal
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_recompute_every(1);
        model.set_override_enabled(true);
        for _ in 0..5 {
            model.observe(
                "olap/parquet",
                &ComputeBackend::Native,
                &io_snap(3, 16 << 20),
            );
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
            ..Default::default()
        }; // static => DataFusionLocal
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_exploration_interval(1)
            .with_recompute_every(1);
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
            ..Default::default()
        }; // static => Native (OLTP)
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_exploration_interval(1)
            .with_recompute_every(1);
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
            ..Default::default()
        };
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_recompute_every(1);
        model.set_override_enabled(true);
        // Even if DataFusion looks absurdly cheap for this class, it is not a
        // freshness-safe candidate for OLTP, so it can never be chosen.
        for _ in 0..5 {
            model.observe(
                "oltp/parquet",
                &ComputeBackend::Native,
                &io_snap(500, 16 << 20),
            );
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
            ..Default::default()
        });

        assert_eq!(plan.backend, ComputeBackend::DataFusionLocal);
        assert_eq!(plan.workload_profile, CatalogWorkloadProfile::Olap);
        assert_eq!(plan.route_explanation().selected_backend, "DataFusionLocal");
        assert_eq!(
            plan.route_explanation().policy_boundary,
            "connector-enforced"
        );
    }

    #[test]
    fn cold_model_consult_returns_static_decision() {
        use crate::query::route_cost_model::RouteCostModel;
        // Empty model (no observations) → frozen table empty → consult None →
        // static decision unchanged, no advisory appended.
        let model = RouteCostModel::new();
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: true,
            ..Default::default()
        };
        let advised = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        assert_eq!(advised.backend, ComputeBackend::DataFusionLocal);
        assert_eq!(advised.source, RouteSource::Static);
        assert!(!advised.reason.contains("cost-model"));
    }

    #[test]
    fn override_sets_decision_source_label() {
        use crate::query::route_cost_model::RouteCostModel;
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: true,
            ..Default::default()
        };
        let model = RouteCostModel::new()
            .with_min_samples(1)
            .with_recompute_every(1);
        model.set_override_enabled(true);
        for _ in 0..5 {
            model.observe(
                "olap/parquet",
                &ComputeBackend::Native,
                &io_snap(3, 16 << 20),
            );
            model.observe(
                "olap/parquet",
                &ComputeBackend::DataFusionLocal,
                &io_snap(300, 16 << 20),
            );
        }
        let d = ComputeScheduler::new().route_select_advised(shape, Some(&model));
        assert_eq!(d.backend, ComputeBackend::Native);
        assert_eq!(d.source, RouteSource::OverrideExploit);
    }

    #[test]
    fn olap_on_native_is_static_with_volcano_backend() {
        // An olap/native route stays on Volcano (single freshness-safe candidate)
        // with a Static source; the ROUTE_OLAP_ON_NATIVE_TOTAL nudge is the
        // operator signal (asserted in route_metrics tests), recorded in finalize_route.
        let shape = QueryShape {
            engages_relational: true,
            parquet_backed: false,
            ..Default::default()
        };
        let d = ComputeScheduler::new().route_select(shape);
        assert_eq!(d.backend, ComputeBackend::Native);
        assert_eq!(d.source, RouteSource::Static);
    }
}
