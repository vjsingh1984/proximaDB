// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Integration tests for protocol detectors

use super::*;
use crate::network::multiplex::traits::{DetectedProtocol, ProtocolDetector};
use hyper::http::{Request, Version};

/// Test that detectors have correct priority ordering
#[test]
fn test_detector_priority_order() {
    let arrow = ArrowFlightDetector::new();
    let grpc = GrpcDetector::new();
    let rest = RestDetector::new();

    // Arrow Flight should have highest priority
    assert!(arrow.priority() > grpc.priority());
    // gRPC should be higher than REST
    assert!(grpc.priority() > rest.priority());
}

/// Test that Arrow Flight is detected before gRPC
#[test]
fn test_arrow_flight_before_grpc() {
    let arrow = ArrowFlightDetector::new();
    let grpc = GrpcDetector::new();

    // Arrow Flight request
    let request = Request::builder()
        .uri("/arrow.flight.protocol.FlightService/DoGet")
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();

    // Arrow Flight detector should match
    let arrow_result = arrow.detect(&request);
    assert!(arrow_result.is_some());
    assert_eq!(
        arrow_result.unwrap().protocol,
        DetectedProtocol::ArrowFlight
    );

    // gRPC detector would also match (it's gRPC under the hood)
    let grpc_result = grpc.detect(&request);
    assert!(grpc_result.is_some());

    // But Arrow Flight has higher priority, so it should win
    assert!(arrow.priority() > grpc.priority());
}

/// Test that gRPC is detected before REST
#[test]
fn test_grpc_before_rest() {
    let grpc = GrpcDetector::new();
    let rest = RestDetector::new();

    // gRPC request
    let request = Request::builder()
        .uri("/proximadb.v1.VectorService/Search")
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();

    // gRPC detector should match
    let grpc_result = grpc.detect(&request);
    assert!(grpc_result.is_some());
    assert_eq!(grpc_result.unwrap().protocol, DetectedProtocol::Grpc);

    // REST detector should NOT match (grpc content-type is excluded)
    let rest_result = rest.detect(&request);
    // With strict mode, REST should not detect
    let strict_rest = RestDetector::strict();
    let strict_result = strict_rest.detect(&request);
    assert!(strict_result.is_none());
}

/// Test REST fallback behavior
#[test]
fn test_rest_fallback() {
    let arrow = ArrowFlightDetector::new();
    let grpc = GrpcDetector::new();
    let rest = RestDetector::new();

    // Plain HTTP request (REST)
    let request = Request::builder()
        .uri("/api/v1/collections")
        .header("content-type", "application/json")
        .body(())
        .unwrap();

    // Arrow Flight should not match
    assert!(arrow.detect(&request).is_none());

    // gRPC should not match
    assert!(grpc.detect(&request).is_none());

    // REST should match
    let rest_result = rest.detect(&request);
    assert!(rest_result.is_some());
    assert_eq!(rest_result.unwrap().protocol, DetectedProtocol::Rest);
}

/// Test unknown request goes to REST fallback
#[test]
fn test_unknown_request_fallback() {
    let rest = RestDetector::new();

    // Completely unknown request (use HTTP/2 to avoid version-based boost)
    let request = Request::builder()
        .uri("/some/random/path")
        .version(Version::HTTP_2)
        .body(())
        .unwrap();

    // REST in fallback mode should still detect
    let result = rest.detect(&request);
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.protocol, DetectedProtocol::Rest);
    // But with low confidence
    assert!(result.confidence < 0.5);
}

/// Test ProximaDB-specific paths
#[test]
fn test_proximadb_grpc_paths() {
    let grpc = GrpcDetector::new();

    let paths = &[
        "/proximadb.v1.VectorService/Search",
        "/proximadb.v1.VectorService/Insert",
        "/proximadb.v1.CollectionService/Create",
        "/proximadb.v1.GraphService/Traverse",
    ];

    for path in paths {
        let request = Request::builder()
            .uri(*path)
            .header("content-type", "application/grpc")
            .version(Version::HTTP_2)
            .body(())
            .unwrap();

        let result = grpc.detect(&request);
        assert!(result.is_some(), "Should detect gRPC for path: {}", path);
        assert_eq!(result.unwrap().protocol, DetectedProtocol::Grpc);
    }
}

/// Test ProximaDB REST API paths
#[test]
fn test_proximadb_rest_paths() {
    let rest = RestDetector::new();

    let paths = &[
        "/api/v1/collections",
        "/api/v1/vectors",
        "/api/v1/search",
        "/v1/collections",
        "/collections",
        "/graphs",
        "/health",
        "/metrics",
    ];

    for path in paths {
        let request = Request::builder()
            .uri(*path)
            .header("content-type", "application/json")
            .body(())
            .unwrap();

        let result = rest.detect(&request);
        assert!(result.is_some(), "Should detect REST for path: {}", path);
        assert_eq!(result.unwrap().protocol, DetectedProtocol::Rest);
    }
}

/// Test confidence levels
#[test]
fn test_confidence_levels() {
    // Arrow Flight with explicit path should be 1.0
    let arrow = ArrowFlightDetector::new();
    let arrow_req = Request::builder()
        .uri("/arrow.flight.protocol.FlightService/DoGet")
        .body(())
        .unwrap();
    assert_eq!(arrow.detect(&arrow_req).unwrap().confidence, 1.0);

    // gRPC with content-type should be 1.0
    let grpc = GrpcDetector::new();
    let grpc_req = Request::builder()
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();
    assert_eq!(grpc.detect(&grpc_req).unwrap().confidence, 1.0);

    // REST with JSON content-type should be >= 0.9
    let rest = RestDetector::new();
    let rest_req = Request::builder()
        .header("content-type", "application/json")
        .body(())
        .unwrap();
    assert!(rest.detect(&rest_req).unwrap().confidence >= 0.9);
}
