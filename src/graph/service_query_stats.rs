use super::GraphOperationsService;
use async_trait::async_trait;
use proximadb_graph_query::service::{GraphQueryResult as QueryResult, GraphQueryStatsService};
use proximadb_proto::proximadb_v1::GraphStats;

#[async_trait]
impl GraphQueryStatsService for GraphOperationsService {
    async fn get_stats(&self, graph_id: &str) -> QueryResult<GraphStats> {
        GraphOperationsService::get_stats(self, graph_id).await
    }
}
