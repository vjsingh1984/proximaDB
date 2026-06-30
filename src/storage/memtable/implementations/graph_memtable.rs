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

//! # Graph-Specific Memtable
//!
//! Provides in-memory storage for graph operations with CSR optimization.
//! This memtable is designed to work with the existing WAL infrastructure
//! while providing graph-specific functionality.

use crate::graph::{Edge, EdgeId, Node, NodeId};
use dashmap::DashMap;
use proximadb_kernel::error::ProximaDBError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::{debug, info};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Graph operation for WAL integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphOperation {
    CreateNode {
        graph_id: String,
        node: Node,
    },
    UpdateNode {
        graph_id: String,
        node_id: NodeId,
        update: NodeUpdate,
    },
    DeleteNode {
        graph_id: String,
        node_id: NodeId,
    },
    CreateEdge {
        graph_id: String,
        edge: Edge,
    },
    UpdateEdge {
        graph_id: String,
        edge_id: EdgeId,
        update: EdgeUpdate,
    },
    DeleteEdge {
        graph_id: String,
        edge_id: EdgeId,
    },
    BatchOperation {
        operations: Vec<GraphOperation>,
    },
    CreateEdgeIndex {
        graph_id: String,
        index_config: String, // Placeholder for index configuration
    },
    DropEdgeIndex {
        graph_id: String,
        index_name: String,
    },
}

/// Node update structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeUpdate {
    pub labels: Option<Vec<String>>,
    pub properties: Option<std::collections::HashMap<String, crate::graph::PropertyValue>>,
    pub embedding: Option<crate::graph::EmbeddingVersion>,
}

/// Edge update structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeUpdate {
    pub properties: Option<std::collections::HashMap<String, crate::graph::PropertyValue>>,
    pub weight: Option<f64>,
}

/// Graph snapshot for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSnapshot {
    pub graph_id: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub csr_offsets: Vec<usize>,
    pub csr_targets: Vec<NodeId>,
    pub timestamp: i64,
}

/// Graph-specific memtable optimized for CSR format
pub struct GraphMemtable {
    /// Graph identifier
    graph_id: String,

    /// CSR format for fast traversal
    csr_offsets: Arc<parking_lot::RwLock<Vec<usize>>>,
    csr_targets: Arc<parking_lot::RwLock<Vec<NodeId>>>,

    /// Node storage with Arc for zero-copy
    nodes: Arc<DashMap<NodeId, Arc<Node>>>,

    /// Edge storage with Arc for zero-copy
    edges: Arc<DashMap<EdgeId, Arc<Edge>>>,

    /// Label index for fast queries
    label_index: Arc<DashMap<String, HashSet<NodeId>>>,

    /// Edge type index
    edge_type_index: Arc<DashMap<String, HashSet<EdgeId>>>,

    /// Node to CSR index mapping
    node_to_index: Arc<DashMap<NodeId, usize>>,

    /// Memory usage tracking
    memory_usage: Arc<AtomicUsize>,

    /// Operation counter for WAL sequencing
    operation_counter: Arc<AtomicUsize>,
}

