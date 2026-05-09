use super::GraphOperationsService;
use async_trait::async_trait;
use proximadb_graph::query::QueryResult;
use proximadb_graph_query::service::GraphQueryTraversalService;
use proximadb_proto::proximadb_v1::{Node, TraversalRequest, TraversalResponse};
use std::sync::Arc;

#[async_trait]
impl GraphQueryTraversalService for GraphOperationsService {
    async fn traverse(
        &self,
        graph_id: &str,
        request: TraversalRequest,
    ) -> QueryResult<TraversalResponse> {
        GraphOperationsService::traverse(self, graph_id, request).await
    }

    async fn get_neighbors(&self, graph_id: &str, node_id: &str) -> QueryResult<Vec<Arc<Node>>> {
        let node_id = node_id.to_string();
        GraphOperationsService::get_neighbors(self, graph_id, &node_id).await
    }
}
