//! # Search Strategy Pattern
//!
//! This module implements a pluggable search strategy pattern that follows
//! the Open/Closed Principle (OCP) and Dependency Inversion Principle (DIP).
//!
//! ## Design Goals
//!
//! 1. **Extensibility**: New search strategies can be added without modifying engines
//! 2. **Abstraction**: Engines depend on traits, not concrete implementations
//! 3. **Composability**: Strategies can be combined and chained
//! 4. **Testability**: Strategies can be mocked for unit testing
//!
//! ## Architecture
//!
//! ```text
//! SearchContext (query abstraction)
//!       ↓
//! SearchStrategy (algorithm selection)
//!       ↓
//! CandidateProvider (data source)
//!       ↓
//! ScoredCandidate (results)
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! // Register a custom strategy
//! let registry = SearchStrategyRegistry::new();
//! registry.register("my_strategy", Arc::new(MyCustomStrategy::new()));
//!
//! // Execute search with strategy
//! if let Some(strategy) = registry.get("approximate") {
//!     let results = strategy.execute(&ctx, &candidates).await?;
//! }
//! ```

mod adaptive;
mod approximate;
mod context;
mod exact;
mod registry;

pub use adaptive::AdaptiveSearchStrategy;
pub use approximate::ApproximateSearchStrategy;
pub use context::{SearchContext, SearchContextImpl};
pub use exact::ExactSearchStrategy;
pub use registry::SearchStrategyRegistry;

use anyhow::Result;
use async_trait::async_trait;

/// A scored search candidate with vector ID and distance
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    /// Vector ID
    pub id: String,
    /// Distance/score from query vector (lower is better for distance metrics)
    pub score: f32,
    /// Original vector data (optional, for re-ranking)
    pub vector: Option<Vec<f32>>,
    /// Associated metadata (optional)
    pub metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
}

impl ScoredCandidate {
    pub fn new(id: String, score: f32) -> Self {
        Self {
            id,
            score,
            vector: None,
            metadata: None,
        }
    }

    pub fn with_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }

    pub fn with_metadata(
        mut self,
        metadata: std::collections::HashMap<String, serde_json::Value>,
    ) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Cost estimate for search strategy selection
#[derive(Debug, Clone)]
pub struct SearchCostEstimate {
    /// Estimated wall-clock time in milliseconds
    pub estimated_time_ms: f64,
    /// Estimated memory usage in bytes
    pub estimated_memory_bytes: u64,
    /// Expected recall rate (0.0 to 1.0)
    pub expected_recall: f32,
    /// Number of candidates that will be evaluated
    pub candidates_to_evaluate: usize,
    /// Strategy priority (higher = preferred when costs are similar)
    pub priority: i32,
}

impl Default for SearchCostEstimate {
    fn default() -> Self {
        Self {
            estimated_time_ms: f64::MAX,
            estimated_memory_bytes: 0,
            expected_recall: 1.0,
            candidates_to_evaluate: 0,
            priority: 0,
        }
    }
}

/// Trait for providing vector candidates to search strategies
///
/// This abstraction allows strategies to work with different data sources:
/// - In-memory vectors
/// - SST file blocks
/// - Parquet row groups
/// - Quantized representations
#[async_trait]
pub trait CandidateProvider: Send + Sync {
    /// Get total number of candidates available
    fn total_candidates(&self) -> usize;

    /// Get all candidate vectors (for exact search)
    async fn get_all_candidates(&self) -> Result<Vec<(String, Vec<f32>)>>;

    /// Get candidates from specific partitions/blocks (for approximate search)
    async fn get_partition_candidates(
        &self,
        partition_ids: &[usize],
    ) -> Result<Vec<(String, Vec<f32>)>>;

    /// Get number of partitions (for IVF-style search)
    fn num_partitions(&self) -> usize {
        1 // Default: single partition
    }

    /// Get partition centroids (for IVF-style search)
    async fn get_partition_centroids(&self) -> Result<Vec<Vec<f32>>> {
        Ok(vec![]) // Default: no centroids
    }

    /// Check if provider supports quantized search
    fn supports_quantized_search(&self) -> bool {
        false
    }

    /// Get quantized candidates (for progressive search)
    async fn get_quantized_candidates(&self) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(vec![])
    }
}

/// Core search strategy trait
///
/// Implementations define how to search through candidates to find
/// the top-k most similar vectors to a query.
///
/// # Design Philosophy
///
/// - **Single Responsibility**: Each strategy handles one search algorithm
/// - **Stateless**: Strategies should be reusable across multiple queries
/// - **Cost-aware**: Strategies can estimate their execution cost
#[async_trait]
pub trait SearchStrategy: Send + Sync {
    /// Strategy name for logging and debugging
    fn name(&self) -> &'static str;

    /// Execute search and return scored candidates
    ///
    /// # Parameters
    /// - `ctx`: Search context with query parameters
    /// - `candidates`: Provider for vector candidates
    ///
    /// # Returns
    /// - Sorted list of scored candidates (best first)
    async fn execute(
        &self,
        ctx: &dyn SearchContext,
        candidates: &dyn CandidateProvider,
    ) -> Result<Vec<ScoredCandidate>>;

    /// Check if this strategy applies to the given context
    ///
    /// Used by the registry to automatically select strategies.
    fn applies_to(&self, ctx: &dyn SearchContext) -> bool;

    /// Estimate execution cost for strategy selection
    ///
    /// Used when multiple strategies could handle a query.
    fn estimate_cost(&self, ctx: &dyn SearchContext, num_candidates: usize) -> SearchCostEstimate;

    /// Optional: Provide hints for query optimization
    fn optimization_hints(&self) -> Vec<String> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scored_candidate_creation() {
        let candidate = ScoredCandidate::new("vec_1".to_string(), 0.5);
        assert_eq!(candidate.id, "vec_1");
        assert_eq!(candidate.score, 0.5);
        assert!(candidate.vector.is_none());
    }

    #[test]
    fn test_scored_candidate_with_vector() {
        let candidate =
            ScoredCandidate::new("vec_1".to_string(), 0.5).with_vector(vec![1.0, 2.0, 3.0]);
        assert!(candidate.vector.is_some());
        assert_eq!(candidate.vector.unwrap().len(), 3);
    }

    #[test]
    fn test_cost_estimate_default() {
        let cost = SearchCostEstimate::default();
        assert_eq!(cost.expected_recall, 1.0);
        assert_eq!(cost.priority, 0);
    }
}
