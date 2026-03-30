//! # Type Mapping - ProximaDB ↔ Arrow ↔ Spark ↔ Trino ↔ Hive
//!
//! Provides type conversion utilities for compute engine compatibility.
//! Also includes type coercion rules for query execution.

use super::proxima_schema::ProximaDataType;

/// Type mapping between ProximaDB, Arrow, Proto, and external engines.
pub struct TypeMapper;

impl TypeMapper {
    // ========================================================================
    // ProximaDB <-> Proto Mapping
    // ========================================================================

    /// ProximaDB to Proto DataType enum value.
    pub fn proxima_to_proto_i32(dt: &ProximaDataType) -> i32 {
        match dt {
            ProximaDataType::Boolean => 1,
            ProximaDataType::Int8 => 2,
            ProximaDataType::Int16 => 3,
            ProximaDataType::Int32 => 4,
            ProximaDataType::Int64 => 5,
            ProximaDataType::UInt8 => 6,
            ProximaDataType::UInt16 => 7,
            ProximaDataType::UInt32 => 8,
            ProximaDataType::UInt64 => 9,
            ProximaDataType::Float32 => 10,
            ProximaDataType::Float64 => 11,
            ProximaDataType::Decimal { .. } => 12,
            ProximaDataType::String => 13,
            ProximaDataType::Binary => 14,
            ProximaDataType::Date => 15,
            ProximaDataType::Time { .. } => 16,
            ProximaDataType::Timestamp { .. } => 17,
            ProximaDataType::Uuid => 18,
            ProximaDataType::List { .. } => 20,
            ProximaDataType::Map { .. } => 21,
            ProximaDataType::Struct { .. } => 22,
            ProximaDataType::Json => 23,
            ProximaDataType::Vector { .. } => 30,
            ProximaDataType::SparseVector { .. } => 31,
            ProximaDataType::BinaryVector { .. } => 32,
            ProximaDataType::QuantizedInt8Vector { .. } => 33,
            ProximaDataType::QuantizedPQVector { .. } => 34,
            ProximaDataType::QuantizedBinaryVector { .. } => 35,
        }
    }

    /// Proto DataType to ProximaDB.
    pub fn proto_i32_to_proxima(value: i32, dimension: Option<u32>) -> ProximaDataType {
        match value {
            1 => ProximaDataType::Boolean,
            2 => ProximaDataType::Int8,
            3 => ProximaDataType::Int16,
            4 => ProximaDataType::Int32,
            5 => ProximaDataType::Int64,
            6 => ProximaDataType::UInt8,
            7 => ProximaDataType::UInt16,
            8 => ProximaDataType::UInt32,
            9 => ProximaDataType::UInt64,
            10 => ProximaDataType::Float32,
            11 => ProximaDataType::Float64,
            12 => ProximaDataType::Decimal {
                precision: 38,
                scale: 10,
            },
            13 => ProximaDataType::String,
            14 => ProximaDataType::Binary,
            15 => ProximaDataType::Date,
            16 => ProximaDataType::Time {
                unit: super::proxima_schema::TimeUnit::Nanosecond,
            },
            17 => ProximaDataType::Timestamp {
                unit: super::proxima_schema::TimeUnit::Nanosecond,
                timezone: None,
            },
            18 => ProximaDataType::Uuid,
            23 => ProximaDataType::Json,
            30 => ProximaDataType::Vector {
                dimension: dimension.unwrap_or(0),
                element_type: super::proxima_schema::VectorElementType::Float32,
            },
            31 => ProximaDataType::SparseVector {
                max_dimension: None,
            },
            32 => ProximaDataType::BinaryVector {
                dimension: dimension.unwrap_or(0),
            },
            _ => ProximaDataType::String,
        }
    }

    // ========================================================================
    // ProximaDB <-> Spark SQL Types
    // ========================================================================

