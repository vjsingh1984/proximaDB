// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # Native hash-join operators (ADR-054 Phase 3, TD-OLAP-11)
//!
//! The native radix/bloom/spill hash join — the #1 performance lever (closes the
//! Q3/Q14 join gap). MVP (Phase 3.0): **in-memory, single non-partitioned hash
//! table** (size-gated radix is a later sub-phase; spill is Phase 3.1).
//!
//! Three operators across two pipelines (the build pipeline drains first and
//! publishes `Arc<JoinHashTable>` via a shared `OnceLock`; the probe pipeline
//! reads it):
//! * [`HashJoinBuildOperator`] — BLOCKING. Consumes the build side, builds the
//!   `key → build-row-indices` map + a bloom over the build keys, publishes.
//! * [`HashJoinProbeOperator`] — STREAMING. Per probe batch: bloom pre-filter →
//!   hash-table lookup → late-materialize matched pairs via `arrow::compute::take`.
//!
//! MVP `JoinKind`: Inner / Left / Semi / Anti. Right / Full (unmatched-build
//! drain) + spill + radix are deferred (see TD-OLAP-11). NULL join keys never
//! match in any kind.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use arrow::array::{Array, ArrayRef, RecordBatch, UInt32Array};
use arrow::compute::{concat_batches, take};
use arrow::datatypes::{DataType, SchemaRef};
use async_trait::async_trait;
use futures::stream::StreamExt;
use proximadb_bloom::{BloomFilterBuilder, BloomFilterStrategy};
use proximadb_execution_contracts::{BatchStream, ExecutionError, ExecutionOperator};
use proximadb_relational_algebra::JoinKind;

/// Minimum build-side cardinality to bother building a bloom (below this the
/// hash table is small enough that the bloom's overhead isn't worth it).
const BLOOM_MIN_KEYS: usize = 1024;

// =========================================================================
// Key encoding + the hash table
// =========================================================================

/// Canonical byte encoding of a composite join key at `row` across `columns`.
/// Returns `None` if ANY key column is NULL at that row (NULL keys never match).
fn canonical_key_bytes(
    columns: &[&dyn Array],
    row: usize,
) -> Result<Option<Vec<u8>>, ExecutionError> {
    let mut buf = Vec::new();
    for col in columns {
        if col.is_null(row) {
            return Ok(None);
        }
        encode_cell(*col, row, &mut buf)?;
        buf.push(0); // separator (multi-column keys)
    }
    Ok(Some(buf))
}

/// Typed downcast that errors (never panics) — the repo's panic-policy forbids
/// the unwrap-on-downcast idiom. (A macro, not a generic fn, to avoid the
/// downcast `'static` lifetime-bound friction.)
macro_rules! dcast {
    ($array:expr, $t:ty) => {
        $array.as_any().downcast_ref::<$t>().ok_or_else(|| {
            ExecutionError::Execution(format!(
                "arrow downcast failed for {:?}",
                $array.data_type()
            ))
        })?
    };
}

