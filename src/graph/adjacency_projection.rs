/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Rebuildable graph adjacency projections over canonical edge records.
//!
//! This module is the root-runtime bridge for Phase 3 of
//! `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc`.
//! It indexes canonical graph edge `ProximaRecord`s into `edges_by_src` and
//! `edges_by_dst` tables for write-heavy graph paths. The projection is not
//! durable authority; it can be dropped and rebuilt from canonical records.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use async_trait::async_trait;
use proximadb_data_model::ProximaValue;
use proximadb_graph::projection::{
    AdjacencyIndexKind, AdjacencyProjectionEntry, GraphTopologyDescriptor, GraphTopologyProjection,
    ProjectionResult, TopologyApplyResult, TopologyEpoch, TopologyFormat,
    adjacency_entries_for_edge,
};
use proximadb_graph::record::{CanonicalEdge, CanonicalNode, GraphNodeKey};
use proximadb_records::{ProximaRecord, ProximaTree, ProximaTreeNode};

use crate::graph::{Edge, Node};
use crate::proto::proximadb_v1::{PropertyValue, property_value};

/// In-memory adjacency table projection for one graph dataset.
#[derive(Debug)]
pub struct InMemoryGraphAdjacencyProjection {
    descriptor: GraphTopologyDescriptor,
    state: RwLock<AdjacencyState>,
}

#[derive(Debug, Default)]
struct AdjacencyState {
    epoch: TopologyEpoch,
    edges_by_src: BTreeMap<String, BTreeMap<String, AdjacencyProjectionEntry>>,
    edges_by_dst: BTreeMap<String, BTreeMap<String, AdjacencyProjectionEntry>>,
}

impl InMemoryGraphAdjacencyProjection {
    pub fn new(graph_id: impl Into<String>) -> Self {
        let graph_id = graph_id.into();
        Self {
            descriptor: GraphTopologyDescriptor::new(
                format!("adjacency-{graph_id}"),
                graph_id,
                TopologyFormat::AdjacencyTable,
            ),
            state: RwLock::new(AdjacencyState::default()),
        }
    }

    /// Return outgoing adjacency entries for a canonical source node oid.
    pub fn edges_by_src(&self, node_oid: &str) -> ProjectionResult<Vec<AdjacencyProjectionEntry>> {
        let state = self.state.read().map_err(|_| {
            crate::core::error::ProximaDBError::Internal(
                "graph adjacency projection read lock poisoned".to_string(),
            )
        })?;
        Ok(state
            .edges_by_src
            .get(node_oid)
            .map(|edges| edges.values().cloned().collect())
            .unwrap_or_default())
    }

    /// Return incoming adjacency entries for a canonical destination node oid.
    pub fn edges_by_dst(&self, node_oid: &str) -> ProjectionResult<Vec<AdjacencyProjectionEntry>> {
        let state = self.state.read().map_err(|_| {
            crate::core::error::ProximaDBError::Internal(
                "graph adjacency projection read lock poisoned".to_string(),
            )
        })?;
        Ok(state
            .edges_by_dst
            .get(node_oid)
            .map(|edges| edges.values().cloned().collect())
            .unwrap_or_default())
    }

    pub fn edge_count(&self) -> ProjectionResult<usize> {
        let state = self.state.read().map_err(|_| {
            crate::core::error::ProximaDBError::Internal(
                "graph adjacency projection read lock poisoned".to_string(),
            )
        })?;
        Ok(state
            .edges_by_src
            .values()
            .map(std::collections::BTreeMap::len)
            .sum())
    }

    /// Snapshot all unique edges as `(from_node_oid, to_node_oid, edge_oid)` tuples.
    ///
    /// Only iterates `edges_by_src` (each edge appears exactly once there) so
    /// callers receive one tuple per edge without duplicates.  Used by
    /// `GraphOperationsService::rebuild_orion_csr_from_adjacency_projection`.
    pub fn snapshot_edge_endpoints(
        &self,
    ) -> ProjectionResult<Vec<(String, String, String)>> {
        let state = self.state.read().map_err(|_| {
            crate::core::error::ProximaDBError::Internal(
                "graph adjacency projection read lock poisoned".to_string(),
            )
        })?;
        let mut result = Vec::new();
        for (src_node_oid, edges) in &state.edges_by_src {
            for (edge_oid, entry) in edges {
                result.push((
                    src_node_oid.clone(),
                    entry.opposite_node_oid.clone(),
                    edge_oid.clone(),
                ));
            }
        }
        Ok(result)
    }

