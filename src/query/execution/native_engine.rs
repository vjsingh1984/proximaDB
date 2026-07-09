// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # Native vectorized execution hook (ADR-054 Phase 0/1)
//!
//! Internal to the `Native` backend — NOT a separate `ComputeBackend` variant.
//! When the native engine receives an analytical query (relational + not
//! Parquet-backed), it checks `native_vectorized_enabled()` and, if set, tries
//! the vectorized pipeline (Phase 1+) before falling back to the Volcano.
//!
//! Phase 1: the Arrow→RelationalRow conversion (the output bridge) is
//! implemented here — self-contained, zero DataFusion dependency. This lets
//! the native vectorized engine produce `ExecutionPipelineResult` from Arrow
//! `RecordBatch`es.
//!
//! Phase 2+: FilterProjectOperator + pipeline execution + LogicalNode lowering.

use crate::query::execution::engine::{
    ExecutionError, ExecutionPipelineResult, ExecutionStreamResult,
};
use proximadb_data_model::ProximaValue;
use proximadb_relational_planner::PhysicalPlan;
use proximadb_relational_types::{ColumnInfo, RelationalRow, RelationalSchema};

/// Gate: is the native vectorized execution path opted in? Default OFF — the
/// Volcano (row-at-a-time) serves all native-path queries until the vectorized
/// path is promoted (ledger-gated, per ADR-054 §8).
pub fn native_vectorized_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("PROXIMADB_NATIVE_VECTORIZED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// Gate: should the native hash-join path be used? Default OFF — distinct from
/// `PROXIMADB_NATIVE_VECTORIZED` so FilterProject/HashAgg can serve without
/// forcing the join on. When ON, `lower_physical::Join` wires the #779
/// `HashJoinBuildOperator`/`HashJoinProbeOperator` (ADR-054 Phase 3, TD-OLAP-11).
pub fn native_join_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("PROXIMADB_NATIVE_JOIN")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// Try the native vectorized execution path for an already-planned query.
/// Called from `NativeVolcanoEngine::execute_physical` (the single native
/// chokepoint, above the Volcano) when [`native_vectorized_enabled`] is set.
///
/// Returns `Ok(Some(result))` if the vectorized engine handled the query.
/// Returns `Ok(None)` when the path is disabled OR the plan shape / constructs
/// are unsupported (Phase 2: only `Project`/`Filter`/`Limit` over `Values`) OR
/// execution hit an issue — in every such case the caller falls back to the
/// Volcano. The experimental, default-off path never fails a query; correctness
/// is policed by the shadow-comparison harness (TD-OLAP-10 §"Shadow comparison").
pub async fn try_vectorized(
    physical: &PhysicalPlan,
) -> Result<Option<ExecutionPipelineResult>, ExecutionError> {
    if !native_vectorized_enabled() {
        return Ok(None);
    }
    // Any decline (unsupported shape) or failure (execution error) → Volcano.
    let lowered = match super::native_ops::lower_physical(physical) {
        Ok(l) => l,
        Err(reason) => {
            tracing::debug!(
                target: "proximadb::native_vectorized",
                %reason,
                "vectorized path declined; falling back to Volcano"
            );
            return Ok(None);
        }
    };
    let batches = match super::native_ops::execute_pipeline(&lowered).await {
        Ok(b) => b,
        Err(reason) => {
            tracing::debug!(
                target: "proximadb::native_vectorized",
                %reason,
                "vectorized path failed mid-execution; falling back to Volcano"
            );
            return Ok(None);
        }
    };
    let schema = lowered.pipeline.output_schema.as_ref();
    Ok(Some(record_batches_to_pipeline_result(schema, &batches)))
}

/// Streaming variant — not yet wired (the materialized path serves Phase 2).
pub async fn try_vectorized_stream(
    _physical: &PhysicalPlan,
) -> Result<Option<ExecutionStreamResult>, ExecutionError> {
    Ok(None)
}

// ---------------------------------------------------------------------------
// Arrow → RelationalRow conversion (Phase 1 — the output bridge)
// ---------------------------------------------------------------------------

/// Convert Arrow RecordBatches → ExecutionPipelineResult.
/// Self-contained (zero DataFusion dependency). Mirrors the DataFusion adapter's
/// private `record_batches_to_pipeline_result` but lives here so the native
/// engine doesn't depend on the adapter. Consumed by `try_vectorized`.
pub(crate) fn record_batches_to_pipeline_result(
    arrow_schema: &arrow::datatypes::Schema,
    batches: &[arrow_array::RecordBatch],
) -> ExecutionPipelineResult {
    let schema = arrow_schema_to_relational(arrow_schema);
    let mut rows: Vec<RelationalRow> = Vec::new();
    for batch in batches {
        rows.extend(record_batch_to_rows(batch));
    }
    ExecutionPipelineResult { schema, rows }
}

fn arrow_schema_to_relational(schema: &arrow::datatypes::Schema) -> RelationalSchema {
    let columns: Vec<ColumnInfo> = schema
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

fn arrow_type_to_proxima(dt: &arrow::datatypes::DataType) -> proximadb_data_model::ProximaType {
    use arrow::datatypes::DataType as D;
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
        D::Float32 => ProximaType::Float32,
        D::Float64 => ProximaType::Float64,
        D::Utf8 | D::LargeUtf8 => ProximaType::String,
        D::Binary | D::LargeBinary => ProximaType::Binary,
        D::Date32 | D::Date64 => ProximaType::Date,
        _ => ProximaType::String,
    }
}

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

pub(crate) fn arrow_cell_to_proxima(array: &dyn arrow_array::Array, row: usize) -> ProximaValue {
    use arrow::datatypes::DataType as D;
    use proximadb_data_model::{ProximaValue, TimeUnit};

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
        D::Boolean => v!(arrow_array::BooleanArray, ProximaValue::Boolean),
        D::Int8 => v!(arrow_array::Int8Array, ProximaValue::Int8),
        D::Int16 => v!(arrow_array::Int16Array, ProximaValue::Int16),
        D::Int32 => v!(arrow_array::Int32Array, ProximaValue::Int32),
        D::Int64 => v!(arrow_array::Int64Array, ProximaValue::Int64),
        D::UInt8 => v!(arrow_array::UInt8Array, ProximaValue::UInt8),
        D::UInt16 => v!(arrow_array::UInt16Array, ProximaValue::UInt16),
        D::UInt32 => v!(arrow_array::UInt32Array, ProximaValue::UInt32),
        D::UInt64 => v!(arrow_array::UInt64Array, ProximaValue::UInt64),
        D::Float32 => v!(arrow_array::Float32Array, ProximaValue::Float32),
        D::Float64 => v!(arrow_array::Float64Array, ProximaValue::Float64),
        D::Utf8 => array
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .map(|a| ProximaValue::String(a.value(row).to_string())),
        D::LargeUtf8 => array
            .as_any()
            .downcast_ref::<arrow_array::LargeStringArray>()
            .map(|a| ProximaValue::String(a.value(row).to_string())),
        D::Binary => array
            .as_any()
            .downcast_ref::<arrow_array::BinaryArray>()
            .map(|a| ProximaValue::Binary(a.value(row).to_vec())),
        D::LargeBinary => array
            .as_any()
            .downcast_ref::<arrow_array::LargeBinaryArray>()
            .map(|a| ProximaValue::Binary(a.value(row).to_vec())),
        D::Date32 => v!(arrow_array::Date32Array, ProximaValue::Date),
        D::Date64 => array
            .as_any()
            .downcast_ref::<arrow_array::Date64Array>()
            .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Millisecond)),
        D::Timestamp(unit, _) => match unit {
            arrow::datatypes::TimeUnit::Second => array
                .as_any()
                .downcast_ref::<arrow_array::TimestampSecondArray>()
                .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Second)),
            arrow::datatypes::TimeUnit::Millisecond => array
                .as_any()
                .downcast_ref::<arrow_array::TimestampMillisecondArray>()
                .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Millisecond)),
            arrow::datatypes::TimeUnit::Microsecond => array
                .as_any()
                .downcast_ref::<arrow_array::TimestampMicrosecondArray>()
                .map(|a| ProximaValue::Timestamp(a.value(row), TimeUnit::Microsecond)),
            arrow::datatypes::TimeUnit::Nanosecond => array
                .as_any()
                .downcast_ref::<arrow_array::TimestampNanosecondArray>()
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow_array::{Int64Array, RecordBatch};
    use std::sync::Arc;

    #[test]
    fn convert_int64_batch_to_pipeline_result() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Int64, false),
            Field::new("v", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(arrow_array::StringArray::from(vec![
                    Some("a"),
                    None,
                    Some("c"),
                ])),
            ],
        )
        .unwrap();

        let result = record_batches_to_pipeline_result(&schema, &[batch]);
        assert_eq!(result.rows.len(), 3);
        assert!(matches!(result.rows[0][0], ProximaValue::Int64(1)));
        assert!(matches!(result.rows[0][1], ProximaValue::String(ref s) if s == "a"));
        assert!(matches!(result.rows[1][0], ProximaValue::Int64(2)));
        assert!(matches!(result.rows[1][1], ProximaValue::Null));
        assert!(matches!(result.rows[2][0], ProximaValue::Int64(3)));
        assert!(matches!(result.rows[2][1], ProximaValue::String(ref s) if s == "c"));
        assert_eq!(result.schema.columns.len(), 2);
        assert_eq!(result.schema.columns[0].name, "k");
        assert_eq!(result.schema.columns[1].name, "v");
    }

    #[test]
    fn convert_empty_batch() {
        let schema = Arc::new(Schema::empty());
        let batch = RecordBatch::new_empty(schema.clone());
        let result = record_batches_to_pipeline_result(&schema, &[batch]);
        assert!(result.rows.is_empty());
    }
}
