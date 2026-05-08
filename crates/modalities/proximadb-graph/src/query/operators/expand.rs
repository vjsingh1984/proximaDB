use super::{
    ColumnSpec, EdgeDirection, PhysicalOperator, QueryValue, ResultTuple, evaluate_property_filter,
};
use crate::query::storage::GraphQueryStorage;
use anyhow::Result;
use proximadb_proto::proximadb_v1::{Edge, Node, PropertyFilter};
use std::sync::Arc;

/// Edge expansion operator.
pub struct ExpandOperator {
    input: Box<dyn PhysicalOperator>,
    storage: Arc<dyn GraphQueryStorage>,
    from_variable: String,
    edge_variable: Option<String>,
    to_variable: String,
    direction: EdgeDirection,
    edge_types: Vec<String>,
    filters: Vec<PropertyFilter>,
    current_input: Option<ResultTuple>,
    edge_iterator: Option<Box<dyn Iterator<Item = (Arc<Edge>, Arc<Node>)> + Send + Sync>>,
    estimated_cardinality: usize,
}

impl ExpandOperator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        input: Box<dyn PhysicalOperator>,
        storage: Arc<dyn GraphQueryStorage>,
        from_variable: String,
        edge_variable: Option<String>,
        to_variable: String,
        direction: EdgeDirection,
        edge_types: Vec<String>,
        filters: Vec<PropertyFilter>,
    ) -> Self {
        Self {
            input,
            storage,
            from_variable,
            edge_variable,
            to_variable,
            direction,
            edge_types,
            filters,
            current_input: None,
            edge_iterator: None,
            estimated_cardinality: 0,
        }
    }

    fn apply_edge_filters(&self, edge: &Edge) -> bool {
        self.filters.iter().all(|filter| {
            edge.properties
                .get(&filter.key)
                .is_some_and(|actual_value| evaluate_property_filter(filter, actual_value))
        })
    }

    fn fetch_edges_for_node(&self, node_id: &str) -> Result<Vec<(Arc<Edge>, Arc<Node>)>> {
        let edges = match self.direction {
            EdgeDirection::Outgoing => self.storage.get_outgoing_edges(node_id, None)?,
            EdgeDirection::Incoming => self.storage.get_incoming_edges(node_id, None)?,
            EdgeDirection::Bidirectional => {
                let mut all_edges = self.storage.get_outgoing_edges(node_id, None)?;
                all_edges.extend(self.storage.get_incoming_edges(node_id, None)?);
                all_edges
            }
        };

        let filtered_edges: Vec<Arc<Edge>> = if self.edge_types.is_empty() {
            edges
        } else {
            edges
                .into_iter()
                .filter(|e| self.edge_types.contains(&e.edge_type))
                .collect()
        };

        let mut edge_node_pairs = Vec::new();
        for edge in filtered_edges
            .into_iter()
            .filter(|e| self.apply_edge_filters(e))
        {
            let target_id = match self.direction {
                EdgeDirection::Outgoing => edge.to_node_id.as_str(),
                EdgeDirection::Incoming => edge.from_node_id.as_str(),
                EdgeDirection::Bidirectional => {
                    if edge.from_node_id == node_id {
                        edge.to_node_id.as_str()
                    } else {
                        edge.from_node_id.as_str()
                    }
                }
            };

            if let Some(target_node) = self.storage.get_node(target_id)? {
                edge_node_pairs.push((edge, target_node));
            }
        }

        Ok(edge_node_pairs)
    }
}

impl PhysicalOperator for ExpandOperator {
    fn open(&mut self) -> Result<()> {
        self.input.open()?;
        self.estimated_cardinality = self.input.estimated_cardinality() * 10;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
        loop {
            if let Some(ref mut iter) = self.edge_iterator
                && let Some((edge, target_node)) = iter.next()
            {
                let mut result = self
                    .current_input
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("missing current input in expand operator"))?
                    .clone();

                if let Some(ref edge_var) = self.edge_variable {
                    result.set(edge_var.clone(), QueryValue::Edge(edge));
                }
                result.set(self.to_variable.clone(), QueryValue::Node(target_node));
                return Ok(Some(result));
            }

            if let Some(input_tuple) = self.input.next()? {
                let from_node = match input_tuple.get(&self.from_variable) {
                    Some(QueryValue::Node(node)) => node.clone(),
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Expected node for variable '{}'",
                            self.from_variable
                        ));
                    }
                };
                let edge_pairs = self.fetch_edges_for_node(&from_node.id)?;
                self.current_input = Some(input_tuple);
                self.edge_iterator = Some(Box::new(edge_pairs.into_iter()));
            } else {
                return Ok(None);
            }
        }
    }

    fn close(&mut self) -> Result<()> {
        self.edge_iterator = None;
        self.current_input = None;
        self.input.close()
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
    use crate::query::operators::scan::NodeScanOperator;
    use crate::query::storage::GraphQueryStorage;
    use std::collections::HashMap;

    struct MockStorage {
        nodes: HashMap<String, Arc<Node>>,
        edges: Vec<Arc<Edge>>,
    }

    impl MockStorage {
        fn new() -> Self {
            let mut nodes = HashMap::new();
            nodes.insert(
                "alice".to_string(),
                Arc::new(Node {
                    id: "alice".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::new(),
                    ..Default::default()
                }),
            );
            nodes.insert(
                "bob".to_string(),
                Arc::new(Node {
                    id: "bob".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::new(),
                    ..Default::default()
                }),
            );
            let edge = Arc::new(Edge {
                id: "e1".to_string(),
                from_node_id: "alice".to_string(),
                to_node_id: "bob".to_string(),
                edge_type: "KNOWS".to_string(),
                properties: HashMap::new(),
                ..Default::default()
            });
            Self {
                nodes,
                edges: vec![edge],
            }
        }
    }

    impl GraphQueryStorage for MockStorage {
        fn get_node(&self, id: &str) -> Result<Option<Arc<Node>>> {
            Ok(self.nodes.get(id).cloned())
        }

        fn get_nodes_by_label(&self, _label: &str) -> Result<Vec<Arc<Node>>> {
            Ok(self.nodes.values().cloned().collect())
        }

        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
            Ok(self.nodes.values().cloned().collect())
        }

        fn get_outgoing_edges(
            &self,
            node_id: &str,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>> {
            Ok(self
                .edges
                .iter()
                .filter(|e| e.from_node_id == node_id)
                .cloned()
                .collect())
        }

        fn get_incoming_edges(
            &self,
            node_id: &str,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>> {
            Ok(self
                .edges
                .iter()
                .filter(|e| e.to_node_id == node_id)
                .cloned()
                .collect())
        }
    }

    #[test]
    fn expand_operator_yields_neighbor_and_edge() {
        let storage = Arc::new(MockStorage::new());
        let scan = NodeScanOperator::new(
            storage.clone(),
            Some("Person".to_string()),
            vec![],
            "p".to_string(),
        );
        let mut expand = ExpandOperator::new(
            Box::new(scan),
            storage,
            "p".to_string(),
            Some("r".to_string()),
            "f".to_string(),
            EdgeDirection::Outgoing,
            vec!["KNOWS".to_string()],
            vec![],
        );

        expand.open().unwrap();
        let tuple = expand.next().unwrap().expect("expected one expanded tuple");
        assert!(tuple.contains("p"));
        assert!(tuple.contains("r"));
        assert!(tuple.contains("f"));
        assert_eq!(
            tuple.get("r").unwrap().as_edge().unwrap().edge_type,
            "KNOWS"
        );
    }
}
