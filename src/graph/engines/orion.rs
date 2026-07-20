/*
 * Copyright 2025 Vijaykumar Singh
 * (Apache-2.0)
 */

//! Shim: the ORION engine now lives in the `proximadb-orion-engine` crate
//! (ORION extraction, 6g). Re-exported here so existing
//! `crate::graph::engines::orion::*` paths resolve unchanged.
pub use proximadb_orion_engine::*;
