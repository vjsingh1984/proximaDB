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
// StorageEngineCompatibility removed - engines handle this internally
use axum::{
    Router,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json as JsonResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::api_handlers::UnifiedHandlers;
use crate::core::conversions;
use crate::proto::proximadb::SearchVectorRecord;

use crate::proto::proximadb::{
    AccessPattern, AnnoyConfig, Collection as ProtoCollection, CollectionConfig,
    CollectionOperation, CollectionRequest, CompressionConfig, DataDensity, DistanceMetric,
    FilterableColumnSpec, FilterableDataType, FlatConfig, FooterCacheSettings, HnswConfig,
    HybridWriterSettings, IndexConfig, IndexUpdateMode, IndexingAlgorithm, IvfConfig, LshConfig,
    NovaEngineSettings, OperationMetrics, ParquetWriterSettings, PqConfig, QuantizationConfig,
    RandomProjectionType, SstEngineSettings, StorageConfig, StorageEngine, VectorBatchRequest,
    VectorRecord, ViperEngineSettings,
};

/// Shared application state for REST handlers
#[derive(Clone)]
pub struct AppState {
    pub unified_handlers: Arc<UnifiedHandlers>,
}

/// Error response for REST API
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub status: u16,
    pub message: String,
    pub error_code: String,
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "error": self.message,
            "error_code": self.error_code
        }));

        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        )
            .into_response()
    }
}

// ============================================================================
// UNIFIED API REQUEST/RESPONSE TYPES - Aligned with Proto
// ============================================================================

/// Unified collection operation request - uses proto CollectionConfig directly
#[derive(Debug, Deserialize, Serialize)]
pub struct CollectionOperationRequest {
    pub operation: String, // "create", "update" (get/list/delete now use dedicated HTTP verbs)
    pub collection_id: Option<String>,
    pub collection_name: Option<String>,
    pub config: Option<CollectionConfigJson>, // JSON-friendly wrapper around proto CollectionConfig
    pub query_params: Option<HashMap<String, String>>, // limit, offset, filters
    pub options: Option<HashMap<String, bool>>, // force, include_stats
}

/// JSON-serializable wrapper for proto CollectionConfig
/// This allows REST API to accept JSON while internally using the same proto structure
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CollectionConfigJson {
    pub name: String,
    pub dimension: usize,
    pub distance_metric: Option<String>, // "cosine", "euclidean", "dot_product" - defaults to "cosine"
    pub storage_engine: Option<String>,  // "viper", "sst" - defaults to "viper"
    pub filterable_columns: Option<Vec<FilterableColumnSpec>>,
    pub index_configs: Option<Vec<IndexConfig>>,
    pub quantization: Option<QuantizationConfig>, // Renamed from quantization
    pub storage_config: Option<StorageConfig>, // Renamed from storage_engine_config and using StorageConfig
    pub primary_index: Option<String>,         // Renamed from primary_index_name
    pub auto_index_selection: Option<bool>,    // Renamed from enable_automatic_index_selection
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub owner: Option<String>,
    // Note: Removed primary_indexing_algorithm, compression, storage_location - now in StorageConfig
}

impl CollectionConfigJson {
    /// Convert JSON config to proto CollectionConfig
    pub fn to_proto(&self) -> CollectionConfig {
        let mut config = CollectionConfig {
            name: self.name.clone(),
            dimension: self.dimension as u32,
            distance_metric: self
                .distance_metric
                .as_deref()
                .and_then(|s| conversions::parse_distance_metric(s).ok())
                .unwrap_or(crate::proto::proximadb::DistanceMetric::Cosine)
                as i32,
            storage_engine: self
                .storage_engine
                .as_deref()
                .and_then(|s| conversions::parse_storage_engine(s).ok())
                .unwrap_or(crate::proto::proximadb::StorageEngine::Viper)
                as i32,
            filterable_columns: self.filterable_columns.clone().unwrap_or_default(),
            index_configs: self.index_configs.clone().unwrap_or_default(),
            quantization: self.quantization.clone(),
            storage_config: self.storage_config.clone(),
            primary_index: self.primary_index.clone(),
            auto_index_selection: self.auto_index_selection,
            description: self.description.clone(),
            tags: self.tags.clone().unwrap_or_default(),
            owner: self.owner.clone(),
            embedding_models: None, // Default to None for embedding models
        };
        config
    }

    /// Create from proto CollectionConfig
    pub fn from_proto(proto: &CollectionConfig) -> Self {
        Self {
            name: proto.name.clone(),
            dimension: proto.dimension as usize,
            distance_metric: Some(
                conversions::distance_metric_to_string(proto.distance_metric).to_owned(),
            ),
            storage_engine: Some(
                conversions::storage_engine_to_string(proto.storage_engine).to_owned(),
            ),
            filterable_columns: if proto.filterable_columns.is_empty() {
                None
            } else {
                Some(proto.filterable_columns.clone())
            },
            index_configs: if proto.index_configs.is_empty() {
                None
            } else {
                Some(proto.index_configs.clone())
            },
            quantization: proto.quantization.clone(),
            storage_config: proto.storage_config.clone(),
            primary_index: proto.primary_index.clone(),
            auto_index_selection: proto.auto_index_selection,
            description: proto.description.clone(),
            tags: if proto.tags.is_empty() {
                None
            } else {
                Some(proto.tags.clone())
            },
            owner: proto.owner.clone(),
        }
    }
}

/// Filterable column spec - aligned with proto
#[derive(Debug, Deserialize, Serialize)]
pub struct FilterableColumn {
    pub name: String,
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
    pub async_update_timeout_ms: Option<u32>,  // Timeout in ms, u32 covers ~49 days
    pub async_update_batch_size: Option<i32>,
    pub enable_background_optimization: Option<bool>,
    pub hnsw_config: Option<RestHnswConfig>,
    pub ivf_config: Option<RestIvfConfig>,
    pub flat_config: Option<RestFlatConfig>,
    pub pq_config: Option<RestPqConfig>,
    pub annoy_config: Option<RestAnnoyConfig>,
    pub lsh_config: Option<RestLshConfig>,
    pub build_concurrency: Option<i32>,
    pub memory_limit_mb: Option<u32>,  // Memory in MB, u32 covers 4TB
    pub checkpoint_interval_ms: Option<i32>,
    pub is_primary: Option<bool>,
    pub use_cases: Option<Vec<String>>,
    pub selectivity_threshold: Option<f32>,
}

