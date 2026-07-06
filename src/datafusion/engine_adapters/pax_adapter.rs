// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # PAX-native OLAP split reader (TD-OLAP-1 slice 1)
//!
//! Bridges ProximaDB's canonical PAX block reader ([`PaxBlockReader`], with its
//! hierarchical prune stack — zone maps, bloom, tenant/time/column-stat) to
//! DataFusion's vectorized execution by emitting Arrow `RecordBatch`es through
//! the [`SplitReader`] seam. This is the structural differentiator ADR-052
//! invariant 2 names: the **PAX-native scan** that co-designs ANN and OLAP on
//! one geometry (the same `PaxBlockReader` the vector path uses).
//!
//! ## Scope (slice 1) — and the architectural boundary it respects
//!
//! Per the block-format crate mandate and ADR-033, **pure relational OLAP stays
//! on Parquet/Iceberg** (the team's TD-OLAP-3/6 path). This reader is **not** a
//! replacement for that — it is the scan for the **hybrid vector+OLAP wedge**
//! (vectors force PAX; scan the relational stripes alongside) and for
//! LSM-resident PAX data not yet materialized to Parquet. Slice 1 delivers the
//! relational-stripe → Arrow bridge + the canonical prune stack; vector-stripe
//! decode (`decode_f32_vec_stripe`) and the hybrid-query integration are slice 2
//! — at which point the wedge value materializes.
//!
//! ## SOLID / co-design
//! * **OCP** — the 7th `SplitReader` impl behind the frozen seam; no existing
//!   path is modified (the ad-hoc `SstBlockStream` stub is left untouched;
//!   consolidating it onto `PaxBlockReader` is slice 3).
//! * **SRP** — the prune stack STAYS in `PaxBlockReader`; this reader owns only
//!   byte-load → open → conservative prune check → typed-stripe decode → Arrow.
//! * **Default-off** — gated by `PROXIMADB_DF_PAX_READER` (mirrors
//!   `PROXIMADB_DF_RUNTIME_FILTER_PRUNE`). NOT wired into any TableProvider /
//!   `route_select` (route flip is slice 2, gated on the TD-OLAP-4 ledger, per
//!   ADR-052 observe→ingest→act).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use arrow_array::{ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::execution::SendableRecordBatchStream;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use futures::{future, stream};
use tracing::trace;

use proximadb_block_format::{ColumnRole, PaxBlockReader};
use proximadb_storage_common::format_splits::{ScalarPredicate, ScalarValue};

