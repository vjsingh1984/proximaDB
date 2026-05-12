//! # Graph Traversal Algorithms
//!
//! BFS, DFS, shortest path, and other traversal algorithms.

use super::core::{Graph, NodeId};
use std::collections::{HashMap, HashSet, VecDeque};

/// Traversal order strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalOrder {
    BreadthFirst,
    DepthFirst,
}

/// Result of a graph traversal
#[derive(Debug, Clone)]
pub struct TraversalResult {
    /// Nodes visited in traversal order
    pub visited_nodes: Vec<NodeId>,
    /// Distance from start node (for shortest path)
    pub distances: HashMap<NodeId, usize>,
    /// Path from start to each node (for shortest path)
    pub paths: HashMap<NodeId, Vec<NodeId>>,
}

impl TraversalResult {
    pub fn new() -> Self {
        Self {
            visited_nodes: Vec::new(),
            distances: HashMap::new(),
            paths: HashMap::new(),
        }
    }

    /// Check if a node was visited
    pub fn was_visited(&self, id: NodeId) -> bool {
        self.visited_nodes.contains(&id)
    }

    /// Get distance to a node
    pub fn distance_to(&self, id: NodeId) -> Option<usize> {
        self.distances.get(&id).copied()
    }

    /// Get path to a node
    pub fn path_to(&self, id: NodeId) -> Option<&[NodeId]> {
        self.paths.get(&id).map(|p| p.as_slice())
    }
}

impl Default for TraversalResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Graph traversal trait
pub trait Traversal {
    fn start(self, start: NodeId) -> Self;
    fn execute(self) -> TraversalResult;
}

/// Breadth-first search traversal
pub struct BreadthFirst<'a, G>
where
    G: Graph,
{
    graph: &'a G,
    start: Option<NodeId>,
}

impl<'a, G> BreadthFirst<'a, G>
where
    G: Graph,
{
    pub fn from(graph: &'a G) -> Self {
        Self { graph, start: None }
    }

    pub fn start(mut self, start: NodeId) -> Self {
        self.start = Some(start);
        self
    }

    pub fn execute(self) -> TraversalResult {
        let start = self.start.expect("Start node not set");
        let mut result = TraversalResult::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut distances = HashMap::new();
        let mut paths = HashMap::new();

        queue.push_back(start);
        visited.insert(start);
        distances.insert(start, 0);
        paths.insert(start, vec![start]);

        while let Some(current) = queue.pop_front() {
            result.visited_nodes.push(current);

            let current_dist = distances[&current];

            for neighbor in self.graph.neighbors(current) {
                if !visited.contains(&neighbor) {
                    visited.insert(neighbor);
                    distances.insert(neighbor, current_dist + 1);

                    let mut path = paths[&current].clone();
                    path.push(neighbor);
                    paths.insert(neighbor, path);

                    queue.push_back(neighbor);
                }
            }
        }

        result.distances = distances;
        result.paths = paths;
        result
    }
}

/// Depth-first search traversal
pub struct DepthFirst<'a, G>
where
    G: Graph,
{
    graph: &'a G,
    start: Option<NodeId>,
}

impl<'a, G> DepthFirst<'a, G>
where
    G: Graph,
{
    pub fn from(graph: &'a G) -> Self {
        Self { graph, start: None }
    }

    pub fn start(mut self, start: NodeId) -> Self {
        self.start = Some(start);
        self
    }

    pub fn execute(self) -> TraversalResult {
        let start = self.start.expect("Start node not set");
        let mut result = TraversalResult::new();
        let mut visited = HashSet::new();
        let mut stack = vec![start];

        while let Some(current) = stack.pop() {
            if !visited.contains(&current) {
                visited.insert(current);
                result.visited_nodes.push(current);

                // Push neighbors in reverse order for natural DFS order
                let neighbors: Vec<_> = self.graph.neighbors(current);
                for neighbor in neighbors.into_iter().rev() {
                    if !visited.contains(&neighbor) {
                        stack.push(neighbor);
                    }
                }
            }
        }

        result
    }
}

