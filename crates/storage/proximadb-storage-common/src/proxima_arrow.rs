//! # proxima_arrow — canonical `ProximaRecord` → Arrow `RecordBatch` converter (F1.5)
//!
//! The single, schema-driven home for turning canonical [`ProximaRecord`]s into an Arrow
//! [`RecordBatch`]. It is the data-plane companion to [`ProximaSchema::to_arrow_schema`]
//! (the *schema* plane): the Arrow schema is taken verbatim from the [`ProximaSchema`], and
//! each column's values are materialized into the exact Arrow array type that
//! [`ProximaColumn::to_arrow_type`] declares — so the produced batch always satisfies
//! `RecordBatch::try_new` against `schema.to_arrow_schema()`.
//!
//! ## Why this exists
//!
//! Three ad-hoc converters predate this one and each loses information:
//! - `src/connectors/spark.rs` (`proxima_records_to_record_batch`) — **id + vector only**;
//!   per-record `props` were an explicit "follow-up".
//! - `src/storage/formats/arrow_conversion.rs` — `VectorRecord`-based (legacy edge type).
//! - `codec.rs::vector_records_to_batch` — collapses all props into one `StructArray`.
//!
//! This module fixes the prop-collapse class at the source: every non-deleted
//! [`ProximaColumn`] becomes its own typed, null-masked Arrow column, read from
//! `record.props` (via [`tree_get`]) for scalar columns and from `record.embeddings` for the
//! vector column. It unblocks the warehouse base-tier write path (F2
//! `write_records_to_parquet` = this converter + a parquet write over `ProximaObjectStore`)
//! and read-side schema/row fidelity (F5).
//!
//! ## Mapping contract
//!
//! Columns are emitted in `schema.columns` order, skipping tombstoned (`is_deleted`) ones,
//! matching `to_arrow_schema`. For each column:
//! - **Scalar columns** read `tree_get(&record.props, &col.name)`. An absent key, a
//!   [`ProximaValue::Null`], or a value that cannot be coerced to the column's family yields
//!   an Arrow null (NOT an error — sparse props are normal). Cross-family coercions are
//!   modest and lossless-widening only (any integer → a wider int; any int/float → float).
//! - **Vector columns** ([`ProximaDataType::Vector`]) read `record.embeddings.first()` and
//!   encode the fp32 values as little-endian bytes into a `FixedSizeBinary(dimension * 4)`
//!   array — exactly the Arrow type `to_arrow_type` declares. A record with no embedding is
//!   null; a dimension mismatch is a hard error (it would corrupt the fixed-width layout).
//!
//! Complex/temporal/decimal column types that have no scalar `props` representation yet
//! return a [`StorageError::Serialization`] (with an "unsupported column type" message)
//! rather than silently emitting nulls, so the gap is visible to the caller.

use std::sync::Arc;

use arrow_array::builder::{
    BinaryBuilder, BooleanBuilder, Date32Builder, FixedSizeBinaryBuilder, Float32Builder,
    Float64Builder, Int8Builder, Int16Builder, Int32Builder, Int64Builder, StringBuilder,
    UInt8Builder, UInt16Builder, UInt32Builder, UInt64Builder,
};
use std::collections::HashMap;

use arrow_array::{ArrayRef, RecordBatch};
use proximadb_kernel::error::StorageError;
use proximadb_records::{ProximaRecord, ProximaTreeNode, ProximaValue, tree_get};

use crate::proxima_schema::{ProximaColumn, ProximaDataType, ProximaSchema, VectorElementType};

/// Convert a slice of canonical [`ProximaRecord`]s into an Arrow [`RecordBatch`] whose schema
/// is `schema.to_arrow_schema()` and whose columns are materialized per the mapping contract
/// documented at the module level.
///
/// Returns a [`StorageError::Serialization`] for column types without a scalar `props`
/// materialization yet, and [`StorageError::InvalidDimension`] when an embedding's length
/// does not match its declared vector dimension.
pub fn proxima_records_to_record_batch(
    records: &[ProximaRecord],
    schema: &ProximaSchema,
) -> Result<RecordBatch, StorageError> {
    let arrow_schema = schema.to_arrow_schema();

    let mut columns: Vec<ArrayRef> = Vec::with_capacity(arrow_schema.fields().len());
    for col in schema.columns.iter().filter(|c| !c.is_deleted) {
        columns.push(build_column(records, col)?);
    }

    RecordBatch::try_new(arrow_schema, columns).map_err(|e| {
        StorageError::Serialization(format!("proxima_arrow: RecordBatch::try_new failed: {e}"))
    })
}

