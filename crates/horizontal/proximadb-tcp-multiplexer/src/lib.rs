// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! TCP port multiplexer, extracted from the root `network/multiplex` module
//! (TD-DECOMP-46).
//!
//! [`tcp_multiplexer`] serves REST, gRPC, and Arrow Flight from a single TCP
//! port by sniffing the first bytes of each connection (HTTP/1, HTTP/2 prior-
//! knowledge, gRPC) and routing accordingly. [`tcp_multiplexer::TcpMultiplexer`]
//! is the accept loop; [`tcp_multiplexer::TcpMultiplexConfig`] /
//! [`tcp_multiplexer::TcpProtocol`] configure it. Depends only on `tokio` +
//! `tracing`, keeping it a clean horizontal-tier leaf.

pub mod tcp_multiplexer;