/// Shortest path using BFS (for unweighted graphs)
pub struct ShortestPath<'a, G>
where
    G: Graph,
{
    graph: &'a G,
    start: Option<NodeId>,
    goal: Option<NodeId>,
}

impl<'a, G> ShortestPath<'a, G>
where
    G: Graph,
{
    pub fn from(graph: &'a G) -> Self {
        Self {
            graph,
            start: None,
            goal: None,
        }
    }

    pub fn from_to(mut self, start: NodeId, goal: NodeId) -> Self {
        self.start = Some(start);
        self.goal = Some(goal);
        self
    }

    pub fn execute(self) -> Option<Vec<NodeId>> {
        let start = self.start?;
        let goal = self.goal?;

        if start == goal {
            return Some(vec![start]);
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut came_from: HashMap<NodeId, NodeId> = HashMap::new();

        queue.push_back(start);
        visited.insert(start);

        while let Some(current) = queue.pop_front() {
            if current == goal {
                // Reconstruct path
                let mut path = vec![current];
                let mut node = current;
                while let Some(&from) = came_from.get(&node) {
                    path.push(from);
                    node = from;
                }
                path.reverse();
                return Some(path);
            }

            for neighbor in self.graph.neighbors(current) {
                if !visited.contains(&neighbor) {
                    visited.insert(neighbor);
                    came_from.insert(neighbor, current);
                    queue.push_back(neighbor);
                }
            }
        }

        None // No path found
    }

    /// Get distance (number of edges) between two nodes
    pub fn distance(self) -> Option<usize> {
        self.execute().map(|path| path.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::DirectedGraph;

    #[test]
    fn test_bfs_traversal() {
        let mut graph = DirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        let n3 = graph.add_node("C");
        let n4 = graph.add_node("D");

        graph.add_edge(n1, n2, "a->b");
        graph.add_edge(n1, n3, "a->c");
        graph.add_edge(n2, n4, "b->d");

        let result = BreadthFirst::from(&graph).start(n1).execute();

        assert_eq!(result.visited_nodes.len(), 4);
        assert_eq!(result.distance_to(n1), Some(0));
        assert_eq!(result.distance_to(n2), Some(1));
        assert_eq!(result.distance_to(n4), Some(2));
    }

    #[test]
    fn test_dfs_traversal() {
        let mut graph = DirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        let n3 = graph.add_node("C");

        graph.add_edge(n1, n2, "a->b");
        graph.add_edge(n1, n3, "a->c");
        graph.add_edge(n2, n3, "b->c");

        let result = DepthFirst::from(&graph).start(n1).execute();

        assert_eq!(result.visited_nodes.len(), 3);
    }

    #[test]
    fn test_shortest_path() {
        let mut graph = DirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        let n3 = graph.add_node("C");
        let n4 = graph.add_node("D");

        graph.add_edge(n1, n2, "a->b");
        graph.add_edge(n2, n4, "b->d");
        graph.add_edge(n1, n3, "a->c");
        graph.add_edge(n3, n4, "c->d");

        let path = ShortestPath::from(&graph).from_to(n1, n4).execute();

        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 3); // A -> B -> D or A -> C -> D
        assert_eq!(path[0], n1);
        assert_eq!(path[2], n4);
    }

    #[test]
    fn test_shortest_path_distance() {
        let mut graph = DirectedGraph::new();
        let n1 = graph.add_node("A");
        let n2 = graph.add_node("B");
        let n3 = graph.add_node("C");

        graph.add_edge(n1, n2, "a->b");
        graph.add_edge(n2, n3, "b->c");

        let distance = ShortestPath::from(&graph).from_to(n1, n3).distance();

        assert_eq!(distance, Some(2));
    }
}