impl GraphMemtable {
    /// Create a new graph memtable
    pub fn new(graph_id: String) -> Self {
        info!("Creating GraphMemtable for graph: {}", graph_id);

        Self {
            graph_id,
            csr_offsets: Arc::new(parking_lot::RwLock::new(vec![0])),
            csr_targets: Arc::new(parking_lot::RwLock::new(Vec::new())),
            nodes: Arc::new(DashMap::new()),
            edges: Arc::new(DashMap::new()),
            label_index: Arc::new(DashMap::new()),
            edge_type_index: Arc::new(DashMap::new()),
            node_to_index: Arc::new(DashMap::new()),
            memory_usage: Arc::new(AtomicUsize::new(0)),
            operation_counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Apply a graph operation to the memtable
    pub fn apply_operation(&self, op: GraphOperation) -> Result<()> {
        self.operation_counter.fetch_add(1, Ordering::SeqCst);

        match op {
            GraphOperation::CreateNode { node, .. } => self.insert_node(node),
            GraphOperation::UpdateNode {
                node_id, update, ..
            } => self.update_node(node_id, update),
            GraphOperation::DeleteNode { node_id, .. } => self.delete_node(node_id),
            GraphOperation::CreateEdge { edge, .. } => self.insert_edge(edge),
            GraphOperation::UpdateEdge {
                edge_id, update, ..
            } => self.update_edge(edge_id, update),
            GraphOperation::DeleteEdge { edge_id, .. } => self.delete_edge(edge_id),
            GraphOperation::CreateEdgeIndex {
                graph_id: _,
                index_config: _,
            } => {
                // Deferred: Implement edge index creation
                Ok(())
            }
            GraphOperation::DropEdgeIndex {
                graph_id: _,
                index_name: _,
            } => {
                // Deferred: Implement edge index dropping
                Ok(())
            }
            GraphOperation::BatchOperation { operations } => {
                for op in operations {
                    self.apply_operation(op)?;
                }
                Ok(())
            }
        }
    }

    /// Insert a node
    fn insert_node(&self, node: Node) -> Result<()> {
        let node_id = node.id.clone();
        let node_arc = Arc::new(node.clone());

        // Update node storage
        self.nodes.insert(node_id.clone(), node_arc.clone());

        // Update label index
        for label in &node.labels {
            self.label_index
                .entry(label.clone())
                .or_default()
                .insert(node_id.clone());
        }

        // Update CSR structure
        self.update_csr_for_node(&node_id)?;

        // Update memory usage
        let node_size = std::mem::size_of::<Node>()
            + node.id.len()
            + node.labels.iter().map(|l| l.len()).sum::<usize>();
        self.memory_usage.fetch_add(node_size, Ordering::Relaxed);

        debug!("Inserted node {} into graph {}", node_id, self.graph_id);
        Ok(())
    }

    /// Update a node
    fn update_node(&self, node_id: NodeId, update: NodeUpdate) -> Result<()> {
        if let Some(mut entry) = self.nodes.get_mut(&node_id) {
            let mut node = (**entry).clone();

            if let Some(labels) = update.labels {
                // Remove from old label indexes
                for label in &node.labels {
                    if let Some(mut index) = self.label_index.get_mut(label) {
                        index.remove(&node_id);
                    }
                }

                // Update labels
                node.labels = labels;

                // Add to new label indexes
                for label in &node.labels {
                    self.label_index
                        .entry(label.clone())
                        .or_default()
                        .insert(node_id.clone());
                }
            }

            if let Some(properties) = update.properties {
                node.properties = properties;
            }

            if let Some(embedding) = update.embedding {
                node.embedding = Some(embedding);
            }

            *entry = Arc::new(node);
            Ok(())
        } else {
            Err(ProximaDBError::Storage(
                proximadb_kernel::error::StorageError::NotFound(format!(
                    "Node {} not found",
                    node_id
                )),
            ))
        }
    }

    /// Delete a node
    fn delete_node(&self, node_id: NodeId) -> Result<()> {
        if let Some((_, node)) = self.nodes.remove(&node_id) {
            // Remove from label index
            for label in &node.labels {
                if let Some(mut index) = self.label_index.get_mut(label) {
                    index.remove(&node_id);
                }
            }

            // Update CSR structure
            self.rebuild_csr()?;

            Ok(())
        } else {
            Err(ProximaDBError::Storage(
                proximadb_kernel::error::StorageError::NotFound(format!(
                    "Node {} not found",
                    node_id
                )),
            ))
        }
    }

    /// Insert an edge
    fn insert_edge(&self, edge: Edge) -> Result<()> {
        let edge_id = edge.id.clone();
        let edge_arc = Arc::new(edge.clone());

        // Update edge storage
        self.edges.insert(edge_id.clone(), edge_arc.clone());

        // Update edge type index
        self.edge_type_index
            .entry(edge.edge_type.clone())
            .or_default()
            .insert(edge_id.clone());

        // Update CSR structure for the edge
        self.update_csr_for_edge(&edge)?;

        // Update memory usage
        let edge_size = std::mem::size_of::<Edge>()
            + edge.id.len()
            + edge.from_node_id.len()
            + edge.to_node_id.len();
        self.memory_usage.fetch_add(edge_size, Ordering::Relaxed);

        debug!("Inserted edge {} into graph {}", edge_id, self.graph_id);
        Ok(())
    }

    /// Update an edge
    fn update_edge(&self, edge_id: EdgeId, update: EdgeUpdate) -> Result<()> {
        if let Some(mut entry) = self.edges.get_mut(&edge_id) {
            let mut edge = (**entry).clone();

            if let Some(properties) = update.properties {
                edge.properties = properties;
            }

            if let Some(weight) = update.weight {
                edge.weight = Some(weight);
            }

            *entry = Arc::new(edge);
            Ok(())
        } else {
            Err(ProximaDBError::Storage(
                proximadb_kernel::error::StorageError::NotFound(format!(
                    "Edge {} not found",
                    edge_id
                )),
            ))
        }
    }

    /// Delete an edge
    fn delete_edge(&self, edge_id: EdgeId) -> Result<()> {
        if let Some((_, edge)) = self.edges.remove(&edge_id) {
            // Remove from edge type index
            if let Some(mut index) = self.edge_type_index.get_mut(&edge.edge_type) {
                index.remove(&edge_id);
            }

            // Update CSR structure
            self.rebuild_csr()?;

            Ok(())
        } else {
            Err(ProximaDBError::Storage(
                proximadb_kernel::error::StorageError::NotFound(format!(
                    "Edge {} not found",
                    edge_id
                )),
            ))
        }
    }

    /// Update CSR structure for a new node
    fn update_csr_for_node(&self, node_id: &NodeId) -> Result<()> {
        let mut offsets = self.csr_offsets.write();

        // Assign index to node
        let new_index = offsets.len() - 1;
        self.node_to_index.insert(node_id.clone(), new_index);

        // Add new offset (initially same as previous, no edges yet)
        let last_offset = offsets.last().copied().unwrap_or(0);
        offsets.push(last_offset);

        Ok(())
    }

    /// Update CSR structure for a new edge
    fn update_csr_for_edge(&self, edge: &Edge) -> Result<()> {
        // This is a simplified version - in production, you'd need more sophisticated CSR updates
        if let Some(from_index) = self.node_to_index.get(&edge.from_node_id) {
            let from_idx = *from_index;

            let mut targets = self.csr_targets.write();
            let mut offsets = self.csr_offsets.write();

            // Find insertion point for this edge
            let _start = offsets[from_idx];
            let end = offsets[from_idx + 1];

            // Insert target node ID
            targets.insert(end, edge.to_node_id.clone());

            // Update all subsequent offsets
            for i in (from_idx + 1)..offsets.len() {
                offsets[i] += 1;
            }
        }

        Ok(())
    }

    /// Rebuild the entire CSR structure
    fn rebuild_csr(&self) -> Result<()> {
        let mut new_offsets = vec![0];
        let mut new_targets = Vec::new();
        let new_node_to_index = DashMap::new();

        // Collect all nodes
        let mut nodes: Vec<_> = self.nodes.iter().map(|entry| entry.key().clone()).collect();
        nodes.sort();

        // Build CSR for each node
        for (idx, node_id) in nodes.iter().enumerate() {
            new_node_to_index.insert(node_id.clone(), idx);

            // Find all outgoing edges
            let outgoing: Vec<_> = self
                .edges
                .iter()
                .filter(|e| e.from_node_id == *node_id)
                .map(|e| e.to_node_id.clone())
                .collect();

            // Add targets
            new_targets.extend(outgoing);
            new_offsets.push(new_targets.len());
        }

        // Update the structures
        *self.csr_offsets.write() = new_offsets;
        *self.csr_targets.write() = new_targets;
        self.node_to_index.clear();
        for (k, v) in new_node_to_index {
            self.node_to_index.insert(k, v);
        }

        Ok(())
    }

    /// Get neighbors of a node using CSR
    pub fn get_neighbors(&self, node_id: &NodeId) -> Result<Vec<NodeId>> {
        if let Some(node_index) = self.node_to_index.get(node_id) {
            let idx = *node_index;
            let offsets = self.csr_offsets.read();
            let targets = self.csr_targets.read();

            let start = offsets[idx];
            let end = offsets.get(idx + 1).copied().unwrap_or(targets.len());

            Ok(targets[start..end].to_vec())
        } else {
            Ok(Vec::new())
        }
    }

    /// Create a snapshot for persistence
    pub fn create_snapshot(&self) -> GraphSnapshot {
        let nodes = self
            .nodes
            .iter()
            .map(|entry| (**entry.value()).clone())
            .collect();

        let edges = self
            .edges
            .iter()
            .map(|entry| (**entry.value()).clone())
            .collect();

        GraphSnapshot {
            graph_id: self.graph_id.clone(),
            nodes,
            edges,
            csr_offsets: self.csr_offsets.read().clone(),
            csr_targets: self.csr_targets.read().clone(),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Restore from a snapshot
    pub fn restore_from_snapshot(&self, snapshot: GraphSnapshot) -> Result<()> {
        // Clear existing data
        self.nodes.clear();
        self.edges.clear();
        self.label_index.clear();
        self.edge_type_index.clear();
        self.node_to_index.clear();

        // Restore nodes
        for node in snapshot.nodes {
            let node_arc = Arc::new(node);
            self.nodes.insert(node_arc.id.clone(), node_arc.clone());

            // Rebuild label index
            for label in &node_arc.labels {
                self.label_index
                    .entry(label.clone())
                    .or_default()
                    .insert(node_arc.id.clone());
            }
        }

        // Restore edges
        for edge in snapshot.edges {
            let edge_arc = Arc::new(edge);
            self.edges.insert(edge_arc.id.clone(), edge_arc.clone());

            // Rebuild edge type index
            self.edge_type_index
                .entry(edge_arc.edge_type.clone())
                .or_default()
                .insert(edge_arc.id.clone());
        }

        // Restore CSR structure
        *self.csr_offsets.write() = snapshot.csr_offsets;
        *self.csr_targets.write() = snapshot.csr_targets;

        // Rebuild node_to_index
        let nodes: Vec<_> = self.nodes.iter().map(|entry| entry.key().clone()).collect();
        for (idx, node_id) in nodes.iter().enumerate() {
            self.node_to_index.insert(node_id.clone(), idx);
        }

        Ok(())
    }

    /// Get memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        self.memory_usage.load(Ordering::Relaxed)
    }

    /// Get operation count
    pub fn operation_count(&self) -> usize {
        self.operation_counter.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_memtable_basic_operations() {
        let memtable = GraphMemtable::new("test_graph".to_string());

        // Create a node
        let node = Node {
            id: "node1".to_string(),
            labels: vec!["TestNode".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        let op = GraphOperation::CreateNode {
            graph_id: "test_graph".to_string(),
            node,
        };

        memtable.apply_operation(op).unwrap();

        // Verify node exists
        assert!(memtable.nodes.contains_key("node1"));
        assert_eq!(memtable.operation_count(), 1);
    }

    #[test]
    fn test_csr_structure() {
        let memtable = GraphMemtable::new("test_graph".to_string());

        // Create nodes
        for i in 0..3 {
            let node = Node {
                id: format!("node{}", i),
                labels: vec!["TestNode".to_string()],
                properties: std::collections::HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };

            let op = GraphOperation::CreateNode {
                graph_id: "test_graph".to_string(),
                node,
            };

            memtable.apply_operation(op).unwrap();
        }

        // Create edges
        let edge = Edge {
            id: "edge1".to_string(),
            from_node_id: "node0".to_string(),
            to_node_id: "node1".to_string(),
            edge_type: "CONNECTS".to_string(),
            properties: std::collections::HashMap::new(),
            weight: Some(1.0),
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        let op = GraphOperation::CreateEdge {
            graph_id: "test_graph".to_string(),
            edge,
        };

        memtable.apply_operation(op).unwrap();

        // Check neighbors
        let neighbors = memtable.get_neighbors(&"node0".to_string()).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0], "node1");
    }
}
