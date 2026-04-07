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

//! # Compute Plan Module
//!
//! Defines the serializable compute plan representation for query execution.
//! Plans are independent of any specific compute provider and can be executed
//! by any provider that supports the required operations.
//!
//! ## Plan Structure
//!
//! ```text
//! ComputePlan
//!     ├── id: String (unique plan identifier)
//!     ├── root: PlanNode (tree of operations)
//!     ├── parameters: HashMap (runtime parameters)
//!     └── hints: PlanHints (optimizer hints)
//!
//! PlanNode (recursive tree)
//!     ├── TableScan     - Read from table/collection
//!     ├── VectorScan    - Vector similarity search
//!     ├── GraphScan     - Graph traversal
//!     ├── Filter        - Predicate filtering
//!     ├── Project       - Column projection
//!     ├── Aggregate     - Grouping and aggregation
//!     ├── Sort          - Ordering
//!     ├── Limit         - Row limiting
//!     ├── HashJoin      - Join operations
//!     ├── Union         - Set operations
//!     └── Exchange      - Data redistribution
//! ```
//!
//! ## Design Principles
//!
//! 1. **Serializable**: Plans can be serialized/deserialized for distributed execution.
//!
//! 2. **Provider-Agnostic**: Plans don't depend on specific compute engines.
//!
//! 3. **Optimizable**: Plans expose enough information for cost-based optimization.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::compute::plan::{ComputePlan, PlanNode, Expr};
//!
//! // Create a simple scan with filter
//! let plan = ComputePlan {
//!     id: "query-001".to_string(),
//!     root: PlanNode::Filter {
//!         input: Box::new(PlanNode::TableScan {
//!             table: "users".to_string(),
//!             columns: vec!["id".to_string(), "name".to_string()],
//!             filter: None,
//!         }),
//!         predicate: Expr::Binary {
//!             left: Box::new(Expr::Column("age".to_string())),
//!             op: BinaryOp::Gt,
//!             right: Box::new(Expr::Literal(LiteralValue::Int(18))),
//!         },
//!     },
//!     parameters: HashMap::new(),
//!     hints: PlanHints::default(),
//! };
//! ```

use std::collections::HashMap;
use std::ops::Not;

use serde::{Deserialize, Serialize};

// ============================================================================
// Core Plan Types
// ============================================================================

/// A complete compute plan ready for execution
///
/// The plan is a tree of `PlanNode` operations rooted at `root`.
/// Parameters can be injected at runtime and hints provide
/// guidance to the optimizer/executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputePlan {
    /// Unique plan identifier
    pub id: String,

    /// Root node of the plan tree
    pub root: PlanNode,

    /// Runtime parameters (for parameterized queries)
    pub parameters: HashMap<String, serde_json::Value>,

    /// Optimizer and execution hints
    pub hints: PlanHints,
}

impl ComputePlan {
    /// Create a new compute plan
    pub fn new(id: impl Into<String>, root: PlanNode) -> Self {
        Self {
            id: id.into(),
            root,
            parameters: HashMap::new(),
            hints: PlanHints::default(),
        }
    }

    /// Add a parameter
    pub fn with_parameter(mut self, name: impl Into<String>, value: serde_json::Value) -> Self {
        self.parameters.insert(name.into(), value);
        self
    }

    /// Set hints
    pub fn with_hints(mut self, hints: PlanHints) -> Self {
        self.hints = hints;
        self
    }

    /// Get all table names referenced in the plan
    pub fn referenced_tables(&self) -> Vec<String> {
        let mut tables = Vec::new();
        self.collect_tables(&self.root, &mut tables);
        tables
    }

    /// Recursively collect all table names referenced by the given plan node.
    fn collect_tables(&self, node: &PlanNode, tables: &mut Vec<String>) {
        match node {
            PlanNode::TableScan { table, .. } => {
                if !tables.contains(table) {
                    tables.push(table.clone());
                }
            }
            PlanNode::VectorScan { collection, .. } => {
                if !tables.contains(collection) {
                    tables.push(collection.clone());
                }
            }
            PlanNode::GraphScan { graph, .. } => {
                if !tables.contains(graph) {
                    tables.push(graph.clone());
                }
            }
            PlanNode::Filter { input, .. } => self.collect_tables(input, tables),
            PlanNode::Project { input, .. } => self.collect_tables(input, tables),
            PlanNode::Aggregate { input, .. } => self.collect_tables(input, tables),
            PlanNode::Sort { input, .. } => self.collect_tables(input, tables),
            PlanNode::Limit { input, .. } => self.collect_tables(input, tables),
            PlanNode::HashJoin { left, right, .. } => {
                self.collect_tables(left, tables);
                self.collect_tables(right, tables);
            }
            PlanNode::Union { inputs, .. } => {
                for input in inputs {
                    self.collect_tables(input, tables);
                }
            }
            PlanNode::Exchange { input, .. } => self.collect_tables(input, tables),
        }
    }

