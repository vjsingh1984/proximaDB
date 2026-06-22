use super::GraphOperationsService;
use async_trait::async_trait;
use proximadb_graph_query::service::{GraphQueryResult as QueryResult, GraphQueryTraversalService};
use proximadb_proto::proximadb_v1::{Node, TraversalRequest, TraversalResponse};
use std::sync::Arc;

// Trait contract is in `proximadb.v1` proto types; the engine speaks the neutral
// model. Convert proto <-> neutral at this boundary (see `graph::proto_convert`).

#[async_trait]
impl GraphQueryTraversalService for GraphOperationsService {
    async fn traverse(
        &self,
        graph_id: &str,
        request: TraversalRequest,
    ) -> QueryResult<TraversalResponse> {
        GraphOperationsService::traverse(self, graph_id, request.into())
            .await
            .map(Into::into)
    }

    async fn get_neighbors(&self, graph_id: &str, node_id: &str) -> QueryResult<Vec<Arc<Node>>> {
        let node_id = node_id.to_string();
        let nodes = GraphOperationsService::get_neighbors(self, graph_id, &node_id).await?;
        Ok(nodes
            .into_iter()
            .map(|n| Arc::new((*n).clone().into()))
            .collect())
    }
}
