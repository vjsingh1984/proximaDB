//! Compression Module - compatibility re-export shim
//!
//! Ownership has moved to the `proximadb-compression` horizontal crate.
//! All types and functions are re-exported here for backward compatibility
//! while consumers migrate to `proximadb_compression` directly.

pub use proximadb_compression::*;