    /// Check if plan contains any vector operations
    pub fn has_vector_operations(&self) -> bool {
        self.check_vector_ops(&self.root)
    }

    /// Recursively check whether any node in the subtree is a vector scan.
    fn check_vector_ops(&self, node: &PlanNode) -> bool {
        match node {
            PlanNode::VectorScan { .. } => true,
            PlanNode::Filter { input, .. } => self.check_vector_ops(input),
            PlanNode::Project { input, .. } => self.check_vector_ops(input),
            PlanNode::Aggregate { input, .. } => self.check_vector_ops(input),
            PlanNode::Sort { input, .. } => self.check_vector_ops(input),
            PlanNode::Limit { input, .. } => self.check_vector_ops(input),
            PlanNode::HashJoin { left, right, .. } => {
                self.check_vector_ops(left) || self.check_vector_ops(right)
            }
            PlanNode::Union { inputs, .. } => inputs.iter().any(|i| self.check_vector_ops(i)),
            PlanNode::Exchange { input, .. } => self.check_vector_ops(input),
            _ => false,
        }
    }

    /// Check if plan contains any graph operations
    pub fn has_graph_operations(&self) -> bool {
        self.check_graph_ops(&self.root)
    }

    /// Recursively check whether any node in the subtree is a graph scan.
    fn check_graph_ops(&self, node: &PlanNode) -> bool {
        match node {
            PlanNode::GraphScan { .. } => true,
            PlanNode::Filter { input, .. } => self.check_graph_ops(input),
            PlanNode::Project { input, .. } => self.check_graph_ops(input),
            PlanNode::Aggregate { input, .. } => self.check_graph_ops(input),
            PlanNode::Sort { input, .. } => self.check_graph_ops(input),
            PlanNode::Limit { input, .. } => self.check_graph_ops(input),
            PlanNode::HashJoin { left, right, .. } => {
                self.check_graph_ops(left) || self.check_graph_ops(right)
            }
            PlanNode::Union { inputs, .. } => inputs.iter().any(|i| self.check_graph_ops(i)),
            PlanNode::Exchange { input, .. } => self.check_graph_ops(input),
            _ => false,
        }
    }
}

/// Plan execution and optimization hints
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanHints {
    /// Preferred compute provider
    pub preferred_provider: Option<String>,

    /// Target parallelism
    pub parallelism: Option<usize>,

    /// Memory budget (bytes)
    pub memory_budget: Option<u64>,

    /// Timeout (milliseconds)
    pub timeout_ms: Option<u64>,

    /// Force specific join strategy
    pub join_strategy: Option<JoinStrategy>,

    /// Enable/disable specific optimizations
    pub optimizations: HashMap<String, bool>,

    /// Provider-specific hints
    pub extensions: HashMap<String, serde_json::Value>,
}

impl PlanHints {
    /// Create hints with preferred provider
    pub fn with_provider(provider: impl Into<String>) -> Self {
        Self {
            preferred_provider: Some(provider.into()),
            ..Default::default()
        }
    }

    /// Set parallelism hint
    pub fn with_parallelism(mut self, parallelism: usize) -> Self {
        self.parallelism = Some(parallelism);
        self
    }

    /// Set memory budget
    pub fn with_memory_budget(mut self, bytes: u64) -> Self {
        self.memory_budget = Some(bytes);
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }

    /// Set join strategy
    pub fn with_join_strategy(mut self, strategy: JoinStrategy) -> Self {
        self.join_strategy = Some(strategy);
        self
    }

    /// Enable an optimization
    pub fn enable_optimization(mut self, name: impl Into<String>) -> Self {
        self.optimizations.insert(name.into(), true);
        self
    }

    /// Disable an optimization
    pub fn disable_optimization(mut self, name: impl Into<String>) -> Self {
        self.optimizations.insert(name.into(), false);
        self
    }
}

