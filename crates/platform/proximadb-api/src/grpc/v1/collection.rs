//! # Collection Service (gRPC)
//!
//! gRPC implementation for collection management operations.
//!
//! ## Status
//!
//! **TEMPORARY PLACEHOLDER**: This module contains placeholder implementations during the
//! workspace refactor to avoid circular dependencies. The actual implementations exist in
//! `src/network/grpc/collection_service.rs` and will be migrated here after UnifiedHandlers
//! moves to `crates/platform/proximadb-runtime`.

use tonic::{Request, Response, Status};

// Use UnifiedHandlers from runtime crate instead of root crate
use proximadb_runtime::UnifiedHandlers;

/// gRPC implementation of the CollectionService for managing vector collections
///
/// **TEMPORARY PLACEHOLDER**: This is a placeholder implementation during the workspace refactor.
/// The actual implementation is in `src/network/grpc/collection_service.rs`.
pub struct CollectionServiceImpl {
    /// Shared unified handlers for business logic delegation
    _request_handlers: std::sync::Arc<UnifiedHandlers>,
}

impl CollectionServiceImpl {
    /// Create a new collection service backed by unified handlers
    ///
    /// **TEMPORARY PLACEHOLDER**: This constructor will be updated after UnifiedHandlers migration.
    pub fn new(_request_handlers: std::sync::Arc<UnifiedHandlers>) -> Self {
        Self { _request_handlers }
    }

    /// Convert this implementation into a tonic gRPC server
    ///
    /// **TEMPORARY PLACEHOLDER**: This method will be updated after migration.
    pub fn into_server(self) -> proximadb_proto::v1::collection_service_server::CollectionServiceServer<Self> {
        proximadb_proto::v1::collection_service_server::CollectionServiceServer::new(self)
    }
}

// Placeholder trait implementation - will be implemented after migration
#[tonic::async_trait]
impl proximadb_proto::v1::collection_service_server::CollectionService for CollectionServiceImpl {
    async fn create_collection(
        &self,
        _request: Request<proximadb_proto::v1::CollectionConfig>,
    ) -> Result<Response<proximadb_proto::v1::Collection>, Status> {
        Err(Status::unimplemented("Collection service migration in progress"))
    }

    async fn get_collection(
        &self,
        _request: Request<proximadb_proto::v1::GetCollectionRequest>,
    ) -> Result<Response<proximadb_proto::v1::Collection>, Status> {
        Err(Status::unimplemented("Collection service migration in progress"))
    }

    async fn list_collections(
        &self,
        _request: Request<proximadb_proto::v1::ListCollectionsRequest>,
    ) -> Result<Response<proximadb_proto::v1::ListCollectionsResponse>, Status> {
        Err(Status::unimplemented("Collection service migration in progress"))
    }

    async fn delete_collection(
        &self,
        _request: Request<proximadb_proto::v1::DeleteCollectionRequest>,
    ) -> Result<Response<proximadb_proto::v1::DeleteCollectionResponse>, Status> {
        Err(Status::unimplemented("Collection service migration in progress"))
    }
}
