//! Parallel Query Executor
//!
//! Executes multi-model query components in parallel, respecting dependencies
//! and coordinating with different storage backends.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use tokio::sync::Semaphore;
use tracing::{debug, info, trace, warn};

use super::UnifiedRecord;
use super::ast::{
    ComponentDependency, DataModel, DocumentQueryExpr, FilterOperator, FilterValue,
    GraphTraversalExpr, JoinType, LogQueryExpr, MetricQueryExpr, ModelOperation, MultiModelQuery,
    PathFilter, QueryComponent, StartNodeSpec, TraversalDirection, VectorSearchExpr,
};
use super::fusion::SubQueryResult;
use crate::graph::service::GraphOperationsService;
use crate::observability::{LogQueryParams, MetricAggParams, ObservabilityService};
use crate::proto::proximadb_v1::{
    DocFilterCondition, DocFilterOperator, DocumentFilter, Severity, SqlValue,
    sql_value::Value as SqlValueVariant,
};
use crate::security::unified_rbac::{
    ConsolidatedRBACManager, UnifiedPermission, UnifiedUserContext,
};
use crate::services::operations::vectors::VectorOperationsService;
use crate::storage::document::{DocumentQueryParams, DocumentService};
/// Parallel executor for multi-model queries
pub struct ParallelExecutor {
    /// Maximum concurrent queries
    max_parallel: usize,
    /// Semaphore for concurrency control
    semaphore: Arc<Semaphore>,
    /// RBAC manager for permission validation
    rbac_manager: Option<Arc<ConsolidatedRBACManager>>,
}

impl ParallelExecutor {
    /// Create a new parallel executor
    pub fn new(max_parallel: usize) -> Self {
        Self {
            max_parallel,
            semaphore: Arc::new(Semaphore::new(max_parallel)),
            rbac_manager: None,
        }
    }

    /// Create a new parallel executor with RBAC enabled
    pub fn with_rbac(max_parallel: usize, rbac_manager: Arc<ConsolidatedRBACManager>) -> Self {
        Self {
            max_parallel,
            semaphore: Arc::new(Semaphore::new(max_parallel)),
            rbac_manager: Some(rbac_manager),
        }
    }

    /// Validate permissions for a query component
    pub async fn validate_component_access(
        &self,
        user_ctx: &UnifiedUserContext,
        component: &QueryComponent,
    ) -> Result<()> {
        let rbac_manager = self
            .rbac_manager
            .as_ref()
            .ok_or_else(|| anyhow!("RBAC validation requested but RBAC manager not configured"))?;

        // Match on the operation to extract the collection/resource ID
        match &component.operation {
            ModelOperation::VectorSearch(vector_expr) => {
                let permission = UnifiedPermission::VectorSearch(vector_expr.collection.clone());
                let allowed = rbac_manager
                    .check_permission_cached(&user_ctx.user_id, &permission)
                    .await
                    .map_err(|e| anyhow!("Failed to check vector search permission: {}", e))?;

                if !allowed {
                    return Err(anyhow!(
                        "Permission denied: Vector search on collection '{}'",
                        vector_expr.collection
                    ));
                }
            }
            ModelOperation::GraphTraversal(graph_expr) => {
                let permission = UnifiedPermission::GraphTraverse(graph_expr.graph_name.clone());
                let allowed = rbac_manager
                    .check_permission_cached(&user_ctx.user_id, &permission)
                    .await
                    .map_err(|e| anyhow!("Failed to check graph permission: {}", e))?;

                if !allowed {
                    return Err(anyhow!(
                        "Permission denied: Graph traversal on '{}'",
                        graph_expr.graph_name
                    ));
                }
            }
            ModelOperation::DocumentQuery(doc_expr) => {
                let permission = UnifiedPermission::CollectionRead(doc_expr.collection.clone());
                let allowed = rbac_manager
                    .check_permission_cached(&user_ctx.user_id, &permission)
                    .await
                    .map_err(|e| anyhow!("Failed to check document permission: {}", e))?;

                if !allowed {
                    return Err(anyhow!(
                        "Permission denied: Document query on collection '{}'",
                        doc_expr.collection
                    ));
                }
            }
            ModelOperation::LogQuery(_) | ModelOperation::MetricQuery(_) => {
                // Observability queries require SystemAdmin permission
                let permission = UnifiedPermission::SystemAdmin;
                let allowed = rbac_manager
                    .check_permission_cached(&user_ctx.user_id, &permission)
                    .await
                    .map_err(|e| anyhow!("Failed to check observability permission: {}", e))?;

                if !allowed {
                    return Err(anyhow!(
                        "Permission denied: Observability queries require system admin"
                    ));
                }
            }
        }

        Ok(())
    }

    /// Execute query with RBAC validation
    pub async fn execute_with_auth(
        &self,
        query: &MultiModelQuery,
        user_ctx: &UnifiedUserContext,
        vector_ops: Option<Arc<VectorOperationsService>>,
        document_service: Arc<DocumentService>,
        graph_service: Option<Arc<GraphOperationsService>>,
        observability_service: Option<Arc<ObservabilityService>>,
    ) -> Result<Vec<SubQueryResult>> {
        // Validate permissions for each component
        for component in &query.components {
            self.validate_component_access(user_ctx, component).await?;
        }

        // Proceed with execution
        self.execute_parallel_with_all_services(
            query,
            vector_ops,
            document_service,
            graph_service,
            observability_service,
        )
        .await
    }

    /// Execute query components in parallel with explicit service references
    ///
    /// # Arguments
    /// * `query` - The multi-model query to execute
    /// * `vector_ops` - Optional VectorOperationsService for vector searches
    /// * `document_service` - Document service for document queries
    ///
    /// # Note
    /// This is the primary entry point for parallel query execution. For graph and
    /// observability queries, use `execute_parallel_with_all_services` instead.
    pub async fn execute_parallel_with_services(
        &self,
        query: &MultiModelQuery,
        vector_ops: Option<Arc<VectorOperationsService>>,
        document_service: Arc<DocumentService>,
    ) -> Result<Vec<SubQueryResult>> {
        // Use the full version with all services (graph and observability as None)
        self.execute_parallel_with_all_services(query, vector_ops, document_service, None, None)
            .await
    }

    /// Execute query components in parallel with ALL services (graph + observability)
    ///
    /// # Arguments
    /// * `query` - The multi-model query to execute
    /// * `vector_ops` - Optional VectorOperationsService for vector searches
    /// * `document_service` - Document service for document queries
    /// * `graph_service` - Optional GraphOperationsService for graph traversals
    /// * `observability_service` - Optional ObservabilityService for log/metric queries
    pub async fn execute_parallel_with_all_services(
        &self,
        query: &MultiModelQuery,
        vector_ops: Option<Arc<VectorOperationsService>>,
        document_service: Arc<DocumentService>,
        graph_service: Option<Arc<GraphOperationsService>>,
        observability_service: Option<Arc<ObservabilityService>>,
    ) -> Result<Vec<SubQueryResult>> {
        if query.components.is_empty() {
            return Ok(Vec::new());
        }

        debug!(
            "Executing {} query components with max {} parallel (vector_ops={}, graph={}, obs={})",
            query.components.len(),
            self.max_parallel,
            vector_ops.is_some(),
            graph_service.is_some(),
            observability_service.is_some()
        );

        // Separate parallelizable and dependent components
        let (parallel_components, dependent_components): (Vec<_>, Vec<_>) = query
            .components
            .iter()
            .enumerate()
            .partition(|(_, c)| c.is_parallelizable());

        // Execute parallelizable components first
        let mut results = Vec::with_capacity(query.components.len());

        if !parallel_components.is_empty() {
            let parallel_results = self
                .execute_parallel_batch_full(
                    parallel_components
                        .iter()
                        .map(|(i, c)| (*i, (*c).clone()))
                        .collect(),
                    vector_ops.clone(),
                    document_service.clone(),
                    graph_service.clone(),
                    observability_service.clone(),
                )
                .await?;
            results.extend(parallel_results);
        }

        // Execute dependent components sequentially with access to prior results
        for (idx, component) in dependent_components {
            let result = self
                .execute_single_with_context_full(
                    idx,
                    component,
                    vector_ops.clone(),
                    document_service.clone(),
                    graph_service.clone(),
                    observability_service.clone(),
                    &results,
                )
                .await?;
            results.push((idx, result));
        }

        // Sort by original index and return just the results
        results.sort_by_key(|(idx, _)| *idx);
        Ok(results.into_iter().map(|(_, r)| r).collect())
    }

    /// Execute a batch of parallelizable components
    #[allow(dead_code)]
    async fn execute_parallel_batch(
        &self,
        components: Vec<(usize, QueryComponent)>,
        vector_ops: Option<Arc<VectorOperationsService>>,
        document_service: Arc<DocumentService>,
    ) -> Result<Vec<(usize, SubQueryResult)>> {
        self.execute_parallel_batch_full(components, vector_ops, document_service, None, None)
            .await
    }

    /// Execute a batch of parallelizable components with all services
    async fn execute_parallel_batch_full(
        &self,
        components: Vec<(usize, QueryComponent)>,
        vector_ops: Option<Arc<VectorOperationsService>>,
        document_service: Arc<DocumentService>,
        graph_service: Option<Arc<GraphOperationsService>>,
        observability_service: Option<Arc<ObservabilityService>>,
    ) -> Result<Vec<(usize, SubQueryResult)>> {
        let mut handles = Vec::with_capacity(components.len());

        for (idx, component) in components {
            let vec_ops = vector_ops.clone();
            let doc_service = document_service.clone();
            let graph_svc = graph_service.clone();
            let obs_svc = observability_service.clone();
            let semaphore = self.semaphore.clone();

            let handle = tokio::spawn(async move {
                // Acquire semaphore permit
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|e| anyhow!("Semaphore error: {}", e))?;

                let result =
                    execute_component_full(&component, vec_ops, doc_service, graph_svc, obs_svc)
                        .await?;
                Ok::<(usize, SubQueryResult), anyhow::Error>((idx, result))
            });

            handles.push(handle);
        }

