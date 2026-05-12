//! # gRPC API Handlers
//!
//! Protocol Buffers API handlers via Tonic framework.

pub mod v1;

// Re-export v1 services
pub use v1::{
    CollectionService, DocumentService, EntityService, GraphService, GraphTraversalService,
    HybridSearchService, LogsService, MetricsService, SecurityService, StreamingService,
    VectorService,
};

/// gRPC API request context
#[derive(Debug, Clone)]
pub struct GrpcRequest {
    pub method: String,
    pub metadata: Vec<(String, String)>,
}

/// gRPC API response wrapper
pub struct GrpcResponse<T> {
    pub inner: T,
}

/// gRPC API handler
pub struct GrpcApiHandler {
    // Service dependencies will be added here
}

impl GrpcApiHandler {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for GrpcApiHandler {
    fn default() -> Self {
        Self::new()
    }
}

// TODO: Move gRPC handlers from src/network/grpc
