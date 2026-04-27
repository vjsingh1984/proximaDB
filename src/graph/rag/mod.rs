//! Modular Graph RAG (TD-045, arXiv:2503.19314 RGL).
//!
//! ## Paper attribution (corrected April 26, 2026 from a deeper read)
//!
//! The RGL paper proposes a 5-stage pipeline — Indexing → Node Retrieval
//! → **Graph Retrieval** → Tokenization → Generation — wrapped in a
//! Python library with four architectural layers (Kernel, Runtime, API,
//! Applications). The paper's headline **143× speedup** is on the
//! *graph retrieval phase only* (10K queries on OGBN-Arxiv complete in
//! <5 minutes vs. NetworkX's 11 hours) and is achieved by **C++
//! efficient implementations and batching of the underlying graph
//! algorithms**, NOT primarily by dynamic filtering. "Dynamic node
//! filtering" in the paper is a *token-budget* lever — it reduces what
//! the downstream LLM sees, not what the database computes.
//!
//! ## What this module ships
//!
//! A trait surface and orchestrator for the **two stages** that live in
//! the database layer:
//!
//! - [`NodeRetriever`] — covers the paper's *Node Retrieval* stage.
//! - [`SubgraphBuilder`] — covers the paper's *Graph Retrieval* stage.
//! - [`RagPipeline`] composes them with a [`RagBudget`] enforced at
//!   both boundaries (the token-budget lever, plus a guard against
//!   over-expanding builders).
//!
//! Stages 1 (Indexing) and 4–5 (Tokenization, Generation) are out of
//! DB scope. Stage 1 is satisfied by ProximaDB's existing graph
//! engines (ORION etc.); stages 4–5 are LLM-side concerns.
//!
//! ## Latency expectations
//!
//! ProximaDB's graph engines are already in Rust with the same
//! engineering tradeoffs the paper used to beat NetworkX. The lift
//! callers should expect from this module is **not** the paper's 143×
//! speedup vs NetworkX — it is the *modularity* benefit (compose
//! retrievers and builders independently) and the *token-budget*
//! benefit (LLM context shrinks predictably).
//!
//! ## Architecture
//!
//! ## Architecture
//!
//! ```text
//!  RagQuery ──► NodeRetriever.retrieve() ──► Vec<NodeId>
//!                                              │
//!                          ┌───────────────────┴─────────┐
//!                          │  RagBudget.prune_seeds()    │  <- the lever
//!                          └───────────────────┬─────────┘
//!                                              ▼
//!                                      SubgraphBuilder.build() ──► Subgraph
//!                                              │
//!                          ┌───────────────────┴─────────┐
//!                          │  RagBudget.prune_subgraph() │  <- second lever
//!                          └───────────────────┬─────────┘
//!                                              ▼
//!                                          Subgraph
//! ```
//!
//! ## Why a free `Subgraph` type instead of `TraversalResults`?
//!
//! [`TraversalResults`](crate::graph::canonical::TraversalResults) is
//! ProximaDB's wire-shape canonical type, populated by
//! `service_traversal_api`. The RAG layer needs a leaner internal
//! representation that doesn't carry the wire-stats payload, so the
//! local [`Subgraph`] is intentionally minimal: nodes + edges, no
//! optional path traces, no execution stats. Convert at the API
//! boundary, not inside the pipeline.
//!
//! ## Status
//!
//! Trait surface, orchestrator, and budget-driven filter ship in this
//! module. Real engine-backed impls of [`NodeRetriever`] (vector,
//! BM25) and [`SubgraphBuilder`] (k-hop, Personalized PageRank) layer
//! on top in follow-up commits — see TD-045 in
//! `docs/10-quality/TECHNICAL_DEBT.adoc`.

use crate::core::error::ProximaDBError;
use crate::graph::NodeId;
use async_trait::async_trait;
use std::collections::HashSet;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// One edge in a [`Subgraph`]. Minimal shape: just enough to
/// reconstruct the relationship without dragging in the full proto
/// `Edge` payload, which carries fields the RAG pipeline does not need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubgraphEdge {
    /// Source node ID.
    pub from: NodeId,
    /// Target node ID.
    pub to: NodeId,
    /// Edge type/label.
    pub edge_type: String,
}

/// Minimal subgraph representation produced by a [`SubgraphBuilder`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subgraph {
    /// Nodes in the subgraph, in the order produced by the builder.
    pub nodes: Vec<NodeId>,
    /// Edges in the subgraph.
    pub edges: Vec<SubgraphEdge>,
}

