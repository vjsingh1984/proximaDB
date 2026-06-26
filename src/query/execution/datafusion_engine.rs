#[cfg(feature = "datafusion-integration")]
use crate::query::execution::engine::normalize_table_key;
use crate::query::execution::engine::{
    ExecutionEngine, ExecutionError, ExecutionPipelineResult, ExecutionStreamResult,
    QueryExecutionContext, RowLimitMode,
};
use async_trait::async_trait;
#[cfg(feature = "datafusion-integration")]
use futures::{StreamExt, stream};
#[cfg(feature = "datafusion-integration")]
use proximadb_data_model::TimeUnit;
#[cfg(feature = "datafusion-integration")]
use proximadb_relational_frontend::lower_sql;
#[cfg(feature = "datafusion-integration")]
use proximadb_relational_types::{ColumnInfo, RelationalRow, RelationalSchema};
#[cfg(feature = "datafusion-integration")]
use std::collections::{HashMap, VecDeque};

/// Execution engine backed by local DataFusion.
///
/// This implementation provides high-performance analytical execution over
/// Parquet-backed tables, utilizing DataFusion's query optimizer and
/// vectorized execution engine.
pub struct DataFusionLocalEngine;

#[async_trait]
impl ExecutionEngine for DataFusionLocalEngine {
    async fn execute_sql(
        &self,
        sql: &str,
        context: QueryExecutionContext,
    ) -> Result<ExecutionPipelineResult, ExecutionError> {
        #[cfg(feature = "datafusion-integration")]
        {
            self.execute_datafusion(sql, context).await
        }
        #[cfg(not(feature = "datafusion-integration"))]
        {
            let _ = (sql, context);
            Err(ExecutionError::FeatureDisabled("datafusion-integration"))
        }
    }

    async fn execute_sql_stream(
        &self,
        sql: &str,
        context: QueryExecutionContext,
    ) -> Result<ExecutionStreamResult, ExecutionError> {
        #[cfg(feature = "datafusion-integration")]
        {
            self.execute_datafusion_stream(sql, context).await
        }
        #[cfg(not(feature = "datafusion-integration"))]
        {
            let _ = (sql, context);
            Err(ExecutionError::FeatureDisabled("datafusion-integration"))
        }
    }
}

