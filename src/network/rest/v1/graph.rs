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
use tracing::{debug, error, info, warn};

// For base64 encoding of bytes (using standard library instead)
// use base64;

// Use proto types directly with custom serde implementations
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
            graph_id: "default".to_string(), // TODO: Extract from REST API path
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
                    serde_json::Value::Object(serde_json::Map::new()) // TODO: Proper object conversion
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
            serde_json::Value::Object(serde_json::Map::new()) // TODO: Proper object conversion
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
            } else { n.as_f64().map(Value::DoubleValue) }
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
    chrono::DateTime::from_timestamp_millis(*ts_ms).map_or_else(|| "1970-01-01T00:00:00.000Z".to_string(), |dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// Create the graph REST router with multi-graph support
pub fn create_graph_router() -> Router<AppState> {
    Router::new()
        // Graph collection management endpoints
        .route("/graphs", post(create_graph_collection))
        .route("/graphs", get(list_graph_collections))
        .route("/graphs/:graph_id", get(get_graph_collection))
        .route("/graphs/:graph_id", delete(delete_graph_collection))
        .route("/graphs/:graph_id/schema", put(update_graph_schema))
        // Multi-graph node operations
        .route("/graphs/:graph_id/nodes", post(create_node))
        .route("/graphs/:graph_id/nodes/:id", get(get_node))
        .route("/graphs/:graph_id/nodes/:id", put(update_node))
        .route("/graphs/:graph_id/nodes/:id", delete(delete_node))
        .route(
            "/graphs/:graph_id/nodes/:id/neighbors",
            get(get_node_neighbors),
        )
        // Multi-graph edge operations
        .route("/graphs/:graph_id/edges", post(create_edge))
        .route("/graphs/:graph_id/edges/:id", get(get_edge))
        .route("/graphs/:graph_id/edges/:id", put(update_edge))
        .route("/graphs/:graph_id/edges/:id", delete(delete_edge))
        // Multi-graph traversal and querying
        .route("/graphs/:graph_id/traverse", post(traverse_graph))
        .route("/graphs/:graph_id/shortest_path", post(shortest_path))
        .route("/graphs/:graph_id/query/nodes", post(query_nodes))
        .route("/graphs/:graph_id/query/edges", post(query_edges))
        // Declarative graph query (Cypher)
        .route("/graphs/:graph_id/query", post(execute_graph_query))
        // Multi-graph batch operations
        .route("/graphs/:graph_id/nodes/batch", post(batch_create_nodes))
        .route("/graphs/:graph_id/edges/batch", post(batch_create_edges))
        // Multi-graph statistics
        .route("/graphs/:graph_id/stats", get(get_graph_stats))
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
        .route("/graphs/:graph_id/cycles", get(check_cycles))
        // PULSAR/QUASAR advanced graph operations
        .route("/graphs/:graph_id/engine", post(create_graph_with_engine))
        .route("/graphs/:graph_id/pulsar/stats", get(get_pulsar_stats))
        .route("/graphs/:graph_id/pulsar/query", post(cross_shard_query))
        .route("/graphs/:graph_id/pulsar/rebalance", post(rebalance_shards))
        .route("/graphs/:graph_id/quasar/stats", get(get_quasar_stats))
        .route("/graphs/:graph_id/quasar/tiers", get(get_tier_stats))
        .route("/graphs/:graph_id/quasar/migrate", post(trigger_migration))
        // Legacy compatibility endpoints (deprecated; redirect to canonical multi-graph routes)
        .route("/nodes", post(create_node_legacy))
        .route("/nodes/:id", get(get_node_legacy))
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
            && let Ok(n) = v.parse::<usize>() {
                req.prefetch_budget = Some(n);
            }
    match app_state
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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

/// Get an edge by ID
pub async fn get_edge(
    State(app_state): State<AppState>,
    Path((graph_id, edge_id)): Path<(String, String)>,
) -> impl IntoResponse {
    debug!("Getting edge: {} from graph: {}", edge_id, graph_id);

    match app_state
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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

    // TODO: Read per-call overrides from headers (temporarily disabled)
    let override_enable_prefetch = None;
    let override_prefetch_budget = None;

    match app_state
        .unified_handlers
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
                && let Ok(n) = rest.parse::<u32>() {
                    q.offset = Some(n);
                }

    match app_state
        .unified_handlers
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
                && let Ok(n) = rest.parse::<u32>() {
                    q.offset = Some(n);
                }
    match app_state
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
        .unified_handlers
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
    // TODO: Implement schema update once GraphSchema is properly defined
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
                    "id": m.id,
                    "score": m.score,
                    "metadata": m.metadata
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

// ===== PULSAR/QUASAR Advanced Graph Operations =====

/// Request for creating a graph with a specific engine
#[derive(Debug, Deserialize)]
pub struct CreateGraphWithEngineRequest {
    pub graph_id: String,
    #[serde(default)]
    pub engine_type: String,
    pub pulsar_config: Option<serde_json::Value>,
    pub quasar_config: Option<serde_json::Value>,
}

/// Create a graph with a specific engine type (ORION, PULSAR, or QUASAR)
pub async fn create_graph_with_engine(
    State(app_state): State<AppState>,
    Json(request): Json<CreateGraphWithEngineRequest>,
) -> impl IntoResponse {
    info!(
        "Creating graph {} with engine type {}",
        request.graph_id, request.engine_type
    );

    // Map engine type string to proto enum
    let engine_type = match request.engine_type.to_lowercase().as_str() {
        "orion" => crate::graph::service::service_advanced::GraphEngineTypeProto::Orion,
        "pulsar" => crate::graph::service::service_advanced::GraphEngineTypeProto::Pulsar,
        "quasar" => crate::graph::service::service_advanced::GraphEngineTypeProto::Quasar,
        _ => {
            let error = GraphError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "Unknown engine type: {}. Valid options: orion, pulsar, quasar",
                    request.engine_type
                ),
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(GraphResponse::<serde_json::Value>::error(error)),
            )
                .into_response();
        }
    };

    let service_request = crate::graph::service::service_advanced::CreateGraphWithEngineRequest {
        graph_id: request.graph_id.clone(),
        engine_type,
        pulsar_config: request
            .pulsar_config
            .map(|v| serde_json::from_value(v).unwrap_or_default()),
        quasar_config: request
            .quasar_config
            .map(|v| serde_json::from_value(v).unwrap_or_default()),
    };

    match app_state
        .unified_handlers
        .graph_operations_service
        .create_graph_with_engine(service_request)
        .await
    {
        Ok(response) => {
            info!(
                "Graph {} created successfully with engine {:?}",
                request.graph_id, engine_type
            );
            let body = serde_json::json!({
                "success": response.success,
                "message": response.message,
                "engine_type": format!("{:?}", response.created_engine_type),
            });
            (StatusCode::CREATED, Json(GraphResponse::success(body))).into_response()
        }
        Err(e) => {
            error!("Failed to create graph {}: {}", request.graph_id, e);
            let graph_error = GraphError::new(ErrorCode::InternalError, e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<serde_json::Value>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Get PULSAR distributed graph statistics
pub async fn get_pulsar_stats(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
) -> impl IntoResponse {
    debug!("Getting PULSAR stats for graph: {}", graph_id);

    let request = crate::proto::v1::GetStatsRequest {
        graph_id: graph_id.clone(),
    };

    match app_state
        .unified_handlers
        .graph_operations_service
        .get_pulsar_stats(request)
        .await
    {
        Ok(stats) => (
            StatusCode::OK,
            Json(GraphResponse::success(
                serde_json::to_value(stats).unwrap_or_default(),
            )),
        )
            .into_response(),
        Err(e) => {
            error!("Failed to get PULSAR stats for graph {}: {}", graph_id, e);
            let graph_error = GraphError::new(ErrorCode::InternalError, e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<serde_json::Value>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Request for cross-shard query
#[derive(Debug, Deserialize)]
pub struct CrossShardQueryRequest {
    pub graph_id: String,
    pub query: String,
    #[serde(default)]
    pub shard_ids: Vec<String>,
}

/// Execute cross-shard query (PULSAR only)
pub async fn cross_shard_query(
    State(app_state): State<AppState>,
    Json(request): Json<CrossShardQueryRequest>,
) -> impl IntoResponse {
    info!(
        "Executing cross-shard query for graph: {}",
        request.graph_id
    );

    let service_request = crate::graph::service::service_advanced::CrossShardQueryRequest {
        graph_id: request.graph_id.clone(),
        query: request.query.clone(),
        shard_ids: request.shard_ids.clone(),
    };

    match app_state
        .unified_handlers
        .graph_operations_service
        .cross_shard_query(service_request)
        .await
    {
        Ok(response) => (
            StatusCode::OK,
            Json(GraphResponse::success(
                serde_json::to_value(response).unwrap_or_default(),
            )),
        )
            .into_response(),
        Err(e) => {
            error!(
                "Cross-shard query failed for graph {}: {}",
                request.graph_id, e
            );
            let graph_error = GraphError::new(ErrorCode::InternalError, e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<serde_json::Value>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Request for rebalancing shards
#[derive(Debug, Deserialize)]
pub struct RebalanceShardsRequest {
    pub graph_id: String,
    #[serde(default)]
    pub shard_ids: Vec<String>,
    #[serde(default)]
    pub force: bool,
}

/// Rebalance shards (PULSAR only)
pub async fn rebalance_shards(
    State(app_state): State<AppState>,
    Json(request): Json<RebalanceShardsRequest>,
) -> impl IntoResponse {
    info!("Rebalancing shards for graph: {}", request.graph_id);

    let service_request = crate::graph::service::service_advanced::RebalanceShardsRequest {
        graph_id: request.graph_id.clone(),
        shard_ids: request.shard_ids.clone(),
        force: request.force,
    };

    match app_state
        .unified_handlers
        .graph_operations_service
        .rebalance_shards(service_request)
        .await
    {
        Ok(response) => {
            let body = serde_json::json!({
                "success": response.success,
                "message": response.message,
                "rebalanced_shards": response.rebalanced_shards,
            });
            (StatusCode::OK, Json(GraphResponse::success(body))).into_response()
        }
        Err(e) => {
            error!(
                "Failed to rebalance shards for graph {}: {}",
                request.graph_id, e
            );
            let graph_error = GraphError::new(ErrorCode::InternalError, e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<serde_json::Value>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Get QUASAR tiering statistics
pub async fn get_quasar_stats(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
) -> impl IntoResponse {
    debug!("Getting QUASAR stats for graph: {}", graph_id);

    let request = crate::proto::v1::GetStatsRequest {
        graph_id: graph_id.clone(),
    };

    match app_state
        .unified_handlers
        .graph_operations_service
        .get_quasar_stats(request)
        .await
    {
        Ok(stats) => (
            StatusCode::OK,
            Json(GraphResponse::success(
                serde_json::to_value(stats).unwrap_or_default(),
            )),
        )
            .into_response(),
        Err(e) => {
            error!("Failed to get QUASAR stats for graph {}: {}", graph_id, e);
            let graph_error = GraphError::new(ErrorCode::InternalError, e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<serde_json::Value>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Get detailed tier statistics (QUASAR only)
pub async fn get_tier_stats(
    State(app_state): State<AppState>,
    Path(graph_id): Path<String>,
) -> impl IntoResponse {
    debug!("Getting tier stats for graph: {}", graph_id);

    let request = crate::graph::service::service_advanced::GetTierStatsRequest {
        graph_id: graph_id.clone(),
        tier_name: None,
    };

    match app_state
        .unified_handlers
        .graph_operations_service
        .get_tier_stats(request)
        .await
    {
        Ok(response) => (
            StatusCode::OK,
            Json(GraphResponse::success(
                serde_json::to_value(response).unwrap_or_default(),
            )),
        )
            .into_response(),
        Err(e) => {
            error!("Failed to get tier stats for graph {}: {}", graph_id, e);
            let graph_error = GraphError::new(ErrorCode::InternalError, e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<serde_json::Value>::error(graph_error)),
            )
                .into_response()
        }
    }
}

/// Request for triggering migration
#[derive(Debug, Deserialize)]
pub struct TriggerMigrationRequest {
    pub graph_id: String,
    #[serde(default)]
    pub node_ids: Vec<String>,
    pub target_tier: String,
}

/// Trigger manual tier migration (QUASAR only)
pub async fn trigger_migration(
    State(app_state): State<AppState>,
    Json(request): Json<TriggerMigrationRequest>,
) -> impl IntoResponse {
    info!("Triggering migration for graph: {}", request.graph_id);

    let service_request = crate::graph::service::service_advanced::TriggerMigrationRequest {
        graph_id: request.graph_id.clone(),
        node_ids: request.node_ids.clone(),
        target_tier: request.target_tier.clone(),
    };

    match app_state
        .unified_handlers
        .graph_operations_service
        .trigger_migration(service_request)
        .await
    {
        Ok(response) => {
            let body = serde_json::json!({
                "success": response.success,
                "message": response.message,
                "migrated_node_ids": response.migrated_node_ids,
            });
            (StatusCode::OK, Json(GraphResponse::success(body))).into_response()
        }
        Err(e) => {
            error!(
                "Failed to trigger migration for graph {}: {}",
                request.graph_id, e
            );
            let graph_error = GraphError::new(ErrorCode::InternalError, e.to_string());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphResponse::<serde_json::Value>::error(graph_error)),
            )
                .into_response()
        }
    }
}
