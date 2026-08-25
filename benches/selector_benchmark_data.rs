//! Synthetic candidate-pool generation for the selector benchmark (TD-SELECTOR-1).
//!
//! Produces ground truth plus a *preserved* candidate pool per query: a vector
//! ranking, a BM25 ranking, and their deduplicated union. Every selector under test
//! consumes the same pool, so a measured difference is attributable to the selector.
//!
//! # What this data is and is not
//!
//! It is a **controlled mechanism bed**: coverage rates and score separability are
//! knobs, so the pool's own recall ceiling is known by construction and a selector's
//! shortfall against that ceiling is measurable. It is **not** a sample of
//! production traffic — absolute numbers here carry no external validity and are
//! not admissible as benchmark evidence.
//!
//! Generation is fully seeded: the same `DataGenConfig` yields byte-identical pools.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

use crate::bench_selector_benchmark::{Candidate, CandidatePool, GroundTruth, Query, ResultSource};

/// How many relevant documents a query has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelevanceDistribution {
    /// Every query has exactly `num_relevant_per_query`.
    Uniform,
    /// Most queries have few relevant docs (right-skewed, long thin tail).
    SkewedHigh,
    /// Most queries have many relevant docs (left-skewed).
    SkewedLow,
    /// Power law: a handful of queries have many, most have very few.
    PowerLaw,
}

/// Additive noise applied to a candidate's base score. `Normal { mean, std }` is
/// noise, so `mean` is normally 0.0 — a non-zero mean shifts every score equally
/// and changes nothing about the ranking.
#[derive(Debug, Clone, Copy)]
pub enum ScoreDistribution {
    /// Uniform noise on `[-half_width, half_width]`.
    Uniform {
        half_width: f64,
    },
    Normal {
        mean: f64,
        std: f64,
    },
    /// Exponential noise, mean-centred so it does not shift the whole ranking.
    Exponential {
        lambda: f64,
    },
}

#[derive(Debug, Clone)]
pub struct DataGenConfig {
    pub num_queries: usize,
    pub num_documents: usize,
    pub num_relevant_per_query: usize,
    /// Candidates each retriever returns (before dedup into the union pool).
    pub candidate_pool_size: usize,
    pub relevance_distribution: RelevanceDistribution,
    pub score_distribution: ScoreDistribution,
    /// Fraction of the vector ranking occupied by genuinely relevant documents.
    pub vector_relevant_rate: f64,
    /// Fraction of the BM25 ranking occupied by genuinely relevant documents.
    /// The two retrievers draw *different* relevant subsets, so their union covers
    /// more than either alone — the effect PR #1726 exploited to lift the ceiling.
    pub bm25_relevant_rate: f64,
    /// Score separation between relevant and non-relevant candidates. Lower values
    /// make the pool harder to order, which is what a selector has to fix.
    pub score_separation: f64,
    pub seed: u64,
}

impl Default for DataGenConfig {
    fn default() -> Self {
        Self {
            num_queries: 100,
            num_documents: 10_000,
            num_relevant_per_query: 50,
            candidate_pool_size: 500,
            relevance_distribution: RelevanceDistribution::SkewedHigh,
            score_distribution: ScoreDistribution::Normal {
                mean: 0.0,
                std: 0.15,
            },
            vector_relevant_rate: 0.30,
            bm25_relevant_rate: 0.25,
            score_separation: 0.25,
            seed: 0x0DEF_ACED_0000_0001,
        }
    }
}

impl DataGenConfig {
    /// A small, fast configuration for tests.
    pub fn small() -> Self {
        Self {
            num_queries: 40,
            num_documents: 2_000,
            num_relevant_per_query: 40,
            candidate_pool_size: 300,
            ..Self::default()
        }
    }
}

/// Generate ground truth, preserved candidate pools, and queries.
pub fn generate_benchmark_data(
    config: &DataGenConfig,
) -> (Vec<GroundTruth>, HashMap<String, CandidatePool>, Vec<Query>) {
    let mut rng = SmallRng::seed_from_u64(config.seed);

    let documents: Vec<String> = (0..config.num_documents.max(1))
        .map(|i| format!("doc{i}"))
        .collect();

    let mut ground_truth = Vec::with_capacity(config.num_queries);
    let mut candidate_pools = HashMap::with_capacity(config.num_queries);
    let mut queries = Vec::with_capacity(config.num_queries);

    for q in 0..config.num_queries {
        let query_id = format!("query{q}");
        let relevant_docs = sample_relevant_docs(&mut rng, &documents, config);

        let pool = generate_candidate_pool(&mut rng, &query_id, &relevant_docs, &documents, config);

        ground_truth.push(GroundTruth {
            query_id: query_id.clone(),
            relevant_docs,
        });
        candidate_pools.insert(query_id.clone(), pool);
        queries.push(Query {
            query_text: format!("synthetic query {q}"),
            query_vector: Some(unit_vector(&mut rng, 384)),
            query_id,
        });
    }

    (ground_truth, candidate_pools, queries)
}

