//! Shared generic foundation traits and helper types.
//!
//! These are low-level contracts intended to stay leaf-like so higher-level
//! storage, query, graph, and service crates can reuse them without pulling in
//! runtime or transport implementations.

pub mod base_traits;
pub mod conversion;
pub mod generic_types;

pub use base_traits::*;
pub use conversion::*;
pub use generic_types::*;
