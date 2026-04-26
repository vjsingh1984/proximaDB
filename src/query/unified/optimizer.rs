//! Query Optimizer for Multi-Model Queries
//!
//! Provides:
//! - Selectivity estimation for query predicates
//! - Component reordering for efficient execution
//! - Filter pushdown to underlying engines
//! - Query statistics collection and caching
//! - **Query plan caching** with LRU eviction and TTL expiration
//!
//! ## Optimization Strategies
//!
//! 1. **Selectivity-Based Ordering**: Execute most selective queries first
//!    to reduce intermediate result sizes
//!
//! 2. **Filter Pushdown**: Push filter predicates down to storage engines
//!    to reduce I/O and network transfer
//!
//! 3. **Cost-Based Optimization**: Estimate query costs and choose
//!    execution plans that minimize total cost
//!
//! 4. **Plan Caching**: Cache optimized plans for repeated queries to avoid
//!    re-optimization overhead (configurable TTL, LRU eviction)

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::RwLock;
use tracing::{debug, info, trace};

use super::ast::{
    DataModel, DocumentQueryExpr, FilterOperator, GraphTraversalExpr, LogQueryExpr,
    MetricQueryExpr, ModelOperation, MultiModelQuery, PathFilter, QueryComponent, VectorSearchExpr,
};

/// Query optimizer for multi-model queries
pub struct QueryOptimizer {
    /// Statistics collector
    stats: Arc<QueryStatistics>,
    /// Optimization configuration
    config: OptimizerConfig,
    /// Query plan cache (optional, based on config)
    plan_cache: Option<Arc<PlanCache>>,
    /// Measured-runtime cache feeding the evolutionary fitness function
    /// (TD-047 sub A). Populated by callers via [`record_plan_execution`]
    /// after queries complete. `None` when measured fitness is disabled.
    plan_execution_cache: Option<Arc<super::plan_execution_cache::PlanExecutionCache>>,
}

/// Configuration for the query optimizer
#[derive(Debug, Clone)]
pub struct OptimizerConfig {
    /// Enable selectivity-based reordering
    pub enable_reordering: bool,
    /// Enable filter pushdown
    pub enable_filter_pushdown: bool,
    /// Minimum selectivity to consider reordering
    pub min_selectivity_threshold: f64,
    /// Maximum number of components to reorder
    pub max_reorder_components: usize,
    /// Enable query plan caching
    pub enable_plan_cache: bool,
    /// Plan cache TTL in seconds
    pub plan_cache_ttl_secs: u64,
    /// Enable evolutionary optimizer for complex queries
    pub enable_evolutionary_optimizer: bool,
    /// Population size for evolutionary optimizer
    pub evolutionary_population_size: usize,
    /// Number of generations for evolutionary optimizer
    pub evolutionary_generations: usize,
    /// Use measured wall-clock time as evolutionary fitness when a plan has
    /// been observed before (TD-047 sub A). Falls back to estimated cost on
    /// cold cache. Implies `enable_evolutionary_optimizer`.
    pub enable_measured_fitness: bool,
    /// Soft cap on measured-fitness cache entries.
    pub measured_fitness_max_entries: usize,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            enable_reordering: true,
            enable_filter_pushdown: true,
            min_selectivity_threshold: 0.1,
            max_reorder_components: 10,
            enable_plan_cache: true,
            plan_cache_ttl_secs: 300,
            enable_evolutionary_optimizer: false, // Disabled by default
            evolutionary_population_size: 20,
            evolutionary_generations: 5,
            enable_measured_fitness: false, // Disabled by default; opt-in
            measured_fitness_max_entries: 1024,
        }
    }
}

/// Selectivity estimate for a query component
#[derive(Debug, Clone)]
pub struct SelectivityEstimate {
    /// Estimated selectivity (0.0 - 1.0)
    /// Lower values = more selective = fewer results
    pub selectivity: f64,
    /// Confidence in the estimate (0.0 - 1.0)
    pub confidence: f64,
    /// Estimated result count
    pub estimated_rows: u64,
    /// Estimation method used
    pub method: EstimationMethod,
}

/// Method used for selectivity estimation
#[derive(Debug, Clone, PartialEq)]
pub enum EstimationMethod {
    /// Based on collected statistics
    Statistics,
    /// Based on filter predicates
    PredicateAnalysis,
    /// Based on historical query results
    Historical,
    /// Default heuristic estimate
    Heuristic,
}

/// Optimized query plan
#[derive(Debug, Clone)]
pub struct OptimizedPlan {
    /// Reordered components
    pub components: Vec<QueryComponent>,
    /// Component execution order (indices into original components)
    pub execution_order: Vec<usize>,
    /// Selectivity estimates per component
    pub selectivity_estimates: Vec<SelectivityEstimate>,
    /// Pushed down filters per component
    pub pushed_filters: Vec<Vec<PushedFilter>>,
    /// Estimated total cost
    pub estimated_cost: f64,
    /// Optimization notes
    pub notes: Vec<String>,
}

/// A filter that has been pushed down to an engine
#[derive(Debug, Clone)]
pub struct PushedFilter {
    /// Field/path the filter applies to
    pub field: String,
    /// Filter operator
    pub operator: FilterOperator,
    /// Filter value as string
    pub value: String,
    /// Target engine
    pub target: DataModel,
}

impl QueryOptimizer {
    /// Create a new query optimizer
    pub fn new(config: OptimizerConfig) -> Self {
        let plan_cache = if config.enable_plan_cache {
            Some(Arc::new(PlanCache::new(1000, config.plan_cache_ttl_secs)))
        } else {
            None
        };
        let plan_execution_cache = if config.enable_measured_fitness {
            Some(super::plan_execution_cache::shared(
                config.measured_fitness_max_entries,
            ))
        } else {
            None
        };

        Self {
            stats: Arc::new(QueryStatistics::new()),
            config,
            plan_cache,
            plan_execution_cache,
        }
    }

    /// Record a measured wall-clock time for a plan that has just executed.
    ///
    /// No-op when measured-fitness is disabled. Callers should invoke this
    /// from the query executor after the result is materialized.
    pub fn record_plan_execution(
        &self,
        components: &[QueryComponent],
        execution_order: &[usize],
        wall_time_us: u64,
    ) {
        if let Some(cache) = &self.plan_execution_cache {
            let shape = super::plan_execution_cache::shape_hash(components);
            cache.record(shape, execution_order, wall_time_us);
        }
    }

    /// Access the measured-fitness cache (mostly for tests and observability).
    pub fn plan_execution_cache(
        &self,
    ) -> Option<&Arc<super::plan_execution_cache::PlanExecutionCache>> {
        self.plan_execution_cache.as_ref()
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(OptimizerConfig::default())
    }

    /// Create without plan caching (for testing or low-memory environments)
    pub fn without_cache() -> Self {
        let config = OptimizerConfig {
            enable_plan_cache: false,
            ..Default::default()
        };
        Self::new(config)
    }

    /// Get statistics collector
    pub fn statistics(&self) -> &Arc<QueryStatistics> {
        &self.stats
    }

    /// Get plan cache (if enabled)
    pub fn plan_cache(&self) -> Option<&Arc<PlanCache>> {
        self.plan_cache.as_ref()
    }

