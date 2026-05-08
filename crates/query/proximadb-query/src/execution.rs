use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
use proximadb_document_query::DocumentQueryExpr;
use proximadb_graph::query::traversal::GraphTraversalExpr;
use proximadb_graph_subset::LoweredGraphQuery as GraphQueryExpr;
use proximadb_multimodel_query::{
    ComponentDependency, ModelOperation, MultiModelQuery, QueryComponent,
};
use proximadb_observability_query::{LogQueryExpr, MetricQueryExpr};
use proximadb_vector_query::VectorSearchExpr;
use tokio::sync::Semaphore;

use crate::{JoinExecutionService, SubQueryResult, execute_multi_join_with};

/// Narrow query-runtime execution contract for modality-backed component execution.
#[async_trait]
pub trait QueryComponentExecutionService: Send + Sync {
    async fn execute_vector_search(&self, expr: &VectorSearchExpr) -> Result<SubQueryResult>;

    async fn execute_document_query(&self, expr: &DocumentQueryExpr) -> Result<SubQueryResult>;

    async fn execute_graph_query(&self, expr: &GraphQueryExpr) -> Result<SubQueryResult>;

    async fn execute_graph_traversal(
        &self,
        expr: &GraphTraversalExpr,
        context: Option<&HashMap<usize, &SubQueryResult>>,
    ) -> Result<SubQueryResult>;

    async fn execute_log_query(&self, expr: &LogQueryExpr) -> Result<SubQueryResult>;

    async fn execute_metric_query(&self, expr: &MetricQueryExpr) -> Result<SubQueryResult>;
}

/// Execute a full multimodel query using extracted query-runtime orchestration.
pub async fn execute_query_components_with_service(
    query: &MultiModelQuery,
    execution_service: Arc<dyn QueryComponentExecutionService>,
    join_service: Arc<dyn JoinExecutionService>,
    max_parallel: usize,
) -> Result<Vec<SubQueryResult>> {
    if query.components.is_empty() {
        return Ok(Vec::new());
    }

    let semaphore = Arc::new(Semaphore::new(max_parallel));
    let (parallel_components, dependent_components): (Vec<_>, Vec<_>) = query
        .components
        .iter()
        .enumerate()
        .partition(|(_, component)| component.is_parallelizable());

    let mut results = Vec::with_capacity(query.components.len());

    if !parallel_components.is_empty() {
        let parallel_results = execute_parallel_batch_with_service(
            parallel_components
                .iter()
                .map(|(idx, component)| (*idx, (*component).clone()))
                .collect(),
            execution_service.clone(),
            semaphore,
        )
        .await?;
        results.extend(parallel_results);
    }

    for (idx, component) in dependent_components {
        let context: HashMap<usize, &SubQueryResult> = results
            .iter()
            .map(|(prior_idx, result)| (*prior_idx, result))
            .collect();
        let result = execute_component_with_context_and_join_service(
            component,
            execution_service.as_ref(),
            &context,
            join_service.as_ref(),
        )
        .await?;
        results.push((idx, result));
    }

    results.sort_by_key(|(idx, _)| *idx);
    Ok(results.into_iter().map(|(_, result)| result).collect())
}

/// Execute a single query component through the extracted query-runtime service contract.
pub async fn execute_component_with_service<S>(
    component: &QueryComponent,
    service: &S,
) -> Result<SubQueryResult>
where
    S: QueryComponentExecutionService + ?Sized,
{
    let start = Instant::now();
    let result = execute_operation_with_service(&component.operation, service, None).await?;
    Ok(with_execution_time(
        result,
        start.elapsed().as_micros() as u64,
    ))
}

/// Execute a query component with dependency context and apply extracted join orchestration.
pub async fn execute_component_with_context_and_join_service<S, J>(
    component: &QueryComponent,
    service: &S,
    context: &HashMap<usize, &SubQueryResult>,
    join_service: &J,
) -> Result<SubQueryResult>
where
    S: QueryComponentExecutionService + ?Sized,
    J: JoinExecutionService + ?Sized,
{
    let start = Instant::now();
    let raw_result =
        execute_operation_with_service(&component.operation, service, Some(context)).await?;
    let raw_result = with_execution_time(raw_result, start.elapsed().as_micros() as u64);

    if component.dependencies.is_empty() {
        return Ok(raw_result);
    }

    Ok(
        apply_component_dependencies(&raw_result, &component.dependencies, context, join_service)
            .await,
    )
}

/// Apply component dependency joins to an already-executed subquery result.
pub async fn apply_component_dependencies<J>(
    component_result: &SubQueryResult,
    dependencies: &[ComponentDependency],
    context: &HashMap<usize, &SubQueryResult>,
    join_service: &J,
) -> SubQueryResult
where
    J: JoinExecutionService + ?Sized,
{
    execute_multi_join_with(component_result, dependencies, context, join_service).await
}