/// HNSW configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestHnswConfig {
    pub m: u32,
    pub ef_construction: u32,
    pub ef_search: u32,
    pub max_partition_size: u32,
    pub adaptive_parameters: bool,
    pub use_simd: bool,
    pub memory_limit_mb: u32,
    pub lazy_loading: bool,
    pub prune_connections: u32,
    pub level_multiplier: f32,
}

/// IVF configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestIvfConfig {
    pub n_lists: u32,
    pub n_probe: u32,
    pub quantization_bits: u32,
    pub use_pq: bool,
    pub pq_subspaces: u32,
    pub train_on_insert: bool,
    pub min_train_size: u32,
}

/// Flat index configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestFlatConfig {
    pub enable_simd: bool,
    pub batch_size: u32,
    pub enable_parallel_search: bool,
}

/// Product Quantization configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestPqConfig {
    pub subvectors: u32,
    pub bits_per_subvector: u32,
    pub training_sample_count: u32,
    pub enable_reranking: bool,
}

/// Annoy configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestAnnoyConfig {
    pub n_trees: u32,
    pub search_k: u32,
    pub max_leaf_size: u32,
    pub enable_mmap: bool,
}

/// LSH configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestLshConfig {
    pub n_hash_tables: u32,
    pub n_hash_functions: u32,
    pub bucket_width: f32,
    pub binary_vectors: bool,
    pub max_candidates: u32,
    pub projection: String, // "gaussian", "binary", "sparse"
}

/// Quantization configuration - aligned with proto
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestQuantizationConfig {
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
    pub level: RestQuantizationLevel,
    pub codebook_id: Option<String>,
    pub progressive_quantization: bool,
    pub storage_compatibility: String,
}

/// Index quantization config
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexQuantizationConfig {
    pub enabled: bool,
    pub strategies: Vec<RestIndexQuantizationStrategy>,
    pub auto_select_strategy: bool,
}

/// Search quantization config
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchQuantizationConfig {
    pub enabled: bool,
    pub default_level: RestQuantizationLevel,
    pub adaptive_precision: bool,
    pub accuracy_threshold: f32,
    pub candidate_multiplier: u32,
}

/// Quantization level
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestQuantizationLevel {
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
pub struct RestIndexQuantizationStrategy {
    pub index_name: String,
    pub level: RestQuantizationLevel,
    pub build_async: bool,
    pub codebook_id: Option<String>,
}

/// Quantization validation
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuantizationValidation {
    pub accuracy_threshold: f32,
    pub validation_sample_size: u32,
    pub enable_quality_monitoring: bool,
    pub retraining_threshold: f32,
}

/// Storage engine configuration - aligned with proto StorageEngineConfig
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestStorageEngineConfig {
    // Optimization hints
    pub access_pattern: Option<String>, // "write_heavy", "read_heavy", "balanced", "archive"
    pub data_density: Option<String>,   // "dense", "sparse", "mixed"
    pub frequent_updates: Option<bool>,
    pub expected_size_mb: Option<u32>,  // Changed to MB for better granularity, u32 covers 4PB
    pub read_write_ratio: Option<f32>,

    // Quick presets
    pub preset: Option<String>, // "maximum_performance", "balanced", "memory_constrained", "cloud_optimized", "real_time"

    // Master optimization control
    pub enable_all_optimizations: Option<bool>,

    // Specific configuration overrides
    pub parquet_writer: Option<RestParquetWriterSettings>,
    pub footer_cache: Option<RestFooterCacheSettings>,
    pub hybrid_writer: Option<RestHybridWriterSettings>,

    // Engine-specific settings
    pub sst_settings: Option<RestSstEngineSettings>,
    pub viper_settings: Option<RestViperEngineSettings>,
    pub nova_settings: Option<RestNovaEngineSettings>,
}

/// Parquet writer settings
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestParquetWriterSettings {
    pub row_group_size: Option<i32>,
    pub page_size: Option<i32>,
    pub enable_bloom_filters: Option<bool>,
    pub bloom_filter_fpp: Option<f32>,
    pub bloom_filter_columns: Option<Vec<String>>,
    pub enable_column_statistics: Option<bool>,
    pub enable_page_index: Option<bool>,
    pub enable_column_index: Option<bool>,
    pub enable_offset_index: Option<bool>,
    pub page_index_granularity: Option<i32>,
    pub enable_dictionary: Option<bool>,
    pub dictionary_threshold: Option<f32>,
    pub enable_delta_encoding: Option<bool>,
    pub enable_byte_stream_split: Option<bool>,
    pub enable_pq_sorting: Option<bool>,
    pub pq_sorting_segments: Option<i32>,
    pub pq_sorting_codebook_size: Option<i32>,
    pub enable_native_metadata: Option<bool>,
    pub metadata_inference_samples: Option<i32>,
    pub write_batch_size: Option<i32>,
    pub id_less_storage: Option<bool>,
}

/// Footer cache settings
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestFooterCacheSettings {
    pub enable: Option<bool>,
    pub max_entries: Option<u32>,  // Cache entries count
    pub ttl_seconds: Option<u32>,  // TTL in seconds, u32 covers ~136 years
    pub time_to_idle_seconds: Option<u32>,  // Idle time in seconds
    pub enable_persistence: Option<bool>,
    pub persistence_path: Option<String>,
    pub enable_prefetch: Option<bool>,
    pub prefetch_threshold: Option<u32>,  // Prefetch threshold count
    pub warming_interval_seconds: Option<u32>,  // Warming interval in seconds
    pub compression: Option<bool>,
    pub compression_level: Option<i32>,
}

/// Hybrid writer settings
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestHybridWriterSettings {
    pub enable: Option<bool>,
    pub initial_mode: Option<String>,
    pub enable_auto_switch: Option<bool>,
    pub mode_switch_threshold: Option<i32>,
    pub pattern_window_size: Option<i32>,
    pub streaming_threshold: Option<f32>,
    pub batch_threshold: Option<i32>,
    pub max_buffer_size: Option<i32>,
    pub buffer_time_limit_seconds: Option<u32>,  // Buffer time limit in seconds
    pub enable_concurrent_writes: Option<bool>,
    pub max_concurrent_writers: Option<i32>,
    pub optimize_row_group_size: Option<bool>,
    pub min_row_group_size: Option<i32>,
    pub max_row_group_size: Option<i32>,
}

/// SST engine settings
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestSstEngineSettings {
    pub enable_bloom_filters: Option<bool>,
    pub bloom_filter_fpp: Option<f32>,
    pub compression: Option<String>,
    pub compression_level: Option<i32>,
    pub write_buffer_size: Option<u64>,  // Buffer size in bytes, u64 for large buffers
    pub max_write_buffers: Option<i32>,
    pub block_size_kb: Option<i32>,
    pub dynamic_block_sizing: Option<bool>,
}

/// VIPER engine settings
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestViperEngineSettings {
    pub inherit_global_settings: Option<bool>,
    pub enable_columnar_compression: Option<bool>,
    pub enable_vector_quantization: Option<bool>,
    pub vector_chunk_size: Option<i32>,
    pub enable_lazy_loading: Option<bool>,
}

/// NOVA engine settings
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RestNovaEngineSettings {
    pub inherit_global_settings: Option<bool>,
    pub enable_real_time_mode: Option<bool>,
    pub streaming_buffer_size: Option<i32>,
    pub prefer_low_latency: Option<bool>,
}

/// Get vector query parameters
#[derive(Debug, Deserialize)]
pub struct GetVectorParams {
    pub include_vector: Option<bool>,
    pub include_metadata: Option<bool>,
}

/// Vector get response
#[derive(Debug, Serialize)]
pub struct VectorGetResponse {
    pub id: String,
    pub collection_id: String,
    pub vector: Option<Vec<f32>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub similarity: Option<f32>,
}

/// Collection response - aligned with proto CollectionResponse
#[derive(Debug, Serialize)]
pub struct CollectionResponse {
    pub success: bool,
    pub operation: String,
    pub collection: Option<Collection>,
    pub collections: Option<Vec<Collection>>,
    pub affected_count: i64,
    pub total_count: Option<u64>,  // Total count, u64 for large collections
    pub metadata: HashMap<String, String>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    pub processing_time_us: i64,
}

/// Collection data
#[derive(Debug, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub config: CollectionConfigJson,
    pub stats: CollectionStats,
    pub timestamp: i64,
    pub updated_at: i64,
}

