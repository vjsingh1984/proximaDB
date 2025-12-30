// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Builder pattern for MultiplexService configuration
//!
//! This module provides a fluent API for constructing MultiplexService instances,
//! following the Builder pattern for complex object construction.

use super::service::{MultiplexConfig, MultiplexService};
use super::traits::{DetectedProtocol, ProtocolDetector, ProtocolHandler, SharedDetector, SharedHandler};
use std::collections::HashMap;
use std::sync::Arc;

/// Builder for constructing MultiplexService instances
///
/// # Example
///
/// ```ignore
/// let service = MultiplexServiceBuilder::new()
///     .add_detector(GrpcDetector::new())
///     .add_detector(ArrowFlightDetector::new())
///     .add_detector(RestDetector::new())
///     .add_handler(grpc_handler)
///     .add_handler(arrow_flight_handler)
///     .add_handler(rest_handler)
///     .with_fallback(DetectedProtocol::Rest)
///     .build();
/// ```
#[derive(Default)]
pub struct MultiplexServiceBuilder {
    detectors: Vec<SharedDetector>,
    handlers: HashMap<DetectedProtocol, SharedHandler>,
    config: MultiplexConfig,
}

impl MultiplexServiceBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a protocol detector
    ///
    /// Detectors are automatically sorted by priority when building.
    pub fn add_detector<D: ProtocolDetector + 'static>(mut self, detector: D) -> Self {
        self.detectors.push(Arc::new(detector));
        self
    }

    /// Add a shared protocol detector
    pub fn add_shared_detector(mut self, detector: SharedDetector) -> Self {
        self.detectors.push(detector);
        self
    }

    /// Add a protocol handler
    ///
    /// The handler is registered for the protocol it reports via `handler.protocol()`.
    pub fn add_handler<H: ProtocolHandler + 'static>(mut self, handler: H) -> Self {
        let protocol = handler.protocol();
        self.handlers.insert(protocol, Arc::new(handler));
        self
    }

    /// Add a shared protocol handler
    pub fn add_shared_handler(mut self, handler: SharedHandler) -> Self {
        let protocol = handler.protocol();
        self.handlers.insert(protocol, handler);
        self
    }

    /// Set whether to use a fallback protocol for unknown requests
    pub fn with_fallback(mut self, protocol: DetectedProtocol) -> Self {
        self.config.use_fallback = true;
        self.config.fallback_protocol = protocol;
        self
    }

    /// Disable fallback protocol
    pub fn without_fallback(mut self) -> Self {
        self.config.use_fallback = false;
        self
    }

    /// Set the minimum detection confidence threshold
    pub fn with_min_confidence(mut self, confidence: f32) -> Self {
        self.config.min_confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Enable debug logging for protocol detection
    pub fn with_debug_detection(mut self, enabled: bool) -> Self {
        self.config.debug_detection = enabled;
        self
    }

    /// Set the full configuration
    pub fn with_config(mut self, config: MultiplexConfig) -> Self {
        self.config = config;
        self
    }

    /// Build the MultiplexService
    ///
    /// # Panics
    ///
    /// This method does not panic, but the resulting service will return errors
    /// for protocols without registered handlers.
    pub fn build(self) -> MultiplexService {
        MultiplexService::new(self.detectors, self.handlers, self.config)
    }

    /// Build the MultiplexService, returning an error if validation fails
    pub fn try_build(self) -> Result<MultiplexService, BuilderError> {
        // Validate that we have at least one detector
        if self.detectors.is_empty() {
            return Err(BuilderError::NoDetectors);
        }

        // Validate that we have at least one handler
        if self.handlers.is_empty() {
            return Err(BuilderError::NoHandlers);
        }

        // Validate that fallback handler exists if fallback is enabled
        if self.config.use_fallback && !self.handlers.contains_key(&self.config.fallback_protocol) {
            return Err(BuilderError::MissingFallbackHandler(
                self.config.fallback_protocol,
            ));
        }

        Ok(self.build())
    }

    /// Check if a handler is registered for a protocol
    pub fn has_handler(&self, protocol: DetectedProtocol) -> bool {
        self.handlers.contains_key(&protocol)
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

/// Errors that can occur during MultiplexService building
#[derive(Debug)]
pub enum BuilderError {
    /// No detectors were registered
    NoDetectors,
    /// No handlers were registered
    NoHandlers,
    /// Fallback is enabled but no handler exists for the fallback protocol
    MissingFallbackHandler(DetectedProtocol),
    /// A detector was registered for a protocol without a corresponding handler
    DetectorWithoutHandler(DetectedProtocol),
}

impl std::fmt::Display for BuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderError::NoDetectors => write!(f, "no protocol detectors registered"),
            BuilderError::NoHandlers => write!(f, "no protocol handlers registered"),
            BuilderError::MissingFallbackHandler(p) => {
                write!(f, "fallback enabled but no handler for protocol: {}", p)
            }
            BuilderError::DetectorWithoutHandler(p) => {
                write!(f, "detector registered for {} but no handler", p)
            }
        }
    }
}

impl std::error::Error for BuilderError {}

