use super::GraphOperationsService;
use async_trait::async_trait;
use proximadb_graph_query::service::{GraphQueryReadService, GraphQueryResult as QueryResult};
use proximadb_proto::proximadb_v1::{Edge, EdgeQuery, Node, NodeQuery};
use std::sync::Arc;

// The `proximadb-graph-query` trait contract is expressed in `proximadb.v1`
// proto types (it is a foundation crate that cannot depend on the root crate's
// neutral `graph::model`). The engine speaks the neutral model, so these impls
// convert proto <-> neutral at the trait boundary (see `graph::proto_convert`).

#[async_trait]
impl GraphQueryReadService for GraphOperationsService {
    async fn list_graphs(&self) -> QueryResult<Vec<String>> {
        GraphOperationsService::list_graphs(self).await
    }

    async fn get_node(&self, graph_id: &str, node_id: &str) -> QueryResult<Option<Arc<Node>>> {
        let node_id = node_id.to_string();
        let node = GraphOperationsService::get_node(self, graph_id, &node_id).await?;
        Ok(node.map(|n| Arc::new((*n).clone().into())))
    }

    async fn query_nodes(&self, graph_id: &str, query: NodeQuery) -> QueryResult<Vec<Arc<Node>>> {
        let nodes = GraphOperationsService::query_nodes(self, graph_id, query.into()).await?;
        Ok(nodes
            .into_iter()
            .map(|n| Arc::new((*n).clone().into()))
            .collect())
    }

    async fn query_edges(&self, graph_id: &str, query: EdgeQuery) -> QueryResult<Vec<Arc<Edge>>> {
        let edges = GraphOperationsService::query_edges(self, graph_id, query.into()).await?;
        Ok(edges
            .into_iter()
            .map(|e| Arc::new((*e).clone().into()))
            .collect())
    }
}
