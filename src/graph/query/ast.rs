//! Compatibility shim for graph query AST types.
//!
//! The canonical AST implementation is being migrated to the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

// TODO: Move implementation from src/graph/query to proximadb-graph crate
// pub use proximadb_graph::query::ast::*;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::cypher_ast::WhereClause;

/// Compiled pattern for graph matching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledPattern {
    pub nodes: Vec<PatternNode>,
    pub edges: Vec<PatternEdge>,
    pub paths: Vec<String>,
    pub where_clause: Option<WhereClause>,
    pub where_clauses: Vec<WhereClause>,
    pub with_clauses: Vec<String>,
}

/// Pattern node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternNode {
    pub id: String,
    pub variable: String,
    pub labels: Vec<String>,
    pub label: Option<String>,
    pub properties: HashMap<String, serde_json::Value>,
    pub optional: bool,
}

/// Pattern edge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternEdge {
    pub variable: Option<String>,
    pub from_variable: String,
    pub to_variable: String,
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub edge_types: Vec<String>,
    pub direction: EdgeDirection,
    pub properties: HashMap<String, serde_json::Value>,
    pub optional: bool,
}

/// Edge direction in pattern
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeDirection {
    Outgoing,
    Incoming,
    Both,
    Bidirectional,
}

/// Result of pattern matching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchResult {
    pub bindings: HashMap<String, GraphNode>,
    pub paths: Vec<FoundPath>,
}

/// Graph node in match result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
}

/// Path found during traversal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoundPath {
    pub nodes: Vec<String>,
    pub edges: Vec<String>,
}

/// Result type for query operations
pub type QueryResult<T> = Result<T, String>;
