//! Compatibility re-export for shared UUID primitives.
//!
//! The implementation lives in `proximadb-kernel` because UUID generation and
//! parsing are foundation-level ID concerns used across storage, graph, network,
//! services, and tests.

pub use proximadb_kernel::uuid::*;
