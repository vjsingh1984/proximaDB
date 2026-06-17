//! Memory pool compatibility shim.
//!
//! The reusable pool implementation is owned by the horizontal
//! `proximadb-runtime-common` crate. Root-crate paths re-export it here for
//! compatibility while consumers migrate to `proximadb_runtime_common::pool`.

pub use proximadb_runtime_common::pool::*;
