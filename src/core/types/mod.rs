/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # ProximaDB Type System
//!
//! This module provides the foundational type system for ProximaRecord,
//! implementing best-in-class data type support with validation, serialization,
//! and Arrow integration.
//!
//! ## Design Principles
//!
//! 1. **SOLID Compliance**:
//!    - **S**ingle Responsibility: Separate traits for validation, serialization, filtering
//!    - **O**pen/Closed: Extensible via trait implementations
//!    - **L**iskov Substitution: All types implement common interfaces
//!    - **I**nterface Segregation: Validatable, Serializable, Filterable traits
//!    - **D**ependency Inversion: Services depend on traits, not concrete types
//!
//! 2. **Type Safety**:
//!    - No silent type coercions
//!    - Validation at insert time
//!    - Explicit conversion functions
//!
//! 3. **Performance**:
//!    - Zero-copy where possible
//!    - Lazy validation options
//!    - Pre-compiled regex patterns
//!
//! ## Supported Types
//!
//! | Type | Arrow Mapping | Description |
//! |------|--------------|-------------|
//! | TEXT | LargeUtf8 | Variable-length UTF-8 text |
//! | TEXT_LARGE | LargeUtf8 | Large text with sidecar storage |
//! | INTEGER | Int64 | 64-bit signed integer |
//! | FLOAT | Float64 | 64-bit floating point |
//! | DECIMAL | Decimal128 | 128-bit decimal (38,18) |
//! | BOOLEAN | Boolean | True/false |
//! | TIMESTAMP | Timestamp | Microseconds since epoch |
//! | TIMESTAMP_TZ | Timestamp + tz | With timezone |
//! | DATE | Date32 | Days since epoch |
//! | TIME | Time64 | Microseconds since midnight |
//! | UUID | FixedSizeBinary(16) | RFC 4122 UUID |
//! | BINARY | Binary | Raw bytes |
//! | JSON | Utf8 | Validated JSON |
//! | ARRAY<T> | List<T> | Homogeneous arrays |
//! | MAP<K,V> | Map<K,V> | Key-value maps |

pub mod validators;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// Re-exports
pub use validators::*;

/// Column data type enumeration with rich type support
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColumnDataType {
    // Text types
    Text,
    TextLarge,

    // Numeric types
    Integer,
    Float,
    Decimal { precision: u8, scale: u8 },

    // Boolean
    Boolean,

    // Temporal types
    Timestamp,
    TimestampTz { timezone: String },
    Date,
    Time,
    Duration,
    Interval,

    // Identifier types
    Uuid,

    // Binary types
    Binary,
    BinaryLarge,

    // Structured types
    Json,

    // Array types
    ArrayText,
    ArrayInteger,
    ArrayFloat,
    ArrayBoolean,
    ArrayUuid,

    // Map types
    MapStringString,
    MapStringAny,
    MapStringInteger,
    MapStringFloat,

    // Geospatial types
    GeoPoint,
    GeoPolygon,

    // Vector types
    Vector { dimension: u32 },
    SparseVector { dimension: u32 },
}

