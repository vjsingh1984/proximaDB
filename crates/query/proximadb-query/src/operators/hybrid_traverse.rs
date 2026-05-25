//! HybridTraverse executor — Phase D, spec §7 (GredoDB Algorithm 1).
//!
//! Interleaves **vector ANN search** with **graph edge expansion** in a single
//! beam-search loop.  The algorithm proceeds as follows:
//!
//! ```text
//! frontier = ANN(seed_query, beam_width)        // vector seeds
//! visited  = {}
//! results  = []
//!
//! while frontier is not empty AND depth <= max_hops:
//!     next_frontier = {}
//!     for each node in frontier:
//!         if node passes edge_pattern:
//!             results.append(node)
//!             next_frontier += graph_neighbors(node, edge_pattern)
//!     frontier = next_frontier \ visited
//!     visited  += frontier
//!
//! return top-k results ranked by score
//! ```
//!
//! This module provides the **execution contract** (traits + structs).  The
//! concrete ANN and graph backends are injected via trait objects so the
//! executor stays modality-agnostic and fully testable in isolation.
//!
//! Reference: GredoDB §3 Algorithm 1 "Hybrid Graph-Vector Traversal".

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use proximadb_multimodel_plan::{EdgePattern, TraversalDirection};

/// A node produced by the traversal (combines vector score + graph depth).
#[derive(Debug, Clone)]
pub struct TraversalNode {
    /// Node identifier (must be globally unique within the graph).
    pub id: String,
    /// Similarity score from the original ANN search (higher = closer).
    pub vector_score: f32,
    /// Graph hop depth at which this node was reached (0 = seed).
    pub hop_depth: u32,
    /// Optional payload carried by the node.
    pub payload: serde_json::Value,
}

/// Statistics from one hybrid-traverse execution.
///
/// Naming note: this type used to be called `TraversalStats` and collided
/// with the proto `proximadb_v1::TraversalStats` wire form (which has a
/// different shape: nodes/edges/max_depth/exec_time_us). Renamed to
/// `HybridTraverseStats` because the field set is operator-scoped
/// (tracks `iterations` + `results_returned`, not depth/time). The proto
/// type remains the canonical wire form. Distinct also from the per-engine
/// stats (OrionTraversalStats, PulsarTraversalStats, CanonicalTraversalStats).
#[derive(Debug, Clone, Default)]
pub struct HybridTraverseStats {
    /// Total frontier expansions performed.
    pub iterations: u32,
    /// Total nodes visited (including pruned).
    pub nodes_visited: usize,
    /// Nodes returned as results.
    pub results_returned: usize,
    /// Edges traversed across all iterations.
    pub edges_traversed: usize,
}

/// ANN seed provider — given a query vector returns `(id, score)` pairs.
pub trait AnnSeedProvider: Send + Sync {
    fn find_seeds(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>>;
}

/// Graph neighbourhood provider — given a node id returns adjacent node ids.
pub trait GraphNeighbourProvider: Send + Sync {
    /// Returns `(neighbour_id, edge_type, direction)` for all edges from `node_id`.
    fn neighbours(
        &self,
        node_id: &str,
    ) -> Result<Vec<(String, Option<String>, TraversalDirection)>>;

    /// Optional: return the payload for a node (defaults to `null`).
    fn node_payload(&self, _node_id: &str) -> serde_json::Value {
        serde_json::Value::Null
    }
}

/// HybridTraverse executor.
pub struct HybridTraverseExecutor {
    /// Width of the ANN seed beam (number of initial seeds).
    pub beam_width: usize,
    /// Maximum number of results to return.
    pub top_k: usize,
}

impl HybridTraverseExecutor {
    pub fn new(beam_width: usize, top_k: usize) -> Self {
        Self { beam_width, top_k }
    }

