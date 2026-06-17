//! # Core Graph Types
//!
//! Fundamental graph data structures: nodes, edges, graphs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Unique graph identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphId(Uuid);

impl GraphId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl Default for GraphId {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique node identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(Uuid);

impl NodeId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique edge identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeId(Uuid);

impl EdgeId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for EdgeId {
    fn default() -> Self {
        Self::new()
    }
}

/// Graph node with optional properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub label: String,
    pub properties: HashMap<String, serde_json::Value>,
}

impl Node {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            id: NodeId::new(),
            label: label.into(),
            properties: HashMap::new(),
        }
    }

    pub fn with_id(id: NodeId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            properties: HashMap::new(),
        }
    }

    pub fn with_property(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.properties.insert(key.into(), value);
        self
    }

    pub fn id(&self) -> NodeId {
        self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn get_property(&self, key: &str) -> Option<&serde_json::Value> {
        self.properties.get(key)
    }
}

/// Graph edge with optional properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    pub label: String,
    pub properties: HashMap<String, serde_json::Value>,
}

impl Edge {
    pub fn new(from: NodeId, to: NodeId, label: impl Into<String>) -> Self {
        Self {
            id: EdgeId::new(),
            from,
            to,
            label: label.into(),
            properties: HashMap::new(),
        }
    }

    pub fn with_id(id: EdgeId, from: NodeId, to: NodeId, label: impl Into<String>) -> Self {
        Self {
            id,
            from,
            to,
            label: label.into(),
            properties: HashMap::new(),
        }
    }

    pub fn with_property(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.properties.insert(key.into(), value);
        self
    }

    pub fn id(&self) -> EdgeId {
        self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn from_node(&self) -> NodeId {
        self.from
    }

    pub fn to_node(&self) -> NodeId {
        self.to
    }
}

/// Base graph trait
pub trait Graph: Send + Sync {
    /// Add a node to the graph
    fn add_node(&mut self, node: Node) -> NodeId;

    /// Get a node by ID
    fn get_node(&self, id: NodeId) -> Option<&Node>;

    /// Remove a node
    fn remove_node(&mut self, id: NodeId) -> bool;

    /// Add an edge to the graph
    fn add_edge(&mut self, edge: Edge) -> EdgeId;

    /// Get an edge by ID
    fn get_edge(&self, id: EdgeId) -> Option<&Edge>;

    /// Remove an edge
    fn remove_edge(&mut self, id: EdgeId) -> bool;

    /// Get all neighbors of a node
    fn neighbors(&self, id: NodeId) -> Vec<NodeId>;

    /// Check if an edge exists
    fn has_edge(&self, from: NodeId, to: NodeId) -> bool;

    /// Get node count
    fn node_count(&self) -> usize;

    /// Get edge count
    fn edge_count(&self) -> usize;

    /// Get all nodes
    fn nodes(&self) -> Vec<&Node>;
}

/// Directed graph implementation using adjacency lists
#[derive(Debug, Clone, Default)]
pub struct DirectedGraph {
    nodes: HashMap<NodeId, Node>,
    edges: HashMap<EdgeId, Edge>,
    adjacency: HashMap<NodeId, Vec<EdgeId>>,
    reverse_adjacency: HashMap<NodeId, Vec<EdgeId>>,
}

impl DirectedGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node with a label, returning its ID
    pub fn add_node(&mut self, label: impl Into<String>) -> NodeId {
        let node = Node::new(label);
        let id = node.id();
        self.nodes.insert(id, node);
        self.adjacency.entry(id).or_default();
        self.reverse_adjacency.entry(id).or_default();
        id
    }

    /// Add an edge between two nodes with a label, returning its ID
    pub fn add_edge(&mut self, from: NodeId, to: NodeId, label: impl Into<String>) -> EdgeId {
        let edge = Edge::new(from, to, label);
        let id = edge.id();
        self.edges.insert(id, edge);
        self.adjacency.entry(from).or_default().push(id);
        self.reverse_adjacency.entry(to).or_default().push(id);
        id
    }

