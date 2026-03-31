//! # ProximaSchema - Arrow-Native Schema Definition
//!
//! Unified schema replacing VectorRecord for compute engine compatibility.
//! Provides stable column IDs, schema evolution, and Arrow conversion.

use arrow_schema::{
    DataType as ArrowDataType, Field, Schema as ArrowSchema, TimeUnit as ArrowTimeUnit,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Unified schema for ProximaDB collections.
///
/// Replaces hardcoded VectorRecord with flexible Arrow-based schema
/// supporting schema evolution and compute engine compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProximaSchema {
    /// Schema identifier (UUID)
    pub schema_id: String,

    /// Schema version (monotonically increasing)
    pub version: u32,

    /// Parent schema ID (for inheritance tracking)
    pub parent_schema_id: Option<String>,

    /// Column definitions with stable IDs
    pub columns: Vec<ProximaColumn>,

    /// Primary key column IDs
    pub primary_key: Vec<i32>,

    /// Schema fingerprint for fast comparison (xxhash64-style)
    pub fingerprint: u64,

    /// Schema metadata (engine hints, statistics)
    pub metadata: HashMap<String, String>,

    /// Creation timestamp (millis since epoch)
    pub created_at_ms: i64,

    /// Flag indicating if this is the legacy VectorRecord schema
    pub is_legacy_vector_record: bool,
}

/// Column definition with stable ID for schema evolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProximaColumn {
    /// Stable column ID (never changes, survives renames)
    pub id: i32,

    /// Column name
    pub name: String,

    /// Data type (ProximaDB native type)
    pub data_type: ProximaDataType,

    /// Is nullable
    pub nullable: bool,

    /// Default value expression (SQL literal or function)
    pub default_value: Option<DefaultValue>,

    /// Column documentation
    pub comment: Option<String>,

    /// Column metadata (encoding hints, bloom filter config)
    pub metadata: HashMap<String, String>,

    /// Tombstone flag (soft delete for schema evolution)
    pub is_deleted: bool,

    /// Original field ID (for tracking renames)
    pub original_id: Option<i32>,
}

/// ProximaDB native data types with vector extensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProximaDataType {
    // Primitive types
    Boolean,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    Decimal {
        precision: u8,
        scale: i8,
    },
    String,
    Binary,
    Date,
    Time {
        unit: TimeUnit,
    },
    Timestamp {
        unit: TimeUnit,
        timezone: Option<String>,
    },
    Uuid,
    Json,

    // Complex types
    List {
        element: Box<ProximaDataType>,
    },
    Map {
        key: Box<ProximaDataType>,
        value: Box<ProximaDataType>,
    },
    Struct {
        fields: Vec<ProximaColumn>,
    },

    // Vector types (ProximaDB extensions)
    Vector {
        dimension: u32,
        element_type: VectorElementType,
    },
    SparseVector {
        max_dimension: Option<u32>,
    },
    BinaryVector {
        dimension: u32,
    },

    // Quantized vector types (internal use)
    QuantizedInt8Vector {
        dimension: u32,
        scale_factor: u32,
        zero_point: i8,
    },
    QuantizedPQVector {
        dimension: u32,
        segments: u8,
        bits: u8,
    },
    QuantizedBinaryVector {
        dimension: u32,
    },
}

