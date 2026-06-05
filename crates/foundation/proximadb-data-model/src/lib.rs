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

use arrow_schema::{
    DataType as ArrowDataType, Field, IntervalUnit as ArrowIntervalUnit, TimeUnit as ArrowTimeUnit,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
/// Derived from arXiv:2604.22085 (Memanto). Categorizing memory allows for
/// type-filtered retrieval and specialized conflict resolution strategies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    /// Objective, verifiable information (e.g., "User lives in PST").
    Fact,
    /// User or system preferences (e.g., "Prefers dark mode").
    Preference,
    /// Choices made affecting the future (e.g., "Chose PostgreSQL for DB").
    Decision,
    /// Promises or obligations (e.g., "Deliver report by Friday").
    Commitment,
    /// Objectives to achieve (e.g., "Reach 10K users by Q4").
    Goal,
    /// Historical occurrences (e.g., "Meeting with CEO at 2pm").
    Event,
    /// Rules and guidelines (e.g., "Always validate input").
    Instruction,
    /// Entity connections (e.g., "Alice manages Bob").
    Relationship,
    /// Situational information (e.g., "Currently in budget review").
    Context,
    /// Lessons from experience (e.g., "Users need simpler onboarding").
    Learning,
    /// Patterns noticed (e.g., "Traffic peaks on Fridays").
    Observation,
    /// Mistakes to avoid (e.g., "Don't use unwrap in production").
    Error,
    /// Document or code references (e.g., "Reference to RFC 7519").
    Artifact,
}

impl MemoryType {
    /// Stable TD-055/Memanto category list.
    pub const ALL: [Self; 13] = [
        Self::Fact,
        Self::Preference,
        Self::Decision,
        Self::Commitment,
        Self::Goal,
        Self::Event,
        Self::Instruction,
        Self::Relationship,
        Self::Context,
        Self::Learning,
        Self::Observation,
        Self::Error,
        Self::Artifact,
    ];
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
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
        };
        f.write_str(value)
    }
}

impl std::str::FromStr for MemoryType {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
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
            _ => Err(format!("unknown memory type: {}", value)),
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
    /// Boolean (true/false).
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
    /// Exact numeric with fixed precision and scale.
    Decimal {
        precision: u8,
        scale: u8,
    },
    // Text
    String,
    /// Dictionary-encoded string for high-cardinality fields.
    Symbol,
    // Binary
    Binary,
    // Temporal
    Date,
    Time(TimeUnit),
    Timestamp(TimeUnit),
    /// Timestamp with time zone.
    TimestampTz(TimeUnit),
    Interval(TimeUnit),
    Duration(TimeUnit),
    // Identifier
    Uuid,
    ULID,
    // Structured
    /// Plain JSON text.
    Json,
    /// Binary JSON for efficient querying and partial updates.
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
    /// Represents the absence of a value.
    Null,
}

impl ProximaType {
    /// Return the PostgreSQL OID for this type for pgwire compatibility.
    ///
    /// Canonical OID authority for ALL surfaces (ADR-024): catalog and storage
    /// types map here via [`ProximaType`] rather than maintaining parallel OID
    /// tables. Types without a precise pg builtin fall back to the closest
    /// representable (wider int / `numeric` / `varchar`).
    pub fn pgwire_oid(&self) -> u32 {
        match self {
            ProximaType::Boolean => 16,
            ProximaType::Binary => 17,
            ProximaType::Int64 | ProximaType::UInt32 | ProximaType::UInt64 => 20, // int8 (uint32/64 widen)
            ProximaType::Int8 | ProximaType::Int16 | ProximaType::UInt8 => 21,    // int2
            ProximaType::Int32 | ProximaType::UInt16 => 23,                       // int4
            ProximaType::Json => 114,
            ProximaType::Point | ProximaType::GeographyPoint => 600, // point
            ProximaType::Float16 | ProximaType::Float32 => 700,      // float4
            ProximaType::Float64 => 701,
            ProximaType::String | ProximaType::Symbol => 1043, // varchar
            ProximaType::Date => 1082,
            ProximaType::Time(_) => 1083,
            ProximaType::Timestamp(_) => 1114,
            ProximaType::TimestampTz(_) => 1184,
            ProximaType::Interval(_) | ProximaType::Duration(_) => 1186, // interval
            ProximaType::Decimal { .. } => 1700,
            ProximaType::Uuid | ProximaType::ULID => 2950,
            ProximaType::Jsonb => 3802,
            ProximaType::Array(_) => 2277, // anyarray
            // Map/Struct/vectors/Null have no pg builtin → varchar text form.
            _ => 1043,
        }
    }

