//! Filtered HybridQuery Builder (Issue #39, SB-09)
//!
//! This module implements a builder for constructing filtered hybrid queries
//! that combine vector similarity search with metadata filtering using the
//! FilterContract interface.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │              HybridQueryBuilder                             │
//! │  - Fluent builder API                                      │
//! │  - Filter contract integration                             │
//! │  - Automatic strategy selection                            │
//! └──────────────────────┬────────────────────────────────────────┘
//!                        │
//!                        ▼
//!     ┌─────────────────────────────────────────┐
//!     │         HybridQuery                     │
//!     │  - Vector search parameters             │
//!     │  - Filter contracts                     │
//!     │  - Candidate set                        │
//!     │  - Execution strategy                   │
//!     └─────────────────────────────────────────┘
//!                        │
//!                        ▼
//!     ┌─────────────────────────────────────────┐
//!     │      Execution                          │
//!     │  - Filter pushdown to HNSW/IVF           │
//!     │  - Candidate set generation             │
//!     │  - Multi-stage filtering                │
//!     └─────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Fluent API**: Chainable builder methods for query construction
//! - **Automatic Strategy Selection**: Choose optimal execution strategy based on filter selectivity
//! - **Filter Pushdown**: Enable efficient filtering at storage layer
//! - **Incremental Results**: Stream-friendly candidate generation
//! - **Zero-Copy**: Minimize data movement during execution

use anyhow::Result;
use tracing::{debug, info, trace};

use crate::core::search::FilterExpression;
use crate::core::search::filter_contract::{CandidateSet, FilterContract, MetadataLookup};
use crate::index::axis::management::manager::HybridQuery as AxisHybridQuery;

/// Selectivity below which filter-first (PreFilter) is used. ADR-011: 5%.
const DEFAULT_FILTER_FIRST_MAX_SELECTIVITY: f64 = 0.05;
const DEFAULT_VECTOR_FIRST_MIN_SELECTIVITY: f64 = 0.50;

/// Selectivity thresholds for automatic hybrid execution strategy selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HybridStrategyPolicy {
    /// Selectivity below this value uses filter-first execution.
    pub filter_first_max_selectivity: f64,
    /// Selectivity above this value uses vector-first execution.
    pub vector_first_min_selectivity: f64,
}

impl Default for HybridStrategyPolicy {
    fn default() -> Self {
        Self {
            filter_first_max_selectivity: DEFAULT_FILTER_FIRST_MAX_SELECTIVITY,
            vector_first_min_selectivity: DEFAULT_VECTOR_FIRST_MIN_SELECTIVITY,
        }
    }
}

impl HybridStrategyPolicy {
    /// Validate threshold ordering and numeric bounds.
    pub fn validate(&self) -> Result<()> {
        if !self.filter_first_max_selectivity.is_finite()
            || !(0.0..=1.0).contains(&self.filter_first_max_selectivity)
        {
            anyhow::bail!("hybrid.filter_first_max_selectivity must be finite and in [0.0, 1.0]");
        }
        if !self.vector_first_min_selectivity.is_finite()
            || !(0.0..=1.0).contains(&self.vector_first_min_selectivity)
        {
            anyhow::bail!("hybrid.vector_first_min_selectivity must be finite and in [0.0, 1.0]");
        }
        if self.filter_first_max_selectivity >= self.vector_first_min_selectivity {
            anyhow::bail!(
                "hybrid.filter_first_max_selectivity must be less than hybrid.vector_first_min_selectivity"
            );
        }
        Ok(())
    }
}

