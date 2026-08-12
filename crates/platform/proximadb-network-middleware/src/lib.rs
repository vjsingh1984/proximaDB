// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! HTTP middleware extracted from the root `network/middleware` module (TD-DECOMP-22).
//!
//! [`cors`] builds the CORS layer ([`cors::create_cors_layer`], [`cors::CorsConfig`]);
//! [`rate_limit`] holds the per-client [`rate_limit::RateLimitState`] token bucket and
//! the [`rate_limit::get_client_ip`] helper. Both are pure over `axum`/`tower-http`/
//! `serde`/`tokio`/`tracing` (no `proximadb_*` deps), keeping them a clean platform-tier
//! leaf and letting their 46 inline tests run in this crate's own binary.

pub mod backpressure;
pub mod cors;
pub mod rate_limit;
pub mod timeout;
