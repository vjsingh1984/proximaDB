//! Parquet-over-object_store TableProvider for the warehouse base tier.
//!
//! This is the DataFusion read leaf for Parquet/Iceberg publications. It uses
//! `IcebergObjectStoreBridge` for base-prefix discovery and Parquet's
//! `ParquetObjectReader` for range-based footer and row-group reads.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::common::ScalarValue as DfScalarValue;
use datafusion::datasource::{TableProvider, TableType};
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::execution::SendableRecordBatchStream;
use datafusion::logical_expr::{Expr, Operator};
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::prelude::SessionContext;
use futures::StreamExt;
use object_store::ObjectStore;
use object_store::path::Path;
use parquet::arrow::async_reader::{ParquetObjectReader, ParquetRecordBatchStreamBuilder};
use parquet::file::metadata::ParquetMetaData;
use parquet::file::statistics::Statistics as ParquetStatistics;
use proximadb_iceberg_engine::IcebergObjectStoreBridge;
use proximadb_storage_common::format_splits::{
    ColumnBounds, FileSplit, ScalarPredicate, ScalarValue, SplitStatistics, SplitType,
};
use proximadb_storage_common::object_store_bridge::ObjectStoreBridge;
use url::Url;

use super::super::proxima_scan_exec::{ProximaScanExec, SplitReader};
use super::super::proxima_table_provider::EngineType;

fn project_schema(full: &SchemaRef, projection: Option<&[usize]>) -> SchemaRef {
    match projection {
        Some(proj) => {
            let fields: Vec<_> = proj
                .iter()
                .filter_map(|&i| full.fields().get(i))
                .map(|f| f.as_ref().clone())
                .collect();
            Arc::new(Schema::new(fields))
        }
        None => full.clone(),
    }
}

fn parquet_reader(
    store: Arc<dyn ObjectStore>,
    path: Path,
    file_size: Option<u64>,
) -> ParquetObjectReader {
    let reader = ParquetObjectReader::new(store, path);
    match file_size {
        Some(size) => reader.with_file_size(size),
        None => reader,
    }
}

fn df_err(context: &str, err: impl std::fmt::Display) -> DataFusionError {
    DataFusionError::Execution(format!("{context}: {err}"))
}

/// Reads a selected Parquet row group through object_store range requests.
#[derive(Debug)]
pub struct ObjectStoreParquetSplitReader {
    schema: SchemaRef,
    store: Arc<dyn ObjectStore>,
    file_sizes: HashMap<String, u64>,
}

#[async_trait]
impl SplitReader for ObjectStoreParquetSplitReader {
    async fn read_split(
        &self,
        split: &FileSplit,
        projection: Option<&[usize]>,
        batch_size: usize,
    ) -> DFResult<SendableRecordBatchStream> {
        let row_group_index = match &split.split_type {
            SplitType::RowGroup {
                row_group_index, ..
            } => *row_group_index,
            other => {
                return Err(df_err(
                    "ObjectStoreParquetSplitReader expects RowGroup splits",
                    format!("{other:?}"),
                ));
            }
        };
        let path = Path::from(split.file_path.as_str());
        let file_size = self.file_sizes.get(&split.file_path).copied();
        let reader = parquet_reader(self.store.clone(), path, file_size);
        let mut stream = ParquetRecordBatchStreamBuilder::new(reader)
            .await
            .map_err(|e| df_err("parquet open", e))?
            .with_row_groups(vec![row_group_index])
            .with_batch_size(batch_size)
            .build()
            .map_err(|e| df_err("parquet reader build", e))?;

        let proj_owned = projection.map(|p| p.to_vec());
        let out_schema = project_schema(&self.schema, projection);
        let mut batches = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch = batch.map_err(|e| df_err("parquet decode", e))?;
            let batch = match &proj_owned {
                Some(proj) => batch.project(proj).map_err(|e| df_err("project", e))?,
                None => batch,
            };
            batches.push(Ok(batch));
        }
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            out_schema,
            futures::stream::iter(batches),
        )))
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn engine_type(&self) -> EngineType {
        EngineType::Viper
    }

    fn supports_projection_pushdown(&self) -> bool {
        true
    }
}

/// DataFusion table backed by one or more Parquet objects in object_store.
#[derive(Debug)]
pub struct ObjectStoreParquetTable {
    schema: SchemaRef,
    store: Arc<dyn ObjectStore>,
    splits: Vec<FileSplit>,
    file_sizes: HashMap<String, u64>,
    location: String,
}

