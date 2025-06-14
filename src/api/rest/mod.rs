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
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;
use chrono::Utc;

use crate::storage::{StorageEngine, CollectionMetadata};
use crate::core::VectorRecord;
use crate::compute::algorithms::SearchResult;

/// API application state
#[derive(Clone)]
pub struct ApiState {
    pub storage: Arc<tokio::sync::RwLock<StorageEngine>>,
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
}

impl From<CollectionMetadata> for CollectionResponse {
    fn from(metadata: CollectionMetadata) -> Self {
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
#[derive(Debug, Serialize)]
pub struct SearchResultResponse {
    pub vector_id: String,
    pub score: f32,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl From<SearchResult> for SearchResultResponse {
    fn from(result: SearchResult) -> Self {
        Self {
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
    let state = ApiState { storage };

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
    let storage = state.storage.write().await;
    
    // Generate collection ID
    let collection_id = uuid::Uuid::new_v4().to_string();
    
    // Create metadata
    let metadata = CollectionMetadata {
        id: collection_id.clone(),
        name: request.name,
        dimension: request.dimension,
        distance_metric: request.distance_metric.unwrap_or_else(|| "cosine".to_string()),
        indexing_algorithm: request.indexing_algorithm.unwrap_or_else(|| "hnsw".to_string()),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        vector_count: 0,
        total_size_bytes: 0,
        config: request.config.unwrap_or_default(),
    };
    
    match storage.create_collection_with_metadata(collection_id, Some(metadata.clone())).await {
        Ok(_) => Ok(Json(CollectionResponse::from(metadata))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
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

/// Delete a collection
async fn delete_collection(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.write().await;
    
    match storage.delete_collection(&collection_id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
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
    let storage = state.storage.write().await;
    
    // Always generate unique server UUID to avoid collisions
    let vector_id = Uuid::new_v4();
    
    // Prepare metadata with optional client ID
    let mut metadata = request.metadata.unwrap_or_default();
    if let Some(client_id) = request.id {
        // Store client ID in metadata for lookup, but use server UUID internally
        metadata.insert("client_id".to_string(), serde_json::Value::String(client_id));
    }

    let vector_record = VectorRecord {
        id: vector_id,
        collection_id: collection_id.clone(),
        vector: request.vector,
        metadata,
        timestamp: Utc::now(),
        expires_at: None, // No expiration by default
    };
    
    match storage.write(vector_record.clone()).await {
        Ok(_) => Ok(Json(VectorResponse::from(vector_record))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "storage_error".to_string(),
                message: e.to_string(),
            }),
        )),
    }
}

/// Get a vector by ID
async fn get_vector(
    State(state): State<ApiState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    let vector_uuid = match Uuid::parse_str(&vector_id) {
        Ok(uuid) => uuid,
        Err(_) => return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid_vector_id".to_string(),
                message: "Vector ID must be a valid UUID".to_string(),
            }),
        )),
    };
    
    match storage.read(&collection_id, &vector_uuid).await {
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
    
    let vector_uuid = match Uuid::parse_str(&vector_id) {
        Ok(uuid) => uuid,
        Err(_) => return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid_vector_id".to_string(),
                message: "Vector ID must be a valid UUID".to_string(),
            }),
        )),
    };
    
    match storage.soft_delete(&collection_id, &vector_uuid).await {
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
) -> Result<Json<Vec<SearchResultResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    
    let k = request.k.unwrap_or(10);
    
    let results = if let Some(filter) = request.filter {
        // Search with filter
        storage.search_vectors_with_filter(
            &collection_id,
            request.vector,
            k,
            move |metadata| {
                // Simple filter: check if all filter key-value pairs match
                for (key, value) in &filter {
                    if metadata.get(key) != Some(value) {
                        return false;
                    }
                }
                true
            },
        ).await
    } else {
        // Search without filter
        storage.search_vectors(&collection_id, request.vector, k).await
    };
    
    match results {
        Ok(search_results) => {
            let response: Vec<SearchResultResponse> = search_results
                .into_iter()
                .map(SearchResultResponse::from)
                .collect();
            Ok(Json(response))
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
    let storage = state.storage.write().await;
    
    // Convert API requests to VectorRecord objects
    let mut vector_records = Vec::with_capacity(request.vectors.len());
    for vector_request in request.vectors {
        // Always generate unique server UUID to avoid collisions
        let vector_id = Uuid::new_v4();
        
        // Prepare metadata with optional client ID
        let mut metadata = vector_request.metadata.unwrap_or_default();
        if let Some(client_id) = vector_request.id {
            // Store client ID in metadata for lookup, but use server UUID internally
            metadata.insert("client_id".to_string(), serde_json::Value::String(client_id));
        }

        let vector_record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector: vector_request.vector,
            metadata,
            timestamp: Utc::now(),
            expires_at: None, // No expiration by default
        };
        vector_records.push(vector_record);
    }
    
    match storage.batch_write(vector_records).await {
        Ok(inserted_ids) => {
            let inserted_id_strings: Vec<String> = inserted_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect();
            
            Ok(Json(BatchInsertResponse {
                total_count: inserted_id_strings.len(),
                inserted_ids: inserted_id_strings,
            }))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "batch_insert_error".to_string(),
                message: e.to_string(),
            }),
        )),
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
    
    // Use a dummy vector for metadata-only search
    let dummy_vector = vec![0.0f32; 1];
    match storage.search_vectors_with_filter(&collection_id, dummy_vector, 1, filter_fn).await {
        Ok(results) => {
            if let Some(first_result) = results.first() {
                // Get the full vector record by UUID
                if let Ok(uuid) = Uuid::parse_str(&first_result.vector_id) {
                    match storage.read(&collection_id, &uuid).await {
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
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "invalid_uuid".to_string(),
                            message: "Invalid UUID in search results".to_string(),
                        }),
                    ))
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
    match storage.list_collections().await {
        Ok(collections) => {
            for collection in collections {
                if collection.name == collection_name {
                    return Ok(collection.id);
                }
            }
            Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "collection_not_found".to_string(),
                    message: format!("Collection '{}' not found", collection_name),
                }),
            ))
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

