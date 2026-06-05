//! # ProximaSchema — alias for the canonical catalog schema (ADR-024 Step 4)
//!
//! Historically this module defined two storage-plane schema structs —
//! `ProximaSchema` and `ProximaColumn` — that ran in parallel with the
//! catalog's durable `CatalogTableSchema`/`CatalogColumn`. ADR-024 Step 4
//! MERGED them: the catalog structs are now the single durable authority and
//! absorbed the storage-plane evolution fields (`is_deleted`, `original_id`,
//! `schema_id`, `version`, `fingerprint`, …) plus the Arrow/fingerprint/lookup
//! methods.
//!
//! `ProximaSchema` and `ProximaColumn` are therefore now **type aliases** for
//! `CatalogTableSchema`/`CatalogColumn`, so the ~19 storage-plane consumers keep
//! compiling unchanged. The Avro-style serialization surface (which references
//! types that live in the storage layer and therefore cannot move into the
//! control-plane catalog crate without inverting the dependency arrow) stays
//! here, expressed as an extension trait ([`ProximaSchemaAvroExt`]) over the
//! alias.

use anyhow::{Context, Result, anyhow};
use proximadb_data_model::{ProximaType, VectorElement as DmVectorElement};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// ADR-024 Step 4: the storage-plane schema is now the canonical catalog
/// schema. One struct, one durable authority.
pub use proximadb_catalog::{CatalogColumn as ProximaColumn, CatalogTableSchema as ProximaSchema};

/// Time unit for temporal types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeUnit {
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
}

/// Default value for columns.
///
/// Retained as a storage-plane descriptor for schema-evolution operations
/// (the canonical [`ProximaColumn::default_value`] is a rendered `String`; this
/// enum is the producer side — see [`DefaultValue::render`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DefaultValue {
    /// Literal value (stored as JSON)
    Literal(String),
    /// SQL expression
    Expression(String),
    /// Auto-generated (UUID, timestamp, sequence)
    AutoGenerate(AutoGenerateType),
}

impl DefaultValue {
    /// Render to the canonical [`ProximaColumn::default_value`] string form.
    ///
    /// ADR-024 Step 4 reconciliation: the unified column stores a single
    /// `Option<String>` default (the catalog's canonical SQL-literal form),
    /// while schema-evolution ops still carry the richer [`DefaultValue`] enum.
    /// This renders the enum to that string: literals/expressions pass through,
    /// and auto-generators render to their canonical SQL keyword.
    pub fn render(&self) -> String {
        match self {
            DefaultValue::Literal(s) | DefaultValue::Expression(s) => s.clone(),
            DefaultValue::AutoGenerate(g) => match g {
                AutoGenerateType::Uuid => "gen_random_uuid()".to_string(),
                AutoGenerateType::CurrentTimestamp => "CURRENT_TIMESTAMP".to_string(),
                AutoGenerateType::Sequence { start, increment } => {
                    format!("SEQUENCE(start={start}, increment={increment})")
                }
            },
        }
    }
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

/// Avro-style schema representation for [`ProximaSchema`].
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

/// Avro-style (de)serialization for the unified [`ProximaSchema`].
///
/// Ported from the deleted inherent `ProximaSchema`/`ProximaColumn` Avro
/// methods. Lives in the storage crate (not the catalog crate) because the
/// `AvroStyle*` carriers are a storage-plane concern; catalog (control) must
/// not depend on storage.
pub trait ProximaSchemaAvroExt: Sized {
    /// Serialize to the Avro-style schema representation.
    fn to_avro_style(&self) -> AvroStyleSchema;
    /// Serialize to a pretty Avro-style JSON string.
    fn to_avro_json(&self) -> Result<String>;
    /// Deserialize from an Avro-style schema representation.
    fn from_avro_style(avro: &AvroStyleSchema) -> Result<Self>;
    /// Deserialize from an Avro-style JSON string.
    fn from_avro_json(json: &str) -> Result<Self>;
}

impl ProximaSchemaAvroExt for ProximaSchema {
    fn to_avro_style(&self) -> AvroStyleSchema {
        let fields: Vec<AvroStyleField> = self
            .columns
            .iter()
            .filter(|c| !c.is_deleted)
            .map(column_to_avro_field)
            .collect();

        AvroStyleSchema {
            schema_type: "record".to_string(),
            namespace: "com.proximadb".to_string(),
            name: self.schema_id.clone(),
            fields,
            aliases: None,
            metadata: self.properties.clone(),
        }
    }

    fn to_avro_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.to_avro_style())
            .context("Failed to serialize schema to Avro-style JSON")
    }

    fn from_avro_style(avro: &AvroStyleSchema) -> Result<Self> {
        let columns: Vec<ProximaColumn> = avro
            .fields
            .iter()
            .enumerate()
            .map(|(idx, field)| avro_field_to_column(field, (idx + 1) as i32))
            .collect::<Result<Vec<_>>>()?;

        // Default the primary key to the first column (the original behavior:
        // the storage-side `from_avro_style` defaulted the PK to the first
        // column). Resolve it through the first column's actual id so the
        // by-name `primary_key` lands on a real column.
        let first_id = columns.first().map(|c| c.id);
        let mut schema =
            ProximaSchema::from_columns(avro.name.clone(), columns, first_id.into_iter().collect());
        schema.properties = avro.metadata.clone();
        Ok(schema)
    }

    fn from_avro_json(json: &str) -> Result<Self> {
        let avro: AvroStyleSchema =
            serde_json::from_str(json).context("Failed to parse Avro-style JSON")?;
        Self::from_avro_style(&avro)
    }
}

