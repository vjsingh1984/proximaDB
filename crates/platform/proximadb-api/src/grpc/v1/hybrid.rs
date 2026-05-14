//! # Hybrid Search Service (gRPC)
//!
//! gRPC implementation for BM25 + vector hybrid search fusion.  Each RPC
//! delegates to the injected `HybridPort`; when no port is provided the
//! service returns `UNIMPLEMENTED`.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use proximadb_proto::v1::{
    hybrid_search_service_server::{HybridSearchService, HybridSearchServiceServer},
    *,
};
use proximadb_runtime::HybridPort;

/// gRPC HybridSearchService backed by a `HybridPort`.
pub struct HybridSearchServiceImpl {
    port: Option<Arc<dyn HybridPort>>,
}

impl HybridSearchServiceImpl {
    /// Construct with a concrete hybrid search port.
    pub fn new(port: Arc<dyn HybridPort>) -> Self {
        Self { port: Some(port) }
    }

    /// Construct without a backend (all RPCs return UNIMPLEMENTED).
    pub fn without_backend() -> Self {
        Self { port: None }
    }

    /// Convert into a tonic gRPC server.
    pub fn into_server(self) -> HybridSearchServiceServer<Self> {
        HybridSearchServiceServer::new(self)
    }

    fn not_configured() -> Status {
        Status::unimplemented("Hybrid search service not configured on this node")
    }

    fn port_err(e: anyhow::Error) -> Status {
        Status::internal(e.to_string())
    }
}

#[tonic::async_trait]
impl HybridSearchService for HybridSearchServiceImpl {
    async fn hybrid_search(
        &self,
        request: Request<HybridFusionSearchRequest>,
    ) -> Result<Response<HybridFusionSearchResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.hybrid_search(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn list_fusion_strategies(
        &self,
        request: Request<ListFusionStrategiesRequest>,
    ) -> Result<Response<ListFusionStrategiesResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.list_fusion_strategies(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }
}
