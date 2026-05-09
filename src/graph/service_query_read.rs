use super::GraphOperationsService;
use async_trait::async_trait;
use proximadb_graph_query::service::{GraphQueryReadService, GraphQueryResult as QueryResult};
use proximadb_proto::proximadb_v1::{Edge, EdgeQuery, Node, NodeQuery};
use std::sync::Arc;

#[async_trait]
impl GraphQueryReadService for GraphOperationsService {
    async fn list_graphs(&self) -> QueryResult<Vec<String>> {
        GraphOperationsService::list_graphs(self).await
    }

    async fn get_node(&self, graph_id: &str, node_id: &str) -> QueryResult<Option<Arc<Node>>> {
        let node_id = node_id.to_string();
        GraphOperationsService::get_node(self, graph_id, &node_id).await
    }

    async fn query_nodes(&self, graph_id: &str, query: NodeQuery) -> QueryResult<Vec<Arc<Node>>> {
        GraphOperationsService::query_nodes(self, graph_id, query).await
    }

    async fn query_edges(&self, graph_id: &str, query: EdgeQuery) -> QueryResult<Vec<Arc<Edge>>> {
        GraphOperationsService::query_edges(self, graph_id, query).await
    }
}