    fn apply_entries(state: &mut AdjacencyState, entries: [AdjacencyProjectionEntry; 2]) -> usize {
        let mut written = 0;
        for entry in entries {
            let edge_oid = entry.key.edge_oid.clone();
            match entry.key.kind {
                AdjacencyIndexKind::EdgesBySrc => {
                    let replaced = state
                        .edges_by_src
                        .entry(entry.key.node_oid.clone())
                        .or_default()
                        .insert(edge_oid, entry)
                        .is_some();
                    if !replaced {
                        written += 1;
                    }
                }
                AdjacencyIndexKind::EdgesByDst => {
                    let replaced = state
                        .edges_by_dst
                        .entry(entry.key.node_oid.clone())
                        .or_default()
                        .insert(edge_oid, entry)
                        .is_some();
                    if !replaced {
                        written += 1;
                    }
                }
            }
        }
        written
    }

    fn remove_edge_oid(state: &mut AdjacencyState, edge_oid: &str) -> bool {
        let mut removed = false;
        for by_node in [&mut state.edges_by_src, &mut state.edges_by_dst] {
            let empty_nodes = by_node
                .iter_mut()
                .filter_map(|(node_oid, edges)| {
                    removed |= edges.remove(edge_oid).is_some();
                    edges.is_empty().then(|| node_oid.clone())
                })
                .collect::<Vec<_>>();
            for node_oid in empty_nodes {
                by_node.remove(&node_oid);
            }
        }
        removed
    }
}

/// Convert the root graph edge compatibility shape into a canonical edge
/// record for projection maintenance.
pub fn edge_to_canonical_record(graph_id: &str, edge: &Edge) -> ProximaRecord {
    let mut canonical = CanonicalEdge::new(
        graph_id,
        edge.id.clone(),
        GraphNodeKey::new(graph_id, edge.from_node_id.clone()).canonical_oid(),
        GraphNodeKey::new(graph_id, edge.to_node_id.clone()).canonical_oid(),
        edge.edge_type.clone(),
        property_map_to_tree(&edge.properties),
    );

    canonical.created_at_ns = edge.created_at_ms.saturating_mul(1_000_000);
    canonical.updated_at_ns = edge.updated_at_ms.saturating_mul(1_000_000);
    canonical.into_proxima_record()
}

/// Convert the root graph node compatibility shape into a canonical node
/// record for durable record-store writes.
pub fn node_to_canonical_record(graph_id: &str, node: &Node) -> ProximaRecord {
    let node_label = node.labels.first().cloned().unwrap_or_default();
    let mut canonical = CanonicalNode::new(
        graph_id,
        node.id.clone(),
        node_label,
        property_map_to_tree(&node.properties),
    );

    canonical.created_at_ns = node.created_at_ms.saturating_mul(1_000_000);
    canonical.updated_at_ns = node.updated_at_ms.saturating_mul(1_000_000);
    canonical.into_proxima_record()
}

fn property_map_to_tree(
    properties: &std::collections::HashMap<String, PropertyValue>,
) -> ProximaTree {
    properties
        .iter()
        .map(|(key, value)| (key.clone(), property_value_to_tree_node(value)))
        .collect()
}

fn property_value_to_tree_node(value: &PropertyValue) -> ProximaTreeNode {
    match &value.value {
        Some(property_value::Value::ObjectValue(object)) => ProximaTreeNode::Object(
            object
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), property_value_to_tree_node(value)))
                .collect(),
        ),
        _ => ProximaTreeNode::Value(property_value_to_proxima(value)),
    }
}

fn property_value_to_proxima(value: &PropertyValue) -> ProximaValue {
    match &value.value {
        Some(property_value::Value::StringValue(value)) => ProximaValue::String(value.clone()),
        Some(property_value::Value::IntValue(value)) => ProximaValue::Int64(*value),
        Some(property_value::Value::DoubleValue(value)) => ProximaValue::Float64(*value),
        Some(property_value::Value::BoolValue(value)) => ProximaValue::Boolean(*value),
        Some(property_value::Value::BytesValue(value)) => ProximaValue::Binary(value.clone()),
        Some(property_value::Value::ArrayValue(array)) => {
            ProximaValue::Array(array.values.iter().map(property_value_to_proxima).collect())
        }
        Some(property_value::Value::ObjectValue(object)) => ProximaValue::Struct(
            object
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), property_value_to_proxima(value)))
                .collect(),
        ),
        Some(property_value::Value::VectorValue(vector)) => {
            ProximaValue::DenseVector(vector.values.clone())
        }
        None => ProximaValue::Null,
    }
}

