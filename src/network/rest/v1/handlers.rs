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
use crate::utils::uuid::Uuid;
use crate::proto::proximadb_v1::{
    CollectionRequest, CollectionResponse, CollectionOperation,
};
use crate::proto::proximadb_v1::{
    VectorSearchRequest as V1VectorSearchRequest,
    VectorBatchRequest as V1VectorBatchRequest,
    VectorOperationResponse as V1VectorOperationResponse,
};
use serde::{Deserialize, Serialize};
use crate::query::explain::ExplainPlan;
use crate::query::QueryEngine;

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
) -> ApiResult<JsonResponse<V1VectorOperationResponse>> {
    info!(
        "Vector search request for collection: {}, top_k: {}",
        request.collection_id, request.top_k
    );
    
    // Validate request
    if request.collection_id.is_empty() {
        return Err(ApiError::InvalidArgument("Collection ID is required".to_string()));
    }
    
    if request.queries.is_empty() {
        return Err(ApiError::InvalidArgument("At least one query is required".to_string()));
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

    Ok(JsonResponse(v1_resp))
}

/// Aligned vector batch operation handler
pub async fn vector_batch(
    State(state): State<AppState>,
    Json(request): Json<V1VectorBatchRequest>,
) -> ApiResult<JsonResponse<V1VectorOperationResponse>> {
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
    let v1_resp = state
        .unified_handlers
        .handle_vector_batch_v1(request)
        .await
        .map_err(|e| {
            error!("Vector batch operation failed: {}", e);
            ApiError::Internal(e.to_string())
        })?;

    Ok(JsonResponse(v1_resp))
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
        operation,
        request.collection_id
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
) -> ApiResult<JsonResponse<CollectionResponse>> {
    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument("Collection ID is required".to_string()));
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
        return Err(ApiError::InvalidArgument("Collection ID is required".to_string()));
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

/// Example using ProtoApiResponse for consistent structure
pub async fn vector_search_with_metadata(
    State(state): State<AppState>,
    Json(request): Json<V1VectorSearchRequest>,
) -> ApiResult<JsonResponse<ProtoApiResponse<V1VectorOperationResponse>>> {
    let start_time = std::time::Instant::now();
    let request_id = Uuid::new_v4().to_string();
    
    info!(
        "Vector search request {} for collection: {}",
        request_id, request.collection_id
    );
    
    // Execute search
    match state.unified_handlers.handle_vector_search_v1(request).await {
        Ok(response) => {
            let elapsed = start_time.elapsed();
            
            let api_response = ProtoApiResponse::success(response)
                .with_metadata(crate::network::rest::proto_json::ResponseMetadata {
                    request_id,
                    processing_time_ms: elapsed.as_millis() as u64,
                    server_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                });
            
            Ok(JsonResponse(api_response))
        }
        Err(e) => {
            error!("Vector search failed: {}", e);
            let api_response = ProtoApiResponse::error(ApiError::Internal(e.to_string()));
            Ok(JsonResponse(api_response))
        }
    }
}

/// SQL query request structure
#[derive(Debug, Serialize, Deserialize)]
pub struct SqlQueryRequest {
    /// SQL query string
    pub query: String,
    /// Optional parameters for parameterized queries
    pub parameters: Option<Vec<serde_json::Value>>,
    /// Optional collection to use as default context
    pub collection: Option<String>,
    /// Optional timeout in milliseconds
    pub timeout_ms: Option<u64>,
}

/// SQL query response structure
#[derive(Debug, Serialize, Deserialize)]
pub struct SqlQueryResponse {
    /// Result rows as JSON objects
    pub rows: Vec<serde_json::Value>,
    /// Column metadata (name, type)
    pub columns: Vec<SqlColumnInfo>,
    /// Number of rows returned
    pub row_count: usize,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
    /// Request ID for tracing
    pub request_id: String,
}

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
) -> ApiResult<JsonResponse<ProtoApiResponse<SqlQueryResponse>>> {
    let start_time = std::time::Instant::now();
    let request_id = Uuid::new_v4().to_string();
    
    info!(
        "SQL query request {} with query: {}",
        request_id,
        request.query.chars().take(100).collect::<String>()
    );
    
    // Validate request
    if request.query.trim().is_empty() {
        return Err(ApiError::InvalidArgument("SQL query cannot be empty".to_string()));
    }

    // Execute SQL query through unified handlers
    match state
        .unified_handlers
        .execute_sql_query(
            request.query,
            request.parameters,
            request.collection,
        )
        .await
    {
        Ok(sql_result) => {
            let execution_time_ms = start_time.elapsed().as_millis() as u64;
            
            // Convert SqlQueryResult to our response format
            let columns = sql_result
                .columns
                .into_iter()
                .map(|(name, data_type)| SqlColumnInfo { name, data_type })
                .collect();

            let response = SqlQueryResponse {
                rows: sql_result.rows,
                columns,
                row_count: sql_result.row_count,
                execution_time_ms,
                request_id: request_id.clone(),
            };

            let api_response = ProtoApiResponse::success(response)
                .with_execution_time(execution_time_ms)
                .with_request_id(&request_id);
            
            info!(
                "SQL query {} completed in {}ms, returned {} rows",
                request_id, execution_time_ms, sql_result.row_count
            );

            Ok(JsonResponse(api_response))
        }
        Err(e) => {
            error!("SQL query {} failed: {}", request_id, e);
            let api_response = ProtoApiResponse::error(ApiError::Internal(e.to_string()));
            Ok(JsonResponse(api_response))
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
) -> ApiResult<JsonResponse<ProtoApiResponse<ExplainQueryResponse>>> {
    let request_id = Uuid::new_v4().to_string();
    
    info!(
        "EXPLAIN query request {} for query: {}",
        request_id,
        request.query.chars().take(100).collect::<String>()
    );
    
    // Validate request
    if request.query.trim().is_empty() {
        return Err(ApiError::InvalidArgument("SQL query cannot be empty".to_string()));
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

    let api_response = ProtoApiResponse::success(response)
        .with_request_id(&request_id);
    
    info!("EXPLAIN query {} completed", request_id);
    Ok(JsonResponse(api_response))
}

// Note: EXPLAIN now uses QueryEngine::explain_sql() for real plans/hints.

/// Create router with all REST endpoints
pub fn create_router(state: AppState) -> axum::Router {
    use axum::routing::{get, post, delete};
    
    axum::Router::new()
        // Vector operations
        .route("/api/v1/search", post(vector_search))
        .route("/api/v1/vectors/batch", post(vector_batch))
        .route("/api/v1/progressive/search/:collection_id", 
            post(crate::network::rest::progressive_search_handler::progressive_search_handler))
        
        // SQL query execution
        .route("/api/v1/sql/execute", post(execute_sql))
        .route("/api/v1/sql/explain", post(explain_sql))
        
        // Collection operations
        .route("/api/v1/collections", post(collection_operation))
        .route("/api/v1/collections", get(list_collections))
        .route("/api/v1/collections/:collection_id", get(get_collection))
        .route("/api/v1/collections/:collection_id", delete(delete_collection))
        
        // Health check
        .route("/health", get(health_check))
        
        // With metadata endpoints
        .route("/api/v1/search/with_metadata", post(vector_search_with_metadata))
        
        // Graph database endpoints
        .nest("/api/v1/graph", crate::network::rest::v1::graph::create_graph_router())
        
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
