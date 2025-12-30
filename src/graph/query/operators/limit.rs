//! Limit operator
//!
//! Limits result set size (LIMIT/SKIP clauses).

use super::{ColumnSpec, PhysicalOperator, ResultTuple};
use anyhow::Result;

/// Limit operator
///
/// Implements LIMIT and SKIP clauses for result pagination.
///
/// # Example
///
/// ```ignore
/// // LIMIT 10 SKIP 5 (return results 6-15)
/// let mut limit = LimitOperator::new(input, Some(5), Some(10));
/// ```
pub struct LimitOperator {
    /// Input operator
    input: Box<dyn PhysicalOperator>,

    /// Number of rows to skip (SKIP clause)
    skip: Option<usize>,

    /// Maximum number of rows to return (LIMIT clause)
    limit: Option<usize>,

    /// Current row count (for tracking)
    current_row: usize,

    /// Rows returned count
    returned_count: usize,
}

impl LimitOperator {
    /// Create new limit operator
    pub fn new(input: Box<dyn PhysicalOperator>, skip: Option<usize>, limit: Option<usize>) -> Self {
        Self {
            input,
            skip,
            limit,
            current_row: 0,
            returned_count: 0,
        }
    }
}

impl PhysicalOperator for LimitOperator {
    fn open(&mut self) -> Result<()> {
        self.input.open()?;
        self.current_row = 0;
        self.returned_count = 0;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
        // Check if we've reached the limit
        if let Some(limit) = self.limit {
            if self.returned_count >= limit {
                return Ok(None);
            }
        }

        // Skip rows if needed
        let skip_count = self.skip.unwrap_or(0);
        while self.current_row < skip_count {
            if self.input.next()?.is_none() {
                return Ok(None);
            }
            self.current_row += 1;
        }

        // Return next row
        if let Some(tuple) = self.input.next()? {
            self.current_row += 1;
            self.returned_count += 1;
            Ok(Some(tuple))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> Result<()> {
        self.input.close()
    }

    fn estimated_cardinality(&self) -> usize {
        let input_card = self.input.estimated_cardinality();
        let skip_count = self.skip.unwrap_or(0);

        if let Some(limit) = self.limit {
            // Return min(limit, input - skip)
            limit.min(input_card.saturating_sub(skip_count))
        } else {
            // No limit, just subtract skip
            input_card.saturating_sub(skip_count)
        }
    }

    fn schema(&self) -> &[ColumnSpec] {
        self.input.schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::GraphEngine;
    use crate::graph::query::operators::scan::NodeScanOperator;
    use crate::proto::proximadb_v1::Node;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct MockEngine {
        nodes: Vec<Arc<Node>>,
    }

    impl MockEngine {
        fn new(count: usize) -> Self {
            let nodes: Vec<Arc<Node>> = (0..count)
                .map(|i| {
                    Arc::new(Node {
                        id: format!("n{}", i),
                        labels: vec!["Test".to_string()],
                        properties: HashMap::new(),
                        ..Default::default()
                    })
                })
                .collect();

            Self { nodes }
        }
    }

    #[async_trait]
    impl GraphEngine for MockEngine {
        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self.nodes.clone())
        }

        fn get_nodes_by_label(&self, _label: &str) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self.nodes.clone())
        }

        // Stub implementations
        async fn insert_node(&self, node: Node) -> Result<Arc<Node>, crate::core::error::ProximaDBError> { Ok(Arc::new(node)) }
        fn get_node(&self, _id: &String) -> Result<Option<Arc<Node>>, crate::core::error::ProximaDBError> { Ok(None) }
        async fn update_node(&self, node: Node) -> Result<Arc<Node>, crate::core::error::ProximaDBError> { Ok(Arc::new(node)) }
        async fn delete_node(&self, _id: &String) -> Result<Option<Arc<Node>>, crate::core::error::ProximaDBError> { Ok(None) }
        async fn insert_edge(&self, edge: crate::proto::proximadb_v1::Edge) -> Result<Arc<crate::proto::proximadb_v1::Edge>, crate::core::error::ProximaDBError> { Ok(Arc::new(edge)) }
        fn get_edge(&self, _id: &String) -> Result<Option<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> { Ok(None) }
        async fn update_edge(&self, edge: crate::proto::proximadb_v1::Edge) -> Result<Arc<crate::proto::proximadb_v1::Edge>, crate::core::error::ProximaDBError> { Ok(Arc::new(edge)) }
        async fn delete_edge(&self, _id: &String) -> Result<Option<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> { Ok(None) }
        fn get_neighbors(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> { Ok(vec![]) }
        fn get_outgoing_edges(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> { Ok(vec![]) }
        fn get_incoming_edges(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> { Ok(vec![]) }
        fn node_count(&self) -> Result<usize, crate::core::error::ProximaDBError> { Ok(self.nodes.len()) }
        fn edge_count(&self) -> Result<usize, crate::core::error::ProximaDBError> { Ok(0) }
    }

    #[test]
    fn test_limit_only() {
        let engine = Arc::new(MockEngine::new(100));
        let scan = NodeScanOperator::new(engine, None, vec![], "n".to_string());
        let mut limit = LimitOperator::new(Box::new(scan), None, Some(10));

        limit.open().unwrap();

        let mut count = 0;
        while limit.next().unwrap().is_some() {
            count += 1;
        }

        assert_eq!(count, 10);

        limit.close().unwrap();
    }

    #[test]
    fn test_skip_only() {
        let engine = Arc::new(MockEngine::new(100));
        let scan = NodeScanOperator::new(engine, None, vec![], "n".to_string());
        let mut limit = LimitOperator::new(Box::new(scan), Some(90), None);

        limit.open().unwrap();

        let mut count = 0;
        while limit.next().unwrap().is_some() {
            count += 1;
        }

        assert_eq!(count, 10); // 100 - 90

        limit.close().unwrap();
    }

    #[test]
    fn test_skip_and_limit() {
        let engine = Arc::new(MockEngine::new(100));
        let scan = NodeScanOperator::new(engine, None, vec![], "n".to_string());
        let mut limit = LimitOperator::new(Box::new(scan), Some(20), Some(10));

        limit.open().unwrap();

        let mut count = 0;
        while limit.next().unwrap().is_some() {
            count += 1;
        }

        assert_eq!(count, 10); // Skip 20, return 10

        limit.close().unwrap();
    }

    #[test]
    fn test_limit_exceeds_available() {
        let engine = Arc::new(MockEngine::new(5));
        let scan = NodeScanOperator::new(engine, None, vec![], "n".to_string());
        let mut limit = LimitOperator::new(Box::new(scan), None, Some(10));

        limit.open().unwrap();

        let mut count = 0;
        while limit.next().unwrap().is_some() {
            count += 1;
        }

        assert_eq!(count, 5); // Only 5 available

        limit.close().unwrap();
    }
}
