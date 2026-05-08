//! Compatibility shim for the unified Cypher parser entry points.
//!
//! The canonical parser implementation now lives in the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

pub use proximadb_graph::query::unified_parser::*;
