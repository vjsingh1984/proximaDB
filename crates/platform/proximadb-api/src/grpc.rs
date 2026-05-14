//! # gRPC API Handlers
//!
//! Protocol Buffers API handlers via Tonic framework.

pub mod builder;
pub mod v1;
pub mod v2;

// Re-export v1 services
pub use v1::{
    CollectionServiceImpl, DocumentServiceImpl, EntityServiceImpl, GraphServiceImpl,
    HybridSearchServiceImpl, ObservabilityServiceImpl, SecurityServiceImpl, SqlServiceImpl,
    StreamingServiceImpl, VectorServiceImpl,
};

// Re-export builder types
pub use builder::{GrpcServiceBuilder, GrpcServiceConfig, GrpcServiceFactory, GrpcServices};

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