/// Join strategy hints
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum JoinStrategy {
    /// Hash join (build hash table, probe)
    Hash,
    /// Sort-merge join
    SortMerge,
    /// Nested loop join
    NestedLoop,
    /// Broadcast smaller side
    Broadcast,
    /// Let optimizer decide
    Auto,
}

// ============================================================================
// Plan Nodes
// ============================================================================

/// Plan node representing a single operation in the plan tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlanNode {
    /// Scan a table/collection
    TableScan {
        /// Table or collection name
        table: String,
        /// Columns to read (empty = all)
        columns: Vec<String>,
        /// Optional filter to push down
        filter: Option<Expr>,
    },

    /// Vector similarity search
    VectorScan {
        /// Vector collection name
        collection: String,
        /// Query vector
        query_vector: Vec<f32>,
        /// Number of nearest neighbors
        top_k: u32,
        /// Optional filter
        filter: Option<Expr>,
        /// Distance metric override
        distance_metric: Option<String>,
    },

    /// Graph traversal
    GraphScan {
        /// Graph name
        graph: String,
        /// Starting node IDs
        start_nodes: Vec<String>,
        /// Traversal specification
        traversal: TraversalSpec,
    },

    /// Filter rows
    Filter {
        /// Input node
        input: Box<PlanNode>,
        /// Filter predicate
        predicate: Expr,
    },

    /// Project columns/expressions
    Project {
        /// Input node
        input: Box<PlanNode>,
        /// Projection expressions
        expressions: Vec<ProjectExpr>,
    },

    /// Aggregate with optional grouping
    Aggregate {
        /// Input node
        input: Box<PlanNode>,
        /// Group by expressions
        group_by: Vec<Expr>,
        /// Aggregate expressions
        aggregates: Vec<AggExpr>,
    },

    /// Sort rows
    Sort {
        /// Input node
        input: Box<PlanNode>,
        /// Sort specifications
        order_by: Vec<SortExpr>,
    },

    /// Limit and offset
    Limit {
        /// Input node
        input: Box<PlanNode>,
        /// Maximum rows
        limit: u64,
        /// Rows to skip
        offset: u64,
    },

    /// Hash join
    HashJoin {
        /// Left input
        left: Box<PlanNode>,
        /// Right input
        right: Box<PlanNode>,
        /// Join condition
        on: JoinCondition,
    },

    /// Union of inputs
    Union {
        /// Input nodes
        inputs: Vec<PlanNode>,
        /// UNION ALL if true, else UNION DISTINCT
        all: bool,
    },

    /// Data exchange (for distributed execution)
    Exchange {
        /// Input node
        input: Box<PlanNode>,
        /// Partitioning scheme
        partitioning: Partitioning,
    },
}

// ============================================================================
// Graph Traversal Types
// ============================================================================

/// Graph traversal specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalSpec {
    /// Edge types to follow (empty = all)
    pub edge_types: Vec<String>,

    /// Traversal direction
    pub direction: TraversalDirection,

    /// Minimum depth
    pub min_depth: u32,

    /// Maximum depth
    pub max_depth: u32,

    /// Node/edge filter
    pub filter: Option<Expr>,
}

/// Traversal direction
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TraversalDirection {
    /// Follow outgoing edges
    Outgoing,
    /// Follow incoming edges
    Incoming,
    /// Follow both directions
    Both,
}

impl Default for TraversalSpec {
    fn default() -> Self {
        Self {
            edge_types: Vec::new(),
            direction: TraversalDirection::Outgoing,
            min_depth: 1,
            max_depth: 3,
            filter: None,
        }
    }
}

// ============================================================================
// Expression Types
// ============================================================================

/// Expression for predicates and projections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    /// Column reference
    Column(String),

    /// Literal value
    Literal(LiteralValue),

    /// Parameter reference ($1, $name)
    Parameter(String),

    /// Binary operation
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },

    /// Unary operation
    Unary { op: UnaryOp, expr: Box<Expr> },

    /// Function call
    Function { name: String, args: Vec<Expr> },

    /// CASE expression
    Case {
        operand: Option<Box<Expr>>,
        when_then: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
    },

    /// IS NULL
    IsNull(Box<Expr>),

    /// IS NOT NULL
    IsNotNull(Box<Expr>),

    /// IN list
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },

    /// BETWEEN
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },

    /// Subquery (scalar)
    ScalarSubquery(Box<ComputePlan>),

    /// EXISTS subquery
    Exists {
        subquery: Box<ComputePlan>,
        negated: bool,
    },

    /// CAST expression
    Cast { expr: Box<Expr>, data_type: String },

    /// Array literal
    Array(Vec<Expr>),

    /// Struct literal
    Struct(Vec<(String, Expr)>),

    /// Wildcard (*)
    Wildcard,
}

