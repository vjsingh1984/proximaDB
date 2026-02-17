// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Arrow Flight protocol handler wrapping the Flight service
//!
//! This handler wraps Arrow Flight services to enable protocol
//! multiplexing on a single port.

use crate::network::multiplex::traits::{BoxResponseFuture, DetectedProtocol, ProtocolHandler};
use axum::body::Body;
use hyper::http::{Request, Response, StatusCode};
use std::sync::Arc;
use tracing::{trace, warn};

/// Arrow Flight protocol handler
///
/// Wraps an Arrow Flight service to handle Flight requests through the multiplexer.
/// Arrow Flight uses gRPC as its transport, so this is similar to the gRPC handler
/// but specialized for Flight service paths.
///
/// # Usage
///
/// ```ignore
/// use arrow_flight::flight_service_server::FlightServiceServer;
/// use crate::network::multiplex::handlers::ArrowFlightHandler;
///
/// let flight_service = FlightServiceServer::new(my_flight_impl);
/// let handler = ArrowFlightHandler::new(flight_service);
/// ```
pub struct ArrowFlightHandler {
    /// Flag indicating if the handler is ready
    ready: bool,
    /// Optional wrapped router (placeholder for future implementation)
    _router: Option<Arc<()>>,
}

impl ArrowFlightHandler {
    /// Create a new Arrow Flight handler
    ///
    /// Note: In a full implementation, this would accept an Arrow Flight service.
    /// Currently returns a placeholder that returns 501 Not Implemented.
    pub fn new() -> Self {
        Self {
            ready: false,
            _router: None,
        }
    }

    /// Create an Arrow Flight handler marked as ready (for testing)
    pub fn ready() -> Self {
        Self {
            ready: true,
            _router: None,
        }
    }
}

impl Default for ArrowFlightHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ArrowFlightHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArrowFlightHandler")
            .field("ready", &self.ready)
            .finish()
    }
}

impl Clone for ArrowFlightHandler {
    fn clone(&self) -> Self {
        Self {
            ready: self.ready,
            _router: self._router.clone(),
        }
    }
}

impl ProtocolHandler for ArrowFlightHandler {
    fn protocol(&self) -> DetectedProtocol {
        DetectedProtocol::ArrowFlight
    }

    fn handle(&self, request: Request<Body>) -> BoxResponseFuture {
        let ready = self.ready;

        Box::pin(async move {
            trace!(
                method = %request.method(),
                uri = %request.uri(),
                "Handling Arrow Flight request"
            );

            if !ready {
                warn!("Arrow Flight handler not configured - returning 501");
                return Response::builder()
                    .status(StatusCode::NOT_IMPLEMENTED)
                    .header("content-type", "application/grpc")
                    .header("grpc-status", "12") // UNIMPLEMENTED
                    .header(
                        "grpc-message",
                        "Arrow Flight handler not configured for unified port mode",
                    )
                    .body(Body::empty())
                    .expect("response builder should not fail");
            }

            // Placeholder - in a full implementation, this would route to Arrow Flight
            Response::builder()
                .status(StatusCode::NOT_IMPLEMENTED)
                .header("content-type", "application/grpc")
                .header("grpc-status", "12")
                .header(
                    "grpc-message",
                    "Arrow Flight multiplexing not yet implemented",
                )
                .body(Body::empty())
                .expect("response builder should not fail")
        })
    }

    fn name(&self) -> &str {
        "arrow_flight"
    }

    fn is_ready(&self) -> bool {
        self.ready
    }
}

/// Builder for creating Arrow Flight handlers
pub struct ArrowFlightHandlerBuilder {
    _ready: bool,
}

impl ArrowFlightHandlerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self { _ready: false }
    }

    /// Mark the handler as ready
    #[allow(dead_code)]
    pub fn ready(mut self) -> Self {
        self._ready = true;
        self
    }

    /// Build the Arrow Flight handler
    #[allow(dead_code)]
    pub fn build(self) -> ArrowFlightHandler {
        ArrowFlightHandler {
            ready: self._ready,
            _router: None,
        }
    }
}

impl Default for ArrowFlightHandlerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arrow_flight_handler_creation() {
        let handler = ArrowFlightHandler::new();
        assert!(!handler.is_ready());
        assert_eq!(handler.protocol(), DetectedProtocol::ArrowFlight);
        assert_eq!(handler.name(), "arrow_flight");
    }

    #[test]
    fn test_arrow_flight_handler_ready() {
        let handler = ArrowFlightHandler::ready();
        assert!(handler.is_ready());
    }

    #[test]
    fn test_arrow_flight_handler_builder() {
        let handler = ArrowFlightHandlerBuilder::new().ready().build();
        assert!(handler.is_ready());
    }
}
