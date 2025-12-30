// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC protocol handler wrapping Tonic services
//!
//! This handler wraps gRPC services built with tonic to enable protocol
//! multiplexing on a single port.

use crate::network::multiplex::traits::{BoxResponseFuture, DetectedProtocol, ProtocolHandler};
use axum::body::Body;
use hyper::http::{Request, Response, StatusCode};
use std::sync::Arc;
use tracing::{trace, warn};

/// gRPC protocol handler
///
/// Wraps Tonic gRPC services to handle gRPC requests through the multiplexer.
///
/// # Usage
///
/// ```ignore
/// use tonic::transport::Server;
/// use crate::network::multiplex::handlers::GrpcHandler;
///
/// // Build your tonic services
/// let grpc_router = Server::builder()
///     .add_service(my_service)
///     .into_service();
///
/// // Wrap in handler for multiplexing
/// let handler = GrpcHandler::new(grpc_router);
/// ```
pub struct GrpcHandler {
    /// Flag indicating if the handler is ready
    ready: bool,
    /// Optional wrapped router (placeholder for future implementation)
    _router: Option<Arc<()>>,
}

impl GrpcHandler {
    /// Create a new gRPC handler
    ///
    /// Note: In a full implementation, this would accept a tonic Routes or similar.
    /// Currently returns a placeholder that returns 501 Not Implemented.
    pub fn new() -> Self {
        Self {
            ready: false,
            _router: None,
        }
    }

    /// Create a gRPC handler marked as ready (for testing)
    pub fn ready() -> Self {
        Self {
            ready: true,
            _router: None,
        }
    }
}

impl Default for GrpcHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for GrpcHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcHandler")
            .field("ready", &self.ready)
            .finish()
    }
}

impl Clone for GrpcHandler {
    fn clone(&self) -> Self {
        Self {
            ready: self.ready,
            _router: self._router.clone(),
        }
    }
}

impl ProtocolHandler for GrpcHandler {
    fn protocol(&self) -> DetectedProtocol {
        DetectedProtocol::Grpc
    }

    fn handle(&self, request: Request<Body>) -> BoxResponseFuture {
        let ready = self.ready;

        Box::pin(async move {
            trace!(
                method = %request.method(),
                uri = %request.uri(),
                "Handling gRPC request"
            );

            if !ready {
                warn!("gRPC handler not configured - returning 501");
                return Response::builder()
                    .status(StatusCode::NOT_IMPLEMENTED)
                    .header("content-type", "application/grpc")
                    .header("grpc-status", "12") // UNIMPLEMENTED
                    .header("grpc-message", "gRPC handler not configured for unified port mode")
                    .body(Body::empty())
                    .expect("response builder should not fail");
            }

            // Placeholder - in a full implementation, this would route to tonic
            Response::builder()
                .status(StatusCode::NOT_IMPLEMENTED)
                .header("content-type", "application/grpc")
                .header("grpc-status", "12")
                .header("grpc-message", "gRPC multiplexing not yet implemented")
                .body(Body::empty())
                .expect("response builder should not fail")
        })
    }

    fn name(&self) -> &str {
        "grpc"
    }

    fn is_ready(&self) -> bool {
        self.ready
    }
}

/// Builder for creating gRPC handlers
pub struct GrpcHandlerBuilder {
    ready: bool,
}

impl GrpcHandlerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self { ready: false }
    }

    /// Mark the handler as ready
    pub fn ready(mut self) -> Self {
        self.ready = true;
        self
    }

    /// Build the gRPC handler
    pub fn build(self) -> GrpcHandler {
        GrpcHandler {
            ready: self.ready,
            _router: None,
        }
    }
}

impl Default for GrpcHandlerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grpc_handler_creation() {
        let handler = GrpcHandler::new();
        assert!(!handler.is_ready());
        assert_eq!(handler.protocol(), DetectedProtocol::Grpc);
        assert_eq!(handler.name(), "grpc");
    }

    #[test]
    fn test_grpc_handler_ready() {
        let handler = GrpcHandler::ready();
        assert!(handler.is_ready());
    }

    #[test]
    fn test_grpc_handler_builder() {
        let handler = GrpcHandlerBuilder::new()
            .ready()
            .build();
        assert!(handler.is_ready());
    }
}
