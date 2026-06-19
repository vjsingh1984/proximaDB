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

//! # REST API Endpoints for Graph Operations
//!
//! This module provides RESTful HTTP endpoints for ProximaDB's native graph database,
//! following REST principles with JSON serialization and comprehensive error handling.
//!
//! ## API Design Philosophy
//!
//! - **Resource-Oriented**: Clear REST endpoints for nodes and edges
//! - **JSON Serialization**: Proto messages converted to/from JSON  
//! - **HTTP Status Codes**: Proper 2xx, 4xx, 5xx response codes
//! - **Error Handling**: Detailed error messages with context
//! - **Performance**: Direct service integration without overhead
//!
//! ## Endpoint Overview
//!
//! ```text
//! POST   /api/v1/graph/graphs                          - Create graph collection
//! GET    /api/v1/graph/graphs                          - List graph collections
//! POST   /api/v1/graph/graphs/{graph_id}/nodes         - Create node
//! GET    /api/v1/graph/graphs/{graph_id}/nodes/{id}    - Get node by ID
//! POST   /api/v1/graph/graphs/{graph_id}/edges         - Create edge
//! GET    /api/v1/graph/graphs/{graph_id}/edges/{id}    - Get edge by ID
//! GET    /api/v1/graph/graphs/{graph_id}/stats         - Graph statistics
//! POST   /api/v1/graph/graphs/{graph_id}/traverse      - Graph traversal
//! POST   /api/v1/graph/graphs/{graph_id}/walk          - Agentic GraphWalk (BFS bounded)
//! POST   /api/v1/graph/graphs/{graph_id}/step          - Agentic single-step navigation
//! POST   /api/v1/graph/graphs/{graph_id}/shortest_path - Dijkstra shortest path
//! POST   /api/v1/graph/graphs/{graph_id}/query         - Declarative graph query
//! POST   /api/v1/graph/graphs/{graph_id}/nodes/batch   - Batch create nodes
//! POST   /api/v1/graph/graphs/{graph_id}/edges/batch   - Batch create edges
//! ```
//!
//! Legacy compatibility routes (`/api/v1/graph/nodes`, `/api/v1/graph/edges`, etc.)
//! return `308 Permanent Redirect` with deprecation metadata and a canonical
//! target route. Sunset date: `2026-06-30`.
//!
//! ## Request/Response Format
//!
//! All endpoints use JSON serialization with proto message compatibility.
//! Proto timestamps are converted to ISO 8601 strings for JSON compatibility.

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

// For base64 encoding of bytes (using standard library instead)
// use base64;

// Use proto types directly with custom serde implementations
use crate::graph::engines::GraphEngine;
use crate::graph::rag::{
    KHopSubgraphBuilder, RagBudget, RagPipeline, RagQuery, Subgraph, VectorNodeRetriever,
};
use crate::network::rest::v1::handlers::AppState;
use crate::proto::proximadb_v1::{Edge, Node};
use crate::proto::proximadb_v1::{EmbeddingVersion, PropertyValue};

// Import canonical types for consistent API responses
use crate::graph::canonical::{
    BatchResults, CanonicalEdge, CanonicalNode, CanonicalPath, ErrorCode, GraphError,
    GraphResponse, QueryResults, ShortestPathResult, TraversalResults, TraversalStats,
};

/// REST-compatible TraversalRequest wrapper for JSON deserialization
#[derive(Debug, serde::Deserialize)]
pub struct RestTraversalRequest {
    /// Starting node ID for the traversal
    start_node_id: String,
    /// Maximum depth to traverse
    max_depth: u32,
    /// Edge types to follow (empty means all types)
    edge_types: Vec<String>,
    /// Node labels to filter (empty means all labels)
    node_labels: Vec<String>,
    /// Whether to return the full path
    _return_path: bool,
    /// Traversal algorithm (bfs, dfs, parallel_bfs)
    algorithm: String,
}

/// Agentic GraphWalk request: bounded BFS expansion in one call.
///
/// Maps to `GraphOperationsService::graph_walk()`. Tradeoff vs `WalkStepRequest`:
/// `walk` returns up to `limit` nodes within `max_depth` hops in a single response;
/// `step` returns the immediate neighbors of one node, leaving the agent to drive.
#[derive(Debug, serde::Deserialize)]
pub struct WalkRequest {
    /// Starting node ID
    pub start_node_id: String,
    /// Maximum BFS depth (0 = unbounded)
    #[serde(default = "default_walk_depth")]
    pub max_depth: u32,
    /// Maximum number of nodes to return
    #[serde(default = "default_walk_limit")]
    pub limit: u32,
}

fn default_walk_depth() -> u32 {
    2
}

fn default_walk_limit() -> u32 {
    100
}

/// Agentic single-step navigation request: return the neighbors of one node.
///
/// Maps to `GraphOperationsService::graph_step()`. The agent calls this
/// repeatedly to walk the graph, picking which neighbor to step to next, so
/// the database never has to materialize a subgraph that doesn't fit in the
/// agent's context window (arXiv:2604.01610 GraphWalk pattern).
#[derive(Debug, serde::Deserialize)]
pub struct WalkStepRequest {
    /// Current node ID
    pub node_id: String,
    /// Optional edge-type filter (empty = all edges)
    #[serde(default)]
    pub edge_type: Option<String>,
    /// Maximum neighbors to return (0 = no cap, but you should set one)
    #[serde(default = "default_step_limit")]
    pub limit: u32,
}

fn default_step_limit() -> u32 {
    50
}

/// REST-compatible NodeQuery wrapper for JSON deserialization
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RestNodeQuery {
    /// Node labels to filter by
    labels: Vec<String>,
    /// Property filters as key-value pairs
    properties: HashMap<String, serde_json::Value>,
    /// Maximum number of results to return
    limit: u32,
    /// Offset for pagination
    offset: Option<u32>,
    /// Token for continuing a previous query
    continuation_token: Option<String>,
}

/// REST-compatible EdgeQuery wrapper for JSON deserialization
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RestEdgeQuery {
    /// Edge type to filter by
    edge_type: String,
    /// Optional source node ID
    from_node_id: Option<String>,
    /// Optional target node ID
    to_node_id: Option<String>,
    /// Property filters as key-value pairs
    properties: HashMap<String, serde_json::Value>,
    /// Maximum number of results to return
    limit: u32,
    /// Offset for pagination
    offset: Option<u32>,
    /// Token for continuing a previous query
    continuation_token: Option<String>,
}

/// Request for Modular Graph RAG (RGL) retrieval.
#[derive(Debug, serde::Deserialize)]
pub struct RagRequest {
    /// Seed retrieval query (text).
    #[serde(default)]
    pub query: String,
    /// Seed retrieval vector (optional).
    #[serde(default)]
    pub query_vector: Option<Vec<f32>>,
    /// Node labels to filter seeds (optional).
    #[serde(default)]
    pub allowed_labels: Vec<String>,
    /// Graph budget for retrieval and expansion.
    #[serde(default)]
    pub budget: RestRagBudget,
    /// Collection to use for seed retrieval (default: graph_id).
    pub seed_collection: Option<String>,
    /// Number of hops for subgraph expansion (default: 2).
    #[serde(default = "default_rag_hops")]
    pub hops: u32,
    /// Whether to use LLM-based dynamic node filtering (TD-045).
    #[serde(default)]
    pub use_llm_filter: bool,
}

fn default_rag_hops() -> u32 {
    2
}

/// Budget constraints for RAG retrieval.
#[derive(Debug, serde::Deserialize)]
pub struct RestRagBudget {
    /// Maximum number of seed nodes to retrieve.
    #[serde(default = "default_max_seeds")]
    pub max_seeds: usize,
    /// Maximum number of total nodes in the final subgraph.
    #[serde(default = "default_max_subgraph_nodes")]
    pub max_subgraph_nodes: usize,
}

impl Default for RestRagBudget {
    fn default() -> Self {
        Self {
            max_seeds: default_max_seeds(),
            max_subgraph_nodes: default_max_subgraph_nodes(),
        }
    }
}

fn default_max_seeds() -> usize {
    10
}

fn default_max_subgraph_nodes() -> usize {
    100
}

/// Subgraph response for RAG.
#[derive(Debug, serde::Serialize)]
pub struct RestSubgraph {
    /// Node IDs in the subgraph.
    pub nodes: Vec<String>,
    /// Edges in the subgraph.
    pub edges: Vec<RestSubgraphEdge>,
}

/// One edge in the RAG subgraph.
#[derive(Debug, serde::Serialize)]
pub struct RestSubgraphEdge {
    pub from: String,
    pub to: String,
    pub edge_type: String,
}

impl From<Subgraph> for RestSubgraph {
    fn from(s: Subgraph) -> Self {
        Self {
            nodes: s.nodes,
            edges: s
                .edges
                .into_iter()
                .map(|e| RestSubgraphEdge {
                    from: e.from,
                    to: e.to,
                    edge_type: e.edge_type,
                })
                .collect(),
        }
    }
}

// Conversion implementations for REST types to Proto types
impl From<RestTraversalRequest> for crate::proto::proximadb_v1::TraversalRequest {
    fn from(rest: RestTraversalRequest) -> Self {
        // Convert algorithm string to enum value
        // graph.proto: BFS=1, DFS=2, PARALLEL_BFS=3
        let algorithm = match rest.algorithm.to_ascii_lowercase().as_str() {
            "bfs" => 1,
            "dfs" => 2,
            "parallel_bfs" | "pbfs" | "parallel" => 3,
            _ => 0, // Unspecified
        };

        crate::proto::proximadb_v1::TraversalRequest {
            // The proto field is unused downstream — `traverse_with_overrides`
            // takes `graph_id: &str` as an explicit first arg (passed from the
            // REST Path<String> extractor). Leaving empty rather than "default"
            // to prevent the hardcode from looking like a fallback that
            // actually shapes behavior.
            graph_id: String::new(),
            start_node_id: rest.start_node_id,
            max_depth: rest.max_depth,
            edge_types: rest.edge_types,
            node_labels: rest.node_labels,
            filters: vec![], // Filters not supported in this wrapper
            algorithm,
            limit: None,
            timeout_ms: None,
            max_frontier: None,
        }
    }
}

