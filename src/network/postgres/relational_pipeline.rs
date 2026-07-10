//! Bridge between the pgwire surface and the new relational
//! pipeline (algebra → planner → executor → engine). This is the
//! S5c integration point — when
//! `PROXIMADB_NEW_RELATIONAL_PIPELINE=1` is set in the
//! environment, SELECT statements on tables that exist in the
//! global [`InMemoryRelationalEngine`] route through this module
//! instead of the legacy `QueryLowering` → vector-collection
//! path.
//!
//! Scope:
//!
//! - Read-only path (SELECT only). INSERT/UPDATE/DELETE on engine
//!   tables stay on the legacy path until a follow-up slice wires
//!   the write side.
//! - Tables are discovered from the engine's catalog
//!   ([`InMemoryRelationalEngine::schema_of`]).
//! - If the SQL doesn't lower cleanly (e.g. it's a pg-specific
//!   query like `SELECT current_schema()`), [`try_run_select`]
//!   returns `None` so the caller falls through to the legacy
//!   path.
//! - The engine is bootstrapped lazily once per process. When
//!   `PROXIMADB_RELATIONAL_BOOTSTRAP_DEMO_TABLE=1` is also set,
//!   a `demo_users(id BIGINT PRIMARY KEY, name TEXT, age INT)`
//!   table is seeded with 3 rows so operators can demonstrate
//!   the new path end-to-end via psql.

use crate::query::execution::{
    ExecutionControls, ExecutionPipelineResult, NativeVolcanoEngine, QueryExecutionContext,
    execute_sql_with_backend, normalize_table_key,
};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use proximadb_data_model::{ProximaType, ProximaValue};
use proximadb_functions::builtins;
use proximadb_relational_algebra::TableId;
use proximadb_relational_engine::{
    EngineReaderFactory, InMemoryRelationalEngine, RelationalWriter,
};
use proximadb_relational_executor::{ExecError, NodeMetric, ReaderFactory};
use proximadb_relational_frontend::{CatalogLookup, lower_sql};
use proximadb_relational_planner::{
    CapabilityResolver, PhysicalPlan, Planner, StaticCapabilities, explain_physical,
};
use proximadb_relational_reader::{ReaderCapabilities, ReaderError, RelationalReader, ScanContext};
use proximadb_relational_types::{ColumnInfo, Expr, ExprError, RelationalRow, RelationalSchema};
use sqlparser::ast::{
    BinaryOperator, Expr as SqlExpr, GroupByExpr, Query as SqlQuery, SelectItem, SetExpr,
    Statement, TableFactor, TableWithJoins,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::search::VectorFreshnessMode;
use crate::query::cache::invalidation_coordinator::CacheInvalidationCoordinator;
use crate::query::cache::query_result_cache::{QueryKey, QueryResultCache, StructuralKey};
use crate::services::dml::DmlService;
use crate::storage::tenant::context::TenantContext;

use super::types::PgType;

// =========================================================================
// Process-wide engine
// =========================================================================

/// Process-wide [`InMemoryRelationalEngine`] used by the new
/// pipeline. Initialised on first access. **Single-process only**:
/// real distribution will replace this with a SharedServices-owned
/// engine; for the MVP wire-in (S5c) this static keeps the
/// integration self-contained.
pub static GLOBAL_ENGINE: Lazy<Arc<InMemoryRelationalEngine>> = Lazy::new(|| {
    let engine = InMemoryRelationalEngine::new();
    if std::env::var("PROXIMADB_RELATIONAL_BOOTSTRAP_DEMO_TABLE").is_ok() {
        bootstrap_demo_table(&engine);
    }
    engine
});

fn bootstrap_demo_table(engine: &Arc<InMemoryRelationalEngine>) {
    let schema = RelationalSchema::new(vec![
        ColumnInfo::new("id", ProximaType::Int64, false),
        ColumnInfo::new("name", ProximaType::String, true),
        ColumnInfo::new("age", ProximaType::Int32, true),
    ]);
    if engine.create_table("demo_users", schema, vec![0]).is_ok() {
        let _ = engine.insert_rows(
            "demo_users",
            vec![
                vec![
                    ProximaValue::Int64(1),
                    ProximaValue::String("alice".into()),
                    ProximaValue::Int32(30),
                ],
                vec![
                    ProximaValue::Int64(2),
                    ProximaValue::String("bob".into()),
                    ProximaValue::Int32(25),
                ],
                vec![
                    ProximaValue::Int64(3),
                    ProximaValue::String("carol".into()),
                    ProximaValue::Int32(40),
                ],
            ],
        );
    }
}

// =========================================================================
// Process-wide OLAP result cache (default-OFF; ADR-051 D2 / mandate #16)
// =========================================================================

/// Master switch for the pgwire OLAP result cache. **Default-OFF.** Set
/// `PROXIMADB_QUERY_RESULT_CACHE` to a truthy token (`1`/`true`/`on`/`yes`,
/// case-insensitive) to enable structurally tenant-keyed caching of OLAP SELECT
/// results with Strong LSN-pinned freshness. Pure + split out so it is
/// unit-testable without mutating the process environment.
fn olap_result_cache_on(var: Option<&str>) -> bool {
    match var.map(str::trim) {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
        None => false,
    }
}

fn olap_result_cache_enabled() -> bool {
    olap_result_cache_on(
        std::env::var("PROXIMADB_QUERY_RESULT_CACHE")
            .ok()
            .as_deref(),
    )
}

/// Process-wide OLAP result cache, keyed structurally on
/// `(tenant, namespace, query)`. Shared across every pgwire + REST/gRPC
/// connection (mirrors [`GLOBAL_ENGINE`]'s singleton pattern).
pub static GLOBAL_OLAP_RESULT_CACHE: Lazy<Arc<QueryResultCache<ExecutionPipelineResult>>> =
    Lazy::new(|| Arc::new(QueryResultCache::with_defaults()));

/// Process-wide invalidation coordinator with the OLAP result cache attached,
/// so writes/DDL drop tenant-scoped entries in one fan-out call (mandate #16b).
///
/// Only the `result_cache` arm is attached, deliberately:
/// - **PlanCache** is already invalidated *lazily* — every
///   `invalidate_collection` call bumps `CorpusVersionRegistry`, and `PlanCache`
///   consults that version on lookup → stale entries already miss. An eager
///   `with_plan_cache` arm would only free memory slightly sooner.
/// - **BatchGroupCache** has no production instance (test-only) and is keyed on
///   `(batch_id, group_id)`, not `(tenant, collection)` — there's no
///   `(tenant, collection) → batch_id` mapping to drive it. Wire it when a live
///   batch registry lands.
pub static GLOBAL_OLAP_CACHE_COORDINATOR: Lazy<Arc<CacheInvalidationCoordinator>> =
    Lazy::new(|| {
        Arc::new(
            CacheInvalidationCoordinator::empty()
                .with_result_cache(GLOBAL_OLAP_RESULT_CACHE.clone()),
        )
    });

/// Returns the shared OLAP result cache only when the feature is enabled
/// (default-OFF). Callers pass `.as_deref()` of this as the `result_cache`
/// argument to [`try_run_select`].
pub fn olap_result_cache() -> Option<Arc<QueryResultCache<ExecutionPipelineResult>>> {
    if olap_result_cache_enabled() {
        Some(GLOBAL_OLAP_RESULT_CACHE.clone())
    } else {
        None
    }
}

/// The shared invalidation coordinator (always reachable; its result-cache arm
/// is a no-op when the flag is off because nothing was ever inserted).
pub fn olap_cache_coordinator() -> Arc<CacheInvalidationCoordinator> {
    GLOBAL_OLAP_CACHE_COORDINATOR.clone()
}

/// Tenant-scoped invalidation of the OLAP result cache after a write/DDL. Drops
/// every cached entry registered under `(tenant, normalize_table_key(table))`.
/// The table name is normalized here (matching the read-side dependency keys,
/// which are `snapshot.tables` keys) so callers can pass the raw parsed name.
/// Default-OFF: a cheap no-op when the cache flag is unset. Call from the
/// pgwire INSERT/UPDATE/DELETE/DDL success paths (mandate #16b).
pub async fn invalidate_olap_result_cache_for(tenant: &str, table: &str) {
    if olap_result_cache().is_some() {
        let normalized = normalize_table_key(table);
        olap_cache_coordinator()
            .invalidate_collection(tenant, &normalized, &[])
            .await;
    }
}

/// Cheap canonical-WAL LSN read — the Strong-freshness anchor (ADR-051 D2 /
/// ADR-046). Returns `0` when WAL-manifest tracking is unavailable, which makes
/// Strong lookups bypass (never serve) — the safe default. Same source as the
/// vector-search path (`services::operations::vectors::legacy`).
async fn current_canonical_lsn() -> u64 {
    match crate::storage::persistence::write_ahead_log::manifest::get_service() {
        Some(svc) => svc.current_lsn().await,
        None => 0,
    }
}

/// Stamp a freshly-executed OLAP result into the result cache under its
/// structural key, with the LSN it was computed at. No-op when the cache is
/// disabled or LSN tracking is unavailable (`current_lsn == 0` → never
/// Strong-eligible, so don't waste a slot).
fn populate_olap_result_cache(
    cache: Option<&QueryResultCache<ExecutionPipelineResult>>,
    skey: &StructuralKey,
    lsn: u64,
    result: ExecutionPipelineResult,
    deps_table_keys: &HashMap<String, PreparedTable>,
) {
    let Some(cache) = cache else {
        return;
    };
    let computed_at_lsn = (lsn != 0).then_some(lsn);
    let deps: Vec<String> = deps_table_keys.keys().cloned().collect();
    if let Err(e) = cache.insert_fresh(skey.clone(), result, deps, computed_at_lsn) {
        tracing::debug!(
            target: "proximadb::pgwire::result_cache",
            error = ?e,
            "failed to cache OLAP result"
        );
    }
}

// =========================================================================
// Public entry point
// =========================================================================

/// Try to run `sql` through the new pipeline.
///
/// Returns:
///
/// - `None` — SQL didn't lower cleanly
///   (the caller should fall through to the legacy SQL path).
/// - `Some(Ok(result))` — pipeline executed; caller should emit
///   the result to the pgwire client.
/// - `Some(Err(msg))` — pipeline reached execution and failed;
///   caller should report a pgwire `ERROR` to the client.
///
/// ADR-018 Phase 2: New pipeline is enabled by default for SELECT queries
/// that can be lowered by the relational frontend. This provides proper
/// multi-column ORDER BY support and other relational features.
/// Set PROXIMADB_NEW_RELATIONAL_PIPELINE=0 to disable and force legacy path.
pub async fn try_run_select(
    sql: &str,
    dml: Option<&Arc<DmlService>>,
    #[cfg_attr(not(feature = "datafusion-integration"), allow(unused_variables))]
    vector_ops: Option<Arc<dyn proximadb_runtime::VectorOpsPort>>,
    #[cfg_attr(not(feature = "datafusion-integration"), allow(unused_variables))] graph_ops: Option<
        Arc<dyn proximadb_graph_query::service::GraphQueryReadService>,
    >,
    tenant: Option<&str>,
    controls: ExecutionControls,
    // pgwire passes `true`: only joins/GROUP BY/aggregates/set-ops engage the
    // real-data relational engine, leaving simple SELECTs on pgwire's hardened
    // legacy path. Off-pgwire callers (gRPC/REST `ExecuteQuery`, TD-121) have no
    // such legacy single-table path, so they pass `false` to route *any*
    // resolvable relational SELECT through this tenant-scoped pipeline; queries
    // whose tables don't resolve return `None` and fall back to the caller's
    // vector/graph engine.
    require_engagement: bool,
    // Namespace / schema (pgwire `search_path`) — a structural key component so
    // identical SQL under different namespaces never collides.
    namespace: Option<&str>,
    // Shared OLAP result cache (default-OFF — `None` when
    // `PROXIMADB_QUERY_RESULT_CACHE` is unset). A hit short-circuits routing +
    // execution; a miss falls through normally and the result is stamped on the
    // real-data success paths.
    result_cache: Option<&QueryResultCache<ExecutionPipelineResult>>,
) -> Option<Result<ExecutionPipelineResult, String>> {
    // TD-064: the connection's tenant scopes every snapshot read to the tenant's
    // record partition (carried into the SnapshotCatalog → DmlTableReader).
    let tenant_ctx: Option<TenantContext> = tenant
        .filter(|t| !t.is_empty())
        .map(TenantContext::for_tenant_id);
    // ADR-018 Phase 2: Allow opting out with explicit "0" value
    if std::env::var("PROXIMADB_NEW_RELATIONAL_PIPELINE")
        .ok()
        .as_deref()
        == Some("0")
    {
        tracing::debug!(
            target: "proximadb::pgwire::new_pipeline",
            "PROXIMADB_NEW_RELATIONAL_PIPELINE=0; skipping new pipeline"
        );
        return None;
    }

    // OLAP result cache lookup (ADR-051 D2 / mandate #16c). Strong freshness:
    // a hit is served ONLY when the entry was computed at the current canonical
    // WAL LSN, so any write since → guaranteed miss → read-after-write correct.
    // `cache_ctx` carries the (key, lsn) pair so the real-data success paths
    // below can stamp the result without recomputing it.
    let cache_ctx: Option<(StructuralKey, u64)> = if let Some(cache) = result_cache {
        let current_lsn = current_canonical_lsn().await;
        let skey = StructuralKey::new(
            tenant.unwrap_or(""),
            namespace.unwrap_or(""),
            QueryKey::from_sql(sql),
        );
        if let Some(cached) = cache.get_fresh(&skey, &VectorFreshnessMode::Strong, current_lsn) {
            tracing::debug!(
                target: "proximadb::pgwire::result_cache",
                tenant = tenant.unwrap_or(""),
                lsn = current_lsn,
                "OLAP result cache hit — short-circuiting route + execution"
            );
            return Some(Ok(cached.result.clone()));
        }
        Some((skey, current_lsn))
    } else {
        None
    };

    // TD-OLAP-4 result-path probe: time ALL of `try_run_select` before execution
    // — including the path-1 engine-catalog `lower_sql` attempt below, which a
    // DataFusion-routed query pays and fails before reaching the path-2 setup.
    // The earlier per-query floor decomposition attributed ~0 ms to the path-2
    // setup alone; starting the clock here tests whether the ~46 ms remainder is
    // the (previously untimed) path-1 lowering attempt.
    let setup_start = std::time::Instant::now();
    // 1) Existing in-memory-engine path (preserves the demo table and any
    //    engine-resident tables). Only engages when the SQL lowers against the
    //    engine's own catalog; a catalog miss falls through to the real-data
    //    path below.
    let engine = GLOBAL_ENGINE.clone();
    if let Ok(logical) = lower_sql(sql, &EngineCatalog(engine.clone())) {
        let factory = EngineReaderFactory::new(engine);
        let resolver = StaticCapabilities {
            caps: ReaderCapabilities::full(),
            pk_columns: Vec::new(),
            secondary_columns: Vec::new(),
        };
        return Some(run_plan(&factory, resolver, logical, controls).await);
    }

    // 2) Real-data path (gated, additive). Engage ONLY for queries the legacy
    //    single-table path can't serve — joins / GROUP BY / aggregates / HAVING
    //    / set-ops — leaving simple SELECTs on the (hardened) legacy path.
    let dml = dml?;
    let statements = Parser::parse_sql(&GenericDialect {}, sql).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    // Engaged = a relational shape the legacy single-table path can't serve
    // (joins / GROUP BY / aggregates / set-ops). pgwire keeps simple SELECTs on
    // its legacy path (`require_engagement = true`); off-pgwire callers route any
    // resolvable relational SELECT here (`require_engagement = false`).
    if require_engagement && !query_engages_relational_engine(query) {
        return None;
    }

    // Pre-resolve every referenced table's SCHEMA from xCatalog (the sync
    // `CatalogLookup`/`ReaderFactory` traits can't await). Rows are fetched
    // lazily per scan in `DmlTableReader::open`, with the executor's
    // projection/predicate/limit pushed into storage.
    let mut names = Vec::new();
    collect_table_names(query, &mut names);
    let mut tables: HashMap<String, PreparedTable> = HashMap::new();
    // P1: per-table Parquet location (object-store backed), populated only under the
    // `datafusion-integration` feature so the OLAP DataFusion route is never taken
    // (nor advertised) when the build can't honor it.
    #[cfg(feature = "datafusion-integration")]
    let mut parquet_loc_by_key: HashMap<String, String> = HashMap::new();
    // ADR-025: per-table OLAP read-merge params (snapshot_lsn + PK column) for the
    // opted-in parquet-backed tables; empty ⇒ all reads stay on the bare Parquet path.
    #[cfg(feature = "datafusion-integration")]
    let mut olap_delta_tables: HashMap<
        String,
        crate::query::execution::olap_delta_merge::OlapDeltaTable,
    > = HashMap::new();
    for raw in &names {
        let key = normalize_table_key(raw);
        if tables.contains_key(&key) {
            continue;
        }
        match dml.resolve_relational_schema(raw, tenant).await {
            Ok(catalog_schema) => {
                #[cfg(feature = "datafusion-integration")]
                if let Some(location) = catalog_table_is_parquet_backed(&catalog_schema) {
                    parquet_loc_by_key.insert(key.clone(), location);
                    if let Some(params) = olap_delta_table_params(&catalog_schema) {
                        olap_delta_tables.insert(key.clone(), params);
                    }
                }
                tables.insert(key, PreparedTable::from_catalog(raw, &catalog_schema));
            }
            Err(e) => {
                tracing::debug!(
                    target: "proximadb::pgwire::new_pipeline",
                    "relational schema resolve for `{raw}` failed: {e}; falling through to legacy"
                );
                return None;
            }
        }
    }

    // Course-correction §5: compute the per-query Parquet-backed signal — only ever
    // true under `datafusion-integration`, where the DataFusion destination is
    // compiled — and feed it into the canonical `ComputeScheduler` so the ROUTE
    // DECISION and the physical DISPATCH come from ONE place. Mixed Parquet+native
    // queries report `false` (cross-engine join is a later phase) → Volcano.
    #[cfg(feature = "datafusion-integration")]
    let parquet_backed =
        !tables.is_empty() && tables.keys().all(|k| parquet_loc_by_key.contains_key(k));
    #[cfg(not(feature = "datafusion-integration"))]
    let parquet_backed = false;

    // C4 Phase-2b: refine the shape-class with real fan-out / cardinality for
    // hot Parquet-backed tables, peeked from the in-memory stat cache warmed by
    // table opens (zero route-time I/O — a cold/never-scanned table stays
    // `Unknown`, backward-compatible). Finer classes let the cost model
    // discriminate within the OLAP-over-Parquet band where a Native↔DataFusion
    // override is freshness-safe.
    #[cfg(feature = "datafusion-integration")]
    let (partition_fanout, cardinality) = if parquet_backed {
        let locations: Vec<String> = parquet_loc_by_key.values().cloned().collect();
        crate::query::route_cost_model::classify_table_shapes(&locations)
    } else {
        (Default::default(), Default::default())
    };
    #[cfg(not(feature = "datafusion-integration"))]
    let (partition_fanout, cardinality) = (Default::default(), Default::default());

    // AST cardinality fallback: when the footer-warmed stat is `Unknown` (native
    // storage, or a cold Parquet table never yet scanned), derive a cheap
    // syntax-only hint (LIMIT / scalar aggregate) so the cost-model shape-class
    // still discriminates instead of collapsing to the coarse class. Pure AST
    // walk — zero route-time I/O (co-design P5).
    let cardinality = if cardinality == crate::query::compute_scheduler::CardinalityClass::Unknown {
        query_cardinality_hint(query)
    } else {
        cardinality
    };

    let decision = crate::query::compute_scheduler::ComputeScheduler::new().route_select_advised(
        crate::query::compute_scheduler::QueryShape {
            engages_relational: true,
            parquet_backed,
            pax_backed: false,
            partition_fanout,
            cardinality,
            operation_class: query_operation_class(query),
        },
        // C4: observe-mode advisory from the trace-driven cost model — augments
        // the reason for telemetry/EXPLAIN, never changes the backend.
        Some(&crate::query::route_cost_model::GLOBAL_ROUTE_COST_MODEL),
    );
    tracing::debug!(
        target: "proximadb::compute_route",
        backend = ?decision.backend,
        workload = ?decision.workload_profile,
        source = decision.source.as_str(),
        reason = %decision.reason,
        "{}",
        decision.explain_line()
    );

    // P1 dispatch driven by the scheduler decision. The DataFusion arm exists only
    // under the feature (and `parquet_backed` — hence a `DataFusionLocal` decision —
    // is only reachable there), so a default build never enters it and stays Volcano.
    #[cfg(feature = "datafusion-integration")]
    if matches!(
        decision.backend,
        crate::query::table_write_plan::ComputeBackend::DataFusionLocal
    ) {
        let parquet_tables: Vec<(String, String)> = tables
            .iter()
            .map(|(k, t)| (t.table_name.clone(), parquet_loc_by_key[k].clone()))
            .collect();
        let context = QueryExecutionContext {
            parquet_tables,
            vector_ops,
            graph_ops,
            tenant_id: tenant.map(str::to_string),
            // ADR-025: reconcile opted-in parquet-backed tables with their
            // post-snapshot WAL delta at scan time. `None` (no opted-in table)
            // keeps the legacy bare-Parquet read (default-OFF).
            olap_delta: if olap_delta_tables.is_empty() {
                None
            } else {
                Some(crate::query::execution::olap_delta_merge::OlapDeltaConfig {
                    source: dml.clone(),
                    tables: olap_delta_tables,
                })
            },
            // Full ANSI SQL over pgwire: when the shared relational frontend can't
            // yet lower a query (e.g. typed DATE literals, certain subquery/window
            // shapes), execute it through DataFusion's own ANSI SQL planner over the
            // same registered Parquet tables. The query still routes through pgwire →
            // ComputeScheduler → the DataFusion engine; only the lowering frontend
            // differs. The shared logical plane stays the fast path where it works.
            allow_engine_sql_fallback: true,
            controls: controls.clone(),
        };
        // ADR-030 / TD-158: time the DataFusion (engine-SQL fallback) execution so
        // the always-on billing observer can attribute KRU to this engine at scope
        // close. `record_compute_ms` no-ops outside an `io_trace` scope.
        crate::observability::io_trace::record_setup_ms(setup_start.elapsed().as_millis() as u64);
        let started = std::time::Instant::now();
        let engine_result = execute_sql_with_backend(decision.backend.clone(), sql, context).await;
        crate::observability::io_trace::record_compute_ms(
            &crate::query::compute_scheduler::backend_label(&decision.backend),
            started.elapsed().as_millis() as u64,
        );
        // TD-OLAP-4 (engine dimension): optional native SHADOW — run the same query
        // on the native vectorized engine over the same parquet, purely to record a
        // `native-vectorized` compute sample next to DataFusion's for the trace.
        // Default-OFF, benchmark-only; the result is discarded (DataFusion's stands)
        // and any failure is swallowed, so it never affects correctness or latency
        // of the served query.
        if crate::query::execution::native_engine::native_shadow_enabled() {
            let probe_snapshot = SnapshotCatalog {
                dml: dml.clone(),
                tables: tables.clone(),
                tenant: tenant_ctx.clone(),
            };
            shadow_probe_native_parquet(sql, &probe_snapshot, &parquet_loc_by_key).await;
        }
        return Some(match engine_result {
            Ok(result) => {
                if let Some((skey, lsn)) = &cache_ctx {
                    populate_olap_result_cache(result_cache, skey, *lsn, result.clone(), &tables);
                }
                Ok(result)
            }
            Err(e) => Err(e.to_string()),
        });
    }

    let snapshot = SnapshotCatalog {
        dml: dml.clone(),
        tables,
        tenant: tenant_ctx,
    };
    // Lower + plan via the shared planning path (so EXPLAIN discloses exactly this
    // plan). `None` → lowering declined → fall through to legacy. From the planned
    // physical onward, errors are real and surface to the client.
    let physical = match plan_over_snapshot(sql, &snapshot)? {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    crate::observability::io_trace::record_setup_ms(setup_start.elapsed().as_millis() as u64);
    match execute_physical(physical, &snapshot, controls).await {
        Ok(result) => {
            if let Some((skey, lsn)) = &cache_ctx {
                populate_olap_result_cache(
                    result_cache,
                    skey,
                    *lsn,
                    result.clone(),
                    &snapshot.tables,
                );
            }
            Some(Ok(result))
        }
        Err(e) => Some(Err(e)),
    }
}

/// Plan + build + drain an executor for `logical` against `factory`, using
/// `resolver` for capability/PK-lookup planning (engine path → `StaticCapabilities`,
/// real-data path → `SnapshotCapabilities`).
async fn run_plan<F: ReaderFactory, R: CapabilityResolver>(
    factory: &F,
    resolver: R,
    logical: proximadb_relational_algebra::LogicalNode,
    controls: ExecutionControls,
) -> Result<ExecutionPipelineResult, String> {
    let planner = Planner::new(resolver);
    let physical = planner.plan(logical).map_err(|e| format!("plan: {e}"))?;
    execute_physical(physical, factory, controls).await
}

/// Build + open + drain an executor for an already-planned `physical` plan.
/// Shared tail of `run_plan` and the real-data path so EXPLAIN (which plans
/// without executing) and execution run the SAME `PhysicalPlan`.
async fn execute_physical<F: ReaderFactory>(
    physical: PhysicalPlan,
    factory: &F,
    controls: ExecutionControls,
) -> Result<ExecutionPipelineResult, String> {
    // ADR-030 / TD-158: feed the per-query I/O trace the native engine's compute
    // time so the always-on billing observer can emit KRU(tenant, "native") at
    // scope close. `record_compute_ms` no-ops outside an `io_trace` scope.
    let started = std::time::Instant::now();
    let result = NativeVolcanoEngine::execute_physical(physical, factory, controls)
        .await
        .map_err(|e| e.to_string());
    crate::observability::io_trace::record_compute_ms(
        "native",
        started.elapsed().as_millis() as u64,
    );
    result
}

/// Like [`execute_physical`] but meters every operator (EXPLAIN ANALYZE): returns the
/// result plus per-operator [`NodeMetric`]s in pre-order (aligned with
/// `explain_physical`'s line order). The executor tree is dropped before reading the
/// metrics so each `MeteredExec` has flushed its counters.
async fn execute_physical_metered<F: ReaderFactory>(
    physical: PhysicalPlan,
    factory: &F,
) -> Result<(ExecutionPipelineResult, Vec<NodeMetric>), String> {
    NativeVolcanoEngine::execute_physical_metered(physical, factory, ExecutionControls::default())
        .await
        .map_err(|e| e.to_string())
}

/// Lower + plan a SELECT over an already-prepared `SnapshotCatalog` to a Volcano
/// `PhysicalPlan`. The single planning path shared by execution
/// ([`try_run_select`]) and `EXPLAIN` ([`explain_select_route_with_catalog`]) so
/// the disclosed plan is exactly the one that runs. `None` = lowering declined
/// (fall through to legacy); `Some(Err)` = a real planning error.
fn plan_over_snapshot(
    sql: &str,
    snapshot: &SnapshotCatalog,
) -> Option<Result<PhysicalPlan, String>> {
    let logical = match lower_sql(sql, snapshot) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(
                target: "proximadb::pgwire::new_pipeline",
                "lower_sql declined `{sql}` over real data: {e}; falling through to legacy"
            );
            return None;
        }
    };
    // Per-table PK ordinals so the planner can rewrite PK-equality scans to
    // ScanAccess::PkLookup (single-column PK only — see PreparedTable).
    let pk_by_table: HashMap<String, Vec<usize>> = snapshot
        .tables
        .iter()
        .map(|(key, prepared)| (key.clone(), prepared.pk_columns.clone()))
        .collect();
    // TD-127: per-table secondary-indexed columns so the planner can rewrite a
    // non-PK equality / IN-list to ScanAccess::SecondaryLookup.
    let secondary_by_table: HashMap<String, Vec<String>> = snapshot
        .tables
        .iter()
        .map(|(key, prepared)| (key.clone(), prepared.secondary_columns.clone()))
        .collect();
    let resolver = SnapshotCapabilities {
        pk_by_table,
        secondary_by_table,
    };
    let planner = Planner::new(resolver);
    Some(planner.plan(logical).map_err(|e| format!("plan: {e}")))
}