#[cfg(feature = "datafusion-integration")]
impl DataFusionLocalEngine {
    /// Build, plan, and row-cap a DataFusion `DataFrame` for `sql`, shared by the
    /// materialized and streaming execution paths.
    ///
    /// Returns the owning `SessionContext` alongside the frame so callers keep it
    /// alive for the duration of execution. The row cap (if any) is pushed into
    /// the plan here via `df.limit`, so the executor stops early instead of
    /// materializing the whole result.
    async fn prepare_dataframe(
        &self,
        sql: &str,
        context: &QueryExecutionContext,
    ) -> Result<
        (
            datafusion::prelude::SessionContext,
            datafusion::prelude::DataFrame,
        ),
        ExecutionError,
    > {
        context.controls.check_cancelled()?;
        // F4: when the route owns the vector service, register the `vector_search` UDTF
        let ctx = match context.vector_ops.clone() {
            Some(ops) => crate::datafusion::create_session_context_with_vector_ops(ops),
            None => crate::datafusion::create_session_context(),
        }
        .map_err(|e| ExecutionError::Context(format!("session: {e}")))?;

        for (name, location) in &context.parquet_tables {
            // ADR-025 (relational cold path): an opt-in table reads its cold
            // Parquet base reconciled with the authoritative post-snapshot WAL
            // delta (deletes/updates/inserts after the `MATERIALIZE` snapshot),
            // keyed by canonical oid. Non-opt-in tables fall through to the bare
            // Parquet read below (default-OFF), so the OLAP ratchet tables are
            // untouched.
            if let Some(cfg) = &context.olap_delta {
                if let Some(tbl) = cfg.tables.get(&normalize_table_key(name)) {
                    register_merged_olap_table(
                        &ctx,
                        name,
                        location,
                        tbl,
                        cfg.source.as_ref(),
                        context.tenant_id.as_deref(),
                    )
                    .await?;
                    continue;
                }
            }
            let table =
                crate::datafusion::register_object_store_parquet_location(&ctx, name, location)
                    .await
                    .map_err(|e| {
                        ExecutionError::Context(format!(
                            "register object-store parquet table {name}: {e}"
                        ))
                    })?;
            // Warm the route-time shape cache for free — the footer is already
            // read, so the next route decision can classify this location's
            // fan-out / cardinality without a cold read (co-design: zero extra
            // I/O on the route path).
            crate::query::route_cost_model::record_table_shape_stat(
                location,
                table.split_count() as u32,
                table.estimated_rows(),
            );
        }

        context.controls.check_cancelled()?;
        // §5 shared logical plane (P4): lower the SQL through the SAME relational
        // frontend the Volcano path uses, then lower that `LogicalNode` to a DataFusion
        // `LogicalPlan`.
        let mut schemas: HashMap<String, RelationalSchema> = HashMap::new();
        for (name, _) in &context.parquet_tables {
            let provider = ctx
                .table_provider(name.as_str())
                .await
                .map_err(|e| ExecutionError::Context(format!("table_provider({name}): {e}")))?;

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

        // P4 lowering
        let lowered_plan = match lower_sql(sql, &catalog) {
            Ok(node) => {
                match crate::datafusion::logical_lowering::lower_logical_node(&ctx, &node).await {
                    Ok(plan) => Some(plan),
                    // The shared frontend accepted the SQL but the DataFusion
                    // logical-node lowering can't yet express it. Rather than hard
                    // failing, fall back to DataFusion's own ANSI SQL planner over
                    // the same registered Parquet tables (full ANSI coverage).
                    Err(e) if context.allow_engine_sql_fallback => {
                        tracing::debug!(
                            target: "proximadb::compute_route",
                            error = %e,
                            "shared frontend lowered but DataFusion logical-node lowering declined; using DataFusion SQL fallback"
                        );
                        None
                    }
                    Err(e) => {
                        return Err(ExecutionError::Planning(format!("lower logical node: {e}")));
                    }
                }
            }
            Err(e) if context.allow_engine_sql_fallback => {
                tracing::debug!(
                    target: "proximadb::compute_route",
                    error = %e,
                    "shared relational frontend declined SQL; using explicit DataFusion SQL fallback"
                );
                None
            }
            Err(e) => {
                return Err(ExecutionError::Planning(format!(
                    "shared relational frontend declined SQL: {e}"
                )));
            }
        };

        let df = match lowered_plan {
            Some(plan) => {
                tracing::debug!(
                    target: "proximadb::compute_route",
                    "DataFusion engine via shared relational frontend (P4 lowering)"
                );
                ctx.execute_logical_plan(plan)
                    .await
                    .map_err(|e| ExecutionError::Execution(format!("execute logical plan: {e}")))?
            }
            None => {
                tracing::debug!(
                    target: "proximadb::compute_route",
                    "DataFusion engine via DataFusion SQL frontend (fallback)"
                );
                ctx.sql(sql)
                    .await
                    .map_err(|e| ExecutionError::Execution(format!("sql: {e}")))?
            }
        };
        let df = match context.controls.max_rows {
            Some(max_rows) => {
                let fetch = match context.controls.row_limit_mode {
                    RowLimitMode::Truncate => max_rows,
                    RowLimitMode::Error => max_rows.saturating_add(1),
                };
                df.limit(0, Some(fetch))
                    .map_err(|e| ExecutionError::Planning(format!("apply row cap: {e}")))?
            }
            None => df,
        };

        Ok((ctx, df))
    }

    /// Execute a DataFusion query and fully materialize the result.
    async fn execute_datafusion(
        &self,
        sql: &str,
        context: QueryExecutionContext,
    ) -> Result<ExecutionPipelineResult, ExecutionError> {
        // `_ctx` is held until after collection: the frame's table providers were
        // resolved against this session.
        let (_ctx, df) = self.prepare_dataframe(sql, &context).await?;
        let arrow_schema = df.schema().as_arrow().clone();
        let batches = df
            .collect()
            .await
            .map_err(|e| ExecutionError::Execution(format!("collect: {e}")))?;
        context.controls.check_cancelled()?;
        let result = record_batches_to_pipeline_result(&arrow_schema, &batches);
        match context.controls.row_limit_mode {
            RowLimitMode::Error => result.enforce_row_limit(context.controls.max_rows),
            RowLimitMode::Truncate => Ok(result),
        }
    }

    /// Execute a DataFusion query and stream rows through the schema-bearing
    /// contract without materializing the whole result.
    ///
    /// Uses DataFusion's native `execute_stream`, converting each `RecordBatch`
    /// to rows lazily. The row cap is already pushed into the plan by
    /// `prepare_dataframe`; in `Error` mode the (n+1)th row the plan lets through
    /// is surfaced as an overflow here, matching the materialized path.
    /// Cancellation is re-checked before every pull.
    async fn execute_datafusion_stream(
        &self,
        sql: &str,
        context: QueryExecutionContext,
    ) -> Result<ExecutionStreamResult, ExecutionError> {
        let (ctx, df) = self.prepare_dataframe(sql, &context).await?;
        let arrow_schema = df.schema().as_arrow().clone();
        let schema = arrow_schema_to_relational(&arrow_schema);

        context.controls.check_cancelled()?;
        let batch_stream = df
            .execute_stream()
            .await
            .map_err(|e| ExecutionError::Execution(format!("execute_stream: {e}")))?;

        // Stream state: the session (held alive for the stream's lifetime), the
        // DataFusion batch stream, a row buffer drained from each batch, the count
        // of rows already emitted (for the Error-mode guard), and the controls.
        let rows = stream::try_unfold(
            (
                ctx,
                batch_stream,
                VecDeque::<RelationalRow>::new(),
                0usize,
                context.controls,
            ),
            |(_ctx, mut batch_stream, mut buffer, emitted, controls)| async move {
                controls.check_cancelled()?;
                // Refill the row buffer from batches until it has a row or the
                // underlying stream is exhausted.
                while buffer.is_empty() {
                    match batch_stream.next().await {
                        Some(Ok(batch)) => buffer.extend(record_batch_to_rows(&batch)),
                        Some(Err(e)) => {
                            return Err(ExecutionError::Execution(format!("scan: {e}")));
                        }
                        None => return Ok(None),
                    }
                }
                match buffer.pop_front() {
                    Some(row) => {
                        if controls.row_limit_mode == RowLimitMode::Error
                            && let Some(limit) = controls.max_rows
                            && emitted >= limit
                        {
                            return Err(ExecutionError::RowLimitExceeded {
                                limit,
                                actual: emitted + 1,
                            });
                        }
                        Ok(Some((
                            row,
                            (_ctx, batch_stream, buffer, emitted + 1, controls),
                        )))
                    }
                    None => Ok(None),
                }
            },
        )
        .boxed();

        Ok(ExecutionStreamResult { schema, rows })
    }
}

#[cfg(feature = "datafusion-integration")]
struct ParquetSchemaCatalog {
    schemas: HashMap<String, RelationalSchema>,
}

#[cfg(feature = "datafusion-integration")]
impl proximadb_relational_frontend::CatalogLookup for ParquetSchemaCatalog {
    fn lookup_table(&self, name: &str) -> Option<RelationalSchema> {
        self.schemas.get(&normalize_table_key(name)).cloned()
    }
}

#[cfg(all(test, feature = "datafusion-integration"))]
mod tests {
    use super::*;
    use crate::query::execution::engine::{ExecutionControls, RowLimitMode};
    use arrow_array::{Float64Array, Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use proximadb_data_model::{ProximaType, ProximaValue};
    use std::sync::{Arc, atomic::AtomicBool};

    fn write_grouped_parquet() -> (tempfile::TempDir, String) {
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
        let location = format!("file://{}", path.display());
        (tmp, location)
    }

    #[test]
    fn test_record_batches_to_pipeline_result() {
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
        assert_eq!(result.rows[1][2], ProximaValue::Null);
    }

    #[tokio::test]
    async fn test_datafusion_engine_executes_sql_over_parquet() {
        let (_tmp, location) = write_grouped_parquet();

        let engine = DataFusionLocalEngine;
        let context = QueryExecutionContext {
            parquet_tables: vec![("t".to_string(), location)],
            ..Default::default()
        };

        // Aggregated OLAP query
        let sql = "SELECT k, SUM(x) as total FROM t GROUP BY k ORDER BY k";
        let result = engine.execute_sql(sql, context).await.unwrap();

        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], ProximaValue::String("a".to_string()));
        assert_eq!(result.rows[0][1], ProximaValue::Float64(4.0));
        assert_eq!(result.rows[1][0], ProximaValue::String("b".to_string()));
        assert_eq!(result.rows[1][1], ProximaValue::Float64(10.0));
    }