    /// Execute the hybrid traversal and return `(results, stats)`.
    pub fn traverse(
        &self,
        query_vector: &[f32],
        edge_pattern: &EdgePattern,
        ann: &dyn AnnSeedProvider,
        graph: &dyn GraphNeighbourProvider,
    ) -> Result<(Vec<TraversalNode>, HybridTraverseStats)> {
        let mut stats = HybridTraverseStats::default();

        // Seed from ANN
        let seeds = ann.find_seeds(query_vector, self.beam_width)?;
        if seeds.is_empty() {
            return Ok((vec![], stats));
        }

        // `None` means unbounded by hop count; traversal still terminates when
        // the frontier is exhausted because visited nodes are de-duplicated.
        let max_hops = edge_pattern.max_hops.unwrap_or(u32::MAX);

        // Build score map so we can rank at the end
        let mut score_map: HashMap<String, f32> = HashMap::new();
        let mut depth_map: HashMap<String, u32> = HashMap::new();

        // Initialise frontier from seeds
        let mut frontier: Vec<String> = Vec::new();
        for (id, score) in &seeds {
            score_map.entry(id.clone()).or_insert(*score);
            depth_map.entry(id.clone()).or_insert(0);
            frontier.push(id.clone());
        }

        let mut visited: HashSet<String> = frontier.iter().cloned().collect();
        stats.nodes_visited = frontier.len();

        // Beam-search expansion
        for depth in 0..max_hops {
            if frontier.is_empty() {
                break;
            }
            stats.iterations += 1;
            let mut next_frontier = Vec::new();

            for node_id in &frontier {
                let neighbours = graph.neighbours(node_id)?;
                stats.edges_traversed += neighbours.len();

                for (nbr_id, edge_type, direction) in neighbours {
                    if visited.contains(&nbr_id) {
                        continue;
                    }
                    if !Self::matches_pattern(&edge_type, direction, edge_pattern) {
                        continue;
                    }
                    // Inherit parent score, decaying by hop
                    let parent_score = *score_map.get(node_id).unwrap_or(&0.0);
                    let decayed = parent_score * 0.9_f32.powi((depth + 1) as i32);
                    score_map.insert(nbr_id.clone(), decayed);
                    depth_map.insert(nbr_id.clone(), depth + 1);
                    visited.insert(nbr_id.clone());
                    next_frontier.push(nbr_id);
                }
            }

            stats.nodes_visited += next_frontier.len();
            frontier = next_frontier;
        }

        // Collect all visited nodes that satisfy min_hops
        let mut results: Vec<TraversalNode> = visited
            .into_iter()
            .filter_map(|id| {
                let hop = *depth_map.get(&id)?;
                if hop < edge_pattern.min_hops {
                    return None;
                }
                let score = *score_map.get(&id).unwrap_or(&0.0);
                let payload = graph.node_payload(&id);
                Some(TraversalNode {
                    id,
                    vector_score: score,
                    hop_depth: hop,
                    payload,
                })
            })
            .collect();

        // Rank by vector_score descending, then take top_k
        results.sort_by(|a, b| {
            b.vector_score
                .partial_cmp(&a.vector_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(self.top_k);

        stats.results_returned = results.len();
        Ok((results, stats))
    }

    fn matches_pattern(
        edge_type: &Option<String>,
        direction: TraversalDirection,
        pattern: &EdgePattern,
    ) -> bool {
        let type_ok = match &pattern.edge_type {
            None => true,
            Some(required) => edge_type.as_deref() == Some(required.as_str()),
        };
        let dir_ok = match pattern.direction {
            TraversalDirection::Both => true,
            TraversalDirection::Outgoing => {
                matches!(
                    direction,
                    TraversalDirection::Outgoing | TraversalDirection::Both
                )
            }
            TraversalDirection::Incoming => {
                matches!(
                    direction,
                    TraversalDirection::Incoming | TraversalDirection::Both
                )
            }
        };
        type_ok && dir_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── Test doubles ──────────────────────────────────────────────────────────

    struct FixedAnn {
        results: Vec<(String, f32)>,
    }
    impl AnnSeedProvider for FixedAnn {
        fn find_seeds(&self, _query: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
            Ok(self.results.iter().take(k).cloned().collect())
        }
    }

    struct SimpleGraph {
        // node → [(neighbour, edge_type)]
        edges: HashMap<String, Vec<(String, Option<String>)>>,
    }
    impl SimpleGraph {
        fn new(edges: Vec<(&str, &str, Option<&str>)>) -> Self {
            let mut map: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
            for (from, to, etype) in edges {
                map.entry(from.to_string())
                    .or_default()
                    .push((to.to_string(), etype.map(|s| s.to_string())));
            }
            Self { edges: map }
        }
    }
    impl GraphNeighbourProvider for SimpleGraph {
        fn neighbours(
            &self,
            node_id: &str,
        ) -> Result<Vec<(String, Option<String>, TraversalDirection)>> {
            Ok(self
                .edges
                .get(node_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(id, et)| (id, et, TraversalDirection::Outgoing))
                .collect())
        }
        fn node_payload(&self, node_id: &str) -> serde_json::Value {
            serde_json::json!({"id": node_id})
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_traverse_returns_seeds_when_no_graph_edges() {
        let ann = FixedAnn {
            results: vec![("v1".to_string(), 0.9), ("v2".to_string(), 0.7)],
        };
        let graph = SimpleGraph::new(vec![]);
        let exec = HybridTraverseExecutor::new(5, 10);
        let pattern = EdgePattern {
            min_hops: 0,
            ..EdgePattern::default()
        };
        let (results, stats) = exec.traverse(&[0.1, 0.2], &pattern, &ann, &graph).unwrap();

        // Seeds at hop 0 pass min_hops=0 filter
        assert_eq!(results.len(), 2, "both seeds returned");
        assert!(stats.edges_traversed == 0);
    }

    #[test]
    fn test_traverse_expands_one_hop() {
        let ann = FixedAnn {
            results: vec![("n1".to_string(), 1.0)],
        };
        // n1 → n2 → n3
        let graph = SimpleGraph::new(vec![("n1", "n2", None), ("n2", "n3", None)]);
        let exec = HybridTraverseExecutor::new(3, 10);
        let pattern = EdgePattern {
            min_hops: 0,
            max_hops: Some(1),
            ..EdgePattern::default()
        };
        let (results, stats) = exec.traverse(&[], &pattern, &ann, &graph).unwrap();

        let ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"n1"), "seed included");
        assert!(ids.contains(&"n2"), "1-hop neighbour included");
        assert!(
            !ids.contains(&"n3"),
            "2-hop neighbour excluded at max_hops=1"
        );
        assert!(stats.edges_traversed >= 1);
    }

    #[test]
    fn test_traverse_two_hops() {
        let ann = FixedAnn {
            results: vec![("root".to_string(), 1.0)],
        };
        let graph = SimpleGraph::new(vec![("root", "mid", None), ("mid", "leaf", None)]);
        let exec = HybridTraverseExecutor::new(3, 10);
        let pattern = EdgePattern {
            min_hops: 0,
            max_hops: Some(2),
            ..EdgePattern::default()
        };
        let (results, _stats) = exec.traverse(&[], &pattern, &ann, &graph).unwrap();

        let ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"root"));
        assert!(ids.contains(&"mid"));
        assert!(ids.contains(&"leaf"), "2-hop node reachable at max_hops=2");
    }

    #[test]
    fn test_traverse_unbounded_hops_until_frontier_exhausted() {
        let ann = FixedAnn {
            results: vec![("root".to_string(), 1.0)],
        };
        let graph = SimpleGraph::new(vec![
            ("root", "one", None),
            ("one", "two", None),
            ("two", "three", None),
        ]);
        let exec = HybridTraverseExecutor::new(3, 10);
        let pattern = EdgePattern {
            min_hops: 0,
            max_hops: None,
            ..EdgePattern::default()
        };
        let (results, _stats) = exec.traverse(&[], &pattern, &ann, &graph).unwrap();

        let ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"three"));
    }

