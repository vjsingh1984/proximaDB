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

//! # gRPC Service Implementation for Graph Operations
//!
//! This module provides high-performance gRPC endpoints for ProximaDB's native graph database,
//! leveraging proto-first architecture for zero-copy operations and optimal throughput.
//!
//! ## Performance Characteristics
//!
//! - **Protocol Buffer Native**: Direct proto handling without JSON conversion overhead
//! - **Streaming Support**: Efficient streaming for large graph operations
//! - **Zero-Copy Design**: Arc-based sharing throughout the stack
//! - **Async/Await**: Full async support for non-blocking operations
//!
//! ## Service Architecture
//!
//! ```text
//! gRPC Client Request (Proto)
//!         ↓
//! GraphServiceImpl (This Module)  
//!         ↓
//! UnifiedHandlers.graph_operations_service
//!         ↓
//! GraphEngine (ORION/PULSAR/QUASAR)
//!         ↓
//! Response (Proto)
//! ```
//!
//! ## Endpoint Overview
//!
//! The service implements the GraphService trait from graph.proto:
//! - CreateNode / UpdateNode / DeleteNode / GetNode
//! - CreateEdge / UpdateEdge / DeleteEdge / GetEdge  
//! - QueryNodes / QueryEdges
//! - TraverseGraph
//! - GetNeighbors
//! - GetGraphStats
//! - BatchCreateNodes / BatchCreateEdges

use std::sync::Arc;
use std::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::graph::canonical::{
    ErrorCode as CanonicalErrorCode, TraversalStats as CanonicalTraversalStats,
};
use crate::proto::proximadb_v1::{
    BatchEdgeRequest,
    BatchErrorCode,
    BatchItemError,
    BatchNodeRequest,
    BatchResponse,
    Component,
    ConnectedComponentsResponse,
    CreateEdgeRequest,
    CreateNodeRequest,
    CycleCheckResponse,
    DeleteEdgeRequest,
    DeleteNodeRequest,
    Edge,
    EdgeQuery,
    GetEdgeRequest,
    GetNeighborsRequest,
    // Common types
    GetNodeRequest,
    GetStatsRequest,
    GraphQueryRequest,
    GraphQueryResponse,
    GraphStats,
    HybridSearchRequest,
    HybridSearchResponse,
    // Request/Response types
    Node,
    NodeQuery,
    ShortestPathRequest,
    ShortestPathResponse,
    TraversalChunk,
    TraversalRequest,
    TraversalResponse,
    UniqueConstraintRequest,
    UniqueConstraintResponse,
    UpdateEdgeRequest,
    UpdateNodeRequest,
    // Graph service definition
    graph_service_server::GraphService,
};

use crate::query::QueryFacadeAdapter;

// ================================================================================
// HELPER FUNCTIONS FOR CANONICAL TYPE ALIGNMENT
// ================================================================================

/// Map an error message to a canonical ErrorCode based on the error content.
/// This provides consistent error categorization across gRPC responses.
fn map_error_to_canonical_code(error_message: &str) -> CanonicalErrorCode {
    let error_lower = error_message.to_lowercase();

    if error_lower.contains("not found") || error_lower.contains("does not exist") {
        CanonicalErrorCode::NotFound
    } else if error_lower.contains("already exists") || error_lower.contains("duplicate") {
        CanonicalErrorCode::AlreadyExists
    } else if error_lower.contains("invalid")
        || error_lower.contains("required")
        || error_lower.contains("missing")
    {
        CanonicalErrorCode::InvalidArgument
    } else if error_lower.contains("constraint") || error_lower.contains("unique") {
        CanonicalErrorCode::ConstraintViolation
    } else if error_lower.contains("timeout") || error_lower.contains("timed out") {
        CanonicalErrorCode::Timeout
    } else if error_lower.contains("permission")
        || error_lower.contains("denied")
        || error_lower.contains("unauthorized")
    {
        CanonicalErrorCode::PermissionDenied
    } else {
        CanonicalErrorCode::InternalError
    }
}

