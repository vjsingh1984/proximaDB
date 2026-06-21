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
    /// The shared graph backing service — the only dependency the handlers use.
    /// Held directly (rather than the whole `UnifiedHandlers`) so the service is
    /// constructible in tests from a standalone `GraphOperationsService`.
    graph: Arc<crate::graph::GraphOperationsService>,
}

impl ProximaGraphServiceImpl {
    /// Create a new service over the shared unified handlers (production wiring).
    pub fn new(request_handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            graph: request_handlers.graph_operations_service.clone(),
        }
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
            .graph
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
        match self.graph.get_node(&graph_id, &req.node_id).await {
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
            .graph
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
        match self.graph.delete_node(&graph_id, &req.node_id).await {
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
            .graph
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
        match self.graph.get_edge(&graph_id, &req.edge_id).await {
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
            .graph
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
        match self.graph.delete_edge(&graph_id, &req.edge_id).await {
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
        match self.graph.query_nodes(&graph_id, query).await {
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
        match self.graph.query_edges(&graph_id, query).await {
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
        match self.graph.get_neighbors(&graph_id, &req.node_id).await {
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
        match self.graph.traverse(&graph_id, internal).await {
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
            .graph
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
        match self.graph.get_stats(&graph_id).await {
            Ok(stats) => Ok(Response::new(conv::stats_to_v2(stats))),
            Err(e) => Err(graph_status("get graph statistics", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphOperationsService;
    use std::collections::HashMap;

    fn gpv(v: pv2::graph_property_value::Value) -> pv2::GraphPropertyValue {
        pv2::GraphPropertyValue { value: Some(v) }
    }

    fn str_pv(s: &str) -> pv2::GraphPropertyValue {
        gpv(pv2::graph_property_value::Value::StringValue(s.to_string()))
    }

    fn int_pv(i: i64) -> pv2::GraphPropertyValue {
        gpv(pv2::graph_property_value::Value::IntValue(i))
    }

    /// Build a v2 graph service backed by a standalone `GraphOperationsService`
    /// with `graph_id` already provisioned (the v2 gRPC surface defers graph
    /// creation, mirroring production where the collection is made via REST).
    async fn service_with_graph(graph_id: &str) -> anyhow::Result<ProximaGraphServiceImpl> {
        let graph = Arc::new(GraphOperationsService::new());
        graph
            .create_graph_collection(pv1::CreateGraphRequest {
                graph_id: graph_id.to_string(),
                name: Some(graph_id.to_string()),
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await?;
        Ok(ProximaGraphServiceImpl { graph })
    }

    /// Pure check of the v2<->internal property-value mapping, including the
    /// nested array/map shapes (`map_value` <-> v1 `object_value`).
    #[test]
    fn conv_property_value_roundtrips_nested() {
        use pv2::graph_property_value::Value as V;
        let original = gpv(V::ArrayValue(pv2::GraphPropertyArray {
            values: vec![
                str_pv("s"),
                int_pv(7),
                gpv(V::DoubleValue(1.5)),
                gpv(V::BoolValue(true)),
                gpv(V::MapValue(pv2::GraphPropertyMap {
                    fields: HashMap::from([("k".to_string(), str_pv("v"))]),
                })),
            ],
        }));
        let back =
            super::conv::property_value_to_v2(super::conv::property_value_to_v1(original.clone()));
        assert_eq!(back, original);
    }

    /// Node create -> get round-trip (properties survive the v2->v1->v2 mapping),
    /// plus a miss returning an empty response rather than an error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn node_crud_roundtrip() -> anyhow::Result<()> {
        let gid = "v2_graph_node_crud";
        let svc = service_with_graph(gid).await?;

        let node = pv2::GraphNode {
            id: "alice".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::from([
                ("name".to_string(), str_pv("Alice")),
                ("age".to_string(), int_pv(30)),
            ]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let created = svc
            .create_node(Request::new(pv2::CreateGraphNodeRequest {
                graph_id: gid.to_string(),
                node: Some(node),
            }))
            .await?
            .into_inner()
            .node
            .ok_or_else(|| anyhow::anyhow!("create_node returned no node"))?;
        assert_eq!(created.id, "alice");
        assert_eq!(created.labels, vec!["Person".to_string()]);

        let got = svc
            .get_node(Request::new(pv2::GetGraphNodeRequest {
                graph_id: gid.to_string(),
                node_id: "alice".to_string(),
            }))
            .await?
            .into_inner()
            .node
            .ok_or_else(|| anyhow::anyhow!("get_node returned no node"))?;
        assert_eq!(got.id, "alice");
        // Property survived the v2 -> internal -> v2 conversion round-trip.
        match got.properties.get("name").and_then(|p| p.value.clone()) {
            Some(pv2::graph_property_value::Value::StringValue(s)) => assert_eq!(s, "Alice"),
            other => anyhow::bail!("unexpected name property: {other:?}"),
        }

        // A miss is an empty response, not a gRPC error.
        let miss = svc
            .get_node(Request::new(pv2::GetGraphNodeRequest {
                graph_id: gid.to_string(),
                node_id: "nobody".to_string(),
            }))
            .await?
            .into_inner();
        assert!(miss.node.is_none());

        let deleted = svc
            .delete_node(Request::new(pv2::DeleteGraphNodeRequest {
                graph_id: gid.to_string(),
                node_id: "alice".to_string(),
            }))
            .await?
            .into_inner();
        assert!(deleted.deleted);
        Ok(())
    }

    /// Edge create + label query + traversal + shortest path + stats over a
    /// two-node KNOWS graph — exercises the full v2 handler surface end to end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn edge_query_traverse_shortest_path() -> anyhow::Result<()> {
        let gid = "v2_graph_edge_traverse";
        let svc = service_with_graph(gid).await?;

        for id in ["alice", "bob"] {
            svc.create_node(Request::new(pv2::CreateGraphNodeRequest {
                graph_id: gid.to_string(),
                node: Some(pv2::GraphNode {
                    id: id.to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                }),
            }))
            .await?;
        }

        let edge = svc
            .create_edge(Request::new(pv2::CreateGraphEdgeRequest {
                graph_id: gid.to_string(),
                edge: Some(pv2::GraphEdge {
                    id: "e1".to_string(),
                    from_node_id: "alice".to_string(),
                    to_node_id: "bob".to_string(),
                    edge_type: "KNOWS".to_string(),
                    properties: HashMap::from([("since".to_string(), int_pv(2020))]),
                    weight: Some(1.5),
                    created_at_ms: 0,
                    updated_at_ms: 0,
                }),
            }))
            .await?
            .into_inner()
            .edge
            .ok_or_else(|| anyhow::anyhow!("create_edge returned no edge"))?;
        assert_eq!(edge.edge_type, "KNOWS");
        assert_eq!(edge.weight, Some(1.5));

        let nodes = svc
            .query_nodes(Request::new(pv2::QueryGraphNodesRequest {
                graph_id: gid.to_string(),
                labels: vec!["Person".to_string()],
                filters: vec![],
                limit: None,
                offset: None,
                continuation_token: None,
            }))
            .await?
            .into_inner();
        assert_eq!(nodes.nodes.len(), 2);

        let traversal = svc
            .traverse_graph(Request::new(pv2::TraverseGraphRequest {
                graph_id: gid.to_string(),
                start_node_id: "alice".to_string(),
                max_depth: 2,
                edge_types: vec![],
                node_labels: vec![],
                filters: vec![],
                algorithm: pv2::GraphTraversalAlgorithm::Bfs as i32,
                limit: None,
                timeout_ms: None,
                max_frontier: None,
            }))
            .await?
            .into_inner();
        assert!(
            traversal.nodes.iter().any(|n| n.id == "bob"),
            "traversal from alice should reach bob"
        );

        let sp = svc
            .shortest_path(Request::new(pv2::GraphShortestPathRequest {
                graph_id: gid.to_string(),
                start_node_id: "alice".to_string(),
                target_node_id: "bob".to_string(),
                max_depth: None,
                edge_types: vec![],
                algorithm: None,
                k: None,
                enable_prefetch: None,
                prefetch_budget: None,
            }))
            .await?
            .into_inner();
        assert!(sp.found, "expected a path alice -> bob");
        assert_eq!(sp.node_ids.first(), Some(&"alice".to_string()));
        assert_eq!(sp.node_ids.last(), Some(&"bob".to_string()));

        let stats = svc
            .get_graph_stats(Request::new(pv2::GetGraphStatsRequest {
                graph_id: gid.to_string(),
            }))
            .await?
            .into_inner();
        assert!(stats.total_nodes >= 2);
        Ok(())
    }

    /// Structural tenant isolation: the same logical `graph_id` under two
    /// different tenants resolves to distinct backing graphs, so a node written
    /// under tenant A is invisible to tenant B.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tenant_namespacing_isolates_graphs() -> anyhow::Result<()> {
        // Provision both tenant-namespaced backing graphs on one service.
        let graph = Arc::new(GraphOperationsService::new());
        for ns in ["tenantA::shared", "tenantB::shared"] {
            graph
                .create_graph_collection(pv1::CreateGraphRequest {
                    graph_id: ns.to_string(),
                    name: Some(ns.to_string()),
                    description: None,
                    schema: None,
                    storage_config: None,
                    engine_config: None,
                    access_control: None,
                })
                .await?;
        }
        let svc = ProximaGraphServiceImpl { graph };

        let mut create = Request::new(pv2::CreateGraphNodeRequest {
            graph_id: "shared".to_string(),
            node: Some(pv2::GraphNode {
                id: "secret".to_string(),
                labels: vec!["Doc".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            }),
        });
        create
            .metadata_mut()
            .insert("x-tenant-id", "tenantA".parse()?);
        svc.create_node(create).await?;

        // Tenant A sees it.
        let mut get_a = Request::new(pv2::GetGraphNodeRequest {
            graph_id: "shared".to_string(),
            node_id: "secret".to_string(),
        });
        get_a
            .metadata_mut()
            .insert("x-tenant-id", "tenantA".parse()?);
        assert!(svc.get_node(get_a).await?.into_inner().node.is_some());

        // Tenant B, same logical graph_id, does not.
        let mut get_b = Request::new(pv2::GetGraphNodeRequest {
            graph_id: "shared".to_string(),
            node_id: "secret".to_string(),
        });
        get_b
            .metadata_mut()
            .insert("x-tenant-id", "tenantB".parse()?);
        assert!(svc.get_node(get_b).await?.into_inner().node.is_none());
        Ok(())
    }
}
