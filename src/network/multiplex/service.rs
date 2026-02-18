// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! MultiplexService - Tower service for protocol multiplexing
//!
//! This module implements a Tower `Service` that routes incoming HTTP requests
//! to the appropriate protocol handler based on request characteristics.

use super::traits::{
    BoxResponseFuture, DetectedProtocol, DetectionResult, MultiplexError, SharedDetector,
    SharedHandler,
};
use axum::body::Body;
use hyper::http::{Request, Response};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::Service;
use tracing::{debug, trace, warn};

/// Configuration for the MultiplexService
#[derive(Debug, Clone)]
pub struct MultiplexConfig {
    /// Whether to use the fallback handler for unknown protocols
    pub use_fallback: bool,
    /// The fallback protocol if detection fails
    pub fallback_protocol: DetectedProtocol,
    /// Minimum confidence required for detection (0.0 to 1.0)
    pub min_confidence: f32,
    /// Enable debug logging for protocol detection
    pub debug_detection: bool,
}

impl Default for MultiplexConfig {
    fn default() -> Self {
        Self {
            use_fallback: true,
            fallback_protocol: DetectedProtocol::Rest,
            min_confidence: 0.5,
            debug_detection: false,
        }
    }
}

/// MultiplexService routes requests to protocol-specific handlers
///
/// This is a Tower `Service` that:
/// 1. Detects the protocol from request headers
/// 2. Routes to the appropriate handler
/// 3. Falls back to REST for unknown protocols
#[derive(Clone)]
pub struct MultiplexService {
    /// Protocol detectors sorted by priority (highest first)
    detectors: Arc<Vec<SharedDetector>>,
    /// Protocol handlers indexed by protocol type
    handlers: Arc<HashMap<DetectedProtocol, SharedHandler>>,
    /// Service configuration
    config: MultiplexConfig,
}

impl MultiplexService {
    /// Create a new MultiplexService with the given detectors and handlers
    pub fn new(
        mut detectors: Vec<SharedDetector>,
        handlers: HashMap<DetectedProtocol, SharedHandler>,
        config: MultiplexConfig,
    ) -> Self {
        // Sort detectors by priority (highest first)
        detectors.sort_by(|a, b| b.priority().cmp(&a.priority()));

        Self {
            detectors: Arc::new(detectors),
            handlers: Arc::new(handlers),
            config,
        }
    }

    /// Detect the protocol for an incoming request
    pub fn detect_protocol(&self, request: &Request<()>) -> DetectionResult {
        // Try each detector in priority order
        for detector in self.detectors.iter() {
            if let Some(result) = detector.detect(request) {
                if result.confidence >= self.config.min_confidence {
                    if self.config.debug_detection {
                        debug!(
                            protocol = %result.protocol,
                            confidence = result.confidence,
                            detector_priority = detector.priority(),
                            "Protocol detected"
                        );
                    }
                    return result;
                } else {
                    trace!(
                        protocol = %result.protocol,
                        confidence = result.confidence,
                        min_required = self.config.min_confidence,
                        "Detection below confidence threshold"
                    );
                }
            }
        }

        // Use fallback if enabled
        if self.config.use_fallback {
            debug!(
                fallback = %self.config.fallback_protocol,
                "Using fallback protocol"
            );
            DetectionResult::uncertain(self.config.fallback_protocol)
        } else {
            DetectionResult::unknown()
        }
    }

    /// Get the handler for a protocol
    pub fn get_handler(&self, protocol: DetectedProtocol) -> Option<SharedHandler> {
        self.handlers.get(&protocol).cloned()
    }

    /// Check if all handlers are ready
    pub fn all_handlers_ready(&self) -> bool {
        self.handlers.values().all(|h| h.is_ready())
    }

    /// Get a list of supported protocols
    pub fn supported_protocols(&self) -> Vec<DetectedProtocol> {
        self.handlers.keys().copied().collect()
    }

    /// Get the number of registered detectors
    pub fn detector_count(&self) -> usize {
        self.detectors.len()
    }

    /// Get the number of registered handlers
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }
}

impl std::fmt::Debug for MultiplexService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiplexService")
            .field("detector_count", &self.detectors.len())
            .field("handler_count", &self.handlers.len())
            .field("supported_protocols", &self.supported_protocols())
            .field("config", &self.config)
            .finish()
    }
}

