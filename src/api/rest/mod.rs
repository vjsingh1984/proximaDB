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

//! REST API implementation for ProximaDB

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;
use chrono::Utc;

use crate::storage::{StorageEngine, CollectionMetadata, MetadataStore};
use crate::storage::metadata::{CollectionFlushConfig, GlobalFlushDefaults};
use crate::core::VectorRecord;
use crate::compute::algorithms::SearchResult;
use crate::services::{VectorService, CollectionService, ServiceError};
use crate::services::migration::{MigrationService, StrategyMigrationRequest, StrategyMigrationResponse};

/// API application state
#[derive(Clone)]
pub struct ApiState {
    pub storage: Arc<tokio::sync::RwLock<StorageEngine>>,
    pub vector_service: VectorService,
    pub collection_service: CollectionService,
    pub migration_service: Option<MigrationService>,
}

/// Collection creation request
#[derive(Debug, Deserialize)]
pub struct CreateCollectionRequest {
    pub name: String,
    pub dimension: u32,
    pub distance_metric: Option<String>,
    pub indexing_algorithm: Option<String>,
    pub allow_client_ids: Option<bool>,  // Allow client-provided IDs
    pub config: Option<HashMap<String, serde_json::Value>>,
    
    // WAL flush configuration (optional - uses global defaults if not specified)
    pub max_wal_age_hours: Option<f64>,    // Max WAL age in hours (default: 24 hours)
    pub max_wal_size_mb: Option<f64>,      // Max WAL size in MB (default: 128MB)  
    pub max_vector_count: Option<u64>,     // Max vectors before flush (default: 1M)
    pub flush_priority: Option<u8>,        // Flush priority 1-100 (default: 50)
    pub enable_background_flush: Option<bool>, // Enable background flushing (default: true)
}

/// Collection response
#[derive(Debug, Serialize)]
pub struct CollectionResponse {
    pub id: String,
    pub name: String,
    pub dimension: u32,
    pub distance_metric: String,
    pub indexing_algorithm: String,
    pub created_at: String,
    pub updated_at: String,
    pub vector_count: u64,
    pub total_size_bytes: u64,
    pub config: HashMap<String, serde_json::Value>,
    
    // Effective flush configuration (resolved with global defaults)
    pub flush_config: FlushConfigResponse,
}

/// Flush configuration response
#[derive(Debug, Serialize)]
pub struct FlushConfigResponse {
    pub max_wal_age_hours: f64,     // Effective max WAL age in hours
    pub max_wal_size_mb: f64,       // Effective max WAL size in MB
    pub max_vector_count: u64,      // Effective max vector count
    pub flush_priority: u8,         // Effective flush priority
    pub enable_background_flush: bool, // Effective background flush setting
    pub using_global_defaults: bool,   // True if using all global defaults
}

impl From<CollectionMetadata> for CollectionResponse {
    fn from(metadata: CollectionMetadata) -> Self {
        // Get global defaults for flush configuration
        let global_defaults = GlobalFlushDefaults::default();
        
        // Resolve effective flush configuration
        let effective_config = metadata.flush_config
            .as_ref()
            .map(|config| config.effective_config(&global_defaults))
            .unwrap_or_else(|| {
                // If no collection-specific config, use all global defaults
                CollectionFlushConfig::default().effective_config(&global_defaults)
            });
        
        let using_global_defaults = metadata.flush_config.is_none();
        
        let flush_config = FlushConfigResponse {
            max_wal_age_hours: effective_config.max_wal_age_secs as f64 / 3600.0, // Convert seconds to hours
            max_wal_size_mb: effective_config.max_wal_size_bytes as f64 / (1024.0 * 1024.0), // Convert bytes to MB
            max_vector_count: effective_config.max_vector_count,
            flush_priority: effective_config.flush_priority,
            enable_background_flush: effective_config.enable_background_flush,
            using_global_defaults,
        };
        
        Self {
            id: metadata.id,
            name: metadata.name,
            dimension: metadata.dimension,
            distance_metric: metadata.distance_metric,
            indexing_algorithm: metadata.indexing_algorithm,
            created_at: metadata.created_at.to_rfc3339(),
            updated_at: metadata.updated_at.to_rfc3339(),
            vector_count: metadata.vector_count,
            total_size_bytes: metadata.total_size_bytes,
            config: metadata.config,
            flush_config,
        }
    }
}