    /// ProximaDB to Spark SQL type string.
    pub fn proxima_to_spark_sql(dt: &ProximaDataType) -> String {
        match dt {
            ProximaDataType::Boolean => "BOOLEAN".to_string(),
            ProximaDataType::Int8 => "TINYINT".to_string(),
            ProximaDataType::Int16 => "SMALLINT".to_string(),
            ProximaDataType::Int32 => "INT".to_string(),
            ProximaDataType::Int64 => "BIGINT".to_string(),
            ProximaDataType::UInt8 => "SMALLINT".to_string(), // Spark has no unsigned
            ProximaDataType::UInt16 => "INT".to_string(),
            ProximaDataType::UInt32 => "BIGINT".to_string(),
            ProximaDataType::UInt64 => "DECIMAL(20,0)".to_string(),
            ProximaDataType::Float32 => "FLOAT".to_string(),
            ProximaDataType::Float64 => "DOUBLE".to_string(),
            ProximaDataType::Decimal { precision, scale } => {
                format!("DECIMAL({}, {})", precision, scale)
            }
            ProximaDataType::String => "STRING".to_string(),
            ProximaDataType::Binary => "BINARY".to_string(),
            ProximaDataType::Date => "DATE".to_string(),
            ProximaDataType::Time { .. } => "STRING".to_string(), // Spark doesn't have TIME
            ProximaDataType::Timestamp { timezone, .. } => {
                if timezone.is_some() {
                    "TIMESTAMP".to_string()
                } else {
                    "TIMESTAMP_NTZ".to_string()
                }
            }
            ProximaDataType::Uuid => "STRING".to_string(),
            ProximaDataType::Json => "STRING".to_string(),
            ProximaDataType::List { element } => {
                format!("ARRAY<{}>", Self::proxima_to_spark_sql(element))
            }
            ProximaDataType::Map { key, value } => {
                format!(
                    "MAP<{}, {}>",
                    Self::proxima_to_spark_sql(key),
                    Self::proxima_to_spark_sql(value)
                )
            }
            ProximaDataType::Struct { fields } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, Self::proxima_to_spark_sql(&f.data_type)))
                    .collect();
                format!("STRUCT<{}>", field_strs.join(", "))
            }
            ProximaDataType::Vector { .. } => "ARRAY<FLOAT>".to_string(),
            ProximaDataType::SparseVector { .. } => "MAP<INT, FLOAT>".to_string(),
            ProximaDataType::BinaryVector { .. } => "BINARY".to_string(),
            ProximaDataType::QuantizedInt8Vector { .. } => "BINARY".to_string(),
            ProximaDataType::QuantizedPQVector { .. } => "BINARY".to_string(),
            ProximaDataType::QuantizedBinaryVector { .. } => "BINARY".to_string(),
        }
    }

    // ========================================================================
    // ProximaDB <-> Trino SQL Types
    // ========================================================================

    /// ProximaDB to Trino SQL type string.
    pub fn proxima_to_trino_sql(dt: &ProximaDataType) -> String {
        match dt {
            ProximaDataType::Boolean => "BOOLEAN".to_string(),
            ProximaDataType::Int8 => "TINYINT".to_string(),
            ProximaDataType::Int16 => "SMALLINT".to_string(),
            ProximaDataType::Int32 => "INTEGER".to_string(),
            ProximaDataType::Int64 => "BIGINT".to_string(),
            ProximaDataType::UInt8 => "SMALLINT".to_string(),
            ProximaDataType::UInt16 => "INTEGER".to_string(),
            ProximaDataType::UInt32 => "BIGINT".to_string(),
            ProximaDataType::UInt64 => "DECIMAL(20,0)".to_string(),
            ProximaDataType::Float32 => "REAL".to_string(),
            ProximaDataType::Float64 => "DOUBLE".to_string(),
            ProximaDataType::Decimal { precision, scale } => {
                format!("DECIMAL({}, {})", precision, scale)
            }
            ProximaDataType::String => "VARCHAR".to_string(),
            ProximaDataType::Binary => "VARBINARY".to_string(),
            ProximaDataType::Date => "DATE".to_string(),
            ProximaDataType::Time { .. } => "TIME".to_string(),
            ProximaDataType::Timestamp { timezone, .. } => {
                if timezone.is_some() {
                    "TIMESTAMP WITH TIME ZONE".to_string()
                } else {
                    "TIMESTAMP".to_string()
                }
            }
            ProximaDataType::Uuid => "UUID".to_string(),
            ProximaDataType::Json => "JSON".to_string(),
            ProximaDataType::List { element } => {
                format!("ARRAY({})", Self::proxima_to_trino_sql(element))
            }
            ProximaDataType::Map { key, value } => {
                format!(
                    "MAP({}, {})",
                    Self::proxima_to_trino_sql(key),
                    Self::proxima_to_trino_sql(value)
                )
            }
            ProximaDataType::Struct { fields } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|f| format!("{} {}", f.name, Self::proxima_to_trino_sql(&f.data_type)))
                    .collect();
                format!("ROW({})", field_strs.join(", "))
            }
            ProximaDataType::Vector { .. } => "ARRAY(REAL)".to_string(),
            ProximaDataType::SparseVector { .. } => "MAP(INTEGER, REAL)".to_string(),
            ProximaDataType::BinaryVector { .. } => "VARBINARY".to_string(),
            ProximaDataType::QuantizedInt8Vector { .. } => "VARBINARY".to_string(),
            ProximaDataType::QuantizedPQVector { .. } => "VARBINARY".to_string(),
            ProximaDataType::QuantizedBinaryVector { .. } => "VARBINARY".to_string(),
        }
    }

    // ========================================================================
    // ProximaDB <-> Hive Types
    // ========================================================================

    /// ProximaDB to Hive type string.
    pub fn proxima_to_hive(dt: &ProximaDataType) -> String {
        match dt {
            ProximaDataType::Boolean => "boolean".to_string(),
            ProximaDataType::Int8 => "tinyint".to_string(),
            ProximaDataType::Int16 => "smallint".to_string(),
            ProximaDataType::Int32 => "int".to_string(),
            ProximaDataType::Int64 => "bigint".to_string(),
            ProximaDataType::UInt8 => "smallint".to_string(),
            ProximaDataType::UInt16 => "int".to_string(),
            ProximaDataType::UInt32 => "bigint".to_string(),
            ProximaDataType::UInt64 => "decimal(20,0)".to_string(),
            ProximaDataType::Float32 => "float".to_string(),
            ProximaDataType::Float64 => "double".to_string(),
            ProximaDataType::Decimal { precision, scale } => {
                format!("decimal({},{})", precision, scale)
            }
            ProximaDataType::String => "string".to_string(),
            ProximaDataType::Binary => "binary".to_string(),
            ProximaDataType::Date => "date".to_string(),
            ProximaDataType::Time { .. } => "string".to_string(),
            ProximaDataType::Timestamp { .. } => "timestamp".to_string(),
            ProximaDataType::Uuid => "string".to_string(),
            ProximaDataType::Json => "string".to_string(),
            ProximaDataType::List { element } => {
                format!("array<{}>", Self::proxima_to_hive(element))
            }
            ProximaDataType::Map { key, value } => {
                format!(
                    "map<{},{}>",
                    Self::proxima_to_hive(key),
                    Self::proxima_to_hive(value)
                )
            }
            ProximaDataType::Struct { fields } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|f| format!("{}:{}", f.name, Self::proxima_to_hive(&f.data_type)))
                    .collect();
                format!("struct<{}>", field_strs.join(","))
            }
            ProximaDataType::Vector { .. } => "array<float>".to_string(),
            ProximaDataType::SparseVector { .. } => "map<int,float>".to_string(),
            _ => "binary".to_string(),
        }
    }

    // ========================================================================
    // ProximaDB <-> PostgreSQL Types
    // ========================================================================

    /// ProximaDB to PostgreSQL type string.
    pub fn proxima_to_postgres(dt: &ProximaDataType) -> String {
        match dt {
            ProximaDataType::Boolean => "BOOLEAN".to_string(),
            ProximaDataType::Int8 => "SMALLINT".to_string(), // PG has no TINYINT
            ProximaDataType::Int16 => "SMALLINT".to_string(),
            ProximaDataType::Int32 => "INTEGER".to_string(),
            ProximaDataType::Int64 => "BIGINT".to_string(),
            ProximaDataType::UInt8 => "SMALLINT".to_string(),
            ProximaDataType::UInt16 => "INTEGER".to_string(),
            ProximaDataType::UInt32 => "BIGINT".to_string(),
            ProximaDataType::UInt64 => "NUMERIC(20,0)".to_string(),
            ProximaDataType::Float32 => "REAL".to_string(),
            ProximaDataType::Float64 => "DOUBLE PRECISION".to_string(),
            ProximaDataType::Decimal { precision, scale } => {
                format!("NUMERIC({}, {})", precision, scale)
            }
            ProximaDataType::String => "TEXT".to_string(),
            ProximaDataType::Binary => "BYTEA".to_string(),
            ProximaDataType::Date => "DATE".to_string(),
            ProximaDataType::Time { .. } => "TIME".to_string(),
            ProximaDataType::Timestamp { timezone, .. } => {
                if timezone.is_some() {
                    "TIMESTAMPTZ".to_string()
                } else {
                    "TIMESTAMP".to_string()
                }
            }
            ProximaDataType::Uuid => "UUID".to_string(),
            ProximaDataType::Json => "JSONB".to_string(),
            ProximaDataType::List { element } => {
                format!("{}[]", Self::proxima_to_postgres(element))
            }
            ProximaDataType::Map { .. } => "JSONB".to_string(), // PG doesn't have native MAP
            ProximaDataType::Struct { .. } => "JSONB".to_string(), // Use JSONB for structs
            ProximaDataType::Vector { dimension, .. } => {
                format!("vector({})", dimension) // pgvector extension
            }
            ProximaDataType::SparseVector { .. } => "JSONB".to_string(),
            _ => "BYTEA".to_string(),
        }
    }
}

