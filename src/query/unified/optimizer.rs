//! Root compatibility adapter for the extracted query optimizer runtime.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
pub use proximadb_query::optimizer_support::{
    EstimationMethod, FusionStrategy, OptimizedPlan, OptimizerCollectionStats, OptimizerConfig,
    PlanCache, PlanCacheStats, PushedFilter, QueryHistoryEntry, QueryStatistics,
    SelectivityEstimate, compute_query_hash, select_fusion_strategy,
};
use proximadb_query::{PlanExecutionCache, QueryOptimizerRuntime};

use super::ast::{MultiModelQuery, QueryComponent};

/// Query optimizer for multi-model queries.
pub struct QueryOptimizer {
    inner: QueryOptimizerRuntime,
    llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
}

impl QueryOptimizer {
    /// Create a new query optimizer.
    pub fn new(config: OptimizerConfig) -> Self {
        Self {
            inner: QueryOptimizerRuntime::new(config),
            llm_engine: None,
        }
    }

    /// Add an LLM engine for AI-assisted optimization.
    pub fn with_llm(
        mut self,
        llm_engine: Arc<crate::ai::llm_integration::LLMIntegrationEngine>,
    ) -> Self {
        self.llm_engine = Some(llm_engine);
        self
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self {
            inner: QueryOptimizerRuntime::with_defaults(),
            llm_engine: None,
        }
    }

    /// Create without plan caching.
    pub fn without_cache() -> Self {
        Self {
            inner: QueryOptimizerRuntime::without_cache(),
            llm_engine: None,
        }
    }

    /// Access optimizer configuration.
    pub fn config(&self) -> &OptimizerConfig {
        self.inner.config()
    }

    /// Get statistics collector.
    pub fn statistics(&self) -> &Arc<QueryStatistics> {
        self.inner.statistics()
    }

    /// Get plan cache, if enabled.
    pub fn plan_cache(&self) -> Option<&Arc<PlanCache>> {
        self.inner.plan_cache()
    }

    /// Access the measured-fitness cache.
    pub fn plan_execution_cache(&self) -> Option<&Arc<PlanExecutionCache>> {
        self.inner.plan_execution_cache()
    }

    /// Invalidate all cached plans.
    pub fn invalidate_plan_cache(&self) {
        self.inner.invalidate_plan_cache();
    }

    /// Invalidate cached plans for a specific collection.
    pub fn invalidate_collection_plans(&self, collection: &str) {
        self.inner.invalidate_collection_plans(collection);
    }

    /// Record a measured wall-clock time for a plan that has just executed.
    pub fn record_plan_execution(
        &self,
        components: &[QueryComponent],
        execution_order: &[usize],
        wall_time_us: u64,
    ) {
        self.inner
            .record_plan_execution(components, execution_order, wall_time_us);
    }

    /// Time a query future and record its wall-clock duration on success.
    pub async fn time_and_record_if_ok<F, T, E>(
        &self,
        components: &[QueryComponent],
        execution_order: &[usize],
        fut: F,
    ) -> std::result::Result<T, E>
    where
        F: std::future::Future<Output = std::result::Result<T, E>>,
    {
        self.inner
            .time_and_record_if_ok(components, execution_order, fut)
            .await
    }

    /// Optimize a multi-model query.
    pub async fn optimize(&self, query: &MultiModelQuery) -> Result<OptimizedPlan> {
        self.inner.optimize(query, self.mutation_advisor()).await
    }

    /// Estimate selectivity for a query component.
    pub fn estimate_selectivity(&self, component: &QueryComponent) -> SelectivityEstimate {
        self.inner.estimate_selectivity(component)
    }

    fn mutation_advisor(
        &self,
    ) -> Option<Arc<dyn proximadb_query::evolutionary::EvolutionaryMutationAdvisor>> {
        self.llm_engine
            .as_ref()
            .map(|llm| super::evolutionary::llm_mutation_advisor(llm.clone()))
    }
}

#[async_trait(?Send)]
impl proximadb_query::QueryOptimizationService for QueryOptimizer {
    async fn optimize_query(&self, query: &MultiModelQuery) -> Result<OptimizedPlan> {
        self.optimize(query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::unified::ast::{
        DataModel, DistanceMetric, ModelOperation, VectorSearchExpr, VectorSearchParams,
    };
    use proximadb_query_fusion::FusionStrategy;

    fn make_vector_search_component(threshold: Option<f32>, top_k: u32) -> QueryComponent {
        QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "embeddings".to_string(),
                query_vector: vec![0.1, 0.2, 0.3],
                top_k,
                threshold,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: vec![],
        }
    }

    #[tokio::test]
    async fn test_optimizer_creation() {
        let optimizer = QueryOptimizer::with_defaults();
        assert!(optimizer.config().enable_reordering);
        assert!(optimizer.config().enable_filter_pushdown);
    }

    #[tokio::test]
    async fn test_plan_cache_hit_and_miss() {
        let optimizer = QueryOptimizer::with_defaults();
        let query = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.8), 10)],
            fusion_strategy: FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        optimizer.optimize(&query).await.expect("first plan");
        optimizer.optimize(&query).await.expect("cached plan");

        let stats = optimizer.plan_cache().expect("cache").stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[tokio::test]
    async fn test_time_and_record_delegates_to_extracted_runtime() {
        let mut config = OptimizerConfig::default();
        config.enable_measured_fitness = true;
        config.measured_fitness_max_entries = 8;
        let optimizer = QueryOptimizer::new(config);
        let components = vec![make_vector_search_component(None, 10)];
        let order = vec![0usize];

        let result: std::result::Result<u32, ()> = optimizer
            .time_and_record_if_ok(&components, &order, async { Ok(7) })
            .await;

        assert_eq!(result.unwrap(), 7);
        let cache = optimizer.plan_execution_cache().expect("cache");
        assert_eq!(cache.total_samples(), 1);
    }
}
