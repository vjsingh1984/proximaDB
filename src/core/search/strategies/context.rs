//! Search Context Abstraction
//!
//! Provides a trait-based abstraction over search query parameters,
//! following the Dependency Inversion Principle.

use crate::compute::distance_computation::DistanceMetric;
use crate::core::search::{BlockPruneConfig, FilterExpression, SearchMode, SearchParams};

/// Abstract search context trait
///
/// This trait abstracts the query parameters, allowing search strategies
/// to work with different query representations without tight coupling.
///
/// # Design Philosophy
///
/// - **Abstraction**: Strategies depend on this trait, not concrete SearchParams
/// - **Extensibility**: New query types can implement this trait
/// - **Testability**: Easy to mock for unit testing
pub trait SearchContext: Send + Sync {
    /// Get the query vector
    fn query_vector(&self) -> &[f32];

    /// Get number of results to return
    fn top_k(&self) -> usize;

    /// Get distance metric for similarity calculation
    fn distance_metric(&self) -> DistanceMetric;

    /// Get optional filter expression for metadata filtering
    fn filter_expression(&self) -> Option<&FilterExpression>;

    /// Get the search mode (exact, approximate, adaptive)
    fn search_mode(&self) -> &SearchMode;

    /// Get block pruning configuration
    fn block_prune_config(&self) -> &BlockPruneConfig;

    /// Get accuracy threshold (0.0 to 1.0)
    fn accuracy_threshold(&self) -> f32 {
        0.95 // Default 95% accuracy
    }

    /// Get timeout in milliseconds
    fn timeout_ms(&self) -> Option<u64> {
        Some(5000) // Default 5 second timeout
    }

    /// Check if two-stage search is enabled
    fn enable_two_stage(&self) -> bool {
        true // Default enabled
    }

    /// Check if progressive search is enabled
    fn enable_progressive_search(&self) -> bool {
        false // Default disabled
    }

    /// Get optimization hints as key-value pairs
    fn optimization_hints(&self) -> Option<&std::collections::HashMap<String, serde_json::Value>> {
        None
    }

    /// Check if this context requires ordering (affects result processing)
    fn requires_ordering(&self) -> bool {
        true // Default: results should be ordered
    }

    /// Get vector dimension
    fn dimension(&self) -> usize {
        self.query_vector().len()
    }

    /// Check if search mode is exact
    fn is_exact_mode(&self) -> bool {
        matches!(self.search_mode(), SearchMode::Exact)
    }

    /// Calculate effective nprobe for approximate search
    fn effective_nprobe(&self, num_partitions: usize, dataset_size: usize) -> usize {
        self.search_mode().effective_nprobe(num_partitions, dataset_size)
    }
}

/// Default implementation of SearchContext backed by SearchParams
pub struct SearchContextImpl {
    query_vector: Vec<f32>,
    top_k: usize,
    distance_metric: DistanceMetric,
    filter_expression: Option<FilterExpression>,
    search_mode: SearchMode,
    block_prune_config: BlockPruneConfig,
    accuracy_threshold: f32,
    timeout_ms: Option<u64>,
    enable_two_stage: bool,
    enable_progressive_search: bool,
    optimization_hints: Option<std::collections::HashMap<String, serde_json::Value>>,
    requires_ordering: bool,
}

impl SearchContextImpl {
    /// Create from SearchParams
    pub fn from_params(params: &SearchParams) -> Option<Self> {
        let query_vector = params.first_query_vector()?.clone();

        Some(Self {
            query_vector,
            top_k: params.top_k.unwrap_or(10),
            distance_metric: params.distance_metric.unwrap_or(DistanceMetric::Cosine),
            filter_expression: params.filter_expression.clone(),
            search_mode: params.search_mode.clone(),
            block_prune_config: params.block_prune.clone(),
            accuracy_threshold: params.accuracy_threshold.unwrap_or(0.95),
            timeout_ms: params.timeout_ms,
            enable_two_stage: params.enable_two_stage.unwrap_or(true),
            enable_progressive_search: params.enable_progressive_search.unwrap_or(false),
            optimization_hints: params.custom_hints.clone(),
            requires_ordering: params.requires_ordering.unwrap_or(true),
        })
    }