// ============================================================================
// Type Coercion Rules
// ============================================================================

/// Defines type coercion rules for query execution.
pub struct TypeCoercion;

impl TypeCoercion {
    /// Get the common type for two types (for UNION, CASE, etc.)
    pub fn common_type(left: &ProximaDataType, right: &ProximaDataType) -> Option<ProximaDataType> {
        use ProximaDataType::*;

        if left == right {
            return Some(left.clone());
        }

        match (left, right) {
            // Integer promotions
            (Int8, Int16) | (Int16, Int8) => Some(Int16),
            (Int8, Int32) | (Int32, Int8) => Some(Int32),
            (Int8, Int64) | (Int64, Int8) => Some(Int64),
            (Int16, Int32) | (Int32, Int16) => Some(Int32),
            (Int16, Int64) | (Int64, Int16) => Some(Int64),
            (Int32, Int64) | (Int64, Int32) => Some(Int64),

            // Unsigned integer promotions
            (UInt8, UInt16) | (UInt16, UInt8) => Some(UInt16),
            (UInt8, UInt32) | (UInt32, UInt8) => Some(UInt32),
            (UInt8, UInt64) | (UInt64, UInt8) => Some(UInt64),
            (UInt16, UInt32) | (UInt32, UInt16) => Some(UInt32),
            (UInt16, UInt64) | (UInt64, UInt16) => Some(UInt64),
            (UInt32, UInt64) | (UInt64, UInt32) => Some(UInt64),

            // Float promotions
            (Float32, Float64) | (Float64, Float32) => Some(Float64),

            // Int to Float
            (Int8 | Int16 | Int32, Float32) | (Float32, Int8 | Int16 | Int32) => Some(Float32),
            (Int8 | Int16 | Int32 | Int64, Float64) | (Float64, Int8 | Int16 | Int32 | Int64) => {
                Some(Float64)
            }

            // String absorbs everything
            (String, _) | (_, String) => Some(String),

            _ => None,
        }
    }