async fn execute_operation_with_service<S>(
    operation: &ModelOperation,
    service: &S,
    context: Option<&HashMap<usize, &SubQueryResult>>,
) -> Result<SubQueryResult>
where
    S: QueryComponentExecutionService + ?Sized,
{
    match operation {
        ModelOperation::VectorSearch(expr) => service.execute_vector_search(expr).await,
        ModelOperation::DocumentQuery(expr) => service.execute_document_query(expr).await,
        ModelOperation::GraphQuery(expr) => service.execute_graph_query(expr).await,
        ModelOperation::GraphTraversal(expr) => {
            service.execute_graph_traversal(expr, context).await
        }
        ModelOperation::LogQuery(expr) => service.execute_log_query(expr).await,
        ModelOperation::MetricQuery(expr) => service.execute_metric_query(expr).await,
    }
}

fn with_execution_time(mut result: SubQueryResult, execution_time_us: u64) -> SubQueryResult {
    result.execution_time_us = execution_time_us;
    result
}

async fn execute_parallel_batch_with_service(
    components: Vec<(usize, QueryComponent)>,
    execution_service: Arc<dyn QueryComponentExecutionService>,
    semaphore: Arc<Semaphore>,
) -> Result<Vec<(usize, SubQueryResult)>> {
    let mut handles = Vec::with_capacity(components.len());

    for (idx, component) in components {
        let execution_service = execution_service.clone();
        let semaphore = semaphore.clone();
        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire().await?;
            let result =
                execute_component_with_service(&component, execution_service.as_ref()).await?;
            Ok::<(usize, SubQueryResult), anyhow::Error>((idx, result))
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(Ok(result)) => results.push(result),
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(anyhow::anyhow!("Task join error: {}", err)),
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use proximadb_data_model::DataModel;
    use proximadb_document_query::DocumentQueryExpr;
    use proximadb_graph::query::traversal::GraphTraversalExpr;
    use proximadb_graph_subset::LoweredGraphQuery as GraphQueryExpr;
    use proximadb_multimodel_query::{
        ComponentDependency, JoinType, ModelOperation, MultiModelQuery, QueryComponent,
    };
    use proximadb_observability_query::{LogQueryExpr, MetricQueryExpr};
    use proximadb_vector_query::{DistanceMetric, VectorSearchExpr, VectorSearchParams};

    use super::*;
    use crate::{JoinResult, UnifiedRecord};

    #[derive(Default)]
    struct MockExecutionService;

    #[async_trait]
    impl QueryComponentExecutionService for MockExecutionService {
        async fn execute_vector_search(&self, expr: &VectorSearchExpr) -> Result<SubQueryResult> {
            Ok(SubQueryResult {
                source_model: DataModel::Vector,
                records_returned: 1,
                records: vec![UnifiedRecord {
                    id: expr.collection.clone(),
                    source_model: DataModel::Vector,
                    data: serde_json::json!({"source": "vector"}),
                    score: Some(0.9),
                    metadata: HashMap::new(),
                }],
                total_count: Some(1),
                execution_time_us: 0,
                records_scanned: 1,
            })
        }

        async fn execute_document_query(&self, expr: &DocumentQueryExpr) -> Result<SubQueryResult> {
            Ok(SubQueryResult {
                source_model: DataModel::Document,
                records_returned: 1,
                records: vec![UnifiedRecord {
                    id: expr.collection.clone(),
                    source_model: DataModel::Document,
                    data: serde_json::json!({"source": "document"}),
                    score: None,
                    metadata: HashMap::new(),
                }],
                total_count: Some(1),
                execution_time_us: 0,
                records_scanned: 1,
            })
        }

        async fn execute_graph_query(&self, expr: &GraphQueryExpr) -> Result<SubQueryResult> {
            Ok(SubQueryResult {
                source_model: DataModel::Graph,
                records_returned: 1,
                records: vec![UnifiedRecord {
                    id: expr.graph_name.clone(),
                    source_model: DataModel::Graph,
                    data: serde_json::json!({"source": "graph_query"}),
                    score: None,
                    metadata: HashMap::new(),
                }],
                total_count: Some(1),
                execution_time_us: 0,
                records_scanned: 1,
            })
        }

        async fn execute_graph_traversal(
            &self,
            expr: &GraphTraversalExpr,
            _context: Option<&HashMap<usize, &SubQueryResult>>,
        ) -> Result<SubQueryResult> {
            Ok(SubQueryResult {
                source_model: DataModel::Graph,
                records_returned: 1,
                records: vec![UnifiedRecord {
                    id: expr.graph_name.clone(),
                    source_model: DataModel::Graph,
                    data: serde_json::json!({"source": "graph_traversal"}),
                    score: None,
                    metadata: HashMap::new(),
                }],
                total_count: Some(1),
                execution_time_us: 0,
                records_scanned: 1,
            })
        }

        async fn execute_log_query(&self, expr: &LogQueryExpr) -> Result<SubQueryResult> {
            Ok(SubQueryResult {
                source_model: DataModel::Observability,
                records_returned: 1,
                records: vec![UnifiedRecord {
                    id: expr.namespace.clone(),
                    source_model: DataModel::Observability,
                    data: serde_json::json!({"source": "log"}),
                    score: None,
                    metadata: HashMap::new(),
                }],
                total_count: Some(1),
                execution_time_us: 0,
                records_scanned: 1,
            })
        }

        async fn execute_metric_query(&self, expr: &MetricQueryExpr) -> Result<SubQueryResult> {
            Ok(SubQueryResult {
                source_model: DataModel::Observability,
                records_returned: 1,
                records: vec![UnifiedRecord {
                    id: expr.metric_name.clone(),
                    source_model: DataModel::Observability,
                    data: serde_json::json!({"source": "metric"}),
                    score: None,
                    metadata: HashMap::new(),
                }],
                total_count: Some(1),
                execution_time_us: 0,
                records_scanned: 1,
            })
        }
    }

    struct MockJoinService;

    #[async_trait(?Send)]
    impl JoinExecutionService for MockJoinService {
        async fn execute_join(
            &self,
            left: &[UnifiedRecord],
            _right: &[UnifiedRecord],
            _dependency: &ComponentDependency,
        ) -> JoinResult {
            JoinResult {
                matched: vec![left[0].clone()],
                unmatched_left: Vec::new(),
                has_matches: true,
            }
        }
    }

    #[tokio::test]
    async fn execute_component_dispatches_vector_search_through_service() {
        let component = QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "vectors".to_string(),
                query_vector: vec![0.1, 0.2],
                top_k: 5,
                threshold: None,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: Vec::new(),
            dependencies: Vec::new(),
        };

        let result = execute_component_with_service(&component, &MockExecutionService)
            .await
            .unwrap();

        assert_eq!(result.source_model, DataModel::Vector);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].id, "vectors");
    }

    #[tokio::test]
    async fn execute_component_with_context_applies_join_dependencies() {
        let component = QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                collection: "documents".to_string(),
                path_filters: Vec::new(),
                text_search: None,
                projection: Vec::new(),
                sort: None,
                limit: None,
            }),
            filters: Vec::new(),
            dependencies: vec![ComponentDependency {
                component_index: 0,
                join_field: "id".to_string(),
                join_type: JoinType::Inner,
            }],
        };

        let prior_result = SubQueryResult {
            source_model: DataModel::Vector,
            records_returned: 1,
            records: vec![UnifiedRecord {
                id: "documents".to_string(),
                source_model: DataModel::Vector,
                data: serde_json::json!({"id": "documents"}),
                score: Some(0.8),
                metadata: HashMap::new(),
            }],
            total_count: Some(1),
            execution_time_us: 0,
            records_scanned: 1,
        };
        let mut context = HashMap::new();
        context.insert(0usize, &prior_result);

        let result = execute_component_with_context_and_join_service(
            &component,
            &MockExecutionService,
            &context,
            &MockJoinService,
        )
        .await
        .unwrap();

        assert_eq!(result.records_returned, 1);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].id, "documents");
    }

    #[tokio::test]
    async fn execute_query_components_with_service_runs_parallel_and_dependent_components() {
        let query = MultiModelQuery {
            components: vec![
                QueryComponent {
                    model: DataModel::Vector,
                    operation: ModelOperation::VectorSearch(VectorSearchExpr {
                        collection: "vectors".to_string(),
                        query_vector: vec![0.1, 0.2],
                        top_k: 5,
                        threshold: None,
                        metric: DistanceMetric::Cosine,
                        params: VectorSearchParams::default(),
                    }),
                    filters: Vec::new(),
                    dependencies: Vec::new(),
                },
                QueryComponent {
                    model: DataModel::Document,
                    operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                        collection: "documents".to_string(),
                        path_filters: Vec::new(),
                        text_search: None,
                        projection: Vec::new(),
                        sort: None,
                        limit: None,
                    }),
                    filters: Vec::new(),
                    dependencies: vec![ComponentDependency {
                        component_index: 0,
                        join_field: "id".to_string(),
                        join_type: JoinType::Inner,
                    }],
                },
            ],
            fusion_strategy: proximadb_query_fusion::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: Vec::new(),
            order_by: None,
        };

        let results = execute_query_components_with_service(
            &query,
            Arc::new(MockExecutionService),
            Arc::new(MockJoinService),
            2,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].source_model, DataModel::Vector);
        assert_eq!(results[1].source_model, DataModel::Document);
        assert_eq!(results[1].records_returned, 1);
    }

    #[test]
    fn apply_component_dependencies_handles_empty_dependency_list() {
        let result = SubQueryResult {
            source_model: DataModel::Document,
            records_returned: 0,
            records: Vec::new(),
            total_count: Some(0),
            execution_time_us: 7,
            records_scanned: 0,
        };
        let context = HashMap::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let joined = runtime.block_on(apply_component_dependencies(
            &result,
            &[],
            &context,
            &MockJoinService,
        ));

        assert_eq!(joined.records_returned, 0);
        assert_eq!(joined.execution_time_us, 7);
    }
}
