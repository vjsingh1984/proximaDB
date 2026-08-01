// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Cluster RPC abstraction layer — inter-node communication plumbing.
//!
//! Moved from the root `src/cluster/rpc/` (TD-DECOMP-3/4, root-monolith
//! decomposition). This is the self-contained abstraction core — error types,
//! RPC type definitions, transport/fanout/sink traits, and the retry/circuit-
//! breaker executor (deps `serde`/`async-trait`/`futures`/`rand`) — plus the
//! tonic-backed client impl (`connection`, `grpc_client`; deps add `tonic`/
//! `dashmap`, proto via `proximadb-proto`). Only `grpc_server` remains root-side
//! (it couples to `crate::cluster::consensus`/`replication`); it consumes these
//! via the root `src/cluster/rpc/mod.rs` re-export.
//!
//! Layering: `proximadb-runtime` is platform-tier and may depend on every layer
//! except root/application/binding, so hosting this RPC layer here is legal.

pub mod connection;
pub mod error;
pub mod grpc_client;
pub mod retry;
pub mod traits;
pub mod types;

// Convenience re-exports (mirror root src/cluster/rpc/mod.rs) so moved callers
// (e.g. `cluster::consensus`) can name rpc items at the cluster_rpc root.
pub use {connection::*, error::*, retry::*, traits::*, types::*};