/// Execution strategy for hybrid queries.
///
/// Variants map to the three ADR-011 ANN filtering modes:
/// - `FilterFirst`  → PreFilter  (selectivity < 5%)
/// - `Inline`       → Inline ANN (5–50%): predicate is passed into the HNSW graph walk
/// - `VectorFirst`  → PostFilter (> 50%): oversample ANN then apply predicate to results
///
/// `Parallel` is retained for backward compatibility; new code should use `Inline`
/// for the moderate-selectivity range.  `Auto` delegates to `from_selectivity_with_policy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HybridExecutionStrategy {
    /// Apply the predicate to build a candidate set, then ANN-search within it.
    /// Avoids vector comparisons on non-matching records entirely (ADR-011 PreFilter).
    FilterFirst,

    /// Pass the predicate into the HNSW graph walk (ACORN semantics, ADR-011 Inline).
    /// Correct for 5–50% selectivity; degrades gracefully when the candidate set
    /// shrinks below `ef_construction * 4` by falling back to FilterFirst.
    Inline,

    /// ANN-search with `top_k * oversample_factor` candidates, then post-filter.
    /// Used when the predicate is not selective enough to prune the search space
    /// (ADR-011 PostFilter, selectivity > 50%).
    VectorFirst,

    /// Legacy: run filter generation and vector search concurrently.
    /// Prefer `Inline` for new code in the moderate-selectivity range.
    Parallel,

    /// Automatic strategy selection based on filter selectivity.
    Auto,
}

impl HybridExecutionStrategy {
    /// Select the optimal strategy based on filter selectivity using ADR-011 thresholds.
    pub fn from_selectivity(selectivity: f64) -> Self {
        Self::from_selectivity_with_policy(selectivity, HybridStrategyPolicy::default())
            .unwrap_or(HybridExecutionStrategy::Inline)
    }

    /// Select the optimal strategy using explicit, validated policy thresholds.
    pub fn from_selectivity_with_policy(
        selectivity: f64,
        policy: HybridStrategyPolicy,
    ) -> Result<Self> {
        policy.validate()?;
        if !selectivity.is_finite() || !(0.0..=1.0).contains(&selectivity) {
            anyhow::bail!("hybrid selectivity must be finite and in [0.0, 1.0]");
        }

        if selectivity < policy.filter_first_max_selectivity {
            Ok(HybridExecutionStrategy::FilterFirst)
        } else if selectivity > policy.vector_first_min_selectivity {
            Ok(HybridExecutionStrategy::VectorFirst)
        } else {
            Ok(HybridExecutionStrategy::Inline)
        }
    }

    /// Return the ADR-011 mode name for EXPLAIN output.
    pub fn as_explain_str(self) -> &'static str {
        match self {
            Self::FilterFirst => "PreFilter",
            Self::Inline => "Inline",
            Self::VectorFirst => "PostFilter",
            Self::Parallel => "Parallel",
            Self::Auto => "Auto",
        }
    }
}

/// Hybrid query combining vector search with metadata filtering
#[derive(Debug, Clone)]
pub struct HybridQuery {
    /// Query vector for similarity search
    pub query_vector: Vec<f32>,

    /// Number of results to return
    pub top_k: usize,

    /// Filter contract for metadata filtering
    pub filter: Option<Box<dyn FilterContract>>,

    /// Collection ID
    pub collection_id: String,

    /// Execution strategy
    pub strategy: HybridExecutionStrategy,

    /// Auto-strategy threshold policy
    pub strategy_policy: HybridStrategyPolicy,

    /// Similarity threshold (0.0 to 1.0)
    pub similarity_threshold: f32,

    /// Include expired vectors in results
    pub include_expired: bool,

    /// Enable candidate set optimization
    pub enable_candidate_optimization: bool,

    /// Maximum candidate set size (for memory management)
    pub max_candidate_size: Option<usize>,
}

impl HybridQuery {
    /// Create a new hybrid query builder
    pub fn builder() -> HybridQueryBuilder {
        HybridQueryBuilder::new()
    }

