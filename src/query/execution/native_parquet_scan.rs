// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

#![allow(dead_code)]

//! # Native parquet scan operator (ADR-054 / TD-OLAP-4 — engine dimension)
//!
//! The arrow-rs analog of [`super::native_scan::PaxScanOperator`]: reads
//! **external parquet** into Arrow `RecordBatch`es and emits them into the native
//! vectorized engine's `BatchStream`, with **zero DataFusion dependency** (drives
//! the `parquet` crate's `ParquetRecordBatchStreamBuilder` directly — the very
//! same reader `object_store_parquet_reader.rs` uses on the DataFusion side).
//!
//! This is the scan source that lets the native vectorized engine
//! (`FilterProject` → `HashAggregate` → `HashJoin`) serve the SAME external-parquet
//! queries as DataFusion — the prerequisite for the native-vs-DataFusion shadow
//! comparison. The scan source is deliberately separable from the execution
//! engine: reading parquet → `RecordBatch` is pure arrow-rs, and native operators
//! consume `RecordBatch`es, so DataFusion is not required to read parquet.
//!
//! Streams row groups lazily (bounded memory — one decoded batch in flight),
//! never materializing the whole file; row-group zone-map pruning is a follow-on.

use std::sync::Arc;

use arrow_schema::SchemaRef;
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt};
use object_store::ObjectStore;
use object_store::path::Path;
use parquet::arrow::ProjectionMask;
use parquet::arrow::async_reader::{ParquetObjectReader, ParquetRecordBatchStreamBuilder};
use proximadb_execution_contracts::{BatchStream, ExecutionError, ExecutionOperator};

/// A native scan source that reads external parquet files via arrow-rs and emits
/// their `RecordBatch`es. Holds only object-store + parquet types — no DataFusion.
#[derive(Debug)]
pub(crate) struct ParquetScanOperator {
    store: Arc<dyn ObjectStore>,
    /// The parquet files to read, with their (optional) byte sizes.
    files: Vec<(Path, Option<u64>)>,
    /// Leaf column indices to fetch (projection pushdown); `None` reads all
    /// columns. When set, `output_schema` MUST be the projected schema in file
    /// order, matching what the reader emits.
    projection: Option<Vec<usize>>,
    output_schema: SchemaRef,
    /// Footer-elision mode: emit a single zero-column batch whose row count is the
    /// sum of the parquet footers' `num_rows`, reading NO column data. This is the
    /// metadata-elision path for unfiltered `COUNT(*)` — the co-design-correct
    /// answer (COUNT(*) is a metadata op, not a scan), matching DataFusion's
    /// `AggregateStatistics` footer elision and avoiding a 100M-row column read.
    count_only: bool,
}

impl ParquetScanOperator {
    pub(crate) fn new(
        store: Arc<dyn ObjectStore>,
        files: Vec<(Path, Option<u64>)>,
        projection: Option<Vec<usize>>,
        output_schema: SchemaRef,
    ) -> Self {
        Self {
            store,
            files,
            projection,
            output_schema,
            count_only: false,
        }
    }

    /// Footer-elision constructor for unfiltered `COUNT(*)`: reads only the parquet
    /// footers (row counts) and emits one zero-column batch carrying the total row
    /// count — no column I/O. The downstream `COUNT(*)` counts that row count. For
    /// footerless formats (CSV) the caller must instead project the smallest-width
    /// column; parquet and PAX carry the count in the footer, so we elide.
    pub(crate) fn new_count_only(
        store: Arc<dyn ObjectStore>,
        files: Vec<(Path, Option<u64>)>,
    ) -> Self {
        Self {
            store,
            files,
            projection: Some(Vec::new()),
            output_schema: Arc::new(arrow_schema::Schema::empty()),
            count_only: true,
        }
    }
}

#[async_trait]
impl ExecutionOperator for ParquetScanOperator {
    fn output_schema(&self) -> SchemaRef {
        self.output_schema.clone()
    }