/// Vector insert request
#[derive(Debug, Deserialize)]
pub struct InsertVectorRequest {
    pub id: Option<String>,  // Optional client-provided ID
    pub vector: Vec<f32>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Vector response
#[derive(Debug, Serialize)]
pub struct VectorResponse {
    pub id: String,
    pub collection_id: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: String,
}

impl From<VectorRecord> for VectorResponse {
    fn from(record: VectorRecord) -> Self {
        Self {
            id: record.id.to_string(),
            collection_id: record.collection_id,
            vector: record.vector,
            metadata: record.metadata,
            timestamp: record.timestamp.to_rfc3339(),
        }
    }
}

/// Vector search request
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub vector: Vec<f32>,
    pub k: Option<usize>,
    pub filter: Option<HashMap<String, serde_json::Value>>,
}

/// Search result response
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultResponse {
    pub id: String,  // Client ID or server UUID
    pub vector_id: String,  // Server UUID
    pub score: f32,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl From<SearchResult> for SearchResultResponse {
    fn from(result: SearchResult) -> Self {
        // Extract client_id from metadata if available, otherwise use vector_id
        let id = result.metadata.as_ref()
            .and_then(|meta| meta.get("client_id"))
            .and_then(|v| v.as_str())
            .unwrap_or(&result.vector_id)
            .to_string();
            
        Self {
            id,
            vector_id: result.vector_id,
            score: result.score,
            metadata: result.metadata,
        }
    }
}

/// Batch insert request
#[derive(Debug, Deserialize)]
pub struct BatchInsertRequest {
    pub vectors: Vec<InsertVectorRequest>,
}

/// Batch insert response
#[derive(Debug, Serialize)]
pub struct BatchInsertResponse {
    pub inserted_ids: Vec<String>,
    pub total_count: usize,
}

/// Batch search request
#[derive(Debug, Deserialize)]
pub struct BatchSearchRequest {
    pub queries: Vec<BatchSearchQuery>,
}

/// Individual search query in batch
#[derive(Debug, Deserialize)]
pub struct BatchSearchQuery {
    pub collection_id: String,
    pub vector: Vec<f32>,
    pub k: Option<usize>,
    pub filter: Option<HashMap<String, serde_json::Value>>,
}

/// Batch search response
#[derive(Debug, Serialize)]
pub struct BatchSearchResponse {
    pub results: Vec<Vec<SearchResultResponse>>,
    pub total_queries: usize,
}

/// Bulk update request
#[derive(Debug, Deserialize)]
pub struct BulkUpdateRequest {
    pub vectors: Vec<InsertVectorRequest>,
}

/// Bulk update response
#[derive(Debug, Serialize)]
pub struct BulkUpdateResponse {
    pub updated_ids: Vec<String>,
    pub total_count: usize,
}

/// Bulk delete request
#[derive(Debug, Deserialize)]
pub struct BulkDeleteRequest {
    pub ids: Vec<String>,
}

/// Bulk delete response
#[derive(Debug, Serialize)]
pub struct BulkDeleteResponse {
    pub deleted_ids: Vec<String>,
    pub total_count: usize,
}

/// Search response wrapper
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub matches: Vec<SearchResultResponse>,
    pub total_count: usize,
}

/// Delete collection response
#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub success: bool,
    pub message: String,
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime: String,
}

