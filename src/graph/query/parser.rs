//! Compatibility shim for the legacy graph-pattern parser.
//!
//! The canonical parser implementation now lives in the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

pub use proximadb_graph::query::parser::*;
