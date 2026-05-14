//! Graph database composition port trait for `proximadb-runtime`.
//!
//! `GraphPort` is the stable contract that the gRPC `GraphService`
//! in `proximadb-api` uses to call into the graph subsystem without
//! importing root-crate concrete types.
//!
//! `stream_traverse` returns `Vec<TraversalChunk>` (batch) to keep the port
//! protocol-neutral; the gRPC adapter wraps it in a `ReceiverStream`.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_proto::v1::{
    BatchEdgeRequest, BatchNodeRequest, BatchResponse, ConnectedComponentsResponse,
    CreateEdgeRequest, CreateNodeRequest, CycleCheckResponse, DeleteEdgeRequest, DeleteNodeRequest,
    Edge, EdgeQuery, GetEdgeRequest, GetNeighborsRequest, GetNodeRequest, GetStatsRequest,
    GraphQueryRequest, GraphQueryResponse, GraphStats, HybridSearchRequest, HybridSearchResponse,
    Node, NodeQuery, ShortestPathRequest, ShortestPathResponse, TraversalChunk, TraversalRequest,
    TraversalResponse, UniqueConstraintRequest, UniqueConstraintResponse, UpdateEdgeRequest,
    UpdateNodeRequest,
};

/// Port for graph database operations (CRUD, traversal, analytics, hybrid query).
///
/// Implemented by root-crate `GraphServiceImpl`.  When absent the gRPC adapter
/// returns `UNIMPLEMENTED` for every RPC.
#[async_trait]
pub trait GraphPort: Send + Sync {
    // ── Node CRUD ─────────────────────────────────────────────────────────

    async fn create_node(&self, request: CreateNodeRequest) -> Result<Node>;
    async fn get_node(&self, request: GetNodeRequest) -> Result<Node>;
    async fn update_node(&self, request: UpdateNodeRequest) -> Result<Node>;
    async fn delete_node(&self, request: DeleteNodeRequest) -> Result<Node>;

    // ── Edge CRUD ─────────────────────────────────────────────────────────

    async fn create_edge(&self, request: CreateEdgeRequest) -> Result<Edge>;
    async fn get_edge(&self, request: GetEdgeRequest) -> Result<Edge>;
    async fn update_edge(&self, request: UpdateEdgeRequest) -> Result<Edge>;
    async fn delete_edge(&self, request: DeleteEdgeRequest) -> Result<Edge>;

    // ── Queries ───────────────────────────────────────────────────────────

    async fn query_nodes(&self, request: NodeQuery) -> Result<BatchResponse>;
    async fn query_edges(&self, request: EdgeQuery) -> Result<BatchResponse>;
    async fn execute_query(&self, request: GraphQueryRequest) -> Result<GraphQueryResponse>;
    async fn get_neighbors(&self, request: GetNeighborsRequest) -> Result<BatchResponse>;

    // ── Traversal ─────────────────────────────────────────────────────────

    async fn traverse_graph(&self, request: TraversalRequest) -> Result<TraversalResponse>;

    /// Batch-fetch traversal chunks for the streaming RPC.
    ///
    /// The gRPC adapter wraps the returned `Vec<TraversalChunk>` in a
    /// `tokio::sync::mpsc` channel stream.
    async fn stream_traverse(&self, request: TraversalRequest) -> Result<Vec<TraversalChunk>>;

    // ── Analytics ─────────────────────────────────────────────────────────

    async fn get_graph_stats(&self, request: GetStatsRequest) -> Result<GraphStats>;
    async fn shortest_path(&self, request: ShortestPathRequest) -> Result<ShortestPathResponse>;

    async fn get_connected_components(
        &self,
        request: GetStatsRequest,
    ) -> Result<ConnectedComponentsResponse>;

    async fn has_cycle(&self, request: GetStatsRequest) -> Result<CycleCheckResponse>;

    // ── Constraints ───────────────────────────────────────────────────────

    async fn add_unique_constraint(
        &self,
        request: UniqueConstraintRequest,
    ) -> Result<UniqueConstraintResponse>;

    async fn remove_unique_constraint(
        &self,
        request: UniqueConstraintRequest,
    ) -> Result<UniqueConstraintResponse>;

    // ── Batch operations ──────────────────────────────────────────────────

    async fn batch_create_nodes(&self, request: BatchNodeRequest) -> Result<BatchResponse>;
    async fn batch_create_edges(&self, request: BatchEdgeRequest) -> Result<BatchResponse>;

    // ── Hybrid query (cross-modal) ────────────────────────────────────────

    async fn execute_hybrid_query(
        &self,
        request: HybridSearchRequest,
    ) -> Result<HybridSearchResponse>;
}