/// Encode one non-null cell to `out` in a canonical, type-stable byte form.
fn encode_cell(array: &dyn Array, row: usize, out: &mut Vec<u8>) -> Result<(), ExecutionError> {
    use arrow::array::*;
    match array.data_type() {
        DataType::Boolean => out.push(dcast!(array, BooleanArray).value(row) as u8),
        DataType::Int8 => out.push(dcast!(array, Int8Array).value(row) as u8),
        DataType::Int16 => {
            out.extend_from_slice(&dcast!(array, Int16Array).value(row).to_le_bytes())
        }
        DataType::Int32 => {
            out.extend_from_slice(&dcast!(array, Int32Array).value(row).to_le_bytes())
        }
        DataType::Int64 => {
            out.extend_from_slice(&dcast!(array, Int64Array).value(row).to_le_bytes())
        }
        DataType::UInt8 => out.push(dcast!(array, UInt8Array).value(row)),
        DataType::UInt16 => {
            out.extend_from_slice(&dcast!(array, UInt16Array).value(row).to_le_bytes())
        }
        DataType::UInt32 => {
            out.extend_from_slice(&dcast!(array, UInt32Array).value(row).to_le_bytes())
        }
        DataType::UInt64 => {
            out.extend_from_slice(&dcast!(array, UInt64Array).value(row).to_le_bytes())
        }
        DataType::Float32 => {
            out.extend_from_slice(&dcast!(array, Float32Array).value(row).to_le_bytes())
        }
        DataType::Float64 => {
            out.extend_from_slice(&dcast!(array, Float64Array).value(row).to_le_bytes())
        }
        DataType::Utf8 => out.extend_from_slice(dcast!(array, StringArray).value(row).as_bytes()),
        DataType::LargeUtf8 => {
            out.extend_from_slice(dcast!(array, LargeStringArray).value(row).as_bytes())
        }
        DataType::Binary => out.extend_from_slice(dcast!(array, BinaryArray).value(row)),
        other => {
            return Err(ExecutionError::NotImplemented(format!(
                "join key type {other:?} not supported in native hash join"
            )));
        }
    }
    Ok(())
}

/// The build side's hash table: composite-key bytes → build-side row indices.
/// Plus the (concatenated) build batch for materializing matched rows, and an
/// optional bloom over the build keys.
pub(crate) struct JoinHashTable {
    pub map: HashMap<Vec<u8>, Vec<u32>>,
    pub build_batch: RecordBatch,
    pub bloom: Option<Arc<dyn BloomFilterStrategy>>,
}

impl std::fmt::Debug for JoinHashTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinHashTable")
            .field("groups", &self.map.len())
            .field("build_rows", &self.build_batch.num_rows())
            .field("has_bloom", &self.bloom.is_some())
            .finish()
    }
}

// =========================================================================
// HashJoinBuildOperator (BLOCKING)
// =========================================================================

/// Builds the hash table + bloom from the build side, then publishes it via the
/// shared `table_slot` (the cross-pipeline handoff to the probe operator).
#[derive(Debug)]
pub(crate) struct HashJoinBuildOperator {
    pub build_keys: Vec<usize>,
    pub build_schema: SchemaRef,
    pub table_slot: Arc<OnceLock<Arc<JoinHashTable>>>,
    pub bloom_enabled: bool,
}

#[async_trait]
impl ExecutionOperator for HashJoinBuildOperator {
    fn output_schema(&self) -> SchemaRef {
        self.build_schema.clone()
    }

    fn is_blocking(&self) -> bool {
        true
    }

    async fn execute(&self, input: BatchStream) -> Result<BatchStream, ExecutionError> {
        let build_keys = self.build_keys.clone();
        let build_schema = self.build_schema.clone();
        let bloom_enabled = self.bloom_enabled;
        let table_slot = self.table_slot.clone();

        let batches: Vec<RecordBatch> = input
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()?;
        let build_batch = if batches.is_empty() {
            RecordBatch::new_empty(build_schema.clone())
        } else if batches.len() == 1 {
            batches
                .into_iter()
                .next()
                .ok_or_else(|| ExecutionError::Execution("join build: empty batch iter".into()))?
        } else {
            concat_batches(&build_schema, batches.iter())
                .map_err(|e| ExecutionError::Execution(format!("join build concat: {e}")))?
        };

        // Build key → row-indices map; collect key bytes for the bloom.
        let key_cols: Vec<&dyn Array> = build_keys
            .iter()
            .map(|&c| build_batch.column(c).as_ref())
            .collect();
        let mut map: HashMap<Vec<u8>, Vec<u32>> = HashMap::new();
        let mut all_keys: Vec<Vec<u8>> = Vec::new();
        for r in 0..build_batch.num_rows() {
            if let Some(kb) = canonical_key_bytes(&key_cols, r)? {
                map.entry(kb.clone()).or_default().push(r as u32);
                all_keys.push(kb);
            } // NULL build keys → never match; absent from the map.
        }

        // Bloom over the build keys (build-then-freeze: insert under &mut, then Arc).
        let bloom: Option<Arc<dyn BloomFilterStrategy>> =
            if bloom_enabled && all_keys.len() >= BLOOM_MIN_KEYS {
                let cfg = proximadb_bloom::CoreBloomFilterConfig {
                    expected_items: all_keys.len(),
                    ..Default::default()
                };
                let mut b = BloomFilterBuilder::new(cfg).build();
                for kb in &all_keys {
                    b.insert(kb);
                }
                Some(Arc::from(b))
            } else {
                None
            };

        let table = Arc::new(JoinHashTable {
            map,
            build_batch,
            bloom,
        });
        // Publish (first writer wins; there is exactly one build pipeline).
        let _ = table_slot.set(table);

        // The build emits nothing the probe consumes; emit an empty sentinel so the
        // pipeline drains cleanly.
        let empty = RecordBatch::new_empty(build_schema);
        Ok(Box::pin(futures::stream::once(async move { Ok(empty) })))
    }
}

