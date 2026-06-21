// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC V2 native graph service implementation.
//!
//! This is the canonical, always-registered graph surface (`proximadb.v2.
//! ProximaGraphService`). It replaces the deprecated `proximadb.v1.GraphService`,
//! which is only reachable behind `enable_grpc_v1_compat` (default off).
//!
//! ## Design
//!
//! - **v2-native wire types.** The handlers speak the self-contained `proximadb.
//!   v2` graph messages (`GraphNode`, `GraphEdge`, ...). No v1 graph types appear
//!   in the public surface.
//! - **Shared backing logic.** Every handler delegates to the same
//!   `UnifiedHandlers::graph_operations_service` the v1 adapter uses — no new
//!   business logic. v2 messages are mapped to the internal (v1 proto) domain
//!   types at the handler boundary only; see the `conv` helpers below.
//! - **Structural tenant isolation.** Each handler derives the effective backing
//!   graph namespace from the request tenant (`x-tenant-id` / auth context) via
//!   [`grpc_auth::tenant_id`]. Isolation is a namespace on the storage key, never
//!   a per-query predicate. The backing `GraphOperationsService` does not yet
//!   accept a `TenantContext`; deeper plumbing is tracked as a follow-up TD.
//!
//! ## Deferred RPCs
//!
//! The first surface ships the core graph operations (node/edge CRUD, node/edge
//! queries, neighbors, traversal, shortest path, stats). StreamTraverse,
//! connected-components/cycle analysis, unique-constraint DDL, batch create,
//! ExecuteQuery (Cypher/Gremlin), ExecuteHybridQuery, and the retired
//! PULSAR/QUASAR RPCs are deferred (tracked as a follow-up TD).

use std::sync::Arc;
use std::time::Instant;

use tonic::{Request, Response, Status};
use tracing::{debug, error};

use crate::api_handlers::UnifiedHandlers;
use crate::network::grpc::auth as grpc_auth;
use crate::proto::proximadb_v1 as pv1;
use crate::proto::proximadb_v2 as pv2;
use crate::proto::proximadb_v2::proxima_graph_service_server::{
    ProximaGraphService, ProximaGraphServiceServer,
};

/// gRPC V2 native graph service.
pub struct ProximaGraphServiceImpl {
    request_handlers: Arc<UnifiedHandlers>,
}

impl ProximaGraphServiceImpl {
    /// Create a new service over the shared unified handlers.
    pub fn new(request_handlers: Arc<UnifiedHandlers>) -> Self {
        Self { request_handlers }
    }

    /// Convert to a tonic server.
    pub fn into_server(self) -> ProximaGraphServiceServer<Self> {
        ProximaGraphServiceServer::new(self)
    }

    /// Derive the effective backing graph namespace from the request tenant.
    ///
    /// Isolation is structural: the tenant is folded into the storage key (the
    /// `GraphOperationsService` registry/path key), never applied as a per-query
    /// predicate. Embedded / unauthenticated calls (no tenant) fall back to the
    /// raw `graph_id` for backward compatibility.
    fn effective_graph_id<T>(request: &Request<T>, graph_id: &str) -> String {
        match grpc_auth::tenant_id(request) {
            Some(tenant) if !tenant.is_empty() => format!("{tenant}::{graph_id}"),
            _ => graph_id.to_string(),
        }
    }
}

// ============================================================================
// v2 <-> internal (v1 proto) type mapping — handler boundary only.
// ============================================================================

mod conv {
    use super::{pv1, pv2};

    /// v2 property value -> internal property value.
    pub(super) fn property_value_to_v1(p: pv2::GraphPropertyValue) -> pv1::PropertyValue {
        use pv1::property_value::Value as V1;
        use pv2::graph_property_value::Value as V2;
        let value = p.value.map(|v| match v {
            V2::StringValue(s) => V1::StringValue(s),
            V2::IntValue(i) => V1::IntValue(i),
            V2::DoubleValue(d) => V1::DoubleValue(d),
            V2::BoolValue(b) => V1::BoolValue(b),
            V2::BytesValue(b) => V1::BytesValue(b),
            V2::ArrayValue(a) => V1::ArrayValue(pv1::PropertyArray {
                values: a.values.into_iter().map(property_value_to_v1).collect(),
            }),
            V2::MapValue(m) => V1::ObjectValue(pv1::PropertyObject {
                fields: m
                    .fields
                    .into_iter()
                    .map(|(k, v)| (k, property_value_to_v1(v)))
                    .collect(),
            }),
        });
        pv1::PropertyValue { value }
    }

