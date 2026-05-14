//! Rebuildable graph topology projection contracts.
//!
//! CSR (Compressed Sparse Row), COO (Coordinate format), and adjacency table
//! layouts are Layer 2 projections over canonical `ProximaRecord` node + edge
//! data. They are NOT the durable source of truth for graph data.
//!
//! Architectural invariants (from RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE):
//! - Durable authority lives in canonical `ProximaRecord` node/edge records.
//! - CSR is an adaptive read-heavy-traversal projection, not the write path.
//! - Write-heavy graph workloads use relational adjacency tables first.
//! - CSR materialization requires workload evidence and freshness/invalidation rules.
//! - All topology projections must be rebuildable from the canonical record set.

use async_trait::async_trait;
use proximadb_kernel::error::ProximaDBError;
use proximadb_records::ProximaRecord;

use crate::record::{GRAPH_EDGE_LABEL, GRAPH_ID_PROP, GRAPH_NODE_LABEL, GraphNodeKey};

/// Result type for topology projection operations.
pub type ProjectionResult<T> = Result<T, ProximaDBError>;

/// Catalog-facing descriptor for a graph topology projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphTopologyDescriptor {
    /// Projection name.
    pub name: String,
    /// Graph dataset this projection covers.
    pub graph_id: String,
    /// Topology format.
    pub format: TopologyFormat,
    /// Whether this projection can be dropped and rebuilt from canonical records.
    pub rebuildable: bool,
    /// Whether this projection is authoritative for write-path topology
    /// (always false; durable truth is in canonical records).
    pub write_authoritative: bool,
}

impl GraphTopologyDescriptor {
    pub fn new(
        name: impl Into<String>,
        graph_id: impl Into<String>,
        format: TopologyFormat,
    ) -> Self {
        Self {
            name: name.into(),
            graph_id: graph_id.into(),
            format,
            rebuildable: true,
            write_authoritative: false,
        }
    }
}

/// Supported topology projection formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyFormat {
    /// Compressed Sparse Row — efficient for read-heavy traversal and algorithms.
    Csr,
    /// Coordinate format — easier to build incrementally, used before CSR materialization.
    Coo,
    /// Relational adjacency table — used as primary write-path topology index.
    AdjacencyTable,
}

/// Result of applying one canonical edge record to a topology projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyApplyResult {
    /// Edge record oid that was consumed.
    pub edge_oid: String,
    /// Number of adjacency entries updated.
    pub entries_written: usize,
}

/// Rebuildable graph topology projection contract.
///
/// Implementations must NOT be the durable write path for graph topology.
/// Canonical node/edge `ProximaRecord` records are the durable write path.
/// Topology projections are read-path acceleration structures that can be
/// dropped and rebuilt from canonical records on demand.
#[async_trait]
pub trait GraphTopologyProjection: Send + Sync {
    fn descriptor(&self) -> &GraphTopologyDescriptor;

    /// Apply one canonical edge record to the projection.
    async fn apply_edge(
        &self,
        edge_record: &ProximaRecord,
    ) -> ProjectionResult<TopologyApplyResult>;

    /// Remove one edge from the projection.
    async fn remove_edge(&self, edge_record: &ProximaRecord) -> ProjectionResult<bool>;

    /// Rebuild the full projection from canonical node + edge records.
    async fn rebuild_from_records(
        &self,
        node_records: &[ProximaRecord],
        edge_records: &[ProximaRecord],
    ) -> ProjectionResult<usize>;
}

/// Filter records by graph id and element type label.
///
/// Used when rebuilding topology projections from a flat canonical record
/// stream that may contain mixed-modality records.
pub fn filter_graph_edges<'a>(
    records: &'a [ProximaRecord],
    graph_id: &str,
) -> Vec<&'a ProximaRecord> {
    use proximadb_data_model::ProximaValue;
    use proximadb_records::ProximaTreeNode;

    records
        .iter()
        .filter(|record| {
            if !record.labels.contains(GRAPH_EDGE_LABEL) {
                return false;
            }
            matches!(
                record.props.get(GRAPH_ID_PROP),
                Some(ProximaTreeNode::Value(ProximaValue::String(id))) if id == graph_id
            )
        })
        .collect()
}

/// Filter records that are graph nodes for a given graph_id.
pub fn filter_graph_nodes<'a>(
    records: &'a [ProximaRecord],
    graph_id: &str,
) -> Vec<&'a ProximaRecord> {
    use proximadb_data_model::ProximaValue;
    use proximadb_records::ProximaTreeNode;

    records
        .iter()
        .filter(|record| {
            if !record.labels.contains(GRAPH_NODE_LABEL) {
                return false;
            }
            matches!(
                record.props.get(GRAPH_ID_PROP),
                Some(ProximaTreeNode::Value(ProximaValue::String(id))) if id == graph_id
            )
        })
        .collect()
}

/// Extract the edge endpoints from a canonical edge record.
///
/// Returns `(src_node_oid, dst_node_oid)` if the record is a valid edge record.
pub fn edge_endpoints(record: &ProximaRecord) -> Option<(String, String)> {
    record
        .edge
        .as_ref()
        .map(|edge_shape| (edge_shape.source_id.clone(), edge_shape.target_id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{CanonicalEdge, CanonicalNode};
    use proximadb_records::ProximaTree;

    fn make_node(graph_id: &str, node_id: &str) -> ProximaRecord {
        CanonicalNode::new(graph_id, node_id, "Person", ProximaTree::new()).into_proxima_record()
    }

    fn make_edge(graph_id: &str, edge_id: &str, src: &str, dst: &str) -> ProximaRecord {
        let src_oid = GraphNodeKey::new(graph_id, src).canonical_oid();
        let dst_oid = GraphNodeKey::new(graph_id, dst).canonical_oid();
        CanonicalEdge::new(
            graph_id,
            edge_id,
            src_oid,
            dst_oid,
            "KNOWS",
            ProximaTree::new(),
        )
        .into_proxima_record()
    }

    #[test]
    fn topology_descriptor_is_never_write_authoritative() {
        let desc = GraphTopologyDescriptor::new("csr-g1", "g1", TopologyFormat::Csr);
        assert!(!desc.write_authoritative);
        assert!(desc.rebuildable);
    }

    #[test]
    fn filter_graph_edges_selects_by_graph_id() {
        let records = vec![
            make_node("g1", "n1"),
            make_edge("g1", "e1", "n1", "n2"),
            make_edge("g2", "e2", "n3", "n4"),
        ];

        let edges = filter_graph_edges(&records, "g1");
        assert_eq!(edges.len(), 1);
        assert!(edges[0].labels.contains(GRAPH_EDGE_LABEL));
    }

    #[test]
    fn filter_graph_nodes_selects_by_graph_id() {
        let records = vec![
            make_node("g1", "n1"),
            make_node("g1", "n2"),
            make_node("g2", "n3"),
            make_edge("g1", "e1", "n1", "n2"),
        ];

        let nodes = filter_graph_nodes(&records, "g1");
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn edge_endpoints_extracts_edge_shape() {
        let edge = make_edge("g1", "e1", "n1", "n2");
        let (src, dst) = edge_endpoints(&edge).expect("endpoints");
        assert_eq!(src, GraphNodeKey::new("g1", "n1").canonical_oid());
        assert_eq!(dst, GraphNodeKey::new("g1", "n2").canonical_oid());
    }

    #[test]
    fn edge_endpoints_returns_none_for_non_edge_records() {
        let node = make_node("g1", "n1");
        assert!(edge_endpoints(&node).is_none());
    }
}
