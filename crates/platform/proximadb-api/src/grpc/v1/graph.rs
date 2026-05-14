//! # Graph Services (gRPC)
//!
//! gRPC implementations for graph database operations including nodes, edges,
//! traversals, and advanced graph algorithms.
//!
//! `execute_hybrid_query` routes through `ApiHandlersPort`. All graph CRUD operations
//! (create/get/update/delete node/edge, traversal, etc.) remain unimplemented until the
//! graph execution service is extracted to the runtime crate.

use std::sync::Arc;
use std::pin::Pin;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::{
    graph_service_server::{GraphService as GraphServiceTrait, GraphServiceServer},
    *
};
use proximadb_runtime::ApiHandlersPort;

/// Streaming response type for stream_traverse
pub type StreamTraverseStream = Pin<
    Box<dyn tokio_stream::Stream<Item = Result<TraversalChunk, Status>> + Send>,
>;

/// Graph service implementation.
pub struct GraphServiceImpl {
    port: Arc<dyn ApiHandlersPort>,
}

impl GraphServiceImpl {
    /// Create a new graph service backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self { port }
    }

    /// Convert to tonic server.
    pub fn into_server(self) -> GraphServiceServer<Self> {
        GraphServiceServer::new(self)
    }
}

// Placeholder trait implementation - will be implemented after migration
#[tonic::async_trait]
impl GraphServiceTrait for GraphServiceImpl {
    async fn create_node(
        &self,
        _request: Request<CreateNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn get_node(
        &self,
        _request: Request<GetNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn update_node(
        &self,
        _request: Request<UpdateNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn delete_node(
        &self,
        _request: Request<DeleteNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn create_edge(
        &self,
        _request: Request<CreateEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn get_edge(
        &self,
        _request: Request<GetEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn update_edge(
        &self,
        _request: Request<UpdateEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn delete_edge(
        &self,
        _request: Request<DeleteEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn query_nodes(
        &self,
        _request: Request<NodeQuery>,
    ) -> Result<Response<BatchResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn query_edges(
        &self,
        _request: Request<EdgeQuery>,
    ) -> Result<Response<BatchResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn execute_query(
        &self,
        _request: Request<GraphQueryRequest>,
    ) -> Result<Response<GraphQueryResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn get_neighbors(
        &self,
        _request: Request<GetNeighborsRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn traverse_graph(
        &self,
        _request: Request<TraversalRequest>,
    ) -> Result<Response<TraversalResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    type StreamTraverseStream = StreamTraverseStream;

    async fn stream_traverse(
        &self,
        _request: Request<TraversalRequest>,
    ) -> Result<Response<Self::StreamTraverseStream>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn get_graph_stats(
        &self,
        _request: Request<GetStatsRequest>,
    ) -> Result<Response<GraphStats>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn shortest_path(
        &self,
        _request: Request<ShortestPathRequest>,
    ) -> Result<Response<ShortestPathResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn get_connected_components(
        &self,
        _request: Request<GetStatsRequest>,
    ) -> Result<Response<ConnectedComponentsResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn has_cycle(
        &self,
        _request: Request<GetStatsRequest>,
    ) -> Result<Response<CycleCheckResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn add_unique_constraint(
        &self,
        _request: Request<UniqueConstraintRequest>,
    ) -> Result<Response<UniqueConstraintResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn remove_unique_constraint(
        &self,
        _request: Request<UniqueConstraintRequest>,
    ) -> Result<Response<UniqueConstraintResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn batch_create_nodes(
        &self,
        _request: Request<BatchNodeRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn batch_create_edges(
        &self,
        _request: Request<BatchEdgeRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        Err(Status::unimplemented("Graph service migration in progress"))
    }

    async fn execute_hybrid_query(
        &self,
        request: Request<HybridSearchRequest>,
    ) -> Result<Response<HybridSearchResponse>, Status> {
        let req = request.into_inner();
        self.port
            .execute_hybrid_query(req)
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Hybrid query failed: {e}")))
    }
}