impl PartialEq for ProximaDataType {
    fn eq(&self, other: &Self) -> bool {
        use ProximaDataType::*;
        match (self, other) {
            (Boolean, Boolean) => true,
            (Int8, Int8) => true,
            (Int16, Int16) => true,
            (Int32, Int32) => true,
            (Int64, Int64) => true,
            (UInt8, UInt8) => true,
            (UInt16, UInt16) => true,
            (UInt32, UInt32) => true,
            (UInt64, UInt64) => true,
            (Float32, Float32) => true,
            (Float64, Float64) => true,
            (
                Decimal {
                    precision: p1,
                    scale: s1,
                },
                Decimal {
                    precision: p2,
                    scale: s2,
                },
            ) => p1 == p2 && s1 == s2,
            (String, String) => true,
            (Binary, Binary) => true,
            (Date, Date) => true,
            (Time { unit: u1 }, Time { unit: u2 }) => u1 == u2,
            (
                Timestamp {
                    unit: u1,
                    timezone: t1,
                },
                Timestamp {
                    unit: u2,
                    timezone: t2,
                },
            ) => u1 == u2 && t1 == t2,
            (Uuid, Uuid) => true,
            (Json, Json) => true,
            (List { element: e1 }, List { element: e2 }) => e1 == e2,
            (Map { key: k1, value: v1 }, Map { key: k2, value: v2 }) => k1 == k2 && v1 == v2,
            (Struct { fields: f1 }, Struct { fields: f2 }) => {
                // Compare by field count and ids (not full equality since ProximaColumn doesn't impl PartialEq)
                f1.len() == f2.len()
                    && f1
                        .iter()
                        .zip(f2.iter())
                        .all(|(a, b)| a.id == b.id && a.name == b.name)
            }
            (
                Vector {
                    dimension: d1,
                    element_type: e1,
                },
                Vector {
                    dimension: d2,
                    element_type: e2,
                },
            ) => d1 == d2 && e1 == e2,
            (SparseVector { max_dimension: m1 }, SparseVector { max_dimension: m2 }) => m1 == m2,
            (BinaryVector { dimension: d1 }, BinaryVector { dimension: d2 }) => d1 == d2,
            (
                QuantizedInt8Vector {
                    dimension: d1,
                    scale_factor: s1,
                    zero_point: z1,
                },
                QuantizedInt8Vector {
                    dimension: d2,
                    scale_factor: s2,
                    zero_point: z2,
                },
            ) => d1 == d2 && s1 == s2 && z1 == z2,
            (
                QuantizedPQVector {
                    dimension: d1,
                    segments: s1,
                    bits: b1,
                },
                QuantizedPQVector {
                    dimension: d2,
                    segments: s2,
                    bits: b2,
                },
            ) => d1 == d2 && s1 == s2 && b1 == b2,
            (QuantizedBinaryVector { dimension: d1 }, QuantizedBinaryVector { dimension: d2 }) => {
                d1 == d2
            }
            _ => false,
        }
    }
}

/// Vector element type for dense vectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorElementType {
    Float32,
    Float64,
    Float16,
    BFloat16,
}

/// Time unit for temporal types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeUnit {
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
}

/// Default value for columns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DefaultValue {
    /// Literal value (stored as JSON)
    Literal(String),
    /// SQL expression
    Expression(String),
    /// Auto-generated (UUID, timestamp, sequence)
    AutoGenerate(AutoGenerateType),
}

/// Auto-generate types for default values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutoGenerateType {
    Uuid,
    CurrentTimestamp,
    Sequence { start: i64, increment: i64 },
}