impl ObjectStoreParquetTable {
    /// Open either a ProximaDB base-prefix location (`data/*.parquet`) or a
    /// direct external `.parquet` object.
    pub async fn open(location: &str) -> DFResult<Self> {
        if location.ends_with(".parquet") {
            return Self::open_direct_object(location).await;
        }
        Self::open_bridge_base(location).await
    }

    async fn open_bridge_base(location: &str) -> DFResult<Self> {
        let bridge = Arc::new(
            IcebergObjectStoreBridge::from_url(location)
                .map_err(|e| df_err(&format!("object-store bridge({location})"), e))?,
        );
        let mut parquet_paths = bridge
            .list_objects(&Path::from("data"))
            .await
            .map_err(|e| df_err(&format!("list parquet base {location}"), e))?
            .into_iter()
            .filter(|path| path.as_ref().ends_with(".parquet"))
            .collect::<Vec<_>>();
        parquet_paths.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        if parquet_paths.is_empty() {
            return Err(df_err(
                "open object-store parquet base",
                format!("no data/*.parquet objects under {location}"),
            ));
        }

        let store = bridge.inner_store();
        let mut files = Vec::with_capacity(parquet_paths.len());
        for relative in parquet_paths {
            let full = bridge.full_object_path(&relative);
            let size = store
                .head(&full)
                .await
                .map_err(|e| df_err(&format!("head {full}"), e))?
                .size;
            files.push((full, Some(size)));
        }
        Self::open_files(store, files, location.to_string()).await
    }

    async fn open_direct_object(location: &str) -> DFResult<Self> {
        let parsed = Url::parse(location).map_err(|e| df_err("parse object-store url", e))?;
        let (store, path) =
            object_store::parse_url(&parsed).map_err(|e| df_err("object_store parse_url", e))?;
        let store: Arc<dyn ObjectStore> = Arc::from(store);
        let size = store
            .head(&path)
            .await
            .map_err(|e| df_err(&format!("head {path}"), e))?
            .size;
        Self::open_files(store, vec![(path, Some(size))], location.to_string()).await
    }

    async fn open_files(
        store: Arc<dyn ObjectStore>,
        files: Vec<(Path, Option<u64>)>,
        location: String,
    ) -> DFResult<Self> {
        let mut schema = None;
        let mut splits = Vec::new();
        let mut file_sizes = HashMap::new();
        for (path, size) in files {
            let reader = parquet_reader(store.clone(), path.clone(), size);
            let builder = ParquetRecordBatchStreamBuilder::new(reader)
                .await
                .map_err(|e| df_err(&format!("parquet metadata {path}"), e))?;
            let file_schema = builder.schema().clone();
            if schema.is_none() {
                schema = Some(file_schema);
            }
            if let Some(size) = size {
                file_sizes.insert(path.as_ref().to_string(), size);
            }
            splits.extend(row_group_splits(path, builder.metadata()));
        }

        Ok(Self {
            schema: schema
                .ok_or_else(|| df_err("open object-store parquet", "no parquet files"))?,
            store,
            splits,
            file_sizes,
            location,
        })
    }

    /// Number of row-group splits before runtime filter pruning.
    pub fn split_count(&self) -> usize {
        self.splits.len()
    }

    /// Count row-group splits that survive the given filters.
    pub fn pruned_split_count(&self, filters: &[Expr]) -> usize {
        self.prune_splits(filters).len()
    }

    fn reader(&self) -> Arc<ObjectStoreParquetSplitReader> {
        Arc::new(ObjectStoreParquetSplitReader {
            schema: self.schema.clone(),
            store: self.store.clone(),
            file_sizes: self.file_sizes.clone(),
        })
    }

    fn prune_splits(&self, filters: &[Expr]) -> Vec<FileSplit> {
        self.splits
            .iter()
            .filter(|split| {
                !filters
                    .iter()
                    .any(|filter| filter_prunes_split(split, filter))
            })
            .cloned()
            .collect()
    }
}

#[async_trait]
impl TableProvider for ObjectStoreParquetTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        let target_partitions = state.config().target_partitions();
        let exec = ProximaScanExec::builder()
            .schema(self.schema.clone())
            .splits(self.prune_splits(filters))
            .reader(self.reader())
            .projection(projection.cloned())
            .filters(filters.to_vec())
            .limit(limit)
            .collection_name(self.location.clone())
            .batch_size(8192)
            .target_partitions(target_partitions)
            .build()?;
        Ok(Arc::new(exec))
    }
}

fn row_group_splits(path: Path, metadata: &ParquetMetaData) -> Vec<FileSplit> {
    let file_path = path.as_ref().to_string();
    let mut splits = Vec::with_capacity(metadata.num_row_groups());
    for i in 0..metadata.num_row_groups() {
        let rg = metadata.row_group(i);
        let mut split = FileSplit::new_row_group(
            file_path.clone(),
            i,
            0,
            rg.total_byte_size().max(0) as u64,
            rg.num_rows(),
        );
        split.statistics = row_group_statistics(metadata, i);
        splits.push(split);
    }
    splits
}

