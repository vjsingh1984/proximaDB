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
//! UnifiedHandlers.graph_service
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
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1::{
    // Graph service definition
    graph_service_server::GraphService,
    // Request/Response types
    Node, Edge, NodeQuery, EdgeQuery, TraversalRequest, TraversalResponse,
    BatchNodeRequest, BatchEdgeRequest, BatchResponse, GraphStats,
    // Common types
    GetNodeRequest, GetEdgeRequest, CreateNodeRequest, CreateEdgeRequest,
    UpdateNodeRequest, UpdateEdgeRequest, DeleteNodeRequest, DeleteEdgeRequest,
    GetNeighborsRequest, GetStatsRequest,
};

/// gRPC implementation of GraphService
pub struct GraphServiceImpl {
    unified_handlers: Arc<UnifiedHandlers>,
}

impl GraphServiceImpl {
    /// Create new GraphServiceImpl
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            unified_handlers,
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
        debug!("gRPC CreateNode request for node: {:?}", req.node);

        let node = req.node.ok_or_else(|| {
            Status::invalid_argument("Node is required")
        })?;

        match self.unified_handlers.graph_service.create_node(node) {
            Ok(created_node) => {
                info!("Successfully created node via gRPC: {}", created_node.id);
                Ok(Response::new((*created_node).clone()))
            },
            Err(err) => {
                error!("Failed to create node via gRPC: {}", err);
                Err(Status::internal(format!("Failed to create node: {}", err)))
            }
        }
    }

    /// Get a node by ID
    async fn get_node(
        &self,
        request: Request<GetNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetNode request for ID: {}", req.node_id);

        match self.unified_handlers.graph_service.get_node(&req.node_id) {
            Ok(Some(node)) => {
                info!("Successfully retrieved node via gRPC: {}", req.node_id);
                Ok(Response::new((*node).clone()))
            },
            Ok(None) => {
                warn!("Node not found via gRPC: {}", req.node_id);
                Err(Status::not_found(format!("Node '{}' not found", req.node_id)))
            },
            Err(err) => {
                error!("Failed to get node via gRPC {}: {}", req.node_id, err);
                Err(Status::internal(format!("Failed to get node: {}", err)))
            }
        }
    }

    /// Update a node
    async fn update_node(
        &self,
        request: Request<UpdateNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let req = request.into_inner();
        debug!("gRPC UpdateNode request for node: {:?}", req.node);

        let node = req.node.ok_or_else(|| {
            Status::invalid_argument("Node is required")
        })?;

        match self.unified_handlers.graph_service.update_node(node) {
            Ok(updated_node) => {
                info!("Successfully updated node via gRPC: {}", updated_node.id);
                Ok(Response::new((*updated_node).clone()))
            },
            Err(err) => {
                error!("Failed to update node via gRPC: {}", err);
                Err(Status::internal(format!("Failed to update node: {}", err)))
            }
        }
    }

    /// Delete a node
    async fn delete_node(
        &self,
        request: Request<DeleteNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let req = request.into_inner();
        debug!("gRPC DeleteNode request for ID: {}", req.node_id);

        match self.unified_handlers.graph_service.delete_node(&req.node_id) {
            Ok(Some(deleted_node)) => {
                info!("Successfully deleted node via gRPC: {}", req.node_id);
                Ok(Response::new((*deleted_node).clone()))
            },
            Ok(None) => {
                warn!("Node not found for deletion via gRPC: {}", req.node_id);
                Err(Status::not_found(format!("Node '{}' not found", req.node_id)))
            },
            Err(err) => {
                error!("Failed to delete node via gRPC {}: {}", req.node_id, err);
                Err(Status::internal(format!("Failed to delete node: {}", err)))
            }
        }
    }

    /// Create a new edge
    async fn create_edge(
        &self,
        request: Request<CreateEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let req = request.into_inner();
        debug!("gRPC CreateEdge request for edge: {:?}", req.edge);

        let edge = req.edge.ok_or_else(|| {
            Status::invalid_argument("Edge is required")
        })?;

        match self.unified_handlers.graph_service.create_edge(edge) {
            Ok(created_edge) => {
                info!("Successfully created edge via gRPC: {}", created_edge.id);
                Ok(Response::new((*created_edge).clone()))
            },
            Err(err) => {
                error!("Failed to create edge via gRPC: {}", err);
                Err(Status::internal(format!("Failed to create edge: {}", err)))
            }
        }
    }

    /// Get an edge by ID
    async fn get_edge(
        &self,
        request: Request<GetEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetEdge request for ID: {}", req.edge_id);

        match self.unified_handlers.graph_service.get_edge(&req.edge_id) {
            Ok(Some(edge)) => {
                info!("Successfully retrieved edge via gRPC: {}", req.edge_id);
                Ok(Response::new((*edge).clone()))
            },
            Ok(None) => {
                warn!("Edge not found via gRPC: {}", req.edge_id);
                Err(Status::not_found(format!("Edge '{}' not found", req.edge_id)))
            },
            Err(err) => {
                error!("Failed to get edge via gRPC {}: {}", req.edge_id, err);
                Err(Status::internal(format!("Failed to get edge: {}", err)))
            }
        }
    }

    /// Update an edge
    async fn update_edge(
        &self,
        request: Request<UpdateEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let req = request.into_inner();
        debug!("gRPC UpdateEdge request for edge: {:?}", req.edge);

        let edge = req.edge.ok_or_else(|| {
            Status::invalid_argument("Edge is required")
        })?;

        match self.unified_handlers.graph_service.update_edge(edge) {
            Ok(updated_edge) => {
                info!("Successfully updated edge via gRPC: {}", updated_edge.id);
                Ok(Response::new((*updated_edge).clone()))
            },
            Err(err) => {
                error!("Failed to update edge via gRPC: {}", err);
                Err(Status::internal(format!("Failed to update edge: {}", err)))
            }
        }
    }

    /// Delete an edge
    async fn delete_edge(
        &self,
        request: Request<DeleteEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let req = request.into_inner();
        debug!("gRPC DeleteEdge request for ID: {}", req.edge_id);

        match self.unified_handlers.graph_service.delete_edge(&req.edge_id) {
            Ok(Some(deleted_edge)) => {
                info!("Successfully deleted edge via gRPC: {}", req.edge_id);
                Ok(Response::new((*deleted_edge).clone()))
            },
            Ok(None) => {
                warn!("Edge not found for deletion via gRPC: {}", req.edge_id);
                Err(Status::not_found(format!("Edge '{}' not found", req.edge_id)))
            },
            Err(err) => {
                error!("Failed to delete edge via gRPC {}: {}", req.edge_id, err);
                Err(Status::internal(format!("Failed to delete edge: {}", err)))
            }
        }
    }

    /// Query nodes by labels and properties
    async fn query_nodes(
        &self,
        request: Request<NodeQuery>,
    ) -> Result<Response<BatchResponse>, Status> {
        let query = request.into_inner();
        debug!("gRPC QueryNodes request with labels: {:?}", query.labels);

        match self.unified_handlers.graph_service.query_nodes(query) {
            Ok(nodes) => {
                info!("Successfully queried {} nodes via gRPC", nodes.len());
                let response = BatchResponse {
                    success: true,
                    nodes: nodes.into_iter().map(|n| (*n).clone()).collect(),
                    edges: vec![],
                    error_message: None,
                };
                Ok(Response::new(response))
            },
            Err(err) => {
                error!("Failed to query nodes via gRPC: {}", err);
                Err(Status::internal(format!("Failed to query nodes: {}", err)))
            }
        }
    }

    /// Query edges by types and properties
    async fn query_edges(
        &self,
        request: Request<EdgeQuery>,
    ) -> Result<Response<BatchResponse>, Status> {
        let query = request.into_inner();
        debug!("gRPC QueryEdges request");

        match self.unified_handlers.graph_service.query_edges(query) {
            Ok(edges) => {
                info!("Successfully queried {} edges via gRPC", edges.len());
                let response = BatchResponse {
                    success: true,
                    nodes: vec![],
                    edges: edges.into_iter().map(|e| (*e).clone()).collect(),
                    error_message: None,
                };
                Ok(Response::new(response))
            },
            Err(err) => {
                error!("Failed to query edges via gRPC: {}", err);
                Err(Status::internal(format!("Failed to query edges: {}", err)))
            }
        }
    }

    /// Get neighbors of a node
    async fn get_neighbors(
        &self,
        request: Request<GetNeighborsRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetNeighbors request for node: {}", req.node_id);

        match self.unified_handlers.graph_service.get_neighbors(&req.node_id) {
            Ok(neighbors) => {
                info!("Successfully retrieved {} neighbors via gRPC for node: {}", 
                    neighbors.len(), req.node_id);
                let response = BatchResponse {
                    success: true,
                    nodes: neighbors.into_iter().map(|n| (*n).clone()).collect(),
                    edges: vec![],
                    error_message: None,
                };
                Ok(Response::new(response))
            },
            Err(err) => {
                error!("Failed to get neighbors via gRPC for node {}: {}", req.node_id, err);
                Err(Status::internal(format!("Failed to get neighbors: {}", err)))
            }
        }
    }

    /// Perform graph traversal
    async fn traverse_graph(
        &self,
        request: Request<TraversalRequest>,
    ) -> Result<Response<TraversalResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC TraverseGraph request from node: {}", req.start_node_id);

        match self.unified_handlers.graph_service.traverse(req).await {
            Ok(response) => {
                info!("Successfully completed graph traversal via gRPC");
                Ok(Response::new(response))
            },
            Err(err) => {
                error!("Failed to traverse graph via gRPC: {}", err);
                Err(Status::internal(format!("Failed to traverse graph: {}", err)))
            }
        }
    }

    /// Get graph statistics
    async fn get_graph_stats(
        &self,
        _request: Request<GetStatsRequest>,
    ) -> Result<Response<GraphStats>, Status> {
        debug!("gRPC GetGraphStats request");

        match self.unified_handlers.graph_service.get_stats() {
            Ok(stats) => {
                info!("Successfully retrieved graph statistics via gRPC");
                Ok(Response::new(stats))
            },
            Err(err) => {
                error!("Failed to get graph statistics via gRPC: {}", err);
                Err(Status::internal(format!("Failed to get graph statistics: {}", err)))
            }
        }
    }

    /// Batch create nodes
    async fn batch_create_nodes(
        &self,
        request: Request<BatchNodeRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC BatchCreateNodes request for {} nodes", req.nodes.len());

        match self.unified_handlers.graph_service.batch_create_nodes(req.nodes) {
            Ok(nodes) => {
                info!("Successfully batch created {} nodes via gRPC", nodes.len());
                let response = BatchResponse {
                    success: true,
                    nodes: nodes.into_iter().map(|n| (*n).clone()).collect(),
                    edges: vec![],
                    error_message: None,
                };
                Ok(Response::new(response))
            },
            Err(err) => {
                error!("Failed to batch create nodes via gRPC: {}", err);
                Err(Status::internal(format!("Failed to batch create nodes: {}", err)))
            }
        }
    }

    /// Batch create edges
    async fn batch_create_edges(
        &self,
        request: Request<BatchEdgeRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC BatchCreateEdges request for {} edges", req.edges.len());

        match self.unified_handlers.graph_service.batch_create_edges(req.edges) {
            Ok(edges) => {
                info!("Successfully batch created {} edges via gRPC", edges.len());
                let response = BatchResponse {
                    success: true,
                    nodes: vec![],
                    edges: edges.into_iter().map(|e| (*e).clone()).collect(),
                    error_message: None,
                };
                Ok(Response::new(response))
            },
            Err(err) => {
                error!("Failed to batch create edges via gRPC: {}", err);
                Err(Status::internal(format!("Failed to batch create edges: {}", err)))
            }
        }
    }
}