    /// Execute this hybrid query using the provided metadata lookup
    pub async fn execute(&self, metadata_lookup: &dyn MetadataLookup) -> Result<HybridQueryResult> {
        info!("Executing hybrid query with strategy: {:?}", self.strategy);

        let start = std::time::Instant::now();

        // Choose execution strategy
        let strategy = if self.strategy == HybridExecutionStrategy::Auto {
            let selectivity = self
                .filter
                .as_ref()
                .map(|f| f.estimated_selectivity())
                .unwrap_or(1.0);
            HybridExecutionStrategy::from_selectivity_with_policy(
                selectivity,
                self.strategy_policy,
            )?
        } else {
            self.strategy
        };

        debug!("Using execution strategy: {:?}", strategy);

        // Execute based on strategy
        let result = match strategy {
            HybridExecutionStrategy::FilterFirst => {
                self.execute_filter_first(metadata_lookup).await?
            }
            // Inline: predicate is threaded into the HNSW walk. The query layer is
            // responsible for passing a predicate_fn to the index; here we fall through
            // to execute_parallel as the placeholder until HNSW predicate API is wired.
            HybridExecutionStrategy::Inline => self.execute_parallel(metadata_lookup).await?,
            HybridExecutionStrategy::VectorFirst => {
                self.execute_vector_first(metadata_lookup).await?
            }
            HybridExecutionStrategy::Parallel => self.execute_parallel(metadata_lookup).await?,
            HybridExecutionStrategy::Auto => {
                return Err(anyhow::anyhow!("Auto strategy should have been resolved"));
            }
        };

        let execution_time = start.elapsed();

        info!(
            "Hybrid query completed in {:?} with {} results",
            execution_time, result.candidate_count
        );

        Ok(result)
    }

    /// Execute filter-first strategy
    async fn execute_filter_first(
        &self,
        metadata_lookup: &dyn MetadataLookup,
    ) -> Result<HybridQueryResult> {
        trace!("Executing filter-first strategy");

        // 1. Apply filter to generate candidate set
        let candidates = if let Some(ref filter) = self.filter {
            self.generate_candidates_from_filter(filter.as_ref(), metadata_lookup)?
        } else {
            // No filter, use all vectors in collection
            return Err(anyhow::anyhow!(
                "Filter-first strategy requires a filter contract"
            ));
        };

        // 2. Limit candidate set size if needed
        let limited_candidates = if let Some(max_size) = self.max_candidate_size {
            if candidates.len() > max_size {
                self.limit_candidate_set(candidates, max_size)?
            } else {
                candidates
            }
        } else {
            candidates
        };

        // 3. Perform vector search on candidate set
        // (Placeholder - in production, you would execute actual vector search)
        debug!(
            "Filter-first: {} candidates → vector search → top {} results",
            limited_candidates.len(),
            self.top_k
        );

        Ok(HybridQueryResult {
            candidate_count: limited_candidates.len(),
            result_count: self.top_k, // Placeholder
            execution_time_ms: 0,     // Placeholder
            strategy_used: HybridExecutionStrategy::FilterFirst,
        })
    }

    /// Execute vector-first strategy
    async fn execute_vector_first(
        &self,
        _metadata_lookup: &dyn MetadataLookup,
    ) -> Result<HybridQueryResult> {
        trace!("Executing vector-first strategy");

        // 1. Perform vector search to get top candidates
        // (Placeholder - in production, you would execute actual vector search)
        let initial_candidate_count = self.top_k * 10; // Get more than needed for filtering

        debug!(
            "Vector-first: vector search → {} candidates → filter",
            initial_candidate_count
        );

        // 2. Apply filter to vector search results
        // (Placeholder - in production, you would filter the actual results)

        Ok(HybridQueryResult {
            candidate_count: initial_candidate_count,
            result_count: self.top_k,
            execution_time_ms: 0,
            strategy_used: HybridExecutionStrategy::VectorFirst,
        })
    }

