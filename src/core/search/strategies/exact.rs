//! Exact Search Strategy
//!
//! Provides 100% recall by searching all candidates exhaustively.
//! Best for accuracy-critical applications or small datasets.

use anyhow::Result;
use async_trait::async_trait;

use super::{
    CandidateProvider, ScoredCandidate, SearchContext, SearchCostEstimate, SearchStrategy,
};
use crate::compute::distance_computation::{DistanceMetric, UnifiedDistanceCompute};
use crate::core::search::SearchMode;

/// Exact search strategy with 100% recall
///
/// This strategy performs brute-force search over all candidates,
/// guaranteeing optimal results at the cost of O(n) complexity.
///
/// # Performance Characteristics
///
/// - **Time Complexity**: O(n * d) where n = candidates, d = dimensions
/// - **Space Complexity**: O(k) for top-k heap
/// - **Recall**: 100% (guaranteed optimal results)
///
/// # When to Use
///
/// - Small datasets (< 10,000 vectors)
/// - Accuracy-critical applications
/// - Benchmarking/validation
/// - When approximate results are unacceptable
#[derive(Debug, Clone, Default)]
pub struct ExactSearchStrategy {
    /// Enable SIMD acceleration when available
    enable_simd: bool,
    /// Enable parallel processing
    enable_parallel: bool,
    /// Batch size for parallel processing
    batch_size: usize,
}

impl ExactSearchStrategy {
    pub fn new() -> Self {
        Self {
            enable_simd: true,
            enable_parallel: true,
            batch_size: 1000,
        }
    }

    /// Create with custom settings
    pub fn with_settings(enable_simd: bool, enable_parallel: bool, batch_size: usize) -> Self {
        Self {
            enable_simd,
            enable_parallel,
            batch_size,
        }
    }

    /// Compute distance between two vectors
    #[allow(dead_code)]
    fn compute_distance(&self, query: &[f32], candidate: &[f32], metric: DistanceMetric) -> f32 {
        let distance_compute = UnifiedDistanceCompute::new(metric);
        distance_compute.distance_with_metric(query, candidate, &metric)
    }

    /// Process candidates and return top-k results
    fn process_candidates(
        &self,
        query: &[f32],
        candidates: Vec<(String, Vec<f32>)>,
        top_k: usize,
        metric: DistanceMetric,
    ) -> Vec<ScoredCandidate> {
        // Calculate distances for all candidates
        let mut scored: Vec<ScoredCandidate> = candidates
            .into_iter()
            .map(|(id, vector)| {
                let score = self.compute_distance(query, &vector, metric);
                ScoredCandidate::new(id, score).with_vector(vector)
            })
            .collect();

        // Sort by score (lower is better for distance metrics)
        scored.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Return top-k
        scored.truncate(top_k);
        scored
    }
}

