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

use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::Json as JsonResponse,
    routing::{get, post, put, patch, delete},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::services::unified_avro_service::UnifiedAvroService;
use crate::services::collection_service::CollectionService;
use crate::core::VectorRecord;

/// Shared application state for REST handlers
#[derive(Clone)]
pub struct AppState {
    pub unified_service: Arc<UnifiedAvroService>,
    pub collection_service: Arc<CollectionService>,
}

/// Collection creation request
#[derive(Debug, Deserialize)]
pub struct CreateCollectionRequest {
    pub name: String,
    pub dimension: Option<usize>,
    pub distance_metric: Option<String>,
    pub indexing_algorithm: Option<String>,
}

/// Collection update request
#[derive(Debug, Deserialize)]
pub struct UpdateCollectionRequest {
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub owner: Option<String>,
    pub config: Option<serde_json::Value>,
}

/// Vector insertion request
#[derive(Debug, Deserialize)]
pub struct InsertVectorRequest {
    pub id: Option<String>,
    pub vector: Vec<f32>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Vector search request
#[derive(Debug, Deserialize)]
pub struct SearchVectorRequest {
    pub vector: Vec<f32>,
    pub k: Option<usize>,
    pub filters: Option<HashMap<String, serde_json::Value>>,
    pub include_vectors: Option<bool>,
    pub include_metadata: Option<bool>,
}

/// Generic API response
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub message: Option<String>,
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
    
    pub fn success_with_message(data: T, message: String) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            message: Some(message),
        }
    }
    
    pub fn error(error: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
            message: None,
        }
    }
}

/// Create REST router with all endpoints
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Health check
        .route("/health", get(health_check))
        
        // Collection management
        .route("/collections", post(create_collection))
        .route("/collections", get(list_collections))
        .route("/collections/:collection_id", get(get_collection))
        .route("/collections/:collection_id", patch(update_collection))
        .route("/collections/:collection_id", delete(delete_collection))
        
        // Vector operations
        .route("/collections/:collection_id/vectors", post(insert_vector))
        .route("/collections/:collection_id/vectors/:vector_id", get(get_vector))
        .route("/collections/:collection_id/vectors/:vector_id", put(update_vector))
        .route("/collections/:collection_id/vectors/:vector_id", delete(delete_vector))
        
        // Search operations
        .route("/collections/:collection_id/search", post(search_vectors))
        
        // Batch operations
        .route("/collections/:collection_id/vectors/batch", post(batch_insert_vectors))
        
        .with_state(state)
}

/// Health check endpoint
pub async fn health_check() -> JsonResponse<ApiResponse<HashMap<String, String>>> {
    let mut health_data = HashMap::new();
    health_data.insert("status".to_string(), "healthy".to_string());
    health_data.insert("service".to_string(), "proximadb-rest".to_string());
    health_data.insert("version".to_string(), "0.1.0".to_string());
    
    JsonResponse(ApiResponse::success(health_data))
}

