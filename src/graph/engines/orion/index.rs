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

//! # ORION Engine Indexing Support
//!
//! This module provides indexing capabilities for the ORION engine to accelerate
//! property-based queries and label-based filtering.
//!
//! ## Index Types
//!
//! - **Label Indexes**: Fast lookup of nodes by label
//! - **Property Indexes**: B-tree indexes on node/edge properties
//! - **Composite Indexes**: Multi-property indexes for complex queries
//! - **Full-Text Indexes**: Text search on string properties (future)

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::{Edge, EdgeId, Node, NodeId, PropertyValue};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Property index for efficient property-based queries
#[derive(Debug)]
pub struct PropertyIndex {
    /// Property key this index covers
    pub property_key: String,

    /// B-tree index mapping property values to node/edge IDs
    pub btree_index: BTreeMap<String, HashSet<String>>,

    /// Statistics about this index
    pub stats: IndexStats,
}

/// Label index for efficient label-based queries
#[derive(Debug)]
pub struct LabelIndex {
    /// Mapping from label to node IDs
    pub label_to_nodes: HashMap<String, HashSet<NodeId>>,

    /// Reverse mapping from node ID to labels
    pub node_to_labels: HashMap<NodeId, HashSet<String>>,

    /// Statistics
    pub stats: IndexStats,
}

/// Edge type index for efficient edge type queries
#[derive(Debug)]
pub struct EdgeTypeIndex {
    /// Mapping from edge type to edge IDs
    pub type_to_edges: HashMap<String, HashSet<EdgeId>>,

    /// Statistics
    pub stats: IndexStats,
}

/// Index statistics
#[derive(Debug, Default)]
pub struct IndexStats {
    pub total_entries: usize,
    pub unique_values: usize,
    pub memory_usage_bytes: usize,
    pub last_updated: Option<std::time::SystemTime>,
}

/// Composite index on multiple properties
#[derive(Debug)]
pub struct CompositeIndex {
    /// Property keys in this composite index
    pub property_keys: Vec<String>,

    /// Composite key mapping to node/edge IDs
    pub composite_index: BTreeMap<Vec<String>, HashSet<String>>,

    /// Statistics
    pub stats: IndexStats,
}

/// Index manager for ORION engine
#[derive(Debug)]
pub struct IndexManager {
    /// Node label indexes
    pub node_label_index: LabelIndex,

    /// Edge type indexes
    pub edge_type_index: EdgeTypeIndex,

    /// Node property indexes (property_key -> index)
    pub node_property_indexes: HashMap<String, PropertyIndex>,

    /// Edge property indexes (property_key -> index)
    pub edge_property_indexes: HashMap<String, PropertyIndex>,

    /// Composite indexes
    pub composite_indexes: HashMap<String, CompositeIndex>,
}

impl PropertyIndex {
    /// Create a new property index
    pub fn new(property_key: String) -> Self {
        Self {
            property_key,
            btree_index: BTreeMap::new(),
            stats: IndexStats::default(),
        }
    }

    /// Add an entry to the index
    pub fn add_entry(&mut self, property_value: &PropertyValue, entity_id: String) -> Result<()> {
        let value_str = property_value_to_string(property_value);

        self.btree_index
            .entry(value_str)
            .or_default()
            .insert(entity_id);

        self.update_stats();
        Ok(())
    }

    /// Remove an entry from the index
    pub fn remove_entry(&mut self, property_value: &PropertyValue, entity_id: &str) -> Result<()> {
        let value_str = property_value_to_string(property_value);

        if let Some(ids) = self.btree_index.get_mut(&value_str) {
            ids.remove(entity_id);
            if ids.is_empty() {
                self.btree_index.remove(&value_str);
            }
        }

        self.update_stats();
        Ok(())
    }

    /// Query the index for exact matches
    pub fn query_exact(&self, property_value: &PropertyValue) -> Result<Vec<String>> {
        let value_str = property_value_to_string(property_value);

        Ok(self
            .btree_index
            .get(&value_str)
            .map(|ids| ids.iter().cloned().collect())
            .unwrap_or_else(Vec::new))
    }