impl From<RestNodeQuery> for crate::proto::proximadb_v1::NodeQuery {
    fn from(rest: RestNodeQuery) -> Self {
        // Convert properties map into equals PropertyFilter list
        let mut filters: Vec<crate::proto::proximadb_v1::PropertyFilter> = Vec::new();
        for (k, v) in rest.properties {
            filters.push(crate::proto::proximadb_v1::PropertyFilter {
                key: k,
                operator: crate::proto::proximadb_v1::PropertyFilterOperator::Equals as i32,
                value: Some(convert_json_to_property_value(v)),
            });
        }

        crate::proto::proximadb_v1::NodeQuery {
            graph_id: "default".to_string(), // Path param used by handler
            labels: rest.labels,
            filters,
            limit: Some(rest.limit),
            offset: rest.offset,
            continuation_token: rest.continuation_token,
        }
    }
}

impl From<RestEdgeQuery> for crate::proto::proximadb_v1::EdgeQuery {
    fn from(rest: RestEdgeQuery) -> Self {
        // Convert properties map into equals PropertyFilter list
        let mut filters: Vec<crate::proto::proximadb_v1::PropertyFilter> = Vec::new();
        for (k, v) in rest.properties {
            filters.push(crate::proto::proximadb_v1::PropertyFilter {
                key: k,
                operator: crate::proto::proximadb_v1::PropertyFilterOperator::Equals as i32,
                value: Some(convert_json_to_property_value(v)),
            });
        }

        crate::proto::proximadb_v1::EdgeQuery {
            graph_id: "default".to_string(), // Path param used by handler
            from_node_id: rest.from_node_id,
            to_node_id: rest.to_node_id,
            edge_types: if rest.edge_type.is_empty() {
                vec![]
            } else {
                vec![rest.edge_type]
            },
            filters,
            limit: Some(rest.limit),
            offset: rest.offset,
            continuation_token: rest.continuation_token,
        }
    }
}

/// REST-compatible Node wrapper for JSON serialization
#[derive(Debug, Serialize, Clone)]
#[allow(dead_code)]
struct RestNode {
    id: String,
    labels: Vec<String>,
    properties: HashMap<String, serde_json::Value>,
    embedding: Option<RestEmbeddingVersion>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// REST-compatible Edge wrapper for JSON serialization
#[derive(Debug, Serialize, Clone)]
#[allow(dead_code)]
struct RestEdge {
    id: String,
    from_node_id: String,
    to_node_id: String,
    edge_type: String,
    properties: HashMap<String, serde_json::Value>,
    weight: Option<f64>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// REST-compatible EmbeddingVersion wrapper
#[derive(Debug, Serialize, Clone)]
#[allow(dead_code)]
struct RestEmbeddingVersion {
    vector: Vec<f32>,
    version: String,
}

/// REST input for creating/updating nodes
#[derive(Debug, Deserialize)]
pub struct RestNodeInput {
    /// Unique node identifier
    id: String,
    /// Node labels (e.g., Person, Organization)
    labels: Vec<String>,
    /// Node properties as key-value pairs
    properties: HashMap<String, serde_json::Value>,
    /// Optional embedding vector with version info
    embedding: Option<RestEmbeddingVersionInput>,
}

/// REST input for creating/updating edges
#[derive(Debug, Deserialize)]
pub struct RestEdgeInput {
    /// Unique edge identifier
    id: String,
    /// Source node ID
    from_node_id: String,
    /// Target node ID
    to_node_id: String,
    /// Edge type (e.g., KNOWS, WORKS_FOR)
    edge_type: String,
    /// Edge properties as key-value pairs
    properties: HashMap<String, serde_json::Value>,
    /// Optional edge weight for weighted algorithms
    weight: Option<f64>,
}

/// REST input for embedding version
#[derive(Debug, Deserialize)]
struct RestEmbeddingVersionInput {
    vector: Vec<f32>,
    version: String,
}

/// REST-compatible TraversalResponse wrapper
#[derive(Debug, Serialize, Clone)]
#[allow(dead_code)]
struct RestTraversalResponse {
    nodes: Vec<RestNode>,
    edges: Vec<RestEdge>,
    paths: Vec<RestGraphPath>,
    stats: Option<RestTraversalStats>,
}

/// REST-compatible GraphPath wrapper
#[derive(Debug, Serialize, Clone)]
#[allow(dead_code)]
struct RestGraphPath {
    node_ids: Vec<String>,
}

/// REST-compatible TraversalStats wrapper
#[derive(Debug, Serialize, Clone)]
#[allow(dead_code)]
struct RestTraversalStats {
    nodes_visited: u64,
    edges_traversed: u64,
    depth_reached: u32,
}

/// REST-compatible GraphStats wrapper
#[derive(Debug, Serialize, Clone)]
struct RestGraphStats {
    total_nodes: u64,
    total_edges: u64,
    label_stats: Vec<RestLabelStats>,
    edge_type_stats: Vec<RestEdgeTypeStats>,
    total_properties: u64,
    memory_usage_bytes: u64,
    average_degree: f64,
    max_degree: u32,
    connected_components: u32,
}

/// REST-compatible LabelStats wrapper
#[derive(Debug, Serialize, Clone)]
struct RestLabelStats {
    label: String,
    count: u64,
}

/// REST-compatible EdgeTypeStats wrapper
#[derive(Debug, Serialize, Clone)]
struct RestEdgeTypeStats {
    edge_type: String,
    count: u64,
}

// NOTE: Legacy response types have been replaced with canonical types from
// crate::graph::canonical. The following types are now deprecated:
// - GraphErrorResponse -> use GraphError from canonical
// - GraphSuccessResponse -> use GraphResponse::success() from canonical
// - GraphBatchResponse -> use BatchResults from canonical
// - GraphQueryResponse -> use QueryResults from canonical

/// Create node request
#[derive(Debug, Deserialize)]
pub struct CreateNodeRequest {
    /// Node data to create
    node: RestNodeInput,
}

/// Create edge request
#[derive(Debug, Deserialize)]
pub struct CreateEdgeRequest {
    /// Edge data to create
    edge: RestEdgeInput,
}

/// Batch create nodes request
#[derive(Debug, Deserialize)]
pub struct BatchCreateNodesRequest {
    /// Nodes to create
    nodes: Vec<RestNodeInput>,
    /// Conflict resolution strategy: "update", "skip", or "error"
    if_exists: Option<String>,
}

/// Batch create edges request
#[derive(Debug, Deserialize)]
pub struct BatchCreateEdgesRequest {
    /// Edges to create
    edges: Vec<RestEdgeInput>,
    /// Conflict resolution strategy: "update", "skip", or "error"
    if_exists: Option<String>,
}

// Conversion functions
impl From<&Node> for RestNode {
    fn from(node: &Node) -> Self {
        RestNode {
            id: node.id.clone(),
            labels: node.labels.clone(),
            properties: convert_properties_to_json(&node.properties),
            embedding: node.embedding.as_ref().map(RestEmbeddingVersion::from),
            created_at: Some(format_timestamp(&node.created_at_ms)),
            updated_at: Some(format_timestamp(&node.updated_at_ms)),
        }
    }
}

impl From<&Edge> for RestEdge {
    fn from(edge: &Edge) -> Self {
        RestEdge {
            id: edge.id.clone(),
            from_node_id: edge.from_node_id.clone(),
            to_node_id: edge.to_node_id.clone(),
            edge_type: edge.edge_type.clone(),
            properties: convert_properties_to_json(&edge.properties),
            weight: edge.weight,
            created_at: Some(format_timestamp(&edge.created_at_ms)),
            updated_at: Some(format_timestamp(&edge.updated_at_ms)),
        }
    }
}

impl From<&EmbeddingVersion> for RestEmbeddingVersion {
    fn from(embed: &EmbeddingVersion) -> Self {
        RestEmbeddingVersion {
            vector: embed.vector.clone(),
            version: embed.model_version.clone(),
        }
    }
}

impl From<&crate::proto::proximadb_v1::TraversalResponse> for RestTraversalResponse {
    fn from(response: &crate::proto::proximadb_v1::TraversalResponse) -> Self {
        RestTraversalResponse {
            nodes: response.nodes.iter().map(RestNode::from).collect(),
            edges: response.edges.iter().map(RestEdge::from).collect(),
            paths: response.paths.iter().map(RestGraphPath::from).collect(),
            stats: response.stats.as_ref().map(RestTraversalStats::from),
        }
    }
}

impl From<&crate::proto::proximadb_v1::GraphPath> for RestGraphPath {
    fn from(path: &crate::proto::proximadb_v1::GraphPath) -> Self {
        // Convert entities to node IDs (GraphPath has entities and relations, not direct node_ids)
        let node_ids: Vec<String> = path.entities.iter().map(|e| e.id.clone()).collect();
        RestGraphPath { node_ids }
    }
}

impl From<&crate::proto::proximadb_v1::TraversalStats> for RestTraversalStats {
    fn from(stats: &crate::proto::proximadb_v1::TraversalStats) -> Self {
        RestTraversalStats {
            nodes_visited: stats.nodes_visited as u64,
            edges_traversed: stats.edges_traversed as u64,
            depth_reached: stats.max_depth_reached,
        }
    }
}

impl From<&crate::proto::proximadb_v1::GraphStats> for RestGraphStats {
    fn from(stats: &crate::proto::proximadb_v1::GraphStats) -> Self {
        RestGraphStats {
            total_nodes: stats.total_nodes,
            total_edges: stats.total_edges,
            label_stats: stats.label_stats.iter().map(RestLabelStats::from).collect(),
            edge_type_stats: stats
                .edge_type_stats
                .iter()
                .map(RestEdgeTypeStats::from)
                .collect(),
            total_properties: stats.total_properties,
            memory_usage_bytes: stats.memory_usage_bytes,
            average_degree: stats.average_degree,
            max_degree: stats.max_degree,
            connected_components: stats.connected_components,
        }
    }
}

impl From<&crate::proto::proximadb_v1::LabelStats> for RestLabelStats {
    fn from(stats: &crate::proto::proximadb_v1::LabelStats) -> Self {
        RestLabelStats {
            label: stats.label.clone(),
            count: stats.count,
        }
    }
}

impl From<&crate::proto::proximadb_v1::EdgeTypeStats> for RestEdgeTypeStats {
    fn from(stats: &crate::proto::proximadb_v1::EdgeTypeStats) -> Self {
        RestEdgeTypeStats {
            edge_type: stats.edge_type.clone(),
            count: stats.count,
        }
    }
}

impl From<RestNodeInput> for Node {
    fn from(input: RestNodeInput) -> Self {
        Node {
            id: input.id,
            labels: input.labels,
            properties: convert_json_to_properties(input.properties),
            embedding: input.embedding.map(|e| {
                let dimension = e.vector.len() as u32;
                EmbeddingVersion {
                    vector: e.vector,
                    model_version: e.version,
                    model_id: String::new(), // Set default empty string
                    dimension,
                    created_at_ms: 0,
                    model_params: std::collections::HashMap::new(),
                    modality: 0, // Default to first modality value
                }
            }),
            created_at_ms: 0, // Set by service
            updated_at_ms: 0, // Set by service
        }
    }
}

impl From<RestEdgeInput> for Edge {
    fn from(input: RestEdgeInput) -> Self {
        Edge {
            id: input.id,
            from_node_id: input.from_node_id,
            to_node_id: input.to_node_id,
            edge_type: input.edge_type,
            properties: convert_json_to_properties(input.properties),
            weight: input.weight,
            created_at_ms: 0, // Set by service
            updated_at_ms: 0, // Set by service
        }
    }
}

#[allow(dead_code)]
fn convert_properties_to_json(
    props: &HashMap<String, PropertyValue>,
) -> HashMap<String, serde_json::Value> {
    props
        .iter()
        .map(|(k, v)| {
            let json_val = match &v.value {
                Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => {
                    serde_json::Value::String(s.clone())
                }
                Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => {
                    serde_json::Value::Number(serde_json::Number::from(*i))
                }
                Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(f)) => {
                    serde_json::Number::from_f64(*f)
                        .map_or(serde_json::Value::Null, serde_json::Value::Number)
                }
                Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => {
                    serde_json::Value::Bool(*b)
                }
                Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(arr)) => {
                    serde_json::Value::Array(
                        arr.values
                            .iter()
                            .map(convert_property_value_to_json)
                            .collect(),
                    )
                }
                Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(b)) => {
                    serde_json::Value::String(format!("{:?}", b)) // Convert to debug string for now
                }
                Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_obj)) => {
                    serde_json::Value::Object(serde_json::Map::new()) // Deferred: Proper object conversion
                }
                Some(crate::proto::proximadb_v1::property_value::Value::VectorValue(vec)) => {
                    serde_json::Value::Array(
                        vec.values
                            .iter()
                            .map(|f| {
                                serde_json::Value::Number(
                                    serde_json::Number::from_f64(*f as f64)
                                        .unwrap_or(serde_json::Number::from(0)),
                                )
                            })
                            .collect(),
                    )
                }
                None => serde_json::Value::Null,
            };
            (k.clone(), json_val)
        })
        .collect()
}

