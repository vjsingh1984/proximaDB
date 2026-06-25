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
use std::time::Duration;

use anyhow::{Context, Result, bail};
use proximadb_graph::record::{GraphEdgeKey, GraphNodeKey};
use proximadb_records::{ProximaRecord, RecordKey};

use crate::core::search::cross_modal_fusion::{
    FusedItem, Fuser, FusionPolicy, FusionStats, SourceCandidates, SourceId,
};
use crate::core::search::results::OptimizedSearchRecord;
use crate::graph::GraphOperationsService;
use crate::graph::model::{Edge, Node};
use crate::network::hybrid_search::HybridFullTextIndexMap;
use crate::services::VectorOperationsService;

/// T1.2: Retry with exponential backoff for transient vector search failures.
async fn retry_vector_search<F, Fut>(
    collection: &str,
    mut attempt_fn: F,
    max_retries: u32,
) -> Result<Vec<OptimizedSearchRecord>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<OptimizedSearchRecord>, anyhow::Error>>,
{
    let mut last_error_msg = String::new();
    for attempt in 0..=max_retries {
        match attempt_fn().await {
            Ok(result) => {
                if attempt > 0 {
                    tracing::info!(
                        collection,
                        attempts = attempt + 1,
                        "Vector search succeeded after retry"
                    );
                }
                return Ok(result);
            }
            Err(e) => {
                last_error_msg = e.to_string();
                if attempt < max_retries {
                    let delay = Duration::from_millis(100 * (2_u64.pow(attempt)));
                    tracing::warn!(
                        collection,
                        attempt,
                        delay_ms = delay.as_millis(),
                        error = %last_error_msg,
                        "Vector search failed, retrying with exponential backoff"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    bail!(
        "Vector search failed after {} attempts for collection '{}': {}",
        max_retries + 1,
        collection,
        last_error_msg
    )
}
use crate::storage::engines::core::formats::columnar::fulltext_index::FulltextSearchResult;

/// `TraversalAlgorithm::Bfs` proto tag.
const ALGORITHM_BFS: i32 = 1;

/// Kill-switch env var (TD-131 parity fallback). When set, [`FusionService::graph_fusion_search`]
/// skips PIT calibration and returns the raw union of seed + expand OIDs via [`raw_union`] — the OID
/// set is byte-identical to the calibrated `Fuser` output when `limit ≥ candidate count`; only scores
/// differ by design. Mirrors the repo `PROXIMADB_DISABLE_*` convention (e.g. `PROXIMADB_DISABLE_WAL`).
const DISABLE_CALIBRATION_ENV: &str = "PROXIMADB_DISABLE_GRAPH_FUSION_CALIBRATION";

fn fusion_calibration_disabled() -> bool {
    std::env::var_os(DISABLE_CALIBRATION_ENV).is_some()
}

/// RBAC predicate over a resolved backing record (TD-134). `None` ⇒ the record is not in the canonical
/// store (or the store is unwired), so within-tenant row RBAC cannot bite ⇒ **allow** (the tenant
/// boundary stays structural). The predicate is pure so the gate is unit-testable without engines.
pub(crate) fn record_is_accessible(record: Option<&ProximaRecord>, principal: &str) -> bool {
    match record {
        Some(rec) => rec.is_accessible_by(principal),
        None => true,
    }
}

/// Raw-union fallback for the kill-switch path (TD-131). Every `oid` from every source becomes a
/// [`FusedItem`] with a nominal score (the best raw score seen) and `source_count` = number of sources
/// that contained it. **The OID set is identical to the calibrated `Fuser` output when `limit ≥
/// candidate count** — calibration only re-ranks, it never invents or suppresses an `oid`. Ordering is
/// deterministic (descending score, then `oid`) so the fallback is stable.
pub(crate) fn raw_union(
    sources: Vec<SourceCandidates>,
    limit: usize,
) -> (Vec<FusedItem>, FusionStats) {
    let candidates_in: usize = sources.iter().map(|s| s.scores.len()).sum();
    let mut per_oid: HashMap<String, (f32, usize)> = HashMap::new();
    for source in &sources {
        for (oid, &score) in &source.scores {
            per_oid
                .entry(oid.clone())
                .and_modify(|(best, count)| {
                    if score > *best {
                        *best = score;
                    }
                    *count += 1;
                })
                .or_insert((score, 1));
        }
    }
    let mut items: Vec<FusedItem> = per_oid
        .into_iter()
        .map(|(oid, (score, count))| FusedItem {
            oid,
            score,
            per_source: HashMap::new(),
            source_count: count,
        })
        .collect();
    items.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.oid.cmp(&b.oid)));
    items.truncate(limit);
    let stats = FusionStats {
        sources_fused: sources.len(),
        sources_skipped: 0,
        candidates_in,
        items_out: items.len(),
    };
    (items, stats)
}

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
    /// Acting principal for within-tenant row-level RBAC (`permitted_principals`, TD-134). `None` ⇒
    /// structural isolation only (no per-record filtering) — the default for unauthenticated/embedded
    /// paths. When `Some`, BOTH the vector-seed and graph-expand legs drop any candidate whose backing
    /// record is not [`ProximaRecord::is_accessible_by`] the principal. The gate fails open when the
    /// canonical record store is unwired or a record cannot be resolved (within-tenant RBAC only bites
    /// on records carrying explicit `permitted_principals`); the tenant boundary stays structural.
    pub principal: Option<String>,
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
    ///
    /// TD-131 hardening:
    /// - **RBAC** (TD-134 seam): when `params.principal` is `Some`, both legs drop candidates whose
    ///   backing record is not `is_accessible_by(principal)`. Fails open when the canonical record
    ///   store is unwired or a record cannot be resolved.
    /// - **Kill-switch**: `PROXIMADB_DISABLE_GRAPH_FUSION_CALIBRATION` skips PIT calibration and
    ///   returns the raw union of OIDs (byte-identical OID set; scores differ by design).
    ///
    /// T1.2: Implements graceful degradation — if graph traversal fails for a seed, the search
    /// continues with the remaining seeds and the vector source alone. Only total failure (vector
    /// search exhausts retries) returns an error.
    pub async fn graph_fusion_search(
        &self,
        params: GraphFusionParams,
    ) -> Result<(Vec<FusedItem>, FusionStats)> {
        use std::time::Instant;

        let started = Instant::now();

        // 1. Vector ANN seed with T1.2 retry logic (max 2 retries, exponential backoff).
        let vector = self.vector.clone();
        let collection = params.vector_collection.clone();
        let query_vector = params.query_vector.clone();
        let limit = params.limit;

        let mut hits = retry_vector_search(
            &collection,
            || vector.unified_search_native(&collection, query_vector.clone(), limit, None, None),
            2,
        )
        .await
        .with_context(|| {
            format!(
                "vector search failed for fusion (collection='{}', graph_id='{}')",
                collection, params.graph_id
            )
        })?;

        // 1b. RBAC (leg 1): drop seed hits whose backing record the principal cannot access. The seed
        //     `id` is the canonical co-indexed `oid`, resolved via the graph service's canonical store.
        if let Some(principal) = params.principal.as_deref() {
            let mut accessible = Vec::with_capacity(hits.len());
            for hit in hits.drain(..) {
                if self.oid_accessible(&hit.id, principal).await {
                    accessible.push(hit);
                }
            }
            hits = accessible;
        }

        let vector_source = vector_hits_to_source(&hits, params.vector_weight);

        // 2. Graph expand from the top seeds (bounded). A seed that is not a node in this graph simply
        //    contributes nothing (its traverse errors are skipped) rather than failing the query.
        // T1.2: Graceful degradation — individual seed failures are logged but don't abort fusion.
        let seeds = seed_node_ids(&params.graph_id, &hits, params.max_seeds);
        let mut nodes: Vec<Node> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        let mut failed_seeds = 0;

        for seed in &seeds {
            let request = crate::graph::model::TraversalRequest {
                graph_id: params.graph_id.clone(),
                start_node_id: seed.clone(),
                max_depth: params.max_depth,
                edge_types: params.edge_types.clone(),
                node_labels: Vec::new(),
                filters: Vec::new(),
                algorithm: ALGORITHM_BFS,
                limit: Some(params.limit as u32),
                timeout_ms: None,
                max_frontier: None,
            };
            match self.graph.traverse(&params.graph_id, request).await {
                Ok(response) => {
                    nodes.extend(response.nodes);
                    edges.extend(response.edges);
                }
                Err(e) => {
                    failed_seeds += 1;
                    tracing::warn!(
                        graph_id = %params.graph_id,
                        seed,
                        error = %e,
                        "Graph traversal failed for seed in fusion, gracefully continuing"
                    );
                }
            }
        }

        if failed_seeds > 0 {
            tracing::info!(
                graph_id = %params.graph_id,
                failed_seeds,
                total_seeds = seeds.len(),
                "Partial graph traversal failure in fusion; continuing with vector source"
            );
        }

        // 2b. RBAC (leg 2): drop expanded nodes the principal cannot access, and any edge touching an
        //     inaccessible node (never leak a link to a hidden node). Node `oid` = canonical
        //     `graph/{graph_id}/node/{node_id}`, resolved via the graph service's canonical store.
        if let Some(principal) = params.principal.as_deref() {
            let gid = &params.graph_id;
            let mut kept_nodes = Vec::with_capacity(nodes.len());
            for n in nodes.drain(..) {
                let oid = GraphNodeKey::new(gid, n.id.clone()).canonical_oid();
                if self.oid_accessible(&oid, principal).await {
                    kept_nodes.push(n);
                }
            }
            nodes = kept_nodes;

            let mut kept_edges = Vec::with_capacity(edges.len());
            for e in edges.drain(..) {
                let from_oid = GraphNodeKey::new(gid, e.from_node_id.clone()).canonical_oid();
                let to_oid = GraphNodeKey::new(gid, e.to_node_id.clone()).canonical_oid();
                if self.oid_accessible(&from_oid, principal).await
                    && self.oid_accessible(&to_oid, principal).await
                {
                    kept_edges.push(e);
                }
            }
            edges = kept_edges;
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

        // 4. Calibrate + fuse-by-oid + rank, OR the raw-union kill-switch fallback (TD-131). The OID
        //    set is identical between the two when `limit ≥ candidate count` — calibration only re-ranks.
        let result = if fusion_calibration_disabled() {
            tracing::debug!(
                graph_id = %params.graph_id,
                "graph fusion calibration disabled (kill-switch) — raw-union fallback"
            );
            raw_union(sources, params.limit)
        } else {
            Fuser::new(params.policy).fuse(sources, params.limit)
        };

        // T1.1: Record fusion metrics for observability.
        let stats = &result.1;
        crate::metrics::fusion::record_fusion(
            stats.sources_fused,
            stats.sources_skipped,
            stats.candidates_in,
            stats.items_out,
            started.elapsed(),
        );

        Ok(result)
    }

    /// Resolve `oid` → backing `ProximaRecord` via the graph service's canonical store and apply the
    /// TD-134 `permitted_principals` predicate. Fails open when the store is unwired or the record
    /// cannot be resolved: within-tenant row RBAC only bites on records carrying explicit principals,
    /// so absence is open access (the tenant boundary stays structural).
    async fn oid_accessible(&self, oid: &str, principal: &str) -> bool {
        let Some(store) = self.graph.canonical_record_store() else {
            return true;
        };
        match store.get_record(&RecordKey::new(oid)).await {
            Ok(record) => record_is_accessible(record.as_ref(), principal),
            Err(error) => {
                tracing::warn!(oid, %error, "RBAC record resolve failed; failing open");
                true
            }
        }
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

    // ---- TD-131 RBAC predicate (pure — no engines required) ----

    fn record(principals: &[&str]) -> ProximaRecord {
        ProximaRecord {
            permitted_principals: principals.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn rbac_predicate_allows_open_record_and_denies_restricted() {
        let open = record(&[]);
        let restricted = record(&["alice"]);

        // Open record (empty permitted_principals) is accessible by anyone.
        assert!(record_is_accessible(Some(&open), "bob"));
        assert!(record_is_accessible(Some(&open), "alice"));

        // Restricted record: only a listed principal is admitted.
        assert!(record_is_accessible(Some(&restricted), "alice"));
        assert!(!record_is_accessible(Some(&restricted), "bob"));

        // Unresolved record (None) ⇒ allow — within-tenant RBAC can't bite, structural isolation
        // remains the tenant boundary.
        assert!(record_is_accessible(None, "bob"));
    }

    // ---- TD-131 kill-switch raw-union (pure — no engines required) ----

    #[test]
    fn raw_union_oid_set_matches_calibrated_fuser() {
        // The vector seed and a graph node share oid "graph/g/node/n1" (source_count 2); each source
        // also carries a unique oid. The fused and raw-union OID sets must be identical when `limit`
        // does not truncate.
        let shared = "graph/g/node/n1";
        let vector = vector_hits_to_source(&[hit(shared, 0.9), hit("graph/g/node/n2", 0.5)], 1.0);
        let graph = traversal_nodes_to_source("g", &[node("n1"), node("n3")], 1.0);
        let sources = vec![vector, graph];

        let (fused, _) = Fuser::new(FusionPolicy::default()).fuse(sources.clone(), 100);
        let (unioned, _) = raw_union(sources, 100);

        let fused_oids: std::collections::HashSet<&str> =
            fused.iter().map(|i| i.oid.as_str()).collect();
        let union_oids: std::collections::HashSet<&str> =
            unioned.iter().map(|i| i.oid.as_str()).collect();
        assert_eq!(
            fused_oids, union_oids,
            "kill-switch OID set must match the fused set"
        );
        for expected in ["graph/g/node/n1", "graph/g/node/n2", "graph/g/node/n3"] {
            assert!(union_oids.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn raw_union_counts_sources_and_truncates_to_limit() {
        let shared = "graph/g/node/n1";
        let vector = vector_hits_to_source(&[hit(shared, 0.9)], 1.0);
        let graph = traversal_nodes_to_source("g", &[node("n1")], 1.0);
        let (items, _) = raw_union(vec![vector, graph], 100);
        let fused = items
            .iter()
            .find(|i| i.oid == shared)
            .expect("shared oid present in raw union");
        assert_eq!(fused.source_count, 2, "oid in two sources ⇒ source_count 2");

        // Truncation honors `limit`.
        let (truncated, _) = raw_union(
            vec![vector_hits_to_source(
                &[hit("graph/g/node/a", 1.0), hit("graph/g/node/b", 0.5)],
                1.0,
            )],
            1,
        );
        assert_eq!(truncated.len(), 1, "limit truncates the raw union");
    }
}