// =========================================================================
// TD-OLAP-4 native SHADOW probe (engine dimension). Default-OFF, benchmark-only:
// run the SAME parquet SELECT on the native vectorized engine alongside DataFusion
// to record a `native-vectorized` compute sample in the io-trace. The native
// result is discarded (DataFusion's is authoritative) and all failures are
// swallowed, so the probe never changes the served result or fails a query.
// =========================================================================

/// Column names emitted by the plan's sole `Scan` node, or `None` if the plan has
/// zero or more than one scan (multi-table / no-table). Used to build the native
/// parquet reader's projection so it reads the same columns DataFusion does.
#[cfg(feature = "datafusion-integration")]
fn single_scan_columns(plan: &PhysicalPlan) -> Option<Vec<String>> {
    fn collect(plan: &PhysicalPlan, count: &mut usize, out: &mut Option<Vec<String>>) {
        match plan {
            PhysicalPlan::Scan { output_schema, .. } => {
                *count += 1;
                *out = Some(
                    output_schema
                        .columns
                        .iter()
                        .map(|c| c.name.clone())
                        .collect(),
                );
            }
            PhysicalPlan::Filter { input, .. }
            | PhysicalPlan::Project { input, .. }
            | PhysicalPlan::Aggregate { input, .. }
            | PhysicalPlan::Sort { input, .. }
            | PhysicalPlan::Limit { input, .. }
            | PhysicalPlan::Distinct { input, .. }
            | PhysicalPlan::AssertMaxOneRow { input } => collect(input, count, out),
            PhysicalPlan::Join { left, right, .. } => {
                collect(left, count, out);
                collect(right, count, out);
            }
            PhysicalPlan::Union { inputs, .. } => {
                for i in inputs {
                    collect(i, count, out);
                }
            }
            _ => {}
        }
    }
    let mut count = 0usize;
    let mut out = None;
    collect(plan, &mut count, &mut out);
    (count == 1).then_some(out).flatten()
}

