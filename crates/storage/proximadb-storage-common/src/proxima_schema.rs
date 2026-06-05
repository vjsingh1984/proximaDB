//! # ProximaSchema - Arrow-Native Schema Definition
//!
//! Unified schema replacing VectorRecord for compute engine compatibility.
//! Provides stable column IDs, schema evolution, and Arrow conversion.

use anyhow::{Context, Result, anyhow};
use arrow_schema::{Field, Schema as ArrowSchema, TimeUnit as ArrowTimeUnit};
use proximadb_data_model::{ProximaType, TimeUnit as DmTimeUnit, VectorElement as DmVectorElement};
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

    /// Data type (canonical ProximaDB logical type, ADR-024)
    pub data_type: proximadb_data_model::ProximaType,

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
                data_type: ProximaType::String,
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
                data_type: ProximaType::DenseVector {
                    element: DmVectorElement::Float32,
                    dim: dimension as usize,
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
                data_type: ProximaType::Json,
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
                data_type: ProximaType::Timestamp(DmTimeUnit::Millisecond),
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
                data_type: ProximaType::Int64,
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
        metadata_fields: Vec<(String, ProximaType)>,
    ) -> Self {
        let mut columns = vec![
            ProximaColumn {
                id: 1,
                name: "id".to_string(),
                data_type: ProximaType::String,
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
                data_type: ProximaType::DenseVector {
                    element: DmVectorElement::Float32,
                    dim: dimension as usize,
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
                data_type: ProximaType::Timestamp(DmTimeUnit::Millisecond),
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
            if let ProximaType::DenseVector { dim, .. } = &col.data_type {
                return Some(*dim as u32);
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
            data_type: ProximaType::from_arrow_type(field.data_type()),
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
        let field_type = proxima_type_to_avro_type(&self.data_type, self.nullable);

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

/// Convert a canonical [`ProximaType`] to an Avro-style type descriptor.
///
/// Free function (ADR-024): the Arrow projection now lives canonically on
/// [`ProximaType::to_arrow_type`]; only this Avro projection remains local to
/// the storage schema descriptor. Logical types without a distinct Avro carrier
/// fall back to their nearest representation (the logical type stays
/// authoritative on the column itself).
fn proxima_type_to_avro_type(ty: &ProximaType, nullable: bool) -> AvroStyleType {
    let base_type = match ty {
        ProximaType::Boolean => AvroStyleType::Simple("boolean".to_string()),
        ProximaType::Int8 | ProximaType::Int16 | ProximaType::Int32 => {
            AvroStyleType::Simple("int".to_string())
        }
        ProximaType::Int64 => AvroStyleType::Simple("long".to_string()),
        ProximaType::UInt8 | ProximaType::UInt16 | ProximaType::UInt32 => {
            AvroStyleType::Simple("int".to_string())
        }
        ProximaType::UInt64 => AvroStyleType::Simple("long".to_string()),
        ProximaType::Float16 | ProximaType::Float32 => AvroStyleType::Simple("float".to_string()),
        ProximaType::Float64 => AvroStyleType::Simple("double".to_string()),
        ProximaType::Decimal { precision, scale } => AvroStyleType::Complex {
            type_name: "bytes".to_string(),
            items: None,
            values: None,
            dimension: None,
            precision: Some(*precision),
            scale: Some(*scale as i8),
        },
        ProximaType::String
        | ProximaType::Symbol
        | ProximaType::Uuid
        | ProximaType::ULID
        | ProximaType::Json
        | ProximaType::Jsonb => AvroStyleType::Simple("string".to_string()),
        ProximaType::Binary => AvroStyleType::Simple("bytes".to_string()),
        ProximaType::Date => AvroStyleType::Simple("int".to_string()),
        ProximaType::Time(_)
        | ProximaType::Timestamp(_)
        | ProximaType::TimestampTz(_)
        | ProximaType::Interval(_)
        | ProximaType::Duration(_) => AvroStyleType::Simple("long".to_string()),
        ProximaType::Array(element) => AvroStyleType::Complex {
            type_name: "array".to_string(),
            items: Some(Box::new(proxima_type_to_avro_type(element, true))),
            values: None,
            dimension: None,
            precision: None,
            scale: None,
        },
        ProximaType::Map { key: _, value } => AvroStyleType::Complex {
            type_name: "map".to_string(),
            items: None,
            values: Some(Box::new(proxima_type_to_avro_type(value, true))),
            dimension: None,
            precision: None,
            scale: None,
        },
        ProximaType::Struct { .. } => AvroStyleType::Simple("string".to_string()),
        ProximaType::DenseVector { dim, .. } => AvroStyleType::Complex {
            type_name: "fixed".to_string(),
            items: None,
            values: None,
            dimension: Some(*dim as u32),
            precision: None,
            scale: None,
        },
        ProximaType::SparseVector { .. } => AvroStyleType::Simple("bytes".to_string()),
        ProximaType::BinaryVector { dim } => AvroStyleType::Complex {
            type_name: "fixed".to_string(),
            items: None,
            values: None,
            dimension: Some(*dim as u32),
            precision: None,
            scale: None,
        },
        ProximaType::Point | ProximaType::GeographyPoint | ProximaType::Null => {
            AvroStyleType::Simple("bytes".to_string())
        }
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
    /// Convert to the canonical [`ProximaType`].
    fn to_proxima_type(&self) -> Result<(ProximaType, bool)> {
        match self {
            AvroStyleType::Simple(t) => {
                let dtype = match t.as_str() {
                    "null" => ProximaType::String,
                    "boolean" => ProximaType::Boolean,
                    "int" => ProximaType::Int32,
                    "long" => ProximaType::Int64,
                    "float" => ProximaType::Float32,
                    "double" => ProximaType::Float64,
                    "bytes" => ProximaType::Binary,
                    "string" => ProximaType::String,
                    _ => ProximaType::String,
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
                    Ok((ProximaType::String, true))
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
                        ProximaType::Array(Box::new(element))
                    }
                    "map" => {
                        let value = values
                            .as_ref()
                            .map(|v| v.to_proxima_type().map(|(t, _)| t))
                            .transpose()?
                            .ok_or_else(|| anyhow!("Map type missing values specification"))?;
                        ProximaType::Map {
                            key: Box::new(ProximaType::String),
                            value: Box::new(value),
                        }
                    }
                    "fixed" => {
                        if let Some(dim) = dimension {
                            ProximaType::DenseVector {
                                element: DmVectorElement::Float32,
                                dim: *dim as usize,
                            }
                        } else {
                            ProximaType::Binary
                        }
                    }
                    "bytes" if precision.is_some() => ProximaType::Decimal {
                        precision: precision
                            .ok_or_else(|| anyhow!("Decimal type missing precision"))?,
                        scale: scale.ok_or_else(|| anyhow!("Decimal type missing scale"))? as u8,
                    },
                    _ => ProximaType::String,
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
    use arrow_schema::DataType as ArrowDataType;

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
                ("category".to_string(), ProximaType::String),
                ("score".to_string(), ProximaType::Float64),
            ],
        );
        schema.columns.push(ProximaColumn {
            id: 99,
            name: "deleted".to_string(),
            data_type: ProximaType::Boolean,
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
    fn proxima_type_arrow_mapping_covers_primitives_complex_vectors_and_fallbacks() {
        // Canonical mapping lives on `ProximaType::to_arrow_type` (ADR-024).
        // `ProximaColumn::to_arrow_field` routes through it; assert via that.
        let cases = vec![
            (ProximaType::Boolean, ArrowDataType::Boolean),
            (ProximaType::Int8, ArrowDataType::Int8),
            (ProximaType::Int16, ArrowDataType::Int16),
            (ProximaType::Int32, ArrowDataType::Int32),
            (ProximaType::Int64, ArrowDataType::Int64),
            (ProximaType::UInt8, ArrowDataType::UInt8),
            (ProximaType::UInt16, ArrowDataType::UInt16),
            (ProximaType::UInt32, ArrowDataType::UInt32),
            (ProximaType::UInt64, ArrowDataType::UInt64),
            (ProximaType::Float32, ArrowDataType::Float32),
            (ProximaType::Float64, ArrowDataType::Float64),
            (
                ProximaType::Decimal {
                    precision: 12,
                    scale: 2,
                },
                ArrowDataType::Decimal128(12, 2),
            ),
            (ProximaType::String, ArrowDataType::Utf8),
            (ProximaType::Binary, ArrowDataType::Binary),
            (ProximaType::Date, ArrowDataType::Date32),
            (
                ProximaType::Time(DmTimeUnit::Nanosecond),
                ArrowDataType::Time64(ArrowTimeUnit::Nanosecond),
            ),
            (
                ProximaType::TimestampTz(DmTimeUnit::Microsecond),
                ArrowDataType::Timestamp(ArrowTimeUnit::Microsecond, Some("UTC".into())),
            ),
            // Canonical change (ADR-024): UUID is a 16-byte identifier, not Utf8.
            (ProximaType::Uuid, ArrowDataType::FixedSizeBinary(16)),
            (ProximaType::Json, ArrowDataType::Utf8),
            (
                ProximaType::DenseVector {
                    element: DmVectorElement::Float32,
                    dim: 3,
                },
                ArrowDataType::FixedSizeBinary(12),
            ),
            (
                ProximaType::BinaryVector { dim: 17 },
                ArrowDataType::FixedSizeBinary(3),
            ),
        ];

        for (proxima, arrow) in cases {
            assert_eq!(proxima.to_arrow_type(), arrow);
        }

        assert!(matches!(
            ProximaType::Array(Box::new(ProximaType::String)).to_arrow_type(),
            ArrowDataType::List(_)
        ));
        assert!(matches!(
            ProximaType::Map {
                key: Box::new(ProximaType::String),
                value: Box::new(ProximaType::Float64)
            }
            .to_arrow_type(),
            ArrowDataType::Map(_, false)
        ));
        assert!(matches!(
            ProximaType::Struct {
                fields: vec![("nested".to_string(), ProximaType::Int32)]
            }
            .to_arrow_type(),
            ArrowDataType::Struct(_)
        ));
        assert!(matches!(
            ProximaType::SparseVector {
                element: DmVectorElement::Float32
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
            ProximaType::from_arrow_type(&ArrowDataType::LargeUtf8),
            ProximaType::String
        );
        assert_eq!(
            ProximaType::from_arrow_type(&ArrowDataType::LargeBinary),
            ProximaType::Binary
        );
        assert_eq!(
            ProximaType::from_arrow_type(&ArrowDataType::Date64),
            ProximaType::Date
        );
        assert_eq!(
            ProximaType::from_arrow_type(&ArrowDataType::Decimal256(20, 4)),
            ProximaType::Decimal {
                precision: 20,
                scale: 4
            }
        );
        assert_eq!(
            ProximaType::from_arrow_type(&ArrowDataType::FixedSizeBinary(3)),
            ProximaType::Binary
        );
        assert_eq!(
            ProximaType::from_arrow_type(&ArrowDataType::Duration(ArrowTimeUnit::Second)),
            ProximaType::Duration(DmTimeUnit::Second)
        );
    }

    #[test]
    fn avro_style_roundtrip_defaults_and_error_paths_are_covered() {
        let columns = vec![
            ProximaColumn {
                id: 10,
                name: "id".to_string(),
                data_type: ProximaType::String,
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
                data_type: ProximaType::Array(Box::new(ProximaType::String)),
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
                data_type: ProximaType::DenseVector {
                    element: DmVectorElement::Float32,
                    dim: 4,
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
                data_type: ProximaType::Decimal {
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
            ProximaType::Array(_)
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
            (ProximaType::String, true)
        );
        assert_eq!(
            AvroStyleType::Simple("unknown".to_string())
                .to_proxima_type()
                .unwrap(),
            (ProximaType::String, false)
        );
    }
}
