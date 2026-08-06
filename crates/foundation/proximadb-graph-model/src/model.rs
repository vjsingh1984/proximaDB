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

/// Direction for impact analysis (TD-131). `Forward` follows OUTGOING edges from the start node
/// ("what does X impact"); `Backward` follows INCOMING edges ("what impacts X"). Distinct from
/// traversal so the API reads as blast-radius, not a graph walk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImpactDirection {
    #[default]
    Forward,
    Backward,
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

// ---------------------------------------------------------------------------
// Graph write operations (moved from src/storage/memtable/implementations/
// graph_memtable.rs — root-crate decomposition). The unified WAL's `GraphOp`
// variant and the ORION graph engine consume these; their payload types (Node,
// Edge, PropertyValue, EmbeddingVersion) already live in this leaf.
// ---------------------------------------------------------------------------

/// Node / edge identifier aliases (transparent to `String`). Kept here so the
/// graph-operation model below is self-contained.
pub type NodeId = String;
pub type EdgeId = String;

/// Graph operation for WAL integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphOperation {
    CreateNode {
        graph_id: String,
        node: Node,
    },
    UpdateNode {
        graph_id: String,
        node_id: NodeId,
        update: NodeUpdate,
    },
    DeleteNode {
        graph_id: String,
        node_id: NodeId,
    },
    CreateEdge {
        graph_id: String,
        edge: Edge,
    },
    UpdateEdge {
        graph_id: String,
        edge_id: EdgeId,
        update: EdgeUpdate,
    },
    DeleteEdge {
        graph_id: String,
        edge_id: EdgeId,
    },
    BatchOperation {
        operations: Vec<GraphOperation>,
    },
    CreateEdgeIndex {
        graph_id: String,
        index_config: String,
    },
    DropEdgeIndex {
        graph_id: String,
        index_name: String,
    },
}

/// Node update structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeUpdate {
    pub labels: Option<Vec<String>>,
    pub properties: Option<HashMap<String, PropertyValue>>,
    pub embedding: Option<EmbeddingVersion>,
}

/// Edge update structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeUpdate {
    pub properties: Option<HashMap<String, PropertyValue>>,
    pub weight: Option<f64>,
}

/// Non-data marker kinds carried in a graph engine's WAL stream (the payload of
/// a `GraphMarker` frame). Moved here from the root WAL module so the
/// `GraphWalPort` contract (storage-ports) — and the ORION graph engine — can
/// name it without a cyclic root dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarkerKind {
    /// "Every engine-WAL frame after this marker was emitted after the canonical
    /// layer durably checkpointed at `lsn`." Recovery skips frames at/before the
    /// latest such marker whose `lsn` ≤ the recovered canonical checkpoint LSN.
    CanonicalEmission(u64),
}

/// A graph-relevant record projected from a unified WAL stream — the read-side
/// counterpart to the graph operations / markers written through
/// [`GraphWalPort`]. A reader port (e.g. `GraphWalReaderPort`) yields these so a
/// graph engine can replay its WAL without naming the concrete unified WAL
/// entry/operation types (the remaining decoupling needed to extract the engine
/// into its own crate). Non-graph unified ops are filtered out by the reader.
#[derive(Debug, Clone)]
pub enum GraphWalRecord {
    /// A graph data operation (CreateNode/CreateEdge/Update*/Delete*/Batch/…).
    /// Boxed because `GraphOperation` (itself an enum over Node/Edge/Batch) is
    /// far larger than [`MarkerKind`]; boxing the heavy variant keeps the enum
    /// pointer-sized and `Vec<GraphWalEntry>` compact during replay.
    Op(Box<GraphOperation>),
    /// A non-data canonical-sync marker.
    Marker(MarkerKind),
}

/// One projected graph WAL frame: its sequence number (LSN) plus the graph
/// record it carries.
#[derive(Debug, Clone)]
pub struct GraphWalEntry {
    /// Monotonic WAL sequence number assigned at append.
    pub sequence_number: u64,
    /// The graph operation or marker.
    pub record: GraphWalRecord,
}