    #[tokio::test]
    async fn datafusion_engine_honors_cancelled_context() {
        let flag = Arc::new(AtomicBool::new(true));
        let engine = DataFusionLocalEngine;
        let err = engine
            .execute_sql(
                "SELECT 1",
                QueryExecutionContext {
                    controls: ExecutionControls {
                        cancellation_flag: Some(flag),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect_err("cancelled query should not execute");

        assert!(matches!(err, ExecutionError::Cancelled));
    }

    #[tokio::test]
    async fn datafusion_engine_enforces_materialized_row_limit() {
        let (_tmp, location) = write_grouped_parquet();
        let engine = DataFusionLocalEngine;
        let err = engine
            .execute_sql(
                "SELECT k, SUM(x) as total FROM t GROUP BY k ORDER BY k",
                QueryExecutionContext {
                    parquet_tables: vec![("t".to_string(), location)],
                    controls: ExecutionControls {
                        max_rows: Some(1),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect_err("two grouped rows should exceed max_rows=1");

        assert!(matches!(
            err,
            ExecutionError::RowLimitExceeded {
                limit: 1,
                actual: 2
            }
        ));
    }

    #[tokio::test]
    async fn datafusion_engine_truncates_when_row_limit_mode_is_truncate() {
        let (_tmp, location) = write_grouped_parquet();
        let engine = DataFusionLocalEngine;
        let result = engine
            .execute_sql(
                "SELECT k, SUM(x) as total FROM t GROUP BY k ORDER BY k",
                QueryExecutionContext {
                    parquet_tables: vec![("t".to_string(), location)],
                    controls: ExecutionControls {
                        max_rows: Some(1),
                        row_limit_mode: RowLimitMode::Truncate,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect("truncate mode should return a capped result");

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], ProximaValue::String("a".to_string()));
    }

    #[tokio::test]
    async fn datafusion_engine_streams_sql_over_parquet() {
        use futures::TryStreamExt;
        let (_tmp, location) = write_grouped_parquet();
        let engine = DataFusionLocalEngine;
        let stream_result = engine
            .execute_sql_stream(
                "SELECT k, SUM(x) as total FROM t GROUP BY k ORDER BY k",
                QueryExecutionContext {
                    parquet_tables: vec![("t".to_string(), location)],
                    ..Default::default()
                },
            )
            .await
            .expect("stream should execute");

        // Schema is available before draining any row.
        assert_eq!(stream_result.schema.columns[0].name, "k");
        let rows = stream_result
            .rows
            .try_collect::<Vec<_>>()
            .await
            .expect("stream rows should be ok");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], ProximaValue::String("a".to_string()));
        assert_eq!(rows[0][1], ProximaValue::Float64(4.0));
        assert_eq!(rows[1][0], ProximaValue::String("b".to_string()));
    }

    #[tokio::test]
    async fn datafusion_engine_stream_truncates_without_error() {
        use futures::TryStreamExt;
        let (_tmp, location) = write_grouped_parquet();
        let engine = DataFusionLocalEngine;
        let stream_result = engine
            .execute_sql_stream(
                "SELECT k, SUM(x) as total FROM t GROUP BY k ORDER BY k",
                QueryExecutionContext {
                    parquet_tables: vec![("t".to_string(), location)],
                    controls: ExecutionControls {
                        max_rows: Some(1),
                        row_limit_mode: RowLimitMode::Truncate,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect("stream should execute");

        let rows = stream_result
            .rows
            .try_collect::<Vec<_>>()
            .await
            .expect("truncate mode streams the capped rows without error");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], ProximaValue::String("a".to_string()));
    }

    #[tokio::test]
    async fn datafusion_engine_stream_errors_on_overflow() {
        use futures::TryStreamExt;
        let (_tmp, location) = write_grouped_parquet();
        let engine = DataFusionLocalEngine;
        let stream_result = engine
            .execute_sql_stream(
                "SELECT k, SUM(x) as total FROM t GROUP BY k ORDER BY k",
                QueryExecutionContext {
                    parquet_tables: vec![("t".to_string(), location)],
                    controls: ExecutionControls {
                        max_rows: Some(1),
                        row_limit_mode: RowLimitMode::Error,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .expect("stream opens; the overflow surfaces while draining");

        let err = stream_result
            .rows
            .try_collect::<Vec<_>>()
            .await
            .expect_err("draining should hit the row-limit overflow");
        assert!(matches!(
            err,
            ExecutionError::RowLimitExceeded {
                limit: 1,
                actual: 2
            }
        ));
    }
}

// Helpers copied from relational_pipeline.rs or shared utilities

/// ADR-025 relational cold-path read-merge. Registers `name` as an in-memory
/// table whose rows are the materialized Parquet base reconciled with the
/// authoritative post-snapshot WAL delta: base rows whose canonical oid (the PK
/// value, recomputed from the base PK column) changed since `snapshot_lsn` are
/// dropped, and the current live row for every changed oid is appended. The
/// suppress-set is a rebuildable projection of the canonical WAL (ADR-020), so no
/// new durable state is introduced. MemTable-backed (whole base in memory) —
/// acceptable for the opt-in foundation; a streaming provider is future work.
#[cfg(feature = "datafusion-integration")]
async fn register_merged_olap_table(
    ctx: &datafusion::prelude::SessionContext,
    name: &str,
    location: &str,
    table: &crate::query::execution::olap_delta_merge::OlapDeltaTable,
    source: &dyn crate::query::execution::olap_delta_merge::OlapDeltaSource,
    tenant: Option<&str>,
) -> Result<(), ExecutionError> {
    use crate::services::record_store::proxima_value_to_unique_text;
    use datafusion::arrow::array::BooleanArray;
    use datafusion::arrow::compute::filter_record_batch;
    use datafusion::arrow::record_batch::RecordBatch;
    use proximadb_storage_common::proxima_arrow::{
        arrow_cell_to_proxima_value, proxima_records_to_record_batch,
    };
    use std::collections::HashSet;
    use std::sync::Arc;

    // 1. Read the cold Parquet base in an isolated session so the hidden base
    //    registration never leaks into the query's table namespace.
    let base_ctx = crate::datafusion::create_session_context()
        .map_err(|e| ExecutionError::Context(format!("olap-merge base session: {e}")))?;
    crate::datafusion::register_object_store_parquet_location(&base_ctx, name, location)
        .await
        .map_err(|e| ExecutionError::Context(format!("olap-merge base register {name}: {e}")))?;
    let base_schema = base_ctx
        .table_provider(name)
        .await
        .map_err(|e| ExecutionError::Context(format!("olap-merge base provider {name}: {e}")))?
        .schema();
    let base_batches = base_ctx
        .table(name)
        .await
        .map_err(|e| ExecutionError::Context(format!("olap-merge base table {name}: {e}")))?
        .collect()
        .await
        .map_err(|e| ExecutionError::Context(format!("olap-merge base collect {name}: {e}")))?;

    // 2. Suppress-set = oids changed since the snapshot (canonical WAL change-feed).
    let changed = source
        .changed_oids_since(name, table.snapshot_lsn, tenant)
        .await
        .map_err(|e| ExecutionError::Context(format!("olap-merge delta {name}: {e}")))?;
    let suppress: HashSet<String> = changed.iter().cloned().collect();

    // 3. Keep base rows whose recomputed oid is not suppressed. (TTL on the base
    //    is eventual-until-rematerialize — the snapshot carries no valid_to_ns,
    //    a documented first-cut limitation; explicit deletes are always caught by
    //    the suppress-set.)
    let pk_idx = base_schema
        .index_of(table.pk_column.as_str())
        .map_err(|e| {
            ExecutionError::Context(format!(
                "olap-merge pk column `{}` not in base {name}: {e}",
                table.pk_column
            ))
        })?;
    let mut batches: Vec<RecordBatch> = Vec::with_capacity(base_batches.len() + 1);
    for batch in &base_batches {
        let pk_array = batch.column(pk_idx);
        let keep: BooleanArray = (0..batch.num_rows())
            .map(|row| {
                let oid = arrow_cell_to_proxima_value(pk_array, row)
                    .map(|v| proxima_value_to_unique_text(&v))
                    .unwrap_or_default();
                Some(!suppress.contains(&oid))
            })
            .collect();
        let kept = filter_record_batch(batch, &keep)
            .map_err(|e| ExecutionError::Context(format!("olap-merge filter {name}: {e}")))?;
        batches.push(kept);
    }

    // 4. Append the current live rows for the changed oids, encoded with the same
    //    catalog schema the base was written with and normalized onto the base
    //    schema so every MemTable partition batch shares one schema.
    let (schema, append_records) = source
        .current_records(name, &changed, tenant)
        .await
        .map_err(|e| ExecutionError::Context(format!("olap-merge appends {name}: {e}")))?;
    if !append_records.is_empty() {
        let append_batch = proxima_records_to_record_batch(&append_records, &schema)
            .map_err(|e| ExecutionError::Context(format!("olap-merge append batch {name}: {e}")))?;
        let normalized = RecordBatch::try_new(base_schema.clone(), append_batch.columns().to_vec())
            .map_err(|e| {
                ExecutionError::Context(format!("olap-merge append schema {name}: {e}"))
            })?;
        batches.push(normalized);
    }

    // 5. Register the reconciled rows as the table the query reads.
    let mem = datafusion::datasource::MemTable::try_new(base_schema, vec![batches])
        .map_err(|e| ExecutionError::Context(format!("olap-merge memtable {name}: {e}")))?;
    ctx.register_table(name, Arc::new(mem))
        .map_err(|e| ExecutionError::Context(format!("olap-merge register {name}: {e}")))?;
    Ok(())
}

#[cfg(feature = "datafusion-integration")]
fn arrow_type_to_proxima(dt: &arrow_schema::DataType) -> proximadb_data_model::ProximaType {
    use arrow_schema::DataType as D;
    use proximadb_data_model::ProximaType;
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
        D::Date32 => ProximaType::Date,
        D::Date64 => ProximaType::Timestamp(TimeUnit::Millisecond),
        D::Timestamp(_, _) => ProximaType::Timestamp(TimeUnit::Microsecond),
        _ => ProximaType::String,
    }
}

#[cfg(feature = "datafusion-integration")]
fn arrow_schema_to_relational(arrow_schema: &arrow_schema::Schema) -> RelationalSchema {
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
    RelationalSchema::new(columns)
}

#[cfg(feature = "datafusion-integration")]
fn record_batch_to_rows(batch: &arrow_array::RecordBatch) -> Vec<RelationalRow> {
    let ncols = batch.num_columns();
    let mut rows: Vec<RelationalRow> = Vec::with_capacity(batch.num_rows());
    for r in 0..batch.num_rows() {
        let mut row: RelationalRow = Vec::with_capacity(ncols);
        for c in 0..ncols {
            row.push(arrow_cell_to_proxima(batch.column(c).as_ref(), r));
        }
        rows.push(row);
    }
    rows
}

#[cfg(feature = "datafusion-integration")]
fn record_batches_to_pipeline_result(
    arrow_schema: &arrow_schema::Schema,
    batches: &[arrow_array::RecordBatch],
) -> ExecutionPipelineResult {
    let schema = arrow_schema_to_relational(arrow_schema);
    let mut rows: Vec<RelationalRow> = Vec::new();
    for batch in batches {
        rows.extend(record_batch_to_rows(batch));
    }
    ExecutionPipelineResult { schema, rows }
}

#[cfg(feature = "datafusion-integration")]
fn arrow_cell_to_proxima(
    array: &dyn arrow_array::Array,
    row: usize,
) -> proximadb_data_model::ProximaValue {
    use arrow_array::*;
    use arrow_schema::DataType as D;
    use proximadb_data_model::ProximaValue;
    if array.is_null(row) {
        return ProximaValue::Null;
    }
    macro_rules! v {
        ($t:ty, $ctor:path) => {
            array
                .as_any()
                .downcast_ref::<$t>()
                .map(|a| $ctor(a.value(row)))
        };
    }
    match array.data_type() {
        D::Boolean => v!(BooleanArray, ProximaValue::Boolean),
        D::Int8 => v!(Int8Array, ProximaValue::Int8),
        D::Int16 => v!(Int16Array, ProximaValue::Int16),
        D::Int32 => v!(Int32Array, ProximaValue::Int32),
        D::Int64 => v!(Int64Array, ProximaValue::Int64),
        D::UInt8 => v!(UInt8Array, ProximaValue::UInt8),
        D::UInt16 => v!(UInt16Array, ProximaValue::UInt16),
        D::UInt32 => v!(UInt32Array, ProximaValue::UInt32),
        D::UInt64 => v!(UInt64Array, ProximaValue::UInt64),
        D::Float32 => v!(Float32Array, ProximaValue::Float32),
        D::Float64 => v!(Float64Array, ProximaValue::Float64),
        D::Utf8 => array
            .as_any()
            .downcast_ref::<StringArray>()
            .map(|a| ProximaValue::String(a.value(row).to_string())),
        D::LargeUtf8 => array
            .as_any()
            .downcast_ref::<LargeStringArray>()
            .map(|a| ProximaValue::String(a.value(row).to_string())),
        D::Binary => array
            .as_any()
            .downcast_ref::<BinaryArray>()
            .map(|a| ProximaValue::Binary(a.value(row).to_vec())),
        D::LargeBinary => array
            .as_any()
            .downcast_ref::<LargeBinaryArray>()
            .map(|a| ProximaValue::Binary(a.value(row).to_vec())),
        D::Date32 => v!(Date32Array, ProximaValue::Date),
        D::Date64 => array
            .as_any()
            .downcast_ref::<Date64Array>()
            .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Millisecond)),
        D::Timestamp(unit, _) => match unit {
            arrow_schema::TimeUnit::Second => array
                .as_any()
                .downcast_ref::<TimestampSecondArray>()
                .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Second)),
            arrow_schema::TimeUnit::Millisecond => array
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Millisecond)),
            arrow_schema::TimeUnit::Microsecond => array
                .as_any()
                .downcast_ref::<TimestampMicrosecondArray>()
                .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Microsecond)),
            arrow_schema::TimeUnit::Nanosecond => array
                .as_any()
                .downcast_ref::<TimestampNanosecondArray>()
                .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Nanosecond)),
        },
        _ => None,
    }
    .unwrap_or_else(|| {
        match arrow::util::display::ArrayFormatter::try_new(
            array,
            &arrow::util::display::FormatOptions::default(),
        ) {
            Ok(f) => ProximaValue::String(f.value(row).to_string()),
            Err(_) => ProximaValue::Null,
        }
    })
}
