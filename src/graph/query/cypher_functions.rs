//! Compatibility shim for Cypher function support.
//!
//! The canonical Cypher function registry and evaluator now live in the
//! `proximadb-graph` workspace crate. This module preserves the historical
//! root import surface.

pub use proximadb_graph::query::cypher_functions::*;
