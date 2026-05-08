//! Compatibility shim for Cypher AST types.
//!
//! The canonical Cypher AST implementation now lives in the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

pub use proximadb_graph::query::cypher_ast::*;
