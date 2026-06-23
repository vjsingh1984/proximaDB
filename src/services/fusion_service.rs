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
use proximadb_graph::record::{GraphEdgeKey, GraphNodeKey};

use crate::core::search::cross_modal_fusion::{
    FusedItem, Fuser, FusionPolicy, FusionStats, SourceCandidates, SourceId,
};
use crate::core::search::results::OptimizedSearchRecord;
use crate::graph::GraphOperationsService;
use crate::graph::model::{Edge, Node};
use crate::network::hybrid_search::HybridFullTextIndexMap;
use crate::services::VectorOperationsService;
use crate::storage::engines::core::formats::columnar::fulltext_index::FulltextSearchResult;

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

/// Graph traversal **edges** → an `oid`-keyed graph source at *edge grain* (the relationship is the
/// fusion unit). `oid` is the canonical `graph/{graph_id}/edge/{edge_id}` — distinct from node `oid`s,
/// so edges rank as their own items. Score = `1/(rank+1)` from traversal order; an edge seen more than
/// once keeps its best score. HELIOS: edge-grain fusion carries **6–12% more signal** than node-grain
/// (D8) — offered as an opt-in grain, with node-grain the default sweet spot.
pub(crate) fn traversal_edges_to_source(
    graph_id: &str,
    edges: &[Edge],
    weight: f32,
) -> SourceCandidates {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (rank, edge) in edges.iter().enumerate() {
        let oid = GraphEdgeKey::new(graph_id, edge.id.clone()).canonical_oid();
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

/// BM25 / full-text hits → an `oid`-keyed document source. The hit `doc_id` is the record `oid`, so it
/// merges with the vector and graph sources by shared `oid`. The BM25 score is `f64`, narrowed to `f32`
/// for the blend — harmless because PIT calibration is rank-relative within the source.
pub(crate) fn bm25_hits_to_source(hits: &[FulltextSearchResult], weight: f32) -> SourceCandidates {
    let scores = hits
        .iter()
        .map(|hit| (hit.doc_id.clone(), hit.score as f32))
        .collect();
    SourceCandidates::new(SourceId::Document, weight, scores)
}

/// The document modality expander: runs BM25/full-text search over a collection's index and emits an
/// `oid`-keyed [`SourceId::Document`] source. A collection without a materialized full-text index (or a
/// poisoned lock) **fails closed** — it returns an empty source so fusion proceeds on the other
/// modalities rather than erroring (D5/D6). This converges the document half of the legacy 12-strategy
/// `HybridFusionEngine` onto the neutral seam (TD-138).
pub struct DocumentExpander {
    indexes: HybridFullTextIndexMap,
}

impl DocumentExpander {
    pub fn new(indexes: HybridFullTextIndexMap) -> Self {
        Self { indexes }
    }

    /// BM25-search `collection` for `text_query` (top `k`) → an `oid`-keyed document source.
    pub fn expand(
        &self,
        collection: &str,
        text_query: &str,
        k: usize,
        weight: f32,
    ) -> SourceCandidates {
        let hits = self
            .indexes
            .read()
            .ok()
            .and_then(|guard| {
                guard
                    .get(collection)
                    .map(|index| index.search(text_query, k))
            })
            .unwrap_or_default();
        bm25_hits_to_source(&hits, weight)
    }
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

/// Whether graph expansion contributes node candidates, edge (relationship) candidates, or both.
/// Edge-grain carries more relational signal (HELIOS: +6–12% over node-grain); node-grain is the
/// default sweet spot (D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphGrain {
    #[default]
    Nodes,
    Edges,
    Both,
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
    /// Node / edge / both grain for the graph contribution (D8).
    pub grain: GraphGrain,
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
        let mut edges: Vec<Edge> = Vec::new();
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
                edges.extend(response.edges);
            }
        }

        // 3. Build the graph contribution at the requested grain (D8: edge-grain carries more
        //    relational signal; node-grain is the default). Node and edge `oid`s are disjoint, so
        //    `Both` simply adds two graph sources.
        let mut sources = vec![vector_source];
        if matches!(params.grain, GraphGrain::Nodes | GraphGrain::Both) {
            sources.push(traversal_nodes_to_source(
                &params.graph_id,
                &nodes,
                params.graph_weight,
            ));
        }
        if matches!(params.grain, GraphGrain::Edges | GraphGrain::Both) {
            sources.push(traversal_edges_to_source(
                &params.graph_id,
                &edges,
                params.graph_weight,
            ));
        }

        // 4. Calibrate + fuse-by-oid + rank.
        let fuser = Fuser::new(params.policy);
        Ok(fuser.fuse(sources, params.limit))
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

    fn doc_hit(doc_id: &str, score: f64) -> FulltextSearchResult {
        FulltextSearchResult {
            doc_id: doc_id.to_string(),
            score,
            matched_terms: Vec::new(),
            term_frequencies: HashMap::new(),
            highlight_positions: HashMap::new(),
        }
    }

    #[test]
    fn bm25_hits_become_oid_keyed_document_source() {
        use crate::storage::engines::core::formats::columnar::fulltext_index::{
            FullTextIndex, TokenizerConfig,
        };
        use std::sync::RwLock;

        let mut index = FullTextIndex::new(TokenizerConfig::for_keyword_search());
        index
            .add_document("d1", "machine learning model")
            .expect("add d1");
        index
            .add_document("d2", "deep neural network")
            .expect("add d2");
        index
            .add_document("d3", "machine learning algorithm")
            .expect("add d3");

        let indexes: HybridFullTextIndexMap =
            Arc::new(RwLock::new(HashMap::from([("col".to_string(), index)])));
        let source = DocumentExpander::new(indexes).expand("col", "machine learning", 10, 1.0);

        assert_eq!(source.source, SourceId::Document);
        assert!(source.scores.contains_key("d1"), "d1 matches");
        assert!(source.scores.contains_key("d3"), "d3 matches");
        assert!(!source.scores.contains_key("d2"), "d2 does not match");
    }

    #[test]
    fn document_expander_fails_closed_on_missing_index() {
        use std::sync::RwLock;
        let indexes: HybridFullTextIndexMap = Arc::new(RwLock::new(HashMap::new()));
        let source = DocumentExpander::new(indexes).expand("absent", "anything", 10, 1.0);
        assert!(
            source.scores.is_empty(),
            "missing index → empty document source (fail-closed)"
        );
    }

    /// A vector hit and a BM25 document hit on the SAME `oid` fuse into one item (`source_count == 2`):
    /// the document modality converges onto the seam by shared `oid`, calibrated by the Fuser.
    #[test]
    fn vector_and_document_fuse_by_shared_oid() {
        let oid = "rec1";
        let vector = vector_hits_to_source(&[hit(oid, 0.9)], 1.0);
        let document = bm25_hits_to_source(&[doc_hit(oid, 3.2)], 1.0);
        let (items, stats) = Fuser::new(FusionPolicy::default()).fuse(vec![vector, document], 10);
        assert_eq!(stats.sources_fused, 2);
        let fused = items
            .iter()
            .find(|i| i.oid == oid)
            .expect("shared oid fused");
        assert_eq!(fused.source_count, 2, "vector + document merge by oid");
    }

    fn edge(id: &str) -> Edge {
        Edge {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn traversal_edges_use_canonical_edge_oid_and_best_rank() {
        let edges = [edge("e1"), edge("e2"), edge("e1")];
        let source = traversal_edges_to_source("g1", &edges, 1.0);
        assert_eq!(source.source, SourceId::Graph);
        // Canonical edge oid form (distinct from node oids); e1 keeps its best (rank 0) score.
        assert_eq!(source.scores.get("graph/g1/edge/e1"), Some(&1.0));
        assert_eq!(source.scores.get("graph/g1/edge/e2"), Some(&0.5));
        assert_eq!(source.scores.len(), 2, "e1 deduped to its best score");
    }

    /// Node-grain and edge-grain contributions occupy disjoint `oid` spaces, so under `GraphGrain::Both`
    /// a node and an edge coexist as distinct fused items (HELIOS edge-grain alongside node-grain).
    #[test]
    fn node_and_edge_oids_are_disjoint_and_coexist() {
        let node_src = traversal_nodes_to_source("g1", &[node("n1")], 1.0);
        let edge_src = traversal_edges_to_source("g1", &[edge("e1")], 1.0);
        let (items, _) = Fuser::new(FusionPolicy::default()).fuse(vec![node_src, edge_src], 10);
        let oids: std::collections::HashSet<&str> = items.iter().map(|i| i.oid.as_str()).collect();
        assert!(oids.contains("graph/g1/node/n1"));
        assert!(oids.contains("graph/g1/edge/e1"));
        assert_eq!(items.len(), 2, "node and edge are distinct fused items");
    }
}
