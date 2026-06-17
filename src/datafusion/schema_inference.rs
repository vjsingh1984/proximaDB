//! # Schema Inference for ProximaDB Collections
//!
//! Extracts Arrow schemas and statistics from ProximaDB collections
//! for DataFusion query optimization.

use std::sync::Arc;

use datafusion::arrow::datatypes::{
    DataType as ArrowDataType, Field, Schema as ArrowSchema, TimeUnit as ArrowTimeUnit,
};
use datafusion::common::{ColumnStatistics, Statistics};
use datafusion::error::Result as DFResult;

use crate::storage::formats::FormatStatistics;
use proximadb_data_model::{ProximaType, TimeUnit as DmTimeUnit, VectorElement};

use crate::storage::schema::{ProximaColumn, ProximaSchema};

/// Infer Arrow schema from a ProximaDB collection.
///
/// This uses the collection's ProximaSchema to generate a compatible Arrow schema.
pub fn infer_schema_from_collection(proxima_schema: &ProximaSchema) -> DFResult<Arc<ArrowSchema>> {
    // to_arrow_schema() already returns Arc<ArrowSchema>
    Ok(proxima_schema.to_arrow_schema())
}

/// Extract DataFusion Statistics from ProximaDB format statistics.
pub fn extract_statistics(format_stats: &FormatStatistics, schema: &ArrowSchema) -> Statistics {
    let num_rows = if format_stats.row_count > 0 {
        datafusion_common::stats::Precision::Exact(format_stats.row_count as usize)
    } else {
        datafusion_common::stats::Precision::Absent
    };

    let total_byte_size = if format_stats.size_bytes > 0 {
        datafusion_common::stats::Precision::Exact(format_stats.size_bytes as usize)
    } else {
        datafusion_common::stats::Precision::Absent
    };

    // Create column statistics for each field
    let column_statistics: Vec<ColumnStatistics> = schema
        .fields()
        .iter()
        .map(|field| {
            // Try to get column stats from format_stats
            if let Some(col_stats) = format_stats.column_stats.get(field.name()) {
                // Build column statistics from stored values
                let mut cs = ColumnStatistics::new_unknown();
                // null_count is u64 (not Option)
                cs.null_count =
                    datafusion_common::stats::Precision::Exact(col_stats.null_count as usize);
                // distinct_count is Option<u64>
                if let Some(distinct) = col_stats.distinct_count {
                    cs.distinct_count =
                        datafusion_common::stats::Precision::Exact(distinct as usize);
                }
                cs
            } else {
                ColumnStatistics::new_unknown()
            }
        })
        .collect();

    Statistics {
        num_rows,
        total_byte_size,
        column_statistics,
    }
}

/// Convert ProximaType to Arrow DataType.
pub fn proxima_to_arrow_type(proxima_type: &ProximaType) -> ArrowDataType {
    match proxima_type {
        ProximaType::Boolean => ArrowDataType::Boolean,
        ProximaType::Int8 => ArrowDataType::Int8,
        ProximaType::Int16 => ArrowDataType::Int16,
        ProximaType::Int32 => ArrowDataType::Int32,
        ProximaType::Int64 => ArrowDataType::Int64,
        ProximaType::UInt8 => ArrowDataType::UInt8,
        ProximaType::UInt16 => ArrowDataType::UInt16,
        ProximaType::UInt32 => ArrowDataType::UInt32,
        ProximaType::UInt64 => ArrowDataType::UInt64,
        ProximaType::Float16 => ArrowDataType::Float16,
        ProximaType::Float32 => ArrowDataType::Float32,
        ProximaType::Float64 => ArrowDataType::Float64,
        ProximaType::Decimal { precision, scale } => {
            ArrowDataType::Decimal128(*precision, *scale as i8)
        }
        ProximaType::String | ProximaType::Symbol => ArrowDataType::Utf8,
        ProximaType::Binary => ArrowDataType::Binary,
        ProximaType::Date => ArrowDataType::Date32,
        ProximaType::Time(unit) => ArrowDataType::Time64(time_unit_to_arrow(*unit)),
        ProximaType::Timestamp(unit) => ArrowDataType::Timestamp(time_unit_to_arrow(*unit), None),
        ProximaType::TimestampTz(unit) => {
            ArrowDataType::Timestamp(time_unit_to_arrow(*unit), Some(Arc::from("UTC")))
        }
        ProximaType::Uuid | ProximaType::ULID => ArrowDataType::FixedSizeBinary(16),
        ProximaType::Json | ProximaType::Jsonb => ArrowDataType::Utf8, // JSON as string
        ProximaType::Array(element) => {
            let element_type = proxima_to_arrow_type(element);
            ArrowDataType::List(Arc::new(Field::new("item", element_type, true)))
        }
        ProximaType::Map { key, value } => {
            let key_type = proxima_to_arrow_type(key);
            let value_type = proxima_to_arrow_type(value);
            let struct_field = ArrowDataType::Struct(
                vec![
                    Field::new("key", key_type, false),
                    Field::new("value", value_type, true),
                ]
                .into(),
            );
            ArrowDataType::Map(Arc::new(Field::new("entries", struct_field, false)), false)
        }
        ProximaType::Struct { fields } => {
            let arrow_fields: Vec<Field> = fields
                .iter()
                .map(|(name, ty)| Field::new(name, proxima_to_arrow_type(ty), true))
                .collect();
            ArrowDataType::Struct(arrow_fields.into())
        }

        // Vector types
        ProximaType::DenseVector { element, dim } => {
            let elem_arrow = match element {
                VectorElement::Float32 => ArrowDataType::Float32,
                VectorElement::Float64 => ArrowDataType::Float64,
                VectorElement::Float16 => ArrowDataType::Float16,
                VectorElement::BFloat16 => ArrowDataType::Float32, // BFloat16 approximated as Float32
                VectorElement::Int8 => ArrowDataType::Int8,
            };
            ArrowDataType::FixedSizeList(
                Arc::new(Field::new("value", elem_arrow, true)),
                *dim as i32,
            )
        }
        ProximaType::SparseVector { .. } => {
            // Sparse vector as struct of indices and values
            ArrowDataType::Struct(
                vec![
                    Field::new(
                        "indices",
                        ArrowDataType::List(Arc::new(Field::new(
                            "item",
                            ArrowDataType::UInt32,
                            false,
                        ))),
                        false,
                    ),
                    Field::new(
                        "values",
                        ArrowDataType::List(Arc::new(Field::new(
                            "item",
                            ArrowDataType::Float32,
                            false,
                        ))),
                        false,
                    ),
                ]
                .into(),
            )
        }
        ProximaType::BinaryVector { dim } => {
            // Binary vector as fixed-size binary
            ArrowDataType::FixedSizeBinary(dim.div_ceil(8) as i32)
        }
        // Interval/Duration/geo/null have no distinct DataFusion projection here.
        ProximaType::Interval(_) => {
            ArrowDataType::Interval(datafusion::arrow::datatypes::IntervalUnit::MonthDayNano)
        }
        ProximaType::Duration(unit) => ArrowDataType::Duration(time_unit_to_arrow(*unit)),
        ProximaType::Point | ProximaType::GeographyPoint => ArrowDataType::FixedSizeBinary(16),
        ProximaType::Null => ArrowDataType::Null,
    }
}

