//! Adaptive Search Strategy
//!
//! Automatically selects between Exact and Approximate search based on
//! dataset size and query characteristics.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{
    ApproximateSearchStrategy, CandidateProvider, ExactSearchStrategy, ScoredCandidate,
    SearchContext, SearchCostEstimate, SearchStrategy,
};
use crate::core::search::SearchMode;

/// Adaptive search strategy that auto-selects search algorithm
///
/// This strategy implements intelligent algorithm selection based on:
/// - Dataset size (small → exact, large → approximate)
/// - Query requirements (accuracy threshold, timeout)
/// - Resource constraints (memory, CPU)
///
/// # Selection Logic
///
/// 1. If dataset < threshold: Use Exact (100% recall)
/// 2. If dataset >= threshold: Use Approximate (faster)
/// 3. If accuracy_threshold > 0.99: Force Exact
/// 4. If timeout is tight: Prefer Approximate
///
/// # Performance Characteristics
///
/// - Adapts to workload automatically
/// - No manual tuning required
/// - Balances recall vs latency
#[derive(Clone)]
pub struct AdaptiveSearchStrategy {
    /// Threshold for switching to approximate search
    size_threshold: usize,
    /// Exact search strategy instance
    exact_strategy: Arc<ExactSearchStrategy>,
    /// Approximate search strategy instance
    approximate_strategy: Arc<ApproximateSearchStrategy>,
    /// Minimum accuracy for approximate search
    min_accuracy_for_approx: f32,
    /// Maximum time budget for exact search (ms)
    max_exact_time_ms: f64,
}

impl Default for AdaptiveSearchStrategy {
    fn default() -> Self {
        Self {
            size_threshold: 10_000,
            exact_strategy: Arc::new(ExactSearchStrategy::new()),
            approximate_strategy: Arc::new(ApproximateSearchStrategy::new()),
            min_accuracy_for_approx: 0.95,
            max_exact_time_ms: 100.0,
        }
    }
}

impl AdaptiveSearchStrategy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with custom threshold
    pub fn with_threshold(threshold: usize) -> Self {
        Self {
            size_threshold: threshold,
            ..Default::default()
        }
    }

    /// Create with custom strategies
    pub fn with_strategies(
        exact: ExactSearchStrategy,
        approximate: ApproximateSearchStrategy,
    ) -> Self {
        Self {
            exact_strategy: Arc::new(exact),
            approximate_strategy: Arc::new(approximate),
            ..Default::default()
        }
    }

    /// Set size threshold
    pub fn set_threshold(&mut self, threshold: usize) {
        self.size_threshold = threshold;
    }

    /// Get threshold from search mode or use default
    fn get_threshold(&self, ctx: &dyn SearchContext) -> usize {
        match ctx.search_mode() {
            SearchMode::Adaptive { threshold } => *threshold,
            _ => self.size_threshold,
        }
    }

    /// Decide which strategy to use based on context and dataset
    fn select_strategy<'a>(
        &'a self,
        ctx: &dyn SearchContext,
        num_candidates: usize,
    ) -> (&'a dyn SearchStrategy, &'static str) {
        let threshold = self.get_threshold(ctx);

        // Check for accuracy requirement override
        if ctx.accuracy_threshold() > 0.99 {
            tracing::debug!(
                "AdaptiveSearchStrategy: high accuracy threshold ({}) → Exact",
                ctx.accuracy_threshold()
            );
            return (self.exact_strategy.as_ref(), "high_accuracy");
        }

        // Check time budget
        if let Some(timeout) = ctx.timeout_ms() {
            let exact_cost = self.exact_strategy.estimate_cost(ctx, num_candidates);
            if exact_cost.estimated_time_ms > timeout as f64 * 0.5 {
                tracing::debug!(
                    "AdaptiveSearchStrategy: time budget tight ({}ms vs {}ms) → Approximate",
                    exact_cost.estimated_time_ms,
                    timeout
                );
                return (self.approximate_strategy.as_ref(), "time_budget");
            }
        }

        // Size-based selection
        if num_candidates < threshold {
            tracing::debug!(
                "AdaptiveSearchStrategy: {} < {} candidates → Exact",
                num_candidates,
                threshold
            );
            (self.exact_strategy.as_ref(), "small_dataset")
        } else {
            tracing::debug!(
                "AdaptiveSearchStrategy: {} >= {} candidates → Approximate",
                num_candidates,
                threshold
            );
            (self.approximate_strategy.as_ref(), "large_dataset")
        }
    }
}

