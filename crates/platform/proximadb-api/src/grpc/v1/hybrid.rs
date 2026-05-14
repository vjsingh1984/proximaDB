//! # Hybrid Search Service (gRPC)
//!
//! gRPC implementation for hybrid BM25 + vector search fusion.
//!
//! ## Status
//!
//! **TEMPORARY PLACEHOLDER**: This module contains placeholder implementations during the
//! workspace refactor. The actual implementations exist in `src/network/grpc/hybrid_search_service.rs`.

use tonic::{Request, Response, Status};

// Placeholder types for hybrid search
// TODO: Replace with actual types after migration
pub struct FusionStrategy;

use proximadb_proto::v1;
use proximadb_proto::v1::hybrid_search_service_server::{
    HybridSearchService, HybridSearchServiceServer,
};

/// gRPC service implementation for Hybrid Search
pub struct HybridSearchServiceImpl;

impl HybridSearchServiceImpl {
    /// Create a new hybrid search service
    pub fn new() -> Self {
        Self {}
    }

    /// Convert the service into a tonic server
    pub fn into_server(self) -> HybridSearchServiceServer<Self> {
        HybridSearchServiceServer::new(self)
    }
}

impl Default for HybridSearchServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

// Placeholder trait implementation - will be implemented after migration
#[tonic::async_trait]
impl HybridSearchService for HybridSearchServiceImpl {
    async fn hybrid_search(
        &self,
        _request: Request<v1::HybridFusionSearchRequest>,
    ) -> Result<Response<v1::HybridFusionSearchResponse>, Status> {
        Err(Status::unimplemented("Hybrid search service migration in progress"))
    }

    async fn list_fusion_strategies(
        &self,
        _request: Request<v1::ListFusionStrategiesRequest>,
    ) -> Result<Response<v1::ListFusionStrategiesResponse>, Status> {
        Err(Status::unimplemented("Hybrid search service migration in progress"))
    }
}