    /// Canonical [`ProximaType`] → Arrow [`ArrowDataType`] mapping (ADR-024).
    ///
    /// This is the single home for the type→Arrow projection that the catalog
    /// and storage layers previously duplicated (`CatalogDataType::to_arrow_datatype`,
    /// `ProximaDataType::to_arrow_type`). Dense vectors encode as
    /// `FixedSizeBinary(dim * element_width)` (little-endian); binary vectors as
    /// `FixedSizeBinary(ceil(dim/8))`. Identifier/geo/jsonb types without an Arrow
    /// builtin project to their physical carrier (Utf8 / FixedSizeBinary), which is
    /// a lossy physical projection (the logical type stays authoritative here).
    pub fn to_arrow_type(&self) -> ArrowDataType {
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
            ProximaType::String | ProximaType::Symbol => ArrowDataType::Utf8,
            ProximaType::Binary => ArrowDataType::Binary,
            ProximaType::Date => ArrowDataType::Date32,
            ProximaType::Time(unit) => ArrowDataType::Time64(time_unit_to_arrow(*unit)),
            ProximaType::Timestamp(unit) => {
                ArrowDataType::Timestamp(time_unit_to_arrow(*unit), None)
            }
            ProximaType::TimestampTz(unit) => {
                ArrowDataType::Timestamp(time_unit_to_arrow(*unit), Some("UTC".into()))
            }
            ProximaType::Interval(_) => ArrowDataType::Interval(ArrowIntervalUnit::MonthDayNano),
            ProximaType::Duration(unit) => ArrowDataType::Duration(time_unit_to_arrow(*unit)),
            // UUID/ULID are 16-byte identifiers; JSON text-backed.
            ProximaType::Uuid | ProximaType::ULID => ArrowDataType::FixedSizeBinary(16),
            ProximaType::Json | ProximaType::Jsonb => ArrowDataType::Utf8,
            ProximaType::Array(element) => {
                ArrowDataType::List(Arc::new(Field::new("item", element.to_arrow_type(), true)))
            }
            ProximaType::Map { key, value } => ArrowDataType::Map(
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
            ProximaType::Struct { fields } => ArrowDataType::Struct(
                fields
                    .iter()
                    .map(|(name, ty)| Field::new(name, ty.to_arrow_type(), true))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            ProximaType::DenseVector { element, dim } => {
                ArrowDataType::FixedSizeBinary((*dim * element.byte_width()) as i32)
            }
            ProximaType::SparseVector { .. } => ArrowDataType::Map(
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
            ),
            ProximaType::BinaryVector { dim } => {
                ArrowDataType::FixedSizeBinary(dim.div_ceil(8) as i32)
            }
            // 2 × f64 little-endian carrier.
            ProximaType::Point | ProximaType::GeographyPoint => ArrowDataType::FixedSizeBinary(16),
            ProximaType::Null => ArrowDataType::Null,
        }
    }

    /// Best-effort Arrow [`ArrowDataType`] → canonical [`ProximaType`] mapping
    /// (the inverse of [`to_arrow_type`](Self::to_arrow_type)).
    ///
    /// Some logical types share a physical Arrow carrier (Symbol/String→Utf8,
    /// Uuid/Point→FixedSizeBinary), so the reverse is necessarily heuristic: a
    /// `FixedSizeBinary` whose width is a multiple of 4 is read back as a
    /// `Float32` dense vector (matching `to_arrow_type`); other widths → `Binary`.
    pub fn from_arrow_type(arrow_type: &ArrowDataType) -> Self {
        match arrow_type {
            ArrowDataType::Boolean => ProximaType::Boolean,
            ArrowDataType::Int8 => ProximaType::Int8,
            ArrowDataType::Int16 => ProximaType::Int16,
            ArrowDataType::Int32 => ProximaType::Int32,
            ArrowDataType::Int64 => ProximaType::Int64,
            ArrowDataType::UInt8 => ProximaType::UInt8,
            ArrowDataType::UInt16 => ProximaType::UInt16,
            ArrowDataType::UInt32 => ProximaType::UInt32,
            ArrowDataType::UInt64 => ProximaType::UInt64,
            ArrowDataType::Float16 => ProximaType::Float16,
            ArrowDataType::Float32 => ProximaType::Float32,
            ArrowDataType::Float64 => ProximaType::Float64,
            ArrowDataType::Decimal128(p, s) | ArrowDataType::Decimal256(p, s) => {
                ProximaType::Decimal {
                    precision: *p,
                    scale: *s as u8,
                }
            }
            ArrowDataType::Utf8 | ArrowDataType::LargeUtf8 => ProximaType::String,
            ArrowDataType::Binary | ArrowDataType::LargeBinary => ProximaType::Binary,
            ArrowDataType::Date32 | ArrowDataType::Date64 => ProximaType::Date,
            ArrowDataType::Time64(unit) | ArrowDataType::Time32(unit) => {
                ProximaType::Time(time_unit_from_arrow(*unit))
            }
            ArrowDataType::Timestamp(unit, tz) => {
                if tz.is_some() {
                    ProximaType::TimestampTz(time_unit_from_arrow(*unit))
                } else {
                    ProximaType::Timestamp(time_unit_from_arrow(*unit))
                }
            }
            ArrowDataType::Duration(unit) => ProximaType::Duration(time_unit_from_arrow(*unit)),
            ArrowDataType::Interval(_) => ProximaType::Interval(TimeUnit::Nanosecond),
            ArrowDataType::Null => ProximaType::Null,
            ArrowDataType::FixedSizeBinary(size) => {
                if *size > 0 && *size % 4 == 0 {
                    ProximaType::DenseVector {
                        element: VectorElement::Float32,
                        dim: (*size / 4) as usize,
                    }
                } else {
                    ProximaType::Binary
                }
            }
            ArrowDataType::List(field) | ArrowDataType::LargeList(field) => {
                ProximaType::Array(Box::new(Self::from_arrow_type(field.data_type())))
            }
            ArrowDataType::Map(field, _) => {
                if let ArrowDataType::Struct(fields) = field.data_type()
                    && fields.len() >= 2
                {
                    ProximaType::Map {
                        key: Box::new(Self::from_arrow_type(fields[0].data_type())),
                        value: Box::new(Self::from_arrow_type(fields[1].data_type())),
                    }
                } else {
                    ProximaType::Json
                }
            }
            ArrowDataType::Struct(fields) => ProximaType::Struct {
                fields: fields
                    .iter()
                    .map(|f| (f.name().clone(), Self::from_arrow_type(f.data_type())))
                    .collect(),
            },
            _ => ProximaType::Binary,
        }
    }
}

/// Map the canonical [`TimeUnit`] to Arrow's time unit.
fn time_unit_to_arrow(unit: TimeUnit) -> ArrowTimeUnit {
    match unit {
        TimeUnit::Second => ArrowTimeUnit::Second,
        TimeUnit::Millisecond => ArrowTimeUnit::Millisecond,
        TimeUnit::Microsecond => ArrowTimeUnit::Microsecond,
        TimeUnit::Nanosecond => ArrowTimeUnit::Nanosecond,
    }
}

/// Map Arrow's time unit back to the canonical [`TimeUnit`].
fn time_unit_from_arrow(unit: ArrowTimeUnit) -> TimeUnit {
    match unit {
        ArrowTimeUnit::Second => TimeUnit::Second,
        ArrowTimeUnit::Millisecond => TimeUnit::Millisecond,
        ArrowTimeUnit::Microsecond => TimeUnit::Microsecond,
        ArrowTimeUnit::Nanosecond => TimeUnit::Nanosecond,
    }
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
    /// Brain float16 (truncated f32 exponent range).
    BFloat16,
    Float32,
    Float64,
    Int8,
}

impl VectorElement {
    /// Byte width of one element in the dense little-endian layout.
    pub fn byte_width(&self) -> usize {
        match self {
            VectorElement::Int8 => 1,
            VectorElement::Float16 | VectorElement::BFloat16 => 2,
            VectorElement::Float32 => 4,
            VectorElement::Float64 => 8,
        }
    }
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
    Float16(f32), // Represented as f32 in-memory
    Float32(f32),
    Float64(f64),
    Decimal(String),
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
    /// Binary JSON data (stored as MessagePack).
    Jsonb(serde_json::Value),
    Array(Vec<ProximaValue>),
    Map(std::collections::HashMap<String, ProximaValue>),
    Struct(std::collections::HashMap<String, ProximaValue>),
    // Vector
    DenseVector(Vec<f32>),
    SparseVector {
        indices: Vec<u32>,
        values: Vec<f32>,
    },
    BinaryVector(Vec<u8>),
    // Null
    Null,
}

impl ProximaValue {
    /// Serialize a `serde_json::Value` to MessagePack bytes for `Jsonb`.
    pub fn to_jsonb_vec(val: &serde_json::Value) -> anyhow::Result<Vec<u8>> {
        let mut buf = Vec::new();
        val.serialize(&mut rmp_serde::Serializer::new(&mut buf))?;
        Ok(buf)
    }

