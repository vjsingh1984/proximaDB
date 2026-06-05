//! # ProximaSchema - Arrow-Native Schema Definition
//!
//! Unified schema replacing VectorRecord for compute engine compatibility.
//! Provides stable column IDs, schema evolution, and Arrow conversion.

use anyhow::{Context, Result, anyhow};
use arrow_schema::{
    DataType as ArrowDataType, Field, Schema as ArrowSchema, TimeUnit as ArrowTimeUnit,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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

// ---------------------------------------------------------------------------
// Canonical bridge: ProximaDataType <-> ProximaType (ADR-024 Step 2a)
//
// `ProximaType` (proximadb-data-model) is the single canonical *logical* type;
// `ProximaDataType` is the storage/Arrow schema descriptor being folded into it.
// These conversions are the previously-missing link, so all code can funnel
// through `ProximaType` and the duplicate Arrow/pgwire logic can delegate.
//
// Per ADR-024, quantization is *physical* encoding, not a logical type: the
// quantized `ProximaDataType` variants map to their logical vector type
// (`DenseVector`/`BinaryVector`), dropping the encoding parameters (those belong
// on a storage encoding attribute, added when `ProximaDataType` is deleted).
// Logical types `ProximaDataType` cannot represent (Float16/Symbol/Interval/
// Duration/ULID/Jsonb/Point/Null) map to their nearest physical carrier — lossy
// by design (the logical `ProximaType` stays authoritative).
// ---------------------------------------------------------------------------

fn file_time_unit_to_dm(unit: &TimeUnit) -> proximadb_data_model::TimeUnit {
    match unit {
        TimeUnit::Second => proximadb_data_model::TimeUnit::Second,
        TimeUnit::Millisecond => proximadb_data_model::TimeUnit::Millisecond,
        TimeUnit::Microsecond => proximadb_data_model::TimeUnit::Microsecond,
        TimeUnit::Nanosecond => proximadb_data_model::TimeUnit::Nanosecond,
    }
}

fn dm_time_unit_to_file(unit: proximadb_data_model::TimeUnit) -> TimeUnit {
    match unit {
        proximadb_data_model::TimeUnit::Second => TimeUnit::Second,
        proximadb_data_model::TimeUnit::Millisecond => TimeUnit::Millisecond,
        proximadb_data_model::TimeUnit::Microsecond => TimeUnit::Microsecond,
        proximadb_data_model::TimeUnit::Nanosecond => TimeUnit::Nanosecond,
    }
}

impl From<&ProximaDataType> for proximadb_data_model::ProximaType {
    fn from(dt: &ProximaDataType) -> Self {
        use proximadb_data_model::{ProximaType as P, VectorElement as Ve};
        let elem = |e: &VectorElementType| match e {
            VectorElementType::Float16 => Ve::Float16,
            VectorElementType::BFloat16 => Ve::BFloat16,
            VectorElementType::Float32 => Ve::Float32,
            VectorElementType::Float64 => Ve::Float64,
        };
        match dt {
            ProximaDataType::Boolean => P::Boolean,
            ProximaDataType::Int8 => P::Int8,
            ProximaDataType::Int16 => P::Int16,
            ProximaDataType::Int32 => P::Int32,
            ProximaDataType::Int64 => P::Int64,
            ProximaDataType::UInt8 => P::UInt8,
            ProximaDataType::UInt16 => P::UInt16,
            ProximaDataType::UInt32 => P::UInt32,
            ProximaDataType::UInt64 => P::UInt64,
            ProximaDataType::Float32 => P::Float32,
            ProximaDataType::Float64 => P::Float64,
            ProximaDataType::Decimal { precision, scale } => P::Decimal {
                precision: *precision,
                scale: *scale as u8,
            },
            ProximaDataType::String => P::String,
            ProximaDataType::Binary => P::Binary,
            ProximaDataType::Date => P::Date,
            ProximaDataType::Time { unit } => P::Time(file_time_unit_to_dm(unit)),
            ProximaDataType::Timestamp { unit, timezone } => {
                if timezone.is_some() {
                    P::TimestampTz(file_time_unit_to_dm(unit))
                } else {
                    P::Timestamp(file_time_unit_to_dm(unit))
                }
            }
            ProximaDataType::Uuid => P::Uuid,
            ProximaDataType::Json => P::Json,
            ProximaDataType::List { element } => P::Array(Box::new(P::from(element.as_ref()))),
            ProximaDataType::Map { key, value } => P::Map {
                key: Box::new(P::from(key.as_ref())),
                value: Box::new(P::from(value.as_ref())),
            },
            ProximaDataType::Struct { fields } => P::Struct {
                fields: fields
                    .iter()
                    .map(|c| (c.name.clone(), P::from(&c.data_type)))
                    .collect(),
            },
            ProximaDataType::Vector {
                dimension,
                element_type,
            } => P::DenseVector {
                element: elem(element_type),
                dim: *dimension as usize,
            },
            ProximaDataType::SparseVector { .. } => P::SparseVector {
                element: Ve::Float32,
            },
            ProximaDataType::BinaryVector { dimension } => P::BinaryVector {
                dim: *dimension as usize,
            },
            // Quantized = physical encoding; reduce to the logical vector type.
            ProximaDataType::QuantizedInt8Vector { dimension, .. }
            | ProximaDataType::QuantizedPQVector { dimension, .. } => P::DenseVector {
                element: Ve::Int8,
                dim: *dimension as usize,
            },
            ProximaDataType::QuantizedBinaryVector { dimension } => P::BinaryVector {
                dim: *dimension as usize,
            },
        }
    }
}

impl From<&proximadb_data_model::ProximaType> for ProximaDataType {
    fn from(ty: &proximadb_data_model::ProximaType) -> Self {
        use proximadb_data_model::{ProximaType as P, VectorElement as Ve};
        let elem = |e: &Ve| match e {
            Ve::Float16 => VectorElementType::Float16,
            Ve::BFloat16 => VectorElementType::BFloat16,
            Ve::Float64 => VectorElementType::Float64,
            // ProximaDataType has no Int8 vector element; carry as f32 (lossy).
            Ve::Float32 | Ve::Int8 => VectorElementType::Float32,
        };
        match ty {
            P::Boolean => ProximaDataType::Boolean,
            P::Int8 => ProximaDataType::Int8,
            P::Int16 => ProximaDataType::Int16,
            P::Int32 => ProximaDataType::Int32,
            P::Int64 => ProximaDataType::Int64,
            P::UInt8 => ProximaDataType::UInt8,
            P::UInt16 => ProximaDataType::UInt16,
            P::UInt32 => ProximaDataType::UInt32,
            P::UInt64 => ProximaDataType::UInt64,
            // ProximaDataType has no Float16 scalar; widen to f32 (lossy).
            P::Float16 | P::Float32 => ProximaDataType::Float32,
            P::Float64 => ProximaDataType::Float64,
            P::Decimal { precision, scale } => ProximaDataType::Decimal {
                precision: *precision,
                scale: *scale as i8,
            },
            P::String | P::Symbol => ProximaDataType::String,
            P::Binary => ProximaDataType::Binary,
            P::Date => ProximaDataType::Date,
            P::Time(u) => ProximaDataType::Time {
                unit: dm_time_unit_to_file(*u),
            },
            P::Timestamp(u) => ProximaDataType::Timestamp {
                unit: dm_time_unit_to_file(*u),
                timezone: None,
            },
            P::TimestampTz(u) => ProximaDataType::Timestamp {
                unit: dm_time_unit_to_file(*u),
                timezone: Some("UTC".to_string()),
            },
            // No interval/duration carrier in the storage descriptor.
            P::Interval(_) | P::Duration(_) => ProximaDataType::Binary,
            P::Uuid | P::ULID => ProximaDataType::Uuid,
            P::Json | P::Jsonb => ProximaDataType::Json,
            P::Array(element) => ProximaDataType::List {
                element: Box::new(ProximaDataType::from(element.as_ref())),
            },
            P::Map { key, value } => ProximaDataType::Map {
                key: Box::new(ProximaDataType::from(key.as_ref())),
                value: Box::new(ProximaDataType::from(value.as_ref())),
            },
            P::Struct { fields } => ProximaDataType::Struct {
                fields: fields
                    .iter()
                    .enumerate()
                    .map(|(i, (name, ty))| ProximaColumn {
                        id: i as i32,
                        name: name.clone(),
                        data_type: ProximaDataType::from(ty),
                        nullable: true,
                        default_value: None,
                        comment: None,
                        metadata: std::collections::HashMap::new(),
                        is_deleted: false,
                        original_id: None,
                    })
                    .collect(),
            },
            P::DenseVector { element, dim } => ProximaDataType::Vector {
                dimension: *dim as u32,
                element_type: elem(element),
            },
            P::SparseVector { .. } => ProximaDataType::SparseVector {
                max_dimension: None,
            },
            P::BinaryVector { dim } => ProximaDataType::BinaryVector {
                dimension: *dim as u32,
            },
            // No geo / null carrier in the storage descriptor.
            P::Point | P::GeographyPoint | P::Null => ProximaDataType::Binary,
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

// ============================================================================
// Avro-Style Schema Serialization
// ============================================================================

/// Avro-style schema representation for ProximaSchema.
///
/// Enables schema serialization compatible with schema registries
/// and provides stable schema fingerprinting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroStyleSchema {
    /// Schema type (always "record" for ProximaSchema)
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Namespace (collection identifier)
    pub namespace: String,
    /// Schema name
    pub name: String,
    /// Schema fields
    pub fields: Vec<AvroStyleField>,
    /// Optional aliases
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    /// Custom metadata
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub metadata: HashMap<String, String>,
}

/// Avro-style field representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroStyleField {
    /// Field name
    pub name: String,
    /// Field type (Avro type union for nullable)
    #[serde(rename = "type")]
    pub field_type: AvroStyleType,
    /// Optional default value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Optional documentation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Original column ID (ProximaDB extension)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_id: Option<i32>,
}

/// Avro-style type representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AvroStyleType {
    /// Simple type name
    Simple(String),
    /// Union type (for nullable)
    Union(Vec<String>),
    /// Complex type with properties
    Complex {
        #[serde(rename = "type")]
        type_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        items: Option<Box<AvroStyleType>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        values: Option<Box<AvroStyleType>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        dimension: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        precision: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        scale: Option<i8>,
    },
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
        for (next_id, (name, dtype)) in (4..).zip(metadata_fields) {
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

    /// Serialize to Avro-style schema format.
    pub fn to_avro_style(&self) -> AvroStyleSchema {
        let fields: Vec<AvroStyleField> = self
            .columns
            .iter()
            .filter(|c| !c.is_deleted)
            .map(|col| col.to_avro_field())
            .collect();

        AvroStyleSchema {
            schema_type: "record".to_string(),
            namespace: "com.proximadb".to_string(),
            name: self.schema_id.clone(),
            fields,
            aliases: None,
            metadata: self.metadata.clone(),
        }
    }

    /// Deserialize from Avro-style schema format.
    pub fn from_avro_style(avro: &AvroStyleSchema) -> Result<Self> {
        let columns: Vec<ProximaColumn> = avro
            .fields
            .iter()
            .enumerate()
            .map(|(idx, field)| field.to_proxima_column((idx + 1) as i32))
            .collect::<Result<Vec<_>>>()?;

        let fingerprint = Self::compute_fingerprint_for_columns(&columns);
        let now_ms = chrono::Utc::now().timestamp_millis();

        Ok(Self {
            schema_id: avro.name.clone(),
            version: 1,
            parent_schema_id: None,
            columns,
            primary_key: vec![1], // Default to first column
            fingerprint,
            metadata: avro.metadata.clone(),
            created_at_ms: now_ms,
            is_legacy_vector_record: false,
        })
    }

    /// Serialize to JSON string.
    pub fn to_avro_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.to_avro_style())
            .context("Failed to serialize schema to Avro-style JSON")
    }

    /// Deserialize from JSON string.
    pub fn from_avro_json(json: &str) -> Result<Self> {
        let avro: AvroStyleSchema =
            serde_json::from_str(json).context("Failed to parse Avro-style JSON")?;
        Self::from_avro_style(&avro)
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

    /// Convert to Avro-style field.
    fn to_avro_field(&self) -> AvroStyleField {
        let field_type = self.data_type.to_avro_type(self.nullable);

        AvroStyleField {
            name: self.name.clone(),
            field_type,
            default: self.default_value.as_ref().map(|d| match d {
                DefaultValue::Literal(s) => {
                    serde_json::from_str(s).unwrap_or_else(|_| JsonValue::String(s.clone()))
                }
                DefaultValue::Expression(s) => JsonValue::String(s.clone()),
                DefaultValue::AutoGenerate(_) => JsonValue::Null,
            }),
            doc: self.comment.clone(),
            column_id: Some(self.id),
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
                    && fields.len() >= 2
                {
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

    /// Convert to Avro-style type.
    fn to_avro_type(&self, nullable: bool) -> AvroStyleType {
        let base_type = match self {
            ProximaDataType::Boolean => AvroStyleType::Simple("boolean".to_string()),
            ProximaDataType::Int8 | ProximaDataType::Int16 | ProximaDataType::Int32 => {
                AvroStyleType::Simple("int".to_string())
            }
            ProximaDataType::Int64 => AvroStyleType::Simple("long".to_string()),
            ProximaDataType::UInt8 | ProximaDataType::UInt16 | ProximaDataType::UInt32 => {
                AvroStyleType::Simple("int".to_string())
            }
            ProximaDataType::UInt64 => AvroStyleType::Simple("long".to_string()),
            ProximaDataType::Float32 => AvroStyleType::Simple("float".to_string()),
            ProximaDataType::Float64 => AvroStyleType::Simple("double".to_string()),
            ProximaDataType::Decimal { precision, scale } => AvroStyleType::Complex {
                type_name: "bytes".to_string(),
                items: None,
                values: None,
                dimension: None,
                precision: Some(*precision),
                scale: Some(*scale),
            },
            ProximaDataType::String | ProximaDataType::Uuid | ProximaDataType::Json => {
                AvroStyleType::Simple("string".to_string())
            }
            ProximaDataType::Binary => AvroStyleType::Simple("bytes".to_string()),
            ProximaDataType::Date => AvroStyleType::Simple("int".to_string()),
            ProximaDataType::Time { .. } => AvroStyleType::Simple("long".to_string()),
            ProximaDataType::Timestamp { .. } => AvroStyleType::Simple("long".to_string()),
            ProximaDataType::List { element } => AvroStyleType::Complex {
                type_name: "array".to_string(),
                items: Some(Box::new(element.to_avro_type(true))),
                values: None,
                dimension: None,
                precision: None,
                scale: None,
            },
            ProximaDataType::Map { key: _, value } => AvroStyleType::Complex {
                type_name: "map".to_string(),
                items: None,
                values: Some(Box::new(value.to_avro_type(true))),
                dimension: None,
                precision: None,
                scale: None,
            },
            ProximaDataType::Struct { .. } => AvroStyleType::Simple("string".to_string()),
            ProximaDataType::Vector { dimension, .. } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
            ProximaDataType::SparseVector { .. } => AvroStyleType::Simple("bytes".to_string()),
            ProximaDataType::BinaryVector { dimension } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
            ProximaDataType::QuantizedInt8Vector { dimension, .. } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
            ProximaDataType::QuantizedPQVector { dimension, .. } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
            ProximaDataType::QuantizedBinaryVector { dimension } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
        };

        if nullable {
            match base_type {
                AvroStyleType::Simple(t) => AvroStyleType::Union(vec!["null".to_string(), t]),
                _ => base_type,
            }
        } else {
            base_type
        }
    }
}

impl AvroStyleField {
    /// Convert to ProximaColumn.
    fn to_proxima_column(&self, default_id: i32) -> Result<ProximaColumn> {
        let (data_type, nullable) = self.field_type.to_proxima_type()?;

        Ok(ProximaColumn {
            id: self.column_id.unwrap_or(default_id),
            name: self.name.clone(),
            data_type,
            nullable,
            default_value: self
                .default
                .as_ref()
                .map(|v| DefaultValue::Literal(v.to_string())),
            comment: self.doc.clone(),
            metadata: HashMap::new(),
            is_deleted: false,
            original_id: None,
        })
    }
}

impl AvroStyleType {
    /// Convert to ProximaDataType.
    fn to_proxima_type(&self) -> Result<(ProximaDataType, bool)> {
        match self {
            AvroStyleType::Simple(t) => {
                let dtype = match t.as_str() {
                    "null" => ProximaDataType::String,
                    "boolean" => ProximaDataType::Boolean,
                    "int" => ProximaDataType::Int32,
                    "long" => ProximaDataType::Int64,
                    "float" => ProximaDataType::Float32,
                    "double" => ProximaDataType::Float64,
                    "bytes" => ProximaDataType::Binary,
                    "string" => ProximaDataType::String,
                    _ => ProximaDataType::String,
                };
                Ok((dtype, false))
            }
            AvroStyleType::Union(types) => {
                let nullable = types.contains(&"null".to_string());
                let non_null: Vec<_> = types.iter().filter(|t| *t != "null").collect();
                if let Some(t) = non_null.first() {
                    let (dtype, _) = AvroStyleType::Simple((*t).clone()).to_proxima_type()?;
                    Ok((dtype, nullable))
                } else {
                    Ok((ProximaDataType::String, true))
                }
            }
            AvroStyleType::Complex {
                type_name,
                items,
                values,
                dimension,
                precision,
                scale,
            } => {
                let dtype = match type_name.as_str() {
                    "array" => {
                        let element = items
                            .as_ref()
                            .map(|i| i.to_proxima_type().map(|(t, _)| t))
                            .transpose()?
                            .ok_or_else(|| anyhow!("Array type missing items specification"))?;
                        ProximaDataType::List {
                            element: Box::new(element),
                        }
                    }
                    "map" => {
                        let value = values
                            .as_ref()
                            .map(|v| v.to_proxima_type().map(|(t, _)| t))
                            .transpose()?
                            .ok_or_else(|| anyhow!("Map type missing values specification"))?;
                        ProximaDataType::Map {
                            key: Box::new(ProximaDataType::String),
                            value: Box::new(value),
                        }
                    }
                    "fixed" => {
                        if let Some(dim) = dimension {
                            ProximaDataType::Vector {
                                dimension: *dim,
                                element_type: VectorElementType::Float32,
                            }
                        } else {
                            ProximaDataType::Binary
                        }
                    }
                    "bytes" if precision.is_some() => ProximaDataType::Decimal {
                        precision: precision
                            .ok_or_else(|| anyhow!("Decimal type missing precision"))?,
                        scale: scale.ok_or_else(|| anyhow!("Decimal type missing scale"))?,
                    },
                    _ => ProximaDataType::String,
                };
                Ok((dtype, false))
            }
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
    use proximadb_data_model::ProximaType;

    #[test]
    fn proxima_data_type_bridges_to_canonical_logical_type() {
        // Cleanly-mappable types round-trip through the canonical ProximaType.
        for dt in [
            ProximaDataType::Boolean,
            ProximaDataType::Int64,
            ProximaDataType::UInt32,
            ProximaDataType::Float64,
            ProximaDataType::String,
            ProximaDataType::Binary,
            ProximaDataType::Date,
            ProximaDataType::Uuid,
            ProximaDataType::Json,
            ProximaDataType::Timestamp {
                unit: TimeUnit::Nanosecond,
                timezone: None,
            },
            ProximaDataType::Vector {
                dimension: 8,
                element_type: VectorElementType::Float32,
            },
            ProximaDataType::BinaryVector { dimension: 64 },
        ] {
            let logical = ProximaType::from(&dt);
            let back = ProximaDataType::from(&logical);
            assert_eq!(back, dt, "round-trip via ProximaType must preserve {dt:?}");
        }
    }

    #[test]
    fn quantized_storage_types_reduce_to_logical_vector() {
        // Per ADR-024 quantization is physical: it folds to the logical vector
        // type (params dropped), so the bridge is intentionally one-way here.
        let q = ProximaDataType::QuantizedInt8Vector {
            dimension: 16,
            scale_factor: 1,
            zero_point: 0,
        };
        assert_eq!(
            ProximaType::from(&q),
            ProximaType::DenseVector {
                element: proximadb_data_model::VectorElement::Int8,
                dim: 16,
            }
        );
    }

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

    #[test]
    fn schema_builders_accessors_and_deleted_columns_are_explicit() {
        let mut schema = ProximaSchema::with_metadata_columns(
            "products".to_string(),
            384,
            vec![
                ("category".to_string(), ProximaDataType::String),
                ("score".to_string(), ProximaDataType::Float64),
            ],
        );
        schema.columns.push(ProximaColumn {
            id: 99,
            name: "deleted".to_string(),
            data_type: ProximaDataType::Boolean,
            nullable: true,
            default_value: None,
            comment: None,
            metadata: HashMap::new(),
            is_deleted: true,
            original_id: Some(4),
        });

        assert!(!schema.is_legacy_vector_record);
        assert_eq!(schema.primary_key, vec![1]);
        assert_eq!(schema.vector_dimension(), Some(384));
        assert_eq!(schema.active_column_count(), 5);
        assert_eq!(schema.next_column_id(), 100);
        assert!(schema.column_by_id(1).is_some());
        assert!(schema.column_by_id(99).is_none());
        assert!(schema.column_by_name("deleted").is_none());

        let arrow = schema.to_arrow_schema();
        assert_eq!(arrow.fields().len(), 5);
        assert!(arrow.field_with_name("category").is_ok());
        assert!(arrow.field_with_name("deleted").is_err());
    }

    #[test]
    fn proxima_data_type_arrow_mapping_covers_primitives_complex_vectors_and_fallbacks() {
        let nested_field = ProximaColumn {
            id: 1,
            name: "nested".to_string(),
            data_type: ProximaDataType::Int32,
            nullable: false,
            default_value: None,
            comment: None,
            metadata: HashMap::new(),
            is_deleted: false,
            original_id: None,
        };

        let cases = vec![
            (ProximaDataType::Boolean, ArrowDataType::Boolean),
            (ProximaDataType::Int8, ArrowDataType::Int8),
            (ProximaDataType::Int16, ArrowDataType::Int16),
            (ProximaDataType::Int32, ArrowDataType::Int32),
            (ProximaDataType::Int64, ArrowDataType::Int64),
            (ProximaDataType::UInt8, ArrowDataType::UInt8),
            (ProximaDataType::UInt16, ArrowDataType::UInt16),
            (ProximaDataType::UInt32, ArrowDataType::UInt32),
            (ProximaDataType::UInt64, ArrowDataType::UInt64),
            (ProximaDataType::Float32, ArrowDataType::Float32),
            (ProximaDataType::Float64, ArrowDataType::Float64),
            (
                ProximaDataType::Decimal {
                    precision: 12,
                    scale: 2,
                },
                ArrowDataType::Decimal128(12, 2),
            ),
            (ProximaDataType::String, ArrowDataType::Utf8),
            (ProximaDataType::Binary, ArrowDataType::Binary),
            (ProximaDataType::Date, ArrowDataType::Date32),
            (
                ProximaDataType::Time {
                    unit: TimeUnit::Nanosecond,
                },
                ArrowDataType::Time64(ArrowTimeUnit::Nanosecond),
            ),
            (
                ProximaDataType::Timestamp {
                    unit: TimeUnit::Microsecond,
                    timezone: Some("UTC".to_string()),
                },
                ArrowDataType::Timestamp(ArrowTimeUnit::Microsecond, Some("UTC".into())),
            ),
            (ProximaDataType::Uuid, ArrowDataType::Utf8),
            (ProximaDataType::Json, ArrowDataType::Utf8),
            (
                ProximaDataType::Vector {
                    dimension: 3,
                    element_type: VectorElementType::Float32,
                },
                ArrowDataType::FixedSizeBinary(12),
            ),
            (
                ProximaDataType::BinaryVector { dimension: 17 },
                ArrowDataType::FixedSizeBinary(3),
            ),
            (
                ProximaDataType::QuantizedInt8Vector {
                    dimension: 8,
                    scale_factor: 100,
                    zero_point: -1,
                },
                ArrowDataType::FixedSizeBinary(8),
            ),
            (
                ProximaDataType::QuantizedPQVector {
                    dimension: 128,
                    segments: 16,
                    bits: 8,
                },
                ArrowDataType::FixedSizeBinary(16),
            ),
            (
                ProximaDataType::QuantizedBinaryVector { dimension: 9 },
                ArrowDataType::FixedSizeBinary(2),
            ),
        ];

        for (proxima, arrow) in cases {
            assert_eq!(proxima.to_arrow_type(), arrow);
        }

        assert!(matches!(
            ProximaDataType::List {
                element: Box::new(ProximaDataType::String)
            }
            .to_arrow_type(),
            ArrowDataType::List(_)
        ));
        assert!(matches!(
            ProximaDataType::Map {
                key: Box::new(ProximaDataType::String),
                value: Box::new(ProximaDataType::Float64)
            }
            .to_arrow_type(),
            ArrowDataType::Map(_, false)
        ));
        assert!(matches!(
            ProximaDataType::Struct {
                fields: vec![nested_field]
            }
            .to_arrow_type(),
            ArrowDataType::Struct(_)
        ));
        assert!(matches!(
            ProximaDataType::SparseVector {
                max_dimension: Some(100)
            }
            .to_arrow_type(),
            ArrowDataType::Map(_, false)
        ));

        for unit in [
            ArrowTimeUnit::Second,
            ArrowTimeUnit::Millisecond,
            ArrowTimeUnit::Microsecond,
            ArrowTimeUnit::Nanosecond,
        ] {
            assert_eq!(TimeUnit::from_arrow(unit).to_arrow_time_unit(), unit);
        }

        assert_eq!(
            ProximaDataType::from_arrow_type(&ArrowDataType::LargeUtf8),
            ProximaDataType::String
        );
        assert_eq!(
            ProximaDataType::from_arrow_type(&ArrowDataType::LargeBinary),
            ProximaDataType::Binary
        );
        assert_eq!(
            ProximaDataType::from_arrow_type(&ArrowDataType::Date64),
            ProximaDataType::Date
        );
        assert_eq!(
            ProximaDataType::from_arrow_type(&ArrowDataType::Decimal256(20, 4)),
            ProximaDataType::Decimal {
                precision: 20,
                scale: 4
            }
        );
        assert_eq!(
            ProximaDataType::from_arrow_type(&ArrowDataType::FixedSizeBinary(3)),
            ProximaDataType::Binary
        );
        assert_eq!(
            ProximaDataType::from_arrow_type(&ArrowDataType::Duration(ArrowTimeUnit::Second)),
            ProximaDataType::Binary
        );
    }

    #[test]
    fn avro_style_roundtrip_defaults_and_error_paths_are_covered() {
        let columns = vec![
            ProximaColumn {
                id: 10,
                name: "id".to_string(),
                data_type: ProximaDataType::String,
                nullable: false,
                default_value: Some(DefaultValue::Literal("\"p1\"".to_string())),
                comment: Some("primary id".to_string()),
                metadata: HashMap::from([("encoding".to_string(), "plain".to_string())]),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 11,
                name: "tags".to_string(),
                data_type: ProximaDataType::List {
                    element: Box::new(ProximaDataType::String),
                },
                nullable: true,
                default_value: Some(DefaultValue::Expression("array[]".to_string())),
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 12,
                name: "embedding".to_string(),
                data_type: ProximaDataType::Vector {
                    dimension: 4,
                    element_type: VectorElementType::Float32,
                },
                nullable: false,
                default_value: Some(DefaultValue::AutoGenerate(AutoGenerateType::Uuid)),
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 13,
                name: "price".to_string(),
                data_type: ProximaDataType::Decimal {
                    precision: 8,
                    scale: 2,
                },
                nullable: false,
                default_value: None,
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
        ];
        let mut schema = ProximaSchema::new("catalog.products".to_string(), columns, vec![10]);
        schema
            .metadata
            .insert("owner".to_string(), "catalog".to_string());

        let avro = schema.to_avro_style();
        assert_eq!(avro.schema_type, "record");
        assert_eq!(avro.namespace, "com.proximadb");
        assert_eq!(avro.fields.len(), 4);
        assert_eq!(avro.fields[0].default, Some(serde_json::json!("p1")));
        assert_eq!(avro.fields[1].default, Some(serde_json::json!("array[]")));
        assert_eq!(avro.fields[2].default, Some(JsonValue::Null));
        assert_eq!(avro.metadata.get("owner"), Some(&"catalog".to_string()));

        let json = schema.to_avro_json().unwrap();
        let from_json = ProximaSchema::from_avro_json(&json).unwrap();
        assert_eq!(from_json.schema_id, "catalog.products");
        assert_eq!(from_json.primary_key, vec![1]);
        assert_eq!(from_json.active_column_count(), 4);
        assert!(matches!(
            from_json.column_by_name("tags").unwrap().data_type,
            ProximaDataType::List { .. }
        ));
        assert_eq!(from_json.vector_dimension(), Some(4));

        assert!(ProximaSchema::from_avro_json("{not json").is_err());

        let missing_array_items = AvroStyleSchema {
            schema_type: "record".to_string(),
            namespace: "n".to_string(),
            name: "bad".to_string(),
            fields: vec![AvroStyleField {
                name: "items".to_string(),
                field_type: AvroStyleType::Complex {
                    type_name: "array".to_string(),
                    items: None,
                    values: None,
                    dimension: None,
                    precision: None,
                    scale: None,
                },
                default: None,
                doc: None,
                column_id: None,
            }],
            aliases: None,
            metadata: HashMap::new(),
        };
        assert!(
            ProximaSchema::from_avro_style(&missing_array_items)
                .unwrap_err()
                .to_string()
                .contains("missing items")
        );

        let missing_map_values = AvroStyleType::Complex {
            type_name: "map".to_string(),
            items: None,
            values: None,
            dimension: None,
            precision: None,
            scale: None,
        };
        assert!(
            missing_map_values
                .to_proxima_type()
                .unwrap_err()
                .to_string()
                .contains("missing values")
        );

        let decimal_missing_scale = AvroStyleType::Complex {
            type_name: "bytes".to_string(),
            items: None,
            values: None,
            dimension: None,
            precision: Some(8),
            scale: None,
        };
        assert!(
            decimal_missing_scale
                .to_proxima_type()
                .unwrap_err()
                .to_string()
                .contains("missing scale")
        );

        assert_eq!(
            AvroStyleType::Union(vec!["null".to_string()])
                .to_proxima_type()
                .unwrap(),
            (ProximaDataType::String, true)
        );
        assert_eq!(
            AvroStyleType::Simple("unknown".to_string())
                .to_proxima_type()
                .unwrap(),
            (ProximaDataType::String, false)
        );
    }

    #[test]
    fn proxima_type_equality_covers_nested_and_vector_variants() {
        assert_eq!(
            ProximaDataType::Struct {
                fields: vec![ProximaColumn {
                    id: 1,
                    name: "a".to_string(),
                    data_type: ProximaDataType::Int32,
                    nullable: false,
                    default_value: None,
                    comment: None,
                    metadata: HashMap::new(),
                    is_deleted: false,
                    original_id: None,
                }]
            },
            ProximaDataType::Struct {
                fields: vec![ProximaColumn {
                    id: 1,
                    name: "a".to_string(),
                    data_type: ProximaDataType::String,
                    nullable: true,
                    default_value: None,
                    comment: None,
                    metadata: HashMap::new(),
                    is_deleted: true,
                    original_id: Some(99),
                }]
            }
        );
        assert_ne!(
            ProximaDataType::Vector {
                dimension: 3,
                element_type: VectorElementType::Float32,
            },
            ProximaDataType::Vector {
                dimension: 3,
                element_type: VectorElementType::Float64,
            }
        );
        assert_eq!(
            ProximaDataType::QuantizedInt8Vector {
                dimension: 4,
                scale_factor: 10,
                zero_point: 0,
            },
            ProximaDataType::QuantizedInt8Vector {
                dimension: 4,
                scale_factor: 10,
                zero_point: 0,
            }
        );
        assert_ne!(
            ProximaDataType::QuantizedPQVector {
                dimension: 8,
                segments: 2,
                bits: 8,
            },
            ProximaDataType::QuantizedPQVector {
                dimension: 8,
                segments: 4,
                bits: 8,
            }
        );
        assert_eq!(
            ProximaDataType::SparseVector {
                max_dimension: None
            },
            ProximaDataType::SparseVector {
                max_dimension: None
            }
        );
    }
}
