/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! REST API handlers that delegate to unified services
//! Proto-aligned API structure for consistency with gRPC

use anyhow::Result;
use axum::{
    extract::{Json, Path, State, Query},
    http::StatusCode,
    response::Json as JsonResponse,
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::handlers::{UnifiedHandlers, conversions};

/// Shared application state for REST handlers
#[derive(Clone)]
pub struct AppState {
    pub unified_handlers: Arc<UnifiedHandlers>,
}

// ============================================================================
// UNIFIED API REQUEST/RESPONSE TYPES - Aligned with Proto
// ============================================================================

/// Unified collection operation request - aligned with proto CollectionRequest
#[derive(Debug, Deserialize, Serialize)]
pub struct CollectionOperationRequest {
    pub operation: String, // "create", "update", "get", "list", "delete"
    pub collection_id: Option<String>,
    pub collection_name: Option<String>,
    pub config: Option<CollectionConfig>,
    pub query_params: Option<HashMap<String, String>>, // limit, offset, filters
    pub options: Option<HashMap<String, bool>>,        // force, include_stats
}

/// Collection config - aligned with proto CollectionConfig
#[derive(Debug, Deserialize, Serialize)]
pub struct CollectionConfig {
    pub name: String,
    pub dimension: i32,
    pub distance_metric: Option<String>,            // "cosine", "euclidean", "dot_product" - defaults to "cosine"
    pub storage_engine: Option<String>,             // "viper", "lsm" - defaults to "viper"
    pub primary_indexing_algorithm: Option<String>, // "hnsw", "ivf", "flat", "pq", "annoy" - defaults to "hnsw"
    pub filterable_columns: Option<Vec<FilterableColumn>>,
    pub index_configs: Option<Vec<IndexConfiguration>>,
    pub quantization_config: Option<QuantizationConfig>,
    pub primary_index_name: Option<String>,
    pub enable_automatic_index_selection: Option<bool>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub owner: Option<String>,
}

/// Filterable column spec - aligned with proto
#[derive(Debug, Deserialize, Serialize)]
pub struct FilterableColumn {
    pub name: String,
    pub data_type: String, // "string", "integer", "float", "boolean", "datetime"
    pub indexed: bool,
    pub supports_range: bool,
    pub estimated_cardinality: Option<i32>,
}

/// Index configuration - aligned with proto IndexConfig
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexConfiguration {
    pub index_name: String,
    pub algorithm: String,
    pub update_mode: String, // "synchronous", "asynchronous", "hybrid_mode"
    pub async_update_timeout_ms: Option<i64>,
    pub async_update_batch_size: Option<i32>,
    pub enable_background_optimization: Option<bool>,
    pub hnsw_config: Option<HnswConfig>,
    pub ivf_config: Option<IvfConfig>,
    pub flat_config: Option<FlatConfig>,
    pub pq_config: Option<PqConfig>,
    pub annoy_config: Option<AnnoyConfig>,
    pub build_concurrency: Option<i32>,
    pub memory_limit_mb: Option<i64>,
    pub checkpoint_interval_ms: Option<i32>,
    pub is_primary: Option<bool>,
    pub use_cases: Option<Vec<String>>,
    pub selectivity_threshold: Option<f32>,
}

/// HNSW configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HnswConfig {
    pub m: i32,
    pub ef_construction: i32,
    pub ef_search: i32,
    pub max_partition_size: i32,
    pub adaptive_parameters: bool,
    pub use_simd: bool,
    pub memory_limit_mb: i32,
    pub lazy_loading: bool,
    pub prune_connections: i32,
    pub level_multiplier: f32,
}

/// IVF configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IvfConfig {
    pub n_lists: i32,
    pub n_probe: i32,
    pub quantization_bits: i32,
    pub use_pq: bool,
    pub pq_subspaces: i32,
    pub train_on_insert: bool,
    pub min_train_size: i32,
}

/// Flat index configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FlatConfig {
    pub enable_simd: bool,
    pub batch_size: i32,
    pub enable_parallel_search: bool,
}

/// Product Quantization configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PqConfig {
    pub subvectors: i32,
    pub bits_per_subvector: i32,
    pub training_sample_count: i32,
    pub enable_reranking: bool,
}

/// Annoy configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnnoyConfig {
    pub n_trees: i32,
    pub search_k: i32,
    pub max_leaf_size: i32,
    pub enable_mmap: bool,
}

/// Quantization configuration - aligned with proto
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuantizationConfig {
    pub enabled: bool,
    pub storage_quantization: Option<StorageQuantizationConfig>,
    pub index_quantization: Option<IndexQuantizationConfig>,
    pub search_quantization: Option<SearchQuantizationConfig>,
    pub compression_ratio_target: Option<f32>,
    pub validation: Option<QuantizationValidation>,
}

/// Storage quantization config
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageQuantizationConfig {
    pub enabled: bool,
    pub level: QuantizationLevel,
    pub codebook_id: Option<String>,
    pub progressive_quantization: bool,
    pub storage_compatibility: String,
}

/// Index quantization config
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexQuantizationConfig {
    pub enabled: bool,
    pub strategies: Vec<IndexQuantizationStrategy>,
    pub auto_select_strategy: bool,
}

/// Search quantization config
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchQuantizationConfig {
    pub enabled: bool,
    pub default_level: QuantizationLevel,
    pub adaptive_precision: bool,
    pub accuracy_threshold: f32,
    pub candidate_multiplier: i32,
}

/// Quantization level
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuantizationLevel {
    pub level_type: String, // "none", "uniform", "pq", "scalar", "binary", "custom"
    pub bits: Option<i32>,
    pub scale: Option<f32>,
    pub offset: Option<f32>,
    pub num_subvectors: Option<i32>,
    pub bits_per_code: Option<i32>,
    pub codebook_id: Option<String>,
    pub adaptive_subvectors: Option<bool>,
    pub threshold: Option<f32>,
    pub sign_based: Option<bool>,
    pub clamp_values: Option<bool>,
    pub type_id: Option<String>,
    pub bits_per_element: Option<i32>,
    pub config: Option<HashMap<String, String>>,
}

