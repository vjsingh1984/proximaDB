//! Apache Iceberg Catalog Backend — re-export shim.
//!
//! Implementation lives in the `proximadb-catalog` workspace crate
//! (`proximadb_catalog::iceberg`). This module preserves the legacy
//! `crate::catalog::iceberg::IcebergCatalog` import path during the Phase 9
//! migration. New code should import from `proximadb_catalog::iceberg` directly.

pub use proximadb_catalog::iceberg::{IcebergBackend, IcebergCatalog, IcebergCatalogConfig};