/// Is this an unfiltered, ungrouped `COUNT(*)` (possibly under a `Limit`) directly
/// over a `Scan`? Then the answer is the sum of the parquet/PAX footer row counts —
/// a metadata read, no column scan. (`scan_cols` can't detect this: the relational
/// `Scan.output_schema` is full-width, so COUNT(*) would otherwise read all columns.)
#[cfg(feature = "datafusion-integration")]
fn is_pure_count_star(plan: &PhysicalPlan) -> bool {
    use proximadb_relational_algebra::AggregateExpr;
    match plan {
        // A `COUNT(*)` lowers to `Project(Aggregate(Scan))` (the Project names/orders
        // the output) — recurse through both Project and Limit to the Aggregate.
        PhysicalPlan::Limit { input, .. } | PhysicalPlan::Project { input, .. } => {
            is_pure_count_star(input)
        }
        PhysicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
            having,
            ..
        } => {
            group_by.is_empty()
                && having.is_none()
                && !aggregates.is_empty()
                && aggregates.iter().all(|a| {
                    matches!(
                        &a.agg,
                        AggregateExpr::Count {
                            arg: None,
                            distinct: false
                        }
                    )
                })
                // MUST be an UNFILTERED scan: a pushed-down `Scan.predicate`
                // (`WHERE AdvEngineID<>0`, `WHERE URL LIKE …`) means the count is
                // over the FILTERED rows — footer elision would ignore the filter
                // and return the wrong (whole-table) count.
                && matches!(input.as_ref(), PhysicalPlan::Scan { predicate: None, .. })
        }
        _ => false,
    }
}

/// TD-OLAP-4 metadata elision: if the plan is an UNGROUPED, UNFILTERED aggregate
/// over a `Scan` whose aggregates are all `COUNT(*)` / `COUNT(col)` / `MIN(col)` /
/// `MAX(col)` on NUMERIC columns, build the answer row directly from the parquet
/// FOOTER (row count + per-column min/max/null_count) — zero column I/O, flat cost
/// (matching DataFusion's `AggregateStatistics`). Returns `None` (→ scan) for
/// SUM/AVG/DISTINCT, non-column args, string columns (truncated stats), or any
/// column lacking full footer coverage.
#[cfg(feature = "datafusion-integration")]
fn elidable_aggregate_batch(
    plan: &PhysicalPlan,
    table: &crate::datafusion::engine_adapters::ObjectStoreParquetTable,
    file_schema: &arrow::datatypes::SchemaRef,
) -> Option<arrow_array::RecordBatch> {
    use arrow_array::{ArrayRef, Float64Array, Int64Array};
    use proximadb_relational_algebra::AggregateExpr;
    use proximadb_relational_types::Expr;

    fn find_agg(p: &PhysicalPlan) -> Option<&PhysicalPlan> {
        match p {
            PhysicalPlan::Project { input, .. } | PhysicalPlan::Limit { input, .. } => {
                find_agg(input)
            }
            PhysicalPlan::Aggregate { .. } => Some(p),
            _ => None,
        }
    }
    let PhysicalPlan::Aggregate {
        input,
        group_by,
        aggregates,
        having,
        ..
    } = find_agg(plan)?
    else {
        return None;
    };
    if !group_by.is_empty()
        || having.is_some()
        || !matches!(
            input.as_ref(),
            PhysicalPlan::Scan {
                predicate: None,
                ..
            }
        )
    {
        return None;
    }
    // Resolve a catalog column name to the FILE column (case-insensitive — CamelCase
    // DDL over a lowercased parquet) → (parquet-stats key, Arrow type).
    let resolve = |name: &str| -> Option<(String, arrow::datatypes::DataType)> {
        file_schema
            .fields()
            .iter()
            .find(|f| f.name().eq_ignore_ascii_case(name))
            .map(|f| (f.name().clone(), f.data_type().clone()))
    };
    let rows = table.estimated_rows()?;
    let mut fields: Vec<arrow::datatypes::Field> = Vec::with_capacity(aggregates.len());
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(aggregates.len());
    for na in aggregates {
        let (dt, arr): (arrow::datatypes::DataType, ArrayRef) = match &na.agg {
            AggregateExpr::Count {
                arg: None,
                distinct: false,
            } => (
                arrow::datatypes::DataType::Int64,
                std::sync::Arc::new(Int64Array::from(vec![rows as i64])),
            ),
            AggregateExpr::Count {
                arg: Some(Expr::Column(c)),
                distinct: false,
            } => {
                let (cn, _) = resolve(&c.name)?;
                let (_, _, nulls) = table.aggregate_numeric_bounds(&cn)?;
                (
                    arrow::datatypes::DataType::Int64,
                    std::sync::Arc::new(Int64Array::from(vec![rows.saturating_sub(nulls) as i64])),
                )
            }
            AggregateExpr::Min {
                arg: Expr::Column(c),
            }
            | AggregateExpr::Max {
                arg: Expr::Column(c),
            } => {
                let (cn, dt) = resolve(&c.name)?;
                let (lo, hi, _) = table.aggregate_numeric_bounds(&cn)?;
                let v = if matches!(na.agg, AggregateExpr::Max { .. }) {
                    hi
                } else {
                    lo
                };
                let arr = arrow::compute::cast(&Float64Array::from(vec![v]), &dt).ok()?;
                (dt, arr)
            }
            // SUM/AVG/DISTINCT/STRING_AGG/non-column → not footer-elidable.
            _ => return None,
        };
        fields.push(arrow::datatypes::Field::new(&na.name, dt, true));
        arrays.push(arr);
    }
    arrow_array::RecordBatch::try_new(
        std::sync::Arc::new(arrow::datatypes::Schema::new(fields)),
        arrays,
    )
    .ok()
}

/// Does any `Scan` in the plan carry a pushed-down `predicate`? Native's scan does
/// NOT apply `Scan.predicate` (no predicate pushdown yet), so it would count/aggregate
/// the UNFILTERED rows — a wrong result. Filtered scans route to DataFusion (which
/// applies the predicate + prunes row groups) until native gains predicate pushdown.
#[cfg(feature = "datafusion-integration")]
fn plan_scan_has_predicate(plan: &PhysicalPlan) -> bool {
    match plan {
        PhysicalPlan::Scan { predicate, .. } => predicate.is_some(),
        PhysicalPlan::Filter { input, .. }
        | PhysicalPlan::Project { input, .. }
        | PhysicalPlan::Aggregate { input, .. }
        | PhysicalPlan::Sort { input, .. }
        | PhysicalPlan::Limit { input, .. }
        | PhysicalPlan::Distinct { input, .. }
        | PhysicalPlan::AssertMaxOneRow { input } => plan_scan_has_predicate(input),
        PhysicalPlan::Join { left, right, .. } => {
            plan_scan_has_predicate(left) || plan_scan_has_predicate(right)
        }
        PhysicalPlan::Union { inputs, .. } => inputs.iter().any(plan_scan_has_predicate),
        _ => false,
    }
}

/// Does the plan contain an `Aggregate` with a non-empty `GROUP BY`? Native's
/// `HashAggregate` is in-memory with no spilling, so a high-cardinality group-by
/// (e.g. `GROUP BY UserID` over 100M rows) balloons the hash table to tens of GB
/// and OOMs. Until a bounded/spilling aggregate lands, the shadow declines grouped
/// aggregates — they route to DataFusion (its partitioned/spilling aggregation
/// handles them), which is itself the dispatch rule the trace confirms.
#[cfg(feature = "datafusion-integration")]
fn plan_has_grouped_aggregate(plan: &PhysicalPlan) -> bool {
    match plan {
        PhysicalPlan::Aggregate {
            input, group_by, ..
        } => !group_by.is_empty() || plan_has_grouped_aggregate(input),
        PhysicalPlan::Filter { input, .. }
        | PhysicalPlan::Project { input, .. }
        | PhysicalPlan::Sort { input, .. }
        | PhysicalPlan::Limit { input, .. }
        | PhysicalPlan::Distinct { input, .. }
        | PhysicalPlan::AssertMaxOneRow { input } => plan_has_grouped_aggregate(input),
        PhysicalPlan::Join { left, right, .. } => {
            plan_has_grouped_aggregate(left) || plan_has_grouped_aggregate(right)
        }
        PhysicalPlan::Union { inputs, .. } => inputs.iter().any(plan_has_grouped_aggregate),
        _ => false,
    }
}

/// Run the native vectorized engine over the same external parquet DataFusion just
/// served, purely to record a per-engine compute sample. Single-table parquet only
/// (MVP); declines silently on anything native can't yet serve.
#[cfg(feature = "datafusion-integration")]
async fn shadow_probe_native_parquet(
    sql: &str,
    snapshot: &SnapshotCatalog,
    parquet_loc_by_key: &HashMap<String, String>,
) {
    // Single-table parquet only — joins are a separately-gated native path.
    if parquet_loc_by_key.len() != 1 {
        return;
    }
    let Some(location) = parquet_loc_by_key.values().next() else {
        return;
    };
    let sqlp = &sql[..sql.len().min(70)];
    // Lower the SAME SQL to the relational physical plan (decline → skip).
    let physical = match plan_over_snapshot(sql, snapshot) {
        Some(Ok(p)) => p,
        _ => {
            tracing::debug!(target: "proximadb::native_shadow", "shadow SKIP plan-declined: {sqlp}");
            return;
        }
    };
    let Some(scan_cols) = single_scan_columns(&physical) else {
        tracing::debug!(target: "proximadb::native_shadow", "shadow SKIP not-single-scan: {sqlp}");
        return;
    };
    // Skip grouped aggregates: native's non-spilling HashAggregate OOMs on
    // high-cardinality GROUP BY at scale. These route to DataFusion.
    if plan_has_grouped_aggregate(&physical) {
        tracing::debug!(target: "proximadb::native_shadow", "shadow SKIP grouped-aggregate: {sqlp}");
        return;
    }
    // Skip filtered scans: native does not apply a pushed-down `Scan.predicate`,
    // so it would (wrongly) aggregate the unfiltered rows. Route to DataFusion.
    if plan_scan_has_predicate(&physical) {
        tracing::debug!(target: "proximadb::native_shadow", "shadow SKIP filtered-scan (no native predicate pushdown): {sqlp}");
        return;
    }
    tracing::debug!(target: "proximadb::native_shadow", "shadow RUN native: {sqlp}");
    // Open the same parquet DataFusion read (table-OPEN cache is warm from its run).
    let table = match crate::datafusion::engine_adapters::ObjectStoreParquetTable::open(location)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(target: "proximadb::native_shadow", %e, "shadow: table open declined");
            return;
        }
    };
    let (store, files, file_schema) = table.native_scan_inputs();
    // Metadata elision (TD-OLAP-4): unfiltered MIN/MAX/COUNT answered from the
    // parquet FOOTER — zero column I/O, flat cost. The single biggest native win at
    // scale (q07 MIN/MAX: a full-column vectorized scan → a footer read). Emits the
    // pre-computed row and bypasses the scan + HashAggregate entirely.
    if let Some(batch) = elidable_aggregate_batch(&physical, &table, &file_schema) {
        tracing::debug!(target: "proximadb::native_shadow", "shadow ELIDE (footer stats): {sqlp}");
        let src = Box::new(
            crate::query::execution::native_parquet_scan::StatsAggregateOperator::new(batch),
        );
        let _ = crate::query::execution::native_engine::run_native_source_only(src).await;
        return;
    }
    // Unfiltered COUNT(*): parquet/PAX carry the row count in the FOOTER, so elide
    // the scan entirely — read footers only, emit the count (a metadata op, not a
    // scan). Detected from the PLAN, not `scan_cols`: the relational Scan schema is
    // full-width, so COUNT(*) would otherwise read all 105 columns.
    let source: Box<dyn proximadb_execution_contracts::ExecutionOperator> =
        if is_pure_count_star(&physical) {
            Box::new(
                crate::query::execution::native_parquet_scan::ParquetScanOperator::new_count_only(
                    store, files,
                ),
            )
        } else {
            // The relational Scan schema is full-width (no projection pushdown yet), so
            // `scan_cols` reads every column the scan declares. Native has neither
            // projection nor predicate pushdown, so a wide scan reads all columns
            // row-wise — impractically slow at scale. Cap it: route wide scans to
            // DataFusion (which prunes columns + row groups). This IS the dispatch rule
            // the trace confirms; narrow scans still sample native. (Lifting the cap
            // needs projection/predicate pushdown — the medium-tier native enabler.)
            const NATIVE_SCAN_WIDTH_CAP: usize = 8;
            if scan_cols.len() > NATIVE_SCAN_WIDTH_CAP {
                tracing::debug!(
                    target: "proximadb::native_shadow",
                    cols = scan_cols.len(),
                    "shadow SKIP wide-scan (no pushdown): {sqlp}"
                );
                return;
            }
            // Map the plan's Scan columns → parquet leaf indices by name; build the
            // native scan output schema from the FILE fields (exact Arrow types). Sort
            // ascending so the reader's file-order `ProjectionMask::roots` matches the
            // operator schema — when the plan preserves declared column order (the
            // common case) native aligns; otherwise the native lowering declines and no
            // sample is recorded.
            let mut paired: Vec<(usize, arrow::datatypes::Field)> =
                Vec::with_capacity(scan_cols.len());
            for name in &scan_cols {
                // Match case-insensitively: the catalog/DDL may declare CamelCase
                // columns over a lowercased parquet (e.g. ClickBench). Ordinals still
                // align positionally — native ops reference columns by ordinal, not name.
                let Some((idx, field)) = file_schema
                    .fields()
                    .iter()
                    .enumerate()
                    .find(|(_, f)| f.name().eq_ignore_ascii_case(name))
                else {
                    return; // column absent from file (schema drift) → never risk a sample
                };
                paired.push((idx, field.as_ref().clone()));
            }
            paired.sort_by_key(|(i, _)| *i);
            let projection: Vec<usize> = paired.iter().map(|(i, _)| *i).collect();
            let out_schema = Arc::new(arrow::datatypes::Schema::new(
                paired.into_iter().map(|(_, f)| f).collect::<Vec<_>>(),
            ));
            Box::new(
                crate::query::execution::native_parquet_scan::ParquetScanOperator::new(
                    store,
                    files,
                    Some(projection),
                    out_schema,
                ),
            )
        };
    match crate::query::execution::native_engine::run_native_over_parquet(&physical, source).await {
        Ok(Some(_)) => {
            tracing::debug!(target: "proximadb::native_shadow", "shadow: native-vectorized sample recorded")
        }
        Ok(None) => {
            tracing::debug!(target: "proximadb::native_shadow", "shadow: native declined (shape/align)")
        }
        Err(e) => {
            tracing::debug!(target: "proximadb::native_shadow", %e, "shadow: native errored")
        }
    }
}

// =========================================================================
// P1: DataFusion OLAP route over Parquet-backed tables (course-correction §6 P1)
// Gated on `datafusion-integration`; when the feature is off none of this is
// compiled and `try_run_select` behaves exactly as before (Volcano only).
// =========================================================================