/// Convert one [`ProximaColumn`] to its Avro-style field.
fn column_to_avro_field(col: &ProximaColumn) -> AvroStyleField {
    let field_type = proxima_type_to_avro_type(&col.data_type, col.nullable);

    AvroStyleField {
        name: col.name.clone(),
        field_type,
        // The canonical column default is a rendered string. Try to interpret
        // it as JSON (matching the old `Literal` JSON round-trip); fall back to
        // a JSON string.
        default: col
            .default_value
            .as_ref()
            .map(|s| serde_json::from_str(s).unwrap_or_else(|_| JsonValue::String(s.clone()))),
        doc: col.comment.clone(),
        column_id: Some(col.id),
    }
}

/// Convert a canonical [`ProximaType`] to an Avro-style type descriptor.
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

/// Convert an Avro-style field to a [`ProximaColumn`].
fn avro_field_to_column(field: &AvroStyleField, default_id: i32) -> Result<ProximaColumn> {
    let (data_type, nullable) = avro_type_to_proxima_type(&field.field_type)?;

    Ok(ProximaColumn {
        id: field.column_id.unwrap_or(default_id),
        name: field.name.clone(),
        data_type,
        nullable,
        // Canonical default is the rendered string form.
        default_value: field.default.as_ref().map(|v| v.to_string()),
        comment: field.doc.clone(),
        properties: HashMap::new(),
        is_deleted: false,
        original_id: None,
    })
}

/// Convert an Avro-style type to the canonical [`ProximaType`].
fn avro_type_to_proxima_type(ty: &AvroStyleType) -> Result<(ProximaType, bool)> {
    match ty {
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
                let (dtype, _) = avro_type_to_proxima_type(&AvroStyleType::Simple((*t).clone()))?;
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
                        .map(|i| avro_type_to_proxima_type(i).map(|(t, _)| t))
                        .transpose()?
                        .ok_or_else(|| anyhow!("Array type missing items specification"))?;
                    ProximaType::Array(Box::new(element))
                }
                "map" => {
                    let value = values
                        .as_ref()
                        .map(|v| avro_type_to_proxima_type(v).map(|(t, _)| t))
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

impl TimeUnit {
    /// Convert to Arrow TimeUnit.
    pub fn to_arrow_time_unit(&self) -> arrow_schema::TimeUnit {
        match self {
            TimeUnit::Second => arrow_schema::TimeUnit::Second,
            TimeUnit::Millisecond => arrow_schema::TimeUnit::Millisecond,
            TimeUnit::Microsecond => arrow_schema::TimeUnit::Microsecond,
            TimeUnit::Nanosecond => arrow_schema::TimeUnit::Nanosecond,
        }
    }

    /// Convert from Arrow TimeUnit.
    pub fn from_arrow(unit: arrow_schema::TimeUnit) -> Self {
        match unit {
            arrow_schema::TimeUnit::Second => TimeUnit::Second,
            arrow_schema::TimeUnit::Millisecond => TimeUnit::Millisecond,
            arrow_schema::TimeUnit::Microsecond => TimeUnit::Microsecond,
            arrow_schema::TimeUnit::Nanosecond => TimeUnit::Nanosecond,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{DataType as ArrowDataType, TimeUnit as ArrowTimeUnit};
    use proximadb_data_model::TimeUnit as DmTimeUnit;

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
            properties: HashMap::new(),
            is_deleted: true,
            original_id: Some(4),
        });

        assert!(!schema.is_legacy_vector_record);
        assert_eq!(schema.primary_key_ids(), vec![1]);
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
                default_value: Some("\"p1\"".to_string()),
                comment: Some("primary id".to_string()),
                properties: HashMap::from([("encoding".to_string(), "plain".to_string())]),
                is_deleted: false,
                original_id: None,
            },
            ProximaColumn {
                id: 11,
                name: "tags".to_string(),
                data_type: ProximaType::Array(Box::new(ProximaType::String)),
                nullable: true,
                default_value: Some("array[]".to_string()),
                comment: None,
                properties: HashMap::new(),
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
                default_value: None,
                comment: None,
                properties: HashMap::new(),
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
                properties: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
        ];
        let mut schema =
            ProximaSchema::from_columns("catalog.products".to_string(), columns, vec![10]);
        schema
            .properties
            .insert("owner".to_string(), "catalog".to_string());

        let avro = schema.to_avro_style();
        assert_eq!(avro.schema_type, "record");
        assert_eq!(avro.namespace, "com.proximadb");
        assert_eq!(avro.fields.len(), 4);
        assert_eq!(avro.fields[0].default, Some(serde_json::json!("p1")));
        assert_eq!(avro.fields[1].default, Some(serde_json::json!("array[]")));
        assert_eq!(avro.metadata.get("owner"), Some(&"catalog".to_string()));

        let json = schema.to_avro_json().unwrap();
        let from_json = ProximaSchema::from_avro_json(&json).unwrap();
        assert_eq!(from_json.schema_id, "catalog.products");
        // PK defaults to the first column; the avro fields carry their original
        // column ids (10..13), so the first column id is 10.
        assert_eq!(from_json.primary_key_ids(), vec![10]);
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
            avro_type_to_proxima_type(&missing_map_values)
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
            avro_type_to_proxima_type(&decimal_missing_scale)
                .unwrap_err()
                .to_string()
                .contains("missing scale")
        );

        assert_eq!(
            avro_type_to_proxima_type(&AvroStyleType::Union(vec!["null".to_string()])).unwrap(),
            (ProximaType::String, true)
        );
        assert_eq!(
            avro_type_to_proxima_type(&AvroStyleType::Simple("unknown".to_string())).unwrap(),
            (ProximaType::String, false)
        );
    }
}