#[async_trait]
impl GraphTopologyProjection for InMemoryGraphAdjacencyProjection {
    fn descriptor(&self) -> &GraphTopologyDescriptor {
        &self.descriptor
    }

    fn applied_epoch(&self) -> TopologyEpoch {
        self.state
            .read()
            .map(|state| state.epoch)
            .unwrap_or_else(|_| TopologyEpoch::initial())
    }

    async fn apply_edge(
        &self,
        edge_record: &ProximaRecord,
    ) -> ProjectionResult<TopologyApplyResult> {
        let entries = adjacency_entries_for_edge(edge_record).ok_or_else(|| {
            crate::core::error::ProximaDBError::InvalidInput(
                "record is not a canonical graph edge".to_string(),
            )
        })?;

        let mut state = self.state.write().map_err(|_| {
            crate::core::error::ProximaDBError::Internal(
                "graph adjacency projection write lock poisoned".to_string(),
            )
        })?;

        Self::remove_edge_oid(&mut state, &edge_record.oid);
        let entries_written = Self::apply_entries(&mut state, entries);
        state.epoch = state.epoch.next();

        Ok(TopologyApplyResult {
            edge_oid: edge_record.oid.clone(),
            entries_written,
            applied_epoch: state.epoch,
        })
    }

    async fn remove_edge(&self, edge_record: &ProximaRecord) -> ProjectionResult<bool> {
        let mut state = self.state.write().map_err(|_| {
            crate::core::error::ProximaDBError::Internal(
                "graph adjacency projection write lock poisoned".to_string(),
            )
        })?;
        let removed = Self::remove_edge_oid(&mut state, &edge_record.oid);
        if removed {
            state.epoch = state.epoch.next();
        }
        Ok(removed)
    }

