// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Entity Store Module
//!
//! This module provides the storage layer for SKS (Semantic Knowledge Store).

// Legacy implementation (split storage)
#[path = "../entity_store_legacy.rs"]
mod legacy;

// New graph-first implementation
pub mod graph_schema;
pub mod orion_backend;
pub mod migration;

// Re-export legacy types for backward compatibility
pub use legacy::{
    CsrRelationsStore, EntityHeader, EntityStore, InMemoryProvenanceRegistry,
    ProximaEntityStore, ProvenanceRegistry, RelationsStore,
};

// Re-export graph-first types
pub use graph_schema::{EntityNodeMapper, RelationEdgeMapper};
pub use orion_backend::OrionBackedEntityStore;
