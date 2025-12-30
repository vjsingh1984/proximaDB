// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! REST protocol detector
//!
//! Detects REST/HTTP API requests. This is the fallback detector with lowest priority.

use crate::network::multiplex::traits::{DetectedProtocol, DetectionResult, ProtocolDetector};
use hyper::http::{header, Request, Version};

/// Default priority for REST detector (lowest - fallback)
pub const REST_DETECTOR_PRIORITY: u32 = 10;

/// REST content types that indicate a REST request
const REST_CONTENT_TYPES: &[&str] = &[
    "application/json",
    "application/x-www-form-urlencoded",
    "multipart/form-data",
    "text/plain",
    "text/html",
    "text/xml",
    "application/xml",
    "application/octet-stream",
];

/// REST protocol detector
///
/// Detection criteria:
/// 1. Content-Type is a known REST type (high confidence)
/// 2. Accept header includes REST types (medium confidence)
/// 3. HTTP/1.x version (medium confidence)
/// 4. Path starts with /api/ or /v1/ (medium confidence)
/// 5. Default fallback for unknown protocols (low confidence)
#[derive(Debug, Clone)]
pub struct RestDetector {
    /// Custom priority override
    priority: Option<u32>,
    /// Whether to detect any request as REST (true fallback mode)
    fallback_mode: bool,
}

impl Default for RestDetector {
    fn default() -> Self {
        Self {
            priority: None,
            fallback_mode: true,
        }
    }
}

impl RestDetector {
    /// Create a new REST detector with default priority
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a REST detector with custom priority
    pub fn with_priority(priority: u32) -> Self {
        Self {
            priority: Some(priority),
            fallback_mode: true,
        }
    }

    /// Create a strict REST detector that won't match everything
    pub fn strict() -> Self {
        Self {
            priority: None,
            fallback_mode: false,
        }
    }

    /// Check if content type indicates REST
    fn is_rest_content_type(content_type: &str) -> bool {
        // Check exact matches and prefixes
        for rest_type in REST_CONTENT_TYPES {
            if content_type.starts_with(rest_type) {
                return true;
            }
        }
        false
    }

    /// Check if path looks like REST API
    fn is_rest_path(path: &str) -> bool {
        // Common REST API path prefixes
        path.starts_with("/api/")
            || path.starts_with("/v1/")
            || path.starts_with("/v2/")
            || path.starts_with("/v3/")
            || path == "/health"
            || path == "/healthz"
            || path == "/metrics"
            || path == "/ready"
            || path == "/readyz"
            || path.starts_with("/collections")
            || path.starts_with("/graphs")
            || path.starts_with("/vectors")
    }
}

impl ProtocolDetector for RestDetector {
    fn detect(&self, request: &Request<()>) -> Option<DetectionResult> {
        let headers = request.headers();
        let mut confidence = 0.0f32;

        // Check Content-Type
        if let Some(content_type) = headers.get(header::CONTENT_TYPE) {
            if let Ok(ct) = content_type.to_str() {
                if Self::is_rest_content_type(ct) {
                    confidence = 0.9;
                } else if ct.starts_with("application/grpc") {
                    // Explicitly not REST
                    return None;
                }
            }
        }

        // Check Accept header
        if let Some(accept) = headers.get(header::ACCEPT) {
            if let Ok(acc) = accept.to_str() {
                if acc.contains("application/json")
                    || acc.contains("text/html")
                    || acc.contains("*/*")
                {
                    confidence = confidence.max(0.7);
                }
            }
        }

        // Check HTTP version (HTTP/1.x strongly indicates REST)
        match request.version() {
            Version::HTTP_10 | Version::HTTP_11 => {
                confidence = confidence.max(0.6);
            }
            Version::HTTP_2 => {
                // HTTP/2 can be REST, gRPC, or Arrow Flight
                // Don't increase confidence
            }
            _ => {}
        }

        // Check path pattern
        if Self::is_rest_path(request.uri().path()) {
            confidence = confidence.max(0.8);
        }

        // If we have any positive signal, return REST
        if confidence > 0.0 {
            return Some(DetectionResult::new(DetectedProtocol::Rest, confidence));
        }

        // Fallback mode: treat any undetected request as REST
        if self.fallback_mode {
            return Some(DetectionResult::new(DetectedProtocol::Rest, 0.3));
        }

        None
    }

    fn priority(&self) -> u32 {
        self.priority.unwrap_or(REST_DETECTOR_PRIORITY)
    }

    fn target_protocol(&self) -> DetectedProtocol {
        DetectedProtocol::Rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request() -> hyper::http::request::Builder {
        Request::builder()
    }

    #[test]
    fn test_rest_json_content_type() {
        let detector = RestDetector::new();

        let request = make_request()
            .header("content-type", "application/json")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.protocol, DetectedProtocol::Rest);
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_rest_json_with_charset() {
        let detector = RestDetector::new();

        let request = make_request()
            .header("content-type", "application/json; charset=utf-8")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        assert_eq!(result.unwrap().protocol, DetectedProtocol::Rest);
    }

    #[test]
    fn test_rest_form_data() {
        let detector = RestDetector::new();

        let request = make_request()
            .header("content-type", "application/x-www-form-urlencoded")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        assert_eq!(result.unwrap().protocol, DetectedProtocol::Rest);
    }

    #[test]
    fn test_rest_api_path() {
        let detector = RestDetector::new();

        let request = make_request().uri("/api/v1/vectors").body(()).unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.protocol, DetectedProtocol::Rest);
        assert!(result.confidence >= 0.8);
    }

    #[test]
    fn test_rest_health_path() {
        let detector = RestDetector::new();

        for path in &["/health", "/healthz", "/metrics", "/ready"] {
            let request = make_request().uri(*path).body(()).unwrap();
            let result = detector.detect(&request);
            assert!(result.is_some(), "Should detect REST for path: {}", path);
        }
    }

    #[test]
    fn test_not_rest_grpc() {
        let detector = RestDetector::strict();

        let request = make_request()
            .header("content-type", "application/grpc")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_none());
    }

    #[test]
    fn test_fallback_mode() {
        let detector = RestDetector::new();

        // Random request with no clear indicators (use HTTP/2 to avoid version-based boost)
        let request = make_request()
            .uri("/unknown/path")
            .version(Version::HTTP_2)
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.protocol, DetectedProtocol::Rest);
        assert!(result.confidence < 0.5); // Low confidence fallback
    }

    #[test]
    fn test_strict_mode_no_fallback() {
        let detector = RestDetector::strict();

        // Random request with no clear indicators (use HTTP/2 to avoid version-based boost)
        let request = make_request()
            .uri("/unknown/path")
            .version(Version::HTTP_2)
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_none());
    }

    #[test]
    fn test_accept_header() {
        let detector = RestDetector::new();

        let request = make_request()
            .header("accept", "application/json")
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
        assert!(result.unwrap().confidence >= 0.7);
    }

    #[test]
    fn test_priority() {
        let default = RestDetector::new();
        assert_eq!(default.priority(), REST_DETECTOR_PRIORITY);
        assert!(default.priority() < super::super::grpc::GRPC_DETECTOR_PRIORITY);

        let custom = RestDetector::with_priority(50);
        assert_eq!(custom.priority(), 50);
    }

    #[test]
    fn test_http_version() {
        let detector = RestDetector::strict();

        // HTTP/1.1 should increase confidence
        let request = make_request()
            .version(Version::HTTP_11)
            .body(())
            .unwrap();

        let result = detector.detect(&request);
        assert!(result.is_some());
    }
}
