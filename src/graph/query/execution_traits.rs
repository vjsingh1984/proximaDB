//! Query execution traits and types
//!
//! Defines the core traits for the query execution framework using the Volcano iterator model.
//! This module provides:
//! - `PhysicalOperator` trait for query execution operators
//! - `QueryValue` enum supporting all graph types (nodes, edges, paths, properties)
//! - `ResultTuple` for streaming query results
//!
//! Design principles:
//! - **Single Responsibility**: Each operator does one thing
//! - **Open-Closed**: New operators without modifying existing code
//! - **Dependency Inversion**: Operators depend on traits, not concrete implementations

use crate::graph::engines::GraphEngine;
use crate::proto::proximadb_v1::{Edge, Node, PropertyValue};
use anyhow::Result;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Physical operator trait implementing the Volcano iterator model
///
/// This trait enables streaming query execution where each operator:
/// 1. Opens resources (via `open()`)
/// 2. Produces results incrementally (via `next()`)
/// 3. Cleans up resources (via `close()`)
///
/// # Design
///
/// - **Streaming**: Results produced on-demand, not materialized upfront
/// - **Composable**: Operators can be chained (e.g., Scan → Filter → Project)
/// - **Extensible**: New operators implement this trait without modifying existing code
///
/// # Example
///
/// ```ignore
/// let mut scan = NodeScanOperator::new(engine, Some("Person".to_string()));
/// scan.open()?;
/// while let Some(tuple) = scan.next()? {
///     // Process tuple
/// }
/// scan.close()?;
/// ```
pub trait PhysicalOperator: Send + Sync {
    /// Initialize operator state
    ///
    /// Called once before query execution. Operators should:
    /// - Acquire resources (e.g., iterators, file handles)
    /// - Perform one-time setup (e.g., building hash tables for joins)
    fn open(&mut self) -> Result<()>;

    /// Get next result tuple (streaming)
    ///
    /// Returns:
    /// - `Ok(Some(tuple))` if more results available
    /// - `Ok(None)` if end of results
    /// - `Err(e)` on execution error
    ///
    /// # Execution Model
    ///
    /// Operators follow a pull-based model:
    /// - Child operators are pulled via their `next()` method
    /// - Tuples are produced incrementally
    /// - Memory usage is O(pipeline depth), not O(result size)
    fn next(&mut self) -> Result<Option<ResultTuple>>;

    /// Cleanup resources
    ///
    /// Called once after query execution completes.
    /// Operators should release resources (e.g., close iterators, drop caches).
    fn close(&mut self) -> Result<()>;

    /// Estimated output cardinality
    ///
    /// Used by query planner for cost-based optimization.
    /// Return 0 if unknown.
    fn estimated_cardinality(&self) -> usize;

    /// Output schema (column names and types)
    ///
    /// Describes the structure of result tuples produced by this operator.
    fn schema(&self) -> &[ColumnSpec];
}

/// Column specification for result schema
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    pub name: String,
    pub value_type: ValueType,
}

/// Type of query value
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Node,
    Edge,
    Path,
    Property,
    List,
    Null,
}

/// Query value enum supporting all graph types
///
/// Represents values that can appear in query results:
/// - **Node**: Full node with properties
/// - **Edge**: Full edge with properties
/// - **Path**: Sequence of nodes and edges
/// - **Property**: Scalar value (string, int, float, bool, etc.)
/// - **List**: Array of values
/// - **Null**: Absence of value
///
/// # Design
///
/// Uses `Arc<Node>` and `Arc<Edge>` for zero-copy sharing with graph engine.
/// No cloning overhead when passing nodes/edges between operators.
#[derive(Clone)]
pub enum QueryValue {
    /// Graph node (Arc for zero-copy)
    Node(Arc<Node>),

    /// Graph edge (Arc for zero-copy)
    Edge(Arc<Edge>),

    /// Path (sequence of nodes and edges)
    Path(Vec<PathElement>),

    /// Scalar property value
    Property(PropertyValue),

    /// List of values
    List(Vec<QueryValue>),

    /// Null value
    Null,
}

impl QueryValue {
    /// Get value type
    pub fn value_type(&self) -> ValueType {
        match self {
            QueryValue::Node(_) => ValueType::Node,
            QueryValue::Edge(_) => ValueType::Edge,
            QueryValue::Path(_) => ValueType::Path,
            QueryValue::Property(_) => ValueType::Property,
            QueryValue::List(_) => ValueType::List,
            QueryValue::Null => ValueType::Null,
        }
    }

    /// Check if value is null
    pub fn is_null(&self) -> bool {
        matches!(self, QueryValue::Null)
    }

    /// Extract node reference (returns None if not a node)
    pub fn as_node(&self) -> Option<&Arc<Node>> {
        match self {
            QueryValue::Node(node) => Some(node),
            _ => None,
        }
    }

    /// Extract edge reference (returns None if not an edge)
    pub fn as_edge(&self) -> Option<&Arc<Edge>> {
        match self {
            QueryValue::Edge(edge) => Some(edge),
            _ => None,
        }
    }

    /// Extract property reference (returns None if not a property)
    pub fn as_property(&self) -> Option<&PropertyValue> {
        match self {
            QueryValue::Property(prop) => Some(prop),
            _ => None,
        }
    }

    /// Extract list reference (returns None if not a list)
    pub fn as_list(&self) -> Option<&[QueryValue]> {
        match self {
            QueryValue::List(list) => Some(list),
            _ => None,
        }
    }
}

