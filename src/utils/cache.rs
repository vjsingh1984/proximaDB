//! Compatibility re-export for generic runtime cache utilities.
//!
//! The implementation lives in `proximadb-runtime-common` so storage, query,
//! core search, and future platform/runtime crates can share it without adding
//! upward dependencies into the root crate.

pub use proximadb_runtime_common::cache::*;