/// Convert a canonical ErrorCode to a proto BatchErrorCode.
/// This ensures alignment between REST and gRPC error representations.
fn canonical_code_to_batch_error_code(code: CanonicalErrorCode) -> BatchErrorCode {
    match code {
        CanonicalErrorCode::NotFound => BatchErrorCode::NotFound,
        CanonicalErrorCode::AlreadyExists => BatchErrorCode::AlreadyExists,
        CanonicalErrorCode::InvalidArgument => BatchErrorCode::InvalidArgument,
        CanonicalErrorCode::ConstraintViolation => BatchErrorCode::ConstraintViolation,
        CanonicalErrorCode::InternalError => BatchErrorCode::InternalError,
        CanonicalErrorCode::Timeout => BatchErrorCode::Timeout,
        CanonicalErrorCode::PermissionDenied => BatchErrorCode::PermissionDenied,
    }
}

/// Create a BatchItemError from an entity ID and error message.
/// Automatically maps the error message to an appropriate error code.
#[allow(dead_code)]
fn create_batch_item_error(id: impl Into<String>, message: impl Into<String>) -> BatchItemError {
    let message_str = message.into();
    let canonical_code = map_error_to_canonical_code(&message_str);
    BatchItemError {
        id: id.into(),
        message: message_str,
        code: canonical_code_to_batch_error_code(canonical_code).into(),
    }
}

/// Convert a canonical ErrorCode to a gRPC Status code.
fn canonical_code_to_grpc_status(code: CanonicalErrorCode, message: impl Into<String>) -> Status {
    let msg = message.into();
    match code {
        CanonicalErrorCode::NotFound => Status::not_found(msg),
        CanonicalErrorCode::AlreadyExists => Status::already_exists(msg),
        CanonicalErrorCode::InvalidArgument => Status::invalid_argument(msg),
        CanonicalErrorCode::ConstraintViolation => Status::failed_precondition(msg),
        CanonicalErrorCode::InternalError => Status::internal(msg),
        CanonicalErrorCode::Timeout => Status::deadline_exceeded(msg),
        CanonicalErrorCode::PermissionDenied => Status::permission_denied(msg),
    }
}

/// Create a gRPC Status from an error, using canonical error code mapping.
fn create_grpc_error(operation: &str, error: impl std::fmt::Display) -> Status {
    let error_message = error.to_string();
    let canonical_code = map_error_to_canonical_code(&error_message);
    let full_message = format!("Failed to {}: {}", operation, error_message);
    canonical_code_to_grpc_status(canonical_code, full_message)
}

/// Convert proto TraversalStats to canonical format for consistent field naming.
/// The canonical format uses:
/// - `max_depth_reached` (same as proto)
/// - `execution_time_ms` (converted from microseconds)
#[allow(dead_code)]
fn convert_traversal_stats_to_canonical(
    stats: &crate::proto::proximadb_v1::TraversalStats,
) -> CanonicalTraversalStats {
    CanonicalTraversalStats::from_proto(stats)
}

/// Create a populated BatchResponse with all canonical fields properly initialized.
/// This ensures consistent structure for batch operations.
///
/// For responses with per-item errors, use `create_batch_response_for_nodes_with_errors` instead.
fn create_batch_response_for_nodes(
    nodes: Vec<crate::proto::proximadb_v1::Node>,
    success: bool,
    error_message: Option<String>,
) -> BatchResponse {
    let created_count = if success {
        Some(nodes.len() as u32)
    } else {
        Some(0)
    };
    let failed_count = if success {
        Some(0)
    } else {
        Some(nodes.len() as u32)
    };

    BatchResponse {
        success,
        nodes,
        edges: vec![],
        error_message,
        next_token: None,
        created_count,
        updated_count: Some(0),
        failed_count,
        failed_ids: vec![],
        error_messages: vec![],
        errors: vec![],
    }
}

/// Create a populated BatchResponse for nodes with structured per-item error details.
/// This aligns with REST API's BatchResults<T> format which includes a Vec<BatchError>.
///
/// Both legacy fields (failed_ids, error_messages) and the new structured errors field
/// are populated for backward compatibility with older clients.
#[allow(dead_code)]
fn create_batch_response_for_nodes_with_errors(
    nodes: Vec<crate::proto::proximadb_v1::Node>,
    errors: Vec<BatchItemError>,
) -> BatchResponse {
    let created_count = nodes.len() as u32;
    let failed_count = errors.len() as u32;
    let success = errors.is_empty();

    // Populate legacy fields for backward compatibility
    let failed_ids: Vec<String> = errors.iter().map(|e| e.id.clone()).collect();
    let error_messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();

    BatchResponse {
        success,
        nodes,
        edges: vec![],
        error_message: None,
        next_token: None,
        created_count: Some(created_count),
        updated_count: Some(0),
        failed_count: Some(failed_count),
        failed_ids,     // Legacy (deprecated but maintained for compatibility)
        error_messages, // Legacy (deprecated but maintained for compatibility)
        errors,         // New structured error field (aligned with REST API)
    }
}

