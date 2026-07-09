//! # gRPC API Handlers
//!
//! Protocol Buffers API handlers via Tonic framework.
//!
//! TD-V1SUNSET-1: the legacy v1 service set (`grpc::v1`) and its
//! `GrpcServiceFactory`/`GrpcServiceBuilder` were removed once the v1 gRPC
//! surface had been default-off through its sunset window. The canonical v2
//! services live in `grpc::v2`.

pub mod v2;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_request_carries_method_and_metadata() {
        let request = GrpcRequest {
            method: "proximadb.v1.Vector/Search".to_string(),
            metadata: vec![("tenant-id".to_string(), "tenant-a".to_string())],
        };

        assert_eq!(request.method, "proximadb.v1.Vector/Search");
        assert_eq!(
            request.metadata,
            vec![("tenant-id".to_string(), "tenant-a".to_string())]
        );
    }

    #[test]
    fn grpc_response_wraps_inner_payload() {
        let response = GrpcResponse { inner: 42 };

        assert_eq!(response.inner, 42);
    }

    #[test]
    fn grpc_handler_default_matches_new() {
        let _from_new = GrpcApiHandler::new();
        let _from_default = GrpcApiHandler::default();
    }
}
