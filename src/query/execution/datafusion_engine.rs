#[cfg(feature = "datafusion-integration")]
use crate::query::execution::engine::normalize_table_key;
use crate::query::execution::engine::{
    ExecutionEngine, ExecutionError, ExecutionPipelineResult, QueryExecutionContext,
};
use async_trait::async_trait;
#[cfg(feature = "datafusion-integration")]
use proximadb_data_model::TimeUnit;
#[cfg(feature = "datafusion-integration")]
use proximadb_relational_frontend::lower_sql;
#[cfg(feature = "datafusion-integration")]
use proximadb_relational_types::{ColumnInfo, RelationalSchema};
#[cfg(feature = "datafusion-integration")]
use std::collections::HashMap;

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
}

#[cfg(feature = "datafusion-integration")]
impl DataFusionLocalEngine {
    async fn execute_datafusion(
        &self,
        sql: &str,
        context: QueryExecutionContext,
    ) -> Result<ExecutionPipelineResult, ExecutionError> {
        context.controls.check_cancelled()?;
        // F4: when the route owns the vector service, register the `vector_search` UDTF
        let ctx = match context.vector_ops {
            Some(ops) => crate::datafusion::create_session_context_with_vector_ops(ops),
            None => crate::datafusion::create_session_context(),
        }
        .map_err(|e| ExecutionError::Context(format!("session: {e}")))?;

        for (name, location) in &context.parquet_tables {
            crate::datafusion::register_object_store_parquet_location(&ctx, name, location)
                .await
                .map_err(|e| {
                    ExecutionError::Context(format!(
                        "register object-store parquet table {name}: {e}"
                    ))
                })?;
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
            Ok(node) => Some(
                crate::datafusion::logical_lowering::lower_logical_node(&ctx, &node)
                    .await
                    .map_err(|e| ExecutionError::Planning(format!("lower logical node: {e}")))?,
            ),
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

        let arrow_schema = df.schema().as_arrow().clone();
        let batches = df
            .collect()
            .await
            .map_err(|e| ExecutionError::Execution(format!("collect: {e}")))?;
        context.controls.check_cancelled()?;
        record_batches_to_pipeline_result(&arrow_schema, &batches)
            .enforce_row_limit(context.controls.max_rows)
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
    use crate::query::execution::engine::ExecutionControls;
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
}

// Helpers copied from relational_pipeline.rs or shared utilities

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
fn record_batches_to_pipeline_result(
    arrow_schema: &arrow_schema::Schema,
    batches: &[arrow_array::RecordBatch],
) -> ExecutionPipelineResult {
    use proximadb_relational_types::RelationalRow;

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
