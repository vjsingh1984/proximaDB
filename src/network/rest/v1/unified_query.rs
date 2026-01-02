//! # Unified Multi-Model Query API
//!
//! REST API for cross-model queries combining all ProximaDB data models.
//!
//! ## Available Endpoints
//!
//! | Endpoint | Method | Description |
//! |----------|--------|-------------|
//! | `/api/v1/unified/query` | POST | Structured multi-model query |
//! | `/api/v1/unified/federated` | POST | SQL with extensions (VECTOR_SEARCH, GRAPH_QUERY, etc.) |
//!
//! ## Query Paths
//!
//! This module provides two query paths:
//!
//! ### 1. Structured Unified Queries (`/query`)
//!
//! Uses `QueryDecomposer` to break down a structured query into sub-queries for each model,
//! execute them in parallel, and fuse the results.
//!
//! ```json
//! {
//!   "vector": { "collection": "embeddings", "query": [0.1, 0.2], "top_k": 10 },
//!   "graph": { "cypher": "MATCH (n)-[:RELATED]->(m) RETURN m" },
//!   "document": { "collection": "docs", "filter": "type = 'article'" },
//!   "fusion_strategy": "rrf"
//! }
//! ```
//!
//! ### 2. Federated SQL Queries (`/federated`)
//!
//! Uses `FederatedQueryContext` to parse and execute SQL with multi-model extensions.
//! Supports LATERAL joins for cross-model correlation.
//!
//! ```json
//! {
//!   "query": "SELECT * FROM VECTOR_SEARCH('products', '[0.1, 0.2]', 10) v JOIN GRAPH_QUERY('MATCH (p)-[:CATEGORY]->(c) RETURN c.name') g ON v.id = g.product_id"
//! }
//! ```
//!
//! ## SQL Extensions
//!
//! - `VECTOR_SEARCH(collection, query_vector, top_k)` - Vector similarity search
//! - `GRAPH_QUERY('cypher')` - Graph traversal via Cypher
//! - `DOCUMENT_QUERY(collection, filter)` - Document queries
//! - `LOGS(namespace)` / `METRICS(namespace)` - Observability queries
//! - `<->` operator - pgvector-compatible distance operator
//!
//! ## Example cURL
//!
//! ```bash
//! # Federated SQL query
//! curl -X POST http://localhost:5678/api/v1/unified/federated \
//!   -H "Content-Type: application/json" \
//!   -d '{"query": "SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1, 0.2]', 10)"}'
//!
//! # Structured query
//! curl -X POST http://localhost:5678/api/v1/unified/query \
//!   -H "Content-Type: application/json" \
//!   -d '{"vector": {"collection": "embeddings", "query": [0.1, 0.2], "top_k": 10}}'
//! ```

