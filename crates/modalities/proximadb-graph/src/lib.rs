//! # ProximaDB Graph Modality
//!
//! This crate contains native graph database capabilities including:
//!
//! - **Core types** - Nodes, edges, properties, graph schemas
//! - **Traversal** - Graph traversal algorithms (BFS, DFS, shortest path)
//! - **Query** - Graph query execution and Cypher/GQL support
//! - **Storage** - Graph storage engines (ORION in-memory, PULSAR distributed)
//! - **RAG** - Graph RAG (Retrieval-Augmented Generation) capabilities
//!
//! ## Architecture
//!
//! The graph modality provides:
//! - CSR (Compressed Sparse Row) format for efficient adjacency
//! - Zero-copy Arc-based node/edge references
//! - Multi-edge support between nodes
//! - Property graph model with typed attributes
//!
//! ## Dependencies
//!
//! - `proximadb-kernel` - Core error types and foundational contracts
//! - `proximadb-proto` - Protocol buffer types
//! - `proximadb-records` - Canonical `ProximaRecord` envelope
//! - `proximadb-data-model` - Canonical `ProximaValue` rich type system
//! - `proximadb-graph-query` - Graph query contracts

pub mod core;
pub mod query;
pub mod record;
pub mod storage;
pub mod traversal;

// Re-export core types
pub use core::{DirectedGraph, Edge, EdgeId, Graph, GraphId, Node, NodeId, UndirectedGraph};

pub use traversal::{
    BreadthFirst, DepthFirst, ShortestPath, Traversal, TraversalOrder, TraversalResult,
};

pub use storage::{GraphStorage, MemoryGraphStorage};

/// Graph direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeDirection {
    Outgoing,
    Incoming,
    Both,
}

/// Graph statistics
#[derive(Debug, Clone, Default)]
pub struct GraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub avg_degree: f32,
    pub diameter: Option<usize>,
    pub is_connected: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_module_imports() {
        let _id = GraphId::new();
        let _node = Node::new("test");
        let _edge = Edge::new(NodeId::new(), NodeId::new(), "connected");
    }

    #[test]
    fn test_directed_graph() {
        let mut graph = DirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        graph.add_edge(n1, n2, "knows");

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn test_traversal() {
        let mut graph = DirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        let n3 = graph.add_node("C");
        graph.add_edge(n1, n2, "knows");
        graph.add_edge(n2, n3, "knows");

        let result = BreadthFirst::from(&graph).start(n1).execute();
        assert_eq!(result.visited_nodes.len(), 3);
    }
}