/// API Router setup
pub fn create_router(storage: Arc<tokio::sync::RwLock<StorageEngine>>) -> Router {
    let state = ApiState { 
        vector_service: VectorService::new(storage.clone()),
        collection_service: CollectionService::new(storage.clone()),
        migration_service: None, // Will be enabled when metadata store is available
        storage,
    };

    Router::new()
        // Health endpoints
        .route("/health", get(health_check))
        .route("/health/ready", get(readiness_check))
        .route("/health/live", get(liveness_check))
        
        // Collection management
        .route("/collections", get(list_collections))
        .route("/collections", post(create_collection))
        .route("/collections/ids", get(list_collection_ids))
        .route("/collections/names", get(list_collection_names))
        .route("/collections/name_to_id/:collection_name", get(get_collection_id_by_name))
        .route("/collections/id_to_name/:collection_id", get(get_collection_name_by_id))
        
        // Collection access by ID (explicit)
        .route("/collections/id/:collection_id", get(get_collection_by_id))
        .route("/collections/id/:collection_id", delete(delete_collection_by_id))
        
        // Collection access by name (explicit - user friendly)
        .route("/collections/name/:collection_name", get(get_collection_by_name))
        .route("/collections/name/:collection_name", delete(delete_collection_by_name))
        
        // Backwards compatible (auto-detect)
        .route("/collections/:collection_identifier", get(get_collection))
        .route("/collections/:collection_identifier", delete(delete_collection))
        
        // Vector operations by ID (explicit)
        .route("/collections/id/:collection_id/vectors", post(insert_vector_by_id))
        .route("/collections/id/:collection_id/vectors/:vector_id", get(get_vector_by_id))
        .route("/collections/id/:collection_id/vectors/:vector_id", delete(delete_vector_by_id))
        .route("/collections/id/:collection_id/vectors/batch", post(batch_insert_vectors_by_id))
        
        // Vector operations by name (explicit - user friendly)
        .route("/collections/name/:collection_name/vectors", post(insert_vector_by_name))
        .route("/collections/name/:collection_name/vectors/:vector_id", get(get_vector_by_name))
        .route("/collections/name/:collection_name/vectors/:vector_id", delete(delete_vector_by_name))
        .route("/collections/name/:collection_name/vectors/batch", post(batch_insert_vectors_by_name))
        
        // Backwards compatible vector operations (auto-detect)
        .route("/collections/:collection_identifier/vectors", post(insert_vector))
        .route("/collections/:collection_identifier/vectors", put(bulk_update_vectors))
        .route("/collections/:collection_identifier/vectors", delete(bulk_delete_vectors))
        .route("/collections/:collection_identifier/vectors/:vector_id", get(get_vector))
        .route("/collections/:collection_identifier/vectors/:vector_id", delete(delete_vector))
        .route("/collections/:collection_identifier/vectors/batch", post(batch_insert_vectors))
        
        // Search operations by ID (explicit)
        .route("/collections/id/:collection_id/search", post(search_vectors_by_id))
        .route("/collections/id/:collection_id/search/client_id/:client_id", get(get_vector_by_client_id_by_id))
        
        // Search operations by name (explicit - user friendly)
        .route("/collections/name/:collection_name/search", post(search_vectors_by_name))
        .route("/collections/name/:collection_name/search/client_id/:client_id", get(get_vector_by_client_id_by_name))
        
        // Backwards compatible search operations (auto-detect)
        .route("/collections/:collection_identifier/search", post(search_vectors))
        .route("/collections/:collection_identifier/search/client_id/:client_id", get(get_vector_by_client_id))
        
        // Global batch operations
        .route("/batch/search", post(batch_search_vectors))
        
        // Index management
        .route("/collections/:collection_id/index/stats", get(get_index_stats))
        .route("/collections/:collection_id/index/optimize", post(optimize_index))
        
        // Collection strategy migration
        .route("/collections/:collection_id/migrate", post(migrate_collection_strategy))
        .route("/collections/:collection_id/migrate/history", get(get_migration_history))
        .route("/collections/:collection_id/strategy", get(get_current_strategy))
        
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: "0.1.0".to_string(),
        uptime: "unknown".to_string(), // TODO: Track actual uptime
    })
}

/// Readiness check endpoint
async fn readiness_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ready".to_string(),
        version: "0.1.0".to_string(),
        uptime: "unknown".to_string(),
    })
}

/// Liveness check endpoint
async fn liveness_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "alive".to_string(),
        version: "0.1.0".to_string(),
        uptime: "unknown".to_string(),
    })
}

