/*
 * Generic traversal utilities over the GraphEngine trait.
 * Provides BFS/DFS/shortest path and simple graph analysis using
 * only trait methods available on all engines.
 */

use proximadb_kernel::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::engines::{GraphEngine, GraphEngineImpl};
use crate::graph::{Edge, Node, NodeId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Result of a generic graph traversal operation
#[derive(Debug, Clone, Default)]
pub struct GenericTraversalResult {
    /// Nodes visited during traversal
    pub nodes: Vec<Arc<Node>>,
    /// Edges traversed during traversal
    pub edges: Vec<Arc<Edge>>,
    /// Paths from start node to each visited node (node IDs)
    pub paths: Vec<Vec<NodeId>>,
    /// Total number of unique nodes visited
    pub nodes_visited: usize,
    /// Total number of edges traversed
    pub edges_traversed: usize,
    /// Maximum depth reached from start node
    pub max_depth_reached: u32,
}

/// Perform breadth-first search (BFS) traversal on a graph
///
/// # Arguments
/// * `engine` - Graph engine implementation
/// * `start` - Starting node ID
/// * `edge_types` - Optional filter for edge types to traverse
/// * `max_depth` - Optional maximum traversal depth
/// * `limit` - Optional limit on number of nodes to visit
///
/// # Returns
/// Traversal result containing visited nodes, edges, and paths
pub fn bfs_generic(
    engine: &GraphEngineImpl,
    start: &NodeId,
    edge_types: Option<&[String]>,
    max_depth: Option<u32>,
    limit: Option<usize>,
) -> Result<GenericTraversalResult> {
    if engine.get_node(start)?.is_none() {
        return Err(ProximaDBError::InvalidInput(format!(
            "Start node {start} not found"
        )));
    }
    let mut visited: HashSet<NodeId> = HashSet::new();
    let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
    let mut q: VecDeque<(NodeId, u32)> = VecDeque::new();
    let allowed: Option<HashSet<String>> = edge_types.map(|v| v.iter().cloned().collect());

    visited.insert(start.clone());
    q.push_back((start.clone(), 0));

    let mut nodes: Vec<Arc<Node>> = Vec::new();
    let mut edges: Vec<Arc<Edge>> = Vec::new();
    let mut max_depth_reached = 0u32;

    while let Some((u, d)) = q.pop_front() {
        max_depth_reached = max_depth_reached.max(d);
        if let Some(n) = engine.get_node(&u)? {
            nodes.push(n);
        }
        if let Some(m) = max_depth
            && d >= m
        {
            continue;
        }
        // Fetch all outgoing and filter by allowed types if provided
        let mut outs = engine.get_outgoing_edges(&u, None)?;
        if let Some(allow) = &allowed {
            outs.retain(|e| allow.contains(&e.edge_type));
        }
        for e in outs {
            let v = e.to_node_id.clone();
            if !visited.contains(&v) {
                visited.insert(v.clone());
                parent.insert(v.clone(), u.clone());
                q.push_back((v.clone(), d + 1));
            }
            edges.push(e);
            if let Some(lim) = limit
                && nodes.len() >= lim
            {
                break;
            }
        }
        if let Some(lim) = limit
            && nodes.len() >= lim
        {
            break;
        }
    }

    // Build simple paths from start to each visited node using parent map
    let mut paths: Vec<Vec<NodeId>> = Vec::new();
    for n in &visited {
        let mut path = Vec::new();
        let mut cur = n.clone();
        path.push(cur.clone());
        while let Some(p) = parent.get(&cur) {
            cur = p.clone();
            path.push(cur.clone());
        }
        if path.last().is_some_and(|x| x == start) {
            path.reverse();
            paths.push(path);
        }
    }

    Ok(GenericTraversalResult {
        nodes_visited: visited.len(),
        edges_traversed: edges.len(),
        nodes,
        edges,
        paths,
        max_depth_reached,
    })
}

/// Find shortest path using Dijkstra's algorithm
///
/// # Arguments
/// * `engine` - Graph engine implementation
/// * `start` - Starting node ID
/// * `target` - Target node ID
/// * `edge_types` - Optional filter for edge types to traverse
///
/// # Returns
/// Optional tuple of (path as node IDs, total distance)
pub fn dijkstra_generic(
    engine: &GraphEngineImpl,
    start: &NodeId,
    target: &NodeId,
    edge_types: Option<&[String]>,
) -> Result<Option<(Vec<NodeId>, f64)>> {
    use std::cmp::Ordering;
    use std::collections::BinaryHeap;

    /// Queue node for Dijkstra's priority queue (min-heap via reverse ordering)
    #[derive(Debug, PartialEq)]
    struct QN {
        node_id: NodeId,
        dist: f64,
    }
    impl Eq for QN {}
    impl Ord for QN {
        fn cmp(&self, other: &Self) -> Ordering {
            // Reverse ordering for min-heap behavior
            other.dist.partial_cmp(&self.dist).unwrap_or_else(|| {
                // Fallback when distances are NaN (compare node IDs)
                match self.node_id.cmp(&other.node_id) {
                    Ordering::Less => Ordering::Greater,
                    Ordering::Greater => Ordering::Less,
                    Ordering::Equal => Ordering::Equal,
                }
            })
        }
    }
    impl PartialOrd for QN {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    if engine.get_node(start)?.is_none() || engine.get_node(target)?.is_none() {
        return Ok(None);
    }
    let allow: Option<HashSet<String>> = edge_types.map(|v| v.iter().cloned().collect());

    let mut heap = BinaryHeap::new();
    let mut dist: HashMap<NodeId, f64> = HashMap::new();
    let mut prev: HashMap<NodeId, NodeId> = HashMap::new();

    dist.insert(start.clone(), 0.0);
    heap.push(QN {
        node_id: start.clone(),
        dist: 0.0,
    });

    while let Some(cur) = heap.pop() {
        if cur.node_id == *target {
            // reconstruct
            let mut path = Vec::new();
            let mut c = target.clone();
            path.push(c.clone());
            while let Some(p) = prev.get(&c) {
                c = p.clone();
                path.push(c.clone());
            }
            path.reverse();
            return Ok(Some((path, cur.dist)));
        }
        if let Some(&best) = dist.get(&cur.node_id)
            && cur.dist > best
        {
            continue;
        }

        let mut outs = engine.get_outgoing_edges(&cur.node_id, None)?;
        if let Some(allow) = &allow {
            outs.retain(|e| allow.contains(&e.edge_type));
        }
        for e in outs {
            let v = e.to_node_id.clone();
            let w = e.weight.unwrap_or(1.0);
            let nd = cur.dist + w;
            if nd < *dist.get(&v).unwrap_or(&f64::INFINITY) {
                dist.insert(v.clone(), nd);
                prev.insert(v.clone(), cur.node_id.clone());
                heap.push(QN {
                    node_id: v,
                    dist: nd,
                });
            }
        }
    }
    Ok(None)
}

/// Find all connected components in an undirected graph
///
/// # Arguments
/// * `engine` - Graph engine implementation
///
/// # Returns
/// Vector of connected components, each component is a vector of node IDs
pub fn connected_components_generic(engine: &GraphEngineImpl) -> Result<Vec<Vec<NodeId>>> {
    let all_nodes = engine.get_all_nodes()?;
    let mut comps: Vec<Vec<NodeId>> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for n in all_nodes {
        if seen.contains(&n.id) {
            continue;
        }
        let mut comp = Vec::new();
        let mut q = VecDeque::new();
        q.push_back(n.id.clone());
        seen.insert(n.id.clone());
        while let Some(u) = q.pop_front() {
            comp.push(u.clone());
            // undirected: outgoing + incoming
            for e in engine.get_outgoing_edges(&u, None)? {
                if seen.insert(e.to_node_id.clone()) {
                    q.push_back(e.to_node_id.clone());
                }
            }
            for e in engine.get_incoming_edges(&u, None)? {
                if seen.insert(e.from_node_id.clone()) {
                    q.push_back(e.from_node_id.clone());
                }
            }
        }
        comps.push(comp);
    }
    Ok(comps)
}

/// Detect if a graph contains a cycle
///
/// # Arguments
/// * `engine` - Graph engine implementation
///
/// # Returns
/// true if a cycle exists, false otherwise
pub fn has_cycle_generic(engine: &GraphEngineImpl) -> Result<bool> {
    /// Node color for cycle detection using DFS
    #[derive(Copy, Clone, PartialEq, Eq)]
    enum Color {
        /// Unvisited node
        White,
        /// Node currently being visited (in recursion stack)
        Gray,
        /// Node completely visited
        Black,
    }
    let mut color: HashMap<NodeId, Color> = HashMap::new();
    for n in engine.get_all_nodes()? {
        color.insert(n.id.clone(), Color::White);
    }

    fn dfs(
        engine: &GraphEngineImpl,
        u: &NodeId,
        color: &mut HashMap<NodeId, Color>,
    ) -> Result<bool> {
        color.insert(u.clone(), Color::Gray);
        for e in engine.get_outgoing_edges(u, None)? {
            let v = e.to_node_id.clone();
            match color.get(&v).copied().unwrap_or(Color::White) {
                Color::White => {
                    if dfs(engine, &v, color)? {
                        return Ok(true);
                    }
                }
                Color::Gray => {
                    return Ok(true);
                }
                Color::Black => {}
            }
        }
        color.insert(u.clone(), Color::Black);
        Ok(false)
    }

    for (nid, c) in color.clone().into_iter() {
        if c == Color::White && dfs(engine, &nid, &mut color)? {
            return Ok(true);
        }
    }
    Ok(false)
}