/// Draw the relevant set for one query, sampled without replacement so a document
/// never appears twice in the ground truth.
fn sample_relevant_docs(
    rng: &mut SmallRng,
    documents: &[String],
    config: &DataGenConfig,
) -> Vec<String> {
    let target = config.num_relevant_per_query.max(1) as f64;
    let count = match config.relevance_distribution {
        RelevanceDistribution::Uniform => target,
        // Right-skewed: u^2 concentrates mass near 0, so most queries land low.
        RelevanceDistribution::SkewedHigh => {
            let u: f64 = rng.r#gen();
            target * u.powi(2)
        }
        // Left-skewed: at least 70% of target, up to target.
        RelevanceDistribution::SkewedLow => {
            let u: f64 = rng.r#gen();
            target * (0.7 + 0.3 * u)
        }
        // Power law with exponent 3 — a heavier head than SkewedHigh.
        RelevanceDistribution::PowerLaw => {
            let u: f64 = rng.r#gen();
            target * (1.0 - u).powi(3)
        }
    };
    let count = (count.round() as usize).clamp(1, documents.len());

    let mut pool: Vec<&String> = documents.iter().collect();
    pool.shuffle(rng);
    pool.into_iter().take(count).cloned().collect()
}

fn generate_candidate_pool(
    rng: &mut SmallRng,
    query_id: &str,
    relevant_docs: &[String],
    documents: &[String],
    config: &DataGenConfig,
) -> CandidatePool {
    let relevant_set: HashSet<&str> = relevant_docs.iter().map(String::as_str).collect();
    let non_relevant: Vec<&String> = documents
        .iter()
        .filter(|d| !relevant_set.contains(d.as_str()))
        .collect();

    let vector_results = build_ranking(
        rng,
        relevant_docs,
        &non_relevant,
        config,
        config.vector_relevant_rate,
        ResultSource::Vector,
    );
    let bm25_results = build_ranking(
        rng,
        relevant_docs,
        &non_relevant,
        config,
        config.bm25_relevant_rate,
        ResultSource::BM25,
    );

    let union_pool = union_of(&vector_results, &bm25_results);

    CandidatePool {
        query_id: query_id.to_string(),
        vector_results,
        bm25_results,
        union_pool,
    }
}

/// Build one retriever's ranking: `relevant_rate` of the slots hold relevant docs
/// (sampled without replacement, so there are no duplicates), the rest hold
/// distractors. Scores separate the two classes by `score_separation` before noise.
fn build_ranking(
    rng: &mut SmallRng,
    relevant_docs: &[String],
    non_relevant: &[&String],
    config: &DataGenConfig,
    relevant_rate: f64,
    source: ResultSource,
) -> Vec<Candidate> {
    let pool_size = config.candidate_pool_size.max(1);
    let want_relevant = ((pool_size as f64) * relevant_rate.clamp(0.0, 1.0)).round() as usize;
    let take_relevant = want_relevant.min(relevant_docs.len());
    let take_distractor = pool_size
        .saturating_sub(take_relevant)
        .min(non_relevant.len());

    let mut relevant_shuffled: Vec<&String> = relevant_docs.iter().collect();
    relevant_shuffled.shuffle(rng);
    let mut distractors: Vec<&String> = non_relevant.to_vec();
    distractors.shuffle(rng);

    let separation = config.score_separation.clamp(0.0, 1.0);
    let mut candidates = Vec::with_capacity(take_relevant + take_distractor);

    for doc in relevant_shuffled.into_iter().take(take_relevant) {
        let base = 0.5 + separation / 2.0;
        candidates.push(make_candidate(rng, doc, base, config, source.clone()));
    }
    for doc in distractors.into_iter().take(take_distractor) {
        let base = 0.5 - separation / 2.0;
        candidates.push(make_candidate(rng, doc, base, config, source.clone()));
    }

    sort_by_score_desc(&mut candidates);
    candidates
}

fn make_candidate(
    rng: &mut SmallRng,
    doc_id: &str,
    base: f64,
    config: &DataGenConfig,
    source: ResultSource,
) -> Candidate {
    let score = (base + noise(rng, config.score_distribution)).clamp(0.0, 1.0);
    Candidate {
        doc_id: doc_id.to_string(),
        score,
        source,
    }
}

