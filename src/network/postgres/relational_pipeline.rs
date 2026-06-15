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
    ExecError, ExecMetrics, ExecutionContext, NodeMetric, ReaderFactory, build_executor, collect,
};
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
    #[cfg_attr(not(feature = "datafusion-integration"), allow(unused_variables))]
    vector_ops: Option<Arc<dyn proximadb_runtime::VectorOpsPort>>,
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
    // Engaged = a relational shape the legacy single-table path can't serve
    // (joins / GROUP BY / aggregates / set-ops). Simple SELECTs stay on legacy.
    if !query_engages_relational_engine(query) {
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
        match dml.resolve_relational_schema(raw).await {
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

    let decision = crate::query::compute_scheduler::ComputeScheduler::new().route_select(
        crate::query::compute_scheduler::QueryShape {
            engages_relational: true,
            parquet_backed,
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
        return Some(run_datafusion_select(sql, &parquet_tables, vector_ops).await);
    }

    let snapshot = SnapshotCatalog {
        dml: dml.clone(),
        tables,
    };
    // Lower + plan via the shared planning path (so EXPLAIN discloses exactly this
    // plan). `None` → lowering declined → fall through to legacy. From the planned
    // physical onward, errors are real and surface to the client.
    let physical = match plan_over_snapshot(sql, &snapshot)? {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    Some(execute_physical(physical, &snapshot).await)
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
    execute_physical(physical, factory).await
}

/// Build + open + drain an executor for an already-planned `physical` plan.
/// Shared tail of `run_plan` and the real-data path so EXPLAIN (which plans
/// without executing) and execution run the SAME `PhysicalPlan`.
async fn execute_physical<F: ReaderFactory>(
    physical: PhysicalPlan,
    factory: &F,
) -> Result<PipelineResult, String> {
    let mut exec = build_executor(physical, factory, &ExecutionContext::default())
        .map_err(|e| format!("build_executor: {e}"))?;
    exec.open().await.map_err(|e| format!("open: {e}"))?;
    let schema = exec.schema().clone();
    let rows = collect(&mut *exec)
        .await
        .map_err(|e| format!("scan: {e}"))?;
    Ok(PipelineResult { schema, rows })
}

/// Like [`execute_physical`] but meters every operator (EXPLAIN ANALYZE): returns the
/// result plus per-operator [`NodeMetric`]s in pre-order (aligned with
/// `explain_physical`'s line order). The executor tree is dropped before reading the
/// metrics so each `MeteredExec` has flushed its counters.
async fn execute_physical_metered<F: ReaderFactory>(
    physical: PhysicalPlan,
    factory: &F,
) -> Result<(PipelineResult, Vec<NodeMetric>), String> {
    let metrics = Arc::new(ExecMetrics::new());
    let result = {
        let mut exec = build_executor(
            physical,
            factory,
            &ExecutionContext::with_metrics(metrics.clone()),
        )
        .map_err(|e| format!("build_executor: {e}"))?;
        exec.open().await.map_err(|e| format!("open: {e}"))?;
        let schema = exec.schema().clone();
        let rows = collect(&mut *exec)
            .await
            .map_err(|e| format!("scan: {e}"))?;
        PipelineResult { schema, rows }
        // `exec` dropped here → MeteredExec counters flush into `metrics`.
    };
    Ok((result, metrics.snapshot()))
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

/// Map an Arrow column type to the relational `ProximaType` for the result schema.
#[cfg(feature = "datafusion-integration")]
fn arrow_type_to_proxima(dt: &arrow_schema::DataType) -> ProximaType {
    use arrow_schema::DataType as D;
    match dt {
        D::Boolean => ProximaType::Boolean,
        D::Int8 => ProximaType::Int8,
        D::Int16 => ProximaType::Int16,
        D::Int32 => ProximaType::Int32,
        D::Int64 => ProximaType::Int64,
        D::UInt8 => ProximaType::UInt8,
        D::UInt16 => ProximaType::UInt16,
        D::UInt32 => ProximaType::UInt32,
        D::UInt64 => ProximaType::UInt64,
        D::Float16 | D::Float32 => ProximaType::Float32,
        D::Float64 => ProximaType::Float64,
        D::Utf8 | D::LargeUtf8 => ProximaType::String,
        D::Binary | D::LargeBinary => ProximaType::Binary,
        D::Date32 | D::Date64 => ProximaType::Date,
        // Other Arrow types are rendered to text in the cell converter.
        _ => ProximaType::String,
    }
}

/// Convert one Arrow cell to a `ProximaValue`. Common scalar types map directly;
/// anything else falls back to its Arrow text rendering (so it still reaches the
/// pgwire client via `text_encode`).
#[cfg(feature = "datafusion-integration")]
fn arrow_cell_to_proxima(array: &dyn arrow_array::Array, row: usize) -> ProximaValue {
    use arrow_array::*;
    use arrow_schema::DataType as D;
    if array.is_null(row) {
        return ProximaValue::Null;
    }
    macro_rules! v {
        ($t:ty) => {
            array.as_any().downcast_ref::<$t>().unwrap().value(row)
        };
    }
    match array.data_type() {
        D::Boolean => ProximaValue::Boolean(v!(BooleanArray)),
        D::Int8 => ProximaValue::Int8(v!(Int8Array)),
        D::Int16 => ProximaValue::Int16(v!(Int16Array)),
        D::Int32 => ProximaValue::Int32(v!(Int32Array)),
        D::Int64 => ProximaValue::Int64(v!(Int64Array)),
        D::UInt8 => ProximaValue::UInt8(v!(UInt8Array)),
        D::UInt16 => ProximaValue::UInt16(v!(UInt16Array)),
        D::UInt32 => ProximaValue::UInt32(v!(UInt32Array)),
        D::UInt64 => ProximaValue::UInt64(v!(UInt64Array)),
        D::Float32 => ProximaValue::Float32(v!(Float32Array)),
        D::Float64 => ProximaValue::Float64(v!(Float64Array)),
        D::Utf8 => ProximaValue::String(v!(StringArray).to_string()),
        D::LargeUtf8 => ProximaValue::String(v!(LargeStringArray).to_string()),
        D::Binary => ProximaValue::Binary(v!(BinaryArray).to_vec()),
        D::Date32 => ProximaValue::Date(v!(Date32Array)),
        _ => match arrow::util::display::ArrayFormatter::try_new(
            array,
            &arrow::util::display::FormatOptions::default(),
        ) {
            Ok(f) => ProximaValue::String(f.value(row).to_string()),
            Err(_) => ProximaValue::Null,
        },
    }
}

/// Convert DataFusion result batches into a `PipelineResult` so the existing pgwire
/// emitter (`text_encode`) renders them unchanged.
#[cfg(feature = "datafusion-integration")]
fn record_batches_to_pipeline_result(
    arrow_schema: &arrow_schema::Schema,
    batches: &[arrow_array::RecordBatch],
) -> PipelineResult {
    let columns: Vec<ColumnInfo> = arrow_schema
        .fields()
        .iter()
        .map(|f| {
            ColumnInfo::new(
                f.name().clone(),
                arrow_type_to_proxima(f.data_type()),
                f.is_nullable(),
            )
        })
        .collect();
    let schema = RelationalSchema::new(columns);
    let mut rows: Vec<RelationalRow> = Vec::new();
    for batch in batches {
        let ncols = batch.num_columns();
        for r in 0..batch.num_rows() {
            let mut row: RelationalRow = Vec::with_capacity(ncols);
            for c in 0..ncols {
                row.push(arrow_cell_to_proxima(batch.column(c).as_ref(), r));
            }
            rows.push(row);
        }
    }
    PipelineResult { schema, rows }
}

/// Execute an OLAP `SELECT` through DataFusion over Parquet-backed table(s), each
/// read through the warehouse object-store bridge (course-correction §6 P3/F5).
/// The query is lowered through the SAME relational frontend the Volcano path uses
/// and then (P4) into a DataFusion `LogicalPlan` — so both physical engines share
/// one logical plane (§5). Shapes the shared lowering doesn't cover yet (e.g. JOIN)
/// fall back to DataFusion's own SQL frontend. Results convert back to a
/// `PipelineResult`.
#[cfg(feature = "datafusion-integration")]
async fn run_datafusion_select(
    sql: &str,
    parquet_tables: &[(String, String)],
    vector_ops: Option<Arc<dyn proximadb_runtime::VectorOpsPort>>,
) -> Result<PipelineResult, String> {
    // F4: when the route owns the vector service, register the `vector_search` UDTF so a
    // cross-modal `... JOIN vector_search('coll','[..]',k)` is expressible over this path.
    let ctx = match vector_ops {
        Some(ops) => crate::datafusion::create_session_context_with_vector_ops(ops),
        None => crate::datafusion::create_session_context(),
    }
    .map_err(|e| format!("session: {e}"))?;
    for (name, location) in parquet_tables {
        crate::datafusion::register_object_store_parquet_location(&ctx, name, location)
            .await
            .map_err(|e| format!("register object-store parquet table {name}: {e}"))?;
    }
    // §5 shared logical plane (P4): lower the SQL through the SAME relational
    // frontend the Volcano path uses, then lower that `LogicalNode` to a DataFusion
    // `LogicalPlan`. The frontend catalog is built from the registered Parquet
    // schemas (`arrow_type_to_proxima`). Shapes the lowering doesn't cover yet
    // (JOIN / UNION / DISTINCT) fall back to DataFusion's own SQL frontend, so the
    // route never regresses.
    let mut schemas: HashMap<String, RelationalSchema> = HashMap::new();
    for (name, _location) in parquet_tables {
        let provider = ctx
            .table_provider(name.as_str())
            .await
            .map_err(|e| format!("table_provider({name}): {e}"))?;
        let cols: Vec<ColumnInfo> = provider
            .schema()
            .fields()
            .iter()
            .map(|f| {
                ColumnInfo::new(
                    f.name(),
                    arrow_type_to_proxima(f.data_type()),
                    f.is_nullable(),
                )
            })
            .collect();
        schemas.insert(normalize_table_key(name), RelationalSchema::new(cols));
    }
    let catalog = ParquetSchemaCatalog { schemas };

    // `lower_sql` (sync) → relational `LogicalNode`; `lower_logical_node` (async,
    // P4) → DataFusion `LogicalPlan`. Either declining (parse miss / unsupported
    // node) yields `None` and we fall back to `ctx.sql`.
    let lowered_plan = match lower_sql(sql, &catalog) {
        Ok(node) => crate::datafusion::logical_lowering::lower_logical_node(&ctx, &node)
            .await
            .ok(),
        Err(_) => None,
    };

    let df = match lowered_plan {
        Some(plan) => {
            tracing::debug!(
                target: "proximadb::compute_route",
                "DataFusion route via shared relational frontend (P4 lowering)"
            );
            ctx.execute_logical_plan(plan)
                .await
                .map_err(|e| format!("datafusion execute_logical_plan: {e}"))?
        }
        None => {
            tracing::debug!(
                target: "proximadb::compute_route",
                "DataFusion route via ctx.sql fallback (shape not yet in shared lowering)"
            );
            ctx.sql(sql)
                .await
                .map_err(|e| format!("datafusion sql: {e}"))?
        }
    };
    let arrow_schema = df.schema().as_arrow().clone();
    let batches = df
        .collect()
        .await
        .map_err(|e| format!("datafusion collect: {e}"))?;
    Ok(record_batches_to_pipeline_result(&arrow_schema, &batches))
}

/// Sync `CatalogLookup` over the schemas of the Parquet tables registered for a
/// DataFusion route, so `lower_sql` can resolve them. Built from the Arrow schema of
/// each registered provider (`arrow_type_to_proxima`); keyed by `normalize_table_key`.
#[cfg(feature = "datafusion-integration")]
struct ParquetSchemaCatalog {
    schemas: HashMap<String, RelationalSchema>,
}

#[cfg(feature = "datafusion-integration")]
impl CatalogLookup for ParquetSchemaCatalog {
    fn lookup_table(&self, name: &str) -> Option<RelationalSchema> {
        self.schemas.get(&normalize_table_key(name)).cloned()
    }
}

#[cfg(all(test, feature = "datafusion-integration"))]
mod datafusion_route_tests {
    use super::*;
    use arrow_array::{Float64Array, Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    #[test]
    fn record_batches_convert_to_pipeline_result() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("service", DataType::Utf8, false),
            Field::new("n", DataType::Int64, false),
            Field::new("avg_x", DataType::Float64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["api", "db"])),
                Arc::new(Int64Array::from(vec![3_i64, 5])),
                Arc::new(Float64Array::from(vec![Some(1.5), None])),
            ],
        )
        .unwrap();

        let result = record_batches_to_pipeline_result(&schema, &[batch]);
        assert_eq!(result.schema.columns.len(), 3);
        assert_eq!(result.schema.columns[0].name, "service");
        assert_eq!(result.schema.columns[1].ty, ProximaType::Int64);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], ProximaValue::String("api".to_string()));
        assert_eq!(result.rows[0][1], ProximaValue::Int64(3));
        assert_eq!(result.rows[0][2], ProximaValue::Float64(1.5));
        // Null preserved.
        assert_eq!(result.rows[1][2], ProximaValue::Null);
    }

    #[test]
    fn catalog_table_is_parquet_backed_detects_external_parquet() {
        use proximadb_catalog::{CatalogPhysicalFormat, CatalogStorageLayout, CatalogTableSchema};

        // External Parquet table → returns the location.
        let layout = CatalogStorageLayout::external_authoritative(
            "ext",
            CatalogPhysicalFormat::Parquet,
            "file:///data/ext.parquet",
        );
        let parquet_table = CatalogTableSchema::default().with_storage_layout(layout);
        assert_eq!(
            catalog_table_is_parquet_backed(&parquet_table).as_deref(),
            Some("file:///data/ext.parquet")
        );

        // A default (InternalCanonical / ProximaBlock) table is not Parquet-backed.
        assert!(catalog_table_is_parquet_backed(&CatalogTableSchema::default()).is_none());
    }

    #[tokio::test]
    async fn parquet_split_summary_discloses_row_group_inventory() {
        use parquet::arrow::ArrowWriter;
        use parquet::file::properties::WriterProperties;

        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1_i64, 2, 3, 4]))],
        )
        .unwrap();
        {
            // Two row groups of 2 rows each → 4 rows / 2 partitions.
            let props = WriterProperties::builder()
                .set_max_row_group_size(2)
                .build();
            let file = std::fs::File::create(data_dir.join("part-0.parquet")).unwrap();
            let mut writer = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }

        let location = format!("file://{}", tmp.path().display());
        let summary = parquet_split_summary(&[location])
            .await
            .expect("split summary for a Parquet-backed location");

        assert_eq!(
            summary.strategy,
            crate::query::read_route::ReadSplitStrategy::RowGroup
        );
        assert_eq!(summary.partition_count, 2);
        assert_eq!(summary.estimated_rows, Some(4));
        assert_eq!(summary.stats_freshness, "fresh");

        // No locations → no disclosure (caller keeps the conservative summary).
        assert!(parquet_split_summary(&[]).await.is_none());
    }

    /// Full P3 chain through the ROUTING path: a native table is created + populated,
    /// materialized to Parquet (catalog layout flipped to ProjectionPublication), and
    /// an OLAP-shape SELECT through `try_run_select` then routes to DataFusion over the
    /// published Parquet — not the native Volcano path — returning the aggregated rows.
    /// This is the end-to-end proof gating the `datafusion-integration` default flip.
    #[tokio::test]
    async fn materialized_native_table_routes_select_to_datafusion_end_to_end() {
        use crate::catalog::CatalogManager;
        use crate::query::table_write_executor::PlannedOnlyTableWriteExecutor;
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{
            DdlService, DdlStatement, FramedTableWalAppender, MemtableRecordStorage,
        };
        use proximadb_iceberg_engine::IcebergObjectStoreBridge;

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("route-e2e.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        let ddl = DdlService::new(manager.clone());
        ddl.execute(DdlStatement::CreateNamespace {
            namespace: vec!["default".to_string()],
            if_not_exists: true,
            properties: HashMap::new(),
        })
        .await
        .expect("create namespace");
        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE inv_route_e2e (id TEXT NOT NULL, status TEXT, qty INT, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            Arc::new(FramedTableWalAppender::open(&wal_path).await.expect("open WAL")),
        ));
        let dml = Arc::new(DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        ));
        for (id, status, qty) in [("i1", "active", 5), ("i2", "active", 15), ("i3", "idle", 25)] {
            let stmt = parser
                .parse_dml(&format!(
                    "INSERT INTO inv_route_e2e (id, status, qty) VALUES ('{id}', '{status}', {qty});"
                ))
                .expect("parse insert")
                .expect("insert stmt");
            dml.execute(stmt).await.expect("insert");
        }

        // P3 materialize: publish a Parquet snapshot to a file:// store the OLAP reader
        // can reopen, and flip the catalog layout to Parquet/ProjectionPublication.
        let store_dir = tempfile::tempdir().expect("store tempdir");
        let root_url = format!("file://{}", store_dir.path().display());
        let bridge = Arc::new(IcebergObjectStoreBridge::from_url(&root_url).expect("bridge"));
        dml.materialize_table_to_parquet(&*bridge, &root_url, "inv_route_e2e", None)
            .await
            .expect("materialize");

        let sql = "SELECT status, count(*) AS c FROM inv_route_e2e GROUP BY status ORDER BY status";

        // Route disclosure proves the engine choice end-to-end: DataFusion over the
        // published Parquet, with the concrete row-group split inventory (P1).
        let explain = explain_select_route_with_catalog(sql, &dml)
            .await
            .expect("explain route");
        assert_eq!(explain.compute_route, "DataFusionLocal");
        assert_eq!(explain.read_route.split_strategy, "row_group");
        assert_eq!(explain.read_route.partition_count, 1);

        // Execution returns the correct aggregates read from the materialized Parquet.
        let result = try_run_select(sql, Some(&dml), None)
            .await
            .expect("pipeline engaged")
            .expect("select ok");
        assert_eq!(result.rows.len(), 2, "two status groups");
        assert_eq!(result.rows[0][0], ProximaValue::String("active".to_string()));
        assert_eq!(result.rows[0][1], ProximaValue::Int64(2));
        assert_eq!(result.rows[1][0], ProximaValue::String("idle".to_string()));
        assert_eq!(result.rows[1][1], ProximaValue::Int64(1));
    }

    #[tokio::test]
    async fn run_datafusion_select_executes_olap_over_parquet() {
        use parquet::arrow::ArrowWriter;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "a", "b"])),
                Arc::new(Float64Array::from(vec![1.0, 3.0, 10.0])),
            ],
        )
        .unwrap();
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut writer = ArrowWriter::try_new(file, schema, None).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }

        let url = format!("file://{}", path.display());
        let result = run_datafusion_select(
            "SELECT k, count(*) AS c, sum(x) AS s FROM t GROUP BY k ORDER BY k",
            &[("t".to_string(), url)],
            None,
        )
        .await
        .expect("datafusion select over parquet");

        // Two groups: a (count 2, sum 4.0), b (count 1, sum 10.0).
        assert_eq!(result.schema.columns.len(), 3);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], ProximaValue::String("a".to_string()));
        assert_eq!(result.rows[0][1], ProximaValue::Int64(2));
        assert_eq!(result.rows[0][2], ProximaValue::Float64(4.0));
        assert_eq!(result.rows[1][0], ProximaValue::String("b".to_string()));
        assert_eq!(result.rows[1][1], ProximaValue::Int64(1));
        assert_eq!(result.rows[1][2], ProximaValue::Float64(10.0));
    }

    /// Guards against a silent regression to the `ctx.sql` fallback: the canonical
    /// OLAP aggregate query MUST lower through the SHARED relational frontend
    /// (`lower_sql` → `LogicalNode`) and P4 (`lower_logical_node` → DataFusion
    /// `LogicalPlan`), then execute — proving `run_datafusion_select` takes the
    /// shared-logical-plane path, not the fallback (§5).
    #[tokio::test]
    async fn datafusion_route_uses_shared_frontend_not_fallback() {
        use datafusion::datasource::MemTable;

        let ctx = crate::datafusion::create_session_context().expect("session ctx");
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b"])),
                Arc::new(Float64Array::from(vec![1.0, 2.0])),
            ],
        )
        .unwrap();
        let mem = MemTable::try_new(schema, vec![vec![batch]]).unwrap();
        ctx.register_table("t", Arc::new(mem)).unwrap();

        // Build the catalog exactly as `run_datafusion_select` does.
        let provider = ctx.table_provider("t").await.unwrap();
        let cols: Vec<ColumnInfo> = provider
            .schema()
            .fields()
            .iter()
            .map(|f| {
                ColumnInfo::new(
                    f.name(),
                    arrow_type_to_proxima(f.data_type()),
                    f.is_nullable(),
                )
            })
            .collect();
        let mut schemas = HashMap::new();
        schemas.insert(normalize_table_key("t"), RelationalSchema::new(cols));
        let catalog = ParquetSchemaCatalog { schemas };

        // MUST lower via the shared frontend — `.expect` here fails loudly if the
        // route silently degraded to the `ctx.sql` fallback.
        let node = lower_sql(
            "SELECT k, count(*) AS c, sum(x) AS s FROM t GROUP BY k ORDER BY k",
            &catalog,
        )
        .expect("shared relational frontend must lower the aggregate query");
        let plan = crate::datafusion::logical_lowering::lower_logical_node(&ctx, &node)
            .await
            .expect("P4 must lower the aggregate LogicalNode to a DataFusion plan");
        let batches = ctx
            .execute_logical_plan(plan)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 2);
    }
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
            .map(|c| ColumnInfo::new(c.name.clone(), c.data_type.clone(), c.nullable))
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
                parquet_backed: false,
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
) -> Option<(SnapshotCatalog, PhysicalPlan)> {
    let mut names = Vec::new();
    collect_table_names(query, &mut names);
    let mut tables: HashMap<String, PreparedTable> = HashMap::new();
    for raw in &names {
        let key = normalize_table_key(raw);
        if tables.contains_key(&key) {
            continue;
        }
        let schema = dml.resolve_relational_schema(raw).await.ok()?;
        tables.insert(key, PreparedTable::from_catalog(raw, &schema));
    }
    let snapshot = SnapshotCatalog {
        dml: dml.clone(),
        tables,
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
) -> Result<SelectRouteExplanation, String> {
    route_and_plan_select(sql, dml, false).await
}

/// Catalog-aware `EXPLAIN ANALYZE SELECT`: like [`explain_select_route_with_catalog`]
/// but ALSO executes the planned native (Volcano) plan and attaches the measured
/// `execution_rows` + `execution_elapsed_us`. SELECT is read-only, so executing it is
/// side-effect free (matches Postgres EXPLAIN ANALYZE). Execution errors propagate.
pub async fn explain_analyze_select_with_catalog(
    sql: &str,
    dml: &Arc<DmlService>,
) -> Result<SelectRouteExplanation, String> {
    route_and_plan_select(sql, dml, true).await
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
                match dml.resolve_relational_schema(raw).await {
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
    let decision = crate::query::compute_scheduler::ComputeScheduler::new().route_select(
        crate::query::compute_scheduler::QueryShape {
            engages_relational: engages,
            parquet_backed,
        },
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
    )
        && let Some(summary) = parquet_split_summary(&parquet_locations).await {
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
        && let Some((snapshot, physical)) = prepare_select_plan(sql, query, dml).await {
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
                .any(select_item_has_aggregate);
            // A WHERE subquery (IN/EXISTS) lowers to a Semi/Anti join — a shape the
            // legacy single-table path can't serve — so engage PATH B. If it turns out
            // unliftable (correlated/NOT IN), lowering declines and the caller falls
            // through to legacy anyway, so engaging on ANY subquery is safe.
            let has_where_subquery = select.selection.as_ref().is_some_and(where_has_subquery);
            // A scalar subquery in the projection list (e.g.
            // `SELECT a, (SELECT max(x) FROM t)`) is hoisted by PATH B into a LEFT
            // JOIN; the legacy single-table path can't serve it. Same safety as the
            // WHERE case — an unliftable shape declines back to legacy.
            let has_projection_subquery = select
                .projection
                .iter()
                .any(select_item_has_subquery);
            has_join
                || has_group_by
                || select.having.is_some()
                || has_aggregate
                || has_where_subquery
                || has_projection_subquery
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
