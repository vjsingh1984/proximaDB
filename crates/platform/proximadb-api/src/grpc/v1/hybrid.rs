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
    use std::sync::Mutex;
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

    #[derive(Default)]
    struct RecordingHybridPort {
        last_request: Mutex<Option<HybridFusionSearchRequest>>,
    }

    #[async_trait::async_trait]
    impl HybridPort for RecordingHybridPort {
        async fn hybrid_search(
            &self,
            request: HybridFusionSearchRequest,
        ) -> anyhow::Result<HybridFusionSearchResponse> {
            *self.last_request.lock().unwrap() = Some(request);
            Ok(HybridFusionSearchResponse {
                results_count: 1,
                fusion_strategy: FusionStrategy::WeightedLinear as i32,
                ..Default::default()
            })
        }

        async fn list_fusion_strategies(
            &self,
            _request: ListFusionStrategiesRequest,
        ) -> anyhow::Result<ListFusionStrategiesResponse> {
            Ok(Default::default())
        }
    }

    #[tokio::test]
    async fn grpc_hybrid_service_forwards_filters_and_fusion_to_port() {
        use proximadb_proto::v1::{
            WeightedLinearParams, fusion_strategy_params,
            hybrid_search_service_server::HybridSearchService,
        };

        let port = Arc::new(RecordingHybridPort::default());
        let service = HybridSearchServiceImpl::new(port.clone());
        let mut filters = std::collections::HashMap::new();
        filters.insert(
            "region".to_string(),
            prost_types::Value {
                kind: Some(prost_types::value::Kind::StringValue("us".to_string())),
            },
        );

        let response = HybridSearchService::hybrid_search(
            &service,
            Request::new(HybridFusionSearchRequest {
                collection: "docs".to_string(),
                text_query: "alpha".to_string(),
                query_vector: vec![0.1, 0.2],
                fusion_strategy: FusionStrategy::WeightedLinear as i32,
                fusion_params: Some(FusionStrategyParams {
                    params: Some(fusion_strategy_params::Params::WeightedLinear(
                        WeightedLinearParams {
                            alpha: 0.25,
                            bm25_normalize: true,
                            vector_normalize: true,
                        },
                    )),
                }),
                top_k: 5,
                filters,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.results_count, 1);
        let captured = port
            .last_request
            .lock()
            .unwrap()
            .clone()
            .expect("gRPC service should call port");
        assert_eq!(captured.collection, "docs");
        assert_eq!(captured.text_query, "alpha");
        assert_eq!(captured.query_vector, vec![0.1, 0.2]);
        assert_eq!(captured.top_k, 5);
        assert_eq!(
            captured.fusion_strategy,
            FusionStrategy::WeightedLinear as i32
        );
        assert!(matches!(
            captured.fusion_params.and_then(|params| params.params),
            Some(fusion_strategy_params::Params::WeightedLinear(params))
                if (params.alpha - 0.25).abs() < f64::EPSILON
                    && params.bm25_normalize
                    && params.vector_normalize
        ));
        assert!(matches!(
            captured
                .filters
                .get("region")
                .and_then(|value| value.kind.as_ref()),
            Some(prost_types::value::Kind::StringValue(value)) if value == "us"
        ));
    }
}