    /// Create a new context with explicit parameters
    pub fn new(
        query_vector: Vec<f32>,
        top_k: usize,
        distance_metric: DistanceMetric,
    ) -> Self {
        Self {
            query_vector,
            top_k,
            distance_metric,
            filter_expression: None,
            search_mode: SearchMode::Exact,
            block_prune_config: BlockPruneConfig::default(),
            accuracy_threshold: 0.95,
            timeout_ms: Some(5000),
            enable_two_stage: true,
            enable_progressive_search: false,
            optimization_hints: None,
            requires_ordering: true,
        }
    }

    /// Builder method: set filter expression
    pub fn with_filter(mut self, filter: FilterExpression) -> Self {
        self.filter_expression = Some(filter);
        self
    }

    /// Builder method: set search mode
    pub fn with_search_mode(mut self, mode: SearchMode) -> Self {
        self.search_mode = mode;
        self
    }

    /// Builder method: set accuracy threshold
    pub fn with_accuracy_threshold(mut self, threshold: f32) -> Self {
        self.accuracy_threshold = threshold;
        self
    }

    /// Builder method: set timeout
    pub fn with_timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = Some(timeout);
        self
    }

    /// Builder method: enable progressive search
    pub fn with_progressive_search(mut self, enabled: bool) -> Self {
        self.enable_progressive_search = enabled;
        self
    }
}

impl SearchContext for SearchContextImpl {
    fn query_vector(&self) -> &[f32] {
        &self.query_vector
    }

    fn top_k(&self) -> usize {
        self.top_k
    }

    fn distance_metric(&self) -> DistanceMetric {
        self.distance_metric
    }

    fn filter_expression(&self) -> Option<&FilterExpression> {
        self.filter_expression.as_ref()
    }

    fn search_mode(&self) -> &SearchMode {
        &self.search_mode
    }

    fn block_prune_config(&self) -> &BlockPruneConfig {
        &self.block_prune_config
    }

    fn accuracy_threshold(&self) -> f32 {
        self.accuracy_threshold
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    fn enable_two_stage(&self) -> bool {
        self.enable_two_stage
    }

    fn enable_progressive_search(&self) -> bool {
        self.enable_progressive_search
    }

    fn optimization_hints(&self) -> Option<&std::collections::HashMap<String, serde_json::Value>> {
        self.optimization_hints.as_ref()
    }

    fn requires_ordering(&self) -> bool {
        self.requires_ordering
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_context_creation() {
        let ctx = SearchContextImpl::new(
            vec![1.0, 2.0, 3.0],
            10,
            DistanceMetric::Cosine,
        );

        assert_eq!(ctx.query_vector().len(), 3);
        assert_eq!(ctx.top_k(), 10);
        assert_eq!(ctx.dimension(), 3);
        assert!(ctx.is_exact_mode());
    }

    #[test]
    fn test_search_context_builder() {
        let ctx = SearchContextImpl::new(
            vec![1.0, 2.0, 3.0],
            10,
            DistanceMetric::Euclidean,
        )
        .with_search_mode(SearchMode::approximate())
        .with_accuracy_threshold(0.9)
        .with_timeout_ms(1000);

        assert!(!ctx.is_exact_mode());
        assert_eq!(ctx.accuracy_threshold(), 0.9);
        assert_eq!(ctx.timeout_ms(), Some(1000));
    }

    #[test]
    fn test_effective_nprobe() {
        let ctx = SearchContextImpl::new(
            vec![1.0, 2.0, 3.0],
            10,
            DistanceMetric::Cosine,
        )
        .with_search_mode(SearchMode::Approximate { nprobe: Some(5) });

        // With explicit nprobe
        assert_eq!(ctx.effective_nprobe(100, 10000), 5);

        // With exact mode
        let exact_ctx = SearchContextImpl::new(
            vec![1.0, 2.0, 3.0],
            10,
            DistanceMetric::Cosine,
        );
        assert_eq!(exact_ctx.effective_nprobe(100, 10000), 100);
    }
}
