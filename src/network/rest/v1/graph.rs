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
//! POST   /v1/graph/nodes           - Create node
//! GET    /v1/graph/nodes/{id}      - Get node by ID
//! PUT    /v1/graph/nodes/{id}      - Update node  
//! DELETE /v1/graph/nodes/{id}      - Delete node
//! POST   /v1/graph/edges           - Create edge
//! GET    /v1/graph/edges/{id}      - Get edge by ID
//! PUT    /v1/graph/edges/{id}      - Update edge
//! DELETE /v1/graph/edges/{id}      - Delete edge
//! GET    /v1/graph/nodes/{id}/neighbors - Get node neighbors
//! POST   /v1/graph/traverse        - Graph traversal
//! GET    /v1/graph/stats           - Graph statistics
//! POST   /v1/graph/nodes/batch     - Batch create nodes
//! POST   /v1/graph/edges/batch     - Batch create edges
//! POST   /v1/graph/query/nodes     - Query nodes
//! POST   /v1/graph/query/edges     - Query edges
//! ```
//!
//! ## Request/Response Format
//!
//! All endpoints use JSON serialization with proto message compatibility.
//! Proto timestamps are converted to ISO 8601 strings for JSON compatibility.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{delete, get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::graph::{GraphService, Node, Edge, NodeId, EdgeId, TraversalRequest, NodeQuery, EdgeQuery};
use crate::network::rest::v1::handlers::AppState;

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

/// Query parameters for pagination
#[derive(Debug, Deserialize)]
struct PaginationQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

/// Create node request
#[derive(Debug, Deserialize)]
struct CreateNodeRequest {
    node: Node,
}

/// Create edge request
#[derive(Debug, Deserialize)]
struct CreateEdgeRequest {
    edge: Edge,
}

/// Batch create nodes request
#[derive(Debug, Deserialize)]
struct BatchCreateNodesRequest {
    nodes: Vec<Node>,
}

/// Batch create edges request
#[derive(Debug, Deserialize)]
struct BatchCreateEdgesRequest {
    edges: Vec<Edge>,
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
        .route("/query/nodes", post(query_nodes))
        .route("/query/edges", post(query_edges))
        // Batch operations
        .route("/nodes/batch", post(batch_create_nodes))
        .route("/edges/batch", post(batch_create_edges))
        // Statistics
        .route("/stats", get(get_graph_stats))
}