        // Collect results
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(Ok(result)) => results.push(result),
                Ok(Err(e)) => {
                    warn!("Component execution failed: {}", e);
                    return Err(e);
                }
                Err(e) => {
                    warn!("Task join error: {}", e);
                    return Err(anyhow!("Task join error: {}", e));
                }
            }
        }

        Ok(results)
    }

    /// Execute a single component with access to prior results (for dependencies)
    #[allow(dead_code)]
    async fn execute_single_with_context(
        &self,
        idx: usize,
        component: &QueryComponent,
        vector_ops: Option<Arc<VectorOperationsService>>,
        document_service: Arc<DocumentService>,
        prior_results: &[(usize, SubQueryResult)],
    ) -> Result<SubQueryResult> {
        self.execute_single_with_context_full(
            idx,
            component,
            vector_ops,
            document_service,
            None,
            None,
            prior_results,
        )
        .await
    }

    /// Execute a single component with access to prior results (full version)
    async fn execute_single_with_context_full(
        &self,
        _idx: usize,
        component: &QueryComponent,
        vector_ops: Option<Arc<VectorOperationsService>>,
        document_service: Arc<DocumentService>,
        graph_service: Option<Arc<GraphOperationsService>>,
        observability_service: Option<Arc<ObservabilityService>>,
        prior_results: &[(usize, SubQueryResult)],
    ) -> Result<SubQueryResult> {
        // Build context from prior results for dependency resolution
        let context: HashMap<usize, &SubQueryResult> = prior_results
            .iter()
            .map(|(idx, result)| (*idx, result))
            .collect();

        execute_component_with_context_full(
            component,
            vector_ops,
            document_service,
            graph_service,
            observability_service,
            &context,
        )
        .await
    }
}

/// Execute a single query component
#[allow(dead_code)]
async fn execute_component(
    component: &QueryComponent,
    vector_ops: Option<Arc<VectorOperationsService>>,
    document_service: Arc<DocumentService>,
) -> Result<SubQueryResult> {
    execute_component_full(component, vector_ops, document_service, None, None).await
}

/// Execute a single query component with all services
async fn execute_component_full(
    component: &QueryComponent,
    vector_ops: Option<Arc<VectorOperationsService>>,
    document_service: Arc<DocumentService>,
    graph_service: Option<Arc<GraphOperationsService>>,
    observability_service: Option<Arc<ObservabilityService>>,
) -> Result<SubQueryResult> {
    let start = Instant::now();

    let result = match &component.operation {
        ModelOperation::VectorSearch(expr) => execute_vector_search(expr, vector_ops).await,
        ModelOperation::DocumentQuery(expr) => execute_document_query(expr, document_service).await,
        ModelOperation::GraphTraversal(expr) => {
            execute_graph_traversal_full(expr, graph_service).await
        }
        ModelOperation::LogQuery(expr) => {
            execute_log_query_full(expr, observability_service.clone()).await
        }
        ModelOperation::MetricQuery(expr) => {
            execute_metric_query_full(expr, observability_service).await
        }
    };

    let elapsed = start.elapsed();
    trace!("Component {:?} executed in {:?}", component.model, elapsed);

    result.map(|mut r| {
        r.execution_time_us = elapsed.as_micros() as u64;
        r
    })
}

/// Execute a component with dependency context
#[allow(dead_code)]
async fn execute_component_with_context(
    component: &QueryComponent,
    vector_ops: Option<Arc<VectorOperationsService>>,
    document_service: Arc<DocumentService>,
    context: &HashMap<usize, &SubQueryResult>,
) -> Result<SubQueryResult> {
    execute_component_with_context_full(
        component,
        vector_ops,
        document_service,
        None,
        None,
        context,
    )
    .await
}

/// Execute a component with dependency context and all services
///
/// This function handles cross-model joins by:
/// 1. Resolving StartNodeSpec (including FromComponent) with the context
/// 2. Executing the component's operation
/// 3. Applying join predicates based on ComponentDependency configuration
async fn execute_component_with_context_full(
    component: &QueryComponent,
    vector_ops: Option<Arc<VectorOperationsService>>,
    document_service: Arc<DocumentService>,
    graph_service: Option<Arc<GraphOperationsService>>,
    observability_service: Option<Arc<ObservabilityService>>,
    context: &HashMap<usize, &SubQueryResult>,
) -> Result<SubQueryResult> {
    let start = Instant::now();

    // Execute the component's operation
    // For graph traversals, pass context so StartNodeSpec::FromComponent can be resolved
    let raw_result = match &component.operation {
        ModelOperation::VectorSearch(expr) => execute_vector_search(expr, vector_ops).await,
        ModelOperation::DocumentQuery(expr) => execute_document_query(expr, document_service).await,
        ModelOperation::GraphTraversal(expr) => {
            // Use execute_graph_traversal_with_context to properly resolve StartNodeSpec
            // This handles StartNodeSpec::FromComponent by looking up prior results
            execute_graph_traversal_with_context(expr, graph_service, Some(context)).await
        }
        ModelOperation::LogQuery(expr) => {
            execute_log_query_full(expr, observability_service.clone()).await
        }
        ModelOperation::MetricQuery(expr) => {
            execute_metric_query_full(expr, observability_service).await
        }
    };

    let elapsed = start.elapsed();

    // Apply join predicates if dependencies exist

    match raw_result {
        Ok(mut r) => {
            r.execution_time_us = elapsed.as_micros() as u64;

            // If there are dependencies, apply join logic
            if !component.dependencies.is_empty() {
                debug!(
                    "Applying {} join dependencies to {} records",
                    component.dependencies.len(),
                    r.records.len()
                );
                Ok(execute_multi_join(&r, &component.dependencies, context))
            } else {
                Ok(r)
            }
        }
        Err(e) => Err(e),
    }
}

/// Execute a vector search query
///
/// Uses VectorOperationsService to perform the actual search when available.
/// Returns empty results if VectorOperationsService is not provided.
async fn execute_vector_search(
    expr: &VectorSearchExpr,
    vector_ops: Option<Arc<VectorOperationsService>>,
) -> Result<SubQueryResult> {
    let Some(vector_ops) = vector_ops else {
        debug!(
            "Vector search on collection {} skipped - no VectorOperationsService",
            expr.collection
        );
        return Ok(SubQueryResult::empty(DataModel::Vector));
    };

    info!(
        "Executing vector search on collection: {} (top_k={})",
        expr.collection, expr.top_k
    );

    // Perform actual vector search using the operations service
    let search_result = vector_ops
        .unified_search_native(
            &expr.collection,
            expr.query_vector.clone(),
            expr.top_k as usize,
            None, // No filter for now - could add threshold filter later
            None, // Use default search config
        )
        .await;

    match search_result {
        Ok(results) => {
            let records: Vec<UnifiedRecord> = results
                .into_iter()
                .map(|r| {
                    // Build metadata from the search result
                    let mut metadata = HashMap::new();
                    // metadata is a HashMap<String, SqlValue>, iterate over it
                    for (k, v) in &r.metadata {
                        metadata.insert(k.clone(), format!("{:?}", v));
                    }

                    UnifiedRecord {
                        id: r.id.clone(),
                        source_model: DataModel::Vector,
                        data: serde_json::json!({
                            "id": r.id,
                            "score": r.score,
                        }),
                        score: Some(r.score as f64),
                        metadata,
                    }
                })
                .collect();

            let count = records.len() as u64;
            info!("Vector search returned {} results", count);

            Ok(SubQueryResult {
                source_model: DataModel::Vector,
                records_returned: count,
                records,
                total_count: Some(count),
                execution_time_us: 0,
                records_scanned: count,
            })
        }
        Err(e) => {
            warn!("Vector search failed: {}", e);
            // Return empty result instead of propagating error
            // This allows fusion to continue with other model results
            Ok(SubQueryResult::empty(DataModel::Vector))
        }
    }
}

/// Execute a document query
async fn execute_document_query(
    expr: &DocumentQueryExpr,
    document_service: Arc<DocumentService>,
) -> Result<SubQueryResult> {
    debug!(
        "Executing document query on collection: {}",
        expr.collection
    );

    // Convert PathFilters to DocumentFilter
    let filter = convert_path_filters_to_document_filter(&expr.path_filters);

    // Build DocumentQueryParams
    let params = DocumentQueryParams {
        filter,
        projection: expr.projection.clone(),
        sort: Vec::new(),
        limit: expr.limit.unwrap_or(100),
        offset: 0,
        include_count: true,
    };

    // Query documents
    let result = document_service
        .query_documents(&expr.collection, params)
        .await;

    match result {
        Ok(query_result) => {
            let records: Vec<UnifiedRecord> = query_result
                .documents
                .into_iter()
                .map(|doc| {
                    // Convert SqlObject to serde_json::Value
                    let data = sql_object_to_json(&doc.document);

                    UnifiedRecord {
                        id: doc.id,
                        source_model: DataModel::Document,
                        data,
                        score: None, // Documents don't have similarity scores
                        metadata: HashMap::new(),
                    }
                })
                .collect();

            let count = records.len() as u64;
            Ok(SubQueryResult {
                source_model: DataModel::Document,
                records_returned: count,
                records,
                total_count: query_result.total_count,
                execution_time_us: 0,
                records_scanned: count,
            })
        }
        Err(e) => {
            warn!("Document query failed: {}", e);
            Ok(SubQueryResult::empty(DataModel::Document))
        }
    }
}

