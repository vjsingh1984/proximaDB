//! Edge expansion operator
//!
//! Expands edges from source nodes, implementing graph traversal.
//! Reuses GraphEngine's edge traversal methods.

use super::{
    ColumnSpec, EdgeDirection, PhysicalOperator, QueryValue, ResultTuple, evaluate_property_filter,
};
use crate::core::error::ProximaDBError;
use crate::graph::engines::GraphEngine;
use crate::proto::proximadb_v1::{Edge, Node, PropertyFilter};
use anyhow::Result;
use std::sync::Arc;

/// Edge expansion operator
///
/// Expands edges from source nodes, implementing the core graph traversal operation.
///
/// # Design
///
/// - **Reuses GraphEngine**: get_outgoing_edges(), get_incoming_edges(), get_node()
/// - **Streaming**: Processes one input tuple at a time
/// - **Filter Pushdown**: Applies edge type and property filters during expansion
///
/// # Example
///
/// ```ignore
/// // Expand KNOWS edges from Person nodes
/// let mut expand = ExpandOperator::new(
///     scan_operator,      // Input: Person nodes
///     engine,
///     "p".to_string(),    // Source variable
///     Some("r".to_string()), // Edge variable
///     "f".to_string(),    // Target variable
///     EdgeDirection::Outgoing,
///     vec!["KNOWS".to_string()],
///     vec![],
/// );
///
/// expand.open()?;
/// while let Some(tuple) = expand.next()? {
///     // tuple contains: p → Person, r → KNOWS edge, f → Friend
/// }
/// ```
pub struct ExpandOperator {
    /// Input operator (e.g., NodeScanOperator)
    input: Box<dyn PhysicalOperator>,

    /// Graph engine for edge/node access
    engine: Arc<dyn GraphEngine>,

    /// Source node variable name (e.g., "p")
    from_variable: String,

    /// Optional edge variable name (e.g., "r")
    edge_variable: Option<String>,

    /// Target node variable name (e.g., "f")
    to_variable: String,

    /// Edge direction
    direction: EdgeDirection,

    /// Edge type filters (e.g., ["KNOWS", "FOLLOWS"])
    edge_types: Vec<String>,

    /// Property filters for edges
    filters: Vec<PropertyFilter>,

    /// Current input tuple being expanded
    current_input: Option<ResultTuple>,

    /// Iterator over edges for current input
    edge_iterator: Option<Box<dyn Iterator<Item = (Arc<Edge>, Arc<Node>)> + Send + Sync>>,

    /// Estimated cardinality
    estimated_cardinality: usize,
}

