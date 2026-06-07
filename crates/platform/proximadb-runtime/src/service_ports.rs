//! Service composition port traits for `proximadb-runtime`.
//!
//! These traits define the stable contracts that `UnifiedHandlers` uses to call into
//! the concrete service layer (collection management, vector operations, query routing).
//! They use only proto types and standard Rust types so `proximadb-runtime` can express
//! the full composition surface without importing any root-crate concrete types.
//!
//! ## Tenant contract
//!
//! Every tenant-sensitive method accepts `tenant_id: Option<&str>`.  The implementing
//! service is responsible for resolving that string to its internal `TenantContext` and
//! enforcing access control.  No tenant context type leaks through the port boundary.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_proto::v1::{
    Collection, CollectionConfig, HybridSearchRequest, HybridSearchResponse, VectorBatchRequest,
    VectorOperationResponse, VectorSearchRequest,
};
use serde_json::Value as JsonValue;

// ── Collection management ──────────────────────────────────────────────────────

/// Port for collection lifecycle operations.
///
/// Implemented by root-crate `CollectionService`.  Each method receives an optional
/// `tenant_id`; the implementation resolves tenant context and enforces isolation
/// internally so no tenant type crosses the port boundary.
#[async_trait]
pub trait CollectionPort: Send + Sync {
    /// Fetch a single collection by name or ID, honouring tenant scope.
    async fn get_collection(
        &self,
        identifier: &str,
        tenant_id: Option<&str>,
    ) -> Result<Option<Collection>>;

    /// Create a collection from a proto config, honouring tenant scope.
    async fn create_collection(
        &self,
        config: CollectionConfig,
        tenant_id: Option<&str>,
    ) -> Result<Collection>;

    /// Update collection configuration, honouring tenant scope.
    async fn update_collection(
        &self,
        id: &str,
        config: CollectionConfig,
        tenant_id: Option<&str>,
    ) -> Result<Collection>;

    /// Delete a collection, honouring tenant scope. Returns true if deleted.
    async fn delete_collection(&self, id: &str, tenant_id: Option<&str>) -> Result<bool>;

    /// List all collections visible to the given tenant (or all if tenant_id is None).
    async fn list_collections(&self, tenant_id: Option<&str>) -> Result<Vec<Collection>>;

    /// Resolve a collection name or UUID to its canonical internal ID.
    async fn resolve_collection_id(&self, identifier: &str) -> Result<Option<String>>;
}

// ── Vector operations ─────────────────────────────────────────────────────────

/// Port for vector CRUD and search operations.
///
/// Implemented by root-crate `VectorOperationsService`.
#[async_trait]
pub trait VectorOpsPort: Send + Sync {
    /// Execute a vector search, tenant-scoped when `tenant_id` is provided.
    async fn search(
        &self,
        request: VectorSearchRequest,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse>;

    /// Execute a batch upsert/delete, tenant-scoped when `tenant_id` is provided.
    async fn batch_upsert(
        &self,
        request: VectorBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse>;

    /// Fetch a single vector by ID.
    async fn get_vector(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse>;

    /// Force-flush all pending WAL entries to the storage engine.
    async fn flush_all(&self) -> Result<()>;

    /// Return engine-level metrics as a JSON blob.
    async fn metrics(&self) -> Result<JsonValue>;
}

// ── Query facade ──────────────────────────────────────────────────────────────

/// Port for unified query routing (SQL, hybrid, vector-via-facade).
///
/// Implemented by root-crate `QueryFacadeAdapter` when the `unified-facade-routing`
/// feature is active.
#[async_trait]
pub trait QueryAdapterPort: Send + Sync {
    /// Route a vector search through the unified query planner.
    async fn vector_search(&self, request: VectorSearchRequest) -> Result<VectorOperationResponse>;

    /// Execute a hybrid (vector + keyword) query.
    async fn execute_hybrid(&self, request: HybridSearchRequest) -> Result<HybridSearchResponse>;

    /// Execute a SQL statement through the unified facade.
    ///
    /// Returns rows as protocol-neutral JSON so the protocol layer can convert
    /// to v1 `ExecuteQueryResponse`, v2 `ProximaValue` rows, or any wire format
    /// without the port accumulating v1 surface debt.
    async fn execute_sql(&self, query: String, collection: Option<String>) -> Result<JsonValue>;
}