/// If `schema` is backed by an external Parquet file (object store / open format),
/// return its location URI. Detects the `CatalogStorageLayout` the route branches on:
/// `physical_format == Parquet` + a read-federation/external authority + a location.
#[cfg(feature = "datafusion-integration")]
fn catalog_table_is_parquet_backed(
    schema: &proximadb_catalog::CatalogTableSchema,
) -> Option<String> {
    use proximadb_catalog::{CatalogAuthorityMode, CatalogPhysicalFormat};
    schema.storage_layouts.iter().find_map(|layout| {
        let format_ok = matches!(layout.physical_format, CatalogPhysicalFormat::Parquet);
        let authority_ok = matches!(
            layout.authority,
            CatalogAuthorityMode::FederatedRead
                | CatalogAuthorityMode::ExternalAuthoritative
                | CatalogAuthorityMode::ImportedSnapshot
                | CatalogAuthorityMode::ExportedPublication
                | CatalogAuthorityMode::ProjectionPublication
        );
        if format_ok && authority_ok {
            layout.location.clone()
        } else {
            None
        }
    })
}

/// Master switch for the ADR-025 OLAP read-merge. **Default-ON** (ADR-025 PR3):
/// once a table is materialized, OLAP `SELECT`s reconcile its cold Parquet base
/// with the authoritative post-snapshot WAL delta so `DELETE`/`UPDATE`/`INSERT`
/// written after `MATERIALIZE` are reflected. Read-mostly tables (nothing changed
/// since the snapshot) take the empty-delta fast path — a bare Parquet read — so
/// default-ON is correctness- and cost-neutral for them.
///
/// Kill-switch: set `PROXIMADB_OLAP_DELTA_MERGE` to a falsy token
/// (`0`/`false`/`off`/`no`, case-insensitive) to force the legacy bare-Parquet
/// read engine-wide; individual tables can still opt back in via the
/// `olap_delta_merge` storage-layout property. Any other value — or leaving it
/// unset — keeps the merge on.
#[cfg(feature = "datafusion-integration")]
fn olap_delta_merge_enabled() -> bool {
    olap_delta_merge_on(std::env::var("PROXIMADB_OLAP_DELTA_MERGE").ok().as_deref())
}

/// Pure kill-switch policy for [`olap_delta_merge_enabled`], split out so it is
/// unit-testable without mutating the process environment: `None` (unset) or any
/// non-falsy value ⇒ merge ON; only an explicit falsy token turns it OFF.
#[cfg(feature = "datafusion-integration")]
fn olap_delta_merge_on(var: Option<&str>) -> bool {
    match var {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        None => true,
    }
}

/// Resolve the ADR-025 read-merge parameters for a parquet-backed table, or `None`
/// when it is ineligible (no single-column PK, no recorded `snapshot_lsn`, or not
/// opted in). Ineligible tables fall back to the bare Parquet read.
#[cfg(feature = "datafusion-integration")]
fn olap_delta_table_params(
    schema: &proximadb_catalog::CatalogTableSchema,
) -> Option<crate::query::execution::olap_delta_merge::OlapDeltaTable> {
    use proximadb_catalog::{CatalogAuthorityMode, CatalogPhysicalFormat};
    // Single-column PK required: the merge recomputes each base row's canonical oid
    // from its PK column. Keyless heaps fall back to the bare Parquet read.
    let pk_column = match schema.primary_key.as_slice() {
        [pk] => pk.clone(),
        _ => return None,
    };
    let layout = schema.storage_layouts.iter().find(|l| {
        matches!(l.physical_format, CatalogPhysicalFormat::Parquet)
            && matches!(
                l.authority,
                CatalogAuthorityMode::FederatedRead
                    | CatalogAuthorityMode::ExternalAuthoritative
                    | CatalogAuthorityMode::ImportedSnapshot
                    | CatalogAuthorityMode::ExportedPublication
                    | CatalogAuthorityMode::ProjectionPublication
            )
    })?;
    let opted_in = olap_delta_merge_enabled()
        || matches!(
            layout
                .properties
                .get("olap_delta_merge")
                .map(String::as_str),
            Some("1" | "true" | "on" | "yes")
        );
    if !opted_in {
        return None;
    }
    let snapshot_lsn: u64 = layout.properties.get("snapshot_lsn")?.parse().ok()?;
    Some(crate::query::execution::olap_delta_merge::OlapDeltaTable {
        snapshot_lsn,
        pk_column,
    })
}

/// Sync `CatalogLookup` wrapper for the process-wide engine.
struct EngineCatalog(Arc<InMemoryRelationalEngine>);

impl CatalogLookup for EngineCatalog {
    fn lookup_table(&self, name: &str) -> Option<RelationalSchema> {
        self.0.schema_of(name)
    }
}

/// Per-table schema/PK cache for a query, pre-resolved from xCatalog.
#[derive(Clone, Debug)]
struct PreparedTable {
    table_name: String,
    schema: RelationalSchema,
    pk_columns: Vec<usize>,
    /// TD-127: names of this table's single-column non-PK secondary indexes
    /// (from `schema_secondary_index_columns`). Threaded into the planner's
    /// `CapabilityResolver::secondary_index_columns` so an equality / IN on
    /// one of these columns can rewrite to `ScanAccess::SecondaryLookup`.
    secondary_columns: Vec<String>,
}

impl PreparedTable {
    fn from_catalog(name: &str, schema: &proximadb_catalog::CatalogTableSchema) -> Self {
        let cols: Vec<ColumnInfo> = schema
            .columns
            .iter()
            .map(|c| ColumnInfo::new(&c.name, c.data_type.clone(), c.nullable))
            .collect();
        let mut pk = Vec::new();
        for (i, c) in schema.columns.iter().enumerate() {
            if schema.primary_key.contains(&c.name) {
                pk.push(i);
            }
        }
        Self {
            table_name: name.to_string(),
            schema: RelationalSchema::new(cols),
            pk_columns: pk,
            secondary_columns: crate::services::record_store::schema_secondary_index_columns(
                schema,
            ),
        }
    }
}

struct SnapshotCatalog {
    dml: Arc<DmlService>,
    tables: HashMap<String, PreparedTable>,
    /// TD-064: connection tenant scope threaded into every snapshot reader so
    /// relational scans/point-lookups read the tenant's record partition.
    tenant: Option<TenantContext>,
}

impl CatalogLookup for SnapshotCatalog {
    fn lookup_table(&self, name: &str) -> Option<RelationalSchema> {
        self.tables
            .get(&normalize_table_key(name))
            .map(|t| t.schema.clone())
    }
}

impl ReaderFactory for SnapshotCatalog {
    fn open_reader(&self, table: &TableId) -> Result<Box<dyn RelationalReader>, ExecError> {
        let key = normalize_table_key(&table.name);
        let prepared = self
            .tables
            .get(&key)
            .ok_or_else(|| ExecError::Internal(format!("table not snapshotted: {}", table.name)))?;
        Ok(Box::new(DmlTableReader {
            dml: self.dml.clone(),
            table_name: prepared.table_name.clone(),
            full_schema: prepared.schema.clone(),
            pk_columns: prepared.pk_columns.clone(),
            tenant: self.tenant.clone(),
            open_state: None,
        }))
    }
}

/// `CapabilityResolver` for the real-data pipeline; mirrors the canonical
/// [`StaticCapabilities`]. Supplies per-table single-column PK ordinals so the
/// planner can rewrite a PK-equality scan to `ScanAccess::PkLookup`.
///
/// Route metadata for the PK-lookup it enables: `workload_profile=Oltp`,
/// `authority_mode=ProximaAuthoritative`, `policy_boundary=engine-enforced`,
/// `freshness=latest-committed` (NativeRecord — not an external open-format
/// route). ADR-018 Phase 2 (pgwire SQL parity); TD-076.
struct SnapshotCapabilities {
    pk_by_table: HashMap<String, Vec<usize>>,
    /// TD-127: per-table secondary-indexed column names. Empty / absent =
    /// no secondary index → planner keeps a full scan + filter.
    secondary_by_table: HashMap<String, Vec<String>>,
}

impl CapabilityResolver for SnapshotCapabilities {
    fn capabilities(&self, _table: &TableId) -> ReaderCapabilities {
        // Unchanged pushdown behavior (projection/predicate). PK-lookup,
        // PK-batch (TD-128) and secondary-lookup (TD-127) are advertised here
        // but gated per-table by `primary_key` / `secondary_index_columns`
        // below, so a table without the relevant index keeps a full scan.
        ReaderCapabilities::full()
            .with_pk_lookup_batch(true)
            .with_secondary_lookup(true)
    }

    fn primary_key(&self, table: &TableId) -> Vec<usize> {
        self.pk_by_table
            .get(&normalize_table_key(&table.name))
            .cloned()
            .unwrap_or_default()
    }

    fn secondary_index_columns(&self, table: &TableId) -> Vec<String> {
        self.secondary_by_table
            .get(&normalize_table_key(&table.name))
            .cloned()
            .unwrap_or_default()
    }
}

// =========================================================================
// DmlTableReader — lazy reader that pushes projection/predicate/limit into
// the DmlService scan (only matching, projected rows are materialized).
// =========================================================================

/// Per-scan state captured at `open`: the projected output schema + the rows
/// already fetched (predicate-filtered + projected + limited at the store).
struct ReaderOpenState {
    output_schema: RelationalSchema,
    rows: Vec<RelationalRow>,
    cursor: usize,
}

struct DmlTableReader {
    dml: Arc<DmlService>,
    table_name: String,
    full_schema: RelationalSchema,
    /// Single-column PK ordinal(s) for the PK-lookup arity check (empty when the
    /// planner won't pick PkLookup for this table).
    pk_columns: Vec<usize>,
    /// TD-064: connection tenant scope for this reader's record partition.
    tenant: Option<TenantContext>,
    open_state: Option<ReaderOpenState>,
}

impl DmlTableReader {
    /// Resolve the projected output schema from `ScanContext::projection`
    /// (`None` = full schema), mirroring the `VecReader` contract.
    fn resolve_output_schema(
        &self,
        projection: &Option<Vec<String>>,
    ) -> Result<RelationalSchema, ReaderError> {
        let Some(names) = projection else {
            return Ok(self.full_schema.clone());
        };
        let mut cols = Vec::with_capacity(names.len());
        for n in names {
            let (_idx, info) = self
                .full_schema
                .column_by_name(n)
                .ok_or_else(|| ReaderError::InvalidProjection(n.clone()))?;
            cols.push(info.clone());
        }
        Ok(RelationalSchema::new(cols))
    }
}

#[async_trait]
impl RelationalReader for DmlTableReader {
    fn name(&self) -> &'static str {
        "dml_relational"
    }

    fn capabilities(&self) -> ReaderCapabilities {
        // Honors projection + predicate + limit + single-column PK lookup
        // (see `lookup_pk`), discrete PK batch (TD-128, `lookup_pk_batch`) and
        // OLTP secondary lookup (TD-127, `lookup_secondary`). The planner gates
        // each per-table via the resolver's `primary_key` /
        // `secondary_index_columns`, so these flags are informational here.
        ReaderCapabilities::full()
            .with_pk_lookup(true)
            .with_pk_lookup_batch(true)
            .with_secondary_lookup(true)
    }

    fn schema(&self) -> &RelationalSchema {
        match &self.open_state {
            Some(state) => &state.output_schema,
            None => &self.full_schema,
        }
    }

    async fn open(&mut self, ctx: &ScanContext) -> Result<(), ReaderError> {
        let output_schema = self.resolve_output_schema(&ctx.projection)?;
        // Predicate ordinals bind to the FULL table schema; evaluate before
        // projection (same contract as VecReader). Type-check up-front.
        if let Some(pred) = &ctx.predicate {
            pred.type_check(&self.full_schema)
                .map_err(ReaderError::PredicateEval)?;
        }
        let predicate: Option<Expr> = ctx.predicate.clone();
        // ADR-043 Invariant 1 — fail-loud predicate: evaluate against the real
        // builtin registry and PROPAGATE any eval error (unknown function, cast,
        // arithmetic, type) instead of coercing it to `false`. The empty registry
        // (`NoFunctions`) and the error-swallowing `matches!(…, Ok(Boolean(true)))`
        // here were a silent-data-loss seam: a pushed-down predicate the native
        // engine could not evaluate dropped every row and presented as a clean empty
        // result. `scan_table_relational` now surfaces the captured error; a query
        // native cannot serve fails loudly and the OLAP route answers it (ADR-039).
        let row_pred = move |full_row: &[ProximaValue]| -> Result<bool, ExprError> {
            match &predicate {
                Some(expr) => match expr.eval(&full_row.to_vec(), builtins())? {
                    ProximaValue::Boolean(b) => Ok(b),
                    // SQL three-valued logic: NULL (and any non-boolean predicate
                    // result) is not TRUE, so the row is excluded — this is a
                    // *value*, not an error, and is the correct filter semantics.
                    _ => Ok(false),
                },
                None => Ok(true),
            }
        };
        let row_pred_ref: Option<
            &(dyn Fn(&[ProximaValue]) -> Result<bool, ExprError> + Send + Sync),
        > = if ctx.predicate.is_some() {
            Some(&row_pred)
        } else {
            None
        };
        let limit = ctx.limit.map(|l| l as usize);
        let (_schema, rows) = self
            .dml
            .scan_table_relational(
                &self.table_name,
                ctx.projection.as_deref(),
                row_pred_ref,
                limit,
                self.tenant.as_ref(),
            )
            .await
            .map_err(|e| ReaderError::Storage(e.to_string()))?;
        self.open_state = Some(ReaderOpenState {
            output_schema,
            rows,
            cursor: 0,
        });
        Ok(())
    }

    async fn next_row(&mut self) -> Result<Option<RelationalRow>, ReaderError> {
        let state = self.open_state.as_mut().ok_or(ReaderError::NotOpen)?;
        if state.cursor >= state.rows.len() {
            return Ok(None);
        }
        let row = state.rows[state.cursor].clone();
        state.cursor += 1;
        Ok(Some(row))
    }

    /// OLTP point-read fast path: resolve a single row by primary key via
    /// `DmlService::point_lookup_relational`. Returns the FULL row (the executor
    /// re-applies projection from `schema()`, which is the full schema on the
    /// PkLookup path — `open` is not called). Single-column PK only.
    /// ADR-018 Phase 2 (pgwire SQL parity); TD-076.
    async fn lookup_pk(&self, key: &[ProximaValue]) -> Result<Option<RelationalRow>, ReaderError> {
        if key.len() != 1 {
            return Err(ReaderError::PkArityMismatch {
                expected: self.pk_columns.len().max(1),
                actual: key.len(),
            });
        }
        // Convert the PK value to the stored `record.oid` string (the same
        // canonical text form used for int/string/uuid keys). SQL NULL → no row.
        let Some(key_str) = text_encode(&key[0]) else {
            return Ok(None);
        };
        tracing::debug!(
            target: "proximadb::pgwire::new_pipeline",
            access_path = "PkLookup",
            table = %self.table_name,
            "relational PK point lookup"
        );
        self.dml
            .point_lookup_relational(&self.table_name, &key_str, self.tenant.as_ref())
            .await
            .map_err(|e| ReaderError::Storage(e.to_string()))
    }

    /// TD-128: discrete multi-key OLTP point-read (`PK IN (...)` / eq-OR).
    /// Forwards to [`DmlService::point_lookup_batch_relational`], which reuses
    /// the same `get_by_key` path (dead-record-filtered) as the single-key
    /// fast-path. Single-column PK only; SQL NULL keys can't match a stored oid
    /// and are skipped. Returns the FULL candidate rows (the executor narrows).
    async fn lookup_pk_batch(
        &self,
        keys: &[Vec<ProximaValue>],
    ) -> Result<Vec<RelationalRow>, ReaderError> {
        let mut key_strs = Vec::with_capacity(keys.len());
        for key in keys {
            if key.len() != 1 {
                return Err(ReaderError::PkArityMismatch {
                    expected: 1,
                    actual: key.len(),
                });
            }
            if let Some(s) = text_encode(&key[0]) {
                key_strs.push(s);
            }
        }
        tracing::debug!(
            target: "proximadb::pgwire::new_pipeline",
            access_path = "PkLookupBatch",
            table = %self.table_name,
            keys = key_strs.len(),
            "relational PK batch point lookup"
        );
        self.dml
            .point_lookup_batch_relational(&self.table_name, &key_strs, self.tenant.as_ref())
            .await
            .map_err(|e| ReaderError::Storage(e.to_string()))
    }

    /// TD-127: OLTP secondary-index point-read. Forwards to
    /// [`DmlService::secondary_lookup_relational`], which probes the store's
    /// single-column hash index and re-materializes each live candidate via
    /// `get_by_key` (dead-record-filtered). `Ok(None)` (no built index, or all
    /// probe values NULL) → the executor falls back to a full scan + the
    /// residual filter; `Ok(Some(rows))` → FULL candidate rows (the executor
    /// narrows and the residual filter re-checks — the index only narrows).
    async fn lookup_secondary(
        &self,
        column: &str,
        values: &[ProximaValue],
    ) -> Result<Option<Vec<RelationalRow>>, ReaderError> {
        let mut value_strs = Vec::with_capacity(values.len());
        for v in values {
            if let Some(s) = text_encode(v) {
                value_strs.push(s);
            }
        }
        if value_strs.is_empty() {
            // No probe-able (non-NULL) values — let the executor scan + filter.
            return Ok(None);
        }
        tracing::debug!(
            target: "proximadb::pgwire::new_pipeline",
            access_path = "SecondaryLookup",
            table = %self.table_name,
            column = %column,
            "relational secondary index point lookup"
        );
        self.dml
            .secondary_lookup_relational(
                &self.table_name,
                column,
                &value_strs,
                self.tenant.as_ref(),
            )
            .await
            .map_err(|e| ReaderError::Storage(e.to_string()))
    }

    async fn close(&mut self) -> Result<(), ReaderError> {
        self.open_state = None;
        Ok(())
    }
}

