//! # Type Mapping - ProximaDB ↔ Arrow ↔ Spark ↔ Trino ↔ Hive
//!
//! Provides type conversion utilities for compute engine compatibility.
//! Also includes type coercion rules for query execution.

use proximadb_data_model::ProximaType;

/// Type mapping between ProximaDB, Arrow, Proto, and external engines.
pub struct TypeMapper;

impl TypeMapper {
    // ========================================================================
    // ProximaDB <-> Proto Mapping
    // ========================================================================

    /// ProximaDB to Proto DataType enum value.
    pub fn proxima_to_proto_i32(dt: &ProximaType) -> i32 {
        match dt {
            ProximaType::Boolean => 1,
            ProximaType::Int8 => 2,
            ProximaType::Int16 => 3,
            ProximaType::Int32 => 4,
            ProximaType::Int64 => 5,
            ProximaType::UInt8 => 6,
            ProximaType::UInt16 => 7,
            ProximaType::UInt32 => 8,
            ProximaType::UInt64 => 9,
            ProximaType::Float16 | ProximaType::Float32 => 10,
            ProximaType::Float64 => 11,
            ProximaType::Decimal { .. } => 12,
            ProximaType::String | ProximaType::Symbol => 13,
            ProximaType::Binary => 14,
            ProximaType::Date => 15,
            ProximaType::Time(_) => 16,
            ProximaType::Timestamp(_)
            | ProximaType::TimestampTz(_)
            | ProximaType::Interval(_)
            | ProximaType::Duration(_) => 17,
            ProximaType::Uuid | ProximaType::ULID => 18,
            ProximaType::Array(_) => 20,
            ProximaType::Map { .. } => 21,
            ProximaType::Struct { .. } => 22,
            ProximaType::Json | ProximaType::Jsonb => 23,
            ProximaType::DenseVector { .. } => 30,
            ProximaType::SparseVector { .. } => 31,
            ProximaType::BinaryVector { .. } => 32,
            // Geo / null types have no proto code yet → string fallback.
            ProximaType::Point | ProximaType::GeographyPoint | ProximaType::Null => 13,
        }
    }

    /// Proto DataType to ProximaDB.
    pub fn proto_i32_to_proxima(value: i32, dimension: Option<u32>) -> ProximaType {
        match value {
            1 => ProximaType::Boolean,
            2 => ProximaType::Int8,
            3 => ProximaType::Int16,
            4 => ProximaType::Int32,
            5 => ProximaType::Int64,
            6 => ProximaType::UInt8,
            7 => ProximaType::UInt16,
            8 => ProximaType::UInt32,
            9 => ProximaType::UInt64,
            10 => ProximaType::Float32,
            11 => ProximaType::Float64,
            12 => ProximaType::Decimal {
                precision: 38,
                scale: 10,
            },
            13 => ProximaType::String,
            14 => ProximaType::Binary,
            15 => ProximaType::Date,
            16 => ProximaType::Time(proximadb_data_model::TimeUnit::Nanosecond),
            17 => ProximaType::Timestamp(proximadb_data_model::TimeUnit::Nanosecond),
            18 => ProximaType::Uuid,
            23 => ProximaType::Json,
            30 => ProximaType::DenseVector {
                element: proximadb_data_model::VectorElement::Float32,
                dim: dimension.unwrap_or(0) as usize,
            },
            31 => ProximaType::SparseVector {
                element: proximadb_data_model::VectorElement::Float32,
            },
            32 => ProximaType::BinaryVector {
                dim: dimension.unwrap_or(0) as usize,
            },
            _ => ProximaType::String,
        }
    }

    // ========================================================================
    // ProximaDB <-> Spark SQL Types
    // ========================================================================