/// Additive, mean-centred noise. Falls back to zero noise when a distribution's
/// parameters are invalid, rather than panicking inside data generation.
fn noise(rng: &mut SmallRng, distribution: ScoreDistribution) -> f64 {
    match distribution {
        ScoreDistribution::Uniform { half_width } => {
            let h = half_width.abs();
            rng.gen_range(-h..=h)
        }
        ScoreDistribution::Normal { mean, std } => match rand_distr::Normal::new(mean, std.abs()) {
            Ok(dist) => rand_distr::Distribution::sample(&dist, rng),
            Err(_) => 0.0,
        },
        ScoreDistribution::Exponential { lambda } => match rand_distr::Exp::new(lambda.abs()) {
            // Subtract the mean (1/λ) so the noise does not shift every score up.
            Ok(dist) => {
                rand_distr::Distribution::sample(&dist, rng)
                    - (1.0 / lambda.abs().max(f64::EPSILON))
            }
            Err(_) => 0.0,
        },
    }
}

/// Union of two rankings, keeping the higher score per document. Sorted by score
/// descending with a doc-id tie-break so the ordering never depends on hash order —
/// the pool this produces is the recall ceiling every selector is measured against.
fn union_of(vector_results: &[Candidate], bm25_results: &[Candidate]) -> Vec<Candidate> {
    let mut merged: HashMap<&str, Candidate> = HashMap::new();
    for candidate in vector_results.iter().chain(bm25_results.iter()) {
        merged
            .entry(candidate.doc_id.as_str())
            .and_modify(|existing| {
                if candidate.score > existing.score {
                    existing.score = candidate.score;
                }
                existing.source = ResultSource::Hybrid;
            })
            .or_insert_with(|| candidate.clone());
    }
    let mut union: Vec<Candidate> = merged.into_values().collect();
    sort_by_score_desc(&mut union);
    union
}

fn sort_by_score_desc(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });
}