use crate::datafusion::physical_filter_translate::pruning_predicates;
use crate::datafusion::proxima_scan_exec::{EmptyRecordBatchStream, SplitReader};
use crate::datafusion::proxima_table_provider::EngineType;
use crate::storage::formats::FileSplit;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// `true` when the PAX-native OLAP scan is opted in (TD-OLAP-1 slice 1).
/// Default OFF — the reader is constructed/tested but not routed until slice 2.
pub fn pax_reader_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("PROXIMADB_DF_PAX_READER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// PAX-native OLAP split reader. Loads a PAX segment, opens the canonical
/// [`PaxBlockReader`], applies zone-map/bloom pruning from the pushed filters,
/// then decodes the projected relational stripes into one Arrow `RecordBatch`.
#[derive(Debug)]
pub struct PaxSplitReader {
    schema: SchemaRef,
    filesystem_factory: Arc<FilesystemFactory>,
    /// Column NAME (DataFusion/SQL-facing) → PAX `column_id`. The prune +
    /// decode APIs of `PaxBlockReader` key on `column_id: i32`.
    name_to_col_id: HashMap<String, i32>,
    /// Translated once at construction from the resolved physical filters.
    /// Empty ⇒ no pruning (correctness-safe). AND-semantics at `block_pruned`.
    prune_predicates: Vec<(String, ScalarPredicate)>,
}

impl PaxSplitReader {
    /// Construct a reader for `schema`, resolving filter→column-id pruning at
    /// build time. `filters` are the resolved physical filters (logical `Expr`s
    /// must be lowered to physical first — the caller's job in slice 2 routing;
    /// pass empty for the decode-only / test path).
    pub fn new(
        schema: SchemaRef,
        filesystem_factory: Arc<FilesystemFactory>,
        name_to_col_id: HashMap<String, i32>,
        filters: Vec<Arc<dyn PhysicalExpr>>,
    ) -> Self {
        let mut prune_predicates = Vec::new();
        for f in &filters {
            prune_predicates.extend(pruning_predicates(f));
        }
        Self {
            schema,
            filesystem_factory,
            name_to_col_id,
            prune_predicates,
        }
    }

    /// Whole-file read of the split's PAX bytes. Slice-1 simplification: a PAX
    /// segment is one block ≈ one batch, so `split.offset`/`length` are not
    /// honored (ranged reads are a follow-up). This is the only I/O path.
    async fn load_bytes(&self, split: &FileSplit) -> DFResult<Vec<u8>> {
        self.filesystem_factory
            .read(&split.file_path)
            .await
            .map_err(|e| {
                DataFusionError::Execution(format!(
                    "PAX scan: read of {} failed: {e}",
                    split.file_path
                ))
            })
    }

    /// Core decode (no I/O): bytes → `PaxBlockReader` → prune → decode → batch.
    /// Returns `Ok(None)` when the block is pruned by the zone maps. Pure / SRP
    /// boundary so the bridge + prune stack can be exercised without a filesystem.
    fn decode_block(&self, bytes: &[u8], out_schema: &SchemaRef) -> DFResult<Option<RecordBatch>> {
        let reader = PaxBlockReader::open(bytes)
            .map_err(|e| DataFusionError::Execution(format!("PAX open failed: {e}")))?;

        if self.block_pruned(&reader) {
            trace!("PAX scan: block pruned by zone map");
            return Ok(None);
        }

        let arrays: Vec<ArrayRef> = out_schema
            .fields()
            .iter()
            .map(|f| self.decode_column(&reader, f.name()))
            .collect::<DFResult<_>>()?;
        let batch = RecordBatch::try_new(out_schema.clone(), arrays)
            .map_err(|e| DataFusionError::Execution(format!("PAX batch build failed: {e}")))?;
        Ok(Some(batch))
    }

    /// AND-semantics block prune: returns `true` if ANY conjunctive predicate
    /// provably excludes the block. Unrecognized column/operator ⇒ no prune.
    fn block_pruned(&self, reader: &PaxBlockReader) -> bool {
        self.prune_predicates.iter().any(|(name, pred)| {
            let Some(&cid) = self.name_to_col_id.get(name) else {
                return false; // unknown column → cannot prune
            };
            !block_may_contain(reader, cid, pred)
        })
    }

    /// Decode one column by its DataFusion name → PAX column_id → typed stripe.
    fn decode_column(&self, reader: &PaxBlockReader, name: &str) -> DFResult<ArrayRef> {
        let cid = *self
            .name_to_col_id
            .get(name)
            .ok_or_else(|| DataFusionError::Execution(format!("PAX: no column_id for {name}")))?;
        let meta = reader
            .column_metas()
            .iter()
            .find(|m| m.column_id == cid)
            .ok_or_else(|| {
                DataFusionError::Execution(format!("PAX: column {name} (id {cid}) not in block"))
            })?;
        // TypeId bytes (proximadb_codec): I64=0x03, F64=0x02, F32=0x01;
        // 0xff = variable-length (string/bytes).
        Ok(match meta.data_type_id {
            0x03 => Arc::new(Int64Array::from(decode_i64(reader, cid, name)?)),
            0x02 => Arc::new(Float64Array::from(decode_f64(reader, cid, name)?)),
            0xff if meta.role == ColumnRole::Props => {
                Arc::new(BinaryArray::from_iter(decode_bytes(reader, cid, name)?))
            }
            0xff => Arc::new(StringArray::from(decode_str(reader, cid, name)?)),
            other => {
                return Err(DataFusionError::NotImplemented(format!(
                    "PAX data_type_id {other:#x} for {name} (slice 1 covers i64/f64/str/bytes; \
                     f32 vector decode is slice 2)"
                )));
            }
        })
    }
}

#[async_trait]
impl SplitReader for PaxSplitReader {
    async fn read_split(
        &self,
        split: &FileSplit,
        projection: Option<&[usize]>,
        _batch_size: usize,
    ) -> DFResult<SendableRecordBatchStream> {
        let out_schema = match projection {
            Some(idx) => Arc::new(
                self.schema
                    .project(idx)
                    .map_err(|e| DataFusionError::Execution(format!("projection: {e}")))?,
            ),
            None => self.schema.clone(),
        };
        let bytes = self.load_bytes(split).await?;
        match self.decode_block(&bytes, &out_schema)? {
            Some(batch) => Ok(Box::pin(RecordBatchStreamAdapter::new(
                out_schema,
                stream::once(future::ok(batch)),
            ))),
            None => Ok(Box::pin(EmptyRecordBatchStream::new(out_schema))),
        }
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn engine_type(&self) -> EngineType {
        // No `Pax` variant exists (PAX is the SST block format); slice 2 may add one.
        EngineType::Sst
    }

    fn supports_filter_pushdown(&self) -> bool {
        true
    }

    fn supports_projection_pushdown(&self) -> bool {
        true
    }
}

// ---- free helpers (no `&self`, reusable / testable in isolation) ----

fn decode_i64(reader: &PaxBlockReader, cid: i32, name: &str) -> DFResult<Vec<Option<i64>>> {
    reader.decode_i64_stripe(cid).ok_or_else(|| {
        DataFusionError::Execution(format!("PAX: i64 decode failed for {name} (id {cid})"))
    })
}

fn decode_f64(reader: &PaxBlockReader, cid: i32, name: &str) -> DFResult<Vec<Option<f64>>> {
    reader.decode_f64_stripe(cid).ok_or_else(|| {
        DataFusionError::Execution(format!("PAX: f64 decode failed for {name} (id {cid})"))
    })
}

fn decode_str(reader: &PaxBlockReader, cid: i32, name: &str) -> DFResult<Vec<Option<String>>> {
    reader.decode_str_stripe(cid).ok_or_else(|| {
        DataFusionError::Execution(format!("PAX: str decode failed for {name} (id {cid})"))
    })
}

fn decode_bytes(reader: &PaxBlockReader, cid: i32, name: &str) -> DFResult<Vec<Option<Vec<u8>>>> {
    reader.decode_bytes_stripe(cid).ok_or_else(|| {
        DataFusionError::Execution(format!("PAX: bytes decode failed for {name} (id {cid})"))
    })
}

/// `true` iff the block MAY contain a row satisfying `pred` (i.e. NOT pruned).
/// Conservative: any unrecognized shape returns `true` (cannot prune).
fn block_may_contain(reader: &PaxBlockReader, cid: i32, pred: &ScalarPredicate) -> bool {
    use ScalarPredicate as P;
    use ScalarValue as V;
    match pred {
        // Point equality → i64 zone point / f64 degenerate range / string bloom+hash.
        P::Equal(V::Int64(v)) => reader.column_may_contain_i64(cid, *v),
        P::Equal(V::Float64(v)) => reader.column_range_overlaps_f64(cid, *v, *v),
        P::Equal(V::String(s)) => reader.column_may_contain_str(cid, s),
        // i64 ranges: Gt/GtEq ⇒ [v, MAX]; Lt/LtEq ⇒ [MIN, v]; Between ⇒ [lo, hi].
        P::GreaterThan(V::Int64(v)) | P::GreaterThanOrEqual(V::Int64(v)) => {
            reader.column_range_overlaps_i64(cid, *v, i64::MAX)
        }
        P::LessThan(V::Int64(v)) | P::LessThanOrEqual(V::Int64(v)) => {
            reader.column_range_overlaps_i64(cid, i64::MIN, *v)
        }
        P::Between(V::Int64(lo), V::Int64(hi)) => reader.column_range_overlaps_i64(cid, *lo, *hi),
        // f64 ranges.
        P::GreaterThan(V::Float64(v)) | P::GreaterThanOrEqual(V::Float64(v)) => {
            reader.column_range_overlaps_f64(cid, *v, f64::INFINITY)
        }
        P::LessThan(V::Float64(v)) | P::LessThanOrEqual(V::Float64(v)) => {
            reader.column_range_overlaps_f64(cid, f64::NEG_INFINITY, *v)
        }
        P::Between(V::Float64(lo), V::Float64(hi)) => {
            reader.column_range_overlaps_f64(cid, *lo, *hi)
        }
        // NotEqual / IsNull / IsNotNull / In / Bool / Null / type-mismatched ⇒ cannot prune.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::logical_expr::Operator;
    use datafusion::physical_expr::expressions::{BinaryExpr, col, lit};
    use proximadb_block_format::{BlockCompression, BlockMode, PaxBlockWriter, col_id};
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

    fn block_with_created_at(r1: i64, r2: i64) -> Vec<u8> {
        let mut w = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "col", 0, 0);
        w.add_record(&record("r1", "t", r1)).unwrap();
        w.add_record(&record("r2", "t", r2)).unwrap();
        w.flush().unwrap()
    }

    /// `[created_at]` schema + its name→column_id map.
    fn created_at_schema() -> (SchemaRef, HashMap<String, i32>) {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "created_at",
            DataType::Int64,
            true,
        )]));
        let map = HashMap::from([(String::from("created_at"), col_id::CREATED_AT)]);
        (schema, map)
    }

    async fn new_reader(
        schema: SchemaRef,
        map: HashMap<String, i32>,
        filters: Vec<Arc<dyn PhysicalExpr>>,
    ) -> PaxSplitReader {
        let fs = Arc::new(FilesystemFactory::create_default().await.unwrap());
        PaxSplitReader::new(schema, fs, map, filters)
    }

    #[tokio::test]
    async fn decodes_relational_stripes_to_arrow() {
        // Zone map for created_at is [1000, 3000].
        let bytes = block_with_created_at(1000, 3000);
        let (schema, map) = created_at_schema();
        let reader = new_reader(schema.clone(), map, vec![]).await;

        let batch = reader
            .decode_block(&bytes, &schema)
            .unwrap()
            .expect("unfiltered block must not prune");
        assert_eq!(batch.num_rows(), 2);
        let ca = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ca.value(0), 1000);
        assert_eq!(ca.value(1), 3000);
    }

    #[tokio::test]
    async fn zone_map_prunes_disjoint_block() {
        let bytes = block_with_created_at(1000, 3000);
        let (schema, map) = created_at_schema();

        // `created_at >= 4000` — range [4000, MAX] is disjoint from [1000, 3000] ⇒ prune.
        let ge_4000: Arc<dyn PhysicalExpr> = Arc::new(BinaryExpr::new(
            col("created_at", schema.as_ref()).unwrap(),
            Operator::GtEq,
            lit(4000i64),
        ));
        let reader = new_reader(schema.clone(), map, vec![ge_4000]).await;
        assert!(
            reader.decode_block(&bytes, &schema).unwrap().is_none(),
            "block [1000,3000] must be pruned under created_at >= 4000"
        );

        // `created_at <= 500` — range [MIN, 500] is disjoint ⇒ prune.
        let le_500: Arc<dyn PhysicalExpr> = Arc::new(BinaryExpr::new(
            col("created_at", schema.as_ref()).unwrap(),
            Operator::LtEq,
            lit(500i64),
        ));
        let reader = new_reader(schema.clone(), created_at_schema().1, vec![le_500]).await;
        assert!(
            reader.decode_block(&bytes, &schema).unwrap().is_none(),
            "block [1000,3000] must be pruned under created_at <= 500"
        );

        // `created_at >= 2000` — range [2000, MAX] overlaps [1000, 3000] ⇒ NOT pruned.
        let ge_2000: Arc<dyn PhysicalExpr> = Arc::new(BinaryExpr::new(
            col("created_at", schema.as_ref()).unwrap(),
            Operator::GtEq,
            lit(2000i64),
        ));
        let reader = new_reader(schema.clone(), created_at_schema().1, vec![ge_2000]).await;
        assert!(
            reader.decode_block(&bytes, &schema).unwrap().is_some(),
            "block [1000,3000] must NOT prune under created_at >= 2000 (overlaps)"
        );
    }

    #[tokio::test]
    async fn empty_filter_set_never_prunes() {
        let bytes = block_with_created_at(1000, 3000);
        let (schema, map) = created_at_schema();
        let reader = new_reader(schema.clone(), map, vec![]).await;
        // No predicates ⇒ never pruned, regardless of data.
        assert!(reader.decode_block(&bytes, &schema).unwrap().is_some());
    }
}
