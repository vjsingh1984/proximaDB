//! Aligned REST API handlers using protobuf-first approach
//!
//! These handlers demonstrate the proper pattern for REST APIs that:
//! 1. Accept protobuf types directly as JSON
//! 2. Return protobuf responses as JSON
//! 3. Use unified ApiError for consistent error handling

use axum::{
    extract::{Extension, Json, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json as JsonResponse},
};
use std::sync::Arc;
#[cfg(any(feature = "ai_endpoints", feature = "sales_endpoints"))]
use tracing::warn;
use tracing::{debug, error, info};

use crate::api_handlers::UnifiedHandlers;
use crate::errors::{ApiError, ApiResult};
use crate::network::middleware::tenant::TenantContext;
use crate::network::rest::health;
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::{CollectionOperation, CollectionRequest};
use crate::proto::proximadb_v1::{VectorBatchRequest, VectorSearchRequest};
use crate::query::QueryFacadeAdapter;
use crate::query::execution::QueryEngine;
use crate::query::explain::ExplainPlan;
use crate::utils::uuid::Uuid;
use serde::{Deserialize, Serialize};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    /// Shared unified handlers for business logic delegation
    pub unified_handlers: Arc<UnifiedHandlers>,
    /// Optional security coordinator for authentication/authorization
    pub security_coordinator: Option<Arc<crate::security::SecurityCoordinator>>,
    /// Data directory from config (e.g., server.data_dir from TOML)
    pub data_dir: std::path::PathBuf,
    /// Query facade adapter for unified query execution
    /// Optional for backward compatibility during feature flag transition
    pub query_adapter: Option<Arc<QueryFacadeAdapter>>,
    /// Per-collection full-text indices for hybrid BM25+vector search
    pub fulltext_indexes: Option<FullTextIndexMap>,
    /// Catalog manager for external catalog integration
    pub catalog_manager: Arc<crate::catalog::CatalogManager>,
}