impl ColumnDataType {
    /// Convert to Arrow DataType
    pub fn to_arrow_type(&self) -> arrow_schema::DataType {
        use arrow_schema::{DataType, Field, TimeUnit};

        match self {
            // Text types
            ColumnDataType::Text => DataType::Utf8,
            ColumnDataType::TextLarge => DataType::LargeUtf8,

            // Numeric types
            ColumnDataType::Integer => DataType::Int64,
            ColumnDataType::Float => DataType::Float64,
            ColumnDataType::Decimal { precision, scale } => {
                DataType::Decimal128(*precision, *scale as i8)
            }

            // Boolean
            ColumnDataType::Boolean => DataType::Boolean,

            // Temporal types
            ColumnDataType::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
            ColumnDataType::TimestampTz { timezone } => {
                DataType::Timestamp(TimeUnit::Microsecond, Some(timezone.clone().into()))
            }
            ColumnDataType::Date => DataType::Date32,
            ColumnDataType::Time => DataType::Time64(TimeUnit::Microsecond),
            ColumnDataType::Duration => DataType::Duration(TimeUnit::Microsecond),
            ColumnDataType::Interval => {
                DataType::Interval(arrow_schema::IntervalUnit::MonthDayNano)
            }

            // Identifier types
            ColumnDataType::Uuid => DataType::FixedSizeBinary(16),

            // Binary types
            ColumnDataType::Binary => DataType::Binary,
            ColumnDataType::BinaryLarge => DataType::LargeBinary,

            // Structured types
            ColumnDataType::Json => DataType::Utf8, // JSON stored as validated UTF-8

            // Array types
            ColumnDataType::ArrayText => {
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true)))
            }
            ColumnDataType::ArrayInteger => {
                DataType::List(Arc::new(Field::new("item", DataType::Int64, true)))
            }
            ColumnDataType::ArrayFloat => {
                DataType::List(Arc::new(Field::new("item", DataType::Float64, true)))
            }
            ColumnDataType::ArrayBoolean => {
                DataType::List(Arc::new(Field::new("item", DataType::Boolean, true)))
            }
            ColumnDataType::ArrayUuid => DataType::List(Arc::new(Field::new(
                "item",
                DataType::FixedSizeBinary(16),
                true,
            ))),

            // Map types
            ColumnDataType::MapStringString => DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(
                        vec![
                            Field::new("key", DataType::Utf8, false),
                            Field::new("value", DataType::Utf8, true),
                        ]
                        .into(),
                    ),
                    false,
                )),
                false,
            ),
            ColumnDataType::MapStringAny => DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(
                        vec![
                            Field::new("key", DataType::Utf8, false),
                            Field::new("value", DataType::Utf8, true), // JSON-encoded values
                        ]
                        .into(),
                    ),
                    false,
                )),
                false,
            ),
            ColumnDataType::MapStringInteger => DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(
                        vec![
                            Field::new("key", DataType::Utf8, false),
                            Field::new("value", DataType::Int64, true),
                        ]
                        .into(),
                    ),
                    false,
                )),
                false,
            ),
            ColumnDataType::MapStringFloat => DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(
                        vec![
                            Field::new("key", DataType::Utf8, false),
                            Field::new("value", DataType::Float64, true),
                        ]
                        .into(),
                    ),
                    false,
                )),
                false,
            ),

            // Geospatial types (stored as struct)
            ColumnDataType::GeoPoint => DataType::Struct(
                vec![
                    Field::new("latitude", DataType::Float64, false),
                    Field::new("longitude", DataType::Float64, false),
                    Field::new("altitude", DataType::Float64, true),
                ]
                .into(),
            ),
            ColumnDataType::GeoPolygon => DataType::List(Arc::new(Field::new(
                "point",
                DataType::Struct(
                    vec![
                        Field::new("latitude", DataType::Float64, false),
                        Field::new("longitude", DataType::Float64, false),
                    ]
                    .into(),
                ),
                false,
            ))),

            // Vector types
            ColumnDataType::Vector { dimension } => DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                *dimension as i32,
            ),
            ColumnDataType::SparseVector { .. } => DataType::Struct(
                vec![
                    Field::new(
                        "indices",
                        DataType::List(Arc::new(Field::new("item", DataType::UInt32, false))),
                        false,
                    ),
                    Field::new(
                        "values",
                        DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
                        false,
                    ),
                ]
                .into(),
            ),
        }
    }

    /// Check if this type supports range queries
    pub fn supports_range(&self) -> bool {
        matches!(
            self,
            ColumnDataType::Integer
                | ColumnDataType::Float
                | ColumnDataType::Decimal { .. }
                | ColumnDataType::Timestamp
                | ColumnDataType::TimestampTz { .. }
                | ColumnDataType::Date
                | ColumnDataType::Time
        )
    }

    /// Check if this type supports fulltext search
    pub fn supports_fulltext(&self) -> bool {
        matches!(self, ColumnDataType::Text | ColumnDataType::TextLarge)
    }

    /// Check if this type is a TEXT variant
    pub fn is_text(&self) -> bool {
        matches!(self, ColumnDataType::Text | ColumnDataType::TextLarge)
    }

    /// Get the storage size hint in bytes (0 for variable size)
    pub fn storage_size_hint(&self) -> usize {
        match self {
            ColumnDataType::Integer => 8,
            ColumnDataType::Float => 8,
            ColumnDataType::Decimal { .. } => 16,
            ColumnDataType::Boolean => 1,
            ColumnDataType::Timestamp => 8,
            ColumnDataType::TimestampTz { .. } => 8,
            ColumnDataType::Date => 4,
            ColumnDataType::Time => 8,
            ColumnDataType::Duration => 8,
            ColumnDataType::Interval => 16,
            ColumnDataType::Uuid => 16,
            ColumnDataType::GeoPoint => 24,
            ColumnDataType::Vector { dimension } => (*dimension as usize) * 4,
            _ => 0, // Variable size
        }
    }
}