impl Subgraph {
    /// Number of nodes — convenient for budget checks.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// Input to a [`RagPipeline`] run.
#[derive(Debug, Clone)]
pub struct RagQuery {
    /// The natural-language or programmatic query the agent issued.
    /// Retrievers interpret this string; the orchestrator does not.
    pub query: String,
    /// Optional vector embedding of the query, for vector retrievers.
    pub query_vector: Option<Vec<f32>>,
    /// Optional set of allowed entry labels — useful for graphs with
    /// label-typed entry points.
    pub allowed_labels: Vec<String>,
}

impl RagQuery {
    /// Construct a text-only query. The most common shape for a first
    /// iteration; downstream retrievers ignore fields they don't use.
    pub fn text<S: Into<String>>(query: S) -> Self {
        Self {
            query: query.into(),
            query_vector: None,
            allowed_labels: Vec::new(),
        }
    }
}

/// Token / size budget enforced at the retriever → builder boundary
/// and again on the resulting subgraph. The paper's headline 143×
/// speedup comes from pruning early — set `max_seeds` aggressively.
#[derive(Debug, Clone, Copy)]
pub struct RagBudget {
    /// Maximum number of seed nodes the retriever may produce. The
    /// pipeline truncates the retriever's output to this length before
    /// invoking the builder.
    pub max_seeds: usize,
    /// Maximum number of nodes in the final subgraph. The pipeline
    /// truncates the builder's output to this length. Edges are kept
    /// only when both endpoints survive the truncation.
    pub max_subgraph_nodes: usize,
}

impl Default for RagBudget {
    fn default() -> Self {
        Self {
            max_seeds: 10,
            max_subgraph_nodes: 100,
        }
    }
}

impl RagBudget {
    /// Truncate seeds in-place to the budget cap. Order is preserved
    /// so retrievers can encode importance via position.
    fn prune_seeds(&self, seeds: &mut Vec<NodeId>) {
        if seeds.len() > self.max_seeds {
            seeds.truncate(self.max_seeds);
        }
    }

    /// Truncate the subgraph in-place to the budget cap. Nodes are
    /// truncated first; edges whose endpoints fell outside the cap are
    /// dropped to keep the subgraph internally consistent (no
    /// dangling references).
    fn prune_subgraph(&self, subgraph: &mut Subgraph) {
        if subgraph.nodes.len() > self.max_subgraph_nodes {
            subgraph.nodes.truncate(self.max_subgraph_nodes);
        }
        let surviving: HashSet<&NodeId> = subgraph.nodes.iter().collect();
        subgraph.edges.retain(|e| {
            surviving.contains(&e.from) && surviving.contains(&e.to)
        });
    }
}

/// Pluggable: select seed nodes from a query.
///
/// Implementations are typically thin: a vector retriever calls AXIS,
/// a BM25 retriever calls the document store's full-text index, a
/// hybrid retriever wraps both and merges. The trait deliberately
/// hides the source — the pipeline doesn't care whether seeds come
/// from a vector index, a label scan, or a precomputed table.
#[async_trait]
pub trait NodeRetriever: Send + Sync {
    /// Return seed node IDs in *importance order* (most important
    /// first). The pipeline truncates this list to the configured
    /// `max_seeds` budget before invoking the [`SubgraphBuilder`], so
    /// implementations should not internally cap the list to small
    /// numbers — let the budget govern.
    async fn retrieve(&self, query: &RagQuery) -> Result<Vec<NodeId>>;
}

/// Pluggable: expand a seed set into a subgraph.
///
/// Common implementations include k-hop BFS, Personalized PageRank
/// (PPR), and Steiner tree extraction. The trait is async because
/// expansion typically dispatches to a graph engine.
#[async_trait]
pub trait SubgraphBuilder: Send + Sync {
    /// Build a subgraph from the supplied seed nodes. The pipeline
    /// has already enforced the seed budget, so implementations can
    /// expand each seed without re-checking caps.
    async fn build(&self, seeds: &[NodeId]) -> Result<Subgraph>;
}

/// Compose a [`NodeRetriever`] and a [`SubgraphBuilder`] into a
/// runnable pipeline, with budget-driven filtering at both boundaries.
///
/// Generic over the trait impls so callers can swap retriever and
/// builder independently — the paper's key abstraction.
pub struct RagPipeline<R: NodeRetriever, B: SubgraphBuilder> {
    retriever: R,
    builder: B,
    budget: RagBudget,
}

impl<R: NodeRetriever, B: SubgraphBuilder> RagPipeline<R, B> {
    /// Compose a retriever and a builder under a shared budget.
    pub fn new(retriever: R, builder: B, budget: RagBudget) -> Self {
        Self {
            retriever,
            builder,
            budget,
        }
    }

    /// Convenience: compose under [`RagBudget::default()`].
    pub fn with_default_budget(retriever: R, builder: B) -> Self {
        Self::new(retriever, builder, RagBudget::default())
    }

    /// Borrow the configured budget — useful for telemetry and tests.
    pub fn budget(&self) -> &RagBudget {
        &self.budget
    }

