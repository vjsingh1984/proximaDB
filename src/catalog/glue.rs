//! AWS Glue Catalog Backend — re-export shim.
//!
//! Implementation lives in the `proximadb-catalog` workspace crate
//! (`proximadb_catalog::glue`). This module preserves the legacy
//! `crate::catalog::glue::GlueCatalog` import path during the Phase 9
//! migration. New code should import from `proximadb_catalog::glue` directly.

pub use proximadb_catalog::glue::{GlueCatalog, GlueCatalogConfig};