    /// Invalidate all cached plans (call on schema changes)
    pub fn invalidate_plan_cache(&self) {
        if let Some(cache) = &self.plan_cache {
            cache.invalidate_all();
        }
    }

    /// Invalidate cached plans for a specific collection
    pub fn invalidate_collection_plans(&self, collection: &str) {
        if let Some(cache) = &self.plan_cache {
            cache.invalidate_collection(collection);
        }
    }

    /// Optimize a multi-model query
    pub fn optimize(&self, query: &MultiModelQuery) -> Result<OptimizedPlan> {
        let start = Instant::now();
        debug!(
            "Optimizing query with {} components",
            query.components.len()
        );

        // Check plan cache first (if enabled)
        let query_hash = compute_query_hash(query);
        if let Some(cache) = &self.plan_cache {
            if let Some(cached_plan) = cache.get(query_hash) {
                debug!(
                    "Plan cache hit (hash={}), returning cached plan",
                    query_hash
                );
                return Ok(cached_plan);
            }
            trace!("Plan cache miss (hash={}), computing new plan", query_hash);
        }

        // Step 1: Estimate selectivity for each component
        let selectivity_estimates: Vec<SelectivityEstimate> = query
            .components
            .iter()
            .map(|c| self.estimate_selectivity(c))
            .collect();

        // Step 2: Determine optimal execution order
        let execution_order = if self.config.enable_evolutionary_optimizer && query.components.len() > 1 {
            self.evolutionary_optimize(&query.components, &selectivity_estimates)
        } else if self.config.enable_reordering {
            self.compute_optimal_order(&query.components, &selectivity_estimates)
        } else {
            (0..query.components.len()).collect()
        };

        // Step 3: Push down filters where possible
        let pushed_filters = if self.config.enable_filter_pushdown {
            query
                .components
                .iter()
                .map(|c| self.extract_pushable_filters(c))
                .collect()
        } else {
            vec![vec![]; query.components.len()]
        };

        // Step 4: Reorder components based on execution order
        let reordered_components: Vec<QueryComponent> = execution_order
            .iter()
            .map(|&idx| query.components[idx].clone())
            .collect();

        // Step 5: Estimate total cost
        let estimated_cost = self.estimate_total_cost(&selectivity_estimates, &execution_order);

        let elapsed = start.elapsed();
        let mut notes = vec![format!("Optimization completed in {:?}", elapsed)];

        if self.config.enable_reordering {
            notes.push(format!(
                "Reordered {} components by selectivity",
                execution_order.len()
            ));
        }

        let pushed_count: usize = pushed_filters.iter().map(|f| f.len()).sum();
        if pushed_count > 0 {
            notes.push(format!("Pushed {} filters to engines", pushed_count));
        }

        let plan = OptimizedPlan {
            components: reordered_components,
            execution_order,
            selectivity_estimates,
            pushed_filters,
            estimated_cost,
            notes,
        };

        // Cache the optimized plan (if caching enabled)
        if let Some(cache) = &self.plan_cache {
            cache.insert(query_hash, plan.clone());
            debug!(
                "Cached optimized plan (hash={}, cost={:.2})",
                query_hash, estimated_cost
            );
        }

        info!(
            "Query optimized: {} components, cost={:.2}, {:?}",
            plan.components.len(),
            estimated_cost,
            elapsed
        );

        Ok(plan)
    }

    /// Estimate selectivity for a query component
    pub fn estimate_selectivity(&self, component: &QueryComponent) -> SelectivityEstimate {
        match &component.operation {
            ModelOperation::VectorSearch(expr) => self.estimate_vector_selectivity(expr),
            ModelOperation::DocumentQuery(expr) => self.estimate_document_selectivity(expr),
            ModelOperation::GraphTraversal(expr) => self.estimate_graph_selectivity(expr),
            ModelOperation::LogQuery(expr) => self.estimate_log_selectivity(expr),
            ModelOperation::MetricQuery(expr) => self.estimate_metric_selectivity(expr),
        }
    }

    /// Estimate selectivity for vector search
    fn estimate_vector_selectivity(&self, expr: &VectorSearchExpr) -> SelectivityEstimate {
        // Vector search selectivity depends on:
        // 1. Similarity threshold (higher = more selective)
        // 2. Top-K limit

        let mut selectivity = 1.0;
        let mut confidence = 0.5;

        // Similarity threshold affects selectivity
        if let Some(threshold) = expr.threshold {
            // Higher threshold = more selective
            // At 0.9 threshold, expect ~10% of vectors to match
            // At 0.5 threshold, expect ~50% of vectors to match
            selectivity *= 1.0 - (threshold as f64);
            confidence = 0.7;
        }

        // Top-K also limits results
        let top_k = expr.top_k;
        if top_k > 0 {
            // Assume 1M vectors as baseline
            let baseline: f64 = 1_000_000.0;
            let top_k_selectivity = (top_k as f64) / baseline;
            selectivity = selectivity.min(top_k_selectivity);
            confidence = f64::max(confidence, 0.6);
        }

        let estimated_rows = (selectivity * 1_000_000.0) as u64;

        SelectivityEstimate {
            selectivity,
            confidence,
            estimated_rows,
            method: EstimationMethod::PredicateAnalysis,
        }
    }

    /// Estimate selectivity for document query
    fn estimate_document_selectivity(&self, expr: &DocumentQueryExpr) -> SelectivityEstimate {
        let mut selectivity = 1.0;
        let mut confidence = 0.4;

        // Each filter reduces selectivity
        for filter in &expr.path_filters {
            let filter_selectivity = self.estimate_filter_selectivity(filter);
            selectivity *= filter_selectivity;
            confidence = f64::max(confidence, 0.5);
        }

        // Check statistics for collection
        if let Some(stats) = self.stats.get_collection_stats(&expr.collection) {
            let estimated_rows = (selectivity * stats.row_count as f64) as u64;
            return SelectivityEstimate {
                selectivity,
                confidence: 0.8, // Higher confidence with actual stats
                estimated_rows,
                method: EstimationMethod::Statistics,
            };
        }

        let estimated_rows = (selectivity * 100_000.0) as u64;

        SelectivityEstimate {
            selectivity,
            confidence,
            estimated_rows,
            method: EstimationMethod::PredicateAnalysis,
        }
    }

    /// Estimate selectivity for a single filter
    fn estimate_filter_selectivity(&self, filter: &PathFilter) -> f64 {
        match filter.operator {
            FilterOperator::Eq => 0.1, // Equality is typically selective
            FilterOperator::Ne => 0.9, // Not equal is not selective
            FilterOperator::Gt | FilterOperator::Lt => 0.5, // Range filters
            FilterOperator::Gte | FilterOperator::Lte => 0.5,
            FilterOperator::In => 0.2,    // In-list depends on list size
            FilterOperator::NotIn => 0.8, // Not in is less selective
            FilterOperator::Contains => 0.3, // Substring match
            FilterOperator::StartsWith => 0.2,
            FilterOperator::EndsWith => 0.3,
            FilterOperator::Exists => 0.8, // Most documents have the field
            FilterOperator::Type => 0.5,   // Type check
        }
    }