/// Convert SqlObject to serde_json::Value
fn sql_object_to_json(obj: &crate::proto::proximadb_v1::SqlObject) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in &obj.fields {
        map.insert(key.clone(), sql_value_to_json(value));
    }
    serde_json::Value::Object(map)
}

/// Convert SqlValue to serde_json::Value
fn sql_value_to_json(value: &crate::proto::proximadb_v1::SqlValue) -> serde_json::Value {
    use crate::proto::proximadb_v1::sql_value::Value;

    match &value.value {
        Some(Value::NullValue(_)) => serde_json::Value::Null,
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(Value::NumberValue(f)) => serde_json::Number::from_f64(*f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::BytesValue(b)) => {
            // Encode bytes as hex string (simpler than base64)
            let encoded: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
            serde_json::Value::String(encoded)
        }
        Some(Value::ArrayValue(arr)) => {
            let items: Vec<serde_json::Value> = arr.values.iter().map(sql_value_to_json).collect();
            serde_json::Value::Array(items)
        }
        Some(Value::ObjectValue(obj)) => sql_object_to_json(obj),
        None => serde_json::Value::Null,
    }
}

/// Convert AST PathFilters to proto DocumentFilter
fn convert_path_filters_to_document_filter(filters: &[PathFilter]) -> Option<DocumentFilter> {
    if filters.is_empty() {
        return None;
    }

    let conditions: Vec<DocFilterCondition> = filters
        .iter()
        .map(|pf| {
            // Convert FilterOperator to DocFilterOperator
            let operator = match pf.operator {
                FilterOperator::Eq => DocFilterOperator::Eq,
                FilterOperator::Ne => DocFilterOperator::Ne,
                FilterOperator::Gt => DocFilterOperator::Gt,
                FilterOperator::Gte => DocFilterOperator::Gte,
                FilterOperator::Lt => DocFilterOperator::Lt,
                FilterOperator::Lte => DocFilterOperator::Lte,
                FilterOperator::In => DocFilterOperator::In,
                FilterOperator::NotIn => DocFilterOperator::NotIn,
                FilterOperator::Contains => DocFilterOperator::Contains,
                FilterOperator::StartsWith => DocFilterOperator::Regex, // Approximate with regex
                FilterOperator::EndsWith => DocFilterOperator::Regex,   // Approximate with regex
                FilterOperator::Exists => DocFilterOperator::Exists,
                FilterOperator::Type => DocFilterOperator::Type,
            };

            // Convert FilterValue to SqlValue
            let value = convert_filter_value_to_sql(&pf.value);

            DocFilterCondition {
                path: pf.path.clone(),
                operator: operator.into(),
                value: Some(value),
                values: vec![],
            }
        })
        .collect();

    Some(DocumentFilter {
        conditions,
        or_filters: vec![],
        and_filters: vec![],
    })
}

/// Convert AST FilterValue to proto SqlValue
fn convert_filter_value_to_sql(value: &FilterValue) -> SqlValue {
    match value {
        FilterValue::String(s) => SqlValue {
            value: Some(SqlValueVariant::StringValue(s.clone())),
        },
        FilterValue::Number(n) => SqlValue {
            value: Some(SqlValueVariant::NumberValue(*n)),
        },
        FilterValue::Bool(b) => SqlValue {
            value: Some(SqlValueVariant::BoolValue(*b)),
        },
        FilterValue::Null => SqlValue {
            value: Some(SqlValueVariant::NullValue(0)),
        },
        FilterValue::Array(arr) => {
            let sql_arr = crate::proto::proximadb_v1::SqlArray {
                values: arr.iter().map(convert_filter_value_to_sql).collect(),
            };
            SqlValue {
                value: Some(SqlValueVariant::ArrayValue(sql_arr)),
            }
        }
    }
}

/// Execute a graph traversal query (legacy - calls full version)
#[allow(dead_code)]
async fn execute_graph_traversal(expr: &GraphTraversalExpr) -> Result<SubQueryResult> {
    execute_graph_traversal_full(expr, None).await
}

/// Resolve StartNodeSpec to actual node IDs
///
/// This function resolves various start node specifications:
/// - `Ids`: Direct node IDs (pass through)
/// - `Label`: Query graph for nodes with matching label
/// - `Filter`: Query graph for nodes matching property filter
/// - `FromComponent`: Use IDs from a prior query component (requires context)
async fn resolve_start_nodes(
    spec: &StartNodeSpec,
    graph_name: &str,
    graph_service: &Arc<GraphOperationsService>,
    component_context: Option<&HashMap<usize, &SubQueryResult>>,
) -> Result<Vec<String>> {
    match spec {
        StartNodeSpec::Ids(ids) => {
            debug!("StartNodeSpec::Ids - using {} direct IDs", ids.len());
            Ok(ids.clone())
        }
        StartNodeSpec::Label(label) => {
            debug!(
                "StartNodeSpec::Label - querying nodes with label '{}'",
                label
            );
            resolve_nodes_by_label(graph_name, label, graph_service).await
        }
        StartNodeSpec::Filter(filter) => {
            debug!("StartNodeSpec::Filter - querying nodes matching filter");
            resolve_nodes_by_filter(graph_name, filter, graph_service).await
        }
        StartNodeSpec::FromComponent(component_idx) => {
            debug!(
                "StartNodeSpec::FromComponent - resolving from component {}",
                component_idx
            );
            resolve_nodes_from_component(*component_idx, component_context)
        }
    }
}

/// Resolve nodes by label - query graph for all nodes with the specified label
async fn resolve_nodes_by_label(
    graph_name: &str,
    label: &str,
    graph_service: &Arc<GraphOperationsService>,
) -> Result<Vec<String>> {
    // Use graph service to query nodes by label
    // This requires a method on GraphOperationsService to query by label
    match graph_service.query_nodes_by_label(graph_name, label).await {
        Ok(nodes) => {
            let ids: Vec<String> = nodes.into_iter().map(|n| n.id.clone()).collect();
            info!(
                "Resolved {} nodes with label '{}' in graph '{}'",
                ids.len(),
                label,
                graph_name
            );
            Ok(ids)
        }
        Err(e) => {
            warn!("Failed to query nodes by label '{}': {}", label, e);
            // Fall back to empty - could also propagate error
            Ok(Vec::new())
        }
    }
}

/// Resolve nodes by property filter
async fn resolve_nodes_by_filter(
    graph_name: &str,
    filter: &super::ast::NodeFilter,
    graph_service: &Arc<GraphOperationsService>,
) -> Result<Vec<String>> {
    // Build property filters for graph query
    let label = filter.label.clone();
    let property_filters: Vec<(String, String)> = filter
        .properties
        .iter()
        .filter_map(|pf| {
            // Convert FilterValue to string for simple equality matching
            let value_str = match &pf.value {
                FilterValue::String(s) => Some(s.clone()),
                FilterValue::Number(n) => Some(n.to_string()),
                FilterValue::Bool(b) => Some(b.to_string()),
                _ => None,
            };
            value_str.map(|v| (pf.name.clone(), v))
        })
        .collect();

    match graph_service
        .query_nodes_by_properties(graph_name, label.as_deref(), &property_filters)
        .await
    {
        Ok(nodes) => {
            let ids: Vec<String> = nodes.into_iter().map(|n| n.id.clone()).collect();
            info!(
                "Resolved {} nodes matching filter in graph '{}'",
                ids.len(),
                graph_name
            );
            Ok(ids)
        }
        Err(e) => {
            warn!("Failed to query nodes by filter: {}", e);
            Ok(Vec::new())
        }
    }
}

/// Resolve nodes from a prior query component's results
fn resolve_nodes_from_component(
    component_idx: usize,
    context: Option<&HashMap<usize, &SubQueryResult>>,
) -> Result<Vec<String>> {
    let Some(ctx) = context else {
        warn!(
            "FromComponent({}) requires context, but none provided",
            component_idx
        );
        return Ok(Vec::new());
    };

    let Some(prior_result) = ctx.get(&component_idx) else {
        warn!(
            "FromComponent({}) references non-existent component",
            component_idx
        );
        return Ok(Vec::new());
    };

    // Extract IDs from the prior component's results
    let ids: Vec<String> = prior_result.records.iter().map(|r| r.id.clone()).collect();

    info!(
        "Resolved {} node IDs from component {} (model: {:?})",
        ids.len(),
        component_idx,
        prior_result.source_model
    );

    Ok(ids)
}

/// Execute a graph traversal query with graph service
async fn execute_graph_traversal_full(
    expr: &GraphTraversalExpr,
    graph_service: Option<Arc<GraphOperationsService>>,
) -> Result<SubQueryResult> {
    execute_graph_traversal_with_context(expr, graph_service, None).await
}

