//! Shared foundational contracts for ProximaDB.
//!
//! This crate is intentionally small. It holds leaf-level types that many
//! higher-level crates depend on and that should not pull in storage, query,
//! transport, or binding implementations.

pub mod encoding;
pub mod error;
pub mod foundation;

pub use encoding::*;
pub use error::*;
pub use foundation::*;
