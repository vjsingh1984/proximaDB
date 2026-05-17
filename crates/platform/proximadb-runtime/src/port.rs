//! API handlers port — the seam between protocol adapters and business logic.
//!
//! Protocol handlers in `proximadb-api` depend on this trait rather than on root-crate
//! concrete types. The root crate implements the trait on its `UnifiedHandlers` and
//! injects the `Arc<dyn ApiHandlersPort>` at server startup.
//!
//! Request/response envelopes that are still explicitly v1-compatible use proto
//! types, but value parameters crossing this runtime seam use the canonical
//! `ProximaValue` model. Protocol adapters are responsible for converting
//! legacy wire values at the edge.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_data_model::ProximaValue;
use proximadb_proto::v1::{
    CollectionRequest, CollectionResponse, ExecuteSqlResponse, HybridSearchRequest,
    HybridSearchResponse, VectorBatchRequest, VectorOperationResponse, VectorSearchRequest,
};

/// Port trait that protocol adapters use to dispatch API requests.
///
/// The root crate's `UnifiedHandlers` implements this trait. `proximadb-api` gRPC stubs
/// hold an `Arc<dyn ApiHandlersPort>` so the real implementation is swapped in without
/// `proximadb-api` importing any root-crate concrete types.
#[async_trait]
pub trait ApiHandlersPort: Send + Sync {
    // ── Collection ────────────────────────────────────────────────────────────

    async fn handle_collection_operation_for_tenant(
        &self,
        request: CollectionRequest,
        tenant_id: Option<&str>,
    ) -> Result<CollectionResponse>;

    // ── Vector ────────────────────────────────────────────────────────────────

    async fn handle_vector_search_v1_for_tenant(
        &self,
        request: VectorSearchRequest,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse>;

    async fn handle_vector_search_v1(
        &self,
        request: VectorSearchRequest,
    ) -> Result<VectorOperationResponse>;

    async fn handle_vector_batch_v1_for_tenant(
        &self,
        request: VectorBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse>;

    async fn handle_vector_v1_for_tenant(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse>;

    // ── Hybrid ────────────────────────────────────────────────────────────────

    async fn execute_hybrid_query(
        &self,
        request: HybridSearchRequest,
    ) -> Result<HybridSearchResponse>;

    // ── SQL ───────────────────────────────────────────────────────────────────

    async fn execute_sql_v1(
        &self,
        query: String,
        parameters: Option<Vec<ProximaValue>>,
        collection: Option<String>,
    ) -> Result<ExecuteSqlResponse>;
}
