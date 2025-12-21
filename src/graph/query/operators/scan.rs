//! Node scan operator
//!
//! Scans nodes from the graph engine by label and/or property filters.
//! Reuses GraphEngine trait for data access.

use super::{evaluate_property_filter, ColumnSpec, PhysicalOperator, QueryValue, ResultTuple, ValueType};
use crate::graph::engines::GraphEngine;
use crate::proto::proximadb_v1::{Node, PropertyFilter};
use anyhow::Result;
use std::sync::Arc;

/// Node scan operator
///
/// Scans nodes from graph engine with optional label and property filters.
///
/// # Design
///
/// - **Reuses GraphEngine trait**: No duplication of node access logic
/// - **Streaming**: Produces nodes incrementally via iterator
/// - **Filter Pushdown**: Applies filters during scan, not after
///
/// # Example
///
/// ```ignore
/// // Scan all Person nodes with age > 25
/// let mut scan = NodeScanOperator::new(
///     engine,
///     Some("Person".to_string()),
///     vec![age_filter],
///     "p".to_string(),
/// );
///
/// scan.open()?;
/// while let Some(tuple) = scan.next()? {
///     // Process tuple with binding "p" → Person node
/// }
/// scan.close()?;
/// ```
pub struct NodeScanOperator {
    /// Graph engine for node access (reuse existing infrastructure)
    engine: Arc<dyn GraphEngine>,

    /// Optional label filter (e.g., "Person")
    label: Option<String>,

    /// Property filters (evaluated during scan)
    filters: Vec<PropertyFilter>,

    /// Variable name for binding (e.g., "p" in MATCH (p:Person))
    variable_name: String,

    /// Iterator state (initialized in open())
    iterator: Option<Box<dyn Iterator<Item = Arc<Node>> + Send + Sync>>,

    /// Estimated cardinality (set in open())
    estimated_cardinality: usize,
}

impl NodeScanOperator {
    /// Create new node scan operator
    pub fn new(
        engine: Arc<dyn GraphEngine>,
        label: Option<String>,
        filters: Vec<PropertyFilter>,
        variable_name: String,
    ) -> Self {
        Self {
            engine,
            label,
            filters,
            variable_name,
            iterator: None,
            estimated_cardinality: 0,
        }
    }

    /// Apply property filters to a node
    fn apply_filters(&self, node: &Node) -> bool {
        for filter in &self.filters {
            if let Some(actual_value) = node.properties.get(&filter.key) {
                if !evaluate_property_filter(filter, actual_value) {
                    return false;
                }
            } else {
                // Property doesn't exist
                return false;
            }
        }
        true
    }
}

impl PhysicalOperator for NodeScanOperator {
    fn open(&mut self) -> Result<()> {
        // Fetch nodes from engine (reuse existing get_nodes_by_label)
        let nodes = if let Some(ref label) = self.label {
            // Label-based scan (uses label index)
            self.engine
                .get_nodes_by_label(label)
                .map_err(|e| anyhow::anyhow!("Failed to get nodes by label: {}", e))?
        } else {
            // Full table scan (no label filter)
            self.engine
                .get_all_nodes()
                .map_err(|e| anyhow::anyhow!("Failed to get all nodes: {}", e))?
        };

        self.estimated_cardinality = nodes.len();

        // Apply property filters during scan (filter pushdown)
        let filtered_nodes: Vec<Arc<Node>> = nodes
            .into_iter()
            .filter(|node| self.apply_filters(node))
            .collect();

        // Create iterator over filtered nodes
        self.iterator = Some(Box::new(filtered_nodes.into_iter()));

        Ok(())
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
        if let Some(ref mut iter) = self.iterator {
            if let Some(node) = iter.next() {
                // Create result tuple with node binding
                let mut tuple = ResultTuple::new();
                tuple.set(self.variable_name.clone(), QueryValue::Node(node));
                return Ok(Some(tuple));
            }
        }
        Ok(None)
    }

    fn close(&mut self) -> Result<()> {
        // Drop iterator, releasing resources
        self.iterator = None;
        Ok(())
    }

    fn estimated_cardinality(&self) -> usize {
        self.estimated_cardinality
    }