// =========================================================================
// PgType / text encoding helpers
// =========================================================================

/// Map a [`ProximaType`] to the closest [`PgType`] for
/// `RowDescription` emission. Unknowns map to `Text` so clients
/// can still display values.
pub fn pg_type_for(t: &ProximaType) -> PgType {
    match t {
        ProximaType::Boolean => PgType::Bool,
        ProximaType::Int8 | ProximaType::Int16 => PgType::Int2,
        ProximaType::Int32 => PgType::Int4,
        ProximaType::Int64 => PgType::Int8,
        ProximaType::UInt8 | ProximaType::UInt16 => PgType::Int4,
        ProximaType::UInt32 | ProximaType::UInt64 => PgType::Int8,
        ProximaType::Float16 | ProximaType::Float32 => PgType::Float4,
        ProximaType::Float64 => PgType::Float8,
        ProximaType::String | ProximaType::Symbol => PgType::Text,
        ProximaType::Binary => PgType::Bytea,
        ProximaType::Date => PgType::Date,
        ProximaType::Timestamp(_) => PgType::Timestamp,
        ProximaType::TimestampTz(_) => PgType::Timestamptz,
        ProximaType::Json => PgType::Json,
        ProximaType::Jsonb => PgType::Jsonb,
        ProximaType::Uuid | ProximaType::ULID => PgType::Uuid,
        _ => PgType::Text,
    }
}

/// Text-encode a [`ProximaValue`] for the pgwire text format.
/// Returns `None` for SQL `NULL` (the caller emits the pgwire
/// null sentinel `-1` length).
pub fn text_encode(v: &ProximaValue) -> Option<String> {
    use ProximaValue as V;
    Some(match v {
        V::Null => return None,
        V::Boolean(b) => (if *b { "t" } else { "f" }).into(),
        V::Int8(x) => x.to_string(),
        V::Int16(x) => x.to_string(),
        V::Int32(x) => x.to_string(),
        V::Int64(x) => x.to_string(),
        V::UInt8(x) => x.to_string(),
        V::UInt16(x) => x.to_string(),
        V::UInt32(x) => x.to_string(),
        V::UInt64(x) => x.to_string(),
        V::Float16(x) | V::Float32(x) => x.to_string(),
        V::Float64(x) => x.to_string(),
        V::Decimal(s) => s.clone(),
        V::String(s) | V::Symbol(s) => s.clone(),
        V::Binary(b) | V::BinaryVector(b) => {
            let mut s = String::with_capacity(2 + b.len() * 2);
            s.push_str("\\x");
            for byte in b {
                s.push_str(&format!("{:02x}", byte));
            }
            s
        }
        V::Date(d) => d.to_string(),
        V::Time(t, _) | V::Timestamp(t, _) | V::TimestampTz(t, _) => t.to_string(),
        V::Uuid(u) | V::ULID(u) => format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            u[0],
            u[1],
            u[2],
            u[3],
            u[4],
            u[5],
            u[6],
            u[7],
            u[8],
            u[9],
            u[10],
            u[11],
            u[12],
            u[13],
            u[14],
            u[15]
        ),
        V::Json(j) | V::Jsonb(j) => j.to_string(),
        V::Array(values) => {
            // Element NULL renders as the empty field in a Postgres array
            // literal (same convention as the legacy simple-query encoder).
            let parts = values
                .iter()
                .map(text_encode)
                .map(Option::unwrap_or_default)
                .collect::<Vec<_>>();
            format!("{{{}}}", parts.join(","))
        }
        V::Map(value) | V::Struct(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
        }
        V::DenseVector(values) => {
            let parts = values.iter().map(ToString::to_string).collect::<Vec<_>>();
            format!("[{}]", parts.join(","))
        }
        V::SparseVector { indices, values } => serde_json::json!({
            "indices": indices,
            "values": values,
        })
        .to_string(),
    })
}

// =========================================================================
// Read-route EXPLAIN surface (course-correction §5 / ADR-004)
// =========================================================================

/// Compute the read-route decision for a `SELECT` without executing it.
///
/// Returns `None` when `sql` is not a single relational query (so the caller has
/// no route to report). Catalog-free in P0 — routes on query shape only via the
/// canonical [`crate::query::compute_scheduler::ComputeScheduler`]; P1 adds the
/// Parquet-backed signal that flips the OLAP arm to DataFusion.
pub fn classify_select_route(
    sql: &str,
) -> Option<crate::query::compute_scheduler::SelectRouteDecision> {
    let statements = Parser::parse_sql(&GenericDialect {}, sql).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    let engages = query_engages_relational_engine(query);
    Some(
        crate::query::compute_scheduler::ComputeScheduler::new().route_select_advised(
            crate::query::compute_scheduler::QueryShape {
                engages_relational: engages,
                parquet_backed: false,
                cardinality: query_cardinality_hint(query),
                ..Default::default()
            },
            Some(&crate::query::route_cost_model::GLOBAL_ROUTE_COST_MODEL),
        ),
    )
}

/// Serializable read-route explanation for `EXPLAIN SELECT` (JSON over Jsonb).
///
/// Field vocabulary mirrors `table_write_plan::RouteDecisionMetadata` so read and
/// write EXPLAIN share one contract (ADR-004 unified EXPLAIN).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SelectRouteExplanation {
    /// Selected physical engine label (e.g. `Native(Volcano)`, `DataFusionLocal`).
    pub compute_route: String,
    /// Workload classification (`Oltp`/`Olap`).
    pub workload_profile: String,
    /// Control-plane routing decision — no durable authority is moved.
    pub authority_mode: String,
    /// Routing granularity — one engine per query plan, never per row.
    pub policy_boundary: String,
    /// Freshness guarantee of the selected route.
    pub freshness_sla: String,
    /// Human-readable reason for the choice.
    pub reason: String,
    /// Typed read-route contract that future split-aware DataFusion/Ballista
    /// execution will consume. Kept nested so existing top-level fields remain
    /// stable while ADR-004 diagnostics converge on `RoutedReadPlan`.
    pub read_route: crate::query::read_route::ReadRouteExplanation,
    /// Structural disclosure of the planned physical plan (one string per
    /// operator, indented), when the query engages the native (Volcano) PATH B
    /// engine. `None` for simple/legacy SELECTs and non-native routes. No cost
    /// estimates — the planner is rule-based (ADR-004; CBO is a follow-up).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_plan: Option<Vec<String>>,
    /// EXPLAIN ANALYZE only: rows the executed plan actually returned. `None` for
    /// plain EXPLAIN (the query is not executed) and non-native routes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_rows: Option<u64>,
    /// EXPLAIN ANALYZE only: wall-clock microseconds to execute the whole plan
    /// (build + open + drain). `None` for plain EXPLAIN. Whole-query granularity —
    /// per-operator timing is a follow-up (no Volcano instrumentation yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_elapsed_us: Option<u64>,
    /// ADR-004 / TD-174 (gap #4): stats-backed estimated selectivity of the
    /// `WHERE` clause — the product of per-`col = ?` equality selectivities
    /// (`1/ndistinct` from the resident HLL distinct count, ADR-037 producer).
    /// A 0..1 fraction the AnvaiOps cost consumer prices. `None` (omitted) when
    /// the query has no equality predicate, targets multiple tables, or the
    /// collection has no resident statistics yet — correct-or-absent, never a
    /// guessed value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_selectivity: Option<f64>,
    /// Estimated surviving rows = `record_count × estimated_selectivity`, when
    /// both the selectivity and the resident `record_count` are known. `None`
    /// (omitted) otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_rows: Option<u64>,
}

/// Build the `EXPLAIN SELECT` payload. `Err` if `sql` is not a routable single
/// relational query.
/// Render a route decision to the serializable EXPLAIN payload (single source so the
/// catalog-free and catalog-aware EXPLAIN variants stay consistent).
fn decision_to_explanation(
    decision: &crate::query::compute_scheduler::SelectRouteDecision,
) -> SelectRouteExplanation {
    let read_route = decision.routed_read_plan().route_explanation();
    SelectRouteExplanation {
        compute_route: decision.compute_route_label(),
        workload_profile: format!("{:?}", decision.workload_profile),
        authority_mode: read_route.authority_mode.clone(),
        policy_boundary: read_route.policy_boundary.clone(),
        freshness_sla: read_route.freshness_sla.clone(),
        reason: decision.reason.clone(),
        read_route,
        // Populated by the catalog-aware EXPLAIN once the plan is built; the
        // catalog-free route disclosure has no plan to render. ANALYZE metrics are
        // filled only when the catalog-aware ANALYZE path actually executes.
        physical_plan: None,
        execution_rows: None,
        execution_elapsed_us: None,
        estimated_selectivity: None,
        estimated_rows: None,
    }
}

/// Collect the LHS column names of top-level `col = <literal>` equality predicates
/// in a single-table SELECT's `WHERE` clause (AND-chained). Non-equality
/// predicates, OR branches, and `col = col` (no literal) are ignored — only the
/// shapes statistics can estimate. Returns an empty Vec for anything else.
fn equality_predicate_columns(query: &SqlQuery) -> Vec<String> {
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Vec::new();
    };
    let Some(selection) = select.selection.as_ref() else {
        return Vec::new();
    };
    let mut cols = Vec::new();
    collect_eq_columns(selection, &mut cols);
    cols
}

fn collect_eq_columns(expr: &SqlExpr, out: &mut Vec<String>) {
    match expr {
        SqlExpr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            collect_eq_columns(left, out);
            collect_eq_columns(right, out);
        }
        SqlExpr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } => {
            // `col = <literal>`: a bare identifier on one side, a literal value on
            // the other (either order). `col = col` has no literal → not estimable.
            if let (Some(col), true) = (ident_name(left), is_literal(right)) {
                out.push(col);
            } else if let (Some(col), true) = (ident_name(right), is_literal(left)) {
                out.push(col);
            }
        }
        _ => {}
    }
}

fn ident_name(expr: &SqlExpr) -> Option<String> {
    match expr {
        SqlExpr::Identifier(id) => Some(id.value.clone()),
        // `t.col` → the bare column name (stats are keyed by column).
        SqlExpr::CompoundIdentifier(parts) => parts.last().map(|p| p.value.clone()),
        _ => None,
    }
}

fn is_literal(expr: &SqlExpr) -> bool {
    matches!(expr, SqlExpr::Value(_))
}

/// Stats-backed selectivity + row estimate for a single-table SELECT's equality
/// `WHERE` predicates. `eq_sel(field) -> Option<f64>` returns the per-field
/// equality selectivity (DI seam for testing; production passes the registry).
/// Returns `(selectivity, estimated_rows)` only when at least one predicate has a
/// resident statistic — else `None` (caller leaves EXPLAIN unchanged). Multiple
/// predicates multiply (independence assumption, standard CBO).
fn stats_filter_estimate(
    cols: &[String],
    record_count: Option<u64>,
    eq_sel: impl Fn(&str) -> Option<f64>,
) -> Option<(f64, Option<u64>)> {
    let mut selectivity = 1.0_f64;
    let mut any = false;
    for col in cols {
        if let Some(s) = eq_sel(col) {
            selectivity *= s;
            any = true;
        }
    }
    if !any {
        return None;
    }
    let rows = record_count.map(|n| (n as f64 * selectivity).ceil() as u64);
    Some((selectivity, rows))
}

/// Prepare an engaging SELECT for EXPLAIN (cold path): resolve table schemas, build
/// the snapshot, then lower + plan via the SAME [`plan_over_snapshot`] execution
/// uses. Returns the snapshot+plan so EXPLAIN can render the tree and (for ANALYZE)
/// execute it via [`execute_physical`]. `None` when a schema can't be resolved,
/// lowering declines, or planning errors — EXPLAIN then falls back to route-only.
async fn prepare_select_plan(
    sql: &str,
    query: &SqlQuery,
    dml: &Arc<DmlService>,
    tenant: Option<&str>,
) -> Option<(SnapshotCatalog, PhysicalPlan)> {
    let mut names = Vec::new();
    collect_table_names(query, &mut names);
    let mut tables: HashMap<String, PreparedTable> = HashMap::new();
    for raw in &names {
        let key = normalize_table_key(raw);
        if tables.contains_key(&key) {
            continue;
        }
        // Resolve under the connection tenant so EXPLAIN finds the tenant's
        // schema row (and, for ANALYZE, reads the tenant's partition).
        let schema = dml.resolve_relational_schema(raw, tenant).await.ok()?;
        tables.insert(key, PreparedTable::from_catalog(raw, &schema));
    }
    let snapshot = SnapshotCatalog {
        dml: dml.clone(),
        tables,
        tenant: tenant
            .filter(|t| !t.is_empty())
            .map(TenantContext::for_tenant_id),
    };
    match plan_over_snapshot(sql, &snapshot)? {
        Ok(physical) => Some((snapshot, physical)),
        Err(_) => None,
    }
}

