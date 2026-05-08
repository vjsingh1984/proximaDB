use anyhow::Result;
use proximadb_proto::proximadb_v1::{Edge, Node};
use std::sync::Arc;

/// Narrow read-side contract for graph query operators.
///
/// This is intentionally smaller than a full graph engine/service API so the
/// operator/runtime layer can live in an isolated crate without depending on
/// root service orchestration or mutation semantics.
pub trait GraphQueryStorage: Send + Sync {
    /// Fetch one node by ID.
    fn get_node(&self, id: &str) -> Result<Option<Arc<Node>>>;

    /// Fetch all nodes with a given label.
    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>>;

    /// Fetch all nodes in the graph.
    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>>;

    /// Fetch outgoing edges from a node.
    fn get_outgoing_edges(&self, node_id: &str, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>>;

    /// Fetch incoming edges to a node.
    fn get_incoming_edges(&self, node_id: &str, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>>;
}
