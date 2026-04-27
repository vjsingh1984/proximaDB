//! Unified Multi-Model Query Engine
//!
//! This module provides cross-model query capabilities combining:
//! - Vector search (similarity queries)
//! - Document queries (JSON path filtering)
//! - Graph traversal (relationship navigation)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Unified Query                             │
//! │   "SELECT * FROM docs WHERE VECTOR_SIMILAR(...) AND         │
//! │    GRAPH_CONNECTED(source, 'KNOWS', target)"                │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                 Query Decomposition                          │
//! │   Parse → Identify model operations → Create sub-queries    │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!         ┌────────────────────┼────────────────────┐
//!         ▼                    ▼                    ▼
//! ┌──────────────┐    ┌──────────────┐    ┌──────────────┐
//! │ Vector Query │    │Document Query│    │ Graph Query  │
//! │   Engine     │    │   Engine     │    │   Engine     │
//! └──────────────┘    └──────────────┘    └──────────────┘
//!         │                    │                    │
//!         └────────────────────┼────────────────────┘
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Result Fusion                             │
//! │   Merge results by strategy (intersection, union, ranked)   │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Query Examples
//!
//! ```sql
//! -- Hybrid vector + document query
//! SELECT * FROM products
//! WHERE $.category = 'electronics'
//!   AND VECTOR_SIMILAR($.embedding, ?, 0.8)
//! ORDER BY VECTOR_DISTANCE($.embedding, ?) ASC
//! LIMIT 10;
//!
//! -- Cross-model join (document + graph)
//! SELECT d.*, g.relationship
//! FROM documents.users d
//! JOIN GRAPH knowledge ON d.id = GRAPH_START(knowledge)
//! WHERE GRAPH_TRAVERSE(knowledge, 'KNOWS', 2)
//!   AND d.$.status = 'active';
//!
//! -- Vector + graph fusion
//! SELECT v.id, v.score, g.path
//! FROM vectors.embeddings v
//! WHERE VECTOR_SIMILAR(v.vector, ?, 0.9)
//!   AND EXISTS (
//!     SELECT 1 FROM GRAPH relations
//!     WHERE GRAPH_CONNECTED(v.id, 'RELATED_TO', ?)
//!   );
//! ```

pub mod ast;
pub mod decomposition;
pub mod evolutionary;
pub mod executor;
pub mod fusion;
pub mod learned_fusion;
pub mod lower; // UQL to MultiModelPlan lowering (Issue #45, SB-15)
pub mod optimizer;
pub mod plan_execution_cache;
pub mod reranking;
pub mod uql;

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info};

pub use ast::{DataModel, MultiModelQuery, QueryComponent};
pub use decomposition::QueryDecomposer;
pub use executor::ParallelExecutor;
pub use fusion::{FusionStrategy, ResultFuser};
pub use learned_fusion::{
    FeedbackSignal, FusionFeatures, FusionModelType, LearnedFusion, LearnedFusionConfig,
    TrainingMetrics, TrainingSample,
};
pub use reranking::{CrossModalReranker, QueryContext, QueryIntent, RerankConfig, RerankedResult};
pub use uql::{UQLParser, UQLStatement};

use crate::services::operations::vectors::VectorOperationsService;
use crate::storage::document::DocumentService;
use crate::storage::traits::UnifiedStorageEngine;

/// Unified query engine for cross-model queries
pub struct UnifiedQueryEngine {
    /// Vector/document storage engine (Phase 2: direct engine access)
    #[allow(dead_code)]
    storage_engine: Arc<dyn UnifiedStorageEngine>,
    /// Vector operations service for vector searches (optional)
    vector_ops: Option<Arc<VectorOperationsService>>,
    /// Document service for JSON document queries
    document_service: Arc<DocumentService>,
    /// Query decomposer
    decomposer: QueryDecomposer,
    /// Parallel executor
    executor: ParallelExecutor,
    /// Result fuser
    fuser: ResultFuser,
    /// Configuration
    config: UnifiedQueryConfig,
    /// Optional query optimizer. When attached via [`with_optimizer`],
    /// `execute` and `execute_with_fusion` reorder components according
    /// to the optimizer's plan and feed the executor's wall-clock time
    /// back into the optimizer's measured-fitness cache (TD-047 sub A).
    /// When `None`, behavior is unchanged from pre-optimizer code paths.
    optimizer: Option<Arc<optimizer::QueryOptimizer>>,
}

