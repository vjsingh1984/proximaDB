// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC protocol detector
//!
//! Detects gRPC requests based on Content-Type and HTTP/2 indicators.

use crate::network::multiplex::traits::{DetectedProtocol, DetectionResult, ProtocolDetector};
use hyper::http::{Request, Version, header};

/// Default priority for gRPC detector (after Arrow Flight, before REST)
pub const GRPC_DETECTOR_PRIORITY: u32 = 100;

/// gRPC content-type prefix
const GRPC_CONTENT_TYPE_PREFIX: &str = "application/grpc";

/// gRPC protocol detector
///
/// Detection criteria:
/// 1. Content-Type starts with `application/grpc` (highest confidence)
/// 2. Has `grpc-timeout` header (high confidence)
/// 3. Path contains gRPC service pattern (medium confidence)
#[derive(Debug, Clone, Default)]
pub struct GrpcDetector {
    /// Custom priority override
    priority: Option<u32>,
}

impl GrpcDetector {
    /// Create a new gRPC detector with default priority
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a gRPC detector with custom priority
    pub fn with_priority(priority: u32) -> Self {
        Self {
            priority: Some(priority),
        }
    }

    /// Check if Content-Type indicates gRPC
    fn is_grpc_content_type(content_type: &str) -> bool {
        content_type.starts_with(GRPC_CONTENT_TYPE_PREFIX)
    }

    /// Check if path looks like a gRPC service call
    fn is_grpc_path(path: &str) -> bool {
        // gRPC paths are typically /package.Service/Method
        // They have at least two segments and often contain dots
        if path.len() < 3 {
            return false;
        }

        // Must start with /
        if !path.starts_with('/') {
            return false;
        }

        // Count segments (gRPC has exactly 2: /Service/Method)
        let segments: Vec<&str> = path[1..].split('/').collect();
        if segments.len() != 2 {
            return false;
        }

        // Service name often contains dots (package.Service)
        let service = segments[0];
        let method = segments[1];

        // Both segments must be non-empty
        if service.is_empty() || method.is_empty() {
            return false;
        }

        // Method names are typically PascalCase and don't have dots
        // Service names often have package prefix with dots
        service.contains('.') || (service.chars().next().map_or(false, |c| c.is_uppercase()))
    }
}

impl ProtocolDetector for GrpcDetector {
    fn detect(&self, request: &Request<()>) -> Option<DetectionResult> {
        let headers = request.headers();

        // Check Content-Type first (most reliable)
        if let Some(content_type) = headers.get(header::CONTENT_TYPE)
            && let Ok(ct) = content_type.to_str()
                && Self::is_grpc_content_type(ct) {
                    return Some(DetectionResult::certain(DetectedProtocol::Grpc));
                }

        // Check for grpc-timeout header (gRPC-specific)
        if headers.contains_key("grpc-timeout") {
            return Some(DetectionResult::new(DetectedProtocol::Grpc, 0.95));
        }

        // Check for te: trailers header (required for gRPC)
        if let Some(te) = headers.get("te")
            && let Ok(te_str) = te.to_str()
                && te_str.contains("trailers") {
                    // Combined with HTTP/2, this is likely gRPC
                    if request.version() == Version::HTTP_2 {
                        return Some(DetectionResult::new(DetectedProtocol::Grpc, 0.85));
                    }
                }

        // Check path pattern
        let path = request.uri().path();
        if Self::is_grpc_path(path) && request.version() == Version::HTTP_2 {
            return Some(DetectionResult::new(DetectedProtocol::Grpc, 0.7));
        }

        None
    }

    fn priority(&self) -> u32 {
        self.priority.unwrap_or(GRPC_DETECTOR_PRIORITY)
    }

    fn target_protocol(&self) -> DetectedProtocol {
        DetectedProtocol::Grpc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request() -> hyper::http::request::Builder {
        Request::builder()
    }

    #[test]
    fn test_grpc_content_type() {
        let detector = GrpcDetector::new();

        let request = make_request()
            .header("content-type", "application/grpc")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.protocol, DetectedProtocol::Grpc);
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn test_grpc_content_type_with_proto() {
        let detector = GrpcDetector::new();

        let request = make_request()
            .header("content-type", "application/grpc+proto")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        assert_eq!(result.unwrap().protocol, DetectedProtocol::Grpc);
    }

    #[test]
    fn test_grpc_timeout_header() {
        let detector = GrpcDetector::new();

        let request = make_request()
            .header("grpc-timeout", "10S")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.protocol, DetectedProtocol::Grpc);
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_not_grpc_json() {
        let detector = GrpcDetector::new();

        let request = make_request()
            .header("content-type", "application/json")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_none());
    }

    #[test]
    fn test_is_grpc_path() {
        assert!(GrpcDetector::is_grpc_path(
            "/proximadb.v1.VectorService/Search"
        ));
        assert!(GrpcDetector::is_grpc_path("/Service/Method"));
        assert!(GrpcDetector::is_grpc_path("/package.Service/Method"));

        assert!(!GrpcDetector::is_grpc_path("/api/v1/health"));
        assert!(!GrpcDetector::is_grpc_path("/"));
        assert!(!GrpcDetector::is_grpc_path(""));
        assert!(!GrpcDetector::is_grpc_path("/singlepath"));
    }

    #[test]
    fn test_priority() {
        let default = GrpcDetector::new();
        assert_eq!(default.priority(), GRPC_DETECTOR_PRIORITY);

        let custom = GrpcDetector::with_priority(150);
        assert_eq!(custom.priority(), 150);
    }
}
