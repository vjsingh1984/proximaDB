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
use proximadb_relational_planner::{Planner, StaticCapabilities};
use proximadb_relational_reader::{ReaderCapabilities, RelationalReader, VecReader};
use proximadb_relational_types::{ColumnInfo, RelationalRow, RelationalSchema};
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
        return Some(run_plan(&factory, logical).await);
    }

    // 2) Real-data path (gated, additive). Engage ONLY for queries the legacy
    //    single-table path can't serve — joins / GROUP BY / aggregates / HAVING
    //    / set-ops — leaving simple SELECTs on the (hardened) legacy path.
    let dml = dml?;
    let statements = Parser::parse_sql(&GenericDialect {}, sql).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    if !query_engages_relational_engine(query) {
        return None;
    }

    // Pre-resolve every referenced table's schema + rows from real storage
    // (the sync `CatalogLookup`/`ReaderFactory` traits can't await xCatalog).
    let mut names = Vec::new();
    collect_table_names(query, &mut names);
    let mut tables: HashMap<String, PreparedTable> = HashMap::new();
    for raw in &names {
        let key = normalize_table_key(raw);
        if tables.contains_key(&key) {
            continue;
        }
        match dml.snapshot_table_for_relational(raw).await {
            Ok((catalog_schema, rows)) => {
                tables.insert(key, PreparedTable::from_catalog(&catalog_schema, rows));
            }
            Err(e) => {
                tracing::debug!(
                    target: "proximadb::pgwire::new_pipeline",
                    "relational snapshot for `{raw}` failed: {e}; falling through to legacy"
                );
                return None;
            }
        }
    }

    let snapshot = SnapshotCatalog { tables };
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
    // From here, errors are real and surface to the client.
    Some(run_plan(&snapshot, logical).await)
}

/// Plan + build + drain an executor for `logical` against `factory`.
async fn run_plan<F: ReaderFactory>(
    factory: &F,
    logical: proximadb_relational_algebra::LogicalNode,
) -> Result<PipelineResult, String> {
    let planner = Planner::new(StaticCapabilities {
        caps: ReaderCapabilities::full(),
        pk_columns: Vec::new(),
    });
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

/// One table's pre-fetched schema + rows, ready to hand to a [`VecReader`].
struct PreparedTable {
    schema: RelationalSchema,
    rows: Vec<RelationalRow>,
    pk_columns: Vec<usize>,
}

impl PreparedTable {
    fn from_catalog(
        catalog_schema: &proximadb_catalog::CatalogTableSchema,
        rows: Vec<RelationalRow>,
    ) -> Self {
        let columns: Vec<ColumnInfo> = catalog_schema
            .columns
            .iter()
            .map(|c| ColumnInfo::new(c.name.clone(), c.data_type.to_proxima_type(), c.nullable))
            .collect();
        // PK ordinals within the column list (matches `rows` column order,
        // which is also `catalog_schema.columns` order — see
        // `DmlService::snapshot_table_for_relational`).
        let pk_columns: Vec<usize> = catalog_schema
            .primary_key
            .iter()
            .filter_map(|pk| catalog_schema.columns.iter().position(|c| &c.name == pk))
            .collect();
        Self {
            schema: RelationalSchema::new(columns),
            rows,
            pk_columns,
        }
    }
}

/// Pre-resolved real-data catalog. Implements both [`CatalogLookup`] (for
/// lowering) and [`ReaderFactory`] (for execution) over the same snapshot, so
/// the schema the frontend lowers against is exactly the one the reader emits.
struct SnapshotCatalog {
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
        Ok(Box::new(VecReader::new(
            prepared.schema.clone(),
            prepared.rows.clone(),
            prepared.pk_columns.clone(),
        )))
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
}
