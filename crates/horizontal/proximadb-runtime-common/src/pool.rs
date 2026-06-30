//! Memory pooling infrastructure for ProximaDB.
//!
//! The implementation now lives in the foundation-tier `proximadb-memory-pool`
//! crate so that foundation crates (e.g. `proximadb-distance-kernel`) can depend
//! on it without an upward (foundation -> horizontal) layering violation.
//!
//! This module re-exports the foundation crate so existing
//! `proximadb_runtime_common::pool::*` consumers remain source-compatible.

pub use proximadb_memory_pool::*;
