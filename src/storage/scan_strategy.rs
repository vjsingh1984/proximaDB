//! Re-export shim — the unified scan-strategy surface has been hoisted to the
//! `proximadb-storage-ports` crate (`scan_strategy` module) as a root-crate
//! decomposition slice. It is a pure foundation-typed contract (the `ScanStrategy`
//! enum, the `ScanIterator` / `UnifiedScanEngine` traits, `ScanStatistics`, …) that
//! every engine implements, so it belongs with the other engine-port traits; the
//! `crate::storage::scan_strategy::*` paths resolve unchanged through this glob.
//!
//! See `docs/12-design/ROOT_CRATE_DECOMPOSITION_ENGINES_EXTRACTION_2026_07_12.adoc`.

pub use proximadb_storage_ports::scan_strategy::*;