#[async_trait]
impl SearchStrategy for AdaptiveSearchStrategy {
    fn name(&self) -> &'static str {
        "adaptive"
    }

    async fn execute(
        &self,
        ctx: &dyn SearchContext,
        candidates: &dyn CandidateProvider,
    ) -> Result<Vec<ScoredCandidate>> {
        let num_candidates = candidates.total_candidates();
        let (strategy, reason) = self.select_strategy(ctx, num_candidates);

        tracing::info!(
            "AdaptiveSearchStrategy: selected '{}' (reason: {}, candidates: {})",
            strategy.name(),
            reason,
            num_candidates
        );

        // Execute selected strategy
        strategy.execute(ctx, candidates).await
    }

    fn applies_to(&self, ctx: &dyn SearchContext) -> bool {
        // Adaptive applies to:
        // 1. Adaptive mode explicitly
        // 2. Any mode when used as default strategy
        matches!(ctx.search_mode(), SearchMode::Adaptive { .. })
    }

    fn estimate_cost(&self, ctx: &dyn SearchContext, num_candidates: usize) -> SearchCostEstimate {
        let threshold = self.get_threshold(ctx);

        // Estimate based on which strategy would be selected
        if num_candidates < threshold {
            self.exact_strategy.estimate_cost(ctx, num_candidates)
        } else {
            self.approximate_strategy.estimate_cost(ctx, num_candidates)
        }
    }

    fn optimization_hints(&self) -> Vec<String> {
        vec![
            format!("size_threshold: {}", self.size_threshold),
            format!("min_accuracy_for_approx: {}", self.min_accuracy_for_approx),
            format!("max_exact_time_ms: {}", self.max_exact_time_ms),
            "auto-selects between exact and approximate".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::super::context::SearchContextImpl;
    use super::*;
    use crate::compute::distance_computation::DistanceMetric;

    struct MockProvider {
        count: usize,
        candidates: Vec<(String, Vec<f32>)>,
    }

    impl MockProvider {
        fn new(count: usize) -> Self {
            let candidates = (0..count)
                .map(|i| (format!("v{}", i), vec![i as f32, 0.0, 0.0]))
                .collect();
            Self { count, candidates }
        }
    }

    #[async_trait]
    impl CandidateProvider for MockProvider {
        fn total_candidates(&self) -> usize {
            self.count
        }

        async fn get_all_candidates(&self) -> Result<Vec<(String, Vec<f32>)>> {
            Ok(self.candidates.clone())
        }

        async fn get_partition_candidates(&self, _: &[usize]) -> Result<Vec<(String, Vec<f32>)>> {
            Ok(self.candidates.clone())
        }
    }

    #[tokio::test]
    async fn test_adaptive_selects_exact_for_small_dataset() {
        let strategy = AdaptiveSearchStrategy::with_threshold(1000);

        let ctx = SearchContextImpl::new(vec![1.0, 0.0, 0.0], 10, DistanceMetric::Cosine)
            .with_search_mode(SearchMode::Adaptive { threshold: 1000 });

        let provider = MockProvider::new(100);

        // Should use exact for small dataset
        let (selected, reason) = strategy.select_strategy(&ctx, provider.total_candidates());
        assert_eq!(selected.name(), "exact");
        assert_eq!(reason, "small_dataset");
    }

    #[tokio::test]
    async fn test_adaptive_selects_approximate_for_large_dataset() {
        let strategy = AdaptiveSearchStrategy::with_threshold(1000);

        let ctx = SearchContextImpl::new(vec![1.0, 0.0, 0.0], 10, DistanceMetric::Cosine)
            .with_search_mode(SearchMode::Adaptive { threshold: 1000 });

        let provider = MockProvider::new(10000);

        // Should use approximate for large dataset
        let (selected, reason) = strategy.select_strategy(&ctx, provider.total_candidates());
        assert_eq!(selected.name(), "approximate");
        assert_eq!(reason, "large_dataset");
    }

    #[tokio::test]
    async fn test_adaptive_respects_high_accuracy() {
        let strategy = AdaptiveSearchStrategy::with_threshold(100);

        let ctx = SearchContextImpl::new(vec![1.0, 0.0, 0.0], 10, DistanceMetric::Cosine)
            .with_search_mode(SearchMode::Adaptive { threshold: 100 })
            .with_accuracy_threshold(0.995); // High accuracy

        let provider = MockProvider::new(10000);

        // Should use exact despite large dataset due to high accuracy requirement
        let (selected, reason) = strategy.select_strategy(&ctx, provider.total_candidates());
        assert_eq!(selected.name(), "exact");
        assert_eq!(reason, "high_accuracy");
    }

    #[test]
    fn test_applies_to() {
        let strategy = AdaptiveSearchStrategy::new();

        let adaptive_ctx = SearchContextImpl::new(vec![1.0, 0.0], 10, DistanceMetric::Cosine)
            .with_search_mode(SearchMode::adaptive());

        assert!(strategy.applies_to(&adaptive_ctx));

        let exact_ctx = SearchContextImpl::new(vec![1.0, 0.0], 10, DistanceMetric::Cosine);

        assert!(!strategy.applies_to(&exact_ctx));
    }
}