impl ExpandOperator {
    /// Create new expand operator
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        input: Box<dyn PhysicalOperator>,
        engine: Arc<dyn GraphEngine>,
        from_variable: String,
        edge_variable: Option<String>,
        to_variable: String,
        direction: EdgeDirection,
        edge_types: Vec<String>,
        filters: Vec<PropertyFilter>,
    ) -> Self {
        Self {
            input,
            engine,
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

    /// Apply property filters to an edge
    fn apply_edge_filters(&self, edge: &Edge) -> bool {
        for filter in &self.filters {
            if let Some(actual_value) = edge.properties.get(&filter.key) {
                if !evaluate_property_filter(filter, actual_value) {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }

    /// Fetch edges for current source node
    fn fetch_edges_for_node(&self, node_id: &String) -> Result<Vec<(Arc<Edge>, Arc<Node>)>> {
        // Fetch edges based on direction (reuse GraphEngine methods)
        let edges = match self.direction {
            EdgeDirection::Outgoing => self
                .engine
                .get_outgoing_edges(node_id, None)
                .map_err(|e| anyhow::anyhow!("Failed to get outgoing edges: {}", e))?,
            EdgeDirection::Incoming => self
                .engine
                .get_incoming_edges(node_id, None)
                .map_err(|e| anyhow::anyhow!("Failed to get incoming edges: {}", e))?,
            EdgeDirection::Bidirectional => {
                let mut all_edges = self
                    .engine
                    .get_outgoing_edges(node_id, None)
                    .map_err(|e| anyhow::anyhow!("Failed to get outgoing edges: {}", e))?;
                let incoming = self
                    .engine
                    .get_incoming_edges(node_id, None)
                    .map_err(|e| anyhow::anyhow!("Failed to get incoming edges: {}", e))?;
                all_edges.extend(incoming);
                all_edges
            }
        };

        // Filter by edge type
        let filtered_edges: Vec<Arc<Edge>> = if self.edge_types.is_empty() {
            edges
        } else {
            edges
                .into_iter()
                .filter(|e| self.edge_types.contains(&e.edge_type))
                .collect()
        };

        // Apply property filters
        let filtered_edges: Vec<Arc<Edge>> = filtered_edges
            .into_iter()
            .filter(|e| self.apply_edge_filters(e))
            .collect();

        // Fetch target nodes
        let mut edge_node_pairs = Vec::new();
        for edge in filtered_edges {
            let target_id = match self.direction {
                EdgeDirection::Outgoing => &edge.to_node_id,
                EdgeDirection::Incoming => &edge.from_node_id,
                EdgeDirection::Bidirectional => {
                    if &edge.from_node_id == node_id {
                        &edge.to_node_id
                    } else {
                        &edge.from_node_id
                    }
                }
            };

            if let Ok(Some(target_node)) = self.engine.get_node(target_id) {
                edge_node_pairs.push((edge, target_node));
            }
        }

        Ok(edge_node_pairs)
    }
}

impl PhysicalOperator for ExpandOperator {
    fn open(&mut self) -> Result<()> {
        // Open input operator
        self.input.open()?;

        // Estimate cardinality (input cardinality * avg degree)
        let input_card = self.input.estimated_cardinality();
        let avg_degree = 10; // Deferred: Get from statistics
        self.estimated_cardinality = input_card * avg_degree;

        Ok(())
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
        loop {
            // Expand current node's edges
            if let Some(ref mut iter) = self.edge_iterator
                && let Some((edge, target_node)) = iter.next() {
                    // Create result tuple by extending input tuple
                    let mut result = self
                        .current_input
                        .as_ref()
                        .ok_or_else(|| {
                            ProximaDBError::Internal(
                                "No current input tuple available in expand operator".to_string(),
                            )
                        })?
                        .clone();

                    // Add edge binding (if requested)
                    if let Some(ref edge_var) = self.edge_variable {
                        result.set(edge_var.clone(), QueryValue::Edge(edge));
                    }

                    // Add target node binding
                    result.set(self.to_variable.clone(), QueryValue::Node(target_node));

                    return Ok(Some(result));
                }

            // Get next input tuple
            if let Some(input_tuple) = self.input.next()? {
                // Extract source node
                let from_node = match input_tuple.get(&self.from_variable) {
                    Some(QueryValue::Node(n)) => n.clone(),
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Expected node for variable '{}', found {:?}",
                            self.from_variable,
                            input_tuple.get(&self.from_variable)
                        ));
                    }
                };

                // Fetch edges for this node
                let edge_pairs = self.fetch_edges_for_node(&from_node.id)?;

                self.current_input = Some(input_tuple);
                self.edge_iterator = Some(Box::new(edge_pairs.into_iter()));
            } else {
                // No more input tuples
                return Ok(None);
            }
        }
    }

    fn close(&mut self) -> Result<()> {
        self.edge_iterator = None;
        self.current_input = None;
        self.input.close()?;
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
    use crate::graph::query::operators::scan::NodeScanOperator;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Mock graph engine for testing
    struct MockEngine {
        nodes: HashMap<String, Arc<Node>>,
        edges: Vec<Arc<Edge>>,
    }

    impl MockEngine {
        fn new() -> Self {
            let mut nodes = HashMap::new();

            // Create test nodes
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

            // Create test edge
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

    #[async_trait]
    impl GraphEngine for MockEngine {
        fn get_nodes_by_label(
            &self,
            _label: &str,
        ) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self.nodes.values().cloned().collect())
        }

        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self.nodes.values().cloned().collect())
        }

        fn get_node(
            &self,
            id: &String,
        ) -> Result<Option<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self.nodes.get(id).cloned())
        }

        fn get_outgoing_edges(
            &self,
            node_id: &String,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>, crate::core::error::ProximaDBError> {
            Ok(self
                .edges
                .iter()
                .filter(|e| &e.from_node_id == node_id)
                .cloned()
                .collect())
        }

        fn get_incoming_edges(
            &self,
            node_id: &String,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>, crate::core::error::ProximaDBError> {
            Ok(self
                .edges
                .iter()
                .filter(|e| &e.to_node_id == node_id)
                .cloned()
                .collect())
        }

        fn get_neighbors(
            &self,
            _node_id: &String,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(vec![])
        }

        // Stub implementations
        async fn insert_node(
            &self,
            node: Node,
        ) -> Result<Arc<Node>, crate::core::error::ProximaDBError> {
            Ok(Arc::new(node))
        }

        async fn update_node(
            &self,
            node: Node,
        ) -> Result<Arc<Node>, crate::core::error::ProximaDBError> {
            Ok(Arc::new(node))
        }

        async fn delete_node(
            &self,
            _id: &String,
        ) -> Result<Option<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(None)
        }

        async fn insert_edge(
            &self,
            edge: Edge,
        ) -> Result<Arc<Edge>, crate::core::error::ProximaDBError> {
            Ok(Arc::new(edge))
        }

        fn get_edge(
            &self,
            _id: &String,
        ) -> Result<Option<Arc<Edge>>, crate::core::error::ProximaDBError> {
            Ok(None)
        }

        async fn update_edge(
            &self,
            edge: Edge,
        ) -> Result<Arc<Edge>, crate::core::error::ProximaDBError> {
            Ok(Arc::new(edge))
        }

        async fn delete_edge(
            &self,
            _id: &String,
        ) -> Result<Option<Arc<Edge>>, crate::core::error::ProximaDBError> {
            Ok(None)
        }

        fn node_count(&self) -> Result<usize, crate::core::error::ProximaDBError> {
            Ok(self.nodes.len())
        }

        fn edge_count(&self) -> Result<usize, crate::core::error::ProximaDBError> {
            Ok(self.edges.len())
        }
    }

    #[tokio::test]
    async fn test_expand_outgoing_edges() {
        let engine = Arc::new(MockEngine::new());

        // Create scan operator for alice
        let scan = NodeScanOperator::new(
            engine.clone(),
            Some("Person".to_string()),
            vec![],
            "p".to_string(),
        );

        // Create expand operator
        let mut expand = ExpandOperator::new(
            Box::new(scan),
            engine.clone(),
            "p".to_string(),
            Some("r".to_string()),
            "f".to_string(),
            EdgeDirection::Outgoing,
            vec!["KNOWS".to_string()],
            vec![],
        );

        expand.open().unwrap();

        let mut found_expansion = false;
        while let Some(tuple) = expand.next().unwrap() {
            // Should have p (alice), r (KNOWS edge), f (bob)
            assert!(tuple.contains("p"));
            assert!(tuple.contains("r"));
            assert!(tuple.contains("f"));

            let edge = tuple.get("r").unwrap().as_edge().unwrap();
            assert_eq!(edge.edge_type, "KNOWS");

            found_expansion = true;
        }

        assert!(found_expansion, "Should find at least one expansion");

        expand.close().unwrap();
    }
}
