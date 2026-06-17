// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Protocol Multiplexing for Unified Port Architecture
//!
//! This module enables ProximaDB to serve multiple protocols (REST, gRPC, Arrow Flight)
//! through a single port, simplifying deployment and reducing operational complexity.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Unified Port (5678)                           │
//! ├─────────────────────────────────────────────────────────────────┤
//! │                    ProtocolRouter                                │
//! │  ┌─────────────────────────────────────────────────────────────┐│
//! │  │              Protocol Detectors (Priority Order)            ││
//! │  │  ┌───────────────┬──────────────┬───────────────────────┐  ││
//! │  │  │ArrowFlightDet │  GrpcDetector│    RestDetector       │  ││
//! │  │  │  (priority:110)│ (priority:100)│   (priority:10)      │  ││
//! │  │  └───────┬───────┴──────┬───────┴──────────┬────────────┘  ││
//! │  └──────────┼──────────────┼──────────────────┼───────────────┘│
//! │             ▼              ▼                  ▼                 │
//! │  ┌─────────────────────────────────────────────────────────────┐│
//! │  │              MultiplexService (Tower Service)               ││
//! │  │  ┌────────────────┬────────────────┬────────────────────┐  ││
//! │  │  │ArrowFlightHdlr │  GrpcHandler   │    RestHandler     │  ││
//! │  │  │   (Flight)     │   (Tonic)      │     (Axum)         │  ││
//! │  │  └───────┬────────┴───────┬────────┴─────────┬──────────┘  ││
//! │  └──────────┼────────────────┼──────────────────┼─────────────┘│
//! │             └────────────────┼──────────────────┘               │
//! │                              ▼                                  │
//! │              ┌────────────────────────────────┐                 │
//! │              │       UnifiedHandlers          │                 │
//! │              │    (Shared Business Logic)     │                 │
//! │              └────────────────────────────────┘                 │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## SOLID Principles
//!
//! | Principle | Implementation |
//! |-----------|----------------|
//! | **S**ingle Responsibility | Each detector/handler does one job |
//! | **O**pen/Closed | Add protocols via `add_handler()` without modifying core |
//! | **L**iskov Substitution | All handlers implement `ProtocolHandler` trait |
//! | **I**nterface Segregation | Minimal traits: `ProtocolDetector`, `ProtocolHandler` |
//! | **D**ependency Inversion | Depend on traits, not concrete implementations |
//!
//! ## Usage
//!
//! ```ignore
//! use proximadb::network::multiplex::{
//!     MultiplexServiceBuilder, DetectedProtocol,
//!     detectors::{GrpcDetector, ArrowFlightDetector, RestDetector},
//! };
//!
//! // Build the multiplexer with detectors and handlers
//! let service = MultiplexServiceBuilder::new()
//!     .add_detector(ArrowFlightDetector::new())   // Priority: 110
//!     .add_detector(GrpcDetector::new())          // Priority: 100
//!     .add_detector(RestDetector::new())          // Priority: 10
//!     .add_handler(arrow_flight_handler)
//!     .add_handler(grpc_handler)
//!     .add_handler(rest_handler)
//!     .with_fallback(DetectedProtocol::Rest)
//!     .build();
//!
//! // Run unified server
//! let server = UnifiedServer::new(service);
//! server.serve("0.0.0.0:5678").await?;
//! ```
//!
//! ## Protocol Detection
//!
//! | Protocol | Detection Method | Priority |
//! |----------|------------------|----------|
//! | Arrow Flight | Path: `/arrow.flight.protocol.*` | 110 |
//! | gRPC | Content-Type: `application/grpc*` | 100 |
//! | REST | Default fallback | 10 |
//!
//! ## Performance
//!
//! | Metric | Expected Impact |
//! |--------|-----------------|
//! | Protocol Detection | +10-50μs per request |
//! | Memory | -100KB (single listener) |
//! | TLS | Single termination point |
//!
//! ## Future Extensibility
//!
//! Adding a new protocol (e.g., WebSocket):
//!
//! ```ignore
//! // 1. Add DetectedProtocol variant (already exists)
//! // 2. Implement detector
//! pub struct WebSocketDetector;
//! impl ProtocolDetector for WebSocketDetector { ... }
//!
//! // 3. Implement handler
//! pub struct WebSocketHandler;
//! impl ProtocolHandler for WebSocketHandler { ... }
//!
//! // 4. Register
//! builder.add_detector(WebSocketDetector);
//! builder.add_handler(WebSocketHandler);
//! ```

pub mod builder;
pub mod detectors;
pub mod handlers;
pub mod protocol_multiplexer;
pub mod service;
pub mod tcp_multiplexer;
pub mod traits;

// Re-export main types
pub use builder::{BuilderError, CommonMultiplexBuilder, MultiplexServiceBuilder};
pub use protocol_multiplexer::{UnifiedServer, UnifiedServerConfig};
pub use service::{MultiplexConfig, MultiplexFuture, MultiplexService, MultiplexStats};
pub use tcp_multiplexer::{TcpMultiplexConfig, TcpMultiplexer, TcpProtocol};
pub use traits::{
    BoxResponseFuture, DetectedProtocol, DetectionResult, HandlerResult, MultiplexError,
    ProtocolDetector, ProtocolHandler, SharedDetector, SharedHandler,
};