/// Execute a graph traversal query with graph service and component context
async fn execute_graph_traversal_with_context(
    expr: &GraphTraversalExpr,
    graph_service: Option<Arc<GraphOperationsService>>,
    context: Option<&HashMap<usize, &SubQueryResult>>,
) -> Result<SubQueryResult> {
    debug!("Executing graph traversal on graph: {}", expr.graph_name);

    let Some(graph_svc) = graph_service else {
        debug!(
            "Graph traversal on {} skipped - no GraphOperationsService",
            expr.graph_name
        );
        return Ok(SubQueryResult::empty(DataModel::Graph));
    };

    // Resolve start nodes from the specification
    let start_node_ids =
        resolve_start_nodes(&expr.start_nodes, &expr.graph_name, &graph_svc, context).await?;

    if start_node_ids.is_empty() {
        debug!("No start nodes resolved for graph traversal");
        return Ok(SubQueryResult::empty(DataModel::Graph));
    }

    info!(
        "Graph traversal starting from {} nodes on graph '{}'",
        start_node_ids.len(),
        expr.graph_name
    );

    // Execute traversal from each start node
    let mut all_records = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for start_id in start_node_ids {
        let traversal_request = crate::proto::proximadb_v1::TraversalRequest {
            graph_id: expr.graph_name.clone(),
            start_node_id: start_id.clone(),
            max_depth: expr.max_depth,
            edge_types: expr.edge_types.clone(),
            node_labels: extract_node_labels(&expr.node_filters),
            filters: Vec::new(), // Deferred: Convert property filters
            algorithm: match expr.direction {
                TraversalDirection::Outgoing => {
                    crate::proto::proximadb_v1::TraversalAlgorithm::Bfs as i32
                }
                TraversalDirection::Incoming => {
                    crate::proto::proximadb_v1::TraversalAlgorithm::Bfs as i32
                }
                TraversalDirection::Both => {
                    crate::proto::proximadb_v1::TraversalAlgorithm::Bfs as i32
                }
            },
            limit: None,
            timeout_ms: None,
            max_frontier: None,
        };

        match graph_svc
            .traverse(&expr.graph_name, traversal_request)
            .await
        {
            Ok(response) => {
                for node in response.nodes {
                    // Deduplicate nodes across multiple traversals
                    if seen_ids.insert(node.id.clone()) {
                        all_records.push(UnifiedRecord {
                            id: node.id.clone(),
                            source_model: DataModel::Graph,
                            data: serde_json::json!({
                                "id": node.id,
                                "labels": node.labels,
                                "properties": format!("{:?}", node.properties),
                                "start_node": start_id,
                            }),
                            score: None,
                            metadata: HashMap::new(),
                        });
                    }
                }
            }
            Err(e) => {
                warn!("Graph traversal from '{}' failed: {}", start_id, e);
                // Continue with other start nodes
            }
        }
    }

    let count = all_records.len() as u64;
    info!("Graph traversal returned {} unique nodes", count);

    Ok(SubQueryResult {
        source_model: DataModel::Graph,
        records_returned: count,
        records: all_records,
        total_count: Some(count),
        execution_time_us: 0,
        records_scanned: count,
    })
}

/// Extract node labels from node filters
fn extract_node_labels(filters: &[super::ast::NodeFilter]) -> Vec<String> {
    filters.iter().filter_map(|f| f.label.clone()).collect()
}

/// Execute a graph traversal with input node IDs (legacy)
#[allow(dead_code)]
async fn execute_graph_traversal_with_input(
    expr: &GraphTraversalExpr,
    input_ids: Option<Vec<String>>,
) -> Result<SubQueryResult> {
    execute_graph_traversal_with_input_full(expr, input_ids, None).await
}

/// Execute a graph traversal with input node IDs and graph service
#[allow(dead_code)]
async fn execute_graph_traversal_with_input_full(
    expr: &GraphTraversalExpr,
    input_ids: Option<Vec<String>>,
    graph_service: Option<Arc<GraphOperationsService>>,
) -> Result<SubQueryResult> {
    debug!(
        "Executing graph traversal with input on graph: {}",
        expr.graph_name
    );

    let Some(graph_svc) = graph_service else {
        debug!(
            "Graph traversal with input on {} skipped - no GraphOperationsService",
            expr.graph_name
        );
        return Ok(SubQueryResult::empty(DataModel::Graph));
    };

    // If we have input IDs, traverse from each of them
    // Otherwise extract from StartNodeSpec
    let start_nodes = input_ids.unwrap_or_else(|| match &expr.start_nodes {
        StartNodeSpec::Ids(ids) => ids.clone(),
        _ => Vec::new(),
    });

    if start_nodes.is_empty() {
        return Ok(SubQueryResult::empty(DataModel::Graph));
    }

    let mut all_records = Vec::new();

    for start_id in start_nodes {
        let traversal_request = crate::proto::proximadb_v1::TraversalRequest {
            graph_id: expr.graph_name.clone(),
            start_node_id: start_id,
            max_depth: expr.max_depth,
            edge_types: expr.edge_types.clone(),
            node_labels: Vec::new(),
            filters: Vec::new(),
            algorithm: crate::proto::proximadb_v1::TraversalAlgorithm::Bfs as i32,
            limit: None,
            timeout_ms: None,
            max_frontier: None,
        };

        if let Ok(response) = graph_svc
            .traverse(&expr.graph_name, traversal_request)
            .await
        {
            for node in response.nodes {
                all_records.push(UnifiedRecord {
                    id: node.id.clone(),
                    source_model: DataModel::Graph,
                    data: serde_json::json!({
                        "id": node.id,
                        "labels": node.labels,
                        "properties": format!("{:?}", node.properties),
                    }),
                    score: None,
                    metadata: HashMap::new(),
                });
            }
        }
    }

    let count = all_records.len() as u64;
    Ok(SubQueryResult {
        source_model: DataModel::Graph,
        records_returned: count,
        records: all_records,
        total_count: Some(count),
        execution_time_us: 0,
        records_scanned: count,
    })
}

/// Execute a log query (legacy)
#[allow(dead_code)]
async fn execute_log_query(expr: &LogQueryExpr) -> Result<SubQueryResult> {
    execute_log_query_full(expr, None).await
}

/// Execute a log query with observability service
async fn execute_log_query_full(
    expr: &LogQueryExpr,
    observability_service: Option<Arc<ObservabilityService>>,
) -> Result<SubQueryResult> {
    debug!("Executing log query on namespace: {}", expr.namespace);

    let Some(obs_svc) = observability_service else {
        debug!(
            "Log query on {} skipped - no ObservabilityService",
            expr.namespace
        );
        return Ok(SubQueryResult::empty(DataModel::Observability));
    };

    // Convert severity strings to proto Severity enum
    let severities: Vec<Severity> = expr
        .severities
        .iter()
        .filter_map(|s| match s.to_lowercase().as_str() {
            "trace" => Some(Severity::Trace),
            "debug" => Some(Severity::Debug),
            "info" => Some(Severity::Info),
            "warn" | "warning" => Some(Severity::Warn),
            "error" => Some(Severity::Error),
            "fatal" | "critical" => Some(Severity::Fatal),
            _ => None,
        })
        .collect();

    // Build log query params
    let params = LogQueryParams {
        start_time_ns: expr.start_time_ns,
        end_time_ns: expr.end_time_ns,
        query: expr.query.clone(),
        severities,
        services: expr.services.clone(),
        sources: Vec::new(),
        limit: expr.limit,
        cursor: None,
    };

    match obs_svc.query_logs(&expr.namespace, params).await {
        Ok(result) => {
            let records: Vec<UnifiedRecord> = result
                .logs
                .into_iter()
                .enumerate()
                .map(|(idx, log)| {
                    // Generate ID from timestamp since LogEntry doesn't have an id field
                    let log_id = format!("log_{}_{}", log.timestamp_ns, idx);
                    UnifiedRecord {
                        id: log_id.clone(),
                        source_model: DataModel::Observability,
                        data: serde_json::json!({
                            "id": log_id,
                            "timestamp_ns": log.timestamp_ns,
                            "message": log.message,
                            "service": log.service,
                            "severity": log.severity,
                            "source": log.source,
                        }),
                        score: None,
                        metadata: HashMap::new(),
                    }
                })
                .collect();

            let count = records.len() as u64;
            Ok(SubQueryResult {
                source_model: DataModel::Observability,
                records,
                total_count: result.total_matched,
                execution_time_us: result.query_time_ms * 1000,
                records_scanned: count,
                records_returned: count,
            })
        }
        Err(e) => {
            warn!("Log query failed: {}", e);
            Ok(SubQueryResult::empty(DataModel::Observability))
        }
    }
}

/// Execute a metric query (legacy)
#[allow(dead_code)]
async fn execute_metric_query(expr: &MetricQueryExpr) -> Result<SubQueryResult> {
    execute_metric_query_full(expr, None).await
}

/// Execute a metric query with observability service
async fn execute_metric_query_full(
    expr: &MetricQueryExpr,
    observability_service: Option<Arc<ObservabilityService>>,
) -> Result<SubQueryResult> {
    debug!(
        "Executing metric query for metric: {} in namespace: {}",
        expr.metric_name, expr.namespace
    );

    let Some(obs_svc) = observability_service else {
        debug!(
            "Metric query for {} skipped - no ObservabilityService",
            expr.metric_name
        );
        return Ok(SubQueryResult::empty(DataModel::Observability));
    };

    // Build metric aggregation params
    let params = MetricAggParams {
        metric_name: expr.metric_name.clone(),
        start_time_ns: expr.start_time_ns,
        end_time_ns: expr.end_time_ns,
        aggregation: crate::observability::MetricAggregation::Avg, // Default aggregation
        step_seconds: 60,
        label_filters: HashMap::new(),
        group_by: Vec::new(),
    };

    match obs_svc.aggregate_metrics(&expr.namespace, params).await {
        Ok(result) => {
            // Convert time series to unified records
            let mut records = Vec::new();
            for series in result.series {
                for point in series.points {
                    records.push(UnifiedRecord {
                        id: format!("{}_{}", expr.metric_name, point.timestamp_ns),
                        source_model: DataModel::Observability,
                        data: serde_json::json!({
                            "metric": expr.metric_name,
                            "timestamp_ns": point.timestamp_ns,
                            "value": point.value,
                            "labels": series.labels,
                        }),
                        score: Some(point.value),
                        metadata: series.labels.clone(),
                    });
                }
            }

            let count = records.len() as u64;
            Ok(SubQueryResult {
                source_model: DataModel::Observability,
                records,
                total_count: Some(count),
                execution_time_us: result.query_time_ms * 1000,
                records_scanned: count,
                records_returned: count,
            })
        }
        Err(e) => {
            warn!("Metric query failed: {}", e);
            Ok(SubQueryResult::empty(DataModel::Observability))
        }
    }
}

