//! Aligned REST API handlers using protobuf-first approach
//!
//! These handlers demonstrate the proper pattern for REST APIs that:
//! 1. Accept protobuf types directly as JSON
//! 2. Return protobuf responses as JSON
//! 3. Use unified ApiError for consistent error handling

use axum::{
    extract::{Json, Path, Query, State},
    response::Json as JsonResponse,
};
use std::sync::Arc;
use tracing::{error, info};

use crate::api_handlers::UnifiedHandlers;
use crate::errors::{ApiError, ApiResult};
use crate::network::rest::proto_json::ProtoApiResponse;
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::{CollectionOperation, CollectionRequest, CollectionResponse};
use crate::proto::proximadb_v1::{
    VectorBatchRequest as V1VectorBatchRequest,
    VectorOperationResponse as V1VectorOperationResponse,
    VectorSearchRequest as V1VectorSearchRequest,
};
use crate::query::QueryEngine;
use crate::query::explain::ExplainPlan;
use crate::utils::uuid::Uuid;
use serde::{Deserialize, Serialize};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub unified_handlers: Arc<UnifiedHandlers>,
}

/// Aligned vector search handler
///
/// This handler demonstrates the protobuf-first approach:
/// - Accepts VectorSearchRequest directly as JSON
/// - Returns VectorOperationResponse directly as JSON
/// - Uses ApiError for consistent error handling
pub async fn vector_search(
    State(state): State<AppState>,
    Json(request): Json<V1VectorSearchRequest>,
) -> ApiResult<JsonResponse<JsonVectorOperationResponse>> {
    info!(
        "Vector search request for collection: {}, top_k: {}",
        request.collection_id, request.top_k
    );

    // Validate request
    if request.collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    if request.queries.is_empty() {
        return Err(ApiError::InvalidArgument(
            "At least one query is required".to_string(),
        ));
    }

    // Delegate to UnifiedHandlers v1 wrapper (returns v1 response)
    let v1_resp = state
        .unified_handlers
        .handle_vector_search_v1(request)
        .await
        .map_err(|e| {
            error!("Vector search failed: {}", e);
            ApiError::Internal(e.to_string())
        })?;

    // Convert proto response to JSON wrapper
    let json_response = v1_resp;
    Ok(JsonResponse(json_response))
}

/// Aligned vector batch operation handler
pub async fn vector_batch(
    State(state): State<AppState>,
    Json(request): Json<V1VectorBatchRequest>,
) -> ApiResult<JsonResponse<JsonVectorOperationResponse>> {
    info!(
        "Vector batch operation for collection: {}, {} records",
        request.collection_id,
        request.vectors.len()
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
    let v1_resp = state
        .unified_handlers
        .handle_vector_batch_v1(request)
        .await
        .map_err(|e| {
            error!("Vector batch operation failed: {}", e);
            ApiError::Internal(e.to_string())
        })?;

    // Convert proto response to JSON wrapper
    let json_response = v1_resp;
    Ok(JsonResponse(json_response))
}

/// Aligned collection operation handler
pub async fn collection_operation(
    State(state): State<AppState>,
    Json(request): Json<CollectionRequest>,
) -> ApiResult<JsonResponse<CollectionResponse>> {
    let operation = CollectionOperation::try_from(request.operation)
        .map_err(|_| ApiError::InvalidArgument("Invalid collection operation".to_string()))?;

    info!(
        "Collection operation: {:?} for collection: {:?}",
        operation, request.collection_id
    );

    // Direct delegation to UnifiedHandlers
    let response = state
        .unified_handlers
        .handle_collection_operation(request)
        .await
        .map_err(|e| {
            error!("Collection operation failed: {}", e);
            ApiError::Internal(e.to_string())
        })?;

    Ok(JsonResponse(response))
}

/// Health check endpoint with proper error handling
pub async fn health_check(
    State(_state): State<AppState>,
) -> ApiResult<JsonResponse<crate::proto::proximadb_v1::VectorOperationResponse>> {
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
) -> ApiResult<JsonResponse<CollectionResponse>> {
    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionGet as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    let response = state
        .unified_handlers
        .handle_collection_operation(request)
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::CollectionNotFound(collection_id)
            } else {
                ApiError::Internal(e.to_string())
            }
        })?;

    Ok(JsonResponse(response))
}

/// List collections with pagination
#[derive(serde::Deserialize)]
pub struct ListCollectionsQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub include_stats: Option<bool>,
}

pub async fn list_collections(
    Query(params): Query<ListCollectionsQuery>,
    State(state): State<AppState>,
) -> ApiResult<JsonResponse<CollectionResponse>> {
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

    let response = state
        .unified_handlers
        .handle_collection_operation(request)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(JsonResponse(response))
}

/// Delete collection with proper error handling
pub async fn delete_collection(
    Path(collection_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<JsonResponse<CollectionResponse>> {
    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    let response = state
        .unified_handlers
        .handle_collection_operation(request)
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::CollectionNotFound(collection_id)
            } else {
                ApiError::Internal(e.to_string())
            }
        })?;

    Ok(JsonResponse(response))
}

/// Example using JSON wrapper types for consistent structure
pub async fn vector_search_with_metadata(
    State(state): State<AppState>,
    Json(request): Json<V1VectorSearchRequest>,
) -> ApiResult<JsonResponse<crate::proto::proximadb_v1::VectorOperationResponse>> {
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

            // Convert proto response to JSON wrapper
            let json_response = response;
            let success_response = Ok(axum::Json(json_response));

            Ok(JsonResponse(success_response))
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
) -> ApiResult<JsonResponse<crate::proto::proximadb_v1::VectorOperationResponse>> {
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
                    row.fields.iter().map(sql_value_to_json).collect::<Vec<_>>()
                }).collect::<Vec<_>>(),
                "column_names": v1_resp.column_names,
                "execution_time_ms": execution_time_ms,
                "affected_rows": v1_resp.affected_rows,
                "request_id": request_id
            });

            let success_response = Ok(axum::Json(json_data));

            info!(
                "SQL query {} completed in {}ms",
                request_id, execution_time_ms
            );

            Ok(JsonResponse(success_response))
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
) -> ApiResult<JsonResponse<crate::proto::proximadb_v1::VectorOperationResponse>> {
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

    // Build a lightweight QueryEngine with vector service and generate a real plan with hints
    let qe = QueryEngine::new_with_vector_service(
        state.unified_handlers.vector_operations_service.clone(),
    );
    let plan = qe
        .explain_sql(&request.query)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to explain SQL: {}", e)))?;

    let response = ExplainQueryResponse {
        plan,
        request_id: request_id.clone(),
    };

    let success_response = Ok(axum::Json(response));

    info!("EXPLAIN query {} completed", request_id);
    Ok(JsonResponse(success_response))
}

// Note: EXPLAIN now uses QueryEngine::explain_sql() for real plans/hints.

/// Create router with all REST endpoints
pub fn create_router(state: AppState) -> axum::Router {
    use axum::routing::{delete, get, post};

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

    axum::Router::new()
        // Vector operations
        .route("/api/v1/search", post(vector_search))
        .route("/api/v1/vectors/batch", post(vector_batch))
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
        // Health check
        .route("/health", get(health_check))
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
        .with_state(state)
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
