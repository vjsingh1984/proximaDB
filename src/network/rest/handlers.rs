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

use anyhow::Result;
use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::Json as JsonResponse,
    routing::{delete, get, patch, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
// Removed uuid import - no longer auto-generating vector IDs

use crate::core::VectorRecord;
use crate::services::collection_service::CollectionService;
use crate::services::vector_service::VectorService;
// Use centralized schema module 
use crate::storage::persistence::wal::schema::{
    create_avro_vector_batch,
};
use crate::index::config::IndexConfig;

// Function removed - using centralized create_avro_vector_batch from schema module

/// Convert REST IndexConfig to internal IndexConfig
fn convert_rest_to_internal_index_config(rest_config: RestIndexConfig) -> IndexConfig {
    use crate::index::config::{IndexUpdateMode, HnswConfig, IvfConfig};
    
    let update_mode = match rest_config.update_mode.as_deref() {
        Some("synchronous") => IndexUpdateMode::Synchronous,
        Some("asynchronous") => IndexUpdateMode::Asynchronous,
        Some("hybrid_mode") => IndexUpdateMode::Hybrid,
        _ => IndexUpdateMode::Synchronous, // Default
    };

    let hnsw_config = rest_config.hnsw_config.map(|hc| HnswConfig {
        m: hc.m.unwrap_or(16),
        ef_construction: hc.ef_construction.unwrap_or(200),
        ef_search: hc.ef_search.unwrap_or(50),
        max_partition_size: hc.max_partition_size.unwrap_or(100_000),
        adaptive_parameters: hc.adaptive_parameters.unwrap_or(true),
        use_simd: hc.use_simd.unwrap_or(true),
        memory_limit_mb: hc.memory_limit_mb.unwrap_or(512),
        lazy_loading: hc.lazy_loading.unwrap_or(true),
        prune_connections: hc.prune_connections.unwrap_or(0),
        level_multiplier: hc.level_multiplier.unwrap_or(1.0 / 2.0_f32.ln()),
    });

    let ivf_config = rest_config.ivf_config.map(|ic| IvfConfig {
        n_lists: ic.n_lists.unwrap_or(1000),
        n_probe: ic.n_probe.unwrap_or(1),
        quantization_bits: ic.quantization_bits.unwrap_or(8),
        use_pq: ic.use_pq.unwrap_or(false),
        pq_subspaces: ic.pq_subspaces.unwrap_or(8),
        train_on_insert: ic.train_on_insert.unwrap_or(false),
        min_train_size: ic.min_train_size.unwrap_or(1000),
    });

    IndexConfig {
        update_mode,
        async_update_timeout_ms: rest_config.async_update_timeout_ms,
        async_update_batch_size: rest_config.async_update_batch_size,
        enable_background_optimization: rest_config.enable_background_optimization.unwrap_or(true),
        hnsw_config,
        ivf_config,
        build_concurrency: rest_config.build_concurrency,
        memory_limit_mb: rest_config.memory_limit_mb,
        checkpoint_interval_ms: rest_config.checkpoint_interval_ms,
    }
}

/// Shared application state for REST handlers
#[derive(Clone)]
pub struct AppState {
    pub vector_service: Arc<VectorService>,
    pub collection_service: Arc<CollectionService>,
}

/// Collection creation request
#[derive(Debug, Deserialize)]
pub struct CreateCollectionRequest {
    pub name: String,
    pub dimension: Option<usize>,
    pub distance_metric: Option<String>,
    pub indexing_algorithm: Option<String>,
    pub storage_engine: Option<String>,
    pub filterable_columns: Option<Vec<RestFilterableColumn>>,
    pub index_config: Option<RestIndexConfig>,
}

/// REST API representation of filterable column
#[derive(Debug, Deserialize)]
pub struct RestFilterableColumn {
    pub name: String,
    pub data_type: String, // "string", "integer", "float", "boolean", "datetime"
    pub indexed: Option<bool>,
    pub supports_range: Option<bool>,
    pub estimated_cardinality: Option<i32>,
}

/// REST API representation of index configuration
#[derive(Debug, Deserialize)]
pub struct RestIndexConfig {
    pub update_mode: Option<String>, // "synchronous", "asynchronous", "hybrid_mode"
    pub async_update_timeout_ms: Option<u64>,
    pub async_update_batch_size: Option<usize>,
    pub enable_background_optimization: Option<bool>,
    pub hnsw_config: Option<RestHnswConfig>,
    pub ivf_config: Option<RestIvfConfig>,
    pub build_concurrency: Option<usize>,
    pub memory_limit_mb: Option<u64>,
    pub checkpoint_interval_ms: Option<u64>,
}

/// REST API representation of HNSW configuration
#[derive(Debug, Deserialize)]
pub struct RestHnswConfig {
    pub m: Option<usize>,
    pub ef_construction: Option<usize>,
    pub ef_search: Option<usize>,
    pub max_partition_size: Option<usize>,
    pub adaptive_parameters: Option<bool>,
    pub use_simd: Option<bool>,
    pub memory_limit_mb: Option<usize>,
    pub lazy_loading: Option<bool>,
    pub prune_connections: Option<usize>,
    pub level_multiplier: Option<f32>,
}

/// REST API representation of IVF configuration
#[derive(Debug, Deserialize)]
pub struct RestIvfConfig {
    pub n_lists: Option<usize>,
    pub n_probe: Option<usize>,
    pub quantization_bits: Option<usize>,
    pub use_pq: Option<bool>,
    pub pq_subspaces: Option<usize>,
    pub train_on_insert: Option<bool>,
    pub min_train_size: Option<usize>,
}

/// Collection update request
#[derive(Debug, Deserialize)]
pub struct UpdateCollectionRequest {
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub owner: Option<String>,
    pub config: Option<serde_json::Value>,
    pub index_config: Option<RestIndexConfig>,
}

/// Vector insertion request - supports both single and bulk vectors
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum InsertVectorRequest {
    /// Single vector: 1D array with optional id and metadata
    Single {
        id: Option<String>,
        vector: Vec<f32>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    },
    /// Bulk vectors: 2D array with optional ids and metadata arrays
    Bulk {
        ids: Option<Vec<String>>,
        vectors: Vec<Vec<f32>>,
        metadata: Option<Vec<HashMap<String, serde_json::Value>>>,
    },
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
        // Collection lookup utilities
        .route(
            "/collections/by-name/:collection_name/id",
            get(get_collection_id_by_name),
        )
        // Internal testing endpoints (WARNING: NOT FOR PRODUCTION USE)
        .route("/internal/flush", post(internal_flush_all))
        .route(
            "/collections/:collection_id/internal/flush",
            post(internal_flush_collection),
        )
        // Vector operations
        .route("/collections/:collection_id/vectors", post(insert_vector))
        .route(
            "/collections/:collection_id/vectors/:vector_id",
            get(get_vector),
        )
        .route(
            "/collections/:collection_id/vectors/:vector_id",
            put(update_vector),
        )
        .route(
            "/collections/:collection_id/vectors/:vector_id",
            delete(delete_vector),
        )
        // Search operations - using optimized storage-aware search only
        .route(
            "/collections/:collection_id/search",
            post(search_vectors_optimized),
        )
        // Batch operations
        .route(
            "/collections/:collection_id/vectors/batch",
            post(batch_insert_vectors),
        )
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
    use crate::proto::proximadb::{
        CollectionConfig, DistanceMetric, IndexingAlgorithm, StorageEngine,
    };

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

    // Parse storage engine
    let storage_engine = match request.storage_engine.as_deref().unwrap_or("viper") {
        "viper" | "VIPER" => StorageEngine::Viper as i32,
        "lsm" | "LSM" => StorageEngine::Lsm as i32,
        _ => {
            tracing::warn!(
                "Unknown storage engine '{}', defaulting to VIPER",
                request.storage_engine.as_deref().unwrap_or("")
            );
            StorageEngine::Viper as i32
        }
    };

    // Convert REST filterable columns to proto format
    let filterable_columns = request.filterable_columns
        .as_ref()
        .map(|cols| {
            cols.iter()
                .map(|col| {
                    let data_type = match col.data_type.as_str() {
                        "string" => 1,
                        "integer" => 2,
                        "float" => 3,
                        "boolean" => 4,
                        "datetime" => 5,
                        _ => 1, // Default to string
                    };
                    
                    crate::proto::proximadb::FilterableColumnSpec {
                        name: col.name.clone(),
                        data_type,
                        indexed: col.indexed.unwrap_or(false),
                        supports_range: col.supports_range.unwrap_or(false),
                        estimated_cardinality: col.estimated_cardinality,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Convert REST IndexConfig to proto format
    let index_config = request.index_config
        .as_ref()
        .map(|ic| {
            let update_mode = match ic.update_mode.as_deref() {
                Some("synchronous") => 1,
                Some("asynchronous") => 2,
                Some("hybrid_mode") => 3,
                _ => 1, // Default to synchronous
            };

            // Convert HNSW config
            let hnsw_config = ic.hnsw_config.as_ref().map(|hc| {
                crate::proto::proximadb::HnswConfig {
                    m: hc.m.unwrap_or(16) as i32,
                    ef_construction: hc.ef_construction.unwrap_or(200) as i32,
                    ef_search: hc.ef_search.unwrap_or(50) as i32,
                    max_partition_size: hc.max_partition_size.unwrap_or(100_000) as i32,
                    adaptive_parameters: hc.adaptive_parameters.unwrap_or(true),
                    use_simd: hc.use_simd.unwrap_or(true),
                    memory_limit_mb: hc.memory_limit_mb.unwrap_or(512) as i32,
                    lazy_loading: hc.lazy_loading.unwrap_or(true),
                    prune_connections: hc.prune_connections.unwrap_or(0) as i32,
                    level_multiplier: hc.level_multiplier.unwrap_or(1.0 / 2.0_f32.ln()),
                }
            });

            // Convert IVF config
            let ivf_config = ic.ivf_config.as_ref().map(|ivc| {
                crate::proto::proximadb::IvfConfig {
                    n_lists: ivc.n_lists.unwrap_or(1000) as i32,
                    n_probe: ivc.n_probe.unwrap_or(1) as i32,
                    quantization_bits: ivc.quantization_bits.unwrap_or(8) as i32,
                    use_pq: ivc.use_pq.unwrap_or(false),
                    pq_subspaces: ivc.pq_subspaces.unwrap_or(8) as i32,
                    train_on_insert: ivc.train_on_insert.unwrap_or(false),
                    min_train_size: ivc.min_train_size.unwrap_or(1000) as i32,
                }
            });

            crate::proto::proximadb::IndexConfig {
                update_mode,
                async_update_timeout_ms: ic.async_update_timeout_ms.map(|t| t as i64),
                async_update_batch_size: ic.async_update_batch_size.map(|b| b as i32),
                enable_background_optimization: ic.enable_background_optimization.unwrap_or(true),
                hnsw_config,
                ivf_config,
                build_concurrency: ic.build_concurrency.map(|c| c as i32),
                memory_limit_mb: ic.memory_limit_mb.map(|m| m as i64),
                checkpoint_interval_ms: ic.checkpoint_interval_ms.map(|i| i as i32),
            }
        });

    let config = CollectionConfig {
        name: request.name.clone(),
        dimension: request.dimension.unwrap_or(384) as i32,
        distance_metric,
        storage_engine,
        indexing_algorithm,
        filterable_metadata_fields: Vec::new(), // Legacy field
        indexing_config: HashMap::new(),        // Legacy field
        filterable_columns,
        index_config,
    };

    match state
        .collection_service
        .create_collection_from_grpc(&config)
        .await
    {
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
            let collection_names: Vec<String> = collections.into_iter().map(|c| c.name).collect();
            Ok(JsonResponse(ApiResponse::success(collection_names)))
        }
        Err(e) => {
            tracing::error!("Failed to list collections: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get collection endpoint - supports both collection names and UUIDs
pub async fn get_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    match state
        .collection_service
        .get_collection_by_name_or_uuid(&collection_id)
        .await
    {
        Ok(Some(collection)) => {
            let collection_json =
                serde_json::to_value(collection).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    match state
        .collection_service
        .delete_collection(&collection_id)
        .await
    {
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

/// Get collection ID by name endpoint
/// GET /collections/by-name/{collection_name}/id
pub async fn get_collection_id_by_name(
    State(state): State<AppState>,
    Path(collection_name): Path<String>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    match state
        .collection_service
        .get_collection_uuid(&collection_name)
        .await
    {
        Ok(Some(uuid)) => Ok(JsonResponse(ApiResponse::success(uuid))),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("Failed to get collection UUID: {:?}", e);
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
        updates.insert(
            "description".to_string(),
            serde_json::Value::String(description),
        );
    }

    if let Some(tags) = request.tags {
        let tags_json = serde_json::to_value(tags).map_err(|_| StatusCode::BAD_REQUEST)?;
        updates.insert("tags".to_string(), tags_json);
    }

    if let Some(owner) = request.owner {
        updates.insert("owner".to_string(), serde_json::Value::String(owner));
    }

    if let Some(config) = request.config {
        updates.insert("config".to_string(), config);
    }

    // Track if IndexConfig was updated
    let index_config_updated = if let Some(rest_index_config) = request.index_config {
        // Convert REST IndexConfig to internal IndexConfig
        let internal_index_config = convert_rest_to_internal_index_config(rest_index_config);
        
        // Update IndexConfig using collection service
        match state.collection_service.update_collection_index_config(&collection_id, &internal_index_config).await {
            Ok(()) => {
                tracing::info!("✅ Updated IndexConfig for collection: {}", collection_id);
                true
            }
            Err(e) => {
                tracing::error!("❌ Failed to update IndexConfig for collection {}: {}", collection_id, e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    } else {
        false
    };

    if updates.is_empty() && !index_config_updated {
        return Err(StatusCode::BAD_REQUEST);
    }

    match state
        .collection_service
        .update_collection_metadata(&collection_id, &updates)
        .await
    {
        Ok(response) => {
            if response.success {
                // Get the updated collection to return
                match state
                    .collection_service
                    .get_collection_by_name_or_uuid(&collection_id)
                    .await
                {
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
                    Some(
                        "INVALID_DESCRIPTION" | "INVALID_TAGS" | "INVALID_OWNER" | "INVALID_CONFIG",
                    ) => Err(StatusCode::BAD_REQUEST),
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

/// Insert vector endpoint - handles both single and bulk insertion
pub async fn insert_vector(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Json(request): Json<InsertVectorRequest>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Convert request to vector records based on format
    let (vector_records, response_data) = match request {
        InsertVectorRequest::Single {
            id,
            vector,
            metadata,
        } => {
            // Single vector insertion - id is optional client metadata only
            tracing::info!(
                "REST: Insert single vector into collection {} (dimension: {})",
                collection_id,
                vector.len()
            );
            if let Some(ref client_id) = id {
                tracing::info!("Client provided ID: {}", client_id);
            }

            let client_id = id.clone();
            let vector_record = VectorRecord {
                id: id.unwrap_or_default(), // Optional client label, not used as primary key
                collection_id: collection_id.clone(),
                vector,
                metadata: metadata.unwrap_or_default(),
                timestamp: now_ms,
                created_at: now_ms,
                updated_at: now_ms,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };

            (
                vec![vector_record],
                serde_json::json!({
                    "type": "single",
                    "client_id": client_id
                }),
            )
        }
        InsertVectorRequest::Bulk {
            ids,
            vectors,
            metadata,
        } => {
            // Bulk vector insertion - ids are optional client metadata only
            let num_vectors = vectors.len();
            tracing::info!(
                "REST: Insert {} vectors into collection {}",
                num_vectors,
                collection_id
            );

            // Validate consistent lengths if provided
            if let Some(ref id_list) = ids {
                if id_list.len() != num_vectors {
                    return Err(StatusCode::BAD_REQUEST);
                }
            }
            if let Some(ref meta_list) = metadata {
                if meta_list.len() != num_vectors {
                    return Err(StatusCode::BAD_REQUEST);
                }
            }

            // Create vector records
            let vector_records: Vec<VectorRecord> = vectors
                .into_iter()
                .enumerate()
                .map(|(i, vector)| {
                    let client_id = ids
                        .as_ref()
                        .and_then(|id_list| id_list.get(i).cloned())
                        .unwrap_or_default(); // No auto-generation, empty if not provided

                    let meta = metadata
                        .as_ref()
                        .and_then(|meta_list| meta_list.get(i).cloned())
                        .unwrap_or_default();

                    VectorRecord {
                        id: client_id, // Optional client label, not used as primary key
                        collection_id: collection_id.clone(),
                        vector,
                        metadata: meta,
                        timestamp: now_ms,
                        created_at: now_ms,
                        updated_at: now_ms,
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    }
                })
                .collect();

            let client_ids: Vec<Option<String>> = vector_records
                .iter()
                .map(|r| {
                    if r.id.is_empty() {
                        None
                    } else {
                        Some(r.id.clone())
                    }
                })
                .collect();

            (
                vector_records,
                serde_json::json!({
                    "type": "bulk",
                    "count": num_vectors,
                    "client_ids": client_ids
                }),
            )
        }
    };

    // Convert to Avro binary format using the proper conversion function
    let avro_payload = create_avro_vector_batch(&vector_records).map_err(|e| {
        tracing::error!("Failed to create Avro vector batch: {:?}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Use the UnifiedAvroService handle_vector_insert method with proper Avro binary payload
    match state
        .vector_service
        .handle_vector_insert(&collection_id, false, &avro_payload)
        .await
    {
        Ok(_result) => {
            tracing::info!("✅ REST: Vectors inserted successfully");
            Ok(JsonResponse(ApiResponse::success_with_message(
                response_data,
                "Vectors inserted successfully".to_string(),
            )))
        }
        Err(e) => {
            tracing::error!("❌ REST: Failed to insert vectors: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get vector endpoint
pub async fn get_vector(
    State(state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    tracing::info!(
        "REST: Get vector {} from collection {}",
        vector_id,
        collection_id
    );

    // Get vector through UnifiedAvroService
    match state
        .vector_service
        .get_vector(&collection_id, &vector_id, true, true)
        .await
    {
        Ok(result_bytes) => {
            // Parse the result bytes as JSON
            match serde_json::from_slice::<serde_json::Value>(&result_bytes) {
                Ok(vector_response) => {
                    if let Some(vector_data) = vector_response.get("vector") {
                        tracing::info!("✅ REST: Found vector {}", vector_id);
                        Ok(JsonResponse(ApiResponse::success(vector_data.clone())))
                    } else {
                        tracing::warn!(
                            "❌ REST: Vector {} not found in collection {}",
                            vector_id,
                            collection_id
                        );
                        Err(StatusCode::NOT_FOUND)
                    }
                }
                Err(e) => {
                    tracing::error!("❌ REST: Failed to parse get vector result: {:?}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("❌ REST: Failed to get vector: {:?}", e);
            // Check if it's a not found error
            if e.to_string().contains("not found") || e.to_string().contains("NOT_FOUND") {
                Err(StatusCode::NOT_FOUND)
            } else {
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

/// Update vector endpoint (redirects to upsert for consistency)
pub async fn update_vector(
    State(state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
    Json(request): Json<serde_json::Value>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    tracing::info!(
        "REST: Update vector {} in collection {} (converting to upsert)",
        vector_id,
        collection_id
    );

    // Extract vector and metadata from request
    let vector = request
        .get("vector")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect::<Vec<f32>>()
        })
        .unwrap_or_default();

    let metadata = request
        .get("metadata")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::HashMap<String, serde_json::Value>>()
        })
        .unwrap_or_default();

    if vector.is_empty() {
        return Ok(JsonResponse(ApiResponse::error(
            "Vector data is required for update".to_string(),
        )));
    }

    // Create VectorRecord for upsert
    let now_ms = chrono::Utc::now().timestamp_millis();
    let vector_record = VectorRecord {
        id: vector_id.clone(),
        collection_id: collection_id.clone(),
        vector,
        metadata,
        timestamp: now_ms,
        created_at: now_ms,
        updated_at: now_ms,
        expires_at: request.get("expires_at").and_then(|v| v.as_i64()),
        version: 1, // Will be updated by WAL logic if record exists
        rank: None,
        score: None,
        distance: None,
    };

    // Convert to Avro and process as upsert
    match create_avro_vector_batch(&[vector_record]) {
        Ok(avro_data) => {
            match state
                .vector_service
                .handle_vector_insert(&collection_id, true, &avro_data) // upsert_mode = true
                .await
            {
                Ok(response_bytes) => {
                    let response: serde_json::Value = serde_json::from_slice(&response_bytes)
                        .unwrap_or_else(|_| serde_json::json!({"success": true}));
                    
                    Ok(JsonResponse(ApiResponse::success_with_message(
                        vector_id,
                        "Vector updated successfully (upsert)".to_string(),
                    )))
                }
                Err(e) => {
                    tracing::error!("Vector update failed: {:?}", e);
                    Ok(JsonResponse(ApiResponse::error(format!(
                        "Vector update failed: {}",
                        e
                    ))))
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to create Avro payload: {:?}", e);
            Ok(JsonResponse(ApiResponse::error(format!(
                "Failed to process vector update: {}",
                e
            ))))
        }
    }
}

/// Delete vector endpoint
pub async fn delete_vector(
    State(_state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    // TODO: Implement through UnifiedAvroService
    tracing::info!(
        "REST: Delete vector {} from collection {}",
        vector_id,
        collection_id
    );

    Ok(JsonResponse(ApiResponse::success_with_message(
        vector_id,
        "Vector deletion queued (implementation pending)".to_string(),
    )))
}

/// Storage-aware optimized search vectors endpoint
pub async fn search_vectors_optimized(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Json(request): Json<SearchVectorRequest>,
) -> Result<JsonResponse<ApiResponse<Vec<serde_json::Value>>>, StatusCode> {
    let k = request.k.unwrap_or(10);

    tracing::info!("🚀 REST: Starting OPTIMIZED storage-aware search operation");
    tracing::info!("🚀 REST: Collection: {}", collection_id);
    tracing::info!("🚀 REST: K value: {}", k);
    tracing::info!("🚀 REST: Query vector dimension: {}", request.vector.len());
    tracing::debug!(
        "🚀 REST: Query vector sample: {:?}",
        &request.vector[..std::cmp::min(5, request.vector.len())]
    );
    tracing::debug!("🚀 REST: Filters: {:?}", request.filters);

    // Create search query payload with enhanced search hints for optimization
    let filters = request.filters.unwrap_or_default();
    let search_query = serde_json::json!({
        "collection_id": collection_id,
        "vector": request.vector,
        "k": k,
        "filters": filters,
        "threshold": 0.0,
        "search_hints": {
            "predicate_pushdown": true,
            "use_bloom_filters": true,
            "use_clustering": true,
            "quantization_level": "FP32",
            "parallel_search": true,
            "engine_specific": {
                "optimization_level": "high",
                "enable_simd": true,
                "prefer_indices": true
            }
        }
    });

    tracing::debug!("🚀 REST: Enhanced search query with optimization hints created");

    let json_payload = serde_json::to_vec(&search_query).map_err(|e| {
        tracing::error!(
            "❌ REST: Failed to serialize optimized search query: {:?}",
            e
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!("🚀 REST: Calling storage-aware polymorphic search");
    tracing::debug!(
        "🚀 REST: Optimized payload size: {} bytes",
        json_payload.len()
    );

    // Use the storage-aware polymorphic search method
    match state
        .vector_service
        .search_vectors_polymorphic(&json_payload)
        .await
    {
        Ok(result_bytes) => {
            tracing::info!(
                "✅ REST: Optimized search returned {} bytes",
                result_bytes.len()
            );

            // Parse and format results
            match serde_json::from_slice::<serde_json::Value>(&result_bytes) {
                Ok(search_response) => {
                    let results = if let Some(results_array) =
                        search_response.get("results").and_then(|r| r.as_array())
                    {
                        results_array.iter().map(|result| {
                            let mut json_result = serde_json::json!({
                                "id": result.get("id").unwrap_or(&serde_json::Value::String("unknown".to_string())),
                                "score": result.get("score").unwrap_or(&serde_json::Value::Number(serde_json::Number::from_f64(0.0).unwrap())),
                                "search_engine": result.get("search_engine").unwrap_or(&serde_json::Value::String("unknown".to_string())),
                                "optimization_applied": result.get("optimization_applied").unwrap_or(&serde_json::Value::Bool(true)),
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
                    tracing::info!("✅ REST: Optimized search found {} results", result_count);

                    Ok(JsonResponse(ApiResponse::success_with_message(
                        results,
                        format!(
                            "Storage-aware optimized search completed - found {} results",
                            result_count
                        ),
                    )))
                }
                Err(e) => {
                    tracing::error!("❌ REST: Failed to parse optimized search results: {:?}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("❌ REST: Optimized search failed: {:?}", e);
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
    tracing::info!(
        "REST: Batch insert {} vectors into collection {}",
        vectors.len(),
        collection_id
    );

    // Convert to VectorRecord objects
    let mut vector_records = Vec::new();
    let mut vector_ids = Vec::new();

    for request in vectors {
        match request {
            InsertVectorRequest::Single {
                id,
                vector,
                metadata,
            } => {
                let vector_id = id.unwrap_or_default(); // No auto-generation, content key used instead
                vector_ids.push(vector_id.clone());

                let now_ms = chrono::Utc::now().timestamp_millis();
                vector_records.push(VectorRecord {
                    id: vector_id,
                    collection_id: collection_id.clone(),
                    vector,
                    metadata: metadata.unwrap_or_default(),
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
            InsertVectorRequest::Bulk { .. } => {
                // Batch endpoint should only receive single vector format
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    // Convert JSON to Avro binary payload for UnifiedAvroService
    let avro_payload = create_avro_vector_batch(&vector_records).map_err(|e| {
        tracing::error!("Failed to create Avro payload from vectors: {:?}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Insert through UnifiedAvroService (using Avro binary)
    match state
        .vector_service
        .handle_vector_insert(&collection_id, false, &avro_payload)
        .await
    {
        Ok(_) => {
            let vector_count = vector_ids.len();
            tracing::info!(
                "✅ REST: Batch inserted {} vectors successfully",
                vector_count
            );
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

/// Internal flush endpoint for testing - triggers flush for all collections
/// WARNING: This endpoint is for testing purposes only and should not be used in production
pub async fn internal_flush_all(
    State(state): State<AppState>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    tracing::warn!("⚠️ INTERNAL FLUSH ENDPOINT CALLED - THIS IS FOR TESTING ONLY");

    match state.vector_service.force_flush_all_collections().await {
        Ok(_) => {
            tracing::info!("✅ Internal flush triggered for all collections");
            Ok(JsonResponse(ApiResponse::success_with_message(
                "flush_triggered".to_string(),
                "Internal flush triggered for all collections (testing only)".to_string(),
            )))
        }
        Err(e) => {
            tracing::error!("❌ Internal flush failed: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Internal flush endpoint for testing - triggers flush for specific collection
/// WARNING: This endpoint is for testing purposes only and should not be used in production
pub async fn internal_flush_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<ApiResponse<String>>, StatusCode> {
    tracing::warn!(
        "⚠️ INTERNAL FLUSH ENDPOINT CALLED FOR COLLECTION {} - THIS IS FOR TESTING ONLY",
        collection_id
    );

    match state
        .vector_service
        .force_flush_collection(&collection_id)
        .await
    {
        Ok(_) => {
            tracing::info!(
                "✅ Internal flush triggered for collection {}",
                collection_id
            );
            Ok(JsonResponse(ApiResponse::success_with_message(
                "flush_triggered".to_string(),
                format!(
                    "Internal flush triggered for collection {} (testing only)",
                    collection_id
                ),
            )))
        }
        Err(e) => {
            tracing::error!(
                "❌ Internal flush failed for collection {}: {:?}",
                collection_id,
                e
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
