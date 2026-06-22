// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Neutral, transport-agnostic graph domain model (TD-123 Step 1 — graph pilot).
//!
//! These are plain Rust types (serde-derivable, no protobuf/prost dependency).
//! The graph engine and `GraphOperationsService` speak ONLY these types; the
//! wire adapters (`proximadb.v1` gRPC in `src/network/grpc/graph_service.rs`,
//! `proximadb.v2.ProximaGraphService` in `src/network/grpc/v2/graph_service.rs`)
//! convert proto <-> these types at their boundary. This decouples the graph
//! core from any wire version, so deleting the v1 proto (TD-121) never touches
//! the engine.
//!
//! The shapes intentionally mirror the former v1 proto message layout (same
//! field and oneof-variant names) so the conversion is lossless and the internal
//! call sites are unchanged — but the types are now self-contained: property
//! vectors carry `Vec<f32>` (not a proto `VectorData`), and a [`GraphPath`] is a
//! plain sequence of node/edge ids (not entity.proto `Entity`/`Relation`).
//!
//! Persistence is unaffected: durable graph state flows through `ProximaRecord`
//! (`adjacency_projection::{node,edge}_to_canonical_record`), so this is an
//! in-memory representation change with no on-disk/WAL format migration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A vertex in the graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub labels: Vec<String>,
    pub properties: HashMap<String, PropertyValue>,
    pub embedding: Option<EmbeddingVersion>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// A directed relationship between two nodes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub edge_type: String,
    pub properties: HashMap<String, PropertyValue>,
    pub weight: Option<f64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// A polymorphic property value (mirrors the former proto oneof shape).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PropertyValue {
    pub value: Option<property_value::Value>,
}

/// Oneof payload for [`PropertyValue`]. Kept in a submodule named `property_value`
/// so existing `property_value::Value::*` references resolve unchanged.
pub mod property_value {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Value {
        StringValue(String),
        IntValue(i64),
        DoubleValue(f64),
        BoolValue(bool),
        BytesValue(Vec<u8>),
        ArrayValue(super::PropertyArray),
        ObjectValue(super::PropertyObject),
        /// Self-contained embedding payload (was proto `VectorData`).
        VectorValue(Vec<f32>),
    }
}

/// Ordered list of property values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PropertyArray {
    pub values: Vec<PropertyValue>,
}

/// Nested key/value property object.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PropertyObject {
    pub fields: HashMap<String, PropertyValue>,
}

/// Optional semantic embedding attached to a node.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingVersion {
    pub model_id: String,
    pub model_version: String,
    pub vector: Vec<f32>,
    pub dimension: u32,
    pub created_at_ms: i64,
    pub model_params: HashMap<String, String>,
    /// Modality discriminant (mirrors entity.proto `Modality` as i32).
    pub modality: i32,
}

/// Property predicate for node/edge queries.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PropertyFilter {
    pub key: String,
    /// [`PropertyFilterOperator`] as i32 (mirrors the proto enum field).
    pub operator: i32,
    pub value: Option<PropertyValue>,
}

/// Comparison operators (same discriminants as the former proto enum).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum PropertyFilterOperator {
    #[default]
    Unspecified = 0,
    Equals = 1,
    NotEquals = 2,
    GreaterThan = 3,
    LessThan = 4,
    GreaterEqual = 5,
    LessEqual = 6,
    Contains = 7,
    StartsWith = 8,
    EndsWith = 9,
}

impl TryFrom<i32> for PropertyFilterOperator {
    type Error = i32;
    fn try_from(v: i32) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::Unspecified),
            1 => Ok(Self::Equals),
            2 => Ok(Self::NotEquals),
            3 => Ok(Self::GreaterThan),
            4 => Ok(Self::LessThan),
            5 => Ok(Self::GreaterEqual),
            6 => Ok(Self::LessEqual),
            7 => Ok(Self::Contains),
            8 => Ok(Self::StartsWith),
            9 => Ok(Self::EndsWith),
            other => Err(other),
        }
    }
}

/// Node query request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeQuery {
    pub graph_id: String,
    pub labels: Vec<String>,
    pub filters: Vec<PropertyFilter>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub continuation_token: Option<String>,
}

/// Edge query request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EdgeQuery {
    pub graph_id: String,
    pub from_node_id: Option<String>,
    pub to_node_id: Option<String>,
    pub edge_types: Vec<String>,
    pub filters: Vec<PropertyFilter>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub continuation_token: Option<String>,
}

/// Graph traversal request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TraversalRequest {
    pub graph_id: String,
    pub start_node_id: String,
    pub max_depth: u32,
    pub edge_types: Vec<String>,
    pub node_labels: Vec<String>,
    pub filters: Vec<PropertyFilter>,
    /// [`TraversalAlgorithm`] as i32 (mirrors the proto enum field).
    pub algorithm: i32,
    pub limit: Option<u32>,
    pub timeout_ms: Option<u32>,
    pub max_frontier: Option<u32>,
}

/// Traversal algorithms (same discriminants as the former proto enum).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum TraversalAlgorithm {
    #[default]
    Unspecified = 0,
    Bfs = 1,
    Dfs = 2,
    ParallelBfs = 3,
}

impl TryFrom<i32> for TraversalAlgorithm {
    type Error = i32;
    fn try_from(v: i32) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::Unspecified),
            1 => Ok(Self::Bfs),
            2 => Ok(Self::Dfs),
            3 => Ok(Self::ParallelBfs),
            other => Err(other),
        }
    }
}

/// Graph traversal response.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TraversalResponse {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub paths: Vec<GraphPath>,
    pub stats: Option<TraversalStats>,
}

/// Traversal statistics.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TraversalStats {
    pub nodes_visited: u32,
    pub edges_traversed: u32,
    pub max_depth_reached: u32,
    pub execution_time_microseconds: u64,
}

/// A path as an ordered sequence of node and edge ids (self-contained — was a
/// proto `GraphPath` of entity.proto `Entity`/`Relation`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphPath {
    pub node_ids: Vec<String>,
    pub edge_ids: Vec<String>,
}

/// Aggregate graph statistics.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphStats {
    pub total_nodes: u64,
    pub total_edges: u64,
    pub label_stats: Vec<LabelStats>,
    pub edge_type_stats: Vec<EdgeTypeStats>,
    pub total_properties: u64,
    pub memory_usage_bytes: u64,
    pub average_degree: f64,
    pub max_degree: u32,
    pub connected_components: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LabelStats {
    pub label: String,
    pub count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EdgeTypeStats {
    pub edge_type: String,
    pub count: u64,
}