    /// Execute parallel strategy
    async fn execute_parallel(
        &self,
        _metadata_lookup: &dyn MetadataLookup,
    ) -> Result<HybridQueryResult> {
        trace!("Executing parallel strategy");

        // 1. Execute filter and vector search in parallel
        // (Placeholder - in production, you would spawn parallel tasks)

        debug!(
            "Parallel: filter generation || vector search → merge → top {} results",
            self.top_k
        );

        // 2. Merge results and apply final ranking

        Ok(HybridQueryResult {
            candidate_count: self.top_k * 5, // Placeholder
            result_count: self.top_k,
            execution_time_ms: 0,
            strategy_used: HybridExecutionStrategy::Parallel,
        })
    }

    /// Generate candidate set from filter
    fn generate_candidates_from_filter(
        &self,
        _filter: &dyn FilterContract,
        _metadata_lookup: &dyn MetadataLookup,
    ) -> Result<Box<dyn CandidateSet>> {
        // For now, this is a placeholder
        // In production, you would:
        // 1. Query the storage engine with the filter
        // 2. Collect matching IDs into a candidate set
        // 3. Return the candidate set

        use crate::core::search::filter_contract::MemoryCandidateSet;

        Ok(Box::new(MemoryCandidateSet::new()))
    }

    /// Limit candidate set size
    fn limit_candidate_set(
        &self,
        candidates: Box<dyn CandidateSet>,
        max_size: usize,
    ) -> Result<Box<dyn CandidateSet>> {
        // For now, just take the first max_size candidates
        // In production, you would use the filter to select the most relevant candidates
        let ids = candidates.to_vec();
        let limited_ids = ids.into_iter().take(max_size).collect();

        use crate::core::search::filter_contract::MemoryCandidateSet;
        Ok(Box::new(MemoryCandidateSet::from_ids(limited_ids)))
    }
}

/// Result of a hybrid query execution
#[derive(Debug, Clone)]
pub struct HybridQueryResult {
    /// Number of candidates processed
    pub candidate_count: usize,

    /// Number of final results
    pub result_count: usize,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,

    /// Strategy that was actually used for execution
    pub strategy_used: HybridExecutionStrategy,
}

/// Builder for constructing hybrid queries
pub struct HybridQueryBuilder {
    /// Query vector
    query_vector: Option<Vec<f32>>,

    /// Top K results
    top_k: Option<usize>,

    /// Filter contract
    filter: Option<Box<dyn FilterContract>>,

    /// Collection ID
    collection_id: Option<String>,

    /// Execution strategy
    strategy: Option<HybridExecutionStrategy>,

    /// Auto-strategy threshold policy
    strategy_policy: HybridStrategyPolicy,

    /// Similarity threshold
    similarity_threshold: Option<f32>,

    /// Include expired vectors
    include_expired: Option<bool>,

    /// Enable candidate optimization
    enable_candidate_optimization: Option<bool>,

    /// Maximum candidate size
    max_candidate_size: Option<usize>,
}

impl HybridQueryBuilder {
    /// Create a new hybrid query builder
    pub fn new() -> Self {
        Self {
            query_vector: None,
            top_k: Some(10),
            filter: None,
            collection_id: None,
            strategy: None,
            strategy_policy: HybridStrategyPolicy::default(),
            similarity_threshold: Some(0.0),
            include_expired: Some(false),
            enable_candidate_optimization: Some(true),
            max_candidate_size: Some(10000),
        }
    }

    /// Set the query vector
    pub fn query_vector(mut self, vector: Vec<f32>) -> Self {
        self.query_vector = Some(vector);
        self
    }

    /// Set the number of results to return
    pub fn top_k(mut self, k: usize) -> Self {
        self.top_k = Some(k);
        self
    }

    /// Set the filter contract
    pub fn filter(mut self, filter: Box<dyn FilterContract>) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Set the filter from a FilterExpression
    pub fn filter_expression(self, expression: FilterExpression) -> Self {
        use crate::core::search::filter_contract::normalize_filter;
        self.filter(normalize_filter(expression))
    }

