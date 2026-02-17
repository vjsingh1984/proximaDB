// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC protocol handler for unified port mode
//!
//! Note: Due to http crate version incompatibilities between hyper 0.14 (http 0.2)
//! and tonic 0.14 (http 1.x), gRPC multiplexing is not yet supported.
//! gRPC clients should connect to the dedicated gRPC port (default 5679).

use crate::network::multiplex::traits::{BoxResponseFuture, DetectedProtocol, ProtocolHandler};
use axum::body::Body;
use hyper::http::{Request, Response, StatusCode};
use std::sync::Arc;
use tracing::{trace, warn};

use crate::api_handlers::UnifiedHandlers;

/// Configuration for the gRPC handler
pub struct GrpcHandlerConfig {
    pub unified_handlers: Arc<UnifiedHandlers>,
    pub compression_enabled: bool,
}

/// gRPC protocol handler
///
/// Currently returns a redirect message since gRPC multiplexing requires
/// http crate version alignment between hyper and tonic.
pub struct GrpcHandler {
    /// Flag indicating if the handler is ready
    ready: bool,
    /// The gRPC port to redirect to
    grpc_port: u16,
}

impl GrpcHandler {
    /// Create a new gRPC handler (not ready)
    pub fn new() -> Self {
        Self {
            ready: false,
            grpc_port: 5679,
        }
    }

    /// Create a gRPC handler marked as ready
    pub fn ready() -> Self {
        Self {
            ready: true,
            grpc_port: 5679,
        }
    }

    /// Create a gRPC handler with configuration
    ///
    /// Note: The unified_handlers are not currently used because gRPC
    /// multiplexing requires http crate version alignment.
    pub fn with_config(config: GrpcHandlerConfig) -> Self {
        // Log that we received the config but can't use it yet
        tracing::debug!(
            "GrpcHandler created with config (compression={}), but gRPC multiplexing not yet supported",
            config.compression_enabled
        );

        Self {
            ready: true,
            grpc_port: 5679,
        }
    }

    /// Set the gRPC port for redirect messages
    pub fn with_grpc_port(mut self, port: u16) -> Self {
        self.grpc_port = port;
        self
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
            .field("grpc_port", &self.grpc_port)
            .finish()
    }
}

impl Clone for GrpcHandler {
    fn clone(&self) -> Self {
        Self {
            ready: self.ready,
            grpc_port: self.grpc_port,
        }
    }
}

impl ProtocolHandler for GrpcHandler {
    fn protocol(&self) -> DetectedProtocol {
        DetectedProtocol::Grpc
    }

    fn handle(&self, request: Request<Body>) -> BoxResponseFuture {
        let grpc_port = self.grpc_port;

        Box::pin(async move {
            trace!(
                method = %request.method(),
                uri = %request.uri(),
                "gRPC request received on unified port"
            );

            warn!(
                "gRPC multiplexing not yet supported. Please connect to dedicated gRPC port {}",
                grpc_port
            );

            // Return a proper gRPC error response
            // grpc-status 12 = UNIMPLEMENTED
            Response::builder()
                .status(StatusCode::OK) // gRPC uses 200 OK with grpc-status header
                .header("content-type", "application/grpc")
                .header("grpc-status", "12")
                .header(
                    "grpc-message",
                    format!(
                        "gRPC multiplexing not yet supported on unified port. Please connect to port {}",
                        grpc_port
                    ),
                )
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
    _ready: bool,
    _grpc_port: u16,
}

impl GrpcHandlerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            _ready: false,
            _grpc_port: 5679,
        }
    }

    /// Mark the handler as ready
    #[allow(dead_code)]
    pub fn ready(mut self) -> Self {
        self._ready = true;
        self
    }

    /// Set the gRPC port
    #[allow(dead_code)]
    pub fn grpc_port(mut self, port: u16) -> Self {
        self._grpc_port = port;
        self
    }

    /// Build the gRPC handler
    #[allow(dead_code)]
    pub fn build(self) -> GrpcHandler {
        GrpcHandler {
            ready: self._ready,
            grpc_port: self._grpc_port,
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
        let handler = GrpcHandlerBuilder::new().ready().grpc_port(5680).build();
        assert!(handler.is_ready());
        assert_eq!(handler.grpc_port, 5680);
    }
}
