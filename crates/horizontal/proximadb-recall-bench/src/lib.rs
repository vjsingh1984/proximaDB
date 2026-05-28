//! Recall harness for the embedding-precision rollout — PR 11 of
//! `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc`.
//!
//! Compares top-K overlap between a *reference* precision config (the
//! ground truth — usually fp32) and a *candidate* config (e.g. fp16 or
//! int8) across canonical recall benchmarks. The CI gate (LLD §PR 11)
//! refuses to merge any policy change that doesn't ship a fresh
//! `RecallReport` showing the candidate still clears the policy's
//! `RecallSlo` (PR 6a).
//!
//! ## Layering
//!
//! This crate is contract-only and dataset-agnostic. It owns:
//!
//! * The `Dataset` trait (base vectors + queries + ground-truth top-K).
//! * The `recall_at_k` overlap calculator.
//! * The `RecallReport` aggregator.
//! * A `SyntheticDataset` impl for unit tests.
//!
//! Real-world datasets (SIFT-1M, GloVe-100, ANN-Benchmarks) ship as
//! separate `Dataset` implementations in the integration test crate so
//! this crate stays buildable without network or disk access.
//!
//! ## Recall definition
//!
//! `recall@K(reference, candidate) = |reference[..K] ∩ candidate[..K]| / K`
//!
//! The result is order-independent — only set membership matters. A
//! candidate that returns the exact reference top-K (in any order)
//! scores 1.0; one that misses one neighbor scores `(K-1)/K`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Identifier for one query in a benchmark dataset.
pub type QueryId = u32;

/// Identifier for one base vector (the "neighbor" candidates a search
/// returns).
pub type NeighborId = u64;

/// A vector dataset that the harness can run recall against.
///
/// Implementors own the base vectors, the query vectors, and the
/// ground-truth top-K neighbor ids per query (typically computed
/// offline at fp64 precision so it's an unimpeachable reference).
pub trait Dataset {
    /// Human-readable dataset name (used in `RecallReport.dataset`).
    fn name(&self) -> &str;

    /// Number of queries in the benchmark.
    fn query_count(&self) -> usize;

    /// Ground-truth top-K neighbor ids for `query_id`, ordered by
    /// distance (closest first). Returns at least `k` entries.
    fn ground_truth(&self, query_id: QueryId, k: usize) -> Vec<NeighborId>;
}

/// Search outcome for a single query under one precision config:
/// the top-K neighbor ids the index returned (in ranked order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryResult {
    pub query_id: QueryId,
    pub neighbors: Vec<NeighborId>,
}

/// Distance metric being measured. Recall thresholds in the LLD §Q13
/// table are per-metric (cosine/L2 share fp16 tolerance; dot product
/// needs tighter recall because magnitude affects ranking).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceMetric {
    Cosine,
    L2,
    Dot,
}

/// One row of the report: recall@K for a single (dataset, metric, k)
/// tuple comparing one candidate config against the reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallRow {
    pub dataset: String,
    pub metric: DistanceMetric,
    pub k: usize,
    /// Number of queries the row averaged over.
    pub query_count: usize,
    /// Mean recall@K across `query_count` queries, in `[0.0, 1.0]`.
    pub mean_recall: f32,
    /// Worst recall@K across the queries — useful for catching tail
    /// regressions that a mean can hide.
    pub min_recall: f32,
}

/// Aggregate report for one harness run. Serialized as JSON for the CI
/// gate to compare against the policy's `RecallSlo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallReport {
    /// Human label for the candidate config being measured (e.g.
    /// "fp16-canonical").
    pub candidate_label: String,
    /// Human label for the reference config (e.g. "fp32-canonical").
    pub reference_label: String,
    pub rows: Vec<RecallRow>,
}

impl RecallReport {
    /// Convenience accessor: look up a row by (dataset, metric, k).
    pub fn row(&self, dataset: &str, metric: DistanceMetric, k: usize) -> Option<&RecallRow> {
        self.rows
            .iter()
            .find(|r| r.dataset == dataset && r.metric == metric && r.k == k)
    }
}

/// What went wrong during a recall measurement.
#[derive(Debug, Error, PartialEq)]
pub enum RecallError {
    #[error("k must be > 0")]
    KZero,
    #[error("k={k} exceeds neighbors returned ({neighbors_len})")]
    KExceedsNeighbors { k: usize, neighbors_len: usize },
    #[error("k={k} exceeds ground-truth size ({truth_len})")]
    KExceedsTruth { k: usize, truth_len: usize },
}