impl ProximaSchema {
    /// Create legacy VectorRecord schema (v0) for backward compatibility.
    pub fn vector_record_schema(dimension: u32) -> Self {
        let columns = vec![
            ProximaColumn {
                id: 1,
                name: "id".to_string(),
                data_type: ProximaDataType::String,
                nullable: false,
                default_value: None,
                comment: Some("Vector record ID".to_string()),
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 2,
                name: "vector".to_string(),
                data_type: ProximaDataType::Vector {
                    dimension,
                    element_type: VectorElementType::Float32,
                },
                nullable: false,
                default_value: None,
                comment: Some("Embedding vector".to_string()),
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 3,
                name: "metadata".to_string(),
                data_type: ProximaDataType::Json,
                nullable: true,
                default_value: None,
                comment: Some("JSON metadata".to_string()),
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 4,
                name: "timestamp".to_string(),
                data_type: ProximaDataType::Timestamp {
                    unit: TimeUnit::Millisecond,
                    timezone: None,
                },
                nullable: false,
                default_value: Some(DefaultValue::AutoGenerate(
                    AutoGenerateType::CurrentTimestamp,
                )),
                comment: Some("Record timestamp".to_string()),
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 5,
                name: "version".to_string(),
                data_type: ProximaDataType::Int64,
                nullable: true,
                default_value: Some(DefaultValue::Literal("1".to_string())),
                comment: Some("MVCC version".to_string()),
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
        ];

        Self {
            schema_id: "vector_record_v0".to_string(),
            version: 0,
            parent_schema_id: None,
            columns: columns.clone(),
            primary_key: vec![1], // id column
            fingerprint: Self::compute_fingerprint_for_columns(&columns),
            metadata: HashMap::from([("legacy".to_string(), "true".to_string())]),
            created_at_ms: 0,
            is_legacy_vector_record: true,
        }
    }

    /// Create a new schema with custom columns.
    pub fn new(schema_id: String, columns: Vec<ProximaColumn>, primary_key: Vec<i32>) -> Self {
        let fingerprint = Self::compute_fingerprint_for_columns(&columns);
        let now_ms = chrono::Utc::now().timestamp_millis();

        Self {
            schema_id,
            version: 1,
            parent_schema_id: None,
            columns,
            primary_key,
            fingerprint,
            metadata: HashMap::new(),
            created_at_ms: now_ms,
            is_legacy_vector_record: false,
        }
    }

    /// Create standard vector schema with custom metadata columns.
    pub fn with_metadata_columns(
        schema_id: String,
        dimension: u32,
        metadata_fields: Vec<(String, ProximaDataType)>,
    ) -> Self {
        let mut columns = vec![
            ProximaColumn {
                id: 1,
                name: "id".to_string(),
                data_type: ProximaDataType::String,
                nullable: false,
                default_value: None,
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 2,
                name: "vector".to_string(),
                data_type: ProximaDataType::Vector {
                    dimension,
                    element_type: VectorElementType::Float32,
                },
                nullable: false,
                default_value: None,
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 3,
                name: "timestamp".to_string(),
                data_type: ProximaDataType::Timestamp {
                    unit: TimeUnit::Millisecond,
                    timezone: None,
                },
                nullable: false,
                default_value: Some(DefaultValue::AutoGenerate(
                    AutoGenerateType::CurrentTimestamp,
                )),
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
        ];

        // Add custom metadata columns
        let mut next_id = 4;
        for (name, dtype) in metadata_fields {
            columns.push(ProximaColumn {
                id: next_id,
                name,
                data_type: dtype,
                nullable: true,
                default_value: None,
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            });
            next_id += 1;
        }

        Self::new(schema_id, columns, vec![1])
    }

    /// Convert to Arrow Schema.
    pub fn to_arrow_schema(&self) -> Arc<ArrowSchema> {
        let fields: Vec<Field> = self
            .columns
            .iter()
            .filter(|c| !c.is_deleted)
            .map(|col| col.to_arrow_field())
            .collect();
        Arc::new(ArrowSchema::new(fields))
    }

    /// Create from Arrow Schema with auto-generated column IDs.
    pub fn from_arrow_schema(schema: &ArrowSchema, schema_id: String) -> Self {
        let columns: Vec<ProximaColumn> = schema
            .fields()
            .iter()
            .enumerate()
            .map(|(idx, field)| ProximaColumn::from_arrow_field(field, (idx + 1) as i32))
            .collect();

        let fingerprint = Self::compute_fingerprint_for_columns(&columns);

        Self {
            schema_id,
            version: 1,
            parent_schema_id: None,
            columns,
            primary_key: vec![],
            fingerprint,
            metadata: HashMap::new(),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            is_legacy_vector_record: false,
        }
    }

    /// Compute schema fingerprint using hash.
    pub fn compute_fingerprint_for_columns(columns: &[ProximaColumn]) -> u64 {
        use std::collections::hash_map::DefaultHasher;

        let mut hasher = DefaultHasher::new();
        for col in columns {
            if !col.is_deleted {
                col.id.hash(&mut hasher);
                col.name.hash(&mut hasher);
                // Hash type representation
                format!("{:?}", col.data_type).hash(&mut hasher);
                col.nullable.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Get column by ID.
    pub fn column_by_id(&self, id: i32) -> Option<&ProximaColumn> {
        self.columns.iter().find(|c| c.id == id && !c.is_deleted)
    }

    /// Get column by name.
    pub fn column_by_name(&self, name: &str) -> Option<&ProximaColumn> {
        self.columns
            .iter()
            .find(|c| c.name == name && !c.is_deleted)
    }

    /// Get next available column ID.
    pub fn next_column_id(&self) -> i32 {
        self.columns.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }

    /// Get active column count (excluding deleted).
    pub fn active_column_count(&self) -> usize {
        self.columns.iter().filter(|c| !c.is_deleted).count()
    }

    /// Get vector dimension if this schema has a vector column.
    pub fn vector_dimension(&self) -> Option<u32> {
        for col in &self.columns {
            if let ProximaDataType::Vector { dimension, .. } = &col.data_type {
                return Some(*dimension);
            }
        }
        None
    }
}

impl ProximaColumn {
    /// Convert to Arrow Field.
    pub fn to_arrow_field(&self) -> Field {
        let arrow_type = self.data_type.to_arrow_type();
        let mut field = Field::new(&self.name, arrow_type, self.nullable);

        // Add column ID and comment as metadata
        let mut meta = HashMap::new();
        meta.insert("proxima_column_id".to_string(), self.id.to_string());
        if let Some(ref comment) = self.comment {
            meta.insert("comment".to_string(), comment.clone());
        }
        field = field.with_metadata(meta);

        field
    }

    /// Create from Arrow Field.
    pub fn from_arrow_field(field: &Field, id: i32) -> Self {
        Self {
            id,
            name: field.name().clone(),
            data_type: ProximaDataType::from_arrow_type(field.data_type()),
            nullable: field.is_nullable(),
            default_value: None,
            comment: field.metadata().get("comment").cloned(),
            metadata: HashMap::new(),
            is_deleted: false,
            original_id: None,
        }
    }
}

impl ProximaDataType {
    /// Convert to Arrow DataType.
    pub fn to_arrow_type(&self) -> ArrowDataType {
        match self {
            ProximaDataType::Boolean => ArrowDataType::Boolean,
            ProximaDataType::Int8 => ArrowDataType::Int8,
            ProximaDataType::Int16 => ArrowDataType::Int16,
            ProximaDataType::Int32 => ArrowDataType::Int32,
            ProximaDataType::Int64 => ArrowDataType::Int64,
            ProximaDataType::UInt8 => ArrowDataType::UInt8,
            ProximaDataType::UInt16 => ArrowDataType::UInt16,
            ProximaDataType::UInt32 => ArrowDataType::UInt32,
            ProximaDataType::UInt64 => ArrowDataType::UInt64,
            ProximaDataType::Float32 => ArrowDataType::Float32,
            ProximaDataType::Float64 => ArrowDataType::Float64,
            ProximaDataType::Decimal { precision, scale } => {
                ArrowDataType::Decimal128(*precision, *scale)
            }
            ProximaDataType::String => ArrowDataType::Utf8,
            ProximaDataType::Binary => ArrowDataType::Binary,
            ProximaDataType::Date => ArrowDataType::Date32,
            ProximaDataType::Time { unit } => ArrowDataType::Time64(unit.to_arrow_time_unit()),
            ProximaDataType::Timestamp { unit, timezone } => ArrowDataType::Timestamp(
                unit.to_arrow_time_unit(),
                timezone.clone().map(Into::into),
            ),
            ProximaDataType::Uuid => ArrowDataType::Utf8, // UUID as string
            ProximaDataType::Json => ArrowDataType::Utf8, // JSON as string
            ProximaDataType::List { element } => {
                ArrowDataType::List(Arc::new(Field::new("item", element.to_arrow_type(), true)))
            }
            ProximaDataType::Map { key, value } => ArrowDataType::Map(
                Arc::new(Field::new(
                    "entries",
                    ArrowDataType::Struct(
                        vec![
                            Field::new("key", key.to_arrow_type(), false),
                            Field::new("value", value.to_arrow_type(), true),
                        ]
                        .into(),
                    ),
                    false,
                )),
                false,
            ),
            ProximaDataType::Struct { fields } => ArrowDataType::Struct(
                fields
                    .iter()
                    .map(|f| f.to_arrow_field())
                    .collect::<Vec<_>>()
                    .into(),
            ),
            ProximaDataType::Vector { dimension, .. } => {
                ArrowDataType::FixedSizeBinary((*dimension * 4) as i32)
            }
            ProximaDataType::SparseVector { .. } => {
                // Sparse vector as Map<i32, f32>
                ArrowDataType::Map(
                    Arc::new(Field::new(
                        "entries",
                        ArrowDataType::Struct(
                            vec![
                                Field::new("key", ArrowDataType::Int32, false),
                                Field::new("value", ArrowDataType::Float32, false),
                            ]
                            .into(),
                        ),
                        false,
                    )),
                    false,
                )
            }
            ProximaDataType::BinaryVector { dimension } => {
                ArrowDataType::FixedSizeBinary(dimension.div_ceil(8) as i32)
            }
            ProximaDataType::QuantizedInt8Vector { dimension, .. } => {
                ArrowDataType::FixedSizeBinary(*dimension as i32)
            }
            ProximaDataType::QuantizedPQVector { segments, .. } => {
                ArrowDataType::FixedSizeBinary(*segments as i32)
            }
            ProximaDataType::QuantizedBinaryVector { dimension } => {
                ArrowDataType::FixedSizeBinary(dimension.div_ceil(8) as i32)
            }
        }
    }

    /// Convert from Arrow DataType.
    pub fn from_arrow_type(arrow_type: &ArrowDataType) -> Self {
        match arrow_type {
            ArrowDataType::Boolean => ProximaDataType::Boolean,
            ArrowDataType::Int8 => ProximaDataType::Int8,
            ArrowDataType::Int16 => ProximaDataType::Int16,
            ArrowDataType::Int32 => ProximaDataType::Int32,
            ArrowDataType::Int64 => ProximaDataType::Int64,
            ArrowDataType::UInt8 => ProximaDataType::UInt8,
            ArrowDataType::UInt16 => ProximaDataType::UInt16,
            ArrowDataType::UInt32 => ProximaDataType::UInt32,
            ArrowDataType::UInt64 => ProximaDataType::UInt64,
            ArrowDataType::Float32 => ProximaDataType::Float32,
            ArrowDataType::Float64 => ProximaDataType::Float64,
            ArrowDataType::Decimal128(p, s) | ArrowDataType::Decimal256(p, s) => {
                ProximaDataType::Decimal {
                    precision: *p,
                    scale: *s,
                }
            }
            ArrowDataType::Utf8 | ArrowDataType::LargeUtf8 => ProximaDataType::String,
            ArrowDataType::Binary | ArrowDataType::LargeBinary => ProximaDataType::Binary,
            ArrowDataType::Date32 | ArrowDataType::Date64 => ProximaDataType::Date,
            ArrowDataType::Time64(unit) => ProximaDataType::Time {
                unit: TimeUnit::from_arrow(*unit),
            },
            ArrowDataType::Timestamp(unit, tz) => ProximaDataType::Timestamp {
                unit: TimeUnit::from_arrow(*unit),
                timezone: tz.as_ref().map(|s| s.to_string()),
            },
            ArrowDataType::FixedSizeBinary(size) => {
                // Heuristic: if divisible by 4 and reasonable size, assume vector
                if *size % 4 == 0 && *size >= 4 && *size <= 16384 {
                    ProximaDataType::Vector {
                        dimension: (*size / 4) as u32,
                        element_type: VectorElementType::Float32,
                    }
                } else {
                    ProximaDataType::Binary
                }
            }
            ArrowDataType::List(field) => ProximaDataType::List {
                element: Box::new(Self::from_arrow_type(field.data_type())),
            },
            ArrowDataType::LargeList(field) => ProximaDataType::List {
                element: Box::new(Self::from_arrow_type(field.data_type())),
            },
            ArrowDataType::Map(field, _) => {
                // Extract key/value types from Map struct
                if let ArrowDataType::Struct(fields) = field.data_type()
                    && fields.len() >= 2 {
                        return ProximaDataType::Map {
                            key: Box::new(Self::from_arrow_type(fields[0].data_type())),
                            value: Box::new(Self::from_arrow_type(fields[1].data_type())),
                        };
                    }
                ProximaDataType::Json // Fallback
            }
            ArrowDataType::Struct(fields) => ProximaDataType::Struct {
                fields: fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| ProximaColumn::from_arrow_field(f, i as i32))
                    .collect(),
            },
            _ => ProximaDataType::Binary, // Fallback for unsupported types
        }
    }
}

impl TimeUnit {
    /// Convert to Arrow TimeUnit.
    pub fn to_arrow_time_unit(&self) -> ArrowTimeUnit {
        match self {
            TimeUnit::Second => ArrowTimeUnit::Second,
            TimeUnit::Millisecond => ArrowTimeUnit::Millisecond,
            TimeUnit::Microsecond => ArrowTimeUnit::Microsecond,
            TimeUnit::Nanosecond => ArrowTimeUnit::Nanosecond,
        }
    }

    /// Convert from Arrow TimeUnit.
    pub fn from_arrow(unit: ArrowTimeUnit) -> Self {
        match unit {
            ArrowTimeUnit::Second => TimeUnit::Second,
            ArrowTimeUnit::Millisecond => TimeUnit::Millisecond,
            ArrowTimeUnit::Microsecond => TimeUnit::Microsecond,
            ArrowTimeUnit::Nanosecond => TimeUnit::Nanosecond,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_record_schema() {
        let schema = ProximaSchema::vector_record_schema(1536);
        assert!(schema.is_legacy_vector_record);
        assert_eq!(schema.version, 0);
        assert_eq!(schema.active_column_count(), 5);
        assert_eq!(schema.vector_dimension(), Some(1536));
    }

    #[test]
    fn test_arrow_conversion() {
        let schema = ProximaSchema::vector_record_schema(768);
        let arrow_schema = schema.to_arrow_schema();
        assert_eq!(arrow_schema.fields().len(), 5);

        // Round-trip test
        let recovered = ProximaSchema::from_arrow_schema(&arrow_schema, "test".to_string());
        assert_eq!(recovered.active_column_count(), 5);
    }

    #[test]
    fn test_column_by_name() {
        let schema = ProximaSchema::vector_record_schema(128);
        assert!(schema.column_by_name("id").is_some());
        assert!(schema.column_by_name("vector").is_some());
        assert!(schema.column_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_fingerprint_stability() {
        let schema1 = ProximaSchema::vector_record_schema(512);
        let schema2 = ProximaSchema::vector_record_schema(512);
        assert_eq!(schema1.fingerprint, schema2.fingerprint);

        let schema3 = ProximaSchema::vector_record_schema(1024);
        assert_ne!(schema1.fingerprint, schema3.fingerprint);
    }
}
