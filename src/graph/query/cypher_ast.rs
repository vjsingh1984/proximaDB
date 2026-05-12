//! Compatibility shim for Cypher AST types.
//!
//! The canonical Cypher AST implementation is being migrated to the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

// TODO: Move implementation from src/graph/query to proximadb-graph crate
// pub use proximadb_graph::query::cypher_ast::*;

use super::ast::EdgeDirection;
use serde::{Deserialize, Serialize};

// Re-export from cypher_parser to avoid duplication
pub use super::cypher_parser::{
    CypherClause, CypherStatement, MatchClause, ReadingClause, ReturnClause, UpdatingClause,
};

/// WHERE clause conditions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WhereClause {
    Property {
        variable: String,
        property: String,
        constraint: PropertyConstraint,
    },
    And(Box<WhereClause>, Box<WhereClause>),
    Or(Box<WhereClause>, Box<WhereClause>),
    Not(Box<WhereClause>),
    Exists(String),
}

/// Property constraint for WHERE clauses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PropertyConstraint {
    Equals(serde_json::Value),
    NotEquals(serde_json::Value),
    GreaterThan(serde_json::Value),
    GreaterOrEqual(serde_json::Value),
    LessThan(serde_json::Value),
    LessOrEqual(serde_json::Value),
    In(Vec<serde_json::Value>),
    Contains(String),
    StartsWith(String),
    EndsWith(String),
    Regex(String),
    NotExists,
    Exists,
}

/// Property projection for RETURN clauses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PropertyProjection {
    Variable(String),
    Property { variable: String, property: String },
    Count,
    Sum { variable: String, property: String },
    Avg { variable: String, property: String },
    Min { variable: String, property: String },
    Max { variable: String, property: String },
}

/// CREATE node specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateNodeSpec {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub properties: std::collections::HashMap<String, serde_json::Value>,
}

/// CREATE edge specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEdgeSpec {
    pub variable: Option<String>,
    pub from_variable: Option<String>,
    pub to_variable: Option<String>,
    pub edge_type: Option<String>,
    pub properties: std::collections::HashMap<String, serde_json::Value>,
    pub direction: EdgeDirection,
}

/// CREATE clause containing nodes and edges
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateClause {
    pub nodes: Vec<CreateNodeSpec>,
    pub edges: Vec<CreateEdgeSpec>,
}

/// DELETE clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteClause {
    pub variables: Vec<String>,
    pub detach: bool,
}

/// SET clause item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SetItem {
    Property {
        variable: String,
        property: String,
        value: serde_json::Value,
    },
    AddLabel {
        variable: String,
        label: String,
    },
    MergeProperties {
        variable: String,
        properties: std::collections::HashMap<String, serde_json::Value>,
    },
    AllProperties {
        variable: String,
        properties: std::collections::HashMap<String, serde_json::Value>,
    },
}

/// SET clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetClause {
    pub items: Vec<SetItem>,
}

/// REMOVE clause item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RemoveItem {
    Property { variable: String, property: String },
    Label { variable: String, label: String },
}

/// REMOVE clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveClause {
    pub items: Vec<RemoveItem>,
}

/// MERGE clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeClause {
    pub pattern: super::ast::CompiledPattern,
    pub on_create: Option<SetClause>,
    pub on_match: Option<SetClause>,
}

/// WITH clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithClause {
    pub projections: Vec<PropertyProjection>,
    pub distinct: bool,
    pub where_clause: Option<WhereClause>,
    pub order_by: Vec<String>,
    pub limit: Option<usize>,
    pub skip: Option<usize>,
}