/// Parse search request from JSON, supporting both proto and simple formats
/// Proto format: { "collection_id": "...", "queries": [{"vector": [...]}], "top_k": 10 }
/// Simple format: { "collection": "...", "vector": [...], "top_k": 10 } (MVP-friendly)
fn parse_search_request(value: serde_json::Value) -> Result<VectorSearchRequest, String> {
    // Check if this is the simple format (has "collection" or "vector" at root level)
    if let Some(obj) = value.as_object() {
        let has_simple_collection = obj.contains_key("collection");
        let has_simple_vector = obj.contains_key("vector");
        let is_simple_format = has_simple_collection || has_simple_vector;

        if is_simple_format {
            // Parse as simple format and convert to proto format
            let collection_id = obj
                .get("collection")
                .or_else(|| obj.get("collection_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let vector: Vec<f32> = obj
                .get("vector")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect()
                })
                .unwrap_or_default();

            let top_k = obj.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as u32;

            // Parse optional filters from simple format
            let filters = obj
                .get("filters")
                .and_then(|v| {
                    serde_json::from_value::<
                        std::collections::HashMap<String, proximadb_v1::SqlValue>,
                    >(v.clone())
                    .ok()
                })
                .unwrap_or_default();

            // Create a single SearchQuery from the simple format
            let query = proximadb_v1::SearchQuery {
                vector,
                filters,
                advanced_filter: None,
            };

            return Ok(VectorSearchRequest {
                collection_id,
                queries: vec![query],
                top_k,
                include_fields: None,
                search_params: None,
                distance_metric_override: None,
                search_optimization: None,
            });
        }
    }

    // Fall back to proto format
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Parse batch request from JSON, supporting both proto and simple formats
fn parse_batch_request(value: serde_json::Value) -> Result<VectorBatchRequest, String> {
    // Check if this is the simple format (has "collection" at root level)
    if let Some(obj) = value.as_object() {
        let has_simple_collection = obj.contains_key("collection");

        if has_simple_collection {
            // Parse as simple format and convert to proto format
            let collection_id = obj
                .get("collection")
                .or_else(|| obj.get("collection_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Parse vectors array - already in proto-compatible format
            let vectors: Vec<proximadb_v1::VectorRecord> = obj
                .get("vectors")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            return Ok(VectorBatchRequest {
                collection_id,
                vectors,
            });
        }
    }

    // Fall back to proto format
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Aligned vector search handler
/// Accepts BOTH:
/// 1. Proto format: { "collection_id": "...", "queries": [{"vector": [...]}], "top_k": 10 }
/// 2. Simple format: { "collection": "...", "vector": [...], "top_k": 10 } (MVP-friendly)
pub async fn vector_search(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    // Try to parse as simple format first, then fall back to proto format
    let request: VectorSearchRequest = parse_search_request(value)
        .map_err(|e| ApiError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    if request.collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    // Log tenant context for audit trail
    debug!(
        "🔍 Vector search: collection='{}', tenant='{}', source='{}'",
        request.collection_id, tenant.tenant_id, tenant.source
    );

    match state
        .unified_handlers
        .handle_vector_search_v1_for_tenant(request.clone(), Some(&tenant.tenant_id))
        .await
    {
        Ok(response) => Ok(JsonResponse(response)),
        Err(e) => {
            error!(
                "❌ Vector search failed for collection '{}': {:?}",
                request.collection_id, e
            );
            error!(
                "Search request details: num_queries={}, top_k={}, has_filters={}, has_advanced_filter={}, has_search_params={}",
                request.queries.len(),
                request.top_k,
                request
                    .queries
                    .first()
                    .is_some_and(|q| !q.filters.is_empty()),
                request
                    .queries
                    .first()
                    .and_then(|q| q.advanced_filter.as_ref())
                    .is_some(),
                request.search_params.is_some()
            );
            Err(ApiError::Internal(format!("Search failed: {}", e)))
        }
    }
}

/// Aligned vector batch operation handler
/// Accepts BOTH:
/// 1. Proto format: { "collection_id": "...", "vectors": [...] }
/// 2. Simple format: { "collection": "...", "vectors": [...] } (MVP-friendly)
pub async fn vector_batch(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    // Parse the JSON value into VectorBatchRequest (supports both formats)
    let request: VectorBatchRequest = parse_batch_request(value)
        .map_err(|e| ApiError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    info!(
        "Vector batch operation for collection: {}, {} records (tenant: {})",
        request.collection_id,
        request.vectors.len(),
        tenant.tenant_id
    );

    // Validate request
    if request.collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    if request.vectors.is_empty() {
        return Err(ApiError::InvalidArgument(
            "At least one record is required".to_string(),
        ));
    }

    // Delegate to UnifiedHandlers v1 wrapper (returns v1 response)
    match state
        .unified_handlers
        .handle_vector_batch_v1_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(v1_resp) => Ok(JsonResponse(v1_resp)),
        Err(e) => {
            error!("Vector batch operation failed: {}", e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

/// Get a single vector by ID
pub async fn get_vector(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Path((collection_id, vector_id)): Path<(String, String)>,
    Query(params): Query<GetVectorParams>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    debug!("Get vector: collection={}, id={}", collection_id, vector_id);

    // Validate parameters
    if collection_id.is_empty() || vector_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "collection_id and vector_id are required".to_string(),
        ));
    }

    let include_vector = params.include_vector.unwrap_or(true);
    let include_metadata = params.include_metadata.unwrap_or(true);

    // Delegate to UnifiedHandlers
    match state
        .unified_handlers
        .handle_vector_v1_for_tenant(
            &collection_id,
            &vector_id,
            include_vector,
            include_metadata,
            Some(&tenant.tenant_id),
        )
        .await
    {
        Ok(response) => Ok(JsonResponse(response)),
        Err(e) => {
            error!(
                "Failed to get vector {}/{}: {}",
                collection_id, vector_id, e
            );
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

/// Delete a single vector by ID
pub async fn delete_vector(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    info!(
        "Delete vector: collection={}, id={}",
        collection_id, vector_id
    );

    // Validate parameters
    if collection_id.is_empty() || vector_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "collection_id and vector_id are required".to_string(),
        ));
    }

    // Create a batch request with vector marked for deletion via expires_at
    let delete_request = proximadb_v1::VectorBatchRequest {
        collection_id: collection_id.clone(),
        vectors: vec![proximadb_v1::VectorRecord {
            id: vector_id.clone(),
            vector: vec![], // Empty vector (tombstone)
            metadata: std::collections::HashMap::new(),
            version: None,
            timestamp: None,
            source: None,
            updated_at: None,
            expires_at: Some(0), // Set to 0 (past time) to mark for immediate deletion
        }],
    };

    // Delegate to vector batch handler (which supports deletions)
    match state
        .unified_handlers
        .handle_vector_batch_v1_for_tenant(delete_request, Some(&tenant.tenant_id))
        .await
    {
        Ok(response) => {
            // Return the batch response (operation is already VsBatch)
            Ok(JsonResponse(response))
        }
        Err(e) => {
            error!(
                "Failed to delete vector {}/{}: {}",
                collection_id, vector_id, e
            );
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

/// Query parameters for get_vector endpoint
#[derive(Debug, Deserialize)]
pub struct GetVectorParams {
    /// Include the raw vector data in the response
    pub include_vector: Option<bool>,
    /// Include metadata fields in the response
    pub include_metadata: Option<bool>,
}

/// Aligned collection operation handler
pub async fn collection_operation(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<JsonResponse<proximadb_v1::CollectionResponse>> {
    info!(
        "🔵 REST API: collection_operation called (tenant: {}) with payload: {}",
        tenant.tenant_id,
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "invalid json".to_string())
    );

    // Parse the JSON value into CollectionRequest
    let request: CollectionRequest = serde_json::from_value(value.clone()).map_err(|e| {
        error!(
            "🔴 REST API: Failed to parse CollectionRequest from payload: {:?}. Error: {}",
            value, e
        );
        ApiError::InvalidArgument(format!("Invalid request format: {}", e))
    })?;

    let operation = match CollectionOperation::try_from(request.operation) {
        Ok(op) => op,
        Err(_) => {
            return Err(ApiError::InvalidArgument(
                "Invalid collection operation".to_string(),
            ));
        }
    };

    info!(
        "Collection operation: {:?} for collection: {:?}",
        operation, request.collection_id
    );

    // Direct delegation to UnifiedHandlers
    match state
        .unified_handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(response) => Ok(JsonResponse(response)),
        Err(e) => {
            error!("Collection operation failed: {}", e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

/// Health check endpoint with proper error handling
pub async fn health_check(
    State(_state): State<AppState>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    // Return basic health status
    // Deferred: Add actual health checks when UnifiedHandlers supports it

    Ok(JsonResponse(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION"),
        "services": {
            "rest_api": "operational",
            "storage": "operational",
            "indexing": "operational"
        }
    })))
}

/// Get collection by ID with aligned error handling
pub async fn get_collection(
    Path(collection_id): Path<String>,
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
) -> impl IntoResponse {
    debug!(
        "Get collection '{}' for tenant '{}'",
        collection_id, tenant.tenant_id
    );

    if collection_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "Collection ID is required").into_response();
    }

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionGet as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    match state
        .unified_handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(response) => JsonResponse(response).into_response(),
        Err(e) => {
            if e.to_string().contains("not found") {
                (
                    StatusCode::NOT_FOUND,
                    format!("Collection not found: {}", collection_id),
                )
                    .into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
    }
}

/// List collections with pagination
#[derive(serde::Deserialize)]
pub struct ListCollectionsQuery {
    /// Maximum number of collections to return
    pub limit: Option<u32>,
    /// Pagination offset
    pub offset: Option<u32>,
    /// Include collection statistics (vector count, storage size)
    pub include_stats: Option<bool>,
}

/// List collections with pagination and optional statistics
pub async fn list_collections(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Query(params): Query<ListCollectionsQuery>,
) -> impl IntoResponse {
    debug!("Listing collections for tenant '{}'", tenant.tenant_id);

    let mut query_params = std::collections::HashMap::new();

    if let Some(limit) = params.limit {
        query_params.insert("limit".to_string(), limit.to_string());
    }
    if let Some(offset) = params.offset {
        query_params.insert("offset".to_string(), offset.to_string());
    }

    let mut options = std::collections::HashMap::new();
    if let Some(include_stats) = params.include_stats {
        options.insert("include_stats".to_string(), include_stats);
    }

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionList as i32,
        collection_id: None,
        collection_config: None,
        query_params,
        options,
        migration_config: Default::default(),
    };

    match state
        .unified_handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(response) => JsonResponse(response).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Delete collection with proper error handling
pub async fn delete_collection(
    Path(collection_id): Path<String>,
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
) -> impl IntoResponse {
    info!(
        "Delete collection '{}' for tenant '{}'",
        collection_id, tenant.tenant_id
    );

    if collection_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "Collection ID is required").into_response();
    }

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    match state
        .unified_handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(response) => JsonResponse(response).into_response(),
        Err(e) => {
            if e.to_string().contains("not found") {
                (
                    StatusCode::NOT_FOUND,
                    format!("Collection not found: {}", collection_id),
                )
                    .into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
    }
}

/// Example using JSON wrapper types for consistent structure
pub async fn vector_search_with_metadata(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    // Parse the JSON value into VectorSearchRequest (supports both formats)
    let request: VectorSearchRequest = parse_search_request(value)
        .map_err(|e| ApiError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    let start_time = std::time::Instant::now();
    let request_id = Uuid::new_v4().to_string();

    info!(
        "Vector search request {} for collection: {} (tenant: {})",
        request_id, request.collection_id, tenant.tenant_id
    );

    // Execute search
    match state
        .unified_handlers
        .handle_vector_search_v1_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(response) => {
            let elapsed = start_time.elapsed();
            info!(
                "Vector search {} completed in {}ms",
                request_id,
                elapsed.as_millis()
            );

            Ok(JsonResponse(response))
        }
        Err(e) => {
            error!("Vector search {} failed: {}", request_id, e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

/// SQL query request structure
#[derive(Debug, Serialize, Deserialize)]
pub struct SqlQueryRequest {
    /// SQL query string
    pub query: String,
    /// Optional parameters for parameterized queries (proto-aligned)
    pub parameters: Option<Vec<proximadb_v1::SqlValue>>,
    /// Optional collection to use as default context
    pub collection: Option<String>,
    /// Optional timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Optional seeding strategy for hybrid (average | per_seed | none)
    pub seeding: Option<String>,
}

// SQL query response structure
// For REST, we now return proximadb.v1 ExecuteSqlResponse directly, wrapped by ProtoApiResponse
/// Column information in SQL results
#[derive(Debug, Serialize, Deserialize)]
pub struct SqlColumnInfo {
    /// Column name
    pub name: String,
    /// Column data type
    pub data_type: String,
}

/// Execute SQL query handler
///
/// Supports vector similarity queries like:
/// ```sql
/// SELECT id, metadata, COSINE_DISTANCE(embedding, [0.1, 0.2, 0.3]) as score
/// FROM my_collection
/// WHERE metadata.category = 'electronics'
/// ORDER BY score ASC
/// LIMIT 10
/// ```
pub async fn execute_sql(
    State(state): State<AppState>,
    Json(request): Json<SqlQueryRequest>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    let start_time = std::time::Instant::now();
    let request_id = Uuid::new_v4().to_string();

    info!(
        "SQL query request {} with query: {}",
        request_id,
        request.query.chars().take(100).collect::<String>()
    );

    // Validate request
    if request.query.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "SQL query cannot be empty".to_string(),
        ));
    }

    // Route through unified facade when adapter is available
    if let Some(ref adapter) = state.query_adapter {
        debug!("Using unified facade routing for SQL query");
        return match adapter.sql_query(&request.query).await {
            Ok(result) => {
                let execution_time_ms = start_time.elapsed().as_millis() as u64;

                // Convert QueryResult to JSON response
                let rows = match result.data {
                    crate::query::QueryResultData::Rows(rows) => rows,
                    crate::query::QueryResultData::Empty => vec![],
                    _ => vec![], // Other types return empty for SQL endpoint
                };

                let json_data = serde_json::json!({
                    "rows": rows,
                    "execution_time_ms": execution_time_ms,
                    "rows_returned": rows.len(),
                    "row_count": rows.len(),
                    "request_id": request_id
                });

                info!(
                    "SQL query {} (facade) completed in {}ms",
                    request_id, execution_time_ms
                );

                Ok(JsonResponse(json_data))
            }
            Err(e) => {
                error!("SQL query {} (facade) failed: {}", request_id, e);
                Err(ApiError::Internal(e.to_string()))
            }
        };
    }

    // Legacy path: Execute through v1 path (typed params and rows)
    // Optional: read seeding strategy from HTTP header (X-Seeding-Strategy) or from request.parameters via a special key
    let _seeding_strategy = crate::query::execution::SeedingStrategy::Average; // default

    let query_with_hint = if let Some(seeding) = &request.seeding {
        let seed_upper = seeding.to_ascii_uppercase();
        format!("-- SEEDING: {}\n{}", seed_upper, request.query)
    } else {
        request.query.clone()
    };

    match state
        .unified_handlers
        .execute_sql_v1(
            query_with_hint,
            request.parameters.clone(),
            request.collection,
        )
        .await
    {
        Ok(v1_resp) => {
            let execution_time_ms = start_time.elapsed().as_millis() as u64;

            // Convert SQL response to JSON value for now
            // Deferred: Create proper JsonExecuteSqlResponse wrapper if needed
            let json_data = serde_json::json!({
                "rows": v1_resp.rows.iter().map(|row| {
                    // Convert fields to a JSON object instead of list of key/value pairs
                    let mut obj = serde_json::Map::new();
                    for field in &row.fields {
                        let value = field.value.as_ref().map_or(serde_json::Value::Null, sql_value_to_json);
                        obj.insert(field.key.clone(), value);
                    }
                    serde_json::Value::Object(obj)
                }).collect::<Vec<_>>(),
                "columns": v1_resp.columns,
                "column_types": v1_resp.column_types,
                "execution_time_ms": execution_time_ms,
                "rows_returned": v1_resp.rows_returned,
                "row_count": v1_resp.rows_returned,  // Add row_count alias for compatibility
                "rows_scanned": v1_resp.rows_scanned,
                "request_id": request_id
            });

            info!(
                "SQL query {} completed in {}ms",
                request_id, execution_time_ms
            );

            Ok(JsonResponse(json_data))
        }
        Err(e) => {
            error!("SQL query {} failed: {}", request_id, e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

/// EXPLAIN query request structure  
#[derive(Debug, Serialize, Deserialize)]
pub struct ExplainQueryRequest {
    /// SQL query string to explain
    pub query: String,
    /// Whether to include execution (ANALYZE)
    pub analyze: Option<bool>,
    /// Optional collection context
    pub collection: Option<String>,
}

/// Helper: convert proto SqlValue to serde_json::Value (temporary until full internal refactor)
fn sql_value_to_json(v: &proximadb_v1::SqlValue) -> serde_json::Value {
    use proximadb_v1::sql_value::Value as V;
    match v.value.as_ref() {
        Some(V::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(V::NumberValue(n)) => serde_json::Value::Number(
            serde_json::Number::from_f64(*n).unwrap_or(serde_json::Number::from(0)),
        ),
        Some(V::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(V::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(V::BytesValue(b)) => {
            // Represent bytes as JSON array of integers
            serde_json::Value::Array(
                b.iter()
                    .map(|x| serde_json::Value::Number((*x as u64).into()))
                    .collect(),
            )
        }
        Some(V::NullValue(_)) => serde_json::Value::Null,
        Some(V::ArrayValue(arr)) => {
            serde_json::Value::Array(arr.values.iter().map(sql_value_to_json).collect())
        }
        Some(V::ObjectValue(obj)) => {
            let mut map = serde_json::Map::new();
            for (k, sv) in &obj.fields {
                map.insert(k.clone(), sql_value_to_json(sv));
            }
            serde_json::Value::Object(map)
        }
        None => serde_json::Value::Null,
    }
}

/// EXPLAIN query response
#[derive(Debug, Serialize, Deserialize)]
pub struct ExplainQueryResponse {
    /// The explain plan
    pub plan: ExplainPlan,
    /// Request ID for tracing
    pub request_id: String,
}

/// EXPLAIN SQL query handler - shows query execution plan with vector and graph hints
pub async fn explain_sql(
    State(state): State<AppState>,
    Json(request): Json<ExplainQueryRequest>,
) -> ApiResult<JsonResponse<ExplainQueryResponse>> {
    let request_id = Uuid::new_v4().to_string();

    info!(
        "EXPLAIN query request {} for query: {}",
        request_id,
        request.query.chars().take(100).collect::<String>()
    );

    // Validate request
    if request.query.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "SQL query cannot be empty".to_string(),
        ));
    }

    // Build a lightweight QueryEngine with vector and graph services
    let qe = QueryEngine::new(
        state.unified_handlers.vector_operations_service.clone(),
        state.unified_handlers.graph_operations_service.clone(),
    );
    // Parse SQL and explain using frontend
    use crate::query::sql_frontend::parser::SqlFrontendParser;
    let parser = SqlFrontendParser::new();
    let parsed = parser
        .parse(&request.query)
        .map_err(|e| ApiError::Internal(format!("Failed to parse SQL: {}", e)))?;

    let explain_result = qe
        .explain_frontend(parsed)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to explain SQL: {}", e)))?;

    // Convert ExplainResult to ExplainPlan
    let plan = ExplainPlan {
        orchestration_steps: explain_result.operations,
        vector_hints: None, // Deferred: Extract from explain_result if needed
        graph_hints: None,  // Deferred: Extract from explain_result if needed
        join_costs: None,
        query_stats: None,
        execution_strategy: Some(format!("{:?}", explain_result.query_type)),
        estimated_total_cost: Some(explain_result.estimated_cost),
        cost_breakdown: None,
        join_strategy: None,
        fusion_strategy: None,
    };

    let response = ExplainQueryResponse {
        plan,
        request_id: request_id.clone(),
    };

    info!("EXPLAIN query {} completed", request_id);
    Ok(JsonResponse(response))
}

// Note: EXPLAIN now uses QueryEngine::explain_sql() for real plans/hints.

// =============================================================================
// Hybrid Search (BM25 + Vector with RRF Fusion)
// =============================================================================

/// Compatibility alias for vector search input
/// Maps internal VectorResult to simple wrapper used by handlers
#[allow(dead_code)]
struct VectorSearchInput {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    score: f32,
}

/// Request body for hybrid search
#[derive(Debug, Deserialize)]
pub struct HybridSearchRequest {
    /// Collection to search
    pub collection: String,
    /// Query vector for similarity search (optional if keyword-only)
    pub vector: Option<Vec<f32>>,
    /// Text query for BM25 keyword search (optional if vector-only)
    pub text_query: Option<String>,
    /// Number of results to return
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Weight for vector results (0.0-1.0). BM25 weight = 1.0 - vector_weight.
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f32,
    /// RRF constant k (default 60)
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u32,
    /// Minimum BM25 score threshold
    #[serde(default)]
    pub min_bm25_score: f64,
}

fn default_top_k() -> usize {
    10
}
fn default_vector_weight() -> f32 {
    0.5
}
fn default_rrf_k() -> u32 {
    60
}

/// Request body for indexing text documents for hybrid search
#[derive(Debug, Deserialize)]
pub struct HybridIndexRequest {
    /// Collection name
    pub collection: String,
    /// Documents to index: list of {id, text}
    pub documents: Vec<HybridDocument>,
}

/// A text document for hybrid search indexing
#[derive(Debug, Deserialize)]
pub struct HybridDocument {
    /// Document/vector ID
    pub id: String,
    /// Text content to index
    pub text: String,
}

/// Response for hybrid search
#[derive(Debug, Serialize)]
pub struct HybridSearchResponse {
    /// Whether the search completed successfully
    pub success: bool,
    /// Fused search result hits
    pub results: Vec<HybridSearchHit>,
    /// Total number of results
    pub total: usize,
    /// Server-side processing time in microseconds
    pub processing_time_us: u64,
    /// Search mode used (e.g., "hybrid", "vector_only", "bm25_only")
    pub mode: String,
}

/// A single hybrid search result hit
#[derive(Debug, Serialize)]
pub struct HybridSearchHit {
    /// Vector/document identifier
    pub id: String,
    /// Fused score combining vector and BM25 signals
    pub combined_score: f64,
    /// Vector similarity score (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_score: Option<f32>,
    /// BM25 text relevance score (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_score: Option<f64>,
    /// Rank in vector-only results (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_rank: Option<usize>,
    /// Rank in BM25-only results (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_rank: Option<usize>,
    /// BM25 terms that matched the query
    pub matched_terms: Vec<String>,
}

/// Response for hybrid index operations
#[derive(Debug, Serialize)]
pub struct HybridIndexResponse {
    /// Whether the indexing operation succeeded
    pub success: bool,
    /// Collection that was indexed
    pub collection: String,
    /// Number of documents indexed in this operation
    pub documents_indexed: usize,
    /// Total number of documents in the full-text index
    pub total_documents: usize,
}

/// Shared state for per-collection full-text indices
pub type FullTextIndexMap = Arc<
    std::sync::RwLock<
        std::collections::HashMap<
            String,
            crate::storage::engines::core::formats::columnar::fulltext_index::FullTextIndex,
        >,
    >,
>;

/// Index text documents for hybrid search
///
/// POST /api/v1/hybrid/index
/// Body: { "collection": "...", "documents": [{"id": "...", "text": "..."}] }
pub async fn hybrid_index(
    State(state): State<AppState>,
    Json(request): Json<HybridIndexRequest>,
) -> ApiResult<JsonResponse<HybridIndexResponse>> {
    if request.collection.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection name is required".to_string(),
        ));
    }
    if request.documents.is_empty() {
        return Err(ApiError::InvalidArgument(
            "At least one document is required".to_string(),
        ));
    }

    let fulltext_indexes = state
        .fulltext_indexes
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Hybrid search not initialized".to_string()))?;

    let mut indexes = fulltext_indexes
        .write()
        .map_err(|e| ApiError::Internal(format!("Lock error: {}", e)))?;

    let index = indexes
        .entry(request.collection.clone())
        .or_insert_with(|| {
            use crate::storage::engines::core::formats::columnar::fulltext_index::{
                FullTextIndex, TokenizerConfig,
            };
            FullTextIndex::new(TokenizerConfig::for_keyword_search())
        });

    let mut indexed = 0;
    for doc in &request.documents {
        // Skip if document already exists (idempotent)
        if index.contains_document(&doc.id) {
            continue;
        }
        if let Err(e) = index.add_document(&doc.id, &doc.text) {
            debug!("Skipping document {}: {}", doc.id, e);
            continue;
        }
        indexed += 1;
    }

    let total = index.document_count();

    info!(
        "Hybrid index: collection='{}', indexed={}, total={}",
        request.collection, indexed, total
    );

    Ok(JsonResponse(HybridIndexResponse {
        success: true,
        collection: request.collection,
        documents_indexed: indexed,
        total_documents: total,
    }))
}

/// Perform hybrid BM25 + vector search with RRF fusion
///
/// POST /api/v1/hybrid/search
/// Body: { "collection": "...", "vector": [...], "text_query": "...", "top_k": 10 }
pub async fn hybrid_search(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(request): Json<HybridSearchRequest>,
) -> ApiResult<JsonResponse<HybridSearchResponse>> {
    let start_time = std::time::Instant::now();

    if request.collection.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection name is required".to_string(),
        ));
    }
    if request.vector.is_none() && request.text_query.is_none() {
        return Err(ApiError::InvalidArgument(
            "At least one of 'vector' or 'text_query' is required".to_string(),
        ));
    }

    let has_vector = request.vector.is_some();
    let has_text = request.text_query.is_some();

    // Determine search mode
    let mode = match (has_vector, has_text) {
        (true, true) => "hybrid",
        (true, false) => "vector_only",
        (false, true) => "keyword_only",
        (false, false) => unreachable!(), // checked above
    };

    debug!(
        "Hybrid search: collection='{}', mode={}, top_k={}, vector_weight={}",
        request.collection, mode, request.top_k, request.vector_weight
    );

    // --- Vector search side ---
    let vector_results = if let Some(ref vector) = request.vector {
        // Build a VectorSearchRequest and execute through existing pipeline
        let search_query = proximadb_v1::SearchQuery {
            vector: vector.clone(),
            filters: std::collections::HashMap::new(),
            advanced_filter: None,
        };
        let search_request = VectorSearchRequest {
            collection_id: request.collection.clone(),
            queries: vec![search_query],
            top_k: request.top_k as u32,
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        // Use query adapter if available, else legacy handlers
        let response = state
            .unified_handlers
            .handle_vector_search_v1_for_tenant(search_request, Some(&tenant.tenant_id))
            .await
            .map_err(|e| ApiError::Internal(format!("Vector search failed: {}", e)))?;

        // Return the raw results - will be converted to VectorResult later
        // NOTE: Old VectorSearchInput code removed (type doesn't exist)
        response.results.map(|r| r.results).unwrap_or_default()
    } else {
        Vec::new()
    };

    // --- BM25 search side ---
    let bm25_results = if let Some(ref text_query) = request.text_query {
        let fulltext_indexes = state.fulltext_indexes.as_ref().ok_or_else(|| {
            ApiError::InvalidArgument(
                "No text index available. POST to /api/v1/hybrid/index first.".to_string(),
            )
        })?;

        let indexes = fulltext_indexes
            .read()
            .map_err(|e| ApiError::Internal(format!("Lock error: {}", e)))?;

        if let Some(index) = indexes.get(&request.collection) {
            use crate::core::search::hybrid::{BM25Result, TextHighlight};
            let search_results = index.search(text_query, request.top_k);
            search_results
                .into_iter()
                .map(|r| BM25Result {
                    doc_id: r.doc_id,
                    score: r.score,
                    highlights: Some(
                        r.matched_terms
                            .iter()
                            .map(|term| TextHighlight {
                                field: "content".to_string(),
                                text: term.clone(),
                                start_offset: 0,
                                end_offset: term.len(),
                            })
                            .collect(),
                    ),
                    metadata: std::collections::HashMap::new(),
                })
                .collect()
        } else {
            // No text index for this collection — return empty BM25 results
            debug!(
                "No text index for collection '{}', using vector-only results",
                request.collection
            );
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // --- RRF Fusion using comprehensive hybrid module ---
    use crate::core::search::hybrid::{FusionStrategy, HybridFusionEngine, VectorResult};

    // Convert vector results to VectorResult format
    let vector_results_compact: Vec<VectorResult> = vector_results
        .into_iter()
        .map(|v| VectorResult {
            doc_id: v.id,
            score: v.score,
            distance: 1.0 - v.score, // Convert similarity to distance
            metadata: std::collections::HashMap::new(),
        })
        .collect();

    let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank {
        k: request.rrf_k as usize,
    });

    let fused = engine
        .fuse(bm25_results, vector_results_compact)
        .map_err(|e| ApiError::Internal(format!("Fusion failed: {}", e)))?;

    let hits: Vec<HybridSearchHit> = fused
        .into_iter()
        .map(|r| HybridSearchHit {
            id: r.doc_id,
            combined_score: r.fused_score,
            vector_score: if r.vector_score > 0.0 {
                Some(r.vector_score as f32)
            } else {
                None
            },
            bm25_score: if r.bm25_score > 0.0 {
                Some(r.bm25_score)
            } else {
                None
            },
            vector_rank: if r.vector_rank != usize::MAX {
                Some(r.vector_rank)
            } else {
                None
            },
            bm25_rank: if r.bm25_rank != usize::MAX {
                Some(r.bm25_rank)
            } else {
                None
            },
            matched_terms: r
                .highlights
                .as_ref()
                .map(|h| h.iter().map(|hl| hl.text.clone()).collect())
                .unwrap_or_default(),
        })
        .collect();

    let total = hits.len();
    let elapsed = start_time.elapsed().as_micros() as u64;

    info!(
        "Hybrid search complete: collection='{}', mode={}, results={}, time={}us",
        request.collection, mode, total, elapsed
    );

    Ok(JsonResponse(HybridSearchResponse {
        success: true,
        results: hits,
        total,
        processing_time_us: elapsed,
        mode: mode.to_string(),
    }))
}

/// Create router with all REST endpoints
pub fn create_router(state: AppState) -> axum::Router {
    use axum::routing::{delete, get, post};

    info!("🔵 REST API: Creating router with collection endpoints...");

    // Initialize SKS in-memory store (v1) using the same storage engine as vector operations
    let entities_router = {
        use crate::network::rest::v1::entities::{self, EntityApiState};
        use crate::storage::entity_store::{
            CsrRelationsStore, InMemoryProvenanceRegistry, ProximaEntityStore,
        };

        let engine = state
            .unified_handlers
            .vector_operations_service
            .unified_engine();
        let legacy_store = ProximaEntityStore::with_vector_service(
            engine,
            Arc::new(CsrRelationsStore::new()),
            Arc::new(InMemoryProvenanceRegistry::new()),
            state.unified_handlers.vector_operations_service.clone(),
        );

        // Register legacy store globally for compatibility (entity API currently uses legacy store).
        let legacy_arc = Arc::new(legacy_store);
        ProximaEntityStore::register_global(legacy_arc.clone());

        // Use the same Arc - no need to clone the inner value
        let store = legacy_arc.clone();
        // Register store globally for hybrid executor access (embedding catalog)
        crate::storage::entity_store::ProximaEntityStore::register_global(store.clone());
        let entity_state = EntityApiState { store };
        entities::configure_routes().with_state(entity_state)
    };

    let mut router = axum::Router::new()
        // Vector operations
        .route("/api/v1/search", post(vector_search))
        .route("/api/v1/vectors/batch", post(vector_batch))
        .route(
            "/api/v1/vectors/:collection_id/:vector_id",
            get(get_vector).delete(delete_vector),
        )
        .route(
            "/api/v1/progressive/search/:collection_id",
            post(crate::network::rest::progressive_search_handler::progressive_search_handler),
        )
        // SQL query execution
        .route("/api/v1/sql/execute", post(execute_sql))
        .route("/api/v1/sql/explain", post(explain_sql))
        // Collection operations
        .route("/api/v1/collections", post(collection_operation))
        .route("/api/v1/collections", get(list_collections))
        .route("/api/v1/collections/:collection_id", get(get_collection))
        .route(
            "/api/v1/collections/:collection_id",
            delete(delete_collection),
        )
        // Health check endpoints
        .route("/health", get(comprehensive_health_check))
        .route("/health/live", get(liveness_check))
        .route("/health/ready", get(readiness_check))
        // Hybrid search production endpoints (AppState-backed)
        .route("/api/v1/hybrid/search", post(hybrid_search))
        .route("/api/v1/hybrid/index", post(hybrid_index))
        // With metadata endpoints
        .route(
            "/api/v1/search/with_metadata",
            post(vector_search_with_metadata),
        )
        // Graph database endpoints
        .nest(
            "/api/v1/graph",
            crate::network::rest::v1::graph::create_graph_router(),
        )
        // SKS entity endpoints (storage-coupled path)
        .nest("/api", entities_router);

    // Document API endpoints (with WAL for durability)
    let document_router = {
        use crate::network::rest::v1::document::{self, DocumentApiState};
        use crate::storage::document::DocumentService;

        let engine = state
            .unified_handlers
            .vector_operations_service
            .unified_engine();

        // Use WAL-enabled constructor for durability (same as gRPC server)
        // data_dir comes from TOML config (server.data_dir)
        let doc_base_path = state.data_dir.join("documents");
        let doc_path_str = doc_base_path.to_string_lossy().to_string();

        let document_service = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match DocumentService::new_with_wal(engine.clone(), &doc_path_str).await {
                    Ok(svc) => Arc::new(svc),
                    Err(e) => {
                        tracing::warn!("Failed to create DocumentService with WAL: {}. Using non-durable storage.", e);
                        Arc::new(DocumentService::new(engine))
                    }
                }
            })
        });

        let doc_state = DocumentApiState { document_service };
        document::create_document_router().with_state(doc_state)
    };
    router = router.nest("/api/v1/documents", document_router);
    info!("✅ Document API endpoints enabled at /api/v1/documents (WAL-enabled)");

    // Observability API endpoints (with WAL for durability)
    // Create observability service first so it can be shared with unified query
    let observability_service: Option<Arc<crate::observability::ObservabilityService>> = {
        use crate::observability::{ObservabilityService, ObservabilityStorage};

        // Create storage in data directory with WAL for durability (same as gRPC server)
        // data_dir comes from TOML config (server.data_dir)
        let obs_base_path = state.data_dir.join("observability");
        let obs_path_str = obs_base_path.to_string_lossy().to_string();

        // Create service with WAL-enabled storage
        match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                // Try WAL-enabled storage first
                let storage = match ObservabilityStorage::new_with_wal(&obs_path_str).await {
                    Ok(s) => Arc::new(s),
                    Err(e) => {
                        tracing::warn!("Failed to create ObservabilityStorage with WAL: {}. Using non-durable storage.", e);
                        Arc::new(ObservabilityStorage::new(&obs_path_str))
                    }
                };
                ObservabilityService::new(storage).await
            })
        }) {
            Ok(service) => Some(Arc::new(service)),
            Err(e) => {
                tracing::warn!("Observability service initialization failed: {}", e);
                None
            }
        }
    };

    // Create observability router if service initialized successfully
    if let Some(ref obs_service) = observability_service {
        use crate::network::rest::v1::observability::{self, ObservabilityApiState};
        let obs_state = ObservabilityApiState {
            observability_service: obs_service.clone(),
        };
        router = router.nest(
            "/api/v1/observability",
            observability::create_observability_router().with_state(obs_state),
        );
        info!("✅ Observability API endpoints enabled at /api/v1/observability");
    }

    // Unified Multi-Model Query API endpoints
    // Routes all queries through QueryFacadeAdapter for consistent execution
    let unified_query_router_opt = {
        use crate::network::rest::v1::unified_query::{self, UnifiedQueryApiState};
        use crate::storage::document::DocumentService;

        let engine = state
            .unified_handlers
            .vector_operations_service
            .unified_engine();

        // Use WAL-enabled constructor for durability (same as document router)
        // data_dir comes from TOML config (server.data_dir)
        let doc_base_path = state.data_dir.join("documents");
        let doc_path_str = doc_base_path.to_string_lossy().to_string();

        let document_service = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match DocumentService::new_with_wal(engine.clone(), &doc_path_str).await {
                    Ok(svc) => Arc::new(svc),
                    Err(e) => {
                        tracing::warn!("Unified query: Failed to create DocumentService with WAL: {}. Using non-durable storage.", e);
                        Arc::new(DocumentService::new(engine.clone()))
                    }
                }
            })
        });

        // Get the query adapter from state (required for unified query execution)
        let query_adapter_opt = state.query_adapter.clone();

        // Use new_with_adapter to route all queries through QueryFacadeAdapter
        query_adapter_opt.map(|adapter| {
            let unified_state =
                UnifiedQueryApiState::new_with_adapter(adapter, document_service, engine);
            unified_query::create_router().with_state(unified_state)
        })
    };

    if let Some(unified_query_router) = unified_query_router_opt {
        router = router.nest("/api/v1/unified", unified_query_router);
        info!("✅ Unified Query API endpoints enabled at /api/v1/unified (via QueryFacadeAdapter)");
    } else {
        tracing::warn!(
            "QueryFacadeAdapter not configured in AppState. Skipping unified query endpoints."
        );
    }

    // Optional enterprise catalog endpoints
    #[cfg(feature = "enterprise-catalogs")]
    {
        let catalog_router = {
            use crate::network::rest::v1::catalog::{self, CatalogApiState};

            let catalog_state = CatalogApiState::new(state.catalog_manager.clone());
            catalog::configure_routes().with_state(catalog_state)
        };
        router = router.nest("/api/v1/catalogs", catalog_router);
        info!("✅ External Catalog API endpoints enabled at /api/v1/catalogs");
    }

    // Experimental hybrid API (mock-backed) stays separate from production path
    let hybrid_router = {
        use crate::network::rest::v1::hybrid::{self, HybridSearchApiState};

        let hybrid_state = HybridSearchApiState::new();
        hybrid::create_router().with_state(hybrid_state)
    };
    router = router.nest("/api/v1/experimental/hybrid", hybrid_router);
    info!("✅ Experimental Hybrid API endpoints enabled at /api/v1/experimental/hybrid");

    // Convert to Router<()> by providing state, with default tenant context for all routes
    let default_tenant = TenantContext::new(
        "default",
        crate::network::middleware::tenant::TenantIdSource::Default,
    );
    let router = router.with_state(state).layer(Extension(default_tenant));

    // Optional AI endpoints (disabled by default; enable with `--features ai_endpoints`)
    #[cfg(feature = "ai_endpoints")]
    {
        use crate::api_handlers::ai_endpoints;

        match tokio::runtime::Runtime::new()
            .and_then(|rt| rt.block_on(ai_endpoints::initialize_ai_service_state()))
        {
            Ok(ai_state) => {
                router = router.nest("/ai", ai_endpoints::create_ai_router(ai_state));
                info!("✅ AI endpoints enabled at /ai");
            }
            Err(e) => {
                warn!("AI endpoints disabled (initialization failed): {}", e);
            }
        }
    }

    // Optional Sales endpoints (disabled by default; enable with `--features sales_endpoints`)
    #[cfg(feature = "sales_endpoints")]
    {
        use crate::api_handlers::sales_endpoints;

        match tokio::runtime::Runtime::new()
            .and_then(|rt| rt.block_on(sales_endpoints::initialize_sales_service_state()))
        {
            Ok(sales_state) => {
                router = router.nest("/sales", sales_endpoints::create_sales_router(sales_state));
                info!("✅ Sales endpoints enabled at /sales");
            }
            Err(e) => {
                warn!("Sales endpoints disabled (initialization failed): {}", e);
            }
        }
    }

    info!("✅ REST API: Router created with routes:");
    info!("   POST   /api/v1/collections (collection_operation)");
    info!("   GET    /api/v1/collections (list_collections)");
    info!("   GET    /api/v1/collections/:id (get_collection)");
    info!("   DELETE /api/v1/collections/:id (delete_collection)");
    info!("   POST   /api/v1/hybrid/search (hybrid_search)");
    info!("   POST   /api/v1/hybrid/index (hybrid_index)");
    info!("   POST   /api/v1/experimental/hybrid/search (mock hybrid API)");

    router
}

