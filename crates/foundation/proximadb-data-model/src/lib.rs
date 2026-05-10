/*
 * Copyright 2025 Vijaykumar Singh
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

//! # ProximaDB Unified Data Model — Phase A (TD-053).
//!
//! Authoritative definition of the ProximaDB type system and model discriminators
//! as specified in MULTIMODAL_OVERHAUL_SPEC_2026_05_08.

use arrow_schema::DataType as ArrowDataType;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Modality Discriminators
// ---------------------------------------------------------------------------

/// Modality discriminator for multi-model routing.
///
/// Matches Section 3.3 of the Overhaul Spec. Every record in ProximaDB projects
/// onto the same envelope but carries one of these modality tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataModel {
    /// Vector similarity search.
    Vector,
    /// JSON document store.
    Document,
    /// Property graph (vertices and edges).
    Graph,
    /// Metrics, logs, traces.
    Observability,
    /// Standard relational tables.
    Relational,
    /// Time-series sample data.
    TimeSeries,
    /// Append-only event or audit log data.
    Event,
}

impl std::fmt::Display for DataModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataModel::Vector => write!(f, "vector"),
            DataModel::Document => write!(f, "document"),
            DataModel::Graph => write!(f, "graph"),
            DataModel::Observability => write!(f, "observability"),
            DataModel::Relational => write!(f, "relational"),
            DataModel::TimeSeries => write!(f, "timeseries"),
            DataModel::Event => write!(f, "event"),
        }
    }
}

/// Legacy alias for [`DataModel`]. Use `DataModel` in new code.
#[deprecated(
    since = "0.2.0",
    note = "Renamed to DataModel per Overhaul Spec Section 3.3"
)]
pub type StoreType = DataModel;

// ---------------------------------------------------------------------------
// Typed Semantic Memory (Memanto — TD-055)
// ---------------------------------------------------------------------------

/// Standardized memory categories for agentic memory systems.
///
/// The 13 categories are intentionally flat so records can be filtered by
/// memory intent without inventing source-specific metadata conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    /// Objective, verifiable information.
    Fact,
    /// User or system preference.
    Preference,
    /// Choice made that affects future behavior.
    Decision,
    /// Promise, obligation, or scheduled responsibility.
    Commitment,
    /// Objective to achieve.
    Goal,
    /// Historical occurrence.
    Event,
    /// Rule or guideline.
    Instruction,
    /// Entity connection.
    Relationship,
    /// Situational information.
    Context,
    /// Lesson learned from experience.
    Learning,
    /// Pattern or signal noticed.
    Observation,
    /// Mistake, failure, or constraint to avoid.
    Error,
    /// Document, code, or external reference.
    Artifact,
}

impl MemoryType {
    /// Stable lowercase name used in metadata, JSON, and AQL type filters.
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryType::Fact => "fact",
            MemoryType::Preference => "preference",
            MemoryType::Decision => "decision",
            MemoryType::Commitment => "commitment",
            MemoryType::Goal => "goal",
            MemoryType::Event => "event",
            MemoryType::Instruction => "instruction",
            MemoryType::Relationship => "relationship",
            MemoryType::Context => "context",
            MemoryType::Learning => "learning",
            MemoryType::Observation => "observation",
            MemoryType::Error => "error",
            MemoryType::Artifact => "artifact",
        }
    }
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryType {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fact" => Ok(MemoryType::Fact),
            "preference" => Ok(MemoryType::Preference),
            "decision" => Ok(MemoryType::Decision),
            "commitment" => Ok(MemoryType::Commitment),
            "goal" => Ok(MemoryType::Goal),
            "event" => Ok(MemoryType::Event),
            "instruction" => Ok(MemoryType::Instruction),
            "relationship" => Ok(MemoryType::Relationship),
            "context" => Ok(MemoryType::Context),
            "learning" => Ok(MemoryType::Learning),
            "observation" => Ok(MemoryType::Observation),
            "error" => Ok(MemoryType::Error),
            "artifact" => Ok(MemoryType::Artifact),
            _ => Err(()),
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical Type System (ProximaType)
// ---------------------------------------------------------------------------

/// The canonical scalar and complex types for ProximaDB.
///
/// Implements Section 4.2 of the Overhaul Spec. This tree is the single
/// source of truth for catalog, wire, and storage types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProximaType {
    Boolean,
    // Numeric
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float16,
    Float32,
    Float64,
    Decimal {
        precision: u8,
        scale: u8,
    },
    // Text
    String,
    Symbol,
    // Binary
    Binary,
    // Temporal
    Date,
    Time(TimeUnit),
    Timestamp(TimeUnit),
    TimestampTz(TimeUnit),
    Interval(TimeUnit),
    Duration(TimeUnit),
    // Identifier
    Uuid,
    ULID,
    // Structured
    Json,
    Jsonb,
    Array(Box<ProximaType>),
    Map {
        key: Box<ProximaType>,
        value: Box<ProximaType>,
    },
    Struct {
        fields: Vec<(String, ProximaType)>,
    },
    // Vector (First-class modality)
    DenseVector {
        element: VectorElement,
        dim: usize,
    },
    SparseVector {
        element: VectorElement,
    },
    BinaryVector {
        dim: usize,
    },
    // Geo
    Point,
    GeographyPoint,
    // Special
    Null,
}

/// Time units for temporal types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeUnit {
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
}

/// Supported element types for vector modalities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorElement {
    Float16,
    Float32,
    Float64,
    Int8,
}

// ---------------------------------------------------------------------------
// Unified Value System (ProximaValue)
// ---------------------------------------------------------------------------

/// The unified scalar and complex value container.
///
/// Every property in a [`ProximaRecord`] and every value in the logical
/// algebra projects onto this type. Replaces legacy `SqlValue` and `PropertyValue`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProximaValue {
    Boolean(bool),
    // Numeric
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    Float16(f32), // Represented as f32 in-memory for ease of use
    Float32(f32),
    Float64(f64),
    Decimal(String), // String-encoded for precision preservation
    // Text
    String(String),
    Symbol(String),
    // Binary
    Binary(Vec<u8>),
    // Temporal
    Date(i32), // Days since epoch
    Time(i64, TimeUnit),
    Timestamp(i64, TimeUnit),
    TimestampTz(i64, TimeUnit),
    // Identifier
    Uuid([u8; 16]),
    ULID([u8; 16]),
    // Structured
    Json(serde_json::Value),
    Jsonb(serde_json::Value),
    Array(Vec<ProximaValue>),
    Map(std::collections::HashMap<String, ProximaValue>),
    Struct(std::collections::HashMap<String, ProximaValue>),
    // Vector
    DenseVector(Vec<f32>),
    SparseVector { indices: Vec<u32>, values: Vec<f32> },
    BinaryVector(Vec<u8>),
    // Null
    Null,
}

// ---------------------------------------------------------------------------
// Arrow + pgwire mappings for ProximaType
// ---------------------------------------------------------------------------

impl ProximaType {
    /// Map to Arrow DataType for columnar storage and query execution.
    pub fn to_arrow(&self) -> ArrowDataType {
        use arrow_schema::Field;
        use std::sync::Arc;

        match self {
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
            ProximaType::String => ArrowDataType::Utf8,
            ProximaType::Symbol => ArrowDataType::Dictionary(
                Box::new(ArrowDataType::Int32),
                Box::new(ArrowDataType::Utf8),
            ),
            ProximaType::Binary => ArrowDataType::Binary,
            ProximaType::Date => ArrowDataType::Date32,
            ProximaType::Time(unit) => ArrowDataType::Time64(arrow_time_unit(unit)),
            ProximaType::Timestamp(unit) => ArrowDataType::Timestamp(arrow_time_unit(unit), None),
            ProximaType::TimestampTz(unit) => {
                ArrowDataType::Timestamp(arrow_time_unit(unit), Some("UTC".into()))
            }
            ProximaType::Interval(_) => {
                ArrowDataType::Interval(arrow_schema::IntervalUnit::MonthDayNano)
            }
            ProximaType::Duration(unit) => ArrowDataType::Duration(arrow_time_unit(unit)),
            ProximaType::Uuid => ArrowDataType::FixedSizeBinary(16),
            ProximaType::ULID => ArrowDataType::FixedSizeBinary(16),
            ProximaType::Json | ProximaType::Jsonb => ArrowDataType::LargeUtf8,
            ProximaType::Array(inner) => {
                let field = Field::new("item", inner.to_arrow(), true);
                ArrowDataType::List(Arc::new(field))
            }
            ProximaType::Map { key, value } => {
                let entries = Field::new(
                    "entries",
                    ArrowDataType::Struct(
                        vec![
                            Field::new("key", key.to_arrow(), false),
                            Field::new("value", value.to_arrow(), true),
                        ]
                        .into(),
                    ),
                    false,
                );
                ArrowDataType::Map(Arc::new(entries), false)
            }
            ProximaType::Struct { fields } => {
                let arrow_fields: Vec<Field> = fields
                    .iter()
                    .map(|(name, ty)| Field::new(name.as_str(), ty.to_arrow(), true))
                    .collect();
                ArrowDataType::Struct(arrow_fields.into())
            }
            ProximaType::DenseVector { element, dim } => {
                let elem_type = match element {
                    VectorElement::Float16 => ArrowDataType::Float16,
                    VectorElement::Float32 => ArrowDataType::Float32,
                    VectorElement::Float64 => ArrowDataType::Float64,
                    VectorElement::Int8 => ArrowDataType::Int8,
                };
                ArrowDataType::FixedSizeList(
                    Arc::new(Field::new("item", elem_type, false)),
                    *dim as i32,
                )
            }
            ProximaType::SparseVector { element } => {
                let elem_type = match element {
                    VectorElement::Float16 => ArrowDataType::Float16,
                    VectorElement::Float32 => ArrowDataType::Float32,
                    VectorElement::Float64 => ArrowDataType::Float64,
                    VectorElement::Int8 => ArrowDataType::Int8,
                };
                let entries = Field::new(
                    "entries",
                    ArrowDataType::Struct(
                        vec![
                            Field::new("index", ArrowDataType::Int32, false),
                            Field::new("value", elem_type, false),
                        ]
                        .into(),
                    ),
                    false,
                );
                ArrowDataType::Map(Arc::new(entries), false)
            }
            ProximaType::BinaryVector { .. } => ArrowDataType::Binary,
            ProximaType::Point => ArrowDataType::Struct(
                vec![
                    Field::new("x", ArrowDataType::Float64, false),
                    Field::new("y", ArrowDataType::Float64, false),
                ]
                .into(),
            ),
            ProximaType::GeographyPoint => ArrowDataType::Struct(
                vec![
                    Field::new("lat", ArrowDataType::Float64, false),
                    Field::new("lon", ArrowDataType::Float64, false),
                ]
                .into(),
            ),
            ProximaType::Null => ArrowDataType::Null,
        }
    }

    /// Return the PostgreSQL wire protocol type OID for this type.
    ///
    /// Used by the pgwire surface (port 5433) to encode column metadata. OIDs
    /// match standard PostgreSQL built-in type catalog values.
    pub fn pgwire_oid(&self) -> u32 {
        match self {
            ProximaType::Boolean => 16,
            ProximaType::Int8 => 21,  // smallint
            ProximaType::Int16 => 21, // smallint
            ProximaType::Int32 => 23, // integer
            ProximaType::Int64 => 20, // bigint
            ProximaType::UInt8 => 21,
            ProximaType::UInt16 => 23,
            ProximaType::UInt32 => 20,
            ProximaType::UInt64 => 1700, // numeric (closest lossless)
            ProximaType::Float16 => 700, // real
            ProximaType::Float32 => 700, // real
            ProximaType::Float64 => 701, // double precision
            ProximaType::Decimal { .. } => 1700, // numeric
            ProximaType::String => 25,   // text
            ProximaType::Symbol => 25,   // text
            ProximaType::Binary => 17,   // bytea
            ProximaType::Date => 1082,
            ProximaType::Time(_) => 1083,
            ProximaType::Timestamp(_) => 1114,
            ProximaType::TimestampTz(_) => 1184,
            ProximaType::Interval(_) => 1186,
            ProximaType::Duration(_) => 1186,
            ProximaType::Uuid => 2950,
            ProximaType::ULID => 2950,
            ProximaType::Json => 114,        // json
            ProximaType::Jsonb => 3802,      // jsonb
            ProximaType::Array(_) => 2277,   // anyarray
            ProximaType::Map { .. } => 3802, // jsonb (no native map)
            ProximaType::Struct { .. } => 3802,
            ProximaType::DenseVector { .. } => 17, // bytea (pgvector uses custom OID when registered)
            ProximaType::SparseVector { .. } => 17,
            ProximaType::BinaryVector { .. } => 17,
            ProximaType::Point => 600, // pg point
            ProximaType::GeographyPoint => 600,
            ProximaType::Null => 705, // unknown
        }
    }
}

fn arrow_time_unit(unit: &TimeUnit) -> arrow_schema::TimeUnit {
    match unit {
        TimeUnit::Second => arrow_schema::TimeUnit::Second,
        TimeUnit::Millisecond => arrow_schema::TimeUnit::Millisecond,
        TimeUnit::Microsecond => arrow_schema::TimeUnit::Microsecond,
        TimeUnit::Nanosecond => arrow_schema::TimeUnit::Nanosecond,
    }
}

// ---------------------------------------------------------------------------
// ProximaValue: dynamic type introspection
// ---------------------------------------------------------------------------

impl ProximaValue {
    /// Return the `ProximaType` discriminant for this value.
    pub fn type_of(&self) -> ProximaType {
        match self {
            ProximaValue::Boolean(_) => ProximaType::Boolean,
            ProximaValue::Int8(_) => ProximaType::Int8,
            ProximaValue::Int16(_) => ProximaType::Int16,
            ProximaValue::Int32(_) => ProximaType::Int32,
            ProximaValue::Int64(_) => ProximaType::Int64,
            ProximaValue::UInt8(_) => ProximaType::UInt8,
            ProximaValue::UInt16(_) => ProximaType::UInt16,
            ProximaValue::UInt32(_) => ProximaType::UInt32,
            ProximaValue::UInt64(_) => ProximaType::UInt64,
            ProximaValue::Float16(_) => ProximaType::Float16,
            ProximaValue::Float32(_) => ProximaType::Float32,
            ProximaValue::Float64(_) => ProximaType::Float64,
            ProximaValue::Decimal(_) => ProximaType::Decimal {
                precision: 38,
                scale: 10,
            },
            ProximaValue::String(_) => ProximaType::String,
            ProximaValue::Symbol(_) => ProximaType::Symbol,
            ProximaValue::Binary(_) => ProximaType::Binary,
            ProximaValue::Date(_) => ProximaType::Date,
            ProximaValue::Time(_, unit) => ProximaType::Time(*unit),
            ProximaValue::Timestamp(_, unit) => ProximaType::Timestamp(*unit),
            ProximaValue::TimestampTz(_, unit) => ProximaType::TimestampTz(*unit),
            ProximaValue::Uuid(_) => ProximaType::Uuid,
            ProximaValue::ULID(_) => ProximaType::ULID,
            ProximaValue::Json(_) => ProximaType::Json,
            ProximaValue::Jsonb(_) => ProximaType::Jsonb,
            ProximaValue::Array(items) => {
                let inner = items
                    .first()
                    .map(|v| v.type_of())
                    .unwrap_or(ProximaType::String);
                ProximaType::Array(Box::new(inner))
            }
            ProximaValue::Map(_) => ProximaType::Map {
                key: Box::new(ProximaType::String),
                value: Box::new(ProximaType::Jsonb),
            },
            ProximaValue::Struct(_) => ProximaType::Struct { fields: vec![] },
            ProximaValue::DenseVector(v) => ProximaType::DenseVector {
                element: VectorElement::Float32,
                dim: v.len(),
            },
            ProximaValue::SparseVector { .. } => ProximaType::SparseVector {
                element: VectorElement::Float32,
            },
            ProximaValue::BinaryVector(v) => ProximaType::BinaryVector { dim: v.len() * 8 },
            ProximaValue::Null => ProximaType::Null,
        }
    }

    /// Return `true` if this value is assignment-compatible with the given type.
    ///
    /// `Null` is compatible with any type. JSON and JSONB are mutually
    /// assignable at this layer because both carry `serde_json::Value`; pgwire
    /// presentation decides whether the column OID is JSON or JSONB.
    pub fn is_compatible_with(&self, ty: &ProximaType) -> bool {
        if matches!(self, ProximaValue::Null) {
            return true;
        }
        match (self, ty) {
            (ProximaValue::Boolean(_), ProximaType::Boolean) => true,
            (ProximaValue::Int8(_), ProximaType::Int8) => true,
            (ProximaValue::Int16(_), ProximaType::Int16) => true,
            (ProximaValue::Int32(_), ProximaType::Int32) => true,
            (ProximaValue::Int64(_), ProximaType::Int64) => true,
            (ProximaValue::UInt8(_), ProximaType::UInt8) => true,
            (ProximaValue::UInt16(_), ProximaType::UInt16) => true,
            (ProximaValue::UInt32(_), ProximaType::UInt32) => true,
            (ProximaValue::UInt64(_), ProximaType::UInt64) => true,
            (ProximaValue::Float16(_), ProximaType::Float16) => true,
            (ProximaValue::Float32(_), ProximaType::Float32) => true,
            (ProximaValue::Float64(_), ProximaType::Float64) => true,
            (ProximaValue::Decimal(_), ProximaType::Decimal { .. }) => true,
            (ProximaValue::String(_), ProximaType::String) => true,
            (ProximaValue::Symbol(_), ProximaType::Symbol) => true,
            (ProximaValue::String(_), ProximaType::Symbol) => true,
            (ProximaValue::Binary(_), ProximaType::Binary) => true,
            (ProximaValue::Date(_), ProximaType::Date) => true,
            (ProximaValue::Time(_, _), ProximaType::Time(_)) => true,
            (ProximaValue::Timestamp(_, _), ProximaType::Timestamp(_)) => true,
            (ProximaValue::TimestampTz(_, _), ProximaType::TimestampTz(_)) => true,
            (ProximaValue::Uuid(_), ProximaType::Uuid) => true,
            (ProximaValue::ULID(_), ProximaType::ULID) => true,
            (ProximaValue::Json(_), ProximaType::Json | ProximaType::Jsonb) => true,
            (ProximaValue::Jsonb(_), ProximaType::Json | ProximaType::Jsonb) => true,
            (ProximaValue::Array(_), ProximaType::Array(_)) => true,
            (ProximaValue::Map(_), ProximaType::Map { .. } | ProximaType::Jsonb) => true,
            (ProximaValue::Struct(_), ProximaType::Struct { .. } | ProximaType::Jsonb) => true,
            (ProximaValue::DenseVector(_), ProximaType::DenseVector { .. }) => true,
            (ProximaValue::SparseVector { .. }, ProximaType::SparseVector { .. }) => true,
            (ProximaValue::BinaryVector(_), ProximaType::BinaryVector { .. }) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_uses_stable_lowercase_names() {
        assert_eq!(DataModel::Vector.to_string(), "vector");
        assert_eq!(DataModel::Relational.to_string(), "relational");
    }

    #[test]
    fn type_tree_is_nested() {
        let nested = ProximaType::Array(Box::new(ProximaType::Int64));
        match nested {
            ProximaType::Array(inner) => assert!(matches!(*inner, ProximaType::Int64)),
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn memory_type_serialization() {
        let mt = MemoryType::Preference;
        let json = serde_json::to_string(&mt).unwrap();
        assert_eq!(json, "\"preference\"");
        assert_eq!(mt.to_string(), "preference");
        assert_eq!(
            "preference".parse::<MemoryType>(),
            Ok(MemoryType::Preference)
        );
    }

    #[test]
    fn test_arrow_roundtrip_all_scalars() {
        let types = vec![
            ProximaType::Boolean,
            ProximaType::Int8,
            ProximaType::Int16,
            ProximaType::Int32,
            ProximaType::Int64,
            ProximaType::UInt8,
            ProximaType::UInt16,
            ProximaType::UInt32,
            ProximaType::UInt64,
            ProximaType::Float16,
            ProximaType::Float32,
            ProximaType::Float64,
            ProximaType::String,
            ProximaType::Symbol,
            ProximaType::Binary,
            ProximaType::Date,
            ProximaType::Uuid,
            ProximaType::ULID,
            ProximaType::Json,
            ProximaType::Jsonb,
            ProximaType::Point,
            ProximaType::GeographyPoint,
            ProximaType::Null,
        ];
        for ty in &types {
            let _ = ty.to_arrow();
        }
    }

    #[test]
    fn test_jsonb_arrow_and_pgwire_oid() {
        assert_eq!(ProximaType::Json.to_arrow(), ArrowDataType::LargeUtf8);
        assert_eq!(ProximaType::Jsonb.to_arrow(), ArrowDataType::LargeUtf8);
        assert_eq!(ProximaType::Json.pgwire_oid(), 114);
        assert_eq!(ProximaType::Jsonb.pgwire_oid(), 3802);
        assert_eq!(
            ProximaType::Map {
                key: Box::new(ProximaType::String),
                value: Box::new(ProximaType::Jsonb),
            }
            .pgwire_oid(),
            3802
        );
    }

    #[test]
    fn test_decimal_arrow() {
        let ty = ProximaType::Decimal {
            precision: 38,
            scale: 10,
        };
        assert_eq!(ty.to_arrow(), ArrowDataType::Decimal128(38, 10));
    }

    #[test]
    fn test_timestamptz_arrow() {
        let ty = ProximaType::TimestampTz(TimeUnit::Nanosecond);
        match ty.to_arrow() {
            ArrowDataType::Timestamp(arrow_schema::TimeUnit::Nanosecond, tz) => {
                assert_eq!(tz.as_deref(), Some("UTC"));
            }
            other => panic!("Expected Timestamp(Nanosecond, UTC), got {:?}", other),
        }
    }

    #[test]
    fn test_timestamp_no_tz_arrow() {
        let ty = ProximaType::Timestamp(TimeUnit::Microsecond);
        match ty.to_arrow() {
            ArrowDataType::Timestamp(arrow_schema::TimeUnit::Microsecond, tz) => {
                assert!(tz.is_none());
            }
            other => panic!("Expected Timestamp(Microsecond, None), got {:?}", other),
        }
    }

    #[test]
    fn test_dense_vector_arrow() {
        let ty = ProximaType::DenseVector {
            element: VectorElement::Float32,
            dim: 512,
        };
        match ty.to_arrow() {
            ArrowDataType::FixedSizeList(field, 512) => {
                assert_eq!(field.data_type(), &ArrowDataType::Float32);
            }
            other => panic!("Expected FixedSizeList(Float32, 512), got {:?}", other),
        }
    }

    #[test]
    fn test_pgwire_decimal_oid() {
        assert_eq!(
            ProximaType::Decimal {
                precision: 10,
                scale: 2
            }
            .pgwire_oid(),
            1700
        );
    }

    #[test]
    fn test_pgwire_timestamptz_oid() {
        assert_eq!(
            ProximaType::TimestampTz(TimeUnit::Microsecond).pgwire_oid(),
            1184
        );
    }

    #[test]
    fn test_pgwire_uuid_oid() {
        assert_eq!(ProximaType::Uuid.pgwire_oid(), 2950);
    }

    #[test]
    fn test_pgwire_vector_oid() {
        assert_eq!(
            ProximaType::DenseVector {
                element: VectorElement::Float32,
                dim: 128
            }
            .pgwire_oid(),
            17
        );
    }

    #[test]
    fn test_proxima_value_type_of_decimal() {
        let val = ProximaValue::Decimal("123.456".to_string());
        assert!(matches!(val.type_of(), ProximaType::Decimal { .. }));
    }

    #[test]
    fn test_proxima_value_type_of_int64() {
        let val = ProximaValue::Int64(42);
        assert!(matches!(val.type_of(), ProximaType::Int64));
    }

    #[test]
    fn test_proxima_value_type_of_timestamptz() {
        let val = ProximaValue::TimestampTz(1_700_000_000, TimeUnit::Microsecond);
        assert!(matches!(val.type_of(), ProximaType::TimestampTz(_)));
    }

    #[test]
    fn test_proxima_value_type_of_jsonb_and_null() {
        let val = ProximaValue::Jsonb(serde_json::json!({"a": 1}));
        assert!(matches!(val.type_of(), ProximaType::Jsonb));
        assert!(matches!(ProximaValue::Null.type_of(), ProximaType::Null));
    }

    #[test]
    fn test_value_compatible_with_exact() {
        assert!(ProximaValue::Int64(5).is_compatible_with(&ProximaType::Int64));
        assert!(ProximaValue::Float32(1.0).is_compatible_with(&ProximaType::Float32));
        assert!(ProximaValue::Decimal("1.0".to_string()).is_compatible_with(
            &ProximaType::Decimal {
                precision: 38,
                scale: 10
            }
        ));
        assert!(ProximaValue::Uuid([0u8; 16]).is_compatible_with(&ProximaType::Uuid));
    }

    #[test]
    fn test_json_and_jsonb_are_assignment_compatible() {
        let json = ProximaValue::Json(serde_json::json!({"a": 1}));
        let jsonb = ProximaValue::Jsonb(serde_json::json!({"a": 1}));
        assert!(json.is_compatible_with(&ProximaType::Jsonb));
        assert!(jsonb.is_compatible_with(&ProximaType::Json));
        assert!(jsonb.is_compatible_with(&ProximaType::Jsonb));
    }

    #[test]
    fn test_value_incompatible_types() {
        assert!(!ProximaValue::Int64(5).is_compatible_with(&ProximaType::Float32));
        assert!(!ProximaValue::String("x".to_string()).is_compatible_with(&ProximaType::Int64));
    }

    #[test]
    fn test_null_compatible_any() {
        assert!(ProximaValue::Null.is_compatible_with(&ProximaType::Int64));
        assert!(
            ProximaValue::Null.is_compatible_with(&ProximaType::Decimal {
                precision: 38,
                scale: 0
            })
        );
        assert!(
            ProximaValue::Null.is_compatible_with(&ProximaType::DenseVector {
                element: VectorElement::Float32,
                dim: 512
            })
        );
    }
}
