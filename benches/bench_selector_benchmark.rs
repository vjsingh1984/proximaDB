//! Calibrated open-weight selector benchmark (TD-SELECTOR-1).
//!
//! Motivation: PR #1726 moved candidate generation from Recall@100 0.3472 to a
//! 0.3734 pool ceiling, but the MiniLM selector delivered only 0.3136 — i.e. the
//! *selector*, not candidate coverage, is now the binding constraint. This harness
//! makes selector strategies comparable under a controlled protocol.
//!
//! # Protocol
//!
//! 1. **Candidate-pool preservation.** Every selector is scored against the SAME
//!    pre-computed pool (`vector_results`, `bm25_results`, and their `union_pool`).
//!    The measured delta is therefore attributable to the selector alone, never to
//!    a difference in generation.
//! 2. **Paired statistics.** Selectors are evaluated on identical queries, so
//!    comparisons are paired: the p-value comes from a sign-flip permutation test
//!    over per-query differences, and intervals are percentile bootstrap — not a
//!    normal approximation and not a placeholder constant.
//! 3. **Determinism.** All randomness is seeded (`SmallRng::seed_from_u64`); the
//!    same inputs produce the same report.
//!
//! # Scope / honesty boundary
//!
//! This harness runs on *synthetic* pools (`selector_benchmark_data`). It measures
//! the mechanism ("does this selector shape lose recall the pool already had?"),
//! not production quality. No number produced here is admissible in
//! `BENCHMARK_EVIDENCE.toml`. `SelectorStrategy::CrossEncoderRerank` is an
//! explicitly-parameterised *model* of a reranker (finite rerank window + imperfect
//! scoring), not an ONNX invocation — wiring `proximadb-rank-onnx` is Gate 5.
//!
//! Note on reuse: `benches/recall_utils.rs` computes recall for ANN ground truth
//! derived from exact distances (`GroundTruthResult`). This harness works on IR
//! relevance *sets* (a doc is relevant or not, independent of distance), so the
//! two do not share a ground-truth type.

// This is a benchmark harness: its data model is a stable surface for callers
// (integration tests, ad-hoc sweeps), and Rust's dead-code pass only sees whichever
// entry point compiled it. Structural dead code is removed rather than silenced.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::{Result, anyhow};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

/// Cutoffs reported by every selector run.
pub const RECALL_KS: [usize; 6] = [1, 5, 10, 20, 50, 100];