fn unit_vector(rng: &mut SmallRng, dim: usize) -> Vec<f32> {
    let mut v: Vec<f32> = (0..dim).map(|_| rng.r#gen::<f32>() * 2.0 - 1.0).collect();
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

/// **The** ceiling: what fraction of the relevant set the preserved pool contains
/// at all, ignoring order. No selector can exceed this, because a selector can only
/// drop candidates — this is the quantity PR #1726 moved from 0.3472 to 0.3734.
///
/// Distinct from [`pool_recall_ceiling`], which is recall@k of one *ordering* of the
/// pool and is therefore beatable by a selector that orders the pool better.
pub fn pool_coverage_ceiling(
    pools: &HashMap<String, CandidatePool>,
    ground_truth: &[GroundTruth],
) -> f64 {
    pool_recall_ceiling(pools, ground_truth, usize::MAX)
}

/// Recall@k of the preserved union pool in its own raw-score order.
///
/// This is a *baseline*, not a bound: for k smaller than the pool, a selector that
/// ranks better can and does exceed it. Use [`pool_coverage_ceiling`] for the bound.
pub fn pool_recall_ceiling(
    pools: &HashMap<String, CandidatePool>,
    ground_truth: &[GroundTruth],
    k: usize,
) -> f64 {
    if ground_truth.is_empty() {
        return 0.0;
    }
    let total: f64 = ground_truth
        .iter()
        .map(|gt| {
            let Some(pool) = pools.get(&gt.query_id) else {
                return 0.0;
            };
            if gt.relevant_docs.is_empty() {
                return 0.0;
            }
            let relevant: HashSet<&str> = gt.relevant_docs.iter().map(String::as_str).collect();
            let hits = pool
                .union_pool
                .iter()
                .take(k)
                .filter(|c| relevant.contains(c.doc_id.as_str()))
                .count();
            hits as f64 / gt.relevant_docs.len() as f64
        })
        .sum();
    total / ground_truth.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_for_a_seed() {
        let config = DataGenConfig::small();
        let (gt_a, pools_a, _) = generate_benchmark_data(&config);
        let (gt_b, pools_b, _) = generate_benchmark_data(&config);

        assert_eq!(gt_a.len(), gt_b.len());
        for (a, b) in gt_a.iter().zip(gt_b.iter()) {
            assert_eq!(a.query_id, b.query_id);
            assert_eq!(a.relevant_docs, b.relevant_docs);
        }
        for (id, pool_a) in &pools_a {
            let pool_b = pools_b.get(id).expect("same query ids");
            let ids_a: Vec<&str> = pool_a
                .union_pool
                .iter()
                .map(|c| c.doc_id.as_str())
                .collect();
            let ids_b: Vec<&str> = pool_b
                .union_pool
                .iter()
                .map(|c| c.doc_id.as_str())
                .collect();
            assert_eq!(ids_a, ids_b, "union pool order must be seed-stable");
        }
    }

    #[test]
    fn changing_the_seed_changes_the_data() {
        let config = DataGenConfig::small();
        let other = DataGenConfig {
            seed: config.seed ^ 0xFFFF,
            ..config.clone()
        };
        let (gt_a, _, _) = generate_benchmark_data(&config);
        let (gt_b, _, _) = generate_benchmark_data(&other);
        assert_ne!(
            gt_a[0].relevant_docs, gt_b[0].relevant_docs,
            "a different seed must produce different ground truth"
        );
    }

    #[test]
    fn rankings_contain_no_duplicate_documents() {
        let (_, pools, _) = generate_benchmark_data(&DataGenConfig::small());
        for pool in pools.values() {
            for ranking in [&pool.vector_results, &pool.bm25_results] {
                let unique: HashSet<&str> = ranking.iter().map(|c| c.doc_id.as_str()).collect();
                assert_eq!(
                    unique.len(),
                    ranking.len(),
                    "a retriever ranking must not repeat a document"
                );
            }
        }
    }

    #[test]
    fn union_pool_covers_at_least_as_much_as_either_retriever() {
        let (ground_truth, pools, _) = generate_benchmark_data(&DataGenConfig::small());
        for gt in &ground_truth {
            let pool = pools.get(&gt.query_id).expect("pool for every query");
            let relevant: HashSet<&str> = gt.relevant_docs.iter().map(String::as_str).collect();
            let count = |c: &[Candidate]| {
                c.iter()
                    .filter(|x| relevant.contains(x.doc_id.as_str()))
                    .map(|x| x.doc_id.as_str())
                    .collect::<HashSet<_>>()
                    .len()
            };
            let union = count(&pool.union_pool);
            assert!(union >= count(&pool.vector_results));
            assert!(union >= count(&pool.bm25_results));
        }
    }

    #[test]
    fn union_pool_is_sorted_by_score() {
        let (_, pools, _) = generate_benchmark_data(&DataGenConfig::small());
        for pool in pools.values() {
            for pair in pool.union_pool.windows(2) {
                assert!(
                    pair[0].score >= pair[1].score,
                    "union pool must be sorted descending"
                );
            }
        }
    }

    #[test]
    fn every_relevance_and_score_distribution_produces_a_usable_bed() {
        // Each variant is a knob a future bed may turn; an unexercised variant is a
        // panic or a degenerate pool waiting for whoever turns it.
        for relevance in [
            RelevanceDistribution::Uniform,
            RelevanceDistribution::SkewedHigh,
            RelevanceDistribution::SkewedLow,
            RelevanceDistribution::PowerLaw,
        ] {
            for score in [
                ScoreDistribution::Uniform { half_width: 0.1 },
                ScoreDistribution::Normal {
                    mean: 0.0,
                    std: 0.1,
                },
                ScoreDistribution::Exponential { lambda: 8.0 },
            ] {
                let config = DataGenConfig {
                    num_queries: 5,
                    num_documents: 300,
                    relevance_distribution: relevance,
                    score_distribution: score,
                    ..DataGenConfig::small()
                };
                let (ground_truth, pools, queries) = generate_benchmark_data(&config);
                assert_eq!(queries.len(), 5);
                for gt in &ground_truth {
                    assert!(
                        !gt.relevant_docs.is_empty(),
                        "{relevance:?}/{score:?} produced a query with no relevant docs"
                    );
                    let pool = pools.get(&gt.query_id).expect("pool for every query");
                    assert!(!pool.union_pool.is_empty());
                    for candidate in &pool.union_pool {
                        assert!(
                            (0.0..=1.0).contains(&candidate.score),
                            "{relevance:?}/{score:?} produced an out-of-range score {}",
                            candidate.score
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn score_separation_orders_relevant_above_distractors_on_average() {
        let config = DataGenConfig {
            score_separation: 0.5,
            score_distribution: ScoreDistribution::Normal {
                mean: 0.0,
                std: 0.05,
            },
            ..DataGenConfig::small()
        };
        let (ground_truth, pools, _) = generate_benchmark_data(&config);
        let gt = &ground_truth[0];
        let relevant: HashSet<&str> = gt.relevant_docs.iter().map(String::as_str).collect();
        let pool = pools.get(&gt.query_id).expect("pool for query0");

        let mean_of = |want_relevant: bool| {
            let scores: Vec<f64> = pool
                .vector_results
                .iter()
                .filter(|c| relevant.contains(c.doc_id.as_str()) == want_relevant)
                .map(|c| c.score)
                .collect();
            scores.iter().sum::<f64>() / scores.len().max(1) as f64
        };
        assert!(
            mean_of(true) > mean_of(false),
            "relevant candidates must score above distractors on average"
        );
    }
}