/// Convenience struct for building a complete multiplex setup with common protocols
pub struct CommonMultiplexBuilder {
    inner: MultiplexServiceBuilder,
}

impl CommonMultiplexBuilder {
    /// Create a new common multiplex builder
    pub fn new() -> Self {
        Self {
            inner: MultiplexServiceBuilder::new(),
        }
    }

    /// Add a REST handler (also adds REST detector with lowest priority)
    pub fn with_rest<H: ProtocolHandler + 'static>(mut self, handler: H) -> Self {
        // REST detector will be added during build if not present
        self.inner = self.inner.add_handler(handler);
        self
    }

    /// Add a gRPC handler (also adds gRPC detector with high priority)
    pub fn with_grpc<H: ProtocolHandler + 'static>(mut self, handler: H) -> Self {
        self.inner = self.inner.add_handler(handler);
        self
    }

    /// Add an Arrow Flight handler (also adds Arrow Flight detector with highest priority)
    pub fn with_arrow_flight<H: ProtocolHandler + 'static>(mut self, handler: H) -> Self {
        self.inner = self.inner.add_handler(handler);
        self
    }

    /// Get the inner builder for additional customization
    pub fn inner(self) -> MultiplexServiceBuilder {
        self.inner
    }

    /// Build with REST as fallback (recommended)
    pub fn build_with_rest_fallback(self) -> MultiplexService {
        self.inner.with_fallback(DetectedProtocol::Rest).build()
    }
}

impl Default for CommonMultiplexBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::multiplex::traits::{BoxResponseFuture, DetectionResult};
    use axum::body::Body;
    use hyper::http::{Request, Response};

    // Test detector
    struct TestDetector(DetectedProtocol);

    impl ProtocolDetector for TestDetector {
        fn detect(&self, _request: &Request<()>) -> Option<DetectionResult> {
            Some(DetectionResult::certain(self.0))
        }

        fn priority(&self) -> u32 {
            match self.0 {
                DetectedProtocol::ArrowFlight => 110,
                DetectedProtocol::Grpc => 100,
                DetectedProtocol::Rest => 10,
                _ => 0,
            }
        }

        fn target_protocol(&self) -> DetectedProtocol {
            self.0
        }
    }

    // Test handler
    struct TestHandler(DetectedProtocol);

    impl ProtocolHandler for TestHandler {
        fn protocol(&self) -> DetectedProtocol {
            self.0
        }

        fn handle(&self, _request: Request<Body>) -> BoxResponseFuture {
            Box::pin(async { Response::new(Body::empty()) })
        }
    }

    #[test]
    fn test_builder_basic() {
        let service = MultiplexServiceBuilder::new()
            .add_detector(TestDetector(DetectedProtocol::Rest))
            .add_handler(TestHandler(DetectedProtocol::Rest))
            .build();

        assert_eq!(service.detector_count(), 1);
        assert_eq!(service.handler_count(), 1);
    }

    #[test]
    fn test_builder_with_fallback() {
        let service = MultiplexServiceBuilder::new()
            .add_detector(TestDetector(DetectedProtocol::Rest))
            .add_handler(TestHandler(DetectedProtocol::Rest))
            .with_fallback(DetectedProtocol::Rest)
            .build();

        assert!(service.supported_protocols().contains(&DetectedProtocol::Rest));
    }

    #[test]
    fn test_try_build_no_detectors() {
        let result = MultiplexServiceBuilder::new()
            .add_handler(TestHandler(DetectedProtocol::Rest))
            .try_build();

        assert!(matches!(result, Err(BuilderError::NoDetectors)));
    }

    #[test]
    fn test_try_build_no_handlers() {
        let result = MultiplexServiceBuilder::new()
            .add_detector(TestDetector(DetectedProtocol::Rest))
            .try_build();

        assert!(matches!(result, Err(BuilderError::NoHandlers)));
    }

    #[test]
    fn test_try_build_missing_fallback() {
        let result = MultiplexServiceBuilder::new()
            .add_detector(TestDetector(DetectedProtocol::Grpc))
            .add_handler(TestHandler(DetectedProtocol::Grpc))
            .with_fallback(DetectedProtocol::Rest) // REST handler not registered
            .try_build();

        assert!(matches!(
            result,
            Err(BuilderError::MissingFallbackHandler(DetectedProtocol::Rest))
        ));
    }

    #[test]
    fn test_builder_count_methods() {
        let builder = MultiplexServiceBuilder::new()
            .add_detector(TestDetector(DetectedProtocol::Rest))
            .add_detector(TestDetector(DetectedProtocol::Grpc))
            .add_handler(TestHandler(DetectedProtocol::Rest));

        assert_eq!(builder.detector_count(), 2);
        assert_eq!(builder.handler_count(), 1);
        assert!(builder.has_handler(DetectedProtocol::Rest));
        assert!(!builder.has_handler(DetectedProtocol::Grpc));
    }

    #[test]
    fn test_common_builder() {
        let _service = CommonMultiplexBuilder::new()
            .with_rest(TestHandler(DetectedProtocol::Rest))
            .build_with_rest_fallback();
    }
}
