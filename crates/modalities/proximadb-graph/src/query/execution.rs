//! Shared graph query execution contracts.
//!
//! This crate owns the engine-agnostic execution vocabulary that graph query
//! planning and runtime code can share without depending on root crate wiring.

use anyhow::Result;
use proximadb_proto::proximadb_v1::{Edge, Node, PropertyValue};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Physical operator trait implementing the Volcano iterator model.
pub trait PhysicalOperator: Send + Sync {
    /// Initialize operator state.
    fn open(&mut self) -> Result<()>;

    /// Get the next result tuple.
    fn next(&mut self) -> Result<Option<ResultTuple>>;

    /// Cleanup resources.
    fn close(&mut self) -> Result<()>;

    /// Estimated output cardinality.
    fn estimated_cardinality(&self) -> usize;

    /// Output schema.
    fn schema(&self) -> &[ColumnSpec];
}

/// Column specification for result schema describing one output column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    /// Column name (typically the variable name from the query).
    pub name: String,
    /// Data type of values in this column.
    pub value_type: ValueType,
}

/// Type of query value produced by graph query operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    /// Graph node value.
    Node,
    /// Graph edge value.
    Edge,
    /// Path (sequence of nodes and edges).
    Path,
    /// Scalar property value.
    Property,
    /// List of values.
    List,
    /// Null (absence of value).
    Null,
}

/// Query value enum supporting all graph result shapes.
#[derive(Clone)]
pub enum QueryValue {
    /// Graph node (Arc for zero-copy).
    Node(Arc<Node>),
    /// Graph edge (Arc for zero-copy).
    Edge(Arc<Edge>),
    /// Path (sequence of nodes and edges).
    Path(Vec<PathElement>),
    /// Scalar property value.
    Property(PropertyValue),
    /// List of values.
    List(Vec<QueryValue>),
    /// Null value.
    Null,
}

impl QueryValue {
    /// Get value type.
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

    /// Check if value is null.
    pub fn is_null(&self) -> bool {
        matches!(self, QueryValue::Null)
    }

    /// Extract node reference.
    pub fn as_node(&self) -> Option<&Arc<Node>> {
        match self {
            QueryValue::Node(node) => Some(node),
            _ => None,
        }
    }

    /// Extract edge reference.
    pub fn as_edge(&self) -> Option<&Arc<Edge>> {
        match self {
            QueryValue::Edge(edge) => Some(edge),
            _ => None,
        }
    }

    /// Extract property reference.
    pub fn as_property(&self) -> Option<&PropertyValue> {
        match self {
            QueryValue::Property(prop) => Some(prop),
            _ => None,
        }
    }

    /// Extract list reference.
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

/// Path element (node or edge in a path).
#[derive(Debug, Clone)]
pub enum PathElement {
    /// A graph node in the path.
    Node(Arc<Node>),
    /// A graph edge connecting two nodes in the path.
    Edge(Arc<Edge>),
}

/// Result tuple with named bindings.
#[derive(Clone)]
pub struct ResultTuple {
    /// Column bindings (variable name → value).
    pub bindings: HashMap<String, QueryValue>,
}

impl ResultTuple {
    /// Create new empty tuple.
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Create tuple with initial bindings.
    pub fn with_bindings(bindings: HashMap<String, QueryValue>) -> Self {
        Self { bindings }
    }

    /// Get value by variable name.
    pub fn get(&self, name: &str) -> Option<&QueryValue> {
        self.bindings.get(name)
    }

    /// Set value for variable name.
    pub fn set(&mut self, name: String, value: QueryValue) {
        self.bindings.insert(name, value);
    }

    /// Check if variable exists.
    pub fn contains(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }

    /// Merge another tuple into this one.
    pub fn merge(&mut self, other: ResultTuple) {
        self.bindings.extend(other.bindings);
    }

    /// Get number of bindings.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Check if tuple is empty.
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

/// Execution statistics for query profiling.
#[derive(Debug, Clone, Default)]
pub struct ExecutionStats {
    /// Total rows processed.
    pub rows_processed: usize,
    /// Total execution time (milliseconds).
    pub execution_time_ms: u64,
    /// Number of index seeks.
    pub index_seeks: usize,
    /// Number of full table scans.
    pub full_scans: usize,
    /// Number of cross-shard operations.
    pub cross_shard_ops: usize,
    /// Cache hit rate (0.0 - 1.0).
    pub cache_hit_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_proto::proximadb_v1::property_value::Value;

    fn string_property(value: &str) -> PropertyValue {
        PropertyValue {
            value: Some(Value::StringValue(value.to_string())),
        }
    }

    #[test]
    fn query_value_type_and_accessors_cover_node_edge_property_and_list() {
        let node = Arc::new(Node {
            id: "n1".to_string(),
            ..Default::default()
        });
        let edge = Arc::new(Edge {
            id: "e1".to_string(),
            from_node_id: "n1".to_string(),
            to_node_id: "n2".to_string(),
            ..Default::default()
        });
        let property = string_property("Alice");
        let node_val = QueryValue::Node(node);
        let edge_val = QueryValue::Edge(edge.clone());
        let property_val = QueryValue::Property(property.clone());
        let list_val = QueryValue::List(vec![property_val.clone(), QueryValue::Null]);
        let path_val = QueryValue::Path(vec![PathElement::Edge(edge)]);

        assert_eq!(node_val.value_type(), ValueType::Node);
        assert_eq!(path_val.value_type(), ValueType::Path);
        assert_eq!(list_val.value_type(), ValueType::List);
        assert!(node_val.as_node().is_some());
        assert!(node_val.as_edge().is_none());
        assert!(edge_val.as_edge().is_some());
        assert!(property_val.as_property().is_some());
        assert_eq!(list_val.as_list().map(|list| list.len()), Some(2));
        assert_eq!(format!("{path_val:?}"), "Path(len=1)");
        assert_eq!(format!("{edge_val:?}"), "Edge(id=e1, n1 → n2)");
    }

    #[test]
    fn result_tuple_helpers_preserve_bindings_and_debug_shape() {
        let mut bindings = HashMap::new();
        bindings.insert(
            "name".to_string(),
            QueryValue::Property(string_property("Alice")),
        );

        let mut tuple = ResultTuple::with_bindings(bindings);
        assert!(!tuple.is_empty());
        assert!(matches!(tuple.get("name"), Some(QueryValue::Property(_))));

        tuple.set("missing".to_string(), QueryValue::Null);
        let debug = format!("{tuple:?}");
        assert!(debug.contains("ResultTuple"));
        assert!(debug.contains("name"));

        let mut other = ResultTuple::new();
        other.set(
            "age".to_string(),
            QueryValue::Property(PropertyValue {
                value: Some(Value::IntValue(30)),
            }),
        );
        tuple.merge(other);

        assert_eq!(tuple.len(), 3);
        assert!(tuple.contains("age"));
        assert!(ResultTuple::default().is_empty());
    }

    #[test]
    fn column_spec_and_stats_defaults_are_stable() {
        let spec = ColumnSpec {
            name: "node_id".to_string(),
            value_type: ValueType::Property,
        };
        let stats = ExecutionStats::default();

        assert_eq!(spec.name, "node_id");
        assert_eq!(spec.value_type, ValueType::Property);
        assert_eq!(stats.rows_processed, 0);
        assert_eq!(stats.execution_time_ms, 0);
        assert_eq!(stats.index_seeks, 0);
        assert_eq!(stats.full_scans, 0);
        assert_eq!(stats.cross_shard_ops, 0);
        assert_eq!(stats.cache_hit_rate, 0.0);
    }
}
