//! Hive Metastore Catalog Backend — re-export shim.
//!
//! Implementation lives in the `proximadb-catalog` workspace crate
//! (`proximadb_catalog::hive`). This module preserves the legacy
//! `crate::catalog::hive::HiveCatalog` import path during the Phase 9
//! migration. New code should import from `proximadb_catalog::hive` directly.

pub use proximadb_catalog::hive::{HiveCatalog, HiveCatalogConfig};
