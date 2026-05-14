//! Hybrid search composition port trait for `proximadb-runtime`.
//!
//! `HybridPort` is the stable contract that the gRPC `HybridSearchService`
//! in `proximadb-api` uses to call into the hybrid search subsystem without
//! importing root-crate concrete types.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_proto::v1::{
    HybridFusionSearchRequest, HybridFusionSearchResponse, ListFusionStrategiesRequest,
    ListFusionStrategiesResponse,
};

/// Port for hybrid (BM25 + vector) search operations.
///
/// Implemented by the root-crate `HybridSearchServiceImpl`.  When absent the
/// gRPC adapter returns `UNIMPLEMENTED` for every RPC.
#[async_trait]
pub trait HybridPort: Send + Sync {
    async fn hybrid_search(
        &self,
        request: HybridFusionSearchRequest,
    ) -> Result<HybridFusionSearchResponse>;

    async fn list_fusion_strategies(
        &self,
        request: ListFusionStrategiesRequest,
    ) -> Result<ListFusionStrategiesResponse>;
}