/// Convert the canonical [`DmTimeUnit`] to Arrow's time unit.
fn time_unit_to_arrow(unit: DmTimeUnit) -> ArrowTimeUnit {
    match unit {
        DmTimeUnit::Second => ArrowTimeUnit::Second,
        DmTimeUnit::Millisecond => ArrowTimeUnit::Millisecond,
        DmTimeUnit::Microsecond => ArrowTimeUnit::Microsecond,
        DmTimeUnit::Nanosecond => ArrowTimeUnit::Nanosecond,
    }
}

/// Convert ProximaColumn to Arrow Field.
pub fn proxima_column_to_arrow_field(column: &ProximaColumn) -> Field {
    let arrow_type = proxima_to_arrow_type(&column.data_type);
    Field::new(&column.name, arrow_type, column.nullable)
}

/// Merge multiple schemas (e.g., from different files).
pub fn merge_schemas(schemas: &[&ArrowSchema]) -> DFResult<ArrowSchema> {
    if schemas.is_empty() {
        return Ok(ArrowSchema::empty());
    }

    if schemas.len() == 1 {
        return Ok(schemas[0].clone());
    }

    // Start with first schema
    let mut merged_fields = schemas[0].fields().to_vec();

    // Merge fields from other schemas
    for schema in schemas.iter().skip(1) {
        for field in schema.fields() {
            if !merged_fields.iter().any(|f| f.name() == field.name()) {
                merged_fields.push(field.clone());
            }
        }
    }

    Ok(ArrowSchema::new(merged_fields))
}

/// Estimate statistics from file metadata.
pub fn estimate_statistics_from_files(
    file_sizes: &[u64],
    row_counts: &[Option<u64>],
) -> Statistics {
    let total_bytes: u64 = file_sizes.iter().sum();
    let total_rows: Option<u64> = row_counts
        .iter()
        .try_fold(0u64, |acc, count| count.map(|c| acc + c));

    let num_rows = match total_rows {
        Some(rows) => datafusion_common::stats::Precision::Exact(rows as usize),
        None => datafusion_common::stats::Precision::Absent,
    };

    Statistics {
        num_rows,
        total_byte_size: datafusion_common::stats::Precision::Exact(total_bytes as usize),
        column_statistics: vec![],
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_schema_from_collection() {
        let schema = ProximaSchema::vector_record_schema(768);
        let arrow_schema = infer_schema_from_collection(&schema).unwrap();

        assert!(arrow_schema.field_with_name("id").is_ok());
        assert!(arrow_schema.field_with_name("vector").is_ok());
    }

    #[test]
    fn test_proxima_to_arrow_type() {
        assert_eq!(
            proxima_to_arrow_type(&ProximaType::Int64),
            ArrowDataType::Int64
        );
        assert_eq!(
            proxima_to_arrow_type(&ProximaType::String),
            ArrowDataType::Utf8
        );
        assert_eq!(
            proxima_to_arrow_type(&ProximaType::Boolean),
            ArrowDataType::Boolean
        );
    }

    #[test]
    fn test_vector_type_conversion() {
        let vector_type = ProximaType::DenseVector {
            element: VectorElement::Float32,
            dim: 128,
        };

        let arrow_type = proxima_to_arrow_type(&vector_type);

        match arrow_type {
            ArrowDataType::FixedSizeList(_, size) => {
                assert_eq!(size, 128);
            }
            _ => panic!("Expected FixedSizeList"),
        }
    }
}
