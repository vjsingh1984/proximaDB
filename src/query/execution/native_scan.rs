// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # Native PAX scan operator (ADR-054 Phase 2.5, TD-OLAP-14)
//!
//! The real-storage scan source for the native vectorized engine. Mirrors the
//! DataFusion `PaxSplitReader`'s decode path (PaxSegmentScanner → per-block
//! decode → RecordBatch) but with **zero DataFusion dependency** — the native
//! engine works on Arrow `RecordBatch`es directly via the `ExecutionOperator`
//! contract.
//!
//! MVP (Phase 2.5 slice 1): `ScanPredicate::default()` (no block-level zone-map
//! pruning); correctness via the FilterProject operator above. Block-level
//! pruning (Expr → ScanPredicate) is slice 2 (the scan-avoidance lever).

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use proximadb_block_format::{ColumnRole, PaxBlockReader};
use proximadb_execution_contracts::{BatchStream, ExecutionError, ExecutionOperator};
use proximadb_storage_common::pax_block::{PaxSegmentScanner, ScanPredicate};

use crate::storage::formats::FileSplit;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// A scan operator that reads PAX-backed storage segments and emits Arrow
/// `RecordBatch`es into the native engine's `BatchStream`. Mirrors
/// `PaxSplitReader::decode_segment` but without DataFusion types.
#[derive(Debug)]
pub(crate) struct PaxScanOperator {
    splits: Vec<FileSplit>,
    filesystem_factory: Arc<FilesystemFactory>,
    name_to_col_id: HashMap<String, i32>,
    output_schema: SchemaRef,
}

impl PaxScanOperator {
    pub(crate) fn new(
        splits: Vec<FileSplit>,
        filesystem_factory: Arc<FilesystemFactory>,
        name_to_col_id: HashMap<String, i32>,
        output_schema: SchemaRef,
    ) -> Self {
        Self {
            splits,
            filesystem_factory,
            name_to_col_id,
            output_schema,
        }
    }
}

#[async_trait]
impl ExecutionOperator for PaxScanOperator {
    fn output_schema(&self) -> SchemaRef {
        self.output_schema.clone()
    }

    async fn execute(&self, _input: BatchStream) -> Result<BatchStream, ExecutionError> {
        let factory = self.filesystem_factory.clone();
        let splits = self.splits.clone();
        let name_to_col_id = self.name_to_col_id.clone();
        let schema = self.output_schema.clone();

        // Eagerly read + decode all segments (MVP; lazy per-segment streaming is a
        // follow-on for large multi-segment collections).
        let mut batches = Vec::new();
        for split in &splits {
            let bytes = factory.read(&split.file_path).await.map_err(|e| {
                ExecutionError::Execution(format!("PAX scan read {}: {e}", split.file_path))
            })?;
            let decoded = decode_pax_segment(&bytes, &name_to_col_id, &schema)?;
            batches.extend(decoded);
        }

        Ok(Box::pin(futures::stream::iter(
            batches.into_iter().map(Ok::<_, ExecutionError>),
        )))
    }
}

/// Decode one PAX segment file's bytes into `RecordBatch`es — one per block.
/// Mirrors `PaxSplitReader::decode_segment` but with `ExecutionError` (zero DF).
fn decode_pax_segment(
    bytes: &[u8],
    name_to_col_id: &HashMap<String, i32>,
    schema: &SchemaRef,
) -> Result<Vec<RecordBatch>, ExecutionError> {
    let predicate = ScanPredicate::default(); // MVP: no block-level pruning (slice 2)
    let mut scanner = PaxSegmentScanner::from_bytes(bytes.to_vec(), predicate)
        .map_err(|e| ExecutionError::Execution(format!("PAX segment open: {e}")))?;

    let mut batches = Vec::new();
    while let Some(reader) = scanner.next_block() {
        let arrays: Vec<ArrayRef> = schema
            .fields()
            .iter()
            .map(|f| decode_column(&reader, f.name(), name_to_col_id))
            .collect::<Result<_, _>>()?;
        let batch = RecordBatch::try_new(schema.clone(), arrays)
            .map_err(|e| ExecutionError::Execution(format!("PAX batch build: {e}")))?;
        batches.push(batch);
    }
    Ok(batches)
}