/// List all collections
async fn list_collections(
    State(state): State<ApiState>,
) -> Result<Json<Vec<CollectionResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match storage.list_collections().await {
        Ok(collections) => {
            let response: Vec<CollectionResponse> = collections
                .into_iter()
                .map(CollectionResponse::from)
                .collect();
            Ok(Json(response))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Create a new collection
async fn create_collection(
    State(state): State<ApiState>,
    Json(request): Json<CreateCollectionRequest>,
) -> Result<Json<CollectionResponse>, (StatusCode, Json<ErrorResponse>)> {
    tracing::info!("🌐 REST API: Creating collection '{}' with dimension {}", 
                  request.name, request.dimension);
    
    // Build flush configuration from request
    let flush_config = if request.max_wal_age_hours.is_some() 
                        || request.max_wal_size_mb.is_some() 
                        || request.max_vector_count.is_some() 
                        || request.flush_priority.is_some() 
                        || request.enable_background_flush.is_some() {
        
        Some(CollectionFlushConfig {
            max_wal_age_secs: request.max_wal_age_hours.map(|hours| (hours * 3600.0) as u64),
            max_wal_size_bytes: request.max_wal_size_mb.map(|mb| (mb * 1024.0 * 1024.0) as usize),
            max_vector_count: request.max_vector_count,
            flush_priority: request.flush_priority,
            enable_background_flush: request.enable_background_flush,
        })
    } else {
        None // Use global defaults
    };
    
    tracing::debug!("🔧 Collection flush config: {:?}", flush_config);
    
    match state.collection_service.create_collection(
        request.name,
        request.dimension,
        request.distance_metric,
        request.indexing_algorithm,
        request.config,
        flush_config,
    ).await {
        Ok(metadata) => Ok(Json(CollectionResponse::from(metadata))),
        Err(e) => {
            let (status_code, error_type) = match e {
                ServiceError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "validation_error"),
                ServiceError::Storage(_) => (StatusCode::INTERNAL_SERVER_ERROR, "storage_error"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            };
            Err((
                status_code,
                Json(ErrorResponse {
                    error: error_type.to_string(),
                    message: e.to_string(),
                }),
            ))
        }
    }
}

/// Get collection details
async fn get_collection(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<CollectionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match storage.get_collection_metadata(&collection_id).await {
        Ok(Some(metadata)) => Ok(Json(CollectionResponse::from(metadata))),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "collection_not_found".to_string(),
                message: format!("Collection '{}' not found", collection_id),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Get collection by ID (explicit)
async fn get_collection_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<CollectionResponse>, (StatusCode, Json<ErrorResponse>)> {
    get_collection(State(state), Path(collection_id)).await
}

/// Get collection by name (explicit)
async fn get_collection_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
) -> Result<Json<CollectionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    get_collection(State(state), Path(collection_id)).await
}

/// Delete a collection
async fn delete_collection(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.write().await;
    
    match storage.delete_collection(&collection_id).await {
        Ok(true) => Ok(Json(DeleteResponse {
            success: true,
            message: format!("Collection '{}' deleted successfully", collection_id),
        })),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "collection_not_found".to_string(),
                message: format!("Collection '{}' not found", collection_id),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Insert a vector into a collection
async fn insert_vector(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<InsertVectorRequest>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.vector_service.insert_vector(
        &collection_id,
        request.id,
        request.vector,
        request.metadata,
    ).await {
        Ok(vector_record) => Ok(Json(VectorResponse::from(vector_record))),
        Err(e) => {
            let (status_code, error_type) = match e {
                ServiceError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "validation_error"),
                ServiceError::InvalidDimension { .. } => (StatusCode::BAD_REQUEST, "dimension_error"),
                ServiceError::CollectionNotFound(_) => (StatusCode::NOT_FOUND, "collection_not_found"),
                ServiceError::Storage(_) => (StatusCode::INTERNAL_SERVER_ERROR, "storage_error"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            };
            Err((
                status_code,
                Json(ErrorResponse {
                    error: error_type.to_string(),
                    message: e.to_string(),
                }),
            ))
        }
    }
}

/// Get a vector by ID
async fn get_vector(
    State(state): State<ApiState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match storage.read(&collection_id, &vector_id).await {
        Ok(Some(record)) => Ok(Json(VectorResponse::from(record))),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "vector_not_found".to_string(),
                message: format!("Vector '{}' not found in collection '{}'", vector_id, collection_id),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Delete a vector by ID
async fn delete_vector(
    State(state): State<ApiState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.write().await;
    
    match storage.soft_delete(&collection_id, &vector_id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "vector_not_found".to_string(),
                message: format!("Vector '{}' not found in collection '{}'", vector_id, collection_id),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Search for vectors
async fn search_vectors(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let k = request.k.unwrap_or(10);
    
    match state.vector_service.search_vectors(
        &collection_id,
        &request.vector,
        k,
        request.filter,
    ).await {
        Ok(search_results) => {
            let matches: Vec<SearchResultResponse> = search_results
                .into_iter()
                .map(SearchResultResponse::from)
                .collect();
            Ok(Json(SearchResponse {
                matches: matches.clone(),
                total_count: matches.len(),
            }))
        }
        Err(e) => {
            let (status_code, error_type) = match e {
                ServiceError::CollectionNotFound(_) => (StatusCode::NOT_FOUND, "collection_not_found"),
                ServiceError::InvalidDimension { .. } => (StatusCode::BAD_REQUEST, "dimension_error"),
                ServiceError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "validation_error"),
                ServiceError::Storage(_) => (StatusCode::INTERNAL_SERVER_ERROR, "storage_error"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "search_error"),
            };
            Err((
                status_code,
                Json(ErrorResponse {
                    error: error_type.to_string(),
                    message: e.to_string(),
                }),
            ))
        }
    }
}

/// Get index statistics
async fn get_index_stats(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<HashMap<String, serde_json::Value>>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match storage.get_index_stats(&collection_id).await {
        Ok(Some(stats)) => Ok(Json(stats)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "collection_not_found".to_string(),
                message: format!("Collection '{}' not found", collection_id),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Optimize index
async fn optimize_index(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match storage.optimize_index(&collection_id).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Batch insert vectors into a collection
async fn batch_insert_vectors(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<BatchInsertRequest>,
) -> Result<Json<BatchInsertResponse>, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!("🌐 REST API: Starting batch_insert_vectors for collection_id={}, vectors_count={}", 
                 collection_id, request.vectors.len());
    
    tracing::debug!("🔒 REST API: Acquiring storage write lock");
    let storage = state.storage.write().await;
    tracing::debug!("✅ REST API: Acquired storage write lock");
    
    // Convert API requests to VectorRecord objects
    tracing::debug!("🔄 REST API: Converting {} API requests to VectorRecord objects", request.vectors.len());
    let mut vector_records = Vec::with_capacity(request.vectors.len());
    for (index, vector_request) in request.vectors.into_iter().enumerate() {
        // Use client-provided ID or generate one
        let vector_id = vector_request.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        
        // Prepare metadata
        let metadata = vector_request.metadata.unwrap_or_default();

        let vector_record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector: vector_request.vector,
            metadata,
            timestamp: Utc::now(),
            expires_at: None, // No expiration by default
        };
        vector_records.push(vector_record);
        
        if (index + 1) % 100 == 0 || index + 1 == vector_records.capacity() {
            tracing::debug!("📋 REST API: Prepared {}/{} vector records", index + 1, vector_records.capacity());
        }
    }
    
    tracing::debug!("✅ REST API: Completed conversion to {} VectorRecord objects", vector_records.len());
    tracing::debug!("🚀 REST API: Calling storage.batch_write()");
    
    match storage.batch_write(vector_records).await {
        Ok(inserted_ids) => {
            tracing::debug!("✅ REST API: storage.batch_write() completed successfully with {} inserted IDs", inserted_ids.len());
            
            let inserted_id_strings: Vec<String> = inserted_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect();
            
            tracing::debug!("🎉 REST API: Returning successful BatchInsertResponse with {} IDs", inserted_id_strings.len());
            Ok(Json(BatchInsertResponse {
                total_count: inserted_id_strings.len(),
                inserted_ids: inserted_id_strings,
            }))
        }
        Err(e) => {
            tracing::error!("❌ REST API: storage.batch_write() failed with error: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "batch_insert_error".to_string(),
                    message: e.to_string(),
                }),
            ))
        }
    }
}

/// Batch search across multiple collections and queries
async fn batch_search_vectors(
    State(state): State<ApiState>,
    Json(request): Json<BatchSearchRequest>,
) -> Result<Json<BatchSearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    // Convert API requests to storage layer requests
    let mut batch_requests = Vec::with_capacity(request.queries.len());
    for query in request.queries {
        let batch_search_request = crate::core::BatchSearchRequest {
            collection_id: query.collection_id,
            query_vector: query.vector,
            k: query.k.unwrap_or(10),
            filter: query.filter,
        };
        batch_requests.push(batch_search_request);
    }
    
    match storage.batch_search(batch_requests).await {
        Ok(search_results) => {
            let response_results: Vec<Vec<SearchResultResponse>> = search_results
                .into_iter()
                .map(|result_group| {
                    result_group
                        .into_iter()
                        .map(SearchResultResponse::from)
                        .collect()
                })
                .collect();
            
            Ok(Json(BatchSearchResponse {
                total_queries: response_results.len(),
                results: response_results,
            }))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "batch_search_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Get vector by client ID
async fn get_vector_by_client_id(
    State(state): State<ApiState>,
    Path((collection_id, client_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!(
        "🔍 get_vector_by_client_id: collection_id='{}', client_id='{}'", 
        collection_id, client_id
    );
    let storage = state.storage.read().await;
    
    // Clone client_id for use in closure and error messages
    let client_id_for_filter = client_id.clone();
    
    // Search for vectors with matching client_id in metadata
    let filter_fn = move |metadata: &HashMap<String, serde_json::Value>| {
        metadata.get("client_id")
            .and_then(|v| v.as_str())
            .map(|id| id == client_id_for_filter)
            .unwrap_or(false)
    };
    
    // Get collection metadata to determine vector dimension
    tracing::debug!("🔍 Getting collection metadata for collection_id='{}'", collection_id);
    let collection_metadata = match storage.get_collection_metadata(&collection_id).await {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "collection_not_found".to_string(),
                message: format!("Collection '{}' not found", collection_id),
            }),
        )),
        Err(e) => return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    };
    
    // Use a dummy vector with correct dimension for metadata-only search
    let dummy_vector = vec![0.0f32; collection_metadata.dimension as usize];
    tracing::debug!(
        "🔍 Searching with filter for client_id='{}', dimension={}, dummy_vector_len={}", 
        client_id, collection_metadata.dimension, dummy_vector.len()
    );
    match storage.search_vectors_with_filter(&collection_id, dummy_vector, 1, filter_fn).await {
        Ok(results) => {
            if let Some(first_result) = results.first() {
                // Get the full vector record by ID
                match storage.read(&collection_id, &first_result.vector_id).await {
                    Ok(Some(record)) => Ok(Json(VectorResponse::from(record))),
                    Ok(None) => Err((
                        StatusCode::NOT_FOUND,
                        Json(ErrorResponse {
                            error: "vector_not_found".to_string(),
                            message: format!("Vector with client_id '{}' not found", client_id),
                        }),
                    )),
                    Err(e) => Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "storage_error".to_string(),
                            message: e.to_string(),
                        }),
                    )),
                }
            } else {
                Err((
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: "vector_not_found".to_string(),
                        message: format!("Vector with client_id '{}' not found", client_id),
                    }),
                ))
            }
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "search_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}
// Helper function to resolve collection by name to ID
async fn resolve_collection_name_to_id(
    storage: &tokio::sync::RwLockReadGuard<'_, StorageEngine>,
    collection_name: &str,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!("🔍 resolve_collection_name_to_id: collection_name='{}'", collection_name);
    match storage.list_collections().await {
        Ok(collections) => {
            tracing::debug!("🔍 Found {} collections total", collections.len());
            for collection in collections {
                tracing::debug!("🔍 Checking collection: id='{}', name='{}'", collection.id, collection.name);
                if collection.name == collection_name {
                    tracing::debug!("🔍 Found matching collection: '{}'", collection.id);
                    return Ok(collection.id);
                }
            }
            tracing::error!("🔥 Collection '{}' not found", collection_name);
            Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "collection_not_found".to_string(),
                    message: format!("Collection '{}' not found", collection_name),
                }),
            ))
        }
        Err(e) => {
            tracing::error!("🔥 Storage error listing collections: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "storage_error".to_string(),
                    message: e.to_string(),
                }),
            ))
        }
    }
}


