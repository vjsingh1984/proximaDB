//! # ProximaDB API Handlers
//!
//! This crate contains API request/response handling for ProximaDB.
//!
//! ## Protocol Support
//!
//! - **REST** - HTTP/JSON API via Axum
//! - **gRPC** - Protocol Buffers API via Tonic
//! - **PostgreSQL Wire** - pgvector-compatible protocol
//! - **Arrow Flight** - Columnar data exchange
//!
//! ## Architecture
//!
//! The API layer provides protocol adapters that:
//! - Validate and map incoming requests
//! - Route to appropriate service calls
//! - Shape responses for protocol compatibility
//! - Handle authentication and middleware
//!
//! ## Dependencies
//!
//! - `proximadb-query` - Query execution contracts
//! - `proximadb-proto` - Protocol buffer types
//! - `proximadb-kernel` - Core error types

pub mod grpc;
pub mod middleware;
pub mod rest;

// TODO: Move these from src/network and src/api_handlers
// pub mod pgwire;
// pub mod arrow_flight;

// Re-export common types
pub use grpc::{GrpcApiHandler, GrpcRequest, GrpcResponse};
pub use rest::{RestApiHandler, RestRequest, RestResponse};

// Re-export v1 handlers
pub use grpc::v1::{
    CollectionService, DocumentService, EntityService, GraphService, GraphTraversalService,
    HybridSearchService, LogsService, MetricsService, SecurityService, StreamingService,
    VectorService,
};
pub use rest::v1::{
    AnalyticsHandler, AqlHandler, CatalogHandler, CollectionHandler, DocumentHandler,
    DocumentQueryHandler, EntityHandler, GraphHandler, GraphTraversalHandler, HybridSearchHandler,
    LogsHandler, MetricsHandler, ProgressiveSearchHandler, VectorHandler,
};

// Re-export v2 agentic contracts
pub use grpc::v2::{
    AgentCheckpointService, AgentEventService, AgentMemoryService, CheckpointServiceRequest,
    CheckpointServiceResponse, EventAppendServiceRequest, EventAppendServiceResponse,
    MemoryPutServiceRequest, MemorySearchServiceRequest, MemorySearchServiceResponse,
};
pub use rest::v2::{
    AgentCheckpointRequest, AgentCheckpointResponse, AgentEventAppendRequest,
    AgentEventAppendResponse, AgentEventRecord, AgentEventReplayRequest, AgentMemoryItem,
    AgentMemoryPutRequest, AgentMemorySearchRequest, AgentMemorySearchResponse,
};

// Re-export middleware
pub use middleware::{
    AuthMiddleware, CorsMiddleware, RateLimitMiddleware, RequestIdMiddleware, auth::AuthConfig,
    cors::CorsConfig, rate_limit::RateLimitConfig,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_module_imports() {
        // Basic test to verify the module structure is working
        // More comprehensive tests will be added as modules are extracted
    }
}
