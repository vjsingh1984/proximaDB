//! # Vector Service (gRPC)
//!
//! gRPC implementation for vector CRUD and search operations.
//!
//! ## Status
//!
//! **TEMPORARY PLACEHOLDER**: This module contains placeholder implementations during the
//! workspace refactor. The actual implementations exist in `src/network/grpc/vector_service.rs`.

use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::debug;

// Use runtime UnifiedHandlers
use proximadb_runtime::UnifiedHandlers;

// Placeholder type for query facade adapter
// TODO: Replace with actual type after migration
pub struct QueryFacadeAdapter;

use proximadb_proto::v1;
use proximadb_proto::v1::vector_service_server::{VectorService, VectorServiceServer};

/// gRPC implementation of the VectorService for vector CRUD and search operations
pub struct VectorServiceImpl {
    /// Shared unified handlers for business logic delegation
    _request_handlers: Arc<UnifiedHandlers>,
    /// Optional query facade adapter for unified routing through the query planner
    _query_adapter: Option<Arc<QueryFacadeAdapter>>,
}

/// Streaming response type for VectorSearchStream
pub type VectorSearchStreamStream = Pin<
    Box<dyn tokio_stream::Stream<Item = Result<v1::SearchVectorRecord, Status>> + Send>,
>;

impl VectorServiceImpl {
    /// Create a new vector service backed by unified handlers
    pub fn new(request_handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            _request_handlers: request_handlers,
            _query_adapter: None,
        }
    }

    /// Create a new VectorServiceImpl with optional facade adapter for unified routing
    pub fn with_adapter(
        request_handlers: Arc<UnifiedHandlers>,
        query_adapter: Option<Arc<QueryFacadeAdapter>>,
    ) -> Self {
        Self {
            _request_handlers: request_handlers,
            _query_adapter: query_adapter,
        }
    }

    /// Convert this implementation into a tonic gRPC server
    pub fn into_server(self) -> VectorServiceServer<Self> {
        VectorServiceServer::new(self)
    }

    fn extract_tenant_id<T>(request: &Request<T>) -> Option<String> {
        request
            .metadata()
            .get("x-tenant-id")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|tenant_id| !tenant_id.is_empty())
            .map(ToOwned::to_owned)
    }
}

#[tonic::async_trait]
impl VectorService for VectorServiceImpl {
    async fn vector_batch(
        &self,
        _request: Request<v1::VectorBatchRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        Err(Status::unimplemented("Vector batch: use root crate implementation"))
    }

    async fn vector_search(
        &self,
        _request: Request<v1::VectorSearchRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        Err(Status::unimplemented("Vector search: use root crate implementation"))
    }

    async fn vector_get(
        &self,
        _request: Request<v1::VectorGetRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        Err(Status::unimplemented("Vector get: use root crate implementation"))
    }

    /// Streaming vector search - returns results as a stream for large result sets
    ///
    /// This method performs the same search as `vector_search` but streams
    /// individual results back to the client, which is useful for:
    /// - Large result sets that might exceed message size limits
    /// - Progressive rendering of search results
    /// - Lower latency for first results
    type VectorSearchStreamStream = VectorSearchStreamStream;

    async fn vector_search_stream(
        &self,
        _request: Request<v1::VectorSearchRequest>,
    ) -> Result<Response<Self::VectorSearchStreamStream>, Status> {
        Err(Status::unimplemented("Vector search stream: use root crate implementation"))
    }
}
