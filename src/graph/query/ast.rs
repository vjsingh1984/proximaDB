/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Abstract Syntax Tree (AST) for Graph Query Patterns
//!
//! This module defines the data structures representing the Abstract Syntax Tree (AST)
//! for Cypher-like graph query patterns. These structures are used by the parser
//! to represent a parsed query and by the planner and executor for processing.

use crate::graph::{Edge, Node};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// A compiled pattern ready for execution
#[derive(Debug, Clone)]
pub struct CompiledPattern {
    /// Pattern nodes
    pub nodes: Vec<NodePattern>,
    /// Pattern edges  
    pub edges: Vec<EdgePattern>,
    /// Path patterns (variable length)
    pub paths: Vec<PathPattern>,
    /// Where clauses
    pub where_clauses: Vec<WhereClause>,
    /// Return specification
    pub return_spec: ReturnSpec,
    /// Pattern variables (for binding)
    pub variables: HashMap<String, VariableBinding>,
}

/// Node pattern specification
#[derive(Debug, Clone)]
pub struct NodePattern {
    /// Variable name (e.g., 'n' in (n:Person))
    pub variable: String,
    /// Node labels (e.g., ['Person', 'Employee'])
    pub labels: Vec<String>,
    /// Property constraints
    pub properties: HashMap<String, PropertyConstraint>,
    /// Whether this is an optional match
    pub optional: bool,
}

/// Edge pattern specification  
#[derive(Debug, Clone)]
pub struct EdgePattern {
    /// Variable name (e.g., 'r' in -[r:KNOWS]->)
    pub variable: Option<String>,
    /// Source node variable
    pub from_variable: String,
    /// Target node variable
    pub to_variable: String,
    /// Edge types (e.g., ['KNOWS', 'FRIENDS_WITH'])
    pub edge_types: Vec<String>,
    /// Property constraints
    pub properties: HashMap<String, PropertyConstraint>,
    /// Direction (incoming, outgoing, bidirectional)
    pub direction: EdgeDirection,
    /// Whether this is an optional match
    pub optional: bool,
}

/// Path pattern for variable-length paths
#[derive(Debug, Clone)]
pub struct PathPattern {
    /// Variable name for the path
    pub variable: String,
    /// Source node variable
    pub from_variable: String,
    /// Target node variable
    pub to_variable: String,
    /// Edge types to follow
    pub edge_types: Vec<String>,
    /// Minimum path length
    pub min_length: u32,
    /// Maximum path length
    pub max_length: u32,
    /// Direction
    pub direction: EdgeDirection,
}

/// Property constraint in patterns
#[derive(Debug, Clone)]
pub enum PropertyConstraint {
    /// Exact value match
    Equals(serde_json::Value),
    /// Not equals
    NotEquals(serde_json::Value),
    /// Greater than
    GreaterThan(serde_json::Value),
    /// Greater than or equal
    GreaterThanOrEqual(serde_json::Value),
    /// Less than
    LessThan(serde_json::Value),
    /// Less than or equal
    LessThanOrEqual(serde_json::Value),
    /// Value in list
    In(Vec<serde_json::Value>),
    /// Value not in list
    NotIn(Vec<serde_json::Value>),
    /// String contains
    Contains(String),
    /// String starts with
    StartsWith(String),
    /// String ends with
    EndsWith(String),
    /// Regular expression match
    Regex(String),
    /// Property exists
    Exists,
    /// Property does not exist
    NotExists,
}

/// Edge direction specification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeDirection {
    /// Outgoing edge: (a)-[]->(b)
    Outgoing,
    /// Incoming edge: (a)<-[]-(b)
    Incoming,
    /// Bidirectional: (a)-[]-(b)
    Bidirectional,
}

/// Where clause for additional filtering
#[derive(Debug, Clone)]
pub struct WhereClause {
    /// Variable name
    pub variable: String,
    /// Property name
    pub property: String,
    /// Constraint
    pub constraint: PropertyConstraint,
    /// Logical operator for combining with next clause
    pub logical_op: Option<LogicalOperator>,
}

/// Logical operators for WHERE clauses
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum LogicalOperator {
    And,
    Or,
}

/// Return specification
#[derive(Debug, Clone)]
pub struct ReturnSpec {
    /// Variables to return
    pub variables: Vec<String>,
    /// Property projections (variable.property)
    pub projections: Vec<PropertyProjection>,
    /// Whether to return distinct results
    pub distinct: bool,
    /// Order by specifications
    pub order_by: Vec<OrderBy>,
    /// Limit
    pub limit: Option<u32>,
    /// Skip/offset
    pub skip: Option<u32>,
}

/// Property projection in RETURN clause
#[derive(Debug, Clone)]
pub struct PropertyProjection {
    /// Variable name
    pub variable: String,
    /// Property name
    pub property: String,
    /// Alias for the projection
    pub alias: Option<String>,
}

/// Order by specification
#[derive(Debug, Clone)]
pub struct OrderBy {
    /// Variable name
    pub variable: String,
    /// Property name (optional)
    pub property: Option<String>,
    /// Ascending or descending
    pub ascending: bool,
}

/// Variable binding during pattern matching
#[derive(Debug, Clone)]
pub enum VariableBinding {
    /// Bound to a specific node
    Node(Arc<Node>),
    /// Bound to a specific edge
    Edge(Arc<Edge>),
    /// Bound to a path (sequence of nodes and edges)
    Path(Vec<PathElement>),
}

/// Element in a path
#[derive(Debug, Clone)]
pub enum PathElement {
    Node(Arc<Node>),
    Edge(Arc<Edge>),
}

/// Pattern matching result
#[derive(Debug, Clone)]
pub struct MatchResult {
    /// Variable bindings for this match
    pub bindings: HashMap<String, VariableBinding>,
    /// Score/confidence of the match (0.0 to 1.0)
    pub score: f64,
}

/// Helper struct for path finding
#[derive(Debug, Clone)]
pub struct FoundPath {
    pub elements: Vec<PathElement>,
    pub length: u32,
}