/// Create a populated BatchResponse for edges with all canonical fields properly initialized.
///
/// For responses with per-item errors, use `create_batch_response_for_edges_with_errors` instead.
fn create_batch_response_for_edges(
    edges: Vec<crate::proto::proximadb_v1::Edge>,
    success: bool,
    error_message: Option<String>,
) -> BatchResponse {
    let created_count = if success {
        Some(edges.len() as u32)
    } else {
        Some(0)
    };
    let failed_count = if success {
        Some(0)
    } else {
        Some(edges.len() as u32)
    };

    BatchResponse {
        success,
        nodes: vec![],
        edges,
        error_message,
        next_token: None,
        created_count,
        updated_count: Some(0),
        failed_count,
        failed_ids: vec![],
        error_messages: vec![],
        errors: vec![],
    }
}

/// Create a populated BatchResponse for edges with structured per-item error details.
/// This aligns with REST API's BatchResults<T> format which includes a Vec<BatchError>.
///
/// Both legacy fields (failed_ids, error_messages) and the new structured errors field
/// are populated for backward compatibility with older clients.
#[allow(dead_code)]
fn create_batch_response_for_edges_with_errors(
    edges: Vec<crate::proto::proximadb_v1::Edge>,
    errors: Vec<BatchItemError>,
) -> BatchResponse {
    let created_count = edges.len() as u32;
    let failed_count = errors.len() as u32;
    let success = errors.is_empty();

    // Populate legacy fields for backward compatibility
    let failed_ids: Vec<String> = errors.iter().map(|e| e.id.clone()).collect();
    let error_messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();

    BatchResponse {
        success,
        nodes: vec![],
        edges,
        error_message: None,
        next_token: None,
        created_count: Some(created_count),
        updated_count: Some(0),
        failed_count: Some(failed_count),
        failed_ids,     // Legacy (deprecated but maintained for compatibility)
        error_messages, // Legacy (deprecated but maintained for compatibility)
        errors,         // New structured error field (aligned with REST API)
    }
}

/// Create a query response with pagination support.
fn create_query_response_for_nodes(
    nodes: Vec<crate::proto::proximadb_v1::Node>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> BatchResponse {
    let has_more = limit.is_some_and(|l| nodes.len() as u32 == l);
    let next_token = if has_more {
        let next_offset = offset.unwrap_or(0).saturating_add(limit.unwrap_or(0));
        Some(format!("offset:{}", next_offset))
    } else {
        None
    };

    BatchResponse {
        success: true,
        nodes,
        edges: vec![],
        error_message: None,
        next_token,
        created_count: None,
        updated_count: None,
        failed_count: None,
        failed_ids: vec![],
        error_messages: vec![],
        errors: vec![],
    }
}

/// Create a query response for edges with pagination support.
fn create_query_response_for_edges(
    edges: Vec<crate::proto::proximadb_v1::Edge>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> BatchResponse {
    let has_more = limit.is_some_and(|l| edges.len() as u32 == l);
    let next_token = if has_more {
        let next_offset = offset.unwrap_or(0).saturating_add(limit.unwrap_or(0));
        Some(format!("offset:{}", next_offset))
    } else {
        None
    };

    BatchResponse {
        success: true,
        nodes: vec![],
        edges,
        error_message: None,
        next_token,
        created_count: None,
        updated_count: None,
        failed_count: None,
        failed_ids: vec![],
        error_messages: vec![],
        errors: vec![],
    }
}

// ================================================================================
// GRPC SERVICE IMPLEMENTATION
// ================================================================================

/// gRPC implementation of GraphService
pub struct GraphServiceImpl {
    unified_handlers: Arc<UnifiedHandlers>,
    /// Query facade adapter for unified query execution (optional for backward compatibility)
    query_adapter: Option<Arc<QueryFacadeAdapter>>,
}

impl GraphServiceImpl {
    /// Create new GraphServiceImpl
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            unified_handlers,
            query_adapter: None,
        }
    }

    /// Create new GraphServiceImpl with query facade adapter
    #[allow(dead_code)]
    pub fn with_adapter(
        unified_handlers: Arc<UnifiedHandlers>,
        query_adapter: Arc<QueryFacadeAdapter>,
    ) -> Self {
        Self {
            unified_handlers,
            query_adapter: Some(query_adapter),
        }
    }
}

