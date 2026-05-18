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

use crate::graph::engines::GraphEngine;
use crate::graph::{Edge, EdgeId, GraphService, Node, NodeId};
use crate::proto::proximadb_v1::{PropertyValue, property_value};
use proximadb_kernel::error::ProximaDBError;

use super::super::traits::{ModelType, StoreCapabilities};
use super::super::transaction::participants::{
    GraphEdge as TransactionGraphEdge, GraphNode as TransactionGraphNode, GraphWriteOperations,
};

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
    /// Shared graph service used by the server/runtime path
    service: Option<Arc<GraphService>>,
    /// Optional default graph identifier for service-backed queries
    default_graph: Option<String>,
    /// Configuration
    config: GraphStoreConfig,
}

impl GraphStore {
    /// Create a new GraphStore with the given configuration
    pub fn new(config: GraphStoreConfig) -> Self {
        Self {
            engine: None,
            service: None,
            default_graph: None,
            config,
        }
    }

    /// Set the underlying graph engine
    pub fn with_engine(mut self, engine: Arc<dyn GraphEngine>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// Set the shared graph service for multi-graph query execution
    pub fn with_service(mut self, service: Arc<GraphService>) -> Self {
        self.service = Some(service);
        self
    }

    /// Set the default graph id used when queries do not specify a graph
    pub fn with_default_graph(mut self, graph_id: impl Into<String>) -> Self {
        self.default_graph = Some(graph_id.into());
        self
    }

    /// Get store capabilities
    pub fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            model_type: ModelType::Graph,
            supports_transactions: true,
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

    /// Get the shared graph service if configured
    pub fn service(&self) -> Option<&Arc<GraphService>> {
        self.service.as_ref()
    }

    /// Get the configuration
    pub fn config(&self) -> &GraphStoreConfig {
        &self.config
    }

    async fn resolve_graph_id(&self) -> Result<String> {
        if let Some(graph_id) = &self.default_graph {
            return Ok(graph_id.clone());
        }

        if let Some(service) = &self.service {
            let graphs = service.list_graphs().await?;
            if let Some(graph_id) = graphs.into_iter().next() {
                return Ok(graph_id);
            }
        }

        Err(ProximaDBError::Config(
            "Graph store has no default graph configured".to_string(),
        ))
    }

    /// Resolve a node through either the shared graph service or a directly attached engine.
    pub async fn fetch_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if let Some(service) = &self.service {
            let graph_id = self.resolve_graph_id().await?;
            return service.get_node(&graph_id, id).await;
        }

        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_node(id)
    }

    /// Resolve neighbors through either the shared graph service or a directly attached engine.
    pub async fn fetch_neighbors(&self, node_id: &NodeId) -> Result<Vec<Arc<Node>>> {
        if let Some(service) = &self.service {
            let graph_id = self.resolve_graph_id().await?;
            return service.get_neighbors(&graph_id, node_id).await;
        }

        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_neighbors(node_id, None)
    }

    /// Fetch nodes by label from the configured graph backend.
    pub async fn fetch_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        if let Some(service) = &self.service {
            let graph_id = self.resolve_graph_id().await?;
            return service.query_nodes_by_label(&graph_id, label).await;
        }

        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_nodes_by_label(label)
    }

    /// Fetch all nodes from the configured graph backend.
    pub async fn fetch_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        if let Some(service) = &self.service {
            let graph_id = self.resolve_graph_id().await?;
            return service
                .query_nodes(
                    &graph_id,
                    crate::proto::proximadb_v1::NodeQuery {
                        graph_id: graph_id.clone(),
                        labels: vec![],
                        filters: vec![],
                        offset: None,
                        limit: None,
                        continuation_token: None,
                    },
                )
                .await;
        }

        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Graph engine not configured".to_string()))?;
        engine.get_all_nodes()
    }

    /// Check if store is operational
    pub fn is_operational(&self) -> bool {
        self.engine.is_some() || self.service.is_some()
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

#[async_trait]
impl GraphWriteOperations for GraphStore {
    async fn create_node(&self, graph_id: &str, node: TransactionGraphNode) -> anyhow::Result<()> {
        let now_ms = current_epoch_ms();
        let graph_node = Node {
            id: node.id,
            labels: vec![node.label],
            properties: string_properties_to_proto(node.properties),
            embedding: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };

        if let Some(service) = &self.service {
            service.create_node(graph_id, graph_node).await?;
        } else {
            self.insert_node(graph_node).await?;
        }

        Ok(())
    }

    async fn create_edge(&self, graph_id: &str, edge: TransactionGraphEdge) -> anyhow::Result<()> {
        let now_ms = current_epoch_ms();
        let graph_edge = Edge {
            id: edge.id,
            from_node_id: edge.source,
            to_node_id: edge.target,
            edge_type: edge.edge_type,
            properties: string_properties_to_proto(edge.properties),
            weight: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };

        if let Some(service) = &self.service {
            service.create_edge(graph_id, graph_edge).await?;
        } else {
            self.insert_edge(graph_edge).await?;
        }

        Ok(())
    }

    async fn delete_node(&self, graph_id: &str, node_id: &str) -> anyhow::Result<()> {
        if let Some(service) = &self.service {
            service.delete_node(graph_id, &node_id.to_string()).await?;
        } else {
            GraphEngine::delete_node(self, &node_id.to_string()).await?;
        }

        Ok(())
    }

    async fn delete_edge(&self, graph_id: &str, edge_id: &str) -> anyhow::Result<()> {
        if let Some(service) = &self.service {
            service.delete_edge(graph_id, &edge_id.to_string()).await?;
        } else {
            GraphEngine::delete_edge(self, &edge_id.to_string()).await?;
        }

        Ok(())
    }
}

fn string_properties_to_proto(
    properties: std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, PropertyValue> {
    properties
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                PropertyValue {
                    value: Some(property_value::Value::StringValue(value)),
                },
            )
        })
        .collect()
}

fn current_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
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
