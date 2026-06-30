// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Unified-Port TCP Multiplexing
//!
//! [`TcpMultiplexer`] serves REST, gRPC, and Arrow Flight from a single TCP port
//! (5678) by peeking the first bytes of each connection to classify the protocol
//! ([`TcpProtocol`]) and forwarding the stream to the matching internal server.
//!
//! This is the only live multiplexer. The historical detector/handler
//! `MultiplexService` stack (`builder`/`detectors`/`handlers`/
//! `protocol_multiplexer`/`service`/`traits` — including the `RestHandlerConfig`
//! that held an `Arc<UnifiedHandlers>`) was production-unreachable (the live
//! wiring in `multi_server` used only `TcpMultiplexer`) and has been removed
//! (TD-104 S3-f dead-code cleanup).

pub mod tcp_multiplexer;

pub use tcp_multiplexer::{TcpMultiplexConfig, TcpMultiplexer, TcpProtocol};