    /// Query the index for range queries (for comparable types)
    pub fn query_range(
        &self,
        min_value: &PropertyValue,
        max_value: &PropertyValue,
    ) -> Result<Vec<String>> {
        let min_str = property_value_to_string(min_value);
        let max_str = property_value_to_string(max_value);

        let mut results = Vec::new();

        for (_value, ids) in self.btree_index.range(min_str..=max_str) {
            results.extend(ids.iter().cloned());
        }

        Ok(results)
    }

    /// Query with prefix matching (for string properties)
    pub fn query_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let mut results = Vec::new();

        // Find all values that start with the prefix
        for (value, ids) in &self.btree_index {
            if value.starts_with(prefix) {
                results.extend(ids.iter().cloned());
            }
        }

        Ok(results)
    }

    /// Update index statistics
    fn update_stats(&mut self) {
        self.stats.total_entries = self.btree_index.values().map(|set| set.len()).sum();
        self.stats.unique_values = self.btree_index.len();
        self.stats.memory_usage_bytes = self.estimate_memory_usage();
        self.stats.last_updated = Some(std::time::SystemTime::now());
    }

    /// Estimate memory usage of the index
    fn estimate_memory_usage(&self) -> usize {
        let btree_size = self.btree_index.len() * std::mem::size_of::<(String, HashSet<String>)>();
        let keys_size: usize = self.btree_index.keys().map(|k| k.len()).sum();
        let values_size: usize = self
            .btree_index
            .values()
            .map(|set| set.iter().map(|s| s.len()).sum::<usize>())
            .sum();

        btree_size + keys_size + values_size
    }
}

impl LabelIndex {
    /// Create a new label index
    pub fn new() -> Self {
        Self {
            label_to_nodes: HashMap::new(),
            node_to_labels: HashMap::new(),
            stats: IndexStats::default(),
        }
    }

    /// Add a node with its labels
    pub fn add_node(&mut self, node_id: NodeId, labels: &[String]) -> Result<()> {
        // Update label -> nodes mapping
        for label in labels {
            self.label_to_nodes
                .entry(label.clone())
                .or_default()
                .insert(node_id.clone());
        }

        // Update node -> labels mapping
        self.node_to_labels
            .insert(node_id, labels.iter().cloned().collect());

        self.update_stats();
        Ok(())
    }

    /// Remove a node and its labels
    pub fn remove_node(&mut self, node_id: &NodeId) -> Result<()> {
        if let Some(labels) = self.node_to_labels.remove(node_id) {
            for label in labels {
                if let Some(nodes) = self.label_to_nodes.get_mut(&label) {
                    nodes.remove(node_id);
                    if nodes.is_empty() {
                        self.label_to_nodes.remove(&label);
                    }
                }
            }
        }

        self.update_stats();
        Ok(())
    }

    /// Query nodes by label
    pub fn query_by_label(&self, label: &str) -> Result<Vec<NodeId>> {
        Ok(self
            .label_to_nodes
            .get(label)
            .map(|nodes| nodes.iter().cloned().collect())
            .unwrap_or_else(Vec::new))
    }

    /// Query nodes having all specified labels (AND operation)
    pub fn query_by_labels_and(&self, labels: &[String]) -> Result<Vec<NodeId>> {
        if labels.is_empty() {
            return Ok(Vec::new());
        }

        // Start with first label
        let mut result: HashSet<NodeId> = self
            .label_to_nodes
            .get(&labels[0])
            .cloned()
            .unwrap_or_else(HashSet::new);

        // Intersect with remaining labels
        for label in labels.iter().skip(1) {
            if let Some(nodes) = self.label_to_nodes.get(label) {
                result = result.intersection(nodes).cloned().collect();
            } else {
                return Ok(Vec::new()); // No nodes have this label
            }
        }

        Ok(result.into_iter().collect())
    }

    /// Query nodes having any of the specified labels (OR operation)
    pub fn query_by_labels_or(&self, labels: &[String]) -> Result<Vec<NodeId>> {
        let mut result = HashSet::new();

        for label in labels {
            if let Some(nodes) = self.label_to_nodes.get(label) {
                result.extend(nodes.iter().cloned());
            }
        }

        Ok(result.into_iter().collect())
    }