impl fmt::Debug for QueryValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryValue::Node(node) => write!(f, "Node(id={})", node.id),
            QueryValue::Edge(edge) => write!(
                f,
                "Edge(id={}, {} → {})",
                edge.id, edge.from_node_id, edge.to_node_id
            ),
            QueryValue::Path(path) => write!(f, "Path(len={})", path.len()),
            QueryValue::Property(prop) => write!(f, "Property({:?})", prop),
            QueryValue::List(list) => write!(f, "List(len={})", list.len()),
            QueryValue::Null => write!(f, "Null"),
        }
    }
}

/// Path element (node or edge in a path)
#[derive(Debug, Clone)]
pub enum PathElement {
    Node(Arc<Node>),
    Edge(Arc<Edge>),
}

/// Result tuple with named bindings
///
/// Represents one row in query results. Each tuple contains named bindings
/// mapping variable names to values.
///
/// # Example
///
/// For query: `MATCH (p:Person)-[r:KNOWS]->(f:Person) RETURN p, r, f`
///
/// A result tuple might contain:
/// ```ignore
/// {
///   "p": Node(id="person1"),
///   "r": Edge(id="edge1"),
///   "f": Node(id="person2"),
/// }
/// ```
#[derive(Clone)]
pub struct ResultTuple {
    /// Column bindings (variable name → value)
    pub bindings: HashMap<String, QueryValue>,
}

impl ResultTuple {
    /// Create new empty tuple
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Create tuple with initial bindings
    pub fn with_bindings(bindings: HashMap<String, QueryValue>) -> Self {
        Self { bindings }
    }

    /// Get value by variable name
    pub fn get(&self, name: &str) -> Option<&QueryValue> {
        self.bindings.get(name)
    }

    /// Set value for variable name
    pub fn set(&mut self, name: String, value: QueryValue) {
        self.bindings.insert(name, value);
    }

    /// Check if variable exists
    pub fn contains(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }

    /// Merge another tuple into this one
    pub fn merge(&mut self, other: ResultTuple) {
        self.bindings.extend(other.bindings);
    }

    /// Get number of bindings
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Check if tuple is empty
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

impl Default for ResultTuple {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ResultTuple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResultTuple")
            .field("bindings", &self.bindings)
            .finish()
    }
}

/// Execution statistics for query profiling
#[derive(Debug, Clone, Default)]
pub struct ExecutionStats {
    /// Total rows processed
    pub rows_processed: usize,

    /// Total execution time (milliseconds)
    pub execution_time_ms: u64,

    /// Number of index seeks
    pub index_seeks: usize,

    /// Number of full table scans
    pub full_scans: usize,

    /// Number of cross-shard operations
    pub cross_shard_ops: usize,

    /// Cache hit rate (0.0 - 1.0)
    pub cache_hit_rate: f64,
}

/// Query execution context
///
/// Provides context for query execution including:
/// - Graph engine for data access
/// - Timeout and resource limits
/// - Profiling and tracing settings
pub struct ExecutionContext {
    /// Graph engine for data access
    pub engine: Arc<dyn GraphEngine>,

    /// Maximum execution time (milliseconds)
    pub timeout_ms: Option<u64>,

    /// Maximum rows to return
    pub limit: Option<usize>,

    /// Enable query profiling
    pub profile: bool,

    /// Execution statistics (accumulated during execution)
    pub stats: ExecutionStats,
}

impl ExecutionContext {
    /// Create new execution context
    pub fn new(engine: Arc<dyn GraphEngine>) -> Self {
        Self {
            engine,
            timeout_ms: None,
            limit: None,
            profile: false,
            stats: ExecutionStats::default(),
        }
    }

    /// Set timeout in milliseconds
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Set result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Enable profiling
    pub fn with_profiling(mut self) -> Self {
        self.profile = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_value_type() {
        let node_val = QueryValue::Node(Arc::new(Node {
            id: "n1".to_string(),
            ..Default::default()
        }));
        assert_eq!(node_val.value_type(), ValueType::Node);
        assert!(!node_val.is_null());

        let null_val = QueryValue::Null;
        assert_eq!(null_val.value_type(), ValueType::Null);
        assert!(null_val.is_null());
    }

    #[test]
    fn test_result_tuple_operations() {
        let mut tuple = ResultTuple::new();
        assert!(tuple.is_empty());

        tuple.set("x".to_string(), QueryValue::Null);
        assert_eq!(tuple.len(), 1);
        assert!(tuple.contains("x"));

        let mut other = ResultTuple::new();
        other.set("y".to_string(), QueryValue::Null);
        tuple.merge(other);
        assert_eq!(tuple.len(), 2);
        assert!(tuple.contains("y"));
    }

    #[test]
    fn test_query_value_as_methods() {
        let node = Arc::new(Node {
            id: "n1".to_string(),
            ..Default::default()
        });
        let node_val = QueryValue::Node(node.clone());

        assert!(node_val.as_node().is_some());
        assert!(node_val.as_edge().is_none());
        assert!(node_val.as_property().is_none());
    }

    #[test]
    fn test_column_spec() {
        let spec = ColumnSpec {
            name: "node_id".to_string(),
            value_type: ValueType::Property,
        };

        assert_eq!(spec.name, "node_id");
        assert_eq!(spec.value_type, ValueType::Property);
    }
}
