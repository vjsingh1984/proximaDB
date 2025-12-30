//! # Pushdown Protocol
//!
//! This module defines the protocol for negotiating pushdown of query operations
//! from external query engines to ProximaDB's storage layer. Effective pushdown
//! significantly reduces data transfer and improves query performance.
//!
//! ## Pushdown Categories
//!
//! 1. **Filter Pushdown**: Predicates are evaluated at the storage layer
//! 2. **Projection Pushdown**: Only required columns are read
//! 3. **Aggregate Pushdown**: Aggregations (COUNT, SUM, etc.) computed at storage
//! 4. **Limit Pushdown**: Early termination after N rows
//! 5. **Vector Search Pushdown**: KNN queries executed by AXIS engine
//! 6. **Graph Traversal Pushdown**: Graph queries executed by ORION/PULSAR
//!
//! ## Negotiation Flow
//!
//! ```text
//! Query Engine                         ProximaDB Connector
//!      │                                       │
//!      │  PushdownRequest                      │
//!      │  (filters, projections, aggregates)   │
//!      │──────────────────────────────────────▶│
//!      │                                       │
//!      │                     negotiate_pushdown()
//!      │                                       │
//!      │  PushdownResponse                     │
//!      │  (accepted_*, estimated_rows)         │
//!      │◀──────────────────────────────────────│
//!      │                                       │
//!      │  Create reader with accepted pushdown │
//!      │──────────────────────────────────────▶│
//!      │                                       │
//! ```
//!
//! ## Expression Types
//!
//! The `Expr` enum supports a rich set of filter expressions including:
//! - Comparisons (=, <, >, <=, >=, !=)
//! - Boolean logic (AND, OR, NOT)
//! - Set operations (IN, NOT IN)
//! - Range queries (BETWEEN)
//! - Pattern matching (LIKE)
//! - Null checks (IS NULL, IS NOT NULL)
//!
//! ## Performance Impact
//!
//! Effective pushdown can provide 10-100x performance improvements by:
//! - Reducing I/O (read only needed columns and rows)
//! - Reducing CPU (avoid deserializing filtered-out data)
//! - Reducing memory (smaller result sets)
//! - Leveraging indexes (bloom filters, skip indexes)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Request for pushdown negotiation.
///
/// The query engine sends this to discover which operations the connector
/// can handle natively, avoiding redundant processing in the engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PushdownRequest {
    /// Filter expressions to push down
    pub filters: Vec<Expr>,

    /// Column projections (empty = all columns)
    pub projections: Vec<String>,

    /// Aggregate expressions to push down
    pub aggregates: Vec<AggExpr>,

    /// Row limit to push down
    pub limit: Option<u64>,

    /// Vector search to push down (if applicable)
    pub vector_search: Option<VectorSearchPushdown>,

    /// Graph traversal to push down (if applicable)
    pub graph_traversal: Option<GraphTraversalPushdown>,
}

impl PushdownRequest {
    /// Create a new empty pushdown request.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a filter expression.
    pub fn with_filter(mut self, filter: Expr) -> Self {
        self.filters.push(filter);
        self
    }

    /// Set the projections.
    pub fn with_projections(mut self, projections: Vec<String>) -> Self {
        self.projections = projections;
        self
    }

    /// Add an aggregate expression.
    pub fn with_aggregate(mut self, aggregate: AggExpr) -> Self {
        self.aggregates.push(aggregate);
        self
    }

    /// Set the row limit.
    pub fn with_limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the vector search pushdown.
    pub fn with_vector_search(mut self, vector_search: VectorSearchPushdown) -> Self {
        self.vector_search = Some(vector_search);
        self
    }

    /// Set the graph traversal pushdown.
    pub fn with_graph_traversal(mut self, graph_traversal: GraphTraversalPushdown) -> Self {
        self.graph_traversal = Some(graph_traversal);
        self
    }

    /// Check if any filters are requested.
    pub fn has_filters(&self) -> bool {
        !self.filters.is_empty()
    }

    /// Check if projections are requested.
    pub fn has_projections(&self) -> bool {
        !self.projections.is_empty()
    }

    /// Check if aggregates are requested.
    pub fn has_aggregates(&self) -> bool {
        !self.aggregates.is_empty()
    }
}