// =========================================================================
// HashJoinProbeOperator (STREAMING)
// =========================================================================

/// Streams probe batches: bloom pre-filter → hash-table lookup → late-materialize.
/// MVP `JoinKind`: Inner / Left / Semi / Anti.
#[derive(Debug)]
pub(crate) struct HashJoinProbeOperator {
    pub table_slot: Arc<OnceLock<Arc<JoinHashTable>>>,
    pub probe_keys: Vec<usize>,
    /// Column ordinals to emit, in output order, as (source, ordinal):
    /// `Probe(ordinal)` or `Build(ordinal)`.
    pub output_columns: Vec<JoinColumn>,
    pub kind: JoinKind,
    pub output_schema: SchemaRef,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum JoinColumn {
    Probe(usize),
    Build(usize),
}

#[async_trait]
impl ExecutionOperator for HashJoinProbeOperator {
    fn output_schema(&self) -> SchemaRef {
        self.output_schema.clone()
    }

    async fn execute(&self, input: BatchStream) -> Result<BatchStream, ExecutionError> {
        let table = self
            .table_slot
            .get()
            .ok_or_else(|| {
                ExecutionError::Execution("join probe ran before build published".into())
            })?
            .clone();
        let probe_keys = self.probe_keys.clone();
        let output_columns = self.output_columns.clone();
        let kind = self.kind;
        let output_schema = self.output_schema.clone();

        Ok(Box::pin(input.map(move |result| {
            let probe_batch = result?;
            probe_one_batch(
                &table,
                &probe_batch,
                &probe_keys,
                &output_columns,
                kind,
                &output_schema,
            )
        })))
    }
}

/// Probe one batch against the table; late-materialize the output via `take`.
fn probe_one_batch(
    table: &JoinHashTable,
    probe_batch: &RecordBatch,
    probe_keys: &[usize],
    output_columns: &[JoinColumn],
    kind: JoinKind,
    output_schema: &SchemaRef,
) -> Result<RecordBatch, ExecutionError> {
    let probe_key_cols: Vec<&dyn Array> = probe_keys
        .iter()
        .map(|&c| probe_batch.column(c).as_ref())
        .collect();
    let nrows = probe_batch.num_rows();

    // Gather matched (probe_row, [build_rows]) into index lists.
    let mut probe_idx: Vec<u32> = Vec::new(); // probe row index per emitted output row (Inner)
    let mut build_idx: Vec<u32> = Vec::new(); // matched build row index per emitted output row (Inner)
    let mut left_unmatched: Vec<u32> = Vec::new(); // probe rows with NO match (Left/Anti)

    for r in 0..nrows {
        let kb_opt = canonical_key_bytes(&probe_key_cols, r)?;
        let matched = match kb_opt {
            None => Vec::new(), // NULL probe key → no match
            Some(kb) => {
                // Bloom pre-filter: skip the hash-table lookup if the bloom says no.
                if let Some(b) = &table.bloom {
                    if !b.might_contain(&kb) {
                        Vec::new()
                    } else {
                        table.map.get(&kb).cloned().unwrap_or_default()
                    }
                } else {
                    table.map.get(&kb).cloned().unwrap_or_default()
                }
            }
        };
        if matched.is_empty() {
            left_unmatched.push(r as u32);
        } else {
            for br in matched {
                probe_idx.push(r as u32);
                build_idx.push(br);
            }
        }
    }

    let probe_indices = UInt32Array::from(probe_idx);
    let build_indices = UInt32Array::from(build_idx);

    let columns: Vec<ArrayRef> = match kind {
        JoinKind::Inner => {
            // One output row per (probe row, matched build row).
            output_columns
                .iter()
                .map(|jc| match jc {
                    JoinColumn::Probe(c) => {
                        take(probe_batch.column(*c).as_ref(), &probe_indices, None)
                    }
                    JoinColumn::Build(c) => {
                        take(table.build_batch.column(*c).as_ref(), &build_indices, None)
                    }
                })
                .collect::<Result<_, _>>()
                .map_err(|e| ExecutionError::Execution(format!("join take: {e}")))?
        }
        JoinKind::Left => {
            // Matched pairs + unmatched probe rows (build cols NULL-padded).
            let n_unmatched = left_unmatched.len();
            let matched_probe = &probe_indices;
            let unmatched_probe = UInt32Array::from(left_unmatched);
            output_columns
                .iter()
                .map(|jc| match jc {
                    JoinColumn::Probe(c) => {
                        let col = probe_batch.column(*c);
                        let matched = take(col.as_ref(), matched_probe, None)?;
                        let unmatched = take(col.as_ref(), &unmatched_probe, None)?;
                        arrow::compute::concat(&[matched.as_ref(), unmatched.as_ref()])
                    }
                    JoinColumn::Build(c) => {
                        let col = table.build_batch.column(*c);
                        let matched = take(col.as_ref(), &build_indices, None)?;
                        let nulls = arrow::array::new_null_array(col.data_type(), n_unmatched);
                        arrow::compute::concat(&[matched.as_ref(), nulls.as_ref()])
                    }
                })
                .collect::<Result<_, _>>()
                .map_err(|e| ExecutionError::Execution(format!("join left take: {e}")))?
        }
        JoinKind::Semi | JoinKind::Anti => {
            // Emit probe rows where a match exists (Semi) or doesn't (Anti).
            let emitted = if matches!(kind, JoinKind::Semi) {
                // probe rows with ≥1 match = all probe rows referenced by probe_idx.
                dedup_sorted(&probe_indices)
            } else {
                // Anti = unmatched probe rows.
                left_unmatched.clone()
            };
            let idx = UInt32Array::from(emitted);
            output_columns
                .iter()
                .map(|jc| match jc {
                    JoinColumn::Probe(c) => take(probe_batch.column(*c).as_ref(), &idx, None),
                    JoinColumn::Build(_) => {
                        // Semi/Anti emit no build columns; output_columns should be
                        // probe-only. Defensive: emit a null column of right width.
                        Ok(arrow::array::new_null_array(&DataType::Null, idx.len()))
                    }
                })
                .collect::<Result<_, _>>()
                .map_err(|e| ExecutionError::Execution(format!("join semi/anti take: {e}")))?
        }
        other => {
            return Err(ExecutionError::NotImplemented(format!(
                "JoinKind {other:?} not supported in native hash join MVP (Inner/Left/Semi/Anti)"
            )));
        }
    };

    RecordBatch::try_new(output_schema.clone(), columns)
        .map_err(|e| ExecutionError::Execution(format!("join output batch: {e}")))
}

/// Distinct, sorted row indices from a (sorted, possibly-duplicated) UInt32 index array.
fn dedup_sorted(idx: &UInt32Array) -> Vec<u32> {
    let mut out: Vec<u32> = idx.values().iter().copied().collect();
    out.sort_unstable();
    out.dedup();
    out
}
