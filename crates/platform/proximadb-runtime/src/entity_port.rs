//! Entity (SKS) composition port trait for `proximadb-runtime`.
//!
//! `EntityPort` is the stable contract that the gRPC `EntityService`
//! in `proximadb-api` uses to call into the entity subsystem without
//! importing root-crate concrete types.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_proto::v1::{
    DeleteEntityRequest, DeleteEntityResponse, GetEntityRequest, GetEntityResponse,
    SearchEntitiesRequest, SearchEntitiesResponse, UpsertEntityRequest, UpsertEntityResponse,
};

/// Port for SKS entity operations (upsert, get, delete, search).
///
/// Implemented by the root-crate `EntityServiceImpl`.  When absent the gRPC
/// adapter returns `UNIMPLEMENTED` for every RPC.
#[async_trait]
pub trait EntityPort: Send + Sync {
    async fn upsert_entity(
        &self,
        request: UpsertEntityRequest,
    ) -> Result<UpsertEntityResponse>;

    async fn get_entity(
        &self,
        request: GetEntityRequest,
    ) -> Result<GetEntityResponse>;

    async fn delete_entity(
        &self,
        request: DeleteEntityRequest,
    ) -> Result<DeleteEntityResponse>;

    async fn search_entities(
        &self,
        request: SearchEntitiesRequest,
    ) -> Result<SearchEntitiesResponse>;
}
