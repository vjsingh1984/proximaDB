//! Recall@k Computation Utilities for Vector Search Benchmarks
//!
//! This module provides utilities for computing recall@k, a standard metric
//! for evaluating approximate nearest neighbor (ANN) search quality.
//!
//! ## Recall@k Definition
//!
//! ```text
//! recall@k = |results_k ∩ ground_truth_k| / k
//! ```
//!
//! Where:
//! - `results_k` = Top-k results from approximate search
//! - `ground_truth_k` = Top-k results from exact/brute-force search
//! - `k` = Number of results requested
//!
//! ## Usage
//!
//! ```rust,ignore
//! use recall_utils::{compute_recall_at_k, compute_ground_truth};
//!
//! // Get ground truth using brute force
//! let ground_truth = compute_ground_truth(&vectors, &query, k);
//!
//! // Run approximate search
//! let results = engine.search(&query, k).await?;
//!
//! // Compute recall
//! let recall = compute_recall_at_k(&results, &ground_truth, k);
//! println!("Recall@{}: {:.2}%", k, recall * 100.0);
//! ```

use proximadb::compute::distance_computation::UnifiedDistanceCompute;
use proximadb::core::VectorRecord;
use std::collections::HashSet;

/// Ground truth result: (vector_id, distance)
pub type GroundTruthResult = (String, f32);

/// Compute L2 distance squared between two vectors
fn l2_distance_squared(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum()
}

/// Compute cosine distance (1 - cosine similarity)
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        1.0 // Max distance for zero vectors
    } else {
        1.0 - (dot_product / (norm_a * norm_b))
    }
}

/// Compute ground truth using brute-force exact search with L2 distance
///
/// Args:
/// - `vectors`: All vectors in the dataset
/// - `query`: Query vector
/// - `k`: Number of nearest neighbors to return
///
/// Returns: Vec of (vector_id, distance) sorted by distance (ascending)
pub fn compute_ground_truth_l2(
    vectors: &[VectorRecord],
    query: &[f32],
    k: usize,
) -> Vec<GroundTruthResult> {
    let mut results: Vec<GroundTruthResult> = vectors
        .iter()
        .map(|v| {
            let dist = l2_distance_squared(query, &v.vector);
            (v.id.clone(), dist)
        })
        .collect();

    // Sort by distance (ascending) and take top-k
    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    results.truncate(k);
    results
}

/// Compute ground truth using cosine distance
pub fn compute_ground_truth_cosine(
    vectors: &[VectorRecord],
    query: &[f32],
    k: usize,
) -> Vec<GroundTruthResult> {
    let mut results: Vec<GroundTruthResult> = vectors
        .iter()
        .map(|v| {
            let dist = cosine_distance(query, &v.vector);
            (v.id.clone(), dist)
        })
        .collect();

    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    results.truncate(k);
    results
}

/// Compute ground truth using UnifiedDistanceCompute (select metric internally)
pub fn compute_ground_truth_with_compute(
    vectors: &[VectorRecord],
    query: &[f32],
    k: usize,
) -> Vec<GroundTruthResult> {
    let distance_compute = UnifiedDistanceCompute::default();

    let mut results: Vec<GroundTruthResult> = vectors
        .iter()
        .map(|v| {
            let dist = distance_compute.distance(query, &v.vector);
            (v.id.clone(), dist)
        })
        .collect();

    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    results.truncate(k);
    results
}

/// Compute recall@k from approximate search results and ground truth
///
/// Args:
/// - `results`: Top-k results from approximate search (vector IDs only)
/// - `ground_truth`: Top-k results from exact search [(id, distance), ...]
/// - `k`: Number of results (for validation)
///
/// Returns: Recall value in range [0.0, 1.0]
///
/// Example:
/// ```rust,ignore
/// let recall = compute_recall_at_k(&result_ids, &ground_truth, k);
/// println!("Recall@{}: {:.2}%", k, recall * 100.0);
/// ```
pub fn compute_recall_at_k(
    results: &[String],
    ground_truth: &[GroundTruthResult],
    k: usize,
) -> f64 {
    if results.is_empty() || ground_truth.is_empty() {
        return 0.0;
    }

    // Create a set of ground truth IDs for efficient lookup
    let gt_set: HashSet<&str> = ground_truth
        .iter()
        .take(k)
        .map(|(id, _)| id.as_str())
        .collect();

    // Count how many results are in the ground truth
    let intersection = results
        .iter()
        .take(k)
        .filter(|id| gt_set.contains(id.as_str()))
        .count();

    intersection as f64 / k as f64
}