    async fn execute(&self, _input: BatchStream) -> Result<BatchStream, ExecutionError> {
        // Footer elision (unfiltered COUNT(*)): sum the parquet footers' row counts
        // and emit ONE zero-column batch carrying that count — zero column I/O.
        if self.count_only {
            let mut total: usize = 0;
            for (path, size) in &self.files {
                let mut reader = ParquetObjectReader::new(self.store.clone(), path.clone());
                if let Some(sz) = size {
                    reader = reader.with_file_size(*sz);
                }
                let builder = ParquetRecordBatchStreamBuilder::new(reader)
                    .await
                    .map_err(|e| {
                        ExecutionError::Execution(format!("parquet footer {path}: {e}"))
                    })?;
                total += builder.metadata().file_metadata().num_rows().max(0) as usize;
            }
            let opts = arrow_array::RecordBatchOptions::new().with_row_count(Some(total));
            let batch = arrow_array::RecordBatch::try_new_with_options(
                self.output_schema.clone(),
                vec![],
                &opts,
            )
            .map_err(|e| ExecutionError::Execution(format!("count batch: {e}")))?;
            return Ok(Box::pin(futures::stream::once(async move { Ok(batch) })));
        }
        // Lazily open each file and stream its row groups — bounded memory (one
        // decoded batch in flight, plus the downstream operator's state), NEVER
        // materializing the whole file. Eager materialization OOMs at scale
        // (100M rows → tens of GB); a blocking `HashAggregate` downstream only
        // needs one input batch at a time.
        let store = self.store.clone();
        let projection = self.projection.clone();
        let stream = futures::stream::iter(self.files.clone().into_iter().map(Ok))
            .and_then(move |(path, size)| {
                let store = store.clone();
                let projection = projection.clone();
                async move {
                    let mut reader = ParquetObjectReader::new(store, path.clone());
                    if let Some(sz) = size {
                        reader = reader.with_file_size(sz);
                    }
                    let mut builder =
                        ParquetRecordBatchStreamBuilder::new(reader)
                            .await
                            .map_err(|e| {
                                ExecutionError::Execution(format!("parquet open {path}: {e}"))
                            })?;
                    if let Some(proj) = &projection {
                        let mask =
                            ProjectionMask::roots(builder.parquet_schema(), proj.iter().copied());
                        builder = builder.with_projection(mask);
                    }
                    let inner = builder.build().map_err(|e| {
                        ExecutionError::Execution(format!("parquet build {path}: {e}"))
                    })?;
                    Ok(inner.map(move |b| {
                        b.map_err(|e| {
                            ExecutionError::Execution(format!("parquet decode {path}: {e}"))
                        })
                    }))
                }
            })
            .try_flatten();
        Ok(Box::pin(stream))
    }
}

/// A metadata-elision source (TD-OLAP-4): emits ONE pre-computed result batch built
/// from the parquet footer statistics (unfiltered `MIN`/`MAX`/`COUNT`), with zero
/// column I/O. The caller (shadow probe) computes the row from the table's
/// `aggregate_numeric_bounds` + footer row count; this operator just streams it, so
/// the elided aggregate runs through the native pipeline like any other operator.
#[derive(Debug)]
pub(crate) struct StatsAggregateOperator {
    batch: arrow_array::RecordBatch,
    output_schema: SchemaRef,
}

impl StatsAggregateOperator {
    pub(crate) fn new(batch: arrow_array::RecordBatch) -> Self {
        let output_schema = batch.schema();
        Self {
            batch,
            output_schema,
        }
    }
}

#[async_trait]
impl ExecutionOperator for StatsAggregateOperator {
    fn output_schema(&self) -> SchemaRef {
        self.output_schema.clone()
    }