    /// Check if implicit cast is allowed.
    pub fn can_implicit_cast(from: &ProximaDataType, to: &ProximaDataType) -> bool {
        if from == to {
            return true;
        }

        Self::common_type(from, to)
            .is_some_and(|t| &t == to)
    }

    /// Get cast expression for SQL.
    pub fn cast_expression(from: &ProximaDataType, to: &ProximaDataType) -> Option<String> {
        if from == to {
            return None;
        }

        // Direct numeric casts
        match (from, to) {
            (
                ProximaDataType::Int8 | ProximaDataType::Int16 | ProximaDataType::Int32,
                ProximaDataType::Int64,
            ) => Some("CAST(? AS BIGINT)".to_string()),
            (ProximaDataType::Float32, ProximaDataType::Float64) => {
                Some("CAST(? AS DOUBLE)".to_string())
            }
            (_, ProximaDataType::String) => Some("CAST(? AS VARCHAR)".to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spark_type_mapping() {
        assert_eq!(
            TypeMapper::proxima_to_spark_sql(&ProximaDataType::Int64),
            "BIGINT"
        );
        assert_eq!(
            TypeMapper::proxima_to_spark_sql(&ProximaDataType::Vector {
                dimension: 512,
                element_type: super::super::proxima_schema::VectorElementType::Float32
            }),
            "ARRAY<FLOAT>"
        );
    }

    #[test]
    fn test_trino_type_mapping() {
        assert_eq!(
            TypeMapper::proxima_to_trino_sql(&ProximaDataType::String),
            "VARCHAR"
        );
        assert_eq!(
            TypeMapper::proxima_to_trino_sql(&ProximaDataType::Json),
            "JSON"
        );
    }

    #[test]
    fn test_postgres_type_mapping() {
        assert_eq!(
            TypeMapper::proxima_to_postgres(&ProximaDataType::Uuid),
            "UUID"
        );
        assert_eq!(
            TypeMapper::proxima_to_postgres(&ProximaDataType::Vector {
                dimension: 1536,
                element_type: super::super::proxima_schema::VectorElementType::Float32
            }),
            "vector(1536)"
        );
    }

    #[test]
    fn test_type_coercion() {
        assert_eq!(
            TypeCoercion::common_type(&ProximaDataType::Int32, &ProximaDataType::Int64),
            Some(ProximaDataType::Int64)
        );

        assert!(TypeCoercion::can_implicit_cast(
            &ProximaDataType::Int32,
            &ProximaDataType::Int64
        ));

        assert!(!TypeCoercion::can_implicit_cast(
            &ProximaDataType::Int64,
            &ProximaDataType::Int32
        ));
    }
}
