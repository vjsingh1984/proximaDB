// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Arrow Flight protocol detector
//!
//! Detects Arrow Flight requests based on path patterns and gRPC indicators.

use crate::network::multiplex::traits::{DetectedProtocol, DetectionResult, ProtocolDetector};
use hyper::http::{Request, header};

/// Default priority for Arrow Flight detector (highest - most specific)
pub const ARROW_FLIGHT_DETECTOR_PRIORITY: u32 = 110;

/// Arrow Flight protocol path prefix
const ARROW_FLIGHT_PATH_PREFIX: &str = "/arrow.flight.protocol.FlightService/";

/// Alternative Flight service path
const FLIGHT_SERVICE_PATH_PREFIX: &str = "/FlightService/";

/// Arrow Flight protocol detector
///
/// Detection criteria:
/// 1. Path starts with `/arrow.flight.protocol.FlightService/` (certain)
/// 2. Path starts with `/FlightService/` (high confidence)
/// 3. gRPC with Arrow-specific metadata (high confidence)
#[derive(Debug, Clone, Default)]
pub struct ArrowFlightDetector {
    /// Custom priority override
    priority: Option<u32>,
}

impl ArrowFlightDetector {
    /// Create a new Arrow Flight detector with default priority
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an Arrow Flight detector with custom priority
    pub fn with_priority(priority: u32) -> Self {
        Self {
            priority: Some(priority),
        }
    }

    /// Known Arrow Flight methods
    const FLIGHT_METHODS: &'static [&'static str] = &[
        "Handshake",
        "ListFlights",
        "GetFlightInfo",
        "GetSchema",
        "DoGet",
        "DoPut",
        "DoExchange",
        "DoAction",
        "ListActions",
    ];

    /// Check if path matches Arrow Flight pattern
    fn is_flight_path(path: &str) -> Option<DetectionResult> {
        // Most specific: full Arrow Flight path
        if path.starts_with(ARROW_FLIGHT_PATH_PREFIX) {
            return Some(DetectionResult::certain(DetectedProtocol::ArrowFlight));
        }

        // Alternative short path
        if let Some(method) = path.strip_prefix(FLIGHT_SERVICE_PATH_PREFIX) {
            // Extract method name
            if Self::FLIGHT_METHODS.contains(&method) {
                return Some(DetectionResult::certain(DetectedProtocol::ArrowFlight));
            }
            // Unknown method but Flight path
            return Some(DetectionResult::new(DetectedProtocol::ArrowFlight, 0.9));
        }

        None
    }
}

impl ProtocolDetector for ArrowFlightDetector {
    fn detect(&self, request: &Request<()>) -> Option<DetectionResult> {
        let path = request.uri().path();

        // Check path first (most reliable)
        if let Some(result) = Self::is_flight_path(path) {
            return Some(result);
        }

        // Check for Arrow-specific headers
        let headers = request.headers();

        // Arrow Flight uses gRPC, so check content-type
        if let Some(content_type) = headers.get(header::CONTENT_TYPE)
            && let Ok(ct) = content_type.to_str() {
                // Must be gRPC for Arrow Flight
                if !ct.starts_with("application/grpc") {
                    return None;
                }
            }

        // Check for Arrow-specific metadata
        // Arrow Flight often has x-arrow-flight-* headers
        for (name, _) in headers {
            if name.as_str().starts_with("x-arrow-flight") {
                return Some(DetectionResult::new(DetectedProtocol::ArrowFlight, 0.95));
            }
        }

        // Check for Arrow IPC format indicator
        if headers.contains_key("x-arrow-schema") {
            return Some(DetectionResult::new(DetectedProtocol::ArrowFlight, 0.9));
        }

        None
    }

    fn priority(&self) -> u32 {
        self.priority
            .unwrap_or(ARROW_FLIGHT_DETECTOR_PRIORITY)
    }

    fn target_protocol(&self) -> DetectedProtocol {
        DetectedProtocol::ArrowFlight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request() -> hyper::http::request::Builder {
        Request::builder()
    }

    #[test]
    fn test_arrow_flight_full_path() {
        let detector = ArrowFlightDetector::new();

        let request = make_request()
            .uri("/arrow.flight.protocol.FlightService/DoGet")
            .body(())
            .expect("failed to build test request");

        let result = detector.detect(&request);
        assert!(result.is_some());
        let result = result.expect("detection result should be present");
        assert_eq!(result.protocol, DetectedProtocol::ArrowFlight);
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn test_arrow_flight_short_path() {
        let detector = ArrowFlightDetector::new();

        let request = make_request()
            .uri("/FlightService/DoGet")
            .body(())
            .expect("failed to build test request");

        let result = detector.detect(&request);
        assert!(result.is_some());
        let result = result.expect("detection result should be present");
        assert_eq!(result.protocol, DetectedProtocol::ArrowFlight);
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn test_arrow_flight_all_methods() {
        let detector = ArrowFlightDetector::new();

        for method in ArrowFlightDetector::FLIGHT_METHODS {
            let path = format!("/FlightService/{}", method);
            let request = make_request()
                .uri(path.as_str())
                .body(())
                .expect("failed to build test request");

            let result = detector.detect(&request);
            assert!(result.is_some(), "Should detect Flight method: {}", method);
            let result = result.expect("detection result should be present");
            assert_eq!(result.protocol, DetectedProtocol::ArrowFlight);
        }
    }

    #[test]
    fn test_not_arrow_flight_regular_grpc() {
        let detector = ArrowFlightDetector::new();

        let request = make_request()
            .uri("/proximadb.v1.VectorService/Search")
            .header("content-type", "application/grpc")
            .body(())
            .expect("failed to build test request");

        let result = detector.detect(&request);
        assert!(result.is_none());
    }

    #[test]
    fn test_not_arrow_flight_rest() {
        let detector = ArrowFlightDetector::new();

        let request = make_request()
            .uri("/api/v1/vectors")
            .header("content-type", "application/json")
            .body(())
            .expect("failed to build test request");

        let result = detector.detect(&request);
        assert!(result.is_none());
    }

    #[test]
    fn test_arrow_specific_header() {
        let detector = ArrowFlightDetector::new();

        let request = make_request()
            .uri("/some/path")
            .header("content-type", "application/grpc")
            .header("x-arrow-flight-descriptor", "test")
            .body(())
            .expect("failed to build test request");

        let result = detector.detect(&request);
        assert!(result.is_some());
        let result = result.expect("detection result should be present");
        assert_eq!(result.protocol, DetectedProtocol::ArrowFlight);
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_priority() {
        let default = ArrowFlightDetector::new();
        assert_eq!(default.priority(), ARROW_FLIGHT_DETECTOR_PRIORITY);
        assert!(default.priority() > super::super::grpc::GRPC_DETECTOR_PRIORITY);

        let custom = ArrowFlightDetector::with_priority(200);
        assert_eq!(custom.priority(), 200);
    }
}
