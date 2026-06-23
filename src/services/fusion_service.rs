//! Graph-modality fusion service — the first instance of the cross-modal fusion seam.
//!
//! Phase 1 (F-B / TD-137) of `docs/12-design/CROSS_MODAL_FUSION_SEAM_2026_06_22.adoc`. Wires the real
//! vector ANN seed + graph k-hop expand into the neutral [`Fuser`] core:
//!
//! ```text
//! vector ANN seed → graph k-hop expand (from the top seeds) → calibrate → fuse-by-oid → rank
//! ```
//!
//! The conversion adapters (`*_to_source`, `seed_node_ids`) are pure functions over the service result
//! types, so the correlation logic is unit-testable without standing up the engines. The graph node's
//! BFS visitation rank is a proximity proxy (closer = earlier) — only the *order* matters because the
//! Fuser's PIT calibration normalizes each source to its empirical CDF (D3). Correlation is by the
//! canonical `oid` (`graph/{graph_id}/node/{node_id}`), so a vector hit and a graph-expanded node that
//! are the same entity are the same `oid` and fuse without a join (D1).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use proximadb_graph::record::GraphNodeKey;

use crate::core::search::cross_modal_fusion::{
    FusedItem, Fuser, FusionPolicy, FusionStats, SourceCandidates, SourceId,
};
use crate::core::search::results::OptimizedSearchRecord;
use crate::graph::GraphOperationsService;
use crate::graph::model::Node;
use crate::services::VectorOperationsService;

/// `TraversalAlgorithm::Bfs` proto tag.
const ALGORITHM_BFS: i32 = 1;

/// Vector ANN hits → an `oid`-keyed vector source. The record `id` is the canonical `oid`.
pub(crate) fn vector_hits_to_source(
    hits: &[OptimizedSearchRecord],
    weight: f32,
) -> SourceCandidates {
    let scores = hits.iter().map(|hit| (hit.id.clone(), hit.score)).collect();
    SourceCandidates::new(SourceId::Vector, weight, scores)
}

/// Graph traversal nodes → an `oid`-keyed graph source. Raw score = `1/(rank+1)` from the node's BFS
/// visitation order (proximity proxy); a node reached more than once keeps its best (earliest) score.
/// `oid` is the canonical `graph/{graph_id}/node/{node_id}` so it merges with the vector source.
pub(crate) fn traversal_nodes_to_source(
    graph_id: &str,
    nodes: &[Node],
    weight: f32,
) -> SourceCandidates {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (rank, node) in nodes.iter().enumerate() {
        let oid = GraphNodeKey::new(graph_id, node.id.clone()).canonical_oid();
        let score = 1.0 / (rank as f32 + 1.0);
        scores
            .entry(oid)
            .and_modify(|existing| {
                if score > *existing {
                    *existing = score;
                }
            })
            .or_insert(score);
    }
    SourceCandidates::new(SourceId::Graph, weight, scores)
}

/// Recover the graph `node_id` to seed traversal from each vector hit. When the vector collection is
/// co-indexed with the graph the record `id` is the canonical `oid` (`graph/{graph_id}/node/{node_id}`),
/// so strip the prefix; otherwise fall back to the raw `id`. Bounded by `max_seeds` (D8 — conservative
/// expansion).
pub(crate) fn seed_node_ids(
    graph_id: &str,
    hits: &[OptimizedSearchRecord],
    max_seeds: usize,
) -> Vec<String> {
    let prefix = format!("graph/{graph_id}/node/");
    hits.iter()
        .take(max_seeds)
        .map(|hit| {
            hit.id
                .strip_prefix(&prefix)
                .map(str::to_string)
                .unwrap_or_else(|| hit.id.clone())
        })
        .collect()
}

/// Parameters for one graph-modality fusion query.
#[derive(Debug, Clone)]
pub struct GraphFusionParams {
    pub graph_id: String,
    pub vector_collection: String,
    pub query_vector: Vec<f32>,
    pub max_depth: u32,
    pub edge_types: Vec<String>,
    /// How many of the top vector seeds to expand from (bounded — D8).
    pub max_seeds: usize,
    pub limit: usize,
    pub vector_weight: f32,
    pub graph_weight: f32,
    pub policy: FusionPolicy,
}

/// Orchestrates the graph instance of the fusion seam over the live vector + graph engines.
pub struct FusionService {
    vector: Arc<VectorOperationsService>,
    graph: Arc<GraphOperationsService>,
}

impl FusionService {
    pub fn new(vector: Arc<VectorOperationsService>, graph: Arc<GraphOperationsService>) -> Self {
        Self { vector, graph }
    }