/// Index quantization strategy
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexQuantizationStrategy {
    pub index_name: String,
    pub level: QuantizationLevel,
    pub build_async: bool,
    pub codebook_id: Option<String>,
}

/// Quantization validation
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuantizationValidation {
    pub accuracy_threshold: f32,
    pub validation_sample_size: i32,
    pub enable_quality_monitoring: bool,
    pub retraining_threshold: f32,
}

/// Collection response - aligned with proto CollectionResponse
#[derive(Debug, Serialize)]
pub struct CollectionResponse {
    pub success: bool,
    pub operation: String,
    pub collection: Option<Collection>,
    pub collections: Option<Vec<Collection>>,
    pub affected_count: i64,
    pub total_count: Option<i64>,
    pub metadata: HashMap<String, String>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    pub processing_time_us: i64,
}

/// Collection data
#[derive(Debug, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub config: CollectionConfig,
    pub stats: CollectionStats,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Collection statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct CollectionStats {
    pub vector_count: i64,
    pub index_size_bytes: i64,
    pub data_size_bytes: i64,
}

/// Vector batch request - aligned with proto VectorBatchRequest
#[derive(Debug, Deserialize, Serialize)]
pub struct VectorBatchRequest {
    pub collection_id: String,
    pub vectors: Vec<VectorData>,
    pub batch_timeout_ms: Option<i64>,
    pub request_id: Option<String>,
}

/// Vector data for batch operations
#[derive(Debug, Deserialize, Serialize)]
pub struct VectorData {
    pub id: Option<String>,
    pub vector: Vec<f32>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub expires_at: Option<i64>, // For TTL/delete
}

/// Vector search request - aligned with proto VectorSearchRequest
#[derive(Debug, Deserialize, Serialize)]
pub struct VectorSearchRequest {
    pub collection_id: String,
    pub queries: Vec<SearchQuery>,
    pub top_k: i32,
    pub distance_metric_override: Option<String>,
    pub search_parameters: Option<SearchParameters>,
    pub include_fields: Option<IncludeFields>,
    pub search_optimization: Option<SearchOptimization>,
}

/// Search query
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchQuery {
    pub vector: Vec<f32>,
    pub id: Option<String>,
    pub metadata_filter: Option<MetadataFilter>,
}

/// Metadata filter
#[derive(Debug, Deserialize, Serialize)]
pub struct MetadataFilter {
    pub conditions: Vec<FilterCondition>,
    pub operator: String, // "and", "or", "not"
}

/// Filter condition
#[derive(Debug, Deserialize, Serialize)]
pub struct FilterCondition {
    pub field_name: String,
    pub operation: String, // "equals", "greater_than", "less_than", "in", etc.
    pub value: serde_json::Value,
}

/// Search parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchParameters {
    pub ef_search: Option<i32>,
    pub max_connections: Option<i32>,
    pub n_probe: Option<i32>,
    pub enable_reranking: Option<bool>,
    pub batch_size: Option<i32>,
    pub timeout_ms: Option<i64>,
    pub accuracy_threshold: Option<f32>,
    pub enable_parallel_search: Option<bool>,
    pub thread_count: Option<i32>,
}

/// Include fields in search results
#[derive(Debug, Deserialize, Serialize)]
pub struct IncludeFields {
    pub vector: bool,
    pub metadata: bool,
    pub score: bool,
    pub rank: bool,
}

/// Search optimization hints
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchOptimization {
    pub top_k: Option<u32>,
    pub filters: Option<HashMap<String, serde_json::Value>>,
    pub accuracy_threshold: Option<f32>,
    pub include_expired: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub enable_two_stage: Option<bool>,
    pub quantization_hint: Option<QuantizationHint>,
    pub enable_clustering_hint: Option<bool>,
    pub enable_metadata_filtering_hint: Option<bool>,
    pub custom_hints: Option<HashMap<String, serde_json::Value>>,
}

/// Quantization hint for search
#[derive(Debug, Deserialize, Serialize)]
pub struct QuantizationHint {
    pub hint_type: String, // "none", "binary", "scalar", "product", "uniform"
    pub parameters: Option<serde_json::Value>,
}

/// Vector operation response - aligned with proto VectorOperationResponse
#[derive(Debug, Serialize)]
pub struct VectorOperationResponse {
    pub success: bool,
    pub operation: String,
    pub metrics: OperationMetrics,
    pub results: Option<Vec<SearchResult>>,
    pub vector_ids: Vec<String>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
}

/// Operation metrics
#[derive(Debug, Serialize)]
pub struct OperationMetrics {
    pub total_processed: i64,
    pub successful_count: i64,
    pub failed_count: i64,
    pub updated_count: i64,
    pub processing_time_us: i64,
    pub wal_write_time_us: i64,
    pub index_update_time_us: i64,
}

/// Search result
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub vector: Option<Vec<f32>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub rank: Option<i32>,
}

/// API response wrapper
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ApiError>,
    pub message: Option<String>,
}

/// API error
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            message: None,
        }
    }

    pub fn error(code: String, message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(ApiError {
                code,
                message,
                details: None,
            }),
            message: None,
        }
    }
}

// ============================================================================
// ROUTER CONFIGURATION
// ============================================================================

/// Create REST router with unified proto-aligned endpoints
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Health and metrics
        .route("/health", get(health_check))
        .route("/metrics", get(get_metrics))
        // Unified collection endpoint (proto-aligned)
        .route("/api/v1/collection", post(collection_operation))
        // Unified vector endpoints (proto-aligned)
        .route("/api/v1/vector/batch", post(vector_batch))
        .route("/api/v1/vector/search", post(vector_search))
        // Convenience endpoints for common operations
        .route("/api/v1/vector/:collection_id/:vector_id", get(get_vector))
        .route("/api/v1/vector/:collection_id/:vector_id", delete(delete_vector))
        // Internal testing endpoints (WARNING: NOT FOR PRODUCTION USE)
        .route("/internal/flush", post(internal_flush_all))
        .route("/internal/flush/:collection_id", post(internal_flush_collection))
        .with_state(state)
}