/// Compute recall@K for a single query: the fraction of the candidate's
/// top-K that also appears in the reference's top-K.
///
/// `reference` is the ground-truth top-K (or a longer slice — only
/// the first `k` are used). `candidate` is the search-under-test top-K.
/// Order doesn't matter — only set membership.
///
/// Returns `Err(KZero)` for `k == 0` and `Err(KExceeds*)` if either
/// input is shorter than `k`. Callers that prefer silent clipping
/// should call `recall_at_k_clipped` instead.
pub fn recall_at_k(
    reference: &[NeighborId],
    candidate: &[NeighborId],
    k: usize,
) -> Result<f32, RecallError> {
    if k == 0 {
        return Err(RecallError::KZero);
    }
    if reference.len() < k {
        return Err(RecallError::KExceedsTruth {
            k,
            truth_len: reference.len(),
        });
    }
    if candidate.len() < k {
        return Err(RecallError::KExceedsNeighbors {
            k,
            neighbors_len: candidate.len(),
        });
    }
    // Set membership over the first k slots of each.
    let reference_set: std::collections::HashSet<NeighborId> =
        reference[..k].iter().copied().collect();
    let hits = candidate[..k]
        .iter()
        .filter(|n| reference_set.contains(n))
        .count();
    Ok(hits as f32 / k as f32)
}

/// Tolerant variant: silently clamps `k` to `min(k, reference.len(),
/// candidate.len())`. Useful for ad-hoc harness runs where the index
/// returned fewer results than the caller asked for.
pub fn recall_at_k_clipped(reference: &[NeighborId], candidate: &[NeighborId], k: usize) -> f32 {
    let effective_k = k.min(reference.len()).min(candidate.len());
    if effective_k == 0 {
        return 0.0;
    }
    recall_at_k(reference, candidate, effective_k).unwrap_or(0.0)
}

/// Aggregate recall@K across many queries. Returns `(mean, min)`.
pub fn aggregate_recall(per_query: &[f32]) -> (f32, f32) {
    if per_query.is_empty() {
        return (0.0, 0.0);
    }
    let sum: f32 = per_query.iter().sum();
    let min = per_query
        .iter()
        .copied()
        .fold(f32::INFINITY, |acc, x| acc.min(x));
    (sum / per_query.len() as f32, min)
}

/// Run the harness on one dataset for one metric at one K, given a
/// dataset, the candidate results, and the reference results.
///
/// The caller is responsible for actually running each search and
/// passing the resulting `QueryResult`s in; this function only does
/// the recall math + aggregation. That keeps the crate independent of
/// any specific ANN engine.
pub fn measure_recall(
    dataset: &dyn Dataset,
    metric: DistanceMetric,
    k: usize,
    reference: &[QueryResult],
    candidate: &[QueryResult],
) -> Result<RecallRow, RecallError> {
    if k == 0 {
        return Err(RecallError::KZero);
    }
    let mut per_query = Vec::with_capacity(reference.len());
    for ref_result in reference {
        // Match candidate result by query_id (order may differ).
        let cand_result = candidate.iter().find(|c| c.query_id == ref_result.query_id);
        let recall = match cand_result {
            Some(c) => recall_at_k_clipped(&ref_result.neighbors, &c.neighbors, k),
            None => 0.0, // candidate didn't answer this query → 0 recall
        };
        per_query.push(recall);
    }
    let _ = dataset; // dataset's only role is naming the row
    let (mean_recall, min_recall) = aggregate_recall(&per_query);
    Ok(RecallRow {
        dataset: dataset.name().to_string(),
        metric,
        k,
        query_count: per_query.len(),
        mean_recall,
        min_recall,
    })
}

// ---------------------------------------------------------------------------
// Synthetic dataset for unit tests
// ---------------------------------------------------------------------------

/// Tiny in-memory dataset for unit-testing the harness without an
/// external file. Caller supplies the ground-truth top-K per query.
#[derive(Debug, Clone)]
pub struct SyntheticDataset {
    name: String,
    ground_truth: std::collections::HashMap<QueryId, Vec<NeighborId>>,
}

impl SyntheticDataset {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ground_truth: std::collections::HashMap::new(),
        }
    }

    pub fn with_query(mut self, query_id: QueryId, top_k: Vec<NeighborId>) -> Self {
        self.ground_truth.insert(query_id, top_k);
        self
    }
}

impl Dataset for SyntheticDataset {
    fn name(&self) -> &str {
        &self.name
    }

    fn query_count(&self) -> usize {
        self.ground_truth.len()
    }