    /// Set the collection ID
    pub fn collection_id(mut self, id: String) -> Self {
        self.collection_id = Some(id);
        self
    }

    /// Set the execution strategy
    pub fn strategy(mut self, strategy: HybridExecutionStrategy) -> Self {
        self.strategy = Some(strategy);
        self
    }

    /// Set the automatic strategy threshold policy.
    pub fn strategy_policy(mut self, policy: HybridStrategyPolicy) -> Self {
        self.strategy_policy = policy;
        self
    }

    /// Set the similarity threshold
    pub fn similarity_threshold(mut self, threshold: f32) -> Self {
        self.similarity_threshold = Some(threshold);
        self
    }

    /// Set whether to include expired vectors
    pub fn include_expired(mut self, include: bool) -> Self {
        self.include_expired = Some(include);
        self
    }

    /// Enable candidate set optimization
    pub fn enable_candidate_optimization(mut self, enable: bool) -> Self {
        self.enable_candidate_optimization = Some(enable);
        self
    }

    /// Set the maximum candidate set size
    pub fn max_candidate_size(mut self, size: usize) -> Self {
        self.max_candidate_size = Some(size);
        self
    }

    /// Build the hybrid query
    pub fn build(self) -> Result<HybridQuery> {
        let query_vector = self
            .query_vector
            .ok_or_else(|| anyhow::anyhow!("Query vector is required"))?;

        let top_k = self.top_k.unwrap_or(10);
        let collection_id = self
            .collection_id
            .ok_or_else(|| anyhow::anyhow!("Collection ID is required"))?;

        // Auto-select strategy if not specified
        let strategy = if let Some(strategy) = self.strategy {
            strategy
        } else {
            HybridExecutionStrategy::Auto
        };

        Ok(HybridQuery {
            query_vector,
            top_k,
            filter: self.filter,
            collection_id,
            strategy,
            strategy_policy: self.strategy_policy,
            similarity_threshold: self.similarity_threshold.unwrap_or(0.0),
            include_expired: self.include_expired.unwrap_or(false),
            enable_candidate_optimization: self.enable_candidate_optimization.unwrap_or(true),
            max_candidate_size: self.max_candidate_size,
        })
    }
}