/// Literal values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LiteralValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Date(String),      // ISO format
    Timestamp(String), // ISO format
    Interval(String),  // Duration string
}

/// Binary operators
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BinaryOp {
    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // Logical
    And,
    Or,

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,

    // String
    Like,
    ILike,
    Concat,

    // Bitwise
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
}

/// Unary operators
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum UnaryOp {
    Not,
    Neg,
    BitwiseNot,
    IsTrue,
    IsFalse,
}

// ============================================================================
// Projection Types
// ============================================================================

/// Projection expression with optional alias
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectExpr {
    /// The expression
    pub expr: Expr,
    /// Optional alias
    pub alias: Option<String>,
}

impl ProjectExpr {
    /// Create a projection without alias
    pub fn new(expr: Expr) -> Self {
        Self { expr, alias: None }
    }

    /// Create a projection with alias
    pub fn with_alias(expr: Expr, alias: impl Into<String>) -> Self {
        Self {
            expr,
            alias: Some(alias.into()),
        }
    }
}

// ============================================================================
// Aggregate Types
// ============================================================================

/// Aggregate expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggExpr {
    /// Aggregate function name
    pub function: AggFunction,
    /// Arguments
    pub args: Vec<Expr>,
    /// DISTINCT modifier
    pub distinct: bool,
    /// Filter clause (FILTER WHERE)
    pub filter: Option<Box<Expr>>,
    /// Alias
    pub alias: Option<String>,
}

/// Built-in aggregate functions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AggFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    First,
    Last,
    ArrayAgg,
    StringAgg,
    StdDev,
    Variance,
    BitAnd,
    BitOr,
    BoolAnd,
    BoolOr,
    ApproxDistinct,
    Percentile { percentile: f64 },
    Custom(String),
}

// ============================================================================
// Sort Types
// ============================================================================

/// Sort expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortExpr {
    /// Expression to sort by
    pub expr: Expr,
    /// Ascending order
    pub ascending: bool,
    /// Nulls first
    pub nulls_first: bool,
}

impl SortExpr {
    /// Create ascending sort
    pub fn asc(expr: Expr) -> Self {
        Self {
            expr,
            ascending: true,
            nulls_first: false,
        }
    }

    /// Create descending sort
    pub fn desc(expr: Expr) -> Self {
        Self {
            expr,
            ascending: false,
            nulls_first: true,
        }
    }
}

// ============================================================================
// Join Types
// ============================================================================

/// Join condition specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinCondition {
    /// Join type
    pub join_type: JoinType,
    /// Left key columns
    pub left_keys: Vec<String>,
    /// Right key columns
    pub right_keys: Vec<String>,
    /// Additional filter
    pub filter: Option<Expr>,
}

/// Join type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum JoinType {
    Inner,
    LeftOuter,
    RightOuter,
    FullOuter,
    LeftSemi,
    LeftAnti,
    Cross,
}

// ============================================================================
// Partitioning Types
// ============================================================================

/// Data partitioning scheme
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Partitioning {
    /// Round-robin distribution
    RoundRobin { partitions: usize },
    /// Hash partitioning on columns
    Hash {
        columns: Vec<String>,
        partitions: usize,
    },
    /// Range partitioning
    Range { column: String, partitions: usize },
    /// Single partition (gather)
    Single,
    /// Broadcast to all partitions
    Broadcast,
}

// ============================================================================
// Builder Helpers
// ============================================================================

impl PlanNode {
    /// Create a table scan
    pub fn table_scan(table: impl Into<String>) -> Self {
        Self::TableScan {
            table: table.into(),
            columns: Vec::new(),
            filter: None,
        }
    }

    /// Create a vector scan
    pub fn vector_scan(collection: impl Into<String>, query: Vec<f32>, k: u32) -> Self {
        Self::VectorScan {
            collection: collection.into(),
            query_vector: query,
            top_k: k,
            filter: None,
            distance_metric: None,
        }
    }

