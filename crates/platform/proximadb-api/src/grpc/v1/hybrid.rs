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
        super::deprecated_status(Status::unimplemented(
            "Hybrid search service not configured on this node",
        ))
    }

    fn port_err(e: anyhow::Error) -> Status {
        super::deprecated_status(Status::internal(e.to_string()))
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
            .map(super::deprecated_response)
            .map_err(Self::port_err)
    }

    async fn list_fusion_strategies(
        &self,
        request: Request<ListFusionStrategiesRequest>,
    ) -> Result<Response<ListFusionStrategiesResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.list_fusion_strategies(request.into_inner())
            .await
            .map(super::deprecated_response)
            .map_err(Self::port_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    fn assert_unimplemented<T>(result: Result<Response<T>, Status>) {
        let err = match result {
            Ok(_) => panic!("backend-less hybrid service should reject RPC"),
            Err(err) => err,
        };
        assert_eq!(err.code(), Code::Unimplemented);
        assert!(
            err.message()
                .contains("Hybrid search service not configured")
        );
    }

    #[tokio::test]
    async fn backendless_hybrid_service_rejects_every_rpc_consistently() {
        let service = HybridSearchServiceImpl::without_backend();

        assert_unimplemented(
            HybridSearchService::hybrid_search(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            HybridSearchService::list_fusion_strategies(&service, Request::new(Default::default()))
                .await,
        );
    }

    #[test]
    fn backendless_hybrid_service_can_be_wrapped_as_tonic_server() {
        let _server = HybridSearchServiceImpl::without_backend().into_server();
    }
}
