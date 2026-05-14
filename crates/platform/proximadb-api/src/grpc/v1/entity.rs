//! # Entity Service (gRPC)
//!
//! gRPC implementation for entity operations in Semantic Knowledge Store (SKS).
//!
//! ## Status
//!
//! **TEMPORARY PLACEHOLDER**: This module contains placeholder implementations during the
//! workspace refactor. The actual implementations exist in `src/network/grpc/entity_service.rs`.

use std::sync::Arc;
use tonic::{Request, Response, Status};

// Placeholder types for entity services
// TODO: Replace with actual types after migration
pub struct ProximaEntityStore;

use proximadb_proto::v1::{
    entity_service_server::{EntityService, EntityServiceServer},
    *
};

/// gRPC implementation of the EntityService
pub struct EntityServiceImpl {
    _store: Arc<ProximaEntityStore>,
}

impl EntityServiceImpl {
    /// Create a new EntityService implementation
    pub fn new(_store: Arc<ProximaEntityStore>) -> Self {
        Self { _store }
    }

    /// Create a tonic service from this implementation
    pub fn into_service(self) -> EntityServiceServer<Self> {
        EntityServiceServer::new(self)
    }
}

// Placeholder trait implementation - will be implemented after migration
#[tonic::async_trait]
impl EntityService for EntityServiceImpl {
    async fn upsert_entity(
        &self,
        _request: Request<UpsertEntityRequest>,
    ) -> Result<Response<UpsertEntityResponse>, Status> {
        Err(Status::unimplemented("Entity service migration in progress"))
    }

    async fn get_entity(
        &self,
        _request: Request<GetEntityRequest>,
    ) -> Result<Response<GetEntityResponse>, Status> {
        Err(Status::unimplemented("Entity service migration in progress"))
    }

    async fn delete_entity(
        &self,
        _request: Request<DeleteEntityRequest>,
    ) -> Result<Response<DeleteEntityResponse>, Status> {
        Err(Status::unimplemented("Entity service migration in progress"))
    }

    async fn search_entities(
        &self,
        _request: Request<SearchEntitiesRequest>,
    ) -> Result<Response<SearchEntitiesResponse>, Status> {
        Err(Status::unimplemented("Entity service migration in progress"))
    }
}