/// Compute recall@k from search results with distances
///
/// This variant accepts results with distances like ground truth format.
pub fn compute_recall_at_k_with_distances(
    results: &[(String, f32)],
    ground_truth: &[(String, f32)],
    k: usize,
) -> f64 {
    if results.is_empty() || ground_truth.is_empty() {
        return 0.0;
    }

    let gt_set: HashSet<&str> = ground_truth
        .iter()
        .take(k)
        .map(|(id, _)| id.as_str())
        .collect();

    let intersection = results
        .iter()
        .take(k)
        .filter(|(id, _)| gt_set.contains(id.as_str()))
        .count();

    intersection as f64 / k as f64
}

/// Compute multiple recall metrics: recall@1, recall@10, recall@100
///
/// Returns a map of k -> recall value
pub fn compute_recall_multi(
    results: &[String],
    ground_truth: &[GroundTruthResult],
    ks: &[usize],
) -> std::collections::HashMap<usize, f64> {
    ks.iter()
        .map(|&k| (k, compute_recall_at_k(results, ground_truth, k)))
        .collect()
}

/// Compute precision@k (variant metric)
///
/// precision@k = |relevant_results| / k
/// where "relevant" = in ground truth
pub fn compute_precision_at_k(
    results: &[String],
    ground_truth: &[GroundTruthResult],
    k: usize,
) -> f64 {
    compute_recall_at_k(results, ground_truth, k)
}

/// Mean Reciprocal Rank (MRR)
///
/// MRR = 1 / |queries| * sum(1 / rank_of_first_relevant)
///
/// Higher is better (1.0 = perfect)
pub fn compute_mrr(results: &[String], ground_truth: &[GroundTruthResult]) -> f64 {
    if results.is_empty() {
        return 0.0;
    }

    let gt_set: HashSet<&str> = ground_truth.iter().map(|(id, _)| id.as_str()).collect();

    // Find first relevant result
    for (rank, id) in results.iter().enumerate() {
        if gt_set.contains(id.as_str()) {
            return 1.0 / (rank + 1) as f64;
        }
    }

    0.0 // No relevant results found
}

/// Normalized Discounted Cumulative Gain (NDCG@k)
///
/// Rewards highly relevant items appearing early in results
pub fn compute_ndcg_at_k(
    results: &[String],
    ground_truth: &[(String, f32)], // distances as relevance (lower = better)
    k: usize,
) -> f64 {
    if results.is_empty() || ground_truth.is_empty() {
        return 0.0;
    }

    // Build relevance map from ground truth (relevance = 1 / (1 + distance))
    let relevance_map: std::collections::HashMap<&str, f64> = ground_truth
        .iter()
        .map(|(id, dist)| (id.as_str(), 1.0 / (1.0 + *dist as f64)))
        .collect();

    // Compute DCG
    let mut dcg = 0.0;
    for (i, id) in results.iter().take(k).enumerate() {
        let rel = relevance_map.get(id.as_str()).copied().unwrap_or(0.0);
        // Discount: log2(i + 2)
        let discount = ((i + 2) as f64).log2();
        dcg += rel / discount;
    }

    // Compute ideal DCG (sorted by relevance)
    let mut ideal_relevance: Vec<f64> = relevance_map.values().copied().collect();
    ideal_relevance.sort_by(|a, b| b.partial_cmp(a).unwrap());
    ideal_relevance.truncate(k);

    let mut idcg = 0.0;
    for (i, rel) in ideal_relevance.iter().enumerate() {
        let discount = ((i + 2) as f64).log2();
        idcg += rel / discount;
    }

    if idcg > 0.0 { dcg / idcg } else { 0.0 }
}

/// Summary statistics for search quality
#[derive(Debug, Clone)]
pub struct SearchQualityMetrics {
    pub k: usize,
    pub recall: f64,
    pub precision: f64,
    pub mrr: f64,
    pub ndcg: f64,
}

