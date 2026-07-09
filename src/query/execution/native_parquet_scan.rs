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
//! MVP: eager read of the selected row-groups (mirrors `PaxScanOperator`'s eager
//! MVP); lazy per-split streaming + row-group zone-map pruning are follow-ons.

use std::sync::Arc;

use arrow_schema::SchemaRef;
use async_trait::async_trait;
use futures::StreamExt;
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
        }
    }
}

#[async_trait]
impl ExecutionOperator for ParquetScanOperator {
    fn output_schema(&self) -> SchemaRef {
        self.output_schema.clone()
    }

    async fn execute(&self, _input: BatchStream) -> Result<BatchStream, ExecutionError> {
        let mut batches = Vec::new();
        for (path, size) in &self.files {
            let mut reader = ParquetObjectReader::new(self.store.clone(), path.clone());
            if let Some(sz) = size {
                reader = reader.with_file_size(*sz);
            }
            let mut builder = ParquetRecordBatchStreamBuilder::new(reader)
                .await
                .map_err(|e| ExecutionError::Execution(format!("parquet open {path}: {e}")))?;
            if let Some(proj) = &self.projection {
                let mask = ProjectionMask::roots(builder.parquet_schema(), proj.iter().copied());
                builder = builder.with_projection(mask);
            }
            let mut stream = builder
                .build()
                .map_err(|e| ExecutionError::Execution(format!("parquet build {path}: {e}")))?;
            while let Some(b) = stream.next().await {
                batches.push(b.map_err(|e| {
                    ExecutionError::Execution(format!("parquet decode {path}: {e}"))
                })?);
            }
        }
        Ok(Box::pin(futures::stream::iter(
            batches.into_iter().map(Ok::<_, ExecutionError>),
        )))
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