/// Delete collection by ID (explicit)
async fn delete_collection_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<ErrorResponse>)> {
    delete_collection(State(state), Path(collection_id)).await
}

/// Delete collection by name (explicit)
async fn delete_collection_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    delete_collection(State(state), Path(collection_id)).await
}

/// Insert vector by collection ID (explicit)
async fn insert_vector_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<InsertVectorRequest>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    insert_vector(State(state), Path(collection_id), Json(request)).await
}

/// Insert vector by collection name (explicit)
async fn insert_vector_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
    Json(request): Json<InsertVectorRequest>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    insert_vector(State(state), Path(collection_id), Json(request)).await
}

/// Batch insert vectors by collection ID (explicit)
async fn batch_insert_vectors_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<BatchInsertRequest>,
) -> Result<Json<BatchInsertResponse>, (StatusCode, Json<ErrorResponse>)> {
    batch_insert_vectors(State(state), Path(collection_id), Json(request)).await
}

/// Batch insert vectors by collection name (explicit)
async fn batch_insert_vectors_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
    Json(request): Json<BatchInsertRequest>,
) -> Result<Json<BatchInsertResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    batch_insert_vectors(State(state), Path(collection_id), Json(request)).await
}

/// Search vectors by collection ID (explicit)
async fn search_vectors_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    search_vectors(State(state), Path(collection_id), Json(request)).await
}

