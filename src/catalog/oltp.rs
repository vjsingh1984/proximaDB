//! OLTP Catalog Backend — re-export shim.
//!
//! The OLTP catalog implementation lives in the `proximadb-catalog` workspace
//! crate (`proximadb_catalog::oltp`). This module exists only to preserve the
//! `crate::catalog::oltp::OltpCatalog` import path for the legacy
//! `src/catalog/*` modules during the Phase 9 migration. New code should
//! import from `proximadb_catalog::oltp` directly.

pub use proximadb_catalog::oltp::{OltpBackend, OltpCatalog, OltpCatalogConfig};