/// Configuration for unified query engine
#[derive(Debug, Clone)]
pub struct UnifiedQueryConfig {
    /// Maximum parallel sub-queries
    pub max_parallel_queries: usize,
    /// Default fusion strategy
    pub default_fusion: FusionStrategy,
    /// Query timeout in milliseconds
    pub query_timeout_ms: u64,
    /// Enable query caching
    pub enable_cache: bool,
    /// Maximum cache entries
    pub max_cache_entries: usize,
}

impl Default for UnifiedQueryConfig {
    fn default() -> Self {
        Self {
            max_parallel_queries: 4,
            default_fusion: FusionStrategy::Intersection,
            query_timeout_ms: 30000,
            enable_cache: true,
            max_cache_entries: 1000,
        }
    }
}

impl UnifiedQueryEngine {
    /// Create a new unified query engine with full vector search support
    pub fn new(
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        vector_ops: Arc<VectorOperationsService>,
        document_service: Arc<DocumentService>,
        config: UnifiedQueryConfig,
    ) -> Self {
        Self {
            storage_engine: storage_engine.clone(),
            vector_ops: Some(vector_ops),
            document_service,
            decomposer: QueryDecomposer::new(),
            executor: ParallelExecutor::new(config.max_parallel_queries),
            fuser: ResultFuser::new(config.default_fusion.clone()),
            config,
            optimizer: None,
        }
    }

    /// Create a new unified query engine without vector operations (document + graph only)
    ///
    /// Note: This constructor is provided for backward compatibility but vector search
    /// queries will return empty results. Use `new()` with VectorOperationsService for
    /// full functionality.
    pub fn without_vector_ops(
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        document_service: Arc<DocumentService>,
        config: UnifiedQueryConfig,
    ) -> Self {
        Self {
            storage_engine: storage_engine.clone(),
            vector_ops: None,
            document_service,
            decomposer: QueryDecomposer::new(),
            executor: ParallelExecutor::new(config.max_parallel_queries),
            fuser: ResultFuser::new(config.default_fusion.clone()),
            config,
            optimizer: None,
        }
    }

    /// Attach a [`QueryOptimizer`](optimizer::QueryOptimizer) to this engine.
    ///
    /// When attached, [`execute`](Self::execute) and
    /// [`execute_with_fusion`](Self::execute_with_fusion) will:
    ///
    /// 1. Ask the optimizer for an [`OptimizedPlan`](optimizer::OptimizedPlan).
    /// 2. Reorder `query.components` by `plan.execution_order`.
    /// 3. Wrap the executor call with
    ///    [`time_and_record_if_ok`](optimizer::QueryOptimizer::time_and_record_if_ok)
    ///    so successful runs feed wall-clock measurements back into the
    ///    optimizer's [`PlanExecutionCache`](plan_execution_cache::PlanExecutionCache).
    ///
    /// When the optimizer is *not* attached, the engine behaves exactly as
    /// before this method existed — this is the no-op default to keep
    /// existing callers untouched.
    ///
    /// Builder style. Returns `self` so attachment can be chained on
    /// construction.
    pub fn with_optimizer(mut self, optimizer: Arc<optimizer::QueryOptimizer>) -> Self {
        self.optimizer = Some(optimizer);
        self
    }

    /// Borrow the attached optimizer, if any. Useful for inspecting the
    /// measured-fitness cache from outside (telemetry, tests).
    pub fn optimizer(&self) -> Option<&Arc<optimizer::QueryOptimizer>> {
        self.optimizer.as_ref()
    }

    /// Execute a multi-model query
    pub async fn execute(&self, query: &str) -> Result<QueryResult> {
        info!("Executing unified query: {}", query);

        // 1. Parse and decompose the query
        let mut multi_model_query = self.decomposer.decompose(query)?;
        debug!(
            "Decomposed into {} components",
            multi_model_query.components.len()
        );

        // 2. Optionally reorder by the optimizer's plan (TD-047 sub A).
        let execution_order =
            self.apply_optimizer_reorder(&mut multi_model_query)?;

        // 3. Execute sub-queries in parallel, optionally feeding the
        //    optimizer's measured-fitness cache on success.
        let sub_results = self
            .run_executor_with_optional_recording(
                &multi_model_query,
                execution_order.as_deref(),
            )
            .await?;

        // 4. Fuse results based on strategy
        let fused_result = self
            .fuser
            .fuse(sub_results, &multi_model_query.fusion_strategy)?;

        Ok(fused_result)
    }