    /// ProximaDB to Spark SQL type string.
    pub fn proxima_to_spark_sql(dt: &ProximaType) -> String {
        match dt {
            ProximaType::Boolean => "BOOLEAN".to_string(),
            ProximaType::Int8 => "TINYINT".to_string(),
            ProximaType::Int16 => "SMALLINT".to_string(),
            ProximaType::Int32 => "INT".to_string(),
            ProximaType::Int64 => "BIGINT".to_string(),
            ProximaType::UInt8 => "SMALLINT".to_string(), // Spark has no unsigned
            ProximaType::UInt16 => "INT".to_string(),
            ProximaType::UInt32 => "BIGINT".to_string(),
            ProximaType::UInt64 => "DECIMAL(20,0)".to_string(),
            ProximaType::Float16 | ProximaType::Float32 => "FLOAT".to_string(),
            ProximaType::Float64 => "DOUBLE".to_string(),
            ProximaType::Decimal { precision, scale } => {
                format!("DECIMAL({}, {})", precision, scale)
            }
            ProximaType::String | ProximaType::Symbol => "STRING".to_string(),
            ProximaType::Binary => "BINARY".to_string(),
            ProximaType::Date => "DATE".to_string(),
            ProximaType::Time(_) => "STRING".to_string(), // Spark doesn't have TIME
            ProximaType::Timestamp(_) => "TIMESTAMP_NTZ".to_string(),
            ProximaType::TimestampTz(_) => "TIMESTAMP".to_string(),
            ProximaType::Interval(_) | ProximaType::Duration(_) => "INTERVAL".to_string(),
            ProximaType::Uuid | ProximaType::ULID => "STRING".to_string(),
            ProximaType::Json | ProximaType::Jsonb => "STRING".to_string(),
            ProximaType::Array(element) => {
                format!("ARRAY<{}>", Self::proxima_to_spark_sql(element))
            }
            ProximaType::Map { key, value } => {
                format!(
                    "MAP<{}, {}>",
                    Self::proxima_to_spark_sql(key),
                    Self::proxima_to_spark_sql(value)
                )
            }
            ProximaType::Struct { fields } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(name, ty)| format!("{}: {}", name, Self::proxima_to_spark_sql(ty)))
                    .collect();
                format!("STRUCT<{}>", field_strs.join(", "))
            }
            ProximaType::DenseVector { .. } => "ARRAY<FLOAT>".to_string(),
            ProximaType::SparseVector { .. } => "MAP<INT, FLOAT>".to_string(),
            ProximaType::BinaryVector { .. } => "BINARY".to_string(),
            ProximaType::Point | ProximaType::GeographyPoint => "BINARY".to_string(),
            ProximaType::Null => "STRING".to_string(),
        }
    }

    // ========================================================================
    // ProximaDB <-> Trino SQL Types
    // ========================================================================

    /// ProximaDB to Trino SQL type string.
    pub fn proxima_to_trino_sql(dt: &ProximaType) -> String {
        match dt {
            ProximaType::Boolean => "BOOLEAN".to_string(),
            ProximaType::Int8 => "TINYINT".to_string(),
            ProximaType::Int16 => "SMALLINT".to_string(),
            ProximaType::Int32 => "INTEGER".to_string(),
            ProximaType::Int64 => "BIGINT".to_string(),
            ProximaType::UInt8 => "SMALLINT".to_string(),
            ProximaType::UInt16 => "INTEGER".to_string(),
            ProximaType::UInt32 => "BIGINT".to_string(),
            ProximaType::UInt64 => "DECIMAL(20,0)".to_string(),
            ProximaType::Float16 | ProximaType::Float32 => "REAL".to_string(),
            ProximaType::Float64 => "DOUBLE".to_string(),
            ProximaType::Decimal { precision, scale } => {
                format!("DECIMAL({}, {})", precision, scale)
            }
            ProximaType::String | ProximaType::Symbol => "VARCHAR".to_string(),
            ProximaType::Binary => "VARBINARY".to_string(),
            ProximaType::Date => "DATE".to_string(),
            ProximaType::Time(_) => "TIME".to_string(),
            ProximaType::Timestamp(_) => "TIMESTAMP".to_string(),
            ProximaType::TimestampTz(_) => "TIMESTAMP WITH TIME ZONE".to_string(),
            ProximaType::Interval(_) | ProximaType::Duration(_) => {
                "INTERVAL DAY TO SECOND".to_string()
            }
            ProximaType::Uuid | ProximaType::ULID => "UUID".to_string(),
            ProximaType::Json | ProximaType::Jsonb => "JSON".to_string(),
            ProximaType::Array(element) => {
                format!("ARRAY({})", Self::proxima_to_trino_sql(element))
            }
            ProximaType::Map { key, value } => {
                format!(
                    "MAP({}, {})",
                    Self::proxima_to_trino_sql(key),
                    Self::proxima_to_trino_sql(value)
                )
            }
            ProximaType::Struct { fields } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(name, ty)| format!("{} {}", name, Self::proxima_to_trino_sql(ty)))
                    .collect();
                format!("ROW({})", field_strs.join(", "))
            }
            ProximaType::DenseVector { .. } => "ARRAY(REAL)".to_string(),
            ProximaType::SparseVector { .. } => "MAP(INTEGER, REAL)".to_string(),
            ProximaType::BinaryVector { .. } => "VARBINARY".to_string(),
            ProximaType::Point | ProximaType::GeographyPoint => "VARBINARY".to_string(),
            ProximaType::Null => "VARCHAR".to_string(),
        }
    }

    // ========================================================================
    // ProximaDB <-> Hive Types
    // ========================================================================

    /// ProximaDB to Hive type string.
    pub fn proxima_to_hive(dt: &ProximaType) -> String {
        match dt {
            ProximaType::Boolean => "boolean".to_string(),
            ProximaType::Int8 => "tinyint".to_string(),
            ProximaType::Int16 => "smallint".to_string(),
            ProximaType::Int32 => "int".to_string(),
            ProximaType::Int64 => "bigint".to_string(),
            ProximaType::UInt8 => "smallint".to_string(),
            ProximaType::UInt16 => "int".to_string(),
            ProximaType::UInt32 => "bigint".to_string(),
            ProximaType::UInt64 => "decimal(20,0)".to_string(),
            ProximaType::Float32 => "float".to_string(),
            ProximaType::Float64 => "double".to_string(),
            ProximaType::Decimal { precision, scale } => {
                format!("decimal({},{})", precision, scale)
            }
            ProximaType::String => "string".to_string(),
            ProximaType::Binary => "binary".to_string(),
            ProximaType::Date => "date".to_string(),
            ProximaType::Time(_) => "string".to_string(),
            ProximaType::Timestamp(_) | ProximaType::TimestampTz(_) => "timestamp".to_string(),
            ProximaType::Uuid => "string".to_string(),
            ProximaType::Json => "string".to_string(),
            ProximaType::Array(element) => {
                format!("array<{}>", Self::proxima_to_hive(element))
            }
            ProximaType::Map { key, value } => {
                format!(
                    "map<{},{}>",
                    Self::proxima_to_hive(key),
                    Self::proxima_to_hive(value)
                )
            }
            ProximaType::Struct { fields } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(name, ty)| format!("{}:{}", name, Self::proxima_to_hive(ty)))
                    .collect();
                format!("struct<{}>", field_strs.join(","))
            }
            ProximaType::DenseVector { .. } => "array<float>".to_string(),
            ProximaType::SparseVector { .. } => "map<int,float>".to_string(),
            _ => "binary".to_string(),
        }
    }

    // ========================================================================
    // ProximaDB <-> PostgreSQL Types
    // ========================================================================

    /// ProximaDB to PostgreSQL type string.
    pub fn proxima_to_postgres(dt: &ProximaType) -> String {
        match dt {
            ProximaType::Boolean => "BOOLEAN".to_string(),
            ProximaType::Int8 => "SMALLINT".to_string(), // PG has no TINYINT
            ProximaType::Int16 => "SMALLINT".to_string(),
            ProximaType::Int32 => "INTEGER".to_string(),
            ProximaType::Int64 => "BIGINT".to_string(),
            ProximaType::UInt8 => "SMALLINT".to_string(),
            ProximaType::UInt16 => "INTEGER".to_string(),
            ProximaType::UInt32 => "BIGINT".to_string(),
            ProximaType::UInt64 => "NUMERIC(20,0)".to_string(),
            ProximaType::Float32 => "REAL".to_string(),
            ProximaType::Float64 => "DOUBLE PRECISION".to_string(),
            ProximaType::Decimal { precision, scale } => {
                format!("NUMERIC({}, {})", precision, scale)
            }
            ProximaType::String => "TEXT".to_string(),
            ProximaType::Binary => "BYTEA".to_string(),
            ProximaType::Date => "DATE".to_string(),
            ProximaType::Time(_) => "TIME".to_string(),
            ProximaType::Timestamp(_) => "TIMESTAMP".to_string(),
            ProximaType::TimestampTz(_) => "TIMESTAMPTZ".to_string(),
            ProximaType::Uuid => "UUID".to_string(),
            ProximaType::Json => "JSONB".to_string(),
            ProximaType::Array(element) => {
                format!("{}[]", Self::proxima_to_postgres(element))
            }
            ProximaType::Map { .. } => "JSONB".to_string(), // PG doesn't have native MAP
            ProximaType::Struct { .. } => "JSONB".to_string(), // Use JSONB for structs
            ProximaType::DenseVector { dim, .. } => {
                format!("vector({})", dim) // pgvector extension
            }
            ProximaType::SparseVector { .. } => "JSONB".to_string(),
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
    pub fn common_type(left: &ProximaType, right: &ProximaType) -> Option<ProximaType> {
        use ProximaType::*;

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
    pub fn can_implicit_cast(from: &ProximaType, to: &ProximaType) -> bool {
        if from == to {
            return true;
        }

        Self::common_type(from, to).is_some_and(|t| &t == to)
    }

    /// Get cast expression for SQL.
    pub fn cast_expression(from: &ProximaType, to: &ProximaType) -> Option<String> {
        if from == to {
            return None;
        }

        // Direct numeric casts
        match (from, to) {
            (ProximaType::Int8 | ProximaType::Int16 | ProximaType::Int32, ProximaType::Int64) => {
                Some("CAST(? AS BIGINT)".to_string())
            }
            (ProximaType::Float32, ProximaType::Float64) => Some("CAST(? AS DOUBLE)".to_string()),
            (_, ProximaType::String) => Some("CAST(? AS VARCHAR)".to_string()),
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
            TypeMapper::proxima_to_spark_sql(&ProximaType::Int64),
            "BIGINT"
        );
        assert_eq!(
            TypeMapper::proxima_to_spark_sql(&ProximaType::DenseVector {
                element: proximadb_data_model::VectorElement::Float32,
                dim: 512,
            }),
            "ARRAY<FLOAT>"
        );
    }

    #[test]
    fn test_trino_type_mapping() {
        assert_eq!(
            TypeMapper::proxima_to_trino_sql(&ProximaType::String),
            "VARCHAR"
        );
        assert_eq!(TypeMapper::proxima_to_trino_sql(&ProximaType::Json), "JSON");
    }

    #[test]
    fn test_postgres_type_mapping() {
        assert_eq!(TypeMapper::proxima_to_postgres(&ProximaType::Uuid), "UUID");
        assert_eq!(
            TypeMapper::proxima_to_postgres(&ProximaType::DenseVector {
                element: proximadb_data_model::VectorElement::Float32,
                dim: 1536,
            }),
            "vector(1536)"
        );
    }

    #[test]
    fn test_type_coercion() {
        assert_eq!(
            TypeCoercion::common_type(&ProximaType::Int32, &ProximaType::Int64),
            Some(ProximaType::Int64)
        );

        assert!(TypeCoercion::can_implicit_cast(
            &ProximaType::Int32,
            &ProximaType::Int64
        ));

        assert!(!TypeCoercion::can_implicit_cast(
            &ProximaType::Int64,
            &ProximaType::Int32
        ));
    }
}