/// Decode one column by name → PAX column_id → typed stripe → Arrow array.
/// Mirrors `PaxSplitReader::decode_column`'s data_type_id dispatch.
fn decode_column(
    reader: &PaxBlockReader,
    name: &str,
    name_to_col_id: &HashMap<String, i32>,
) -> Result<ArrayRef, ExecutionError> {
    let cid = *name_to_col_id
        .get(name)
        .ok_or_else(|| ExecutionError::Schema(format!("PAX: no column_id for {name}")))?;
    let meta = reader
        .column_metas()
        .iter()
        .find(|m| m.column_id == cid)
        .ok_or_else(|| {
            ExecutionError::Schema(format!("PAX: column {name} (id {cid}) not in block"))
        })?;

    // TypeId bytes (proximadb_codec): I64=0x03, F64=0x02; 0xff = variable-length.
    Ok(match meta.data_type_id {
        0x03 => {
            let vals = reader.decode_i64_stripe(cid).ok_or_else(|| {
                ExecutionError::Execution(format!("PAX: i64 decode failed for {name} (id {cid})"))
            })?;
            Arc::new(Int64Array::from(vals))
        }
        0x02 => {
            let vals = reader.decode_f64_stripe(cid).ok_or_else(|| {
                ExecutionError::Execution(format!("PAX: f64 decode failed for {name} (id {cid})"))
            })?;
            Arc::new(Float64Array::from(vals))
        }
        0xff if meta.role == ColumnRole::Props => {
            let vals = reader.decode_bytes_stripe(cid).ok_or_else(|| {
                ExecutionError::Execution(format!("PAX: bytes decode failed for {name} (id {cid})"))
            })?;
            Arc::new(BinaryArray::from_iter(vals))
        }
        0xff => {
            let vals = reader.decode_str_stripe(cid).ok_or_else(|| {
                ExecutionError::Execution(format!("PAX: str decode failed for {name} (id {cid})"))
            })?;
            Arc::new(StringArray::from(vals))
        }
        other => {
            return Err(ExecutionError::NotImplemented(format!(
                "PAX data_type_id {other:#x} for {name} (MVP: i64/f64/str/bytes)"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::sst::segment_format::write_pax_segment;
    use arrow_array::Int64Array;
    use arrow_schema::{DataType, Field, Schema};
    use futures::StreamExt;
    use proximadb_block_format::{VectorQuant, col_id};
    use proximadb_records::ProximaRecord;

    fn record(oid: &str, tenant: &str, ts: i64) -> ProximaRecord {
        ProximaRecord {
            oid: oid.into(),
            tenant_id: tenant.into(),
            created_at_ns: ts,
            updated_at_ns: ts,
            ..Default::default()
        }
    }

    /// `[created_at]` schema + its name→column_id map (mirrors pax_adapter tests).
    fn created_at_schema() -> (SchemaRef, HashMap<String, i32>) {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "created_at",
            DataType::Int64,
            true,
        )]));
        let map = HashMap::from([(String::from("created_at"), col_id::CREATED_AT)]);
        (schema, map)
    }

    #[tokio::test]
    async fn pax_scan_reads_real_pax_data() {
        // Write a REAL .pax segment with 2 records (created_at = 1000, 3000).
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("seg.pax");
        let records = vec![record("r1", "t", 1000), record("r2", "t", 3000)];
        write_pax_segment(&path, &records, "col", 0, VectorQuant::Auto, None)
            .expect("write_pax_segment");

        // Construct the PaxScanOperator with a real FilesystemFactory.
        let fs = Arc::new(
            FilesystemFactory::create_default()
                .await
                .expect("FilesystemFactory"),
        );
        let (schema, map) = created_at_schema();
        let split = FileSplit::new_block(path.to_str().unwrap().to_string(), 0, 0, 0, 0);
        let scan = PaxScanOperator::new(vec![split], fs, map, schema.clone());

        // Execute the scan.
        let empty: BatchStream = Box::pin(futures::stream::empty());
        let stream = scan.execute(empty).await.expect("scan execute");
        let batches: Vec<RecordBatch> = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()
            .expect("drain stream");

        // Assert: 2 rows total with created_at values 1000 and 3000.
        assert!(!batches.is_empty(), "expected at least one batch");
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 2, "expected 2 rows from the PAX segment");
        let arr = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64Array for created_at");
        let vals: Vec<i64> = arr.iter().map(|o| o.unwrap()).collect();
        assert!(
            vals.contains(&1000) && vals.contains(&3000),
            "expected created_at values [1000, 3000], got {vals:?}"
        );
    }
}