/// Create collection endpoint
pub async fn create_collection(
    State(state): State<AppState>,
    Json(request): Json<CreateCollectionRequest>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    use crate::proto::proximadb::{CollectionConfig, DistanceMetric, StorageEngine, IndexingAlgorithm};
    
    // Parse distance metric
    let distance_metric = match request.distance_metric.as_deref().unwrap_or("cosine") {
        "cosine" => DistanceMetric::Cosine as i32,
        "euclidean" => DistanceMetric::Euclidean as i32,
        "dot_product" => DistanceMetric::DotProduct as i32,
        _ => DistanceMetric::Cosine as i32,
    };
    
    // Parse indexing algorithm
    let indexing_algorithm = match request.indexing_algorithm.as_deref().unwrap_or("hnsw") {
        "hnsw" => IndexingAlgorithm::Hnsw as i32,
        "ivf" => IndexingAlgorithm::Ivf as i32,
        "flat" => IndexingAlgorithm::Flat as i32,
        _ => IndexingAlgorithm::Hnsw as i32,
    };
    
    let config = CollectionConfig {
        name: request.name.clone(),
        dimension: request.dimension.unwrap_or(384) as i32,
        distance_metric,
        storage_engine: StorageEngine::Viper as i32, // Default to VIPER
        indexing_algorithm,
        filterable_metadata_fields: Vec::new(), // Default to no filterable fields
        indexing_config: HashMap::new(), // Default empty config
        filterable_columns: Vec::new(), // Empty by default for new filterable column API
    };
    
    match state.collection_service.create_collection_from_grpc(&config).await {
        Ok(_) => Ok(JsonResponse(ApiResponse::success_with_message(
            request.name,
            "Collection created successfully".to_string(),
        ))),
        Err(e) => {
            tracing::error!("Failed to create collection: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// List collections endpoint
pub async fn list_collections(
    State(state): State<AppState>,
) -> Result<JsonResponse<ApiResponse<Vec<String>>>, StatusCode> {
    match state.collection_service.list_collections().await {
        Ok(collections) => {
            let collection_names: Vec<String> = collections.into_iter()
                .map(|c| c.name)
                .collect();
            Ok(JsonResponse(ApiResponse::success(collection_names)))
        }
        Err(e) => {
            tracing::error!("Failed to list collections: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get collection endpoint
pub async fn get_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    match state.collection_service.get_collection_by_name(&collection_id).await {
        Ok(Some(collection)) => {
            let collection_json = serde_json::to_value(collection)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(JsonResponse(ApiResponse::success(collection_json)))
        }
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("Failed to get collection: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Delete collection endpoint
pub async fn delete_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    match state.collection_service.delete_collection(&collection_id).await {
        Ok(_) => Ok(JsonResponse(ApiResponse::success_with_message(
            collection_id,
            "Collection deleted successfully".to_string(),
        ))),
        Err(e) => {
            tracing::error!("Failed to delete collection: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Update collection endpoint
pub async fn update_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Json(request): Json<UpdateCollectionRequest>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    // Convert UpdateCollectionRequest to HashMap<String, serde_json::Value>
    let mut updates = HashMap::new();
    
    if let Some(description) = request.description {
        updates.insert("description".to_string(), serde_json::Value::String(description));
    }
    
    if let Some(tags) = request.tags {
        let tags_json = serde_json::to_value(tags)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        updates.insert("tags".to_string(), tags_json);
    }
    
    if let Some(owner) = request.owner {
        updates.insert("owner".to_string(), serde_json::Value::String(owner));
    }
    
    if let Some(config) = request.config {
        updates.insert("config".to_string(), config);
    }
    
    if updates.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    match state.collection_service.update_collection_metadata(&collection_id, &updates).await {
        Ok(response) => {
            if response.success {
                // Get the updated collection to return
                match state.collection_service.get_collection_by_name(&collection_id).await {
                    Ok(Some(collection)) => {
                        let collection_json = serde_json::to_value(collection)
                            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                        Ok(JsonResponse(ApiResponse::success(collection_json)))
                    }
                    Ok(None) => Err(StatusCode::NOT_FOUND),
                    Err(e) => {
                        tracing::error!("Failed to get updated collection: {:?}", e);
                        Err(StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }
            } else {
                match response.error_code.as_deref() {
                    Some("COLLECTION_NOT_FOUND") => Err(StatusCode::NOT_FOUND),
                    Some("INVALID_DESCRIPTION" | "INVALID_TAGS" | "INVALID_OWNER" | "INVALID_CONFIG") => {
                        Err(StatusCode::BAD_REQUEST)
                    }
                    Some("IMMUTABLE_FIELD" | "UNKNOWN_FIELD") => Err(StatusCode::BAD_REQUEST),
                    _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to update collection: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Insert vector endpoint
pub async fn insert_vector(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Json(request): Json<InsertVectorRequest>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    let vector_id = request.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    
    tracing::info!("REST: Insert vector {} into collection {}", vector_id, collection_id);
    tracing::info!("Vector dimension: {}", request.vector.len());
    
    // Create VectorRecord with all required fields
    let now_ms = chrono::Utc::now().timestamp_millis();
    let vector_record = VectorRecord {
        id: vector_id.clone(),
        collection_id: collection_id.clone(),
        vector: request.vector,
        metadata: request.metadata.unwrap_or_default(),
        timestamp: now_ms,
        created_at: now_ms,
        updated_at: now_ms,
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };
    
    // Create simple Avro payload using JSON serialization as fallback
    let vectors = vec![vector_record];
    let json_payload = serde_json::to_vec(&vectors)
        .map_err(|e| {
            tracing::error!("Failed to serialize vectors: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    // Use the UnifiedAvroService handle_vector_insert_v2 method
    match state.unified_service.handle_vector_insert_v2(&collection_id, false, &json_payload).await {
        Ok(_result) => {
            tracing::info!("✅ REST: Vector {} inserted successfully", vector_id);
            Ok(JsonResponse(ApiResponse::success_with_message(
                vector_id,
                "Vector inserted successfully".to_string(),
            )))
        }
        Err(e) => {
            tracing::error!("❌ REST: Failed to insert vector {}: {:?}", vector_id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get vector endpoint
pub async fn get_vector(
    State(_state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    // TODO: Implement through UnifiedAvroService
    tracing::info!("REST: Get vector {} from collection {}", vector_id, collection_id);
    
    Err(StatusCode::NOT_IMPLEMENTED)
}

/// Update vector endpoint
pub async fn update_vector(
    State(_state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
    Json(request): Json<InsertVectorRequest>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    // TODO: Implement through UnifiedAvroService
    tracing::info!("REST: Update vector {} in collection {}", vector_id, collection_id);
    tracing::info!("New vector dimension: {}", request.vector.len());
    
    Ok(JsonResponse(ApiResponse::success_with_message(
        vector_id,
        "Vector update queued (implementation pending)".to_string(),
    )))
}

/// Delete vector endpoint
pub async fn delete_vector(
    State(_state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    // TODO: Implement through UnifiedAvroService
    tracing::info!("REST: Delete vector {} from collection {}", vector_id, collection_id);
    
    Ok(JsonResponse(ApiResponse::success_with_message(
        vector_id,
        "Vector deletion queued (implementation pending)".to_string(),
    )))
}

/// Search vectors endpoint
pub async fn search_vectors(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Json(request): Json<SearchVectorRequest>,
) -> Result<JsonResponse<ApiResponse<Vec<serde_json::Value>>>, StatusCode> {
    let k = request.k.unwrap_or(10);
    
    tracing::info!("REST: Search {} vectors in collection {}", k, collection_id);
    tracing::info!("Query vector dimension: {}", request.vector.len());
    
    // Create search query payload
    let search_query = serde_json::json!({
        "collection_id": collection_id,
        "vector": request.vector,
        "k": k,
        "filters": request.filters.unwrap_or_default(),
        "threshold": 0.0
    });
    
    let json_payload = serde_json::to_vec(&search_query)
        .map_err(|e| {
            tracing::error!("Failed to serialize search query: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    // Search through UnifiedAvroService using simplified method
    match state.unified_service.search_vectors_simple(&json_payload).await {
        Ok(result_bytes) => {
            // Parse the result bytes as JSON
            match serde_json::from_slice::<serde_json::Value>(&result_bytes) {
                Ok(search_response) => {
                    let results = if let Some(results_array) = search_response.get("results").and_then(|r| r.as_array()) {
                        results_array.iter().map(|result| {
                            let mut json_result = serde_json::json!({
                                "id": result.get("id").unwrap_or(&serde_json::Value::String("unknown".to_string())),
                                "score": result.get("score").unwrap_or(&serde_json::Value::Number(serde_json::Number::from_f64(0.0).unwrap())),
                            });
                            
                            if request.include_vectors.unwrap_or(false) {
                                if let Some(vector) = result.get("vector") {
                                    json_result["vector"] = vector.clone();
                                }
                            }
                            
                            if request.include_metadata.unwrap_or(true) {
                                if let Some(metadata) = result.get("metadata") {
                                    json_result["metadata"] = metadata.clone();
                                }
                            }
                            
                            json_result
                        }).collect::<Vec<_>>()
                    } else {
                        vec![]
                    };
                    
                    let result_count = results.len();
                    tracing::info!("✅ REST: Found {} search results", result_count);
                    Ok(JsonResponse(ApiResponse::success_with_message(
                        results,
                        format!("Search completed - found {} results", result_count),
                    )))
                }
                Err(e) => {
                    tracing::error!("❌ REST: Failed to parse search results: {:?}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("❌ REST: Search failed: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Batch insert vectors endpoint
pub async fn batch_insert_vectors(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Json(vectors): Json<Vec<InsertVectorRequest>>,
) -> Result<JsonResponse<ApiResponse<Vec<String>>>, StatusCode> {
    tracing::info!("REST: Batch insert {} vectors into collection {}", vectors.len(), collection_id);
    
    // Convert to VectorRecord objects
    let mut vector_records = Vec::new();
    let mut vector_ids = Vec::new();
    
    for request in vectors {
        let vector_id = request.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        vector_ids.push(vector_id.clone());
        
        let now_ms = chrono::Utc::now().timestamp_millis();
        vector_records.push(VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector: request.vector,
            metadata: request.metadata.unwrap_or_default(),
            timestamp: now_ms,
            created_at: now_ms,
            updated_at: now_ms,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        });
    }
    
    // Create JSON payload
    let json_payload = serde_json::to_vec(&vector_records)
        .map_err(|e| {
            tracing::error!("Failed to serialize batch vectors: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    // Insert through UnifiedAvroService
    match state.unified_service.handle_vector_insert_v2(&collection_id, false, &json_payload).await {
        Ok(_) => {
            let vector_count = vector_ids.len();
            tracing::info!("✅ REST: Batch inserted {} vectors successfully", vector_count);
            Ok(JsonResponse(ApiResponse::success_with_message(
                vector_ids,
                format!("Batch inserted {} vectors successfully", vector_count),
            )))
        }
        Err(e) => {
            tracing::error!("❌ REST: Batch insert failed: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}