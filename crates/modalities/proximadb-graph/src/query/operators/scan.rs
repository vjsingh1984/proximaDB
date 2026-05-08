use super::{ColumnSpec, PhysicalOperator, QueryValue, ResultTuple, evaluate_property_filter};
use crate::query::storage::GraphQueryStorage;
use anyhow::Result;
use proximadb_proto::proximadb_v1::{Node, PropertyFilter};
use std::sync::Arc;

/// Node scan operator.
pub struct NodeScanOperator {
    storage: Arc<dyn GraphQueryStorage>,
    label: Option<String>,
    filters: Vec<PropertyFilter>,
    variable_name: String,
    iterator: Option<Box<dyn Iterator<Item = Arc<Node>> + Send + Sync>>,
    estimated_cardinality: usize,
}

impl NodeScanOperator {
    /// Create a new node scan operator.
    pub fn new(
        storage: Arc<dyn GraphQueryStorage>,
        label: Option<String>,
        filters: Vec<PropertyFilter>,
        variable_name: String,
    ) -> Self {
        Self {
            storage,
            label,
            filters,
            variable_name,
            iterator: None,
            estimated_cardinality: 0,
        }
    }

    fn apply_filters(&self, node: &Node) -> bool {
        self.filters.iter().all(|filter| {
            node.properties
                .get(&filter.key)
                .is_some_and(|actual_value| evaluate_property_filter(filter, actual_value))
        })
    }
}

impl PhysicalOperator for NodeScanOperator {
    fn open(&mut self) -> Result<()> {
        let nodes = if let Some(ref label) = self.label {
            self.storage.get_nodes_by_label(label)?
        } else {
            self.storage.get_all_nodes()?
        };

        self.estimated_cardinality = nodes.len();
        let filtered_nodes: Vec<Arc<Node>> = nodes
            .into_iter()
            .filter(|node| self.apply_filters(node))
            .collect();
        self.iterator = Some(Box::new(filtered_nodes.into_iter()));
        Ok(())
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
        if let Some(ref mut iter) = self.iterator
            && let Some(node) = iter.next()
        {
            let mut tuple = ResultTuple::new();
            tuple.set(self.variable_name.clone(), QueryValue::Node(node));
            return Ok(Some(tuple));
        }
        Ok(None)
    }

    fn close(&mut self) -> Result<()> {
        self.iterator = None;
        Ok(())
    }

    fn estimated_cardinality(&self) -> usize {
        self.estimated_cardinality
    }

    fn schema(&self) -> &[ColumnSpec] {
        static SCHEMA: &[ColumnSpec] = &[];
        SCHEMA
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::storage::GraphQueryStorage;
    use proximadb_proto::proximadb_v1::{
        Edge, PropertyFilterOperator, PropertyValue, property_value::Value,
    };
    use std::collections::HashMap;

    struct MockStorage {
        nodes: Vec<Arc<Node>>,
    }

    impl MockStorage {
        fn new(nodes: Vec<Node>) -> Self {
            Self {
                nodes: nodes.into_iter().map(Arc::new).collect(),
            }
        }
    }

    impl GraphQueryStorage for MockStorage {
        fn get_node(&self, _id: &str) -> Result<Option<Arc<Node>>> {
            Ok(None)
        }

        fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
            Ok(self
                .nodes
                .iter()
                .filter(|n| n.labels.contains(&label.to_string()))
                .cloned()
                .collect())
        }

        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
            Ok(self.nodes.clone())
        }

        fn get_outgoing_edges(
            &self,
            _node_id: &str,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>> {
            Ok(vec![])
        }

        fn get_incoming_edges(
            &self,
            _node_id: &str,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>> {
            Ok(vec![])
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
    fn node_scan_filters_by_label_and_property() {
        let storage = Arc::new(MockStorage::new(vec![
            create_test_node("n1", "Person", 30),
            create_test_node("n2", "Person", 25),
            create_test_node("n3", "Company", 0),
        ]));
        let filter = PropertyFilter {
            key: "age".to_string(),
            operator: PropertyFilterOperator::GreaterThan as i32,
            value: Some(PropertyValue {
                value: Some(Value::IntValue(25)),
            }),
        };

        let mut scan = NodeScanOperator::new(
            storage,
            Some("Person".to_string()),
            vec![filter],
            "p".to_string(),
        );
        scan.open().unwrap();

        let tuple = scan.next().unwrap().expect("expected one matching tuple");
        let node = tuple.get("p").unwrap().as_node().unwrap();
        assert_eq!(node.id, "n1");
        assert!(scan.next().unwrap().is_none());
    }
}