#[tonic::async_trait]
impl GraphService for GraphServiceImpl {
    /// Create a new node
    async fn create_node(
        &self,
        request: Request<CreateNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC CreateNode request for graph: {} node: {:?}",
            req.graph_id, req.node
        );

        let node = req
            .node
            .ok_or_else(|| Status::invalid_argument("Node is required"))?;

        match self
            .unified_handlers
            .graph_operations_service
            .create_node(&req.graph_id, node)
            .await
        {
            Ok(created_node) => {
                info!("Successfully created node via gRPC: {}", created_node.id);
                Ok(Response::new((*created_node).clone()))
            }
            Err(err) => {
                error!("Failed to create node via gRPC: {}", err);
                Err(create_grpc_error("create node", err))
            }
        }
    }

    /// Get a node by ID
    async fn get_node(&self, request: Request<GetNodeRequest>) -> Result<Response<Node>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC GetNode request for graph: {} ID: {}",
            req.graph_id, req.node_id
        );

        match self
            .unified_handlers
            .graph_operations_service
            .get_node(&req.graph_id, &req.node_id)
            .await
        {
            Ok(Some(node)) => {
                info!("Successfully retrieved node via gRPC: {}", req.node_id);
                Ok(Response::new((*node).clone()))
            }
            Ok(None) => {
                warn!("Node not found via gRPC: {}", req.node_id);
                Err(canonical_code_to_grpc_status(
                    CanonicalErrorCode::NotFound,
                    format!("Node '{}' not found", req.node_id),
                ))
            }
            Err(err) => {
                error!("Failed to get node via gRPC {}: {}", req.node_id, err);
                Err(create_grpc_error("get node", err))
            }
        }
    }

    /// Update a node
    async fn update_node(
        &self,
        request: Request<UpdateNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC UpdateNode request for graph: {} node: {:?}",
            req.graph_id, req.node
        );

        let node = req
            .node
            .ok_or_else(|| Status::invalid_argument("Node is required"))?;

        match self
            .unified_handlers
            .graph_operations_service
            .update_node(&req.graph_id, node)
            .await
        {
            Ok(updated_node) => {
                info!("Successfully updated node via gRPC: {}", updated_node.id);
                Ok(Response::new((*updated_node).clone()))
            }
            Err(err) => {
                error!("Failed to update node via gRPC: {}", err);
                Err(create_grpc_error("update node", err))
            }
        }
    }

    /// Delete a node
    async fn delete_node(
        &self,
        request: Request<DeleteNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC DeleteNode request for graph: {} ID: {}",
            req.graph_id, req.node_id
        );

        match self
            .unified_handlers
            .graph_operations_service
            .delete_node(&req.graph_id, &req.node_id)
            .await
        {
            Ok(Some(deleted_node)) => {
                info!("Successfully deleted node via gRPC: {}", req.node_id);
                Ok(Response::new((*deleted_node).clone()))
            }
            Ok(None) => {
                warn!("Node not found for deletion via gRPC: {}", req.node_id);
                Err(canonical_code_to_grpc_status(
                    CanonicalErrorCode::NotFound,
                    format!("Node '{}' not found", req.node_id),
                ))
            }
            Err(err) => {
                error!("Failed to delete node via gRPC {}: {}", req.node_id, err);
                Err(create_grpc_error("delete node", err))
            }
        }
    }

    /// Create a new edge
    async fn create_edge(
        &self,
        request: Request<CreateEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC CreateEdge request for graph: {} edge: {:?}",
            req.graph_id, req.edge
        );

        let edge = req
            .edge
            .ok_or_else(|| Status::invalid_argument("Edge is required"))?;

        match self
            .unified_handlers
            .graph_operations_service
            .create_edge(&req.graph_id, edge)
            .await
        {
            Ok(created_edge) => {
                info!("Successfully created edge via gRPC: {}", created_edge.id);
                Ok(Response::new((*created_edge).clone()))
            }
            Err(err) => {
                error!("Failed to create edge via gRPC: {}", err);
                Err(create_grpc_error("create edge", err))
            }
        }
    }

    /// Get an edge by ID
    async fn get_edge(&self, request: Request<GetEdgeRequest>) -> Result<Response<Edge>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC GetEdge request for graph: {} ID: {}",
            req.graph_id, req.edge_id
        );

        match self
            .unified_handlers
            .graph_operations_service
            .get_edge(&req.graph_id, &req.edge_id)
            .await
        {
            Ok(Some(edge)) => {
                info!("Successfully retrieved edge via gRPC: {}", req.edge_id);
                Ok(Response::new((*edge).clone()))
            }
            Ok(None) => {
                warn!("Edge not found via gRPC: {}", req.edge_id);
                Err(canonical_code_to_grpc_status(
                    CanonicalErrorCode::NotFound,
                    format!("Edge '{}' not found", req.edge_id),
                ))
            }
            Err(err) => {
                error!("Failed to get edge via gRPC {}: {}", req.edge_id, err);
                Err(create_grpc_error("get edge", err))
            }
        }
    }

    /// Update an edge
    async fn update_edge(
        &self,
        request: Request<UpdateEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC UpdateEdge request for graph: {} edge: {:?}",
            req.graph_id, req.edge
        );

        let edge = req
            .edge
            .ok_or_else(|| Status::invalid_argument("Edge is required"))?;

        match self
            .unified_handlers
            .graph_operations_service
            .update_edge(&req.graph_id, edge)
            .await
        {
            Ok(updated_edge) => {
                info!("Successfully updated edge via gRPC: {}", updated_edge.id);
                Ok(Response::new((*updated_edge).clone()))
            }
            Err(err) => {
                error!("Failed to update edge via gRPC: {}", err);
                Err(create_grpc_error("update edge", err))
            }
        }
    }

    /// Delete an edge
    async fn delete_edge(
        &self,
        request: Request<DeleteEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC DeleteEdge request for graph: {} ID: {}",
            req.graph_id, req.edge_id
        );

        match self
            .unified_handlers
            .graph_operations_service
            .delete_edge(&req.graph_id, &req.edge_id)
            .await
        {
            Ok(Some(deleted_edge)) => {
                info!("Successfully deleted edge via gRPC: {}", req.edge_id);
                Ok(Response::new((*deleted_edge).clone()))
            }
            Ok(None) => {
                warn!("Edge not found for deletion via gRPC: {}", req.edge_id);
                Err(canonical_code_to_grpc_status(
                    CanonicalErrorCode::NotFound,
                    format!("Edge '{}' not found", req.edge_id),
                ))
            }
            Err(err) => {
                error!("Failed to delete edge via gRPC {}: {}", req.edge_id, err);
                Err(create_grpc_error("delete edge", err))
            }
        }
    }

    /// Query nodes by labels and properties
    async fn query_nodes(
        &self,
        request: Request<NodeQuery>,
    ) -> Result<Response<BatchResponse>, Status> {
        let mut query = request.into_inner();
        debug!(
            "gRPC QueryNodes request for graph: {} with labels: {:?}",
            query.graph_id, query.labels
        );
        // Continuation token parsing: format "offset:<n>"
        if query.offset.is_none()
            && let Some(token) = &query.continuation_token
                && let Some(rest) = token.strip_prefix("offset:")
                    && let Ok(n) = rest.parse::<u32>() {
                        query.offset = Some(n);
                    }

        match self
            .unified_handlers
            .graph_operations_service
            .query_nodes(&query.graph_id, query.clone())
            .await
        {
            Ok(nodes) => {
                info!("Successfully queried {} nodes via gRPC", nodes.len());
                let nodes_vec: Vec<Node> = nodes.into_iter().map(|n| (*n).clone()).collect();
                let response =
                    create_query_response_for_nodes(nodes_vec, query.limit, query.offset);
                Ok(Response::new(response))
            }
            Err(err) => {
                error!("Failed to query nodes via gRPC: {}", err);
                Err(create_grpc_error("query nodes", err))
            }
        }
    }

    /// Query edges by types and properties
    async fn query_edges(
        &self,
        request: Request<EdgeQuery>,
    ) -> Result<Response<BatchResponse>, Status> {
        let mut query = request.into_inner();
        debug!("gRPC QueryEdges request for graph: {}", query.graph_id);
        if query.offset.is_none()
            && let Some(token) = &query.continuation_token
                && let Some(rest) = token.strip_prefix("offset:")
                    && let Ok(n) = rest.parse::<u32>() {
                        query.offset = Some(n);
                    }

        match self
            .unified_handlers
            .graph_operations_service
            .query_edges(&query.graph_id, query.clone())
            .await
        {
            Ok(edges) => {
                info!("Successfully queried {} edges via gRPC", edges.len());
                let edges_vec: Vec<Edge> = edges.into_iter().map(|e| (*e).clone()).collect();
                let response =
                    create_query_response_for_edges(edges_vec, query.limit, query.offset);
                Ok(Response::new(response))
            }
            Err(err) => {
                error!("Failed to query edges via gRPC: {}", err);
                Err(create_grpc_error("query edges", err))
            }
        }
    }

    /// Get neighbors of a node
    async fn get_neighbors(
        &self,
        request: Request<GetNeighborsRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC GetNeighbors request for graph: {} node: {}",
            req.graph_id, req.node_id
        );

        match self
            .unified_handlers
            .graph_operations_service
            .get_neighbors(&req.graph_id, &req.node_id)
            .await
        {
            Ok(neighbors) => {
                info!(
                    "Successfully retrieved {} neighbors via gRPC for node: {}",
                    neighbors.len(),
                    req.node_id
                );
                let nodes_vec: Vec<Node> = neighbors.into_iter().map(|n| (*n).clone()).collect();
                let response = create_batch_response_for_nodes(nodes_vec, true, None);
                Ok(Response::new(response))
            }
            Err(err) => {
                error!(
                    "Failed to get neighbors via gRPC for node {}: {}",
                    req.node_id, err
                );
                Err(create_grpc_error("get neighbors", err))
            }
        }
    }

    /// Perform graph traversal
    async fn traverse_graph(
        &self,
        request: Request<TraversalRequest>,
    ) -> Result<Response<TraversalResponse>, Status> {
        let req = request.into_inner();
        let start_time = Instant::now();
        debug!(
            "gRPC TraverseGraph request for graph: {} from node: {}",
            req.graph_id, req.start_node_id
        );

        match self
            .unified_handlers
            .graph_operations_service
            .traverse(&req.graph_id, req.clone())
            .await
        {
            Ok(mut response) => {
                info!("Successfully completed graph traversal via gRPC");
                // Ensure execution_time_microseconds is populated if stats exist
                if let Some(ref mut stats) = response.stats
                    && stats.execution_time_microseconds == 0 {
                        stats.execution_time_microseconds = start_time.elapsed().as_micros() as u64;
                    }
                Ok(Response::new(response))
            }
            Err(err) => {
                error!("Failed to traverse graph via gRPC: {}", err);
                Err(create_grpc_error("traverse graph", err))
            }
        }
    }

    /// Stream traversal in chunks
    type StreamTraverseStream = ReceiverStream<Result<TraversalChunk, Status>>;
    async fn stream_traverse(
        &self,
        request: Request<TraversalRequest>,
    ) -> Result<Response<Self::StreamTraverseStream>, Status> {
        let req = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let handlers = self.unified_handlers.clone();
        let graph_id = req.graph_id.clone();
        tokio::spawn(async move {
            match handlers
                .graph_operations_service
                .traverse(&graph_id, req)
                .await
            {
                Ok(resp) => {
                    let chunk_size = 1000usize;
                    let mut idx = 0;
                    let nodes = resp.nodes;
                    let total = nodes.len();
                    while idx < total {
                        let end = (idx + chunk_size).min(total);
                        let mut chunk = TraversalChunk {
                            nodes: nodes[idx..end].to_vec(),
                            edges: vec![],
                            paths: vec![],
                            stats: None,
                            done: false,
                        };
                        if end == total {
                            chunk.edges = resp.edges.clone();
                            chunk.paths = resp.paths.clone();
                            chunk.stats = resp.stats;
                            chunk.done = true;
                        }
                        if tx.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                        idx = end;
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!(
                            "StreamTraverse failed: {}",
                            e
                        ))))
                        .await;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    /// Compute shortest path between nodes
    async fn shortest_path(
        &self,
        request: Request<ShortestPathRequest>,
    ) -> Result<Response<ShortestPathResponse>, Status> {
        let md = request.metadata().clone();
        let req = request.into_inner();
        debug!(
            "gRPC ShortestPath request for graph: {} from {} to {}",
            req.graph_id, req.start_node_id, req.target_node_id
        );

        let edge_types = if req.edge_types.is_empty() {
            None
        } else {
            Some(req.edge_types.clone())
        };
        let algorithm = req.algorithm();
        let k = req.k;

        // Per-call overrides via gRPC metadata
        let override_enable_prefetch = md
            .get("x-graph-prefetch-enabled")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1");
        let override_prefetch_budget = md
            .get("x-graph-prefetch-budget")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok());

        match self
            .unified_handlers
            .graph_operations_service
            .shortest_path(
                &req.graph_id,
                &req.start_node_id,
                &req.target_node_id,
                req.max_depth,
                edge_types,
                Some(algorithm),
                k,
                override_enable_prefetch,
                override_prefetch_budget,
            )
            .await
        {
            Ok(Some((path, total_weight))) => {
                // Response uses node_ids which represents the path
                Ok(Response::new(ShortestPathResponse {
                    node_ids: path,
                    total_weight: Some(total_weight),
                }))
            }
            Ok(None) => {
                // Return success with empty path - aligns with REST API semantics
                // where no path found is a successful query with empty result
                debug!(
                    "No path found between '{}' and '{}' - returning empty response",
                    req.start_node_id, req.target_node_id
                );
                Ok(Response::new(ShortestPathResponse {
                    node_ids: vec![],
                    total_weight: None,
                }))
            }
            Err(e) => Err(create_grpc_error("compute shortest path", e)),
        }
    }

    /// Get graph statistics
    async fn get_graph_stats(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<GraphStats>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetGraphStats request for graph: {}", req.graph_id);

        match self
            .unified_handlers
            .graph_operations_service
            .get_stats(&req.graph_id)
            .await
        {
            Ok(stats) => {
                info!("Successfully retrieved graph statistics via gRPC");
                Ok(Response::new(stats))
            }
            Err(err) => {
                error!("Failed to get graph statistics via gRPC: {}", err);
                Err(create_grpc_error("get graph statistics", err))
            }
        }
    }

    /// Batch create nodes
    async fn batch_create_nodes(
        &self,
        request: Request<BatchNodeRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC BatchCreateNodes request for graph: {} with {} nodes",
            req.graph_id,
            req.nodes.len()
        );

        match self
            .unified_handlers
            .graph_operations_service
            .batch_create_nodes(&req.graph_id, req.nodes)
            .await
        {
            Ok(nodes) => {
                info!("Successfully batch created {} nodes via gRPC", nodes.len());
                let nodes_vec: Vec<Node> = nodes.into_iter().map(|n| (*n).clone()).collect();
                let response = create_batch_response_for_nodes(nodes_vec, true, None);
                Ok(Response::new(response))
            }
            Err(err) => {
                error!("Failed to batch create nodes via gRPC: {}", err);
                Err(create_grpc_error("batch create nodes", err))
            }
        }
    }

    /// Batch create edges
    async fn batch_create_edges(
        &self,
        request: Request<BatchEdgeRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC BatchCreateEdges request for graph: {} with {} edges",
            req.graph_id,
            req.edges.len()
        );

        match self
            .unified_handlers
            .graph_operations_service
            .batch_create_edges(&req.graph_id, req.edges)
            .await
        {
            Ok(edges) => {
                info!("Successfully batch created {} edges via gRPC", edges.len());
                let edges_vec: Vec<Edge> = edges.into_iter().map(|e| (*e).clone()).collect();
                let response = create_batch_response_for_edges(edges_vec, true, None);
                Ok(Response::new(response))
            }
            Err(err) => {
                error!("Failed to batch create edges via gRPC: {}", err);
                Err(create_grpc_error("batch create edges", err))
            }
        }
    }

    /// Get connected components (weak)
    async fn get_connected_components(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<ConnectedComponentsResponse>, Status> {
        let req = request.into_inner();
        match self
            .unified_handlers
            .graph_operations_service
            .connected_components(&req.graph_id)
            .await
        {
            Ok(comps) => {
                let components = comps
                    .into_iter()
                    .map(|nodes| Component { node_ids: nodes })
                    .collect();
                Ok(Response::new(ConnectedComponentsResponse { components }))
            }
            Err(e) => Err(create_grpc_error("get connected components", e)),
        }
    }

    /// Check for directed cycles
    async fn has_cycle(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<CycleCheckResponse>, Status> {
        let req = request.into_inner();
        match self
            .unified_handlers
            .graph_operations_service
            .has_cycle(&req.graph_id)
            .await
        {
            Ok(has) => Ok(Response::new(CycleCheckResponse { has_cycle: has })),
            Err(e) => Err(create_grpc_error("check for cycles", e)),
        }
    }

    /// Add unique constraint (label, property)
    async fn add_unique_constraint(
        &self,
        request: Request<UniqueConstraintRequest>,
    ) -> Result<Response<UniqueConstraintResponse>, Status> {
        let req = request.into_inner();
        match self
            .unified_handlers
            .graph_operations_service
            .add_unique_constraint(&req.graph_id, &req.label, &req.property)
            .await
        {
            Ok(()) => Ok(Response::new(UniqueConstraintResponse {
                success: true,
                error_message: None,
            })),
            Err(e) => Ok(Response::new(UniqueConstraintResponse {
                success: false,
                error_message: Some(e.to_string()),
            })),
        }
    }

    /// Remove unique constraint (label, property)
    async fn remove_unique_constraint(
        &self,
        request: Request<UniqueConstraintRequest>,
    ) -> Result<Response<UniqueConstraintResponse>, Status> {
        let req = request.into_inner();
        match self
            .unified_handlers
            .graph_operations_service
            .remove_unique_constraint(&req.graph_id, &req.label, &req.property)
            .await
        {
            Ok(_) => Ok(Response::new(UniqueConstraintResponse {
                success: true,
                error_message: None,
            })),
            Err(e) => Ok(Response::new(UniqueConstraintResponse {
                success: false,
                error_message: Some(e.to_string()),
            })),
        }
    }

    /// Execute hybrid vector-graph query
    async fn execute_hybrid_query(
        &self,
        request: Request<HybridSearchRequest>,
    ) -> Result<Response<HybridSearchResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC ExecuteHybridQuery request with strategy: {:?}",
            req.combination_strategy
        );

        match self.unified_handlers.execute_hybrid_query(req).await {
            Ok(response) => {
                info!("Successfully executed hybrid query via gRPC");
                Ok(Response::new(response))
            }
            Err(err) => {
                error!("Failed to execute hybrid query via gRPC: {}", err);
                Err(create_grpc_error("execute hybrid query", err))
            }
        }
    }

    /// Execute declarative graph query (Cypher/Gremlin)
    async fn execute_query(
        &self,
        request: Request<GraphQueryRequest>,
    ) -> Result<Response<GraphQueryResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC ExecuteQuery request for graph: {} language: {:?}",
            req.graph_id,
            req.language()
        );

        // Route through unified facade when adapter is available
        if let Some(ref adapter) = self.query_adapter {
            debug!("Using unified facade routing for graph query");
            let graph_name = if req.graph_id.is_empty() {
                None
            } else {
                Some(req.graph_id.as_str())
            };

            return match adapter.graph_query(&req.query, graph_name).await {
                Ok(result) => {
                    use crate::proto::proximadb_v1::{
                        PropertyValue, QueryValue, ResultRow, property_value, query_value,
                    };

                    // Helper to create a string QueryValue
                    let make_string_value = |s: String| -> QueryValue {
                        QueryValue {
                            value: Some(query_value::Value::Property(PropertyValue {
                                value: Some(property_value::Value::StringValue(s)),
                            })),
                        }
                    };

                    // Convert QueryResult to GraphQueryResponse rows
                    let rows: Vec<ResultRow> = match result.data {
                        crate::query::QueryResultData::Graph(graph_result) => {
                            // Convert graph nodes/edges to rows
                            graph_result
                                .nodes
                                .into_iter()
                                .map(|node| {
                                    let mut columns = std::collections::HashMap::new();
                                    columns.insert(
                                        "data".to_string(),
                                        make_string_value(node.to_string()),
                                    );
                                    ResultRow { columns }
                                })
                                .collect()
                        }
                        crate::query::QueryResultData::Rows(json_rows) => {
                            // Convert JSON rows to ResultRow format
                            json_rows
                                .into_iter()
                                .map(|row| {
                                    let mut columns = std::collections::HashMap::new();
                                    columns.insert(
                                        "data".to_string(),
                                        make_string_value(row.to_string()),
                                    );
                                    ResultRow { columns }
                                })
                                .collect()
                        }
                        _ => vec![],
                    };

                    Ok(Response::new(GraphQueryResponse {
                        rows,
                        stats: None,
                        query_plan: None,
                        error_message: None,
                    }))
                }
                Err(e) => {
                    error!("Graph query (facade) failed: {}", e);
                    Err(Status::internal(format!("Graph query failed: {}", e)))
                }
            };
        }

        // Legacy path: Return unimplemented error
        Err(Status::unimplemented(
            "Declarative query execution not yet implemented. \
             Use QueryNodes/QueryEdges for property-based queries, \
             or TraverseGraph for graph traversal.",
        ))
    }
}