/// Catalog-free `EXPLAIN SELECT` route (shape only). Reports the Volcano/Native route;
/// use [`explain_select_route_with_catalog`] when a `DmlService` is available so
/// Parquet-backed tables disclose the DataFusion route they actually take.
pub fn explain_select_route(sql: &str) -> Result<SelectRouteExplanation, String> {
    let decision =
        classify_select_route(sql).ok_or_else(|| "not a routable relational SELECT".to_string())?;
    Ok(decision_to_explanation(&decision))
}

/// Catalog-aware `EXPLAIN SELECT`: resolves referenced tables so the disclosed route
/// matches what [`try_run_select`] executes. Discloses the planned physical plan but
/// does NOT execute the query. Under `datafusion-integration`, an all-Parquet-backed
/// query discloses `DataFusionLocal`; otherwise it discloses the Volcano/Native route.
pub async fn explain_select_route_with_catalog(
    sql: &str,
    dml: &Arc<DmlService>,
    tenant: Option<&str>,
) -> Result<SelectRouteExplanation, String> {
    route_and_plan_select(sql, dml, false, tenant).await
}

/// Catalog-aware `EXPLAIN ANALYZE SELECT`: like [`explain_select_route_with_catalog`]
/// but ALSO executes the planned native (Volcano) plan and attaches the measured
/// `execution_rows` + `execution_elapsed_us`. SELECT is read-only, so executing it is
/// side-effect free (matches Postgres EXPLAIN ANALYZE). Execution errors propagate.
pub async fn explain_analyze_select_with_catalog(
    sql: &str,
    dml: &Arc<DmlService>,
    tenant: Option<&str>,
) -> Result<SelectRouteExplanation, String> {
    route_and_plan_select(sql, dml, true, tenant).await
}

/// Shared core for catalog-aware `EXPLAIN [ANALYZE] SELECT`. Routes the query, then for
/// an engaging native plan renders the physical tree; when `analyze`, also executes it
/// (timed) and records actual rows + wall-clock. The plan is built ONCE via
/// [`prepare_select_plan`] and (for ANALYZE) executed via the same [`execute_physical`]
/// path real queries use — so the disclosed plan and the measured run never diverge.
async fn route_and_plan_select(
    sql: &str,
    dml: &Arc<DmlService>,
    analyze: bool,
    tenant: Option<&str>,
) -> Result<SelectRouteExplanation, String> {
    let statements =
        Parser::parse_sql(&GenericDialect {}, sql).map_err(|e| format!("parse: {e}"))?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err("not a routable relational SELECT".to_string());
    };
    let engages = query_engages_relational_engine(query);

    #[allow(unused_mut)]
    let mut parquet_backed = false;
    // Locations of the all-Parquet table set, captured during the route check so the
    // EXPLAIN split disclosure can reopen exactly the objects the executor scans.
    #[cfg(feature = "datafusion-integration")]
    let mut parquet_locations: Vec<String> = Vec::new();
    #[cfg(feature = "datafusion-integration")]
    {
        let mut names = Vec::new();
        collect_table_names(query, &mut names);
        if !names.is_empty() {
            let mut all_parquet = true;
            let mut locations = Vec::with_capacity(names.len());
            for raw in &names {
                match dml.resolve_relational_schema(raw, tenant).await {
                    Ok(schema) => match catalog_table_is_parquet_backed(&schema) {
                        Some(location) => locations.push(location),
                        None => {
                            all_parquet = false;
                            break;
                        }
                    },
                    _ => {
                        all_parquet = false;
                        break;
                    }
                }
            }
            parquet_backed = all_parquet;
            if all_parquet {
                parquet_locations = locations;
            }
        }
    }
    let decision = crate::query::compute_scheduler::ComputeScheduler::new().route_select_advised(
        crate::query::compute_scheduler::QueryShape {
            engages_relational: engages,
            parquet_backed,
            cardinality: query_cardinality_hint(query),
            ..Default::default()
        },
        Some(&crate::query::route_cost_model::GLOBAL_ROUTE_COST_MODEL),
    );
    let mut explanation = decision_to_explanation(&decision);

    // ADR-004 / TD-174 (gap #4): disclose stats-backed selectivity for a
    // single-table SELECT with `col = ?` equality predicate(s). Resolve the table
    // to the canonical collection id (the key the flush path stamps stats under),
    // multiply the resident per-field equality selectivities, and estimate
    // surviving rows from the resident record_count. Best-effort and
    // correct-or-absent: any unresolved table / absent statistics leaves the
    // fields `None` (omitted from EXPLAIN) rather than guessing.
    {
        let mut table_names = Vec::new();
        collect_table_names(query, &mut table_names);
        if table_names.len() == 1 {
            let cols = equality_predicate_columns(query);
            if !cols.is_empty()
                && let Some(collection_id) =
                    dml.resolve_collection_id(&table_names[0], tenant).await
            {
                let registry = crate::core::statistics::statistics_registry();
                let record_count = registry.envelope(&collection_id).map(|e| e.record_count);
                if let Some((selectivity, rows)) = stats_filter_estimate(&cols, record_count, |f| {
                    registry.equality_selectivity(&collection_id, f)
                }) {
                    explanation.estimated_selectivity = Some(selectivity);
                    explanation.estimated_rows = rows;
                }
            }
        }
    }
    // For the DataFusion route, disclose the concrete Parquet row-group split
    // inventory (partition count + footer row/byte estimates) in place of the
    // conservative whole-collection placeholder, by reading the same object-store
    // tables the executor will scan. Best-effort: any open failure leaves the
    // conservative summary intact rather than failing EXPLAIN.
    #[cfg(feature = "datafusion-integration")]
    if matches!(
        decision.backend,
        crate::query::table_write_plan::ComputeBackend::DataFusionLocal
    ) && let Some(summary) = parquet_split_summary(&parquet_locations).await
    {
        explanation.read_route = decision
            .routed_read_plan_with_splits(summary)
            .route_explanation();
    }
    // Disclose the planned physical plan for native (Volcano) PATH B queries — the same
    // plan execution runs (via the shared `prepare_select_plan` / `execute_physical`).
    if engages
        && matches!(
            decision.backend,
            crate::query::table_write_plan::ComputeBackend::Native
        )
        && let Some((snapshot, physical)) = prepare_select_plan(sql, query, dml, tenant).await
    {
        let base_lines = explain_physical(&physical);
        if analyze {
            // EXPLAIN ANALYZE: run the query (read-only) and record actuals —
            // whole-query totals plus per-operator rows/time annotated onto the
            // plan lines (metrics are pre-order aligned with `base_lines`).
            let started = std::time::Instant::now();
            let (result, node_metrics) = execute_physical_metered(physical, &snapshot).await?;
            explanation.execution_rows = Some(result.rows.len() as u64);
            explanation.execution_elapsed_us = Some(started.elapsed().as_micros() as u64);
            explanation.physical_plan = Some(annotate_plan_lines(base_lines, &node_metrics));
        } else {
            explanation.physical_plan = Some(base_lines);
        }
    }
    Ok(explanation)
}

/// Open the object-store Parquet tables backing a DataFusion-routed query and sum
/// their row-group split inventory into a [`crate::query::read_route::ReadSplitSummary`]
/// for EXPLAIN. Returns `None` (caller keeps the conservative whole-collection
/// summary) when there are no locations or any table fails to open — split
/// disclosure is diagnostic and must never make EXPLAIN itself fail.
#[cfg(feature = "datafusion-integration")]
async fn parquet_split_summary(
    locations: &[String],
) -> Option<crate::query::read_route::ReadSplitSummary> {
    use crate::datafusion::engine_adapters::ObjectStoreParquetTable;
    if locations.is_empty() {
        return None;
    }
    let mut partitions = 0usize;
    let (mut rows, mut bytes) = (0u64, 0u64);
    let (mut any_rows, mut any_bytes) = (false, false);
    for location in locations {
        let table = ObjectStoreParquetTable::open(location).await.ok()?;
        let splits = table.split_count();
        let est_rows = table.estimated_rows();
        partitions += splits;
        if let Some(r) = est_rows {
            rows = rows.saturating_add(r);
            any_rows = true;
        }
        if let Some(b) = table.estimated_bytes() {
            bytes = bytes.saturating_add(b);
            any_bytes = true;
        }
        // EXPLAIN opens the footer anyway — warm the route-time shape cache so
        // the next SELECT's route decision can classify this location's fan-out
        // / cardinality without a cold read (co-design: zero extra I/O).
        crate::query::route_cost_model::record_table_shape_stat(location, splits as u32, est_rows);
    }
    Some(crate::query::read_route::ReadSplitSummary::row_groups(
        partitions,
        any_rows.then_some(rows),
        any_bytes.then_some(bytes),
    ))
}

/// Append per-operator actuals to each physical-plan line. Metrics are pre-order
/// aligned with the lines (both walk the plan parent-first, left-to-right); if the
/// counts ever disagree, leave the lines unannotated (whole-query metrics still
/// reported) rather than mislabel. `time` is inclusive of children (Postgres "actual
/// time"); `self` subtracts the direct children's inclusive time — the operator's own
/// cost (the headline "which node is actually slow").
fn annotate_plan_lines(lines: Vec<String>, metrics: &[NodeMetric]) -> Vec<String> {
    if lines.len() != metrics.len() {
        return lines;
    }
    let self_ns = proximadb_relational_executor::self_times(metrics);
    lines
        .into_iter()
        .zip(metrics)
        .zip(self_ns)
        .map(|((line, m), self_ns)| {
            format!(
                "{line} (actual rows={} time={}us self={}us)",
                m.rows,
                m.elapsed_ns / 1000,
                self_ns / 1000
            )
        })
        .collect()
}

#[cfg(test)]
mod route_explain_tests {
    use super::*;

    #[test]
    fn olap_select_explains_as_olap() {
        let expl = explain_select_route("SELECT service, count(*) FROM events GROUP BY service")
            .expect("routable");
        assert_eq!(expl.workload_profile, "Olap");
        // P0 invariant: OLAP shape still executes on Volcano.
        assert_eq!(expl.compute_route, "Native(Volcano)");
        assert_eq!(expl.freshness_sla, "synchronous");
        assert_eq!(expl.read_route.selected_backend, "Native");
        assert_eq!(expl.read_route.split_strategy, "whole_collection");
    }

    #[test]
    fn simple_select_explains_as_oltp() {
        let expl =
            explain_select_route("SELECT id, name FROM users WHERE id = 1").expect("routable");
        assert_eq!(expl.workload_profile, "Oltp");
        assert_eq!(expl.compute_route, "Native(Volcano)");
    }

    #[test]
    fn non_select_is_not_routable() {
        assert!(explain_select_route("INSERT INTO t VALUES (1)").is_err());
    }

    #[test]
    fn catalog_free_route_carries_no_physical_plan() {
        // The catalog-free route disclosure has no planner output to render, so
        // `physical_plan` stays None and is omitted from the JSON. The physical
        // plan is only attached by the catalog-aware variant (covered e2e).
        let expl = explain_select_route("SELECT service, count(*) FROM events GROUP BY service")
            .expect("routable");
        assert!(expl.physical_plan.is_none());
        let json = serde_json::to_string(&expl).unwrap();
        assert!(
            !json.contains("physical_plan"),
            "None physical_plan is skipped in JSON: {json}"
        );
    }

    #[test]
    fn route_only_explanation_omits_analyze_metrics() {
        // Plain (non-ANALYZE) route disclosure never executes, so the ANALYZE metric
        // fields stay None and are omitted from the JSON. They are populated only by
        // the catalog-aware ANALYZE path that actually runs the plan (covered e2e).
        let expl = explain_select_route("SELECT service, count(*) FROM events GROUP BY service")
            .expect("routable");
        assert!(expl.execution_rows.is_none() && expl.execution_elapsed_us.is_none());
        let json = serde_json::to_string(&expl).unwrap();
        assert!(
            !json.contains("execution_rows") && !json.contains("execution_elapsed_us"),
            "None ANALYZE metrics are skipped in JSON: {json}"
        );
    }

    #[test]
    fn explain_surfaces_cost_model_advisory_once_warm() {
        // C4 slice 3: once the trace-driven cost model has warmed history for a
        // shape-class, the EXPLAIN reason discloses its (observe-mode) advisory.
        use crate::observability::io_trace::IoTraceSnapshot;
        use crate::query::route_cost_model::GLOBAL_ROUTE_COST_MODEL;
        let snap = IoTraceSnapshot {
            range_gets: 5,
            bytes_read: 1 << 20,
            ..Default::default()
        };
        // The GROUP BY query classifies as "olap/native"; warm Native there.
        for _ in 0..3 {
            GLOBAL_ROUTE_COST_MODEL.observe_by_label("olap/native", "Native(Volcano)", &snap);
        }
        // Publish the frozen consult table so the advisory is visible this query
        // (production recomputes debounced every N observations; tests force it).
        GLOBAL_ROUTE_COST_MODEL.recompute();
        let expl = explain_select_route("SELECT service, count(*) FROM events GROUP BY service")
            .expect("routable");
        // Observe-mode: the chosen engine is unchanged ...
        assert_eq!(expl.compute_route, "Native(Volcano)");
        // ... but the cost-model advisory is now visible in the EXPLAIN reason.
        assert!(
            expl.reason.contains("cost-model"),
            "advisory surfaced in EXPLAIN reason: {}",
            expl.reason
        );
    }
}

// =========================================================================
// Engagement gate + table-name collection (over the parsed AST)
// =========================================================================

/// True iff the query uses a feature the legacy single-table path can't serve
/// (join / GROUP BY / HAVING / aggregate projection / set-op), so it should be
/// routed to the algebra engine over real data. Simple single-table SELECTs
/// return false and stay on the legacy path.
fn query_engages_relational_engine(query: &SqlQuery) -> bool {
    set_expr_engages(&query.body)
}

/// A cheap, syntax-only cardinality hint from the parsed AST — the fallback
/// when the footer-warmed [`crate::query::route_cost_model::classify_table_shapes`]
/// stat is `Unknown` (native storage, or a cold Parquet table never yet scanned),
/// so the cost-model shape-class still discriminates instead of collapsing to
/// the coarse class. Returns [`CardinalityClass::Small`] only when the query
/// shape GUARANTEES a tiny result, else `Unknown` (never over-claims). Pure AST
/// walk — zero route-time I/O (co-design P5: I/O round-trips, not CPU, dominate;
/// the route path must not probe storage).
/// TD-OLAP-4 operation dimension: classify the SELECT's OLAP operation from the AST
/// (syntax-only, zero route-time I/O). Feeds the cost-model shape-class so per-engine
/// samples accumulate per operation — the geometry the shadow ledger showed engines
/// win/lose on. Priority: grouped > string-heavy > metadata-elidable > scalar-agg.
fn query_operation_class(query: &SqlQuery) -> crate::query::compute_scheduler::OperationClass {
    use crate::query::compute_scheduler::OperationClass;
    let SetExpr::Select(select) = query.body.as_ref() else {
        return OperationClass::Other;
    };
    let has_group_by = match &select.group_by {
        GroupByExpr::All(_) => true,
        GroupByExpr::Expressions(exprs, _) => !exprs.is_empty(),
    };
    if has_group_by {
        return OperationClass::Grouped;
    }
    // A LIKE/regex predicate → native can't push it down → DataFusion class.
    if select.selection.as_ref().is_some_and(expr_is_string_heavy) {
        return OperationClass::StringHeavy;
    }
    let has_agg = select.projection.iter().any(select_item_has_aggregate);
    if has_agg && select.having.is_none() {
        // Unfiltered + every projection a COUNT/MIN/MAX → footer-elidable.
        if select.selection.is_none()
            && select
                .projection
                .iter()
                .all(select_item_is_elidable_aggregate)
        {
            return OperationClass::MetadataElidable;
        }
        return OperationClass::ScalarAggregate;
    }
    OperationClass::Other
}