/// Comprehensive health check handler
///
/// Wraps the health module's health_check function with our AppState
pub async fn comprehensive_health_check(
    State(state): State<AppState>,
    query: Query<health::HealthParams>,
) -> ApiResult<Json<health::HealthResponse>> {
    let health_state = health::HealthState::new(state.unified_handlers.clone());
    health::health_check(axum::extract::State(health_state), query)
        .await}

/// Liveness check handler
///
/// Simple liveness check for load balancers
pub async fn liveness_check(
    State(state): State<AppState>,
) -> ApiResult<Json<health::LivenessResponse>> {
    let health_state = health::HealthState::new(state.unified_handlers.clone());
    health::liveness_check(axum::extract::State(health_state))
        .await}

/// Readiness check handler
///
/// Returns 200 when ready, 503 when not ready
pub async fn readiness_check(
    State(state): State<AppState>,
) -> Result<Json<health::ReadinessResponse>, (StatusCode, Json<health::ReadinessResponse>)> {
    let health_state = health::HealthState::new(state.unified_handlers.clone());
    health::readiness_check(axum::extract::State(health_state)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::rest::proto_json::ProtoApiResponse;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::RwLock;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.to_string_lossy())
    }

    async fn build_test_app_state() -> (AppState, TempDir) {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let storage_path = temp_dir.path().join("storage");
        let metadata_path = temp_dir.path().join("metadata");
        let data_dir = temp_dir.path().join("server_data");
        std::fs::create_dir_all(&storage_path).expect("failed to create storage path");
        std::fs::create_dir_all(&metadata_path).expect("failed to create metadata path");
        std::fs::create_dir_all(&data_dir).expect("failed to create data dir");

        let mut config = crate::core::config::Config::default();
        config.server.data_dir = data_dir.clone();
        config.storage.metadata_url = file_url(&metadata_path);
        config.storage.storage_locations = vec![crate::core::config::StorageLocation {
            url: file_url(&storage_path),
            weight: 1,
            tags: vec!["test".to_string()],
        }];

        let (shared_services, _) = crate::network::multi_server::SharedServices::new(
            None,
            &config.storage,
            None,
            Some(&config),
        )
        .await
        .expect("failed to initialize shared services for test app state");

        let state = AppState {
            unified_handlers: shared_services.unified_handlers,
            security_coordinator: None,
            data_dir,
            query_adapter: None,
            fulltext_indexes: Some(Arc::new(RwLock::new(HashMap::new()))),
            catalog_manager: Arc::new(crate::catalog::CatalogManager::new()),
        };
        (state, temp_dir)
    }

    #[test]
    fn test_error_conversion() {
        let err = ApiError::CollectionNotFound("test_collection".to_string());
        let response = ProtoApiResponse::<()>::error(err);
        assert!(!response.success);
        assert!(response.error.is_some());
    }

    #[test]
    fn test_hybrid_search_request_deserialization() {
        let json = serde_json::json!({
            "collection": "test_col",
            "vector": [0.1, 0.2, 0.3],
            "text_query": "machine learning",
            "top_k": 5,
            "vector_weight": 0.7
        });
        let req: HybridSearchRequest =
            serde_json::from_value(json).expect("failed to deserialize HybridSearchRequest");
        assert_eq!(req.collection, "test_col");
        assert_eq!(req.vector.expect("vector should be present").len(), 3);
        assert_eq!(
            req.text_query.expect("text_query should be present"),
            "machine learning"
        );
        assert_eq!(req.top_k, 5);
        assert!((req.vector_weight - 0.7).abs() < 0.001);
        assert_eq!(req.rrf_k, 60); // default
    }

    #[test]
    fn test_hybrid_search_request_defaults() {
        let json = serde_json::json!({
            "collection": "test_col",
            "text_query": "hello"
        });
        let req: HybridSearchRequest =
            serde_json::from_value(json).expect("failed to deserialize HybridSearchRequest");
        assert_eq!(req.top_k, 10);
        assert!((req.vector_weight - 0.5).abs() < 0.001);
        assert_eq!(req.rrf_k, 60);
        assert!(req.vector.is_none());
    }

    #[test]
    fn test_hybrid_index_request_deserialization() {
        let json = serde_json::json!({
            "collection": "test_col",
            "documents": [
                {"id": "doc1", "text": "The quick brown fox"},
                {"id": "doc2", "text": "jumps over the lazy dog"}
            ]
        });
        let req: HybridIndexRequest =
            serde_json::from_value(json).expect("failed to deserialize HybridIndexRequest");
        assert_eq!(req.collection, "test_col");
        assert_eq!(req.documents.len(), 2);
        assert_eq!(req.documents[0].id, "doc1");
        assert_eq!(req.documents[1].text, "jumps over the lazy dog");
    }

    #[test]
    fn test_fulltext_index_map_operations() {
        use crate::storage::engines::core::formats::columnar::fulltext_index::{
            FullTextIndex, TokenizerConfig,
        };

        let map: FullTextIndexMap =
            Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        // Add an index
        {
            let mut indexes = map.write().expect("RwLock should not be poisoned");
            let mut index = FullTextIndex::new(TokenizerConfig::for_keyword_search());
            index
                .add_document("doc1", "machine learning neural networks")
                .expect("failed to add document to index");
            index
                .add_document("doc2", "deep learning transformers")
                .expect("failed to add document to index");
            index
                .add_document("doc3", "database systems query optimization")
                .expect("failed to add document to index");
            indexes.insert("test_col".to_string(), index);
        }

        // Search
        {
            let indexes = map.read().expect("RwLock should not be poisoned");
            let index = indexes
                .get("test_col")
                .expect("test_col index should exist");
            let results = index.search("learning", 10);
            assert_eq!(results.len(), 2);
            // doc1 and doc2 both contain "learning"
            let ids: Vec<&str> = results.iter().map(|r| r.doc_id.as_str()).collect();
            assert!(ids.contains(&"doc1"));
            assert!(ids.contains(&"doc2"));
        }
    }

    #[test]
    fn test_hybrid_search_response_serialization() {
        let response = HybridSearchResponse {
            success: true,
            results: vec![
                HybridSearchHit {
                    id: "doc1".to_string(),
                    combined_score: 0.05,
                    vector_score: Some(0.95),
                    bm25_score: Some(3.2),
                    vector_rank: Some(1),
                    bm25_rank: Some(2),
                    matched_terms: vec!["learning".to_string()],
                },
                HybridSearchHit {
                    id: "doc2".to_string(),
                    combined_score: 0.03,
                    vector_score: None,
                    bm25_score: Some(5.1),
                    vector_rank: None,
                    bm25_rank: Some(1),
                    matched_terms: vec!["machine".to_string(), "learning".to_string()],
                },
            ],
            total: 2,
            processing_time_us: 1234,
            mode: "hybrid".to_string(),
        };

        let json =
            serde_json::to_string(&response).expect("failed to serialize HybridSearchResponse");
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"mode\":\"hybrid\""));
        // doc2 should NOT have vector_score/vector_rank (skip_serializing_if = None)
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("failed to deserialize JSON value");
        let doc2 = &parsed["results"][1];
        assert!(doc2.get("vector_score").is_none());
        assert!(doc2.get("vector_rank").is_none());
    }

    // Test parse_search_request with simple format
    #[test]
    fn test_parse_search_request_simple_format() {
        let json = serde_json::json!({
            "collection": "test_collection",
            "vector": [0.1, 0.2, 0.3, 0.4],
            "top_k": 20
        });

        let result = parse_search_request(json);
        assert!(result.is_ok());

        let request = result.expect("parse_search_request should succeed for simple format");
        assert_eq!(request.collection_id, "test_collection");
        assert_eq!(request.queries.len(), 1);
        assert_eq!(request.queries[0].vector, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(request.top_k, 20);
    }

    // Test parse_search_request with proto format
    #[test]
    fn test_parse_search_request_proto_format() {
        let json = serde_json::json!({
            "collection_id": "proto_collection",
            "queries": [
                {"vector": [0.5, 0.6, 0.7]},
                {"vector": [0.8, 0.9, 1.0]}
            ],
            "top_k": 15
        });

        let result = parse_search_request(json);
        assert!(result.is_ok());

        let request = result.expect("parse_search_request should succeed for proto format");
        assert_eq!(request.collection_id, "proto_collection");
        assert_eq!(request.queries.len(), 2);
        assert_eq!(request.top_k, 15);
    }

    // Test parse_search_request with filters
    #[test]
    fn test_parse_search_request_with_filters() {
        let json = serde_json::json!({
            "collection": "filtered_collection",
            "vector": [0.1, 0.2, 0.3],
            "top_k": 10,
            "filters": {
                "category": "electronics",
                "price": 299
            }
        });

        let result = parse_search_request(json);
        assert!(result.is_ok());

        let request = result.expect("parse_search_request should succeed with filters");
        assert_eq!(request.queries[0].filters.len(), 2);
        assert!(request.queries[0].filters.contains_key("category"));
    }

    // Test ApiError variants
    #[test]
    fn test_api_error_variants() {
        use std::io;

        // Test CollectionNotFound
        let err = ApiError::CollectionNotFound("test_col".to_string());
        assert_eq!(err.to_string(), "Collection not found: test_col");

        // Test InvalidArgument
        let err = ApiError::InvalidArgument("bad argument".to_string());
        assert_eq!(err.to_string(), "Invalid argument: bad argument");

        // Test Internal
        let err = ApiError::Internal("internal error".to_string());
        assert_eq!(err.to_string(), "Internal error: internal error");

        // Test IO error message propagation
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let api_err = ApiError::Internal(io_err.to_string());
        assert!(api_err.to_string().contains("file not found"));
    }

    // Test ApiDisplay trait implementation
    #[test]
    fn test_api_display() {
        let err = ApiError::CollectionNotFound("my_collection".to_string());
        let display = format!("{}", err);
        assert!(display.contains("my_collection"));
    }

    // Test empty collection validation
    #[test]
    fn test_empty_collection_validation() {
        let json = serde_json::json!({
            "collection": "",
            "vector": [0.1, 0.2]
        });

        let result = parse_search_request(json);
        assert!(result.is_ok()); // parse succeeds but validation happens in handler
        assert_eq!(result.expect("parse should succeed").collection_id, "");
    }

    // Test default values for optional fields
    #[test]
    fn test_parse_search_request_defaults() {
        let json = serde_json::json!({
            "collection": "defaults_test",
            "vector": [0.1]
        });

        let result = parse_search_request(json);
        assert!(result.is_ok());

        let request = result.expect("parse_search_request with defaults should succeed");
        assert_eq!(request.top_k, 10); // default top_k
        assert!(request.include_fields.is_none()); // optional field
        assert!(request.search_params.is_none()); // optional field
    }

    // Test VectorSearchRequest roundtrip
    #[test]
    fn test_vector_search_request_roundtrip() {
        let original_json = serde_json::json!({
            "collection": "roundtrip",
            "vector": [0.1, 0.2, 0.3, 0.4, 0.5],
            "top_k": 100,
            "filters": {"status": "active"}
        });

        let parsed = parse_search_request(original_json.clone())
            .expect("parse_search_request roundtrip should succeed");
        let serialized = serde_json::to_value(&parsed).expect("serialization should succeed");

        assert_eq!(
            serialized["collection_id"].as_str(),
            original_json["collection"].as_str()
        );
        assert_eq!(serialized["top_k"].as_u64(), Some(100));
    }

    // Test error message formatting
    #[test]
    fn test_error_message_formatting() {
        let errors = vec![
            ApiError::CollectionNotFound("test".to_string()),
            ApiError::InvalidArgument("invalid".to_string()),
            ApiError::Internal("server error".to_string()),
        ];

        for err in errors {
            let msg = format!("{}", err);
            assert!(!msg.is_empty());
            assert!(!msg.contains("ApiError(")); // Should be user-friendly
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_hybrid_search_canonical_production_route_returns_bad_request_not_not_found() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/hybrid/search")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"collection":"","text_query":"hybrid route test"}"#,
            ))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling canonical hybrid route request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_vector_search_canonical_production_route_returns_bad_request_not_not_found() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/search")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"collection":"","vector":[0.1,0.2,0.3],"top_k":5}"#,
            ))
            .expect("failed to build request");
        request
            .extensions_mut()
            .insert(crate::network::middleware::tenant::TenantContext::default_tenant());

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling canonical vector route request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_document_index_canonical_production_route_returns_bad_request_not_not_found() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/documents/collections/ws1_docs/indexes")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"path":"content","index_type":"fulltext","unique":false}"#,
            ))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling canonical document route request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_graph_shortest_path_canonical_production_route_returns_unprocessable_entity_not_not_found()
     {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/graph/graphs/ws1_graph/shortest_path")
            .header("content-type", "application/json")
            .body(Body::from(r#"{}"#))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling canonical graph route request");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_graph_legacy_nodes_endpoint_redirects_to_canonical_multi_graph_route() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/graph/nodes")
            .header("content-type", "application/json")
            .body(Body::from(r#"{}"#))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling legacy graph nodes route request");
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok());
        assert_eq!(location, Some("/api/v1/graph/graphs/default/nodes"));
        let deprecation = response
            .headers()
            .get("deprecation")
            .and_then(|v| v.to_str().ok());
        assert_eq!(deprecation, Some("true"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_graph_legacy_edges_endpoint_redirects_to_canonical_multi_graph_route() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/graph/edges")
            .header("content-type", "application/json")
            .body(Body::from(r#"{}"#))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling legacy graph edges route request");
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok());
        assert_eq!(location, Some("/api/v1/graph/graphs/default/edges"));
        let deprecation = response
            .headers()
            .get("deprecation")
            .and_then(|v| v.to_str().ok());
        assert_eq!(deprecation, Some("true"));
    }

    // ============================================================
    // parse_search_request extended tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_parse_search_request_no_vector() {
        let json = serde_json::json!({
            "collection": "test_col"
        });
        let result = parse_search_request(json);
        assert!(result.is_ok());
        let req = result.unwrap();
        assert_eq!(req.collection_id, "test_col");
        assert!(req.queries[0].vector.is_empty());
        assert_eq!(req.top_k, 10); // default
    }

    #[test]
    fn test_parse_search_request_empty_vector() {
        let json = serde_json::json!({
            "collection": "test_col",
            "vector": [],
            "top_k": 5
        });
        let result = parse_search_request(json);
        assert!(result.is_ok());
        let req = result.unwrap();
        assert!(req.queries[0].vector.is_empty());
        assert_eq!(req.top_k, 5);
    }

    #[test]
    fn test_parse_search_request_collection_id_fallback() {
        // Simple format with "collection_id" instead of "collection"
        // but since it doesn't have "collection" key, it needs "vector" key to trigger simple format
        let json = serde_json::json!({
            "vector": [1.0, 2.0],
            "collection_id": "fallback_col"
        });
        let result = parse_search_request(json);
        assert!(result.is_ok());
        let req = result.unwrap();
        assert_eq!(req.collection_id, "fallback_col");
    }

    #[test]
    fn test_parse_search_request_invalid_json() {
        let json = serde_json::json!("just a string");
        let result = parse_search_request(json);
        // Should fail because a string is not an object
        assert!(result.is_err());
    }

    // ============================================================
    // parse_batch_request tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_parse_batch_request_simple_format() {
        let json = serde_json::json!({
            "collection": "batch_col",
            "vectors": []
        });
        let result = parse_batch_request(json);
        assert!(result.is_ok());
        let req = result.unwrap();
        assert_eq!(req.collection_id, "batch_col");
        assert!(req.vectors.is_empty());
    }

    #[test]
    fn test_parse_batch_request_proto_format() {
        let json = serde_json::json!({
            "collection_id": "proto_batch_col",
            "vectors": []
        });
        let result = parse_batch_request(json);
        assert!(result.is_ok());
        let req = result.unwrap();
        assert_eq!(req.collection_id, "proto_batch_col");
    }

    #[test]
    fn test_parse_batch_request_collection_id_fallback() {
        let json = serde_json::json!({
            "collection": "preferred_col",
            "collection_id": "fallback_col",
            "vectors": []
        });
        let result = parse_batch_request(json);
        assert!(result.is_ok());
        // "collection" key takes precedence in simple format
        let req = result.unwrap();
        assert_eq!(req.collection_id, "preferred_col");
    }

    #[test]
    fn test_parse_batch_request_invalid() {
        let json = serde_json::json!(42);
        let result = parse_batch_request(json);
        assert!(result.is_err());
    }

    // ============================================================
    // sql_value_to_json tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_sql_value_to_json_string() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::StringValue("hello".to_string())),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!("hello"));
    }

    #[test]
    fn test_sql_value_to_json_number() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::NumberValue(42.5)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!(42.5));
    }

    #[test]
    fn test_sql_value_to_json_bool() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::BoolValue(true)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!(true));
    }

    #[test]
    fn test_sql_value_to_json_int64() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::Int64Value(9999)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!(9999));
    }

    #[test]
    fn test_sql_value_to_json_bytes() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::BytesValue(vec![0, 1, 255])),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!([0, 1, 255]));
    }

    #[test]
    fn test_sql_value_to_json_null() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::NullValue(0)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn test_sql_value_to_json_none() {
        let val = proximadb_v1::SqlValue { value: None };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn test_sql_value_to_json_array() {
        let arr = proximadb_v1::SqlArray {
            values: vec![
                proximadb_v1::SqlValue {
                    value: Some(proximadb_v1::sql_value::Value::Int64Value(1)),
                },
                proximadb_v1::SqlValue {
                    value: Some(proximadb_v1::sql_value::Value::Int64Value(2)),
                },
            ],
        };
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::ArrayValue(arr)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!([1, 2]));
    }

    #[test]
    fn test_sql_value_to_json_object() {
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            "name".to_string(),
            proximadb_v1::SqlValue {
                value: Some(proximadb_v1::sql_value::Value::StringValue("Alice".to_string())),
            },
        );
        fields.insert(
            "age".to_string(),
            proximadb_v1::SqlValue {
                value: Some(proximadb_v1::sql_value::Value::Int64Value(30)),
            },
        );
        let obj = proximadb_v1::SqlObject { fields };
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::ObjectValue(obj)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json["name"], serde_json::json!("Alice"));
        assert_eq!(json["age"], serde_json::json!(30));
    }

    #[test]
    fn test_sql_value_to_json_nan_number() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::NumberValue(f64::NAN)),
        };
        let json = sql_value_to_json(&val);
        // NaN cannot be represented in JSON, falls back to 0
        assert_eq!(json, serde_json::json!(0));
    }

    // ============================================================
    // SqlQueryRequest/SqlColumnInfo tests
    // ============================================================

    #[test]
    fn test_sql_query_request_deserialization() {
        let json = serde_json::json!({
            "query": "SELECT * FROM my_collection LIMIT 10",
            "collection": "my_collection",
            "timeout_ms": 5000
        });
        let req: SqlQueryRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.query, "SELECT * FROM my_collection LIMIT 10");
        assert_eq!(req.collection, Some("my_collection".to_string()));
        assert_eq!(req.timeout_ms, Some(5000));
        assert!(req.parameters.is_none());
        assert!(req.seeding.is_none());
    }

    #[test]
    fn test_sql_column_info_serialization() {
        let col = SqlColumnInfo {
            name: "embedding".to_string(),
            data_type: "vector".to_string(),
        };
        let json = serde_json::to_string(&col).unwrap();
        assert!(json.contains("embedding"));
        assert!(json.contains("vector"));
    }
}