    /// Execute with a specific fusion strategy
    pub async fn execute_with_fusion(
        &self,
        query: &str,
        fusion: FusionStrategy,
    ) -> Result<QueryResult> {
        let mut multi_model_query = self.decomposer.decompose(query)?;
        multi_model_query.fusion_strategy = fusion;

        let execution_order =
            self.apply_optimizer_reorder(&mut multi_model_query)?;

        let sub_results = self
            .run_executor_with_optional_recording(
                &multi_model_query,
                execution_order.as_deref(),
            )
            .await?;

        self.fuser
            .fuse(sub_results, &multi_model_query.fusion_strategy)
    }

    /// If an optimizer is attached, ask it for an `OptimizedPlan` and
    /// reorder `query.components` accordingly. Returns the execution order
    /// in *original-component* indices so callers can later record measured
    /// runtime for that exact plan shape.
    ///
    /// Thin shim around [`reorder_components_with_optimizer`] -- the free
    /// function is the testable seam, this method just plumbs `&self.optimizer`.
    fn apply_optimizer_reorder(
        &self,
        query: &mut MultiModelQuery,
    ) -> Result<Option<Vec<usize>>> {
        reorder_components_with_optimizer(self.optimizer.as_ref(), query)
    }

    /// Run the parallel executor. When an optimizer is attached and an
    /// execution order was produced, wrap the run with
    /// [`QueryOptimizer::time_and_record_if_ok`] so successful executions
    /// feed the measured-fitness cache. Failed runs are deliberately not
    /// recorded (failure-mode timing is unrepresentative).
    async fn run_executor_with_optional_recording(
        &self,
        query: &MultiModelQuery,
        execution_order: Option<&[usize]>,
    ) -> Result<Vec<crate::query::unified::fusion::SubQueryResult>> {
        let fut = self.executor.execute_parallel_with_services(
            query,
            self.vector_ops.clone(),
            self.document_service.clone(),
        );

        match (self.optimizer.as_ref(), execution_order) {
            (Some(optimizer), Some(order)) if !order.is_empty() => {
                // Convert the executor's `Result<_, anyhow::Error>` into
                // a Result the optimizer's helper accepts. The helper
                // records on Ok(_) only.
                optimizer
                    .time_and_record_if_ok(&query.components, order, fut)
                    .await
            }
            _ => fut.await,
        }
    }

    /// Explain the query execution plan
    pub fn explain(&self, query: &str) -> Result<QueryPlan> {
        let multi_model_query = self.decomposer.decompose(query)?;

        // Compute estimated cost before moving fusion_strategy
        let estimated_total_cost = self.estimate_total_cost(&multi_model_query);

        Ok(QueryPlan {
            components: multi_model_query
                .components
                .iter()
                .map(|c| ComponentPlan {
                    model: c.model.clone(),
                    estimated_cost: self.estimate_cost(c),
                    parallelizable: c.is_parallelizable(),
                })
                .collect(),
            fusion_strategy: multi_model_query.fusion_strategy,
            estimated_total_cost,
        })
    }

    fn estimate_cost(&self, component: &QueryComponent) -> f64 {
        // Simple cost estimation based on model type
        match component.model {
            DataModel::Vector => 1.0,   // Vector search is typically fast
            DataModel::Document => 2.0, // Document queries vary
            DataModel::Graph => 3.0,    // Graph traversal can be expensive
            DataModel::Observability | DataModel::TimeSeries => 2.5,
            DataModel::Relational => 1.5,
            DataModel::Event => 2.0,
        }
    }

    fn estimate_total_cost(&self, query: &MultiModelQuery) -> f64 {
        // Parallel execution reduces total cost
        let component_costs: Vec<f64> = query
            .components
            .iter()
            .map(|c| self.estimate_cost(c))
            .collect();

        if query.components.len() <= self.config.max_parallel_queries {
            // All can run in parallel - cost is the max
            component_costs.iter().cloned().fold(0.0, f64::max)
        } else {
            // Some sequential execution needed
            component_costs.iter().sum::<f64>() / self.config.max_parallel_queries as f64
        }
    }
}

/// Result of a unified query
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Result records
    pub records: Vec<UnifiedRecord>,
    /// Total count (if available)
    pub total_count: Option<u64>,
    /// Execution metrics
    pub metrics: QueryMetrics,
}