    /// Deserialize MessagePack bytes back to a `serde_json::Value`.
    pub fn from_jsonb_slice(slice: &[u8]) -> anyhow::Result<serde_json::Value> {
        let val: serde_json::Value = rmp_serde::from_slice(slice)?;
        Ok(val)
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
    }

    #[test]
    fn memory_type_parses_stable_names() {
        assert_eq!(
            "decision".parse::<MemoryType>().unwrap(),
            MemoryType::Decision
        );
        assert_eq!(
            " Relationship ".parse::<MemoryType>().unwrap(),
            MemoryType::Relationship
        );
        assert!("unknown".parse::<MemoryType>().is_err());
    }

    #[test]
    fn memory_type_all_contains_thirteen_stable_categories() {
        assert_eq!(MemoryType::ALL.len(), 13);
        assert_eq!(MemoryType::ALL[0], MemoryType::Fact);
        assert_eq!(MemoryType::ALL[12], MemoryType::Artifact);

        for memory_type in MemoryType::ALL {
            assert_eq!(
                memory_type.to_string().parse::<MemoryType>().unwrap(),
                memory_type
            );
        }
    }

    #[test]
    fn test_pgwire_oids() {
        assert_eq!(ProximaType::Json.pgwire_oid(), 114);
        assert_eq!(ProximaType::Jsonb.pgwire_oid(), 3802);
        assert_eq!(
            ProximaType::TimestampTz(TimeUnit::Millisecond).pgwire_oid(),
            1184
        );
        // Extended (ADR-024) coverage: previously fell through to varchar.
        assert_eq!(ProximaType::Int8.pgwire_oid(), 21); // int2
        assert_eq!(ProximaType::Time(TimeUnit::Microsecond).pgwire_oid(), 1083);
        assert_eq!(
            ProximaType::Timestamp(TimeUnit::Nanosecond).pgwire_oid(),
            1114
        );
        assert_eq!(ProximaType::Point.pgwire_oid(), 600);
        assert_eq!(
            ProximaType::Uuid.pgwire_oid(),
            ProximaType::ULID.pgwire_oid()
        );
    }

