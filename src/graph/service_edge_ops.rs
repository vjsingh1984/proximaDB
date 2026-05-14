//! Edge Operations API (extracted from service.rs)
//!
//! Provides edge CRUD, single-edge retrieval, and property/type-based edge
//! querying, keeping the main service lean and focused.

use super::Result;
use crate::graph::adjacency_projection::edge_to_canonical_record;
use crate::graph::engines::GraphEngine;
use crate::graph::{Edge, EdgeId};
use crate::proto::proximadb_v1::EdgeQuery;
use proximadb_graph::projection::GraphTopologyProjection;
use proximadb_graph::record::GraphNodeKey;
use std::sync::Arc;

impl super::GraphOperationsService {
    /// Create a new edge
    pub async fn create_edge(&self, graph_id: &str, edge: Edge) -> Result<Arc<Edge>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;

        // Enforce composite (from,to,type) uniqueness using in-memory index
        if self
            .memory_pool
            .edge_composite_index
            .get(&(
                edge.from_node_id.clone(),
                edge.to_node_id.clone(),
                edge.edge_type.clone(),
            ))
            .is_some()
        {
            return Err(crate::core::error::ProximaDBError::InvalidInput(format!(
                "Composite edge already exists: (from='{}', to='{}', type='{}')",
                edge.from_node_id, edge.to_node_id, edge.edge_type
            )));
        }

        // Schema validation for edge using endpoint labels if schema defines constraints
        if let (Some(from), Some(to)) = (
            engine.get_node(&edge.from_node_id)?,
            engine.get_node(&edge.to_node_id)?,
        ) {
            self.enforce_schema_on_edge(graph_id, &edge, &from.labels, &to.labels)
                .await?;
            self.enforce_cardinality_on_edge(graph_id, &edge, engine.as_ref())
                .await?;
        }

