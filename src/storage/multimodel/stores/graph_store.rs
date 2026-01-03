//! # Graph Store
//!
//! Wraps the ORION graph engine for native graph storage with CSR format.
//!
//! ## Engine: ORION
//!
//! - **CSR (Compressed Sparse Row)** format for efficient adjacency traversal
//! - **Arc-based zero-copy** memory sharing
//! - **DashMap** concurrent access
//! - **WAL persistence** for durability
//! - **1M+ edges/sec** traversal throughput

use async_trait::async_trait;
use std::sync::Arc;

use crate::core::error::ProximaDBError;
use crate::graph::engines::GraphEngine;
use crate::graph::{Edge, EdgeId, Node, NodeId};

use super::super::traits::{ModelType, StoreCapabilities};

// Use the graph engine's Result type
type Result<T> = std::result::Result<T, ProximaDBError>;

/// Configuration for the graph store
#[derive(Debug, Clone)]
pub struct GraphStoreConfig {
    /// Enable WAL persistence
    pub enable_wal: bool,
    /// WAL path (if enabled)
    pub wal_path: Option<String>,
    /// Enable property indexes
    pub enable_property_indexes: bool,
}

impl Default for GraphStoreConfig {
    fn default() -> Self {
        Self {
            enable_wal: true,
            wal_path: None,
            enable_property_indexes: true,
        }
    }
}

/// GraphStore wraps the ORION engine for multi-model integration
///
/// ## Architecture
///
/// ```text
/// ┌─────────────────────────────────────────┐
/// │           GraphStore                     │
/// │  ┌─────────────────────────────────────┐│
/// │  │      ORION Engine                   ││
/// │  │  - CSR adjacency storage            ││
/// │  │  - Arc-based memory pool            ││
/// │  │  - DashMap concurrent access        ││
/// │  └─────────────────────────────────────┘│
/// │              │                           │
/// │    ┌─────────▼───────────────────────┐  │
/// │    │     WAL Persistence             │  │
/// │    │  (Graph operations log)         │  │
/// │    └─────────────────────────────────┘  │
/// └─────────────────────────────────────────┘
/// ```
pub struct GraphStore {
    /// The underlying graph engine (ORION, PULSAR, or QUASAR)
    engine: Option<Arc<dyn GraphEngine>>,
    /// Configuration
    config: GraphStoreConfig,
}

impl GraphStore {
    /// Create a new GraphStore with the given configuration
    pub fn new(config: GraphStoreConfig) -> Self {
        Self {
            engine: None,
            config,
        }
    }

    /// Set the underlying graph engine
    pub fn with_engine(mut self, engine: Arc<dyn GraphEngine>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// Get store capabilities
    pub fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            model_type: ModelType::Graph,
            supports_transactions: false, // Future: add graph transactions
            supports_secondary_indexes: true, // Property indexes
            supports_acid: false,
            supports_streaming: true,
            max_recommended_records: Some(100_000_000), // 100M nodes/edges
            description: "Native graph storage with ORION CSR engine (1M+ edges/sec traversal)"
                .to_string(),
        }
    }

    /// Get the underlying engine
    pub fn engine(&self) -> Option<&Arc<dyn GraphEngine>> {
        self.engine.as_ref()
    }

    /// Get the configuration
    pub fn config(&self) -> &GraphStoreConfig {
        &self.config
    }

    /// Check if store is operational
    pub fn is_operational(&self) -> bool {
        self.engine.is_some()
    }

    /// Get node count (convenience method)
    pub fn node_count(&self) -> usize {
        self.engine
            .as_ref()
            .and_then(|e| e.node_count().ok())
            .unwrap_or(0)
    }

    /// Get edge count (convenience method)
    pub fn edge_count(&self) -> usize {
        self.engine
            .as_ref()
            .and_then(|e| e.edge_count().ok())
            .unwrap_or(0)
    }
}

/// Delegate to the underlying GraphEngine trait
#[async_trait]
impl GraphEngine for GraphStore {
    async fn insert_node(&self, node: Node) -> Result<Arc<Node>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.insert_node(node).await
    }

    fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_node(id)
    }

    async fn update_node(&self, node: Node) -> Result<Arc<Node>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.update_node(node).await
    }

    async fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.delete_node(id).await
    }

    async fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.insert_edge(edge).await
    }

    fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_edge(id)
    }

    async fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.update_edge(edge).await
    }

    async fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.delete_edge(id).await
    }

    fn get_outgoing_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_outgoing_edges(node_id, edge_type)
    }

    fn get_incoming_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_incoming_edges(node_id, edge_type)
    }

    fn get_neighbors(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Node>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_neighbors(node_id, edge_type)
    }

    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_nodes_by_label(label)
    }

    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_all_nodes()
    }

    fn node_count(&self) -> Result<usize> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.node_count()
    }

    fn edge_count(&self) -> Result<usize> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.edge_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_store_config_default() {
        let config = GraphStoreConfig::default();
        assert!(config.enable_wal);
        assert!(config.enable_property_indexes);
    }

    #[test]
    fn test_graph_store_capabilities() {
        let store = GraphStore::new(GraphStoreConfig::default());
        let caps = store.capabilities();

        assert_eq!(caps.model_type, ModelType::Graph);
        assert!(caps.supports_secondary_indexes);
    }
}
