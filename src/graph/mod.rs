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

//! # ProximaDB Native Graph Database Engine
//!
//! This module implements ProximaDB's native graph database capabilities over
//! canonical `ProximaRecord` node and edge records. Protocol-specific graph
//! types are compatibility edges; durable graph truth is the shared record
//! envelope defined by the convergence design.
//!
//! ## Design Principles
//!
//! - **Record-First**: Nodes and edges map to canonical `ProximaRecord`
//! - **Arc-Based Sharing**: Zero-copy memory sharing between vector and graph engines
//! - **CSR Projection**: Compressed Sparse Row is a rebuildable topology projection
//! - **Modular Engines**: ORION (in-memory), PULSAR (distributed), QUASAR (hybrid)
//!
//! ## Performance Characteristics
//!
//! - **Traversal**: 1M+ edges/second
//! - **Node Lookup**: < 1μs
//! - **Memory Overhead**: < 100 bytes/node
//! - **Arc Clone**: ~8 bytes (pointer copy)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │            GraphService             │
//! │        (Business Logic Layer)       │
//! ├─────────────────────────────────────┤
//! │              Engines                │
//! │  ┌─────────┬─────────┬───────────┐  │
//! │  │  ORION  │ PULSAR  │  QUASAR   │  │
//! │  │(Memory) │(Distrib)│ (Hybrid)  │  │
//! │  └─────────┴─────────┴───────────┘  │
//! ├─────────────────────────────────────┤
//! │           Arc Memory Pool           │
//! │    ┌────────────┬─────────────┐     │
//! │    │ Nodes/Edges│ Adj/CSR     │     │
//! │    │ Records    │ Projections │     │
//! │    └────────────┴─────────────┘     │
//! └─────────────────────────────────────┘
//! ```

pub mod adjacency_projection;
pub mod canonical;
pub mod engines;
pub mod merge;
pub mod rag;
// Generic, engine-agnostic traversal utilities
pub use engines::generic_traversal;
pub mod hybrid;
pub mod monitoring;
pub mod query;
pub mod service;
pub mod service_algorithms;

// Re-export public types
pub use engines::orion::OrionGraphEngine;
#[cfg(feature = "distributed-graph")]
pub use engines::pulsar::PulsarGraphEngine;
#[cfg(feature = "tiered-graph")]
pub use engines::quasar::QuasarGraphEngine;
pub use engines::{
    EmbeddingMode, EngineCapabilities, GraphEngineConfig, GraphEngineFactory, GraphEngineType,
};
pub use hybrid::HybridQueryEngine;
pub use monitoring::GraphMonitor;
pub use query::{PatternMatcher, QueryPlanner};
pub use service::GraphOperationsService;
// Backward compatibility alias
pub use service::GraphOperationsService as GraphService;
// Transaction support types
pub use service::{
    IsolationLevel, TransactionHandle, TransactionId, TransactionManager, TransactionState,
    UnitOfWork,
};

// Algorithm types for high-level API
pub use service_algorithms::{
    CentralityAlgorithm, CentralityConfig, CentralityResult, CommunityAlgorithm, CommunityConfig,
    CommunityResult,
};

// Canonical types for REST/gRPC parity
pub use canonical::{
    BatchError, BatchResults, CanonicalEdge, CanonicalEmbedding, CanonicalNode, CanonicalPath,
    ErrorCode, GraphError, GraphResponse, QueryResults, ResponseMetadata, ShortestPathResult,
    TraversalResults, TraversalStats as CanonicalTraversalStats,
};

// Export proto types for convenience
pub use crate::proto::proximadb_v1::{
    BatchEdgeRequest, BatchNodeRequest, BatchResponse, Edge, EdgeQuery, EdgeTypeStats, GraphPath,
    GraphStats, LabelStats, Node, NodeQuery, PropertyArray, PropertyFilter, PropertyFilterOperator,
    PropertyObject, PropertyValue, TraversalAlgorithm, TraversalRequest, TraversalResponse,
    TraversalStats, property_value::Value,
};

use dashmap::DashMap;
use parking_lot::RwLock as ParkingRwLock;
use proximadb_kernel::error::ProximaDBError;
use std::sync::Arc;
type Result<T> = std::result::Result<T, ProximaDBError>;

/// Node ID type alias for clarity
pub type NodeId = String;

/// Edge ID type alias for clarity  
pub type EdgeId = String;

/// Graph operation mode for flexible deployment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationMode {
    /// Graph operations only
    GraphOnly,
    /// Vector operations only (graph operations return errors)
    VectorOnly,
    /// Both graph and vector operations available
    Unified,
}