/// Create a new node
pub async fn create_node(
    State(app_state): State<AppState>,
    Json(request): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    debug!("Creating node: {:?}", request.node.id);
    
    match app_state.unified_handlers.graph_service.create_node(request.node) {
        Ok(node) => {
            info!("Successfully created node: {}", node.id);
            Json(GraphSuccessResponse {
                success: true,
                data: node,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to create node: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "creation_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_CREATE_ERROR".to_string(),
                })
            ).into_response()
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
            Json(GraphSuccessResponse {
                success: true,
                data: node,
            }).into_response()
        },
        Ok(None) => {
            warn!("Node not found: {}", node_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphErrorResponse {
                    error: "not_found".to_string(),
                    message: format!("Node '{}' not found", node_id),
                    code: "GRAPH_NODE_NOT_FOUND".to_string(),
                })
            ).into_response()
        },
        Err(err) => {
            error!("Failed to get node {}: {}", node_id, err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "retrieval_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_GET_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Update a node
pub async fn update_node(
    State(app_state): State<AppState>,
    Path(node_id): Path<String>,
    Json(mut node): Json<Node>,
) -> impl IntoResponse {
    debug!("Updating node: {}", node_id);
    
    // Ensure the node ID matches the path parameter
    node.id = node_id.clone();
    
    match app_state.unified_handlers.graph_service.update_node(node) {
        Ok(updated_node) => {
            info!("Successfully updated node: {}", node_id);
            Json(GraphSuccessResponse {
                success: true,
                data: updated_node,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to update node {}: {}", node_id, err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "update_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_UPDATE_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Delete a node
pub async fn delete_node(
    State(app_state): State<AppState>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    debug!("Deleting node: {}", node_id);
    
    match app_state.unified_handlers.graph_service.delete_node(&node_id) {
        Ok(Some(deleted_node)) => {
            info!("Successfully deleted node: {}", node_id);
            Json(GraphSuccessResponse {
                success: true,
                data: deleted_node,
            }).into_response()
        },
        Ok(None) => {
            warn!("Node not found for deletion: {}", node_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphErrorResponse {
                    error: "not_found".to_string(),
                    message: format!("Node '{}' not found", node_id),
                    code: "GRAPH_NODE_NOT_FOUND".to_string(),
                })
            ).into_response()
        },
        Err(err) => {
            error!("Failed to delete node {}: {}", node_id, err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "deletion_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_DELETE_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Get neighbors of a node
pub async fn get_node_neighbors(
    State(app_state): State<AppState>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    debug!("Getting neighbors for node: {}", node_id);
    
    match app_state.unified_handlers.graph_service.get_neighbors(&node_id) {
        Ok(neighbors) => {
            info!("Successfully retrieved {} neighbors for node: {}", neighbors.len(), node_id);
            Json(GraphSuccessResponse {
                success: true,
                data: neighbors,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to get neighbors for node {}: {}", node_id, err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "neighbors_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NEIGHBORS_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Create a new edge
pub async fn create_edge(
    State(app_state): State<AppState>,
    Json(request): Json<CreateEdgeRequest>,
) -> impl IntoResponse {
    debug!("Creating edge: {:?}", request.edge.id);
    
    match app_state.unified_handlers.graph_service.create_edge(request.edge) {
        Ok(edge) => {
            info!("Successfully created edge: {}", edge.id);
            Json(GraphSuccessResponse {
                success: true,
                data: edge,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to create edge: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "creation_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_CREATE_ERROR".to_string(),
                })
            ).into_response()
        }
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
            Json(GraphSuccessResponse {
                success: true,
                data: edge,
            }).into_response()
        },
        Ok(None) => {
            warn!("Edge not found: {}", edge_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphErrorResponse {
                    error: "not_found".to_string(),
                    message: format!("Edge '{}' not found", edge_id),
                    code: "GRAPH_EDGE_NOT_FOUND".to_string(),
                })
            ).into_response()
        },
        Err(err) => {
            error!("Failed to get edge {}: {}", edge_id, err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "retrieval_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_GET_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Update an edge
pub async fn update_edge(
    State(app_state): State<AppState>,
    Path(edge_id): Path<String>,
    Json(mut edge): Json<Edge>,
) -> impl IntoResponse {
    debug!("Updating edge: {}", edge_id);
    
    // Ensure the edge ID matches the path parameter
    edge.id = edge_id.clone();
    
    match app_state.unified_handlers.graph_service.update_edge(edge) {
        Ok(updated_edge) => {
            info!("Successfully updated edge: {}", edge_id);
            Json(GraphSuccessResponse {
                success: true,
                data: updated_edge,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to update edge {}: {}", edge_id, err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "update_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_UPDATE_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Delete an edge
pub async fn delete_edge(
    State(app_state): State<AppState>,
    Path(edge_id): Path<String>,
) -> impl IntoResponse {
    debug!("Deleting edge: {}", edge_id);
    
    match app_state.unified_handlers.graph_service.delete_edge(&edge_id) {
        Ok(Some(deleted_edge)) => {
            info!("Successfully deleted edge: {}", edge_id);
            Json(GraphSuccessResponse {
                success: true,
                data: deleted_edge,
            }).into_response()
        },
        Ok(None) => {
            warn!("Edge not found for deletion: {}", edge_id);
            (
                StatusCode::NOT_FOUND,
                Json(GraphErrorResponse {
                    error: "not_found".to_string(),
                    message: format!("Edge '{}' not found", edge_id),
                    code: "GRAPH_EDGE_NOT_FOUND".to_string(),
                })
            ).into_response()
        },
        Err(err) => {
            error!("Failed to delete edge {}: {}", edge_id, err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "deletion_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_DELETE_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Perform graph traversal
pub async fn traverse_graph(
    State(app_state): State<AppState>,
    Json(request): Json<TraversalRequest>,
) -> impl IntoResponse {
    debug!("Starting graph traversal from node: {}", request.start_node_id);
    
    match app_state.unified_handlers.graph_service.traverse(request).await {
        Ok(response) => {
            info!("Successfully completed graph traversal");
            Json(GraphSuccessResponse {
                success: true,
                data: response,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to traverse graph: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "traversal_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_TRAVERSAL_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Query nodes by labels and properties
pub async fn query_nodes(
    State(app_state): State<AppState>,
    Json(query): Json<NodeQuery>,
) -> impl IntoResponse {
    debug!("Querying nodes with labels: {:?}", query.labels);
    
    match app_state.unified_handlers.graph_service.query_nodes(query) {
        Ok(nodes) => {
            info!("Successfully queried {} nodes", nodes.len());
            Json(GraphSuccessResponse {
                success: true,
                data: nodes,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to query nodes: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "query_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_NODE_QUERY_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Query edges by types and properties
pub async fn query_edges(
    State(app_state): State<AppState>,
    Json(query): Json<EdgeQuery>,
) -> impl IntoResponse {
    debug!("Querying edges");
    
    match app_state.unified_handlers.graph_service.query_edges(query) {
        Ok(edges) => {
            info!("Successfully queried {} edges", edges.len());
            Json(GraphSuccessResponse {
                success: true,
                data: edges,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to query edges: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "query_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_EDGE_QUERY_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Batch create nodes
pub async fn batch_create_nodes(
    State(app_state): State<AppState>,
    Json(request): Json<BatchCreateNodesRequest>,
) -> impl IntoResponse {
    debug!("Batch creating {} nodes", request.nodes.len());
    
    match app_state.unified_handlers.graph_service.batch_create_nodes(request.nodes) {
        Ok(nodes) => {
            info!("Successfully batch created {} nodes", nodes.len());
            Json(GraphBatchResponse {
                success: true,
                created_count: nodes.len(),
                failed_count: 0,
                results: nodes,
                errors: vec![],
            }).into_response()
        },
        Err(err) => {
            error!("Failed to batch create nodes: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "batch_creation_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_BATCH_NODES_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Batch create edges
pub async fn batch_create_edges(
    State(app_state): State<AppState>,
    Json(request): Json<BatchCreateEdgesRequest>,
) -> impl IntoResponse {
    debug!("Batch creating {} edges", request.edges.len());
    
    match app_state.unified_handlers.graph_service.batch_create_edges(request.edges) {
        Ok(edges) => {
            info!("Successfully batch created {} edges", edges.len());
            Json(GraphBatchResponse {
                success: true,
                created_count: edges.len(),
                failed_count: 0,
                results: edges,
                errors: vec![],
            }).into_response()
        },
        Err(err) => {
            error!("Failed to batch create edges: {}", err);
            (
                StatusCode::BAD_REQUEST,
                Json(GraphErrorResponse {
                    error: "batch_creation_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_BATCH_EDGES_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}

/// Get graph statistics
pub async fn get_graph_stats(
    State(app_state): State<AppState>,
) -> impl IntoResponse {
    debug!("Getting graph statistics");
    
    match app_state.unified_handlers.graph_service.get_stats() {
        Ok(stats) => {
            info!("Successfully retrieved graph statistics");
            Json(GraphSuccessResponse {
                success: true,
                data: stats,
            }).into_response()
        },
        Err(err) => {
            error!("Failed to get graph statistics: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphErrorResponse {
                    error: "stats_failed".to_string(),
                    message: err.to_string(),
                    code: "GRAPH_STATS_ERROR".to_string(),
                })
            ).into_response()
        }
    }
}