use axum::{
    extract::{Json, State},
    response::Json as JsonResponse,
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::errors::{ApiError, ApiResult};
use crate::graph::service::GraphOperationsService;
use crate::observability::ObservabilityService;
use crate::query::federated::{
    FederatedQueryContext, FederatedParser, QueryType as FederatedQueryType,
};
use crate::query::unified::{
    DataModel, FusionStrategy, MultiModelQuery, QueryDecomposer, ResultFuser,
    UnifiedQueryConfig,
};
use crate::query::unified::ast::{
    DistanceMetric, DocumentQueryExpr, FilterOperator, FilterValue, GraphTraversalExpr,
    PathFilter, StartNodeSpec, TraversalDirection, VectorSearchExpr, VectorSearchParams,
};
use crate::query::unified::executor::ParallelExecutor;
use crate::query::QueryFacadeAdapter;
use crate::services::VectorOperationsService;
use crate::storage::document::DocumentService;
use crate::storage::multimodel::MultiModelStorageFacade;
use crate::storage::traits::UnifiedStorageEngine;

/// Unified Query API state with all services for cross-model queries
#[derive(Clone)]
pub struct UnifiedQueryApiState {
    /// Document service for document queries
    pub document_service: Arc<DocumentService>,
    /// Storage engine for vector queries
    pub storage_engine: Arc<dyn UnifiedStorageEngine>,
    /// Vector operations service for vector searches
    pub vector_ops: Option<Arc<VectorOperationsService>>,
    /// Graph operations service for graph traversals
    pub graph_service: Option<Arc<GraphOperationsService>>,
    /// Observability service for logs/metrics
    pub observability_service: Option<Arc<ObservabilityService>>,
    /// Query decomposer
    pub decomposer: Arc<QueryDecomposer>,
    /// Parallel executor for multi-model queries
    pub executor: Arc<ParallelExecutor>,
    /// Result fuser
    pub fuser: Arc<ResultFuser>,
    /// Configuration
    pub config: UnifiedQueryConfig,
    /// Federated query context for SQL with multi-model extensions
    pub federated_context: Option<Arc<FederatedQueryContext>>,
    /// Federated parser for detecting multi-model query patterns
    federated_parser: Arc<FederatedParser>,
    /// Query facade adapter for unified query execution (optional for feature-gated routing)
    pub query_adapter: Option<Arc<QueryFacadeAdapter>>,
}

impl UnifiedQueryApiState {
    /// Create a new unified query API state (minimal, for backwards compatibility)
    pub fn new(
        document_service: Arc<DocumentService>,
        storage_engine: Arc<dyn UnifiedStorageEngine>,
    ) -> Self {
        let config = UnifiedQueryConfig::default();
        Self {
            document_service,
            storage_engine,
            vector_ops: None,
            graph_service: None,
            observability_service: None,
            decomposer: Arc::new(QueryDecomposer::new()),
            executor: Arc::new(ParallelExecutor::new(4)), // 4 parallel queries by default
            fuser: Arc::new(ResultFuser::new(config.default_fusion.clone())),
            config,
            federated_context: None,
            federated_parser: Arc::new(FederatedParser::new()),
            query_adapter: None,
        }
    }

    /// Create a new unified query API state with all services
    pub fn new_with_services(
        document_service: Arc<DocumentService>,
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        vector_ops: Option<Arc<VectorOperationsService>>,
        graph_service: Option<Arc<GraphOperationsService>>,
        observability_service: Option<Arc<ObservabilityService>>,
    ) -> Self {
        let config = UnifiedQueryConfig::default();
        Self {
            document_service,
            storage_engine,
            vector_ops,
            graph_service,
            observability_service,
            decomposer: Arc::new(QueryDecomposer::new()),
            executor: Arc::new(ParallelExecutor::new(4)),
            fuser: Arc::new(ResultFuser::new(config.default_fusion.clone())),
            config,
            federated_context: None,
            federated_parser: Arc::new(FederatedParser::new()),
            query_adapter: None,
        }
    }

    /// Create a new unified query API state with federated query context
    ///
    /// This enables SQL with multi-model extensions like:
    /// - VECTOR_SEARCH('collection', vector, top_k)
    /// - GRAPH_QUERY('MATCH (a)-[:KNOWS]->(b) RETURN b')
    /// - DOCUMENT_QUERY('collection', filter)
    /// - LOGS('namespace'), METRICS('namespace')
    /// - pgvector-compatible <-> operator
    pub fn new_with_federated(
        document_service: Arc<DocumentService>,
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        vector_ops: Option<Arc<VectorOperationsService>>,
        graph_service: Option<Arc<GraphOperationsService>>,
        observability_service: Option<Arc<ObservabilityService>>,
        multimodel_storage: Arc<MultiModelStorageFacade>,
    ) -> Self {
        let config = UnifiedQueryConfig::default();
        let federated_context = Arc::new(FederatedQueryContext::new(multimodel_storage));

        Self {
            document_service,
            storage_engine,
            vector_ops,
            graph_service,
            observability_service,
            decomposer: Arc::new(QueryDecomposer::new()),
            executor: Arc::new(ParallelExecutor::new(4)),
            fuser: Arc::new(ResultFuser::new(config.default_fusion.clone())),
            config,
            federated_context: Some(federated_context),
            federated_parser: Arc::new(FederatedParser::new()),
            query_adapter: None,
        }
    }

    /// Set the federated query context after construction
    pub fn with_federated_context(mut self, context: Arc<FederatedQueryContext>) -> Self {
        self.federated_context = Some(context);
        self
    }

    /// Set the query facade adapter for unified query execution
    ///
    /// When set, queries will route through the unified facade instead of
    /// using the internal decomposer/executor pipeline.
    pub fn with_query_adapter(mut self, adapter: Arc<QueryFacadeAdapter>) -> Self {
        self.query_adapter = Some(adapter);
        self
    }

    /// Check if a query should be routed to the federated query engine
    fn should_use_federated(&self, query: &str) -> bool {
        // Check for multi-model SQL extensions
        let query_upper = query.to_uppercase();
        query_upper.contains("VECTOR_SEARCH")
            || query_upper.contains("GRAPH_QUERY")
            || query_upper.contains("DOCUMENT_QUERY")
            || query_upper.contains("LOGS(")
            || query_upper.contains("METRICS(")
            || query.contains("<->")  // pgvector distance operator
            || query.contains("::vector")  // pgvector type cast
    }
}

/// Execute unified query request
#[derive(Debug, Deserialize)]
pub struct ExecuteQueryRequest {
    /// SQL-like query string (e.g., "SELECT * FROM products WHERE $.category = 'electronics' AND VECTOR_SIMILAR(embedding, ?, 0.8)")
    pub query: String,
    /// Query vector (for VECTOR_SIMILAR clauses)
    #[serde(default)]
    pub query_vector: Option<Vec<f32>>,
    /// Fusion strategy override
    #[serde(default)]
    pub fusion_strategy: Option<String>,
    /// Maximum results
    #[serde(default = "default_limit")]
    pub limit: Option<u32>,
}

fn default_limit() -> Option<u32> {
    Some(100)
}

/// Multi-model query request (programmatic API)
#[derive(Debug, Deserialize)]
pub struct MultiModelQueryRequest {
    /// Components of the query
    pub components: Vec<QueryComponentRequest>,
    /// Fusion strategy
    #[serde(default = "default_fusion")]
    pub fusion_strategy: String,
    /// Maximum results
    #[serde(default = "default_limit")]
    pub limit: Option<u32>,
}

fn default_fusion() -> String {
    "intersection".to_string()
}

/// Single query component
#[derive(Debug, Deserialize)]
pub struct QueryComponentRequest {
    /// Component type: "vector", "document", "graph", "log", "metric"
    pub component_type: String,
    /// Component-specific configuration
    pub config: serde_json::Value,
}

/// Query result response
#[derive(Debug, Serialize)]
pub struct QueryResultResponse {
    /// Result records
    pub records: Vec<UnifiedRecordResponse>,
    /// Total count (if available)
    pub total_count: Option<u64>,
    /// Number of records returned in this response
    pub records_returned: u64,
    /// Execution metrics
    pub metrics: QueryMetricsResponse,
}

/// Single unified record
#[derive(Debug, Serialize)]
pub struct UnifiedRecordResponse {
    /// Record ID
    pub id: String,
    /// Source model (vector, document, graph, observability)
    pub source_model: String,
    /// Record data
    pub data: serde_json::Value,
    /// Relevance score (if applicable)
    pub score: Option<f64>,
    /// Additional metadata
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Query execution metrics
#[derive(Debug, Serialize)]
pub struct QueryMetricsResponse {
    /// Total execution time in milliseconds
    pub total_time_ms: f64,
    /// Time per sub-query
    pub sub_query_times: Vec<SubQueryTimeResponse>,
    /// Number of records scanned
    pub records_scanned: u64,
    /// Number of records returned
    pub records_returned: u64,
}

/// Sub-query timing info
#[derive(Debug, Serialize)]
pub struct SubQueryTimeResponse {
    /// Model type
    pub model: String,
    /// Execution time in milliseconds
    pub time_ms: f64,
}

/// Query explain response
#[derive(Debug, Serialize)]
pub struct ExplainResponse {
    /// Component plans
    pub components: Vec<ComponentPlanResponse>,
    /// Fusion strategy
    pub fusion_strategy: String,
    /// Estimated total cost
    pub estimated_total_cost: f64,
}

/// Component plan info
#[derive(Debug, Serialize)]
pub struct ComponentPlanResponse {
    /// Data model
    pub model: String,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Whether parallelizable
    pub parallelizable: bool,
}

/// Create router for unified query endpoints
pub fn create_router() -> Router<UnifiedQueryApiState> {
    Router::new()
        .route("/execute", post(execute_query))
        .route("/multi-model", post(execute_multi_model_query))
        .route("/federated", post(execute_federated_query))
        .route("/explain", post(explain_query))
}

/// Execute a unified SQL-like query
///
/// POST /api/v1/unified/execute
///
/// This endpoint intelligently routes queries based on content:
/// - Queries with multi-model extensions (VECTOR_SEARCH, GRAPH_QUERY, etc.) are routed to FederatedQueryContext
/// - Standard SQL queries use the existing QueryDecomposer
///
/// Request body:
/// ```json
/// {
///   "query": "SELECT * FROM products WHERE $.category = 'electronics' AND VECTOR_SIMILAR(embedding, ?, 0.8)",
///   "query_vector": [0.1, 0.2, ...],
///   "fusion_strategy": "intersection",
///   "limit": 100
/// }
/// ```
///
/// ## Multi-Model SQL Extensions (routed to federated engine)
///
/// - `VECTOR_SEARCH('collection', vector, top_k)` - Vector similarity search
/// - `GRAPH_QUERY('MATCH (a)-[:KNOWS]->(b) RETURN b')` - Graph traversal via Cypher
/// - `DOCUMENT_QUERY('collection', filter)` - Document queries
/// - `LOGS('namespace')`, `METRICS('namespace')` - Observability queries
/// - `embedding <-> '[0.1,0.2]'::vector` - pgvector-compatible distance operator
async fn execute_query(
    State(state): State<UnifiedQueryApiState>,
    Json(request): Json<ExecuteQueryRequest>,
) -> ApiResult<JsonResponse<QueryResultResponse>> {
    use std::time::Instant;

    info!("Executing unified query: {}", request.query);
    let start = Instant::now();

    // Check if query should use federated engine
    if state.should_use_federated(&request.query) {
        debug!("Routing query to federated engine");
        return execute_federated_query_internal(&state, &request, start).await;
    }

    // Standard query path using QueryDecomposer
    debug!("Using standard query decomposer");

    // Decompose the query
    let multi_model_query = match state.decomposer.decompose(&request.query) {
        Ok(q) => q,
        Err(e) => {
            warn!("Query decomposition failed: {}", e);
            return Err(ApiError::InvalidArgument(format!("Invalid query: {}", e)));
        }
    };

    debug!("Decomposed into {} components", multi_model_query.components.len());

    // Execute using the parallel executor with all available services
    let sub_results = match state.executor.execute_parallel_with_all_services(
        &multi_model_query,
        state.vector_ops.clone(),
        state.document_service.clone(),
        state.graph_service.clone(),
        state.observability_service.clone(),
    ).await {
        Ok(results) => results,
        Err(e) => {
            warn!("Query execution failed: {}", e);
            return Err(ApiError::Internal(format!("Execution failed: {}", e)));
        }
    };

    // Determine fusion strategy
    let fusion_strategy = request.fusion_strategy
        .as_ref()
        .map(|s| parse_fusion_strategy(s))
        .unwrap_or_else(|| state.config.default_fusion.clone());

    // Fuse results
    let fused = match state.fuser.fuse(sub_results, &fusion_strategy) {
        Ok(result) => result,
        Err(e) => {
            warn!("Result fusion failed: {}", e);
            return Err(ApiError::Internal(format!("Fusion failed: {}", e)));
        }
    };

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Apply limit if specified
    let records: Vec<_> = fused.records.into_iter()
        .take(request.limit.unwrap_or(100) as usize)
        .map(|r| UnifiedRecordResponse {
            id: r.id,
            source_model: format!("{:?}", r.source_model),
            data: r.data,
            score: r.score,
            metadata: r.metadata,
        })
        .collect();

    let response = QueryResultResponse {
        total_count: fused.total_count,
        records_returned: records.len() as u64,
        records,
        metrics: QueryMetricsResponse {
            total_time_ms: elapsed_ms,
            sub_query_times: fused.metrics.sub_query_times.iter().map(|(model, time_us)| {
                SubQueryTimeResponse {
                    model: format!("{:?}", model),
                    time_ms: *time_us as f64 / 1000.0,
                }
            }).collect(),
            records_scanned: fused.metrics.records_scanned,
            records_returned: fused.metrics.records_returned,
        },
    };

    info!("Query executed in {:.2}ms, returned {} records", elapsed_ms, response.records.len());
    Ok(JsonResponse(response))
}

/// Execute a programmatic multi-model query
///
/// POST /api/v1/unified/multi-model
///
/// Request body:
/// ```json
/// {
///   "components": [
///     {
///       "component_type": "vector",
///       "config": {
///         "collection": "products",
///         "query_vector": [0.1, 0.2, ...],
///         "top_k": 10,
///         "threshold": 0.8
///       }
///     },
///     {
///       "component_type": "document",
///       "config": {
///         "collection": "products",
///         "filter": "$.category = 'electronics'"
///       }
///     }
///   ],
///   "fusion_strategy": "intersection",
///   "limit": 100
/// }
/// ```
async fn execute_multi_model_query(
    State(state): State<UnifiedQueryApiState>,
    Json(request): Json<MultiModelQueryRequest>,
) -> ApiResult<JsonResponse<QueryResultResponse>> {
    use std::time::Instant;
    use crate::query::unified::ast::{ModelOperation, QueryComponent};

    info!("Executing multi-model query with {} components", request.components.len());
    let start = Instant::now();

    // Parse fusion strategy
    let fusion_strategy = parse_fusion_strategy(&request.fusion_strategy);

    // Build query components from request
    let mut components = Vec::new();
    for component in &request.components {
        let model = match component.component_type.as_str() {
            "vector" => DataModel::Vector,
            "document" => DataModel::Document,
            "graph" => DataModel::Graph,
            "log" | "metric" => DataModel::Observability,
            _ => {
                return Err(ApiError::InvalidArgument(format!(
                    "Unknown component type: {}",
                    component.component_type
                )));
            }
        };

        // Parse component config based on type
        let operation = match component.component_type.as_str() {
            "vector" => {
                let collection = component.config.get("collection")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                let query_vector = component.config.get("query_vector")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_f64()).map(|f| f as f32).collect())
                    .unwrap_or_default();
                let top_k = component.config.get("top_k")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10) as u32;
                let threshold = component.config.get("threshold")
                    .and_then(|v| v.as_f64())
                    .map(|f| f as f32);

                ModelOperation::VectorSearch(VectorSearchExpr {
                    collection,
                    query_vector,
                    top_k,
                    threshold,
                    metric: DistanceMetric::Euclidean,
                    params: VectorSearchParams::default(),
                })
            }
            "document" => {
                let collection = component.config.get("collection")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                let filter_str = component.config.get("filter")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let path_filters = parse_path_filters(filter_str);

                ModelOperation::DocumentQuery(DocumentQueryExpr {
                    collection,
                    path_filters,
                    text_search: None,
                    projection: Vec::new(),
                    sort: None,
                    limit: request.limit,
                })
            }
            "graph" => {
                let graph_name = component.config.get("graph")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                let start_node = component.config.get("start_node")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let edge_types: Vec<String> = component.config.get("edge_type")
                    .and_then(|v| v.as_str())
                    .map(|s| vec![s.to_string()])
                    .unwrap_or_default();
                let max_depth = component.config.get("max_depth")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(2) as u32;

                ModelOperation::GraphTraversal(GraphTraversalExpr {
                    graph_name,
                    start_nodes: StartNodeSpec::Ids(vec![start_node]),
                    edge_types,
                    direction: TraversalDirection::Outgoing,
                    max_depth,
                    min_depth: 0,
                    node_filters: Vec::new(),
                    edge_filters: Vec::new(),
                    return_paths: false,
                })
            }
            _ => continue, // Skip unknown types
        };

        components.push(QueryComponent {
            model,
            operation,
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
    }

    // Build multi-model query
    let multi_model_query = MultiModelQuery {
        components,
        fusion_strategy: fusion_strategy.clone(),
        limit: request.limit,
        offset: None,
        projection: Vec::new(),
        order_by: None,
    };

    // Execute using parallel executor
    let sub_results = match state.executor.execute_parallel_with_all_services(
        &multi_model_query,
        state.vector_ops.clone(),
        state.document_service.clone(),
        state.graph_service.clone(),
        state.observability_service.clone(),
    ).await {
        Ok(results) => results,
        Err(e) => {
            warn!("Query execution failed: {}", e);
            return Err(ApiError::Internal(format!("Execution failed: {}", e)));
        }
    };

    // Fuse results
    let fused = match state.fuser.fuse(sub_results, &fusion_strategy) {
        Ok(result) => result,
        Err(e) => {
            warn!("Result fusion failed: {}", e);
            return Err(ApiError::Internal(format!("Fusion failed: {}", e)));
        }
    };

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Apply limit if specified
    let records: Vec<_> = fused.records.into_iter()
        .take(request.limit.unwrap_or(100) as usize)
        .map(|r| UnifiedRecordResponse {
            id: r.id,
            source_model: format!("{:?}", r.source_model),
            data: r.data,
            score: r.score,
            metadata: r.metadata,
        })
        .collect();

    let response = QueryResultResponse {
        total_count: fused.total_count,
        records_returned: records.len() as u64,
        records,
        metrics: QueryMetricsResponse {
            total_time_ms: elapsed_ms,
            sub_query_times: fused.metrics.sub_query_times.iter().map(|(model, time_us)| {
                SubQueryTimeResponse {
                    model: format!("{:?}", model),
                    time_ms: *time_us as f64 / 1000.0,
                }
            }).collect(),
            records_scanned: fused.metrics.records_scanned,
            records_returned: fused.metrics.records_returned,
        },
    };

    info!("Multi-model query executed in {:.2}ms, returned {} records", elapsed_ms, response.records.len());
    Ok(JsonResponse(response))
}

/// Parse path filters from a simple filter string (e.g., "$.category = 'electronics'")
fn parse_path_filters(filter_str: &str) -> Vec<PathFilter> {
    if filter_str.is_empty() {
        return Vec::new();
    }

    // Simple parser for path filters
    let mut filters = Vec::new();
    for part in filter_str.split(" AND ") {
        let part = part.trim();
        if let Some((path, rest)) = part.split_once('=') {
            let path = path.trim().trim_start_matches("$.").to_string();
            let value = rest.trim().trim_matches('\'').to_string();
            filters.push(PathFilter {
                path,
                operator: FilterOperator::Eq, // Eq for equals
                value: FilterValue::String(value),
            });
        }
    }
    filters
}

/// Explain a query's execution plan
///
/// POST /api/v1/unified/explain
///
/// Request body:
/// ```json
/// {
///   "query": "SELECT * FROM products WHERE VECTOR_SIMILAR(embedding, ?, 0.8)"
/// }
/// ```
async fn explain_query(
    State(state): State<UnifiedQueryApiState>,
    Json(request): Json<ExecuteQueryRequest>,
) -> ApiResult<JsonResponse<ExplainResponse>> {
    info!("Explaining query: {}", request.query);

    // Decompose the query
    let multi_model_query = match state.decomposer.decompose(&request.query) {
        Ok(q) => q,
        Err(e) => {
            warn!("Query decomposition failed: {}", e);
            return Err(ApiError::InvalidArgument(format!("Invalid query: {}", e)));
        }
    };

    // Build explain response
    let response = ExplainResponse {
        components: multi_model_query.components.iter().map(|c| {
            ComponentPlanResponse {
                model: format!("{:?}", c.model),
                estimated_cost: estimate_cost(&c.model),
                parallelizable: c.is_parallelizable(),
            }
        }).collect(),
        fusion_strategy: format!("{:?}", multi_model_query.fusion_strategy),
        estimated_total_cost: multi_model_query.components.iter()
            .map(|c| estimate_cost(&c.model))
            .fold(0.0, f64::max), // Max for parallel execution
    };

    Ok(JsonResponse(response))
}

/// Parse fusion strategy from string
fn parse_fusion_strategy(s: &str) -> FusionStrategy {
    match s.to_lowercase().as_str() {
        "intersection" | "and" => FusionStrategy::Intersection,
        "union" | "or" => FusionStrategy::Union,
        "rrf" | "reciprocal_rank_fusion" => FusionStrategy::ReciprocalRankFusion { k: 60 },
        "ranked" | "weighted" => FusionStrategy::RankedFusion {
            weights: HashMap::new(),
            normalize: true,
        },
        _ => FusionStrategy::Intersection,
    }
}

/// Estimate cost for a data model
fn estimate_cost(model: &DataModel) -> f64 {
    match model {
        DataModel::Vector => 1.0,
        DataModel::Document => 2.0,
        DataModel::Graph => 3.0,
        DataModel::Observability => 2.5,
    }
}

/// Execute a federated SQL query with multi-model extensions
///
/// POST /api/v1/unified/federated
///
/// This endpoint explicitly routes queries through the FederatedQueryContext,
/// supporting PostgreSQL-compatible SQL with multi-model extensions.
///
/// ## Request body:
/// ```json
/// {
///   "query": "SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1,0.2]', 10)",
///   "query_vector": [0.1, 0.2, ...],
///   "limit": 100
/// }
/// ```
///
/// ## Supported SQL Extensions:
///
/// ### Vector Search (pgvector compatible)
/// ```sql
/// SELECT * FROM products ORDER BY embedding <-> '[0.1,0.2,...]'::vector LIMIT 10;
/// SELECT * FROM VECTOR_SEARCH('embeddings', query_vector, 10);
/// ```
///
/// ### Graph Traversal (embedded Cypher)
/// ```sql
/// SELECT * FROM GRAPH_QUERY('MATCH (a)-[:KNOWS]->(b) RETURN b.name');
/// ```
///
/// ### Document Query
/// ```sql
/// SELECT * FROM DOCUMENT_QUERY('products', '$.price > 100');
/// ```
///
/// ### Cross-Model Joins
/// ```sql
/// SELECT u.*, v.similar_products
/// FROM users u
/// JOIN LATERAL VECTOR_SEARCH('embeddings', u.preference_vector, 10) v ON true;
/// ```
///
/// ### Observability Queries
/// ```sql
/// SELECT * FROM LOGS('production') WHERE timestamp > now() - interval '1h';
/// SELECT * FROM METRICS('cpu') WHERE value > 90;
/// ```
async fn execute_federated_query(
    State(state): State<UnifiedQueryApiState>,
    Json(request): Json<ExecuteQueryRequest>,
) -> ApiResult<JsonResponse<FederatedQueryResponse>> {
    use std::time::Instant;

    info!("Executing federated query: {}", request.query);
    let start = Instant::now();

    // When unified-facade-routing is enabled and adapter is available, use it
    #[cfg(feature = "unified-facade-routing")]
    if let Some(ref adapter) = state.query_adapter {
        debug!("Routing federated query through QueryFacadeAdapter");
        return execute_federated_via_adapter(adapter, &request, start).await;
    }

    // Use the federated query context if available
    let federated_context = match &state.federated_context {
        Some(ctx) => ctx.clone(),
        None => {
            warn!("Federated query context not configured, falling back to standard execution");
            // Convert to standard query response format
            let standard_result = execute_federated_query_internal(&state, &request, start).await?;
            return Ok(JsonResponse(FederatedQueryResponse {
                records: standard_result.0.records.into_iter().map(|r| FederatedRecordResponse {
                    id: r.id,
                    source_model: r.source_model,
                    data: r.data,
                    score: r.score,
                    metadata: r.metadata,
                }).collect(),
                total_count: standard_result.0.total_count,
                records_returned: standard_result.0.records_returned,
                query_type: "fallback".to_string(),
                involved_models: vec!["unknown".to_string()],
                execution_plan: None,
                metrics: FederatedMetricsResponse {
                    total_time_ms: standard_result.0.metrics.total_time_ms,
                    parse_time_ms: 0.0,
                    optimize_time_ms: 0.0,
                    execute_time_ms: standard_result.0.metrics.total_time_ms,
                    sub_query_times: standard_result.0.metrics.sub_query_times.into_iter()
                        .map(|s| SubQueryTimeResponse { model: s.model, time_ms: s.time_ms })
                        .collect(),
                    rows_scanned: standard_result.0.metrics.records_scanned,
                },
            }));
        }
    };

    // Parse the query to determine its type
    let parsed_query = match federated_context.parser.parse(&request.query) {
        Ok(q) => q,
        Err(e) => {
            warn!("Federated query parsing failed: {}", e);
            return Err(ApiError::InvalidArgument(format!("Invalid query: {}", e)));
        }
    };

    let parse_time = start.elapsed().as_secs_f64() * 1000.0;
    debug!("Parsed query type: {:?}, extensions: {:?}", parsed_query.query_type, parsed_query.extensions.len());

    // Execute the query through the federated context
    let execution_result = match federated_context.execute(&request.query).await {
        Ok(result) => result,
        Err(e) => {
            warn!("Federated query execution failed: {}", e);
            return Err(ApiError::Internal(format!("Execution failed: {}", e)));
        }
    };

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Convert Arrow batches to JSON records
    let records = convert_arrow_to_records(&execution_result, request.limit.unwrap_or(100) as usize);

    // Build response with detailed execution info
    let response = FederatedQueryResponse {
        records_returned: records.len() as u64,
        total_count: Some(execution_result.row_count() as u64),
        records,
        query_type: format!("{:?}", parsed_query.query_type),
        involved_models: execution_result.stats.models_queried.iter()
            .map(|m| format!("{:?}", m))
            .collect(),
        execution_plan: Some(FederatedPlanResponse {
            is_cross_model: parsed_query.is_cross_model_join,
            extensions_used: parsed_query.extensions.iter()
                .map(|e| format!("{:?}", e))
                .collect(),
        }),
        metrics: FederatedMetricsResponse {
            total_time_ms: elapsed_ms,
            parse_time_ms: parse_time,
            optimize_time_ms: 0.0, // TODO: Track separately
            execute_time_ms: execution_result.stats.execution_time_us as f64 / 1000.0,
            sub_query_times: vec![],
            rows_scanned: execution_result.stats.bytes_scanned / 100, // Estimate
        },
    };

    info!(
        "Federated query executed in {:.2}ms, type={:?}, returned {} records",
        elapsed_ms, parsed_query.query_type, response.records_returned
    );

    Ok(JsonResponse(response))
}

/// Internal function to execute federated query through FederatedQueryContext
async fn execute_federated_query_internal(
    state: &UnifiedQueryApiState,
    request: &ExecuteQueryRequest,
    start: std::time::Instant,
) -> ApiResult<JsonResponse<QueryResultResponse>> {
    // Try to use federated context if available
    if let Some(ref federated_context) = state.federated_context {
        // Parse to get query metadata
        let parsed_query = match federated_context.parser.parse(&request.query) {
            Ok(q) => q,
            Err(e) => {
                warn!("Failed to parse query for federated execution: {}", e);
                return Err(ApiError::InvalidArgument(format!("Invalid query: {}", e)));
            }
        };

        debug!("Federated query type: {:?}", parsed_query.query_type);

        // Execute through federated context
        let execution_result = match federated_context.execute(&request.query).await {
            Ok(result) => result,
            Err(e) => {
                warn!("Federated execution failed: {}", e);
                return Err(ApiError::Internal(format!("Federated execution failed: {}", e)));
            }
        };

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        // Convert Arrow results to unified record format
        let records = convert_arrow_to_unified_records(
            &execution_result,
            &parsed_query.query_type,
            request.limit.unwrap_or(100) as usize,
        );

        let response = QueryResultResponse {
            total_count: Some(execution_result.row_count() as u64),
            records_returned: records.len() as u64,
            records,
            metrics: QueryMetricsResponse {
                total_time_ms: elapsed_ms,
                sub_query_times: execution_result.stats.models_queried.iter().map(|model| {
                    SubQueryTimeResponse {
                        model: format!("{:?}", model),
                        time_ms: execution_result.stats.execution_time_us as f64 / 1000.0 / execution_result.stats.models_queried.len() as f64,
                    }
                }).collect(),
                records_scanned: execution_result.stats.rows_produced as u64,
                records_returned: execution_result.row_count() as u64,
            },
        };

        return Ok(JsonResponse(response));
    }

    // Fallback: no federated context, return error
    Err(ApiError::Internal("Federated query context not configured".to_string()))
}

/// Convert Arrow RecordBatches to JSON records for the federated response
fn convert_arrow_to_records(
    result: &crate::query::federated::ExecutionResult,
    limit: usize,
) -> Vec<FederatedRecordResponse> {
    let mut records = Vec::new();
    let mut count = 0;

    for batch in &result.batches {
        if count >= limit {
            break;
        }

        let schema = batch.schema();
        for row_idx in 0..batch.num_rows() {
            if count >= limit {
                break;
            }

            let mut data = serde_json::Map::new();
            let mut id = format!("row_{}", count);
            let mut score: Option<f64> = None;

            for (col_idx, field) in schema.fields().iter().enumerate() {
                let column = batch.column(col_idx);
                let value = extract_value_from_array(column.as_ref(), row_idx);

                // Extract special fields
                if field.name() == "id" {
                    if let serde_json::Value::String(s) = &value {
                        id = s.clone();
                    }
                } else if field.name() == "score" || field.name() == "distance" {
                    if let serde_json::Value::Number(n) = &value {
                        score = n.as_f64();
                    }
                }

                data.insert(field.name().clone(), value);
            }

            records.push(FederatedRecordResponse {
                id,
                source_model: detect_source_model(&schema),
                data: serde_json::Value::Object(data),
                score,
                metadata: HashMap::new(),
            });

            count += 1;
        }
    }

    records
}

/// Convert Arrow RecordBatches to unified record format
fn convert_arrow_to_unified_records(
    result: &crate::query::federated::ExecutionResult,
    _query_type: &FederatedQueryType,
    limit: usize,
) -> Vec<UnifiedRecordResponse> {
    let federated_records = convert_arrow_to_records(result, limit);

    federated_records.into_iter().map(|r| {
        UnifiedRecordResponse {
            id: r.id,
            source_model: r.source_model,
            data: r.data,
            score: r.score,
            metadata: r.metadata,
        }
    }).collect()
}

/// Extract a JSON value from an Arrow array at the given row index
fn extract_value_from_array(array: &dyn arrow::array::Array, row_idx: usize) -> serde_json::Value {
    use arrow::array::{StringArray, Float32Array, Float64Array, Int32Array, Int64Array, BooleanArray};
    use arrow::datatypes::DataType;

    if array.is_null(row_idx) {
        return serde_json::Value::Null;
    }

    match array.data_type() {
        DataType::Utf8 => {
            let arr = array.as_any().downcast_ref::<StringArray>().unwrap();
            serde_json::Value::String(arr.value(row_idx).to_string())
        }
        DataType::Float32 => {
            let arr = array.as_any().downcast_ref::<Float32Array>().unwrap();
            serde_json::json!(arr.value(row_idx))
        }
        DataType::Float64 => {
            let arr = array.as_any().downcast_ref::<Float64Array>().unwrap();
            serde_json::json!(arr.value(row_idx))
        }
        DataType::Int32 => {
            let arr = array.as_any().downcast_ref::<Int32Array>().unwrap();
            serde_json::json!(arr.value(row_idx))
        }
        DataType::Int64 => {
            let arr = array.as_any().downcast_ref::<Int64Array>().unwrap();
            serde_json::json!(arr.value(row_idx))
        }
        DataType::Boolean => {
            let arr = array.as_any().downcast_ref::<BooleanArray>().unwrap();
            serde_json::json!(arr.value(row_idx))
        }
        _ => serde_json::Value::String(format!("<unsupported type: {:?}>", array.data_type())),
    }
}

/// Detect the source model from the schema
fn detect_source_model(schema: &arrow::datatypes::Schema) -> String {
    let field_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();

    if field_names.contains(&"score") || field_names.contains(&"embedding") || field_names.contains(&"vector") {
        "Vector".to_string()
    } else if field_names.contains(&"node_id") || field_names.contains(&"edge_id") || field_names.contains(&"label") {
        "Graph".to_string()
    } else if field_names.contains(&"document") || field_names.contains(&"doc") {
        "Document".to_string()
    } else if field_names.contains(&"timestamp") && (field_names.contains(&"level") || field_names.contains(&"metric_name")) {
        "Observability".to_string()
    } else {
        "Relational".to_string()
    }
}

/// Response for federated query execution
#[derive(Debug, Serialize)]
pub struct FederatedQueryResponse {
    /// Result records
    pub records: Vec<FederatedRecordResponse>,
    /// Total count (if available)
    pub total_count: Option<u64>,
    /// Number of records returned in this response
    pub records_returned: u64,
    /// Query type detected
    pub query_type: String,
    /// Models involved in the query
    pub involved_models: Vec<String>,
    /// Execution plan details
    pub execution_plan: Option<FederatedPlanResponse>,
    /// Execution metrics
    pub metrics: FederatedMetricsResponse,
}

/// Single federated record
#[derive(Debug, Serialize)]
pub struct FederatedRecordResponse {
    /// Record ID
    pub id: String,
    /// Source model (vector, document, graph, observability, relational)
    pub source_model: String,
    /// Record data
    pub data: serde_json::Value,
    /// Relevance score (if applicable)
    pub score: Option<f64>,
    /// Additional metadata
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Federated execution plan response
#[derive(Debug, Serialize)]
pub struct FederatedPlanResponse {
    /// Whether this is a cross-model join
    pub is_cross_model: bool,
    /// SQL extensions used
    pub extensions_used: Vec<String>,
}

/// Federated execution metrics
#[derive(Debug, Serialize)]
pub struct FederatedMetricsResponse {
    /// Total execution time in milliseconds
    pub total_time_ms: f64,
    /// Parse time in milliseconds
    pub parse_time_ms: f64,
    /// Optimization time in milliseconds
    pub optimize_time_ms: f64,
    /// Execution time in milliseconds
    pub execute_time_ms: f64,
    /// Time per sub-query
    pub sub_query_times: Vec<SubQueryTimeResponse>,
    /// Rows scanned
    pub rows_scanned: u64,
}

/// Execute federated query through the unified QueryFacadeAdapter
///
/// This function routes the query through the facade for consistent execution
/// across all query paths (REST, gRPC, internal).
#[cfg(feature = "unified-facade-routing")]
async fn execute_federated_via_adapter(
    adapter: &QueryFacadeAdapter,
    request: &ExecuteQueryRequest,
    start: std::time::Instant,
) -> ApiResult<JsonResponse<FederatedQueryResponse>> {
    use crate::query::facade::QueryResultData;

    let result = adapter.federated_query(&request.query)
        .await
        .map_err(|e| ApiError::Internal(format!("Federated query failed: {}", e)))?;

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Convert QueryResult to FederatedQueryResponse
    let records = match result.data {
        QueryResultData::Rows(rows) => {
            rows.into_iter()
                .take(request.limit.unwrap_or(100) as usize)
                .enumerate()
                .map(|(i, row)| FederatedRecordResponse {
                    id: format!("row_{}", i),
                    source_model: "unified".to_string(),
                    data: row,
                    score: None,
                    metadata: HashMap::new(),
                })
                .collect()
        }
        QueryResultData::VectorResults(matches) => {
            matches.into_iter()
                .take(request.limit.unwrap_or(100) as usize)
                .map(|m| {
                    // Convert metadata from Value to HashMap<String, String>
                    let metadata = m.metadata
                        .and_then(|v| v.as_object().cloned())
                        .map(|obj| {
                            obj.into_iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                                .collect()
                        })
                        .unwrap_or_default();
                    FederatedRecordResponse {
                        id: m.id,
                        source_model: "vector".to_string(),
                        data: serde_json::json!({ "score": m.score }),
                        score: Some(m.score as f64),
                        metadata,
                    }
                })
                .collect()
        }
        QueryResultData::Graph(graph_result) => {
            graph_result.nodes.into_iter()
                .take(request.limit.unwrap_or(100) as usize)
                .enumerate()
                .map(|(i, node)| FederatedRecordResponse {
                    id: format!("node_{}", i),
                    source_model: "graph".to_string(),
                    data: node,
                    score: None,
                    metadata: HashMap::new(),
                })
                .collect()
        }
        QueryResultData::Empty => vec![],
    };

    let metrics_info = result.metrics.unwrap_or_default();

    let response = FederatedQueryResponse {
        records_returned: records.len() as u64,
        total_count: Some(records.len() as u64),
        records,
        query_type: "unified".to_string(),
        involved_models: vec!["unified".to_string()],
        execution_plan: Some(FederatedPlanResponse {
            is_cross_model: false,
            extensions_used: vec![],
        }),
        metrics: FederatedMetricsResponse {
            total_time_ms: elapsed_ms,
            parse_time_ms: metrics_info.planning_time_ms as f64,
            optimize_time_ms: 0.0,
            execute_time_ms: metrics_info.execution_time_ms as f64,
            sub_query_times: vec![],
            rows_scanned: 0,
        },
    };

    info!(
        "Federated query via adapter in {:.2}ms, returned {} records",
        elapsed_ms, response.records_returned
    );

    Ok(JsonResponse(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fusion_strategy() {
        assert!(matches!(parse_fusion_strategy("intersection"), FusionStrategy::Intersection));
        assert!(matches!(parse_fusion_strategy("union"), FusionStrategy::Union));
        assert!(matches!(parse_fusion_strategy("rrf"), FusionStrategy::ReciprocalRankFusion { .. }));
        assert!(matches!(parse_fusion_strategy("ranked"), FusionStrategy::RankedFusion { .. }));
        assert!(matches!(parse_fusion_strategy("unknown"), FusionStrategy::Intersection));
    }

    #[test]
    fn test_estimate_cost() {
        assert_eq!(estimate_cost(&DataModel::Vector), 1.0);
        assert_eq!(estimate_cost(&DataModel::Document), 2.0);
        assert_eq!(estimate_cost(&DataModel::Graph), 3.0);
    }

    #[test]
    fn test_should_use_federated_detection() {
        // Test the federated detection logic directly using a helper function
        // This avoids the need for complex mock setup

        // VECTOR_SEARCH should use federated
        assert!(should_use_federated_pattern("SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1,0.2]', 10)"));

        // pgvector distance operator should use federated
        assert!(should_use_federated_pattern("SELECT * FROM products ORDER BY embedding <-> '[0.1,0.2]' LIMIT 10"));

        // pgvector type cast should use federated
        assert!(should_use_federated_pattern("SELECT * FROM products ORDER BY embedding <-> '[0.1]'::vector LIMIT 10"));

        // GRAPH_QUERY should use federated
        assert!(should_use_federated_pattern("SELECT * FROM GRAPH_QUERY('MATCH (a)-[:KNOWS]->(b) RETURN b.name')"));

        // DOCUMENT_QUERY should use federated
        assert!(should_use_federated_pattern("SELECT * FROM DOCUMENT_QUERY('products', '$.price > 100')"));

        // LOGS should use federated
        assert!(should_use_federated_pattern("SELECT * FROM LOGS('production') WHERE timestamp > now() - interval '1h'"));

        // METRICS should use federated
        assert!(should_use_federated_pattern("SELECT * FROM METRICS('cpu') WHERE value > 90"));

        // Standard SQL should not use federated
        assert!(!should_use_federated_pattern("SELECT * FROM users WHERE id = 1"));
        assert!(!should_use_federated_pattern("SELECT u.name, o.total FROM users u JOIN orders o ON u.id = o.user_id"));
        assert!(!should_use_federated_pattern("INSERT INTO users (name) VALUES ('test')"));
    }

    /// Helper function to test federated detection without needing full state
    fn should_use_federated_pattern(query: &str) -> bool {
        let query_upper = query.to_uppercase();
        query_upper.contains("VECTOR_SEARCH")
            || query_upper.contains("GRAPH_QUERY")
            || query_upper.contains("DOCUMENT_QUERY")
            || query_upper.contains("LOGS(")
            || query_upper.contains("METRICS(")
            || query.contains("<->")  // pgvector distance operator
            || query.contains("::vector")  // pgvector type cast
    }

    #[test]
    fn test_detect_source_model() {
        use arrow::datatypes::{Schema, Field, DataType};

        // Vector schema
        let vector_schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("score", DataType::Float32, false),
        ]);
        assert_eq!(detect_source_model(&vector_schema), "Vector");

        // Graph schema
        let graph_schema = Schema::new(vec![
            Field::new("node_id", DataType::Utf8, false),
            Field::new("label", DataType::Utf8, true),
        ]);
        assert_eq!(detect_source_model(&graph_schema), "Graph");

        // Document schema
        let doc_schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("document", DataType::Utf8, false),
        ]);
        assert_eq!(detect_source_model(&doc_schema), "Document");

        // Observability schema (logs)
        let obs_schema = Schema::new(vec![
            Field::new("timestamp", DataType::Int64, false),
            Field::new("level", DataType::Utf8, false),
            Field::new("message", DataType::Utf8, false),
        ]);
        assert_eq!(detect_source_model(&obs_schema), "Observability");

        // Relational schema (default)
        let rel_schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, true),
        ]);
        assert_eq!(detect_source_model(&rel_schema), "Relational");
    }

    #[test]
    fn test_federated_parser_creation() {
        let parser = FederatedParser::new();
        let extensions = parser.supported_extensions();
        assert!(extensions.contains(&"VECTOR_SEARCH"));
        assert!(extensions.contains(&"GRAPH_QUERY"));
        assert!(extensions.contains(&"LOGS"));
    }
}
