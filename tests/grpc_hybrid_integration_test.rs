//! T3.2 Slice 4 — gRPC hybrid_search in-process integration test.
//!
//! Proves the gRPC wire round-trips end-to-end through `HybridSearchServiceImpl`
//! using an ephemeral-port tonic server bound to 127.0.0.1:0 and a local
//! tonic client. The `HybridPort` injection is a test-local mock
//! (`CapturingHybridPort`) so the test covers exactly the wire layer:
//! proto encoding/decoding, service dispatch, and error mapping. Business-
//! logic coverage (BM25+vector fusion correctness) is in the per-crate unit
//! tests; this slice fills the missing wire-layer gap.
//!
//! Slice 1 (commit `6a73ead7f`) wired the production gRPC service to
//! `RestHybridPortImpl`. This test confirms the surrounding tonic machinery
//! actually delivers requests to whatever `HybridPort` is injected — which
//! the production deployment depends on.
//!
//! The `start_test_server` harness here is the canonical reusable pattern
//! for any future in-process tonic integration test in this codebase
//! (mirrors what Slice 9 did for REST via `tower::ServiceExt::oneshot`).

use anyhow::Result;
use proximadb_api::grpc::v1::hybrid::HybridSearchServiceImpl;
use proximadb_proto::v1::hybrid_search_service_client::HybridSearchServiceClient;
use proximadb_proto::v1::{
    FusionStrategy, FusionStrategyInfo, FusionStrategyParams, HybridFusionSearchRequest,
    HybridFusionSearchResponse, HybridSearchResult, ListFusionStrategiesRequest,
    ListFusionStrategiesResponse, WeightedLinearParams, fusion_strategy_params,
};
use proximadb_runtime::HybridPort;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Channel, Endpoint, Server};

/// Test-local mock that records inbound requests and returns canned responses.
///
/// Independent of `RestHybridPortImpl` (which needs the heavy `VectorOpsPort`
/// dep) so the test scope stays focused on the wire layer.
#[derive(Default)]
struct CapturingHybridPort {
    inbound: Arc<Mutex<Vec<HybridFusionSearchRequest>>>,
    inbound_strategies: Arc<Mutex<Vec<ListFusionStrategiesRequest>>>,
    canned_search_response: Arc<Mutex<Option<HybridFusionSearchResponse>>>,
    canned_strategies_response: Arc<Mutex<Option<ListFusionStrategiesResponse>>>,
    canned_search_error: Arc<Mutex<Option<String>>>,
}

impl CapturingHybridPort {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn set_search_response(&self, response: HybridFusionSearchResponse) {
        *self.canned_search_response.lock().unwrap() = Some(response);
    }

    fn set_search_error(&self, message: &str) {
        *self.canned_search_error.lock().unwrap() = Some(message.to_string());
    }

    fn set_strategies_response(&self, response: ListFusionStrategiesResponse) {
        *self.canned_strategies_response.lock().unwrap() = Some(response);
    }

