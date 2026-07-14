//! Re-export shim — `raptor/config` has been hoisted into the
//! `proximadb-raptor-common` crate (alongside `common`). All
//! `crate::storage::engines::raptor::config::*` paths resolve unchanged.
pub use proximadb_raptor_common::config::*;
