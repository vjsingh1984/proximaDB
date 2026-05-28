//! Configuration types for vector search operations.

use crate::query::query_optimizer::OptimizationGoal;

/// Unified search configuration that works for SQL, REST, and gRPC.
#[derive(Debug, Clone)]
pub struct UnifiedSearchConfig {
    /// Optimization goal (speed vs accuracy)
    pub optimization_goal: OptimizationGoal,
    /// Enable progressive quantization search
    pub progressive_search: bool,
    /// Custom recall targets for progressive search
    pub progressive_recalls: Option<crate::core::search::ProgressiveRecalls>,
    /// Include vectors in results
    pub include_vectors: bool,
    /// Include metadata in results
    pub include_metadata: bool,
    /// Search scenario hint
    pub scenario: Option<String>,
    /// Search mode for accuracy vs speed tradeoff (LanceDB-inspired IVF optimization)
    /// - Exact: 100% recall, searches all partitions (default)
    /// - Approximate { nprobe }: Faster search with configurable partition count
    /// - Adaptive { threshold }: Auto-selects based on dataset size
    pub search_mode: crate::core::search::SearchMode,
    /// Vector Object Economy freshness mode (Phase 5). `None` means
    /// "use the service-layer default", currently
    /// [`VectorFreshnessMode::Strong`] — every search merges the WAL
    /// delta with directory-routed candidates so writes acknowledged
    /// by the canonical WAL are immediately visible.
    ///
    /// [`VectorFreshnessMode::Strong`]: crate::core::search::VectorFreshnessMode::Strong
    pub freshness_mode: Option<crate::core::search::VectorFreshnessMode>,
}

impl Default for UnifiedSearchConfig {
    fn default() -> Self {
        Self {
            optimization_goal: OptimizationGoal::Balanced,
            progressive_search: true,
            progressive_recalls: None,
            include_vectors: false,
            include_metadata: true,
            scenario: None,
            search_mode: crate::core::search::SearchMode::default(),
            freshness_mode: None,
        }
    }
}

/// Optional debug/explain hints for vector planning and pruning.
#[derive(Debug, Clone, Default)]
pub struct SearchPlanHints {
    /// Whether the result was served from the query cache.
    pub cache_hit: bool,
    /// Number of SST/HELIX files pruned by the query optimizer.
    pub pruned_files: Option<usize>,
    /// HNSW `ef_search` value used during this query.
    pub ef_search: Option<usize>,
    /// IVF `nprobe` value (number of cells searched).
    pub nprobe: Option<usize>,
    /// Total candidate vectors evaluated before final re-ranking.
    pub candidates: Option<usize>,
    /// Ordered list of progressive search stages executed (e.g., `["binary", "int8", "pq", "full"]`).
    pub progressive_stages: Option<Vec<String>>,
    /// Estimated recall at each progressive stage, if available.
    pub recall_estimates: Option<Vec<f32>>,
    /// ADR-011 ANN filtering mode chosen by the planner ("PreFilter", "Inline", "PostFilter").
    pub ann_filtering_mode: Option<String>,
    /// Phase 5: Vector Object Economy route metadata populated when the
    /// strong-route delta merge runs. `None` when the request didn't
    /// require a delta merge (`StaleOk`, or watermark up to date), or
    /// when the request was served from cache.
    pub vector_object_economy: Option<crate::query::explain::VectorObjectEconomyExplain>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_search_config_default() {
        let config = UnifiedSearchConfig::default();
        assert_eq!(config.optimization_goal, OptimizationGoal::Balanced);
        assert!(config.progressive_search);
        assert!(!config.include_vectors);
        assert!(config.include_metadata);
    }

    #[test]
    fn test_search_plan_hints_default() {
        let hints = SearchPlanHints::default();
        assert!(!hints.cache_hit);
        assert!(hints.pruned_files.is_none());
        assert!(hints.ef_search.is_none());
    }
}