// ============================================================================
// HANDLER IMPLEMENTATIONS
// ============================================================================

/// Health check endpoint
pub async fn health_check(
    State(state): State<AppState>,
) -> Result<JsonResponse<ApiResponse<HashMap<String, serde_json::Value>>>, StatusCode> {
    match state.unified_handlers.vector_service.health_check().await {
        Ok(health_bytes) => {
            match serde_json::from_slice::<serde_json::Value>(&health_bytes) {
                Ok(health_data) => {
                    let mut response_data = HashMap::new();
                    response_data.insert("status".to_string(), json!("healthy"));
                    response_data.insert("service".to_string(), json!("proximadb-rest"));
                    response_data.insert("version".to_string(), json!(env!("CARGO_PKG_VERSION")));
                    response_data.insert("vector_service".to_string(), health_data);
                    
                    Ok(JsonResponse(ApiResponse::success(response_data)))
                }
                Err(_) => {
                    let mut response_data = HashMap::new();
                    response_data.insert("status".to_string(), json!("degraded"));
                    response_data.insert("service".to_string(), json!("proximadb-rest"));
                    response_data.insert("version".to_string(), json!(env!("CARGO_PKG_VERSION")));
                    response_data.insert("error".to_string(), json!("Failed to parse health data"));
                    
                    Ok(JsonResponse(ApiResponse::success(response_data)))
                }
            }
        }
        Err(e) => {
            let mut response_data = HashMap::new();
            response_data.insert("status".to_string(), json!("unhealthy"));
            response_data.insert("service".to_string(), json!("proximadb-rest"));
            response_data.insert("version".to_string(), json!(env!("CARGO_PKG_VERSION")));
            response_data.insert("error".to_string(), json!(e.to_string()));
            
            Ok(JsonResponse(ApiResponse::error(
                "Service unhealthy".to_string(),
                "SERVICE_UNHEALTHY".to_string(),
            )))
        }
    }
}