/// Response from pushdown negotiation.
///
/// The connector returns this to indicate which operations it can handle
/// and to provide cost estimates for query planning.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PushdownResponse {
    /// Filters that the connector will handle
    pub accepted_filters: Vec<Expr>,

    /// Projections that the connector will handle
    pub accepted_projections: Vec<String>,

    /// Whether the connector will handle aggregates
    pub accepts_aggregates: bool,

    /// Whether the connector will handle the limit
    pub accepts_limit: bool,

    /// Whether the connector will handle vector search
    pub accepts_vector_search: bool,

    /// Whether the connector will handle graph traversal
    pub accepts_graph_traversal: bool,

    /// Estimated row count after applying accepted filters
    pub estimated_rows: Option<u64>,

    /// Estimated data size in bytes after pushdown
    pub estimated_bytes: Option<u64>,

    /// Cost estimate for the query (arbitrary units)
    pub cost_estimate: Option<f64>,

    /// Filters that were rejected (query engine must handle)
    pub rejected_filters: Vec<Expr>,

    /// Reason for any rejections (for debugging)
    pub rejection_reasons: HashMap<String, String>,
}

impl PushdownResponse {
    /// Create a new empty response (no pushdown accepted).
    pub fn none() -> Self {
        Self::default()
    }

    /// Create a response that accepts all pushdown.
    pub fn accept_all(request: &PushdownRequest) -> Self {
        Self {
            accepted_filters: request.filters.clone(),
            accepted_projections: request.projections.clone(),
            accepts_aggregates: !request.aggregates.is_empty(),
            accepts_limit: request.limit.is_some(),
            accepts_vector_search: request.vector_search.is_some(),
            accepts_graph_traversal: request.graph_traversal.is_some(),
            ..Default::default()
        }
    }

    /// Set the estimated row count.
    pub fn with_estimated_rows(mut self, rows: u64) -> Self {
        self.estimated_rows = Some(rows);
        self
    }

    /// Set the estimated bytes.
    pub fn with_estimated_bytes(mut self, bytes: u64) -> Self {
        self.estimated_bytes = Some(bytes);
        self
    }

    /// Set the cost estimate.
    pub fn with_cost_estimate(mut self, cost: f64) -> Self {
        self.cost_estimate = Some(cost);
        self
    }

    /// Add a rejected filter with reason.
    pub fn reject_filter(mut self, filter: Expr, reason: impl Into<String>) -> Self {
        let reason_str = reason.into();
        self.rejection_reasons.insert(format!("{:?}", filter), reason_str);
        self.rejected_filters.push(filter);
        self
    }

    /// Check if any pushdown was accepted.
    pub fn has_pushdown(&self) -> bool {
        !self.accepted_filters.is_empty()
            || !self.accepted_projections.is_empty()
            || self.accepts_aggregates
            || self.accepts_limit
            || self.accepts_vector_search
            || self.accepts_graph_traversal
    }

    /// Get the selectivity estimate (0.0 to 1.0).
    pub fn selectivity(&self, total_rows: u64) -> Option<f64> {
        self.estimated_rows.map(|est| est as f64 / total_rows as f64)
    }
}

/// Filter expression for pushdown.
///
/// Represents a predicate that can be evaluated at the storage layer.
/// Expressions form a tree structure with logical operators at internal
/// nodes and comparisons at leaves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// Column reference
    Column(String),

    /// Literal value
    Literal(LiteralValue),

    /// Binary comparison (=, <, >, etc.)
    BinaryOp {
        left: Box<Expr>,
        op: BinaryOperator,
        right: Box<Expr>,
    },

    /// Logical AND of expressions
    And(Vec<Expr>),

    /// Logical OR of expressions
    Or(Vec<Expr>),

    /// Logical NOT of expression
    Not(Box<Expr>),

    /// IN clause (column IN (value1, value2, ...))
    In {
        column: String,
        values: Vec<LiteralValue>,
        negated: bool,
    },

    /// BETWEEN clause (column BETWEEN low AND high)
    Between {
        column: String,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },

    /// LIKE pattern matching
    Like {
        column: String,
        pattern: String,
        escape: Option<char>,
        negated: bool,
    },

    /// IS NULL check
    IsNull {
        column: String,
        negated: bool,
    },

    /// Function call (for extensibility)
    Function {
        name: String,
        args: Vec<Expr>,
    },

    /// Vector similarity comparison (for vector search)
    VectorSimilarity {
        column: String,
        query_vector: Vec<f32>,
        metric: String,
        threshold: f32,
    },

    /// Subquery (for complex filters)
    Subquery {
        query: String,
    },
}

