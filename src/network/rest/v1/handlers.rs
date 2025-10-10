//! Aligned REST API handlers using protobuf-first approach
//!
//! These handlers demonstrate the proper pattern for REST APIs that:
//! 1. Accept protobuf types directly as JSON
//! 2. Return protobuf responses as JSON
//! 3. Use unified ApiError for consistent error handling

use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::{Json as JsonResponse, IntoResponse},
};
use std::sync::Arc;
use tracing::{error, info};

use crate::api_handlers::UnifiedHandlers;
use crate::errors::{ApiError, ApiResult};
use crate::network::rest::health;
use crate::network::rest::proto_json::ProtoApiResponse;
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::{CollectionOperation, CollectionRequest};
use crate::proto::proximadb_v1::{
    VectorBatchRequest,
    VectorSearchRequest,
};
use crate::query::execution::QueryEngine;
use crate::query::explain::ExplainPlan;
use crate::utils::uuid::Uuid;
use serde::{Deserialize, Serialize};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub unified_handlers: Arc<UnifiedHandlers>,
}

/// Aligned vector search handler
pub async fn vector_search(
    State(state): State<AppState>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    // Parse the JSON value into VectorSearchRequest
    let request: VectorSearchRequest = serde_json::from_value(value)
        .map_err(|e| ApiError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    if request.collection_id.is_empty() {
        return Err(ApiError::InvalidArgument("Collection ID is required".to_string()));
    }

    match state
        .unified_handlers
        .handle_vector_search_v1(request.clone())
        .await
    {
        Ok(response) => Ok(JsonResponse(response)),
        Err(e) => {
            error!(
                "❌ Vector search failed for collection '{}': {:?}",
                request.collection_id, e
            );
            error!("Search request details: num_queries={}, top_k={}, has_filters={}, has_advanced_filter={}, has_search_params={}",
                request.queries.len(),
                request.top_k,
                request.queries.first().map(|q| !q.filters.is_empty()).unwrap_or(false),
                request.queries.first().and_then(|q| q.advanced_filter.as_ref()).is_some(),
                request.search_params.is_some()
            );
            Err(ApiError::Internal(format!("Search failed: {}", e)))
        }
    }
}

/// Aligned vector batch operation handler
pub async fn vector_batch(
    State(state): State<AppState>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    // Parse the JSON value into VectorBatchRequest
    let request: VectorBatchRequest = serde_json::from_value(value)
        .map_err(|e| ApiError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    info!(
        "Vector batch operation for collection: {}, {} records",
        request.collection_id,
        request.vectors.len()
    );

    // Validate request
    if request.collection_id.is_empty() {
        return Err(ApiError::InvalidArgument("Collection ID is required".to_string()));
    }

    if request.vectors.is_empty() {
        return Err(ApiError::InvalidArgument("At least one record is required".to_string()));
    }

    // Delegate to UnifiedHandlers v1 wrapper (returns v1 response)
    match state
        .unified_handlers
        .handle_vector_batch_v1(request)
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
    Path((collection_id, vector_id)): Path<(String, String)>,
    Query(params): Query<GetVectorParams>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    info!(
        "Get vector: collection={}, id={}, include_vector={}, include_metadata={}",
        collection_id,
        vector_id,
        params.include_vector.unwrap_or(true),
        params.include_metadata.unwrap_or(true)
    );

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
        .handle_vector_v1(&collection_id, &vector_id, include_vector, include_metadata)
        .await
    {
        Ok(response) => Ok(JsonResponse(response)),
        Err(e) => {
            error!("Failed to get vector {}/{}: {}", collection_id, vector_id, e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

/// Delete a single vector by ID
pub async fn delete_vector(
    State(state): State<AppState>,
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
        .handle_vector_batch_v1(delete_request)
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
    pub include_vector: Option<bool>,
    pub include_metadata: Option<bool>,
}

/// Aligned collection operation handler
pub async fn collection_operation(
    State(state): State<AppState>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<JsonResponse<proximadb_v1::CollectionResponse>> {
    info!("🔵 REST API: collection_operation called with payload: {}", serde_json::to_string_pretty(&value).unwrap_or_else(|_| "invalid json".to_string()));

    // Parse the JSON value into CollectionRequest
    let request: CollectionRequest = serde_json::from_value(value.clone())
        .map_err(|e| {
            error!("🔴 REST API: Failed to parse CollectionRequest from payload: {:?}. Error: {}", value, e);
            ApiError::InvalidArgument(format!("Invalid request format: {}", e))
        })?;

    let operation = match CollectionOperation::try_from(request.operation) {
        Ok(op) => op,
        Err(_) => return Err(ApiError::InvalidArgument("Invalid collection operation".to_string())),
    };

    info!(
        "Collection operation: {:?} for collection: {:?}",
        operation, request.collection_id
    );

    // Direct delegation to UnifiedHandlers
    match state
        .unified_handlers
        .handle_collection_operation(request)
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
    // TODO: Add actual health checks when UnifiedHandlers supports it

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
) -> impl IntoResponse {
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
        .handle_collection_operation(request)
        .await
    {
        Ok(response) => JsonResponse(response).into_response(),
        Err(e) => {
            if e.to_string().contains("not found") {
                (StatusCode::NOT_FOUND, format!("Collection not found: {}", collection_id)).into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
    }
}

/// List collections with pagination
#[derive(serde::Deserialize)]
pub struct ListCollectionsQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub include_stats: Option<bool>,
}

pub async fn list_collections(
    State(state): State<AppState>,
    Query(params): Query<ListCollectionsQuery>,
) -> impl IntoResponse {
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
        .handle_collection_operation(request)
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
) -> impl IntoResponse {
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
        .handle_collection_operation(request)
        .await
    {
        Ok(response) => JsonResponse(response).into_response(),
        Err(e) => {
            if e.to_string().contains("not found") {
                (StatusCode::NOT_FOUND, format!("Collection not found: {}", collection_id)).into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
    }
}

/// Example using JSON wrapper types for consistent structure
pub async fn vector_search_with_metadata(
    State(state): State<AppState>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<JsonResponse<proximadb_v1::VectorOperationResponse>> {
    // Parse the JSON value into VectorSearchRequest
    let request: VectorSearchRequest = serde_json::from_value(value)
        .map_err(|e| ApiError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    let start_time = std::time::Instant::now();
    let request_id = Uuid::new_v4().to_string();

    info!(
        "Vector search request {} for collection: {}",
        request_id, request.collection_id
    );

    // Execute search
    match state
        .unified_handlers
        .handle_vector_search_v1(request)
        .await
    {
        Ok(response) => {
            let elapsed = start_time.elapsed();
            info!(
                "Vector search {} completed in {}ms",
                request_id, elapsed.as_millis()
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

/// SQL query response structure
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

    // Execute through v1 path (typed params and rows)
    // Optional: read seeding strategy from HTTP header (X-Seeding-Strategy) or from request.parameters via a special key
    let seeding_strategy = crate::query::execution::SeedingStrategy::Average; // default

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
            // TODO: Create proper JsonExecuteSqlResponse wrapper if needed
            let json_data = serde_json::json!({
                "rows": v1_resp.rows.iter().map(|row| {
                    // Convert fields to a JSON object instead of list of key/value pairs
                    let mut obj = serde_json::Map::new();
                    for field in &row.fields {
                        let value = field.value.as_ref().map(sql_value_to_json).unwrap_or(serde_json::Value::Null);
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
    let parsed = parser.parse(&request.query)
        .map_err(|e| ApiError::Internal(format!("Failed to parse SQL: {}", e)))?;

    let explain_result = qe
        .explain_frontend(parsed)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to explain SQL: {}", e)))?;

    // Convert ExplainResult to ExplainPlan
    let plan = ExplainPlan {
        orchestration_steps: explain_result.operations,
        vector_hints: None, // TODO: Extract from explain_result if needed
        graph_hints: None,  // TODO: Extract from explain_result if needed
        join_costs: None,
        query_stats: None,
        execution_strategy: Some(format!("{:?}", explain_result.query_type)),
        estimated_total_cost: Some(explain_result.estimated_cost),
    };

    let response = ExplainQueryResponse {
        plan,
        request_id: request_id.clone(),
    };

    info!("EXPLAIN query {} completed", request_id);
    Ok(JsonResponse(response))
}

// Note: EXPLAIN now uses QueryEngine::explain_sql() for real plans/hints.

/// Create router with all REST endpoints
pub fn create_router(state: AppState) -> axum::Router {
    use axum::routing::{delete, get, post};

    info!("🔵 REST API: Creating router with collection endpoints...");

    // Initialize SKS in-memory store (v1) using the same storage engine as vector operations
    let entities_router = {
        use crate::storage::entity_store::{CsrRelationsStore, InMemoryProvenanceRegistry, ProximaEntityStore};
        use crate::network::rest::v1::entities::{self, EntityApiState};
        let engine = state
            .unified_handlers
            .vector_operations_service
            .unified_engine();
        let store = Arc::new(ProximaEntityStore::with_vector_service(
            engine,
            Arc::new(CsrRelationsStore::new()),
            Arc::new(InMemoryProvenanceRegistry::new()),
            state.unified_handlers.vector_operations_service.clone(),
        ));
        // Register store globally for hybrid executor access (embedding catalog)
        crate::storage::entity_store::ProximaEntityStore::register_global(store.clone());
        let entity_state = EntityApiState { store };
        entities::configure_routes().with_state(entity_state)
    };

    let router = axum::Router::new()
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
        .nest("/api", entities_router)
        .with_state(state);

    info!("✅ REST API: Router created with routes:");
    info!("   POST   /api/v1/collections (collection_operation)");
    info!("   GET    /api/v1/collections (list_collections)");
    info!("   GET    /api/v1/collections/:id (get_collection)");
    info!("   DELETE /api/v1/collections/:id (delete_collection)");

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
        .await
        .map_err(ApiError::from)
}

/// Liveness check handler
/// 
/// Simple liveness check for load balancers
pub async fn liveness_check(
    State(state): State<AppState>,
) -> ApiResult<Json<health::LivenessResponse>> {
    let health_state = health::HealthState::new(state.unified_handlers.clone());
    health::liveness_check(axum::extract::State(health_state))
        .await
        .map_err(ApiError::from)
}

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

    #[test]
    fn test_error_conversion() {
        let err = ApiError::CollectionNotFound("test_collection".to_string());
        let response = ProtoApiResponse::<()>::error(err);
        assert!(!response.success);
        assert!(response.error.is_some());
    }
}