fn convert_json_to_properties(
    props: HashMap<String, serde_json::Value>,
) -> HashMap<String, PropertyValue> {
    props
        .into_iter()
        .map(|(k, v)| {
            let prop_val = convert_json_to_property_value(v);
            (k, prop_val)
        })
        .collect()
}

#[allow(dead_code)]
fn convert_property_value_to_json(prop: &PropertyValue) -> serde_json::Value {
    match &prop.value {
        Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => {
            serde_json::Value::String(s.clone())
        }
        Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => {
            serde_json::Value::Number(serde_json::Number::from(*i))
        }
        Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(f)) => {
            serde_json::Number::from_f64(*f)
                .map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => {
            serde_json::Value::Bool(*b)
        }
        Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(arr)) => {
            serde_json::Value::Array(
                arr.values
                    .iter()
                    .map(convert_property_value_to_json)
                    .collect(),
            )
        }
        Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(b)) => {
            serde_json::Value::String(format!("{:?}", b)) // Convert to debug string for now
        }
        Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_obj)) => {
            serde_json::Value::Object(serde_json::Map::new()) // Deferred: Proper object conversion
        }
        Some(crate::proto::proximadb_v1::property_value::Value::VectorValue(vec)) => {
            serde_json::Value::Array(
                vec.values
                    .iter()
                    .map(|f| {
                        serde_json::Value::Number(
                            serde_json::Number::from_f64(*f as f64)
                                .unwrap_or(serde_json::Number::from(0)),
                        )
                    })
                    .collect(),
            )
        }
        None => serde_json::Value::Null,
    }
}

fn convert_json_to_property_value(value: serde_json::Value) -> PropertyValue {
    // PropertyValue is now a struct, not enum - use direct field access;
    use crate::proto::proximadb_v1::property_value::Value;
    let prop_value = match value {
        serde_json::Value::String(s) => Some(Value::StringValue(s)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Value::IntValue(i))
            } else {
                n.as_f64().map(Value::DoubleValue)
            }
        }
        serde_json::Value::Bool(b) => Some(Value::BoolValue(b)),
        serde_json::Value::Array(arr) => {
            let values: Vec<PropertyValue> = arr
                .into_iter()
                .map(convert_json_to_property_value)
                .collect();
            Some(Value::ArrayValue(
                crate::proto::proximadb_v1::PropertyArray { values },
            ))
        }
        _ => None,
    };
    PropertyValue { value: prop_value }
}

fn format_timestamp(ts_ms: &i64) -> String {
    // Convert Unix epoch milliseconds to ISO 8601 string
    chrono::DateTime::from_timestamp_millis(*ts_ms).map_or_else(
        || "1970-01-01T00:00:00.000Z".to_string(),
        |dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
    )
}

/// Create the graph REST router with multi-graph support
pub fn create_graph_router() -> Router<AppState> {
    Router::new()
        // Graph collection management endpoints
        .route("/graphs", post(create_graph_collection))
        .route("/graphs", get(list_graph_collections))
        .route("/graphs/{graph_id}", get(get_graph_collection))
        .route("/graphs/{graph_id}", delete(delete_graph_collection))
        .route("/graphs/{graph_id}/schema", put(update_graph_schema))
        // Multi-graph node operations
        .route("/graphs/{graph_id}/nodes", post(create_node))
        .route("/graphs/{graph_id}/nodes/{id}", get(get_node))
        .route("/graphs/{graph_id}/nodes/{id}", put(update_node))
        .route("/graphs/{graph_id}/nodes/{id}", delete(delete_node))
        .route(
            "/graphs/:graph_id/nodes/:id/neighbors",
            get(get_node_neighbors),
        )
        // Multi-graph edge operations
        .route("/graphs/{graph_id}/edges", post(create_edge))
        .route("/graphs/{graph_id}/edges/{id}", get(get_edge))
        .route("/graphs/{graph_id}/edges/{id}", put(update_edge))
        .route("/graphs/{graph_id}/edges/{id}", delete(delete_edge))
        // Multi-graph traversal and querying
        .route("/graphs/{graph_id}/traverse", post(traverse_graph))
        // Agentic GraphWalk surface (TD-046, arXiv:2604.01610)
        .route("/graphs/{graph_id}/walk", post(walk_graph))
        .route("/graphs/{graph_id}/step", post(step_graph))
        .route("/graphs/{graph_id}/shortest_path", post(shortest_path))
        .route("/graphs/{graph_id}/query/nodes", post(query_nodes))
        .route("/graphs/{graph_id}/query/edges", post(query_edges))
        // Declarative graph query (Cypher)
        .route("/graphs/{graph_id}/query", post(execute_graph_query))
        // Modular Graph RAG (RGL, TD-045)
        .route("/graphs/{graph_id}/rag", post(rag_query))
        // Multi-graph batch operations
        .route("/graphs/{graph_id}/nodes/batch", post(batch_create_nodes))
        .route("/graphs/{graph_id}/edges/batch", post(batch_create_edges))
        // Multi-graph statistics
        .route("/graphs/{graph_id}/stats", get(get_graph_stats))
        // Multi-graph constraints DDL
        .route(
            "/graphs/:graph_id/constraints/unique",
            post(add_unique_constraint),
        )
        .route(
            "/graphs/:graph_id/constraints/unique",
            delete(remove_unique_constraint),
        )
        // Multi-graph analysis
        .route(
            "/graphs/:graph_id/components",
            get(get_connected_components),
        )
        .route("/graphs/{graph_id}/cycles", get(check_cycles))
        // Legacy compatibility endpoints (deprecated; redirect to canonical multi-graph routes)
        .route("/nodes", post(create_node_legacy))
        .route("/nodes/{id}", get(get_node_legacy))
        .route("/edges", post(create_edge_legacy))
        .route("/stats", get(get_graph_stats_legacy))
}