/// Build one Arrow array for `col` over every record, in row order.
fn build_column(records: &[ProximaRecord], col: &ProximaColumn) -> Result<ArrayRef, StorageError> {
    let n = records.len();
    macro_rules! scalar_col {
        ($builder:ident, $extract:expr) => {{
            let mut b = $builder::with_capacity(n);
            for r in records {
                match tree_get(&r.props, &col.name) {
                    Some(v) => match ($extract)(v) {
                        Some(x) => b.append_value(x),
                        None => b.append_null(),
                    },
                    None => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()) as ArrayRef)
        }};
    }

    match &col.data_type {
        ProximaDataType::Boolean => scalar_col!(BooleanBuilder, pv_as_bool),
        ProximaDataType::Int8 => scalar_col!(Int8Builder, |v| pv_as_i64(v).map(|x| x as i8)),
        ProximaDataType::Int16 => scalar_col!(Int16Builder, |v| pv_as_i64(v).map(|x| x as i16)),
        ProximaDataType::Int32 => scalar_col!(Int32Builder, |v| pv_as_i64(v).map(|x| x as i32)),
        ProximaDataType::Int64 => scalar_col!(Int64Builder, pv_as_i64),
        ProximaDataType::UInt8 => scalar_col!(UInt8Builder, |v| pv_as_u64(v).map(|x| x as u8)),
        ProximaDataType::UInt16 => scalar_col!(UInt16Builder, |v| pv_as_u64(v).map(|x| x as u16)),
        ProximaDataType::UInt32 => scalar_col!(UInt32Builder, |v| pv_as_u64(v).map(|x| x as u32)),
        ProximaDataType::UInt64 => scalar_col!(UInt64Builder, pv_as_u64),
        ProximaDataType::Float32 => scalar_col!(Float32Builder, |v| pv_as_f64(v).map(|x| x as f32)),
        ProximaDataType::Float64 => scalar_col!(Float64Builder, pv_as_f64),
        ProximaDataType::Date => scalar_col!(Date32Builder, pv_as_date32),
        // Utf8-backed types per `to_arrow_type` (String, Uuid, Json).
        ProximaDataType::String | ProximaDataType::Uuid | ProximaDataType::Json => {
            let mut b = StringBuilder::new();
            for r in records {
                match tree_get(&r.props, &col.name).and_then(pv_as_string) {
                    Some(s) => b.append_value(s),
                    None => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()) as ArrayRef)
        }
        ProximaDataType::Binary => {
            let mut b = BinaryBuilder::new();
            for r in records {
                match tree_get(&r.props, &col.name).and_then(pv_as_binary) {
                    Some(bytes) => b.append_value(bytes),
                    None => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()) as ArrayRef)
        }
        ProximaDataType::Vector { dimension, .. } => build_vector_column(records, col, *dimension),
        other => Err(StorageError::Serialization(format!(
            "proxima_arrow: column `{}` has type {:?}, which has no canonical ProximaValue→Arrow \
             materialization yet",
            col.name, other
        ))),
    }
}

/// Build the dense-vector column as `FixedSizeBinary(dimension * 4)`, encoding each record's
/// first embedding as little-endian fp32 bytes (the layout `to_arrow_type` declares for
/// [`ProximaDataType::Vector`]). Absent embedding → null; length mismatch → hard error.
fn build_vector_column(
    records: &[ProximaRecord],
    col: &ProximaColumn,
    dimension: u32,
) -> Result<ArrayRef, StorageError> {
    let byte_width = (dimension as usize) * 4;
    let mut b = FixedSizeBinaryBuilder::with_capacity(records.len(), byte_width as i32);
    for r in records {
        match r.embeddings.first() {
            Some(cell) => {
                let vals = cell.values.to_fp32_owned();
                if vals.len() != dimension as usize {
                    return Err(StorageError::InvalidDimension {
                        expected: dimension as usize,
                        actual: vals.len(),
                    });
                }
                let mut bytes = Vec::with_capacity(byte_width);
                for f in &vals {
                    bytes.extend_from_slice(&f.to_le_bytes());
                }
                b.append_value(&bytes).map_err(|e| {
                    StorageError::Serialization(format!(
                        "proxima_arrow: vector column `{}` append failed: {e}",
                        col.name
                    ))
                })?;
            }
            None => b.append_null(),
        }
    }
    Ok(Arc::new(b.finish()) as ArrayRef)
}