impl Expr {
    /// Create a column reference.
    pub fn column(name: impl Into<String>) -> Self {
        Self::Column(name.into())
    }

    /// Create a literal value.
    pub fn literal<T: Into<LiteralValue>>(value: T) -> Self {
        Self::Literal(value.into())
    }

    /// Create an equality expression.
    pub fn eq(left: Expr, right: Expr) -> Self {
        Self::BinaryOp {
            left: Box::new(left),
            op: BinaryOperator::Eq,
            right: Box::new(right),
        }
    }

    /// Create a not-equal expression.
    pub fn ne(left: Expr, right: Expr) -> Self {
        Self::BinaryOp {
            left: Box::new(left),
            op: BinaryOperator::NotEq,
            right: Box::new(right),
        }
    }

    /// Create a less-than expression.
    pub fn lt(left: Expr, right: Expr) -> Self {
        Self::BinaryOp {
            left: Box::new(left),
            op: BinaryOperator::Lt,
            right: Box::new(right),
        }
    }

    /// Create a less-than-or-equal expression.
    pub fn le(left: Expr, right: Expr) -> Self {
        Self::BinaryOp {
            left: Box::new(left),
            op: BinaryOperator::LtEq,
            right: Box::new(right),
        }
    }

    /// Create a greater-than expression.
    pub fn gt(left: Expr, right: Expr) -> Self {
        Self::BinaryOp {
            left: Box::new(left),
            op: BinaryOperator::Gt,
            right: Box::new(right),
        }
    }

    /// Create a greater-than-or-equal expression.
    pub fn ge(left: Expr, right: Expr) -> Self {
        Self::BinaryOp {
            left: Box::new(left),
            op: BinaryOperator::GtEq,
            right: Box::new(right),
        }
    }

    /// Create an AND expression.
    pub fn and(exprs: Vec<Expr>) -> Self {
        if exprs.len() == 1 {
            exprs.into_iter().next().expect("checked length")
        } else {
            Self::And(exprs)
        }
    }

    /// Create an OR expression.
    pub fn or(exprs: Vec<Expr>) -> Self {
        if exprs.len() == 1 {
            exprs.into_iter().next().expect("checked length")
        } else {
            Self::Or(exprs)
        }
    }

    /// Create a NOT expression.
    pub fn not(expr: Expr) -> Self {
        Self::Not(Box::new(expr))
    }

    /// Create an IN expression.
    pub fn in_list(column: impl Into<String>, values: Vec<LiteralValue>) -> Self {
        Self::In {
            column: column.into(),
            values,
            negated: false,
        }
    }

    /// Create a NOT IN expression.
    pub fn not_in_list(column: impl Into<String>, values: Vec<LiteralValue>) -> Self {
        Self::In {
            column: column.into(),
            values,
            negated: true,
        }
    }

    /// Create a BETWEEN expression.
    pub fn between(column: impl Into<String>, low: Expr, high: Expr) -> Self {
        Self::Between {
            column: column.into(),
            low: Box::new(low),
            high: Box::new(high),
            negated: false,
        }
    }

    /// Create a NOT BETWEEN expression.
    pub fn not_between(column: impl Into<String>, low: Expr, high: Expr) -> Self {
        Self::Between {
            column: column.into(),
            low: Box::new(low),
            high: Box::new(high),
            negated: true,
        }
    }

    /// Create a LIKE expression.
    pub fn like(column: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::Like {
            column: column.into(),
            pattern: pattern.into(),
            escape: None,
            negated: false,
        }
    }

    /// Create an IS NULL expression.
    pub fn is_null(column: impl Into<String>) -> Self {
        Self::IsNull {
            column: column.into(),
            negated: false,
        }
    }