    /// Estimate selectivity for graph traversal
    fn estimate_graph_selectivity(&self, expr: &GraphTraversalExpr) -> SelectivityEstimate {
        let mut confidence = 0.3;
        let mut selectivity;

        // Number of start nodes affects result size
        match &expr.start_nodes {
            super::ast::StartNodeSpec::Ids(ids) => {
                // Known number of start nodes
                let start_count = ids.len() as f64;
                // Estimate fan-out based on depth
                let fan_out = 3.0_f64.powi(expr.max_depth as i32);
                selectivity = (start_count * fan_out) / 1_000_000.0;
                confidence = 0.6;
            }
            super::ast::StartNodeSpec::Label(_) => {
                // Label-based start: estimate 10% of nodes
                selectivity = 0.1;
            }
            super::ast::StartNodeSpec::Filter(_) => {
                // Filter-based start: estimate 5% of nodes
                selectivity = 0.05;
            }
            super::ast::StartNodeSpec::FromComponent(_) => {
                // Depends on prior component - conservative estimate
                selectivity = 0.01;
                confidence = 0.2;
            }
        }

        // Edge type filters are selective
        if !expr.edge_types.is_empty() {
            // Each edge type reduces potential paths
            selectivity *= 0.3_f64.powi(expr.edge_types.len() as i32);
        }

        // Node filters reduce result size
        if !expr.node_filters.is_empty() {
            selectivity *= 0.5_f64.powi(expr.node_filters.len() as i32);
        }

        let estimated_rows = (selectivity * 1_000_000.0).max(1.0) as u64;

        SelectivityEstimate {
            selectivity: selectivity.min(1.0),
            confidence,
            estimated_rows,
            method: EstimationMethod::Heuristic,
        }
    }

    /// Estimate selectivity for log query
    fn estimate_log_selectivity(&self, expr: &LogQueryExpr) -> SelectivityEstimate {
        let mut selectivity = 1.0;
        let confidence = 0.5;

        // Time range affects selectivity significantly
        if expr.start_time_ns > 0 && expr.end_time_ns > 0 {
            let range_ns = (expr.end_time_ns - expr.start_time_ns) as f64;
            let one_day_ns = 86_400_000_000_000.0;
            // Assume 30 days of logs
            let total_range = 30.0 * one_day_ns;
            selectivity = (range_ns / total_range).min(1.0);
        }

        // Severity filters are selective
        if !expr.severities.is_empty() {
            // ERROR and FATAL are typically <5% of logs
            selectivity *= 0.2;
        }

        // Service filter
        if !expr.services.is_empty() {
            selectivity *= 0.1_f64.powi(expr.services.len() as i32);
        }

        // Query string search
        if expr.query.is_some() {
            selectivity *= 0.1;
        }

        let estimated_rows = (selectivity * 10_000_000.0) as u64;

        SelectivityEstimate {
            selectivity,
            confidence,
            estimated_rows,
            method: EstimationMethod::PredicateAnalysis,
        }
    }

    /// Estimate selectivity for metric query
    fn estimate_metric_selectivity(&self, expr: &MetricQueryExpr) -> SelectivityEstimate {
        let mut selectivity = 1.0;
        let confidence = 0.5;

        // Time range
        if expr.start_time_ns > 0 && expr.end_time_ns > 0 {
            let range_ns = (expr.end_time_ns - expr.start_time_ns) as f64;
            let one_day_ns = 86_400_000_000_000.0;
            let total_range = 7.0 * one_day_ns; // 7 days of metrics
            selectivity = (range_ns / total_range).min(1.0);
        }

        // Metric name filter is very selective (a specific metric name is given)
        if !expr.metric_name.is_empty() {
            selectivity *= 0.01; // Specific metrics
        }

        // Label filters
        selectivity *= 0.5_f64.powi(expr.label_filters.len() as i32);

        let estimated_rows = (selectivity * 1_000_000.0) as u64;

        SelectivityEstimate {
            selectivity,
            confidence,
            estimated_rows,
            method: EstimationMethod::PredicateAnalysis,
        }
    }

    /// Compute optimal execution order based on selectivity and dependencies
    fn compute_optimal_order(
        &self,
        components: &[QueryComponent],
        selectivity: &[SelectivityEstimate],
    ) -> Vec<usize> {
        let n = components.len();
        if n <= 1 {
            return (0..n).collect();
        }

        // Build dependency graph
        let mut dependencies: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, component) in components.iter().enumerate() {
            for dep in &component.dependencies {
                dependencies.entry(i).or_default().push(dep.component_index);
            }
        }