    async fn rebuild_from_records(
        &self,
        _node_records: &[ProximaRecord],
        edge_records: &[ProximaRecord],
    ) -> ProjectionResult<usize> {
        let mut next = AdjacencyState::default();
        let mut edge_oids = BTreeSet::new();

        for edge_record in edge_records {
            if let Some(entries) = adjacency_entries_for_edge(edge_record) {
                Self::apply_entries(&mut next, entries);
                edge_oids.insert(edge_record.oid.clone());
            }
        }

        next.epoch = self.applied_epoch().next();
        let edge_count = edge_oids.len();

        let mut state = self.state.write().map_err(|_| {
            crate::core::error::ProximaDBError::Internal(
                "graph adjacency projection write lock poisoned".to_string(),
            )
        })?;
        *state = next;

        Ok(edge_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_graph::record::{CanonicalEdge, GraphNodeKey};
    use proximadb_records::ProximaTree;
    use std::collections::HashMap;

    fn edge_record(graph_id: &str, edge_id: &str, src: &str, dst: &str) -> ProximaRecord {
        CanonicalEdge::new(
            graph_id,
            edge_id,
            GraphNodeKey::new(graph_id, src).canonical_oid(),
            GraphNodeKey::new(graph_id, dst).canonical_oid(),
            "KNOWS",
            ProximaTree::new(),
        )
        .into_proxima_record()
    }

    #[test]
    fn root_edge_maps_to_canonical_edge_record() {
        let mut properties = HashMap::new();
        properties.insert(
            "rank".to_string(),
            PropertyValue {
                value: Some(property_value::Value::IntValue(7)),
            },
        );
        let edge = Edge {
            id: "e1".to_string(),
            from_node_id: "n1".to_string(),
            to_node_id: "n2".to_string(),
            edge_type: "KNOWS".to_string(),
            properties,
            weight: Some(0.5),
            created_at_ms: 10,
            updated_at_ms: 20,
        };

        let record = edge_to_canonical_record("g1", &edge);

        assert_eq!(record.oid, "graph/g1/edge/e1");
        assert_eq!(record.created_at_ns, 10_000_000);
        assert_eq!(record.updated_at_ns, 20_000_000);
        assert_eq!(
            record.edge.as_ref().expect("edge shape").source_id,
            "graph/g1/node/n1"
        );
        assert_eq!(
            record.props.get("rank"),
            Some(&ProximaTreeNode::Value(ProximaValue::Int64(7)))
        );
    }

    #[test]
    fn root_node_maps_to_canonical_node_record() {
        let mut properties = HashMap::new();
        properties.insert(
            "name".to_string(),
            PropertyValue {
                value: Some(property_value::Value::StringValue("Ada".to_string())),
            },
        );
        let node = Node {
            id: "n1".to_string(),
            labels: vec!["Person".to_string()],
            properties,
            embedding: None,
            created_at_ms: 10,
            updated_at_ms: 20,
        };

        let record = node_to_canonical_record("g1", &node);

        assert_eq!(record.oid, "graph/g1/node/n1");
        assert_eq!(record.created_at_ns, 10_000_000);
        assert_eq!(record.updated_at_ns, 20_000_000);
        assert_eq!(
            record.props.get("name"),
            Some(&ProximaTreeNode::Value(ProximaValue::String(
                "Ada".to_string()
            )))
        );
    }

    #[tokio::test]
    async fn indexes_edges_by_src_and_dst() {
        let projection = InMemoryGraphAdjacencyProjection::new("g1");
        let edge = edge_record("g1", "e1", "n1", "n2");

        let result = projection.apply_edge(&edge).await.expect("apply edge");
        assert_eq!(result.entries_written, 2);
        assert_eq!(result.applied_epoch, TopologyEpoch(1));

        let outgoing = projection
            .edges_by_src("graph/g1/node/n1")
            .expect("outgoing entries");
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].opposite_node_oid, "graph/g1/node/n2");

        let incoming = projection
            .edges_by_dst("graph/g1/node/n2")
            .expect("incoming entries");
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].opposite_node_oid, "graph/g1/node/n1");
    }

    #[tokio::test]
    async fn update_replaces_existing_edge_entries() {
        let projection = InMemoryGraphAdjacencyProjection::new("g1");
        let first = edge_record("g1", "e1", "n1", "n2");
        let second = edge_record("g1", "e1", "n1", "n3");

        projection.apply_edge(&first).await.expect("apply first");
        projection.apply_edge(&second).await.expect("apply second");

        assert_eq!(projection.edge_count().expect("edge count"), 1);
        assert!(
            projection
                .edges_by_dst("graph/g1/node/n2")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            projection.edges_by_dst("graph/g1/node/n3").unwrap()[0]
                .key
                .edge_oid,
            "graph/g1/edge/e1"
        );
        assert_eq!(projection.applied_epoch(), TopologyEpoch(2));
    }

    #[tokio::test]
    async fn removes_edge_from_both_directions() {
        let projection = InMemoryGraphAdjacencyProjection::new("g1");
        let edge = edge_record("g1", "e1", "n1", "n2");

        projection.apply_edge(&edge).await.expect("apply edge");
        assert!(projection.remove_edge(&edge).await.expect("remove edge"));

        assert!(
            projection
                .edges_by_src("graph/g1/node/n1")
                .unwrap()
                .is_empty()
        );
        assert!(
            projection
                .edges_by_dst("graph/g1/node/n2")
                .unwrap()
                .is_empty()
        );
        assert_eq!(projection.applied_epoch(), TopologyEpoch(2));
    }

    #[tokio::test]
    async fn rebuilds_from_canonical_edge_records() {
        let projection = InMemoryGraphAdjacencyProjection::new("g1");
        let edges = vec![
            edge_record("g1", "e1", "n1", "n2"),
            edge_record("g1", "e2", "n1", "n3"),
        ];

        let rebuilt = projection
            .rebuild_from_records(&[], &edges)
            .await
            .expect("rebuild");

        assert_eq!(rebuilt, 2);
        assert_eq!(
            projection.edges_by_src("graph/g1/node/n1").unwrap().len(),
            2
        );
        assert_eq!(projection.edge_count().expect("edge count"), 2);
        assert!(projection.freshness(TopologyEpoch(1)).is_fresh());
    }
}