// ---------------------------------------------------------------------------
// ProximaValue extractors (None = emit Arrow null; modest, widening-only coercions)
// ---------------------------------------------------------------------------

fn pv_as_bool(v: &ProximaValue) -> Option<bool> {
    match v {
        ProximaValue::Boolean(b) => Some(*b),
        _ => None,
    }
}

/// Any signed/unsigned integer variant widened to `i64`. (`UInt64` is reinterpreted; values
/// above `i64::MAX` are vanishingly rare for relational keys and would only matter for a true
/// `UInt64` column, which routes through [`pv_as_u64`] instead.)
fn pv_as_i64(v: &ProximaValue) -> Option<i64> {
    match v {
        ProximaValue::Int8(x) => Some(*x as i64),
        ProximaValue::Int16(x) => Some(*x as i64),
        ProximaValue::Int32(x) => Some(*x as i64),
        ProximaValue::Int64(x) => Some(*x),
        ProximaValue::UInt8(x) => Some(*x as i64),
        ProximaValue::UInt16(x) => Some(*x as i64),
        ProximaValue::UInt32(x) => Some(*x as i64),
        ProximaValue::UInt64(x) => Some(*x as i64),
        _ => None,
    }
}

fn pv_as_u64(v: &ProximaValue) -> Option<u64> {
    match v {
        ProximaValue::UInt8(x) => Some(*x as u64),
        ProximaValue::UInt16(x) => Some(*x as u64),
        ProximaValue::UInt32(x) => Some(*x as u64),
        ProximaValue::UInt64(x) => Some(*x),
        ProximaValue::Int8(x) if *x >= 0 => Some(*x as u64),
        ProximaValue::Int16(x) if *x >= 0 => Some(*x as u64),
        ProximaValue::Int32(x) if *x >= 0 => Some(*x as u64),
        ProximaValue::Int64(x) if *x >= 0 => Some(*x as u64),
        _ => None,
    }
}

/// Any float or integer variant widened to `f64`.
fn pv_as_f64(v: &ProximaValue) -> Option<f64> {
    match v {
        ProximaValue::Float16(x) | ProximaValue::Float32(x) => Some(*x as f64),
        ProximaValue::Float64(x) => Some(*x),
        ProximaValue::Int8(x) => Some(*x as f64),
        ProximaValue::Int16(x) => Some(*x as f64),
        ProximaValue::Int32(x) => Some(*x as f64),
        ProximaValue::Int64(x) => Some(*x as f64),
        ProximaValue::UInt8(x) => Some(*x as f64),
        ProximaValue::UInt16(x) => Some(*x as f64),
        ProximaValue::UInt32(x) => Some(*x as f64),
        ProximaValue::UInt64(x) => Some(*x as f64),
        _ => None,
    }
}

fn pv_as_string(v: &ProximaValue) -> Option<String> {
    match v {
        ProximaValue::String(s) | ProximaValue::Symbol(s) | ProximaValue::Decimal(s) => {
            Some(s.clone())
        }
        ProximaValue::Json(j) | ProximaValue::Jsonb(j) => Some(j.to_string()),
        ProximaValue::Uuid(b) | ProximaValue::ULID(b) => Some(format_uuid(b)),
        _ => None,
    }
}

fn pv_as_binary(v: &ProximaValue) -> Option<&[u8]> {
    match v {
        ProximaValue::Binary(b) | ProximaValue::BinaryVector(b) => Some(b.as_slice()),
        _ => None,
    }
}