impl SearchQualityMetrics {
    /// Compute all metrics at once
    pub fn compute(results: &[String], ground_truth: &[(String, f32)], k: usize) -> Self {
        Self {
            k,
            recall: compute_recall_at_k(results, ground_truth, k),
            precision: compute_precision_at_k(results, ground_truth, k),
            mrr: compute_mrr(results, ground_truth),
            ndcg: compute_ndcg_at_k(results, ground_truth, k),
        }
    }

    /// Format as CSV row
    pub fn to_csv_row(&self) -> String {
        format!(
            "{},{},{:.4},{:.4},{:.4},{:.4}",
            self.k, self.k, self.recall, self.precision, self.mrr, self.ndcg
        )
    }

    /// CSV header for SearchQualityMetrics
    pub fn csv_header() -> String {
        "k,k,recall,precision,mrr,ndcg".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vectors() -> Vec<VectorRecord> {
        vec![
            VectorRecord {
                id: "v1".to_string(),
                vector: vec![1.0, 0.0, 0.0],
                metadata: std::collections::HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
            VectorRecord {
                id: "v2".to_string(),
                vector: vec![0.9, 0.1, 0.0],
                metadata: std::collections::HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
            VectorRecord {
                id: "v3".to_string(),
                vector: vec![0.0, 1.0, 0.0],
                metadata: std::collections::HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
            VectorRecord {
                id: "v4".to_string(),
                vector: vec![0.0, 0.0, 1.0],
                metadata: std::collections::HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
        ]
    }

    #[test]
    fn test_compute_ground_truth() {
        let vectors = make_vectors();
        let query = vec![1.0, 0.0, 0.0];
        let gt = compute_ground_truth_l2(&vectors, &query, 2);

        assert_eq!(gt.len(), 2);
        assert_eq!(gt[0].0, "v1"); // v1 is closest (distance = 0)
        assert_eq!(gt[1].0, "v2"); // v2 is second closest
    }

    #[test]
    fn test_compute_recall_perfect() {
        let ground_truth = vec![
            ("v1".to_string(), 0.0),
            ("v2".to_string(), 0.1),
            ("v3".to_string(), 0.5),
        ];
        let results = vec!["v1".to_string(), "v2".to_string(), "v3".to_string()];
        let recall = compute_recall_at_k(&results, &ground_truth, 3);
        assert!((recall - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_recall_partial() {
        let ground_truth = vec![
            ("v1".to_string(), 0.0),
            ("v2".to_string(), 0.1),
            ("v3".to_string(), 0.5),
            ("v4".to_string(), 0.7),
        ];
        let results = vec!["v1".to_string(), "v5".to_string(), "v3".to_string()];
        let recall = compute_recall_at_k(&results, &ground_truth, 3);
        // v1 and v3 match, v5 doesn't -> 2/3
        assert!((recall - 2.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_recall_zero() {
        let ground_truth = vec![("v1".to_string(), 0.0), ("v2".to_string(), 0.1)];
        let results = vec!["v3".to_string(), "v4".to_string()];
        let recall = compute_recall_at_k(&results, &ground_truth, 2);
        assert_eq!(recall, 0.0);
    }

    #[test]
    fn test_compute_mrr() {
        let ground_truth = vec![("v1".to_string(), 0.0), ("v2".to_string(), 0.1)];
        let results = vec!["v5".to_string(), "v1".to_string(), "v2".to_string()];
        let mrr = compute_mrr(&results, &ground_truth);
        // First relevant (v1) at rank 1 -> 1/(1+1) = 0.5
        assert!((mrr - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_search_quality_metrics() {
        let ground_truth = vec![
            ("v1".to_string(), 0.0),
            ("v2".to_string(), 0.1),
            ("v3".to_string(), 0.5),
        ];
        let results = vec!["v1".to_string(), "v3".to_string()];

        let metrics = SearchQualityMetrics::compute(&results, &ground_truth, 2);
        assert_eq!(metrics.k, 2);
        assert!((metrics.recall - 0.5).abs() < 0.001); // 1/2 match
        assert_eq!(
            SearchQualityMetrics::csv_header(),
            "k,k,recall,precision,mrr,ndcg"
        );
    }
}