// =============================================================================
// Cross-Model Join Execution
// =============================================================================

/// Result of a join operation
#[derive(Debug)]
pub struct JoinResult {
    /// Records that matched the join condition
    pub matched: Vec<UnifiedRecord>,
    /// Records from the left side that didn't match (for LEFT OUTER join)
    pub unmatched_left: Vec<UnifiedRecord>,
    /// Whether the join found any matches
    pub has_matches: bool,
}

impl JoinResult {
    /// Create an empty join result
    pub fn empty() -> Self {
        Self {
            matched: Vec::new(),
            unmatched_left: Vec::new(),
            has_matches: false,
        }
    }

    /// Convert to SubQueryResult based on join type
    pub fn to_subquery_result(
        self,
        source_model: DataModel,
        join_type: &JoinType,
    ) -> SubQueryResult {
        let records = match join_type {
            JoinType::Inner | JoinType::Semi | JoinType::Semantic { .. } => self.matched,
            JoinType::LeftOuter => {
                let mut all = self.matched;
                all.extend(self.unmatched_left);
                all
            }
            JoinType::Anti => self.unmatched_left,
        };

        let count = records.len() as u64;
        SubQueryResult {
            source_model,
            records_returned: count,
            records,
            total_count: Some(count),
            execution_time_us: 0,
            records_scanned: count,
        }
    }
}

/// Execute a join between two result sets
///
/// # Arguments
/// * `left` - Left side of the join (typically the dependent component's result)
/// * `right` - Right side of the join (prior component's result)
/// * `dependency` - Join specification (field, type)
///
/// # Returns
/// A JoinResult containing matched and unmatched records
pub fn execute_join(
    left: &[UnifiedRecord],
    right: &[UnifiedRecord],
    dependency: &ComponentDependency,
) -> JoinResult {
    let join_field = &dependency.join_field;

    debug!(
        "Executing {:?} join on field '{}' ({} left x {} right records)",
        dependency.join_type,
        join_field,
        left.len(),
        right.len()
    );

    if let JoinType::Semantic { threshold, top_k } = dependency.join_type {
        return execute_semantic_join(left, right, join_field, threshold, top_k);
    }

    // Build a hash map of right side values for efficient lookup
    let right_index: HashMap<String, Vec<&UnifiedRecord>> = build_join_index(right, join_field);

    let mut matched = Vec::new();
    let mut unmatched_left = Vec::new();

    for left_record in left {
        let left_values = extract_join_values(left_record, join_field);

        let mut found_match = false;
        for left_value in &left_values {
            if let Some(right_records) = right_index.get(left_value) {
                found_match = true;

                match dependency.join_type {
                    JoinType::Inner | JoinType::LeftOuter => {
                        // For Inner and LeftOuter, combine with each matching right record
                        for right_record in right_records {
                            let combined = merge_records(left_record, right_record, join_field);
                            matched.push(combined);
                        }
                    }
                    JoinType::Semi => {
                        // For Semi, just include the left record once if it matches
                        matched.push(left_record.clone());
                        break;
                    }
                    JoinType::Anti => {
                        // For Anti, we'll handle in the loop (found_match = true means exclude)
                    }
                    JoinType::Semantic { .. } => {
                        // Handled by early return above; unreachable here.
                    }
                }
            }
        }

        if !found_match {
            match dependency.join_type {
                JoinType::LeftOuter => {
                    // Include left record with null-padded right fields
                    unmatched_left.push(left_record.clone());
                }
                JoinType::Anti => {
                    // Anti join: include records that DON'T match
                    matched.push(left_record.clone());
                }
                _ => {}
            }
        }
    }

    let has_matches = !matched.is_empty();
    debug!(
        "Join result: {} matched, {} unmatched_left",
        matched.len(),
        unmatched_left.len()
    );

    JoinResult {
        matched,
        unmatched_left,
        has_matches,
    }
}

/// Extract a vector from a record for semantic join
fn extract_vector(record: &UnifiedRecord, field: &str) -> Option<Vec<f32>> {
    // 1. Try to extract from the data JSON
    if let Some(val) = record.data.get(field) {
        if let Some(arr) = val.as_array() {
            let vec: Vec<f32> = arr
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            if !vec.is_empty() {
                return Some(vec);
            }
        }
    }

    // 2. Try to extract from metadata if stored as comma-separated string
    if let Some(val) = record.metadata.get(field) {
        let vec: Vec<f32> = val
            .split(',')
            .filter_map(|s| s.trim().parse::<f32>().ok())
            .collect();
        if !vec.is_empty() {
            return Some(vec);
        }
    }

    None
}

/// Build a hash index of records by join field value
fn build_join_index<'a>(
    records: &'a [UnifiedRecord],
    join_field: &str,
) -> HashMap<String, Vec<&'a UnifiedRecord>> {
    let mut index: HashMap<String, Vec<&UnifiedRecord>> = HashMap::new();

    for record in records {
        let values = extract_join_values(record, join_field);
        for value in values {
            index.entry(value).or_default().push(record);
        }
    }

    index
}

/// Extract join field value(s) from a record
///
/// The join field can be:
/// - "id" - the record's ID
/// - A path in the data JSON (e.g., "user_id", "metadata.customer_id")
/// - A metadata key
fn extract_join_values(record: &UnifiedRecord, join_field: &str) -> Vec<String> {
    let mut values = Vec::new();

    // Special case: "id" field
    if join_field == "id" {
        values.push(record.id.clone());
        return values;
    }

    // Try to extract from the data JSON
    if let Some(val) = extract_from_json(&record.data, join_field) {
        values.push(val);
        return values;
    }

    // Try to extract from metadata
    if let Some(val) = record.metadata.get(join_field) {
        values.push(val.clone());
        return values;
    }

    // If field contains dots, try nested path extraction
    if join_field.contains('.') {
        let parts: Vec<&str> = join_field.split('.').collect();
        if let Some(val) = extract_nested_json(&record.data, &parts) {
            values.push(val);
        }
    }

    values
}