/// Search vectors by collection name (explicit)
async fn search_vectors_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    search_vectors(State(state), Path(collection_id), Json(request)).await
}

/// Get vector by client ID with collection ID (explicit)
async fn get_vector_by_client_id_by_id(
    State(state): State<ApiState>,
    Path((collection_id, client_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    get_vector_by_client_id(State(state), Path((collection_id, client_id))).await
}

/// Get vector by client ID with collection name (explicit)
async fn get_vector_by_client_id_by_name(
    State(state): State<ApiState>,
    Path((collection_name, client_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!(
        "🔍 get_vector_by_client_id_by_name: collection_name='{}', client_id='{}'", 
        collection_name, client_id
    );
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    tracing::debug!("🔍 Resolved collection name '{}' to ID '{}'", collection_name, collection_id);
    drop(storage);
    get_vector_by_client_id(State(state), Path((collection_id, client_id))).await
}

async fn get_vector_by_id(
    State(state): State<ApiState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    get_vector(State(state), Path((collection_id, vector_id))).await
}

async fn get_vector_by_name(
    State(state): State<ApiState>,
    Path((collection_name, vector_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    get_vector(State(state), Path((collection_id, vector_id))).await
}

async fn delete_vector_by_id(
    State(state): State<ApiState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    delete_vector(State(state), Path((collection_id, vector_id))).await
}

async fn delete_vector_by_name(
    State(state): State<ApiState>,
    Path((collection_name, vector_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    delete_vector(State(state), Path((collection_id, vector_id))).await
}

/// List only collection IDs
async fn list_collection_ids(
    State(state): State<ApiState>,
) -> Result<Json<Vec<String>>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match storage.list_collections().await {
        Ok(collections) => {
            let ids: Vec<String> = collections
                .into_iter()
                .map(|collection| collection.id)
                .collect();
            Ok(Json(ids))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// List only collection names
async fn list_collection_names(
    State(state): State<ApiState>,
) -> Result<Json<Vec<String>>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match storage.list_collections().await {
        Ok(collections) => {
            let names: Vec<String> = collections
                .into_iter()
                .map(|collection| collection.name)
                .collect();
            Ok(Json(names))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Get collection ID by name
async fn get_collection_id_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match resolve_collection_name_to_id(&storage, &collection_name).await {
        Ok(collection_id) => Ok(Json(serde_json::json!({
            "name": collection_name,
            "id": collection_id
        }))),
        Err(error_response) => Err(error_response),
    }
}

/// Get collection name by ID
async fn get_collection_name_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    match storage.get_collection_metadata(&collection_id).await {
        Ok(Some(metadata)) => Ok(Json(serde_json::json!({
            "id": collection_id,
            "name": metadata.name
        }))),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "collection_not_found".to_string(),
                message: format!("Collection '{}' not found", collection_id),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Bulk update vectors (upsert)
async fn bulk_update_vectors(
    State(state): State<ApiState>,
    Path(collection_identifier): Path<String>,
    Json(request): Json<BulkUpdateRequest>,
) -> Result<Json<BulkUpdateResponse>, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!(
        "🔄 bulk_update_vectors: collection_identifier='{}', vector_count={}", 
        collection_identifier, request.vectors.len()
    );
    // Resolve collection name to ID if needed
    let collection_id = {
        let storage_read = state.storage.read().await;
        if uuid::Uuid::parse_str(&collection_identifier).is_ok() {
            tracing::debug!("🔄 Collection identifier is already a UUID");
            collection_identifier
        } else {
            tracing::debug!("🔄 Collection identifier is a name, resolving to ID");
            // It might be a name, try to resolve
            match resolve_collection_name_to_id(&storage_read, &collection_identifier).await {
                Ok(id) => {
                    tracing::debug!("🔄 Resolved collection name '{}' to ID '{}'", collection_identifier, id);
                    id
                },
                Err(err) => {
                    tracing::error!("🔥 Failed to resolve collection name '{}': {:?}", collection_identifier, err);
                    return Err(err);
                }
            }
        }
    };
    
    let storage = state.storage.write().await;
    
    // Convert API requests to VectorRecord objects for update/upsert
    let mut vector_records = Vec::with_capacity(request.vectors.len());
    for vector_request in request.vectors {
        // Use client-provided ID or generate one
        let vector_id = vector_request.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        
        // Prepare metadata
        let metadata = vector_request.metadata.unwrap_or_default();
        
        let vector_record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector: vector_request.vector,
            metadata,
            timestamp: Utc::now(),
            expires_at: None,
        };
        vector_records.push(vector_record);
    }
    
    // Use batch_write for upsert functionality
    match storage.batch_write(vector_records).await {
        Ok(updated_ids) => {
            let updated_id_strings: Vec<String> = updated_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect();
            
            Ok(Json(BulkUpdateResponse {
                total_count: updated_id_strings.len(),
                updated_ids: updated_id_strings,
            }))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "bulk_update_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Bulk delete vectors by IDs
async fn bulk_delete_vectors(
    State(state): State<ApiState>,
    Path(collection_identifier): Path<String>,
    Json(request): Json<BulkDeleteRequest>,
) -> Result<Json<BulkDeleteResponse>, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!(
        "🗑️ bulk_delete_vectors: collection_identifier='{}', id_count={}", 
        collection_identifier, request.ids.len()
    );
    // Resolve collection name to ID if needed
    let collection_id = {
        let storage_read = state.storage.read().await;
        if uuid::Uuid::parse_str(&collection_identifier).is_ok() {
            tracing::debug!("🗑️ Collection identifier is already a UUID");
            collection_identifier
        } else {
            tracing::debug!("🗑️ Collection identifier is a name, resolving to ID");
            // It might be a name, try to resolve
            match resolve_collection_name_to_id(&storage_read, &collection_identifier).await {
                Ok(id) => {
                    tracing::debug!("🗑️ Resolved collection name '{}' to ID '{}'", collection_identifier, id);
                    id
                },
                Err(err) => {
                    tracing::error!("🔥 Failed to resolve collection name '{}': {:?}", collection_identifier, err);
                    return Err(err);
                }
            }
        }
    };
    
    let storage = state.storage.write().await;
    
    // Get collection metadata to determine vector dimension for dummy vector searches
    let collection_metadata = match storage.get_collection_metadata(&collection_id).await {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "collection_not_found".to_string(),
                message: format!("Collection '{}' not found", collection_id),
            }),
        )),
        Err(e) => return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    };
    
    let mut deleted_ids = Vec::new();
    let mut errors = Vec::new();
    
    // Delete each vector by ID (handle both UUIDs and client IDs)
    for id_str in request.ids {
        // Delete by ID directly (no UUID parsing needed)
        match storage.soft_delete(&collection_id, &id_str).await {
            Ok(true) => deleted_ids.push(id_str.clone()),
            Ok(false) => errors.push(format!("Vector '{}' not found", id_str)),
            Err(e) => errors.push(format!("Error deleting '{}': {}", id_str, e)),
        }
    }
    
    if !errors.is_empty() && deleted_ids.is_empty() {
        // All deletes failed
        Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "bulk_delete_failed".to_string(),
                message: format!("All deletes failed: {}", errors.join(", ")),
            }),
        ))
    } else {
        // Some or all deletes succeeded
        Ok(Json(BulkDeleteResponse {
            total_count: deleted_ids.len(),
            deleted_ids,
        }))
    }
}

/// Migrate collection strategy
async fn migrate_collection_strategy(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<StrategyMigrationRequest>,
) -> Result<Json<StrategyMigrationResponse>, (StatusCode, Json<ErrorResponse>)> {
    tracing::info!("🔄 REST API: Strategy migration requested for collection: {}", collection_id);

    // Check if migration service is available
    let migration_service = match &state.migration_service {
        Some(service) => service,
        None => {
            tracing::error!("❌ REST API: Migration service not available");
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "migration_service_unavailable".to_string(),
                    message: "Migration service is not configured. Metadata store required.".to_string(),
                }),
            ));
        }
    };

    // Ensure the request collection_id matches the path parameter
    let mut migration_request = request;
    migration_request.collection_id = collection_id.clone();

    match migration_service.migrate_collection_strategy(migration_request).await {
        Ok(response) => {
            tracing::info!("✅ REST API: Strategy migration completed for collection: {}", collection_id);
            Ok(Json(response))
        }
        Err(e) => {
            tracing::error!("❌ REST API: Strategy migration failed for collection {}: {}", collection_id, e);
            let (status_code, error_type) = match e.to_string().as_str() {
                s if s.contains("Collection not found") => (StatusCode::NOT_FOUND, "collection_not_found"),
                s if s.contains("invalid") => (StatusCode::BAD_REQUEST, "validation_error"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "migration_error"),
            };
            Err((
                status_code,
                Json(ErrorResponse {
                    error: error_type.to_string(),
                    message: e.to_string(),
                }),
            ))
        }
    }
}

