//! AQL Source implementation for Graph data model.

use async_trait::async_trait;
use proximadb_graph::query::service::GraphExecutionService;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::query::aql::{
    AqlFrom, AqlQuery, AqlResult, AqlSource, AqlValue, AuditContext, AuditFrame, AuditOp,
    DataModel, Result,
};

pub struct GraphAqlSource {
    graph_svc: Arc<dyn GraphExecutionService>,
}

impl GraphAqlSource {
    pub fn new(graph_svc: Arc<dyn GraphExecutionService>) -> Self {
        Self { graph_svc }
    }

    fn extract_graph_params(&self, query: &AqlQuery) -> (String, u32) {
        let mut graph_id = "default".to_string();
        let max_depth = 2; // Default

        if let AqlFrom::Source { name, .. } = &query.from {
            graph_id = name.clone();
        }

        // Predicates could specify traversal depth or starting nodes
        // For this skeleton, we use defaults.

        (graph_id, max_depth)
    }
}

#[async_trait]
impl AqlSource for GraphAqlSource {
    fn model(&self) -> DataModel {
        DataModel::Graph
    }

    async fn execute(&self, query: &AqlQuery, ctx: &mut AuditContext) -> Result<AqlResult> {
        let (graph_id, depth) = self.extract_graph_params(query);
        let start = Instant::now();

        // In a real implementation, we'd use the AQL WHERE clause to find
        // starting nodes and then perform a traversal.
        // For this skeleton, we'll perform a generic traversal from a placeholder ID
        // if no specific start nodes are identified.

        let traversal_request = crate::proto::proximadb_v1::TraversalRequest {
            graph_id: graph_id.clone(),
            start_node_id: "root".to_string(), // Placeholder
            max_depth: depth,
            edge_types: Vec::new(),
            node_labels: Vec::new(),
            filters: Vec::new(),
            algorithm: crate::proto::proximadb_v1::TraversalAlgorithm::Bfs as i32,
            limit: Some(100),
            timeout_ms: None,
            max_frontier: None,
        };

        let traversal_result = self
            .graph_svc
            .traverse(&graph_id, traversal_request)
            .await
            .map_err(|e| {
                crate::core::error::ProximaDBError::Storage(
                    crate::core::error::StorageError::SstEngine(e.to_string()),
                )
            })?;

        let wall_time_us = start.elapsed().as_micros() as u64;

        // Convert to AQL rows
        let mut rows = Vec::new();
        for node in traversal_result.nodes {
            let mut row = HashMap::new();
            row.insert("id".to_string(), AqlValue::String(node.id.clone()));
            row.insert(
                "labels".to_string(),
                AqlValue::String(node.labels.join(",")),
            );

            // In a real system, we'd expand properties here
            rows.push(row);
        }

        // Emit audit frame
        let frame = AuditFrame {
            frame_id: 0,
            source: self.model(),
            op: AuditOp::GraphTraversal {
                graph_id: graph_id.clone(),
                depth,
                algorithm: "BFS".to_string(),
            },
            filters_pushed: Vec::new(),
            filters_post: Vec::new(),
            records_scanned: rows.len() as u64, // Approximated
            records_returned: rows.len() as u64,
            wall_time_us,
            error: None,
            redaction_count: 0,
        };

        let frame_id = ctx.push_frame(frame);

        Ok(AqlResult { rows, frame_id })
    }
}