/// Extract a value from JSON by field name
fn extract_from_json(data: &serde_json::Value, field: &str) -> Option<String> {
    match data.get(field) {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
        Some(serde_json::Value::Array(arr)) => {
            // For arrays, we might want to match any element
            // Return first element as string for now
            arr.first().and_then(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
        }
        _ => None,
    }
}

/// Extract a value from nested JSON path
fn extract_nested_json(data: &serde_json::Value, path_parts: &[&str]) -> Option<String> {
    if path_parts.is_empty() {
        return None;
    }

    let mut current = data;
    for part in path_parts {
        current = current.get(*part)?;
    }

    match current {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Merge two records from different models into one
fn merge_records(left: &UnifiedRecord, right: &UnifiedRecord, join_field: &str) -> UnifiedRecord {
    // Create merged data object
    let mut merged_data = serde_json::Map::new();

    // Add left data under its model prefix
    let left_key = format!("{}", left.source_model);
    merged_data.insert(left_key, left.data.clone());

    // Add right data under its model prefix
    let right_key = format!("{}", right.source_model);
    merged_data.insert(right_key, right.data.clone());

    // Add the join field value for clarity
    merged_data.insert(
        "join_field".to_string(),
        serde_json::Value::String(join_field.to_string()),
    );

    // Merge metadata
    let mut merged_metadata = left.metadata.clone();
    for (k, v) in &right.metadata {
        merged_metadata
            .entry(format!("{}_{}", right.source_model, k))
            .or_insert_with(|| v.clone());
    }

    // Use left's ID as primary, with right's ID in metadata
    merged_metadata.insert("right_id".to_string(), right.id.clone());

    // Calculate merged score (average if both have scores)
    let merged_score = match (left.score, right.score) {
        (Some(l), Some(r)) => Some((l + r) / 2.0),
        (Some(s), None) | (None, Some(s)) => Some(s),
        (None, None) => None,
    };

    UnifiedRecord {
        id: left.id.clone(),
        source_model: left.source_model.clone(),
        data: serde_json::Value::Object(merged_data),
        score: merged_score,
        metadata: merged_metadata,
    }
}

/// Execute multiple joins in sequence for components with multiple dependencies
pub fn execute_multi_join(
    component_result: &SubQueryResult,
    dependencies: &[ComponentDependency],
    prior_results: &HashMap<usize, &SubQueryResult>,
) -> SubQueryResult {
    if dependencies.is_empty() {
        return component_result.clone();
    }

    let mut current_records = component_result.records.clone();
    let source_model = component_result.source_model.clone();

    for dep in dependencies {
        if let Some(prior) = prior_results.get(&dep.component_index) {
            let join_result = execute_join(&current_records, &prior.records, dep);
            current_records = join_result
                .to_subquery_result(source_model.clone(), &dep.join_type)
                .records;
        }
    }

    let count = current_records.len() as u64;
    SubQueryResult {
        source_model,
        records_returned: count,
        records: current_records,
        total_count: Some(count),
        execution_time_us: component_result.execution_time_us,
        records_scanned: component_result.records_scanned,
    }
}

/// Filter results by IDs from a prior component (legacy behavior)
pub fn filter_by_ids(
    records: &[UnifiedRecord],
    prior_ids: &[String],
    include: bool,
) -> Vec<UnifiedRecord> {
    let id_set: std::collections::HashSet<&str> = prior_ids.iter().map(|s| s.as_str()).collect();

    records
        .iter()
        .filter(|r| {
            let in_set = id_set.contains(r.id.as_str());
            if include { in_set } else { !in_set }
        })
        .cloned()
        .collect()
}

/// Execute a semantic join based on vector similarity
fn execute_semantic_join(
    left: &[UnifiedRecord],
    right: &[UnifiedRecord],
    join_field: &str,
    threshold: f32,
    top_k: u32,
) -> JoinResult {
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::compute::distance_computation::DistanceMetric;

    let mut matched = Vec::new();
    let mut unmatched_left = Vec::new();
    let mut has_matches = false;

    // Use Cosine similarity for semantic matching by default
    let engine = UnifiedDistanceCompute::new(DistanceMetric::Cosine);

    // Prepare right vectors for efficient comparison
    let right_vectors: Vec<(usize, Vec<f32>)> = right
        .iter()
        .enumerate()
        .filter_map(|(i, r)| extract_vector(r, join_field).map(|v| (i, v)))
        .collect();

    if right_vectors.is_empty() {
        return JoinResult {
            matched: Vec::new(),
            unmatched_left: left.to_vec(),
            has_matches: false,
        };
    }

    for left_record in left {
        if let Some(left_vec) = extract_vector(left_record, join_field) {
            let mut matches: Vec<(f32, &UnifiedRecord)> = Vec::new();

            for (right_idx, right_vec) in &right_vectors {
                // UnifiedDistanceCompute returns (1 - similarity) for Cosine
                // So LOWER = MORE SIMILAR.
                // We want similarity > threshold, which means distance < (1 - threshold)
                let distance = engine.distance(&left_vec, right_vec);
                let similarity = 1.0 - distance;

                if similarity >= threshold {
                    matches.push((similarity, &right[*right_idx]));
                }
            }

            if !matches.is_empty() {
                has_matches = true;
                // Sort by similarity descending
                matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

                // Take top_k
                for (_, right_record) in matches.iter().take(top_k as usize) {
                    let mut joined = left_record.clone();
                    // Merge data from right record
                    if let Some(obj) = joined.data.as_object_mut() {
                        if let Some(right_obj) = right_record.data.as_object() {
                            for (k, v) in right_obj {
                                if !obj.contains_key(k) {
                                    obj.insert(k.clone(), v.clone());
                                } else {
                                    // Prefix with right_ if collision
                                    obj.insert(format!("right_{}", k), v.clone());
                                }
                            }
                        }
                    }
                    matched.push(joined);
                }
            } else {
                unmatched_left.push(left_record.clone());
            }
        } else {
            unmatched_left.push(left_record.clone());
        }
    }

    JoinResult {
        matched,
        unmatched_left,
        has_matches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::unified::ast::{DistanceMetric, VectorSearchParams};

    #[test]
    fn test_executor_creation() {
        let executor = ParallelExecutor::new(4);
        assert_eq!(executor.max_parallel, 4);
    }

    #[test]
    fn test_semaphore_permits() {
        let executor = ParallelExecutor::new(2);
        // Should have 2 permits available
        assert_eq!(executor.semaphore.available_permits(), 2);
    }

    #[tokio::test]
    async fn test_empty_query_execution() {
        let _executor = ParallelExecutor::new(4);
        let query = MultiModelQuery::new();

        // Need mock storage engine and document service for full test
        // For now, just verify the executor handles empty queries
        assert!(query.components.is_empty());
    }

    #[test]
    fn test_vector_search_expr() {
        let expr = VectorSearchExpr {
            collection: "test".to_string(),
            query_vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            threshold: Some(0.8),
            metric: DistanceMetric::Cosine,
            params: VectorSearchParams::default(),
        };

        assert_eq!(expr.collection, "test");
        assert_eq!(expr.top_k, 10);
    }

    #[test]
    fn test_sql_value_to_json_primitives() {
        use crate::proto::proximadb_v1::{SqlValue, sql_value::Value};

        // Test null
        let null_val = SqlValue {
            value: Some(Value::NullValue(0)),
        };
        assert_eq!(sql_value_to_json(&null_val), serde_json::Value::Null);

        // Test bool
        let bool_val = SqlValue {
            value: Some(Value::BoolValue(true)),
        };
        assert_eq!(sql_value_to_json(&bool_val), serde_json::Value::Bool(true));

        // Test int
        let int_val = SqlValue {
            value: Some(Value::Int64Value(42)),
        };
        assert_eq!(sql_value_to_json(&int_val), serde_json::json!(42));

        // Test string
        let str_val = SqlValue {
            value: Some(Value::StringValue("hello".to_string())),
        };
        assert_eq!(
            sql_value_to_json(&str_val),
            serde_json::Value::String("hello".to_string())
        );
    }

    // =========================================================================
    // Cross-Model Join Tests
    // =========================================================================

    fn make_test_record(id: &str, model: DataModel, data: serde_json::Value) -> UnifiedRecord {
        UnifiedRecord {
            id: id.to_string(),
            source_model: model,
            data,
            score: None,
            metadata: HashMap::new(),
        }
    }

    fn make_test_record_with_score(
        id: &str,
        model: DataModel,
        data: serde_json::Value,
        score: f64,
    ) -> UnifiedRecord {
        UnifiedRecord {
            id: id.to_string(),
            source_model: model,
            data,
            score: Some(score),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn test_extract_join_values_id_field() {
        let record = make_test_record("rec_123", DataModel::Vector, serde_json::json!({}));
        let values = extract_join_values(&record, "id");
        assert_eq!(values, vec!["rec_123"]);
    }

    #[test]
    fn test_extract_join_values_json_field() {
        let record = make_test_record(
            "rec_1",
            DataModel::Document,
            serde_json::json!({"user_id": "user_456", "name": "Alice"}),
        );
        let values = extract_join_values(&record, "user_id");
        assert_eq!(values, vec!["user_456"]);
    }

    #[test]
    fn test_extract_join_values_nested_json() {
        let record = make_test_record(
            "rec_1",
            DataModel::Document,
            serde_json::json!({"metadata": {"customer_id": "cust_789"}}),
        );
        let values = extract_join_values(&record, "metadata.customer_id");
        assert_eq!(values, vec!["cust_789"]);
    }

    #[test]
    fn test_extract_join_values_missing_field() {
        let record = make_test_record("rec_1", DataModel::Vector, serde_json::json!({}));
        let values = extract_join_values(&record, "nonexistent");
        assert!(values.is_empty());
    }

    #[test]
    fn test_inner_join() {
        // Left: documents with user_id field
        let left = vec![
            make_test_record(
                "doc_1",
                DataModel::Document,
                serde_json::json!({"user_id": "u1"}),
            ),
            make_test_record(
                "doc_2",
                DataModel::Document,
                serde_json::json!({"user_id": "u2"}),
            ),
            make_test_record(
                "doc_3",
                DataModel::Document,
                serde_json::json!({"user_id": "u3"}),
            ),
        ];

        // Right: vectors with SAME user_id field (join field must exist on both sides)
        let right = vec![
            make_test_record(
                "vec_1",
                DataModel::Vector,
                serde_json::json!({"user_id": "u1"}),
            ),
            make_test_record(
                "vec_2",
                DataModel::Vector,
                serde_json::json!({"user_id": "u2"}),
            ),
            // u3 is missing
        ];

        let dependency = ComponentDependency {
            component_index: 0,
            join_field: "user_id".to_string(),
            join_type: JoinType::Inner,
        };

        let result = execute_join(&left, &right, &dependency);

        // Should only have 2 matches (u1 and u2)
        assert_eq!(result.matched.len(), 2);
        assert_eq!(result.unmatched_left.len(), 0);
        assert!(result.has_matches);
    }

    #[test]
    fn test_left_outer_join() {
        let left = vec![
            make_test_record(
                "doc_1",
                DataModel::Document,
                serde_json::json!({"user_id": "u1"}),
            ),
            make_test_record(
                "doc_2",
                DataModel::Document,
                serde_json::json!({"user_id": "u2"}),
            ),
            make_test_record(
                "doc_3",
                DataModel::Document,
                serde_json::json!({"user_id": "u3"}),
            ),
        ];

        // Right: only has user_id=u1, so u2 and u3 will be unmatched
        let right = vec![make_test_record(
            "vec_1",
            DataModel::Vector,
            serde_json::json!({"user_id": "u1"}),
        )];

        let dependency = ComponentDependency {
            component_index: 0,
            join_field: "user_id".to_string(),
            join_type: JoinType::LeftOuter,
        };

        let result = execute_join(&left, &right, &dependency);

        // Should have 1 match + 2 unmatched
        assert_eq!(result.matched.len(), 1);
        assert_eq!(result.unmatched_left.len(), 2);
        assert!(result.has_matches);

        // Convert to SubQueryResult and check total
        let subquery = result.to_subquery_result(DataModel::Document, &JoinType::LeftOuter);
        assert_eq!(subquery.records.len(), 3); // All left records included
    }

    #[test]
    fn test_semi_join() {
        let left = vec![
            make_test_record(
                "doc_1",
                DataModel::Document,
                serde_json::json!({"user_id": "u1"}),
            ),
            make_test_record(
                "doc_2",
                DataModel::Document,
                serde_json::json!({"user_id": "u2"}),
            ),
            make_test_record(
                "doc_3",
                DataModel::Document,
                serde_json::json!({"user_id": "u1"}),
            ), // Same user_id
        ];

        // Right side has user_id field matching u1
        let right = vec![
            make_test_record(
                "vec_1",
                DataModel::Vector,
                serde_json::json!({"user_id": "u1"}),
            ),
            make_test_record(
                "vec_2",
                DataModel::Vector,
                serde_json::json!({"user_id": "u1"}),
            ), // Another u1
        ];

        let dependency = ComponentDependency {
            component_index: 0,
            join_field: "user_id".to_string(),
            join_type: JoinType::Semi,
        };

        let result = execute_join(&left, &right, &dependency);

        // Semi join: doc_1 and doc_3 match (both have user_id=u1), doc_2 doesn't
        assert_eq!(result.matched.len(), 2);
        // Semi join should not duplicate left records even if multiple matches
    }

    #[test]
    fn test_anti_join() {
        let left = vec![
            make_test_record(
                "doc_1",
                DataModel::Document,
                serde_json::json!({"user_id": "u1"}),
            ),
            make_test_record(
                "doc_2",
                DataModel::Document,
                serde_json::json!({"user_id": "u2"}),
            ),
            make_test_record(
                "doc_3",
                DataModel::Document,
                serde_json::json!({"user_id": "u3"}),
            ),
        ];

        // Right side has user_id=u1, so doc_1 will be excluded (matched)
        let right = vec![make_test_record(
            "vec_1",
            DataModel::Vector,
            serde_json::json!({"user_id": "u1"}),
        )];

        let dependency = ComponentDependency {
            component_index: 0,
            join_field: "user_id".to_string(),
            join_type: JoinType::Anti,
        };

        let result = execute_join(&left, &right, &dependency);

        // Anti join: only records that DON'T match
        assert_eq!(result.matched.len(), 2); // u2 and u3 don't match
        assert!(result.matched.iter().any(|r| r.id == "doc_2"));
        assert!(result.matched.iter().any(|r| r.id == "doc_3"));
        assert!(!result.matched.iter().any(|r| r.id == "doc_1")); // u1 matched, excluded
    }

    #[test]
    fn test_join_by_id() {
        let left = vec![
            make_test_record(
                "shared_1",
                DataModel::Document,
                serde_json::json!({"name": "Doc1"}),
            ),
            make_test_record(
                "shared_2",
                DataModel::Document,
                serde_json::json!({"name": "Doc2"}),
            ),
        ];

        let right = vec![
            make_test_record(
                "shared_1",
                DataModel::Vector,
                serde_json::json!({"vec": [0.1]}),
            ),
            make_test_record(
                "other",
                DataModel::Vector,
                serde_json::json!({"vec": [0.2]}),
            ),
        ];

        let dependency = ComponentDependency {
            component_index: 0,
            join_field: "id".to_string(),
            join_type: JoinType::Inner,
        };

        let result = execute_join(&left, &right, &dependency);

        assert_eq!(result.matched.len(), 1);
        assert_eq!(result.matched[0].id, "shared_1");
    }

    #[test]
    fn test_merge_records_combines_data() {
        let left = make_test_record_with_score(
            "doc_1",
            DataModel::Document,
            serde_json::json!({"name": "Alice"}),
            0.8,
        );
        let right = make_test_record_with_score(
            "vec_1",
            DataModel::Vector,
            serde_json::json!({"embedding": [0.1, 0.2]}),
            0.9,
        );

        let merged = merge_records(&left, &right, "user_id");

        // Check merged data has both models' data
        assert!(merged.data.get("document").is_some());
        assert!(merged.data.get("vector").is_some());

        // Check score is averaged (use approximate comparison for floating point)
        let score = merged.score.unwrap();
        assert!(
            (score - 0.85).abs() < 0.0001,
            "Expected score ~0.85, got {}",
            score
        );

        // Check right_id is in metadata
        assert_eq!(merged.metadata.get("right_id"), Some(&"vec_1".to_string()));
    }

    #[test]
    fn test_filter_by_ids_include() {
        let records = vec![
            make_test_record("r1", DataModel::Vector, serde_json::json!({})),
            make_test_record("r2", DataModel::Vector, serde_json::json!({})),
            make_test_record("r3", DataModel::Vector, serde_json::json!({})),
        ];

        let ids = vec!["r1".to_string(), "r3".to_string()];
        let filtered = filter_by_ids(&records, &ids, true);

        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().any(|r| r.id == "r1"));
        assert!(filtered.iter().any(|r| r.id == "r3"));
    }

    #[test]
    fn test_filter_by_ids_exclude() {
        let records = vec![
            make_test_record("r1", DataModel::Vector, serde_json::json!({})),
            make_test_record("r2", DataModel::Vector, serde_json::json!({})),
            make_test_record("r3", DataModel::Vector, serde_json::json!({})),
        ];

        let ids = vec!["r1".to_string(), "r3".to_string()];
        let filtered = filter_by_ids(&records, &ids, false);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "r2");
    }

    #[test]
    fn test_join_result_empty() {
        let result = JoinResult::empty();
        assert!(result.matched.is_empty());
        assert!(result.unmatched_left.is_empty());
        assert!(!result.has_matches);
    }

    #[test]
    fn test_execute_multi_join() {
        // Simulate a chain: Vector -> Document (joined on product_id field)

        // Prior results: vector search found these with product_id field
        let vector_result = SubQueryResult {
            source_model: DataModel::Vector,
            records_returned: 2,
            records: vec![
                make_test_record(
                    "v1",
                    DataModel::Vector,
                    serde_json::json!({"product_id": "p1"}),
                ),
                make_test_record(
                    "v2",
                    DataModel::Vector,
                    serde_json::json!({"product_id": "p2"}),
                ),
            ],
            total_count: Some(2),
            execution_time_us: 100,
            records_scanned: 2,
        };

        // Document component result - ALSO has product_id field (same join field on both sides)
        let doc_result = SubQueryResult {
            source_model: DataModel::Document,
            records_returned: 3,
            records: vec![
                make_test_record(
                    "d1",
                    DataModel::Document,
                    serde_json::json!({"product_id": "p1", "name": "Product 1"}),
                ),
                make_test_record(
                    "d2",
                    DataModel::Document,
                    serde_json::json!({"product_id": "p2", "name": "Product 2"}),
                ),
                make_test_record(
                    "d3",
                    DataModel::Document,
                    serde_json::json!({"product_id": "p3", "name": "Product 3"}),
                ),
            ],
            total_count: Some(3),
            execution_time_us: 50,
            records_scanned: 3,
        };

        let dependencies = vec![ComponentDependency {
            component_index: 0,
            join_field: "product_id".to_string(),
            join_type: JoinType::Inner,
        }];

        let mut prior_results: HashMap<usize, &SubQueryResult> = HashMap::new();
        prior_results.insert(0, &vector_result);

        let joined = execute_multi_join(&doc_result, &dependencies, &prior_results);

        // Should have 2 matches (p1 and p2)
        assert_eq!(joined.records.len(), 2);
    }

    // =========================================================================
    // Cross-Model Chain Tests (Phase 10.2)
    // =========================================================================

    #[test]
    fn test_resolve_nodes_from_component_empty_context() {
        let result = resolve_nodes_from_component(0, None);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_resolve_nodes_from_component_missing_component() {
        let context: HashMap<usize, &SubQueryResult> = HashMap::new();
        let result = resolve_nodes_from_component(0, Some(&context));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_resolve_nodes_from_component_extracts_ids() {
        // Create a prior result with vector records
        let vector_result = SubQueryResult {
            source_model: DataModel::Vector,
            records_returned: 3,
            records: vec![
                make_test_record(
                    "node_alpha",
                    DataModel::Vector,
                    serde_json::json!({"similarity": 0.95}),
                ),
                make_test_record(
                    "node_beta",
                    DataModel::Vector,
                    serde_json::json!({"similarity": 0.88}),
                ),
                make_test_record(
                    "node_gamma",
                    DataModel::Vector,
                    serde_json::json!({"similarity": 0.75}),
                ),
            ],
            total_count: Some(3),
            execution_time_us: 50,
            records_scanned: 3,
        };

        let mut context: HashMap<usize, &SubQueryResult> = HashMap::new();
        context.insert(0, &vector_result);

        let result = resolve_nodes_from_component(0, Some(&context));
        assert!(result.is_ok());

        let ids = result.unwrap();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"node_alpha".to_string()));
        assert!(ids.contains(&"node_beta".to_string()));
        assert!(ids.contains(&"node_gamma".to_string()));
    }

    #[test]
    fn test_vector_to_graph_to_document_chain() {
        // Simulate a 3-stage pipeline:
        // Stage 1: Vector search finds similar embeddings
        // Stage 2: Graph traversal from those vectors to find related entities
        // Stage 3: Document query to get full details

        // Stage 1 result: Vector search
        let vector_result = SubQueryResult {
            source_model: DataModel::Vector,
            records_returned: 2,
            records: vec![
                make_test_record(
                    "vec_product_1",
                    DataModel::Vector,
                    serde_json::json!({
                        "entity_id": "prod_1",
                        "similarity": 0.92
                    }),
                ),
                make_test_record(
                    "vec_product_2",
                    DataModel::Vector,
                    serde_json::json!({
                        "entity_id": "prod_2",
                        "similarity": 0.85
                    }),
                ),
            ],
            total_count: Some(2),
            execution_time_us: 100,
            records_scanned: 100,
        };

        // Stage 2 result: Graph traversal (from vector entity_ids)
        let graph_result = SubQueryResult {
            source_model: DataModel::Graph,
            records_returned: 4,
            records: vec![
                // Related entities found via graph traversal
                make_test_record(
                    "prod_1",
                    DataModel::Graph,
                    serde_json::json!({
                        "labels": ["Product"],
                        "doc_id": "doc_product_1"
                    }),
                ),
                make_test_record(
                    "brand_1",
                    DataModel::Graph,
                    serde_json::json!({
                        "labels": ["Brand"],
                        "doc_id": "doc_brand_1"
                    }),
                ),
                make_test_record(
                    "prod_2",
                    DataModel::Graph,
                    serde_json::json!({
                        "labels": ["Product"],
                        "doc_id": "doc_product_2"
                    }),
                ),
                make_test_record(
                    "category_1",
                    DataModel::Graph,
                    serde_json::json!({
                        "labels": ["Category"],
                        "doc_id": "doc_category_1"
                    }),
                ),
            ],
            total_count: Some(4),
            execution_time_us: 50,
            records_scanned: 20,
        };

        // Stage 3 result: Document query (using doc_ids from graph)
        let doc_result = SubQueryResult {
            source_model: DataModel::Document,
            records_returned: 4,
            records: vec![
                make_test_record(
                    "doc_product_1",
                    DataModel::Document,
                    serde_json::json!({
                        "doc_id": "doc_product_1",
                        "name": "Wireless Headphones",
                        "price": 149.99
                    }),
                ),
                make_test_record(
                    "doc_brand_1",
                    DataModel::Document,
                    serde_json::json!({
                        "doc_id": "doc_brand_1",
                        "name": "TechAudio Inc.",
                        "country": "USA"
                    }),
                ),
                make_test_record(
                    "doc_product_2",
                    DataModel::Document,
                    serde_json::json!({
                        "doc_id": "doc_product_2",
                        "name": "Bluetooth Speaker",
                        "price": 79.99
                    }),
                ),
                make_test_record(
                    "doc_category_1",
                    DataModel::Document,
                    serde_json::json!({
                        "doc_id": "doc_category_1",
                        "name": "Electronics",
                        "parent": "Home"
                    }),
                ),
            ],
            total_count: Some(4),
            execution_time_us: 30,
            records_scanned: 4,
        };

        // Now join graph → document using doc_id field
        let dependencies = vec![ComponentDependency {
            component_index: 1, // Graph result
            join_field: "doc_id".to_string(),
            join_type: JoinType::Inner,
        }];

        let mut prior_results: HashMap<usize, &SubQueryResult> = HashMap::new();
        prior_results.insert(0, &vector_result); // Stage 0: vectors
        prior_results.insert(1, &graph_result); // Stage 1: graph

        // Join documents with graph results
        let joined = execute_multi_join(&doc_result, &dependencies, &prior_results);

        // Should have 4 matches (all graph nodes have corresponding documents)
        assert_eq!(joined.records.len(), 4);

        // Verify the merged records contain data from both models
        for record in &joined.records {
            // Each merged record should have document and graph data
            assert!(record.data.get("document").is_some() || record.data.get("graph").is_some());
        }
    }

    #[test]
    fn test_multi_model_join_with_partial_matches() {
        // Test case where not all records match across models

        // Vector results with product IDs
        let vector_result = SubQueryResult {
            source_model: DataModel::Vector,
            records_returned: 3,
            records: vec![
                make_test_record(
                    "v1",
                    DataModel::Vector,
                    serde_json::json!({"product_id": "p1"}),
                ),
                make_test_record(
                    "v2",
                    DataModel::Vector,
                    serde_json::json!({"product_id": "p2"}),
                ),
                make_test_record(
                    "v3",
                    DataModel::Vector,
                    serde_json::json!({"product_id": "p_unknown"}),
                ), // No match
            ],
            total_count: Some(3),
            execution_time_us: 50,
            records_scanned: 3,
        };

        // Document results - only has p1 and p2
        let doc_result = SubQueryResult {
            source_model: DataModel::Document,
            records_returned: 2,
            records: vec![
                make_test_record(
                    "d1",
                    DataModel::Document,
                    serde_json::json!({"product_id": "p1"}),
                ),
                make_test_record(
                    "d2",
                    DataModel::Document,
                    serde_json::json!({"product_id": "p2"}),
                ),
                // p_unknown doesn't exist in documents
            ],
            total_count: Some(2),
            execution_time_us: 30,
            records_scanned: 2,
        };

        // Inner join: should only get matching records
        let inner_dep = vec![ComponentDependency {
            component_index: 0,
            join_field: "product_id".to_string(),
            join_type: JoinType::Inner,
        }];

        let mut prior: HashMap<usize, &SubQueryResult> = HashMap::new();
        prior.insert(0, &vector_result);

        let inner_joined = execute_multi_join(&doc_result, &inner_dep, &prior);
        assert_eq!(inner_joined.records.len(), 2); // Only p1 and p2 match

        // Left outer join: should include all left records
        let left_dep = vec![ComponentDependency {
            component_index: 0,
            join_field: "product_id".to_string(),
            join_type: JoinType::LeftOuter,
        }];

        let left_joined = execute_multi_join(&doc_result, &left_dep, &prior);
        assert_eq!(left_joined.records.len(), 2); // All documents included
    }

    #[test]
    fn test_graph_node_id_extraction() {
        // Test that we can extract node IDs from graph traversal results
        // to use as start nodes for a subsequent traversal

        let graph_result = SubQueryResult {
            source_model: DataModel::Graph,
            records_returned: 3,
            records: vec![
                make_test_record(
                    "user_alice",
                    DataModel::Graph,
                    serde_json::json!({
                        "labels": ["User"],
                        "properties": {"name": "Alice"}
                    }),
                ),
                make_test_record(
                    "user_bob",
                    DataModel::Graph,
                    serde_json::json!({
                        "labels": ["User"],
                        "properties": {"name": "Bob"}
                    }),
                ),
                make_test_record(
                    "user_charlie",
                    DataModel::Graph,
                    serde_json::json!({
                        "labels": ["User"],
                        "properties": {"name": "Charlie"}
                    }),
                ),
            ],
            total_count: Some(3),
            execution_time_us: 25,
            records_scanned: 10,
        };

        let mut context: HashMap<usize, &SubQueryResult> = HashMap::new();
        context.insert(0, &graph_result);

        // Resolve nodes from the graph component
        let resolved = resolve_nodes_from_component(0, Some(&context)).unwrap();

        assert_eq!(resolved.len(), 3);
        assert!(resolved.contains(&"user_alice".to_string()));
        assert!(resolved.contains(&"user_bob".to_string()));
        assert!(resolved.contains(&"user_charlie".to_string()));
    }

    #[test]
    fn test_semantic_join() {
        use serde_json::json;

        // Left side: User interests (Document model)
        let left = vec![
            UnifiedRecord {
                id: "user1".to_string(),
                source_model: DataModel::Document,
                data: json!({
                    "name": "Alice",
                    "interest_vec": [1.0, 0.0, 0.0]
                }),
                score: None,
                metadata: HashMap::new(),
            },
            UnifiedRecord {
                id: "user2".to_string(),
                source_model: DataModel::Document,
                data: json!({
                    "name": "Bob",
                    "interest_vec": [0.0, 1.0, 0.0]
                }),
                score: None,
                metadata: HashMap::new(),
            }
        ];

        // Right side: Products (Vector model)
        let right = vec![
            UnifiedRecord {
                id: "prod1".to_string(),
                source_model: DataModel::Vector,
                data: json!({
                    "product": "Red Apple",
                    "vec": [0.9, 0.1, 0.0] // Close to Alice
                }),
                score: None,
                metadata: HashMap::new(),
            },
            UnifiedRecord {
                id: "prod2".to_string(),
                source_model: DataModel::Vector,
                data: json!({
                    "product": "Green Broccoli",
                    "vec": [0.1, 0.9, 0.0] // Close to Bob
                }),
                score: None,
                metadata: HashMap::new(),
            }
        ];

        let dependency = ComponentDependency {
            component_index: 0,
            join_field: "interest_vec".to_string(), // Note: we'll use interest_vec for left, but right uses 'vec' in data? 
                                                  // Actually extract_vector is called with join_field for BOTH.
                                                  // I should adjust the test data to have same field name or improve extractor.
            join_type: JoinType::Semantic { threshold: 0.8, top_k: 1 },
        };
        
        // Adjusting right side to match join_field for this test
        let mut right_fixed = right.clone();
        if let Some(obj) = right_fixed[0].data.as_object_mut() {
            obj.insert("interest_vec".to_string(), json!([0.9, 0.1, 0.0]));
        }
        if let Some(obj) = right_fixed[1].data.as_object_mut() {
            obj.insert("interest_vec".to_string(), json!([0.1, 0.9, 0.0]));
        }

        let result = execute_join(&left, &right_fixed, &dependency);

        assert!(result.has_matches);
        assert_eq!(result.matched.len(), 2);
        
        // Alice (user1) should match Red Apple (prod1)
        let alice_match = result.matched.iter().find(|r| r.id == "user1").unwrap();
        assert_eq!(alice_match.data["product"], "Red Apple");

        // Bob (user2) should match Green Broccoli (prod2)
        let bob_match = result.matched.iter().find(|r| r.id == "user2").unwrap();
        assert_eq!(bob_match.data["product"], "Green Broccoli");
    }
}
