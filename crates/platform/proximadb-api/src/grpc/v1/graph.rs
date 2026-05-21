//! # Graph Services (gRPC)
//!
//! gRPC implementation for graph database operations — node/edge CRUD,
//! traversal, analytics, hybrid query.  Each RPC delegates to the injected
//! `GraphPort`; when no port is provided the service returns `UNIMPLEMENTED`.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::{
    graph_service_server::{GraphService as GraphServiceTrait, GraphServiceServer},
    *,
};
use proximadb_runtime::GraphPort;

/// Streaming response type for `stream_traverse`.
pub type StreamTraverseStream = ReceiverStream<Result<TraversalChunk, Status>>;

/// gRPC GraphService backed by a `GraphPort`.
pub struct GraphServiceImpl {
    port: Option<Arc<dyn GraphPort>>,
}

impl GraphServiceImpl {
    /// Construct with a concrete graph port.
    pub fn new(port: Arc<dyn GraphPort>) -> Self {
        Self { port: Some(port) }
    }

    /// Construct without a backend (all RPCs return UNIMPLEMENTED).
    pub fn without_backend() -> Self {
        Self { port: None }
    }

    /// Convert into a tonic gRPC server.
    pub fn into_server(self) -> GraphServiceServer<Self> {
        GraphServiceServer::new(self)
    }

    fn not_configured() -> Status {
        Status::unimplemented("Graph service not configured on this node")
    }

    fn port_err(e: anyhow::Error) -> Status {
        Status::internal(e.to_string())
    }
}

#[tonic::async_trait]
impl GraphServiceTrait for GraphServiceImpl {
    // ── Node CRUD ─────────────────────────────────────────────────────────

    async fn create_node(
        &self,
        request: Request<CreateNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.create_node(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn get_node(&self, request: Request<GetNodeRequest>) -> Result<Response<Node>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_node(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn update_node(
        &self,
        request: Request<UpdateNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.update_node(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn delete_node(
        &self,
        request: Request<DeleteNodeRequest>,
    ) -> Result<Response<Node>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.delete_node(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Edge CRUD ─────────────────────────────────────────────────────────

    async fn create_edge(
        &self,
        request: Request<CreateEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.create_edge(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn get_edge(&self, request: Request<GetEdgeRequest>) -> Result<Response<Edge>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_edge(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn update_edge(
        &self,
        request: Request<UpdateEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.update_edge(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn delete_edge(
        &self,
        request: Request<DeleteEdgeRequest>,
    ) -> Result<Response<Edge>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.delete_edge(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Queries ───────────────────────────────────────────────────────────

    async fn query_nodes(
        &self,
        request: Request<NodeQuery>,
    ) -> Result<Response<BatchResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.query_nodes(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn query_edges(
        &self,
        request: Request<EdgeQuery>,
    ) -> Result<Response<BatchResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.query_edges(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn execute_query(
        &self,
        request: Request<GraphQueryRequest>,
    ) -> Result<Response<GraphQueryResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.execute_query(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn get_neighbors(
        &self,
        request: Request<GetNeighborsRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_neighbors(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Traversal ─────────────────────────────────────────────────────────

    async fn traverse_graph(
        &self,
        request: Request<TraversalRequest>,
    ) -> Result<Response<TraversalResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.traverse_graph(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    type StreamTraverseStream = StreamTraverseStream;

    async fn stream_traverse(
        &self,
        request: Request<TraversalRequest>,
    ) -> Result<Response<Self::StreamTraverseStream>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        let chunks = port
            .stream_traverse(request.into_inner())
            .await
            .map_err(Self::port_err)?;

        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            for chunk in chunks {
                if tx.send(Ok(chunk)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    // ── Analytics ─────────────────────────────────────────────────────────

    async fn get_graph_stats(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<GraphStats>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_graph_stats(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn shortest_path(
        &self,
        request: Request<ShortestPathRequest>,
    ) -> Result<Response<ShortestPathResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.shortest_path(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn get_connected_components(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<ConnectedComponentsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_connected_components(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn has_cycle(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<CycleCheckResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.has_cycle(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Constraints ───────────────────────────────────────────────────────

    async fn add_unique_constraint(
        &self,
        request: Request<UniqueConstraintRequest>,
    ) -> Result<Response<UniqueConstraintResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.add_unique_constraint(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn remove_unique_constraint(
        &self,
        request: Request<UniqueConstraintRequest>,
    ) -> Result<Response<UniqueConstraintResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.remove_unique_constraint(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Batch operations ──────────────────────────────────────────────────

    async fn batch_create_nodes(
        &self,
        request: Request<BatchNodeRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.batch_create_nodes(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn batch_create_edges(
        &self,
        request: Request<BatchEdgeRequest>,
    ) -> Result<Response<BatchResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.batch_create_edges(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Hybrid query ──────────────────────────────────────────────────────

    async fn execute_hybrid_query(
        &self,
        request: Request<HybridSearchRequest>,
    ) -> Result<Response<HybridSearchResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.execute_hybrid_query(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    fn assert_unimplemented<T>(result: Result<Response<T>, Status>) {
        let err = result.expect_err("backend-less graph service should reject RPC");
        assert_eq!(err.code(), Code::Unimplemented);
        assert!(err.message().contains("Graph service not configured"));
    }

    #[tokio::test]
    async fn backendless_graph_service_rejects_crud_and_query_rpcs() {
        let service = GraphServiceImpl::without_backend();

        assert_unimplemented(
            GraphServiceTrait::create_node(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::get_node(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::update_node(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::delete_node(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::create_edge(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::get_edge(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::update_edge(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::delete_edge(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::query_nodes(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::query_edges(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::execute_query(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::get_neighbors(&service, Request::new(Default::default())).await,
        );
    }

    #[tokio::test]
    async fn backendless_graph_service_rejects_traversal_analytics_and_batch_rpcs() {
        let service = GraphServiceImpl::without_backend();

        assert_unimplemented(
            GraphServiceTrait::traverse_graph(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::stream_traverse(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::get_graph_stats(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::shortest_path(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::get_connected_components(&service, Request::new(Default::default()))
                .await,
        );
        assert_unimplemented(
            GraphServiceTrait::has_cycle(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::add_unique_constraint(&service, Request::new(Default::default()))
                .await,
        );
        assert_unimplemented(
            GraphServiceTrait::remove_unique_constraint(&service, Request::new(Default::default()))
                .await,
        );
        assert_unimplemented(
            GraphServiceTrait::batch_create_nodes(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::batch_create_edges(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            GraphServiceTrait::execute_hybrid_query(&service, Request::new(Default::default()))
                .await,
        );
    }

    #[test]
    fn backendless_graph_service_can_be_wrapped_as_tonic_server() {
        let _server = GraphServiceImpl::without_backend().into_server();
    }
}