/// Deterministic seed for bootstrap / permutation resampling.
/// Named metric column, kept out of line so the comparison table stays readable.
type MetricFn<T> = (&'static str, fn(&T) -> f64);

const STATS_SEED: u64 = 0x5E1E_C704_0000_0001;

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// Ground-truth relevance set for one query.
#[derive(Debug, Clone)]
pub struct GroundTruth {
    pub query_id: String,
    /// Relevant doc ids, most-relevant first (order drives graded NDCG gains).
    pub relevant_docs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub query_id: String,
    pub query_text: String,
    pub query_vector: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultSource {
    Vector,
    BM25,
    Hybrid,
    Reranked,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub doc_id: String,
    pub score: f64,
    pub source: ResultSource,
}

/// The preserved candidate pool for one query: what every selector is handed.
#[derive(Debug, Clone)]
pub struct CandidatePool {
    pub query_id: String,
    pub vector_results: Vec<Candidate>,
    pub bm25_results: Vec<Candidate>,
    /// Union of the two rankings, deduplicated on doc id, sorted by score
    /// descending. This is the set every selector may draw from, so its coverage
    /// is the ceiling on what any selector can return.
    pub union_pool: Vec<Candidate>,
}

// ---------------------------------------------------------------------------
// Selector configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectorStrategy {
    /// Return the preserved union pool untouched — the recall ceiling.
    NoOp,
    /// Linear combination of vector and BM25 scores.
    WeightedFusion,
    /// Reciprocal rank fusion (k = 60).
    ReciprocalRankFusion,
    /// Vector retrieves the window, BM25 re-scores inside it.
    CascadeVectorFirst,
    /// BM25 retrieves the window, vector re-scores inside it.
    CascadeBM25First,
    /// Modelled cross-encoder: finite rerank window + imperfect scoring.
    CrossEncoderRerank,
    /// Weighted fusion, then a cross-encoder pass over its head.
    HybridMultiStage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorConfig {
    pub name: String,
    pub strategy: SelectorStrategy,
    pub vector_weight: f64,
    pub bm25_weight: f64,
    /// Informational: which reranker this configuration stands in for.
    pub reranker_model: Option<String>,
    /// Cascade stage-1 depth, and the reranker's rerank window.
    /// Candidates outside the window are DROPPED — this is the mechanism by which
    /// a selector can score below the pool's own recall ceiling.
    pub rerank_window: usize,
    /// Modelled reranker fidelity in `[0.0, 1.0]`: 1.0 orders the window perfectly,
    /// 0.0 orders it at random. Only read by the cross-encoder strategies.
    pub reranker_quality: f64,
}

impl SelectorConfig {
    /// A no-op baseline that measures the preserved pool's own ceiling.
    pub fn noop(name: &str) -> Self {
        Self {
            name: name.to_string(),
            strategy: SelectorStrategy::NoOp,
            vector_weight: 0.5,
            bm25_weight: 0.5,
            reranker_model: None,
            rerank_window: usize::MAX,
            reranker_quality: 1.0,
        }
    }

    pub fn fusion(name: &str, strategy: SelectorStrategy, vector_weight: f64) -> Self {
        Self {
            name: name.to_string(),
            strategy,
            vector_weight,
            bm25_weight: 1.0 - vector_weight,
            reranker_model: None,
            rerank_window: 100,
            reranker_quality: 1.0,
        }
    }

    pub fn reranker(name: &str, model: &str, rerank_window: usize, quality: f64) -> Self {
        Self {
            name: name.to_string(),
            strategy: SelectorStrategy::CrossEncoderRerank,
            vector_weight: 0.6,
            bm25_weight: 0.4,
            reranker_model: Some(model.to_string()),
            rerank_window,
            reranker_quality: quality,
        }
    }
}

// ---------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SelectorResult {
    pub doc_id: String,
    pub score: f64,
    pub rank: usize,
    pub source: ResultSource,
}

/// Per-(selector, query) measurement.
#[derive(Debug, Clone)]
pub struct SelectorMetrics {
    pub selector_name: String,
    pub query_id: String,
    pub recall_at_k: HashMap<usize, f64>,
    pub mrr: f64,
    pub ndcg_at_10: f64,
    pub latency_ms: f64,
    /// Candidates the selector emitted.
    pub results_returned: usize,
    /// Candidates available in the preserved pool.
    pub pool_size: usize,
    /// `results_returned / pool_size` as a percentage.
    pub candidate_pool_utilization: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceIntervals {
    pub recall_at_k: HashMap<usize, (f64, f64)>,
    pub mrr: (f64, f64),
    pub ndcg_at_10: (f64, f64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateMetrics {
    pub mean_recall_at_k: HashMap<usize, f64>,
    pub mean_mrr: f64,
    pub mean_ndcg_at_10: f64,
    pub mean_latency_ms: f64,
    pub mean_pool_utilization: f64,
    pub num_queries: usize,
    pub confidence_intervals: ConfidenceIntervals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairwiseComparison {
    pub selector_a: String,
    pub selector_b: String,
    pub metric: String,
    /// `mean(a) - mean(b)` over the paired queries.
    pub mean_difference: f64,
    /// Percentile bootstrap CI of the paired difference.
    pub difference_ci: (f64, f64),
    /// Two-sided sign-flip permutation p-value.
    pub p_value: f64,
    pub statistically_significant: bool,
    pub winner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub selector: String,
    pub reasoning: String,
    pub expected_recall_gain: Option<f64>,
    pub expected_latency_cost: Option<f64>,
    /// `1 - p` of the comparison that justifies the recommendation, or 0.0 when
    /// no comparison reached significance.
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResults {
    pub selector_configs: Vec<SelectorConfig>,
    pub aggregate_metrics: HashMap<String, AggregateMetrics>,
    pub pairwise_comparisons: Vec<PairwiseComparison>,
    pub recommendations: Vec<Recommendation>,
}

impl BenchmarkResults {
    pub fn aggregate(&self, selector: &str) -> Result<&AggregateMetrics> {
        self.aggregate_metrics
            .get(selector)
            .ok_or_else(|| anyhow!("no aggregate metrics for selector '{selector}'"))
    }

    pub fn mean_recall_at(&self, selector: &str, k: usize) -> Result<f64> {
        self.aggregate(selector)?
            .mean_recall_at_k
            .get(&k)
            .copied()
            .ok_or_else(|| anyhow!("selector '{selector}' has no Recall@{k}"))
    }

    pub fn comparison(&self, a: &str, b: &str, metric: &str) -> Option<&PairwiseComparison> {
        self.pairwise_comparisons.iter().find(|c| {
            c.metric == metric
                && ((c.selector_a == a && c.selector_b == b)
                    || (c.selector_a == b && c.selector_b == a))
        })
    }

    /// Human-readable report, ordered by Recall@100 descending.
    pub fn render(&self) -> String {
        let mut rows: Vec<(&String, &AggregateMetrics)> = self.aggregate_metrics.iter().collect();
        rows.sort_by(|a, b| {
            let ra = a.1.mean_recall_at_k.get(&100).copied().unwrap_or(0.0);
            let rb = b.1.mean_recall_at_k.get(&100).copied().unwrap_or(0.0);
            rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut out = String::new();
        out.push_str(
            "selector                 R@10    R@100   [95% CI]           MRR     NDCG@10  ms\n",
        );
        for (name, agg) in rows {
            let r10 = agg.mean_recall_at_k.get(&10).copied().unwrap_or(0.0);
            let r100 = agg.mean_recall_at_k.get(&100).copied().unwrap_or(0.0);
            let (lo, hi) = agg
                .confidence_intervals
                .recall_at_k
                .get(&100)
                .copied()
                .unwrap_or((0.0, 0.0));
            out.push_str(&format!(
                "{name:<24} {r10:.4}  {r100:.4}  [{lo:.4}, {hi:.4}]  {:.4}  {:.4}   {:.2}\n",
                agg.mean_mrr, agg.mean_ndcg_at_10, agg.mean_latency_ms
            ));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

pub struct SelectorBenchmark {
    ground_truth: Vec<GroundTruth>,
    candidate_pools: HashMap<String, CandidatePool>,
    queries: Vec<Query>,
    resamples: usize,
}

impl SelectorBenchmark {
    pub fn new(
        ground_truth: Vec<GroundTruth>,
        candidate_pools: HashMap<String, CandidatePool>,
        queries: Vec<Query>,
    ) -> Self {
        Self {
            ground_truth,
            candidate_pools,
            queries,
            resamples: 1000,
        }
    }

    /// Number of bootstrap / permutation resamples (default 1000).
    pub fn with_resamples(mut self, resamples: usize) -> Self {
        self.resamples = resamples.max(1);
        self
    }

    pub fn run(&self, selector_configs: Vec<SelectorConfig>) -> Result<BenchmarkResults> {
        if self.queries.is_empty() {
            return Err(anyhow!("selector benchmark requires at least one query"));
        }

        let mut all_metrics: HashMap<String, Vec<SelectorMetrics>> = HashMap::new();
        for config in &selector_configs {
            if all_metrics.contains_key(&config.name) {
                return Err(anyhow!("duplicate selector name '{}'", config.name));
            }
            all_metrics.insert(config.name.clone(), self.evaluate_selector(config)?);
        }

        let aggregate_metrics = self.aggregate(&all_metrics);
        let pairwise_comparisons = self.compare_pairwise(&selector_configs, &all_metrics);
        let recommendations =
            self.recommend(&aggregate_metrics, &pairwise_comparisons, &selector_configs);

        Ok(BenchmarkResults {
            selector_configs,
            aggregate_metrics,
            pairwise_comparisons,
            recommendations,
        })
    }

    /// The ranking this selector emits per query, keyed by query id.
    ///
    /// Exposed so the answer-quality gate scores the *same* rankings this gate
    /// measures, instead of re-simulating selection with a second code path.
    pub fn rankings(
        &self,
        config: &SelectorConfig,
    ) -> Result<HashMap<String, Vec<SelectorResult>>> {
        let mut out = HashMap::with_capacity(self.queries.len());
        for query in &self.queries {
            let pool = self
                .candidate_pools
                .get(&query.query_id)
                .ok_or_else(|| anyhow!("missing candidate pool for query '{}'", query.query_id))?;
            out.insert(query.query_id.clone(), self.apply_selector(config, pool));
        }
        Ok(out)
    }

    fn evaluate_selector(&self, config: &SelectorConfig) -> Result<Vec<SelectorMetrics>> {
        let mut metrics = Vec::with_capacity(self.queries.len());
        for query in &self.queries {
            let pool = self
                .candidate_pools
                .get(&query.query_id)
                .ok_or_else(|| anyhow!("missing candidate pool for query '{}'", query.query_id))?;
            let ground_truth = self
                .ground_truth
                .iter()
                .find(|gt| gt.query_id == query.query_id)
                .ok_or_else(|| anyhow!("missing ground truth for query '{}'", query.query_id))?;

            let start = Instant::now();
            let results = self.apply_selector(config, pool);
            let latency_ms = start.elapsed().as_secs_f64() * 1000.0;

            let pool_size = pool.union_pool.len();
            metrics.push(SelectorMetrics {
                selector_name: config.name.clone(),
                query_id: query.query_id.clone(),
                recall_at_k: recall_at_ks(&results, ground_truth, &RECALL_KS),
                mrr: reciprocal_rank(&results, ground_truth),
                ndcg_at_10: ndcg_at_k(&results, ground_truth, 10),
                latency_ms,
                results_returned: results.len(),
                pool_size,
                candidate_pool_utilization: if pool_size > 0 {
                    (results.len() as f64 / pool_size as f64) * 100.0
                } else {
                    0.0
                },
            });
        }
        Ok(metrics)
    }

    // -- strategies ---------------------------------------------------------

    fn apply_selector(&self, config: &SelectorConfig, pool: &CandidatePool) -> Vec<SelectorResult> {
        match config.strategy {
            SelectorStrategy::NoOp => rank(
                pool.union_pool
                    .iter()
                    .map(|c| (c.doc_id.clone(), c.score, c.source.clone())),
            ),
            SelectorStrategy::WeightedFusion => self.weighted_fusion(config, pool),
            SelectorStrategy::ReciprocalRankFusion => self.rrf_fusion(pool),
            SelectorStrategy::CascadeVectorFirst => self.cascade(
                config,
                &pool.vector_results,
                &pool.bm25_results,
                config.vector_weight,
                config.bm25_weight,
            ),
            SelectorStrategy::CascadeBM25First => self.cascade(
                config,
                &pool.bm25_results,
                &pool.vector_results,
                config.bm25_weight,
                config.vector_weight,
            ),
            SelectorStrategy::CrossEncoderRerank => {
                let fused = self.weighted_fusion(config, pool);
                self.cross_encoder(config, pool, &fused)
            }
            SelectorStrategy::HybridMultiStage => {
                let rrf = self.rrf_fusion(pool);
                self.cross_encoder(config, pool, &rrf)
            }
        }
    }

    fn weighted_fusion(
        &self,
        config: &SelectorConfig,
        pool: &CandidatePool,
    ) -> Vec<SelectorResult> {
        let mut scored: HashMap<String, f64> = HashMap::new();
        for c in &pool.vector_results {
            *scored.entry(c.doc_id.clone()).or_insert(0.0) += c.score * config.vector_weight;
        }
        for c in &pool.bm25_results {
            *scored.entry(c.doc_id.clone()).or_insert(0.0) += c.score * config.bm25_weight;
        }
        rank(
            scored
                .into_iter()
                .map(|(doc_id, score)| (doc_id, score, ResultSource::Hybrid)),
        )
    }

    fn rrf_fusion(&self, pool: &CandidatePool) -> Vec<SelectorResult> {
        const RRF_K: f64 = 60.0;
        let mut scored: HashMap<String, f64> = HashMap::new();
        for (i, c) in pool.vector_results.iter().enumerate() {
            *scored.entry(c.doc_id.clone()).or_insert(0.0) += 1.0 / (RRF_K + i as f64 + 1.0);
        }
        for (i, c) in pool.bm25_results.iter().enumerate() {
            *scored.entry(c.doc_id.clone()).or_insert(0.0) += 1.0 / (RRF_K + i as f64 + 1.0);
        }
        rank(
            scored
                .into_iter()
                .map(|(doc_id, score)| (doc_id, score, ResultSource::Hybrid)),
        )
    }

    /// Stage 1 retrieves `rerank_window` from `primary`; stage 2 re-scores only
    /// inside that window. Documents `secondary` found but `primary` missed are
    /// unreachable — the cascade's structural recall cost.
    fn cascade(
        &self,
        config: &SelectorConfig,
        primary: &[Candidate],
        secondary: &[Candidate],
        primary_weight: f64,
        secondary_weight: f64,
    ) -> Vec<SelectorResult> {
        let window = config.rerank_window.min(primary.len());
        let mut scored: HashMap<String, f64> = HashMap::new();
        for c in primary.iter().take(window) {
            scored
                .entry(c.doc_id.clone())
                .and_modify(|s| *s = s.max(c.score * primary_weight))
                .or_insert(c.score * primary_weight);
        }
        for c in secondary {
            if let Some(slot) = scored.get_mut(&c.doc_id) {
                *slot += c.score * secondary_weight;
            }
        }
        rank(
            scored
                .into_iter()
                .map(|(doc_id, score)| (doc_id, score, ResultSource::Hybrid)),
        )
    }

    /// Models a cross-encoder pass: it sees only the head `rerank_window` of the
    /// upstream ranking and re-orders it with fidelity `reranker_quality`. Anything
    /// past the window is dropped, so the emitted list can hold LESS recall than the
    /// pool it came from — the failure mode PR #1726 observed.
    fn cross_encoder(
        &self,
        config: &SelectorConfig,
        pool: &CandidatePool,
        upstream: &[SelectorResult],
    ) -> Vec<SelectorResult> {
        let window = config.rerank_window.min(upstream.len());
        let quality = config.reranker_quality.clamp(0.0, 1.0);
        // Per-query seed so the modelled reranker is deterministic but decorrelated
        // across queries.
        let mut rng = SmallRng::seed_from_u64(STATS_SEED ^ fnv1a(&pool.query_id));

        let mut reranked: Vec<(String, f64)> = Vec::with_capacity(window);
        for (i, r) in upstream.iter().take(window).enumerate() {
            // Ideal component: the upstream head order, normalised to (0, 1].
            let ideal = 1.0 - (i as f64 / window.max(1) as f64);
            let noise: f64 = rng.r#gen();
            reranked.push((r.doc_id.clone(), quality * ideal + (1.0 - quality) * noise));
        }
        rank(
            reranked
                .into_iter()
                .map(|(doc_id, score)| (doc_id, score, ResultSource::Reranked)),
        )
    }

    // -- statistics ---------------------------------------------------------

    fn aggregate(
        &self,
        all_metrics: &HashMap<String, Vec<SelectorMetrics>>,
    ) -> HashMap<String, AggregateMetrics> {
        let mut aggregates = HashMap::new();
        for (name, metrics) in all_metrics {
            if metrics.is_empty() {
                continue;
            }
            let mut mean_recall_at_k = HashMap::new();
            let mut recall_ci = HashMap::new();
            for &k in RECALL_KS.iter() {
                let values = per_query(metrics, |m| m.recall_at_k.get(&k).copied().unwrap_or(0.0));
                mean_recall_at_k.insert(k, stats::mean(&values));
                recall_ci.insert(k, stats::bootstrap_ci(&values, self.resamples, STATS_SEED));
            }
            let mrr = per_query(metrics, |m| m.mrr);
            let ndcg = per_query(metrics, |m| m.ndcg_at_10);

            aggregates.insert(
                name.clone(),
                AggregateMetrics {
                    mean_recall_at_k,
                    mean_mrr: stats::mean(&mrr),
                    mean_ndcg_at_10: stats::mean(&ndcg),
                    mean_latency_ms: stats::mean(&per_query(metrics, |m| m.latency_ms)),
                    mean_pool_utilization: stats::mean(&per_query(metrics, |m| {
                        m.candidate_pool_utilization
                    })),
                    num_queries: metrics.len(),
                    confidence_intervals: ConfidenceIntervals {
                        recall_at_k: recall_ci,
                        mrr: stats::bootstrap_ci(&mrr, self.resamples, STATS_SEED),
                        ndcg_at_10: stats::bootstrap_ci(&ndcg, self.resamples, STATS_SEED),
                    },
                },
            );
        }
        aggregates
    }

    fn compare_pairwise(
        &self,
        configs: &[SelectorConfig],
        all_metrics: &HashMap<String, Vec<SelectorMetrics>>,
    ) -> Vec<PairwiseComparison> {
        // Iterate configs (not the HashMap) so comparison order is deterministic.
        let extractors: [MetricFn<SelectorMetrics>; 3] = [
            ("Recall@100", |m| {
                m.recall_at_k.get(&100).copied().unwrap_or(0.0)
            }),
            ("MRR", |m| m.mrr),
            ("NDCG@10", |m| m.ndcg_at_10),
        ];

        let mut comparisons = Vec::new();
        for (i, a) in configs.iter().enumerate() {
            for b in configs.iter().skip(i + 1) {
                let (Some(ma), Some(mb)) = (all_metrics.get(&a.name), all_metrics.get(&b.name))
                else {
                    continue;
                };
                for (metric, extract) in extractors {
                    // Pair strictly by query id; unmatched queries are excluded.
                    let index_b: HashMap<&str, f64> = mb
                        .iter()
                        .map(|m| (m.query_id.as_str(), extract(m)))
                        .collect();
                    let diffs: Vec<f64> = ma
                        .iter()
                        .filter_map(|m| index_b.get(m.query_id.as_str()).map(|vb| extract(m) - *vb))
                        .collect();
                    if diffs.is_empty() {
                        continue;
                    }

                    let mean_difference = stats::mean(&diffs);
                    let p_value = stats::sign_flip_p(&diffs, self.resamples, STATS_SEED);
                    let significant = p_value < 0.05;
                    comparisons.push(PairwiseComparison {
                        selector_a: a.name.clone(),
                        selector_b: b.name.clone(),
                        metric: metric.to_string(),
                        mean_difference,
                        difference_ci: stats::bootstrap_ci(&diffs, self.resamples, STATS_SEED),
                        p_value,
                        statistically_significant: significant,
                        winner: if !significant || mean_difference == 0.0 {
                            None
                        } else if mean_difference > 0.0 {
                            Some(a.name.clone())
                        } else {
                            Some(b.name.clone())
                        },
                    });
                }
            }
        }
        comparisons
    }

    /// Recommend only what the paired tests support. A selector that wins on the
    /// point estimate but loses every significance test yields no recommendation.
    fn recommend(
        &self,
        aggregates: &HashMap<String, AggregateMetrics>,
        comparisons: &[PairwiseComparison],
        configs: &[SelectorConfig],
    ) -> Vec<Recommendation> {
        let mut best: Option<(&str, f64)> = None;
        for config in configs {
            let Some(agg) = aggregates.get(&config.name) else {
                continue;
            };
            let recall = agg.mean_recall_at_k.get(&100).copied().unwrap_or(0.0);
            if best.is_none_or(|(_, b)| recall > b) {
                best = Some((config.name.as_str(), recall));
            }
        }
        let Some((leader, leader_recall)) = best else {
            return Vec::new();
        };

        // The leader is promotable only if it beats every rival significantly on
        // Recall@100 (Gate 3: promote only statistically supported improvements).
        let mut worst_p: f64 = 0.0;
        let mut unsupported = Vec::new();
        for config in configs {
            if config.name == leader {
                continue;
            }
            match comparisons
                .iter()
                .find(|c| {
                    c.metric == "Recall@100"
                        && ((c.selector_a == leader && c.selector_b == config.name)
                            || (c.selector_b == leader && c.selector_a == config.name))
                })
                .filter(|c| c.statistically_significant && c.winner.as_deref() == Some(leader))
            {
                Some(c) => worst_p = worst_p.max(c.p_value),
                None => unsupported.push(config.name.clone()),
            }
        }

        let Some(agg) = aggregates.get(leader) else {
            return Vec::new();
        };
        let (lo, hi) = agg
            .confidence_intervals
            .recall_at_k
            .get(&100)
            .copied()
            .unwrap_or((0.0, 0.0));

        let (reasoning, confidence) = if unsupported.is_empty() {
            (
                format!(
                    "Highest Recall@100 {leader_recall:.4} (95% CI [{lo:.4}, {hi:.4}]) over \
                     {} queries, and significantly better than every rival (max p = {worst_p:.4}).",
                    agg.num_queries
                ),
                1.0 - worst_p,
            )
        } else {
            (
                format!(
                    "Highest Recall@100 {leader_recall:.4} (95% CI [{lo:.4}, {hi:.4}]) over {} \
                     queries, but NOT significantly better than {}. Not promotable on this \
                     evidence — collect more queries or accept the incumbent.",
                    agg.num_queries,
                    unsupported.join(", ")
                ),
                0.0,
            )
        };

        vec![Recommendation {
            selector: leader.to_string(),
            reasoning,
            expected_recall_gain: Some(leader_recall),
            expected_latency_cost: Some(agg.mean_latency_ms),
            confidence,
        }]
    }
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Sort by score descending, assign ranks, and materialise `SelectorResult`s.
/// Ties break on doc id so the ranking is deterministic regardless of hash order.
fn rank(items: impl Iterator<Item = (String, f64, ResultSource)>) -> Vec<SelectorResult> {
    let mut collected: Vec<(String, f64, ResultSource)> = items.collect();
    collected.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    collected
        .into_iter()
        .enumerate()
        .map(|(rank, (doc_id, score, source))| SelectorResult {
            doc_id,
            score,
            rank,
            source,
        })
        .collect()
}

/// Recall@k = |top-k ∩ relevant| / |relevant|.
///
/// The previous revision of this harness divided by `k`, which is precision@k —
/// with 100 relevant docs and k = 1 it reported 1.0 for a perfect hit where recall
/// is 0.01. Every recall number in this harness uses the definition below.
pub fn recall_at_ks(
    results: &[SelectorResult],
    ground_truth: &GroundTruth,
    ks: &[usize],
) -> HashMap<usize, f64> {
    let relevant: HashSet<&str> = ground_truth
        .relevant_docs
        .iter()
        .map(String::as_str)
        .collect();
    let denominator = relevant.len() as f64;
    ks.iter()
        .map(|&k| {
            if denominator == 0.0 {
                return (k, 0.0);
            }
            let hits = results
                .iter()
                .take(k)
                .filter(|r| relevant.contains(r.doc_id.as_str()))
                .count();
            (k, hits as f64 / denominator)
        })
        .collect()
}

/// Reciprocal rank of the first relevant result (0.0 when none is retrieved).
/// Averaged across queries by the harness, this is MRR.
pub fn reciprocal_rank(results: &[SelectorResult], ground_truth: &GroundTruth) -> f64 {
    let relevant: HashSet<&str> = ground_truth
        .relevant_docs
        .iter()
        .map(String::as_str)
        .collect();
    results
        .iter()
        .position(|r| relevant.contains(r.doc_id.as_str()))
        .map(|i| 1.0 / (i + 1) as f64)
        .unwrap_or(0.0)
}

/// NDCG@k with graded gains: the ground-truth list is ordered most-relevant first,
/// and position `i` in it carries gain `1 / (i + 1)`.
pub fn ndcg_at_k(results: &[SelectorResult], ground_truth: &GroundTruth, k: usize) -> f64 {
    if ground_truth.relevant_docs.is_empty() {
        return 0.0;
    }
    let gains: HashMap<&str, f64> = ground_truth
        .relevant_docs
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), 1.0 / (i + 1) as f64))
        .collect();

    let dcg: f64 = results
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, r)| {
            gains.get(r.doc_id.as_str()).copied().unwrap_or(0.0) / ((i + 2) as f64).log2()
        })
        .sum();
    let idcg: f64 = (0..ground_truth.relevant_docs.len().min(k))
        .map(|i| (1.0 / (i + 1) as f64) / ((i + 2) as f64).log2())
        .sum();

    if idcg > 0.0 { dcg / idcg } else { 0.0 }
}

fn per_query(metrics: &[SelectorMetrics], extract: impl Fn(&SelectorMetrics) -> f64) -> Vec<f64> {
    metrics.iter().map(extract).collect()
}

/// FNV-1a over a string — a stable, platform-independent seed source.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Resampling statistics. Deliberately not a normal approximation: recall per query
/// is bounded and heavily tied, so the normal CI over-covers near 0 and 1.
pub mod stats {
    use rand::rngs::SmallRng;
    use rand::{Rng, SeedableRng};

    pub fn mean(values: &[f64]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        values.iter().sum::<f64>() / values.len() as f64
    }

    /// Percentile bootstrap 95% CI of the mean.
    pub fn bootstrap_ci(values: &[f64], resamples: usize, seed: u64) -> (f64, f64) {
        if values.is_empty() {
            return (0.0, 0.0);
        }
        if values.len() == 1 {
            return (values[0], values[0]);
        }
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut means: Vec<f64> = Vec::with_capacity(resamples);
        for _ in 0..resamples {
            let mut acc = 0.0;
            for _ in 0..values.len() {
                acc += values[rng.gen_range(0..values.len())];
            }
            means.push(acc / values.len() as f64);
        }
        means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        (percentile(&means, 0.025), percentile(&means, 0.975))
    }

    /// Two-sided paired permutation (sign-flip) test on per-query differences.
    ///
    /// Under H0 the sign of each difference is exchangeable, so we resample signs
    /// and count how often `|mean|` reaches the observed value. The `+1` correction
    /// keeps the p-value strictly positive — an exact 0.0 would overstate certainty
    /// that finite resampling cannot support.
    pub fn sign_flip_p(diffs: &[f64], resamples: usize, seed: u64) -> f64 {
        if diffs.is_empty() {
            return 1.0;
        }
        let observed = mean(diffs).abs();
        if observed == 0.0 {
            return 1.0;
        }
        let mut rng = SmallRng::seed_from_u64(seed ^ 0x9E37_79B9_7F4A_7C15);
        let mut at_least_as_extreme = 0usize;
        for _ in 0..resamples {
            let mut acc = 0.0;
            for &d in diffs {
                acc += if rng.r#gen::<bool>() { d } else { -d };
            }
            if (acc / diffs.len() as f64).abs() >= observed {
                at_least_as_extreme += 1;
            }
        }
        (at_least_as_extreme as f64 + 1.0) / (resamples as f64 + 1.0)
    }

    /// Linear-interpolated percentile of a pre-sorted slice.
    fn percentile(sorted: &[f64], q: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let pos = q * (sorted.len() - 1) as f64;
        let lo = pos.floor() as usize;
        let hi = pos.ceil() as usize;
        if lo == hi {
            sorted[lo]
        } else {
            sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
        }
    }
}
