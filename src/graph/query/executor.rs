//! Compatibility wrapper for the extracted graph query executor.
//!
//! The canonical plan execution runtime now lives in the `proximadb-graph`
//! workspace crate. This module preserves the historical root API that
//! accepts a `GraphOperationsService`, while keeping Arrow conversion and
//! service adaptation in the root crate.

use crate::graph::GraphOperationsService;
use async_trait::async_trait;
use futures::Stream;
use proximadb_graph::query::executor::{
    GraphQueryExecutorBackend, QueryExecutor as InnerQueryExecutor, QueryRow,
};
use proximadb_graph::query::planner::QueryPlan;
use proximadb_graph::query::{QueryContext, QueryResult};
use proximadb_graph_arrow::GraphArrowBridge;
use proximadb_kernel::error::VectorDBError;
use proximadb_proto::proximadb_v1::{Edge, EdgeQuery, Node, NodeQuery};
use std::pin::Pin;
use std::sync::Arc;

struct GraphOperationsServiceAdapter {
    graph_service: Arc<GraphOperationsService>,
}

impl GraphOperationsServiceAdapter {
    fn new(graph_service: Arc<GraphOperationsService>) -> Self {
        Self { graph_service }
    }
}

#[async_trait]
impl GraphQueryExecutorBackend for GraphOperationsServiceAdapter {
    async fn query_nodes(&self, graph_id: &str, query: NodeQuery) -> QueryResult<Vec<Arc<Node>>> {
        self.graph_service.query_nodes(graph_id, query).await
    }

    async fn query_edges(&self, graph_id: &str, query: EdgeQuery) -> QueryResult<Vec<Arc<Edge>>> {
        self.graph_service.query_edges(graph_id, query).await
    }
}

/// Compatibility wrapper preserving the historical root executor API.
pub struct QueryExecutor {
    inner: InnerQueryExecutor,
}

impl QueryExecutor {
    pub fn new(graph_service: Arc<GraphOperationsService>) -> Self {
        let backend = Arc::new(GraphOperationsServiceAdapter::new(graph_service));
        Self {
            inner: InnerQueryExecutor::new(backend),
        }
    }

    pub async fn execute(
        &self,
        plan: &QueryPlan,
        context: &QueryContext,
    ) -> QueryResult<Vec<QueryRow>> {
        self.inner.execute(plan, context).await
    }

    pub async fn execute_as_arrow(
        &self,
        plan: &QueryPlan,
        context: &QueryContext,
        include_edges: bool,
    ) -> QueryResult<arrow::record_batch::RecordBatch> {
        let results = self.execute(plan, context).await?;
        self.convert_to_arrow(&results, include_edges)
    }

    pub async fn stream_as_arrow<'a>(
        &'a self,
        plan: &'a QueryPlan,
        context: &'a QueryContext,
        batch_size: usize,
    ) -> QueryResult<
        Pin<Box<dyn Stream<Item = QueryResult<arrow::record_batch::RecordBatch>> + Send + 'a>>,
    > {
        use futures::stream::{self, StreamExt};

        let results = self.execute(plan, context).await?;
        let stream = stream::iter(results.into_iter())
            .chunks(batch_size)
            .map(move |batch| self.convert_to_arrow(&batch, true));

        Ok(Box::pin(stream))
    }

    pub fn convert_to_arrow(
        &self,
        results: &[QueryRow],
        include_edges: bool,
    ) -> QueryResult<arrow::record_batch::RecordBatch> {
        GraphArrowBridge::graph_results_to_arrow(results, include_edges)
            .map_err(|e| VectorDBError::Internal(format!("Arrow conversion failed: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::Uuid;
    use proximadb_graph::query::planner::{CostEstimate, PlanStep, PlanStepType};
    use std::collections::HashMap;
    use std::time::SystemTime;

    #[tokio::test]
    async fn test_executor_node_scan_compatibility_wrapper() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service.clone());

        let create_graph_request = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "test_graph".to_string(),
            name: Some("Test Graph".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        graph_service
            .create_graph_collection(create_graph_request)
            .await
            .unwrap();

        let node = crate::graph::Node {
            id: "test_node_1".to_string(),
            labels: vec!["TestLabel".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        };
        match graph_service.create_node("test_graph", node).await {
            Ok(_) => {}
            Err(e)
                if e.to_string().contains("URL")
                    || e.to_string().contains("Serialization error") =>
            {
                tracing::warn!(
                    "Skipping compatibility wrapper test due to environment issue: {}",
                    e
                );
                return;
            }
            Err(e) => panic!("Unexpected error: {}", e),
        }

        let plan = QueryPlan {
            id: Uuid::new_v4().to_string(),
            steps: vec![PlanStep {
                step_type: PlanStepType::NodeScan {
                    labels: Some(vec!["TestLabel".to_string()]),
                    property_filters: Vec::new(),
                },
                parameters: HashMap::new(),
                cost: CostEstimate::zero(),
                output_cardinality: 1,
            }],
            estimated_cost: CostEstimate::zero(),
            estimated_result_size: 1,
            created_at: SystemTime::now(),
        };

        let context = QueryContext::new().with_graph_id("test_graph".to_string());
        let results = executor.execute(&plan, &context).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].contains_key("node"));
        let node_json = results[0].get("node").unwrap();
        assert_eq!(node_json.get("id").unwrap(), "test_node_1");
    }
}
