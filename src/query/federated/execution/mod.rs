//! # Federated Query Executor
//!
//! Executes federated query plans across multiple data models.
//!
//! ## Features
//!
//! - **Arrow-based vectorized execution**: Efficient batch processing
//! - **Cross-model join operators**: Hash join, nested loop, index join
//! - **Streaming results**: Memory-efficient result streaming
//! - **Parallel execution**: Execute independent branches concurrently

use anyhow::{Result, anyhow};
use arrow::array::{
    Array, ArrayRef, Float32Array, Int64Array, RecordBatch, StringArray, UInt32Builder,
    new_null_array,
};
use arrow::compute::{concat, take};
use arrow::datatypes::{DataType, Field, Schema};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::optimizer::{
    JoinType, ObservabilityQueryType, PlanNode, PlanNodeType, QueryPlan, VectorSource,
};
use crate::core::search::SearchParams;
use crate::proto::proximadb_v1::{
    Collection, DocFilterCondition, DocFilterOperator, DocumentFilter, SqlValue, sql_value,
};
use crate::storage::multimodel::{ModelType, MultiModelStorageFacade};
use crate::storage::traits::{
    DocumentStorageOperations, MetricAggregationParams, ObservabilityStorageOperations,
    StorageQueryContext,
};

/// Execution result containing Arrow record batches
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Result batches
    pub batches: Vec<RecordBatch>,
    /// Result schema
    pub schema: Arc<Schema>,
    /// Execution statistics
    pub stats: ExecutionStats,
}

impl ExecutionResult {
    /// Create an empty result
    pub fn empty() -> Self {
        let schema = Arc::new(Schema::empty());
        Self {
            batches: vec![],
            schema,
            stats: ExecutionStats::default(),
        }
    }

    /// Create a result with a single batch
    pub fn from_batch(batch: RecordBatch) -> Self {
        let schema = batch.schema();
        let rows = batch.num_rows();
        Self {
            batches: vec![batch],
            schema,
            stats: ExecutionStats {
                rows_produced: rows,
                ..Default::default()
            },
        }
    }

    /// Create an empty result with a known schema
    pub fn empty_with_schema(schema: Arc<Schema>) -> Self {
        Self {
            batches: vec![],
            schema,
            stats: ExecutionStats::default(),
        }
    }

    /// Get total row count
    pub fn row_count(&self) -> usize {
        self.batches.iter().map(|b| b.num_rows()).sum()
    }
}

/// Execution statistics
#[derive(Debug, Default, Clone)]
pub struct ExecutionStats {
    /// Total rows produced
    pub rows_produced: usize,
    /// Execution time in microseconds
    pub execution_time_us: u64,
    /// Bytes scanned
    pub bytes_scanned: u64,
    /// Models queried
    pub models_queried: Vec<ModelType>,
    /// Cache hits
    pub cache_hits: u64,
    /// Cache misses
    pub cache_misses: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum JoinKey {
    Utf8(String),
    Int64(i64),
    Int32(i32),
    UInt64(u64),
    UInt32(u32),
    Boolean(bool),
}

/// Federated query executor
pub struct FederatedExecutor {
    /// Multi-model storage facade
    storage: Arc<MultiModelStorageFacade>,
    /// Execution configuration
    config: ExecutionConfig,
}

/// Execution configuration
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    /// Maximum batch size
    pub batch_size: usize,
    /// Enable parallel execution
    pub parallel_execution: bool,
    /// Maximum parallel tasks
    pub max_parallel_tasks: usize,
    /// Spill threshold in bytes
    pub spill_threshold_bytes: usize,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            batch_size: 10_000,
            parallel_execution: true,
            max_parallel_tasks: 4,
            spill_threshold_bytes: 100 * 1024 * 1024, // 100MB
        }
    }
}

