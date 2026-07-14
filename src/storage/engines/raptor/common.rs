//! Re-export shim — the RAPTOR common types have been hoisted to the
//! `proximadb-raptor-common` crate as the raptor cycle-hub inversion (unblocks the
//! raptor engine extraction). All `crate::storage::engines::raptor::common::*` paths
//! (the 2 direct importers + the `Predicate` re-export in `mod.rs`, used widely)
//! resolve unchanged through this glob.
//!
//! See `docs/12-design/ROOT_CRATE_DECOMPOSITION_ENGINES_EXTRACTION_2026_07_12.adoc`.

pub use proximadb_raptor_common::*;