    /// Create an IS NOT NULL expression.
    pub fn is_not_null(column: impl Into<String>) -> Self {
        Self::IsNull {
            column: column.into(),
            negated: true,
        }
    }

    /// Check if this expression references a specific column.
    pub fn references_column(&self, name: &str) -> bool {
        match self {
            Self::Column(col) => col == name,
            Self::Literal(_) => false,
            Self::BinaryOp { left, right, .. } => {
                left.references_column(name) || right.references_column(name)
            }
            Self::And(exprs) | Self::Or(exprs) => {
                exprs.iter().any(|e| e.references_column(name))
            }
            Self::Not(expr) => expr.references_column(name),
            Self::In { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column, .. } => column == name,
            Self::Between { column, low, high, .. } => {
                column == name || low.references_column(name) || high.references_column(name)
            }
            Self::Function { args, .. } => args.iter().any(|a| a.references_column(name)),
            Self::VectorSimilarity { column, .. } => column == name,
            Self::Subquery { .. } => false,
        }
    }

    /// Get all columns referenced by this expression.
    pub fn referenced_columns(&self) -> Vec<String> {
        let mut columns = Vec::new();
        self.collect_columns(&mut columns);
        columns.sort();
        columns.dedup();
        columns
    }

    fn collect_columns(&self, columns: &mut Vec<String>) {
        match self {
            Self::Column(col) => columns.push(col.clone()),
            Self::Literal(_) => {}
            Self::BinaryOp { left, right, .. } => {
                left.collect_columns(columns);
                right.collect_columns(columns);
            }
            Self::And(exprs) | Self::Or(exprs) => {
                for expr in exprs {
                    expr.collect_columns(columns);
                }
            }
            Self::Not(expr) => expr.collect_columns(columns),
            Self::In { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column, .. }
            | Self::VectorSimilarity { column, .. } => columns.push(column.clone()),
            Self::Between { column, low, high, .. } => {
                columns.push(column.clone());
                low.collect_columns(columns);
                high.collect_columns(columns);
            }
            Self::Function { args, .. } => {
                for arg in args {
                    arg.collect_columns(columns);
                }
            }
            Self::Subquery { .. } => {}
        }
    }
}

/// Binary operators for comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinaryOperator {
    /// Equal (=)
    Eq,
    /// Not equal (!=, <>)
    NotEq,
    /// Less than (<)
    Lt,
    /// Less than or equal (<=)
    LtEq,
    /// Greater than (>)
    Gt,
    /// Greater than or equal (>=)
    GtEq,
    /// Addition (+)
    Add,
    /// Subtraction (-)
    Sub,
    /// Multiplication (*)
    Mul,
    /// Division (/)
    Div,
    /// Modulo (%)
    Mod,
    /// String concatenation (||)
    Concat,
}

impl std::fmt::Display for BinaryOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eq => write!(f, "="),
            Self::NotEq => write!(f, "!="),
            Self::Lt => write!(f, "<"),
            Self::LtEq => write!(f, "<="),
            Self::Gt => write!(f, ">"),
            Self::GtEq => write!(f, ">="),
            Self::Add => write!(f, "+"),
            Self::Sub => write!(f, "-"),
            Self::Mul => write!(f, "*"),
            Self::Div => write!(f, "/"),
            Self::Mod => write!(f, "%"),
            Self::Concat => write!(f, "||"),
        }
    }
}

/// Literal values for expressions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LiteralValue {
    /// Null value
    Null,
    /// Boolean value
    Boolean(bool),
    /// 64-bit signed integer
    Int64(i64),
    /// 64-bit floating point
    Float64(f64),
    /// UTF-8 string
    String(String),
    /// Binary data
    Binary(Vec<u8>),
    /// Date (days since epoch)
    Date(i32),
    /// Timestamp (microseconds since epoch)
    Timestamp(i64),
    /// Interval
    Interval { months: i32, days: i32, nanos: i64 },
    /// Array of values
    Array(Vec<LiteralValue>),
    /// Map of key-value pairs
    Map(Vec<(LiteralValue, LiteralValue)>),
}

impl From<bool> for LiteralValue {
    fn from(v: bool) -> Self {
        Self::Boolean(v)
    }
}

