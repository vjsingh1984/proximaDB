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
    /// DEPRECATED (TD-138): the v1 hybrid query — the legacy 12-strategy
    /// `HybridFusionEngine` (vector+BM25, `doc_id`-keyed, no graph). Use the v2
    /// fusion seam (`FusionService`) instead: `POST /api/v2/graphs/{graph_id}/fusion-search`
    /// or gRPC `ProximaFusionService.FusionSearch`. Removal + caller migration is
    /// phased (TD-143 did this for the gRPC v1 surface). Note-only (no `since`):
    /// clippy `deprecated_semver` denies a non-semver `since`.
    ///
    /// `tenant_id` is the request tenant identity extracted at the network
    /// boundary (`X-Tenant-ID` middleware). Implementations thread it to the
    /// metadata-filter gate so the allowed id-set resolves under the caller's
    /// tenant instead of failing closed to empty (#949). `None` preserves the
    /// legacy tenantless behavior.
    #[deprecated(
        note = "v1 hybrid_search (legacy 12-strategy HybridFusionEngine) is deprecated; use the v2 fusion seam (FusionService). See TD-138."
    )]
    async fn hybrid_search(
        &self,
        request: HybridFusionSearchRequest,
        tenant_id: Option<String>,
    ) -> Result<HybridFusionSearchResponse>;

    async fn list_fusion_strategies(
        &self,
        request: ListFusionStrategiesRequest,
    ) -> Result<ListFusionStrategiesResponse>;
}