    /// Get all labels for a node
    pub fn get_node_labels(&self, node_id: &NodeId) -> Option<&HashSet<String>> {
        self.node_to_labels.get(node_id)
    }

    /// Update statistics
    fn update_stats(&mut self) {
        self.stats.total_entries = self.node_to_labels.len();
        self.stats.unique_values = self.label_to_nodes.len();
        self.stats.memory_usage_bytes = self.estimate_memory_usage();
        self.stats.last_updated = Some(std::time::SystemTime::now());
    }

    /// Estimate memory usage
    fn estimate_memory_usage(&self) -> usize {
        let label_to_nodes_size: usize = self
            .label_to_nodes
            .iter()
            .map(|(label, nodes)| label.len() + nodes.len() * std::mem::size_of::<NodeId>())
            .sum();

        let node_to_labels_size: usize = self
            .node_to_labels
            .iter()
            .map(|(node_id, labels)| node_id.len() + labels.iter().map(|l| l.len()).sum::<usize>())
            .sum();

        label_to_nodes_size + node_to_labels_size
    }
}

impl EdgeTypeIndex {
    /// Create a new edge type index
    pub fn new() -> Self {
        Self {
            type_to_edges: HashMap::new(),
            stats: IndexStats::default(),
        }
    }

    /// Add an edge
    pub fn add_edge(&mut self, edge_id: EdgeId, edge_type: &str) -> Result<()> {
        self.type_to_edges
            .entry(edge_type.to_string())
            .or_default()
            .insert(edge_id);

        self.update_stats();
        Ok(())
    }

    /// Remove an edge
    pub fn remove_edge(&mut self, edge_id: &EdgeId, edge_type: &str) -> Result<()> {
        if let Some(edges) = self.type_to_edges.get_mut(edge_type) {
            edges.remove(edge_id);
            if edges.is_empty() {
                self.type_to_edges.remove(edge_type);
            }
        }

        self.update_stats();
        Ok(())
    }

    /// Query edges by type
    pub fn query_by_type(&self, edge_type: &str) -> Result<Vec<EdgeId>> {
        Ok(self
            .type_to_edges
            .get(edge_type)
            .map(|edges| edges.iter().cloned().collect())
            .unwrap_or_else(Vec::new))
    }

    /// Update statistics
    fn update_stats(&mut self) {
        self.stats.total_entries = self.type_to_edges.values().map(|set| set.len()).sum();
        self.stats.unique_values = self.type_to_edges.len();
        self.stats.memory_usage_bytes = self.estimate_memory_usage();
        self.stats.last_updated = Some(std::time::SystemTime::now());
    }

    /// Estimate memory usage
    fn estimate_memory_usage(&self) -> usize {
        self.type_to_edges
            .iter()
            .map(|(edge_type, edges)| {
                edge_type.len() + edges.iter().map(|e| e.len()).sum::<usize>()
            })
            .sum()
    }
}

impl IndexManager {
    /// Create a new index manager
    pub fn new() -> Self {
        Self {
            node_label_index: LabelIndex::new(),
            edge_type_index: EdgeTypeIndex::new(),
            node_property_indexes: HashMap::new(),
            edge_property_indexes: HashMap::new(),
            composite_indexes: HashMap::new(),
        }
    }

    /// Create a property index for nodes
    pub fn create_node_property_index(&mut self, property_key: String) -> Result<()> {
        if self.node_property_indexes.contains_key(&property_key) {
            return Err(ProximaDBError::InvalidInput(format!(
                "Property index for '{}' already exists",
                property_key
            )));
        }

        self.node_property_indexes
            .insert(property_key.clone(), PropertyIndex::new(property_key));

        Ok(())
    }

    /// Create a property index for edges
    pub fn create_edge_property_index(&mut self, property_key: String) -> Result<()> {
        if self.edge_property_indexes.contains_key(&property_key) {
            return Err(ProximaDBError::InvalidInput(format!(
                "Edge property index for '{}' already exists",
                property_key
            )));
        }

        self.edge_property_indexes
            .insert(property_key.clone(), PropertyIndex::new(property_key));

        Ok(())
    }