/// Shared memory pool for Arc-based zero-copy architecture
#[derive(Debug)]
pub struct GraphMemoryPool {
    /// Node storage with Arc for zero-copy sharing
    pub nodes: DashMap<NodeId, Arc<Node>>,

    /// Edge storage with Arc for zero-copy sharing
    pub edges: DashMap<EdgeId, Arc<Edge>>,

    /// Property indexes for efficient querying
    pub node_property_indexes: DashMap<String, DashMap<String, Vec<NodeId>>>,
    /// Ordered string indexes for node properties (for range/prefix queries)
    pub node_property_str_ordered:
        DashMap<String, ParkingRwLock<std::collections::BTreeMap<String, Vec<NodeId>>>>,
    /// Numeric indexes for node properties (for numeric range queries)
    pub node_property_num_indexes:
        DashMap<String, ParkingRwLock<std::collections::HashMap<i64, Vec<NodeId>>>>,
    /// Property indexes for edge-level properties, keyed by property name.
    pub edge_property_indexes: DashMap<String, DashMap<String, Vec<EdgeId>>>,
    /// Ordered string indexes for edge properties (for range/prefix queries)
    pub edge_property_str_ordered:
        DashMap<String, ParkingRwLock<std::collections::BTreeMap<String, Vec<EdgeId>>>>,
    /// Numeric indexes for edge properties (for numeric range queries)
    pub edge_property_num_indexes:
        DashMap<String, ParkingRwLock<std::collections::HashMap<i64, Vec<EdgeId>>>>,

    /// Label indexes for fast label-based queries
    pub label_indexes: DashMap<String, Vec<NodeId>>,
    /// Edge type indexes mapping edge type strings to edge identifiers.
    pub edge_type_indexes: DashMap<String, Vec<EdgeId>>,

    /// Composite (from,to,type) edge index for uniqueness checks
    pub edge_composite_index: DashMap<(NodeId, NodeId, String), EdgeId>,

    /// Unique constraints registry: (graph_id, label, property) -> (value -> node_id)
    pub unique_constraints: DashMap<(String, String, String), DashMap<String, NodeId>>,
    /// Multi-property unique constraints per graph:
    /// Key: (graph_id, labels_key, props_key) where labels_key and props_key are joined, normalized strings
    /// Value: composite_key -> node_id
    pub unique_constraints_multi: DashMap<(String, String, String), DashMap<String, NodeId>>,
}

impl GraphMemoryPool {
    /// Create a new graph memory pool
    pub fn new() -> Self {
        Self {
            nodes: DashMap::new(),
            edges: DashMap::new(),
            node_property_indexes: DashMap::new(),
            node_property_str_ordered: DashMap::new(),
            node_property_num_indexes: DashMap::new(),
            edge_property_indexes: DashMap::new(),
            edge_property_str_ordered: DashMap::new(),
            edge_property_num_indexes: DashMap::new(),
            label_indexes: DashMap::new(),
            edge_type_indexes: DashMap::new(),
            edge_composite_index: DashMap::new(),
            unique_constraints: DashMap::new(),
            unique_constraints_multi: DashMap::new(),
        }
    }

    /// Get node count
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get edge count
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Get a node by ID (returns Arc for zero-copy)
    pub fn get_node(&self, id: &NodeId) -> Option<Arc<Node>> {
        self.nodes.get(id).map(|entry| Arc::clone(&entry))
    }

    /// Get an edge by ID (returns Arc for zero-copy)
    pub fn get_edge(&self, id: &EdgeId) -> Option<Arc<Edge>> {
        self.edges.get(id).map(|entry| Arc::clone(&entry))
    }

    /// Insert a node (stored as Arc for sharing)
    pub fn insert_node(&self, node: Node) -> Arc<Node> {
        let node_id = node.id.clone();
        let node_arc = Arc::new(node);
        self.nodes.insert(node_id, Arc::clone(&node_arc));

        // Update indexes
        let _ = self.update_node_indexes(&node_arc);

        node_arc
    }

    /// Insert an edge (stored as Arc for sharing)
    pub fn insert_edge(&self, edge: Edge) -> Arc<Edge> {
        let edge_id = edge.id.clone();
        let edge_arc = Arc::new(edge);
        self.edges.insert(edge_id, Arc::clone(&edge_arc));

        // Update indexes
        let _ = self.update_edge_indexes(&edge_arc);

        edge_arc
    }

    /// Remove a node
    pub fn remove_node(&self, id: &NodeId) -> Option<Arc<Node>> {
        if let Some((_, node)) = self.nodes.remove(id) {
            let _ = self.remove_node_indexes(&node);
            Some(node)
        } else {
            None
        }
    }