    /// Internal property value -> v2 property value.
    ///
    /// Vector-valued properties have no v2 representation (embeddings live on the
    /// dedicated `embedding` field) and map to an empty value.
    pub(super) fn property_value_to_v2(p: pv1::PropertyValue) -> pv2::GraphPropertyValue {
        use pv1::property_value::Value as V1;
        use pv2::graph_property_value::Value as V2;
        let value = p.value.and_then(|v| match v {
            V1::StringValue(s) => Some(V2::StringValue(s)),
            V1::IntValue(i) => Some(V2::IntValue(i)),
            V1::DoubleValue(d) => Some(V2::DoubleValue(d)),
            V1::BoolValue(b) => Some(V2::BoolValue(b)),
            V1::BytesValue(b) => Some(V2::BytesValue(b)),
            V1::ArrayValue(a) => Some(V2::ArrayValue(pv2::GraphPropertyArray {
                values: a.values.into_iter().map(property_value_to_v2).collect(),
            })),
            V1::ObjectValue(o) => Some(V2::MapValue(pv2::GraphPropertyMap {
                fields: o
                    .fields
                    .into_iter()
                    .map(|(k, v)| (k, property_value_to_v2(v)))
                    .collect(),
            })),
            _ => None,
        });
        pv2::GraphPropertyValue { value }
    }

    fn embedding_to_v1(e: pv2::GraphEmbedding) -> pv1::EmbeddingVersion {
        pv1::EmbeddingVersion {
            model_id: e.model_id,
            model_version: e.model_version,
            vector: e.vector,
            dimension: e.dimension,
            created_at_ms: 0,
            model_params: Default::default(),
            modality: 0,
        }
    }

    fn embedding_to_v2(e: pv1::EmbeddingVersion) -> pv2::GraphEmbedding {
        pv2::GraphEmbedding {
            vector: e.vector,
            dimension: e.dimension,
            model_id: e.model_id,
            model_version: e.model_version,
        }
    }

    pub(super) fn node_to_v1(n: pv2::GraphNode) -> pv1::Node {
        pv1::Node {
            id: n.id,
            labels: n.labels,
            properties: n
                .properties
                .into_iter()
                .map(|(k, v)| (k, property_value_to_v1(v)))
                .collect(),
            embedding: n.embedding.map(embedding_to_v1),
            created_at_ms: n.created_at_ms,
            updated_at_ms: n.updated_at_ms,
        }
    }

    pub(super) fn node_to_v2(n: pv1::Node) -> pv2::GraphNode {
        pv2::GraphNode {
            id: n.id,
            labels: n.labels,
            properties: n
                .properties
                .into_iter()
                .map(|(k, v)| (k, property_value_to_v2(v)))
                .collect(),
            embedding: n.embedding.map(embedding_to_v2),
            created_at_ms: n.created_at_ms,
            updated_at_ms: n.updated_at_ms,
        }
    }

    pub(super) fn edge_to_v1(e: pv2::GraphEdge) -> pv1::Edge {
        pv1::Edge {
            id: e.id,
            from_node_id: e.from_node_id,
            to_node_id: e.to_node_id,
            edge_type: e.edge_type,
            properties: e
                .properties
                .into_iter()
                .map(|(k, v)| (k, property_value_to_v1(v)))
                .collect(),
            weight: e.weight,
            created_at_ms: e.created_at_ms,
            updated_at_ms: e.updated_at_ms,
        }
    }

