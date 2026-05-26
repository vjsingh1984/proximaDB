//! Apache Polaris Catalog Backend — re-export shim.
//!
//! Implementation lives in the `proximadb-catalog` workspace crate
//! (`proximadb_catalog::polaris`). This module preserves the legacy
//! `crate::catalog::polaris::PolarisCatalog` import path during the Phase 9
//! migration. New code should import from `proximadb_catalog::polaris` directly.

pub use proximadb_catalog::polaris::{PolarisCatalog, PolarisCatalogConfig};