fn pv_as_date32(v: &ProximaValue) -> Option<i32> {
    match v {
        ProximaValue::Date(d) => Some(*d),
        ProximaValue::Int32(d) => Some(*d),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Schema inference (for schema-less write paths, e.g. ObjectStoreBridge)
// ---------------------------------------------------------------------------

/// Column name synthesized for the dense-vector column when inferring a schema.
///
/// [`proxima_records_to_record_batch`] reads the vector from `record.embeddings.first()`
/// regardless of the column's *name*, so the exact name is arbitrary — this constant just
/// keeps it stable across a write→read round-trip.
pub const INFERRED_VECTOR_COLUMN: &str = "vector";

/// Map a concrete [`ProximaValue`] to the [`ProximaDataType`] the canonical converter can
/// materialize. Returns `None` for `Null` and for variants without a scalar Arrow column in
/// [`build_column`] (Decimal — needs precision/scale; Time/Timestamp; Array/Map/Struct;
/// sparse/quantized vectors) so inference never produces a column the converter would reject.
fn proxima_value_to_data_type(v: &ProximaValue) -> Option<ProximaDataType> {
    Some(match v {
        ProximaValue::Boolean(_) => ProximaDataType::Boolean,
        ProximaValue::Int8(_) => ProximaDataType::Int8,
        ProximaValue::Int16(_) => ProximaDataType::Int16,
        ProximaValue::Int32(_) => ProximaDataType::Int32,
        ProximaValue::Int64(_) => ProximaDataType::Int64,
        ProximaValue::UInt8(_) => ProximaDataType::UInt8,
        ProximaValue::UInt16(_) => ProximaDataType::UInt16,
        ProximaValue::UInt32(_) => ProximaDataType::UInt32,
        ProximaValue::UInt64(_) => ProximaDataType::UInt64,
        ProximaValue::Float16(_) | ProximaValue::Float32(_) => ProximaDataType::Float32,
        ProximaValue::Float64(_) => ProximaDataType::Float64,
        ProximaValue::String(_) | ProximaValue::Symbol(_) => ProximaDataType::String,
        ProximaValue::Binary(_) => ProximaDataType::Binary,
        ProximaValue::Date(_) => ProximaDataType::Date,
        ProximaValue::Uuid(_) | ProximaValue::ULID(_) => ProximaDataType::Uuid,
        ProximaValue::Json(_) | ProximaValue::Jsonb(_) => ProximaDataType::Json,
        _ => return None,
    })
}

/// Best-effort [`ProximaSchema`] inference from a batch of records — for schema-less write
/// paths such as `ObjectStoreBridge::write_records_to_parquet`, which take only `&[ProximaRecord]`
/// (the records are self-describing).
///
/// Rules (conservative — emits only column types [`proxima_records_to_record_batch`] can
/// materialize, so inference + conversion never disagree):
/// - Scalar columns are the union of top-level `props` **leaf** keys, in first-seen order; a
///   column's type is taken from the first non-`Null`, materializable [`ProximaValue`] observed
///   for that key. Keys that are only ever `Null`/complex/unsupported are skipped (documented
///   loss). Nested `Object` props are not flattened in v1.
/// - Every column is nullable (a key absent from some record is a null there).
/// - If any record carries an embedding, one dense-vector column named [`INFERRED_VECTOR_COLUMN`]
///   is appended, its dimension = the first embedding's length, element type `Float32`.
pub fn infer_proxima_schema(records: &[ProximaRecord]) -> ProximaSchema {
    let mut order: Vec<String> = Vec::new();
    let mut types: HashMap<String, ProximaDataType> = HashMap::new();

    for r in records {
        for (key, node) in &r.props {
            if types.contains_key(key) {
                continue;
            }
            if let ProximaTreeNode::Value(v) = node {
                if let Some(dt) = proxima_value_to_data_type(v) {
                    order.push(key.clone());
                    types.insert(key.clone(), dt);
                }
                // Null / unsupported leaf: leave unlocked so a later record's concrete value
                // for the same key can still define the column.
            }
        }
    }

    let mut columns: Vec<ProximaColumn> = Vec::with_capacity(order.len() + 1);
    let mut next_id = 1;
    let mut push = |name: String, data_type: ProximaDataType, id: &mut i32| {
        columns.push(ProximaColumn {
            id: *id,
            name,
            data_type,
            nullable: true,
            default_value: None,
            comment: None,
            metadata: HashMap::new(),
            is_deleted: false,
            original_id: None,
        });
        *id += 1;
    };

    for name in order {
        let dt = types.remove(&name).expect("type recorded for ordered key");
        push(name, dt, &mut next_id);
    }

    if let Some(dim) = records
        .iter()
        .find_map(|r| r.embeddings.first().map(|c| c.values.len()))
    {
        push(
            INFERRED_VECTOR_COLUMN.to_string(),
            ProximaDataType::Vector {
                dimension: dim as u32,
                element_type: VectorElementType::Float32,
            },
            &mut next_id,
        );
    }

    ProximaSchema::new("inferred".to_string(), columns, Vec::new())
}

/// Render a 16-byte UUID/ULID as the canonical hyphenated hex form.
fn format_uuid(b: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{
        Array, BooleanArray, FixedSizeBinaryArray, Float64Array, Int64Array, StringArray,
    };
    use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaTreeNode};
    use std::collections::HashMap;

    use crate::proxima_schema::VectorElementType;

    fn col(id: i32, name: &str, dt: ProximaDataType, nullable: bool) -> ProximaColumn {
        ProximaColumn {
            id,
            name: name.to_string(),
            data_type: dt,
            nullable,
            default_value: None,
            comment: None,
            metadata: HashMap::new(),
            is_deleted: false,
            original_id: None,
        }
    }

    fn leaf(v: ProximaValue) -> ProximaTreeNode {
        ProximaTreeNode::Value(v)
    }

    fn record_with_props(oid: &str, props: Vec<(&str, ProximaValue)>) -> ProximaRecord {
        let mut r = ProximaRecord {
            oid: oid.to_string(),
            ..Default::default()
        };
        for (k, v) in props {
            r.props.insert(k.to_string(), leaf(v));
        }
        r
    }

    #[test]
    fn typed_columns_with_null_masking() {
        // Schema: name:Utf8, age:Int64, score:Float64, active:Boolean.
        let schema = ProximaSchema::new(
            "test".to_string(),
            vec![
                col(1, "name", ProximaDataType::String, true),
                col(2, "age", ProximaDataType::Int64, true),
                col(3, "score", ProximaDataType::Float64, true),
                col(4, "active", ProximaDataType::Boolean, true),
            ],
            vec![1],
        );

        // Row 0: all present. Row 1: `age` and `active` ABSENT (must become Arrow null).
        let records = vec![
            record_with_props(
                "r0",
                vec![
                    ("name", ProximaValue::String("alice".into())),
                    ("age", ProximaValue::Int64(30)),
                    ("score", ProximaValue::Float64(9.5)),
                    ("active", ProximaValue::Boolean(true)),
                ],
            ),
            record_with_props(
                "r1",
                vec![
                    ("name", ProximaValue::String("bob".into())),
                    ("score", ProximaValue::Float64(1.25)),
                ],
            ),
        ];

        let batch = proxima_records_to_record_batch(&records, &schema).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 4);

        let names = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "alice");
        assert_eq!(names.value(1), "bob");

        let ages = batch
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ages.value(0), 30);
        assert!(ages.is_null(1), "absent prop must be a null, not 0");

        let scores = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(scores.value(0), 9.5);
        assert_eq!(scores.value(1), 1.25);

        let active = batch
            .column(3)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        assert!(active.value(0));
        assert!(active.is_null(1), "absent bool must be a null");
    }

    #[test]
    fn vector_column_encodes_fp32_le_and_nulls() {
        let dim = 3u32;
        let schema = ProximaSchema::new(
            "vec".to_string(),
            vec![
                col(1, "id", ProximaDataType::String, false),
                col(
                    2,
                    "embedding",
                    ProximaDataType::Vector {
                        dimension: dim,
                        element_type: VectorElementType::Float32,
                    },
                    true,
                ),
            ],
            vec![1],
        );

        let mut r0 = record_with_props("r0", vec![("id", ProximaValue::String("r0".into()))]);
        r0.embeddings.push(EmbeddingCell {
            values: EmbeddingValues::Fp32(vec![1.0, 2.0, 3.0]),
            ..Default::default()
        });
        // r1 has NO embedding -> null vector row.
        let r1 = record_with_props("r1", vec![("id", ProximaValue::String("r1".into()))]);

        let batch = proxima_records_to_record_batch(&[r0, r1], &schema).unwrap();
        let vecs = batch
            .column(1)
            .as_any()
            .downcast_ref::<FixedSizeBinaryArray>()
            .unwrap();
        assert_eq!(vecs.value_length(), (dim * 4) as i32);

        let raw = vecs.value(0);
        let decoded: Vec<f32> = raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(decoded, vec![1.0, 2.0, 3.0]);
        assert!(vecs.is_null(1), "record without embedding must be null");
    }

    #[test]
    fn vector_dimension_mismatch_is_error() {
        let schema = ProximaSchema::new(
            "vec".to_string(),
            vec![col(
                1,
                "embedding",
                ProximaDataType::Vector {
                    dimension: 4,
                    element_type: VectorElementType::Float32,
                },
                true,
            )],
            vec![],
        );
        let mut r0 = ProximaRecord {
            oid: "r0".into(),
            ..Default::default()
        };
        r0.embeddings.push(EmbeddingCell {
            values: EmbeddingValues::Fp32(vec![1.0, 2.0, 3.0]), // 3 != 4
            ..Default::default()
        });
        let err = proxima_records_to_record_batch(&[r0], &schema).unwrap_err();
        assert!(matches!(err, StorageError::InvalidDimension { .. }));
    }

    #[test]
    fn integer_widening_coercion() {
        // Column is Int64 but the prop is an Int32 — widening must succeed, not null out.
        let schema = ProximaSchema::new(
            "coerce".to_string(),
            vec![col(1, "n", ProximaDataType::Int64, true)],
            vec![],
        );
        let r = record_with_props("r0", vec![("n", ProximaValue::Int32(7))]);
        let batch = proxima_records_to_record_batch(&[r], &schema).unwrap();
        let ns = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ns.value(0), 7);
    }

    #[test]
    fn infer_schema_unions_props_and_appends_vector() {
        // r0 defines name(String)+age(Int64) and an embedding; r1 adds score(Float64) and a
        // value `tags` that is only ever Null in r0 -> tags must NOT become a column from r0,
        // but r1 gives it a concrete String -> it should appear, after the first-seen columns.
        let mut r0 = record_with_props(
            "r0",
            vec![
                ("name", ProximaValue::String("a".into())),
                ("age", ProximaValue::Int64(1)),
                ("tags", ProximaValue::Null),
            ],
        );
        r0.embeddings.push(EmbeddingCell {
            values: EmbeddingValues::Fp32(vec![0.0, 1.0]),
            ..Default::default()
        });
        let r1 = record_with_props(
            "r1",
            vec![
                ("name", ProximaValue::String("b".into())),
                ("score", ProximaValue::Float64(2.0)),
                ("tags", ProximaValue::String("x".into())),
            ],
        );

        let schema = infer_proxima_schema(&[r0, r1]);
        let by_name: HashMap<&str, &ProximaColumn> = schema
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();

        assert_eq!(by_name["name"].data_type, ProximaDataType::String);
        assert_eq!(by_name["age"].data_type, ProximaDataType::Int64);
        assert_eq!(by_name["score"].data_type, ProximaDataType::Float64);
        assert_eq!(by_name["tags"].data_type, ProximaDataType::String);
        assert!(by_name.values().all(|c| c.nullable));

        let vec_col = by_name[INFERRED_VECTOR_COLUMN];
        assert_eq!(
            vec_col.data_type,
            ProximaDataType::Vector {
                dimension: 2,
                element_type: VectorElementType::Float32
            }
        );

        // The inferred schema must round-trip the records through the canonical converter.
        let r0b = {
            let mut r = record_with_props("r0", vec![("name", ProximaValue::String("a".into()))]);
            r.embeddings.push(EmbeddingCell {
                values: EmbeddingValues::Fp32(vec![0.0, 1.0]),
                ..Default::default()
            });
            r
        };
        let batch = proxima_records_to_record_batch(&[r0b], &schema).unwrap();
        assert_eq!(batch.num_columns(), schema.columns.len());
    }

    #[test]
    fn infer_schema_skips_only_null_or_complex_columns() {
        // `blob` is an Array (unsupported) and `nothing` is always Null -> neither becomes a column.
        let r = record_with_props(
            "r0",
            vec![
                ("ok", ProximaValue::Int64(1)),
                ("blob", ProximaValue::Array(vec![ProximaValue::Int64(1)])),
                ("nothing", ProximaValue::Null),
            ],
        );
        let schema = infer_proxima_schema(&[r]);
        let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["ok"]);
    }
}
