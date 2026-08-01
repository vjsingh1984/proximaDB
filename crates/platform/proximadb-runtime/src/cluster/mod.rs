// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Cluster core — moved from the root `src/cluster/` (TD-DECOMP-5, root-monolith
//! decomposition). Self-contained cluster subsystem pieces with no upward edge
//! into the root: shard metadata, the cache-affinity registry, the node registry,
//! and the metadata service. Deps only `proximadb-config` + external crates.
//!
//! `consensus`/`replication`/`distributed_ops` (which reach the rpc connection/
//! grpc layer) follow in a later slice once the tonic rpc impl lands here.
//!
//! Layering: `proximadb-runtime` is platform-tier and may depend on every layer
//! except root/application/binding.

pub mod cache_affinity;
pub mod consensus;
pub mod metadata_service;
pub mod node_registry;
pub mod shard;