    /// Create a graph scan
    pub fn graph_scan(graph: impl Into<String>, starts: Vec<String>) -> Self {
        Self::GraphScan {
            graph: graph.into(),
            start_nodes: starts,
            traversal: TraversalSpec::default(),
        }
    }

    /// Wrap in filter
    pub fn filter(self, predicate: Expr) -> Self {
        Self::Filter {
            input: Box::new(self),
            predicate,
        }
    }

    /// Wrap in projection
    pub fn project(self, expressions: Vec<ProjectExpr>) -> Self {
        Self::Project {
            input: Box::new(self),
            expressions,
        }
    }

    /// Wrap in sort
    pub fn sort(self, order_by: Vec<SortExpr>) -> Self {
        Self::Sort {
            input: Box::new(self),
            order_by,
        }
    }

    /// Wrap in limit
    pub fn limit(self, limit: u64) -> Self {
        Self::Limit {
            input: Box::new(self),
            limit,
            offset: 0,
        }
    }

    /// Wrap in limit with offset
    pub fn limit_offset(self, limit: u64, offset: u64) -> Self {
        Self::Limit {
            input: Box::new(self),
            limit,
            offset,
        }
    }
}

impl Expr {
    /// Create a column reference
    pub fn col(name: impl Into<String>) -> Self {
        Self::Column(name.into())
    }

    /// Create a literal integer
    pub fn lit_int(val: i64) -> Self {
        Self::Literal(LiteralValue::Int(val))
    }

    /// Create a literal float
    pub fn lit_float(val: f64) -> Self {
        Self::Literal(LiteralValue::Float(val))
    }

    /// Create a literal string
    pub fn lit_str(val: impl Into<String>) -> Self {
        Self::Literal(LiteralValue::String(val.into()))
    }

    /// Create a literal boolean
    pub fn lit_bool(val: bool) -> Self {
        Self::Literal(LiteralValue::Bool(val))
    }

    /// Equal comparison
    pub fn eq(self, other: Expr) -> Self {
        Self::Binary {
            left: Box::new(self),
            op: BinaryOp::Eq,
            right: Box::new(other),
        }
    }

    /// Not equal comparison
    pub fn ne(self, other: Expr) -> Self {
        Self::Binary {
            left: Box::new(self),
            op: BinaryOp::Ne,
            right: Box::new(other),
        }
    }

    /// Greater than
    pub fn gt(self, other: Expr) -> Self {
        Self::Binary {
            left: Box::new(self),
            op: BinaryOp::Gt,
            right: Box::new(other),
        }
    }

    /// Greater than or equal
    pub fn gte(self, other: Expr) -> Self {
        Self::Binary {
            left: Box::new(self),
            op: BinaryOp::Ge,
            right: Box::new(other),
        }
    }

    /// Less than
    pub fn lt(self, other: Expr) -> Self {
        Self::Binary {
            left: Box::new(self),
            op: BinaryOp::Lt,
            right: Box::new(other),
        }
    }

    /// Less than or equal
    pub fn lte(self, other: Expr) -> Self {
        Self::Binary {
            left: Box::new(self),
            op: BinaryOp::Le,
            right: Box::new(other),
        }
    }

    /// Logical AND
    pub fn and(self, other: Expr) -> Self {
        Self::Binary {
            left: Box::new(self),
            op: BinaryOp::And,
            right: Box::new(other),
        }
    }

    /// Logical OR
    pub fn or(self, other: Expr) -> Self {
        Self::Binary {
            left: Box::new(self),
            op: BinaryOp::Or,
            right: Box::new(other),
        }
    }

    /// Logical NOT
    pub fn logical_not(self) -> Self {
        Self::Unary {
            op: UnaryOp::Not,
            expr: Box::new(self),
        }
    }
}

impl Not for Expr {
    type Output = Self;