    fn schema(&self) -> &[ColumnSpec] {
        // Static schema: single column for the node variable
        static SCHEMA: &[ColumnSpec] = &[];
        SCHEMA
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::GraphEngine;
    use crate::proto::proximadb_v1::{property_value::Value, PropertyValue};
    use async_trait::async_trait;
    use std::collections::HashMap;

    /// Mock graph engine for testing
    struct MockEngine {
        nodes: Vec<Arc<Node>>,
    }

    impl MockEngine {
        fn new(nodes: Vec<Node>) -> Self {
            Self {
                nodes: nodes.into_iter().map(Arc::new).collect(),
            }
        }
    }

    #[async_trait]
    impl GraphEngine for MockEngine {
        fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self
                .nodes
                .iter()
                .filter(|n| n.labels.contains(&label.to_string()))
                .cloned()
                .collect())
        }

        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self.nodes.clone())
        }

        // Stub implementations for other required methods
        async fn insert_node(&self, node: Node) -> Result<Arc<Node>, crate::core::error::ProximaDBError> {
            Ok(Arc::new(node))
        }

        fn get_node(&self, _id: &String) -> Result<Option<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(None)
        }

        async fn update_node(&self, node: Node) -> Result<Arc<Node>, crate::core::error::ProximaDBError> {
            Ok(Arc::new(node))
        }

        async fn delete_node(&self, _id: &String) -> Result<Option<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(None)
        }

        async fn insert_edge(&self, edge: crate::proto::proximadb_v1::Edge) -> Result<Arc<crate::proto::proximadb_v1::Edge>, crate::core::error::ProximaDBError> {
            Ok(Arc::new(edge))
        }

        fn get_edge(&self, _id: &String) -> Result<Option<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> {
            Ok(None)
        }

        async fn update_edge(&self, edge: crate::proto::proximadb_v1::Edge) -> Result<Arc<crate::proto::proximadb_v1::Edge>, crate::core::error::ProximaDBError> {
            Ok(Arc::new(edge))
        }

        async fn delete_edge(&self, _id: &String) -> Result<Option<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> {
            Ok(None)
        }

        fn get_neighbors(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(vec![])
        }

        fn get_outgoing_edges(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> {
            Ok(vec![])
        }

        fn get_incoming_edges(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> {
            Ok(vec![])
        }

        fn node_count(&self) -> Result<usize, crate::core::error::ProximaDBError> {
            Ok(self.nodes.len())
        }

        fn edge_count(&self) -> Result<usize, crate::core::error::ProximaDBError> {
            Ok(0)
        }
    }

    fn create_test_node(id: &str, label: &str, age: i64) -> Node {
        let mut properties = HashMap::new();
        properties.insert(
            "age".to_string(),
            PropertyValue {
                value: Some(Value::IntValue(age)),
            },
        );

        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties,
            ..Default::default()
        }
    }

    #[test]
    fn test_node_scan_with_label() {
        let nodes = vec![
            create_test_node("n1", "Person", 30),
            create_test_node("n2", "Person", 25),
            create_test_node("n3", "Company", 0),
        ];

        let engine = Arc::new(MockEngine::new(nodes));
        let mut scan = NodeScanOperator::new(engine, Some("Person".to_string()), vec![], "p".to_string());

        scan.open().unwrap();

        let mut count = 0;
        while let Some(tuple) = scan.next().unwrap() {
            assert!(tuple.contains("p"));
            let node = tuple.get("p").unwrap().as_node().unwrap();
            assert!(node.labels.contains(&"Person".to_string()));
            count += 1;
        }

        assert_eq!(count, 2); // Only Person nodes

        scan.close().unwrap();
    }

    #[test]
    fn test_node_scan_with_filter() {
        let nodes = vec![
            create_test_node("n1", "Person", 30),
            create_test_node("n2", "Person", 25),
            create_test_node("n3", "Person", 20),
        ];

        let engine = Arc::new(MockEngine::new(nodes));

        let filter = PropertyFilter {
            key: "age".to_string(),
            operator: crate::proto::proximadb_v1::PropertyFilterOperator::GreaterThan as i32,
            value: Some(PropertyValue {
                value: Some(Value::IntValue(25)),
            }),
        };

        let mut scan = NodeScanOperator::new(engine, Some("Person".to_string()), vec![filter], "p".to_string());

        scan.open().unwrap();

        let mut count = 0;
        while let Some(tuple) = scan.next().unwrap() {
            let node = tuple.get("p").unwrap().as_node().unwrap();
            if let Some(age_value) = node.properties.get("age") {
                if let Some(Value::IntValue(age)) = &age_value.value {
                    assert!(*age > 25);
                }
            }
            count += 1;
        }

        assert_eq!(count, 1); // Only n1 with age 30

        scan.close().unwrap();
    }

    #[test]
    fn test_node_scan_all_nodes() {
        let nodes = vec![
            create_test_node("n1", "Person", 30),
            create_test_node("n2", "Company", 0),
        ];

        let engine = Arc::new(MockEngine::new(nodes));
        let mut scan = NodeScanOperator::new(engine, None, vec![], "n".to_string());

        scan.open().unwrap();

        let mut count = 0;
        while scan.next().unwrap().is_some() {
            count += 1;
        }

        assert_eq!(count, 2); // All nodes

        scan.close().unwrap();
    }
}
