/*
 * Copyright 2025 Vijaykumar Singh
 * (Apache-2.0)
 */

//! ORION in-memory node/edge/index store. Moved out of the root `graph` module
//! so the ORION engine owns its memory pool (ORION extraction, 6g). The root
//! re-exports it via `pub use proximadb_orion_engine::GraphMemoryPool`.

use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::RwLock as ParkingRwLock;
use proximadb_graph_model::{Edge, EdgeId, Node, NodeId, PropertyValue};
use proximadb_kernel::error::ProximaDBError;

type Result<T> = std::result::Result<T, ProximaDBError>;

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
                Some(proximadb_graph_model::property_value::Value::StringValue(_))
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
                Some(proximadb_graph_model::property_value::Value::IntValue(i)) => Some(*i),
                Some(proximadb_graph_model::property_value::Value::DoubleValue(d)) => {
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
                Some(proximadb_graph_model::property_value::Value::StringValue(_))
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
                Some(proximadb_graph_model::property_value::Value::IntValue(i)) => Some(*i),
                Some(proximadb_graph_model::property_value::Value::DoubleValue(d)) => {
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
                Some(proximadb_graph_model::property_value::Value::StringValue(_))
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
                Some(proximadb_graph_model::property_value::Value::IntValue(i)) => Some(*i),
                Some(proximadb_graph_model::property_value::Value::DoubleValue(d)) => {
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
                Some(proximadb_graph_model::property_value::Value::StringValue(_))
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
                Some(proximadb_graph_model::property_value::Value::IntValue(i)) => Some(*i),
                Some(proximadb_graph_model::property_value::Value::DoubleValue(d)) => {
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

/// Convert PropertyValue to string for indexing (moved from root graph module).
pub fn property_value_to_string(value: &PropertyValue) -> String {
    match &value.value {
        Some(proximadb_graph_model::property_value::Value::StringValue(s)) => s.clone(),
        Some(proximadb_graph_model::property_value::Value::IntValue(i)) => i.to_string(),
        Some(proximadb_graph_model::property_value::Value::DoubleValue(d)) => d.to_string(),
        Some(proximadb_graph_model::property_value::Value::BoolValue(b)) => b.to_string(),
        Some(proximadb_graph_model::property_value::Value::BytesValue(b)) => {
            format!("bytes:{}", b.len())
        }
        Some(proximadb_graph_model::property_value::Value::ArrayValue(_)) => "array".to_string(),
        Some(proximadb_graph_model::property_value::Value::ObjectValue(_)) => "object".to_string(),
        Some(proximadb_graph_model::property_value::Value::VectorValue(_)) => "vector".to_string(),
        None => "null".to_string(),
    }
}
