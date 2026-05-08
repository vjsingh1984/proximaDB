use async_trait::async_trait;
use proximadb_kernel::error::ProximaDBError;
use proximadb_proto::proximadb_v1::{
    Edge, EdgeQuery, GraphStats, Node, NodeQuery, TraversalRequest, TraversalResponse,
};
use std::sync::Arc;

/// Canonical graph-query contract result.
pub type GraphQueryResult<T> = std::result::Result<T, ProximaDBError>;

/// Narrow async read/query contract for graph-facing query runtimes.
#[async_trait]
pub trait GraphQueryReadService: Send + Sync {
    /// List known graphs so callers can discover a default target.
    async fn list_graphs(&self) -> GraphQueryResult<Vec<String>>;

    /// Fetch one node by graph and node ID.
    async fn get_node(&self, graph_id: &str, node_id: &str) -> GraphQueryResult<Option<Arc<Node>>>;

    /// Query nodes using the canonical node query contract.
    async fn query_nodes(
        &self,
        graph_id: &str,
        query: NodeQuery,
    ) -> GraphQueryResult<Vec<Arc<Node>>>;

    /// Query edges using the canonical edge query contract.
    async fn query_edges(
        &self,
        graph_id: &str,
        query: EdgeQuery,
    ) -> GraphQueryResult<Vec<Arc<Edge>>>;
}

/// Narrow async graph-metadata contract for planning and validation.
#[async_trait]
pub trait GraphQueryStatsService: Send + Sync {
    /// Fetch graph statistics for planning or validation.
    async fn get_stats(&self, graph_id: &str) -> GraphQueryResult<GraphStats>;
}

/// Narrow async traversal contract for graph query execution paths.
#[async_trait]
pub trait GraphQueryTraversalService: Send + Sync {
    /// Execute a graph traversal request.
    async fn traverse(
        &self,
        graph_id: &str,
        request: TraversalRequest,
    ) -> GraphQueryResult<TraversalResponse>;

    /// Fetch immediate neighbors for a node.
    async fn get_neighbors(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> GraphQueryResult<Vec<Arc<Node>>>;
}

/// Composite graph query contract for runtimes that need both declarative
/// read/query capabilities and traversal execution.
pub trait GraphQueryService: GraphQueryReadService + GraphQueryTraversalService {}

impl<T> GraphQueryService for T where T: GraphQueryReadService + GraphQueryTraversalService + ?Sized {}

/// Composite graph execution contract for execution engines that need
/// traversal plus graph metadata/statistics validation.
pub trait GraphExecutionService: GraphQueryStatsService + GraphQueryTraversalService {}

impl<T> GraphExecutionService for T where
    T: GraphQueryStatsService + GraphQueryTraversalService + ?Sized
{
}