        // Topological sort with selectivity-based ordering
        let mut order = Vec::with_capacity(n);
        let mut in_degree: Vec<usize> = vec![0; n];
        let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];

        // Calculate in-degrees and dependents
        for (i, component) in components.iter().enumerate() {
            for dep in &component.dependencies {
                in_degree[i] += 1;
                dependents[dep.component_index].push(i);
            }
        }

        // Find components with no dependencies, sorted by selectivity
        let mut ready: Vec<(usize, f64)> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, d)| **d == 0)
            .map(|(i, _)| (i, selectivity[i].selectivity))
            .collect();

        // Sort by selectivity (most selective = lowest value first, using descending order for pop())
        ready.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        while let Some((idx, _)) = ready.pop() {
            order.push(idx);

            // Update dependents
            for &dep_idx in &dependents[idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    ready.push((dep_idx, selectivity[dep_idx].selectivity));
                    ready
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                }
            }
        }

        // If we couldn't order all components (cycle), fall back to original order
        if order.len() != n {
            trace!("Dependency cycle detected, using original order");
            return (0..n).collect();
        }

        order
    }

    /// Compute optimal order using evolutionary algorithms.
    ///
    /// Fitness function:
    /// 1. If measured-fitness is enabled and the cache holds a sample for
    ///    this `(shape, order)`, return the running mean wall-clock time
    ///    in microseconds.
    /// 2. Otherwise fall back to [`estimate_total_cost`].
    ///
    /// The two ranges are not directly comparable (microseconds vs unitless
    /// cost), so on warm caches the optimizer effectively converges toward
    /// observed-fast plans; on cold caches it follows the estimate. As a
    /// plan accumulates samples it transitions from estimate-driven to
    /// measurement-driven without a discontinuity at the optimizer level
    /// (each evolutionary generation is internally consistent because the
    /// fitness function is deterministic for a given `(shape, order)`).
    fn evolutionary_optimize(
        &self,
        components: &[QueryComponent],
        selectivity: &[SelectivityEstimate],
    ) -> Vec<usize> {
        use super::evolutionary::EvolutionaryOptimizer;

        let optimizer = EvolutionaryOptimizer::new(
            self.config.evolutionary_population_size,
            self.config.evolutionary_generations,
        );

        let exec_cache = self.plan_execution_cache.as_ref();
        let shape = exec_cache.map(|_| super::plan_execution_cache::shape_hash(components));

        optimizer.optimize(components, selectivity, |sel, order| {
            if let (Some(cache), Some(s)) = (exec_cache, shape) {
                if let Some(measured) = cache.get_mean_us(s, order) {
                    return measured;
                }
            }
            self.estimate_total_cost(sel, order)
        })
    }

    /// Extract filters that can be pushed down to engines
    fn extract_pushable_filters(&self, component: &QueryComponent) -> Vec<PushedFilter> {
        let mut filters = Vec::new();

        // First, push component-level filters (these apply to any model)
        for filter in &component.filters {
            let target = component.model.clone();
            filters.push(PushedFilter {
                field: filter.field.clone(),
                operator: filter.operator.clone(),
                value: format!("{:?}", filter.value),
                target,
            });
        }

        // Then, extract operation-specific filters
        match &component.operation {
            ModelOperation::VectorSearch(_expr) => {
                // Vector-specific filters are already in component.filters
                // Nothing extra to extract here
            }
            ModelOperation::DocumentQuery(expr) => {
                // Document path filters can be pushed
                for filter in &expr.path_filters {
                    filters.push(PushedFilter {
                        field: filter.path.clone(),
                        operator: filter.operator.clone(),
                        value: format!("{:?}", filter.value),
                        target: DataModel::Document,
                    });
                }
            }
            ModelOperation::GraphTraversal(expr) => {
                // Edge type and node label filters can be pushed
                for edge_type in &expr.edge_types {
                    filters.push(PushedFilter {
                        field: "edge_type".to_string(),
                        operator: FilterOperator::Eq,
                        value: edge_type.clone(),
                        target: DataModel::Graph,
                    });
                }
            }
            ModelOperation::LogQuery(expr) => {
                // Service and severity filters can be pushed
                for service in &expr.services {
                    filters.push(PushedFilter {
                        field: "service".to_string(),
                        operator: FilterOperator::Eq,
                        value: service.clone(),
                        target: DataModel::Observability,
                    });
                }
            }
            ModelOperation::MetricQuery(expr) => {
                // Metric name filter can be pushed
                if !expr.metric_name.is_empty() {
                    filters.push(PushedFilter {
                        field: "metric_name".to_string(),
                        operator: FilterOperator::Eq,
                        value: expr.metric_name.clone(),
                        target: DataModel::Observability,
                    });
                }
            }
        }

        filters
    }

    /// Estimate total execution cost
    fn estimate_total_cost(
        &self,
        selectivity: &[SelectivityEstimate],
        execution_order: &[usize],
    ) -> f64 {
        let mut total_cost = 0.0;
        let mut intermediate_size = 1.0;

        for &idx in execution_order {
            let sel = &selectivity[idx];
            // Cost = base cost * intermediate size
            let component_cost = match sel.method {
                EstimationMethod::Statistics => 1.0,
                EstimationMethod::PredicateAnalysis => 1.5,
                EstimationMethod::Historical => 1.2,
                EstimationMethod::Heuristic => 2.0,
            };

            total_cost += component_cost * intermediate_size;
            // Intermediate size grows/shrinks based on selectivity
            intermediate_size *= sel.selectivity;
        }

        total_cost
    }
}

/// Query statistics collector
pub struct QueryStatistics {
    /// Collection statistics
    collection_stats: RwLock<HashMap<String, OptimizerCollectionStats>>,
    /// Query execution history
    query_history: RwLock<Vec<QueryHistoryEntry>>,
    /// Total queries executed
    total_queries: AtomicU64,
    /// Total execution time (microseconds)
    total_execution_time_us: AtomicU64,
}

/// Optimizer-local collection statistics with column-level detail
///
/// Extends the canonical `storage::traits::CollectionStats` with optimizer-specific
/// fields (distinct counts per column, name, timestamps). Use `From<storage::traits::CollectionStats>`
/// to convert from the canonical form.
#[derive(Debug, Clone)]
pub struct OptimizerCollectionStats {
    /// Collection name
    pub name: String,
    /// Estimated row count
    pub row_count: u64,
    /// Average row size in bytes
    pub avg_row_size: u64,
    /// Distinct value counts for indexed columns
    pub distinct_counts: HashMap<String, u64>,
    /// Last updated timestamp
    pub last_updated: u64,
}

impl OptimizerCollectionStats {
    /// Create from canonical CollectionStats with a collection name
    pub fn from_canonical(name: String, stats: &crate::storage::traits::CollectionStats) -> Self {
        Self {
            name,
            row_count: stats.row_count,
            avg_row_size: stats.avg_vector_bytes,
            distinct_counts: HashMap::new(),
            last_updated: 0,
        }
    }
}

/// Historical query execution entry
#[derive(Debug, Clone)]
pub struct QueryHistoryEntry {
    /// Query hash (for matching similar queries)
    pub query_hash: u64,
    /// Actual result count
    pub result_count: u64,
    /// Execution time in microseconds
    pub execution_time_us: u64,
    /// Timestamp
    pub timestamp: u64,
}

impl QueryStatistics {
    /// Create new statistics collector
    pub fn new() -> Self {
        Self {
            collection_stats: RwLock::new(HashMap::new()),
            query_history: RwLock::new(Vec::new()),
            total_queries: AtomicU64::new(0),
            total_execution_time_us: AtomicU64::new(0),
        }
    }

    /// Get collection statistics
    pub fn get_collection_stats(&self, collection: &str) -> Option<OptimizerCollectionStats> {
        self.collection_stats.read().get(collection).cloned()
    }

    /// Update collection statistics
    pub fn update_collection_stats(&self, stats: OptimizerCollectionStats) {
        let name = stats.name.clone();
        self.collection_stats.write().insert(name, stats);
    }

