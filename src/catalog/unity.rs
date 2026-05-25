//! Databricks Unity Catalog Backend — re-export shim.
//!
//! Implementation lives in the `proximadb-catalog` workspace crate
//! (`proximadb_catalog::unity`). This module preserves the legacy
//! `crate::catalog::unity::UnityCatalog` import path during the Phase 9
//! migration. New code should import from `proximadb_catalog::unity` directly.

pub use proximadb_catalog::unity::{UnityCatalog, UnityCatalogConfig};