    async fn execute(&self, _input: BatchStream) -> Result<BatchStream, ExecutionError> {
        let batch = self.batch.clone();
        Ok(Box::pin(futures::stream::once(async move { Ok(batch) })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use object_store::local::LocalFileSystem;
    use parquet::arrow::ArrowWriter;

    #[tokio::test]
    async fn parquet_scan_emits_all_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("t.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
                Arc::new(Int64Array::from(vec![10, 20, 30])),
            ],
        )
        .unwrap();
        let f = std::fs::File::create(&file_path).unwrap();
        let mut w = ArrowWriter::try_new(f, schema.clone(), None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();

        let store = Arc::new(LocalFileSystem::new_with_prefix(tmp.path()).unwrap());
        let size = std::fs::metadata(&file_path).unwrap().len();
        let op = ParquetScanOperator::new(
            store,
            vec![(Path::from("t.parquet"), Some(size))],
            None,
            schema.clone(),
        );

        let empty: BatchStream = Box::pin(futures::stream::empty());
        let mut out = op.execute(empty).await.unwrap();
        let mut rows = 0usize;
        let mut cols = 0usize;
        while let Some(b) = out.next().await {
            let b = b.unwrap();
            rows += b.num_rows();
            cols = b.num_columns();
        }
        assert_eq!(rows, 3, "all rows emitted");
        assert_eq!(cols, 2, "all columns emitted (no projection)");
        assert_eq!(op.output_schema().fields().len(), 2);
    }

    /// End-to-end: a relational `PhysicalPlan::Scan` lowers over an injected
    /// `ParquetScanOperator` (via `lower_physical_over_source`) and drains all
    /// rows through the native vectorized engine — proving native serves external
    /// parquet with ZERO DataFusion (TD-OLAP-4 engine dimension).
    #[tokio::test]
    async fn native_lowers_scan_over_parquet_source() {
        use crate::query::execution::native_ops::{execute_pipeline, lower_physical_over_source};
        use proximadb_data_model::ProximaType;
        use proximadb_relational_algebra::TableId;
        use proximadb_relational_planner::{PhysicalPlan, ScanAccess};
        use proximadb_relational_types::{ColumnInfo, RelationalSchema};

        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("t.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c", "d"])),
                Arc::new(Int64Array::from(vec![10, 20, 30, 40])),
            ],
        )
        .unwrap();
        let f = std::fs::File::create(&file_path).unwrap();
        let mut w = ArrowWriter::try_new(f, schema.clone(), None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();

        let store = Arc::new(LocalFileSystem::new_with_prefix(tmp.path()).unwrap());
        let size = std::fs::metadata(&file_path).unwrap().len();
        let source = Box::new(ParquetScanOperator::new(
            store,
            vec![(Path::from("t.parquet"), Some(size))],
            None,
            schema.clone(),
        ));

        // Relational plan: a bare Scan whose leaf is the injected parquet source.
        let plan = PhysicalPlan::Scan {
            table: TableId::new("hits"),
            output_schema: RelationalSchema::new(vec![
                ColumnInfo::new("k", ProximaType::String, false),
                ColumnInfo::new("x", ProximaType::Int64, false),
            ]),
            projection: None,
            predicate: None,
            limit: None,
            access: ScanAccess::FullScan,
        };

        let lowered = lower_physical_over_source(&plan, source).unwrap();
        let batches = execute_pipeline(&lowered).await.unwrap();
        let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(
            rows, 4,
            "native drained every parquet row via the Scan leaf"
        );
        assert_eq!(batches[0].num_columns(), 2);
    }

    #[tokio::test]
    async fn count_only_elides_to_footer_row_count_with_zero_columns() {
        // Footer elision: emit ONE zero-column batch whose row_count == footer total,
        // reading no column data (the COUNT(*) metadata path).
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("t.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c", "d", "e"])),
                Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5])),
            ],
        )
        .unwrap();
        let f = std::fs::File::create(&file_path).unwrap();
        let mut w = ArrowWriter::try_new(f, schema.clone(), None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();

        let store = Arc::new(LocalFileSystem::new_with_prefix(tmp.path()).unwrap());
        let size = std::fs::metadata(&file_path).unwrap().len();
        let op =
            ParquetScanOperator::new_count_only(store, vec![(Path::from("t.parquet"), Some(size))]);
        assert_eq!(op.output_schema().fields().len(), 0, "zero-column output");

        let empty: BatchStream = Box::pin(futures::stream::empty());
        let mut out = op.execute(empty).await.unwrap();
        let b = out.next().await.unwrap().unwrap();
        assert_eq!(b.num_columns(), 0, "no columns read");
        assert_eq!(b.num_rows(), 5, "row count from footer");
        assert!(out.next().await.is_none(), "single metadata batch");
    }

    #[tokio::test]
    async fn parquet_scan_projection_narrows_columns() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("t.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b"])),
                Arc::new(Int64Array::from(vec![1, 2])),
            ],
        )
        .unwrap();
        let f = std::fs::File::create(&file_path).unwrap();
        let mut w = ArrowWriter::try_new(f, schema.clone(), None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();

        let store = Arc::new(LocalFileSystem::new_with_prefix(tmp.path()).unwrap());
        let size = std::fs::metadata(&file_path).unwrap().len();
        // Project only column 1 ("x").
        let projected = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)]));
        let op = ParquetScanOperator::new(
            store,
            vec![(Path::from("t.parquet"), Some(size))],
            Some(vec![1]),
            projected,
        );

        let empty: BatchStream = Box::pin(futures::stream::empty());
        let mut out = op.execute(empty).await.unwrap();
        let b = out.next().await.unwrap().unwrap();
        assert_eq!(b.num_columns(), 1, "projection kept only 'x'");
        assert_eq!(b.num_rows(), 2);
    }
}
