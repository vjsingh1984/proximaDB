//! # Graph Storage
//!
//! Graph storage engines and persistence layer.

use super::core::DirectedGraph;
use std::collections::HashMap;

/// Graph storage trait for pluggable storage backends
pub trait GraphStorage: Send + Sync {
    /// Load a graph by ID
    fn load(&self, id: &str) -> Result<DirectedGraph, String>;

    /// Save a graph
    fn save(&self, id: &str, graph: &DirectedGraph) -> Result<(), String>;

    /// Delete a graph
    fn delete(&self, id: &str) -> Result<(), String>;

    /// List all graph IDs
    fn list(&self) -> Result<Vec<String>, String>;
}

/// In-memory graph storage
pub struct MemoryGraphStorage {
    graphs: HashMap<String, DirectedGraph>,
}

impl Default for MemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryGraphStorage {
    pub fn new() -> Self {
        Self {
            graphs: HashMap::new(),
        }
    }

    pub fn register(&mut self, id: String, graph: DirectedGraph) {
        self.graphs.insert(id, graph);
    }
}

impl GraphStorage for MemoryGraphStorage {
    fn load(&self, id: &str) -> Result<DirectedGraph, String> {
        self.graphs
            .get(id)
            .cloned()
            .ok_or_else(|| format!("Graph '{}' not found", id))
    }

    fn save(&self, id: &str, graph: &DirectedGraph) -> Result<(), String> {
        // Note: this doesn't actually save since we'd need &mut self
        // In real usage, graphs would be managed through register
        let _ = id;
        let _ = graph;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        if self.graphs.contains_key(id) {
            Ok(())
        } else {
            Err(format!("Graph '{}' not found", id))
        }
    }

    fn list(&self) -> Result<Vec<String>, String> {
        Ok(self.graphs.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Graph;

    #[test]
    fn test_memory_storage() {
        let mut storage = MemoryGraphStorage::new();

        let mut graph = DirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        graph.add_edge(n1, n2, "knows");

        storage.register("test".to_string(), graph);

        let list = storage.list().unwrap();
        assert!(list.contains(&"test".to_string()));

        let loaded = storage.load("test").unwrap();
        assert_eq!(loaded.node_count(), 2);
    }
}