impl From<i32> for LiteralValue {
    fn from(v: i32) -> Self {
        Self::Int64(v as i64)
    }
}

impl From<i64> for LiteralValue {
    fn from(v: i64) -> Self {
        Self::Int64(v)
    }
}

impl From<f32> for LiteralValue {
    fn from(v: f32) -> Self {
        Self::Float64(v as f64)
    }
}

impl From<f64> for LiteralValue {
    fn from(v: f64) -> Self {
        Self::Float64(v)
    }
}

impl From<&str> for LiteralValue {
    fn from(v: &str) -> Self {
        Self::String(v.to_string())
    }
}

impl From<String> for LiteralValue {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<Vec<u8>> for LiteralValue {
    fn from(v: Vec<u8>) -> Self {
        Self::Binary(v)
    }
}

impl std::fmt::Display for LiteralValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => write!(f, "NULL"),
            Self::Boolean(v) => write!(f, "{}", v),
            Self::Int64(v) => write!(f, "{}", v),
            Self::Float64(v) => write!(f, "{}", v),
            Self::String(v) => write!(f, "'{}'", v),
            Self::Binary(v) => write!(f, "x'{}'", hex::encode(v)),
            Self::Date(v) => write!(f, "DATE '{}'", v),
            Self::Timestamp(v) => write!(f, "TIMESTAMP '{}'", v),
            Self::Interval { months, days, nanos } => {
                write!(f, "INTERVAL '{} months {} days {} ns'", months, days, nanos)
            }
            Self::Array(v) => {
                write!(f, "[")?;
                for (i, item) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "]")
            }
            Self::Map(v) => {
                write!(f, "{{")?;
                for (i, (k, val)) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, val)?;
                }
                write!(f, "}}")
            }
        }
    }
}

/// Aggregate expression for pushdown.
///
/// Represents an aggregation that can be computed at the storage layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggExpr {
    /// Aggregate function (COUNT, SUM, AVG, MIN, MAX, etc.)
    pub function: AggFunction,

    /// Column to aggregate (None for COUNT(*))
    pub column: Option<String>,

    /// Whether DISTINCT is applied
    pub distinct: bool,

    /// Optional filter (WHERE clause within aggregate)
    pub filter: Option<Box<Expr>>,

    /// Output alias
    pub alias: Option<String>,
}

impl AggExpr {
    /// Create a COUNT(*) aggregate.
    pub fn count_star() -> Self {
        Self {
            function: AggFunction::Count,
            column: None,
            distinct: false,
            filter: None,
            alias: None,
        }
    }

    /// Create a COUNT(column) aggregate.
    pub fn count(column: impl Into<String>) -> Self {
        Self {
            function: AggFunction::Count,
            column: Some(column.into()),
            distinct: false,
            filter: None,
            alias: None,
        }
    }

    /// Create a SUM aggregate.
    pub fn sum(column: impl Into<String>) -> Self {
        Self {
            function: AggFunction::Sum,
            column: Some(column.into()),
            distinct: false,
            filter: None,
            alias: None,
        }
    }

    /// Create an AVG aggregate.
    pub fn avg(column: impl Into<String>) -> Self {
        Self {
            function: AggFunction::Avg,
            column: Some(column.into()),
            distinct: false,
            filter: None,
            alias: None,
        }
    }

    /// Create a MIN aggregate.
    pub fn min(column: impl Into<String>) -> Self {
        Self {
            function: AggFunction::Min,
            column: Some(column.into()),
            distinct: false,
            filter: None,
            alias: None,
        }
    }

    /// Create a MAX aggregate.
    pub fn max(column: impl Into<String>) -> Self {
        Self {
            function: AggFunction::Max,
            column: Some(column.into()),
            distinct: false,
            filter: None,
            alias: None,
        }
    }

    /// Make this aggregate distinct.
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    /// Add a filter to this aggregate.
    pub fn filter(mut self, filter: Expr) -> Self {
        self.filter = Some(Box::new(filter));
        self
    }

    /// Set an alias for this aggregate.
    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = Some(alias.into());
        self
    }
}