    /// Remove an edge
    pub fn remove_edge(&self, id: &EdgeId) -> Option<Arc<Edge>> {
        if let Some((_, edge)) = self.edges.remove(id) {
            let _ = self.remove_edge_indexes(&edge);
            Some(edge)
        } else {
            None
        }
    }

    /// Update node indexes
    fn update_node_indexes(&self, node: &Arc<Node>) -> Result<()> {
        // Update label indexes
        for label in &node.labels {
            self.label_indexes
                .entry(label.clone())
                .or_default()
                .push(node.id.clone());
        }

        // Update property indexes
        for (key, value) in &node.properties {
            let value_str = property_value_to_string(value);
            self.node_property_indexes
                .entry(key.clone())
                .or_default()
                .entry(value_str)
                .or_default()
                .push(node.id.clone());

            // Ordered string index
            if matches!(
                value.value,
                Some(crate::proto::proximadb_v1::property_value::Value::StringValue(_))
            ) {
                let map_lock = self
                    .node_property_str_ordered
                    .entry(key.clone())
                    .or_insert_with(|| ParkingRwLock::new(std::collections::BTreeMap::new()));
                let mut map = map_lock.write();
                map.entry(property_value_to_string(value))
                    .or_default()
                    .push(node.id.clone());
            }

            // Numeric index (convert doubles to i64 for indexing)
            if let Some(num) = match &value.value {
                Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => Some(*i),
                Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                    Some(*d as i64)
                }
                _ => None,
            } {
                let map_lock = self
                    .node_property_num_indexes
                    .entry(key.clone())
                    .or_insert_with(|| ParkingRwLock::new(std::collections::HashMap::new()));
                let mut map = map_lock.write();
                map.entry(num).or_default().push(node.id.clone());
            }
        }
        Ok(())
    }

    /// Update edge indexes
    fn update_edge_indexes(&self, edge: &Arc<Edge>) -> Result<()> {
        // Update edge type indexes
        self.edge_type_indexes
            .entry(edge.edge_type.clone())
            .or_default()
            .push(edge.id.clone());

        // Update property indexes
        for (key, value) in &edge.properties {
            let value_str = property_value_to_string(value);
            self.edge_property_indexes
                .entry(key.clone())
                .or_default()
                .entry(value_str)
                .or_default()
                .push(edge.id.clone());

            // Ordered string index (for string values)
            if matches!(
                value.value,
                Some(crate::proto::proximadb_v1::property_value::Value::StringValue(_))
            ) {
                let map_lock = self
                    .edge_property_str_ordered
                    .entry(key.clone())
                    .or_insert_with(|| ParkingRwLock::new(std::collections::BTreeMap::new()));
                let mut map = map_lock.write();
                map.entry(property_value_to_string(value))
                    .or_default()
                    .push(edge.id.clone());
            }

            // Ordered numeric index (for int/double)
            if let Some(num) = match &value.value {
                Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => Some(*i),
                Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                    Some(*d as i64)
                }
                _ => None,
            } {
                let map_lock = self
                    .edge_property_num_indexes
                    .entry(key.clone())
                    .or_insert_with(|| ParkingRwLock::new(std::collections::HashMap::new()));
                let mut map = map_lock.write();
                map.entry(num).or_default().push(edge.id.clone());
            }
        }

        // Update composite uniqueness index
        self.edge_composite_index.insert(
            (
                edge.from_node_id.clone(),
                edge.to_node_id.clone(),
                edge.edge_type.clone(),
            ),
            edge.id.clone(),
        );
        Ok(())
    }

    /// Remove node from indexes
    fn remove_node_indexes(&self, node: &Arc<Node>) -> Result<()> {
        // Remove from label indexes
        for label in &node.labels {
            if let Some(mut entry) = self.label_indexes.get_mut(label) {
                entry.retain(|id| id != &node.id);
            }
        }

        // Remove from property indexes
        for (key, value) in &node.properties {
            let value_str = property_value_to_string(value);
            if let Some(prop_map) = self.node_property_indexes.get_mut(key)
                && let Some(mut ids) = prop_map.get_mut(&value_str)
            {
                ids.retain(|id| id != &node.id);
            }

            // Remove from ordered string index
            if matches!(
                value.value,
                Some(crate::proto::proximadb_v1::property_value::Value::StringValue(_))
            ) && let Some(map_lock) = self.node_property_str_ordered.get(key)
            {
                let mut map = map_lock.write();
                if let Some(ids) = map.get_mut(&property_value_to_string(value)) {
                    ids.retain(|id| id != &node.id);
                    if ids.is_empty() {
                        map.remove(&property_value_to_string(value));
                    }
                }
            }

            // Remove from ordered numeric index
            if let Some(num) = match &value.value {
                Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => Some(*i),
                Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                    Some(*d as i64)
                }
                _ => None,
            } && let Some(map_lock) = self.node_property_num_indexes.get(key)
            {
                let mut map = map_lock.write();
                if let Some(ids) = map.get_mut(&num) {
                    ids.retain(|id| id != &node.id);
                    if ids.is_empty() {
                        map.remove(&num);
                    }
                }
            }
        }
        Ok(())
    }

    /// Remove edge from indexes
    fn remove_edge_indexes(&self, edge: &Arc<Edge>) -> Result<()> {
        // Remove from edge type indexes
        if let Some(mut entry) = self.edge_type_indexes.get_mut(&edge.edge_type) {
            entry.retain(|id| id != &edge.id);
        }

        // Remove from property indexes
        for (key, value) in &edge.properties {
            let value_str = property_value_to_string(value);
            if let Some(prop_map) = self.edge_property_indexes.get_mut(key)
                && let Some(mut ids) = prop_map.get_mut(&value_str)
            {
                ids.retain(|id| id != &edge.id);
            }

            // Remove from ordered string index
            if matches!(
                value.value,
                Some(crate::proto::proximadb_v1::property_value::Value::StringValue(_))
            ) && let Some(map_lock) = self.edge_property_str_ordered.get(key)
            {
                let mut map = map_lock.write();
                if let Some(ids) = map.get_mut(&property_value_to_string(value)) {
                    ids.retain(|id| id != &edge.id);
                    if ids.is_empty() {
                        map.remove(&property_value_to_string(value));
                    }
                }
            }

            // Remove from ordered numeric index
            if let Some(num) = match &value.value {
                Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => Some(*i),
                Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                    Some(*d as i64)
                }
                _ => None,
            } && let Some(map_lock) = self.edge_property_num_indexes.get(key)
            {
                let mut map = map_lock.write();
                if let Some(ids) = map.get_mut(&num) {
                    ids.retain(|id| id != &edge.id);
                    if ids.is_empty() {
                        map.remove(&num);
                    }
                }
            }
        }

        // Remove from composite index
        self.edge_composite_index.remove(&(
            edge.from_node_id.clone(),
            edge.to_node_id.clone(),
            edge.edge_type.clone(),
        ));
        Ok(())
    }
}

