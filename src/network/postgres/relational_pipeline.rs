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
    ExecutionControls, ExecutionPipelineResult, NativeVolcanoEngine, normalize_table_key,
};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use proximadb_data_model::{ProximaType, ProximaValue};
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
use proximadb_relational_types::{ColumnInfo, Expr, NoFunctions, RelationalRow, RelationalSchema};
use sqlparser::ast::{
    Expr as SqlExpr, GroupByExpr, Query as SqlQuery, SelectItem, SetExpr, Statement, TableFactor,
    TableWithJoins,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::collections::HashMap;
use std::sync::Arc;

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

    let decision = crate::query::compute_scheduler::ComputeScheduler::new().route_select_advised(
        crate::query::compute_scheduler::QueryShape {
            engages_relational: true,
            parquet_backed,
            ..Default::default()
        },
        // C4: observe-mode advisory from the trace-driven cost model — augments
        // the reason for telemetry/EXPLAIN, never changes the backend.
        Some(&crate::query::route_cost_model::GLOBAL_ROUTE_COST_MODEL),
    );
    tracing::debug!(
        target: "proximadb::compute_route",
        backend = ?decision.backend,
        workload = ?decision.workload_profile,
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
            tenant_id: tenant.map(str::to_string),
            // Full ANSI SQL over pgwire: when the shared relational frontend can't
            // yet lower a query (e.g. typed DATE literals, certain subquery/window
            // shapes), execute it through DataFusion's own ANSI SQL planner over the
            // same registered Parquet tables. The query still routes through pgwire →
            // ComputeScheduler → the DataFusion engine; only the lowering frontend
            // differs. The shared logical plane stays the fast path where it works.
            allow_engine_sql_fallback: true,
            controls: controls.clone(),
        };
        return Some(
            execute_sql_with_backend(decision.backend.clone(), sql, context)
                .await
                .map_err(|e| e.to_string()),
        );
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
    Some(execute_physical(physical, &snapshot, controls).await)
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
    NativeVolcanoEngine::execute_physical(physical, factory, controls)
        .await
        .map_err(|e| e.to_string())
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
    let resolver = SnapshotCapabilities { pk_by_table };
    let planner = Planner::new(resolver);
    Some(planner.plan(logical).map_err(|e| format!("plan: {e}")))
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
}

impl CapabilityResolver for SnapshotCapabilities {
    fn capabilities(&self, _table: &TableId) -> ReaderCapabilities {
        // Unchanged pushdown behavior (projection/predicate); PK-lookup is gated
        // per-table by `primary_key` below.
        ReaderCapabilities::full()
    }

    fn primary_key(&self, table: &TableId) -> Vec<usize> {
        self.pk_by_table
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
        // (see `lookup_pk`). The planner gates PkLookup per-table via the
        // resolver's `primary_key`, so this flag is informational here.
        ReaderCapabilities::full().with_pk_lookup(true)
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
        let row_pred = move |full_row: &[ProximaValue]| -> bool {
            match &predicate {
                Some(expr) => matches!(
                    expr.eval(&full_row.to_vec(), &NoFunctions),
                    Ok(ProximaValue::Boolean(true))
                ),
                None => true,
            }
        };
        let row_pred_ref: Option<&(dyn Fn(&[ProximaValue]) -> bool + Send + Sync)> =
            if ctx.predicate.is_some() {
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
        V::Binary(b) => {
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
        other => format!("{other:?}"),
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
    }
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
            ..Default::default()
        },
        Some(&crate::query::route_cost_model::GLOBAL_ROUTE_COST_MODEL),
    );
    let mut explanation = decision_to_explanation(&decision);
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
        partitions += table.split_count();
        if let Some(r) = table.estimated_rows() {
            rows = rows.saturating_add(r);
            any_rows = true;
        }
        if let Some(b) = table.estimated_bytes() {
            bytes = bytes.saturating_add(b);
            any_bytes = true;
        }
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
            has_join
                || has_group_by
                || select.having.is_some()
                || has_aggregate
                || has_where_subquery
                || has_projection_subquery
                || has_derived
        }
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
        TableFactor::Table { name, .. } => out.push(name.to_string()),
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
        let resolver = SnapshotCapabilities { pk_by_table };

        // Single-col PK table → planner can pick PkLookup (name normalized).
        assert_eq!(resolver.primary_key(&TableId::new("users")), vec![0]);
        assert_eq!(resolver.primary_key(&TableId::new("USERS")), vec![0]);
        // Composite/no-PK and unknown tables → empty → planner keeps full scan.
        assert!(resolver.primary_key(&TableId::new("edges")).is_empty());
        assert!(resolver.primary_key(&TableId::new("unknown")).is_empty());
        // Pushdown capabilities unchanged (pk_lookup gated per-table by primary_key).
        assert!(resolver.capabilities(&TableId::new("users")).pk_lookup);
    }
}