    pub(super) fn edge_to_v2(e: pv1::Edge) -> pv2::GraphEdge {
        pv2::GraphEdge {
            id: e.id,
            from_node_id: e.from_node_id,
            to_node_id: e.to_node_id,
            edge_type: e.edge_type,
            properties: e
                .properties
                .into_iter()
                .map(|(k, v)| (k, property_value_to_v2(v)))
                .collect(),
            weight: e.weight,
            created_at_ms: e.created_at_ms,
            updated_at_ms: e.updated_at_ms,
        }
    }

    /// v2 property filter -> internal. Operator ordinals are aligned by design,
    /// so the enum is a direct numeric carry-over.
    pub(super) fn filter_to_v1(f: pv2::GraphPropertyFilter) -> pv1::PropertyFilter {
        pv1::PropertyFilter {
            key: f.key,
            operator: f.operator,
            value: f.value.map(property_value_to_v1),
        }
    }

    pub(super) fn stats_to_v2(s: pv1::GraphStats) -> pv2::GraphStats {
        pv2::GraphStats {
            total_nodes: s.total_nodes,
            total_edges: s.total_edges,
            label_stats: s
                .label_stats
                .into_iter()
                .map(|l| pv2::GraphLabelStats {
                    label: l.label,
                    count: l.count,
                })
                .collect(),
            edge_type_stats: s
                .edge_type_stats
                .into_iter()
                .map(|e| pv2::GraphEdgeTypeStats {
                    edge_type: e.edge_type,
                    count: e.count,
                })
                .collect(),
            total_properties: s.total_properties,
            memory_usage_bytes: s.memory_usage_bytes,
            average_degree: s.average_degree,
            max_degree: s.max_degree,
            connected_components: s.connected_components,
        }
    }

    pub(super) fn traversal_stats_to_v2(s: pv1::TraversalStats) -> pv2::GraphTraversalStats {
        pv2::GraphTraversalStats {
            nodes_visited: s.nodes_visited,
            edges_traversed: s.edges_traversed,
            max_depth_reached: s.max_depth_reached,
            execution_time_microseconds: s.execution_time_microseconds,
        }
    }
}

/// Map an internal error to a gRPC `Status`, inferring the status code from the
/// message (mirrors the v1 adapter's canonical-code mapping).
fn graph_status(operation: &str, err: impl std::fmt::Display) -> Status {
    let message = err.to_string();
    let lower = message.to_lowercase();
    let full = format!("Failed to {operation}: {message}");
    if lower.contains("not found") || lower.contains("does not exist") {
        Status::not_found(full)
    } else if lower.contains("already exists") || lower.contains("duplicate") {
        Status::already_exists(full)
    } else if lower.contains("invalid") || lower.contains("required") || lower.contains("missing") {
        Status::invalid_argument(full)
    } else if lower.contains("constraint") || lower.contains("unique") {
        Status::failed_precondition(full)
    } else if lower.contains("timeout") || lower.contains("timed out") {
        Status::deadline_exceeded(full)
    } else if lower.contains("permission") || lower.contains("denied") {
        Status::permission_denied(full)
    } else {
        Status::internal(full)
    }
}

/// Resolve a paging cursor: parse `offset:<n>` continuation tokens into an
/// explicit offset, and compute the `next_token` when more results may exist.
fn next_token(returned: usize, limit: Option<u32>, offset: Option<u32>) -> Option<String> {
    match limit {
        Some(l) if returned as u32 == l && l > 0 => {
            let next = offset.unwrap_or(0).saturating_add(l);
            Some(format!("offset:{next}"))
        }
        _ => None,
    }
}

/// Apply an `offset:<n>` continuation token to an absent offset.
fn resolve_offset(offset: Option<u32>, token: &Option<String>) -> Option<u32> {
    if offset.is_some() {
        return offset;
    }
    token
        .as_ref()
        .and_then(|t| t.strip_prefix("offset:"))
        .and_then(|rest| rest.parse::<u32>().ok())
}

