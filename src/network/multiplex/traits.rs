// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Core traits for protocol multiplexing - SOLID Interface Segregation
//!
//! This module defines minimal, focused traits that allow different protocols
//! (REST, gRPC, Arrow Flight) to share a single port while maintaining
//! extensibility for future protocols.

use axum::body::Body;
use hyper::http::{Request, Response, StatusCode};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Detected protocol type for routing requests
/// Open/Closed principle: Add variants without modifying existing code
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetectedProtocol {
    /// HTTP/1.1 REST API (JSON, form data, etc.)
    Rest,
    /// gRPC over HTTP/2 (application/grpc*)
    Grpc,
    /// Apache Arrow Flight over HTTP/2
    ArrowFlight,
    /// WebSocket connection (future)
    WebSocket,
    /// Unknown or undetectable protocol
    Unknown,
}

impl DetectedProtocol {
    /// Returns true if this protocol requires HTTP/2
    pub fn requires_http2(&self) -> bool {
        matches!(self, DetectedProtocol::Grpc | DetectedProtocol::ArrowFlight)
    }

    /// Returns the protocol name for logging/metrics
    pub fn name(&self) -> &'static str {
        match self {
            DetectedProtocol::Rest => "rest",
            DetectedProtocol::Grpc => "grpc",
            DetectedProtocol::ArrowFlight => "arrow_flight",
            DetectedProtocol::WebSocket => "websocket",
            DetectedProtocol::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for DetectedProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Protocol detection result with confidence
#[derive(Debug, Clone)]
pub struct DetectionResult {
    /// The detected protocol
    pub protocol: DetectedProtocol,
    /// Detection confidence (0.0 to 1.0)
    /// Higher values indicate more certain detection
    pub confidence: f32,
}

impl DetectionResult {
    /// Create a new detection result
    pub fn new(protocol: DetectedProtocol, confidence: f32) -> Self {
        Self {
            protocol,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }

    /// Create a certain detection (confidence = 1.0)
    pub fn certain(protocol: DetectedProtocol) -> Self {
        Self::new(protocol, 1.0)
    }

    /// Create an uncertain detection (confidence = 0.5)
    pub fn uncertain(protocol: DetectedProtocol) -> Self {
        Self::new(protocol, 0.5)
    }

    /// Create an unknown detection
    pub fn unknown() -> Self {
        Self::new(DetectedProtocol::Unknown, 0.0)
    }
}

/// Protocol detector trait - Interface Segregation Principle
///
/// Each detector has a single responsibility: detect one protocol type.
/// Detectors are ordered by priority (higher = checked first).
pub trait ProtocolDetector: Send + Sync + 'static {
    /// Attempt to detect the protocol from request headers
    ///
    /// Returns `Some(result)` if this detector can identify the protocol,
    /// or `None` if it cannot determine the protocol.
    fn detect(&self, request: &Request<()>) -> Option<DetectionResult>;

    /// Detection priority (higher = checked first)
    ///
    /// Recommended ranges:
    /// - 100-150: Highly specific protocols (Arrow Flight)
    /// - 50-99: Standard protocols (gRPC)
    /// - 1-49: Fallback protocols (REST)
    fn priority(&self) -> u32;

    /// Protocol this detector is specialized for
    fn target_protocol(&self) -> DetectedProtocol;
}

/// Type alias for boxed response future
pub type BoxResponseFuture = Pin<Box<dyn Future<Output = Response<Body>> + Send>>;

/// Type alias for handler result
pub type HandlerResult = Result<Response<Body>, MultiplexError>;

/// Protocol handler trait - Dependency Inversion Principle
///
/// All protocol handlers implement this trait, allowing the multiplexer
/// to depend on the abstraction rather than concrete implementations.
pub trait ProtocolHandler: Send + Sync + 'static {
    /// The protocol this handler processes
    fn protocol(&self) -> DetectedProtocol;

    /// Handle an incoming request
    ///
    /// The handler receives the full HTTP request and returns a response.
    /// The request body may be empty if it was consumed during detection.
    fn handle(&self, request: Request<Body>) -> BoxResponseFuture;

    /// Check if this handler is ready to receive requests
    fn is_ready(&self) -> bool {
        true
    }

    /// Optional name for logging/metrics
    fn name(&self) -> &str {
        self.protocol().name()
    }
}

/// Errors that can occur during multiplexing
#[derive(Debug)]
pub enum MultiplexError {
    /// No handler available for the detected protocol
    NoHandler(DetectedProtocol),
    /// Protocol could not be detected
    UnknownProtocol,
    /// Handler returned an error
    HandlerError(String),
    /// Request timeout
    Timeout,
    /// Connection error
    Connection(String),
    /// Internal error
    Internal(String),
}

impl std::fmt::Display for MultiplexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MultiplexError::NoHandler(p) => write!(f, "no handler for protocol: {}", p),
            MultiplexError::UnknownProtocol => write!(f, "could not detect protocol"),
            MultiplexError::HandlerError(msg) => write!(f, "handler error: {}", msg),
            MultiplexError::Timeout => write!(f, "request timeout"),
            MultiplexError::Connection(msg) => write!(f, "connection error: {}", msg),
            MultiplexError::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for MultiplexError {}

impl MultiplexError {
    /// Convert to HTTP status code
    pub fn status_code(&self) -> StatusCode {
        match self {
            MultiplexError::NoHandler(_) => StatusCode::NOT_IMPLEMENTED,
            MultiplexError::UnknownProtocol => StatusCode::BAD_REQUEST,
            MultiplexError::HandlerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            MultiplexError::Timeout => StatusCode::GATEWAY_TIMEOUT,
            MultiplexError::Connection(_) => StatusCode::BAD_GATEWAY,
            MultiplexError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Create error response body
    pub fn to_response(&self) -> Response<Body> {
        let status = self.status_code();
        let body = serde_json::json!({
            "error": self.to_string(),
            "code": status.as_u16()
        });

        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap_or_else(|_| {
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::empty())
                    .unwrap_or_else(|_| Response::new(Body::empty()))
            })
    }
}

/// Shared handler wrapped in Arc for thread-safe sharing
pub type SharedHandler = Arc<dyn ProtocolHandler>;

/// Shared detector wrapped in Arc for thread-safe sharing
pub type SharedDetector = Arc<dyn ProtocolDetector>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detected_protocol_requires_http2() {
        assert!(!DetectedProtocol::Rest.requires_http2());
        assert!(DetectedProtocol::Grpc.requires_http2());
        assert!(DetectedProtocol::ArrowFlight.requires_http2());
        assert!(!DetectedProtocol::WebSocket.requires_http2());
        assert!(!DetectedProtocol::Unknown.requires_http2());
    }

    #[test]
    fn test_detected_protocol_name() {
        assert_eq!(DetectedProtocol::Rest.name(), "rest");
        assert_eq!(DetectedProtocol::Grpc.name(), "grpc");
        assert_eq!(DetectedProtocol::ArrowFlight.name(), "arrow_flight");
    }

    #[test]
    fn test_detection_result_confidence_clamped() {
        let result = DetectionResult::new(DetectedProtocol::Rest, 1.5);
        assert_eq!(result.confidence, 1.0);

        let result = DetectionResult::new(DetectedProtocol::Rest, -0.5);
        assert_eq!(result.confidence, 0.0);
    }

    #[test]
    fn test_multiplex_error_status_codes() {
        assert_eq!(
            MultiplexError::NoHandler(DetectedProtocol::Grpc).status_code(),
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            MultiplexError::UnknownProtocol.status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            MultiplexError::Timeout.status_code(),
            StatusCode::GATEWAY_TIMEOUT
        );
    }
}
