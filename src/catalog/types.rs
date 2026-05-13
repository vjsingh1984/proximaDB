//! Compatibility re-exports for xCatalog contract types.
//!
//! Canonical definitions live in `proximadb-catalog` so catalog, query, API, and storage layers can
//! depend on catalog contracts without pulling in the root runtime crate.

pub use proximadb_catalog::*;