impl Default for HybridQueryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert HybridQuery to AXIS HybridQuery for compatibility
impl From<HybridQuery> for AxisHybridQuery {
    fn from(query: HybridQuery) -> Self {
        use crate::index::axis::management::manager::AnnFilteringMode;
        let ann_filtering_mode = match query.strategy {
            HybridExecutionStrategy::FilterFirst => AnnFilteringMode::PreFilter,
            HybridExecutionStrategy::Inline => AnnFilteringMode::Inline,
            _ => AnnFilteringMode::PostFilter,
        };
        AxisHybridQuery {
            collection_id: query.collection_id,
            vector_query: Some(
                crate::index::axis::management::manager::VectorQuery::Dense {
                    vector: query.query_vector,
                    similarity_threshold: query.similarity_threshold,
                },
            ),
            metadata_filters: Vec::new(),
            id_filters: Vec::new(),
            top_k: query.top_k,
            include_expired: query.include_expired,
            ann_filtering_mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::ComparisonOperator;

    #[test]
    fn test_strategy_selection_from_selectivity() {
        // < 5%: PreFilter (ADR-011)
        assert_eq!(
            HybridExecutionStrategy::from_selectivity(0.03),
            HybridExecutionStrategy::FilterFirst
        );

        // 5–50%: Inline HNSW walk (ADR-011)
        assert_eq!(
            HybridExecutionStrategy::from_selectivity(0.30),
            HybridExecutionStrategy::Inline
        );

        // boundary: exactly 5% → Inline (not < 0.05)
        assert_eq!(
            HybridExecutionStrategy::from_selectivity(0.05),
            HybridExecutionStrategy::Inline
        );

        // > 50%: PostFilter (ADR-011)
        assert_eq!(
            HybridExecutionStrategy::from_selectivity(0.70),
            HybridExecutionStrategy::VectorFirst
        );
    }

    #[test]
    fn test_strategy_selection_uses_configured_policy() {
        let policy = HybridStrategyPolicy {
            filter_first_max_selectivity: 0.2,
            vector_first_min_selectivity: 0.8,
        };

        assert_eq!(
            HybridExecutionStrategy::from_selectivity_with_policy(0.15, policy).unwrap(),
            HybridExecutionStrategy::FilterFirst
        );
        assert_eq!(
            HybridExecutionStrategy::from_selectivity_with_policy(0.60, policy).unwrap(),
            HybridExecutionStrategy::Inline
        );
        assert_eq!(
            HybridExecutionStrategy::from_selectivity_with_policy(0.90, policy).unwrap(),
            HybridExecutionStrategy::VectorFirst
        );
    }

    #[test]
    fn test_strategy_policy_validation_rejects_bad_thresholds() {
        let policy = HybridStrategyPolicy {
            filter_first_max_selectivity: 0.8,
            vector_first_min_selectivity: 0.2,
        };

        assert!(policy.validate().is_err());
    }

    #[test]
    fn test_hybrid_query_builder_basic() {
        let query = HybridQuery::builder()
            .query_vector(vec![0.1, 0.2, 0.3])
            .top_k(10)
            .collection_id("test_collection".to_string())
            .build()
            .unwrap();

        assert_eq!(query.query_vector, vec![0.1, 0.2, 0.3]);
        assert_eq!(query.top_k, 10);
        assert_eq!(query.collection_id, "test_collection");
        assert_eq!(query.strategy, HybridExecutionStrategy::Auto);
        assert_eq!(query.strategy_policy, HybridStrategyPolicy::default());
    }

    #[test]
    fn test_hybrid_query_builder_with_filter() {
        let expression = FilterExpression::Comparison {
            field: "price".to_string(),
            operator: ComparisonOperator::LessThan,
            value: serde_json::json!(1000),
        };

        let query = HybridQuery::builder()
            .query_vector(vec![0.1, 0.2, 0.3])
            .top_k(5)
            .collection_id("products".to_string())
            .filter_expression(expression)
            .build()
            .unwrap();

        assert!(query.filter.is_some());
        assert_eq!(query.top_k, 5);
    }

    #[test]
    fn test_hybrid_query_builder_with_strategy() {
        let query = HybridQuery::builder()
            .query_vector(vec![0.1, 0.2, 0.3])
            .top_k(10)
            .collection_id("test".to_string())
            .strategy(HybridExecutionStrategy::FilterFirst)
            .build()
            .unwrap();

        assert_eq!(query.strategy, HybridExecutionStrategy::FilterFirst);
    }

    #[test]
    fn test_hybrid_query_builder_comprehensive() {
        let expression = FilterExpression::Comparison {
            field: "status".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("active"),
        };

        let query = HybridQuery::builder()
            .query_vector(vec![0.5; 384])
            .top_k(20)
            .collection_id("users".to_string())
            .filter_expression(expression)
            .strategy(HybridExecutionStrategy::VectorFirst)
            .similarity_threshold(0.7)
            .include_expired(true)
            .max_candidate_size(5000)
            .build()
            .unwrap();

        assert_eq!(query.query_vector.len(), 384);
        assert_eq!(query.top_k, 20);
        assert_eq!(query.collection_id, "users");
        assert!(query.filter.is_some());
        assert_eq!(query.strategy, HybridExecutionStrategy::VectorFirst);
        assert_eq!(query.similarity_threshold, 0.7);
        assert!(query.include_expired);
        assert_eq!(query.max_candidate_size, Some(5000));
    }
}
