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
    http::{StatusCode, HeaderMap},
    response::{IntoResponse, Json},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

// For base64 encoding of bytes (using standard library instead)
// use base64;

// Use proto types directly with custom serde implementations
use crate::proto::proximadb_v1::{
    Edge, EdgeQuery, Node, NodeQuery, TraversalRequest,
};
use crate::network::rest::v1::handlers::AppState;
use crate::proto::proximadb_v1::{PropertyValue, EmbeddingVersion};

/// REST-compatible TraversalRequest wrapper for JSON deserialization
#[derive(Debug, serde::Deserialize)]
struct RestTraversalRequest {
    start_node_id: String,
    max_depth: u32,
    edge_types: Vec<String>,
    node_labels: Vec<String>,
    return_path: bool,
    algorithm: String,
}

/// REST-compatible NodeQuery wrapper for JSON deserialization
#[derive(Debug, Clone, serde::Deserialize)]
struct RestNodeQuery {
    labels: Vec<String>,
    properties: HashMap<String, serde_json::Value>,
    limit: u32,
    offset: Option<u32>,
    continuation_token: Option<String>,
}

/// REST-compatible EdgeQuery wrapper for JSON deserialization
#[derive(Debug, Clone, serde::Deserialize)]
struct RestEdgeQuery {
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
        // Convert algorithm string to enum value (simplified)
        let algorithm = match rest.algorithm.as_str() {
            "dfs" => 1, // TraversalAlgorithm::Dfs
            "bfs" => 2, // TraversalAlgorithm::Bfs
            _ => 0, // TraversalAlgorithm::Unspecified
        };

        crate::proto::proximadb_v1::TraversalRequest {
            start_node_id: rest.start_node_id,
            max_depth: rest.max_depth,
            edge_types: rest.edge_types,
            node_labels: rest.node_labels,
            filters: vec![], // REST doesn't have filters yet
            algorithm,
            limit: None,
            timeout_ms: None,
            max_frontier: None,
        }
    }
}

impl From<RestNodeQuery> for crate::proto::proximadb_v1::NodeQuery {
    fn from(rest: RestNodeQuery) -> Self {
        crate::proto::proximadb_v1::NodeQuery {
            labels: rest.labels,
            filters: vec![], // Convert properties to filters if needed
            limit: Some(rest.limit),
            offset: rest.offset,
            continuation_token: rest.continuation_token,
        }
    }
}