impl FederatedExecutor {
    /// Create a new federated executor
    pub fn new(storage: Arc<MultiModelStorageFacade>) -> Self {
        Self {
            storage,
            config: ExecutionConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(storage: Arc<MultiModelStorageFacade>, config: ExecutionConfig) -> Self {
        Self { storage, config }
    }

    /// Execute a query plan
    pub async fn execute(&self, plan: QueryPlan) -> Result<ExecutionResult> {
        let start = std::time::Instant::now();

        let result = self.execute_node(&plan.root).await?;

        let mut stats = result.stats.clone();
        stats.execution_time_us = start.elapsed().as_micros() as u64;
        stats.models_queried = plan.metadata.involved_models;

        Ok(ExecutionResult {
            batches: result.batches,
            schema: result.schema,
            stats,
        })
    }

    /// Execute a single plan node
    fn execute_node<'a>(
        &'a self,
        node: &'a PlanNode,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ExecutionResult>> + Send + 'a>>
    {
        Box::pin(async move {
            match &node.node_type {
                PlanNodeType::Scan {
                    target,
                    model_type,
                    predicates,
                } => self.execute_scan(target, model_type, predicates).await,
                PlanNodeType::VectorSearch {
                    collection,
                    top_k,
                    query_vector_source,
                } => {
                    self.execute_vector_search(collection, query_vector_source, *top_k)
                        .await
                }
                PlanNodeType::GraphTraversal {
                    cypher,
                    start_nodes,
                } => {
                    self.execute_graph_traversal(cypher, start_nodes.as_ref())
                        .await
                }
                PlanNodeType::DocumentQuery { collection, filter } => {
                    self.execute_document_query(collection, filter.as_ref())
                        .await
                }
                PlanNodeType::ObservabilityQuery {
                    namespace,
                    query_type,
                    time_range: _,
                } => {
                    self.execute_observability_query(namespace, query_type)
                        .await
                }
                PlanNodeType::HashJoin {
                    left,
                    right,
                    join_keys,
                    join_type,
                } => {
                    self.execute_hash_join(left, right, join_keys, join_type)
                        .await
                }
                PlanNodeType::NestedLoopJoin {
                    outer,
                    inner,
                    correlation,
                } => {
                    self.execute_nested_loop_join(outer, inner, correlation)
                        .await
                }
                PlanNodeType::IndexJoin {
                    left,
                    right,
                    index_lookup,
                } => self.execute_index_join(left, right, index_lookup).await,
                PlanNodeType::Filter { input, predicate } => {
                    self.execute_filter(input, predicate).await
                }
                PlanNodeType::Project { input, columns } => {
                    self.execute_project(input, columns).await
                }
                PlanNodeType::Sort { input, order_by } => self.execute_sort(input, order_by).await,
                PlanNodeType::Limit {
                    input,
                    limit,
                    offset,
                } => self.execute_limit(input, *limit, *offset).await,
                PlanNodeType::Aggregate {
                    input,
                    group_by,
                    aggregates,
                } => self.execute_aggregate(input, group_by, aggregates).await,
                PlanNodeType::Union { inputs, all } => self.execute_union(inputs, *all).await,
            }
        })
    }

    /// Execute a table/collection scan
    async fn execute_scan(
        &self,
        target: &str,
        model_type: &ModelType,
        _predicates: &[super::optimizer::Predicate],
    ) -> Result<ExecutionResult> {
        Err(anyhow!(
            "Scan execution is not configured for target '{}' ({:?}); live federated execution currently supports function-backed vector/graph/document/observability sources, not generic relational scans",
            target,
            model_type
        ))
    }

    /// Execute vector similarity search
    async fn execute_vector_search(
        &self,
        collection: &str,
        query_vector_source: &VectorSource,
        top_k: usize,
    ) -> Result<ExecutionResult> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("score", DataType::Float32, false),
        ]));

        let vector_store = self
            .storage
            .get_vector_store()
            .ok_or_else(|| anyhow!("Vector store is not configured"))?;
        let engine = vector_store
            .primary_engine()
            .ok_or_else(|| anyhow!("Vector store has no primary engine"))?;
        let query_vector = self.resolve_query_vector(query_vector_source)?;

        if query_vector.is_empty() {
            return Err(anyhow!(
                "Vector search requires a non-empty query vector for collection '{}'",
                collection
            ));
        }

        use crate::proto::proximadb_v1::CollectionConfig;
        let collection_config = Arc::new(Collection {
            id: collection.to_string(),
            config: Some(CollectionConfig {
                name: collection.to_string(),
                dimension: query_vector.len() as u32,
                ..Default::default()
            }),
            ..Default::default()
        });

        let search_params = Arc::new(SearchParams {
            top_k: Some(top_k),
            vector: Some(query_vector),
            ..Default::default()
        });
        let query_context = StorageQueryContext::new(search_params, collection_config);
        let results = engine.search_vectors_unified(&query_context).await?;

        if results.is_empty() {
            return Ok(ExecutionResult::empty_with_schema(schema));
        }

        let ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
        let scores: Vec<f32> = results.iter().map(|r| r.score).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)) as ArrayRef,
                Arc::new(Float32Array::from(scores)) as ArrayRef,
            ],
        )?;

        Ok(ExecutionResult::from_batch(batch))
    }

    /// Execute graph traversal
    async fn execute_graph_traversal(
        &self,
        cypher: &str,
        start_nodes: Option<&Vec<String>>,
    ) -> Result<ExecutionResult> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("node_id", DataType::Utf8, false),
            Field::new("label", DataType::Utf8, true),
            Field::new("properties", DataType::Utf8, true),
        ]));

        let graph_store = self
            .storage
            .get_graph_store()
            .ok_or_else(|| anyhow!("Graph store is not configured"))?;

        let nodes = if let Some(start_node_ids) = start_nodes {
            let mut result_nodes = Vec::new();
            for node_id in start_node_ids {
                if let Some(node) = graph_store.fetch_node(node_id).await? {
                    result_nodes.push(node);
                    if cypher.contains("-->") || cypher.contains("-[") {
                        result_nodes.extend(graph_store.fetch_neighbors(node_id).await?);
                    }
                }
            }
            result_nodes
        } else if cypher.contains(':') {
            let label = cypher
                .split(':')
                .nth(1)
                .and_then(|s| s.split(|c| c == ')' || c == ' ' || c == '{').next())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty());

            if let Some(label) = label {
                graph_store.fetch_nodes_by_label(label).await?
            } else {
                graph_store.fetch_all_nodes().await?
            }
        } else {
            graph_store.fetch_all_nodes().await?
        };

        if nodes.is_empty() {
            return Ok(ExecutionResult::empty_with_schema(schema));
        }

        let node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        let labels: Vec<Option<String>> = nodes.iter().map(|n| n.labels.first().cloned()).collect();
        let props: Vec<String> = nodes
            .iter()
            .map(|n| serde_json::to_string(&n.properties).unwrap_or_else(|_| "{}".to_string()))
            .collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(node_ids)) as ArrayRef,
                Arc::new(StringArray::from(labels)) as ArrayRef,
                Arc::new(StringArray::from(props)) as ArrayRef,
            ],
        )?;

        Ok(ExecutionResult::from_batch(batch))
    }

    /// Execute document query
    async fn execute_document_query(
        &self,
        collection: &str,
        filter: Option<&String>,
    ) -> Result<ExecutionResult> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("document", DataType::Utf8, false),
        ]));

        let doc_store = self
            .storage
            .get_document_store()
            .ok_or_else(|| anyhow!("Document store is not configured"))?;

        let doc_filter = filter.and_then(|f| {
            if f.contains('=') {
                let parts: Vec<&str> = f.splitn(2, '=').collect();
                if parts.len() == 2 {
                    Some(DocumentFilter {
                        conditions: vec![DocFilterCondition {
                            path: parts[0].trim().to_string(),
                            operator: DocFilterOperator::Eq as i32,
                            value: Some(SqlValue {
                                value: Some(sql_value::Value::StringValue(
                                    parts[1].trim().trim_matches('"').to_string(),
                                )),
                            }),
                            values: vec![],
                        }],
                        or_filters: vec![],
                        and_filters: vec![],
                    })
                } else {
                    None
                }
            } else {
                None
            }
        });

        let documents = doc_store
            .query_documents(collection, doc_filter, self.config.batch_size, 0)
            .await?;

        if documents.is_empty() {
            return Ok(ExecutionResult::empty_with_schema(schema));
        }

        let ids: Vec<String> = documents.iter().map(|d| d.id.clone()).collect();
        let docs: Vec<String> = documents
            .iter()
            .map(|d| serde_json::to_string(&d.document).unwrap_or_else(|_| "{}".to_string()))
            .collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)) as ArrayRef,
                Arc::new(StringArray::from(docs)) as ArrayRef,
            ],
        )?;

        Ok(ExecutionResult::from_batch(batch))
    }

    /// Execute observability query
    async fn execute_observability_query(
        &self,
        namespace: &str,
        query_type: &ObservabilityQueryType,
    ) -> Result<ExecutionResult> {
        let schema = match query_type {
            ObservabilityQueryType::Logs => Arc::new(Schema::new(vec![
                Field::new("timestamp", DataType::Int64, false),
                Field::new("level", DataType::Utf8, false),
                Field::new("message", DataType::Utf8, false),
            ])),
            ObservabilityQueryType::Metrics => Arc::new(Schema::new(vec![
                Field::new("timestamp", DataType::Int64, false),
                Field::new("metric_name", DataType::Utf8, false),
                Field::new("value", DataType::Float32, false),
            ])),
            ObservabilityQueryType::Traces => Arc::new(Schema::new(vec![
                Field::new("trace_id", DataType::Utf8, false),
                Field::new("span_id", DataType::Utf8, false),
                Field::new("operation", DataType::Utf8, false),
                Field::new("duration_ns", DataType::Int64, false),
            ])),
        };

        let obs_store = self
            .storage
            .get_observability_store()
            .ok_or_else(|| anyhow!("Observability store is not configured"))?;
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let hour_ago_ns = now_ns - (3600 * 1_000_000_000);

        match query_type {
            ObservabilityQueryType::Logs => {
                let result = obs_store
                    .query_logs(namespace, hour_ago_ns, now_ns, None, 1000)
                    .await?;
                if result.logs.is_empty() {
                    return Ok(ExecutionResult::empty_with_schema(schema));
                }

                use crate::proto::proximadb_v1::Severity;
                let timestamps: Vec<i64> = result.logs.iter().map(|l| l.timestamp_ns).collect();
                let levels: Vec<String> = result
                    .logs
                    .iter()
                    .map(|l| {
                        match Severity::try_from(l.severity).unwrap_or(Severity::Info) {
                            Severity::Debug => "DEBUG",
                            Severity::Info => "INFO",
                            Severity::Warn => "WARN",
                            Severity::Error => "ERROR",
                            Severity::Fatal => "FATAL",
                            _ => "UNKNOWN",
                        }
                        .to_string()
                    })
                    .collect();
                let messages: Vec<String> = result.logs.iter().map(|l| l.message.clone()).collect();

                let batch = RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(Int64Array::from(timestamps)) as ArrayRef,
                        Arc::new(StringArray::from(levels)) as ArrayRef,
                        Arc::new(StringArray::from(messages)) as ArrayRef,
                    ],
                )?;

                Ok(ExecutionResult::from_batch(batch))
            }
            ObservabilityQueryType::Metrics => {
                use crate::proto::proximadb_v1::MetricAggregation;
                let params = MetricAggregationParams {
                    metric_name: "*".to_string(),
                    start_time_ns: hour_ago_ns,
                    end_time_ns: now_ns,
                    aggregation: MetricAggregation::Avg,
                    step_seconds: 60,
                    label_filters: HashMap::new(),
                    group_by: vec![],
                };
                let result = obs_store.aggregate_metrics(namespace, params).await?;
                let mut timestamps = Vec::new();
                let mut names = Vec::new();
                let mut values = Vec::new();

                for series in &result.series {
                    let series_name = series
                        .labels
                        .get("__name__")
                        .cloned()
                        .unwrap_or_else(|| "metric".to_string());
                    for point in &series.points {
                        timestamps.push(point.timestamp_ns);
                        names.push(series_name.clone());
                        values.push(point.value as f32);
                    }
                }

                if timestamps.is_empty() {
                    return Ok(ExecutionResult::empty_with_schema(schema));
                }

                let batch = RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(Int64Array::from(timestamps)) as ArrayRef,
                        Arc::new(StringArray::from(names)) as ArrayRef,
                        Arc::new(Float32Array::from(values)) as ArrayRef,
                    ],
                )?;

                Ok(ExecutionResult::from_batch(batch))
            }
            ObservabilityQueryType::Traces => {
                let traces = obs_store
                    .query_traces(namespace, hour_ago_ns, now_ns, None, None, 100)
                    .await?;

                if traces.is_empty() {
                    return Ok(ExecutionResult::empty_with_schema(schema));
                }

                let trace_ids: Vec<String> = traces.iter().map(|t| t.trace_id.clone()).collect();
                let span_ids: Vec<String> = traces.iter().map(|t| t.span_id.clone()).collect();
                let operations: Vec<String> = traces.iter().map(|t| t.name.clone()).collect();
                let durations: Vec<i64> = traces
                    .iter()
                    .map(|t| t.end_time_ns - t.start_time_ns)
                    .collect();

                let batch = RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(StringArray::from(trace_ids)) as ArrayRef,
                        Arc::new(StringArray::from(span_ids)) as ArrayRef,
                        Arc::new(StringArray::from(operations)) as ArrayRef,
                        Arc::new(Int64Array::from(durations)) as ArrayRef,
                    ],
                )?;

                Ok(ExecutionResult::from_batch(batch))
            }
        }
    }

    fn resolve_query_vector(&self, source: &VectorSource) -> Result<Vec<f32>> {
        match source {
            VectorSource::Literal(vector) => Ok(vector.clone()),
            VectorSource::Expression(expr) => Self::parse_vector_literal(expr).ok_or_else(|| {
                anyhow!(
                    "Unsupported vector expression '{}' in federated executor; provide a literal vector for now",
                    expr
                )
            }),
            VectorSource::ColumnRef { table, column } => Err(anyhow!(
                "Correlated vector source '{}.{}' is not yet executable in the federated executor",
                table,
                column
            )),
            VectorSource::Subquery(_) => Err(anyhow!(
                "Subquery-derived vector sources are not yet executable in the federated executor"
            )),
        }
    }

    fn parse_vector_literal(raw: &str) -> Option<Vec<f32>> {
        let trimmed = raw.trim();
        let without_cast = trimmed
            .strip_suffix("::vector")
            .or_else(|| trimmed.strip_suffix("::VECTOR"))
            .unwrap_or(trimmed)
            .trim();
        let unquoted = without_cast.trim_matches('\'').trim_matches('"').trim();

        if !(unquoted.starts_with('[') && unquoted.ends_with(']')) {
            return None;
        }

        let inner = &unquoted[1..unquoted.len() - 1];
        if inner.trim().is_empty() {
            return Some(Vec::new());
        }

        inner
            .split(',')
            .map(|value| value.trim().parse::<f32>().ok())
            .collect()
    }

    fn merge_batches(&self, result: &ExecutionResult) -> Result<RecordBatch> {
        if result.batches.is_empty() {
            return Ok(RecordBatch::new_empty(result.schema.clone()));
        }

        if result.batches.len() == 1 {
            return Ok(result.batches[0].clone());
        }

        let columns = (0..result.schema.fields().len())
            .map(|column_idx| {
                let arrays: Vec<&dyn Array> = result
                    .batches
                    .iter()
                    .map(|batch| batch.column(column_idx).as_ref())
                    .collect();
                concat(&arrays)
            })
            .collect::<arrow::error::Result<Vec<_>>>()?;

        Ok(RecordBatch::try_new(result.schema.clone(), columns)?)
    }

    fn normalize_column_name(column: &str) -> &str {
        column.rsplit('.').next().unwrap_or(column).trim()
    }

    fn resolve_column_index(schema: &Schema, requested: &str) -> Option<usize> {
        let normalized = Self::normalize_column_name(requested);

        schema
            .fields()
            .iter()
            .position(|field| field.name().eq_ignore_ascii_case(requested))
            .or_else(|| {
                schema
                    .fields()
                    .iter()
                    .position(|field| field.name().eq_ignore_ascii_case(normalized))
            })
            .or_else(|| {
                schema.fields().iter().position(|field| {
                    Self::normalize_column_name(field.name()).eq_ignore_ascii_case(normalized)
                })
            })
            .or_else(|| {
                if normalized.eq_ignore_ascii_case("id") {
                    schema
                        .fields()
                        .iter()
                        .position(|field| field.name().eq_ignore_ascii_case("node_id"))
                } else {
                    None
                }
            })
    }

    fn predicate_matches_row(
        batch: &RecordBatch,
        row: usize,
        predicate: &super::optimizer::Predicate,
    ) -> Result<bool> {
        let column_index = Self::resolve_column_index(batch.schema().as_ref(), &predicate.column)
            .ok_or_else(|| {
            anyhow!(
                "Column '{}' was not found in schema {:?}",
                predicate.column,
                batch
                    .schema()
                    .fields()
                    .iter()
                    .map(|field| field.name().clone())
                    .collect::<Vec<_>>()
            )
        })?;
        let array = batch.column(column_index);

        if matches!(predicate.op, super::optimizer::PredicateOp::IsNull) {
            return Ok(array.is_null(row));
        }
        if matches!(predicate.op, super::optimizer::PredicateOp::IsNotNull) {
            return Ok(!array.is_null(row));
        }
        if array.is_null(row) {
            return Ok(false);
        }

        match array.data_type() {
            DataType::Utf8 => {
                let values = array
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| anyhow!("Failed to downcast Utf8 predicate column"))?;
                let value = values.value(row);
                match (&predicate.op, &predicate.value) {
                    (
                        super::optimizer::PredicateOp::Eq,
                        super::optimizer::PredicateValue::String(expected),
                    ) => Ok(value == expected),
                    (
                        super::optimizer::PredicateOp::Ne,
                        super::optimizer::PredicateValue::String(expected),
                    ) => Ok(value != expected),
                    (
                        super::optimizer::PredicateOp::Like,
                        super::optimizer::PredicateValue::String(expected),
                    ) => Ok(Self::like_matches(value, expected)),
                    (
                        super::optimizer::PredicateOp::Gt,
                        super::optimizer::PredicateValue::String(expected),
                    ) => Ok(value > expected.as_str()),
                    (
                        super::optimizer::PredicateOp::Ge,
                        super::optimizer::PredicateValue::String(expected),
                    ) => Ok(value >= expected.as_str()),
                    (
                        super::optimizer::PredicateOp::Lt,
                        super::optimizer::PredicateValue::String(expected),
                    ) => Ok(value < expected.as_str()),
                    (
                        super::optimizer::PredicateOp::Le,
                        super::optimizer::PredicateValue::String(expected),
                    ) => Ok(value <= expected.as_str()),
                    _ => Err(anyhow!(
                        "Predicate {:?} with value {:?} is not supported for Utf8 column '{}'",
                        predicate.op,
                        predicate.value,
                        predicate.column
                    )),
                }
            }
            DataType::Int64 => {
                let values = array
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| anyhow!("Failed to downcast Int64 predicate column"))?;
                let value = values.value(row);
                Self::compare_numeric_predicate(value as f64, predicate)
            }
            DataType::Int32 => {
                let values = array
                    .as_any()
                    .downcast_ref::<arrow::array::Int32Array>()
                    .ok_or_else(|| anyhow!("Failed to downcast Int32 predicate column"))?;
                let value = values.value(row);
                Self::compare_numeric_predicate(value as f64, predicate)
            }
            DataType::UInt64 => {
                let values = array
                    .as_any()
                    .downcast_ref::<arrow::array::UInt64Array>()
                    .ok_or_else(|| anyhow!("Failed to downcast UInt64 predicate column"))?;
                let value = values.value(row);
                Self::compare_numeric_predicate(value as f64, predicate)
            }
            DataType::UInt32 => {
                let values = array
                    .as_any()
                    .downcast_ref::<arrow::array::UInt32Array>()
                    .ok_or_else(|| anyhow!("Failed to downcast UInt32 predicate column"))?;
                let value = values.value(row);
                Self::compare_numeric_predicate(value as f64, predicate)
            }
            DataType::Float32 => {
                let values = array
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow!("Failed to downcast Float32 predicate column"))?;
                let value = values.value(row);
                Self::compare_numeric_predicate(value as f64, predicate)
            }
            DataType::Float64 => {
                let values = array
                    .as_any()
                    .downcast_ref::<arrow::array::Float64Array>()
                    .ok_or_else(|| anyhow!("Failed to downcast Float64 predicate column"))?;
                let value = values.value(row);
                Self::compare_numeric_predicate(value, predicate)
            }
            DataType::Boolean => {
                let values = array
                    .as_any()
                    .downcast_ref::<arrow::array::BooleanArray>()
                    .ok_or_else(|| anyhow!("Failed to downcast Boolean predicate column"))?;
                let value = values.value(row);
                match (&predicate.op, &predicate.value) {
                    (
                        super::optimizer::PredicateOp::Eq,
                        super::optimizer::PredicateValue::Bool(expected),
                    ) => Ok(value == *expected),
                    (
                        super::optimizer::PredicateOp::Ne,
                        super::optimizer::PredicateValue::Bool(expected),
                    ) => Ok(value != *expected),
                    _ => Err(anyhow!(
                        "Predicate {:?} with value {:?} is not supported for Boolean column '{}'",
                        predicate.op,
                        predicate.value,
                        predicate.column
                    )),
                }
            }
            other => Err(anyhow!(
                "Filtering on data type {:?} is not yet supported for column '{}'",
                other,
                predicate.column
            )),
        }
    }

    fn compare_numeric_predicate(
        actual: f64,
        predicate: &super::optimizer::Predicate,
    ) -> Result<bool> {
        let expected = match &predicate.value {
            super::optimizer::PredicateValue::Int(value) => *value as f64,
            super::optimizer::PredicateValue::Float(value) => *value,
            _ => {
                return Err(anyhow!(
                    "Predicate {:?} requires a numeric literal for column '{}'",
                    predicate.op,
                    predicate.column
                ));
            }
        };

        Ok(match predicate.op {
            super::optimizer::PredicateOp::Eq => actual == expected,
            super::optimizer::PredicateOp::Ne => actual != expected,
            super::optimizer::PredicateOp::Lt => actual < expected,
            super::optimizer::PredicateOp::Le => actual <= expected,
            super::optimizer::PredicateOp::Gt => actual > expected,
            super::optimizer::PredicateOp::Ge => actual >= expected,
            _ => {
                return Err(anyhow!(
                    "Numeric predicate {:?} is not supported for column '{}'",
                    predicate.op,
                    predicate.column
                ));
            }
        })
    }

    fn like_matches(value: &str, pattern: &str) -> bool {
        if pattern == "%" {
            return true;
        }
        if let Some(inner) = pattern
            .strip_prefix('%')
            .and_then(|trimmed| trimmed.strip_suffix('%'))
        {
            return value.contains(inner);
        }
        if let Some(suffix) = pattern.strip_prefix('%') {
            return value.ends_with(suffix);
        }
        if let Some(prefix) = pattern.strip_suffix('%') {
            return value.starts_with(prefix);
        }
        value == pattern
    }

    fn compare_rows(
        batch: &RecordBatch,
        left_row: usize,
        right_row: usize,
        order_by: &[super::optimizer::OrderByClause],
    ) -> Result<std::cmp::Ordering> {
        for clause in order_by {
            let column_index = Self::resolve_column_index(batch.schema().as_ref(), &clause.column)
                .ok_or_else(|| anyhow!("Sort column '{}' was not found", clause.column))?;
            let array = batch.column(column_index);
            let left_null = array.is_null(left_row);
            let right_null = array.is_null(right_row);

            let ordering = match (left_null, right_null) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => {
                    if clause.nulls_first {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
                (false, true) => {
                    if clause.nulls_first {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Less
                    }
                }
                (false, false) => match array.data_type() {
                    DataType::Utf8 => {
                        let values = array
                            .as_any()
                            .downcast_ref::<StringArray>()
                            .ok_or_else(|| anyhow!("Failed to downcast Utf8 sort column"))?;
                        values.value(left_row).cmp(values.value(right_row))
                    }
                    DataType::Int64 => {
                        let values = array
                            .as_any()
                            .downcast_ref::<Int64Array>()
                            .ok_or_else(|| anyhow!("Failed to downcast Int64 sort column"))?;
                        values.value(left_row).cmp(&values.value(right_row))
                    }
                    DataType::Int32 => {
                        let values = array
                            .as_any()
                            .downcast_ref::<arrow::array::Int32Array>()
                            .ok_or_else(|| anyhow!("Failed to downcast Int32 sort column"))?;
                        values.value(left_row).cmp(&values.value(right_row))
                    }
                    DataType::UInt64 => {
                        let values = array
                            .as_any()
                            .downcast_ref::<arrow::array::UInt64Array>()
                            .ok_or_else(|| anyhow!("Failed to downcast UInt64 sort column"))?;
                        values.value(left_row).cmp(&values.value(right_row))
                    }
                    DataType::UInt32 => {
                        let values = array
                            .as_any()
                            .downcast_ref::<arrow::array::UInt32Array>()
                            .ok_or_else(|| anyhow!("Failed to downcast UInt32 sort column"))?;
                        values.value(left_row).cmp(&values.value(right_row))
                    }
                    DataType::Float32 => {
                        let values = array
                            .as_any()
                            .downcast_ref::<Float32Array>()
                            .ok_or_else(|| anyhow!("Failed to downcast Float32 sort column"))?;
                        values
                            .value(left_row)
                            .partial_cmp(&values.value(right_row))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    }
                    DataType::Float64 => {
                        let values = array
                            .as_any()
                            .downcast_ref::<arrow::array::Float64Array>()
                            .ok_or_else(|| anyhow!("Failed to downcast Float64 sort column"))?;
                        values
                            .value(left_row)
                            .partial_cmp(&values.value(right_row))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    }
                    DataType::Boolean => {
                        let values = array
                            .as_any()
                            .downcast_ref::<arrow::array::BooleanArray>()
                            .ok_or_else(|| anyhow!("Failed to downcast Boolean sort column"))?;
                        values.value(left_row).cmp(&values.value(right_row))
                    }
                    other => {
                        return Err(anyhow!(
                            "Sorting on data type {:?} is not yet supported for column '{}'",
                            other,
                            clause.column
                        ));
                    }
                },
            };

            let ordering = if clause.ascending {
                ordering
            } else {
                ordering.reverse()
            };

            if ordering != std::cmp::Ordering::Equal {
                return Ok(ordering);
            }
        }

        Ok(std::cmp::Ordering::Equal)
    }

    fn extract_join_key(
        batch: &RecordBatch,
        row: usize,
        key_indices: &[usize],
    ) -> Result<Option<Vec<JoinKey>>> {
        let mut key = Vec::with_capacity(key_indices.len());

        for &index in key_indices {
            let array = batch.column(index);
            let value = match array.data_type() {
                DataType::Utf8 => {
                    let values = array
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| anyhow!("Failed to downcast Utf8 join column"))?;
                    if values.is_null(row) {
                        return Ok(None);
                    }
                    JoinKey::Utf8(values.value(row).to_string())
                }
                DataType::Int64 => {
                    let values = array
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .ok_or_else(|| anyhow!("Failed to downcast Int64 join column"))?;
                    if values.is_null(row) {
                        return Ok(None);
                    }
                    JoinKey::Int64(values.value(row))
                }
                DataType::Int32 => {
                    let values = array
                        .as_any()
                        .downcast_ref::<arrow::array::Int32Array>()
                        .ok_or_else(|| anyhow!("Failed to downcast Int32 join column"))?;
                    if values.is_null(row) {
                        return Ok(None);
                    }
                    JoinKey::Int32(values.value(row))
                }
                DataType::UInt64 => {
                    let values = array
                        .as_any()
                        .downcast_ref::<arrow::array::UInt64Array>()
                        .ok_or_else(|| anyhow!("Failed to downcast UInt64 join column"))?;
                    if values.is_null(row) {
                        return Ok(None);
                    }
                    JoinKey::UInt64(values.value(row))
                }
                DataType::UInt32 => {
                    let values = array
                        .as_any()
                        .downcast_ref::<arrow::array::UInt32Array>()
                        .ok_or_else(|| anyhow!("Failed to downcast UInt32 join column"))?;
                    if values.is_null(row) {
                        return Ok(None);
                    }
                    JoinKey::UInt32(values.value(row))
                }
                DataType::Boolean => {
                    let values = array
                        .as_any()
                        .downcast_ref::<arrow::array::BooleanArray>()
                        .ok_or_else(|| anyhow!("Failed to downcast Boolean join column"))?;
                    if values.is_null(row) {
                        return Ok(None);
                    }
                    JoinKey::Boolean(values.value(row))
                }
                other => {
                    return Err(anyhow!(
                        "Join keys of type {:?} are not yet supported in federated execution",
                        other
                    ));
                }
            };

            key.push(value);
        }

        Ok(Some(key))
    }

    fn build_take_indices(indices: &[Option<usize>]) -> arrow::array::UInt32Array {
        let mut builder = UInt32Builder::with_capacity(indices.len());
        for index in indices {
            match index {
                Some(index) => builder.append_value(*index as u32),
                None => builder.append_null(),
            }
        }
        builder.finish()
    }

    fn build_join_schema(&self, left: &Schema, right: &Schema) -> Arc<Schema> {
        let mut fields = Vec::with_capacity(left.fields().len() + right.fields().len());
        let mut seen = HashSet::new();

        for field in left.fields() {
            seen.insert(field.name().to_string());
            fields.push(field.as_ref().clone());
        }

        for field in right.fields() {
            let mut name = field.name().to_string();
            while seen.contains(&name) {
                name = format!("right_{}", name);
            }
            seen.insert(name.clone());
            fields.push(Field::new(
                &name,
                field.data_type().clone(),
                field.is_nullable(),
            ));
        }

        Arc::new(Schema::new(fields))
    }

    /// Execute hash join
    async fn execute_hash_join(
        &self,
        left: &PlanNode,
        right: &PlanNode,
        join_keys: &[(String, String)],
        join_type: &JoinType,
    ) -> Result<ExecutionResult> {
        let left_result = self.execute_node(left).await?;
        let right_result = self.execute_node(right).await?;

        let left_batch = self.merge_batches(&left_result)?;
        let right_batch = self.merge_batches(&right_result)?;
        let joined_schema =
            self.build_join_schema(left_batch.schema().as_ref(), right_batch.schema().as_ref());

        if *join_type == JoinType::Cross {
            let mut left_indices = Vec::new();
            let mut right_indices = Vec::new();

            for left_row in 0..left_batch.num_rows() {
                for right_row in 0..right_batch.num_rows() {
                    left_indices.push(Some(left_row));
                    right_indices.push(Some(right_row));
                }
            }

            if left_indices.is_empty() {
                return Ok(ExecutionResult::empty_with_schema(joined_schema));
            }

            let left_take = Self::build_take_indices(&left_indices);
            let right_take = Self::build_take_indices(&right_indices);
            let mut columns = Vec::with_capacity(
                left_batch.schema().fields().len() + right_batch.schema().fields().len(),
            );

            for column in left_batch.columns() {
                columns.push(take(column.as_ref(), &left_take, None)?);
            }
            for column in right_batch.columns() {
                columns.push(take(column.as_ref(), &right_take, None)?);
            }

            let batch = RecordBatch::try_new(joined_schema.clone(), columns)?;
            return Ok(ExecutionResult::from_batch(batch));
        }

        if join_keys.is_empty() {
            return Err(anyhow!(
                "Hash join requires at least one join key unless join type is Cross"
            ));
        }

        let left_key_indices = join_keys
            .iter()
            .map(|(left_key, _)| {
                Self::resolve_column_index(left_batch.schema().as_ref(), left_key).ok_or_else(
                    || {
                        anyhow!(
                            "Join key '{}' was not found in left schema {:?}",
                            left_key,
                            left_batch
                                .schema()
                                .fields()
                                .iter()
                                .map(|field| field.name().clone())
                                .collect::<Vec<_>>()
                        )
                    },
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let right_key_indices = join_keys
            .iter()
            .map(|(_, right_key)| {
                Self::resolve_column_index(right_batch.schema().as_ref(), right_key).ok_or_else(
                    || {
                        anyhow!(
                            "Join key '{}' was not found in right schema {:?}",
                            right_key,
                            right_batch
                                .schema()
                                .fields()
                                .iter()
                                .map(|field| field.name().clone())
                                .collect::<Vec<_>>()
                        )
                    },
                )
            })
            .collect::<Result<Vec<_>>>()?;

        let mut right_index: HashMap<Vec<JoinKey>, Vec<usize>> = HashMap::new();
        for row in 0..right_batch.num_rows() {
            if let Some(key) = Self::extract_join_key(&right_batch, row, &right_key_indices)? {
                right_index.entry(key).or_default().push(row);
            }
        }

        let mut matched_right_rows = vec![false; right_batch.num_rows()];
        let mut left_indices = Vec::new();
        let mut right_indices = Vec::new();

        for left_row in 0..left_batch.num_rows() {
            let Some(key) = Self::extract_join_key(&left_batch, left_row, &left_key_indices)?
            else {
                if matches!(join_type, JoinType::Left | JoinType::Full) {
                    left_indices.push(Some(left_row));
                    right_indices.push(None);
                }
                continue;
            };

            if let Some(matches) = right_index.get(&key) {
                for &right_row in matches {
                    matched_right_rows[right_row] = true;
                    left_indices.push(Some(left_row));
                    right_indices.push(Some(right_row));
                }
            } else if matches!(join_type, JoinType::Left | JoinType::Full) {
                left_indices.push(Some(left_row));
                right_indices.push(None);
            }
        }

        if matches!(join_type, JoinType::Right | JoinType::Full) {
            for (right_row, matched) in matched_right_rows.iter().enumerate() {
                if !matched {
                    left_indices.push(None);
                    right_indices.push(Some(right_row));
                }
            }
        }

        if left_indices.is_empty() && right_indices.is_empty() {
            return Ok(ExecutionResult::empty_with_schema(joined_schema));
        }

        let left_take = Self::build_take_indices(&left_indices);
        let right_take = Self::build_take_indices(&right_indices);
        let mut columns = Vec::with_capacity(
            left_batch.schema().fields().len() + right_batch.schema().fields().len(),
        );

        for column in left_batch.columns() {
            columns.push(take(column.as_ref(), &left_take, None)?);
        }
        for column in right_batch.columns() {
            columns.push(take(column.as_ref(), &right_take, None)?);
        }

        let batch = if left_indices.is_empty() {
            let arrays = joined_schema
                .fields()
                .iter()
                .map(|field| new_null_array(field.data_type(), 0))
                .collect();
            RecordBatch::try_new(joined_schema.clone(), arrays)?
        } else {
            RecordBatch::try_new(joined_schema.clone(), columns)?
        };

        Ok(ExecutionResult::from_batch(batch))
    }

    /// Execute nested loop join (for LATERAL)
    async fn execute_nested_loop_join(
        &self,
        outer: &PlanNode,
        inner: &PlanNode,
        correlation: &[String],
    ) -> Result<ExecutionResult> {
        let _ = (outer, inner);
        Err(anyhow!(
            "Nested-loop/lateral join execution is not implemented for correlations {:?}; correlated multi-model joins are still experimental",
            correlation
        ))
    }

    /// Execute index join
    async fn execute_index_join(
        &self,
        left: &PlanNode,
        right: &PlanNode,
        _index_lookup: &str,
    ) -> Result<ExecutionResult> {
        // Similar to hash join but uses index
        self.execute_hash_join(left, right, &[], &JoinType::Inner)
            .await
    }

    /// Execute filter
    async fn execute_filter(
        &self,
        input: &PlanNode,
        predicate: &super::optimizer::Predicate,
    ) -> Result<ExecutionResult> {
        let result = self.execute_node(input).await?;
        let batch = self.merge_batches(&result)?;

        if batch.num_rows() == 0 {
            return Ok(ExecutionResult::empty_with_schema(batch.schema()));
        }

        let matched_indices = (0..batch.num_rows())
            .filter_map(
                |row| match Self::predicate_matches_row(&batch, row, predicate) {
                    Ok(true) => Some(Ok(Some(row))),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .collect::<Result<Vec<_>>>()?;

        if matched_indices.is_empty() {
            return Ok(ExecutionResult::empty_with_schema(batch.schema()));
        }

        let take_indices = Self::build_take_indices(&matched_indices);
        let columns = batch
            .columns()
            .iter()
            .map(|column| take(column.as_ref(), &take_indices, None))
            .collect::<arrow::error::Result<Vec<_>>>()?;

        let filtered = RecordBatch::try_new(batch.schema(), columns)?;
        Ok(ExecutionResult::from_batch(filtered))
    }

    /// Execute projection
    async fn execute_project(
        &self,
        input: &PlanNode,
        columns: &[String],
    ) -> Result<ExecutionResult> {
        let result = self.execute_node(input).await?;
        if columns.iter().any(|column| column == "*") {
            return Ok(result);
        }

        let batch = self.merge_batches(&result)?;
        let projected_indices = columns
            .iter()
            .map(|column| {
                Self::resolve_column_index(batch.schema().as_ref(), column)
                    .ok_or_else(|| anyhow!("Projection column '{}' was not found", column))
            })
            .collect::<Result<Vec<_>>>()?;

        let batch_schema = batch.schema();
        let projected_fields = projected_indices
            .iter()
            .zip(columns.iter())
            .map(|(index, requested_name)| {
                let field = batch_schema.field(*index);
                if field.name() == requested_name {
                    field.as_ref().clone()
                } else {
                    Field::new(
                        requested_name,
                        field.data_type().clone(),
                        field.is_nullable(),
                    )
                }
            })
            .collect::<Vec<_>>();
        let schema = Arc::new(Schema::new(projected_fields));

        if batch.num_rows() == 0 {
            return Ok(ExecutionResult::empty_with_schema(schema));
        }

        let projected_columns = projected_indices
            .iter()
            .map(|index| batch.column(*index).clone())
            .collect::<Vec<_>>();
        let projected = RecordBatch::try_new(schema, projected_columns)?;
        Ok(ExecutionResult::from_batch(projected))
    }

    /// Execute sort
    async fn execute_sort(
        &self,
        input: &PlanNode,
        order_by: &[super::optimizer::OrderByClause],
    ) -> Result<ExecutionResult> {
        let result = self.execute_node(input).await?;
        if order_by.is_empty() {
            return Ok(result);
        }

        let batch = self.merge_batches(&result)?;
        if batch.num_rows() <= 1 {
            return Ok(ExecutionResult::from_batch(batch));
        }

        for clause in order_by {
            let column_index = Self::resolve_column_index(batch.schema().as_ref(), &clause.column)
                .ok_or_else(|| anyhow!("Sort column '{}' was not found", clause.column))?;
            match batch.column(column_index).data_type() {
                DataType::Utf8
                | DataType::Int64
                | DataType::Int32
                | DataType::UInt64
                | DataType::UInt32
                | DataType::Float32
                | DataType::Float64
                | DataType::Boolean => {}
                other => {
                    return Err(anyhow!(
                        "Sorting on data type {:?} is not yet supported for column '{}'",
                        other,
                        clause.column
                    ));
                }
            }
        }

        let mut row_indices: Vec<usize> = (0..batch.num_rows()).collect();
        row_indices.sort_by(|left, right| {
            Self::compare_rows(&batch, *left, *right, order_by)
                .expect("sort columns should be validated before sorting")
        });

        let take_indices = Self::build_take_indices(
            &row_indices
                .into_iter()
                .map(Some)
                .collect::<Vec<Option<usize>>>(),
        );
        let columns = batch
            .columns()
            .iter()
            .map(|column| take(column.as_ref(), &take_indices, None))
            .collect::<arrow::error::Result<Vec<_>>>()?;

        let sorted = RecordBatch::try_new(batch.schema(), columns)?;
        Ok(ExecutionResult::from_batch(sorted))
    }

    /// Execute limit
    async fn execute_limit(
        &self,
        input: &PlanNode,
        limit: usize,
        offset: usize,
    ) -> Result<ExecutionResult> {
        let result = self.execute_node(input).await?;

        // Apply limit/offset to batches
        let mut remaining_offset = offset;
        let mut remaining_limit = limit;
        let mut output_batches = Vec::new();

        for batch in &result.batches {
            if remaining_offset >= batch.num_rows() {
                remaining_offset -= batch.num_rows();
                continue;
            }

            let start = remaining_offset;
            let end = (start + remaining_limit).min(batch.num_rows());
            remaining_offset = 0;

            if end > start {
                let sliced = batch.slice(start, end - start);
                remaining_limit -= sliced.num_rows();
                output_batches.push(sliced);
            }

            if remaining_limit == 0 {
                break;
            }
        }

        Ok(ExecutionResult {
            batches: output_batches,
            schema: result.schema,
            stats: ExecutionStats {
                rows_produced: limit.saturating_sub(remaining_limit),
                ..Default::default()
            },
        })
    }

    /// Execute aggregate
    async fn execute_aggregate(
        &self,
        input: &PlanNode,
        group_by: &[String],
        aggregates: &[super::optimizer::AggregateExpr],
    ) -> Result<ExecutionResult> {
        let _ = input;
        let aggregate_aliases: Vec<&str> = aggregates
            .iter()
            .map(|aggregate| aggregate.alias.as_str())
            .collect();
        Err(anyhow!(
            "Aggregate execution is not implemented for group_by {:?} and aggregates {:?}",
            group_by,
            aggregate_aliases
        ))
    }

    /// Execute union
    async fn execute_union(&self, inputs: &[PlanNode], _all: bool) -> Result<ExecutionResult> {
        let mut all_batches = Vec::new();
        let mut schema = None;

        for input in inputs {
            let result = self.execute_node(input).await?;
            if schema.is_none() {
                schema = Some(result.schema.clone());
            }
            all_batches.extend(result.batches);
        }

        Ok(ExecutionResult {
            batches: all_batches,
            schema: schema.unwrap_or_else(|| Arc::new(Schema::empty())),
            stats: ExecutionStats::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_result_empty() {
        let result = ExecutionResult::empty();
        assert_eq!(result.row_count(), 0);
        assert!(result.batches.is_empty());
    }

    #[test]
    fn test_execution_config_default() {
        let config = ExecutionConfig::default();
        assert_eq!(config.batch_size, 10_000);
        assert!(config.parallel_execution);
    }

    #[tokio::test]
    async fn test_executor_creation() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let executor = FederatedExecutor::new(storage);
        assert!(executor.config.parallel_execution);
    }
}