    fn inbound_search_requests(&self) -> Vec<HybridFusionSearchRequest> {
        self.inbound.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl HybridPort for CapturingHybridPort {
    async fn hybrid_search(
        &self,
        request: HybridFusionSearchRequest,
    ) -> anyhow::Result<HybridFusionSearchResponse> {
        self.inbound.lock().unwrap().push(request);
        if let Some(msg) = self.canned_search_error.lock().unwrap().clone() {
            anyhow::bail!(msg);
        }
        Ok(self
            .canned_search_response
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default())
    }

    async fn list_fusion_strategies(
        &self,
        request: ListFusionStrategiesRequest,
    ) -> anyhow::Result<ListFusionStrategiesResponse> {
        self.inbound_strategies.lock().unwrap().push(request);
        Ok(self
            .canned_strategies_response
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default())
    }
}

/// Spin up an in-process tonic server bound to an ephemeral 127.0.0.1 port,
/// returning a connected `Channel` ready for client construction. The server
/// runs on a spawned task; dropping the channel and the test scope tears
/// down the server.
async fn start_test_server(port: Arc<dyn HybridPort>) -> Result<(SocketAddr, Channel)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let incoming = TcpListenerStream::new(listener);

    let service = HybridSearchServiceImpl::new(port).into_server();
    let server = Server::builder()
        .add_service(service)
        .serve_with_incoming(incoming);

    tokio::spawn(async move {
        let _ = server.await;
    });

    let channel = Endpoint::from_shared(format!("http://{}", addr))?
        .connect()
        .await?;

    Ok((addr, channel))
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Round-trip a successful response through the gRPC wire.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn grpc_hybrid_search_round_trips_successful_response() -> Result<()> {
    let port = CapturingHybridPort::new();
    port.set_search_response(HybridFusionSearchResponse {
        results_count: 2,
        fusion_strategy: FusionStrategy::WeightedLinear as i32,
        results: vec![
            HybridSearchResult {
                id: "doc-1".to_string(),
                bm25_score: 0.9,
                vector_score: 0.7,
                fused_score: 0.8,
                ..Default::default()
            },
            HybridSearchResult {
                id: "doc-2".to_string(),
                bm25_score: 0.6,
                vector_score: 0.8,
                fused_score: 0.7,
                ..Default::default()
            },
        ],
        ..Default::default()
    });

    let (_addr, channel) = start_test_server(port.clone()).await?;
    let mut client = HybridSearchServiceClient::new(channel);

    let request = HybridFusionSearchRequest {
        collection: "docs".to_string(),
        text_query: "alpha".to_string(),
        top_k: 5,
        ..Default::default()
    };
    let response = client.hybrid_search(request).await?.into_inner();

    assert_eq!(response.results_count, 2);
    assert_eq!(response.results.len(), 2);
    assert_eq!(response.results[0].id, "doc-1");
    assert_eq!(response.results[1].id, "doc-2");

    // Confirm the mock saw exactly one inbound request with the collection set.
    let captured = port.inbound_search_requests();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].collection, "docs");
    assert_eq!(captured[0].text_query, "alpha");
    assert_eq!(captured[0].top_k, 5);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Preserve filter + fusion params through the wire (no proto field drift).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn grpc_hybrid_search_preserves_filter_and_fusion_params() -> Result<()> {
    let port = CapturingHybridPort::new();
    let (_addr, channel) = start_test_server(port.clone()).await?;
    let mut client = HybridSearchServiceClient::new(channel);

    let mut filters = HashMap::new();
    filters.insert(
        "region".to_string(),
        prost_types::Value {
            kind: Some(prost_types::value::Kind::StringValue("us".to_string())),
        },
    );

    let request = HybridFusionSearchRequest {
        collection: "docs".to_string(),
        text_query: "search query".to_string(),
        query_vector: vec![0.1_f32, 0.2, 0.3, 0.4],
        fusion_strategy: FusionStrategy::WeightedLinear as i32,
        fusion_params: Some(FusionStrategyParams {
            params: Some(fusion_strategy_params::Params::WeightedLinear(
                WeightedLinearParams {
                    alpha: 0.25,
                    bm25_normalize: true,
                    vector_normalize: false,
                },
            )),
        }),
        top_k: 7,
        filters,
    };
    client.hybrid_search(request).await?;

    let captured = port.inbound_search_requests();
    assert_eq!(captured.len(), 1);
    let req = &captured[0];

    // Confirm every field round-tripped intact through encode → wire → decode.
    assert_eq!(req.collection, "docs");
    assert_eq!(req.text_query, "search query");
    assert_eq!(req.query_vector, vec![0.1_f32, 0.2, 0.3, 0.4]);
    assert_eq!(req.top_k, 7);
    assert_eq!(req.fusion_strategy, FusionStrategy::WeightedLinear as i32);
    assert!(matches!(
        req.fusion_params.as_ref().and_then(|p| p.params.as_ref()),
        Some(fusion_strategy_params::Params::WeightedLinear(params))
            if (params.alpha - 0.25).abs() < f64::EPSILON
                && params.bm25_normalize
                && !params.vector_normalize
    ));
    assert!(matches!(
        req.filters.get("region").and_then(|v| v.kind.as_ref()),
        Some(prost_types::value::Kind::StringValue(value)) if value == "us"
    ));

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Port error propagates as a tonic Status::internal.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn grpc_hybrid_search_propagates_port_error_as_status() -> Result<()> {
    let port = CapturingHybridPort::new();
    port.set_search_error("backend failure xyz");

    let (_addr, channel) = start_test_server(port.clone()).await?;
    let mut client = HybridSearchServiceClient::new(channel);

    let response = client
        .hybrid_search(HybridFusionSearchRequest {
            collection: "docs".to_string(),
            text_query: "q".to_string(),
            ..Default::default()
        })
        .await;

    let status = match response {
        Ok(_) => panic!("expected the port error to surface as a tonic Status"),
        Err(s) => s,
    };
    assert_eq!(status.code(), tonic::Code::Internal);
    assert!(
        status.message().contains("backend failure xyz"),
        "Status message should preserve the underlying error: got {}",
        status.message()
    );

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. list_fusion_strategies round-trips a canned response.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn grpc_list_fusion_strategies_round_trips() -> Result<()> {
    let port = CapturingHybridPort::new();
    port.set_strategies_response(ListFusionStrategiesResponse {
        strategies: vec![
            FusionStrategyInfo {
                id: "rrf".to_string(),
                name: "Reciprocal Rank Fusion".to_string(),
                description: "score = 1/(k+rank_bm25) + 1/(k+rank_vector)".to_string(),
                default_params: None,
            },
            FusionStrategyInfo {
                id: "weighted_linear".to_string(),
                name: "Weighted Linear".to_string(),
                description: "alpha*bm25 + (1-alpha)*vector".to_string(),
                default_params: None,
            },
        ],
    });

    let (_addr, channel) = start_test_server(port.clone()).await?;
    let mut client = HybridSearchServiceClient::new(channel);

    let response = client
        .list_fusion_strategies(ListFusionStrategiesRequest::default())
        .await?
        .into_inner();

    assert_eq!(response.strategies.len(), 2);
    let ids: Vec<&str> = response.strategies.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"rrf"));
    assert!(ids.contains(&"weighted_linear"));

    Ok(())
}