fn row_group_statistics(metadata: &ParquetMetaData, row_group_index: usize) -> SplitStatistics {
    let rg = metadata.row_group(row_group_index);
    let mut stats = SplitStatistics {
        row_count: Some(rg.num_rows().max(0) as u64),
        byte_size: Some(rg.total_byte_size().max(0) as u64),
        ..Default::default()
    };
    for column in rg.columns() {
        let Some(bounds) = column.statistics().and_then(column_bounds_from_parquet) else {
            continue;
        };
        stats
            .column_stats
            .insert(column.column_path().string(), bounds);
    }
    stats
}

fn column_bounds_from_parquet(stats: &ParquetStatistics) -> Option<ColumnBounds> {
    let (min, max) = match stats {
        ParquetStatistics::Boolean(s) => (
            s.min_opt().map(|v| serde_json::json!(*v)),
            s.max_opt().map(|v| serde_json::json!(*v)),
        ),
        ParquetStatistics::Int32(s) => (
            s.min_opt().map(|v| serde_json::json!(*v as i64)),
            s.max_opt().map(|v| serde_json::json!(*v as i64)),
        ),
        ParquetStatistics::Int64(s) => (
            s.min_opt().map(|v| serde_json::json!(*v)),
            s.max_opt().map(|v| serde_json::json!(*v)),
        ),
        ParquetStatistics::Float(s) => (
            s.min_opt().map(|v| serde_json::json!(*v as f64)),
            s.max_opt().map(|v| serde_json::json!(*v as f64)),
        ),
        ParquetStatistics::Double(s) => (
            s.min_opt().map(|v| serde_json::json!(*v)),
            s.max_opt().map(|v| serde_json::json!(*v)),
        ),
        ParquetStatistics::ByteArray(s) => (
            s.min_opt()
                .and_then(|v| v.as_utf8().ok())
                .map(|v| serde_json::json!(v)),
            s.max_opt()
                .and_then(|v| v.as_utf8().ok())
                .map(|v| serde_json::json!(v)),
        ),
        _ => return None,
    };
    Some(ColumnBounds {
        min,
        max,
        null_count: stats.null_count_opt().unwrap_or(0),
        distinct_count: stats.distinct_count_opt(),
    })
}

