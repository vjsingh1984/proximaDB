//! Compatibility re-export for runtime proto defaults.
//!
//! Runtime defaults inspect host capacity and emit diagnostics, so the behavior
//! lives in the platform runtime crate rather than generated proto contracts.

pub use proximadb_runtime::proto_defaults::*;
