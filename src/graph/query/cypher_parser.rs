//! Compatibility shim for the recursive-descent Cypher parser.
//!
//! The canonical parser implementation now lives in the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

pub use proximadb_graph::query::cypher_parser::*;