    #[test]
    fn test_traverse_edge_type_filter() {
        let ann = FixedAnn {
            results: vec![("a".to_string(), 1.0)],
        };
        let graph = SimpleGraph::new(vec![("a", "b", Some("KNOWS")), ("a", "c", Some("LIKES"))]);
        let exec = HybridTraverseExecutor::new(3, 10);
        let pattern = EdgePattern {
            edge_type: Some("KNOWS".to_string()),
            min_hops: 0,
            max_hops: Some(1),
            direction: TraversalDirection::Outgoing,
        };
        let (results, _) = exec.traverse(&[], &pattern, &ann, &graph).unwrap();

        let ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"b"), "KNOWS edge followed");
        assert!(!ids.contains(&"c"), "LIKES edge filtered out");
    }

    #[test]
    fn test_traverse_no_revisit_cycles() {
        let ann = FixedAnn {
            results: vec![("x".to_string(), 1.0)],
        };
        // Cycle: x → y → x
        let graph = SimpleGraph::new(vec![("x", "y", None), ("y", "x", None)]);
        let exec = HybridTraverseExecutor::new(3, 10);
        let pattern = EdgePattern {
            min_hops: 0,
            max_hops: Some(3),
            ..EdgePattern::default()
        };
        let (results, _) = exec.traverse(&[], &pattern, &ann, &graph).unwrap();

        // Exactly 2 unique nodes
        let ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "cycle must not cause duplicates");
    }

    #[test]
    fn test_traverse_top_k_limit() {
        let ann = FixedAnn {
            results: (0..5)
                .map(|i| (format!("n{i}"), 1.0 - i as f32 * 0.1))
                .collect(),
        };
        let graph = SimpleGraph::new(vec![]);
        let exec = HybridTraverseExecutor::new(5, 3);
        let pattern = EdgePattern {
            min_hops: 0,
            ..EdgePattern::default()
        };
        let (results, _) = exec.traverse(&[], &pattern, &ann, &graph).unwrap();

        assert_eq!(results.len(), 3, "top_k=3 must cap results");
    }

    #[test]
    fn test_traverse_results_ranked_by_score_descending() {
        let ann = FixedAnn {
            results: vec![
                ("low".to_string(), 0.3),
                ("high".to_string(), 0.9),
                ("mid".to_string(), 0.6),
            ],
        };
        let graph = SimpleGraph::new(vec![]);
        let exec = HybridTraverseExecutor::new(5, 10);
        let pattern = EdgePattern {
            min_hops: 0,
            ..EdgePattern::default()
        };
        let (results, _) = exec.traverse(&[], &pattern, &ann, &graph).unwrap();

        let scores: Vec<f32> = results.iter().map(|r| r.vector_score).collect();
        for i in 1..scores.len() {
            assert!(
                scores[i - 1] >= scores[i],
                "results must be sorted descending"
            );
        }
    }

    #[test]
    fn test_traverse_empty_ann_returns_empty() {
        let ann = FixedAnn { results: vec![] };
        let graph = SimpleGraph::new(vec![]);
        let exec = HybridTraverseExecutor::new(5, 10);
        let (results, stats) = exec
            .traverse(&[], &EdgePattern::default(), &ann, &graph)
            .unwrap();

        assert!(results.is_empty());
        assert_eq!(stats.results_returned, 0);
    }

    #[test]
    fn test_traverse_hop_depth_recorded() {
        let ann = FixedAnn {
            results: vec![("root".to_string(), 1.0)],
        };
        let graph = SimpleGraph::new(vec![("root", "child", None)]);
        let exec = HybridTraverseExecutor::new(3, 10);
        let pattern = EdgePattern {
            min_hops: 0,
            max_hops: Some(1),
            ..EdgePattern::default()
        };
        let (results, _) = exec.traverse(&[], &pattern, &ann, &graph).unwrap();

        let root = results.iter().find(|r| r.id == "root").unwrap();
        let child = results.iter().find(|r| r.id == "child").unwrap();
        assert_eq!(root.hop_depth, 0);
        assert_eq!(child.hop_depth, 1);
    }
}