    /// Record query execution
    pub fn record_query(&self, hash: u64, result_count: u64, execution_time_us: u64) {
        self.total_queries.fetch_add(1, Ordering::Relaxed);
        self.total_execution_time_us
            .fetch_add(execution_time_us, Ordering::Relaxed);

        let entry = QueryHistoryEntry {
            query_hash: hash,
            result_count,
            execution_time_us,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        let mut history = self.query_history.write();
        history.push(entry);

        // Keep last 10000 entries
        if history.len() > 10000 {
            history.remove(0);
        }
    }

    /// Get average execution time for similar queries
    pub fn get_avg_execution_time(&self, query_hash: u64) -> Option<u64> {
        let history = self.query_history.read();
        let matching: Vec<&QueryHistoryEntry> = history
            .iter()
            .filter(|e| e.query_hash == query_hash)
            .collect();

        if matching.is_empty() {
            return None;
        }

        let total: u64 = matching.iter().map(|e| e.execution_time_us).sum();
        Some(total / matching.len() as u64)
    }

    /// Get total query count
    pub fn total_queries(&self) -> u64 {
        self.total_queries.load(Ordering::Relaxed)
    }

    /// Get average execution time across all queries
    pub fn avg_execution_time_us(&self) -> u64 {
        let total = self.total_queries.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        self.total_execution_time_us.load(Ordering::Relaxed) / total
    }
}

impl Default for QueryStatistics {
    fn default() -> Self {
        Self::new()
    }
}

/// Cached query plan entry with expiration
#[derive(Debug, Clone)]
struct CachedPlan {
    /// The optimized plan
    plan: OptimizedPlan,
    /// When this entry was created
    created_at: Instant,
    /// Last access time (for LRU)
    last_accessed: Instant,
    /// Number of times this plan was used
    hit_count: u64,
}

/// LRU cache for query plans with TTL expiration
pub struct PlanCache {
    /// Cached plans keyed by query hash
    cache: RwLock<HashMap<u64, CachedPlan>>,
    /// Maximum number of cached plans
    max_entries: usize,
    /// Time-to-live for cached plans
    ttl: Duration,
    /// Cache hit counter
    hits: AtomicU64,
    /// Cache miss counter
    misses: AtomicU64,
    /// Eviction counter
    evictions: AtomicU64,
}

impl PlanCache {
    /// Create a new plan cache
    pub fn new(max_entries: usize, ttl_secs: u64) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            max_entries,
            ttl: Duration::from_secs(ttl_secs),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Get a cached plan if it exists and hasn't expired
    pub fn get(&self, query_hash: u64) -> Option<OptimizedPlan> {
        let now = Instant::now();

        // First try with read lock
        {
            let cache = self.cache.read();
            if let Some(entry) = cache.get(&query_hash) {
                // Check TTL
                if now.duration_since(entry.created_at) < self.ttl {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    // We'll update last_accessed in a separate write lock to avoid contention
                    let plan = entry.plan.clone();
                    drop(cache);

                    // Update last_accessed time
                    if let Some(entry) = self.cache.write().get_mut(&query_hash) {
                        entry.last_accessed = now;
                        entry.hit_count += 1;
                    }

                    return Some(plan);
                }
            }
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert a plan into the cache
    pub fn insert(&self, query_hash: u64, plan: OptimizedPlan) {
        let now = Instant::now();
        let mut cache = self.cache.write();

        // Evict expired entries first
        let expired: Vec<u64> = cache
            .iter()
            .filter(|(_, entry)| now.duration_since(entry.created_at) >= self.ttl)
            .map(|(k, _)| *k)
            .collect();

        for key in expired {
            cache.remove(&key);
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }

        // If still at capacity, evict LRU entry
        while cache.len() >= self.max_entries {
            if let Some(lru_key) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed)
                .map(|(k, _)| *k)
            {
                cache.remove(&lru_key);
                self.evictions.fetch_add(1, Ordering::Relaxed);
            } else {
                break;
            }
        }

        cache.insert(
            query_hash,
            CachedPlan {
                plan,
                created_at: now,
                last_accessed: now,
                hit_count: 0,
            },
        );
    }

    /// Invalidate all cached plans (e.g., on schema change)
    pub fn invalidate_all(&self) {
        let mut cache = self.cache.write();
        let count = cache.len();
        cache.clear();
        self.evictions.fetch_add(count as u64, Ordering::Relaxed);
        info!("Invalidated {} cached query plans", count);
    }

    /// Invalidate cached plans for a specific collection
    pub fn invalidate_collection(&self, _collection: &str) {
        // For now, invalidate all plans since we don't track which plans use which collections
        // A more sophisticated implementation would track collection dependencies
        self.invalidate_all();
    }

    /// Get cache statistics
    pub fn stats(&self) -> PlanCacheStats {
        let cache = self.cache.read();
        PlanCacheStats {
            size: cache.len(),
            max_entries: self.max_entries,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }
}

/// Statistics for the plan cache
#[derive(Debug, Clone)]
pub struct PlanCacheStats {
    /// Current number of cached plans
    pub size: usize,
    /// Maximum cache capacity
    pub max_entries: usize,
    /// Number of cache hits
    pub hits: u64,
    /// Number of cache misses
    pub misses: u64,
    /// Number of evictions (LRU + TTL)
    pub evictions: u64,
}

impl PlanCacheStats {
    /// Calculate cache hit rate (0.0 - 1.0)
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }
}

/// Compute a hash for a MultiModelQuery to use as cache key
fn compute_query_hash(query: &MultiModelQuery) -> u64 {
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();

    // Hash component count
    query.components.len().hash(&mut hasher);

    // Hash each component
    for component in &query.components {
        // Hash model type
        std::mem::discriminant(&component.model).hash(&mut hasher);

        // Hash operation type and key details
        match &component.operation {
            ModelOperation::VectorSearch(expr) => {
                "VectorSearch".hash(&mut hasher);
                expr.collection.hash(&mut hasher);
                expr.top_k.hash(&mut hasher);
                // Hash threshold as bits to avoid float hashing issues
                if let Some(t) = expr.threshold {
                    t.to_bits().hash(&mut hasher);
                }
                std::mem::discriminant(&expr.metric).hash(&mut hasher);
            }
            ModelOperation::DocumentQuery(expr) => {
                "DocumentQuery".hash(&mut hasher);
                expr.collection.hash(&mut hasher);
                expr.path_filters.len().hash(&mut hasher);
                for filter in &expr.path_filters {
                    filter.path.hash(&mut hasher);
                    std::mem::discriminant(&filter.operator).hash(&mut hasher);
                }
            }
            ModelOperation::GraphTraversal(expr) => {
                "GraphTraversal".hash(&mut hasher);
                expr.graph_name.hash(&mut hasher);
                expr.max_depth.hash(&mut hasher);
                expr.edge_types.len().hash(&mut hasher);
                for edge_type in &expr.edge_types {
                    edge_type.hash(&mut hasher);
                }
            }
            ModelOperation::LogQuery(expr) => {
                "LogQuery".hash(&mut hasher);
                expr.namespace.hash(&mut hasher);
                expr.start_time_ns.hash(&mut hasher);
                expr.end_time_ns.hash(&mut hasher);
                expr.services.len().hash(&mut hasher);
            }
            ModelOperation::MetricQuery(expr) => {
                "MetricQuery".hash(&mut hasher);
                expr.namespace.hash(&mut hasher);
                expr.metric_name.hash(&mut hasher);
                expr.start_time_ns.hash(&mut hasher);
                expr.end_time_ns.hash(&mut hasher);
            }
        }

        // Hash dependencies
        component.dependencies.len().hash(&mut hasher);
        for dep in &component.dependencies {
            dep.component_index.hash(&mut hasher);
            dep.join_field.hash(&mut hasher);
        }
    }

    // Hash fusion strategy
    std::mem::discriminant(&query.fusion_strategy).hash(&mut hasher);

    // Hash limit/offset
    query.limit.hash(&mut hasher);
    query.offset.hash(&mut hasher);

    hasher.finish()
}

// ============================================================================
// FUSION STRATEGY SELECTION (Cost-Based Optimizer)
// ============================================================================

/// Fusion strategy for combining results from multi-model sub-queries
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FusionStrategy {
    /// Reciprocal Rank Fusion — normalizes heterogeneous score scales
    Rrf,
    /// Intersection — keeps only results present in all sub-queries
    Intersection,
    /// Union — returns results from any sub-query (maximum recall)
    Union,
    /// Weighted sum — linearly combines scores with caller-supplied weights
    WeightedSum,
}

/// Select the optimal fusion strategy based on query characteristics
///
/// Decision logic:
/// - **Rrf**: when both vector and graph components are present, scores come
///   from fundamentally different distributions (cosine similarity vs path
///   length); RRF normalizes via rank position.
/// - **Intersection**: when combined filter selectivity is very low (<10%),
///   taking the intersection maximizes precision.
/// - **Union**: default strategy — maximizes recall by including results
///   from any sub-query.
pub fn select_fusion_strategy(
    has_vector_component: bool,
    has_graph_component: bool,
    filter_selectivity: f64,
) -> FusionStrategy {
    if has_vector_component && has_graph_component {
        // Heterogeneous score scales — use RRF for normalization
        FusionStrategy::Rrf
    } else if filter_selectivity < 0.1 {
        // Highly selective filters — intersect for precision
        FusionStrategy::Intersection
    } else {
        // Default — union for maximum recall
        FusionStrategy::Union
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::unified::ast::{
        ComponentDependency, DistanceMetric, FilterValue, JoinType, StartNodeSpec,
        TraversalDirection, VectorSearchParams,
    };

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

    fn make_document_query_component(path_filters: Vec<PathFilter>) -> QueryComponent {
        QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                collection: "products".to_string(),
                path_filters,
                text_search: None,
                projection: vec![],
                sort: None,
                limit: None,
            }),
            filters: vec![],
            dependencies: vec![],
        }
    }