/// Unified collection operation handler - thin adapter to UnifiedHandlers
pub async fn collection_operation(
    State(state): State<AppState>,
    Json(request): Json<CollectionOperationRequest>,
) -> Result<JsonResponse<CollectionResponse>, StatusCode> {
    // Convert REST request to proto request
    let proto_request = conversions::CollectionRequestBuilder::from_json(serde_json::to_value(&request).unwrap())
        .map_err(|e| {
            tracing::error!("Failed to parse collection request: {}", e);
            StatusCode::BAD_REQUEST
        })?;
    
    // Delegate to unified handlers
    let proto_response = state.unified_handlers
        .handle_collection_operation(proto_request)
        .await
        .map_err(|e| {
            tracing::error!("Collection operation failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    // Convert proto response to REST response
    let response = CollectionResponse {
        success: proto_response.success,
        operation: conversions::collection_operation_to_string(proto_response.operation),
        collection: proto_response.collection.map(|c| convert_from_proto_collection(c)),
        collections: if proto_response.collections.is_empty() { 
            None 
        } else { 
            Some(proto_response.collections.into_iter().map(convert_from_proto_collection).collect()) 
        },
        affected_count: proto_response.affected_count,
        total_count: proto_response.total_count,
        metadata: proto_response.metadata.into_iter().collect(),
        error_message: proto_response.error_message,
        error_code: proto_response.error_code,
        processing_time_us: proto_response.processing_time_us,
    };
    
    Ok(JsonResponse(response))
}

/// Unified vector batch handler - thin adapter to UnifiedHandlers
pub async fn vector_batch(
    State(state): State<AppState>,
    Json(request): Json<VectorBatchRequest>,
) -> Result<JsonResponse<VectorOperationResponse>, StatusCode> {
    // Convert REST request to proto request
    let proto_request = conversions::VectorBatchRequestBuilder::from_json(serde_json::to_value(&request).unwrap())
        .map_err(|e| {
            tracing::error!("Failed to parse vector batch request: {}", e);
            StatusCode::BAD_REQUEST
        })?;
    
    // Delegate to unified handlers
    let proto_response = state.unified_handlers
        .handle_vector_batch(proto_request)
        .await
        .map_err(|e| {
            tracing::error!("Vector batch operation failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    // Convert proto response to REST response  
    let metrics = proto_response.metrics.unwrap_or(crate::proto::proximadb::OperationMetrics {
        total_processed: 0,
        successful_count: 0,
        failed_count: 0,
        updated_count: 0,
        processing_time_us: 0,
        wal_write_time_us: 0,
        index_update_time_us: 0,
    });
    
    let response = VectorOperationResponse {
        success: proto_response.success,
        operation: conversions::vector_operation_to_string(proto_response.operation),
        metrics: OperationMetrics {
            total_processed: metrics.total_processed,
            successful_count: metrics.successful_count,
            failed_count: metrics.failed_count,
            updated_count: metrics.updated_count,
            processing_time_us: metrics.processing_time_us,
            wal_write_time_us: metrics.wal_write_time_us,
            index_update_time_us: metrics.index_update_time_us,
        },
        results: None,
        vector_ids: proto_response.vector_ids,
        error_message: proto_response.error_message,
        error_code: proto_response.error_code,
    };
    
    Ok(JsonResponse(response))
}

/// Unified vector search handler - thin adapter to UnifiedHandlers
pub async fn vector_search(
    State(state): State<AppState>,
    Json(request): Json<VectorSearchRequest>,
) -> Result<JsonResponse<VectorOperationResponse>, StatusCode> {
    // Convert REST request to proto request
    let proto_request = conversions::VectorSearchRequestBuilder::from_json(serde_json::to_value(&request).unwrap())
        .map_err(|e| {
            tracing::error!("Failed to parse vector search request: {}", e);
            StatusCode::BAD_REQUEST
        })?;
    
    // Delegate to unified handlers
    let proto_response = state.unified_handlers
        .handle_vector_search(proto_request)
        .await
        .map_err(|e| {
            tracing::error!("Vector search operation failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    // Convert proto response to REST response
    let results = if let Some(result_payload) = proto_response.result_payload {
        match result_payload {
            crate::proto::proximadb::vector_operation_response::ResultPayload::CompactResults(compact) => {
                compact.results.into_iter()
                    .map(|r| SearchResult {
                        id: r.id.unwrap_or_default(),
                        score: r.score,
                        vector: if r.vector.is_empty() { None } else { Some(r.vector) },
                        metadata: if r.metadata.is_empty() { None } else { 
                            Some(r.metadata.into_iter()
                                .map(|item| (item.key, serde_json::Value::String(item.value)))
                                .collect()) 
                        },
                        rank: r.rank,
                    })
                    .collect()
            }
            _ => vec![],
        }
    } else {
        vec![]
    };
    
    let metrics = proto_response.metrics.unwrap_or(crate::proto::proximadb::OperationMetrics {
        total_processed: 0,
        successful_count: 0,
        failed_count: 0,
        updated_count: 0,
        processing_time_us: 0,
        wal_write_time_us: 0,
        index_update_time_us: 0,
    });
    
    let response = VectorOperationResponse {
        success: proto_response.success,
        operation: "search".to_string(),
        metrics: OperationMetrics {
            total_processed: metrics.total_processed,
            successful_count: metrics.successful_count,
            failed_count: 0,
            updated_count: 0,
            processing_time_us: metrics.processing_time_us,
            wal_write_time_us: 0,
            index_update_time_us: 0,
        },
        results: Some(results),
        vector_ids: vec![],
        error_message: proto_response.error_message,
        error_code: proto_response.error_code,
    };
    
    Ok(JsonResponse(response))
}

/// Get single vector by ID - thin adapter to UnifiedHandlers
pub async fn get_vector(
    State(state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    let include_vector = params.get("include_vector")
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(true);
    let include_metadata = params.get("include_metadata")
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(true);
    
    // Delegate to unified handlers
    match state.unified_handlers.get_vector(
        &collection_id,
        &vector_id,
        include_vector,
        include_metadata,
    ).await {
        Ok(result_bytes) => {
            match serde_json::from_slice::<serde_json::Value>(&result_bytes) {
                Ok(response) => {
                    if response.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                        if let Some(results) = response.get("results").and_then(|r| r.as_array()) {
                            if let Some(first_result) = results.first() {
                                return Ok(JsonResponse(ApiResponse::success(first_result.clone())));
                            }
                        }
                        Ok(JsonResponse(ApiResponse::error(
                            "Vector not found".to_string(),
                            "NOT_FOUND".to_string(),
                        )))
                    } else {
                        Ok(JsonResponse(ApiResponse::error(
                            response.get("error_message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Vector not found")
                                .to_string(),
                            "NOT_FOUND".to_string(),
                        )))
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to parse get vector response: {:?}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("Get vector failed: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Delete single vector by ID - thin adapter to UnifiedHandlers
pub async fn delete_vector(
    State(state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<JsonResponse<ApiResponse<HashMap<String, serde_json::Value>>>, StatusCode> {
    // Delegate to unified handlers
    match state.unified_handlers.delete_vector(&collection_id, &vector_id).await {
        Ok(result_bytes) => {
            match serde_json::from_slice::<serde_json::Value>(&result_bytes) {
                Ok(response) => {
                    let mut result_data = HashMap::new();
                    result_data.insert("deleted".to_string(), json!(response.get("success").and_then(|v| v.as_bool()).unwrap_or(false)));
                    result_data.insert("vector_id".to_string(), json!(vector_id));
                    result_data.insert("collection_id".to_string(), json!(collection_id));
                    
                    if response.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                        Ok(JsonResponse(ApiResponse::success(result_data)))
                    } else {
                        Ok(JsonResponse(ApiResponse::error(
                            response.get("error_message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Failed to delete vector")
                                .to_string(),
                            "DELETE_FAILED".to_string(),
                        )))
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to parse delete response: {:?}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("Delete vector failed: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get metrics endpoint - thin adapter to UnifiedHandlers
pub async fn get_metrics(
    State(state): State<AppState>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    // Delegate to unified handlers
    match state.unified_handlers.get_metrics().await {
        Ok(metrics_bytes) => {
            match serde_json::from_slice::<serde_json::Value>(&metrics_bytes) {
                Ok(metrics_data) => {
                    Ok(JsonResponse(ApiResponse::success(metrics_data)))
                }
                Err(e) => {
                    tracing::error!("Failed to parse metrics: {:?}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("Get metrics failed: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}


/// Internal flush all collections (testing only) - thin adapter to UnifiedHandlers
pub async fn internal_flush_all(
    State(state): State<AppState>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    tracing::warn!("⚠️ INTERNAL FLUSH ENDPOINT CALLED - THIS IS FOR TESTING ONLY");
    
    // Delegate to unified handlers
    match state.unified_handlers.force_flush_all().await {
        Ok(stats) => {
            Ok(JsonResponse(ApiResponse::success(stats)))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// Internal flush specific collection (testing only) - thin adapter to UnifiedHandlers
pub async fn internal_flush_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    tracing::warn!("⚠️ INTERNAL FLUSH ENDPOINT CALLED FOR {} - THIS IS FOR TESTING ONLY", collection_id);
    
    // Delegate to unified handlers
    match state.unified_handlers.force_flush_collection(&collection_id).await {
        Ok(stats) => {
            Ok(JsonResponse(ApiResponse::success(stats)))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Convert index config to proto
fn convert_index_config_to_proto(config: IndexConfiguration) -> crate::proto::proximadb::IndexConfig {
    use crate::proto::proximadb;
    
    let algorithm = match config.algorithm.as_str() {
        "hnsw" => proximadb::IndexingAlgorithm::Hnsw as i32,
        "ivf" => proximadb::IndexingAlgorithm::Ivf as i32,
        "flat" => proximadb::IndexingAlgorithm::Flat as i32,
        "pq" => proximadb::IndexingAlgorithm::Pq as i32,
        "annoy" => proximadb::IndexingAlgorithm::Annoy as i32,
        _ => proximadb::IndexingAlgorithm::Hnsw as i32,
    };
    
    let update_mode = match config.update_mode.as_str() {
        "synchronous" => proximadb::IndexUpdateMode::Synchronous as i32,
        "asynchronous" => proximadb::IndexUpdateMode::Asynchronous as i32,
        "hybrid_mode" => proximadb::IndexUpdateMode::HybridMode as i32,
        _ => proximadb::IndexUpdateMode::Synchronous as i32,
    };
    
    proximadb::IndexConfig {
        index_name: config.index_name,
        algorithm,
        update_mode,
        async_update_timeout_ms: config.async_update_timeout_ms,
        async_update_batch_size: config.async_update_batch_size,
        enable_background_optimization: config.enable_background_optimization.unwrap_or(true),
        hnsw_config: config.hnsw_config.map(|c| proximadb::HnswConfig {
            m: c.m,
            ef_construction: c.ef_construction,
            ef_search: c.ef_search,
            max_partition_size: c.max_partition_size,
            adaptive_parameters: c.adaptive_parameters,
            use_simd: c.use_simd,
            memory_limit_mb: c.memory_limit_mb,
            lazy_loading: c.lazy_loading,
            prune_connections: c.prune_connections,
            level_multiplier: c.level_multiplier,
        }),
        ivf_config: config.ivf_config.map(|c| proximadb::IvfConfig {
            n_lists: c.n_lists,
            n_probe: c.n_probe,
            quantization_bits: c.quantization_bits,
            use_pq: c.use_pq,
            pq_subspaces: c.pq_subspaces,
            train_on_insert: c.train_on_insert,
            min_train_size: c.min_train_size,
        }),
        flat_config: config.flat_config.map(|c| proximadb::FlatConfig {
            enable_simd: c.enable_simd,
            batch_size: c.batch_size,
            enable_parallel_search: c.enable_parallel_search,
        }),
        pq_config: config.pq_config.map(|c| proximadb::PqConfig {
            subvectors: c.subvectors,
            bits_per_subvector: c.bits_per_subvector,
            training_sample_count: c.training_sample_count,
            enable_reranking: c.enable_reranking,
        }),
        annoy_config: config.annoy_config.map(|c| proximadb::AnnoyConfig {
            n_trees: c.n_trees,
            search_k: c.search_k,
            max_leaf_size: c.max_leaf_size,
            enable_mmap: c.enable_mmap,
        }),
        build_concurrency: config.build_concurrency,
        memory_limit_mb: config.memory_limit_mb,
        checkpoint_interval_ms: config.checkpoint_interval_ms,
        is_primary: config.is_primary.unwrap_or(false),
        use_cases: config.use_cases.unwrap_or_default(),
        selectivity_threshold: config.selectivity_threshold,
    }
}

/// Convert quantization config to proto
fn convert_quantization_config_to_proto(config: QuantizationConfig) -> crate::proto::proximadb::QuantizationConfig {
    use crate::proto::proximadb;
    
    proximadb::QuantizationConfig {
        enabled: config.enabled,
        storage_quantization: config.storage_quantization.map(|sq| {
            proximadb::StorageQuantizationConfig {
                enabled: sq.enabled,
                level: Some(convert_quantization_level_to_proto(sq.level)),
                codebook_id: sq.codebook_id,
                progressive_quantization: sq.progressive_quantization,
                storage_compatibility: match sq.storage_compatibility.as_str() {
                    "viper_only" => proximadb::StorageEngineCompatibility::ViperOnly as i32,
                    "all_engines" => proximadb::StorageEngineCompatibility::AllEngines as i32,
                    "lsm_and_viper" => proximadb::StorageEngineCompatibility::LsmAndViper as i32,
                    _ => proximadb::StorageEngineCompatibility::ViperOnly as i32,
                },
            }
        }),
        index_quantization: config.index_quantization.map(|iq| {
            proximadb::IndexQuantizationConfig {
                enabled: iq.enabled,
                strategies: iq.strategies.into_iter().map(|s| {
                    proximadb::IndexQuantizationStrategy {
                        index_name: s.index_name,
                        level: Some(convert_quantization_level_to_proto(s.level)),
                        build_async: s.build_async,
                        codebook_id: s.codebook_id,
                    }
                }).collect(),
                auto_select_strategy: iq.auto_select_strategy,
            }
        }),
        search_quantization: config.search_quantization.map(|sq| {
            proximadb::SearchQuantizationConfig {
                enabled: sq.enabled,
                default_level: Some(convert_quantization_level_to_proto(sq.default_level)),
                adaptive_precision: sq.adaptive_precision,
                accuracy_threshold: sq.accuracy_threshold,
                candidate_multiplier: sq.candidate_multiplier,
            }
        }),
        compression_ratio_target: config.compression_ratio_target.unwrap_or(1.0),
        validation: config.validation.map(|v| {
            proximadb::QuantizationValidation {
                accuracy_threshold: v.accuracy_threshold,
                validation_sample_size: v.validation_sample_size,
                enable_quality_monitoring: v.enable_quality_monitoring,
                retraining_threshold: v.retraining_threshold,
            }
        }),
    }
}

/// Convert quantization level to proto
fn convert_quantization_level_to_proto(level: QuantizationLevel) -> crate::proto::proximadb::QuantizationLevel {
    use crate::proto::proximadb::{self, quantization_level::LevelType};
    
    let level_type = match level.level_type.as_str() {
        "none" => Some(LevelType::None(proximadb::NoQuantization {})),
        "uniform" => Some(LevelType::Uniform(proximadb::UniformQuantization {
            bits: level.bits.unwrap_or(8),
            scale: level.scale,
            offset: level.offset,
        })),
        "pq" => Some(LevelType::Pq(proximadb::ProductQuantization {
            bits_per_code: level.bits_per_code.unwrap_or(8),
            num_subvectors: level.num_subvectors.unwrap_or(8),
            codebook_id: level.codebook_id,
            adaptive_subvectors: false,
        })),
        "scalar" => Some(LevelType::Scalar(proximadb::ScalarQuantization {
            bits: level.bits.unwrap_or(8),
            scale: level.scale.unwrap_or(1.0),
            offset: level.offset.unwrap_or(0.0),
            clamp_values: false,
        })),
        "binary" => Some(LevelType::Binary(proximadb::BinaryQuantization {
            threshold: level.threshold,
            sign_based: level.sign_based.unwrap_or(false),
        })),
        "custom" => Some(LevelType::Custom(proximadb::CustomQuantization {
            type_id: level.type_id.unwrap_or_default(),
            bits_per_element: level.bits_per_element.unwrap_or(8),
            config: level.config.unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, v))
                .collect(),
        })),
        _ => Some(LevelType::None(proximadb::NoQuantization {})),
    };
    
    proximadb::QuantizationLevel { level_type }
}

/// Convert REST config to proto config
fn convert_to_proto_config(config: CollectionConfig) -> Result<crate::proto::proximadb::CollectionConfig, StatusCode> {
    use crate::proto::proximadb;
    
    // Apply defaults for optional fields
    let distance_metric_str = config.distance_metric.as_deref().unwrap_or("cosine");
    let distance_metric = match distance_metric_str {
        "cosine" => proximadb::DistanceMetric::Cosine as i32,
        "euclidean" => proximadb::DistanceMetric::Euclidean as i32,
        "dot_product" => proximadb::DistanceMetric::DotProduct as i32,
        _ => proximadb::DistanceMetric::Cosine as i32,
    };
    
    let storage_engine_str = config.storage_engine.as_deref().unwrap_or("viper");
    let storage_engine = match storage_engine_str {
        "viper" => proximadb::StorageEngine::Viper as i32,
        "lsm" => proximadb::StorageEngine::Lsm as i32,
        _ => proximadb::StorageEngine::Viper as i32,
    };
    
    let indexing_algorithm_str = config.primary_indexing_algorithm.as_deref().unwrap_or("hnsw");
    let indexing_algorithm = match indexing_algorithm_str {
        "hnsw" => proximadb::IndexingAlgorithm::Hnsw as i32,
        "ivf" => proximadb::IndexingAlgorithm::Ivf as i32,
        "flat" => proximadb::IndexingAlgorithm::Flat as i32,
        "pq" => proximadb::IndexingAlgorithm::Pq as i32,
        "annoy" => proximadb::IndexingAlgorithm::Annoy as i32,
        _ => proximadb::IndexingAlgorithm::Hnsw as i32,
    };
    
    // Convert filterable columns
    let filterable_columns = config.filterable_columns.unwrap_or_default()
        .into_iter()
        .map(|col| {
            let data_type = match col.data_type.as_str() {
                "string" => proximadb::FilterableDataType::FilterableString as i32,
                "integer" => proximadb::FilterableDataType::FilterableInteger as i32,
                "float" => proximadb::FilterableDataType::FilterableFloat as i32,
                "boolean" => proximadb::FilterableDataType::FilterableBoolean as i32,
                "datetime" => proximadb::FilterableDataType::FilterableDatetime as i32,
                "array_string" => proximadb::FilterableDataType::FilterableArrayString as i32,
                "array_integer" => proximadb::FilterableDataType::FilterableArrayInteger as i32,
                "array_float" => proximadb::FilterableDataType::FilterableArrayFloat as i32,
                _ => proximadb::FilterableDataType::FilterableString as i32,
            };
            
            proximadb::FilterableColumnSpec {
                name: col.name,
                data_type,
                indexed: col.indexed,
                supports_range: col.supports_range,
                estimated_cardinality: col.estimated_cardinality,
            }
        })
        .collect();
    
    // Convert index configs
    let index_configs = config.index_configs.unwrap_or_default()
        .into_iter()
        .map(|idx| convert_index_config_to_proto(idx))
        .collect();
    
    // Convert quantization config
    let quantization_config = config.quantization_config.map(convert_quantization_config_to_proto);
    
    Ok(proximadb::CollectionConfig {
        name: config.name,
        dimension: config.dimension,
        distance_metric,
        storage_engine,
        primary_indexing_algorithm: indexing_algorithm,
        filterable_columns,
        index_configs,
        quantization_config,
        primary_index_name: config.primary_index_name.unwrap_or_default(),
        enable_automatic_index_selection: config.enable_automatic_index_selection.unwrap_or(false),
        description: config.description,
        tags: config.tags.unwrap_or_default(),
        owner: config.owner,
    })
}

/// Convert index config from proto
fn convert_index_config_from_proto(config: crate::proto::proximadb::IndexConfig) -> IndexConfiguration {
    IndexConfiguration {
        index_name: config.index_name,
        algorithm: match config.algorithm {
            x if x == crate::proto::proximadb::IndexingAlgorithm::Hnsw as i32 => "hnsw",
            x if x == crate::proto::proximadb::IndexingAlgorithm::Ivf as i32 => "ivf",
            x if x == crate::proto::proximadb::IndexingAlgorithm::Flat as i32 => "flat",
            x if x == crate::proto::proximadb::IndexingAlgorithm::Pq as i32 => "pq",
            x if x == crate::proto::proximadb::IndexingAlgorithm::Annoy as i32 => "annoy",
            _ => "hnsw",
        }.to_string(),
        update_mode: match config.update_mode {
            x if x == crate::proto::proximadb::IndexUpdateMode::Synchronous as i32 => "synchronous",
            x if x == crate::proto::proximadb::IndexUpdateMode::Asynchronous as i32 => "asynchronous",
            x if x == crate::proto::proximadb::IndexUpdateMode::HybridMode as i32 => "hybrid_mode",
            _ => "synchronous",
        }.to_string(),
        async_update_timeout_ms: config.async_update_timeout_ms,
        async_update_batch_size: config.async_update_batch_size,
        enable_background_optimization: Some(config.enable_background_optimization),
        hnsw_config: config.hnsw_config.map(|c| HnswConfig {
            m: c.m,
            ef_construction: c.ef_construction,
            ef_search: c.ef_search,
            max_partition_size: c.max_partition_size,
            adaptive_parameters: c.adaptive_parameters,
            use_simd: c.use_simd,
            memory_limit_mb: c.memory_limit_mb,
            lazy_loading: c.lazy_loading,
            prune_connections: c.prune_connections,
            level_multiplier: c.level_multiplier,
        }),
        ivf_config: config.ivf_config.map(|c| IvfConfig {
            n_lists: c.n_lists,
            n_probe: c.n_probe,
            quantization_bits: c.quantization_bits,
            use_pq: c.use_pq,
            pq_subspaces: c.pq_subspaces,
            train_on_insert: c.train_on_insert,
            min_train_size: c.min_train_size,
        }),
        flat_config: config.flat_config.map(|c| FlatConfig {
            enable_simd: c.enable_simd,
            batch_size: c.batch_size,
            enable_parallel_search: c.enable_parallel_search,
        }),
        pq_config: config.pq_config.map(|c| PqConfig {
            subvectors: c.subvectors,
            bits_per_subvector: c.bits_per_subvector,
            training_sample_count: c.training_sample_count,
            enable_reranking: c.enable_reranking,
        }),
        annoy_config: config.annoy_config.map(|c| AnnoyConfig {
            n_trees: c.n_trees,
            search_k: c.search_k,
            max_leaf_size: c.max_leaf_size,
            enable_mmap: c.enable_mmap,
        }),
        build_concurrency: config.build_concurrency,
        memory_limit_mb: config.memory_limit_mb,
        checkpoint_interval_ms: config.checkpoint_interval_ms,
        is_primary: Some(config.is_primary),
        use_cases: Some(config.use_cases),
        selectivity_threshold: config.selectivity_threshold,
    }
}

/// Convert quantization config from proto
fn convert_quantization_config_from_proto(config: crate::proto::proximadb::QuantizationConfig) -> QuantizationConfig {
    QuantizationConfig {
        enabled: config.enabled,
        storage_quantization: config.storage_quantization.map(|sq| StorageQuantizationConfig {
            enabled: sq.enabled,
            level: sq.level.map(convert_quantization_level_from_proto).unwrap_or(QuantizationLevel {
                level_type: "none".to_string(),
                bits: None,
                scale: None,
                offset: None,
                num_subvectors: None,
                bits_per_code: None,
                codebook_id: None,
                adaptive_subvectors: None,
                threshold: None,
                sign_based: None,
                clamp_values: None,
                type_id: None,
                bits_per_element: None,
                config: None,
            }),
            codebook_id: sq.codebook_id,
            progressive_quantization: sq.progressive_quantization,
            storage_compatibility: match sq.storage_compatibility {
                x if x == crate::proto::proximadb::StorageEngineCompatibility::ViperOnly as i32 => "viper_only",
                x if x == crate::proto::proximadb::StorageEngineCompatibility::AllEngines as i32 => "all_engines",
                x if x == crate::proto::proximadb::StorageEngineCompatibility::LsmAndViper as i32 => "lsm_and_viper",
                _ => "viper_only",
            }.to_string(),
        }),
        index_quantization: config.index_quantization.map(|iq| IndexQuantizationConfig {
            enabled: iq.enabled,
            strategies: iq.strategies.into_iter().map(|s| IndexQuantizationStrategy {
                index_name: s.index_name,
                level: s.level.map(convert_quantization_level_from_proto).unwrap_or(QuantizationLevel {
                    level_type: "none".to_string(),
                    bits: None,
                    scale: None,
                    offset: None,
                    num_subvectors: None,
                    bits_per_code: None,
                    codebook_id: None,
                    adaptive_subvectors: None,
                    threshold: None,
                    sign_based: None,
                    clamp_values: None,
                    type_id: None,
                    bits_per_element: None,
                    config: None,
                }),
                build_async: s.build_async,
                codebook_id: s.codebook_id,
            }).collect(),
            auto_select_strategy: iq.auto_select_strategy,
        }),
        search_quantization: config.search_quantization.map(|sq| SearchQuantizationConfig {
            enabled: sq.enabled,
            default_level: sq.default_level.map(convert_quantization_level_from_proto).unwrap_or(QuantizationLevel {
                level_type: "none".to_string(),
                bits: None,
                scale: None,
                offset: None,
                num_subvectors: None,
                bits_per_code: None,
                codebook_id: None,
                adaptive_subvectors: None,
                threshold: None,
                sign_based: None,
                clamp_values: None,
                type_id: None,
                bits_per_element: None,
                config: None,
            }),
            adaptive_precision: sq.adaptive_precision,
            accuracy_threshold: sq.accuracy_threshold,
            candidate_multiplier: sq.candidate_multiplier,
        }),
        compression_ratio_target: Some(config.compression_ratio_target),
        validation: config.validation.map(|v| QuantizationValidation {
            accuracy_threshold: v.accuracy_threshold,
            validation_sample_size: v.validation_sample_size,
            enable_quality_monitoring: v.enable_quality_monitoring,
            retraining_threshold: v.retraining_threshold,
        }),
    }
}

/// Convert quantization level from proto
fn convert_quantization_level_from_proto(level: crate::proto::proximadb::QuantizationLevel) -> QuantizationLevel {
    use crate::proto::proximadb::quantization_level::LevelType;
    
    match level.level_type {
        Some(LevelType::None(_)) => QuantizationLevel {
            level_type: "none".to_string(),
            bits: None,
            scale: None,
            offset: None,
            num_subvectors: None,
            bits_per_code: None,
            codebook_id: None,
            adaptive_subvectors: None,
            threshold: None,
            sign_based: None,
            clamp_values: None,
            type_id: None,
            bits_per_element: None,
            config: None,
        },
        Some(LevelType::Uniform(u)) => QuantizationLevel {
            level_type: "uniform".to_string(),
            bits: Some(u.bits),
            scale: u.scale,
            offset: u.offset,
            num_subvectors: None,
            bits_per_code: None,
            codebook_id: None,
            adaptive_subvectors: None,
            threshold: None,
            sign_based: None,
            clamp_values: None,
            type_id: None,
            bits_per_element: None,
            config: None,
        },
        Some(LevelType::Pq(p)) => QuantizationLevel {
            level_type: "pq".to_string(),
            bits: None,
            scale: None,
            offset: None,
            num_subvectors: Some(p.num_subvectors),
            bits_per_code: Some(p.bits_per_code),
            codebook_id: p.codebook_id,
            adaptive_subvectors: Some(p.adaptive_subvectors),
            threshold: None,
            sign_based: None,
            clamp_values: None,
            type_id: None,
            bits_per_element: None,
            config: None,
        },
        Some(LevelType::Scalar(s)) => QuantizationLevel {
            level_type: "scalar".to_string(),
            bits: Some(s.bits),
            scale: Some(s.scale),
            offset: Some(s.offset),
            num_subvectors: None,
            bits_per_code: None,
            codebook_id: None,
            adaptive_subvectors: None,
            threshold: None,
            sign_based: None,
            clamp_values: Some(s.clamp_values),
            type_id: None,
            bits_per_element: None,
            config: None,
        },
        Some(LevelType::Binary(b)) => QuantizationLevel {
            level_type: "binary".to_string(),
            bits: None,
            scale: None,
            offset: None,
            num_subvectors: None,
            bits_per_code: None,
            codebook_id: None,
            adaptive_subvectors: None,
            threshold: b.threshold,
            sign_based: Some(b.sign_based),
            clamp_values: None,
            type_id: None,
            bits_per_element: None,
            config: None,
        },
        Some(LevelType::Custom(c)) => QuantizationLevel {
            level_type: "custom".to_string(),
            bits: None,
            scale: None,
            offset: None,
            num_subvectors: None,
            bits_per_code: None,
            codebook_id: None,
            adaptive_subvectors: None,
            threshold: None,
            sign_based: None,
            clamp_values: None,
            type_id: Some(c.type_id),
            bits_per_element: Some(c.bits_per_element),
            config: Some(c.config),
        },
        None => QuantizationLevel {
            level_type: "none".to_string(),
            bits: None,
            scale: None,
            offset: None,
            num_subvectors: None,
            bits_per_code: None,
            codebook_id: None,
            adaptive_subvectors: None,
            threshold: None,
            sign_based: None,
            clamp_values: None,
            type_id: None,
            bits_per_element: None,
            config: None,
        },
    }
}

/// Convert proto collection to REST collection
fn convert_from_proto_collection(proto: crate::proto::proximadb::Collection) -> Collection {
    let config = proto.config.unwrap_or_default();
    
    let distance_metric = match config.distance_metric {
        x if x == crate::proto::proximadb::DistanceMetric::Cosine as i32 => "cosine",
        x if x == crate::proto::proximadb::DistanceMetric::Euclidean as i32 => "euclidean",
        x if x == crate::proto::proximadb::DistanceMetric::DotProduct as i32 => "dot_product",
        _ => "cosine",
    }.to_string();
    
    let storage_engine = match config.storage_engine {
        x if x == crate::proto::proximadb::StorageEngine::Viper as i32 => "viper",
        x if x == crate::proto::proximadb::StorageEngine::Lsm as i32 => "lsm",
        _ => "viper",
    }.to_string();
    
    let indexing_algorithm = match config.primary_indexing_algorithm {
        x if x == crate::proto::proximadb::IndexingAlgorithm::Hnsw as i32 => "hnsw",
        x if x == crate::proto::proximadb::IndexingAlgorithm::Ivf as i32 => "ivf",
        x if x == crate::proto::proximadb::IndexingAlgorithm::Flat as i32 => "flat",
        x if x == crate::proto::proximadb::IndexingAlgorithm::Pq as i32 => "pq",
        x if x == crate::proto::proximadb::IndexingAlgorithm::Annoy as i32 => "annoy",
        _ => "hnsw",
    }.to_string();
    
    Collection {
        id: proto.id,
        config: CollectionConfig {
            name: config.name,
            dimension: config.dimension,
            distance_metric: Some(distance_metric),
            storage_engine: Some(storage_engine),
            primary_indexing_algorithm: Some(indexing_algorithm),
            filterable_columns: Some(config.filterable_columns.into_iter().map(|col| {
                FilterableColumn {
                    name: col.name,
                    data_type: match col.data_type {
                        x if x == crate::proto::proximadb::FilterableDataType::FilterableString as i32 => "string",
                        x if x == crate::proto::proximadb::FilterableDataType::FilterableInteger as i32 => "integer",
                        x if x == crate::proto::proximadb::FilterableDataType::FilterableFloat as i32 => "float",
                        x if x == crate::proto::proximadb::FilterableDataType::FilterableBoolean as i32 => "boolean",
                        x if x == crate::proto::proximadb::FilterableDataType::FilterableDatetime as i32 => "datetime",
                        x if x == crate::proto::proximadb::FilterableDataType::FilterableArrayString as i32 => "array_string",
                        x if x == crate::proto::proximadb::FilterableDataType::FilterableArrayInteger as i32 => "array_integer",
                        x if x == crate::proto::proximadb::FilterableDataType::FilterableArrayFloat as i32 => "array_float",
                        _ => "string",
                    }.to_string(),
                    indexed: col.indexed,
                    supports_range: col.supports_range,
                    estimated_cardinality: col.estimated_cardinality,
                }
            }).collect()),
            index_configs: Some(config.index_configs.into_iter().map(convert_index_config_from_proto).collect()),
            quantization_config: config.quantization_config.map(convert_quantization_config_from_proto),
            primary_index_name: if config.primary_index_name.is_empty() { None } else { Some(config.primary_index_name) },
            enable_automatic_index_selection: Some(config.enable_automatic_index_selection),
            description: config.description,
            tags: Some(config.tags),
            owner: config.owner,
        },
        stats: CollectionStats {
            vector_count: proto.stats.as_ref().map(|s| s.vector_count).unwrap_or(0),
            index_size_bytes: proto.stats.as_ref().map(|s| s.index_size_bytes).unwrap_or(0),
            data_size_bytes: proto.stats.as_ref().map(|s| s.data_size_bytes).unwrap_or(0),
        },
        created_at: proto.created_at,
        updated_at: proto.updated_at,
    }
}