impl From<RestEdgeQuery> for crate::proto::proximadb_v1::EdgeQuery {
    fn from(rest: RestEdgeQuery) -> Self {
        crate::proto::proximadb_v1::EdgeQuery {
            from_node_id: rest.from_node_id,
            to_node_id: rest.to_node_id,
            edge_types: vec![rest.edge_type], // Convert single edge_type to vector
            filters: vec![], // Convert properties to filters if needed
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
struct RestNodeInput {
    id: String,
    labels: Vec<String>,
    properties: HashMap<String, serde_json::Value>,
    embedding: Option<RestEmbeddingVersionInput>,
}

/// REST input for creating/updating edges
#[derive(Debug, Deserialize)]
struct RestEdgeInput {
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

/// Graph API error response
#[derive(Debug, Serialize)]
struct GraphErrorResponse {
    error: String,
    message: String,
    code: String,
}

/// Success response wrapper
#[derive(Debug, Serialize)]
struct GraphSuccessResponse<T> {
    success: bool,
    data: T,
}

/// Batch operation response
#[derive(Debug, Serialize)]
struct GraphBatchResponse<T> {
    success: bool,
    created_count: usize,
    failed_count: usize,
    results: Vec<T>,
    errors: Vec<String>,
}

/// Query response with optional pagination token
#[derive(Debug, Serialize)]
struct GraphQueryResponse<T> {
    success: bool,
    data: T,
    next_token: Option<String>,
}

/// Query parameters for pagination
#[derive(Debug, Deserialize)]
struct PaginationQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

/// Create node request
#[derive(Debug, Deserialize)]
struct CreateNodeRequest {
    node: RestNodeInput,
}

/// Create edge request
#[derive(Debug, Deserialize)]
struct CreateEdgeRequest {
    edge: RestEdgeInput,
}

/// Batch create nodes request
#[derive(Debug, Deserialize)]
struct BatchCreateNodesRequest {
    nodes: Vec<RestNodeInput>,
    if_exists: Option<String>, // "update" | "skip" | "error"
}

/// Batch create edges request
#[derive(Debug, Deserialize)]
struct BatchCreateEdgesRequest {
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
        RestGraphPath {
            node_ids,
        }
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
            edge_type_stats: stats.edge_type_stats.iter().map(RestEdgeTypeStats::from).collect(),
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

fn convert_properties_to_json(props: &HashMap<String, PropertyValue>) -> HashMap<String, serde_json::Value> {
    props.iter().map(|(k, v)| {
        let json_val = match &v.value {
            Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => serde_json::Value::String(s.clone()),
            Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => serde_json::Value::Number(serde_json::Number::from(*i)),
            Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(f)) => serde_json::Number::from_f64(*f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
            Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => serde_json::Value::Bool(*b),
            Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(arr)) => {
                serde_json::Value::Array(arr.values.iter().map(|v| convert_property_value_to_json(v)).collect())
            },
            Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(b)) => {
                serde_json::Value::String(format!("{:?}", b)) // Convert to debug string for now
            },
            Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_obj)) => {
                serde_json::Value::Object(serde_json::Map::new()) // TODO: Proper object conversion
            },
            Some(crate::proto::proximadb_v1::property_value::Value::VectorValue(vec)) => {
                serde_json::Value::Array(vec.values.iter().map(|f| serde_json::Value::Number(serde_json::Number::from_f64(*f as f64).unwrap_or(serde_json::Number::from(0)))).collect())
            },
            None => serde_json::Value::Null,
        };
        (k.clone(), json_val)
    }).collect()
}

fn convert_json_to_properties(props: HashMap<String, serde_json::Value>) -> HashMap<String, PropertyValue> {
    props.into_iter().map(|(k, v)| {
        let prop_val = convert_json_to_property_value(v);
        (k, prop_val)
    }).collect()
}

fn convert_property_value_to_json(prop: &PropertyValue) -> serde_json::Value {
    match &prop.value {
        Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => serde_json::Value::Number(serde_json::Number::from(*i)),
        Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(f)) => serde_json::Number::from_f64(*f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
        Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(arr)) => {
            serde_json::Value::Array(arr.values.iter().map(|v| convert_property_value_to_json(v)).collect())
        },
        Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(b)) => {
            serde_json::Value::String(format!("{:?}", b)) // Convert to debug string for now
        },
        Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_obj)) => {
            serde_json::Value::Object(serde_json::Map::new()) // TODO: Proper object conversion
        },
        Some(crate::proto::proximadb_v1::property_value::Value::VectorValue(vec)) => {
            serde_json::Value::Array(vec.values.iter().map(|f| serde_json::Value::Number(serde_json::Number::from_f64(*f as f64).unwrap_or(serde_json::Number::from(0)))).collect())
        },
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
        },
        serde_json::Value::Bool(b) => Some(Value::BoolValue(b)),
        serde_json::Value::Array(arr) => {
            let values: Vec<PropertyValue> = arr.into_iter().map(convert_json_to_property_value).collect();
            Some(Value::ArrayValue(crate::proto::proximadb_v1::PropertyArray { values }))
        },
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

/// Create the graph REST router
pub fn create_graph_router() -> Router<AppState> {
    Router::new()
        // Node operations
        .route("/nodes", post(create_node))
        .route("/nodes/:id", get(get_node))
        .route("/nodes/:id", put(update_node))
        .route("/nodes/:id", delete(delete_node))
        .route("/nodes/:id/neighbors", get(get_node_neighbors))
        // Edge operations
        .route("/edges", post(create_edge))
        .route("/edges/:id", get(get_edge))
        .route("/edges/:id", put(update_edge))
        .route("/edges/:id", delete(delete_edge))
        // Traversal and querying
        .route("/traverse", post(traverse_graph))
        .route("/shortest_path", post(shortest_path))
        .route("/query/nodes", post(query_nodes))
        .route("/query/edges", post(query_edges))
        // Batch operations
        .route("/nodes/batch", post(batch_create_nodes))
        .route("/edges/batch", post(batch_create_edges))
        // Statistics
        .route("/stats", get(get_graph_stats))
        // Constraints DDL
        .route("/constraints/unique", post(add_unique_constraint))
        .route("/constraints/unique", delete(remove_unique_constraint))
        // Graph analysis
        .route("/components", get(get_connected_components))
        .route("/cycles", get(check_cycles))
}