/// Aggregate functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AggFunction {
    /// Count rows/values
    Count,
    /// Sum of values
    Sum,
    /// Average of values
    Avg,
    /// Minimum value
    Min,
    /// Maximum value
    Max,
    /// First value
    First,
    /// Last value
    Last,
    /// Standard deviation (sample)
    StdDev,
    /// Standard deviation (population)
    StdDevPop,
    /// Variance (sample)
    Variance,
    /// Variance (population)
    VariancePop,
    /// Approximate count distinct (HyperLogLog)
    ApproxCountDistinct,
    /// Approximate percentile
    ApproxPercentile,
    /// Collect values into array
    ArrayAgg,
    /// Boolean AND
    BoolAnd,
    /// Boolean OR
    BoolOr,
}

impl std::fmt::Display for AggFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count => write!(f, "COUNT"),
            Self::Sum => write!(f, "SUM"),
            Self::Avg => write!(f, "AVG"),
            Self::Min => write!(f, "MIN"),
            Self::Max => write!(f, "MAX"),
            Self::First => write!(f, "FIRST"),
            Self::Last => write!(f, "LAST"),
            Self::StdDev => write!(f, "STDDEV"),
            Self::StdDevPop => write!(f, "STDDEV_POP"),
            Self::Variance => write!(f, "VARIANCE"),
            Self::VariancePop => write!(f, "VAR_POP"),
            Self::ApproxCountDistinct => write!(f, "APPROX_COUNT_DISTINCT"),
            Self::ApproxPercentile => write!(f, "APPROX_PERCENTILE"),
            Self::ArrayAgg => write!(f, "ARRAY_AGG"),
            Self::BoolAnd => write!(f, "BOOL_AND"),
            Self::BoolOr => write!(f, "BOOL_OR"),
        }
    }
}

/// Vector search pushdown specification.
///
/// Represents a KNN query that can be executed natively by ProximaDB's
/// AXIS engine, avoiding data transfer to the query engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorSearchPushdown {
    /// Target collection name
    pub collection: String,

    /// Query vector for similarity search
    pub query_vector: Vec<f32>,

    /// Number of nearest neighbors to return
    pub top_k: u32,

    /// Minimum similarity threshold
    pub threshold: Option<f32>,

    /// Distance metric (cosine, euclidean, dot_product)
    pub metric: String,
}

impl VectorSearchPushdown {
    /// Create a new vector search pushdown.
    pub fn new(collection: impl Into<String>, query_vector: Vec<f32>, top_k: u32) -> Self {
        Self {
            collection: collection.into(),
            query_vector,
            top_k,
            threshold: None,
            metric: "cosine".to_string(),
        }
    }

    /// Set the distance metric.
    pub fn with_metric(mut self, metric: impl Into<String>) -> Self {
        self.metric = metric.into();
        self
    }

    /// Set a similarity threshold.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = Some(threshold);
        self
    }

    /// Validate the pushdown specification.
    pub fn validate(&self) -> Result<(), String> {
        if self.query_vector.is_empty() {
            return Err("Query vector cannot be empty".to_string());
        }
        if self.top_k == 0 {
            return Err("top_k must be greater than 0".to_string());
        }
        if !["cosine", "euclidean", "dot_product", "l2"].contains(&self.metric.as_str()) {
            return Err(format!("Unknown metric: {}", self.metric));
        }
        if let Some(threshold) = self.threshold {
            if !(0.0..=1.0).contains(&threshold) && self.metric == "cosine" {
                return Err("Cosine threshold must be between 0.0 and 1.0".to_string());
            }
        }
        Ok(())
    }
}

/// Graph traversal pushdown specification.
///
/// Represents a graph query that can be executed natively by ProximaDB's
/// ORION or PULSAR engines, leveraging CSR format for efficient traversal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphTraversalPushdown {
    /// Target graph name
    pub graph: String,

    /// Starting node IDs for traversal
    pub start_nodes: Vec<String>,

    /// Edge types to follow (empty = all)
    pub edge_types: Vec<String>,

    /// Traversal direction (outbound, inbound, both)
    pub direction: String,

    /// Maximum traversal depth
    pub max_depth: u32,
}