    /// Add a node to all relevant indexes
    pub fn index_node(&mut self, node: &Node) -> Result<()> {
        // Add to label index
        self.node_label_index
            .add_node(node.id.clone(), &node.labels)?;

        // Add to property indexes
        for (property_key, property_value) in &node.properties {
            if let Some(index) = self.node_property_indexes.get_mut(property_key) {
                index.add_entry(property_value, node.id.clone())?;
            }
        }

        Ok(())
    }

    /// Add an edge to all relevant indexes
    pub fn index_edge(&mut self, edge: &Edge) -> Result<()> {
        // Add to edge type index
        self.edge_type_index
            .add_edge(edge.id.clone(), &edge.edge_type)?;

        // Add to property indexes
        for (property_key, property_value) in &edge.properties {
            if let Some(index) = self.edge_property_indexes.get_mut(property_key) {
                index.add_entry(property_value, edge.id.clone())?;
            }
        }

        Ok(())
    }

    /// Remove a node from all indexes
    pub fn remove_node_from_indexes(&mut self, node: &Node) -> Result<()> {
        // Remove from label index
        self.node_label_index.remove_node(&node.id)?;

        // Remove from property indexes
        for (property_key, property_value) in &node.properties {
            if let Some(index) = self.node_property_indexes.get_mut(property_key) {
                index.remove_entry(property_value, &node.id)?;
            }
        }

        Ok(())
    }

    /// Remove an edge from all indexes
    pub fn remove_edge_from_indexes(&mut self, edge: &Edge) -> Result<()> {
        // Remove from edge type index
        self.edge_type_index
            .remove_edge(&edge.id, &edge.edge_type)?;

        // Remove from property indexes
        for (property_key, property_value) in &edge.properties {
            if let Some(index) = self.edge_property_indexes.get_mut(property_key) {
                index.remove_entry(property_value, &edge.id)?;
            }
        }

        Ok(())
    }
}

impl Default for LabelIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for EdgeTypeIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for IndexManager {
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
    fn test_property_index() {
        let mut index = PropertyIndex::new("name".to_string());

        let value = PropertyValue {
            value: Some(
                crate::proto::proximadb_v1::property_value::Value::StringValue("Alice".to_string()),
            ),
        };

        // Add entry
        index
            .add_entry(&value, "node1".to_string())
            .expect("Failed to add entry to property index");

        // Query exact
        let results = index
            .query_exact(&value)
            .expect("Failed to query exact match in property index");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "node1");

        // Remove entry
        index
            .remove_entry(&value, "node1")
            .expect("Failed to remove entry from property index");
        let results = index
            .query_exact(&value)
            .expect("Failed to query exact match after removal");
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_label_index() {
        let mut index = LabelIndex::new();

        // Add node with labels
        let labels = vec!["Person".to_string(), "Employee".to_string()];
        index
            .add_node("node1".to_string(), &labels)
            .expect("Failed to add node to label index");

        // Query by single label
        let results = index
            .query_by_label("Person")
            .expect("Failed to query label index by label");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "node1");

        // Query by multiple labels (AND)
        let results = index
            .query_by_labels_and(&labels)
            .expect("Failed to query label index with AND");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "node1");

        // Query by labels (OR)
        let results = index
            .query_by_labels_or(&["Person".to_string()])
            .expect("Failed to query label index with OR");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "node1");
    }

    #[test]
    fn test_edge_type_index() {
        let mut index = EdgeTypeIndex::new();

        // Add edge
        index
            .add_edge("edge1".to_string(), "KNOWS")
            .expect("Failed to add edge to edge type index");

        // Query by type
        let results = index
            .query_by_type("KNOWS")
            .expect("Failed to query edge type index");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "edge1");

        // Remove edge
        index
            .remove_edge(&"edge1".to_string(), "KNOWS")
            .expect("Failed to remove edge from edge type index");
        let results = index
            .query_by_type("KNOWS")
            .expect("Failed to query edge type index after removal");
        assert_eq!(results.len(), 0);
    }
}
