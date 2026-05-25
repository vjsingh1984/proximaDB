//! Catalog Cache — re-export shim.
//!
//! The cache implementation lives in the `proximadb-catalog` workspace crate
//! (`proximadb_catalog::cache`). This module exists only to preserve the
//! `crate::catalog::cache::CatalogCache` import path used by the legacy
//! `src/catalog/*` backends during the Phase 9 migration. New code should
//! import from `proximadb_catalog::cache` directly.

pub use proximadb_catalog::cache::{CacheStats, CatalogCache};