    fn make_graph_traversal_component(edge_types: Vec<String>, depth: u32) -> QueryComponent {
        QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                graph_name: "knowledge".to_string(),
                start_nodes: StartNodeSpec::Ids(vec!["node1".to_string()]),
                edge_types,
                direction: TraversalDirection::Outgoing,
                max_depth: depth,
                min_depth: 1,
                node_filters: vec![],
                edge_filters: vec![],
                return_paths: false,
            }),
            filters: vec![],
            dependencies: vec![],
        }
    }

    #[test]
    fn test_optimizer_creation() {
        let optimizer = QueryOptimizer::with_defaults();
        assert!(optimizer.config.enable_reordering);
        assert!(optimizer.config.enable_filter_pushdown);
    }

    #[test]
    fn test_vector_selectivity_with_threshold() {
        let optimizer = QueryOptimizer::with_defaults();

        // High threshold = low selectivity (more selective)
        // Use large top_k so threshold dominates selectivity
        let high_thresh = make_vector_search_component(Some(0.9), 1_000_000);
        let estimate = optimizer.estimate_selectivity(&high_thresh);
        assert!(
            estimate.selectivity < 0.2,
            "High threshold should be selective"
        );

        // Low threshold = high selectivity (less selective)
        let low_thresh = make_vector_search_component(Some(0.3), 1_000_000);
        let estimate = optimizer.estimate_selectivity(&low_thresh);
        assert!(
            estimate.selectivity > 0.5,
            "Low threshold should be less selective"
        );
    }

    #[test]
    fn test_vector_selectivity_with_top_k() {
        let optimizer = QueryOptimizer::with_defaults();

        let small_k = make_vector_search_component(None, 10);
        let estimate = optimizer.estimate_selectivity(&small_k);
        assert!(
            estimate.selectivity < 0.001,
            "Small top_k should be very selective"
        );

        let large_k = make_vector_search_component(None, 10000);
        let estimate = optimizer.estimate_selectivity(&large_k);
        assert!(
            estimate.selectivity > 0.001,
            "Large top_k should be less selective"
        );
    }

    #[test]
    fn test_document_selectivity_with_filters() {
        let optimizer = QueryOptimizer::with_defaults();

        // No filters = not selective
        let no_filters = make_document_query_component(vec![]);
        let estimate = optimizer.estimate_selectivity(&no_filters);
        assert_eq!(estimate.selectivity, 1.0, "No filters = full scan");

        // Equality filter = selective
        let eq_filter = make_document_query_component(vec![PathFilter {
            path: "category".to_string(),
            operator: FilterOperator::Eq,
            value: FilterValue::String("electronics".to_string()),
        }]);
        let estimate = optimizer.estimate_selectivity(&eq_filter);
        assert!(
            estimate.selectivity < 0.5,
            "Equality filter should be selective"
        );

        // Multiple filters = very selective
        let multi_filters = make_document_query_component(vec![
            PathFilter {
                path: "category".to_string(),
                operator: FilterOperator::Eq,
                value: FilterValue::String("electronics".to_string()),
            },
            PathFilter {
                path: "price".to_string(),
                operator: FilterOperator::Lt,
                value: FilterValue::Number(100.0),
            },
        ]);
        let estimate = optimizer.estimate_selectivity(&multi_filters);
        assert!(
            estimate.selectivity < 0.1,
            "Multiple filters should be very selective"
        );
    }

    #[test]
    fn test_graph_selectivity() {
        let optimizer = QueryOptimizer::with_defaults();

        // No edge type filter
        let no_edge_filter = make_graph_traversal_component(vec![], 2);
        let estimate = optimizer.estimate_selectivity(&no_edge_filter);

        // With edge type filters = more selective
        let with_edge_filter = make_graph_traversal_component(vec!["KNOWS".to_string()], 2);
        let estimate2 = optimizer.estimate_selectivity(&with_edge_filter);

        assert!(
            estimate2.selectivity < estimate.selectivity,
            "Edge type filter should increase selectivity"
        );
    }

    #[test]
    fn test_component_reordering() {
        let optimizer = QueryOptimizer::with_defaults();

        // Create query with 3 components of varying selectivity
        let query = MultiModelQuery {
            components: vec![
                // Component 0: Low selectivity (should run last) - no filters = 1.0
                make_document_query_component(vec![]),
                // Component 1: High selectivity (should run first) - top_k=1 gives ~0.000001
                make_vector_search_component(None, 1),
                // Component 2: Medium selectivity - graph with depth 3 (larger fan-out)
                make_graph_traversal_component(vec![], 3),
            ],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let plan = optimizer
            .optimize(&query)
            .expect("optimization should succeed");

        // Most selective should be first (lowest selectivity value)
        // Vector search with top_k=1 is most selective (selectivity ~0.000001)
        // Document with no filters is least selective (selectivity = 1.0)
        // Check that document (index 0) is NOT first
        assert_ne!(
            plan.execution_order[0], 0,
            "Document with no filters should not execute first"
        );
        // Most selective (vector or graph) should be first
        assert!(
            plan.execution_order[0] == 1 || plan.execution_order[0] == 2,
            "Most selective component should execute first"
        );
    }

    #[test]
    fn test_filter_pushdown() {
        let optimizer = QueryOptimizer::with_defaults();

        let doc_query = make_document_query_component(vec![PathFilter {
            path: "status".to_string(),
            operator: FilterOperator::Eq,
            value: FilterValue::String("active".to_string()),
        }]);

        let pushed = optimizer.extract_pushable_filters(&doc_query);
        assert_eq!(pushed.len(), 1, "Should push 1 filter");
        assert_eq!(pushed[0].field, "status");
        assert_eq!(pushed[0].target, DataModel::Document);
    }

    #[test]
    fn test_statistics_collection() {
        let stats = QueryStatistics::new();

        // Record some queries
        stats.record_query(12345, 100, 5000);
        stats.record_query(12345, 120, 4500);
        stats.record_query(67890, 50, 3000);

        assert_eq!(stats.total_queries(), 3);

        // Check average for specific query
        let avg = stats
            .get_avg_execution_time(12345)
            .expect("should have average execution time");
        assert_eq!(avg, 4750); // (5000 + 4500) / 2
    }

    #[test]
    fn test_collection_stats() {
        let stats = QueryStatistics::new();

        let collection_stats = OptimizerCollectionStats {
            name: "products".to_string(),
            row_count: 1_000_000,
            avg_row_size: 512,
            distinct_counts: HashMap::new(),
            last_updated: 0,
        };

        stats.update_collection_stats(collection_stats);

        let retrieved = stats
            .get_collection_stats("products")
            .expect("should have collection stats");
        assert_eq!(retrieved.row_count, 1_000_000);
    }

    #[test]
    fn test_dependency_aware_ordering() {
        let optimizer = QueryOptimizer::with_defaults();

        // Create query with dependencies
        let mut doc_component = make_document_query_component(vec![]);
        doc_component.dependencies = vec![ComponentDependency {
            component_index: 0,
            join_field: "id".to_string(),
            join_type: JoinType::Inner,
        }];

        let query = MultiModelQuery {
            components: vec![
                // Vector search (no deps)
                make_vector_search_component(Some(0.9), 10),
                // Document query (depends on vector)
                doc_component,
            ],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let plan = optimizer
            .optimize(&query)
            .expect("optimization should succeed");

        // Vector must come before document due to dependency
        let vec_pos = plan
            .execution_order
            .iter()
            .position(|&x| x == 0)
            .expect("vector component should be in execution order");
        let doc_pos = plan
            .execution_order
            .iter()
            .position(|&x| x == 1)
            .expect("document component should be in execution order");
        assert!(
            vec_pos < doc_pos,
            "Vector must execute before dependent document query"
        );
    }

    #[test]
    fn test_optimized_plan_notes() {
        let optimizer = QueryOptimizer::with_defaults();

        let query = MultiModelQuery {
            components: vec![
                make_vector_search_component(Some(0.9), 10),
                make_document_query_component(vec![PathFilter {
                    path: "category".to_string(),
                    operator: FilterOperator::Eq,
                    value: FilterValue::String("test".to_string()),
                }]),
            ],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let plan = optimizer
            .optimize(&query)
            .expect("optimization should succeed");

        // Should have optimization notes
        assert!(!plan.notes.is_empty(), "Should have optimization notes");
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("Optimization completed")),
            "Should have completion note"
        );
    }

    #[test]
    fn test_estimate_total_cost() {
        let optimizer = QueryOptimizer::with_defaults();

        let query = MultiModelQuery {
            components: vec![
                make_vector_search_component(Some(0.9), 10),
                make_document_query_component(vec![]),
            ],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let plan = optimizer
            .optimize(&query)
            .expect("optimization should succeed");

        assert!(
            plan.estimated_cost > 0.0,
            "Should have positive estimated cost"
        );
    }

    // ==================== Plan Caching Tests ====================

    #[test]
    fn test_plan_cache_creation() {
        // Default optimizer should have cache enabled
        let optimizer = QueryOptimizer::with_defaults();
        assert!(
            optimizer.plan_cache().is_some(),
            "Default should have cache enabled"
        );

        // Optimizer without cache
        let optimizer_no_cache = QueryOptimizer::without_cache();
        assert!(
            optimizer_no_cache.plan_cache().is_none(),
            "without_cache should disable cache"
        );
    }

    #[test]
    fn test_plan_cache_hit_and_miss() {
        let optimizer = QueryOptimizer::with_defaults();

        let query = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.8), 100)],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        // First call should be a cache miss
        let _plan1 = optimizer
            .optimize(&query)
            .expect("optimization should succeed");
        let cache = optimizer
            .plan_cache()
            .expect("plan cache should be enabled");
        let stats1 = cache.stats();
        assert_eq!(stats1.misses, 1, "First call should be a miss");
        assert_eq!(stats1.hits, 0, "First call should have no hits");

        // Second call with same query should be a cache hit
        let _plan2 = optimizer
            .optimize(&query)
            .expect("optimization should succeed");
        let stats2 = cache.stats();
        assert_eq!(stats2.hits, 1, "Second call should be a hit");
        assert_eq!(stats2.misses, 1, "Misses should remain 1");
    }

    #[test]
    fn test_plan_cache_different_queries() {
        let optimizer = QueryOptimizer::with_defaults();

        let query1 = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.8), 100)],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let query2 = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.9), 50)], // Different params
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        // Two different queries should both be misses
        let _plan1 = optimizer
            .optimize(&query1)
            .expect("optimization should succeed");
        let _plan2 = optimizer
            .optimize(&query2)
            .expect("optimization should succeed");

        let cache = optimizer
            .plan_cache()
            .expect("plan cache should be enabled");
        let stats = cache.stats();
        assert_eq!(stats.misses, 2, "Both queries should be misses");
        assert_eq!(stats.size, 2, "Cache should have 2 entries");
    }

    #[test]
    fn test_plan_cache_invalidation() {
        let optimizer = QueryOptimizer::with_defaults();

        let query = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.8), 100)],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        // Cache a plan
        let _plan1 = optimizer
            .optimize(&query)
            .expect("optimization should succeed");
        let cache = optimizer
            .plan_cache()
            .expect("plan cache should be enabled");
        assert_eq!(cache.stats().size, 1, "Cache should have 1 entry");

        // Invalidate all
        optimizer.invalidate_plan_cache();
        assert_eq!(
            cache.stats().size,
            0,
            "Cache should be empty after invalidation"
        );

        // Next call should be a miss
        let _plan2 = optimizer
            .optimize(&query)
            .expect("optimization should succeed");
        let stats = cache.stats();
        assert_eq!(stats.misses, 2, "Should have 2 misses total");
    }

    #[test]
    fn test_plan_cache_stats_hit_rate() {
        let cache = PlanCache::new(100, 300);

        // No queries yet
        let stats = cache.stats();
        assert_eq!(stats.hit_rate(), 0.0, "Empty cache should have 0 hit rate");

        // Create a simple plan to cache
        let plan = OptimizedPlan {
            components: vec![],
            execution_order: vec![],
            selectivity_estimates: vec![],
            pushed_filters: vec![],
            estimated_cost: 1.0,
            notes: vec![],
        };

        // Insert and get (miss then hit)
        cache.insert(12345, plan.clone());
        let _ = cache.get(99999); // miss
        let _ = cache.get(12345); // hit
        let _ = cache.get(12345); // hit

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!(
            (stats.hit_rate() - 0.666).abs() < 0.01,
            "Hit rate should be ~66%"
        );
    }

    #[test]
    fn test_plan_cache_lru_eviction() {
        // Create cache with max 3 entries
        let cache = PlanCache::new(3, 300);

        let plan = OptimizedPlan {
            components: vec![],
            execution_order: vec![],
            selectivity_estimates: vec![],
            pushed_filters: vec![],
            estimated_cost: 1.0,
            notes: vec![],
        };

        // Insert 4 plans (should evict 1)
        cache.insert(1, plan.clone());
        std::thread::sleep(std::time::Duration::from_millis(10));
        cache.insert(2, plan.clone());
        std::thread::sleep(std::time::Duration::from_millis(10));
        cache.insert(3, plan.clone());
        std::thread::sleep(std::time::Duration::from_millis(10));
        cache.insert(4, plan.clone()); // Should evict entry 1 (oldest last_accessed)

        let stats = cache.stats();
        assert_eq!(stats.size, 3, "Cache should have 3 entries after eviction");
        assert_eq!(stats.evictions, 1, "Should have 1 eviction");

        // Entry 1 should be gone, entry 4 should be present
        assert!(cache.get(1).is_none(), "Entry 1 should have been evicted");
        assert!(cache.get(4).is_some(), "Entry 4 should be present");
    }

    #[test]
    fn test_query_hash_consistency() {
        let query = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.8), 100)],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: Some(10),
            offset: None,
            projection: vec![],
            order_by: None,
        };

        // Same query should produce same hash
        let hash1 = compute_query_hash(&query);
        let hash2 = compute_query_hash(&query);
        assert_eq!(hash1, hash2, "Same query should produce same hash");

        // Different query should produce different hash
        let query2 = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.9), 100)], // Different threshold
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: Some(10),
            offset: None,
            projection: vec![],
            order_by: None,
        };
        let hash3 = compute_query_hash(&query2);
        assert_ne!(
            hash1, hash3,
            "Different query should produce different hash"
        );
    }

    #[test]
    fn test_query_hash_different_components() {
        let vec_query = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.8), 100)],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let doc_query = MultiModelQuery {
            components: vec![make_document_query_component(vec![])],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let graph_query = MultiModelQuery {
            components: vec![make_graph_traversal_component(vec!["KNOWS".to_string()], 2)],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let hash_vec = compute_query_hash(&vec_query);
        let hash_doc = compute_query_hash(&doc_query);
        let hash_graph = compute_query_hash(&graph_query);

        assert_ne!(
            hash_vec, hash_doc,
            "Vector and document queries should have different hashes"
        );
        assert_ne!(
            hash_vec, hash_graph,
            "Vector and graph queries should have different hashes"
        );
        assert_ne!(
            hash_doc, hash_graph,
            "Document and graph queries should have different hashes"
        );
    }

    #[test]
    fn test_optimizer_without_cache_still_works() {
        let optimizer = QueryOptimizer::without_cache();

        let query = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.8), 100)],
            fusion_strategy: super::super::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        // Should work without cache
        let plan1 = optimizer
            .optimize(&query)
            .expect("optimization should succeed");
        let plan2 = optimizer
            .optimize(&query)
            .expect("optimization should succeed");

        // Both should succeed and produce same result
        assert_eq!(plan1.estimated_cost, plan2.estimated_cost);
        assert_eq!(plan1.execution_order, plan2.execution_order);

        // invalidate_plan_cache should not panic when cache is None
        optimizer.invalidate_plan_cache();
        optimizer.invalidate_collection_plans("test_collection");
    }

    // ========================================================================
    // FUSION STRATEGY SELECTION TESTS
    // ========================================================================

    #[test]
    fn test_fusion_strategy_rrf_for_vector_and_graph() {
        let strategy = select_fusion_strategy(true, true, 0.5);
        assert_eq!(strategy, FusionStrategy::Rrf);
    }

    #[test]
    fn test_fusion_strategy_intersection_for_selective_filter() {
        let strategy = select_fusion_strategy(true, false, 0.05);
        assert_eq!(strategy, FusionStrategy::Intersection);
    }

    #[test]
    fn test_fusion_strategy_union_default() {
        let strategy = select_fusion_strategy(true, false, 0.5);
        assert_eq!(strategy, FusionStrategy::Union);
    }

    #[test]
    fn test_fusion_strategy_rrf_overrides_selectivity() {
        // Even with low selectivity, vector+graph uses RRF
        let strategy = select_fusion_strategy(true, true, 0.01);
        assert_eq!(strategy, FusionStrategy::Rrf);
    }

    #[test]
    fn test_fusion_strategy_no_components() {
        let strategy = select_fusion_strategy(false, false, 0.5);
        assert_eq!(strategy, FusionStrategy::Union);
    }

    #[test]
    fn test_record_plan_execution_no_op_when_disabled() {
        // Default config has enable_measured_fitness = false; recording is a
        // no-op and the cache is absent.
        let optimizer = QueryOptimizer::with_defaults();
        assert!(optimizer.plan_execution_cache().is_none());

        let components = vec![
            make_vector_search_component(None, 10),
            make_document_query_component(vec![]),
        ];
        // Should not panic.
        optimizer.record_plan_execution(&components, &[0, 1], 1234);
    }

    #[test]
    fn test_record_plan_execution_populates_cache_when_enabled() {
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        config.enable_measured_fitness = true;
        config.measured_fitness_max_entries = 16;
        let optimizer = QueryOptimizer::new(config);

        let components = vec![
            make_vector_search_component(None, 10),
            make_document_query_component(vec![]),
        ];

        let cache = optimizer
            .plan_execution_cache()
            .expect("cache should exist when enable_measured_fitness=true");
        assert!(cache.is_empty());

        optimizer.record_plan_execution(&components, &[0, 1], 5_000);
        optimizer.record_plan_execution(&components, &[0, 1], 7_000);

        let shape =
            crate::query::unified::plan_execution_cache::shape_hash(&components);
        let mean = cache
            .get_mean_us(shape, &[0, 1])
            .expect("cache should hold the recorded plan");
        assert!((mean - 6_000.0).abs() < 1e-6, "mean was {}", mean);
    }

    #[test]
    fn test_evolutionary_optimize_uses_measured_fitness_when_available() {
        // Set up an optimizer with measured fitness on. Two query orders;
        // one is much faster according to recorded measurements but slower
        // according to the cost estimator. The optimizer should converge
        // toward the empirically faster one.
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        config.enable_measured_fitness = true;
        config.evolutionary_population_size = 12;
        config.evolutionary_generations = 8;
        let optimizer = QueryOptimizer::new(config);

        // Two independent components -- the dependency graph allows either
        // order, so the evolutionary search has a real choice.
        let components = vec![
            make_vector_search_component(Some(0.5), 100),
            make_document_query_component(vec![]),
        ];

        // Seed the cache: order [1, 0] is dramatically faster in measured
        // time, regardless of what the cost estimator might prefer.
        let shape =
            crate::query::unified::plan_execution_cache::shape_hash(&components);
        let cache = optimizer.plan_execution_cache().unwrap();
        cache.record(shape, &[0, 1], 100_000);
        cache.record(shape, &[1, 0], 1_000);

        // Manually drive the evolutionary path. (We bypass `optimize` here
        // because `optimize_query` requires a full UnifiedQuery construction
        // and we want to test the order selection in isolation.)
        let selectivity: Vec<_> = components
            .iter()
            .map(|c| optimizer.estimate_selectivity(c))
            .collect();
        let order = optimizer.evolutionary_optimize(&components, &selectivity);

        assert_eq!(
            order,
            vec![1, 0],
            "evolutionary planner should converge to the empirically faster order"
        );
    }
}