    fn not(self) -> Self::Output {
        self.logical_not()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_plan_creation() {
        let plan = ComputePlan::new("test", PlanNode::table_scan("users"));

        assert_eq!(plan.id, "test");
        assert_eq!(plan.referenced_tables(), vec!["users"]);
    }

    #[test]
    fn test_plan_with_parameters() {
        let plan = ComputePlan::new("test", PlanNode::table_scan("users"))
            .with_parameter("limit", serde_json::json!(100))
            .with_parameter("offset", serde_json::json!(0));

        assert_eq!(plan.parameters.len(), 2);
        assert_eq!(plan.parameters["limit"], 100);
    }

    #[test]
    fn test_plan_node_builder() {
        let plan_node = PlanNode::table_scan("users")
            .filter(Expr::col("age").gt(Expr::lit_int(18)))
            .project(vec![
                ProjectExpr::new(Expr::col("name")),
                ProjectExpr::with_alias(Expr::col("age"), "user_age"),
            ])
            .sort(vec![SortExpr::desc(Expr::col("age"))])
            .limit(10);

        // Verify structure
        match plan_node {
            PlanNode::Limit { limit, input, .. } => {
                assert_eq!(limit, 10);
                match input.as_ref() {
                    PlanNode::Sort { order_by, .. } => {
                        assert_eq!(order_by.len(), 1);
                        assert!(!order_by[0].ascending);
                    }
                    _ => panic!("Expected Sort"),
                }
            }
            _ => panic!("Expected Limit"),
        }
    }

    #[test]
    fn test_expr_builder() {
        let expr = Expr::col("age")
            .gt(Expr::lit_int(18))
            .and(Expr::col("name").ne(Expr::lit_str("admin")));

        match expr {
            Expr::Binary {
                op: BinaryOp::And, ..
            } => {}
            _ => panic!("Expected AND expression"),
        }
    }

    #[test]
    fn test_vector_plan() {
        let plan = ComputePlan::new(
            "vector-search",
            PlanNode::vector_scan("embeddings", vec![0.1, 0.2, 0.3], 10),
        );

        assert!(plan.has_vector_operations());
        assert!(!plan.has_graph_operations());
    }

    #[test]
    fn test_graph_plan() {
        let plan = ComputePlan::new(
            "graph-traversal",
            PlanNode::graph_scan("social", vec!["user-1".to_string()]),
        );

        assert!(plan.has_graph_operations());
        assert!(!plan.has_vector_operations());
    }

    #[test]
    fn test_plan_serialization() {
        let plan = ComputePlan::new(
            "test",
            PlanNode::table_scan("users").filter(Expr::col("active").eq(Expr::lit_bool(true))),
        );

        let json = serde_json::to_string(&plan).unwrap();
        let parsed: ComputePlan = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "test");
    }

    #[test]
    fn test_traversal_spec() {
        let spec = TraversalSpec {
            edge_types: vec!["FOLLOWS".to_string(), "LIKES".to_string()],
            direction: TraversalDirection::Both,
            min_depth: 1,
            max_depth: 5,
            filter: Some(Expr::col("weight").gt(Expr::lit_float(0.5))),
        };

        assert_eq!(spec.edge_types.len(), 2);
        assert_eq!(spec.max_depth, 5);
    }

    #[test]
    fn test_aggregate_expr() {
        let agg = AggExpr {
            function: AggFunction::Sum,
            args: vec![Expr::col("amount")],
            distinct: false,
            filter: Some(Box::new(Expr::col("status").eq(Expr::lit_str("completed")))),
            alias: Some("total_amount".to_string()),
        };

        assert!(!agg.distinct);
        assert!(agg.filter.is_some());
    }

    #[test]
    fn test_join_condition() {
        let condition = JoinCondition {
            join_type: JoinType::LeftOuter,
            left_keys: vec!["user_id".to_string()],
            right_keys: vec!["id".to_string()],
            filter: None,
        };

        assert_eq!(condition.left_keys.len(), 1);
        matches!(condition.join_type, JoinType::LeftOuter);
    }

    #[test]
    fn test_partitioning() {
        let hash_part = Partitioning::Hash {
            columns: vec!["user_id".to_string()],
            partitions: 8,
        };

        match hash_part {
            Partitioning::Hash { partitions, .. } => assert_eq!(partitions, 8),
            _ => panic!("Expected Hash partitioning"),
        }
    }

    #[test]
    fn test_plan_hints() {
        let hints = PlanHints::with_provider("spark")
            .with_parallelism(16)
            .with_memory_budget(4 * 1024 * 1024 * 1024)
            .with_join_strategy(JoinStrategy::Broadcast)
            .enable_optimization("predicate_pushdown")
            .disable_optimization("constant_folding");

        assert_eq!(hints.preferred_provider, Some("spark".to_string()));
        assert_eq!(hints.parallelism, Some(16));
        assert_eq!(hints.optimizations.get("predicate_pushdown"), Some(&true));
        assert_eq!(hints.optimizations.get("constant_folding"), Some(&false));
    }
}
