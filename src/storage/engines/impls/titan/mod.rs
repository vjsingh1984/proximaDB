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

//! # TITAN Graph Storage Engine
//!
//! **STATUS**: Skeleton (Apr 2026)
//!
//! Traversal-Indexed Topology and Adjacency Network -- an LSM-backed graph
//! storage engine for ProximaDB.
//!
//! TITAN is primarily a **GraphEngine** (not a general-purpose vector store).
//! The `TitanEngine` struct provides a thin `UnifiedStorageEngine` stub so the
//! engine can be registered via the factory, but all meaningful graph work is
//! handled by `TitanGraphEngine`.
//!
//! ## Design
//!
//! - DashMap-based concurrent node/edge storage with Arc zero-copy sharing
//! - Separate outgoing/incoming adjacency lists for O(degree) traversal
//! - Proto-first: uses `crate::proto::proximadb_v1::{Node, Edge}` directly

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;

use crate::proto::proximadb_v1::{Edge, Node};
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult,
    StorageEngineStrategy, StorageQueryContext, UnifiedStorageEngine,
};

use crate::proto::proximadb_v1::VectorRecord;

// ---------------------------------------------------------------------------
// TitanGraphEngine -- the real graph engine
// ---------------------------------------------------------------------------

/// TITAN graph engine -- concurrent in-memory graph with adjacency indexes.
///
/// All nodes and edges are stored as `Arc<Node>` / `Arc<Edge>` for zero-copy
/// sharing across concurrent readers.
pub struct TitanGraphEngine {
    /// Primary node storage keyed by node id.
    nodes: DashMap<String, Arc<Node>>,
    /// Primary edge storage keyed by edge id.
    edges: DashMap<String, Arc<Edge>>,
    /// Outgoing adjacency list: source node id -> list of edges leaving that node.
    outgoing: DashMap<String, Vec<Arc<Edge>>>,
    /// Incoming adjacency list: target node id -> list of edges arriving at that node.
    incoming: DashMap<String, Vec<Arc<Edge>>>,
}

impl TitanGraphEngine {
    /// Create an empty `TitanGraphEngine` with no nodes or edges.
    pub fn new() -> Self {
        Self {
            nodes: DashMap::new(),
            edges: DashMap::new(),
            outgoing: DashMap::new(),
            incoming: DashMap::new(),
        }
    }

    /// Insert a node into the graph and return the Arc-wrapped copy.
    pub async fn insert_node(&self, node: Node) -> Result<Arc<Node>> {
        let id = node.id.clone();
        let arc = Arc::new(node);
        self.nodes.insert(id, Arc::clone(&arc));
        Ok(arc)
    }

    /// Insert an edge into the graph, updating adjacency lists.
    pub async fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        let id = edge.id.clone();
        let from = edge.from_node_id.clone();
        let to = edge.to_node_id.clone();
        let arc = Arc::new(edge);

        self.edges.insert(id, Arc::clone(&arc));

        // Update outgoing adjacency for the source node.
        self.outgoing
            .entry(from)
            .or_default()
            .push(Arc::clone(&arc));

        // Update incoming adjacency for the target node.
        self.incoming
            .entry(to)
            .or_default()
            .push(Arc::clone(&arc));

        Ok(arc)
    }

    /// Retrieve a node by id, returning `None` if it does not exist.
    pub fn get_node(&self, id: &str) -> Result<Option<Arc<Node>>> {
        Ok(self.nodes.get(id).map(|entry| Arc::clone(&entry)))
    }

    /// Retrieve an edge by id, returning `None` if it does not exist.
    pub fn get_edge(&self, id: &str) -> Result<Option<Arc<Edge>>> {
        Ok(self.edges.get(id).map(|entry| Arc::clone(&entry)))
    }

    /// Get all outgoing edges from a node. When `edge_type` is provided, only
    /// edges whose `edge_type` field matches are returned.
    pub fn get_outgoing_edges(
        &self,
        node_id: &str,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        let edges = self
            .outgoing
            .get(node_id)
            .map(|entry| entry.value().clone())
            .unwrap_or_default();

        let filtered = match edge_type {
            Some(et) => edges.into_iter().filter(|e| e.edge_type == et).collect(),
            None => edges,
        };
        Ok(filtered)
    }

    /// Get all incoming edges to a node. When `edge_type` is provided, only
    /// edges whose `edge_type` field matches are returned.
    pub fn get_incoming_edges(
        &self,
        node_id: &str,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        let edges = self
            .incoming
            .get(node_id)
            .map(|entry| entry.value().clone())
            .unwrap_or_default();

        let filtered = match edge_type {
            Some(et) => edges.into_iter().filter(|e| e.edge_type == et).collect(),
            None => edges,
        };
        Ok(filtered)
    }

    /// Return the total number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Return the total number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

impl Default for TitanGraphEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// TitanEngine -- thin UnifiedStorageEngine wrapper for factory registration
// ---------------------------------------------------------------------------

/// Thin wrapper that implements `UnifiedStorageEngine` so TITAN can be
/// registered through the standard engine factory. All methods return stubs;
/// the real graph work lives in `TitanGraphEngine`.
pub struct TitanEngine {
    #[allow(dead_code)]
    graph: TitanGraphEngine,
}

impl TitanEngine {
    /// Create a new `TitanEngine` wrapping a fresh `TitanGraphEngine`.
    pub fn new() -> Self {
        Self {
            graph: TitanGraphEngine::new(),
        }
    }
}

impl Default for TitanEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// UnifiedStorageEngine implementation (stubs)
// ---------------------------------------------------------------------------