/// A unified record from any data model
#[derive(Debug, Clone)]
pub struct UnifiedRecord {
    /// Record ID
    pub id: String,
    /// Source model
    pub source_model: DataModel,
    /// Record data as JSON
    pub data: serde_json::Value,
    /// Relevance score (if applicable)
    pub score: Option<f64>,
    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

/// Query execution metrics
#[derive(Debug, Clone, Default)]
pub struct QueryMetrics {
    /// Total execution time in microseconds
    pub total_time_us: u64,
    /// Time per sub-query
    pub sub_query_times: Vec<(DataModel, u64)>,
    /// Number of records scanned
    pub records_scanned: u64,
    /// Number of records returned
    pub records_returned: u64,
    /// Cache hit rate
    pub cache_hit_rate: f64,
}

/// Query execution plan
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// Component plans
    pub components: Vec<ComponentPlan>,
    /// Fusion strategy
    pub fusion_strategy: FusionStrategy,
    /// Estimated total cost
    pub estimated_total_cost: f64,
}

/// Plan for a single query component
#[derive(Debug, Clone)]
pub struct ComponentPlan {
    /// Data model
    pub model: DataModel,
    /// Estimated cost (relative units)
    pub estimated_cost: f64,
    /// Whether this can run in parallel
    pub parallelizable: bool,
}