/// Does the predicate use a string-matching construct (`LIKE`/`ILIKE`/`SIMILAR TO`
/// or a `regexp_*` function)? Recursive over boolean structure.
fn expr_is_string_heavy(expr: &SqlExpr) -> bool {
    match expr {
        SqlExpr::Like { .. } | SqlExpr::ILike { .. } | SqlExpr::SimilarTo { .. } => true,
        SqlExpr::Function(f) => {
            let n = f.name.to_string().to_ascii_lowercase();
            matches!(
                n.rsplit('.').next().unwrap_or(&n),
                "regexp_replace" | "regexp_match" | "regexp_matches" | "regexp_like"
            )
        }
        SqlExpr::BinaryOp { left, right, .. } => {
            expr_is_string_heavy(left) || expr_is_string_heavy(right)
        }
        SqlExpr::UnaryOp { expr, .. } | SqlExpr::Nested(expr) | SqlExpr::Cast { expr, .. } => {
            expr_is_string_heavy(expr)
        }
        _ => false,
    }
}

/// Is this projection item a `COUNT`/`MIN`/`MAX` aggregate (footer-elidable, unlike
/// `SUM`/`AVG` which need column data)?
fn select_item_is_elidable_aggregate(item: &SelectItem) -> bool {
    let expr = match item {
        SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => e,
        _ => return false,
    };
    if let SqlExpr::Function(f) = expr {
        let n = f.name.to_string().to_ascii_lowercase();
        matches!(n.rsplit('.').next().unwrap_or(&n), "count" | "min" | "max")
    } else {
        false
    }
}

fn query_cardinality_hint(query: &SqlQuery) -> crate::query::compute_scheduler::CardinalityClass {
    use crate::query::compute_scheduler::CardinalityClass;
    // sqlparser 0.59 carries LIMIT on the Query as a `LimitClause` enum (standard
    // `LIMIT N [OFFSET]` vs MySQL `LIMIT offset, N`). A literal renders to a bare
    // number string; a placeholder / expression doesn't parse → stay Unknown
    // (never over-claim). Version-agnostic (no `Value::Number` probe).
    if let Some(rows) = query
        .limit_clause
        .as_ref()
        .and_then(limit_expr)
        .and_then(|e| e.to_string().parse::<u64>().ok())
    {
        return CardinalityClass::from_estimate(Some(rows));
    }
    cardinality_hint_body(&query.body)
}

/// The limit `Expr` from either `LimitClause` variant (`None` for `LIMIT ALL`).
fn limit_expr(clause: &sqlparser::ast::LimitClause) -> Option<&SqlExpr> {
    use sqlparser::ast::LimitClause;
    match clause {
        LimitClause::LimitOffset { limit, .. } => limit.as_ref(),
        LimitClause::OffsetCommaLimit { limit, .. } => Some(limit),
    }
}

fn cardinality_hint_body(body: &SetExpr) -> crate::query::compute_scheduler::CardinalityClass {
    use crate::query::compute_scheduler::CardinalityClass;
    match body {
        SetExpr::Select(select) => {
            // A scalar aggregate (aggregate projection, no GROUP BY / HAVING)
            // yields exactly one row.
            let has_agg = select.projection.iter().any(select_item_has_aggregate);
            let has_group_by = match &select.group_by {
                GroupByExpr::All(_) => true,
                GroupByExpr::Expressions(exprs, _) => !exprs.is_empty(),
            };
            if has_agg && !has_group_by && select.having.is_none() {
                return CardinalityClass::Small;
            }
            CardinalityClass::Unknown
        }
        // A parenthesized subquery may carry its own LIMIT; a set-op branch's
        // LIMIT does not bound the whole result, so be conservative elsewhere.
        SetExpr::Query(q) => {
            if let Some(rows) = q
                .limit_clause
                .as_ref()
                .and_then(limit_expr)
                .and_then(|e| e.to_string().parse::<u64>().ok())
            {
                return CardinalityClass::from_estimate(Some(rows));
            }
            cardinality_hint_body(&q.body)
        }
        _ => CardinalityClass::Unknown,
    }
}

fn set_expr_engages(body: &SetExpr) -> bool {
    match body {
        SetExpr::SetOperation { .. } => true,
        SetExpr::Query(q) => set_expr_engages(&q.body),
        SetExpr::Select(select) => {
            let has_join =
                select.from.len() > 1 || select.from.iter().any(|twj| !twj.joins.is_empty());
            let has_group_by = match &select.group_by {
                GroupByExpr::All(_) => true,
                GroupByExpr::Expressions(exprs, _) => !exprs.is_empty(),
            };
            let has_aggregate = select.projection.iter().any(select_item_has_aggregate);
            // A WHERE subquery (IN/EXISTS) lowers to a Semi/Anti join — a shape the
            // legacy single-table path can't serve — so engage PATH B. If it turns out
            // unliftable (correlated/NOT IN), lowering declines and the caller falls
            // through to legacy anyway, so engaging on ANY subquery is safe.
            let has_where_subquery = select.selection.as_ref().is_some_and(where_has_subquery);
            // A scalar subquery in the projection list (e.g.
            // `SELECT a, (SELECT max(x) FROM t)`) is hoisted by PATH B into a LEFT
            // JOIN; the legacy single-table path can't serve it. Same safety as the
            // WHERE case — an unliftable shape declines back to legacy.
            let has_projection_subquery = select.projection.iter().any(select_item_has_subquery);
            // A derived table (subquery in FROM, e.g. `FROM (SELECT … row_number()
            // OVER …) t WHERE rn = 1`, the TSBS last-point shape) is not something
            // the legacy single-table path can serve — engage the relational/OLAP
            // route so it reaches DataFusion.
            let has_derived = select.from.iter().any(table_with_joins_has_derived);
            // TD-183: a JSON-path extraction (`->`/`->>`/`json_extract_path_text`,
            // lowered to `JSON_EXTRACT[_TEXT]` calls by the translator) in a plain
            // projection or WHERE must engage the relational/OLAP route. Otherwise the
            // non-engaging SELECT falls through to store-type dispatch, is misread as a
            // *document* query because the SQL text contains `JSON_EXTRACT`, and is
            // served by the document handler against an empty collection → 0 rows.
            // Aggregated forms (e.g. d09 GROUP BY) already engage via `has_group_by`.
            let has_json_extract = select.projection.iter().any(select_item_has_json_extract)
                || select.selection.as_ref().is_some_and(expr_has_json_extract);
            has_join
                || has_group_by
                || select.having.is_some()
                || has_aggregate
                || has_where_subquery
                || has_projection_subquery
                || has_derived
                || has_json_extract
        }
        _ => false,
    }
}

/// True if a projected item applies a JSON-path extraction function. Mirrors
/// [`select_item_has_aggregate`]; used by the engagement gate (TD-183).
fn select_item_has_json_extract(item: &SelectItem) -> bool {
    match item {
        SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
            expr_has_json_extract(expr)
        }
        _ => false,
    }
}

/// True if `expr` contains a `JSON_EXTRACT` / `JSON_EXTRACT_TEXT` /
/// `json_extract_path_text` call anywhere in its tree. The extraction is always the
/// outermost function or is wrapped in Cast/Nested/Unary/Binary (e.g.
/// `(JSON_EXTRACT_TEXT(doc,'price'))::int > 8`), so walking those node kinds — the
/// same set [`expr_has_aggregate`] walks — is sufficient.
fn expr_has_json_extract(expr: &SqlExpr) -> bool {
    match expr {
        SqlExpr::Function(f) => {
            let name = f.name.to_string().to_ascii_lowercase();
            matches!(
                name.rsplit('.').next().unwrap_or(&name),
                "json_extract" | "json_extract_text" | "json_extract_path_text"
            )
        }
        SqlExpr::Nested(inner) => expr_has_json_extract(inner),
        SqlExpr::UnaryOp { expr, .. } => expr_has_json_extract(expr),
        SqlExpr::BinaryOp { left, right, .. } => {
            expr_has_json_extract(left) || expr_has_json_extract(right)
        }
        SqlExpr::Cast { expr, .. } => expr_has_json_extract(expr),
        _ => false,
    }
}

/// True if a WHERE expression contains a subquery (`IN (…)`, `EXISTS`, or a scalar
/// subquery) anywhere in its tree — used by the engagement gate to route subquery
/// predicates onto the relational (Semi/Anti) path.
fn where_has_subquery(expr: &SqlExpr) -> bool {
    match expr {
        SqlExpr::InSubquery { .. } | SqlExpr::Exists { .. } | SqlExpr::Subquery(_) => true,
        SqlExpr::BinaryOp { left, right, .. } => {
            where_has_subquery(left) || where_has_subquery(right)
        }
        SqlExpr::UnaryOp { expr, .. } | SqlExpr::Nested(expr) => where_has_subquery(expr),
        _ => false,
    }
}

/// True if a projection item's expression contains a subquery (scalar `(SELECT …)`,
/// `EXISTS`, or `IN (SELECT …)`) anywhere in its tree — routes a SELECT whose output
/// list carries a subquery onto PATH B, which hoists it into a join. Reuses the
/// general subquery-detection of [`where_has_subquery`].
fn select_item_has_subquery(item: &SelectItem) -> bool {
    match item {
        SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
            where_has_subquery(e)
        }
        _ => false,
    }
}

fn select_item_has_aggregate(item: &SelectItem) -> bool {
    match item {
        SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
            expr_has_aggregate(expr)
        }
        _ => false,
    }
}

fn expr_has_aggregate(expr: &SqlExpr) -> bool {
    match expr {
        SqlExpr::Function(f) => {
            let name = f.name.to_string().to_ascii_lowercase();
            matches!(
                name.rsplit('.').next().unwrap_or(&name),
                "count"
                    | "sum"
                    | "avg"
                    | "min"
                    | "max"
                    | "stddev"
                    | "stddev_pop"
                    | "stddev_samp"
                    | "variance"
                    | "var_pop"
                    | "var_samp"
            )
        }
        SqlExpr::Nested(inner) => expr_has_aggregate(inner),
        SqlExpr::UnaryOp { expr, .. } => expr_has_aggregate(expr),
        SqlExpr::BinaryOp { left, right, .. } => {
            expr_has_aggregate(left) || expr_has_aggregate(right)
        }
        SqlExpr::Cast { expr, .. } => expr_has_aggregate(expr),
        _ => false,
    }
}

fn collect_table_names(query: &SqlQuery, out: &mut Vec<String>) {
    collect_from_set_expr(&query.body, out);
}

fn collect_from_set_expr(body: &SetExpr, out: &mut Vec<String>) {
    match body {
        SetExpr::Select(select) => {
            for twj in &select.from {
                collect_from_table_with_joins(twj, out);
            }
            // Also descend into WHERE subqueries (IN/EXISTS lower to Semi/Anti joins
            // whose right side scans the subquery's tables) so those tables get
            // prepared in the snapshot — otherwise the subquery fails to lower and
            // the query silently falls through to the legacy path.
            if let Some(where_expr) = &select.selection {
                collect_subquery_tables_in_expr(where_expr, out);
            }
        }
        SetExpr::Query(q) => collect_table_names(q, out),
        SetExpr::SetOperation { left, right, .. } => {
            collect_from_set_expr(left, out);
            collect_from_set_expr(right, out);
        }
        _ => {}
    }
}

/// Collect table names referenced by subqueries inside a WHERE expression
/// (`IN (…)`, `EXISTS`, scalar subqueries), recursing through boolean structure.
fn collect_subquery_tables_in_expr(expr: &SqlExpr, out: &mut Vec<String>) {
    match expr {
        SqlExpr::InSubquery { subquery, .. } | SqlExpr::Exists { subquery, .. } => {
            collect_table_names(subquery, out);
        }
        SqlExpr::Subquery(q) => collect_table_names(q, out),
        SqlExpr::BinaryOp { left, right, .. } => {
            collect_subquery_tables_in_expr(left, out);
            collect_subquery_tables_in_expr(right, out);
        }
        SqlExpr::UnaryOp { expr, .. } | SqlExpr::Nested(expr) => {
            collect_subquery_tables_in_expr(expr, out);
        }
        _ => {}
    }
}

/// True if the FROM item (or any of its joins) is a derived table — a subquery in
/// FROM. Such a shape must engage the relational/OLAP route (the legacy single-table
/// path can't serve it).
fn table_with_joins_has_derived(twj: &TableWithJoins) -> bool {
    matches!(twj.relation, TableFactor::Derived { .. })
        || twj
            .joins
            .iter()
            .any(|j| matches!(j.relation, TableFactor::Derived { .. }))
}

fn collect_from_table_with_joins(twj: &TableWithJoins, out: &mut Vec<String>) {
    collect_from_table_factor(&twj.relation, out);
    for join in &twj.joins {
        collect_from_table_factor(&join.relation, out);
    }
}

fn collect_from_table_factor(factor: &TableFactor, out: &mut Vec<String>) {
    match factor {
        // A `FROM name(args)` item is a table-valued function (cross-modal source:
        // vector_search / timeseries_range / graph_traverse), NOT a catalog table.
        // Collecting it as a table name makes catalog resolution decline the query to
        // the legacy path AND breaks the parquet-backed route check. Skip it here and
        // let the DataFusion `ctx.sql` fallback (which has the UDTFs registered) resolve it.
        // Only a plain `name` (args: None) is a catalog table. A `FROM name(args)` item is a
        // table-valued function (cross-modal source: vector_search / timeseries_range /
        // graph_traverse) — it matches neither arm below, falls through to `_`, and is left for
        // the DataFusion `ctx.sql` fallback (where the UDTFs are registered) to resolve.
        TableFactor::Table {
            name, args: None, ..
        } => out.push(name.to_string()),
        TableFactor::Derived { subquery, .. } => collect_table_names(subquery, out),
        _ => {}
    }
}

// =========================================================================
// PgType / text encoding helpers
// =========================================================================

/// Map a [`ProximaType`] to the closest [`PgType`] for
/// `RowDescription` emission. Unknowns map to `Text` so clients
/// can still display values.
#[cfg(test)]
mod tests {
    use super::*;

    fn gate(sql: &str) -> bool {
        let statements = Parser::parse_sql(&GenericDialect {}, sql).expect("parse");
        match statements.as_slice() {
            [Statement::Query(query)] => query_engages_relational_engine(query),
            _ => panic!("expected a single SELECT statement"),
        }
    }

