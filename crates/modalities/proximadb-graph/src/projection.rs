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

use crate::record::{GRAPH_EDGE_LABEL, GRAPH_ID_PROP, GRAPH_NODE_LABEL};

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

/// Directional adjacency index maintained from canonical edge records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjacencyIndexKind {
    /// `edges_by_src`: source node oid -> edge records leaving that node.
    EdgesBySrc,
    /// `edges_by_dst`: destination node oid -> edge records entering that node.
    EdgesByDst,
}

impl AdjacencyIndexKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EdgesBySrc => "edges_by_src",
            Self::EdgesByDst => "edges_by_dst",
        }
    }
}

/// Deterministic key for a directional adjacency projection entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjacencyProjectionKey {
    /// Graph dataset id.
    pub graph_id: String,
    /// Directional adjacency index.
    pub kind: AdjacencyIndexKind,
    /// Canonical node oid used as source or destination, depending on `kind`.
    pub node_oid: String,
    /// Canonical edge record oid.
    pub edge_oid: String,
}

impl AdjacencyProjectionKey {
    pub fn new(
        graph_id: impl Into<String>,
        kind: AdjacencyIndexKind,
        node_oid: impl Into<String>,
        edge_oid: impl Into<String>,
    ) -> Self {
        Self {
            graph_id: graph_id.into(),
            kind,
            node_oid: node_oid.into(),
            edge_oid: edge_oid.into(),
        }
    }

    /// Stable storage key for projection engines and cache invalidation.
    pub fn storage_key(&self) -> String {
        format!(
            "graph/{}/{}/{}/{}",
            self.graph_id,
            self.kind.as_str(),
            self.node_oid,
            self.edge_oid
        )
    }
}

/// One rebuildable adjacency entry derived from a canonical edge record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjacencyProjectionEntry {
    pub key: AdjacencyProjectionKey,
    pub opposite_node_oid: String,
    pub edge_label: String,
    pub record_version: u64,
    pub updated_at_ns: i64,
}

/// Monotonic freshness marker for topology projections.
///
/// The source of truth is the canonical edge record epoch. CSR projections are
/// fresh only when their applied epoch is at least the latest edge-change epoch
/// observed by the graph facade.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TopologyEpoch(pub u64);

impl TopologyEpoch {
    pub fn initial() -> Self {
        Self(0)
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Projection freshness state used by CSR/COO/adjacency consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionFreshness {
    /// Projection has applied the latest observed canonical edge epoch.
    Fresh,
    /// Projection can answer reads but is behind canonical edge state.
    Stale {
        applied_epoch: TopologyEpoch,
        required_epoch: TopologyEpoch,
    },
}

impl ProjectionFreshness {
    pub fn from_epochs(applied_epoch: TopologyEpoch, required_epoch: TopologyEpoch) -> Self {
        if applied_epoch >= required_epoch {
            Self::Fresh
        } else {
            Self::Stale {
                applied_epoch,
                required_epoch,
            }
        }
    }

    pub fn is_fresh(self) -> bool {
        matches!(self, Self::Fresh)
    }
}

/// Result of applying one canonical edge record to a topology projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyApplyResult {
    /// Edge record oid that was consumed.
    pub edge_oid: String,
    /// Number of adjacency entries updated.
    pub entries_written: usize,
    /// Projection epoch after the edge mutation is applied.
    pub applied_epoch: TopologyEpoch,
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

    /// Last canonical edge epoch applied by this projection.
    fn applied_epoch(&self) -> TopologyEpoch {
        TopologyEpoch::initial()
    }

    /// Freshness against the latest canonical edge epoch known to the caller.
    fn freshness(&self, required_epoch: TopologyEpoch) -> ProjectionFreshness {
        ProjectionFreshness::from_epochs(self.applied_epoch(), required_epoch)
    }

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

/// Build the `edges_by_src` and `edges_by_dst` adjacency entries for one edge.
pub fn adjacency_entries_for_edge(record: &ProximaRecord) -> Option<[AdjacencyProjectionEntry; 2]> {
    let graph_id = graph_id(record)?;
    let edge_shape = record.edge.as_ref()?;
    let edge_label = edge_shape.edge_type.clone();

    Some([
        AdjacencyProjectionEntry {
            key: AdjacencyProjectionKey::new(
                graph_id.clone(),
                AdjacencyIndexKind::EdgesBySrc,
                edge_shape.source_id.clone(),
                record.oid.clone(),
            ),
            opposite_node_oid: edge_shape.target_id.clone(),
            edge_label: edge_label.clone(),
            record_version: record.record_version,
            updated_at_ns: record.updated_at_ns,
        },
        AdjacencyProjectionEntry {
            key: AdjacencyProjectionKey::new(
                graph_id,
                AdjacencyIndexKind::EdgesByDst,
                edge_shape.target_id.clone(),
                record.oid.clone(),
            ),
            opposite_node_oid: edge_shape.source_id.clone(),
            edge_label,
            record_version: record.record_version,
            updated_at_ns: record.updated_at_ns,
        },
    ])
}

fn graph_id(record: &ProximaRecord) -> Option<String> {
    use proximadb_data_model::ProximaValue;
    use proximadb_records::ProximaTreeNode;

    match record.props.get(GRAPH_ID_PROP) {
        Some(ProximaTreeNode::Value(ProximaValue::String(id))) => Some(id.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{CanonicalEdge, CanonicalNode, GraphNodeKey};
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

    #[test]
    fn adjacency_entries_are_directional_and_stable() {
        let edge = make_edge("g1", "e1", "n1", "n2");
        let entries = adjacency_entries_for_edge(&edge).expect("adjacency entries");

        assert_eq!(entries[0].key.kind, AdjacencyIndexKind::EdgesBySrc);
        assert_eq!(entries[0].key.node_oid, "graph/g1/node/n1");
        assert_eq!(entries[0].opposite_node_oid, "graph/g1/node/n2");
        assert_eq!(
            entries[0].key.storage_key(),
            "graph/g1/edges_by_src/graph/g1/node/n1/graph/g1/edge/e1"
        );

        assert_eq!(entries[1].key.kind, AdjacencyIndexKind::EdgesByDst);
        assert_eq!(entries[1].key.node_oid, "graph/g1/node/n2");
        assert_eq!(entries[1].opposite_node_oid, "graph/g1/node/n1");
        assert_eq!(
            entries[1].key.storage_key(),
            "graph/g1/edges_by_dst/graph/g1/node/n2/graph/g1/edge/e1"
        );
    }

    #[test]
    fn projection_freshness_tracks_required_epoch() {
        let applied = TopologyEpoch(3);

        assert!(ProjectionFreshness::from_epochs(applied, TopologyEpoch(2)).is_fresh());
        assert!(ProjectionFreshness::from_epochs(applied, TopologyEpoch(3)).is_fresh());
        assert_eq!(
            ProjectionFreshness::from_epochs(applied, TopologyEpoch(4)),
            ProjectionFreshness::Stale {
                applied_epoch: TopologyEpoch(3),
                required_epoch: TopologyEpoch(4)
            }
        );
    }
}