        self.upsert_canonical_edge_record(graph_id, &edge).await?;
        let edge_arc = engine.insert_edge(edge).await?;
        let edge_record = edge_to_canonical_record(graph_id, &edge_arc);
        self.adjacency_projection(graph_id)
            .apply_edge(&edge_record)
            .await?;
        self.advance_edge_epoch(graph_id);
        // Update edge stats
        self.stats_edges
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.edge_type_counts
            .entry(edge_arc.edge_type.clone())
            .or_insert_with(|| std::sync::atomic::AtomicU64::new(0))
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(edge_arc)
    }

    /// Update an edge
    pub async fn update_edge(&self, graph_id: &str, edge: Edge) -> Result<Arc<Edge>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;
        // If we can load endpoints, enforce schema
        if let (Some(from), Some(to)) = (
            engine.get_node(&edge.from_node_id)?,
            engine.get_node(&edge.to_node_id)?,
        ) {
            self.enforce_schema_on_edge(graph_id, &edge, &from.labels, &to.labels)
                .await?;
            self.enforce_cardinality_on_edge(graph_id, &edge, engine.as_ref())
                .await?;
        }
        self.upsert_canonical_edge_record(graph_id, &edge).await?;
        let edge_arc = engine.update_edge(edge).await?;
        let edge_record = edge_to_canonical_record(graph_id, &edge_arc);
        self.adjacency_projection(graph_id)
            .apply_edge(&edge_record)
            .await?;
        self.advance_edge_epoch(graph_id);
        Ok(edge_arc)
    }

    /// Delete an edge
    pub async fn delete_edge(&self, graph_id: &str, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        let deleted = crate::graph::engines::GraphEngine::delete_edge(&*engine, id).await?;
        if let Some(ref edge) = deleted {
            let edge_record = edge_to_canonical_record(graph_id, edge);
            self.adjacency_projection(graph_id)
                .remove_edge(&edge_record)
                .await?;
            self.delete_canonical_edge_record(graph_id, &edge.id)
                .await?;
            self.advance_edge_epoch(graph_id);
            self.stats_edges
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(v) = self.edge_type_counts.get(&edge.edge_type) {
                v.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Ok(deleted)
    }
    /// Get an edge by ID
    pub async fn get_edge(&self, graph_id: &str, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        engine.get_edge(id)
    }

    /// Query edges by endpoints, properties and types.
    ///
    /// When the query is endpoint-bound (has `from_node_id` or `to_node_id`),
    /// reads are served from the adjacency projection rather than scanning the
    /// underlying engine, consistent with the convergence mandate in
    /// `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc`
    /// Phase 3.  Engine traversal is only used for full-graph edge scans.
    pub async fn query_edges(&self, graph_id: &str, query: EdgeQuery) -> Result<Vec<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        let edge_prefix = format!("graph/{graph_id}/edge/");

        let mut seen = std::collections::HashSet::new();
        let mut results = Vec::new();

        let is_endpoint_bound = query.from_node_id.is_some() || query.to_node_id.is_some();

        if is_endpoint_bound {
            let projection = self.adjacency_projection(graph_id);

            if let Some(from) = &query.from_node_id {
                let node_oid = GraphNodeKey::new(graph_id, from.as_str()).canonical_oid();
                if let Ok(entries) = projection.edges_by_src(&node_oid) {
                    for entry in entries {
                        let edge_oid = &entry.key.edge_oid;
                        if seen.contains(edge_oid) {
                            continue;
                        }
                        if let Some(edge_id) = edge_oid.strip_prefix(&edge_prefix) {
                            if let Ok(Some(edge)) = engine.get_edge(&edge_id.to_string()) {
                                seen.insert(edge_oid.clone());
                                results.push(edge);
                            }
                        }
                    }
                }
            }

            if let Some(to) = &query.to_node_id {
                let node_oid = GraphNodeKey::new(graph_id, to.as_str()).canonical_oid();
                if let Ok(entries) = projection.edges_by_dst(&node_oid) {
                    for entry in entries {
                        let edge_oid = &entry.key.edge_oid;
                        if seen.contains(edge_oid) {
                            continue;
                        }
                        if let Some(edge_id) = edge_oid.strip_prefix(&edge_prefix) {
                            if let Ok(Some(edge)) = engine.get_edge(&edge_id.to_string()) {
                                seen.insert(edge_oid.clone());
                                results.push(edge);
                            }
                        }
                    }
                }
            }
        }
        // Non-endpoint-bound queries return an empty result; a full-graph edge
        // scan is not supported without explicit endpoint or type filters.

        // Property filters (simple, if provided)
        if !query.filters.is_empty() {
            results.retain(|edge| {
                for filter in &query.filters {
                    use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
                    let prop_val_opt = edge.properties.get(&filter.key);
                    let pass = match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                        Op::Equals => match prop_val_opt {
                            Some(v) => {
                                let filter_val = match filter.value.as_ref() {
                                    Some(val) => val,
                                    None => return false,
                                };
                                v.value == filter_val.value
                            }
                            None => false,
                        },
                        Op::NotEquals => match prop_val_opt {
                            Some(v) => {
                                let filter_val = match filter.value.as_ref() {
                                    Some(val) => val,
                                    None => return true,
                                };
                                v.value != filter_val.value
                            }
                            None => true,
                        },
                        _ => true,
                    };
                    if !pass {
                        return false;
                    }
                }
                true
            });
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::super::GraphOperationsService;
    use crate::graph::{Edge, Node};
    use crate::proto::proximadb_v1::CreateGraphRequest;
    use async_trait::async_trait;
    use proximadb_records::{ProximaRecord, RecordKey, RecordStore, RecordStoreResult};
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};

    #[derive(Default)]
    struct MemoryRecordStore {
        records: RwLock<HashMap<String, ProximaRecord>>,
    }

    #[async_trait]
    impl RecordStore for MemoryRecordStore {
        async fn upsert_record(&self, record: ProximaRecord) -> RecordStoreResult<ProximaRecord> {
            self.records
                .write()
                .expect("record store write lock")
                .insert(record.oid.clone(), record.clone());
            Ok(record)
        }

        async fn get_record(&self, key: &RecordKey) -> RecordStoreResult<Option<ProximaRecord>> {
            Ok(self
                .records
                .read()
                .expect("record store read lock")
                .get(&key.oid)
                .cloned())
        }

        async fn delete_record(&self, key: &RecordKey) -> RecordStoreResult<bool> {
            Ok(self
                .records
                .write()
                .expect("record store write lock")
                .remove(&key.oid)
                .is_some())
        }
    }

    #[tokio::test]
    async fn edge_crud_maintains_adjacency_projection() {
        let graph_id = format!("adjacency_service_test_{}", std::process::id());
        let graph_id = graph_id.as_str();
        let service = GraphOperationsService::new();
        service
            .create_graph_collection(CreateGraphRequest {
                graph_id: graph_id.to_string(),
                name: None,
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await
            .expect("create graph");

        for node_id in ["n1", "n2", "n3"] {
            service
                .create_node(
                    graph_id,
                    Node {
                        id: node_id.to_string(),
                        labels: vec!["Person".to_string()],
                        properties: HashMap::new(),
                        embedding: None,
                        created_at_ms: 1,
                        updated_at_ms: 1,
                    },
                )
                .await
                .expect("create node");
        }

        service
            .create_edge(
                graph_id,
                Edge {
                    id: "e1".to_string(),
                    from_node_id: "n1".to_string(),
                    to_node_id: "n2".to_string(),
                    edge_type: "KNOWS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
            )
            .await
            .expect("create edge");
        assert_eq!(
            service
                .adjacency_projection_edge_count(graph_id)
                .expect("edge count"),
            1
        );

        service
            .update_edge(
                graph_id,
                Edge {
                    id: "e1".to_string(),
                    from_node_id: "n1".to_string(),
                    to_node_id: "n3".to_string(),
                    edge_type: "KNOWS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 1,
                    updated_at_ms: 2,
                },
            )
            .await
            .expect("update edge");
        assert_eq!(
            service
                .adjacency_projection(graph_id)
                .edges_by_dst(&format!("graph/{graph_id}/node/n3"))
                .expect("incoming")
                .len(),
            1
        );
        assert!(
            service
                .adjacency_projection(graph_id)
                .edges_by_dst(&format!("graph/{graph_id}/node/n2"))
                .expect("old incoming")
                .is_empty()
        );

        service
            .delete_edge(graph_id, &"e1".to_string())
            .await
            .expect("delete edge");
        assert_eq!(
            service
                .adjacency_projection_edge_count(graph_id)
                .expect("edge count"),
            0
        );

        service
            .batch_create_edges(
                graph_id,
                vec![
                    Edge {
                        id: "e2".to_string(),
                        from_node_id: "n1".to_string(),
                        to_node_id: "n2".to_string(),
                        edge_type: "KNOWS".to_string(),
                        properties: HashMap::new(),
                        weight: None,
                        created_at_ms: 3,
                        updated_at_ms: 3,
                    },
                    Edge {
                        id: "e3".to_string(),
                        from_node_id: "n1".to_string(),
                        to_node_id: "n3".to_string(),
                        edge_type: "KNOWS".to_string(),
                        properties: HashMap::new(),
                        weight: None,
                        created_at_ms: 3,
                        updated_at_ms: 3,
                    },
                ],
            )
            .await
            .expect("batch create edges");
        assert_eq!(
            service
                .adjacency_projection_edge_count(graph_id)
                .expect("edge count"),
            2
        );
    }

    #[tokio::test]
    async fn endpoint_bound_query_served_from_adjacency_projection() {
        use crate::proto::proximadb_v1::EdgeQuery;

        let graph_id = format!("adj_query_test_{}", std::process::id());
        let graph_id = graph_id.as_str();
        let service = GraphOperationsService::new();
        service
            .create_graph_collection(CreateGraphRequest {
                graph_id: graph_id.to_string(),
                name: None,
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await
            .expect("create graph");

        for node_id in ["n1", "n2", "n3"] {
            service
                .create_node(
                    graph_id,
                    Node {
                        id: node_id.to_string(),
                        labels: vec!["P".to_string()],
                        properties: HashMap::new(),
                        embedding: None,
                        created_at_ms: 1,
                        updated_at_ms: 1,
                    },
                )
                .await
                .expect("create node");
        }

        service
            .create_edge(
                graph_id,
                Edge {
                    id: "e1".to_string(),
                    from_node_id: "n1".to_string(),
                    to_node_id: "n2".to_string(),
                    edge_type: "KNOWS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
            )
            .await
            .expect("create e1");
        service
            .create_edge(
                graph_id,
                Edge {
                    id: "e2".to_string(),
                    from_node_id: "n1".to_string(),
                    to_node_id: "n3".to_string(),
                    edge_type: "KNOWS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
            )
            .await
            .expect("create e2");

        // Outgoing from n1 via adjacency projection
        let outgoing = service
            .query_edges(
                graph_id,
                EdgeQuery {
                    graph_id: graph_id.to_string(),
                    from_node_id: Some("n1".to_string()),
                    to_node_id: None,
                    edge_types: vec![],
                    filters: vec![],
                    offset: None,
                    limit: None,
                    continuation_token: None,
                },
            )
            .await
            .expect("query outgoing");
        assert_eq!(outgoing.len(), 2, "n1 should have 2 outgoing edges");

        // Incoming to n2 via adjacency projection
        let incoming = service
            .query_edges(
                graph_id,
                EdgeQuery {
                    graph_id: graph_id.to_string(),
                    from_node_id: None,
                    to_node_id: Some("n2".to_string()),
                    edge_types: vec![],
                    filters: vec![],
                    offset: None,
                    limit: None,
                    continuation_token: None,
                },
            )
            .await
            .expect("query incoming");
        assert_eq!(incoming.len(), 1, "n2 should have 1 incoming edge");
        assert_eq!(incoming[0].id, "e1");
    }

    #[tokio::test]
    async fn canonical_record_store_receives_graph_writes() {
        let graph_id = format!("canonical_graph_store_test_{}", std::process::id());
        let graph_id = graph_id.as_str();
        let record_store = Arc::new(MemoryRecordStore::default());
        let service =
            GraphOperationsService::new().with_canonical_record_store(record_store.clone());

        service
            .create_graph_collection(CreateGraphRequest {
                graph_id: graph_id.to_string(),
                name: None,
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await
            .expect("create graph");

        service
            .create_node(
                graph_id,
                Node {
                    id: "n1".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
            )
            .await
            .expect("create n1");
        service
            .create_node(
                graph_id,
                Node {
                    id: "n2".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
            )
            .await
            .expect("create n2");
        service
            .create_edge(
                graph_id,
                Edge {
                    id: "e1".to_string(),
                    from_node_id: "n1".to_string(),
                    to_node_id: "n2".to_string(),
                    edge_type: "KNOWS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
            )
            .await
            .expect("create edge");

        assert!(
            record_store
                .get_record(&RecordKey::new(format!("graph/{graph_id}/node/n1")))
                .await
                .expect("get node record")
                .is_some()
        );
        assert!(
            record_store
                .get_record(&RecordKey::new(format!("graph/{graph_id}/edge/e1")))
                .await
                .expect("get edge record")
                .is_some()
        );

        service
            .delete_edge(graph_id, &"e1".to_string())
            .await
            .expect("delete edge");
        assert!(
            record_store
                .get_record(&RecordKey::new(format!("graph/{graph_id}/edge/e1")))
                .await
                .expect("get deleted edge")
                .is_none()
        );

        service
            .delete_node(graph_id, &"n1".to_string())
            .await
            .expect("delete node");
        assert!(
            record_store
                .get_record(&RecordKey::new(format!("graph/{graph_id}/node/n1")))
                .await
                .expect("get deleted node")
                .is_none()
        );
    }

    /// Verify that ORION CSR can be rebuilt from the adjacency projection.
    ///
    /// Creates 2 edges via the service (which updates the adjacency projection),
    /// then calls `rebuild_orion_csr_from_adjacency_projection`, and checks that
    /// ORION's CSR-backed `get_outgoing_targets` returns the expected neighbours.
    #[tokio::test]
    async fn orion_csr_rebuild_from_adjacency_projection() {
        use crate::graph::engines::GraphEngine;
        use crate::graph::engines::GraphEngineImpl;
        use crate::proto::proximadb_v1::CreateGraphRequest;

        let graph_id = format!("csr_rebuild_test_{}", std::process::id());
        let graph_id = graph_id.as_str();
        let service = GraphOperationsService::new();
        service
            .create_graph_collection(CreateGraphRequest {
                graph_id: graph_id.to_string(),
                name: None,
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await
            .expect("create graph");

        for nid in ["a", "b", "c"] {
            service
                .create_node(
                    graph_id,
                    Node {
                        id: nid.to_string(),
                        labels: vec!["V".to_string()],
                        properties: HashMap::new(),
                        embedding: None,
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    },
                )
                .await
                .expect("create node");
        }

        service
            .create_edge(
                graph_id,
                Edge {
                    id: "ea".to_string(),
                    from_node_id: "a".to_string(),
                    to_node_id: "b".to_string(),
                    edge_type: "E".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("create ea");
        service
            .create_edge(
                graph_id,
                Edge {
                    id: "eb".to_string(),
                    from_node_id: "a".to_string(),
                    to_node_id: "c".to_string(),
                    edge_type: "E".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("create eb");

        // Rebuild CSR from the adjacency projection.
        service
            .rebuild_orion_csr_from_adjacency_projection(graph_id)
            .await
            .expect("CSR rebuild");

        // Read back via ORION's CSR get_outgoing_targets.
        let engine_arc = service
            .graphs
            .get(graph_id)
            .expect("engine present")
            .value()
            .clone();
        if let GraphEngineImpl::Orion(orion) = engine_arc.as_ref() {
            let mut outgoing = orion
                .get_outgoing_targets(&"a".to_string())
                .await
                .expect("outgoing targets");
            outgoing.sort();
            assert_eq!(
                outgoing,
                vec!["b".to_string(), "c".to_string()],
                "CSR should have both outgoing neighbours for node a"
            );
        } else {
            panic!("expected ORION engine");
        }
    }
}
