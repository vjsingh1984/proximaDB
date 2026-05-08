//! Shared unified-query orchestration helpers that do not depend on root services.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use proximadb_multimodel_query::{DataModel, MultiModelQuery, QueryComponent};
use proximadb_query_fusion::FusionStrategy;

use crate::optimizer_support::OptimizedPlan;

/// Query execution plan summary for explain-style surfaces.
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// Component plans.
    pub components: Vec<ComponentPlan>,
    /// Fusion strategy.
    pub fusion_strategy: FusionStrategy,
    /// Estimated total cost.
    pub estimated_total_cost: f64,
}

/// Plan summary for a single query component.
#[derive(Debug, Clone)]
pub struct ComponentPlan {
    /// Data model.
    pub model: DataModel,
    /// Estimated cost in relative units.
    pub estimated_cost: f64,
    /// Whether the component can run in parallel.
    pub parallelizable: bool,
}

/// Narrow optimizer seam for orchestration helpers.
#[async_trait(?Send)]
pub trait QueryOptimizationService: Send + Sync {
    /// Optimize a multi-model query and return the resulting plan.
    async fn optimize_query(&self, query: &MultiModelQuery) -> Result<OptimizedPlan>;
}

#[async_trait(?Send)]
impl QueryOptimizationService for crate::optimizer::QueryOptimizerRuntime {
    async fn optimize_query(&self, query: &MultiModelQuery) -> Result<OptimizedPlan> {
        self.optimize(query, None).await
    }
}

/// Reorder `query.components` according to a query optimizer's plan.
pub async fn reorder_components_with_optimizer<O>(
    optimizer: Option<&Arc<O>>,
    query: &mut MultiModelQuery,
) -> Result<Option<Vec<usize>>>
where
    O: QueryOptimizationService + ?Sized,
{
    let Some(optimizer) = optimizer else {
        return Ok(None);
    };
    if query.components.len() < 2 {
        return Ok(None);
    }

    let plan = optimizer.optimize_query(query).await?;
    let order = plan.execution_order;

    let mut reordered = Vec::with_capacity(query.components.len());
    for &idx in &order {
        reordered.push(query.components[idx].clone());
    }
    query.components = reordered;

    Ok(Some(order))
}

/// Estimate cost for a single query component.
pub fn estimate_component_cost(component: &QueryComponent) -> f64 {
    match component.model {
        DataModel::Vector => 1.0,
        DataModel::Document => 2.0,
        DataModel::Graph => 3.0,
        DataModel::Observability | DataModel::TimeSeries => 2.5,
        DataModel::Relational => 1.5,
        DataModel::Event => 2.0,
    }
}

/// Estimate total cost for a query given a parallelism budget.
pub fn estimate_total_cost(query: &MultiModelQuery, max_parallel_queries: usize) -> f64 {
    let component_costs: Vec<f64> = query
        .components
        .iter()
        .map(estimate_component_cost)
        .collect();

    if query.components.len() <= max_parallel_queries {
        component_costs.iter().cloned().fold(0.0, f64::max)
    } else {
        component_costs.iter().sum::<f64>() / max_parallel_queries as f64
    }
}

/// Build an explain-plan summary for a query.
pub fn explain_query_plan(query: &MultiModelQuery, max_parallel_queries: usize) -> QueryPlan {
    QueryPlan {
        components: query
            .components
            .iter()
            .map(|c| ComponentPlan {
                model: c.model,
                estimated_cost: estimate_component_cost(c),
                parallelizable: c.is_parallelizable(),
            })
            .collect(),
        fusion_strategy: query.fusion_strategy.clone(),
        estimated_total_cost: estimate_total_cost(query, max_parallel_queries),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::QueryOptimizerRuntime;
    use crate::optimizer_support::OptimizerConfig;
    use proximadb_document_query::DocumentQueryExpr;
    use proximadb_multimodel_query::{
        ComponentDependency, DataModel, JoinType, ModelOperation, QueryComponent,
    };
    use proximadb_query_filter::{FilterOperator, FilterValue};
    use proximadb_vector_query::{DistanceMetric, VectorSearchExpr, VectorSearchParams};

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
                path_filters: vec![proximadb_document_query::PathFilter {
                    path: "$.kind".to_string(),
                    operator: FilterOperator::Eq,
                    value: FilterValue::String("doc".to_string()),
                }],
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

    #[tokio::test]
    async fn reorder_returns_none_without_optimizer() {
        let mut q = empty_query(vec![vector_component(), document_component()]);
        let original_models: Vec<_> = q.components.iter().map(|c| c.model).collect();

        let result = reorder_components_with_optimizer::<QueryOptimizerRuntime>(None, &mut q)
            .await
            .unwrap();
        assert!(result.is_none());
        let after: Vec<_> = q.components.iter().map(|c| c.model).collect();
        assert_eq!(after, original_models);
    }

    #[tokio::test]
    async fn reorder_returns_none_for_single_component_query() {
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        config.enable_measured_fitness = true;
        let optimizer = Arc::new(QueryOptimizerRuntime::new(config));

        let mut q = empty_query(vec![vector_component()]);
        let result = reorder_components_with_optimizer(Some(&optimizer), &mut q)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn reorder_picks_measured_faster_order() {
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        config.enable_measured_fitness = true;
        config.evolutionary_population_size = 12;
        config.evolutionary_generations = 8;
        let optimizer = Arc::new(QueryOptimizerRuntime::new(config));

        let components = vec![vector_component(), document_component()];
        let cache = optimizer.plan_execution_cache().unwrap();
        let shape = crate::plan_execution_cache::shape_hash(&components);
        cache.record(shape, &[0, 1], 100_000);
        cache.record(shape, &[1, 0], 1_000);

        let mut q = empty_query(components);
        let order = reorder_components_with_optimizer(Some(&optimizer), &mut q)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(order, vec![1, 0]);
        assert_eq!(q.components[0].model, DataModel::Document);
        assert_eq!(q.components[1].model, DataModel::Vector);
    }

    #[tokio::test]
    async fn reorder_respects_dependency_topology() {
        let mut config = OptimizerConfig::default();
        config.enable_evolutionary_optimizer = true;
        let optimizer = Arc::new(QueryOptimizerRuntime::new(config));

        let mut c1 = document_component();
        c1.dependencies = vec![ComponentDependency {
            component_index: 0,
            join_field: "id".to_string(),
            join_type: JoinType::Inner,
        }];
        let mut q = empty_query(vec![vector_component(), c1]);

        let order = reorder_components_with_optimizer(Some(&optimizer), &mut q)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn explain_query_plan_uses_shared_cost_heuristics() {
        let query = empty_query(vec![vector_component(), document_component()]);
        let plan = explain_query_plan(&query, 4);

        assert_eq!(plan.components.len(), 2);
        assert_eq!(plan.components[0].estimated_cost, 1.0);
        assert_eq!(plan.components[1].estimated_cost, 2.0);
        assert_eq!(plan.estimated_total_cost, 2.0);
    }
}
