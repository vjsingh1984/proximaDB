//! Shared query optimizer runtime for multi-model queries.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use proximadb_document_query::{DocumentQueryExpr, PathFilter};
use proximadb_graph_query::traversal::{GraphTraversalExpr, StartNodeSpec};
use proximadb_graph_subset::LoweredGraphQuery as GraphQueryExpr;
use proximadb_multimodel_query::{DataModel, ModelOperation, MultiModelQuery, QueryComponent};
use proximadb_observability_query::{LogQueryExpr, MetricQueryExpr};
use proximadb_query_filter::FilterOperator;
use proximadb_vector_query::VectorSearchExpr;
use tracing::{debug, info, trace};

use crate::evolutionary::{EvolutionaryMutationAdvisor, EvolutionaryOptimizer};
use crate::optimizer_support::{
    EstimationMethod, OptimizedPlan, OptimizerConfig, PlanCache, PushedFilter, QueryStatistics,
    SelectivityEstimate, compute_query_hash,
};
use crate::plan_execution_cache::{PlanExecutionCache, shape_hash, shared};

/// Shared optimizer runtime that owns pure query-planning behavior.
pub struct QueryOptimizerRuntime {
    stats: Arc<QueryStatistics>,
    config: OptimizerConfig,
    plan_cache: Option<Arc<PlanCache>>,
    plan_execution_cache: Option<Arc<PlanExecutionCache>>,
}