    #[test]
    fn operation_class_classifies_olap_shapes() {
        use crate::query::compute_scheduler::OperationClass;
        let op = |sql: &str| {
            let s = Parser::parse_sql(&GenericDialect {}, sql).expect("parse");
            match s.as_slice() {
                [Statement::Query(q)] => query_operation_class(q),
                _ => panic!("one SELECT"),
            }
        };
        // Unfiltered COUNT(*) / MIN / MAX → footer-elidable (native wins).
        assert_eq!(
            op("SELECT COUNT(*) FROM hits"),
            OperationClass::MetadataElidable
        );
        assert_eq!(
            op("SELECT MIN(eventdate), MAX(eventdate) FROM hits"),
            OperationClass::MetadataElidable
        );
        // SUM/AVG need column data → scalar aggregate.
        assert_eq!(
            op("SELECT SUM(advengineid), AVG(resolutionwidth) FROM hits"),
            OperationClass::ScalarAggregate
        );
        // A plain (non-string) filter is still a scalar aggregate.
        assert_eq!(
            op("SELECT COUNT(*) FROM hits WHERE advengineid <> 0"),
            OperationClass::ScalarAggregate
        );
        // LIKE predicate → string-heavy (routes to DataFusion).
        assert_eq!(
            op("SELECT COUNT(*) FROM hits WHERE url LIKE '%google%'"),
            OperationClass::StringHeavy
        );
        // GROUP BY → grouped.
        assert_eq!(
            op("SELECT regionid, COUNT(*) FROM hits GROUP BY regionid"),
            OperationClass::Grouped
        );
    }

    #[cfg(feature = "datafusion-integration")]
    fn scan_fixture(cols: &[&str]) -> PhysicalPlan {
        PhysicalPlan::Scan {
            table: TableId::new("hits"),
            output_schema: RelationalSchema::new(
                cols.iter()
                    .map(|c| ColumnInfo::new(*c, ProximaType::Int64, false))
                    .collect(),
            ),
            projection: None,
            predicate: None,
            limit: None,
            access: proximadb_relational_planner::ScanAccess::FullScan,
        }
    }

    #[cfg(feature = "datafusion-integration")]
    #[test]
    fn single_scan_columns_detects_sole_scan_and_rejects_multi_or_none() {
        // Bare scan → its projected column names.
        assert_eq!(
            single_scan_columns(&scan_fixture(&["k", "x"])),
            Some(vec!["k".to_string(), "x".to_string()])
        );
        // Recurses through an intermediate op (Limit) to the sole scan.
        let limited = PhysicalPlan::Limit {
            input: Box::new(scan_fixture(&["k", "x"])),
            limit: Some(10),
            offset: 0,
        };
        assert_eq!(
            single_scan_columns(&limited),
            Some(vec!["k".to_string(), "x".to_string()])
        );
        // Two scans (Union) → None: the native shadow is single-table only.
        let two = PhysicalPlan::Union {
            inputs: vec![scan_fixture(&["k"]), scan_fixture(&["x"])],
            all: true,
        };
        assert_eq!(single_scan_columns(&two), None);
        // No scan at all → None.
        let no_scan = PhysicalPlan::Values {
            rows: vec![],
            output_schema: RelationalSchema::new(vec![]),
        };
        assert_eq!(single_scan_columns(&no_scan), None);
    }

    #[test]
    fn gate_engages_for_join_groupby_aggregate_and_union() {
        // Queries the legacy single-table path can't serve → route to PATH B.
        assert!(gate(
            "SELECT emp.name FROM emp JOIN dept ON emp.dept_id = dept.id"
        ));
        assert!(gate("SELECT status, COUNT(*) FROM inv GROUP BY status"));
        assert!(gate("SELECT COUNT(*) AS n FROM inv"));
        assert!(gate(
            "SELECT status FROM inv GROUP BY status HAVING COUNT(*) > 1"
        ));
        assert!(gate("SELECT id FROM a UNION SELECT id FROM b"));
        assert!(gate("SELECT SUM(qty) AS s FROM inv"));
        // WHERE subqueries (IN/EXISTS) lower to Semi/Anti joins → engage PATH B,
        // even on a single FROM table (incl. when ANDed with a plain predicate).
        assert!(gate("SELECT id FROM inv WHERE id IN (SELECT id FROM dept)"));
        assert!(gate(
            "SELECT id FROM inv WHERE EXISTS (SELECT id FROM dept)"
        ));
        assert!(gate(
            "SELECT id FROM inv WHERE qty > 5 AND id IN (SELECT id FROM dept)"
        ));
    }

    #[test]
    fn gate_skips_simple_single_table_selects() {
        // These stay on the (hardened) legacy path — gate must NOT engage.
        assert!(!gate("SELECT * FROM inv"));
        assert!(!gate(
            "SELECT id FROM inv WHERE status = 'active' OR qty >= 30"
        ));
        assert!(!gate("SELECT id FROM inv ORDER BY qty LIMIT 5"));
        assert!(!gate("SELECT id, status FROM inv WHERE id = 'i1'"));
    }

    /// ADR-025 PR3: the OLAP read-merge is default-ON, with an explicit falsy
    /// `PROXIMADB_OLAP_DELTA_MERGE` token as the engine-wide kill-switch. Tests the
    /// pure policy directly so it never has to mutate the process environment.
    #[cfg(feature = "datafusion-integration")]
    #[test]
    fn olap_delta_merge_default_on_with_explicit_killswitch() {
        // Default-ON: unset and any non-falsy value enable the merge.
        assert!(olap_delta_merge_on(None));
        for on in ["1", "true", "on", "yes", "anything", ""] {
            assert!(olap_delta_merge_on(Some(on)), "`{on}` must keep merge ON");
        }
        // Kill-switch: explicit falsy tokens (case-insensitive, trimmed) disable it.
        for off in ["0", "false", "off", "no", "OFF", " False ", "No"] {
            assert!(
                !olap_delta_merge_on(Some(off)),
                "`{off}` must disable merge"
            );
        }
    }

    fn card_hint(sql: &str) -> crate::query::compute_scheduler::CardinalityClass {
        let statements = Parser::parse_sql(&GenericDialect {}, sql).expect("parse");
        match statements.as_slice() {
            [Statement::Query(query)] => query_cardinality_hint(query),
            _ => panic!("expected a single SELECT statement"),
        }
    }

    #[test]
    fn cardinality_hint_small_for_limit_and_scalar_aggregate() {
        use crate::query::compute_scheduler::CardinalityClass;
        // LIMIT n bounds the result (a literal renders to a bare number string).
        assert_eq!(
            card_hint("SELECT * FROM inv LIMIT 10"),
            CardinalityClass::Small
        );
        // A large LIMIT still buckets via from_estimate (≤1M → Medium).
        assert_eq!(
            card_hint("SELECT * FROM inv LIMIT 100000"),
            CardinalityClass::Medium
        );
        // A scalar aggregate (no GROUP BY / HAVING) → exactly one row → Small.
        assert_eq!(
            card_hint("SELECT COUNT(*) FROM inv"),
            CardinalityClass::Small
        );
        assert_eq!(
            card_hint("SELECT SUM(qty), AVG(qty) FROM inv"),
            CardinalityClass::Small
        );
        // GROUP BY defeats the scalar-aggregate signal (could be many groups).
        assert_eq!(
            card_hint("SELECT status, COUNT(*) FROM inv GROUP BY status"),
            CardinalityClass::Unknown
        );
        // A plain unbounded scan → Unknown (never over-claim).
        assert_eq!(card_hint("SELECT * FROM inv"), CardinalityClass::Unknown);
        // A non-literal LIMIT expression doesn't parse to a count → Unknown.
        assert_eq!(
            card_hint("SELECT * FROM inv LIMIT 1 + 1"),
            CardinalityClass::Unknown
        );
    }

    #[test]
    fn collect_table_names_covers_join_and_normalizes() {
        let statements = Parser::parse_sql(
            &GenericDialect {},
            "SELECT * FROM Emp JOIN \"dept\" ON Emp.d = dept.id",
        )
        .expect("parse");
        let [Statement::Query(query)] = statements.as_slice() else {
            panic!("expected query");
        };
        let mut names = Vec::new();
        collect_table_names(query, &mut names);
        let keys: Vec<String> = names.iter().map(|n| normalize_table_key(n)).collect();
        assert!(keys.contains(&"emp".to_string()), "got {keys:?}");
        assert!(keys.contains(&"dept".to_string()), "got {keys:?}");
    }

    fn parse_query(sql: &str) -> SqlQuery {
        let statements = Parser::parse_sql(&GenericDialect {}, sql).expect("parse");
        let [Statement::Query(query)] = statements.as_slice() else {
            panic!("expected query");
        };
        (**query).clone()
    }

    #[test]
    fn equality_predicate_columns_extracts_and_chained_literals_only() {
        // AND-chained equalities → both columns; `t.col` → bare name; literal on
        // either side works.
        let q = parse_query("SELECT * FROM t WHERE status = 'open' AND 7 = t.priority");
        let mut cols = equality_predicate_columns(&q);
        cols.sort();
        assert_eq!(cols, vec!["priority".to_string(), "status".to_string()]);

        // No WHERE → empty.
        assert!(equality_predicate_columns(&parse_query("SELECT * FROM t")).is_empty());
        // Non-equality (range) and col=col (no literal) → ignored.
        assert!(
            equality_predicate_columns(&parse_query("SELECT * FROM t WHERE a > 1 AND b = c"))
                .is_empty()
        );
        // OR is not AND-chained equality → ignored (not estimable here).
        assert!(
            equality_predicate_columns(&parse_query("SELECT * FROM t WHERE a = 1 OR b = 2"))
                .is_empty()
        );
    }

    #[test]
    fn stats_filter_estimate_multiplies_and_estimates_rows() {
        let cols = vec!["a".to_string(), "b".to_string(), "unknown".to_string()];
        // a: 1/4, b: 1/5, unknown: no stat. product = 0.05; rows = ceil(1000*0.05)=50.
        let (sel, rows) = stats_filter_estimate(&cols, Some(1000), |f| match f {
            "a" => Some(0.25),
            "b" => Some(0.2),
            _ => None,
        })
        .expect("at least one field has a stat");
        assert!((sel - 0.05).abs() < 1e-9, "selectivity {sel}");
        assert_eq!(rows, Some(50));

        // No field has a stat → None (EXPLAIN left unchanged, never guessed).
        assert!(stats_filter_estimate(&cols, Some(1000), |_| None).is_none());
        // Stat but no record_count → selectivity present, rows None.
        let (sel2, rows2) =
            stats_filter_estimate(&["a".to_string()], None, |_| Some(0.1)).expect("has stat");
        assert!((sel2 - 0.1).abs() < 1e-9);
        assert_eq!(rows2, None);
    }

    #[test]
    fn collect_table_names_descends_into_where_subqueries() {
        let statements = Parser::parse_sql(
            &GenericDialect {},
            "SELECT ename FROM emp WHERE dept_id IN (SELECT id FROM dept WHERE dname = 'eng')",
        )
        .expect("parse");
        let [Statement::Query(query)] = statements.as_slice() else {
            panic!("expected query");
        };
        let mut names = Vec::new();
        collect_table_names(query, &mut names);
        let keys: Vec<String> = names.iter().map(|n| normalize_table_key(n)).collect();
        // Both the outer table AND the subquery table must be collected so the
        // snapshot prepares `dept` (else the Semi-join subquery can't lower).
        assert!(keys.contains(&"emp".to_string()), "got {keys:?}");
        assert!(keys.contains(&"dept".to_string()), "got {keys:?}");
    }

    #[test]
    fn normalize_table_key_strips_qualifier_and_quotes() {
        assert_eq!(normalize_table_key("Inv"), "inv");
        assert_eq!(normalize_table_key("public.Orders"), "orders");
        assert_eq!(normalize_table_key("\"Mixed\""), "mixed");
    }

    #[test]
    fn snapshot_capabilities_advertises_single_col_pk_only() {
        let mut pk_by_table = HashMap::new();
        pk_by_table.insert("users".to_string(), vec![0]); // single-col PK at ordinal 0
        pk_by_table.insert("edges".to_string(), Vec::new()); // composite/no-PK → none
        let mut secondary_by_table = HashMap::new();
        secondary_by_table.insert("users".to_string(), vec!["name".to_string()]);
        let resolver = SnapshotCapabilities {
            pk_by_table,
            secondary_by_table,
        };

        // Single-col PK table → planner can pick PkLookup (name normalized).
        assert_eq!(resolver.primary_key(&TableId::new("users")), vec![0]);
        assert_eq!(resolver.primary_key(&TableId::new("USERS")), vec![0]);
        // Composite/no-PK and unknown tables → empty → planner keeps full scan.
        assert!(resolver.primary_key(&TableId::new("edges")).is_empty());
        assert!(resolver.primary_key(&TableId::new("unknown")).is_empty());
        // Pushdown capabilities unchanged (pk_lookup gated per-table by primary_key).
        assert!(resolver.capabilities(&TableId::new("users")).pk_lookup);
        // TD-128/TD-127: batch + secondary advertised (gated per-table below).
        assert!(
            resolver
                .capabilities(&TableId::new("users"))
                .pk_lookup_batch
        );
        assert!(
            resolver
                .capabilities(&TableId::new("users"))
                .secondary_lookup
        );
        // TD-127: secondary-indexed columns surfaced per-table (name-normalized);
        // unknown tables → empty → planner keeps a full scan.
        assert_eq!(
            resolver.secondary_index_columns(&TableId::new("USERS")),
            vec!["name".to_string()]
        );
        assert!(
            resolver
                .secondary_index_columns(&TableId::new("edges"))
                .is_empty()
        );
    }

    #[test]
    fn text_encode_null_is_none_and_exotic_types_render() {
        // SQL NULL → None (the caller emits the pgwire `-1` null sentinel).
        // This is the core of the pgwire NULL-rendering fix.
        assert_eq!(text_encode(&ProximaValue::Null), None);

        // Scalars render as before.
        assert_eq!(
            text_encode(&ProximaValue::String("alice".into())),
            Some("alice".to_string())
        );
        assert_eq!(
            text_encode(&ProximaValue::Boolean(false)),
            Some("f".to_string())
        );

        // Exotic types now render explicitly rather than as a Debug fallback.
        assert_eq!(
            text_encode(&ProximaValue::Array(vec![
                ProximaValue::Int32(1),
                ProximaValue::Int32(2),
            ])),
            Some("{1,2}".to_string())
        );
        // A NULL element inside an array uses the empty-field convention.
        assert_eq!(
            text_encode(&ProximaValue::Array(vec![
                ProximaValue::Int32(1),
                ProximaValue::Null,
            ])),
            Some("{1,}".to_string())
        );

        // UUID renders dashed (Postgres format), not raw hex.
        let uuid = ProximaValue::Uuid([
            0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44,
            0x00, 0x00,
        ]);
        let rendered = text_encode(&uuid).expect("uuid must encode");
        assert_eq!(rendered, "550e8400-e29b-41d4-a716-446655440000");
        assert!(
            rendered.contains('-'),
            "UUID must be dashed (Postgres format)"
        );
    }
}
