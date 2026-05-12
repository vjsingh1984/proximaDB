//! Compatibility shim for the recursive-descent Cypher parser.
//!
//! The canonical parser implementation is being migrated to the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

// TODO: Move implementation from src/graph/query to proximadb-graph crate
// pub use proximadb_graph::query::cypher_parser::*;

use serde::{Deserialize, Serialize};

use super::cypher_ast::{
    CreateClause, DeleteClause, MergeClause, PropertyProjection, RemoveClause, SetClause,
    WithClause,
};
use crate::graph::query::ast::CompiledPattern;

/// Cypher query parser
#[derive(Debug, Clone)]
pub struct CypherParser;

impl CypherParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&self, _query: &str) -> Result<CypherStatement, String> {
        // Placeholder implementation
        Ok(CypherStatement::default())
    }
}

impl Default for CypherParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Cypher statement AST node
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CypherStatement {
    pub clauses: Vec<CypherClause>,
    pub reading_clauses: Vec<ReadingClause>,
    pub updating_clauses: Vec<UpdatingClause>,
    pub with_clauses: Vec<WithClause>,
    pub union_clauses: Vec<CypherStatement>,
    pub return_spec: Option<ReturnClause>,
}

impl CypherStatement {
    /// Returns true when the statement has no write clauses.
    ///
    /// Cypher/GQL read routing and PostgreSQL wire read-only transaction handling both
    /// classify statements by side effects, not by result shape. MATCH, OPTIONAL MATCH,
    /// WITH, RETURN, ORDER BY, SKIP, and LIMIT are read-only. CREATE, DELETE, SET,
    /// REMOVE, and MERGE are write clauses.
    pub fn is_read_only(&self) -> bool {
        self.updating_clauses.is_empty()
            && self.union_clauses.iter().all(CypherStatement::is_read_only)
    }

    pub fn has_create(&self) -> bool {
        self.updating_clauses
            .iter()
            .any(|clause| matches!(clause, UpdatingClause::Create(_)))
            || self.union_clauses.iter().any(CypherStatement::has_create)
    }

    pub fn has_delete(&self) -> bool {
        self.updating_clauses
            .iter()
            .any(|clause| matches!(clause, UpdatingClause::Delete(_)))
            || self.union_clauses.iter().any(CypherStatement::has_delete)
    }

    pub fn has_merge(&self) -> bool {
        self.updating_clauses
            .iter()
            .any(|clause| matches!(clause, UpdatingClause::Merge(_)))
            || self.union_clauses.iter().any(CypherStatement::has_merge)
    }

    pub fn has_set(&self) -> bool {
        self.updating_clauses
            .iter()
            .any(|clause| matches!(clause, UpdatingClause::Set(_)))
            || self.union_clauses.iter().any(CypherStatement::has_set)
    }

    pub fn has_remove(&self) -> bool {
        self.updating_clauses
            .iter()
            .any(|clause| matches!(clause, UpdatingClause::Remove(_)))
            || self.union_clauses.iter().any(CypherStatement::has_remove)
    }
}

/// Cypher clause (MATCH, RETURN, WHERE, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CypherClause {
    Match(MatchClause),
    Return(ReturnClause),
    // TODO: Add other clause types
}

/// MATCH clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchClause {
    pub patterns: Vec<CompiledPattern>,
}

/// RETURN clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnClause {
    pub items: Vec<String>,
    pub variables: Option<Vec<String>>,
    pub projections: Option<Vec<PropertyProjection>>,
    pub distinct: bool,
    pub order_by: Vec<String>,
    pub limit: Option<usize>,
    pub skip: Option<usize>,
}

/// Reading clause (MATCH, OPTIONAL MATCH, WITH).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReadingClause {
    Match {
        pattern: CompiledPattern,
        optional: bool,
    },
    With(WithClause),
}

/// Updating clause as defined by Cypher write semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UpdatingClause {
    Create(CreateClause),
    Delete(DeleteClause),
    Set(SetClause),
    Remove(RemoveClause),
    Merge(MergeClause),
}
