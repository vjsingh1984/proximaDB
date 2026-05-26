//! Delta Lake Catalog Backend — re-export shim.
//!
//! Implementation lives in the `proximadb-catalog` workspace crate
//! (`proximadb_catalog::delta`). This module preserves the legacy
//! `crate::catalog::delta::DeltaCatalog` import path during the Phase 9
//! migration. New code should import from `proximadb_catalog::delta` directly.

pub use proximadb_catalog::delta::{DeltaCatalog, DeltaCatalogConfig};