/// Create a new node
pub async fn create_node(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    debug!(
        "Creating node: {:?} in graph: {}",
        request.node.id, graph_id
    );

    // Convert REST input to proto Node
    let proto_node: Node = request.node.into();

    match app_state
        .request_handlers
        .graph_operations_service
        .create_node(&graph_id, proto_node)
        .await
    {
        Ok(node) => {
            info!("Successfully created node: {}", node.id);
            let canonical_node = CanonicalNode::from_proto(&node);
            (
                StatusCode::CREATED,
                Json(GraphResponse::success(canonical_node)),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to create node: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<CanonicalNode>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Get a node by ID
pub async fn get_node(
    State(app_state): State<AppState>,
    Path((graph_id, node_id)): Path<(String, String)>,
) -> impl IntoResponse {
    debug!("Getting node: {} from graph: {}", node_id, graph_id);

    match app_state
        .request_handlers
        .graph_operations_service
        .get_node(&graph_id, &node_id)
        .await
    {
        Ok(Some(node)) => {
            info!("Successfully retrieved node: {}", node_id);
            let canonical_node = CanonicalNode::from_proto(&node);
            Json(GraphResponse::success(canonical_node)).into_response()
        }
        Ok(None) => {
            warn!("Node not found: {}", node_id);
            let graph_error = GraphError::not_found("Node", &node_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphResponse::<CanonicalNode>::error(graph_error)),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to get node {}: {}", node_id, err);
            let graph_error = GraphError::internal(err.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<CanonicalNode>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Update a node
pub async fn update_node(
    State(app_state): State<AppState>,
    Path((graph_id, node_id)): Path<(String, String)>,
    Json(mut node_input): Json<RestNodeInput>,
) -> impl IntoResponse {
    debug!("Updating node: {} in graph: {}", node_id, graph_id);

    // Ensure the node ID matches the path parameter
    node_input.id = node_id.clone();

    // Convert REST input to proto Node
    let proto_node: Node = node_input.into();

    match app_state
        .request_handlers
        .graph_operations_service
        .update_node(&graph_id, proto_node)
        .await
    {
        Ok(updated_node) => {
            info!("Successfully updated node: {}", node_id);
            let canonical_node = CanonicalNode::from_proto(&updated_node);
            Json(GraphResponse::success(canonical_node)).into_response()
        }
        Err(err) => {
            error!("Failed to update node {}: {}", node_id, err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<CanonicalNode>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Delete a node
pub async fn delete_node(
    State(app_state): State<AppState>,
    Path((graph_id, node_id)): Path<(String, String)>,
) -> impl IntoResponse {
    debug!("Deleting node: {} from graph: {}", node_id, graph_id);

    match app_state
        .request_handlers
        .graph_operations_service
        .delete_node(&graph_id, &node_id)
        .await
    {
        Ok(Some(deleted_node)) => {
            info!("Successfully deleted node: {}", node_id);
            let canonical_node = CanonicalNode::from_proto(&deleted_node);
            Json(GraphResponse::success(canonical_node)).into_response()
        }
        Ok(None) => {
            warn!("Node not found for deletion: {}", node_id);
            let graph_error = GraphError::not_found("Node", &node_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphResponse::<CanonicalNode>::error(graph_error)),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to delete node {}: {}", node_id, err);
            let graph_error = GraphError::internal(err.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<CanonicalNode>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Get neighbors of a node
pub async fn get_node_neighbors(
    State(app_state): State<AppState>,
    Path((graph_id, node_id)): Path<(String, String)>,
) -> impl IntoResponse {
    debug!(
        "Getting neighbors for node: {} in graph: {}",
        node_id, graph_id
    );

    match app_state
        .request_handlers
        .graph_operations_service
        .get_neighbors(&graph_id, &node_id)
        .await
    {
        Ok(neighbors) => {
            info!(
                "Successfully retrieved {} neighbors for node: {}",
                neighbors.len(),
                node_id
            );
            let canonical_nodes: Vec<CanonicalNode> = neighbors
                .into_iter()
                .map(|n| CanonicalNode::from_proto(&n))
                .collect();
            Json(GraphResponse::success(canonical_nodes)).into_response()
        }
        Err(err) => {
            error!("Failed to get neighbors for node {}: {}", node_id, err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<Vec<CanonicalNode>>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Create a new edge
pub async fn create_edge(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<CreateEdgeRequest>,
) -> impl IntoResponse {
    debug!(
        "Creating edge: {:?} in graph: {}",
        request.edge.id, graph_id
    );

    // Convert REST input to proto Edge
    let proto_edge: Edge = request.edge.into();

    match app_state
        .request_handlers
        .graph_operations_service
        .create_edge(&graph_id, proto_edge)
        .await
    {
        Ok(edge) => {
            info!("Successfully created edge: {}", edge.id);
            let canonical_edge = CanonicalEdge::from_proto(&edge);
            (
                StatusCode::CREATED,
                Json(GraphResponse::success(canonical_edge)),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to create edge: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<CanonicalEdge>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Request parameters for shortest path computation
#[derive(Debug, Deserialize)]
pub struct ShortestPathRequest {
    /// Starting node ID
    start_node_id: String,
    /// Target node ID
    target_node_id: String,
    /// Maximum search depth
    max_depth: Option<u32>,
    /// Edge types to traverse (empty means all)
    edge_types: Option<Vec<String>>,
    /// Algorithm: "DIJKSTRA" or "ASTAR"
    algorithm: Option<String>,
    /// K for k-shortest paths
    k: Option<u32>,
    /// Enable prefetch optimization
    enable_prefetch: Option<bool>,
    /// Prefetch budget (number of nodes)
    prefetch_budget: Option<usize>,
}

/// Request to create a unique constraint on a graph property
#[derive(Debug, Deserialize)]
pub struct UniqueConstraintRequest {
    /// Node label to constrain
    label: String,
    /// Property that must be unique
    property: String,
}

#[derive(Debug, Serialize)]
struct DdlResponse {
    success: bool,
}

/// Compute shortest path using Dijkstra algorithm
pub async fn shortest_path(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    headers: HeaderMap,
    Json(mut req): Json<ShortestPathRequest>,
) -> impl IntoResponse {
    // Header-based overrides if JSON fields not provided
    if req.enable_prefetch.is_none()
        && let Some(v) = headers
            .get("x-graph-prefetch-enabled")
            .and_then(|v| v.to_str().ok())
    {
        req.enable_prefetch = Some(v.eq_ignore_ascii_case("true") || v == "1");
    }
    if req.prefetch_budget.is_none()
        && let Some(v) = headers
            .get("x-graph-prefetch-budget")
            .and_then(|v| v.to_str().ok())
        && let Ok(n) = v.parse::<usize>()
    {
        req.prefetch_budget = Some(n);
    }
    match app_state
        .request_handlers
        .graph_operations_service
        .shortest_path(
            &graph_id,
            &req.start_node_id,
            &req.target_node_id,
            req.max_depth,
            req.edge_types,
            parse_sp_algorithm(req.algorithm.as_deref()),
            req.k,
            req.enable_prefetch,
            req.prefetch_budget,
        )
        .await
    {
        Ok(Some((path, total_weight))) => {
            let result = ShortestPathResult::found(path, total_weight);
            Json(GraphResponse::success(result)).into_response()
        }
        Ok(None) => {
            let result = ShortestPathResult::not_found();
            // Return success with not_found result (the result itself indicates no path)
            Json(GraphResponse::success(result)).into_response()
        }
        Err(e) => {
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, e.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<ShortestPathResult>::error(graph_error)),
            )
                .into_response()
        }
    }
}

fn parse_sp_algorithm(
    s: Option<&str>,
) -> Option<crate::proto::proximadb_v1::ShortestPathAlgorithm> {
    match s.unwrap_or("DIJKSTRA").to_ascii_uppercase().as_str() {
        "ASTAR" => Some(crate::proto::proximadb_v1::ShortestPathAlgorithm::Astar),
        "DIJKSTRA" => Some(crate::proto::proximadb_v1::ShortestPathAlgorithm::Dijkstra),
        _ => None,
    }
}

/// Add unique constraint (label, property)
pub async fn add_unique_constraint(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(req): Json<UniqueConstraintRequest>,
) -> impl IntoResponse {
    match app_state
        .request_handlers
        .graph_operations_service
        .add_unique_constraint(&graph_id, &req.label, &req.property)
        .await
    {
        Ok(()) => Json(GraphResponse::success(DdlResponse { success: true })).into_response(),
        Err(e) => {
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, e.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<DdlResponse>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Remove unique constraint (label, property)
pub async fn remove_unique_constraint(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(req): Json<UniqueConstraintRequest>,
) -> impl IntoResponse {
    // remove_unique_constraint now returns Result and is async
    match app_state
        .request_handlers
        .graph_operations_service
        .remove_unique_constraint(&graph_id, &req.label, &req.property)
        .await
    {
        Ok(()) => Json(GraphResponse::success(DdlResponse { success: true })).into_response(),
        Err(e) => {
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, e.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<DdlResponse>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Get connected components (weakly connected)
pub async fn get_connected_components(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
) -> impl IntoResponse {
    match app_state
        .request_handlers
        .graph_operations_service
        .connected_components(&graph_id)
        .await
    {
        Ok(components) => {
            // Use a lightweight wrapper for components
            #[derive(Debug, Serialize)]
            struct ComponentsData {
                components: Vec<Vec<String>>,
            }
            Json(GraphResponse::success(ComponentsData { components })).into_response()
        }
        Err(e) => {
            let graph_error = GraphError::internal(e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<Vec<Vec<String>>>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Detect directed cycles
pub async fn check_cycles(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
) -> impl IntoResponse {
    match app_state
        .request_handlers
        .graph_operations_service
        .has_cycle(&graph_id)
        .await
    {
        Ok(has) => {
            // Use a lightweight wrapper for cycle detection result
            #[derive(Debug, Serialize)]
            struct CycleData {
                has_cycle: bool,
            }
            Json(GraphResponse::success(CycleData { has_cycle: has })).into_response()
        }
        Err(e) => {
            let graph_error = GraphError::internal(e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<bool>::error(graph_error)),
            )
                .into_response()
        }
    }
}

pub async fn rag_query(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<RagRequest>,
) -> impl IntoResponse {
    let seed_collection = request
        .seed_collection
        .clone()
        .unwrap_or_else(|| graph_id.clone());

    let engine = match app_state
        .request_handlers
        .graph_operations_service
        .get_or_create_graph_engine(&graph_id)
        .await
    {
        Ok(e) => e,
        Err(err) => {
            let graph_error = GraphError::new(
                ErrorCode::InvalidArgument,
                format!("Graph '{}' not found: {}", graph_id, err),
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<RestSubgraph>::error(graph_error)),
            )
                .into_response();
        }
    };

    let retriever = VectorNodeRetriever::new(
        app_state.vector_operations_service.clone(),
        seed_collection,
        request.budget.max_seeds,
    );

    let builder =
        KHopSubgraphBuilder::new(engine.clone() as Arc<dyn GraphEngine>, request.hops, None);

    let budget = RagBudget {
        max_seeds: request.budget.max_seeds,
        max_subgraph_nodes: request.budget.max_subgraph_nodes,
    };

    let rag_query = RagQuery {
        query: request.query,
        query_vector: request.query_vector,
        allowed_labels: request.allowed_labels,
    };

    if request.use_llm_filter {
        if app_state.llm_engine.is_some() {
            warn!(
                "LLM filter requested for graph {} but no LLM-aware node filter is currently wired; using the standard RAG pipeline",
                graph_id
            );
        } else {
            warn!(
                "LLM filter requested but LLM engine not available; using the standard RAG pipeline"
            );
        }
    }

    let pipeline = RagPipeline::without_filter(retriever, builder, budget);
    execute_rag_pipeline(pipeline, &rag_query, &graph_id).await
}

/// Helper to execute the pipeline and format the response.
async fn execute_rag_pipeline<R, B, F>(
    pipeline: RagPipeline<R, B, F>,
    query: &RagQuery,
    graph_id: &str,
) -> Response
where
    R: crate::graph::rag::NodeRetriever,
    B: crate::graph::rag::SubgraphBuilder,
    F: crate::graph::rag::NodeFilter,
{
    match pipeline.run(query).await {
        Ok(subgraph) => {
            info!(
                "Successfully executed RGL query for graph {}: {} nodes, {} edges",
                graph_id,
                subgraph.nodes.len(),
                subgraph.edges.len()
            );
            Json(GraphResponse::success(RestSubgraph::from(subgraph))).into_response()
        }
        Err(err) => {
            error!("RGL query failed for graph {}: {}", graph_id, err);
            let graph_error = GraphError::internal(err.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<RestSubgraph>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Get an edge by ID
pub async fn get_edge(
    State(app_state): State<AppState>,
    Path((graph_id, edge_id)): Path<(String, String)>,
) -> impl IntoResponse {
    debug!("Getting edge: {} from graph: {}", edge_id, graph_id);

    match app_state
        .request_handlers
        .graph_operations_service
        .get_edge(&graph_id, &edge_id)
        .await
    {
        Ok(Some(edge)) => {
            info!("Successfully retrieved edge: {}", edge_id);
            let canonical_edge = CanonicalEdge::from_proto(&edge);
            Json(GraphResponse::success(canonical_edge)).into_response()
        }
        Ok(None) => {
            warn!("Edge not found: {}", edge_id);
            let graph_error = GraphError::not_found("Edge", &edge_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphResponse::<CanonicalEdge>::error(graph_error)),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to get edge {}: {}", edge_id, err);
            let graph_error = GraphError::internal(err.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<CanonicalEdge>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Update an edge
pub async fn update_edge(
    State(app_state): State<AppState>,
    Path((graph_id, edge_id)): Path<(String, String)>,
    Json(mut edge_input): Json<RestEdgeInput>,
) -> impl IntoResponse {
    debug!("Updating edge: {} in graph: {}", edge_id, graph_id);

    // Ensure the edge ID matches the path parameter
    edge_input.id = edge_id.clone();

    // Convert REST input to proto Edge
    let proto_edge: Edge = edge_input.into();

    match app_state
        .request_handlers
        .graph_operations_service
        .update_edge(&graph_id, proto_edge)
        .await
    {
        Ok(updated_edge) => {
            info!("Successfully updated edge: {}", edge_id);
            let canonical_edge = CanonicalEdge::from_proto(&updated_edge);
            Json(GraphResponse::success(canonical_edge)).into_response()
        }
        Err(err) => {
            error!("Failed to update edge {}: {}", edge_id, err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<CanonicalEdge>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Delete an edge
pub async fn delete_edge(
    State(app_state): State<AppState>,
    Path((graph_id, edge_id)): Path<(String, String)>,
) -> impl IntoResponse {
    debug!("Deleting edge: {} from graph: {}", edge_id, graph_id);

    match app_state
        .request_handlers
        .graph_operations_service
        .delete_edge(&graph_id, &edge_id)
        .await
    {
        Ok(Some(deleted_edge)) => {
            info!("Successfully deleted edge: {}", edge_id);
            let canonical_edge = CanonicalEdge::from_proto(&deleted_edge);
            Json(GraphResponse::success(canonical_edge)).into_response()
        }
        Ok(None) => {
            warn!("Edge not found for deletion: {}", edge_id);
            let graph_error = GraphError::not_found("Edge", &edge_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphResponse::<CanonicalEdge>::error(graph_error)),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to delete edge {}: {}", edge_id, err);
            let graph_error = GraphError::internal(err.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<CanonicalEdge>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Perform graph traversal
pub async fn traverse_graph(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<RestTraversalRequest>,
) -> impl IntoResponse {
    debug!(
        "Starting graph traversal from node: {} in graph: {}",
        request.start_node_id, graph_id
    );

    // Deferred: Read per-call overrides from headers (temporarily disabled)
    let override_enable_prefetch = None;
    let override_prefetch_budget = None;

    match app_state
        .request_handlers
        .graph_operations_service
        .traverse_with_overrides(
            &graph_id,
            request.into(),
            override_enable_prefetch,
            override_prefetch_budget,
        )
        .await
    {
        Ok(response) => {
            info!("Successfully completed graph traversal");
            // Convert to canonical TraversalResults
            let canonical_nodes: Vec<CanonicalNode> = response
                .nodes
                .iter()
                .map(CanonicalNode::from_proto)
                .collect();
            let canonical_edges: Vec<CanonicalEdge> = response
                .edges
                .iter()
                .map(CanonicalEdge::from_proto)
                .collect();
            let canonical_paths: Option<Vec<CanonicalPath>> = if response.paths.is_empty() {
                None
            } else {
                Some(
                    response
                        .paths
                        .iter()
                        .map(|p| {
                            CanonicalPath::from_node_ids(
                                p.entities.iter().map(|e| e.id.clone()).collect(),
                            )
                        })
                        .collect(),
                )
            };
            let stats = response.stats.as_ref().map(TraversalStats::from_proto);

            let traversal_results = TraversalResults {
                nodes: canonical_nodes,
                edges: canonical_edges,
                paths: canonical_paths,
                stats,
            };
            Json(GraphResponse::success(traversal_results)).into_response()
        }
        Err(err) => {
            error!("Failed to traverse graph: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<TraversalResults>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Bounded BFS expansion for agentic graph navigation.
///
/// Returns up to `limit` nodes within `max_depth` hops of `start_node_id`.
/// See `WalkRequest` for the tradeoff vs single-step `step_graph`.
pub async fn walk_graph(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<WalkRequest>,
) -> impl IntoResponse {
    debug!(
        "GraphWalk: graph={} start={} max_depth={} limit={}",
        graph_id, request.start_node_id, request.max_depth, request.limit
    );

    match app_state
        .request_handlers
        .graph_operations_service
        .graph_walk(
            &graph_id,
            &request.start_node_id,
            request.max_depth,
            request.limit as usize,
        )
        .await
    {
        Ok(results) => Json(GraphResponse::success(results)).into_response(),
        Err(err) => {
            error!("GraphWalk failed for graph {}: {}", graph_id, err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<TraversalResults>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Single-step graph navigation: return the immediate neighbors of one node.
///
/// The starting node is included as the first entry in `nodes` so the agent
/// has its own properties without an extra round trip. Use this primitive in a
/// loop when the agent needs to drive traversal step by step.
pub async fn step_graph(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<WalkStepRequest>,
) -> impl IntoResponse {
    debug!(
        "GraphStep: graph={} node={} edge_type={:?} limit={}",
        graph_id, request.node_id, request.edge_type, request.limit
    );

    match app_state
        .request_handlers
        .graph_operations_service
        .graph_step(
            &graph_id,
            &request.node_id,
            request.edge_type.as_deref(),
            request.limit as usize,
        )
        .await
    {
        Ok(results) => Json(GraphResponse::success(results)).into_response(),
        Err(err) => {
            error!("GraphStep failed for graph {}: {}", graph_id, err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<TraversalResults>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Query nodes by labels and properties
pub async fn query_nodes(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(query): Json<RestNodeQuery>,
) -> impl IntoResponse {
    debug!(
        "Querying nodes with labels: {:?} in graph: {}",
        query.labels, graph_id
    );
    let mut q = query;
    // Continuation token support: format "offset:<n>"
    if q.offset.is_none()
        && let Some(token) = &q.continuation_token
        && let Some(rest) = token.strip_prefix("offset:")
        && let Ok(n) = rest.parse::<u32>()
    {
        q.offset = Some(n);
    }

    match app_state
        .request_handlers
        .graph_operations_service
        .query_nodes(&graph_id, q.clone().into())
        .await
    {
        Ok(nodes) => {
            info!("Successfully queried {} nodes", nodes.len());
            let lim = q.limit;
            let has_more = (nodes.len() as u32) == lim;
            let canonical_nodes: Vec<CanonicalNode> = nodes
                .into_iter()
                .map(|n| CanonicalNode::from_proto(&n))
                .collect();

            let mut query_results = QueryResults::new(canonical_nodes, has_more);
            if has_more {
                let next_off = q.offset.unwrap_or(0).saturating_add(lim);
                query_results = query_results.with_next_token(format!("offset:{}", next_off));
            }
            Json(GraphResponse::success(query_results)).into_response()
        }
        Err(err) => {
            error!("Failed to query nodes: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<QueryResults<CanonicalNode>>::error(
                    graph_error,
                )),
            )
                .into_response()
        }
    }
}

/// Query edges by types and properties
pub async fn query_edges(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(query): Json<RestEdgeQuery>,
) -> impl IntoResponse {
    debug!("Querying edges in graph: {}", graph_id);
    let mut q = query;
    if q.offset.is_none()
        && let Some(token) = &q.continuation_token
        && let Some(rest) = token.strip_prefix("offset:")
        && let Ok(n) = rest.parse::<u32>()
    {
        q.offset = Some(n);
    }
    match app_state
        .request_handlers
        .graph_operations_service
        .query_edges(&graph_id, q.clone().into())
        .await
    {
        Ok(edges) => {
            info!("Successfully queried {} edges", edges.len());
            let lim = q.limit;
            let has_more = (edges.len() as u32) == lim;
            let canonical_edges: Vec<CanonicalEdge> = edges
                .into_iter()
                .map(|e| CanonicalEdge::from_proto(&e))
                .collect();

            let mut query_results = QueryResults::new(canonical_edges, has_more);
            if has_more {
                let next_off = q.offset.unwrap_or(0).saturating_add(lim);
                query_results = query_results.with_next_token(format!("offset:{}", next_off));
            }
            Json(GraphResponse::success(query_results)).into_response()
        }
        Err(err) => {
            error!("Failed to query edges: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<QueryResults<CanonicalEdge>>::error(
                    graph_error,
                )),
            )
                .into_response()
        }
    }
}

/// Batch create nodes
pub async fn batch_create_nodes(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<BatchCreateNodesRequest>,
) -> impl IntoResponse {
    debug!(
        "Batch creating {} nodes in graph: {}",
        request.nodes.len(),
        graph_id
    );
    let strategy = request.if_exists.unwrap_or_else(|| "error".into());

    // Convert REST inputs to proto Nodes
    let proto_nodes: Vec<Node> = request.nodes.into_iter().map(|n| n.into()).collect();

    match app_state
        .request_handlers
        .graph_operations_service
        .batch_create_nodes_with_strategy(&graph_id, proto_nodes, strategy.as_str())
        .await
    {
        Ok(nodes) => {
            info!("Successfully batch created {} nodes", nodes.len());
            let canonical_nodes: Vec<CanonicalNode> = nodes
                .into_iter()
                .map(|n| CanonicalNode::from_proto(&n))
                .collect();
            let batch_results = BatchResults::new(canonical_nodes);
            Json(GraphResponse::success(batch_results)).into_response()
        }
        Err(err) => {
            error!("Failed to batch create nodes: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<BatchResults<CanonicalNode>>::error(
                    graph_error,
                )),
            )
                .into_response()
        }
    }
}

/// Batch create edges
pub async fn batch_create_edges(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<BatchCreateEdgesRequest>,
) -> impl IntoResponse {
    debug!(
        "Batch creating {} edges in graph: {}",
        request.edges.len(),
        graph_id
    );
    let _strategy = request.if_exists.clone().unwrap_or_else(|| "error".into());

    // Convert REST inputs to proto Edges
    let proto_edges: Vec<Edge> = request.edges.into_iter().map(|e| e.into()).collect();

    match app_state
        .request_handlers
        .graph_operations_service
        .batch_create_edges(&graph_id, proto_edges)
        .await
    {
        Ok(edges) => {
            info!("Successfully batch created {} edges", edges.len());
            let canonical_edges: Vec<CanonicalEdge> = edges
                .into_iter()
                .map(|e| CanonicalEdge::from_proto(&e))
                .collect();
            let batch_results = BatchResults::new(canonical_edges);
            Json(GraphResponse::success(batch_results)).into_response()
        }
        Err(err) => {
            error!("Failed to batch create edges: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<BatchResults<CanonicalEdge>>::error(
                    graph_error,
                )),
            )
                .into_response()
        }
    }
}

/// Get graph statistics
pub async fn get_graph_stats(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
) -> impl IntoResponse {
    debug!("Getting graph statistics for graph: {}", graph_id);

    match app_state
        .request_handlers
        .graph_operations_service
        .get_stats(&graph_id)
        .await
    {
        Ok(stats) => {
            info!("Successfully retrieved graph statistics");
            let rest_stats = RestGraphStats::from(&stats);
            Json(GraphResponse::success(rest_stats)).into_response()
        }
        Err(err) => {
            error!("Failed to get graph statistics: {}", err);
            let graph_error = GraphError::internal(err.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<RestGraphStats>::error(graph_error)),
            )
                .into_response()
        }
    }
}

// ====================================================================
// Legacy Compatibility Handlers (using default graph)
// ====================================================================

const DEFAULT_GRAPH_ID: &str = "default";
const LEGACY_GRAPH_SUNSET_DATE: &str = "2026-06-30";

fn legacy_graph_redirect(canonical_path: String) -> Response {
    warn!(
        canonical_route = %canonical_path,
        sunset_date = LEGACY_GRAPH_SUNSET_DATE,
        "Legacy graph endpoint is deprecated; redirecting to canonical multi-graph route"
    );

    let mut response = StatusCode::PERMANENT_REDIRECT.into_response();

    if let Ok(location_value) = HeaderValue::from_str(&canonical_path) {
        response
            .headers_mut()
            .insert(header::LOCATION, location_value.clone());
        response.headers_mut().insert(
            header::HeaderName::from_static("x-proximadb-canonical-route"),
            location_value,
        );
    }

    response.headers_mut().insert(
        header::HeaderName::from_static("deprecation"),
        HeaderValue::from_static("true"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("sunset"),
        HeaderValue::from_static(LEGACY_GRAPH_SUNSET_DATE),
    );
    response
}

/// Legacy create node handler (uses default graph)
pub async fn create_node_legacy() -> impl IntoResponse {
    legacy_graph_redirect(format!("/api/v1/graph/graphs/{}/nodes", DEFAULT_GRAPH_ID))
}

/// Legacy get node handler (uses default graph)
pub async fn get_node_legacy(Path(node_id): Path<String>) -> impl IntoResponse {
    legacy_graph_redirect(format!(
        "/api/v1/graph/graphs/{}/nodes/{}",
        DEFAULT_GRAPH_ID, node_id
    ))
}

/// Legacy create edge handler (uses default graph)
pub async fn create_edge_legacy() -> impl IntoResponse {
    legacy_graph_redirect(format!("/api/v1/graph/graphs/{}/edges", DEFAULT_GRAPH_ID))
}

/// Legacy get graph stats handler (uses default graph)
pub async fn get_graph_stats_legacy() -> impl IntoResponse {
    legacy_graph_redirect(format!("/api/v1/graph/graphs/{}/stats", DEFAULT_GRAPH_ID))
}

// ============================================================================
// Graph Collection Management Handlers
// ============================================================================

/// Input for creating a graph collection
#[derive(Debug, Deserialize)]
pub struct CreateGraphCollectionRequest {
    /// Unique graph identifier
    graph_id: String,
    /// Human-readable name
    name: Option<String>,
    /// Graph description
    description: Option<String>,
}

/// Response for graph collection operations - contains collection metadata
#[derive(Debug, Serialize)]
struct GraphCollectionData {
    graph_id: String,
    name: String,
    description: String,
    created_at: String,
    updated_at: String,
}

/// Create a new graph collection
pub async fn create_graph_collection(
    State(app_state): State<AppState>,
    Json(request): Json<CreateGraphCollectionRequest>,
) -> impl IntoResponse {
    let create_request = crate::proto::proximadb_v1::CreateGraphRequest {
        graph_id: request.graph_id.clone(),
        name: request.name,
        description: request.description,
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };

    match app_state
        .request_handlers
        .graph_collection_service
        .create_graph(create_request)
        .await
    {
        Ok(collection) => {
            info!(
                "Successfully created graph collection: {}",
                collection.graph_id
            );
            let data = GraphCollectionData {
                graph_id: collection.graph_id.clone(),
                name: collection.name.clone(),
                description: collection.description.clone(),
                created_at: format_timestamp(&collection.created_at),
                updated_at: format_timestamp(&collection.updated_at),
            };
            (StatusCode::CREATED, Json(GraphResponse::success(data))).into_response()
        }
        Err(err) => {
            error!("Failed to create graph collection: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<GraphCollectionData>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// List all graph collections
pub async fn list_graph_collections(State(app_state): State<AppState>) -> impl IntoResponse {
    match app_state
        .request_handlers
        .graph_collection_service
        .list_graphs()
        .await
    {
        Ok(collections) => {
            let items: Vec<GraphCollectionData> = collections
                .iter()
                .map(|c| GraphCollectionData {
                    graph_id: c.graph_id.clone(),
                    name: c.name.clone(),
                    description: c.description.clone(),
                    created_at: format_timestamp(&c.created_at),
                    updated_at: format_timestamp(&c.updated_at),
                })
                .collect();
            Json(GraphResponse::success(items)).into_response()
        }
        Err(err) => {
            error!("Failed to list graph collections: {}", err);
            let graph_error = GraphError::internal(err.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<Vec<GraphCollectionData>>::error(
                    graph_error,
                )),
            )
                .into_response()
        }
    }
}

/// Get a specific graph collection
pub async fn get_graph_collection(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
) -> impl IntoResponse {
    match app_state
        .request_handlers
        .graph_collection_service
        .get_graph(&graph_id)
        .await
    {
        Ok(Some(collection)) => {
            let data = GraphCollectionData {
                graph_id: collection.graph_id.clone(),
                name: collection.name.clone(),
                description: collection.description.clone(),
                created_at: format_timestamp(&collection.created_at),
                updated_at: format_timestamp(&collection.updated_at),
            };
            Json(GraphResponse::success(data)).into_response()
        }
        Ok(None) => {
            let graph_error = GraphError::not_found("Graph collection", &graph_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphResponse::<GraphCollectionData>::error(graph_error)),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to get graph collection: {}", err);
            let graph_error = GraphError::internal(err.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<GraphCollectionData>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Delete a graph collection
pub async fn delete_graph_collection(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
) -> impl IntoResponse {
    match app_state
        .request_handlers
        .graph_collection_service
        .delete_graph(&graph_id)
        .await
    {
        Ok(()) => {
            info!("Successfully deleted graph collection: {}", graph_id);
            #[derive(Debug, Serialize)]
            struct DeleteResult {
                deleted: bool,
                graph_id: String,
            }
            let result = DeleteResult {
                deleted: true,
                graph_id,
            };
            Json(GraphResponse::success(result)).into_response()
        }
        Err(err) => {
            error!("Failed to delete graph collection: {}", err);
            let graph_error = GraphError::new(ErrorCode::InvalidArgument, err.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<serde_json::Value>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Update graph schema
pub async fn update_graph_schema(
    State(_app_state): State<AppState>,
    Path(_graph_id): Path<String>,
    Json(_schema): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Deferred: Implement schema update once GraphSchema is properly defined
    let graph_error = GraphError::new(
        ErrorCode::InvalidArgument,
        "Schema update not yet implemented",
    );
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(GraphResponse::<serde_json::Value>::error(graph_error)),
    )
        .into_response()
}

// ============================================================================
// Declarative Graph Query (Cypher) Handler
// ============================================================================

/// Request body for executing a declarative graph query
#[derive(Debug, Deserialize)]
pub struct GraphQueryRequest {
    /// The Cypher query string
    query: String,
    /// Query language (currently only "cypher" is supported)
    #[serde(default = "default_query_language")]
    _language: String,
}

fn default_query_language() -> String {
    "cypher".to_string()
}

/// Response for graph query execution
#[derive(Debug, Serialize)]
struct GraphQueryResultResponse {
    /// Result rows
    rows: Vec<serde_json::Value>,
    /// Total number of rows returned
    #[allow(dead_code)]
    row_count: u64,
    /// Execution time in milliseconds
    execution_time_ms: f64,
}

/// Execute a declarative graph query (Cypher)
///
/// POST /api/v1/graph/graphs/:graph_id/query
///
/// Request body:
/// ```json
/// {
///   "query": "MATCH (n:Person)-[:KNOWS]->(m) RETURN m.name",
///   "language": "cypher"
/// }
/// ```
///
/// When the `unified-facade-routing` feature is enabled and a query adapter is available,
/// the query is routed through the QueryFacadeAdapter for consistent metrics and tracing.
pub async fn execute_graph_query(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
    Json(request): Json<GraphQueryRequest>,
) -> impl IntoResponse {
    debug!(
        "Executing graph query on graph '{}': {}",
        graph_id,
        request.query.chars().take(100).collect::<String>()
    );

    let start = std::time::Instant::now();

    // Route through unified facade when adapter is available
    if let Some(ref adapter) = app_state.query_adapter {
        debug!("Using unified facade routing for graph query");
        let graph_name = if graph_id.is_empty() || graph_id == "default" {
            None
        } else {
            Some(graph_id.as_str())
        };

        return match adapter.graph_query(&request.query, graph_name).await {
            Ok(result) => {
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

                // Convert QueryResult to response format
                let rows = convert_query_result_to_rows(&result);
                let row_count = rows.len() as u64;

                info!(
                    "Graph query (facade) completed in {:.2}ms with {} rows",
                    elapsed_ms, row_count
                );

                let response = GraphQueryResultResponse {
                    rows,
                    row_count,
                    execution_time_ms: elapsed_ms,
                };
                Json(GraphResponse::success(response)).into_response()
            }
            Err(e) => {
                error!("Graph query (facade) failed: {}", e);
                let graph_error = GraphError::new(ErrorCode::InternalError, e.to_string());
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(GraphResponse::<GraphQueryResultResponse>::error(
                        graph_error,
                    )),
                )
                    .into_response()
            }
        };
    }

    // Legacy path: Return unimplemented error (no direct Cypher execution without facade)
    let graph_error = GraphError::new(
        ErrorCode::InvalidArgument,
        "Declarative query execution requires unified-facade-routing feature. \
         Use /query/nodes or /query/edges for property-based queries, \
         or /traverse for graph traversal.",
    );
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(GraphResponse::<GraphQueryResultResponse>::error(
            graph_error,
        )),
    )
        .into_response()
}

/// Convert QueryResult from the unified facade to JSON rows
fn convert_query_result_to_rows(result: &crate::query::QueryResult) -> Vec<serde_json::Value> {
    use crate::query::QueryResultData;

    match &result.data {
        QueryResultData::Rows(rows) => rows.clone(),
        QueryResultData::VectorResults(matches) => matches
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.record.oid,
                    "score": m.score,
                    "metadata": &m.record.props
                })
            })
            .collect(),
        QueryResultData::Graph(graph_result) => {
            // Convert graph nodes to JSON rows
            graph_result.nodes.clone()
        }
        QueryResultData::Empty => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ================================================================
    // Node CRUD tests
    // ================================================================

    #[test]
    fn test_create_node_request_parsing() {
        let json_input = json!({
            "node": {
                "id": "node-001",
                "labels": ["Person", "Employee"],
                "properties": {
                    "name": "Alice",
                    "age": 30,
                    "active": true
                }
            }
        });

        let request: CreateNodeRequest =
            serde_json::from_value(json_input).expect("CreateNodeRequest should deserialize");
        assert_eq!(request.node.id, "node-001");
        assert_eq!(request.node.labels, vec!["Person", "Employee"]);
        assert_eq!(request.node.properties.len(), 3);
        assert_eq!(request.node.properties["name"], json!("Alice"));
        assert_eq!(request.node.properties["age"], json!(30));
        assert_eq!(request.node.properties["active"], json!(true));

        // Verify conversion to proto Node preserves fields
        let proto_node: Node = request.node.into();
        assert_eq!(proto_node.id, "node-001");
        assert_eq!(proto_node.labels, vec!["Person", "Employee"]);
        assert_eq!(proto_node.properties.len(), 3);
        // Check that the property value roundtrips correctly
        let name_prop = &proto_node.properties["name"];
        match &name_prop.value {
            Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => {
                assert_eq!(s, "Alice");
            }
            other => panic!("Expected StringValue, got {:?}", other),
        }
    }

    #[test]
    fn test_get_node_response_serialization() {
        let node = CanonicalNode {
            id: "node-101".to_string(),
            labels: vec!["Person".to_string(), "Author".to_string()],
            properties: {
                let mut p = HashMap::new();
                p.insert("name".to_string(), json!("Bob"));
                p.insert("score".to_string(), json!(95.5));
                p
            },
            embedding: None,
            created_at: "2026-01-15T10:30:00.000Z".to_string(),
            updated_at: "2026-01-15T10:30:00.000Z".to_string(),
        };

        let response = GraphResponse::success(node);
        let serialized = serde_json::to_value(&response).expect("Should serialize GraphResponse");

        assert_eq!(serialized["success"], json!(true));
        assert!(serialized.get("error").is_none());

        let data = &serialized["data"];
        assert_eq!(data["id"], json!("node-101"));
        assert_eq!(data["labels"], json!(["Person", "Author"]));
        assert_eq!(data["properties"]["name"], json!("Bob"));
        assert_eq!(data["properties"]["score"], json!(95.5));
        assert_eq!(data["created_at"], json!("2026-01-15T10:30:00.000Z"));

        // Verify round-trip: deserialize back
        let roundtrip: GraphResponse<CanonicalNode> =
            serde_json::from_value(serialized).expect("Should deserialize back");
        assert!(roundtrip.success);
        let roundtrip_data = roundtrip.data.expect("data should be present");
        assert_eq!(roundtrip_data.id, "node-101");
        assert_eq!(roundtrip_data.labels.len(), 2);
    }

    #[test]
    fn test_update_node_request_parsing() {
        let json_input = json!({
            "id": "node-002",
            "labels": ["Person"],
            "properties": {
                "name": "Charlie",
                "age": 45,
                "email": "charlie@example.com"
            }
        });

        let node_input: RestNodeInput =
            serde_json::from_value(json_input).expect("RestNodeInput should deserialize");
        assert_eq!(node_input.id, "node-002");
        assert_eq!(node_input.labels, vec!["Person"]);
        assert_eq!(node_input.properties.len(), 3);
        assert_eq!(node_input.properties["email"], json!("charlie@example.com"));

        // Convert to proto and verify property types are correct
        let proto_node: Node = node_input.into();
        assert_eq!(proto_node.id, "node-002");
        let age_prop = &proto_node.properties["age"];
        match &age_prop.value {
            Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => {
                assert_eq!(*i, 45);
            }
            other => panic!("Expected IntValue(45), got {:?}", other),
        }
    }

    #[test]
    fn test_delete_node_request_parsing() {
        // Delete uses path parameters (graph_id, node_id) not a JSON body.
        // Verify that the path tuple can be destructured as the handler expects.
        let graph_id = "social-graph".to_string();
        let node_id = "node-999".to_string();

        // Simulate the path extraction
        let (extracted_graph_id, extracted_node_id): (String, String) =
            (graph_id.clone(), node_id.clone());
        assert_eq!(extracted_graph_id, "social-graph");
        assert_eq!(extracted_node_id, "node-999");

        // Also verify that a CanonicalNode can represent a deleted node response
        let deleted_node = CanonicalNode {
            id: node_id,
            labels: vec!["Person".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            updated_at: "2026-04-04T12:00:00.000Z".to_string(),
        };
        let response = GraphResponse::success(deleted_node);
        let serialized = serde_json::to_value(&response).expect("Should serialize");
        assert_eq!(serialized["data"]["id"], json!("node-999"));
        assert!(serialized["success"].as_bool().unwrap_or(false));
    }

    // ================================================================
    // Edge CRUD tests
    // ================================================================

    #[test]
    fn test_create_edge_request_parsing() {
        let json_input = json!({
            "edge": {
                "id": "edge-001",
                "from_node_id": "node-A",
                "to_node_id": "node-B",
                "edge_type": "KNOWS",
                "properties": {
                    "since": "2020-01-01",
                    "strength": 0.85
                },
                "weight": 1.5
            }
        });

        let request: CreateEdgeRequest =
            serde_json::from_value(json_input).expect("CreateEdgeRequest should deserialize");
        assert_eq!(request.edge.id, "edge-001");
        assert_eq!(request.edge.from_node_id, "node-A");
        assert_eq!(request.edge.to_node_id, "node-B");
        assert_eq!(request.edge.edge_type, "KNOWS");
        assert_eq!(request.edge.properties.len(), 2);
        assert_eq!(request.edge.weight, Some(1.5));

        // Convert to proto Edge and verify
        let proto_edge: Edge = request.edge.into();
        assert_eq!(proto_edge.id, "edge-001");
        assert_eq!(proto_edge.from_node_id, "node-A");
        assert_eq!(proto_edge.to_node_id, "node-B");
        assert_eq!(proto_edge.edge_type, "KNOWS");
        assert_eq!(proto_edge.weight, Some(1.5));
        // Verify property conversion
        let since_prop = &proto_edge.properties["since"];
        match &since_prop.value {
            Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => {
                assert_eq!(s, "2020-01-01");
            }
            other => panic!("Expected StringValue, got {:?}", other),
        }
    }

    #[test]
    fn test_query_edges_request_parsing() {
        let json_input = json!({
            "edge_type": "WORKS_FOR",
            "from_node_id": "person-1",
            "to_node_id": null,
            "properties": {
                "department": "Engineering"
            },
            "limit": 50,
            "offset": 10,
            "continuation_token": null
        });

        let query: RestEdgeQuery =
            serde_json::from_value(json_input).expect("RestEdgeQuery should deserialize");
        assert_eq!(query.edge_type, "WORKS_FOR");
        assert_eq!(query.from_node_id, Some("person-1".to_string()));
        assert!(query.to_node_id.is_none());
        assert_eq!(query.properties.len(), 1);
        assert_eq!(query.limit, 50);
        assert_eq!(query.offset, Some(10));

        // Convert to proto EdgeQuery and verify
        let proto_query: crate::proto::proximadb_v1::EdgeQuery = query.into();
        assert_eq!(proto_query.edge_types, vec!["WORKS_FOR"]);
        assert_eq!(proto_query.from_node_id, Some("person-1".to_string()));
        assert!(proto_query.to_node_id.is_none());
        assert_eq!(proto_query.limit, Some(50));
        assert_eq!(proto_query.offset, Some(10));
        assert_eq!(proto_query.filters.len(), 1);
        assert_eq!(proto_query.filters[0].key, "department");
    }

    #[test]
    fn test_delete_edge_request_parsing() {
        // Delete edge uses path parameters (graph_id, edge_id)
        let graph_id = "knowledge-graph".to_string();
        let edge_id = "edge-abc-123".to_string();

        let (extracted_graph_id, extracted_edge_id): (String, String) =
            (graph_id.clone(), edge_id.clone());
        assert_eq!(extracted_graph_id, "knowledge-graph");
        assert_eq!(extracted_edge_id, "edge-abc-123");

        // Verify canonical edge response for deletion
        let deleted_edge = CanonicalEdge {
            id: edge_id,
            from_node_id: "src-node".to_string(),
            to_node_id: "dst-node".to_string(),
            edge_type: "REFERENCES".to_string(),
            properties: HashMap::new(),
            weight: Some(2.0),
            created_at: "2026-02-01T00:00:00.000Z".to_string(),
            updated_at: "2026-03-15T08:00:00.000Z".to_string(),
        };
        let response = GraphResponse::success(deleted_edge);
        let serialized = serde_json::to_value(&response).expect("Should serialize");
        assert_eq!(serialized["data"]["id"], json!("edge-abc-123"));
        assert_eq!(serialized["data"]["edge_type"], json!("REFERENCES"));
        assert_eq!(serialized["data"]["weight"], json!(2.0));
    }

    // ================================================================
    // Graph operations tests
    // ================================================================

    #[test]
    fn test_graph_query_request_parsing() {
        let json_input = json!({
            "query": "MATCH (n:Person)-[:KNOWS]->(m) WHERE n.name = 'Alice' RETURN m.name, m.age",
            "_language": "cypher"
        });

        let request: GraphQueryRequest =
            serde_json::from_value(json_input).expect("GraphQueryRequest should deserialize");
        assert_eq!(
            request.query,
            "MATCH (n:Person)-[:KNOWS]->(m) WHERE n.name = 'Alice' RETURN m.name, m.age"
        );

        // Test with default language (omitted from JSON)
        let json_minimal = json!({
            "query": "MATCH (n) RETURN n LIMIT 10"
        });
        let request2: GraphQueryRequest =
            serde_json::from_value(json_minimal).expect("Should deserialize with defaults");
        assert_eq!(request2.query, "MATCH (n) RETURN n LIMIT 10");
    }

    #[test]
    fn test_graph_stats_response_serialization() {
        let stats = RestGraphStats {
            total_nodes: 10_000,
            total_edges: 50_000,
            label_stats: vec![
                RestLabelStats {
                    label: "Person".to_string(),
                    count: 5000,
                },
                RestLabelStats {
                    label: "Organization".to_string(),
                    count: 3000,
                },
            ],
            edge_type_stats: vec![
                RestEdgeTypeStats {
                    edge_type: "KNOWS".to_string(),
                    count: 30_000,
                },
                RestEdgeTypeStats {
                    edge_type: "WORKS_FOR".to_string(),
                    count: 20_000,
                },
            ],
            total_properties: 120_000,
            memory_usage_bytes: 52_428_800,
            average_degree: 5.0,
            max_degree: 150,
            connected_components: 3,
        };

        let response = GraphResponse::success(stats);
        let serialized = serde_json::to_value(&response).expect("Should serialize stats response");

        assert_eq!(serialized["success"], json!(true));
        let data = &serialized["data"];
        assert_eq!(data["total_nodes"], json!(10_000));
        assert_eq!(data["total_edges"], json!(50_000));
        assert_eq!(data["average_degree"], json!(5.0));
        assert_eq!(data["max_degree"], json!(150));
        assert_eq!(data["connected_components"], json!(3));
        assert_eq!(data["memory_usage_bytes"], json!(52_428_800));
        assert_eq!(data["total_properties"], json!(120_000));

        // Verify label stats array
        let labels = data["label_stats"].as_array().expect("Should be array");
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0]["label"], json!("Person"));
        assert_eq!(labels[0]["count"], json!(5000));

        // Verify edge type stats array
        let edge_types = data["edge_type_stats"].as_array().expect("Should be array");
        assert_eq!(edge_types.len(), 2);
        assert_eq!(edge_types[0]["edge_type"], json!("KNOWS"));
        assert_eq!(edge_types[0]["count"], json!(30_000));
    }

    #[test]
    fn test_traversal_request_parsing() {
        let json_input = json!({
            "start_node_id": "start-node-42",
            "max_depth": 5,
            "edge_types": ["KNOWS", "FOLLOWS"],
            "node_labels": ["Person"],
            "_return_path": true,
            "algorithm": "bfs"
        });

        let request: RestTraversalRequest =
            serde_json::from_value(json_input).expect("RestTraversalRequest should deserialize");
        assert_eq!(request.start_node_id, "start-node-42");
        assert_eq!(request.max_depth, 5);
        assert_eq!(request.edge_types, vec!["KNOWS", "FOLLOWS"]);
        assert_eq!(request.node_labels, vec!["Person"]);
        assert_eq!(request.algorithm, "bfs");

        // Convert to proto TraversalRequest and verify
        let proto_req: crate::proto::proximadb_v1::TraversalRequest = request.into();
        assert_eq!(proto_req.start_node_id, "start-node-42");
        assert_eq!(proto_req.max_depth, 5);
        assert_eq!(proto_req.edge_types, vec!["KNOWS", "FOLLOWS"]);
        assert_eq!(proto_req.node_labels, vec!["Person"]);
        assert_eq!(proto_req.algorithm, 1); // BFS = 1

        // Test DFS algorithm mapping
        let dfs_input = json!({
            "start_node_id": "n1",
            "max_depth": 3,
            "edge_types": [],
            "node_labels": [],
            "_return_path": false,
            "algorithm": "dfs"
        });
        let dfs_req: RestTraversalRequest = serde_json::from_value(dfs_input).unwrap();
        let proto_dfs: crate::proto::proximadb_v1::TraversalRequest = dfs_req.into();
        assert_eq!(proto_dfs.algorithm, 2); // DFS = 2

        // Test parallel_bfs algorithm mapping
        let pbfs_input = json!({
            "start_node_id": "n2",
            "max_depth": 10,
            "edge_types": [],
            "node_labels": [],
            "_return_path": false,
            "algorithm": "parallel_bfs"
        });
        let pbfs_req: RestTraversalRequest = serde_json::from_value(pbfs_input).unwrap();
        let proto_pbfs: crate::proto::proximadb_v1::TraversalRequest = pbfs_req.into();
        assert_eq!(proto_pbfs.algorithm, 3); // PARALLEL_BFS = 3
    }

    // ================================================================
    // Batch operations tests
    // ================================================================

    #[test]
    fn test_batch_create_nodes_request() {
        let json_input = json!({
            "nodes": [
                {
                    "id": "batch-node-1",
                    "labels": ["Person"],
                    "properties": {"name": "Alice"}
                },
                {
                    "id": "batch-node-2",
                    "labels": ["Person", "Developer"],
                    "properties": {"name": "Bob", "level": 5}
                },
                {
                    "id": "batch-node-3",
                    "labels": ["Organization"],
                    "properties": {"name": "Acme Corp"}
                }
            ],
            "if_exists": "skip"
        });

        let request: BatchCreateNodesRequest =
            serde_json::from_value(json_input).expect("BatchCreateNodesRequest should deserialize");
        assert_eq!(request.nodes.len(), 3);
        assert_eq!(request.if_exists, Some("skip".to_string()));

        // Verify first node
        assert_eq!(request.nodes[0].id, "batch-node-1");
        assert_eq!(request.nodes[0].labels, vec!["Person"]);
        assert_eq!(request.nodes[0].properties["name"], json!("Alice"));

        // Verify second node has multiple labels
        assert_eq!(request.nodes[1].labels, vec!["Person", "Developer"]);
        assert_eq!(request.nodes[1].properties["level"], json!(5));

        // Convert all to proto and verify count
        let proto_nodes: Vec<Node> = request.nodes.into_iter().map(|n| n.into()).collect();
        assert_eq!(proto_nodes.len(), 3);
        assert_eq!(proto_nodes[2].labels, vec!["Organization"]);

        // Test with default if_exists (omitted)
        let json_no_strategy = json!({
            "nodes": [
                {
                    "id": "n1",
                    "labels": [],
                    "properties": {}
                }
            ]
        });
        let req2: BatchCreateNodesRequest =
            serde_json::from_value(json_no_strategy).expect("Should deserialize without if_exists");
        assert!(req2.if_exists.is_none());
    }

    #[test]
    fn test_batch_create_edges_request() {
        let json_input = json!({
            "edges": [
                {
                    "id": "batch-edge-1",
                    "from_node_id": "node-A",
                    "to_node_id": "node-B",
                    "edge_type": "KNOWS",
                    "properties": {"since": "2024"},
                    "weight": 1.0
                },
                {
                    "id": "batch-edge-2",
                    "from_node_id": "node-B",
                    "to_node_id": "node-C",
                    "edge_type": "WORKS_FOR",
                    "properties": {},
                    "weight": null
                }
            ],
            "if_exists": "update"
        });

        let request: BatchCreateEdgesRequest =
            serde_json::from_value(json_input).expect("BatchCreateEdgesRequest should deserialize");
        assert_eq!(request.edges.len(), 2);
        assert_eq!(request.if_exists, Some("update".to_string()));

        // Verify first edge
        assert_eq!(request.edges[0].id, "batch-edge-1");
        assert_eq!(request.edges[0].from_node_id, "node-A");
        assert_eq!(request.edges[0].to_node_id, "node-B");
        assert_eq!(request.edges[0].edge_type, "KNOWS");
        assert_eq!(request.edges[0].weight, Some(1.0));

        // Verify second edge has no weight
        assert_eq!(request.edges[1].edge_type, "WORKS_FOR");
        assert!(request.edges[1].weight.is_none());

        // Convert to proto and verify
        let proto_edges: Vec<Edge> = request.edges.into_iter().map(|e| e.into()).collect();
        assert_eq!(proto_edges.len(), 2);
        assert_eq!(proto_edges[0].weight, Some(1.0));
        assert_eq!(proto_edges[1].weight, None);
        assert_eq!(proto_edges[1].from_node_id, "node-B");
        assert_eq!(proto_edges[1].to_node_id, "node-C");
    }
}