/// Get migration history for a collection
async fn get_migration_history(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!("📊 REST API: Migration history requested for collection: {}", collection_id);

    // Check if migration service is available
    let migration_service = match &state.migration_service {
        Some(service) => service,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "migration_service_unavailable".to_string(),
                    message: "Migration service is not configured. Metadata store required.".to_string(),
                }),
            ));
        }
    };

    match migration_service.get_migration_history(&collection_id).await {
        Ok(history) => Ok(Json(serde_json::json!({
            "collection_id": collection_id,
            "change_history": history
        }))),
        Err(e) => {
            let (status_code, error_type) = match e.to_string().as_str() {
                s if s.contains("Collection not found") => (StatusCode::NOT_FOUND, "collection_not_found"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "storage_error"),
            };
            Err((
                status_code,
                Json(ErrorResponse {
                    error: error_type.to_string(),
                    message: e.to_string(),
                }),
            ))
        }
    }
}

/// Get current strategy for a collection
async fn get_current_strategy(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!("📊 REST API: Current strategy requested for collection: {}", collection_id);

    // Check if migration service is available
    let migration_service = match &state.migration_service {
        Some(service) => service,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "migration_service_unavailable".to_string(),
                    message: "Migration service is not configured. Metadata store required.".to_string(),
                }),
            ));
        }
    };

    match migration_service.get_current_strategy(&collection_id).await {
        Ok(strategy) => Ok(Json(serde_json::json!({
            "collection_id": collection_id,
            "current_strategy": strategy
        }))),
        Err(e) => {
            let (status_code, error_type) = match e.to_string().as_str() {
                s if s.contains("Collection not found") => (StatusCode::NOT_FOUND, "collection_not_found"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "storage_error"),
            };
            Err((
                status_code,
                Json(ErrorResponse {
                    error: error_type.to_string(),
                    message: e.to_string(),
                }),
            ))
        }
    }
}
