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
//! POST   /api/v1/graph/nodes           - Create node
//! GET    /api/v1/graph/nodes/{id}      - Get node by ID
//! PUT    /api/v1/graph/nodes/{id}      - Update node  
//! DELETE /api/v1/graph/nodes/{id}      - Delete node
//! POST   /api/v1/graph/edges           - Create edge
//! GET    /api/v1/graph/edges/{id}      - Get edge by ID
//! PUT    /api/v1/graph/edges/{id}      - Update edge
//! DELETE /api/v1/graph/edges/{id}      - Delete edge
//! GET    /api/v1/graph/nodes/{id}/neighbors - Get node neighbors
//! POST   /api/v1/graph/traverse        - Graph traversal
//! POST   /api/v1/graph/shortest_path   - Dijkstra shortest path
//! POST   /api/v1/graph/constraints/unique   - Add unique constraint
//! DELETE /api/v1/graph/constraints/unique   - Remove unique constraint
//! GET    /api/v1/graph/components       - Connected components (weak)
//! GET    /api/v1/graph/cycles           - Detect directed cycles
//! GET    /api/v1/graph/stats           - Graph statistics
//! POST   /api/v1/graph/nodes/batch     - Batch create nodes
//! POST   /api/v1/graph/edges/batch     - Batch create edges
//! POST   /api/v1/graph/query/nodes     - Query nodes
//! POST   /api/v1/graph/query/edges     - Query edges
//! ```
//!
//! ## Request/Response Format
//!
//! All endpoints use JSON serialization with proto message compatibility.
//! Proto timestamps are converted to ISO 8601 strings for JSON compatibility.

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
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
    start_node_id: String,
    max_depth: u32,
    edge_types: Vec<String>,
    node_labels: Vec<String>,
    return_path: bool,
    algorithm: String,
}

/// REST-compatible NodeQuery wrapper for JSON deserialization
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RestNodeQuery {
    labels: Vec<String>,
    properties: HashMap<String, serde_json::Value>,
    limit: u32,
    offset: Option<u32>,
    continuation_token: Option<String>,
}

/// REST-compatible EdgeQuery wrapper for JSON deserialization
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RestEdgeQuery {
    edge_type: String,
    from_node_id: Option<String>,
    to_node_id: Option<String>,
    properties: HashMap<String, serde_json::Value>,
    limit: u32,
    offset: Option<u32>,
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
struct RestEmbeddingVersion {
    vector: Vec<f32>,
    version: String,
}

/// REST input for creating/updating nodes
#[derive(Debug, Deserialize)]
pub struct RestNodeInput {
    id: String,
    labels: Vec<String>,
    properties: HashMap<String, serde_json::Value>,
    embedding: Option<RestEmbeddingVersionInput>,
}

/// REST input for creating/updating edges
#[derive(Debug, Deserialize)]
pub struct RestEdgeInput {
    id: String,
    from_node_id: String,
    to_node_id: String,
    edge_type: String,
    properties: HashMap<String, serde_json::Value>,
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
struct RestTraversalResponse {
    nodes: Vec<RestNode>,
    edges: Vec<RestEdge>,
    paths: Vec<RestGraphPath>,
    stats: Option<RestTraversalStats>,
}

/// REST-compatible GraphPath wrapper
#[derive(Debug, Serialize, Clone)]
struct RestGraphPath {
    node_ids: Vec<String>,
}

/// REST-compatible TraversalStats wrapper
#[derive(Debug, Serialize, Clone)]
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
    node: RestNodeInput,
}

/// Create edge request
#[derive(Debug, Deserialize)]
pub struct CreateEdgeRequest {
    edge: RestEdgeInput,
}

/// Batch create nodes request
#[derive(Debug, Deserialize)]
pub struct BatchCreateNodesRequest {
    nodes: Vec<RestNodeInput>,
    if_exists: Option<String>, // "update" | "skip" | "error"
}

/// Batch create edges request
#[derive(Debug, Deserialize)]
pub struct BatchCreateEdgesRequest {
    edges: Vec<RestEdgeInput>,
    if_exists: Option<String>, // "update" | "skip" | "error"
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
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
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
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null)
        }
        Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => {
            serde_json::Value::Bool(*b)
        }
        Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(arr)) => {
            serde_json::Value::Array(
                arr.values
                    .iter()
                    .map(|v| convert_property_value_to_json(v))
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
            } else if let Some(f) = n.as_f64() {
                Some(Value::DoubleValue(f))
            } else {
                None
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
    chrono::DateTime::from_timestamp_millis(*ts_ms)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
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
        // Legacy compatibility endpoints (using default graph)
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
    start_node_id: String,
    target_node_id: String,
    max_depth: Option<u32>,
    edge_types: Option<Vec<String>>,
    algorithm: Option<String>, // "DIJKSTRA" or "ASTAR"
    k: Option<u32>,
    enable_prefetch: Option<bool>,
    prefetch_budget: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct UniqueConstraintRequest {
    label: String,
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
    if req.enable_prefetch.is_none() {
        if let Some(v) = headers
            .get("x-graph-prefetch-enabled")
            .and_then(|v| v.to_str().ok())
        {
            req.enable_prefetch = Some(v.eq_ignore_ascii_case("true") || v == "1");
        }
    }
    if req.prefetch_budget.is_none() {
        if let Some(v) = headers
            .get("x-graph-prefetch-budget")
            .and_then(|v| v.to_str().ok())
        {
            if let Ok(n) = v.parse::<usize>() {
                req.prefetch_budget = Some(n);
            }
        }
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
    if q.offset.is_none() {
        if let Some(token) = &q.continuation_token {
            if let Some(rest) = token.strip_prefix("offset:") {
                if let Ok(n) = rest.parse::<u32>() {
                    q.offset = Some(n);
                }
            }
        }
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
    if q.offset.is_none() {
        if let Some(token) = &q.continuation_token {
            if let Some(rest) = token.strip_prefix("offset:") {
                if let Ok(n) = rest.parse::<u32>() {
                    q.offset = Some(n);
                }
            }
        }
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

/// Legacy create node handler (uses default graph)
pub async fn create_node_legacy(
    State(app_state): State<AppState>,
    Json(request): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    create_node(
        State(app_state),
        Path(DEFAULT_GRAPH_ID.to_string()),
        Json(request),
    )
    .await
}

/// Legacy get node handler (uses default graph)
pub async fn get_node_legacy(
    State(app_state): State<AppState>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    get_node(
        State(app_state),
        Path((DEFAULT_GRAPH_ID.to_string(), node_id)),
    )
    .await
}

/// Legacy create edge handler (uses default graph)
pub async fn create_edge_legacy(
    State(app_state): State<AppState>,
    Json(request): Json<CreateEdgeRequest>,
) -> impl IntoResponse {
    create_edge(
        State(app_state),
        Path(DEFAULT_GRAPH_ID.to_string()),
        Json(request),
    )
    .await
}

/// Legacy get graph stats handler (uses default graph)
pub async fn get_graph_stats_legacy(State(app_state): State<AppState>) -> impl IntoResponse {
    get_graph_stats(State(app_state), Path(DEFAULT_GRAPH_ID.to_string())).await
}

// ============================================================================
// Graph Collection Management Handlers
// ============================================================================

/// Input for creating a graph collection
#[derive(Debug, Deserialize)]
pub struct CreateGraphCollectionRequest {
    graph_id: String,
    name: Option<String>,
    description: Option<String>,
    // Schema and configuration can be added later
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
    language: String,
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