/// Future returned by MultiplexService
pub struct MultiplexFuture {
    inner: BoxResponseFuture,
}

impl Future for MultiplexFuture {
    type Output = Result<Response<Body>, std::convert::Infallible>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll the inner future
        match Pin::new(&mut self.inner).poll(cx) {
            Poll::Ready(response) => Poll::Ready(Ok(response)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Service<Request<Body>> for MultiplexService {
    type Response = Response<Body>;
    type Error = std::convert::Infallible;
    type Future = MultiplexFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Always ready - handlers manage their own backpressure
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        // Create a minimal request for protocol detection (without consuming the body)
        // We build a new Request with just the headers and URI we need for detection
        let _detection_request = Request::builder()
            .method(request.method().clone())
            .uri(request.uri().clone())
            .version(request.version())
            .body(())
            .expect("building detection request should not fail");

        // Copy headers from original request to detection request
        // We need to rebuild since Request doesn't allow header modification after creation
        let mut detection_builder = Request::builder()
            .method(request.method().clone())
            .uri(request.uri().clone())
            .version(request.version());

        // Copy all headers
        for (key, value) in request.headers().iter() {
            detection_builder = detection_builder.header(key, value);
        }
        let detection_request = detection_builder
            .body(())
            .expect("building detection request should not fail");

        // Detect protocol
        let detection = self.detect_protocol(&detection_request);

        // Use the original request directly (body intact)
        let full_request = request;

        // Route to handler
        let response_future: BoxResponseFuture = if detection.protocol == DetectedProtocol::Unknown
        {
            warn!("Could not detect protocol, returning error");
            Box::pin(async move { MultiplexError::UnknownProtocol.to_response() })
        } else if let Some(handler) = self.get_handler(detection.protocol) {
            trace!(
                protocol = %detection.protocol,
                confidence = detection.confidence,
                "Routing request to handler"
            );
            handler.handle(full_request)
        } else {
            warn!(
                protocol = %detection.protocol,
                "No handler registered for detected protocol"
            );
            let error = MultiplexError::NoHandler(detection.protocol);
            Box::pin(async move { error.to_response() })
        };

        MultiplexFuture {
            inner: response_future,
        }
    }
}

/// Statistics about protocol routing
#[derive(Debug, Default, Clone)]
pub struct MultiplexStats {
    /// Total requests processed
    pub total_requests: u64,
    /// Requests by protocol
    pub requests_by_protocol: HashMap<DetectedProtocol, u64>,
    /// Detection failures
    pub detection_failures: u64,
    /// Handler not found errors
    pub handler_not_found: u64,
}

impl MultiplexStats {
    /// Record a request for the given protocol
    pub fn record_request(&mut self, protocol: DetectedProtocol) {
        self.total_requests += 1;
        *self.requests_by_protocol.entry(protocol).or_insert(0) += 1;
    }

    /// Record a detection failure
    pub fn record_detection_failure(&mut self) {
        self.detection_failures += 1;
    }

    /// Record a handler not found error
    pub fn record_handler_not_found(&mut self) {
        self.handler_not_found += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::multiplex::traits::{ProtocolDetector, ProtocolHandler};

    // Mock detector for testing
    struct MockDetector {
        protocol: DetectedProtocol,
        priority: u32,
        should_detect: bool,
    }

    impl ProtocolDetector for MockDetector {
        fn detect(&self, _request: &Request<()>) -> Option<DetectionResult> {
            if self.should_detect {
                Some(DetectionResult::certain(self.protocol))
            } else {
                None
            }
        }

        fn priority(&self) -> u32 {
            self.priority
        }

        fn target_protocol(&self) -> DetectedProtocol {
            self.protocol
        }
    }

    // Mock handler for testing
    struct MockHandler {
        protocol: DetectedProtocol,
    }

    impl ProtocolHandler for MockHandler {
        fn protocol(&self) -> DetectedProtocol {
            self.protocol
        }

        fn handle(&self, _request: Request<Body>) -> BoxResponseFuture {
            let protocol = self.protocol;
            Box::pin(async move {
                Response::builder()
                    .status(200)
                    .header("x-protocol", protocol.name())
                    .body(Body::from(format!("Handled by {}", protocol)))
                    .unwrap()
            })
        }
    }

    #[test]
    fn test_detector_priority_sorting() {
        let detectors: Vec<SharedDetector> = vec![
            Arc::new(MockDetector {
                protocol: DetectedProtocol::Rest,
                priority: 10,
                should_detect: true,
            }),
            Arc::new(MockDetector {
                protocol: DetectedProtocol::Grpc,
                priority: 100,
                should_detect: true,
            }),
            Arc::new(MockDetector {
                protocol: DetectedProtocol::ArrowFlight,
                priority: 110,
                should_detect: true,
            }),
        ];

        let handlers = HashMap::new();
        let service = MultiplexService::new(detectors, handlers, MultiplexConfig::default());

        // First detector should be Arrow Flight (highest priority)
        assert_eq!(service.detectors[0].priority(), 110);
        assert_eq!(service.detectors[1].priority(), 100);
        assert_eq!(service.detectors[2].priority(), 10);
    }

    #[test]
    fn test_protocol_detection() {
        let detectors: Vec<SharedDetector> = vec![Arc::new(MockDetector {
            protocol: DetectedProtocol::Grpc,
            priority: 100,
            should_detect: true,
        })];

        let handlers = HashMap::new();
        let service = MultiplexService::new(detectors, handlers, MultiplexConfig::default());

        let request = Request::builder().body(()).unwrap();
        let result = service.detect_protocol(&request);

        assert_eq!(result.protocol, DetectedProtocol::Grpc);
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn test_fallback_protocol() {
        let detectors: Vec<SharedDetector> = vec![Arc::new(MockDetector {
            protocol: DetectedProtocol::Grpc,
            priority: 100,
            should_detect: false, // Won't detect
        })];

        let handlers = HashMap::new();
        let config = MultiplexConfig {
            use_fallback: true,
            fallback_protocol: DetectedProtocol::Rest,
            ..Default::default()
        };
        let service = MultiplexService::new(detectors, handlers, config);

        let request = Request::builder().body(()).unwrap();
        let result = service.detect_protocol(&request);

        assert_eq!(result.protocol, DetectedProtocol::Rest);
    }

    #[test]
    fn test_no_fallback() {
        let detectors: Vec<SharedDetector> = vec![];
        let handlers = HashMap::new();
        let config = MultiplexConfig {
            use_fallback: false,
            ..Default::default()
        };
        let service = MultiplexService::new(detectors, handlers, config);

        let request = Request::builder().body(()).unwrap();
        let result = service.detect_protocol(&request);

        assert_eq!(result.protocol, DetectedProtocol::Unknown);
    }

    #[test]
    fn test_supported_protocols() {
        let detectors = vec![];
        let mut handlers: HashMap<DetectedProtocol, SharedHandler> = HashMap::new();
        handlers.insert(
            DetectedProtocol::Rest,
            Arc::new(MockHandler {
                protocol: DetectedProtocol::Rest,
            }),
        );
        handlers.insert(
            DetectedProtocol::Grpc,
            Arc::new(MockHandler {
                protocol: DetectedProtocol::Grpc,
            }),
        );

        let service = MultiplexService::new(detectors, handlers, MultiplexConfig::default());
        let protocols = service.supported_protocols();

        assert_eq!(protocols.len(), 2);
        assert!(protocols.contains(&DetectedProtocol::Rest));
        assert!(protocols.contains(&DetectedProtocol::Grpc));
    }

    #[test]
    fn test_multiplex_stats() {
        let mut stats = MultiplexStats::default();

        stats.record_request(DetectedProtocol::Rest);
        stats.record_request(DetectedProtocol::Rest);
        stats.record_request(DetectedProtocol::Grpc);
        stats.record_detection_failure();

        assert_eq!(stats.total_requests, 3);
        assert_eq!(
            stats.requests_by_protocol.get(&DetectedProtocol::Rest),
            Some(&2)
        );
        assert_eq!(
            stats.requests_by_protocol.get(&DetectedProtocol::Grpc),
            Some(&1)
        );
        assert_eq!(stats.detection_failures, 1);
    }
}
