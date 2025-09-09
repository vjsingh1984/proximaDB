use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::proximadb;
use crate::proto::proximadb_v1::vector_service_server::{VectorService, VectorServiceServer};

pub struct VectorServiceImpl {
    unified_handlers: Arc<UnifiedHandlers>,
}

impl VectorServiceImpl {
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self { unified_handlers }
    }

    pub fn into_server(self) -> VectorServiceServer<Self> { VectorServiceServer::new(self) }
}

#[tonic::async_trait]
impl VectorService for VectorServiceImpl {
    async fn collection_operation(
        &self,
        request: Request<proximadb::CollectionRequest>,
    ) -> Result<Response<proximadb::CollectionResponse>, Status> {
        let req = request.into_inner();
        self.unified_handlers
            .handle_collection_operation(req)
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Collection operation failed: {}", e)))
    }

    async fn vector_batch(
        &self,
        request: Request<proximadb::VectorBatchRequest>,
    ) -> Result<Response<proximadb::VectorOperationResponse>, Status> {
        let req = request.into_inner();
        self.unified_handlers
            .handle_vector_batch(req)
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector batch failed: {}", e)))
    }

    async fn vector_search(
        &self,
        request: Request<proximadb::VectorSearchRequest>,
    ) -> Result<Response<proximadb::VectorOperationResponse>, Status> {
        let req = request.into_inner();
        self.unified_handlers
            .handle_vector_search(req)
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector search failed: {}", e)))
    }

    async fn vector_get(
        &self,
        request: Request<proximadb::VectorGetRequest>,
    ) -> Result<Response<proximadb::VectorOperationResponse>, Status> {
        let req = request.into_inner();
        let include_vector = req.include_vector.unwrap_or(false);
        let include_metadata = req.include_metadata.unwrap_or(true);
        self.unified_handlers
            .handle_vector(&req.collection_id, &req.vector_id, include_vector, include_metadata)
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector get failed: {}", e)))
    }
}