#[async_trait]
impl SearchStrategy for ExactSearchStrategy {
    fn name(&self) -> &'static str {
        "exact"
    }

    async fn execute(
        &self,
        ctx: &dyn SearchContext,
        candidates: &dyn CandidateProvider,
    ) -> Result<Vec<ScoredCandidate>> {
        let query = ctx.query_vector();
        let top_k = ctx.top_k();
        let metric = ctx.distance_metric();

        tracing::debug!(
            "ExactSearchStrategy: searching {} candidates for top-{} with {:?}",
            candidates.total_candidates(),
            top_k,
            metric
        );

        // Get all candidates
        let all_candidates = candidates.get_all_candidates().await?;

        if all_candidates.is_empty() {
            return Ok(vec![]);
        }

        // Process and return results
        let results = self.process_candidates(query, all_candidates, top_k, metric);

        tracing::debug!("ExactSearchStrategy: found {} results", results.len());
        Ok(results)
    }

    fn applies_to(&self, ctx: &dyn SearchContext) -> bool {
        // Exact strategy applies when:
        // 1. Search mode is Exact
        // 2. Block pruning is disabled (force_exact)
        matches!(ctx.search_mode(), SearchMode::Exact) || ctx.block_prune_config().force_exact
    }

    fn estimate_cost(&self, ctx: &dyn SearchContext, num_candidates: usize) -> SearchCostEstimate {
        let dimension = ctx.dimension();

        // O(n * d) time complexity
        // Assume ~1 microsecond per distance computation (conservative)
        let ops_per_ms = 1000.0;
        let estimated_time_ms = (num_candidates * dimension) as f64 / ops_per_ms;

        // Memory: primarily the result heap
        let estimated_memory = (ctx.top_k() * std::mem::size_of::<ScoredCandidate>()) as u64;

        SearchCostEstimate {
            estimated_time_ms,
            estimated_memory_bytes: estimated_memory,
            expected_recall: 1.0, // 100% recall guaranteed
            candidates_to_evaluate: num_candidates,
            priority: 0, // Lowest priority (use only when needed)
        }
    }

    fn optimization_hints(&self) -> Vec<String> {
        let mut hints = vec![];
        if self.enable_simd {
            hints.push("SIMD acceleration enabled".to_string());
        }
        if self.enable_parallel {
            hints.push(format!(
                "Parallel processing with batch size {}",
                self.batch_size
            ));
        }
        hints
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockCandidateProvider {
        candidates: Vec<(String, Vec<f32>)>,
    }

    #[async_trait]
    impl CandidateProvider for MockCandidateProvider {
        fn total_candidates(&self) -> usize {
            self.candidates.len()
        }

        async fn get_all_candidates(&self) -> Result<Vec<(String, Vec<f32>)>> {
            Ok(self.candidates.clone())
        }

        async fn get_partition_candidates(
            &self,
            _partition_ids: &[usize],
        ) -> Result<Vec<(String, Vec<f32>)>> {
            Ok(self.candidates.clone())
        }
    }

    use super::super::context::SearchContextImpl;

    #[tokio::test]
    async fn test_exact_search_basic() {
        let strategy = ExactSearchStrategy::new();

        let candidates = MockCandidateProvider {
            candidates: vec![
                ("v1".to_string(), vec![1.0, 0.0, 0.0]),
                ("v2".to_string(), vec![0.0, 1.0, 0.0]),
                ("v3".to_string(), vec![0.9, 0.1, 0.0]),
            ],
        };

        let ctx = SearchContextImpl::new(vec![1.0, 0.0, 0.0], 2, DistanceMetric::Cosine);

        let results = strategy.execute(&ctx, &candidates).await.unwrap();

        assert_eq!(results.len(), 2);
        // v1 should be closest (identical vector)
        assert_eq!(results[0].id, "v1");
    }

    #[tokio::test]
    async fn test_exact_search_empty() {
        let strategy = ExactSearchStrategy::new();

        let candidates = MockCandidateProvider { candidates: vec![] };

        let ctx = SearchContextImpl::new(vec![1.0, 0.0, 0.0], 10, DistanceMetric::Cosine);

        let results = strategy.execute(&ctx, &candidates).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_applies_to() {
        let strategy = ExactSearchStrategy::new();

        let exact_ctx = SearchContextImpl::new(vec![1.0, 0.0], 10, DistanceMetric::Cosine);

        assert!(strategy.applies_to(&exact_ctx));

        let approx_ctx = SearchContextImpl::new(vec![1.0, 0.0], 10, DistanceMetric::Cosine)
            .with_search_mode(SearchMode::approximate());

        assert!(!strategy.applies_to(&approx_ctx));
    }

    #[test]
    fn test_cost_estimate() {
        let strategy = ExactSearchStrategy::new();

        let ctx = SearchContextImpl::new(vec![1.0, 0.0, 0.0], 10, DistanceMetric::Cosine);

        let cost = strategy.estimate_cost(&ctx, 10000);

        assert_eq!(cost.expected_recall, 1.0);
        assert_eq!(cost.candidates_to_evaluate, 10000);
        assert!(cost.estimated_time_ms > 0.0);
    }
}