    /// Run the full pipeline: retrieve seeds, prune to budget, build
    /// subgraph, prune to budget. Both prunes are no-ops when the
    /// upstream output already fits.
    pub async fn run(&self, query: &RagQuery) -> Result<Subgraph> {
        let mut seeds = self.retriever.retrieve(query).await?;
        self.budget.prune_seeds(&mut seeds);

        let mut subgraph = self.builder.build(&seeds).await?;
        self.budget.prune_subgraph(&mut subgraph);

        Ok(subgraph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- mock impls ----------------------------------------------------
    //
    // These mocks let the tests verify pipeline behavior without standing
    // up a real graph engine. They also record call counts so we can
    // pin down "the budget is enforced *before* the builder runs" --
    // critical because the paper attributes the speedup to early
    // pruning, not late pruning.

    struct StaticRetriever {
        seeds: Vec<NodeId>,
        calls: Arc<AtomicUsize>,
    }

    impl StaticRetriever {
        fn new(seeds: Vec<NodeId>) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    seeds,
                    calls: calls.clone(),
                },
                calls,
            )
        }
    }

    #[async_trait]
    impl NodeRetriever for StaticRetriever {
        async fn retrieve(&self, _query: &RagQuery) -> Result<Vec<NodeId>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.seeds.clone())
        }
    }

    /// A builder that records every seed it received and returns a
    /// 1-hop fan-out subgraph. We can inspect its received_seeds()
    /// after a pipeline run to verify what the budget pruned.
    struct ObservingBuilder {
        seen_seeds: Arc<parking_lot::Mutex<Vec<NodeId>>>,
    }

    impl ObservingBuilder {
        fn new() -> (Self, Arc<parking_lot::Mutex<Vec<NodeId>>>) {
            let seen = Arc::new(parking_lot::Mutex::new(Vec::<NodeId>::new()));
            (
                Self {
                    seen_seeds: seen.clone(),
                },
                seen,
            )
        }
    }

    #[async_trait]
    impl SubgraphBuilder for ObservingBuilder {
        async fn build(&self, seeds: &[NodeId]) -> Result<Subgraph> {
            *self.seen_seeds.lock() = seeds.to_vec();
            // Synthesize a subgraph: each seed gets one synthetic
            // "child_<seed>" neighbor connected by an "X" edge.
            let mut nodes: Vec<NodeId> = seeds.to_vec();
            let mut edges = Vec::new();
            for seed in seeds {
                let child = format!("child_{}", seed);
                edges.push(SubgraphEdge {
                    from: seed.clone(),
                    to: child.clone(),
                    edge_type: "X".to_string(),
                });
                nodes.push(child);
            }
            Ok(Subgraph { nodes, edges })
        }
    }

    /// Builder that always errors. Used to prove the pipeline
    /// propagates errors without panicking.
    struct FailingBuilder;

    #[async_trait]
    impl SubgraphBuilder for FailingBuilder {
        async fn build(&self, _seeds: &[NodeId]) -> Result<Subgraph> {
            Err(ProximaDBError::InvalidInput("synthetic builder failure".into()))
        }
    }

    // ---- tests ---------------------------------------------------------

    fn ids(values: &[&str]) -> Vec<NodeId> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn pipeline_composes_retriever_and_builder() {
        let (retriever, retriever_calls) =
            StaticRetriever::new(ids(&["a", "b"]));
        let (builder, builder_seen) = ObservingBuilder::new();
        let pipeline =
            RagPipeline::with_default_budget(retriever, builder);

        let result = pipeline.run(&RagQuery::text("hello")).await.unwrap();

        assert_eq!(retriever_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*builder_seen.lock(), ids(&["a", "b"]));
        // ObservingBuilder fan-out: 2 seeds -> 4 nodes, 2 edges.
        assert_eq!(result.node_count(), 4);
        assert_eq!(result.edges.len(), 2);
    }

    #[tokio::test]
    async fn budget_prunes_seeds_BEFORE_builder_runs() {
        // The paper's headline lever: trim the seed set BEFORE
        // expanding into a subgraph, not after. This test pins that
        // ordering by inspecting what the builder actually received.
        let (retriever, _) =
            StaticRetriever::new(ids(&["a", "b", "c", "d", "e"]));
        let (builder, builder_seen) = ObservingBuilder::new();
        let budget = RagBudget {
            max_seeds: 2,
            max_subgraph_nodes: 100,
        };
        let pipeline = RagPipeline::new(retriever, builder, budget);

        let result = pipeline.run(&RagQuery::text("any")).await.unwrap();

        // Builder only saw the first 2 seeds (importance order).
        assert_eq!(*builder_seen.lock(), ids(&["a", "b"]));
        // 2 seeds -> 4 fan-out nodes -> well under the subgraph cap.
        assert_eq!(result.node_count(), 4);
    }

    #[tokio::test]
    async fn budget_prunes_subgraph_when_builder_exceeds_cap() {
        // Builder produces seeds + child_<seed> -> 6 nodes total. With
        // a 3-node cap, the pipeline should keep the first 3 in the
        // builder's output order and drop edges that reference dropped
        // endpoints (no dangling refs).
        let (retriever, _) =
            StaticRetriever::new(ids(&["a", "b", "c"]));
        let (builder, _) = ObservingBuilder::new();
        let budget = RagBudget {
            max_seeds: 10,
            max_subgraph_nodes: 3,
        };
        let pipeline = RagPipeline::new(retriever, builder, budget);

        let result = pipeline.run(&RagQuery::text("any")).await.unwrap();
        assert_eq!(result.node_count(), 3);

        // Every edge in the result must reference only surviving nodes.
        let surviving: HashSet<_> = result.nodes.iter().collect();
        for edge in &result.edges {
            assert!(
                surviving.contains(&edge.from),
                "edge.from {} dangling after prune",
                edge.from
            );
            assert!(
                surviving.contains(&edge.to),
                "edge.to {} dangling after prune",
                edge.to
            );
        }
    }

    #[tokio::test]
    async fn budget_is_noop_when_outputs_already_fit() {
        let (retriever, _) =
            StaticRetriever::new(ids(&["a", "b"]));
        let (builder, builder_seen) = ObservingBuilder::new();
        let budget = RagBudget {
            max_seeds: 100,
            max_subgraph_nodes: 100,
        };
        let pipeline = RagPipeline::new(retriever, builder, budget);

        let result = pipeline.run(&RagQuery::text("any")).await.unwrap();
        assert_eq!(*builder_seen.lock(), ids(&["a", "b"]));
        assert_eq!(result.node_count(), 4); // 2 seeds + 2 fan-out children.
        assert_eq!(result.edges.len(), 2);
    }

    #[tokio::test]
    async fn pipeline_propagates_builder_errors() {
        let (retriever, _) = StaticRetriever::new(ids(&["a"]));
        let pipeline =
            RagPipeline::with_default_budget(retriever, FailingBuilder);

        let err = pipeline.run(&RagQuery::text("any")).await.unwrap_err();
        assert!(matches!(err, ProximaDBError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn empty_seeds_produce_empty_subgraph() {
        // Edge case: retriever returns no seeds. The builder should
        // still be invoked (no special-casing — keeps the contract
        // simple) and the result should be empty.
        let (retriever, _) = StaticRetriever::new(vec![]);
        let (builder, builder_seen) = ObservingBuilder::new();
        let pipeline =
            RagPipeline::with_default_budget(retriever, builder);

        let result = pipeline.run(&RagQuery::text("any")).await.unwrap();
        assert_eq!(*builder_seen.lock(), Vec::<NodeId>::new());
        assert_eq!(result.node_count(), 0);
        assert!(result.edges.is_empty());
    }

    // ---- focused unit tests on RagBudget itself ------------------------

    #[test]
    fn prune_seeds_preserves_order_and_truncates() {
        let budget = RagBudget {
            max_seeds: 2,
            max_subgraph_nodes: 100,
        };
        let mut seeds = ids(&["a", "b", "c", "d"]);
        budget.prune_seeds(&mut seeds);
        assert_eq!(seeds, ids(&["a", "b"]));
    }

    #[test]
    fn prune_subgraph_drops_edges_with_dropped_endpoints() {
        let budget = RagBudget {
            max_seeds: 100,
            max_subgraph_nodes: 2,
        };
        let mut sg = Subgraph {
            nodes: ids(&["a", "b", "c"]),
            edges: vec![
                SubgraphEdge {
                    from: "a".into(),
                    to: "b".into(),
                    edge_type: "X".into(),
                },
                // This edge references c (will be pruned) -> drop.
                SubgraphEdge {
                    from: "a".into(),
                    to: "c".into(),
                    edge_type: "X".into(),
                },
                // Also dangling on the other side.
                SubgraphEdge {
                    from: "c".into(),
                    to: "b".into(),
                    edge_type: "X".into(),
                },
            ],
        };
        budget.prune_subgraph(&mut sg);
        assert_eq!(sg.nodes, ids(&["a", "b"]));
        assert_eq!(sg.edges.len(), 1);
        assert_eq!(sg.edges[0].from, "a");
        assert_eq!(sg.edges[0].to, "b");
    }

    #[test]
    fn rag_query_text_constructor() {
        let q = RagQuery::text("find authors who wrote about graph rag");
        assert_eq!(q.query, "find authors who wrote about graph rag");
        assert!(q.query_vector.is_none());
        assert!(q.allowed_labels.is_empty());
    }
}