impl QueryOptimizerRuntime {
    /// Create a new query optimizer runtime.
    pub fn new(config: OptimizerConfig) -> Self {
        let plan_cache = if config.enable_plan_cache {
            Some(Arc::new(PlanCache::new(1000, config.plan_cache_ttl_secs)))
        } else {
            None
        };
        let plan_execution_cache = if config.enable_measured_fitness {
            Some(shared(config.measured_fitness_max_entries))
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

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(OptimizerConfig::default())
    }

    /// Create without plan caching.
    pub fn without_cache() -> Self {
        let config = OptimizerConfig {
            enable_plan_cache: false,
            ..Default::default()
        };
        Self::new(config)
    }

    /// Access optimizer configuration.
    pub fn config(&self) -> &OptimizerConfig {
        &self.config
    }

    /// Get statistics collector.
    pub fn statistics(&self) -> &Arc<QueryStatistics> {
        &self.stats
    }

    /// Get plan cache, if enabled.
    pub fn plan_cache(&self) -> Option<&Arc<PlanCache>> {
        self.plan_cache.as_ref()
    }

    /// Access the measured-fitness cache.
    pub fn plan_execution_cache(&self) -> Option<&Arc<PlanExecutionCache>> {
        self.plan_execution_cache.as_ref()
    }

    /// Invalidate all cached plans.
    pub fn invalidate_plan_cache(&self) {
        if let Some(cache) = &self.plan_cache {
            cache.invalidate_all();
        }
    }

    /// Invalidate cached plans for a specific collection.
    pub fn invalidate_collection_plans(&self, collection: &str) {
        if let Some(cache) = &self.plan_cache {
            cache.invalidate_collection(collection);
        }
    }

    /// Record a measured wall-clock time for a plan that has just executed.
    pub fn record_plan_execution(
        &self,
        components: &[QueryComponent],
        execution_order: &[usize],
        wall_time_us: u64,
    ) {
        if let Some(cache) = &self.plan_execution_cache {
            let shape = shape_hash(components);
            cache.record(shape, execution_order, wall_time_us);
        }
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
        let start = std::time::Instant::now();
        let result = fut.await;
        if result.is_ok() {
            let elapsed_us: u64 = start.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
            self.record_plan_execution(components, execution_order, elapsed_us);
        }
        result
    }

    /// Optimize a multi-model query.
    pub async fn optimize(
        &self,
        query: &MultiModelQuery,
        mutation_advisor: Option<Arc<dyn EvolutionaryMutationAdvisor>>,
    ) -> Result<OptimizedPlan> {
        let start = Instant::now();
        debug!(
            "Optimizing query with {} components",
            query.components.len()
        );

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

        let selectivity_estimates: Vec<SelectivityEstimate> = query
            .components
            .iter()
            .map(|c| self.estimate_selectivity(c))
            .collect();

        let execution_order = if self.config.enable_evolutionary_optimizer
            && query.components.len() > 1
        {
            self.evolutionary_optimize(&query.components, &selectivity_estimates, mutation_advisor)
                .await
        } else if self.config.enable_reordering {
            self.compute_optimal_order(&query.components, &selectivity_estimates)
        } else {
            (0..query.components.len()).collect()
        };

        let pushed_filters = if self.config.enable_filter_pushdown {
            query
                .components
                .iter()
                .map(|c| self.extract_pushable_filters(c))
                .collect()
        } else {
            vec![vec![]; query.components.len()]
        };

        let reordered_components: Vec<QueryComponent> = execution_order
            .iter()
            .map(|idx| query.components[*idx].clone())
            .collect();

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

    /// Estimate selectivity for a query component.
    pub fn estimate_selectivity(&self, component: &QueryComponent) -> SelectivityEstimate {
        match &component.operation {
            ModelOperation::VectorSearch(expr) => self.estimate_vector_selectivity(expr),
            ModelOperation::DocumentQuery(expr) => self.estimate_document_selectivity(expr),
            ModelOperation::GraphQuery(expr) => self.estimate_graph_query_selectivity(expr),
            ModelOperation::GraphTraversal(expr) => self.estimate_graph_selectivity(expr),
            ModelOperation::LogQuery(expr) => self.estimate_log_selectivity(expr),
            ModelOperation::MetricQuery(expr) => self.estimate_metric_selectivity(expr),
        }
    }

    /// Compute optimal execution order based on selectivity and dependencies.
    pub fn compute_optimal_order(
        &self,
        components: &[QueryComponent],
        selectivity: &[SelectivityEstimate],
    ) -> Vec<usize> {
        let n = components.len();
        if n <= 1 {
            return (0..n).collect();
        }

        let mut dependencies: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, component) in components.iter().enumerate() {
            for dep in &component.dependencies {
                dependencies.entry(i).or_default().push(dep.component_index);
            }
        }

        let mut order = Vec::with_capacity(n);
        let mut in_degree: Vec<usize> = vec![0; n];
        let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];

        for (i, component) in components.iter().enumerate() {
            for dep in &component.dependencies {
                in_degree[i] += 1;
                dependents[dep.component_index].push(i);
            }
        }

        let mut ready: Vec<(usize, f64)> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, d)| **d == 0)
            .map(|(i, _)| (i, selectivity[i].selectivity))
            .collect();

        ready.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        while let Some((idx, _)) = ready.pop() {
            order.push(idx);

            for &dep_idx in &dependents[idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    ready.push((dep_idx, selectivity[dep_idx].selectivity));
                    ready
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                }
            }
        }

        if order.len() != n {
            trace!("Dependency cycle detected, using original order");
            return (0..n).collect();
        }

        order
    }

    /// Compute optimal order using evolutionary algorithms.
    pub async fn evolutionary_optimize(
        &self,
        components: &[QueryComponent],
        selectivity: &[SelectivityEstimate],
        mutation_advisor: Option<Arc<dyn EvolutionaryMutationAdvisor>>,
    ) -> Vec<usize> {
        let mut optimizer = EvolutionaryOptimizer::new(
            self.config.evolutionary_population_size,
            self.config.evolutionary_generations,
        );

        if let Some(advisor) = mutation_advisor {
            optimizer = optimizer.with_advisor(advisor);
        }

        let exec_cache = self.plan_execution_cache.as_ref();
        let shape = exec_cache.map(|_| shape_hash(components));

        optimizer
            .optimize(components, selectivity, |sel, order| {
                if let (Some(cache), Some(s)) = (exec_cache, shape)
                    && let Some(measured) = cache.get_mean_us(s, order)
                {
                    return measured;
                }
                self.estimate_total_cost(sel, order)
            })
            .await
    }

    /// Extract filters that can be pushed down to engines.
    pub fn extract_pushable_filters(&self, component: &QueryComponent) -> Vec<PushedFilter> {
        let mut filters = Vec::new();

        for filter in &component.filters {
            let target = component.model;
            filters.push(PushedFilter {
                field: filter.field.clone(),
                operator: filter.operator.clone(),
                value: format!("{:?}", filter.value),
                target,
            });
        }

        match &component.operation {
            ModelOperation::VectorSearch(_) => {}
            ModelOperation::DocumentQuery(expr) => {
                for filter in &expr.path_filters {
                    filters.push(PushedFilter {
                        field: filter.path.clone(),
                        operator: filter.operator.clone(),
                        value: format!("{:?}", filter.value),
                        target: DataModel::Document,
                    });
                }
            }
            ModelOperation::GraphQuery(_) => {}
            ModelOperation::GraphTraversal(expr) => {
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

    /// Estimate total execution cost.
    pub fn estimate_total_cost(
        &self,
        selectivity: &[SelectivityEstimate],
        execution_order: &[usize],
    ) -> f64 {
        let mut total_cost = 0.0;
        let mut intermediate_size = 1.0;

        for &idx in execution_order {
            let sel = &selectivity[idx];
            let component_cost = match sel.method {
                EstimationMethod::Statistics => 1.0,
                EstimationMethod::PredicateAnalysis => 1.5,
                EstimationMethod::Historical => 1.2,
                EstimationMethod::Heuristic => 2.0,
            };

            total_cost += component_cost * intermediate_size;
            intermediate_size *= sel.selectivity;
        }

        total_cost
    }

    fn estimate_vector_selectivity(&self, expr: &VectorSearchExpr) -> SelectivityEstimate {
        let mut selectivity = 1.0;
        let mut confidence = 0.5;

        if let Some(threshold) = expr.threshold {
            selectivity *= 1.0 - (threshold as f64);
            confidence = 0.7;
        }

        let top_k = expr.top_k;
        if top_k > 0 {
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

    fn estimate_document_selectivity(&self, expr: &DocumentQueryExpr) -> SelectivityEstimate {
        let mut selectivity = 1.0;
        let mut confidence = 0.4;

        for filter in &expr.path_filters {
            let filter_selectivity = self.estimate_filter_selectivity(filter);
            selectivity *= filter_selectivity;
            confidence = f64::max(confidence, 0.5);
        }

        if let Some(stats) = self.stats.get_collection_stats(&expr.collection) {
            let estimated_rows = (selectivity * stats.row_count as f64) as u64;
            return SelectivityEstimate {
                selectivity,
                confidence: 0.8,
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

    fn estimate_filter_selectivity(&self, filter: &PathFilter) -> f64 {
        match filter.operator {
            FilterOperator::Eq => 0.1,
            FilterOperator::Ne => 0.9,
            FilterOperator::Gt | FilterOperator::Lt => 0.5,
            FilterOperator::Gte | FilterOperator::Lte => 0.5,
            FilterOperator::In => 0.2,
            FilterOperator::NotIn => 0.8,
            FilterOperator::Contains => 0.3,
            FilterOperator::StartsWith => 0.2,
            FilterOperator::EndsWith => 0.3,
            FilterOperator::Exists => 0.8,
            FilterOperator::Type => 0.5,
        }
    }

    fn estimate_graph_selectivity(&self, expr: &GraphTraversalExpr) -> SelectivityEstimate {
        let mut confidence = 0.3;
        let mut selectivity;

        match &expr.start_nodes {
            StartNodeSpec::Ids(ids) => {
                let start_count = ids.len() as f64;
                let fan_out = 3.0_f64.powi(expr.max_depth as i32);
                selectivity = (start_count * fan_out) / 1_000_000.0;
                confidence = 0.6;
            }
            StartNodeSpec::Label(_) => {
                selectivity = 0.1;
            }
            StartNodeSpec::Filter(_) => {
                selectivity = 0.05;
            }
            StartNodeSpec::FromComponent(_) => {
                selectivity = 0.01;
                confidence = 0.2;
            }
        }

        if !expr.edge_types.is_empty() {
            selectivity *= 0.3_f64.powi(expr.edge_types.len() as i32);
        }

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

    fn estimate_graph_query_selectivity(&self, expr: &GraphQueryExpr) -> SelectivityEstimate {
        let pseudo_traversal = GraphTraversalExpr {
            graph_name: expr.graph_name.clone(),
            start_nodes: StartNodeSpec::Label("*".to_string()),
            edge_types: Vec::new(),
            direction: proximadb_graph_query::traversal::TraversalDirection::Outgoing,
            max_depth: expr.max_depth,
            min_depth: 0,
            node_filters: Vec::new(),
            edge_filters: Vec::new(),
            return_paths: false,
        };
        self.estimate_graph_selectivity(&pseudo_traversal)
    }

    fn estimate_log_selectivity(&self, expr: &LogQueryExpr) -> SelectivityEstimate {
        let mut selectivity = 1.0;
        let confidence = 0.5;

        if expr.start_time_ns > 0 && expr.end_time_ns > 0 {
            let range_ns = (expr.end_time_ns - expr.start_time_ns) as f64;
            let one_day_ns = 86_400_000_000_000.0;
            let total_range = 30.0 * one_day_ns;
            selectivity = (range_ns / total_range).min(1.0);
        }

        if !expr.severities.is_empty() {
            selectivity *= 0.2;
        }

        if !expr.services.is_empty() {
            selectivity *= 0.1_f64.powi(expr.services.len() as i32);
        }

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

    fn estimate_metric_selectivity(&self, expr: &MetricQueryExpr) -> SelectivityEstimate {
        let mut selectivity = 1.0;
        let confidence = 0.5;

        if expr.start_time_ns > 0 && expr.end_time_ns > 0 {
            let range_ns = (expr.end_time_ns - expr.start_time_ns) as f64;
            let one_day_ns = 86_400_000_000_000.0;
            let total_range = 7.0 * one_day_ns;
            selectivity = (range_ns / total_range).min(1.0);
        }

        if !expr.metric_name.is_empty() {
            selectivity *= 0.01;
        }

        selectivity *= 0.5_f64.powi(expr.label_filters.len() as i32);

        let estimated_rows = (selectivity * 1_000_000.0) as u64;

        SelectivityEstimate {
            selectivity,
            confidence,
            estimated_rows,
            method: EstimationMethod::PredicateAnalysis,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_document_query::DocumentQueryExpr;
    use proximadb_multimodel_query::{
        ComponentDependency, JoinType, ModelOperation, MultiModelQuery, QueryComponent,
    };
    use proximadb_query_filter::FilterValue;
    use proximadb_query_fusion::FusionStrategy;
    use proximadb_vector_query::{DistanceMetric, VectorSearchParams};

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
                direction: proximadb_graph_query::traversal::TraversalDirection::Outgoing,
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
    fn optimizer_runtime_creation() {
        let optimizer = QueryOptimizerRuntime::with_defaults();
        assert!(optimizer.config().enable_reordering);
        assert!(optimizer.config().enable_filter_pushdown);
    }

    #[test]
    fn vector_selectivity_tracks_threshold_and_top_k() {
        let optimizer = QueryOptimizerRuntime::with_defaults();

        let high_thresh = make_vector_search_component(Some(0.9), 1_000_000);
        let estimate = optimizer.estimate_selectivity(&high_thresh);
        assert!(estimate.selectivity < 0.2);

        let small_k = make_vector_search_component(None, 10);
        let estimate = optimizer.estimate_selectivity(&small_k);
        assert!(estimate.selectivity < 0.001);
    }

    #[test]
    fn document_selectivity_uses_collection_stats_when_available() {
        let optimizer = QueryOptimizerRuntime::with_defaults();
        optimizer.statistics().update_collection_stats(
            crate::optimizer_support::OptimizerCollectionStats {
                name: "products".to_string(),
                row_count: 5_000,
                avg_row_size: 100,
                distinct_counts: HashMap::new(),
                last_updated: 0,
            },
        );

        let component = make_document_query_component(vec![PathFilter {
            path: "$.category".to_string(),
            operator: FilterOperator::Eq,
            value: FilterValue::String("electronics".to_string()),
        }]);

        let estimate = optimizer.estimate_selectivity(&component);
        assert_eq!(estimate.method, EstimationMethod::Statistics);
        assert_eq!(estimate.estimated_rows, 500);
    }

    #[test]
    fn graph_selectivity_becomes_more_selective_with_edge_filters() {
        let optimizer = QueryOptimizerRuntime::with_defaults();
        let no_edge_filter = make_graph_traversal_component(vec![], 2);
        let with_edge_filter = make_graph_traversal_component(vec!["KNOWS".to_string()], 2);

        let estimate = optimizer.estimate_selectivity(&no_edge_filter);
        let estimate2 = optimizer.estimate_selectivity(&with_edge_filter);
        assert!(estimate2.selectivity < estimate.selectivity);
    }

    #[test]
    fn dependency_aware_ordering_keeps_dependencies_before_dependents() {
        let optimizer = QueryOptimizerRuntime::with_defaults();
        let components = vec![
            QueryComponent {
                model: DataModel::Vector,
                operation: make_vector_search_component(None, 100).operation,
                filters: vec![],
                dependencies: vec![],
            },
            QueryComponent {
                model: DataModel::Document,
                operation: make_document_query_component(vec![]).operation,
                filters: vec![],
                dependencies: vec![ComponentDependency {
                    component_index: 0,
                    join_field: "id".to_string(),
                    join_type: JoinType::Inner,
                }],
            },
        ];
        let selectivity = components
            .iter()
            .map(|c| optimizer.estimate_selectivity(c))
            .collect::<Vec<_>>();

        let order = optimizer.compute_optimal_order(&components, &selectivity);
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn document_filters_are_extracted_for_pushdown() {
        let optimizer = QueryOptimizerRuntime::with_defaults();
        let component = make_document_query_component(vec![PathFilter {
            path: "$.price".to_string(),
            operator: FilterOperator::Gt,
            value: FilterValue::Number(100.0),
        }]);
        let pushed = optimizer.extract_pushable_filters(&component);
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].field, "$.price");
        assert_eq!(pushed[0].target, DataModel::Document);
    }

    #[tokio::test]
    async fn optimize_uses_plan_cache_when_enabled() {
        let optimizer = QueryOptimizerRuntime::with_defaults();
        let query = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.8), 10)],
            fusion_strategy: FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        let plan1 = optimizer
            .optimize(&query, None)
            .await
            .expect("optimize should succeed");
        let plan2 = optimizer
            .optimize(&query, None)
            .await
            .expect("cached optimize should succeed");

        assert_eq!(plan1.execution_order, plan2.execution_order);
        assert_eq!(optimizer.plan_cache().expect("cache").stats().hits, 1);
    }

    #[tokio::test]
    async fn evolutionary_optimize_uses_measured_fitness_when_available() {
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        config.enable_measured_fitness = true;
        config.evolutionary_population_size = 12;
        config.evolutionary_generations = 8;
        let optimizer = QueryOptimizerRuntime::new(config);

        let components = vec![
            make_vector_search_component(None, 50),
            make_document_query_component(vec![PathFilter {
                path: "$.category".to_string(),
                operator: FilterOperator::Eq,
                value: FilterValue::String("electronics".to_string()),
            }]),
        ];
        let selectivity = components
            .iter()
            .map(|c| optimizer.estimate_selectivity(c))
            .collect::<Vec<_>>();

        optimizer.record_plan_execution(&components, &[1, 0], 1_000);
        optimizer.record_plan_execution(&components, &[1, 0], 1_100);
        optimizer.record_plan_execution(&components, &[0, 1], 8_000);
        optimizer.record_plan_execution(&components, &[0, 1], 7_500);

        let best_order = optimizer
            .evolutionary_optimize(&components, &selectivity, None)
            .await;
        assert_eq!(best_order, vec![1, 0]);
    }

    #[tokio::test]
    async fn time_and_record_records_on_ok() {
        let mut config = OptimizerConfig::default();
        config.enable_measured_fitness = true;
        config.measured_fitness_max_entries = 8;
        let optimizer = QueryOptimizerRuntime::new(config);
        let components = vec![make_vector_search_component(None, 10)];
        let order = vec![0usize];

        let result: std::result::Result<u32, ()> = optimizer
            .time_and_record_if_ok(&components, &order, async { Ok(7) })
            .await;

        assert_eq!(result.unwrap(), 7);
        let cache = optimizer.plan_execution_cache().expect("cache");
        let shape = shape_hash(&components);
        assert!(cache.get_mean_us(shape, &order).is_some());
    }
}