impl Default for GraphMemoryPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert PropertyValue to string for indexing
fn property_value_to_string(value: &PropertyValue) -> String {
    match &value.value {
        Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => s.clone(),
        Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => i.to_string(),
        Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => d.to_string(),
        Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => b.to_string(),
        Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(b)) => {
            format!("bytes:{}", b.len())
        }
        Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(_)) => {
            "array".to_string()
        }
        Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_)) => {
            "object".to_string()
        }
        Some(crate::proto::proximadb_v1::property_value::Value::VectorValue(_)) => {
            "vector".to_string()
        }
        None => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // PropertyValue is now a struct, not enum - use direct field access;

    #[test]
    fn test_memory_pool_creation() {
        let pool = GraphMemoryPool::new();
        assert_eq!(pool.node_count(), 0);
        assert_eq!(pool.edge_count(), 0);
    }

    #[test]
    fn test_node_operations() {
        let pool = GraphMemoryPool::new();

        // Create a test node
        let node = Node {
            id: "node1".to_string(),
            labels: vec!["Person".to_string()],
            properties: std::collections::HashMap::from([(
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue("Alice".to_string())),
                },
            )]),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        };

        // Insert node
        let node_arc = pool.insert_node(node);
        assert_eq!(pool.node_count(), 1);

        // Get node
        let retrieved = pool.get_node(&"node1".to_string()).unwrap();
        assert_eq!(retrieved.id, "node1");
        assert_eq!(retrieved.labels[0], "Person");

        // Verify Arc sharing (same pointer)
        assert!(Arc::ptr_eq(&node_arc, &retrieved));

        // Remove node
        let removed = pool.remove_node(&"node1".to_string()).unwrap();
        assert_eq!(removed.id, "node1");
        assert_eq!(pool.node_count(), 0);
    }

    #[test]
    fn test_property_value_to_string() {
        let string_val = PropertyValue {
            value: Some(Value::StringValue("test".to_string())),
        };
        assert_eq!(property_value_to_string(&string_val), "test");

        let int_val = PropertyValue {
            value: Some(Value::IntValue(42)),
        };
        assert_eq!(property_value_to_string(&int_val), "42");

        let bool_val = PropertyValue {
            value: Some(Value::BoolValue(true)),
        };
        assert_eq!(property_value_to_string(&bool_val), "true");
    }
}