fn filter_prunes_split(split: &FileSplit, expr: &Expr) -> bool {
    match expr {
        Expr::BinaryExpr(binary) => match binary.op {
            Operator::And => {
                filter_prunes_split(split, &binary.left)
                    || filter_prunes_split(split, &binary.right)
            }
            Operator::Or => {
                filter_prunes_split(split, &binary.left)
                    && filter_prunes_split(split, &binary.right)
            }
            _ => comparison_predicate(&binary.left, binary.op, &binary.right)
                .map(|(column, predicate)| split.can_prune_scalar(&column, &predicate))
                .unwrap_or(false),
        },
        Expr::IsNull(inner) => column_name(inner)
            .map(|column| split.can_prune_scalar(&column, &ScalarPredicate::IsNull))
            .unwrap_or(false),
        Expr::IsNotNull(inner) => column_name(inner)
            .map(|column| split.can_prune_scalar(&column, &ScalarPredicate::IsNotNull))
            .unwrap_or(false),
        Expr::InList(list) if !list.negated => {
            let Some(column) = column_name(&list.expr) else {
                return false;
            };
            let values = list
                .list
                .iter()
                .filter_map(literal_value)
                .collect::<Vec<_>>();
            !values.is_empty() && split.can_prune_scalar(&column, &ScalarPredicate::In(values))
        }
        Expr::Between(between) if !between.negated => {
            let Some(column) = column_name(&between.expr) else {
                return false;
            };
            match (literal_value(&between.low), literal_value(&between.high)) {
                (Some(low), Some(high)) => {
                    split.can_prune_scalar(&column, &ScalarPredicate::Between(low, high))
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn comparison_predicate(
    left: &Expr,
    op: Operator,
    right: &Expr,
) -> Option<(String, ScalarPredicate)> {
    if let (Some(column), Some(value)) = (column_name(left), literal_value(right)) {
        return scalar_predicate(op, value).map(|predicate| (column, predicate));
    }
    if let (Some(value), Some(column)) = (literal_value(left), column_name(right)) {
        return scalar_predicate(reverse_operator(op), value).map(|predicate| (column, predicate));
    }
    None
}

fn scalar_predicate(op: Operator, value: ScalarValue) -> Option<ScalarPredicate> {
    match op {
        Operator::Eq => Some(ScalarPredicate::Equal(value)),
        Operator::NotEq => Some(ScalarPredicate::NotEqual(value)),
        Operator::Lt => Some(ScalarPredicate::LessThan(value)),
        Operator::LtEq => Some(ScalarPredicate::LessThanOrEqual(value)),
        Operator::Gt => Some(ScalarPredicate::GreaterThan(value)),
        Operator::GtEq => Some(ScalarPredicate::GreaterThanOrEqual(value)),
        _ => None,
    }
}

fn reverse_operator(op: Operator) -> Operator {
    match op {
        Operator::Lt => Operator::Gt,
        Operator::LtEq => Operator::GtEq,
        Operator::Gt => Operator::Lt,
        Operator::GtEq => Operator::LtEq,
        other => other,
    }
}

fn column_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Column(column) => Some(column.name.clone()),
        Expr::Cast(cast) => column_name(&cast.expr),
        Expr::TryCast(cast) => column_name(&cast.expr),
        _ => None,
    }
}

fn literal_value(expr: &Expr) -> Option<ScalarValue> {
    match expr {
        Expr::Literal(value, _) => df_scalar_value(value),
        Expr::Cast(cast) => literal_value(&cast.expr),
        Expr::TryCast(cast) => literal_value(&cast.expr),
        _ => None,
    }
}

fn df_scalar_value(value: &DfScalarValue) -> Option<ScalarValue> {
    match value {
        DfScalarValue::Null => Some(ScalarValue::Null),
        DfScalarValue::Boolean(Some(v)) => Some(ScalarValue::Bool(*v)),
        DfScalarValue::Int8(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::Int16(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::Int32(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::Int64(Some(v)) => Some(ScalarValue::Int64(*v)),
        DfScalarValue::UInt8(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::UInt16(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::UInt32(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::UInt64(Some(v)) => i64::try_from(*v).ok().map(ScalarValue::Int64),
        DfScalarValue::Float32(Some(v)) => Some(ScalarValue::Float64(*v as f64)),
        DfScalarValue::Float64(Some(v)) => Some(ScalarValue::Float64(*v)),
        DfScalarValue::Utf8(Some(v))
        | DfScalarValue::Utf8View(Some(v))
        | DfScalarValue::LargeUtf8(Some(v)) => Some(ScalarValue::String(v.clone())),
        _ => None,
    }
}

/// Register an object_store-backed Parquet location as a DataFusion table.
pub async fn register_object_store_parquet_location(
    ctx: &SessionContext,
    name: &str,
    location: &str,
) -> DFResult<Arc<ObjectStoreParquetTable>> {
    let table = Arc::new(ObjectStoreParquetTable::open(location).await?);
    ctx.register_table(name, table.clone())?;
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::common::{Column, ScalarValue as DfScalarValue};
    use datafusion::logical_expr::BinaryExpr;
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;

    fn write_two_row_group_parquet(path: &std::path::Path) -> SchemaRef {
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "a", "b", "b"])),
                Arc::new(Int64Array::from(vec![1_i64, 2, 100, 101])),
            ],
        )
        .unwrap();
        let props = WriterProperties::builder()
            .set_max_row_group_size(2)
            .build();
        let file = std::fs::File::create(path).unwrap();
        let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        schema
    }

    fn gt_filter(column: &str, value: i64) -> Expr {
        Expr::BinaryExpr(BinaryExpr::new(
            Box::new(Expr::Column(Column::from_name(column))),
            Operator::Gt,
            Box::new(Expr::Literal(DfScalarValue::Int64(Some(value)), None)),
        ))
    }

    #[tokio::test]
    async fn opens_bridge_base_prefix_and_discovers_data_parquet() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        write_two_row_group_parquet(&data_dir.join("part-0.parquet"));

        let location = format!("file://{}", tmp.path().display());
        let table = ObjectStoreParquetTable::open(&location).await.unwrap();
        assert_eq!(table.split_count(), 2);
        assert_eq!(table.schema().fields().len(), 2);
    }

    #[tokio::test]
    async fn prunes_row_groups_from_parquet_min_max_statistics() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        write_two_row_group_parquet(&data_dir.join("part-0.parquet"));

        let location = format!("file://{}", tmp.path().display());
        let table = ObjectStoreParquetTable::open(&location).await.unwrap();
        let filters = vec![gt_filter("x", 50)];

        assert_eq!(table.split_count(), 2);
        assert_eq!(table.pruned_split_count(&filters), 1);
    }
}
