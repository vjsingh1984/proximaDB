//! Re-export shim — the consolidated RAPTOR reader has been hoisted to the
//! `proximadb-raptor-engine` crate (the first raptor leaf moved out of the root).
//! All `crate::storage::engines::raptor::consolidated_reader::*` paths resolve
//! unchanged through this glob.
//!
//! See `docs/12-design/ROOT_CRATE_DECOMPOSITION_ENGINES_EXTRACTION_2026_07_12.adoc`.

pub use proximadb_raptor_engine::*;