/// Reorder `query.components` according to a `QueryOptimizer`'s plan.
///
/// Returns `Ok(None)` when:
/// - the optimizer is `None` (no behavioral change vs. pre-optimizer code),
/// - the query has fewer than two components (nothing to reorder).
///
/// Returns `Ok(Some(order))` with the original-index execution order so
/// callers can later record measured runtime against this exact plan shape
/// in [`PlanExecutionCache`](plan_execution_cache::PlanExecutionCache).
///
/// The free-function shape is deliberate: it makes the reorder behavior
/// testable without standing up a full `UnifiedQueryEngine` (storage
/// engine + document service) — only a constructed optimizer is needed.
pub fn reorder_components_with_optimizer(
    optimizer: Option<&Arc<optimizer::QueryOptimizer>>,
    query: &mut MultiModelQuery,
) -> Result<Option<Vec<usize>>> {
    let Some(optimizer) = optimizer else {
        return Ok(None);
    };
    if query.components.len() < 2 {
        return Ok(None);
    }

    let plan = optimizer.optimize(query)?;
    let order = plan.execution_order;

    // Reorder components so the executor processes them in the
    // optimizer-chosen order. The order is already topologically valid
    // (the optimizer respects component dependencies).
    let mut reordered = Vec::with_capacity(query.components.len());
    for &idx in &order {
        reordered.push(query.components[idx].clone());
    }
    query.components = reordered;

    Ok(Some(order))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::unified::ast::{
        ComponentDependency, DistanceMetric, DocumentQueryExpr, JoinType, ModelOperation,
        QueryComponent, VectorSearchExpr, VectorSearchParams,
    };
    use crate::query::unified::optimizer::{OptimizerConfig, QueryOptimizer};

    #[test]
    fn test_default_config() {
        let config = UnifiedQueryConfig::default();
        assert_eq!(config.max_parallel_queries, 4);
        assert!(config.enable_cache);
    }

    // ============================================================
    // TD-047 sub A wiring: reorder_components_with_optimizer
    //
    // These tests exercise the free function directly so we don't need
    // to construct a full UnifiedQueryEngine (which requires a real
    // storage engine + document service) to verify the wiring contract:
    //
    // - No optimizer attached  -> Ok(None), components unchanged.
    // - Fewer than 2 components -> Ok(None), components unchanged.
    // - Optimizer attached + multi-component query -> components are
    //   reordered per the plan, execution_order returned.
    // - Measured-fitness end-to-end: when the cache holds samples that
    //   make order [1, 0] empirically faster, the reorder picks it.
    // ============================================================

    fn vector_component() -> QueryComponent {
        QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "vectors".to_string(),
                query_vector: vec![0.1, 0.2],
                top_k: 10,
                threshold: Some(0.5),
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: vec![],
        }
    }

    fn document_component() -> QueryComponent {
        QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                collection: "docs".to_string(),
                path_filters: vec![],
                text_search: None,
                projection: vec![],
                sort: None,
                limit: None,
            }),
            filters: vec![],
            dependencies: vec![],
        }
    }

    fn empty_query(components: Vec<QueryComponent>) -> MultiModelQuery {
        MultiModelQuery {
            components,
            fusion_strategy: FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        }
    }

    #[test]
    fn reorder_returns_none_without_optimizer() {
        let mut q = empty_query(vec![vector_component(), document_component()]);
        let original_models: Vec<_> = q.components.iter().map(|c| c.model.clone()).collect();

        let result = reorder_components_with_optimizer(None, &mut q).unwrap();
        assert!(result.is_none());
        let after: Vec<_> = q.components.iter().map(|c| c.model.clone()).collect();
        assert_eq!(after, original_models, "no optimizer -> no reorder");
    }

    #[test]
    fn reorder_returns_none_for_single_component_query() {
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        config.enable_measured_fitness = true;
        let optimizer = Arc::new(QueryOptimizer::new(config));

        let mut q = empty_query(vec![vector_component()]);
        let result =
            reorder_components_with_optimizer(Some(&optimizer), &mut q).unwrap();
        assert!(result.is_none(), "single component -> no reorder");
        assert_eq!(q.components.len(), 1);
    }

    #[test]
    fn reorder_returns_none_for_empty_query() {
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        let optimizer = Arc::new(QueryOptimizer::new(config));

        let mut q = empty_query(vec![]);
        let result =
            reorder_components_with_optimizer(Some(&optimizer), &mut q).unwrap();
        assert!(result.is_none());
        assert!(q.components.is_empty());
    }

    #[test]
    fn reorder_picks_measured_faster_order() {
        // The strongest end-to-end check at this layer: seed the
        // optimizer's measured-fitness cache so order [1, 0] is much
        // faster than [0, 1], then confirm the reorder helper picks
        // [1, 0]. This proves the wiring from
        // reorder_components_with_optimizer -> optimizer.optimize ->
        // evolutionary_optimize -> plan_execution_cache.
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        config.enable_measured_fitness = true;
        config.evolutionary_population_size = 12;
        config.evolutionary_generations = 8;
        let optimizer = Arc::new(QueryOptimizer::new(config));

        // 2-component independent query (either order is topologically
        // valid; the search has a real choice).
        let components = vec![vector_component(), document_component()];

        // Seed the cache with measurements that strongly prefer [1, 0].
        let shape =
            crate::query::unified::plan_execution_cache::shape_hash(&components);
        let cache = optimizer.plan_execution_cache().unwrap();
        cache.record(shape, &[0, 1], 100_000);
        cache.record(shape, &[1, 0], 1_000);

        let mut q = empty_query(components.clone());
        let order = reorder_components_with_optimizer(Some(&optimizer), &mut q)
            .unwrap()
            .expect("multi-component + optimizer -> reorder applied");

        assert_eq!(
            order,
            vec![1, 0],
            "measured-fitness should pick order [1, 0] given the seeded cache; got {:?}",
            order
        );
        // Components must be physically reordered so the executor sees
        // doc-then-vector.
        assert_eq!(q.components[0].model, DataModel::Document);
        assert_eq!(q.components[1].model, DataModel::Vector);
    }

    #[test]
    fn reorder_preserves_components_when_dependency_forces_topology() {
        // If component 1 depends on component 0, the optimizer must
        // produce [0, 1] regardless of fitness preferences. This pins
        // the topological-validity contract that downstream callers
        // rely on (e.g. the executor uses prior-component results to
        // satisfy dependent components).
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        let optimizer = Arc::new(QueryOptimizer::new(config));

        let mut c1 = document_component();
        c1.dependencies = vec![ComponentDependency {
            component_index: 0,
            join_field: "id".to_string(),
            join_type: JoinType::Inner,
        }];
        let mut q = empty_query(vec![vector_component(), c1]);

        let order =
            reorder_components_with_optimizer(Some(&optimizer), &mut q).unwrap();
        let order = order.expect("multi-component -> reorder applied");
        assert_eq!(
            order,
            vec![0, 1],
            "dependency forces vector-before-document; got {:?}",
            order
        );
    }

    #[test]
    fn execution_order_indices_address_original_components() {
        // The returned `Vec<usize>` must use the *original* component
        // indices as keys so the optimizer's PlanExecutionCache stays
        // coherent across runs of the same query shape (the cache is
        // keyed on (shape_hash, plan_order_hash) -- if order indices
        // shifted from "original index" to "post-reorder position",
        // every reordered run would miss the cache).
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        let optimizer = Arc::new(QueryOptimizer::new(config));

        let mut q = empty_query(vec![vector_component(), document_component()]);
        let order =
            reorder_components_with_optimizer(Some(&optimizer), &mut q).unwrap();
        let order = order.unwrap();
        // Whatever order the optimizer picks, every entry must be a
        // valid index into the original 0..2 range, no duplicates.
        assert_eq!(order.len(), 2);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1]);
    }
}

// RBAC integration tests
#[cfg(test)]
pub mod rbac_integration_tests;
