// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Cluster RPC abstraction layer — inter-node communication plumbing.
//!
//! Moved from the root `src/cluster/rpc/` (TD-DECOMP-3, root-monolith
//! decomposition). This is the self-contained abstraction core — error types,
//! RPC type definitions, transport/fanout/sink traits, and the retry/circuit-
//! breaker executor. Deps are only `serde`/`async-trait`/`futures`/`rand` (no
//! `tonic`/`proto`, no upward edge into the root). The tonic/proto-backed impl
//! (`connection`, `grpc_client`, `grpc_server`) remains in the root and consumes
//! these via the root `src/cluster/rpc/mod.rs` re-export.
//!
//! Layering: `proximadb-runtime` is platform-tier and may depend on every layer
//! except root/application/binding, so hosting this RPC layer here is legal.

pub mod error;
pub mod retry;
pub mod traits;
pub mod types;
