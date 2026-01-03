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
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyConstraint {
    /// Exact value match
    Equals(serde_json::Value),
    /// Not equals
    NotEquals(serde_json::Value),
    /// Greater than
    GreaterThan(serde_json::Value),
    /// Greater than or equal (alias for compatibility)
    GreaterThanOrEqual(serde_json::Value),
    /// Greater or equal (canonical name)
    GreaterOrEqual(serde_json::Value),
    /// Less than
    LessThan(serde_json::Value),
    /// Less than or equal (alias for compatibility)
    LessThanOrEqual(serde_json::Value),
    /// Less or equal (canonical name)
    LessOrEqual(serde_json::Value),
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

/// Where clause for additional filtering (supports complex conditions)
#[derive(Debug, Clone)]
pub enum WhereClause {
    /// Simple property constraint
    Property {
        variable: String,
        property: String,
        constraint: PropertyConstraint,
    },
    /// Logical AND of two conditions
    And(Box<WhereClause>, Box<WhereClause>),
    /// Logical OR of two conditions
    Or(Box<WhereClause>, Box<WhereClause>),
    /// Logical NOT of a condition
    Not(Box<WhereClause>),
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
    /// Property projections (variable.property) or aggregations
    pub projections: Vec<PropertyProjection>,
    /// Whether to return distinct results
    pub distinct: bool,
    /// Order by specifications (variable_name, ascending)
    pub order_by: Vec<(String, bool)>,
    /// Limit
    pub limit: Option<usize>,
    /// Skip/offset
    pub skip: Option<usize>,
}

/// Property projection in RETURN clause (supports aggregations)
#[derive(Debug, Clone)]
pub enum PropertyProjection {
    /// Simple variable (e.g., RETURN n)
    Variable(String),
    /// Property access (e.g., RETURN n.name)
    Property { variable: String, property: String },
    /// COUNT(*) aggregation
    Count,
    /// SUM(variable.property) aggregation
    Sum { variable: String, property: String },
    /// AVG(variable.property) aggregation
    Avg { variable: String, property: String },
    /// MIN(variable.property) aggregation
    Min { variable: String, property: String },
    /// MAX(variable.property) aggregation
    Max { variable: String, property: String },
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

// ==================== Mutation Operation Types ====================

/// A complete Cypher query with all clauses
#[derive(Debug, Clone)]
pub struct CypherQuery {
    /// Reading clauses (MATCH, OPTIONAL MATCH)
    pub reading_clauses: Vec<ReadingClause>,
    /// Updating clauses (CREATE, DELETE, SET, REMOVE, MERGE)
    pub updating_clauses: Vec<UpdatingClause>,
    /// WITH clauses for intermediate projections
    pub with_clauses: Vec<WithClause>,
    /// Final RETURN specification (optional for update-only queries)
    pub return_spec: Option<ReturnSpec>,
}

/// Reading clause types
#[derive(Debug, Clone)]
pub enum ReadingClause {
    /// Standard MATCH clause
    Match {
        pattern: MatchPattern,
        optional: bool,
    },
    /// UNWIND clause for list expansion
    Unwind {
        expression: String,
        variable: String,
    },
    /// CALL clause for procedure calls
    Call {
        procedure: String,
        arguments: Vec<serde_json::Value>,
        yield_items: Vec<String>,
    },
}

/// Match pattern (nodes and edges together)
#[derive(Debug, Clone)]
pub struct MatchPattern {
    /// Node patterns
    pub nodes: Vec<NodePattern>,
    /// Edge patterns
    pub edges: Vec<EdgePattern>,
    /// Path patterns
    pub paths: Vec<PathPattern>,
    /// WHERE clause for this MATCH
    pub where_clause: Option<WhereClause>,
}

/// Updating clause types
#[derive(Debug, Clone)]
pub enum UpdatingClause {
    /// CREATE clause for creating nodes/edges
    Create(CreateClause),
    /// DELETE clause for removing nodes/edges
    Delete(DeleteClause),
    /// SET clause for updating properties
    Set(SetClause),
    /// REMOVE clause for removing properties/labels
    Remove(RemoveClause),
    /// MERGE clause for create-if-not-exists
    Merge(MergeClause),
    /// FOREACH clause for iteration
    ForEach(ForEachClause),
}

/// CREATE clause for creating nodes and relationships
#[derive(Debug, Clone)]
pub struct CreateClause {
    /// Nodes to create
    pub nodes: Vec<CreateNodeSpec>,
    /// Edges to create
    pub edges: Vec<CreateEdgeSpec>,
}

/// Specification for creating a node
#[derive(Debug, Clone)]
pub struct CreateNodeSpec {
    /// Variable name for the created node
    pub variable: Option<String>,
    /// Labels for the new node
    pub labels: Vec<String>,
    /// Properties for the new node
    pub properties: HashMap<String, serde_json::Value>,
}

/// Specification for creating an edge
#[derive(Debug, Clone)]
pub struct CreateEdgeSpec {
    /// Variable name for the created edge
    pub variable: Option<String>,
    /// Source node variable
    pub from_variable: String,
    /// Target node variable
    pub to_variable: String,
    /// Edge type
    pub edge_type: String,
    /// Properties for the new edge
    pub properties: HashMap<String, serde_json::Value>,
}

/// DELETE clause for removing nodes and relationships
#[derive(Debug, Clone)]
pub struct DeleteClause {
    /// Variables to delete
    pub variables: Vec<String>,
    /// Whether to use DETACH DELETE (delete edges too)
    pub detach: bool,
}

/// SET clause for updating properties
#[derive(Debug, Clone)]
pub struct SetClause {
    /// Property updates
    pub items: Vec<SetItem>,
}

/// Individual SET item
#[derive(Debug, Clone)]
pub enum SetItem {
    /// Set a single property: SET n.name = 'Alice'
    Property {
        variable: String,
        property: String,
        value: serde_json::Value,
    },
    /// Set all properties: SET n = {name: 'Alice', age: 30}
    AllProperties {
        variable: String,
        properties: HashMap<String, serde_json::Value>,
    },
    /// Add/merge properties: SET n += {age: 31}
    MergeProperties {
        variable: String,
        properties: HashMap<String, serde_json::Value>,
    },
    /// Add label: SET n:NewLabel
    AddLabel { variable: String, label: String },
}

/// REMOVE clause for removing properties and labels
#[derive(Debug, Clone)]
pub struct RemoveClause {
    /// Items to remove
    pub items: Vec<RemoveItem>,
}

/// Individual REMOVE item
#[derive(Debug, Clone)]
pub enum RemoveItem {
    /// Remove a property: REMOVE n.property
    Property { variable: String, property: String },
    /// Remove a label: REMOVE n:Label
    Label { variable: String, label: String },
}

/// MERGE clause for create-if-not-exists pattern
#[derive(Debug, Clone)]
pub struct MergeClause {
    /// Pattern to match or create
    pub pattern: MatchPattern,
    /// Actions to perform when creating
    pub on_create: Option<SetClause>,
    /// Actions to perform when matching existing
    pub on_match: Option<SetClause>,
}

/// FOREACH clause for iteration
#[derive(Debug, Clone)]
pub struct ForEachClause {
    /// Variable name for iteration
    pub variable: String,
    /// Expression to iterate over
    pub expression: String,
    /// Updating clauses to apply
    pub updating_clauses: Vec<UpdatingClause>,
}

/// WITH clause for intermediate projections
#[derive(Debug, Clone)]
pub struct WithClause {
    /// Projection specification (same as RETURN)
    pub projections: Vec<(String, PropertyProjection)>,
    /// Whether to use DISTINCT
    pub distinct: bool,
    /// ORDER BY specifications
    pub order_by: Vec<(String, bool)>,
    /// LIMIT
    pub limit: Option<usize>,
    /// SKIP
    pub skip: Option<usize>,
    /// WHERE clause for filtering after WITH
    pub where_clause: Option<WhereClause>,
}

// ==================== Query Builder Helpers ====================

impl CypherQuery {
    /// Create a new empty query
    pub fn new() -> Self {
        Self {
            reading_clauses: Vec::new(),
            updating_clauses: Vec::new(),
            with_clauses: Vec::new(),
            return_spec: None,
        }
    }

    /// Check if this is a read-only query
    pub fn is_read_only(&self) -> bool {
        self.updating_clauses.is_empty()
    }

    /// Check if this query has any MATCH clauses
    pub fn has_match(&self) -> bool {
        self.reading_clauses
            .iter()
            .any(|c| matches!(c, ReadingClause::Match { .. }))
    }

    /// Check if this query has any CREATE clauses
    pub fn has_create(&self) -> bool {
        self.updating_clauses
            .iter()
            .any(|c| matches!(c, UpdatingClause::Create(_)))
    }

    /// Check if this query has any DELETE clauses
    pub fn has_delete(&self) -> bool {
        self.updating_clauses
            .iter()
            .any(|c| matches!(c, UpdatingClause::Delete(_)))
    }

    /// Check if this query has any MERGE clauses
    pub fn has_merge(&self) -> bool {
        self.updating_clauses
            .iter()
            .any(|c| matches!(c, UpdatingClause::Merge(_)))
    }
}

impl Default for CypherQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateClause {
    /// Create a new empty CREATE clause
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Add a node to create
    pub fn add_node(mut self, node: CreateNodeSpec) -> Self {
        self.nodes.push(node);
        self
    }

    /// Add an edge to create
    pub fn add_edge(mut self, edge: CreateEdgeSpec) -> Self {
        self.edges.push(edge);
        self
    }
}

impl Default for CreateClause {
    fn default() -> Self {
        Self::new()
    }
}

impl DeleteClause {
    /// Create a new DELETE clause
    pub fn new(variables: Vec<String>) -> Self {
        Self {
            variables,
            detach: false,
        }
    }

    /// Create a DETACH DELETE clause
    pub fn detach(variables: Vec<String>) -> Self {
        Self {
            variables,
            detach: true,
        }
    }
}

impl SetClause {
    /// Create a new empty SET clause
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Add a property set
    pub fn set_property(
        mut self,
        variable: &str,
        property: &str,
        value: serde_json::Value,
    ) -> Self {
        self.items.push(SetItem::Property {
            variable: variable.to_string(),
            property: property.to_string(),
            value,
        });
        self
    }

    /// Add a label
    pub fn add_label(mut self, variable: &str, label: &str) -> Self {
        self.items.push(SetItem::AddLabel {
            variable: variable.to_string(),
            label: label.to_string(),
        });
        self
    }
}

impl Default for SetClause {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoveClause {
    /// Create a new empty REMOVE clause
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Remove a property
    pub fn remove_property(mut self, variable: &str, property: &str) -> Self {
        self.items.push(RemoveItem::Property {
            variable: variable.to_string(),
            property: property.to_string(),
        });
        self
    }

    /// Remove a label
    pub fn remove_label(mut self, variable: &str, label: &str) -> Self {
        self.items.push(RemoveItem::Label {
            variable: variable.to_string(),
            label: label.to_string(),
        });
        self
    }
}

impl Default for RemoveClause {
    fn default() -> Self {
        Self::new()
    }
}