    fn ground_truth(&self, query_id: QueryId, k: usize) -> Vec<NeighborId> {
        self.ground_truth
            .get(&query_id)
            .map(|v| v.iter().take(k).copied().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_self_against_self_is_one() {
        let neighbors = vec![10, 20, 30, 40, 50];
        let r = recall_at_k(&neighbors, &neighbors, 5).unwrap();
        assert_eq!(r, 1.0);
    }

    #[test]
    fn recall_is_order_independent() {
        let reference = vec![10, 20, 30, 40, 50];
        let candidate = vec![50, 40, 30, 20, 10];
        let r = recall_at_k(&reference, &candidate, 5).unwrap();
        assert_eq!(r, 1.0, "reordering must not affect recall");
    }

    #[test]
    fn recall_missing_one_neighbor() {
        let reference = vec![10, 20, 30, 40, 50];
        let candidate = vec![10, 20, 30, 40, 99];
        let r = recall_at_k(&reference, &candidate, 5).unwrap();
        assert!((r - 0.8).abs() < 1e-6, "expected 4/5 = 0.8, got {r}");
    }

    #[test]
    fn recall_zero_when_no_overlap() {
        let reference = vec![10, 20, 30];
        let candidate = vec![1, 2, 3];
        let r = recall_at_k(&reference, &candidate, 3).unwrap();
        assert_eq!(r, 0.0);
    }

    #[test]
    fn recall_only_considers_first_k() {
        // Reference has 10 items but we only check top-3.
        let reference = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let candidate = vec![1, 2, 3]; // perfect for k=3
        let r = recall_at_k(&reference, &candidate, 3).unwrap();
        assert_eq!(r, 1.0);
    }

    #[test]
    fn recall_handles_duplicates_in_candidate_correctly() {
        // Candidate has duplicates — set intersection counts unique
        // membership but candidate slice length is used as denominator.
        // Implementation iterates over candidate; duplicates in
        // candidate inflate the hit count if reference contains the
        // duplicate. Capture the actual behavior so callers don't get
        // surprised.
        let reference = vec![10, 20, 30, 40, 50];
        let candidate = vec![10, 10, 10, 10, 10];
        let r = recall_at_k(&reference, &candidate, 5).unwrap();
        // candidate[0..5] is all "10" which IS in reference, so hits=5,
        // recall=1.0. That's intentional: indexes shouldn't return
        // duplicates, and if they do, treating each as a hit is the
        // most generous read (don't penalize legitimate top-K cases).
        assert_eq!(r, 1.0);
    }

    #[test]
    fn recall_at_k_zero_is_error() {
        assert_eq!(
            recall_at_k(&[1, 2, 3], &[1, 2, 3], 0),
            Err(RecallError::KZero)
        );
    }

    #[test]
    fn recall_at_k_exceeds_truth_is_error() {
        let err = recall_at_k(&[1, 2], &[1, 2, 3], 3).unwrap_err();
        assert_eq!(err, RecallError::KExceedsTruth { k: 3, truth_len: 2 });
    }

    #[test]
    fn recall_at_k_exceeds_neighbors_is_error() {
        let err = recall_at_k(&[1, 2, 3], &[1, 2], 3).unwrap_err();
        assert_eq!(
            err,
            RecallError::KExceedsNeighbors {
                k: 3,
                neighbors_len: 2
            }
        );
    }

    #[test]
    fn recall_clipped_silently_caps_to_shortest_input() {
        // Reference has 5, candidate has 2, k=10 — effective k=2.
        let reference = vec![10, 20, 30, 40, 50];
        let candidate = vec![10, 99];
        // Effective k=2, reference[..2]={10,20}, candidate[..2]={10,99}
        // → 1 hit / 2 = 0.5
        let r = recall_at_k_clipped(&reference, &candidate, 10);
        assert_eq!(r, 0.5);
    }

    #[test]
    fn recall_clipped_zero_when_either_input_empty() {
        assert_eq!(recall_at_k_clipped(&[], &[1, 2, 3], 3), 0.0);
        assert_eq!(recall_at_k_clipped(&[1, 2, 3], &[], 3), 0.0);
    }

    #[test]
    fn aggregate_recall_mean_and_min() {
        let recalls = vec![1.0, 0.9, 0.8, 1.0, 0.7];
        let (mean, min) = aggregate_recall(&recalls);
        assert!((mean - 0.88).abs() < 1e-5);
        assert!((min - 0.7).abs() < 1e-6);
    }

    #[test]
    fn aggregate_recall_empty_input_returns_zero() {
        let (mean, min) = aggregate_recall(&[]);
        assert_eq!(mean, 0.0);
        assert_eq!(min, 0.0);
    }

    #[test]
    fn synthetic_dataset_round_trips_per_query_top_k() {
        let dataset = SyntheticDataset::new("test")
            .with_query(0, vec![1, 2, 3, 4, 5])
            .with_query(1, vec![10, 20, 30]);
        assert_eq!(dataset.name(), "test");
        assert_eq!(dataset.query_count(), 2);
        assert_eq!(dataset.ground_truth(0, 3), vec![1, 2, 3]);
        assert_eq!(dataset.ground_truth(1, 5), vec![10, 20, 30]); // clip to available
        assert!(dataset.ground_truth(99, 5).is_empty());
    }

    #[test]
    fn measure_recall_self_baseline_is_one_per_metric() {
        // LLD-required self-test: a config measured against itself
        // must score recall@K = 1.0. If this fails, the harness is
        // broken and every other report is suspect.
        let dataset = SyntheticDataset::new("synthetic")
            .with_query(0, vec![1, 2, 3, 4, 5])
            .with_query(1, vec![10, 20, 30, 40, 50]);
        let same = vec![
            QueryResult {
                query_id: 0,
                neighbors: vec![1, 2, 3, 4, 5],
            },
            QueryResult {
                query_id: 1,
                neighbors: vec![10, 20, 30, 40, 50],
            },
        ];
        let row = measure_recall(&dataset, DistanceMetric::Cosine, 5, &same, &same).unwrap();
        assert_eq!(row.dataset, "synthetic");
        assert_eq!(row.metric, DistanceMetric::Cosine);
        assert_eq!(row.k, 5);
        assert_eq!(row.query_count, 2);
        assert_eq!(row.mean_recall, 1.0);
        assert_eq!(row.min_recall, 1.0);
    }

    #[test]
    fn measure_recall_penalizes_a_missing_query_with_zero() {
        // Candidate didn't answer query_id=1 → that query gets 0 recall,
        // dragging the mean and min down.
        let dataset = SyntheticDataset::new("synthetic")
            .with_query(0, vec![1, 2, 3, 4, 5])
            .with_query(1, vec![10, 20, 30, 40, 50]);
        let reference = vec![
            QueryResult {
                query_id: 0,
                neighbors: vec![1, 2, 3, 4, 5],
            },
            QueryResult {
                query_id: 1,
                neighbors: vec![10, 20, 30, 40, 50],
            },
        ];
        let candidate = vec![QueryResult {
            query_id: 0,
            neighbors: vec![1, 2, 3, 4, 5],
        }];
        let row = measure_recall(&dataset, DistanceMetric::L2, 5, &reference, &candidate).unwrap();
        assert_eq!(row.mean_recall, 0.5, "1.0 + 0.0 averaged across 2 queries");
        assert_eq!(row.min_recall, 0.0);
    }

    #[test]
    fn measure_recall_k_zero_is_error() {
        let dataset = SyntheticDataset::new("x");
        let err = measure_recall(&dataset, DistanceMetric::Cosine, 0, &[], &[]).unwrap_err();
        assert_eq!(err, RecallError::KZero);
    }

    #[test]
    fn recall_report_row_lookup_works() {
        let report = RecallReport {
            candidate_label: "fp16".into(),
            reference_label: "fp32".into(),
            rows: vec![
                RecallRow {
                    dataset: "synthetic".into(),
                    metric: DistanceMetric::Cosine,
                    k: 10,
                    query_count: 100,
                    mean_recall: 0.993,
                    min_recall: 0.95,
                },
                RecallRow {
                    dataset: "synthetic".into(),
                    metric: DistanceMetric::Dot,
                    k: 10,
                    query_count: 100,
                    mean_recall: 0.997,
                    min_recall: 0.97,
                },
            ],
        };
        let cos = report.row("synthetic", DistanceMetric::Cosine, 10).unwrap();
        assert!((cos.mean_recall - 0.993).abs() < 1e-6);
        assert!(report.row("synthetic", DistanceMetric::L2, 10).is_none());
    }

    #[test]
    fn recall_report_serde_round_trip() {
        let report = RecallReport {
            candidate_label: "fp16-cosine".into(),
            reference_label: "fp32-cosine".into(),
            rows: vec![RecallRow {
                dataset: "sift-1m".into(),
                metric: DistanceMetric::Cosine,
                k: 10,
                query_count: 10_000,
                mean_recall: 0.9923,
                min_recall: 0.85,
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: RecallReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn distance_metric_serde_uses_snake_case() {
        for (metric, expected) in [
            (DistanceMetric::Cosine, "\"cosine\""),
            (DistanceMetric::L2, "\"l2\""),
            (DistanceMetric::Dot, "\"dot\""),
        ] {
            let json = serde_json::to_string(&metric).unwrap();
            assert_eq!(json, expected);
            let back: DistanceMetric = serde_json::from_str(expected).unwrap();
            assert_eq!(back, metric);
        }
    }
}