    /// Vector-seed → graph-expand → fuse-by-`oid`. Tenant isolation is structural (the collection and
    /// graph carry the tenant boundary), per the co-design mandate — never a post-fusion predicate.
    pub async fn graph_fusion_search(
        &self,
        params: GraphFusionParams,
    ) -> Result<(Vec<FusedItem>, FusionStats)> {
        // 1. Vector ANN seed.
        let hits = self
            .vector
            .unified_search_native(
                &params.vector_collection,
                params.query_vector.clone(),
                params.limit,
                None,
                None,
            )
            .await?;
        let vector_source = vector_hits_to_source(&hits, params.vector_weight);

        // 2. Graph expand from the top seeds (bounded). A seed that is not a node in this graph simply
        //    contributes nothing (its traverse errors are skipped) rather than failing the query.
        let seeds = seed_node_ids(&params.graph_id, &hits, params.max_seeds);
        let mut nodes: Vec<Node> = Vec::new();
        for seed in seeds {
            let request = crate::graph::model::TraversalRequest {
                graph_id: params.graph_id.clone(),
                start_node_id: seed,
                max_depth: params.max_depth,
                edge_types: params.edge_types.clone(),
                node_labels: Vec::new(),
                filters: Vec::new(),
                algorithm: ALGORITHM_BFS,
                limit: Some(params.limit as u32),
                timeout_ms: None,
                max_frontier: None,
            };
            if let Ok(response) = self.graph.traverse(&params.graph_id, request).await {
                nodes.extend(response.nodes);
            }
        }
        let graph_source = traversal_nodes_to_source(&params.graph_id, &nodes, params.graph_weight);

        // 3. Calibrate + fuse-by-oid + rank.
        let fuser = Fuser::new(params.policy);
        Ok(fuser.fuse(vec![vector_source, graph_source], params.limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, score: f32) -> OptimizedSearchRecord {
        OptimizedSearchRecord {
            id: id.to_string(),
            score,
            ..Default::default()
        }
    }

    fn node(id: &str) -> Node {
        Node {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn vector_hits_become_oid_keyed_source() {
        let hits = [hit("a", 0.9), hit("b", 0.7)];
        let source = vector_hits_to_source(&hits, 1.0);
        assert_eq!(source.source, SourceId::Vector);
        assert_eq!(source.scores.get("a"), Some(&0.9));
        assert_eq!(source.scores.get("b"), Some(&0.7));
    }

    #[test]
    fn traversal_nodes_use_canonical_oid_and_best_rank_score() {
        // n1 appears first (rank 0 → 1.0) and again later (rank 2 → 0.333) → keeps the best (1.0).
        let nodes = [node("n1"), node("n2"), node("n1")];
        let source = traversal_nodes_to_source("g1", &nodes, 1.0);
        assert_eq!(source.source, SourceId::Graph);
        // Canonical oid form, matching the vector/record spine.
        assert_eq!(source.scores.get("graph/g1/node/n1"), Some(&1.0));
        assert_eq!(source.scores.get("graph/g1/node/n2"), Some(&0.5));
        assert_eq!(source.scores.len(), 2, "n1 deduped to its best score");
    }

    #[test]
    fn seed_node_ids_strip_canonical_prefix_and_bound() {
        let hits = [
            hit("graph/g1/node/n1", 0.9),
            hit("graph/g1/node/n2", 0.8),
            hit("raw_id", 0.7), // not in canonical form → used as-is
        ];
        let seeds = seed_node_ids("g1", &hits, 2);
        assert_eq!(
            seeds,
            vec!["n1".to_string(), "n2".to_string()],
            "prefix stripped, bounded to 2"
        );
        let all = seed_node_ids("g1", &hits, 10);
        assert_eq!(all[2], "raw_id", "non-canonical id falls through");
    }

    /// The conversions feed the Fuser: a vector hit and a graph node that are the SAME entity (same
    /// canonical oid) fuse into one item with `source_count == 2`.
    #[test]
    fn vector_and_graph_sources_fuse_by_shared_oid() {
        let oid = "graph/g1/node/n1";
        let vector = vector_hits_to_source(&[hit(oid, 0.9)], 1.0);
        let graph = traversal_nodes_to_source("g1", &[node("n1")], 1.0);
        let (items, stats) = Fuser::new(FusionPolicy::default()).fuse(vec![vector, graph], 10);
        assert_eq!(stats.sources_fused, 2);
        let fused = items
            .iter()
            .find(|i| i.oid == oid)
            .expect("shared oid fused");
        assert_eq!(
            fused.source_count, 2,
            "vector + graph hit on the same oid merge"
        );
    }
}