#[async_trait]
impl UnifiedStorageEngine for TitanEngine {
    fn engine_name(&self) -> &'static str {
        "titan"
    }

    fn engine_version(&self) -> &'static str {
        "0.1.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        // TITAN is primarily a GraphEngine; use Sst as the default strategy
        // since there is no dedicated Titan variant in StorageEngineStrategy.
        StorageEngineStrategy::Sst
    }

    fn get_filesystem_factory(&self) -> &FilesystemFactory {
        use std::sync::OnceLock;
        static FACTORY: OnceLock<FilesystemFactory> = OnceLock::new();
        use futures::executor::block_on;

        FACTORY.get_or_init(|| {
            block_on(async {
                FilesystemFactory::create(FilesystemConfig::default())
                    .await
                    .unwrap_or_else(|_| {
                        #[allow(clippy::panic)]
                        {
                            panic!("Failed to create filesystem factory for TITAN engine")
                        }
                    })
            })
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        metrics.insert(
            "engine".to_string(),
            serde_json::Value::String("titan".to_string()),
        );
        metrics.insert(
            "node_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.graph.node_count())),
        );
        metrics.insert(
            "edge_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.graph.edge_count())),
        );
        Ok(metrics)
    }

    async fn vector_by_id(
        &self,
        collection_id: &str,
        base_path: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        let _ = (collection_id, base_path, vector_id);
        Ok(None)
    }

    async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        let _ = ctx;
        Ok(vec![])
    }

    async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
        Ok(FlushResult::default())
    }

    async fn do_compact(&self, _params: &CompactionParameters) -> Result<CompactionResult> {
        Ok(CompactionResult::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{Edge, Node, PropertyValue, property_value::Value};

    fn make_node(id: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec!["TestLabel".to_string()],
            properties: HashMap::from([(
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue(format!("node_{id}"))),
                },
            )]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn make_edge(id: &str, from: &str, to: &str, edge_type: &str) -> Edge {
        Edge {
            id: id.to_string(),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: edge_type.to_string(),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn test_titan_node_count_empty() {
        let engine = TitanGraphEngine::new();
        assert_eq!(engine.node_count(), 0);
    }

    #[test]
    fn test_titan_edge_count_empty() {
        let engine = TitanGraphEngine::new();
        assert_eq!(engine.edge_count(), 0);
    }

    #[test]
    fn test_titan_get_node_missing() {
        let engine = TitanGraphEngine::new();
        let result = engine.get_node("x").expect("get_node should not fail");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_titan_insert_and_get_node() {
        let engine = TitanGraphEngine::new();
        let node = make_node("n1");

        let inserted = engine
            .insert_node(node)
            .await
            .expect("insert_node should succeed");
        assert_eq!(inserted.id, "n1");
        assert_eq!(engine.node_count(), 1);

        let retrieved = engine
            .get_node("n1")
            .expect("get_node should not fail")
            .expect("node should exist");
        assert_eq!(retrieved.id, "n1");
        assert!(Arc::ptr_eq(&inserted, &retrieved));
    }

    #[tokio::test]
    async fn test_titan_insert_and_get_edge() {
        let engine = TitanGraphEngine::new();
        let edge = make_edge("e1", "a", "b", "KNOWS");

        let inserted = engine
            .insert_edge(edge)
            .await
            .expect("insert_edge should succeed");
        assert_eq!(inserted.id, "e1");
        assert_eq!(engine.edge_count(), 1);

        let retrieved = engine
            .get_edge("e1")
            .expect("get_edge should not fail")
            .expect("edge should exist");
        assert!(Arc::ptr_eq(&inserted, &retrieved));
    }

    #[tokio::test]
    async fn test_titan_outgoing_edges() {
        let engine = TitanGraphEngine::new();
        engine
            .insert_edge(make_edge("e1", "a", "b", "KNOWS"))
            .await
            .expect("insert should succeed");
        engine
            .insert_edge(make_edge("e2", "a", "c", "LIKES"))
            .await
            .expect("insert should succeed");

        let all = engine
            .get_outgoing_edges("a", None)
            .expect("outgoing should succeed");
        assert_eq!(all.len(), 2);

        let knows_only = engine
            .get_outgoing_edges("a", Some("KNOWS"))
            .expect("filtered outgoing should succeed");
        assert_eq!(knows_only.len(), 1);
        assert_eq!(knows_only[0].edge_type, "KNOWS");
    }

    #[tokio::test]
    async fn test_titan_incoming_edges() {
        let engine = TitanGraphEngine::new();
        engine
            .insert_edge(make_edge("e1", "a", "b", "KNOWS"))
            .await
            .expect("insert should succeed");
        engine
            .insert_edge(make_edge("e2", "c", "b", "LIKES"))
            .await
            .expect("insert should succeed");

        let all = engine
            .get_incoming_edges("b", None)
            .expect("incoming should succeed");
        assert_eq!(all.len(), 2);

        let likes_only = engine
            .get_incoming_edges("b", Some("LIKES"))
            .expect("filtered incoming should succeed");
        assert_eq!(likes_only.len(), 1);
        assert_eq!(likes_only[0].edge_type, "LIKES");
    }

    #[test]
    fn test_titan_engine_name() {
        let engine = TitanEngine::new();
        assert_eq!(engine.engine_name(), "titan");
    }

    #[test]
    fn test_titan_engine_strategy() {
        let engine = TitanEngine::new();
        assert_eq!(engine.strategy(), StorageEngineStrategy::Sst);
    }
}
