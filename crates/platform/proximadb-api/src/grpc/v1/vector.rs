//! # Vector Service (gRPC)
//!
//! gRPC implementation for vector CRUD and search operations.
//! Routes through `ApiHandlersPort` — the seam between this protocol adapter and
//! the business logic in `proximadb-runtime`.

use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::vector_service_server::{VectorService, VectorServiceServer};
use proximadb_proto::v1::{self as v1};
use proximadb_runtime::ApiHandlersPort;

/// gRPC implementation of the VectorService for vector CRUD and search operations.
pub struct VectorServiceImpl {
    port: Arc<dyn ApiHandlersPort>,
}

/// Streaming response type for VectorSearchStream.
pub type VectorSearchStreamStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<v1::SearchVectorRecord, Status>> + Send>>;

impl VectorServiceImpl {
    /// Create a new vector service backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self { port }
    }

    /// Convert this implementation into a tonic gRPC server.
    pub fn into_server(self) -> VectorServiceServer<Self> {
        VectorServiceServer::new(self)
    }

    fn extract_tenant_id<T>(request: &Request<T>) -> Option<String> {
        request
            .metadata()
            .get("x-tenant-id")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(ToOwned::to_owned)
    }
}

#[tonic::async_trait]
impl VectorService for VectorServiceImpl {
    async fn vector_batch(
        &self,
        request: Request<v1::VectorBatchRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let req = request.into_inner();
        self.port
            .handle_vector_batch_v1_for_tenant(req, tenant_id.as_deref())
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector batch failed: {e}")))
    }

    async fn vector_search(
        &self,
        request: Request<v1::VectorSearchRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let req = request.into_inner();
        if tenant_id.is_some() {
            self.port
                .handle_vector_search_v1_for_tenant(req, tenant_id.as_deref())
                .await
                .map(Response::new)
                .map_err(|e| Status::internal(format!("Vector search failed: {e}")))
        } else {
            self.port
                .handle_vector_search_v1(req)
                .await
                .map(Response::new)
                .map_err(|e| Status::internal(format!("Vector search failed: {e}")))
        }
    }

    async fn vector_get(
        &self,
        request: Request<v1::VectorGetRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let req = request.into_inner();
        self.port
            .handle_vector_v1_for_tenant(
                &req.collection_id,
                &req.vector_id,
                req.include_vector.unwrap_or(false),
                req.include_metadata.unwrap_or(true),
                tenant_id.as_deref(),
            )
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector get failed: {e}")))
    }

    type VectorSearchStreamStream = VectorSearchStreamStream;

    async fn vector_search_stream(
        &self,
        _request: Request<v1::VectorSearchRequest>,
    ) -> Result<Response<Self::VectorSearchStreamStream>, Status> {
        Err(Status::unimplemented(
            "Streaming vector search is not yet available via this endpoint",
        ))
    }
}