#[tonic::async_trait]
impl ProximaGraphService for ProximaGraphServiceImpl {
    async fn create_node(
        &self,
        request: Request<pv2::CreateGraphNodeRequest>,
    ) -> Result<Response<pv2::GraphNodeResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC CreateNode graph={graph_id}");
        let node = req
            .node
            .ok_or_else(|| Status::invalid_argument("node is required"))?;
        match self
            .request_handlers
            .graph_operations_service
            .create_node(&graph_id, conv::node_to_v1(node))
            .await
        {
            Ok(created) => Ok(Response::new(pv2::GraphNodeResponse {
                node: Some(conv::node_to_v2((*created).clone())),
            })),
            Err(e) => {
                error!("v2 gRPC CreateNode failed: {e}");
                Err(graph_status("create node", e))
            }
        }
    }

    async fn get_node(
        &self,
        request: Request<pv2::GetGraphNodeRequest>,
    ) -> Result<Response<pv2::GraphNodeResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC GetNode graph={graph_id} node={}", req.node_id);
        match self
            .request_handlers
            .graph_operations_service
            .get_node(&graph_id, &req.node_id)
            .await
        {
            Ok(found) => Ok(Response::new(pv2::GraphNodeResponse {
                node: found.map(|n| conv::node_to_v2((*n).clone())),
            })),
            Err(e) => Err(graph_status("get node", e)),
        }
    }

    async fn update_node(
        &self,
        request: Request<pv2::UpdateGraphNodeRequest>,
    ) -> Result<Response<pv2::GraphNodeResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC UpdateNode graph={graph_id}");
        let node = req
            .node
            .ok_or_else(|| Status::invalid_argument("node is required"))?;
        match self
            .request_handlers
            .graph_operations_service
            .update_node(&graph_id, conv::node_to_v1(node))
            .await
        {
            Ok(updated) => Ok(Response::new(pv2::GraphNodeResponse {
                node: Some(conv::node_to_v2((*updated).clone())),
            })),
            Err(e) => Err(graph_status("update node", e)),
        }
    }

    async fn delete_node(
        &self,
        request: Request<pv2::DeleteGraphNodeRequest>,
    ) -> Result<Response<pv2::DeleteGraphNodeResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC DeleteNode graph={graph_id} node={}", req.node_id);
        match self
            .request_handlers
            .graph_operations_service
            .delete_node(&graph_id, &req.node_id)
            .await
        {
            Ok(removed) => Ok(Response::new(pv2::DeleteGraphNodeResponse {
                deleted: removed.is_some(),
                node: removed.map(|n| conv::node_to_v2((*n).clone())),
            })),
            Err(e) => Err(graph_status("delete node", e)),
        }
    }

    async fn create_edge(
        &self,
        request: Request<pv2::CreateGraphEdgeRequest>,
    ) -> Result<Response<pv2::GraphEdgeResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC CreateEdge graph={graph_id}");
        let edge = req
            .edge
            .ok_or_else(|| Status::invalid_argument("edge is required"))?;
        match self
            .request_handlers
            .graph_operations_service
            .create_edge(&graph_id, conv::edge_to_v1(edge))
            .await
        {
            Ok(created) => Ok(Response::new(pv2::GraphEdgeResponse {
                edge: Some(conv::edge_to_v2((*created).clone())),
            })),
            Err(e) => Err(graph_status("create edge", e)),
        }
    }

    async fn get_edge(
        &self,
        request: Request<pv2::GetGraphEdgeRequest>,
    ) -> Result<Response<pv2::GraphEdgeResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC GetEdge graph={graph_id} edge={}", req.edge_id);
        match self
            .request_handlers
            .graph_operations_service
            .get_edge(&graph_id, &req.edge_id)
            .await
        {
            Ok(found) => Ok(Response::new(pv2::GraphEdgeResponse {
                edge: found.map(|e| conv::edge_to_v2((*e).clone())),
            })),
            Err(e) => Err(graph_status("get edge", e)),
        }
    }

    async fn update_edge(
        &self,
        request: Request<pv2::UpdateGraphEdgeRequest>,
    ) -> Result<Response<pv2::GraphEdgeResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC UpdateEdge graph={graph_id}");
        let edge = req
            .edge
            .ok_or_else(|| Status::invalid_argument("edge is required"))?;
        match self
            .request_handlers
            .graph_operations_service
            .update_edge(&graph_id, conv::edge_to_v1(edge))
            .await
        {
            Ok(updated) => Ok(Response::new(pv2::GraphEdgeResponse {
                edge: Some(conv::edge_to_v2((*updated).clone())),
            })),
            Err(e) => Err(graph_status("update edge", e)),
        }
    }

    async fn delete_edge(
        &self,
        request: Request<pv2::DeleteGraphEdgeRequest>,
    ) -> Result<Response<pv2::DeleteGraphEdgeResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC DeleteEdge graph={graph_id} edge={}", req.edge_id);
        match self
            .request_handlers
            .graph_operations_service
            .delete_edge(&graph_id, &req.edge_id)
            .await
        {
            Ok(removed) => Ok(Response::new(pv2::DeleteGraphEdgeResponse {
                deleted: removed.is_some(),
                edge: removed.map(|e| conv::edge_to_v2((*e).clone())),
            })),
            Err(e) => Err(graph_status("delete edge", e)),
        }
    }

    async fn query_nodes(
        &self,
        request: Request<pv2::QueryGraphNodesRequest>,
    ) -> Result<Response<pv2::QueryGraphNodesResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!(
            "v2 gRPC QueryNodes graph={graph_id} labels={:?}",
            req.labels
        );
        let offset = resolve_offset(req.offset, &req.continuation_token);
        let query = pv1::NodeQuery {
            graph_id: graph_id.clone(),
            labels: req.labels,
            filters: req.filters.into_iter().map(conv::filter_to_v1).collect(),
            limit: req.limit,
            offset,
            continuation_token: None,
        };
        match self
            .request_handlers
            .graph_operations_service
            .query_nodes(&graph_id, query)
            .await
        {
            Ok(nodes) => {
                let nodes: Vec<pv2::GraphNode> = nodes
                    .into_iter()
                    .map(|n| conv::node_to_v2((*n).clone()))
                    .collect();
                let token = next_token(nodes.len(), req.limit, offset);
                Ok(Response::new(pv2::QueryGraphNodesResponse {
                    nodes,
                    next_token: token,
                }))
            }
            Err(e) => Err(graph_status("query nodes", e)),
        }
    }

    async fn query_edges(
        &self,
        request: Request<pv2::QueryGraphEdgesRequest>,
    ) -> Result<Response<pv2::QueryGraphEdgesResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC QueryEdges graph={graph_id}");
        let offset = resolve_offset(req.offset, &req.continuation_token);
        let query = pv1::EdgeQuery {
            graph_id: graph_id.clone(),
            from_node_id: req.from_node_id,
            to_node_id: req.to_node_id,
            edge_types: req.edge_types,
            filters: req.filters.into_iter().map(conv::filter_to_v1).collect(),
            limit: req.limit,
            offset,
            continuation_token: None,
        };
        match self
            .request_handlers
            .graph_operations_service
            .query_edges(&graph_id, query)
            .await
        {
            Ok(edges) => {
                let edges: Vec<pv2::GraphEdge> = edges
                    .into_iter()
                    .map(|e| conv::edge_to_v2((*e).clone()))
                    .collect();
                let token = next_token(edges.len(), req.limit, offset);
                Ok(Response::new(pv2::QueryGraphEdgesResponse {
                    edges,
                    next_token: token,
                }))
            }
            Err(e) => Err(graph_status("query edges", e)),
        }
    }

    async fn get_neighbors(
        &self,
        request: Request<pv2::GetGraphNeighborsRequest>,
    ) -> Result<Response<pv2::GetGraphNeighborsResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!("v2 gRPC GetNeighbors graph={graph_id} node={}", req.node_id);
        match self
            .request_handlers
            .graph_operations_service
            .get_neighbors(&graph_id, &req.node_id)
            .await
        {
            Ok(nodes) => Ok(Response::new(pv2::GetGraphNeighborsResponse {
                nodes: nodes
                    .into_iter()
                    .map(|n| conv::node_to_v2((*n).clone()))
                    .collect(),
            })),
            Err(e) => Err(graph_status("get neighbors", e)),
        }
    }

    async fn traverse_graph(
        &self,
        request: Request<pv2::TraverseGraphRequest>,
    ) -> Result<Response<pv2::TraverseGraphResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        let start = Instant::now();
        debug!(
            "v2 gRPC TraverseGraph graph={graph_id} start={}",
            req.start_node_id
        );
        // Algorithm ordinals are aligned with the internal enum by design.
        let internal = pv1::TraversalRequest {
            graph_id: graph_id.clone(),
            start_node_id: req.start_node_id,
            max_depth: req.max_depth,
            edge_types: req.edge_types,
            node_labels: req.node_labels,
            filters: req.filters.into_iter().map(conv::filter_to_v1).collect(),
            algorithm: req.algorithm,
            limit: req.limit,
            timeout_ms: req.timeout_ms,
            max_frontier: req.max_frontier,
        };
        match self
            .request_handlers
            .graph_operations_service
            .traverse(&graph_id, internal)
            .await
        {
            Ok(resp) => {
                let mut stats = resp.stats.map(conv::traversal_stats_to_v2);
                if let Some(stats) = stats.as_mut()
                    && stats.execution_time_microseconds == 0
                {
                    stats.execution_time_microseconds = start.elapsed().as_micros() as u64;
                }
                Ok(Response::new(pv2::TraverseGraphResponse {
                    nodes: resp.nodes.into_iter().map(conv::node_to_v2).collect(),
                    edges: resp.edges.into_iter().map(conv::edge_to_v2).collect(),
                    paths: resp
                        .paths
                        .into_iter()
                        .map(|p| pv2::GraphPath {
                            node_ids: p.entities.into_iter().map(|e| e.id).collect(),
                        })
                        .collect(),
                    stats,
                }))
            }
            Err(e) => Err(graph_status("traverse graph", e)),
        }
    }

    async fn shortest_path(
        &self,
        request: Request<pv2::GraphShortestPathRequest>,
    ) -> Result<Response<pv2::GraphShortestPathResponse>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        let req = request.into_inner();
        debug!(
            "v2 gRPC ShortestPath graph={graph_id} {} -> {}",
            req.start_node_id, req.target_node_id
        );
        let edge_types = if req.edge_types.is_empty() {
            None
        } else {
            Some(req.edge_types)
        };
        // Map the v2 algorithm onto the internal enum, defaulting to Dijkstra.
        let algorithm = match pv1::ShortestPathAlgorithm::try_from(req.algorithm.unwrap_or(0)) {
            Ok(pv1::ShortestPathAlgorithm::Astar) => pv1::ShortestPathAlgorithm::Astar,
            _ => pv1::ShortestPathAlgorithm::Dijkstra,
        };
        match self
            .request_handlers
            .graph_operations_service
            .shortest_path(
                &graph_id,
                &req.start_node_id,
                &req.target_node_id,
                req.max_depth,
                edge_types,
                Some(algorithm),
                req.k,
                req.enable_prefetch,
                req.prefetch_budget.map(|b| b as usize),
            )
            .await
        {
            Ok(Some((path, total_weight))) => Ok(Response::new(pv2::GraphShortestPathResponse {
                node_ids: path,
                total_weight: Some(total_weight),
                found: true,
            })),
            Ok(None) => Ok(Response::new(pv2::GraphShortestPathResponse {
                node_ids: vec![],
                total_weight: None,
                found: false,
            })),
            Err(e) => Err(graph_status("compute shortest path", e)),
        }
    }

    async fn get_graph_stats(
        &self,
        request: Request<pv2::GetGraphStatsRequest>,
    ) -> Result<Response<pv2::GraphStats>, Status> {
        let graph_id = Self::effective_graph_id(&request, &request.get_ref().graph_id);
        debug!("v2 gRPC GetGraphStats graph={graph_id}");
        match self
            .request_handlers
            .graph_operations_service
            .get_stats(&graph_id)
            .await
        {
            Ok(stats) => Ok(Response::new(conv::stats_to_v2(stats))),
            Err(e) => Err(graph_status("get graph statistics", e)),
        }
    }
}