impl GraphTraversalPushdown {
    /// Create a new graph traversal pushdown.
    pub fn new(graph: impl Into<String>, start_nodes: Vec<String>) -> Self {
        Self {
            graph: graph.into(),
            start_nodes,
            edge_types: Vec::new(),
            direction: "outbound".to_string(),
            max_depth: 3,
        }
    }

    /// Set the edge types to follow.
    pub fn with_edge_types(mut self, edge_types: Vec<String>) -> Self {
        self.edge_types = edge_types;
        self
    }

    /// Set the traversal direction.
    pub fn with_direction(mut self, direction: impl Into<String>) -> Self {
        self.direction = direction.into();
        self
    }

    /// Set the maximum depth.
    pub fn with_max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Validate the pushdown specification.
    pub fn validate(&self) -> Result<(), String> {
        if self.start_nodes.is_empty() {
            return Err("At least one start node is required".to_string());
        }
        if self.max_depth == 0 {
            return Err("max_depth must be greater than 0".to_string());
        }
        if !["outbound", "inbound", "both"].contains(&self.direction.as_str()) {
            return Err(format!("Unknown direction: {}", self.direction));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expr_builder() {
        let expr = Expr::and(vec![
            Expr::eq(Expr::column("category"), Expr::literal("science")),
            Expr::gt(Expr::column("score"), Expr::literal(0.8f64)),
        ]);

        assert!(expr.references_column("category"));
        assert!(expr.references_column("score"));
        assert!(!expr.references_column("name"));
    }

    #[test]
    fn test_referenced_columns() {
        let expr = Expr::or(vec![
            Expr::eq(Expr::column("a"), Expr::literal(1)),
            Expr::eq(Expr::column("b"), Expr::literal(2)),
            Expr::eq(Expr::column("a"), Expr::literal(3)),
        ]);

        let columns = expr.referenced_columns();
        assert_eq!(columns, vec!["a", "b"]);
    }

    #[test]
    fn test_vector_search_validation() {
        let valid = VectorSearchPushdown::new("vectors", vec![0.1, 0.2, 0.3], 10);
        assert!(valid.validate().is_ok());

        let empty_vector = VectorSearchPushdown::new("vectors", vec![], 10);
        assert!(empty_vector.validate().is_err());

        let zero_k = VectorSearchPushdown::new("vectors", vec![0.1], 0);
        assert!(zero_k.validate().is_err());
    }

    #[test]
    fn test_graph_traversal_validation() {
        let valid = GraphTraversalPushdown::new("graph", vec!["n1".to_string()]);
        assert!(valid.validate().is_ok());

        let no_start = GraphTraversalPushdown::new("graph", vec![]);
        assert!(no_start.validate().is_err());

        let bad_direction = GraphTraversalPushdown::new("graph", vec!["n1".to_string()])
            .with_direction("invalid");
        assert!(bad_direction.validate().is_err());
    }

    #[test]
    fn test_pushdown_response() {
        let request = PushdownRequest::new()
            .with_projections(vec!["id".to_string(), "name".to_string()])
            .with_limit(100);

        let response = PushdownResponse::accept_all(&request)
            .with_estimated_rows(50);

        assert!(response.has_pushdown());
        assert_eq!(response.accepted_projections.len(), 2);
        assert!(response.accepts_limit);
        assert_eq!(response.estimated_rows, Some(50));
    }

    #[test]
    fn test_agg_expr_builder() {
        let agg = AggExpr::count("id")
            .distinct()
            .filter(Expr::gt(Expr::column("score"), Expr::literal(0.5f64)))
            .alias("unique_count");

        assert_eq!(agg.function, AggFunction::Count);
        assert_eq!(agg.column, Some("id".to_string()));
        assert!(agg.distinct);
        assert!(agg.filter.is_some());
        assert_eq!(agg.alias, Some("unique_count".to_string()));
    }

    #[test]
    fn test_literal_display() {
        assert_eq!(format!("{}", LiteralValue::Null), "NULL");
        assert_eq!(format!("{}", LiteralValue::Boolean(true)), "true");
        assert_eq!(format!("{}", LiteralValue::Int64(42)), "42");
        assert_eq!(format!("{}", LiteralValue::String("hello".to_string())), "'hello'");
    }
}
