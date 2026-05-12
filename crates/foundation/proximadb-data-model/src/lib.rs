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
    pub fn pgwire_oid(&self) -> u32 {
        match self {
            ProximaType::Boolean => 16,
            ProximaType::Int16 => 21,
            ProximaType::Int32 => 23,
            ProximaType::Int64 => 20,
            ProximaType::Float32 => 700,
            ProximaType::Float64 => 701,
            ProximaType::String => 1043, // varchar
            ProximaType::Symbol => 1043,
            ProximaType::TimestampTz(_) => 1184,
            ProximaType::Uuid => 2950,
            ProximaType::Binary => 17,
            ProximaType::Date => 1082,
            ProximaType::Json => 114,
            ProximaType::Jsonb => 3802,
            ProximaType::Decimal { .. } => 1700,
            ProximaType::Array(_) => 2277, // anyarray
            _ => 1043,                     // Default to varchar
        }
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
    fn test_pgwire_oids() {
        assert_eq!(ProximaType::Json.pgwire_oid(), 114);
        assert_eq!(ProximaType::Jsonb.pgwire_oid(), 3802);
        assert_eq!(
            ProximaType::TimestampTz(TimeUnit::Millisecond).pgwire_oid(),
            1184
        );
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