/// Collection info for list response
#[derive(Debug, Serialize)]
pub struct CollectionInfo {
    pub id: String,
    pub name: String,
    pub dimension: usize,
    pub metric: String,
    pub timestamp: i64,
    pub updated_at: i64,
    pub vector_count: Option<u64>,  // Vector count, u64 for billions of vectors
    pub indexed: bool,
}

/// List collections response
#[derive(Debug, Serialize)]
pub struct ListCollectionsResponse {
    pub collections: Vec<CollectionInfo>,
    pub total_count: u32,
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
pub struct RestVectorBatchRequest {
    pub collection_id: String,
    pub vectors: Vec<VectorData>,
    pub batch_timeout_ms: Option<u32>,
    pub request_id: Option<String>,
}

/// Vector data for batch operations
#[derive(Debug, Deserialize, Serialize)]
pub struct VectorData {
    pub id: Option<String>,
    pub vector: Vec<f32>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub expires_at: Option<u32>, // For TTL/delete in seconds since epoch
}

/// Vector search request - aligned with proto VectorSearchRequest
#[derive(Debug, Deserialize, Serialize)]
pub struct VectorSearchRequest {
    pub collection_id: String,
    pub queries: Vec<SearchQuery>,
    pub top_k: u32,
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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetadataFilter {
    pub conditions: Vec<FilterCondition>,
    pub operator: String, // "and", "or", "not"
}

/// Filter condition
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    pub timeout_ms: Option<u32>,  // Search timeout in ms, u32 covers ~49 days
    pub accuracy_threshold: Option<f32>,
    pub enable_parallel_search: Option<bool>,
    pub thread_count: Option<i32>,
}

/// Include fields in search results
#[derive(Debug, Deserialize, Serialize)]
pub struct IncludeFields {
    pub vector: bool,
    pub metadata: bool,
    pub similarity: bool,
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
    pub metrics: RestOperationMetrics,
    pub results: Option<Vec<SearchVectorRecord>>,
    pub vector_ids: Vec<String>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
}

/// Operation metrics
#[derive(Debug, Serialize)]
pub struct RestOperationMetrics {
    pub total_processed: i64,
    pub successful_count: i64,
    pub failed_count: i64,
    pub updated_count: i64,
    pub processing_time_us: i64,
    pub wal_write_time_us: i64,
    pub index_update_time_us: i64,
}

// SearchVectorRecord now imported from proto definitions - aligned with gRPC

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
// SQL QUERY TYPES
// ============================================================================

/// SQL query request - aligned with proto SqlQueryRequest
#[derive(Debug, Deserialize, Serialize)]
pub struct SqlQueryRequest {
    pub query: String,
    pub parameters: Option<Vec<serde_json::Value>>,
    pub collection: Option<String>, // Optional if specified in FROM clause
}

/// SQL query response - aligned with proto SqlQueryResponse
#[derive(Debug, Serialize)]
pub struct SqlQueryResponse {
    pub rows: Vec<serde_json::Value>,
    pub columns: Vec<ColumnInfo>,
    pub row_count: usize,
    pub execution_time_ms: f64,
}

/// Column information for SQL results
#[derive(Debug, Serialize)]
pub struct ColumnInfo {
    pub name: String,
}

// ============================================================================
// ROUTER CONFIGURATION
// ============================================================================

/// Create REST router with unified proto-aligned endpoints
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Health and metrics
        .route("/health", get(health_check))
        .route("/metrics", get(metrics))
        .route("/metrics/:collection_id", get(collection_metrics))
        .route("/metrics/query-hints/:collection_id", get(query_hints))
        // Collection endpoints with proper REST verbs
        .route("/api/v1/collection", post(collection_operation)) // create/update operations
        .route("/api/v1/collections", get(list_collections)) // list all collections
        .route(
            "/api/v1/collection/:collection_id",
            get(collection).delete(delete_collection),
        ) // get/delete single collection
        // Vector endpoints with proper REST verbs
        .route("/api/v1/vector/batch", post(vector_batch)) // insert/update operations
        .route("/api/v1/vector/search", post(vector_search)) // search operations
        .route("/api/v1/vector/get/:collection_id/:vector_id", get(vector)) // get single vector
        .route("/api/v1/vectors/:collection_id", delete(delete_vectors)) // delete vectors
        // SQL query endpoint
        .route("/api/v1/sql/execute", post(execute_sql)) // execute SQL queries
        // Convenience endpoints for common operations
        // Internal testing endpoints (WARNING: NOT FOR PRODUCTION USE)
        .route("/internal/flush", post(internal_flush_all))
        .route(
            "/internal/flush/:collection_id",
            post(internal_flush_collection),
        )
        // Debug endpoints (TEMPORARY - FOR DEBUGGING ONLY)
        .route(
            "/debug/vectors/:collection_id",
            get(debug_list_unflushed_vectors),
        )
        .with_state(state)
}

