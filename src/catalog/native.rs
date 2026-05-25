//! Native Catalog Backend — re-export shim.
//!
//! Implementation lives in the `proximadb-catalog` workspace crate
//! (`proximadb_catalog::native`). This module preserves the legacy
//! `crate::catalog::native::NativeCatalog` import path during the Phase 9
//! migration. New code should import from `proximadb_catalog::native` directly.

pub use proximadb_catalog::native::{NativeCatalog, NativeCatalogConfig};
