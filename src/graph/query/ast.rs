//! Compatibility shim for graph query AST types.
//!
//! The canonical AST implementation now lives in the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

pub use proximadb_graph::query::ast::*;