/// TEXT storage strategy for columnar storage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum TextStorageStrategy {
    /// Store inline in main Parquet column (<4KB)
    Inline,
    /// Split into chunks with embeddings (4KB-1MB)
    Chunked,
    /// Store in separate sidecar file (>1MB)
    Sidecar,
    /// Auto-select based on actual size (default)
    #[default]
    Adaptive,
}


impl TextStorageStrategy {
    /// Size thresholds for adaptive strategy
    pub const INLINE_MAX_SIZE: usize = 4 * 1024; // 4KB
    pub const CHUNKED_MAX_SIZE: usize = 1024 * 1024; // 1MB

    /// Determine strategy based on content size
    pub fn for_size(size: usize) -> Self {
        if size <= Self::INLINE_MAX_SIZE {
            Self::Inline
        } else if size <= Self::CHUNKED_MAX_SIZE {
            Self::Chunked
        } else {
            Self::Sidecar
        }
    }
}

/// Typed column definition with validation constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypedColumnDefinition {
    /// Column name
    pub name: String,
    /// Data type
    pub data_type: ColumnDataType,
    /// Whether null values are allowed
    pub nullable: bool,
    /// Whether to create a secondary index
    pub indexed: bool,
    /// Whether to enable filtering
    pub filterable: bool,
    /// Whether values must be unique
    pub unique: bool,

    /// Validation constraints
    pub constraints: ColumnConstraints,

    /// TEXT-specific options
    pub text_options: Option<TextColumnOptions>,

    /// Description
    pub description: Option<String>,
    /// Custom annotations
    pub annotations: HashMap<String, String>,
}

/// Validation constraints for a column
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColumnConstraints {
    /// Maximum length for TEXT/BINARY
    pub max_length: Option<u32>,
    /// Minimum length for TEXT/BINARY
    pub min_length: Option<u32>,
    /// Minimum value for numeric types
    pub min_value: Option<i64>,
    /// Maximum value for numeric types
    pub max_value: Option<i64>,
    /// Minimum float value
    pub min_float_value: Option<f64>,
    /// Maximum float value
    pub max_float_value: Option<f64>,
    /// Regex pattern for TEXT validation
    pub regex_pattern: Option<String>,
    /// Default value (serialized)
    pub default_value: Option<String>,
    /// Maximum array items
    pub array_max_items: Option<u32>,
    /// Maximum JSON depth
    pub json_max_depth: Option<u32>,
    /// JSON Schema for validation
    pub json_schema: Option<String>,
}

/// TEXT column-specific options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextColumnOptions {
    /// Storage strategy
    pub storage_strategy: TextStorageStrategy,
    /// Enable fulltext index (Tantivy)
    pub enable_fulltext_index: bool,
    /// Enable n-gram bloom filter
    pub enable_ngram_bloom: bool,
    /// N-gram size (default: 3)
    pub ngram_size: u32,
    /// Chunk size for chunked storage
    pub chunk_size: Option<usize>,
}

impl Default for TextColumnOptions {
    fn default() -> Self {
        Self {
            storage_strategy: TextStorageStrategy::Adaptive,
            enable_fulltext_index: false,
            enable_ngram_bloom: false,
            ngram_size: 3,
            chunk_size: None,
        }
    }
}

/// Schema enforcement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum SchemaEnforcementMode {
    /// All columns must match schema exactly
    Strict,
    /// Schema on read, no validation at insert
    Flexible,
    /// Core columns enforced, additional fields allowed
    #[default]
    Hybrid,
}


/// Record schema with enforcement rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordSchema {
    /// Schema ID (UUID)
    pub schema_id: String,
    /// Schema version (semantic versioning)
    pub schema_version: String,
    /// Human-readable name
    pub schema_name: String,
    /// Column definitions
    pub columns: Vec<TypedColumnDefinition>,
    /// Enforcement mode
    pub enforcement_mode: SchemaEnforcementMode,
    /// Allow additional fields in HYBRID mode
    pub allow_additional_fields: bool,
    /// Parent schema ID for evolution
    pub parent_schema_id: Option<String>,
    /// Creation timestamp
    pub created_at: i64,
    /// Creator
    pub created_by: Option<String>,
    /// Description
    pub description: Option<String>,
}

