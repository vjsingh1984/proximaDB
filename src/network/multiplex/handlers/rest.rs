// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! REST protocol handler wrapping the Axum server
//!
//! This handler wraps Axum routers to enable protocol
//! multiplexing on a single port.

use crate::network::multiplex::traits::{BoxResponseFuture, DetectedProtocol, ProtocolHandler};
use axum::body::Body;
use hyper::http::{Request, Response, StatusCode};
use std::sync::Arc;
use tracing::{trace, warn};

/// REST protocol handler
///
/// Wraps an Axum Router to handle HTTP/REST requests through the multiplexer.
///
/// # Usage
///
/// ```ignore
/// use axum::Router;
/// use crate::network::multiplex::handlers::RestHandler;
///
/// let router = Router::new().route("/health", get(handler));
/// let handler = RestHandler::new(router);
/// ```
pub struct RestHandler {
    /// Flag indicating if the handler is ready
    ready: bool,
    /// Optional wrapped router (placeholder for future implementation)
    _router: Option<Arc<()>>,
}

impl RestHandler {
    /// Create a new REST handler
    ///
    /// Note: In a full implementation, this would accept an Axum Router.
    /// Currently returns a placeholder that returns 501 Not Implemented.
    pub fn new() -> Self {
        Self {
            ready: false,
            _router: None,
        }
    }

    /// Create a REST handler marked as ready (for testing)
    pub fn ready() -> Self {
        Self {
            ready: true,
            _router: None,
        }
    }
}

impl Default for RestHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RestHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestHandler")
            .field("ready", &self.ready)
            .finish()
    }
}

impl Clone for RestHandler {
    fn clone(&self) -> Self {
        Self {
            ready: self.ready,
            _router: self._router.clone(),
        }
    }
}

impl ProtocolHandler for RestHandler {
    fn protocol(&self) -> DetectedProtocol {
        DetectedProtocol::Rest
    }

    fn handle(&self, request: Request<Body>) -> BoxResponseFuture {
        let ready = self.ready;

        Box::pin(async move {
            trace!(
                method = %request.method(),
                uri = %request.uri(),
                "Handling REST request"
            );

            if !ready {
                warn!("REST handler not configured - returning 501");
                return Response::builder()
                    .status(StatusCode::NOT_IMPLEMENTED)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"error":"REST handler not configured for unified port mode"}"#))
                    .expect("response builder should not fail");
            }

            // Placeholder - in a full implementation, this would route to Axum
            Response::builder()
                .status(StatusCode::NOT_IMPLEMENTED)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"error":"REST multiplexing not yet implemented"}"#))
                .expect("response builder should not fail")
        })
    }

    fn name(&self) -> &str {
        "rest"
    }

    fn is_ready(&self) -> bool {
        self.ready
    }
}

/// Builder for creating REST handlers
pub struct RestHandlerBuilder {
    ready: bool,
}

impl RestHandlerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self { ready: false }
    }

    /// Mark the handler as ready
    pub fn ready(mut self) -> Self {
        self.ready = true;
        self
    }

    /// Build the REST handler
    pub fn build(self) -> RestHandler {
        RestHandler {
            ready: self.ready,
            _router: None,
        }
    }
}

impl Default for RestHandlerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rest_handler_creation() {
        let handler = RestHandler::new();
        assert!(!handler.is_ready());
        assert_eq!(handler.protocol(), DetectedProtocol::Rest);
        assert_eq!(handler.name(), "rest");
    }

    #[test]
    fn test_rest_handler_ready() {
        let handler = RestHandler::ready();
        assert!(handler.is_ready());
    }

    #[test]
    fn test_rest_handler_builder() {
        let handler = RestHandlerBuilder::new()
            .ready()
            .build();
        assert!(handler.is_ready());
    }
}