// ============================================================================
// HANDLER IMPLEMENTATIONS
// ============================================================================

/// Health check endpoint
pub async fn health_check(
    State(state): State<AppState>,
) -> Result<JsonResponse<ApiResponse<HashMap<String, serde_json::Value>>>, StatusCode> {
    match state
        .unified_handlers
        .vector_operations_service
        .health_check()
        .await
    {
        Ok(health_data) => {
            let mut response_data = HashMap::new();
            response_data.insert("status".to_string(), json!("healthy"));
            response_data.insert("service".to_string(), json!("proximadb-rest"));
            response_data.insert("version".to_string(), json!(env!("CARGO_PKG_VERSION")));
            response_data.insert("vector_service".to_string(), health_data);

            Ok(JsonResponse(ApiResponse::success(response_data)))
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
    // Convert REST request to proto request directly
    let proto_request = crate::proto::proximadb::CollectionRequest {
        operation: conversions::parse_collection_operation(&request.operation)
            .map_err(|_| StatusCode::BAD_REQUEST)? as i32,
        collection_id: request.collection_id.clone(),
        collection_config: request.config.as_ref().and_then(|c| {
            conversions::build_collection_config(
                c.name.clone(),
                c.dimension,
                c.distance_metric.clone(),
                c.storage_engine.clone(),
                None, // indexing_algorithm - removed from config
                None, // metadata
            )
            .ok()
        }),
        query_params: std::collections::HashMap::new(),
        options: std::collections::HashMap::new(),
        migration_config: std::collections::HashMap::new(),
    };

    // Delegate to unified handlers
    let proto_response = state
        .unified_handlers
        .handle_collection_operation(proto_request)
        .await
        .map_err(|e| {
            tracing::error!("Collection operation failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Convert proto response to REST response
    let response = CollectionResponse {
        success: proto_response.success,
        operation: conversions::collection_operation_to_string(proto_response.operation)
            .to_string(),
        collection: proto_response
            .collection
            .map(|c| convert_from_proto_collection(c)),
        collections: if proto_response.collections.is_empty() {
            None
        } else {
            Some(
                proto_response
                    .collections
                    .into_iter()
                    .map(convert_from_proto_collection)
                    .collect(),
            )
        },
        affected_count: proto_response.affected_count,
        total_count: proto_response.total_count.map(|x| x as u64),
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
    Json(mut request_json): Json<serde_json::Value>,
) -> Result<JsonResponse<VectorOperationResponse>, StatusCode> {
    // Debug log the incoming request
    tracing::debug!(
        "vector_batch received JSON: {}",
        serde_json::to_string_pretty(&request_json).unwrap_or_else(|_| "invalid json".to_string())
    );

    // Handle flexible metadata format before conversion
    if let Some(vectors) = request_json
        .get_mut("vectors")
        .and_then(|v| v.as_array_mut())
    {
        for vector in vectors {
            if let Some(metadata) = vector.get_mut("metadata_info") {
                // Convert object format to array format if needed
                if let serde_json::Value::Object(obj) = metadata {
                    let array_format: Vec<serde_json::Value> = obj
                        .iter()
                        .map(|(key, value)| {
                            let mut item = serde_json::json!({"key": key});
                            match value {
                                serde_json::Value::String(s) => {
                                    item["string_value"] = serde_json::Value::String(s.clone())
                                }
                                serde_json::Value::Number(n) => {
                                    item["double_value"] = serde_json::Value::Number(n.clone())
                                }
                                serde_json::Value::Bool(b) => {
                                    item["bool_value"] = serde_json::Value::Bool(*b)
                                }
                                _ => {
                                    item["string_value"] =
                                        serde_json::Value::String(value.to_string())
                                }
                            }
                            item
                        })
                        .collect();
                    *metadata = serde_json::Value::Array(array_format);
                }
            }
        }
    }

    // Deserialize the modified JSON into our request type
    let request: RestVectorBatchRequest = serde_json::from_value(request_json).map_err(|e| {
        tracing::error!("Failed to deserialize vector batch request: {:?}", e);
        StatusCode::BAD_REQUEST
    })?;

    // Convert REST request to proto request directly
    let proto_request = crate::proto::proximadb::VectorBatchRequest {
        collection_id: request.collection_id.clone(),
        vectors: request
            .vectors
            .into_iter()
            .map(|v| crate::proto::proximadb::VectorRecord {
                expires_at: None,
                quantized_vector: None,
                source: None,
                id: v.id.unwrap_or_default(),
                vector: v.vector.clone(),
                metadata: v
                    .metadata
                    .map(|m| {
                        conversions::convert_metadata_to_proto(serde_json::Map::from_iter(
                            m.into_iter(),
                        ))
                    })
                    .unwrap_or_default(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as u32,
                version: None,
                updated_at: None,
            })
            .collect(),
        batch_timeout_ms: request.batch_timeout_ms,
        request_id: request.request_id,
    };

    // Delegate to unified handlers
    let proto_response = state
        .unified_handlers
        .handle_vector_batch(proto_request)
        .await
        .map_err(|e| {
            tracing::error!("Vector batch operation failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Convert proto response to REST response
    let metrics = proto_response.metrics.unwrap_or(OperationMetrics {
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
        operation: conversions::vector_operation_to_string(proto_response.operation).to_string(),
        metrics: RestOperationMetrics {
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
    // Convert REST request to proto request directly
    // Use the first query if available
    let first_query = request.queries.first();
    let proto_request = conversions::build_vector_search_request(
        request.collection_id.clone(),
        first_query.map(|q| q.vector.clone()).unwrap_or_default(),
        request.top_k,
        None, // TODO: Convert MetadataFilter to serde_json::Map
        request
            .include_fields
            .as_ref()
            .map(|f| f.vector)
            .unwrap_or(false),
        request
            .include_fields
            .as_ref()
            .map(|f| f.metadata)
            .unwrap_or(true),
    );

    // Delegate to unified handlers
    let proto_response = state
        .unified_handlers
        .handle_vector_search(proto_request)
        .await
        .map_err(|e| {
            tracing::error!("Vector search operation failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Convert proto response to REST response
    let results = if let Some(search_result) = proto_response.results {
        search_result
            .results
            .into_iter()
            .map(|r| SearchVectorRecord {
                id: r.id.clone(),
                vector: r.vector,
                metadata: r.metadata,
                score: r.score,
                similarity: r.similarity,
                version: r.version,
                timestamp: r.timestamp,
                source: r.source,
                expanded_context: r.expanded_context,
            })
            .collect()
    } else {
        vec![]
    };

    let metrics = proto_response.metrics.unwrap_or(OperationMetrics {
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
        metrics: RestOperationMetrics {
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

/// Get a single vector by ID
pub async fn vector(
    State(state): State<AppState>,
    Path((collection_id, vector_id)): Path<(String, String)>,
    Query(params): Query<GetVectorParams>,
) -> Result<JsonResponse<VectorGetResponse>, ErrorResponse> {
    let include_vector = params.include_vector.unwrap_or(true);
    let include_metadata = params.include_metadata.unwrap_or(true);

    match state
        .unified_handlers
        .handle_vector(&collection_id, &vector_id, include_vector, include_metadata)
        .await
    {
        Ok(response) => {
            if response.success {
                // Extract the single result from search results
                if let Some(search_result) = response.results {
                    if let Some(result) = search_result.results.first() {
                        let vector_response = VectorGetResponse {
                            id: result.id.clone(),
                            collection_id: collection_id.clone(),
                            vector: if include_vector {
                                Some(result.vector.clone())
                            } else {
                                None
                            },
                            metadata: if include_metadata {
                                Some(crate::core::proto_metadata_helper::proto_metadata_to_json(
                                    &result.metadata,
                                ))
                            } else {
                                None
                            },
                            similarity: Some(result.score),
                            // rank removed -  Some(result.rank),
                        };
                        Ok(Json(vector_response))
                    } else {
                        Err(ErrorResponse {
                            status: StatusCode::NOT_FOUND.as_u16(),
                            message: format!(
                                "Vector '{}' not found in collection '{}'",
                                vector_id, collection_id
                            ),
                            error_code: "NOT_FOUND".to_string(),
                        })
                    }
                } else {
                    Err(ErrorResponse {
                        status: StatusCode::NOT_FOUND.as_u16(),
                        message: format!(
                            "Vector '{}' not found in collection '{}'",
                            vector_id, collection_id
                        ),
                        error_code: "NOT_FOUND".to_string(),
                    })
                }
            } else {
                Err(ErrorResponse {
                    status: StatusCode::NOT_FOUND.as_u16(),
                    message: response
                        .error_message
                        .unwrap_or_else(|| format!("Vector '{}' not found", vector_id)),
                    error_code: response
                        .error_code
                        .unwrap_or_else(|| "NOT_FOUND".to_string()),
                })
            }
        }
        Err(e) => Err(ErrorResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            message: format!("Failed to get vector: {}", e),
            error_code: "INTERNAL_ERROR".to_string(),
        }),
    }
}

/// Execute SQL query endpoint
pub async fn execute_sql(
    State(state): State<AppState>,
    Json(request): Json<SqlQueryRequest>,
) -> Result<JsonResponse<SqlQueryResponse>, ErrorResponse> {
    // Track execution time
    let start_time = std::time::Instant::now();

    // Delegate to unified handlers
    match state
        .unified_handlers
        .execute_sql_query(request.query, request.parameters, request.collection)
        .await
    {
        Ok(result) => {
            let elapsed_ms = start_time.elapsed().as_millis() as f64;

            let response = SqlQueryResponse {
                rows: result.rows,
                columns: result
                    .columns
                    .into_iter()
                    .map(|(name, _data_type)| ColumnInfo { name })
                    .collect(),
                row_count: result.row_count,
                execution_time_ms: elapsed_ms,
            };

            Ok(JsonResponse(response))
        }
        Err(e) => {
            let error_response = ErrorResponse {
                status: 400,
                message: e.to_string(),
                error_code: "SQL_EXECUTION_ERROR".to_string(),
            };
            Err(error_response)
        }
    }
}

/// Get metrics endpoint - thin adapter to UnifiedHandlers
pub async fn metrics(
    State(state): State<AppState>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    // Delegate to unified handlers
    match state.unified_handlers.metrics().await {
        Ok(metrics_data) => Ok(JsonResponse(ApiResponse::success(metrics_data))),
        Err(e) => {
            tracing::error!("Get metrics failed: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get collection-specific metrics endpoint
pub async fn collection_metrics(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    // Parse query options
    let include_hints = params
        .get("include_hints")
        .and_then(|v| v.parse().ok())
        .unwrap_or(false);
    let _include_history = params
        .get("include_history")
        .and_then(|v| v.parse().ok())
        .unwrap_or(false);

    // TODO: Delegate to metrics query service when integrated
    match state
        .unified_handlers
        .collection_metrics(&collection_id, include_hints)
        .await
    {
        Ok(metrics_data) => Ok(JsonResponse(ApiResponse::success(metrics_data))),
        Err(e) => {
            tracing::error!(
                "Get collection metrics failed for {}: {:?}",
                collection_id,
                e
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get query optimization hints endpoint
pub async fn query_hints(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    let query_type = params.get("query_type").cloned();

    // TODO: Delegate to metrics query service when integrated
    match state
        .unified_handlers
        .query_hints(&collection_id, query_type)
        .await
    {
        Ok(hints_data) => Ok(JsonResponse(ApiResponse::success(hints_data))),
        Err(e) => {
            tracing::error!("Get query hints failed for {}: {:?}", collection_id, e);
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
        Ok(stats) => Ok(JsonResponse(ApiResponse::success(stats))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Internal flush specific collection (testing only) - thin adapter to UnifiedHandlers
pub async fn internal_flush_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<ApiResponse<serde_json::Value>>, StatusCode> {
    tracing::warn!(
        "⚠️ INTERNAL FLUSH ENDPOINT CALLED FOR {} - THIS IS FOR TESTING ONLY",
        collection_id
    );

    // Delegate to unified handlers
    match state
        .unified_handlers
        .force_flush_collection(&collection_id)
        .await
    {
        Ok(stats) => Ok(JsonResponse(ApiResponse::success(stats))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Convert index config to proto
fn convert_index_config_to_proto(config: IndexConfiguration) -> IndexConfig {
    let algorithm = match config.algorithm.as_str() {
        "hnsw" => IndexingAlgorithm::Hnsw as i32,
        "ivf" => IndexingAlgorithm::Ivf as i32,
        "flat" => IndexingAlgorithm::Flat as i32,
        "pq" => IndexingAlgorithm::Pq as i32,
        "annoy" => IndexingAlgorithm::Annoy as i32,
        "lsh" => IndexingAlgorithm::Lsh as i32,
        _ => IndexingAlgorithm::Hnsw as i32,
    };

    let update_mode = match config.update_mode.as_str() {
        "synchronous" => IndexUpdateMode::Synchronous as i32,
        "asynchronous" => IndexUpdateMode::Asynchronous as i32,
        "hybrid_mode" => IndexUpdateMode::HybridMode as i32,
        _ => IndexUpdateMode::Synchronous as i32,
    };

    IndexConfig {
        index_name: config.index_name,
        algorithm,
        update_mode,
        async_update_timeout_ms: config.async_update_timeout_ms,
        async_update_batch_size: config.async_update_batch_size.map(|x| x as u32),
        enable_background_optimization: config.enable_background_optimization.unwrap_or(false),
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
        lsh_config: config.lsh_config.map(|c| LshConfig {
            n_hash_tables: c.n_hash_tables,
            n_hash_functions: c.n_hash_functions,
            bucket_width: c.bucket_width,
            binary_vectors: c.binary_vectors,
            max_candidates: c.max_candidates,
            projection: match c.projection.as_str() {
                "binary" => RandomProjectionType::Binary as i32,
                "sparse" => RandomProjectionType::Sparse as i32,
                _ => RandomProjectionType::Gaussian as i32,
            },
        }),
        build_concurrency: config.build_concurrency.map(|x| x as u32),
        memory_limit_mb: config.memory_limit_mb,
        checkpoint_interval_ms: config.checkpoint_interval_ms.map(|x| x as u32),
        is_primary: config.is_primary.unwrap_or(false),
        use_cases: config.use_cases.clone().unwrap_or_default(),
        selectivity_threshold: config.selectivity_threshold,
        use_quantization: None,      // Will inherit from collection by default
        quantization_override: None, // No quantization override by default
        queue_representation: None,  // Default queue representation
    }
}

/// Convert quantization config to proto (new granular format)
fn convert_quantization_config_to_proto(config: RestQuantizationConfig) -> QuantizationConfig {
    // Map from REST structure to new granular proto structure
    let strategy = if let Some(sq) = &config.storage_quantization {
        // Try to infer strategy from the level type in storage config
        match sq.level.level_type.as_str() {
            "pq" | "pq4" | "pq8" | "int8" | "scalar" | "binary" => {
                crate::proto::proximadb::quantization_config::Strategy::CustomLevels
            }
            _ => crate::proto::proximadb::quantization_config::Strategy::SmartDefaults,
        }
    } else {
        crate::proto::proximadb::quantization_config::Strategy::SmartDefaults
    };

    QuantizationConfig {
        enabled: config.enabled,
        strategy: strategy as i32,
        custom_levels: vec![], // TODO: Convert REST levels to proto levels
        enable_progressive_search: true,
        binary_filter_selectivity: 0.3,
        int8_ranking_selectivity: 0.1,
        pq_ranking_selectivity: 0.05,
        training_sample_size: 10000,
        quality_threshold: 0.95,
        enable_adaptive_training: true,
        optimize_for_storage: false,
        optimize_for_memory: false,
        enable_simd_acceleration: true,
        // NEW: Direct quantization type enables
        enable_binary: true,
        enable_int8: true,
        enable_pq: true,
        // Product Quantization specific settings
        pq_segments: 8,
        pq_bits: 8,
        pq_codebooks: vec![],
        // Thresholds for progressive search
        binary_threshold: 0.3,
        int8_threshold: 0.1,
        pq_threshold: 0.05,
    }
}

/// Convert REST config to proto config
fn convert_to_proto_config(config: CollectionConfigJson) -> Result<CollectionConfig, StatusCode> {
    // Simply use the to_proto method we already have
    Ok(config.to_proto())
}

/// Convert index config from proto
fn convert_index_config_from_proto(config: IndexConfig) -> IndexConfiguration {
    IndexConfiguration {
        index_name: config.index_name,
        algorithm: match config.algorithm {
            x if x == IndexingAlgorithm::Hnsw as i32 => "hnsw",
            x if x == IndexingAlgorithm::Ivf as i32 => "ivf",
            x if x == IndexingAlgorithm::Flat as i32 => "flat",
            x if x == IndexingAlgorithm::Pq as i32 => "pq",
            x if x == IndexingAlgorithm::Annoy as i32 => "annoy",
            x if x == IndexingAlgorithm::Lsh as i32 => "lsh",
            _ => "hnsw",
        }
        .to_string(),
        update_mode: match config.update_mode {
            x if x == IndexUpdateMode::Synchronous as i32 => "synchronous",
            x if x == IndexUpdateMode::Asynchronous as i32 => "asynchronous",
            x if x == IndexUpdateMode::HybridMode as i32 => "hybrid_mode",
            _ => "synchronous",
        }
        .to_string(),
        async_update_timeout_ms: config.async_update_timeout_ms,
        async_update_batch_size: config.async_update_batch_size.map(|x| x as i32),
        enable_background_optimization: Some(config.enable_background_optimization),
        hnsw_config: config.hnsw_config.map(|c| RestHnswConfig {
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
        ivf_config: config.ivf_config.map(|c| RestIvfConfig {
            n_lists: c.n_lists,
            n_probe: c.n_probe,
            quantization_bits: c.quantization_bits,
            use_pq: c.use_pq,
            pq_subspaces: c.pq_subspaces,
            train_on_insert: c.train_on_insert,
            min_train_size: c.min_train_size,
        }),
        flat_config: config.flat_config.map(|c| RestFlatConfig {
            enable_simd: c.enable_simd,
            batch_size: c.batch_size,
            enable_parallel_search: c.enable_parallel_search,
        }),
        pq_config: config.pq_config.map(|c| RestPqConfig {
            subvectors: c.subvectors,
            bits_per_subvector: c.bits_per_subvector,
            training_sample_count: c.training_sample_count,
            enable_reranking: c.enable_reranking,
        }),
        annoy_config: config.annoy_config.map(|c| RestAnnoyConfig {
            n_trees: c.n_trees,
            search_k: c.search_k,
            max_leaf_size: c.max_leaf_size,
            enable_mmap: c.enable_mmap,
        }),
        lsh_config: config.lsh_config.map(|c| RestLshConfig {
            n_hash_tables: c.n_hash_tables,
            n_hash_functions: c.n_hash_functions,
            bucket_width: c.bucket_width,
            binary_vectors: c.binary_vectors,
            max_candidates: c.max_candidates,
            projection: match c.projection {
                x if x == RandomProjectionType::Binary as i32 => "binary",
                x if x == RandomProjectionType::Sparse as i32 => "sparse",
                _ => "gaussian",
            }
            .to_string(),
        }),
        build_concurrency: config.build_concurrency.map(|x| x as i32),
        memory_limit_mb: config.memory_limit_mb,
        checkpoint_interval_ms: config.checkpoint_interval_ms.map(|x| x as i32),
        is_primary: Some(config.is_primary),
        use_cases: Some(config.use_cases),
        selectivity_threshold: config.selectivity_threshold,
    }
}

/// Convert quantization config from proto
fn convert_quantization_config_from_proto(config: QuantizationConfig) -> RestQuantizationConfig {
    // The new proto structure is simpler - map to REST structure with defaults
    RestQuantizationConfig {
        enabled: config.enabled,
        storage_quantization: if config.enabled {
            // Create a default storage quantization config based on the strategy
            Some(StorageQuantizationConfig {
                enabled: true,
                level: RestQuantizationLevel {
                    level_type: match config.strategy {
                        x if x
                            == crate::proto::proximadb::quantization_config::Strategy::Aggressive
                                as i32 =>
                        {
                            "pq4"
                        }
                        x if x
                            == crate::proto::proximadb::quantization_config::Strategy::Minimal
                                as i32 =>
                        {
                            "int8"
                        }
                        _ => "pq8",
                    }
                    .to_string(),
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
                codebook_id: None,
                progressive_quantization: config.enable_progressive_search,
                storage_compatibility: "all_engines".to_string(),
            })
        } else {
            None
        },
        index_quantization: None, // Index quantization inherits from storage by default
        search_quantization: if config.enable_progressive_search {
            Some(SearchQuantizationConfig {
                enabled: true,
                default_level: RestQuantizationLevel {
                    level_type: "auto".to_string(),
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
                adaptive_precision: config.enable_adaptive_training,
                accuracy_threshold: config.quality_threshold,
                candidate_multiplier: 2, // Default multiplier
            })
        } else {
            None
        },
        compression_ratio_target: None, // Not in new proto structure
        validation: Some(QuantizationValidation {
            accuracy_threshold: config.quality_threshold,
            validation_sample_size: config.training_sample_size,
            enable_quality_monitoring: config.enable_adaptive_training,
            retraining_threshold: 0.9, // Default threshold
        }),
    }
}

/// Convert quantization level from proto
// TODO: Restore when QuantizationLevel and LevelType are available
/* fn convert_quantization_level_from_proto(level: QuantizationLevel) -> RestQuantizationLevel {

    match level.level_type {
        Some(LevelType::None(_)) => RestQuantizationLevel {
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
        Some(LevelType::Uniform(u)) => RestQuantizationLevel {
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
        Some(LevelType::Pq(p)) => RestQuantizationLevel {
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
        Some(LevelType::Scalar(s)) => RestQuantizationLevel {
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
        Some(LevelType::Binary(b)) => RestQuantizationLevel {
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
        Some(LevelType::Custom(c)) => RestQuantizationLevel {
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
        None => RestQuantizationLevel {
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
} */

/// Convert proto collection to REST collection
fn convert_from_proto_collection(proto: ProtoCollection) -> Collection {
    let config = proto.config.clone();

    Collection {
        id: proto.id,
        config: config
            .map(|c| CollectionConfigJson::from_proto(&c))
            .unwrap_or_else(|| {
                // Create a minimal default config if none exists
                CollectionConfigJson {
                    name: String::new(),
                    dimension: 0,
                    distance_metric: None,
                    storage_engine: None,
                    filterable_columns: None,
                    index_configs: None,
                    quantization: None,
                    storage_config: None,
                    primary_index: None,
                    auto_index_selection: None,
                    description: None,
                    tags: None,
                    owner: None,
                }
            }),
        stats: CollectionStats {
            vector_count: proto.stats.as_ref().map(|s| s.vector_count).unwrap_or(0),
            index_size_bytes: proto
                .stats
                .as_ref()
                .map(|s| s.index_size_bytes)
                .unwrap_or(0),
            data_size_bytes: proto.stats.as_ref().map(|s| s.data_size_bytes).unwrap_or(0),
        },
        timestamp: proto.created_at,
        updated_at: proto.updated_at,
    }
}

/// List all collections
pub async fn list_collections(
    State(state): State<AppState>,
) -> Result<JsonResponse<ListCollectionsResponse>, ErrorResponse> {
    tracing::info!("📋 REST API: Listing all collections");

    let collections = state
        .unified_handlers
        .list_collections()
        .await
        .map_err(|e| {
            tracing::error!("Failed to list collections: {:?}", e);
            ErrorResponse {
                status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                message: "Failed to list collections".to_string(),
                error_code: "LIST_FAILED".to_string(),
            }
        })?;

    // Convert proto Collections to REST response
    let collection_responses: Vec<CollectionInfo> = collections
        .into_iter()
        .map(|c| {
            let config = c.config.as_ref();
            let stats = c.stats.as_ref();

            CollectionInfo {
                id: c.id,
                name: config.map(|cfg| cfg.name.clone()).unwrap_or_default(),
                dimension: config.map(|cfg| cfg.dimension as usize).unwrap_or(0),
                metric: match config.and_then(|cfg| Some(cfg.distance_metric)) {
                    Some(x) if x == DistanceMetric::Cosine as i32 => "cosine",
                    Some(x) if x == DistanceMetric::Euclidean as i32 => "euclidean",
                    Some(x) if x == DistanceMetric::DotProduct as i32 => "dot_product",
                    _ => "cosine",
                }
                .to_string(),
                timestamp: c.created_at,
                updated_at: c.updated_at,
                vector_count: stats.map(|s| s.vector_count as u64),
                indexed: stats.map(|s| s.index_size_bytes > 0).unwrap_or(false),
            }
        })
        .collect();

    let total_count = collection_responses.len() as u32;

    Ok(JsonResponse(ListCollectionsResponse {
        collections: collection_responses,
        total_count,
    }))
}

/// Get a specific collection by ID
pub async fn collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<CollectionInfo>, ErrorResponse> {
    tracing::info!("🔍 REST API: Getting collection: {}", collection_id);

    let collection = state
        .unified_handlers
        .collection(&collection_id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to get collection: {:?}", e);
            ErrorResponse {
                status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                message: "Failed to get collection".to_string(),
                error_code: "GET_FAILED".to_string(),
            }
        })?;

    match collection {
        Some(c) => {
            let config = c.config.as_ref();
            let stats = c.stats.as_ref();

            let collection_info = CollectionInfo {
                id: c.id,
                name: config.map(|cfg| cfg.name.clone()).unwrap_or_default(),
                dimension: config.map(|cfg| cfg.dimension as usize).unwrap_or(0),
                metric: match config.and_then(|cfg| Some(cfg.distance_metric)) {
                    Some(x) if x == DistanceMetric::Cosine as i32 => "cosine",
                    Some(x) if x == DistanceMetric::Euclidean as i32 => "euclidean",
                    Some(x) if x == DistanceMetric::DotProduct as i32 => "dot_product",
                    _ => "cosine",
                }
                .to_string(),
                timestamp: c.created_at,
                updated_at: c.updated_at,
                vector_count: stats.map(|s| s.vector_count as u64),
                indexed: stats.map(|s| s.index_size_bytes > 0).unwrap_or(false),
            };
            Ok(JsonResponse(collection_info))
        }
        None => Err(ErrorResponse {
            status: StatusCode::NOT_FOUND.as_u16(),
            message: format!("Collection with ID '{}' does not exist", collection_id),
            error_code: "NOT_FOUND".to_string(),
        }),
    }
}

/// Delete a collection using standard REST DELETE verb
pub async fn delete_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<CollectionResponse>, StatusCode> {
    tracing::info!("🗑️ REST API: Deleting collection: {}", collection_id);

    // Create a proto request for delete operation
    let proto_request = CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        migration_config: std::collections::HashMap::new(),
        query_params: std::collections::HashMap::new(),
        options: std::collections::HashMap::new(),
    };

    // Delegate to unified handlers
    let proto_response = state
        .unified_handlers
        .handle_collection_operation(proto_request)
        .await
        .map_err(|e| {
            tracing::error!("Collection deletion failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Convert proto response to REST response
    let response = CollectionResponse {
        success: proto_response.success,
        operation: "delete".to_string(),
        collection: proto_response
            .collection
            .map(|c| convert_from_proto_collection(c)),
        collections: None,
        affected_count: proto_response.affected_count,
        total_count: proto_response.total_count.map(|x| x as u64),
        metadata: proto_response.metadata.into_iter().collect(),
        error_message: proto_response.error_message,
        error_code: proto_response.error_code,
        processing_time_us: proto_response.processing_time_us,
    };

    Ok(JsonResponse(response))
}

/// Delete vectors using standard REST DELETE verb with JSON body (supports single or multiple)
pub async fn delete_vectors(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Json(request): Json<serde_json::Value>,
) -> Result<JsonResponse<VectorOperationResponse>, StatusCode> {
    tracing::info!(
        "🗑️ REST API: Batch deleting vectors from collection {}",
        collection_id
    );

    // Extract vector IDs from request body
    let vector_ids: Vec<String> = match request.get("ids") {
        Some(ids_value) => {
            serde_json::from_value(ids_value.clone()).map_err(|_| StatusCode::BAD_REQUEST)?
        }
        None => return Err(StatusCode::BAD_REQUEST),
    };

    // Create tombstone vector records with expires_at set to mark for deletion
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let tombstone_vectors: Vec<VectorRecord> = vector_ids
        .into_iter()
        .map(|id| VectorRecord {
            id: id.clone(),
            vector: vec![], // Empty vector for tombstone
            metadata: vec![],
            timestamp: (current_time / 1000) as u32,
            updated_at: Some((current_time / 1000) as u32),
            expires_at: Some((current_time / 1000) as u32), // Mark for deletion (convert ms to seconds)
            version: Some(1),
            quantized_vector: None,
            source: None, // No source for tombstone
        })
        .collect();

    // Create a proto request for batch operation with tombstone vectors
    let proto_request = VectorBatchRequest {
        collection_id: collection_id.clone(),
        vectors: tombstone_vectors,
        batch_timeout_ms: None,
        request_id: None,
    };

    // Delegate to unified handlers
    let proto_response = state
        .unified_handlers
        .handle_vector_batch(proto_request)
        .await
        .map_err(|e| {
            tracing::error!("Batch vector deletion failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Convert proto response to REST response
    let metrics = proto_response.metrics.unwrap_or(OperationMetrics {
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
        operation: "delete".to_string(),
        metrics: RestOperationMetrics {
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

/// 🛠️ TEMPORARY DEBUG: List all unflushed vectors for a collection
pub async fn debug_list_unflushed_vectors(
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<JsonResponse<serde_json::Value>, ErrorResponse> {
    tracing::info!(
        "🔍 DEBUG REST: Listing unflushed vectors for collection: {}",
        collection_id
    );

    match state
        .unified_handlers
        .vector_operations_service
        .debug_list_all_unflushed_vectors(&collection_id)
        .await
    {
        Ok(vectors) => {
            let debug_info = serde_json::json!({
                "collection_id": collection_id,
                "unflushed_vector_count": vectors.len(),
                "vectors": vectors.iter().map(|v| serde_json::json!({
                    "id": v.id,
                    "vector_length": v.vector.len(),
                    "metadata_count": v.metadata.len(),
                    "vector_preview": v.vector.iter().take(4).cloned().collect::<Vec<f32>>(),
                    "metadata_info": v.metadata.iter().map(|m| serde_json::json!({
                        "key": m.key,
                        "value": m.value
                    })).collect::<Vec<_>>()
                })).collect::<Vec<_>>()
            });

            Ok(JsonResponse(debug_info))
        }
        Err(e) => {
            tracing::error!("🔍 DEBUG REST: Failed to list unflushed vectors: {:?}", e);
            Err(ErrorResponse {
                status: 500,
                message: format!("Failed to list unflushed vectors: {}", e),
                error_code: "INTERNAL_ERROR".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod handlers_metadata_test;

#[cfg(test)]
mod handlers_simple_test;
// Note: Tests now use real UnifiedHandlers instance for integration testing