impl RecordSchema {
    /// Create a new schema with default settings
    pub fn new(name: String, columns: Vec<TypedColumnDefinition>) -> Self {
        Self {
            schema_id: uuid::Uuid::new_v4().to_string(),
            schema_version: "1.0.0".to_string(),
            schema_name: name,
            columns,
            enforcement_mode: SchemaEnforcementMode::Hybrid,
            allow_additional_fields: true,
            parent_schema_id: None,
            created_at: chrono::Utc::now().timestamp_millis(),
            created_by: None,
            description: None,
        }
    }

    /// Get column by name
    pub fn get_column(&self, name: &str) -> Option<&TypedColumnDefinition> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Get all TEXT columns
    pub fn text_columns(&self) -> Vec<&TypedColumnDefinition> {
        self.columns
            .iter()
            .filter(|c| c.data_type.is_text())
            .collect()
    }

    /// Validate a value against column constraints
    pub fn validate_value(&self, column_name: &str, value: &TypedValue) -> Result<()> {
        let column = self
            .get_column(column_name)
            .ok_or_else(|| anyhow!("Column '{column_name}' not found in schema"))?;

        // Check null
        if value.is_null() {
            if !column.nullable {
                return Err(anyhow!("Column '{column_name}' does not allow null values"));
            }
            return Ok(());
        }

        // Validate type match
        if !value.matches_type(&column.data_type) {
            return Err(anyhow!(
                "Type mismatch for column '{column_name}': expected {:?}, got {:?}",
                column.data_type,
                value.type_name()
            ));
        }

        // Validate constraints
        value.validate_constraints(&column.constraints)?;

        Ok(())
    }
}

/// Typed value wrapper for ProximaRecord fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypedValue {
    Null,
    Text(String),
    Integer(i64),
    Float(f64),
    Decimal {
        value: i128,
        precision: u8,
        scale: u8,
    },
    Boolean(bool),
    Timestamp(i64),
    TimestampTz {
        timestamp: i64,
        timezone: String,
    },
    Date(i32),
    Time(i64),
    Duration(i64),
    Interval {
        months: i32,
        days: i32,
        nanos: i64,
    },
    Uuid(Vec<u8>),
    Binary(Vec<u8>),
    Json(String),
    ArrayText(Vec<String>),
    ArrayInteger(Vec<i64>),
    ArrayFloat(Vec<f64>),
    ArrayBoolean(Vec<bool>),
    ArrayUuid(Vec<Vec<u8>>),
    MapStringString(HashMap<String, String>),
    MapStringInteger(HashMap<String, i64>),
    MapStringFloat(HashMap<String, f64>),
    GeoPoint {
        latitude: f64,
        longitude: f64,
        altitude: Option<f64>,
    },
    GeoPolygon(Vec<(f64, f64)>),
    Vector(Vec<f32>),
    SparseVector {
        indices: Vec<u32>,
        values: Vec<f32>,
        dimension: u32,
    },
}

impl TypedValue {
    /// Check if value is null
    pub fn is_null(&self) -> bool {
        matches!(self, TypedValue::Null)
    }

