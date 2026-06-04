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

use async_trait::async_trait;
use once_cell::sync::Lazy;
use proximadb_data_model::{ProximaType, ProximaValue};
use proximadb_relational_algebra::TableId;
use proximadb_relational_engine::{
    EngineReaderFactory, InMemoryRelationalEngine, RelationalWriter,
};
use proximadb_relational_executor::{
    ExecError, ExecutionContext, ReaderFactory, build_executor, collect,
};
use proximadb_relational_frontend::{CatalogLookup, lower_sql};
use proximadb_relational_planner::{CapabilityResolver, Planner, StaticCapabilities};
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

/// Result of a successful pipeline execution.
#[derive(Debug)]
pub struct PipelineResult {
    pub schema: RelationalSchema,
    pub rows: Vec<RelationalRow>,
}

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
) -> Option<Result<PipelineResult, String>> {
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
        return Some(run_plan(&factory, resolver, logical).await);
    }

    // 2) Real-data path (gated, additive). Engage ONLY for queries the legacy
    //    single-table path can't serve — joins / GROUP BY / aggregates / HAVING
    //    / set-ops — leaving simple SELECTs on the (hardened) legacy path.
    let dml = dml?;
    let statements = Parser::parse_sql(&GenericDialect {}, sql).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    let engages = query_engages_relational_engine(query);

    // Course-correction §5 (P0): materialize the compute-route decision at the
    // planner boundary via the canonical `ComputeScheduler`. P0 ALWAYS resolves
    // to `Native` (the live Volcano path) — additive, no behavior change — so the
    // decision is observable and P1 has one contract-bound place to flip the OLAP
    // arm to DataFusion. (A SELECT EXPLAIN surface can later print
    // `decision.explain_line()`; today we emit it as a structured trace.)
    let decision = crate::query::compute_scheduler::ComputeScheduler::new().route_select(
        crate::query::compute_scheduler::QueryShape {
            engages_relational: engages,
        },
    );
    tracing::debug!(
        target: "proximadb::compute_route",
        backend = ?decision.backend,
        workload = ?decision.workload_profile,
        reason = %decision.reason,
        "{}",
        decision.explain_line()
    );

    if !engages {
        return None;
    }

    // Pre-resolve every referenced table's SCHEMA from xCatalog (the sync
    // `CatalogLookup`/`ReaderFactory` traits can't await). Rows are fetched
    // lazily per scan in `DmlTableReader::open`, with the executor's
    // projection/predicate/limit pushed into storage.
    let mut names = Vec::new();
    collect_table_names(query, &mut names);
    let mut tables: HashMap<String, PreparedTable> = HashMap::new();
    for raw in &names {
        let key = normalize_table_key(raw);
        if tables.contains_key(&key) {
            continue;
        }
        match dml.resolve_relational_schema(raw).await {
            Ok(catalog_schema) => {
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

    let snapshot = SnapshotCatalog {
        dml: dml.clone(),
        tables,
    };
    // Lowering failure → fall through to legacy (over-inclusive, never wrong).
    let logical = match lower_sql(sql, &snapshot) {
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
    // From here, errors are real and surface to the client.
    Some(run_plan(&snapshot, resolver, logical).await)
}

/// Plan + build + drain an executor for `logical` against `factory`, using
/// `resolver` for capability/PK-lookup planning (engine path → `StaticCapabilities`,
/// real-data path → `SnapshotCapabilities`).
async fn run_plan<F: ReaderFactory, R: CapabilityResolver>(
    factory: &F,
    resolver: R,
    logical: proximadb_relational_algebra::LogicalNode,
) -> Result<PipelineResult, String> {
    let planner = Planner::new(resolver);
    let physical = planner.plan(logical).map_err(|e| format!("plan: {e}"))?;
    let mut exec = build_executor(physical, factory, &ExecutionContext::default())
        .map_err(|e| format!("build_executor: {e}"))?;
    exec.open().await.map_err(|e| format!("open: {e}"))?;
    let schema = exec.schema().clone();
    let rows = collect(&mut *exec)
        .await
        .map_err(|e| format!("scan: {e}"))?;
    Ok(PipelineResult { schema, rows })
}

struct EngineCatalog(Arc<InMemoryRelationalEngine>);

impl CatalogLookup for EngineCatalog {
    fn lookup_table(&self, name: &str) -> Option<RelationalSchema> {
        self.0.schema_of(name)
    }
}

// =========================================================================
// Real-data snapshot: a pre-fetched, sync view of catalog tables that
// backs BOTH the frontend's CatalogLookup and the executor's ReaderFactory.
// =========================================================================

/// One table's pre-resolved schema (rows are fetched lazily per scan).
struct PreparedTable {
    table_name: String,
    schema: RelationalSchema,
    /// Primary-key column ordinal(s) for the PK-lookup access path. Populated
    /// ONLY for single-column PK (the storage point lookup keys on `record.oid`);
    /// composite/no-PK → empty, so the planner keeps a full scan.
    pk_columns: Vec<usize>,
}

impl PreparedTable {
    fn from_catalog(
        table_name: &str,
        catalog_schema: &proximadb_catalog::CatalogTableSchema,
    ) -> Self {
        let columns: Vec<ColumnInfo> = catalog_schema
            .columns
            .iter()
            .map(|c| ColumnInfo::new(c.name.clone(), c.data_type.to_proxima_type(), c.nullable))
            .collect();
        // Single-column PK only: advertise its ordinal so the planner can pick
        // ScanAccess::PkLookup; composite/no-PK advertise nothing → full scan.
        let pk_columns: Vec<usize> = if catalog_schema.primary_key.len() == 1 {
            catalog_schema
                .columns
                .iter()
                .position(|c| c.name == catalog_schema.primary_key[0])
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        Self {
            table_name: table_name.to_string(),
            schema: RelationalSchema::new(columns),
            pk_columns,
        }
    }
}

/// Pre-resolved real-data catalog. Implements both [`CatalogLookup`] (for
/// lowering) and [`ReaderFactory`] (for execution) over the same schema set, so
/// the schema the frontend lowers against is exactly the one the reader emits.
/// Holds the [`DmlService`] so readers can fetch rows lazily with pushdown.
struct SnapshotCatalog {
    dml: Arc<DmlService>,
    tables: HashMap<String, PreparedTable>,
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
            .point_lookup_relational(&self.table_name, &key_str)
            .await
            .map_err(|e| ReaderError::Storage(e.to_string()))
    }

    async fn close(&mut self) -> Result<(), ReaderError> {
        self.open_state = None;
        Ok(())
    }
}

/// Normalize a SQL table reference to a lookup key: last dotted segment,
/// unquoted, lowercased. Applied identically when pre-fetching, on
/// `CatalogLookup`, and on `ReaderFactory` so all three agree regardless of
/// how the frontend renders the name.
fn normalize_table_key(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or(name)
        .trim_matches('"')
        .to_ascii_lowercase()
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
        crate::query::compute_scheduler::ComputeScheduler::new().route_select(
            crate::query::compute_scheduler::QueryShape {
                engages_relational: engages,
            },
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
}

/// Build the `EXPLAIN SELECT` payload. `Err` if `sql` is not a routable single
/// relational query.
pub fn explain_select_route(sql: &str) -> Result<SelectRouteExplanation, String> {
    let decision =
        classify_select_route(sql).ok_or_else(|| "not a routable relational SELECT".to_string())?;
    let freshness_sla = match decision.backend {
        crate::query::table_write_plan::ComputeBackend::Native => {
            "strong (Volcano over WAL+RecordStorage)".to_string()
        }
        _ => "base-snapshot (engine read path)".to_string(),
    };
    Ok(SelectRouteExplanation {
        compute_route: decision.compute_route_label(),
        workload_profile: format!("{:?}", decision.workload_profile),
        authority_mode: "control-plane-route (no durable authority moved)".to_string(),
        policy_boundary: "query-plan (one engine per plan)".to_string(),
        freshness_sla,
        reason: decision.reason.clone(),
    })
}

#[cfg(test)]
mod route_explain_tests {
    use super::*;

    #[test]
    fn olap_select_explains_as_olap() {
        let expl = explain_select_route(
            "SELECT service, count(*) FROM events GROUP BY service",
        )
        .expect("routable");
        assert_eq!(expl.workload_profile, "Olap");
        // P0 invariant: OLAP shape still executes on Volcano.
        assert_eq!(expl.compute_route, "Native(Volcano)");
        assert!(expl.freshness_sla.to_lowercase().contains("strong"));
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
            let has_aggregate = select
                .projection
                .iter()
                .any(|item| select_item_has_aggregate(item));
            has_join || has_group_by || select.having.is_some() || has_aggregate
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
        }
        SetExpr::Query(q) => collect_table_names(q, out),
        SetExpr::SetOperation { left, right, .. } => {
            collect_from_set_expr(left, out);
            collect_from_set_expr(right, out);
        }
        _ => {}
    }
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
