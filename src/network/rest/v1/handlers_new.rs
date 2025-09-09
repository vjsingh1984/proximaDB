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
use crate::network::rest::proto_json::{ProtoJson, ProtoApiResponse};
use crate::proto::proximadb::{
    VectorSearchRequest, VectorOperationResponse, VectorBatchRequest,
    CollectionRequest, CollectionResponse, CollectionOperation,
};

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
pub async fn vector_search_aligned(
    State(state): State<AppState>,
    Json(request): Json<VectorSearchRequest>,
) -> ApiResult<JsonResponse<VectorOperationResponse>> {
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
    
    // Direct delegation to UnifiedHandlers - no conversion needed
    let response = state
        .unified_handlers
        .handle_vector_search(request)
        .await
        .map_err(|e| {
            error!("Vector search failed: {}", e);
            ApiError::Internal(e.to_string())
        })?;
    
    Ok(JsonResponse(response))
}

/// Aligned vector batch operation handler
pub async fn vector_batch_aligned(
    State(state): State<AppState>,
    Json(request): Json<VectorBatchRequest>,
) -> ApiResult<JsonResponse<VectorOperationResponse>> {
    info!(
        "Vector batch operation for collection: {}, {} records",
        request.collection_id,
        request.records.len()
    );
    
    // Validate request
    if request.collection_id.is_empty() {
        return Err(ApiError::InvalidArgument("Collection ID is required".to_string()));
    }
    
    if request.records.is_empty() {
        return Err(ApiError::InvalidArgument("At least one record is required".to_string()));
    }
    
    // Direct delegation to UnifiedHandlers
    let response = state
        .unified_handlers
        .handle_vector_batch(request)
        .await
        .map_err(|e| {
            error!("Vector batch operation failed: {}", e);
            ApiError::Internal(e.to_string())
        })?;
    
    Ok(JsonResponse(response))
}

/// Aligned collection operation handler
pub async fn collection_operation_aligned(
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
pub async fn health_check_aligned(
    State(state): State<AppState>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    // Check system health
    let health_status = state
        .unified_handlers
        .get_health_status()
        .await
        .map_err(|e| ApiError::Internal(format!("Health check failed: {}", e)))?;
    
    Ok(JsonResponse(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "services": health_status
    })))
}

/// Get collection by ID with aligned error handling
pub async fn get_collection_aligned(
    Path(collection_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<JsonResponse<CollectionResponse>> {
    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument("Collection ID is required".to_string()));
    }
    
    let request = CollectionRequest {
        operation: CollectionOperation::Get as i32,
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

pub async fn list_collections_aligned(
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
        operation: CollectionOperation::List as i32,
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
pub async fn delete_collection_aligned(
    Path(collection_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<JsonResponse<CollectionResponse>> {
    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument("Collection ID is required".to_string()));
    }
    
    let request = CollectionRequest {
        operation: CollectionOperation::Delete as i32,
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
    Json(request): Json<VectorSearchRequest>,
) -> ApiResult<JsonResponse<ProtoApiResponse<VectorOperationResponse>>> {
    let start_time = std::time::Instant::now();
    let request_id = uuid::Uuid::new_v4().to_string();
    
    info!(
        "Vector search request {} for collection: {}",
        request_id, request.collection_id
    );
    
    // Execute search
    match state.unified_handlers.handle_vector_search(request).await {
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