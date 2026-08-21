// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! HTTP middleware extracted from the root `network/middleware` module (TD-DECOMP-22).
//!
//! [`cors`] builds the CORS layer ([`cors::create_cors_layer`], [`cors::CorsConfig`]);
//! [`rate_limit`] holds the per-client [`rate_limit::RateLimitState`] token bucket and
//! the [`rate_limit::get_client_ip`] helper. [`client_addr`] resolves the client address
//! itself (TD-TENANT-4) — from the observed transport peer, with a forwarded header
//! honored only from a declared trusted proxy. All are pure over `axum`/`tower-http`/
//! `serde`/`tokio`/`tracing`/`ipnet` (no `proximadb_*` deps), keeping them a clean
//! platform-tier leaf and letting their inline tests run in this crate's own binary.

pub mod backpressure;
pub mod client_addr;
pub mod cors;
pub mod rate_limit;
pub mod timeout;

pub use client_addr::{ClientAddr, ClientAddrSource, TrustedProxies};