    /// Get outgoing edges from a node
    pub fn outgoing_edges(&self, id: NodeId) -> Vec<&Edge> {
        self.adjacency
            .get(&id)
            .map(|edge_ids| {
                edge_ids
                    .iter()
                    .filter_map(|eid| self.edges.get(eid))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get incoming edges to a node
    pub fn incoming_edges(&self, id: NodeId) -> Vec<&Edge> {
        self.reverse_adjacency
            .get(&id)
            .map(|edge_ids| {
                edge_ids
                    .iter()
                    .filter_map(|eid| self.edges.get(eid))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Graph for DirectedGraph {
    fn add_node(&mut self, node: Node) -> NodeId {
        let id = node.id();
        self.nodes.insert(id, node);
        self.adjacency.entry(id).or_default();
        self.reverse_adjacency.entry(id).or_default();
        id
    }

    fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    fn remove_node(&mut self, id: NodeId) -> bool {
        if self.nodes.remove(&id).is_some() {
            // Remove all edges connected to this node
            if let Some(edge_ids) = self.adjacency.remove(&id) {
                for eid in edge_ids {
                    self.edges.remove(&eid);
                }
            }
            if let Some(edge_ids) = self.reverse_adjacency.remove(&id) {
                for eid in edge_ids {
                    self.edges.remove(&eid);
                }
            }
            true
        } else {
            false
        }
    }

    fn add_edge(&mut self, edge: Edge) -> EdgeId {
        let id = edge.id();
        let from = edge.from;
        let to = edge.to;
        self.edges.insert(id, edge);
        self.adjacency.entry(from).or_default().push(id);
        self.reverse_adjacency.entry(to).or_default().push(id);
        id
    }

    fn get_edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.get(&id)
    }

    fn remove_edge(&mut self, id: EdgeId) -> bool {
        if let Some(edge) = self.edges.remove(&id) {
            let from = edge.from;
            let to = edge.to;
            if let Some(adj) = self.adjacency.get_mut(&from) {
                adj.retain(|&eid| eid != id);
            }
            if let Some(adj) = self.reverse_adjacency.get_mut(&to) {
                adj.retain(|&eid| eid != id);
            }
            true
        } else {
            false
        }
    }

    fn neighbors(&self, id: NodeId) -> Vec<NodeId> {
        self.outgoing_edges(id).iter().map(|e| e.to).collect()
    }

    fn has_edge(&self, from: NodeId, to: NodeId) -> bool {
        self.outgoing_edges(from).iter().any(|e| e.to == to)
    }

    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn edge_count(&self) -> usize {
        self.edges.len()
    }

    fn nodes(&self) -> Vec<&Node> {
        self.nodes.values().collect()
    }
}

/// Undirected graph implementation
#[derive(Debug, Clone, Default)]
pub struct UndirectedGraph {
    directed: DirectedGraph,
}

impl UndirectedGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, label: impl Into<String>) -> NodeId {
        self.directed.add_node(label)
    }

    pub fn add_edge(&mut self, from: NodeId, to: NodeId, label: impl Into<String>) -> EdgeId {
        // In undirected graph, add edge in both directions
        let edge = Edge::new(from, to, label);
        let id = edge.id();
        self.directed.edges.insert(id, edge);
        self.directed.adjacency.entry(from).or_default().push(id);
        self.directed.adjacency.entry(to).or_default().push(id);
        id
    }

    pub fn edges(&self, id: NodeId) -> Vec<&Edge> {
        self.directed.outgoing_edges(id)
    }
}

impl Graph for UndirectedGraph {
    fn add_node(&mut self, node: Node) -> NodeId {
        let id = node.id();
        Graph::add_node(&mut self.directed, node);
        id
    }

    fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.directed.get_node(id)
    }

    fn remove_node(&mut self, id: NodeId) -> bool {
        self.directed.remove_node(id)
    }

    fn add_edge(&mut self, edge: Edge) -> EdgeId {
        let from = edge.from;
        let to = edge.to;
        let label = edge.label;
        self.add_edge(from, to, label)
    }

    fn get_edge(&self, id: EdgeId) -> Option<&Edge> {
        self.directed.get_edge(id)
    }

    fn remove_edge(&mut self, id: EdgeId) -> bool {
        self.directed.remove_edge(id)
    }

    fn neighbors(&self, id: NodeId) -> Vec<NodeId> {
        self.directed.neighbors(id)
    }

    fn has_edge(&self, from: NodeId, to: NodeId) -> bool {
        self.directed.has_edge(from, to) || self.directed.has_edge(to, from)
    }

    fn node_count(&self) -> usize {
        self.directed.node_count()
    }

    fn edge_count(&self) -> usize {
        self.directed.edge_count()
    }

    fn nodes(&self) -> Vec<&Node> {
        self.directed.nodes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_creation() {
        let node = Node::new("Person");
        assert_eq!(node.label(), "Person");
    }

    #[test]
    fn test_edge_creation() {
        let from = NodeId::new();
        let to = NodeId::new();
        let edge = Edge::new(from, to, "knows");
        assert_eq!(edge.label(), "knows");
        assert_eq!(edge.from_node(), from);
        assert_eq!(edge.to_node(), to);
    }

    #[test]
    fn test_directed_graph() {
        let mut graph = DirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        graph.add_edge(n1, n2, "knows");

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
        assert!(graph.has_edge(n1, n2));
        assert!(!graph.has_edge(n2, n1));
    }

    #[test]
    fn test_undirected_graph() {
        let mut graph = UndirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        graph.add_edge(n1, n2, "connected");

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
        assert!(graph.has_edge(n1, n2));
        assert!(graph.has_edge(n2, n1)); // Symmetric in undirected
    }
}