/// Create a new node
pub async fn create_node(
    State(app_state): State<AppState>,
    Json(request): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    debug!("Creating node: {:?}", request.node.id);

    // Convert REST input to proto Node
    let proto_node: Node = request.node.into();

    match app_state
        .unified_handlers
        .graph_service
        .create_node(proto_node)
    {
        Ok(node) => {
            info!("Successfully created node: {}", node.id);
            let rest_node = RestNode::from(&*node);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_node,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to create node: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "creation_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_CREATE_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Get a node by ID
pub async fn get_node(
    State(app_state): State<AppState>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    debug!("Getting node: {}", node_id);

    match app_state.unified_handlers.graph_service.get_node(&node_id) {
        Ok(Some(node)) => {
            info!("Successfully retrieved node: {}", node_id);
            let rest_node = RestNode::from(&*node);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_node,
            })
            .into_response()
        }
        Ok(None) => {
            warn!("Node not found: {}", node_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphErrorResponse {
                    error: "not_found".to_string(),
                    message: format!("Node '{}' not found", node_id),
                    code: "GRAPH_NODE_NOT_FOUND".to_string(),
                }),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to get node {}: {}", node_id, err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "retrieval_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_GET_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Update a node
pub async fn update_node(
    State(app_state): State<AppState>,
    Path(node_id): Path<String>,
    Json(mut node_input): Json<RestNodeInput>,
) -> impl IntoResponse {
    debug!("Updating node: {}", node_id);

    // Ensure the node ID matches the path parameter
    node_input.id = node_id.clone();
    
    // Convert REST input to proto Node
    let proto_node: Node = node_input.into();

    match app_state.unified_handlers.graph_service.update_node(proto_node) {
        Ok(updated_node) => {
            info!("Successfully updated node: {}", node_id);
            let rest_node = RestNode::from(&*updated_node);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_node,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to update node {}: {}", node_id, err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "update_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_UPDATE_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Delete a node
pub async fn delete_node(
    State(app_state): State<AppState>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    debug!("Deleting node: {}", node_id);

    match app_state
        .unified_handlers
        .graph_service
        .delete_node(&node_id)
    {
        Ok(Some(deleted_node)) => {
            info!("Successfully deleted node: {}", node_id);
            let rest_node = RestNode::from(&*deleted_node);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_node,
            })
            .into_response()
        }
        Ok(None) => {
            warn!("Node not found for deletion: {}", node_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphErrorResponse {
                    error: "not_found".to_string(),
                    message: format!("Node '{}' not found", node_id),
                    code: "GRAPH_NODE_NOT_FOUND".to_string(),
                }),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to delete node {}: {}", node_id, err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "deletion_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_DELETE_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Get neighbors of a node
pub async fn get_node_neighbors(
    State(app_state): State<AppState>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    debug!("Getting neighbors for node: {}", node_id);

    match app_state
        .unified_handlers
        .graph_service
        .get_neighbors(&node_id)
    {
        Ok(neighbors) => {
            info!(
                "Successfully retrieved {} neighbors for node: {}",
                neighbors.len(),
                node_id
            );
            let rest_nodes: Vec<RestNode> = neighbors
                .into_iter()
                .map(|n| RestNode::from(&*n))
                .collect();
            Json(GraphSuccessResponse {
                success: true,
                data: rest_nodes,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to get neighbors for node {}: {}", node_id, err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "neighbors_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NEIGHBORS_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Create a new edge
pub async fn create_edge(
    State(app_state): State<AppState>,
    Json(request): Json<CreateEdgeRequest>,
) -> impl IntoResponse {
    debug!("Creating edge: {:?}", request.edge.id);

    // Convert REST input to proto Edge
    let proto_edge: Edge = request.edge.into();

    match app_state
        .unified_handlers
        .graph_service
        .create_edge(proto_edge)
    {
        Ok(edge) => {
            info!("Successfully created edge: {}", edge.id);
            let rest_edge = RestEdge::from(&*edge);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_edge,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to create edge: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "creation_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_CREATE_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ShortestPathRequest {
    start_node_id: String,
    target_node_id: String,
    max_depth: Option<u32>,
    edge_types: Option<Vec<String>>,
    algorithm: Option<String>, // "DIJKSTRA" or "ASTAR"
    k: Option<u32>,
    enable_prefetch: Option<bool>,
    prefetch_budget: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ShortestPathResponse {
    success: bool,
    path: Option<Vec<String>>, // node IDs
    total_weight: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct UniqueConstraintRequest {
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
    headers: HeaderMap,
    Json(mut req): Json<ShortestPathRequest>,
) -> impl IntoResponse {
    // Header-based overrides if JSON fields not provided
    if req.enable_prefetch.is_none() {
        if let Some(v) = headers.get("x-graph-prefetch-enabled").and_then(|v| v.to_str().ok()) {
            req.enable_prefetch = Some(v.eq_ignore_ascii_case("true") || v == "1");
        }
    }
    if req.prefetch_budget.is_none() {
        if let Some(v) = headers.get("x-graph-prefetch-budget").and_then(|v| v.to_str().ok()) {
            if let Ok(n) = v.parse::<usize>() {
                req.prefetch_budget = Some(n);
            }
        }
    }
    match app_state
        .unified_handlers
        .graph_service
        .shortest_path(
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
        Ok(Some((path, total_weight))) => Json(ShortestPathResponse {
            success: true,
            path: Some(path),
            total_weight: Some(total_weight),
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(GraphErrorResponse {
                error: "no_path".to_string(),
                message: "No path found between nodes".to_string(),
                code: "GRAPH_NO_PATH".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(GraphErrorResponse {
                error: "shortest_path_failed".into(),
                message: e.to_string(),
                code: "GRAPH_SHORTEST_PATH_ERROR".into(),
            }),
        )
            .into_response(),
    }
}

fn parse_sp_algorithm(
    s: Option<&str>,
) -> Option<crate::proto::proximadb_v1::ShortestPathAlgorithm> {
    match s.unwrap_or("DIJKSTRA").to_ascii_uppercase().as_str() {
        "ASTAR" => {
            Some(crate::proto::proximadb_v1::ShortestPathAlgorithm::Astar)
        }
        "DIJKSTRA" => {
            Some(crate::proto::proximadb_v1::ShortestPathAlgorithm::Dijkstra)
        }
        _ => None,
    }
}

/// Add unique constraint (label, property)
pub async fn add_unique_constraint(
    State(app_state): State<AppState>,
    Json(req): Json<UniqueConstraintRequest>,
) -> impl IntoResponse {
    match app_state
        .unified_handlers
        .graph_service
        .add_unique_constraint(&req.label, &req.property)
    {
        Ok(()) => Json(DdlResponse { success: true }).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(GraphErrorResponse {
                error: "add_unique_failed".into(),
                message: e.to_string(),
                code: "GRAPH_ADD_UNIQUE_ERROR".into(),
            }),
        )
            .into_response(),
    }
}

/// Remove unique constraint (label, property)
pub async fn remove_unique_constraint(
    State(app_state): State<AppState>,
    Json(req): Json<UniqueConstraintRequest>,
) -> impl IntoResponse {
    app_state
        .unified_handlers
        .graph_service
        .remove_unique_constraint(&req.label, &req.property);
    Json(DdlResponse { success: true }).into_response()
}

/// Get connected components (weakly connected)
pub async fn get_connected_components(State(app_state): State<AppState>) -> impl IntoResponse {
    match app_state
        .unified_handlers
        .graph_service
        .connected_components()
        .await
    {
        Ok(components) => Json(ComponentsResponse {
            success: true,
            components,
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GraphErrorResponse {
                error: "components_failed".into(),
                message: e.to_string(),
                code: "GRAPH_COMPONENTS_ERROR".into(),
            }),
        )
            .into_response(),
    }
}

/// Detect directed cycles
pub async fn check_cycles(State(app_state): State<AppState>) -> impl IntoResponse {
    match app_state.unified_handlers.graph_service.has_cycle().await {
        Ok(has) => Json(CycleResponse {
            success: true,
            has_cycle: has,
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GraphErrorResponse {
                error: "cycles_failed".into(),
                message: e.to_string(),
                code: "GRAPH_CYCLE_ERROR".into(),
            }),
        )
            .into_response(),
    }
}

/// Get an edge by ID
pub async fn get_edge(
    State(app_state): State<AppState>,
    Path(edge_id): Path<String>,
) -> impl IntoResponse {
    debug!("Getting edge: {}", edge_id);

    match app_state.unified_handlers.graph_service.get_edge(&edge_id) {
        Ok(Some(edge)) => {
            info!("Successfully retrieved edge: {}", edge_id);
            let rest_edge = RestEdge::from(&*edge);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_edge,
            })
            .into_response()
        }
        Ok(None) => {
            warn!("Edge not found: {}", edge_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphErrorResponse {
                    error: "not_found".to_string(),
                    message: format!("Edge '{}' not found", edge_id),
                    code: "GRAPH_EDGE_NOT_FOUND".to_string(),
                }),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to get edge {}: {}", edge_id, err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "retrieval_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_GET_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Update an edge
pub async fn update_edge(
    State(app_state): State<AppState>,
    Path(edge_id): Path<String>,
    Json(mut edge_input): Json<RestEdgeInput>,
) -> impl IntoResponse {
    debug!("Updating edge: {}", edge_id);

    // Ensure the edge ID matches the path parameter
    edge_input.id = edge_id.clone();
    
    // Convert REST input to proto Edge
    let proto_edge: Edge = edge_input.into();

    match app_state.unified_handlers.graph_service.update_edge(proto_edge) {
        Ok(updated_edge) => {
            info!("Successfully updated edge: {}", edge_id);
            let rest_edge = RestEdge::from(&*updated_edge);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_edge,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to update edge {}: {}", edge_id, err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "update_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_UPDATE_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Delete an edge
pub async fn delete_edge(
    State(app_state): State<AppState>,
    Path(edge_id): Path<String>,
) -> impl IntoResponse {
    debug!("Deleting edge: {}", edge_id);

    match app_state
        .unified_handlers
        .graph_service
        .delete_edge(&edge_id)
    {
        Ok(Some(deleted_edge)) => {
            info!("Successfully deleted edge: {}", edge_id);
            let rest_edge = RestEdge::from(&*deleted_edge);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_edge,
            })
            .into_response()
        }
        Ok(None) => {
            warn!("Edge not found for deletion: {}", edge_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphErrorResponse {
                    error: "not_found".to_string(),
                    message: format!("Edge '{}' not found", edge_id),
                    code: "GRAPH_EDGE_NOT_FOUND".to_string(),
                }),
            )
                .into_response()
        }
        Err(err) => {
            error!("Failed to delete edge {}: {}", edge_id, err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "deletion_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_DELETE_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Perform graph traversal
pub async fn traverse_graph(
    State(app_state): State<AppState>,
    Json(request): Json<RestTraversalRequest>,
) -> impl IntoResponse {
    debug!(
        "Starting graph traversal from node: {}",
        request.start_node_id
    );

    // TODO: Read per-call overrides from headers (temporarily disabled)
    let override_enable_prefetch = None;
    let override_prefetch_budget = None;

    match app_state
        .unified_handlers
        .graph_service
        .traverse_with_overrides(request.into(), override_enable_prefetch, override_prefetch_budget)
        .await
    {
        Ok(response) => {
            info!("Successfully completed graph traversal");
            let rest_response = RestTraversalResponse::from(&response);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_response,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to traverse graph: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "traversal_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_TRAVERSAL_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Query nodes by labels and properties
pub async fn query_nodes(
    State(app_state): State<AppState>,
    Json(query): Json<RestNodeQuery>,
) -> impl IntoResponse {
    debug!("Querying nodes with labels: {:?}", query.labels);
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
        .graph_service
        .query_nodes(q.clone().into())
    {
        Ok(nodes) => {
            info!("Successfully queried {} nodes", nodes.len());
            let mut next_token = None;
            let lim = q.limit;
            if (nodes.len() as u32) == lim {
                let next_off = q.offset.unwrap_or(0).saturating_add(lim);
                next_token = Some(format!("offset:{}", next_off));
            }
            let rest_nodes: Vec<RestNode> = nodes.into_iter().map(|n| RestNode::from(&*n)).collect();
            Json(GraphQueryResponse {
                success: true,
                data: rest_nodes,
                next_token,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to query nodes: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "query_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_QUERY_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Query edges by types and properties
pub async fn query_edges(
    State(app_state): State<AppState>,
    Json(query): Json<RestEdgeQuery>,
) -> impl IntoResponse {
    debug!("Querying edges");
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
        .graph_service
        .query_edges(q.clone().into())
    {
        Ok(edges) => {
            info!("Successfully queried {} edges", edges.len());
            let mut next_token = None;
            let lim = q.limit;
            if (edges.len() as u32) == lim {
                let next_off = q.offset.unwrap_or(0).saturating_add(lim);
                next_token = Some(format!("offset:{}", next_off));
            }
            let rest_edges: Vec<RestEdge> = edges.into_iter().map(|e| RestEdge::from(&*e)).collect();
            Json(GraphQueryResponse {
                success: true,
                data: rest_edges,
                next_token,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to query edges: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "query_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_QUERY_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Batch create nodes
pub async fn batch_create_nodes(
    State(app_state): State<AppState>,
    Json(request): Json<BatchCreateNodesRequest>,
) -> impl IntoResponse {
    debug!("Batch creating {} nodes", request.nodes.len());
    let strategy = request.if_exists.unwrap_or_else(|| "error".into());
    
    // Convert REST inputs to proto Nodes
    let proto_nodes: Vec<Node> = request.nodes.into_iter().map(|n| n.into()).collect();
    
    match app_state
        .unified_handlers
        .graph_service
        .batch_create_nodes_with_strategy(proto_nodes, strategy.as_str())
    {
        Ok(nodes) => {
            info!("Successfully batch created {} nodes", nodes.len());
            let rest_nodes: Vec<RestNode> = nodes.into_iter().map(|n| RestNode::from(&*n)).collect();
            Json(GraphBatchResponse {
                success: true,
                created_count: rest_nodes.len(),
                failed_count: 0,
                results: rest_nodes,
                errors: vec![],
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to batch create nodes: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "batch_creation_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_BATCH_NODES_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Batch create edges
pub async fn batch_create_edges(
    State(app_state): State<AppState>,
    Json(request): Json<BatchCreateEdgesRequest>,
) -> impl IntoResponse {
    debug!("Batch creating {} edges", request.edges.len());
    let _strategy = request.if_exists.clone().unwrap_or_else(|| "error".into());
    
    // Convert REST inputs to proto Edges
    let proto_edges: Vec<Edge> = request.edges.into_iter().map(|e| e.into()).collect();
    
    match app_state
        .unified_handlers
        .graph_service
        .batch_create_edges(proto_edges)
    {
        Ok(edges) => {
            info!("Successfully batch created {} edges", edges.len());
            let rest_edges: Vec<RestEdge> = edges.into_iter().map(|e| RestEdge::from(&*e)).collect();
            Json(GraphBatchResponse {
                success: true,
                created_count: rest_edges.len(),
                failed_count: 0,
                results: rest_edges,
                errors: vec![],
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to batch create edges: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "batch_creation_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_BATCH_EDGES_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}

/// Get graph statistics
#[derive(Debug, Serialize)]
struct ComponentsResponse {
    success: bool,
    components: Vec<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct CycleResponse {
    success: bool,
    has_cycle: bool,
}

pub async fn get_graph_stats(State(app_state): State<AppState>) -> impl IntoResponse {
    debug!("Getting graph statistics");

    match app_state.unified_handlers.graph_service.get_stats() {
        Ok(stats) => {
            info!("Successfully retrieved graph statistics");
            let rest_stats = RestGraphStats::from(&stats);
            Json(GraphSuccessResponse {
                success: true,
                data: rest_stats,
            })
            .into_response()
        }
        Err(err) => {
            error!("Failed to get graph statistics: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "stats_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_STATS_ERROR".to_string(),
                }),
            )
                .into_response()
        }
    }
}