/// Get collection by ID (explicit)
async fn get_collection_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<Json<CollectionResponse>, (StatusCode, Json<ErrorResponse>)> {
    get_collection_impl(state, collection_id).await
}

/// Get collection by name (explicit)
async fn get_collection_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
) -> Result<Json<CollectionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    get_collection_impl(state, collection_id).await
}

/// Internal implementation for getting collection
async fn get_collection_impl(
    state: ApiState,
    collection_id: String,
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

/// Delete collection by ID (explicit)
async fn delete_collection_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    delete_collection_impl(state, collection_id).await
}

/// Delete collection by name (explicit)
async fn delete_collection_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    delete_collection_impl(state, collection_id).await
}

/// Internal implementation for deleting collection
async fn delete_collection_impl(
    state: ApiState,
    collection_id: String,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.write().await;
    
    match storage.delete_collection(&collection_id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
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

/// Insert vector by collection ID (explicit)
async fn insert_vector_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<InsertVectorRequest>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    insert_vector_impl(state, collection_id, request).await
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
    insert_vector_impl(state, collection_id, request).await
}

/// Batch insert vectors by collection ID (explicit)
async fn batch_insert_vectors_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<BatchInsertRequest>,
) -> Result<Json<BatchInsertResponse>, (StatusCode, Json<ErrorResponse>)> {
    batch_insert_vectors_impl(state, collection_id, request).await
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
    batch_insert_vectors_impl(state, collection_id, request).await
}

/// Search vectors by collection ID (explicit)
async fn search_vectors_by_id(
    State(state): State<ApiState>,
    Path(collection_id): Path<String>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Vec<SearchResultResponse>>, (StatusCode, Json<ErrorResponse>)> {
    search_vectors_impl(state, collection_id, request).await
}

/// Search vectors by collection name (explicit)
async fn search_vectors_by_name(
    State(state): State<ApiState>,
    Path(collection_name): Path<String>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Vec<SearchResultResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    search_vectors_impl(state, collection_id, request).await
}

/// Get vector by client ID with collection ID (explicit)
async fn get_vector_by_client_id_by_id(
    State(state): State<ApiState>,
    Path((collection_id, client_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    get_vector_by_client_id_impl(state, collection_id, client_id).await
}

/// Get vector by client ID with collection name (explicit)
async fn get_vector_by_client_id_by_name(
    State(state): State<ApiState>,
    Path((collection_name, client_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    get_vector_by_client_id_impl(state, collection_id, client_id).await
}

async fn get_vector_by_id(
    State(state): State<ApiState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    get_vector_impl(state, collection_id, vector_id).await
}

async fn get_vector_by_name(
    State(state): State<ApiState>,
    Path((collection_name, vector_id)): Path<(String, String)>,
) -> Result<Json<VectorResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    get_vector_impl(state, collection_id, vector_id).await
}

async fn delete_vector_by_id(
    State(state): State<ApiState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    delete_vector_impl(state, collection_id, vector_id).await
}

async fn delete_vector_by_name(
    State(state): State<ApiState>,
    Path((collection_name, vector_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let storage = state.storage.read().await;
    let collection_id = resolve_collection_name_to_id(&storage, &collection_name).await?;
    drop(storage);
    delete_vector_impl(state, collection_id, vector_id).await
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
                .map( < /dev/null | collection| collection.id)
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