    /// Get the type name for error messages
    pub fn type_name(&self) -> &'static str {
        match self {
            TypedValue::Null => "null",
            TypedValue::Text(_) => "text",
            TypedValue::Integer(_) => "integer",
            TypedValue::Float(_) => "float",
            TypedValue::Decimal { .. } => "decimal",
            TypedValue::Boolean(_) => "boolean",
            TypedValue::Timestamp(_) => "timestamp",
            TypedValue::TimestampTz { .. } => "timestamp_tz",
            TypedValue::Date(_) => "date",
            TypedValue::Time(_) => "time",
            TypedValue::Duration(_) => "duration",
            TypedValue::Interval { .. } => "interval",
            TypedValue::Uuid(_) => "uuid",
            TypedValue::Binary(_) => "binary",
            TypedValue::Json(_) => "json",
            TypedValue::ArrayText(_) => "array_text",
            TypedValue::ArrayInteger(_) => "array_integer",
            TypedValue::ArrayFloat(_) => "array_float",
            TypedValue::ArrayBoolean(_) => "array_boolean",
            TypedValue::ArrayUuid(_) => "array_uuid",
            TypedValue::MapStringString(_) => "map_string_string",
            TypedValue::MapStringInteger(_) => "map_string_integer",
            TypedValue::MapStringFloat(_) => "map_string_float",
            TypedValue::GeoPoint { .. } => "geo_point",
            TypedValue::GeoPolygon(_) => "geo_polygon",
            TypedValue::Vector(_) => "vector",
            TypedValue::SparseVector { .. } => "sparse_vector",
        }
    }

    /// Check if value matches the expected column type
    pub fn matches_type(&self, expected: &ColumnDataType) -> bool {
        match (self, expected) {
            (TypedValue::Null, _) => true, // Null matches any type
            (TypedValue::Text(_), ColumnDataType::Text | ColumnDataType::TextLarge) => true,
            (TypedValue::Integer(_), ColumnDataType::Integer) => true,
            (TypedValue::Float(_), ColumnDataType::Float) => true,
            (TypedValue::Decimal { .. }, ColumnDataType::Decimal { .. }) => true,
            (TypedValue::Boolean(_), ColumnDataType::Boolean) => true,
            (TypedValue::Timestamp(_), ColumnDataType::Timestamp) => true,
            (TypedValue::TimestampTz { .. }, ColumnDataType::TimestampTz { .. }) => true,
            (TypedValue::Date(_), ColumnDataType::Date) => true,
            (TypedValue::Time(_), ColumnDataType::Time) => true,
            (TypedValue::Duration(_), ColumnDataType::Duration) => true,
            (TypedValue::Interval { .. }, ColumnDataType::Interval) => true,
            (TypedValue::Uuid(_), ColumnDataType::Uuid) => true,
            (TypedValue::Binary(_), ColumnDataType::Binary | ColumnDataType::BinaryLarge) => true,
            (TypedValue::Json(_), ColumnDataType::Json) => true,
            (TypedValue::ArrayText(_), ColumnDataType::ArrayText) => true,
            (TypedValue::ArrayInteger(_), ColumnDataType::ArrayInteger) => true,
            (TypedValue::ArrayFloat(_), ColumnDataType::ArrayFloat) => true,
            (TypedValue::ArrayBoolean(_), ColumnDataType::ArrayBoolean) => true,
            (TypedValue::ArrayUuid(_), ColumnDataType::ArrayUuid) => true,
            (TypedValue::MapStringString(_), ColumnDataType::MapStringString) => true,
            (TypedValue::MapStringInteger(_), ColumnDataType::MapStringInteger) => true,
            (TypedValue::MapStringFloat(_), ColumnDataType::MapStringFloat) => true,
            (TypedValue::GeoPoint { .. }, ColumnDataType::GeoPoint) => true,
            (TypedValue::GeoPolygon(_), ColumnDataType::GeoPolygon) => true,
            (TypedValue::Vector(_), ColumnDataType::Vector { .. }) => true,
            (TypedValue::SparseVector { .. }, ColumnDataType::SparseVector { .. }) => true,
            _ => false,
        }
    }

    /// Validate against column constraints
    pub fn validate_constraints(&self, constraints: &ColumnConstraints) -> Result<()> {
        match self {
            TypedValue::Text(s) | TypedValue::Json(s) => {
                if let Some(max) = constraints.max_length
                    && s.len() > max as usize {
                        return Err(anyhow!("Text length {} exceeds maximum {}", s.len(), max));
                    }
                if let Some(min) = constraints.min_length
                    && s.len() < min as usize {
                        return Err(anyhow!(
                            "Text length {} is less than minimum {}",
                            s.len(),
                            min
                        ));
                    }
                if let Some(pattern) = &constraints.regex_pattern {
                    let re = regex::Regex::new(pattern)
                        .map_err(|e| anyhow!("Invalid regex pattern: {e}"))?;
                    if !re.is_match(s) {
                        return Err(anyhow!("Value '{s}' does not match pattern '{pattern}'"));
                    }
                }
            }
            TypedValue::Integer(v) => {
                if let Some(min) = constraints.min_value
                    && *v < min {
                        return Err(anyhow!("Value {v} is less than minimum {min}"));
                    }
                if let Some(max) = constraints.max_value
                    && *v > max {
                        return Err(anyhow!("Value {v} exceeds maximum {max}"));
                    }
            }
            TypedValue::Float(v) => {
                if let Some(min) = constraints.min_float_value
                    && *v < min {
                        return Err(anyhow!("Value {v} is less than minimum {min}"));
                    }
                if let Some(max) = constraints.max_float_value
                    && *v > max {
                        return Err(anyhow!("Value {v} exceeds maximum {max}"));
                    }
            }
            TypedValue::Binary(b) => {
                if let Some(max) = constraints.max_length
                    && b.len() > max as usize {
                        return Err(anyhow!("Binary length {} exceeds maximum {max}", b.len()));
                    }
            }
            TypedValue::ArrayText(_) | TypedValue::ArrayInteger(_) | TypedValue::ArrayFloat(_) => {
                if let Some(max) = constraints.array_max_items {
                    let len = match self {
                        TypedValue::ArrayText(a) => a.len(),
                        TypedValue::ArrayInteger(a) => a.len(),
                        TypedValue::ArrayFloat(a) => a.len(),
                        TypedValue::ArrayBoolean(a) => a.len(),
                        TypedValue::ArrayUuid(a) => a.len(),
                        _ => 0,
                    };
                    if len > max as usize {
                        return Err(anyhow!("Array length {} exceeds maximum {}", len, max));
                    }
                }
                // Validate array text for Text arrays
                if let TypedValue::ArrayText(arr) = self {
                    for s in arr {
                        if let Some(max) = constraints.max_length
                            && s.len() > max as usize {
                                return Err(anyhow!(
                                    "Array element length {} exceeds maximum {}",
                                    s.len(),
                                    max
                                ));
                            }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Text field for dedicated columnar storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextField {
    /// Field name
    pub name: String,
    /// Text content
    pub content: String,
    /// Storage strategy hint
    pub storage_hint: TextStorageStrategy,
    /// Number of chunks (populated after storage)
    pub chunk_count: Option<u32>,
    /// Reference to chunks sidecar file
    pub chunk_reference: Option<String>,
}

impl TextField {
    /// Create a new text field
    pub fn new(name: String, content: String) -> Self {
        let storage_hint = TextStorageStrategy::for_size(content.len());
        Self {
            name,
            content,
            storage_hint,
            chunk_count: None,
            chunk_reference: None,
        }
    }

    /// Create with explicit storage strategy
    pub fn with_strategy(name: String, content: String, strategy: TextStorageStrategy) -> Self {
        Self {
            name,
            content,
            storage_hint: strategy,
            chunk_count: None,
            chunk_reference: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_data_type_to_arrow() {
        let text_type = ColumnDataType::Text;
        assert!(matches!(
            text_type.to_arrow_type(),
            arrow_schema::DataType::Utf8
        ));

        let int_type = ColumnDataType::Integer;
        assert!(matches!(
            int_type.to_arrow_type(),
            arrow_schema::DataType::Int64
        ));

        let decimal_type = ColumnDataType::Decimal {
            precision: 38,
            scale: 18,
        };
        assert!(matches!(
            decimal_type.to_arrow_type(),
            arrow_schema::DataType::Decimal128(38, 18)
        ));

        let uuid_type = ColumnDataType::Uuid;
        assert!(matches!(
            uuid_type.to_arrow_type(),
            arrow_schema::DataType::FixedSizeBinary(16)
        ));
    }

    #[test]
    fn test_text_storage_strategy() {
        assert_eq!(
            TextStorageStrategy::for_size(100),
            TextStorageStrategy::Inline
        );
        assert_eq!(
            TextStorageStrategy::for_size(5000),
            TextStorageStrategy::Chunked
        );
        assert_eq!(
            TextStorageStrategy::for_size(2_000_000),
            TextStorageStrategy::Sidecar
        );
    }

    #[test]
    fn test_typed_value_matches_type() {
        let text_value = TypedValue::Text("hello".to_string());
        assert!(text_value.matches_type(&ColumnDataType::Text));
        assert!(text_value.matches_type(&ColumnDataType::TextLarge));
        assert!(!text_value.matches_type(&ColumnDataType::Integer));

        let null_value = TypedValue::Null;
        assert!(null_value.matches_type(&ColumnDataType::Text));
        assert!(null_value.matches_type(&ColumnDataType::Integer));
    }

    #[test]
    fn test_constraint_validation() {
        let constraints = ColumnConstraints {
            max_length: Some(10),
            min_length: Some(2),
            ..Default::default()
        };

        let valid_text = TypedValue::Text("hello".to_string());
        assert!(valid_text.validate_constraints(&constraints).is_ok());

        let too_long = TypedValue::Text("hello world!".to_string());
        assert!(too_long.validate_constraints(&constraints).is_err());

        let too_short = TypedValue::Text("a".to_string());
        assert!(too_short.validate_constraints(&constraints).is_err());
    }

    #[test]
    fn test_uuid_validation() {
        let uuid_type = ColumnDataType::Uuid;
        assert_eq!(uuid_type.storage_size_hint(), 16);

        let uuid_value = TypedValue::Uuid(vec![0u8; 16]);
        assert!(uuid_value.matches_type(&uuid_type));
    }
}
