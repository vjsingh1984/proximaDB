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
//! A trait surface and orchestrator for the **three stages** that live in
//! the database layer:
//!
//! - [`NodeRetriever`] — covers the paper's *Node Retrieval* stage.
//! - [`NodeFilter`] — covers the paper's *Dynamic Node Filtering* stage (TD-045).
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
//! ```text
//!  RagQuery ──► NodeRetriever.retrieve() ──► Vec<NodeId>
//!                                              │
//!                          ┌───────────────────┴─────────┐
//!                          │  NodeFilter.filter()        │  <- the RGL filter
//!                          └───────────────────┬─────────┘
//!                                              ▼
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

pub mod engine_impls;
pub use engine_impls::{KHopSubgraphBuilder, VectorNodeRetriever};

#[cfg(test)]
mod engine_impls_test;

use proximadb_kernel::error::ProximaDBError;
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
        subgraph
            .edges
            .retain(|e| surviving.contains(&e.from) && surviving.contains(&e.to));
    }
}

/// Pluggable: select seed nodes from a query.
#[async_trait]
pub trait NodeRetriever: Send + Sync {
    /// Return seed node IDs in *importance order*.
    async fn retrieve(&self, query: &RagQuery) -> Result<Vec<NodeId>>;
}

/// Pluggable: expand a seed set into a subgraph.
#[async_trait]
pub trait SubgraphBuilder: Send + Sync {
    /// Build a subgraph from the supplied seed nodes.
    async fn build(&self, seeds: &[NodeId]) -> Result<Subgraph>;
}

/// Pluggable: filter seed nodes based on query context (TD-045).
#[async_trait]
pub trait NodeFilter: Send + Sync {
    /// Filter or re-rank seed nodes.
    async fn filter(&self, query: &RagQuery, seeds: Vec<NodeId>) -> Result<Vec<NodeId>>;
}

/// A filter that passes all nodes through unchanged.
pub struct IdentityFilter;

#[async_trait]
impl NodeFilter for IdentityFilter {
    async fn filter(&self, _query: &RagQuery, seeds: Vec<NodeId>) -> Result<Vec<NodeId>> {
        Ok(seeds)
    }
}

/// Compose a [`NodeRetriever`], [`NodeFilter`], and [`SubgraphBuilder`] into a
/// runnable pipeline, with budget-driven filtering.
pub struct RagPipeline<R, B, F = IdentityFilter>
where
    R: NodeRetriever,
    B: SubgraphBuilder,
    F: NodeFilter,
{
    retriever: R,
    builder: B,
    filter: F,
    budget: RagBudget,
}

impl<R, B, F> RagPipeline<R, B, F>
where
    R: NodeRetriever,
    B: SubgraphBuilder,
    F: NodeFilter,
{
    /// Compose a retriever, builder, and filter under a shared budget.
    pub fn new(retriever: R, builder: B, filter: F, budget: RagBudget) -> Self {
        Self {
            retriever,
            builder,
            filter,
            budget,
        }
    }

    /// Run the full pipeline: retrieve seeds, filter, prune, build.
    pub async fn run(&self, query: &RagQuery) -> Result<Subgraph> {
        let mut seeds = self.retriever.retrieve(query).await?;

        // Dynamic node filtering (TD-045 RGL paper)
        seeds = self.filter.filter(query, seeds).await?;

        self.budget.prune_seeds(&mut seeds);

        let mut subgraph = self.builder.build(&seeds).await?;
        self.budget.prune_subgraph(&mut subgraph);

        Ok(subgraph)
    }
}

// Special case for backward compatibility or when filter is omitted.
impl<R, B> RagPipeline<R, B, IdentityFilter>
where
    R: NodeRetriever,
    B: SubgraphBuilder,
{
    /// Convenience: compose without an explicit filter.
    pub fn without_filter(retriever: R, builder: B, budget: RagBudget) -> Self {
        Self::new(retriever, builder, IdentityFilter, budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn ids(values: &[&str]) -> Vec<NodeId> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn pipeline_composes_with_identity_filter() {
        let (retriever, _) = StaticRetriever::new(ids(&["a", "b"]));
        let (builder, _) = ObservingBuilder::new();
        let pipeline = RagPipeline::without_filter(retriever, builder, RagBudget::default());

        let result = pipeline.run(&RagQuery::text("hello")).await.unwrap();
        assert_eq!(result.node_count(), 4);
    }
}