    #[test]
    fn proxima_type_arrow_round_trips_scalars_and_temporal() {
        for ty in [
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
            ProximaType::Binary,
            ProximaType::Date,
            ProximaType::Time(TimeUnit::Microsecond),
            ProximaType::Timestamp(TimeUnit::Nanosecond),
            ProximaType::TimestampTz(TimeUnit::Nanosecond),
            ProximaType::Duration(TimeUnit::Second),
            ProximaType::Null,
        ] {
            let arrow = ty.to_arrow_type();
            let back = ProximaType::from_arrow_type(&arrow);
            assert_eq!(ty, back, "Arrow round-trip must preserve {ty:?}");
        }
    }

    #[test]
    fn proxima_type_dense_vector_arrow_layout_matches_element_width() {
        // f32 dim 8 -> FixedSizeBinary(32); reverse reads back as f32 dense vector.
        let v = ProximaType::DenseVector {
            element: VectorElement::Float32,
            dim: 8,
        };
        assert_eq!(v.to_arrow_type(), ArrowDataType::FixedSizeBinary(32));
        assert_eq!(ProximaType::from_arrow_type(&v.to_arrow_type()), v);
        // int8 element is 1 byte wide.
        let q = ProximaType::DenseVector {
            element: VectorElement::Int8,
            dim: 8,
        };
        assert_eq!(q.to_arrow_type(), ArrowDataType::FixedSizeBinary(8));
        assert_eq!(VectorElement::BFloat16.byte_width(), 2);
    }

    #[test]
    fn proxima_type_nested_arrow_round_trips() {
        let ty = ProximaType::Array(Box::new(ProximaType::Int64));
        assert_eq!(ProximaType::from_arrow_type(&ty.to_arrow_type()), ty);
        let m = ProximaType::Map {
            key: Box::new(ProximaType::String),
            value: Box::new(ProximaType::Float64),
        };
        assert_eq!(ProximaType::from_arrow_type(&m.to_arrow_type()), m);
    }

    #[test]
    fn test_jsonb_serialization_roundtrip() {
        let original = serde_json::json!({
            "key": "value",
            "number": 123,
            "bool": true,
            "nested": { "a": [1, 2, 3] }
        });

        let encoded = ProximaValue::to_jsonb_vec(&original).unwrap();
        let decoded = ProximaValue::from_jsonb_slice(&encoded).unwrap();

        assert_eq!(original, decoded);
    }
}
