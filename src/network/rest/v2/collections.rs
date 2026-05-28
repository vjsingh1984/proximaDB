/*
 * Copyright 2025 ProximaDB
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

//! Collection management with schema support for v2 API
//!
//! This module provides REST endpoints for creating and managing collections
//! with ProximaRecord support and typed schema definitions.
//!
//! ## Endpoints
//!
//! - `POST /api/v2/collections` - Create collection with schema
//! - `GET /api/v2/collections/{id}` - Get collection details
//!
//! ## Schema Enforcement Modes
//!
//! - **Strict**: All columns must match schema exactly
//! - **Flexible**: Schema on read, no validation at insert
//! - **Hybrid**: Core columns enforced, additional fields allowed (default)

use axum::{
    Json,
    extract::{Extension, Path, Query, State},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::errors::{ApiError, ApiResult};
use crate::network::middleware::tenant::TenantContext;
use crate::network::rest::v1::handlers::AppState;
use crate::proto::proximadb_v1::{CollectionConfig, CollectionOperation, CollectionRequest};

fn collection_storage_engine_label(storage_engine: Option<i32>) -> &'static str {
    match storage_engine {
        Some(raw) if raw != 0 => crate::core::conversions::storage_engine_to_string(raw),
        _ => "auto",
    }
}

fn collection_distance_metric_label(distance_metric: Option<i32>) -> &'static str {
    match distance_metric {
        Some(raw) if raw != 0 => crate::core::conversions::distance_metric_to_string(raw),
        _ => "cosine",
    }
}

fn non_negative_stat(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Request to create a collection with schema support
///
/// ## Example JSON
///
/// ```json
/// {
///     "name": "products",
///     "dimension": 768,
///     "engine": "sst",
///     "schema": {
///         "columns": [
///             {"name": "category", "data_type": "text", "indexed": true},
///             {"name": "price", "data_type": "float", "filterable": true},
///             {"name": "description", "data_type": "text", "max_length": 10000}
///         ],
///         "enforcement": "hybrid",
///         "allow_additional_fields": true
///     },
///     "enable_proxima_record": true
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct CreateCollectionV2Request {
    /// Collection name (required)
    pub name: String,
    /// Vector dimension (required)
    pub dimension: u32,
    /// Storage engine selection
    ///
    /// Options: "auto", "sst", "helix", "viper", "swift", "nova", "raptor", "tst"
    /// Default: "auto" (system selects optimal engine)
    pub engine: Option<String>,
    /// Schema definition with column types
    pub schema: Option<SchemaDefinition>,
    /// Enable ProximaRecord support for this collection
    ///
    /// When enabled:
    /// - Records can use rich props and text_fields
    /// - Schema validation is applied at insert time
    /// - TEXT columns are stored in dedicated columnar format
    ///
    /// Default: false (backward compatible with v1)
    pub enable_proxima_record: Option<bool>,
    /// Distance metric for vector similarity
    ///
    /// Options: "cosine", "euclidean", "dot_product"
    /// Default: "cosine"
    pub distance_metric: Option<String>,
    /// Initial capacity hint for pre-allocation
    pub initial_capacity: Option<u64>,
    /// Canonical embedding precision for stored vectors
    ///
    /// Options: "fp32" (default), "fp16", "bf16", "int8", "uint8".
    /// Accepts the same string forms as the gRPC / Arrow Flight surfaces
    /// (e.g. "half", "float16", "EMBEDDING_PRECISION_FP16"). The DDL
    /// service applies the same normalisation, so SDKs and pgwire
    /// converge on the same enum discriminant.
    pub canonical_embedding_precision: Option<String>,
}

/// Schema definition for a collection
///
/// Defines the typed columns and enforcement rules for ProximaRecord support.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SchemaDefinition {
    /// Column definitions
    pub columns: Vec<RestColumnDefinition>,
    /// Schema enforcement mode
    ///
    /// - "strict": All columns must match schema exactly
    /// - "flexible": Schema on read, no validation at insert
    /// - "hybrid": Core columns enforced, additional fields allowed (default)
    pub enforcement: Option<String>,
    /// Allow additional fields not defined in schema
    ///
    /// Only applies in "hybrid" mode.
    /// Default: true
    pub allow_additional_fields: Option<bool>,
}

/// Backwards-compat alias for [`RestColumnDefinition`].
pub type ColumnDefinition = RestColumnDefinition;

/// Column definition for schema
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RestColumnDefinition {
    /// Column name
    pub name: String,
    /// Data type
    ///
    /// Supported types:
    /// - "text": Variable-length UTF-8 text
    /// - "text_large": Large text with sidecar storage
    /// - "integer": 64-bit signed integer
    /// - "float": 64-bit floating point
    /// - "decimal": 128-bit decimal (precision, scale)
    /// - "boolean": True/false
    /// - "timestamp": Microseconds since epoch
    /// - "timestamp_tz": Timestamp with timezone
    /// - "date": Days since epoch
    /// - "time": Microseconds since midnight
    /// - "uuid": RFC 4122 UUID
    /// - "binary": Raw bytes
    /// - "json": Validated JSON
    /// - "array_text", "array_integer", "array_float", "array_boolean"
    /// - "map_string_string", "map_string_any"
    /// - "geo_point": Latitude/longitude point
    /// - "vector": Fixed-dimension vector (specify dimension)
    pub data_type: String,
    /// Whether null values are allowed
    ///
    /// Default: true
    pub nullable: Option<bool>,
    /// Create secondary index for this column
    ///
    /// Improves query performance for equality/range filters.
    /// Default: false
    pub indexed: Option<bool>,
    /// Enable filtering on this column
    ///
    /// When true, the column can be used in WHERE clauses.
    /// Default: true for indexed columns, false otherwise
    pub filterable: Option<bool>,
    /// Maximum length for TEXT/BINARY columns
    ///
    /// Default: no limit
    pub max_length: Option<u32>,
    /// Precision for DECIMAL type (1-38)
    pub precision: Option<u8>,
    /// Scale for DECIMAL type (0-precision)
    pub scale: Option<u8>,
    /// Dimension for VECTOR type
    pub vector_dimension: Option<u32>,
}

/// Response for collection creation
#[derive(Debug, Serialize)]
pub struct CreateCollectionV2Response {
    /// Collection ID (same as name)
    pub collection_id: String,
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: u32,
    /// Selected storage engine
    pub engine: String,
    /// Whether ProximaRecord is enabled
    pub proxima_record_enabled: bool,
    /// Schema ID (if schema was defined)
    pub schema_id: Option<String>,
    /// Creation timestamp
    pub created_at: String,
}

/// POST /api/v2/collections
///
/// Create a new collection with optional schema support.
///
/// ## Request Body
///
/// See [`CreateCollectionV2Request`] for the expected JSON format.
///
/// ## Response
///
/// Returns [`CreateCollectionV2Response`] with collection details.
///
/// ## Errors
///
/// - `400 Bad Request`: Invalid request or schema
/// - `409 Conflict`: Collection already exists
/// - `500 Internal Server Error`: Creation failed
pub async fn create_collection_v2(
    Extension(tenant): Extension<TenantContext>,
    State(state): State<AppState>,
    Json(request): Json<CreateCollectionV2Request>,
) -> ApiResult<Json<CreateCollectionV2Response>> {
    info!(
        "V2 API: Creating collection '{}' with dimension {}",
        request.name, request.dimension
    );

    // Validate request
    if request.name.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection name is required".to_string(),
        ));
    }

    if request.dimension == 0 {
        return Err(ApiError::InvalidArgument(
            "Vector dimension must be greater than 0".to_string(),
        ));
    }

    // Validate engine if specified
    let engine = request.engine.as_deref().unwrap_or("auto");
    let valid_engines = [
        "auto", "sst", "helix", "viper", "swift", "nova", "raptor", "tst",
    ];
    if !valid_engines.contains(&engine) {
        return Err(ApiError::InvalidArgument(format!(
            "Invalid storage engine '{}'. Valid engines: {:?}",
            engine, valid_engines
        )));
    }

    // Validate distance metric if specified
    if let Some(ref metric) = request.distance_metric {
        let valid_metrics = ["cosine", "euclidean", "dot_product"];
        if !valid_metrics.contains(&metric.as_str()) {
            return Err(ApiError::InvalidArgument(format!(
                "Invalid distance metric '{}'. Valid metrics: {:?}",
                metric, valid_metrics
            )));
        }
    }

    // Validate schema if provided
    let schema_id = if let Some(ref schema) = request.schema {
        // Validate enforcement mode
        if let Some(ref enforcement) = schema.enforcement {
            let valid_modes = ["strict", "flexible", "hybrid"];
            if !valid_modes.contains(&enforcement.as_str()) {
                return Err(ApiError::InvalidArgument(format!(
                    "Invalid schema enforcement mode '{}'. Valid modes: {:?}",
                    enforcement, valid_modes
                )));
            }
        }

        // Validate columns
        for column in &schema.columns {
            if column.name.is_empty() {
                return Err(ApiError::InvalidArgument(
                    "Column name cannot be empty".to_string(),
                ));
            }

            // Validate data type
            let valid_types = [
                "text",
                "text_large",
                "integer",
                "float",
                "decimal",
                "boolean",
                "timestamp",
                "timestamp_tz",
                "date",
                "time",
                "uuid",
                "binary",
                "json",
                "array_text",
                "array_integer",
                "array_float",
                "array_boolean",
                "map_string_string",
                "map_string_any",
                "geo_point",
                "vector",
            ];
            if !valid_types.contains(&column.data_type.as_str()) {
                return Err(ApiError::InvalidArgument(format!(
                    "Invalid data type '{}' for column '{}'. Valid types: {:?}",
                    column.data_type, column.name, valid_types
                )));
            }

            // Validate decimal precision/scale
            if column.data_type == "decimal" {
                let precision = column.precision.ok_or_else(|| {
                    ApiError::InvalidArgument(format!(
                        "Column '{}' with type 'decimal' requires precision",
                        column.name
                    ))
                })?;
                let scale = column.scale.ok_or_else(|| {
                    ApiError::InvalidArgument(format!(
                        "Column '{}' with type 'decimal' requires scale",
                        column.name
                    ))
                })?;
                if precision == 0 || precision > 38 {
                    return Err(ApiError::InvalidArgument(format!(
                        "Column '{}': decimal precision must be between 1 and 38",
                        column.name
                    )));
                }
                if scale > precision {
                    return Err(ApiError::InvalidArgument(format!(
                        "Column '{}': decimal scale cannot exceed precision",
                        column.name
                    )));
                }
            }

            // Validate vector dimension
            if column.data_type == "vector" && column.vector_dimension.is_none() {
                return Err(ApiError::InvalidArgument(format!(
                    "Column '{}' with type 'vector' requires vector_dimension",
                    column.name
                )));
            }
        }

        // Generate schema ID
        Some(uuid::Uuid::new_v4().to_string())
    } else {
        None
    };

    let proxima_record_enabled = request.enable_proxima_record.unwrap_or(false);

    debug!(
        "Creating collection: engine={}, proxima_record={}, schema={:?}",
        engine, proxima_record_enabled, schema_id
    );

    // Create collection config for unified handlers
    // Map engine name to StorageEngine enum value
    let storage_engine_value = if engine == "auto" {
        crate::proto::proximadb_v1::StorageEngine::Unspecified as i32
    } else {
        crate::core::conversions::parse_storage_engine(engine)
            .map(|engine| engine as i32)
            .map_err(|e| ApiError::InvalidArgument(e.to_string()))?
    };

    // Map distance metric name to DistanceMetric enum value
    let distance_metric_value = match request.distance_metric.as_deref() {
        Some(metric) => Some(
            crate::core::conversions::parse_distance_metric(metric)
                .map(|metric| metric as i32)
                .map_err(|e| ApiError::InvalidArgument(e.to_string()))?,
        ),
        None => Some(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32),
    };

    // Map canonical_embedding_precision label to the proto discriminant
    // (matches the dispatch the DDL service / Arrow Flight DoAction use, so
    // every protocol resolves the same string to the same enum value).
    let canonical_embedding_precision = request
        .canonical_embedding_precision
        .as_deref()
        .and_then(|raw| {
            use crate::proto::proximadb_v1::EmbeddingPrecision;
            let key = raw.trim().to_ascii_lowercase();
            let stripped = key.strip_prefix("embedding_precision_").unwrap_or(&key);
            match stripped {
                "unspecified" => Some(EmbeddingPrecision::Unspecified),
                "fp32" | "f32" | "float32" => Some(EmbeddingPrecision::Fp32),
                "fp16" | "f16" | "half" | "float16" => Some(EmbeddingPrecision::Fp16),
                "bf16" | "bfloat16" => Some(EmbeddingPrecision::Bf16),
                "int8" | "i8" | "int8_scalar" => Some(EmbeddingPrecision::Int8),
                "uint8" | "u8" | "uint8_scalar" => Some(EmbeddingPrecision::Uint8),
                _ => None,
            }
        })
        .map(|p| p as i32);

    let collection_config = CollectionConfig {
        name: request.name.clone(),
        dimension: request.dimension,
        storage_engine: Some(storage_engine_value),
        distance_metric: distance_metric_value,
        canonical_embedding_precision,
        ..Default::default()
    };

    let collection_request = CollectionRequest {
        operation: CollectionOperation::CollectionCreate as i32,
        collection_id: Some(request.name.clone()),
        collection_config: Some(collection_config),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    // Create collection via unified handlers
    match state
        .request_handlers
        .handle_collection_operation_for_tenant(collection_request, Some(&tenant.tenant_id))
        .await
    {
        Ok(_resp) => {
            let response = CreateCollectionV2Response {
                collection_id: request.name.clone(),
                name: request.name,
                dimension: request.dimension,
                engine: engine.to_string(),
                proxima_record_enabled,
                schema_id,
                created_at: chrono::Utc::now().to_rfc3339(),
            };

            info!(
                "V2 API: Collection '{}' created successfully",
                response.collection_id
            );

            Ok(Json(response))
        }
        Err(e) => {
            if e.to_string().contains("already exists") {
                Err(ApiError::AlreadyExists(format!(
                    "Collection '{}' already exists",
                    request.name
                )))
            } else {
                Err(ApiError::Internal(format!(
                    "Failed to create collection: {}",
                    e
                )))
            }
        }
    }
}

/// Collection details response
#[derive(Debug, Serialize)]
pub struct CollectionV2Response {
    /// Collection ID
    pub collection_id: String,
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: u32,
    /// Storage engine
    pub engine: String,
    /// Distance metric
    pub distance_metric: String,
    /// Whether ProximaRecord is enabled
    pub proxima_record_enabled: bool,
    /// Schema definition (if defined)
    pub schema: Option<SchemaDefinition>,
    /// Collection statistics
    pub stats: CollectionStatsV2,
    /// Creation timestamp
    pub created_at: String,
    /// Last update timestamp
    pub updated_at: Option<String>,
}

/// Collection statistics for v2 API
#[derive(Debug, Serialize)]
pub struct CollectionStatsV2 {
    /// Total number of records
    pub record_count: u64,
    /// Total storage size in bytes
    pub storage_size_bytes: u64,
    /// Number of indexed fields
    pub indexed_fields: u32,
    /// Number of TEXT fields with dedicated storage
    pub text_field_count: u32,
}

/// GET /api/v2/collections/{collection_id}
///
/// Get collection details including schema and statistics.
///
/// ## Path Parameters
///
/// - `collection_id`: Collection name/ID
///
/// ## Response
///
/// Returns [`CollectionV2Response`] with collection details.
///
/// ## Errors
///
/// - `404 Not Found`: Collection does not exist
/// - `500 Internal Server Error`: Retrieval failed
pub async fn get_collection_v2(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
    State(state): State<AppState>,
) -> ApiResult<Json<CollectionV2Response>> {
    debug!("V2 API: Getting collection '{}'", collection_id);

    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    // Get collection via unified handlers
    let request = CollectionRequest {
        operation: CollectionOperation::CollectionGet as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    match state
        .request_handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(resp) => {
            // Extract collection info from response
            let collection = resp.collection.unwrap_or_default();
            let config = collection.config.unwrap_or_default();
            let stats = collection.stats.unwrap_or_default();

            let engine_str = collection_storage_engine_label(config.storage_engine);
            let distance_metric_str = collection_distance_metric_label(config.distance_metric);

            let response = CollectionV2Response {
                collection_id: collection_id.clone(),
                name: collection_id,
                dimension: config.dimension,
                engine: engine_str.to_string(),
                distance_metric: distance_metric_str.to_string(),
                proxima_record_enabled: false, // Would be stored in metadata
                schema: None,                  // Would be loaded from metadata
                stats: CollectionStatsV2 {
                    record_count: non_negative_stat(stats.vector_count),
                    storage_size_bytes: non_negative_stat(stats.data_size_bytes),
                    indexed_fields: 0,
                    text_field_count: 0,
                },
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: None,
            };

            Ok(Json(response))
        }
        Err(e) => {
            if e.to_string().contains("not found") {
                Err(ApiError::CollectionNotFound(collection_id))
            } else {
                Err(ApiError::Internal(format!(
                    "Failed to get collection: {}",
                    e
                )))
            }
        }
    }
}

/// Query parameters for listing collections
#[derive(Debug, Deserialize)]
pub struct ListCollectionsV2Query {
    /// Maximum number of collections to return (default: 100)
    pub limit: Option<u32>,
    /// Offset for pagination (default: 0)
    pub offset: Option<u32>,
    /// Whether to include statistics
    pub include_stats: Option<bool>,
}

/// Response for listing collections
#[derive(Debug, Serialize)]
pub struct ListCollectionsV2Response {
    /// List of collections
    pub collections: Vec<CollectionV2Summary>,
    /// Total count of collections
    pub total: u64,
    /// Limit used in this request
    pub limit: u32,
    /// Offset used in this request
    pub offset: u32,
    /// Whether there are more results
    pub has_more: bool,
}

/// Response for deleting a collection through the v2 API.
#[derive(Debug, Serialize)]
pub struct DeleteCollectionV2Response {
    /// Whether the delete request was accepted.
    pub success: bool,
    /// Deleted collection ID.
    pub collection_id: String,
}

/// Summary of a collection for list operations
#[derive(Debug, Serialize)]
pub struct CollectionV2Summary {
    /// Collection ID
    pub collection_id: String,
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: u32,
    /// Storage engine
    pub engine: String,
    /// Whether ProximaRecord is enabled
    pub proxima_record_enabled: bool,
    /// Record count (if include_stats is true)
    pub record_count: Option<u64>,
}

/// GET /api/v2/collections
///
/// List all collections with pagination.
///
/// ## Query Parameters
///
/// - `limit`: Maximum number of collections to return (default: 100)
/// - `offset`: Offset for pagination (default: 0)
/// - `include_stats`: Whether to include collection statistics
///
/// ## Response
///
/// Returns [`ListCollectionsV2Response`] with collection list.
///
/// ## Errors
///
/// - `500 Internal Server Error`: List operation failed
pub async fn list_collections_v2(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Query(params): Query<ListCollectionsV2Query>,
) -> ApiResult<Json<ListCollectionsV2Response>> {
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);
    let include_stats = params.include_stats.unwrap_or(false);

    debug!(
        "V2 API: Listing collections, limit={}, offset={}, include_stats={}",
        limit, offset, include_stats
    );

    let mut query_params = std::collections::HashMap::new();
    query_params.insert("limit".to_string(), limit.to_string());
    query_params.insert("offset".to_string(), offset.to_string());

    let mut options = std::collections::HashMap::new();
    options.insert("include_stats".to_string(), include_stats);

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionList as i32,
        collection_id: None,
        collection_config: None,
        query_params,
        options,
        migration_config: Default::default(),
    };

    match state
        .request_handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(resp) => {
            let collections: Vec<CollectionV2Summary> = resp
                .collections
                .iter()
                .map(|c| {
                    let cfg = c.config.as_ref();
                    // Map storage engine enum to string
                    let engine_str = match cfg.and_then(|c| c.storage_engine).unwrap_or(0) {
                        1 => "sst",
                        2 => "helix",
                        3 => "viper",
                        4 => "swift",
                        5 => "nova",
                        6 => "raptor",
                        _ => "auto",
                    };
                    CollectionV2Summary {
                        collection_id: c.id.clone(),
                        name: c.id.clone(),
                        dimension: cfg.map_or(0, |cfg| cfg.dimension),
                        engine: engine_str.to_string(),
                        proxima_record_enabled: false,
                        record_count: if include_stats {
                            Some(c.stats.as_ref().map_or(0, |s| s.vector_count as u64))
                        } else {
                            None
                        },
                    }
                })
                .collect();

            let total_count = resp.total_count as u64;
            let has_more = (offset as u64 + collections.len() as u64) < total_count;

            let response = ListCollectionsV2Response {
                collections,
                total: total_count,
                limit,
                offset,
                has_more,
            };

            Ok(Json(response))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to list collections: {}",
            e
        ))),
    }
}

/// DELETE /api/v2/collections/{collection_id}
///
/// Delete a collection by ID/name. This v2 route keeps SDK lifecycle methods on
/// the ProximaRecord-era API while delegating to the existing collection control
/// plane.
pub async fn delete_collection_v2(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
    State(state): State<AppState>,
) -> ApiResult<Json<DeleteCollectionV2Response>> {
    info!("V2 API: Deleting collection '{}'", collection_id);

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

    state
        .request_handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::CollectionNotFound(collection_id.clone())
            } else {
                ApiError::Internal(format!("Failed to delete collection: {}", e))
            }
        })?;

    Ok(Json(DeleteCollectionV2Response {
        success: true,
        collection_id,
    }))
}

// ---------------------------------------------------------------------------
// Route-health diagnostic endpoint (experimental, v1)
//
// Exposes a machine-readable capability contract for a single collection.
// Each field reports state that is verifiable from the codebase today; gaps
// are surfaced as typed `degraded_reasons` rather than hidden behind
// optimistic defaults. Lives under `/_diagnostics/collections/...` to signal
// that the JSON shape may evolve before it graduates to `/collections/...`.
// ---------------------------------------------------------------------------

/// Top-level route-health response. Versioned via `schema_version` so the
/// shape can evolve without breaking consumers that have pinned the contract.
#[derive(Debug, Serialize, PartialEq)]
pub struct CollectionRouteHealthV2 {
    pub schema_version: &'static str,
    pub stability: &'static str,
    pub collection_id: String,
    pub engine: String,
    pub dimension: u32,
    pub distance_metric: String,
    pub record_count: u64,
    pub storage_size_bytes: u64,
    pub index_size_bytes: u64,

    pub filtered_ann: FilteredAnnHealth,
    pub writes: WriteContractHealth,
    pub freshness: FreshnessHealth,
    pub object_economy: ObjectEconomyHealth,
    pub recall_probe: RecallProbeHealth,

    pub degraded_reasons: Vec<DegradedReason>,
}

/// Filtered-ANN capability state. Reflects the current AXIS HNSW predicate
/// path: ID filters and ProximaRecord-backed metadata predicates are both
/// evaluated during traversal, then reapplied as a residual guard. The older
/// standalone `make_id_predicate` helper is still ID-only, but the manager's
/// query path uses the record-aware bridge.
#[derive(Debug, Serialize, PartialEq)]
pub struct FilteredAnnHealth {
    /// ID-based filter clauses are evaluated inside the index traversal.
    pub id_predicate_supported: bool,
    /// Non-ID metadata predicates are evaluated against ProximaRecord-derived
    /// metadata during AXIS HNSW traversal.
    pub record_aware_predicates: bool,
    /// `MetadataFilterPushdown` infrastructure (bloom filters, column stats,
    /// selectivity estimator) exists in `src/core/search/metadata_filter_pushdown.rs`.
    pub predicate_pushdown_infrastructure_present: bool,
    /// Whether that infrastructure is wired into the default query path.
    /// Today: container is built, runtime integration is minimal.
    pub predicate_pushdown_default_wired: bool,
    /// Whether the planner discloses post-filter shortfall in EXPLAIN /
    /// result metadata.
    pub post_filter_shortfall_disclosure: bool,
    /// Tracking ID for the open design item. Lets clients correlate the
    /// state reported here with the design doc.
    pub td_064_status: &'static str,
}

/// Write-contract state. Mirrors the modes actually wired through the
/// v2 records surface, not aspirational batch-mode enums.
#[derive(Debug, Serialize, PartialEq)]
pub struct WriteContractHealth {
    pub insert: bool,
    pub upsert: bool,
    pub update: bool,
    pub delete: bool,
    /// Conditional writes (compare-and-set semantics) are not wired today.
    pub conditional_write: bool,
    /// Filter writes (delete/update where predicate) are not wired today.
    pub filter_write: bool,
    /// Patch (partial property update) is not wired today.
    pub patch: bool,
}

/// Freshness state. Collection-level strong / bounded-stale / stale-ok modes
/// are not wired yet — only per-projection freshness exists in the storage
/// layer. We report this honestly rather than fabricating booleans.
#[derive(Debug, Serialize, PartialEq)]
pub struct FreshnessHealth {
    pub scope: &'static str,
    pub collection_level_modes_wired: bool,
    pub notes: &'static str,
}

/// Object-economy directory state. The directory format and sidecar live in
/// the SST engine; route-health reports cached in-process status when present
/// without forcing an object-storage read.
#[derive(Debug, Serialize, PartialEq)]
pub struct ObjectEconomyHealth {
    pub eligible: bool,
    pub directory_format_present: bool,
    pub live_status_in_app_state: bool,
    pub live_status: &'static str,
    pub route_hint: Option<&'static str>,
    pub notes: &'static str,
}

/// Recall-probe gate state. The `RecallProbeGate` exists with a complete
/// state machine but is not wired into the query path or `AppState` yet.
#[derive(Debug, Serialize, PartialEq)]
pub struct RecallProbeHealth {
    pub implementation_present: bool,
    pub wired_to_query_path: bool,
    pub live_state_in_app_state: bool,
    /// Per-scope (tenant + collection) gate-open state when the gate is
    /// reachable from `AppState`. `None` when the gate isn't wired into
    /// `AppState` for this deployment, or when the scope has never been
    /// observed (default-closed). The value is taken from
    /// `RecallProbeGate::is_open` at request time and reflects the most
    /// recent probe outcome held in memory.
    pub gate_open: Option<bool>,
    pub notes: &'static str,
}

/// Typed reasons the collection's route is degraded. Closed enum — adding a
/// reason is a contract change, which is the point.
#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DegradedReason {
    FilteredAnnRecordPredicateBridgePartial,
    PostFilterShortfallNotDisclosed,
    ObjectEconomyLiveStatusNotReachable,
    ObjectEconomyDirectoryDegraded,
    RecallProbeNotWired,
    FreshnessModesNotCollectionLevel,
    ConditionalWritesUnsupported,
    FilterWritesUnsupported,
}

/// Compute the degraded-reasons list from already-built capability substructs.
///
/// Pure function so each `if !flag` branch can be exercised on both sides
/// without instantiating the full handler. Order is part of the contract —
/// the JSON snapshot test depends on it.
fn compute_degraded_reasons(
    filtered_ann: &FilteredAnnHealth,
    writes: &WriteContractHealth,
    freshness: &FreshnessHealth,
    object_economy: &ObjectEconomyHealth,
    recall_probe: &RecallProbeHealth,
) -> Vec<DegradedReason> {
    let mut reasons = Vec::new();
    if !filtered_ann.record_aware_predicates {
        reasons.push(DegradedReason::FilteredAnnRecordPredicateBridgePartial);
    }
    if !filtered_ann.post_filter_shortfall_disclosure {
        reasons.push(DegradedReason::PostFilterShortfallNotDisclosed);
    }
    if !object_economy.live_status_in_app_state {
        reasons.push(DegradedReason::ObjectEconomyLiveStatusNotReachable);
    }
    if object_economy.eligible
        && object_economy.live_status_in_app_state
        && object_economy.live_status != "loaded"
    {
        reasons.push(DegradedReason::ObjectEconomyDirectoryDegraded);
    }
    if !recall_probe.wired_to_query_path {
        reasons.push(DegradedReason::RecallProbeNotWired);
    }
    if !freshness.collection_level_modes_wired {
        reasons.push(DegradedReason::FreshnessModesNotCollectionLevel);
    }
    if !writes.conditional_write {
        reasons.push(DegradedReason::ConditionalWritesUnsupported);
    }
    if !writes.filter_write {
        reasons.push(DegradedReason::FilterWritesUnsupported);
    }
    reasons
}

/// Build the route-health response from the resolved collection facts.
///
/// Kept pure so it can be unit-tested without spinning up `AppState`.
/// The `engine` and `distance_metric` strings are the same labels the
/// existing `get_collection_v2` handler returns, so contracts stay aligned.
fn build_route_health(
    collection_id: String,
    engine: String,
    dimension: u32,
    distance_metric: String,
    record_count: u64,
    storage_size_bytes: u64,
    index_size_bytes: u64,
) -> CollectionRouteHealthV2 {
    build_route_health_with_live_state(
        collection_id,
        engine,
        dimension,
        distance_metric,
        record_count,
        storage_size_bytes,
        index_size_bytes,
        None,
        RecallProbeLiveState::Unwired,
    )
}

/// Per-scope recall-probe state resolved at handler time. `Unwired` means
/// the gate isn't reachable from `AppState` for this deployment, so the
/// route-health response reports `live_state_in_app_state: false` and
/// `gate_open: None`. `Wired { gate_open }` flips `live_state_in_app_state`
/// to true and exposes the actual gate state for the requested scope.
#[derive(Debug, Clone, Copy)]
enum RecallProbeLiveState {
    Unwired,
    Wired { gate_open: bool },
}

fn build_route_health_with_live_state(
    collection_id: String,
    engine: String,
    dimension: u32,
    distance_metric: String,
    record_count: u64,
    storage_size_bytes: u64,
    index_size_bytes: u64,
    cached_object_economy_status: Option<&'static str>,
    recall_probe_state: RecallProbeLiveState,
) -> CollectionRouteHealthV2 {
    let filtered_ann = FilteredAnnHealth {
        id_predicate_supported: true,
        // AxisManager::query_hnsw_with_predicate builds a metadata map from
        // collection_vectors and evaluates metadata predicates during HNSW
        // traversal, then reapplies the same expression as a residual guard.
        // The older standalone AxisMetadataLookup helper is still a placeholder
        // and must not be used as the source of truth for this route.
        record_aware_predicates: true,
        predicate_pushdown_infrastructure_present: true,
        predicate_pushdown_default_wired: false,
        // Genuinely wired: predicate_diagnostics::scope + take_shortfall
        // are called by REST records.rs and gRPC record_service.rs, the
        // captured PredicateShortfall is set on SearchPlanTrace via
        // mark_predicate_shortfall, and axis_predicate_shortfall_total
        // counts every event. Verified 2026-05-28.
        post_filter_shortfall_disclosure: true,
        td_064_status: "record_predicate_and_shortfall_wired",
    };
    let writes = WriteContractHealth {
        insert: true,
        upsert: true,
        update: true,
        delete: true,
        conditional_write: false,
        filter_write: false,
        patch: false,
    };
    let freshness = FreshnessHealth {
        scope: "projection_only",
        collection_level_modes_wired: false,
        notes: "Collection-level strong/bounded-stale modes are not wired; \
                projection-level ProjectionFreshness lives in the storage layer only.",
    };
    let object_economy_eligible = engine == "sst";
    let object_economy_live_status = if object_economy_eligible {
        cached_object_economy_status.unwrap_or("not_checked")
    } else {
        "not_applicable"
    };
    let object_economy_live_status_in_app_state =
        object_economy_eligible && cached_object_economy_status.is_some();
    let object_economy = ObjectEconomyHealth {
        eligible: object_economy_eligible,
        directory_format_present: object_economy_eligible,
        live_status_in_app_state: object_economy_live_status_in_app_state,
        live_status: object_economy_live_status,
        route_hint: object_economy_eligible.then_some("object_economy"),
        notes: if object_economy_eligible && cached_object_economy_status.is_some() {
            "VectorObjectEconomyDirectory status was read from the in-process \
             cache without object-storage I/O."
        } else if object_economy_eligible {
            "VectorObjectEconomyDirectory exists for SST; no cached live status \
             is currently present in AppState."
        } else {
            "Vector object-economy directory is currently SST-specific."
        },
    };
    let (recall_probe_in_app_state, recall_probe_gate_open) = match recall_probe_state {
        RecallProbeLiveState::Unwired => (false, None),
        RecallProbeLiveState::Wired { gate_open } => (true, Some(gate_open)),
    };
    let recall_probe = RecallProbeHealth {
        implementation_present: true,
        // Search code does not yet consult `RecallProbeGate::is_open` when
        // choosing the quantized vs full-precision route — that wiring is
        // its own follow-up. This route only proves the gate is reachable.
        wired_to_query_path: false,
        live_state_in_app_state: recall_probe_in_app_state,
        gate_open: recall_probe_gate_open,
        notes: if recall_probe_in_app_state {
            "RecallProbeGate is reachable from AppState; gate_open reflects \
             the most recent probe outcome for this (tenant, collection) \
             scope. Search-path consultation is a separate follow-up."
        } else {
            "RecallProbeGate state machine exists in catalog/recall_probe.rs \
             but is not wired into AppState for this deployment."
        },
    };

    let degraded_reasons = compute_degraded_reasons(
        &filtered_ann,
        &writes,
        &freshness,
        &object_economy,
        &recall_probe,
    );

    CollectionRouteHealthV2 {
        schema_version: "v1",
        stability: "experimental",
        collection_id,
        engine,
        dimension,
        distance_metric,
        record_count,
        storage_size_bytes,
        index_size_bytes,
        filtered_ann,
        writes,
        freshness,
        object_economy,
        recall_probe,
        degraded_reasons,
    }
}

fn object_economy_status_label(
    status: &crate::storage::engines::sst::object_economy_directory::DirectoryLoadStatus,
) -> &'static str {
    use crate::storage::engines::sst::object_economy_directory::DirectoryLoadStatus;

    match status {
        DirectoryLoadStatus::Loaded => "loaded",
        DirectoryLoadStatus::Missing => "missing",
        DirectoryLoadStatus::Corrupt(_) => "corrupt",
        DirectoryLoadStatus::Mismatch { .. } => "mismatch",
    }
}

/// GET /api/v2/_diagnostics/collections/{collection_id}/route-health
///
/// Experimental: returns a machine-readable capability contract describing
/// what the collection's route can guarantee today. The endpoint does not
/// perform any search/write side effects; it resolves collection facts via
/// the same `CollectionGet` path as `get_collection_v2` and otherwise
/// reports static, code-verified capability state.
///
/// ## Errors
///
/// - `404 Not Found`: Collection does not exist
/// - `500 Internal Server Error`: Lookup failed
pub async fn get_collection_route_health_v2(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
    State(state): State<AppState>,
) -> ApiResult<Json<CollectionRouteHealthV2>> {
    debug!(
        "V2 API: route-health for collection '{}' (experimental)",
        collection_id
    );

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

    let resp = state
        .request_handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::CollectionNotFound(collection_id.clone())
            } else {
                ApiError::Internal(format!("Failed to get collection: {}", e))
            }
        })?;

    let collection = resp.collection.unwrap_or_default();
    let config = collection.config.unwrap_or_default();
    let stats = collection.stats.unwrap_or_default();

    let engine_str = collection_storage_engine_label(config.storage_engine).to_string();
    let distance_metric_str = collection_distance_metric_label(config.distance_metric).to_string();
    let cached_object_economy_status = if engine_str == "sst" {
        state
            .request_handlers
            .vector_operations_service
            .cached_object_economy_directory_status(&collection_id)
            .as_ref()
            .map(object_economy_status_label)
    } else {
        None
    };

    let recall_probe_state = match &state.recall_probe_gate {
        Some(gate) => {
            let scope =
                crate::catalog::ProbeScope::new(tenant.tenant_id.clone(), collection_id.clone());
            RecallProbeLiveState::Wired {
                gate_open: gate.is_open(&scope).await,
            }
        }
        None => RecallProbeLiveState::Unwired,
    };

    Ok(Json(build_route_health_with_live_state(
        collection_id,
        engine_str,
        config.dimension,
        distance_metric_str,
        non_negative_stat(stats.vector_count),
        non_negative_stat(stats.data_size_bytes),
        non_negative_stat(stats.index_size_bytes),
        cached_object_economy_status,
        recall_probe_state,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_request_deserialization() {
        let json = r#"{
            "name": "products",
            "dimension": 768,
            "engine": "sst",
            "schema": {
                "columns": [
                    {"name": "category", "data_type": "text", "indexed": true},
                    {"name": "price", "data_type": "float", "filterable": true}
                ],
                "enforcement": "hybrid",
                "allow_additional_fields": true
            },
            "enable_proxima_record": true
        }"#;

        let request: CreateCollectionV2Request = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "products");
        assert_eq!(request.dimension, 768);
        assert_eq!(request.engine, Some("sst".to_string()));
        assert!(request.enable_proxima_record.unwrap());

        let schema = request.schema.unwrap();
        assert_eq!(schema.columns.len(), 2);
        assert_eq!(schema.enforcement, Some("hybrid".to_string()));
    }

    #[test]
    fn test_schema_definition_serialization() {
        let schema = SchemaDefinition {
            columns: vec![RestColumnDefinition {
                name: "title".to_string(),
                data_type: "text".to_string(),
                nullable: Some(false),
                indexed: Some(true),
                filterable: Some(true),
                max_length: Some(255),
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: Some("strict".to_string()),
            allow_additional_fields: Some(false),
        };

        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains("\"title\""));
        assert!(json.contains("\"text\""));
        assert!(json.contains("\"strict\""));
    }

    #[test]
    fn test_decimal_column_validation() {
        let json = r#"{
            "name": "price",
            "data_type": "decimal",
            "precision": 10,
            "scale": 2
        }"#;

        let column: RestColumnDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(column.precision, Some(10));
        assert_eq!(column.scale, Some(2));
    }

    #[test]
    fn test_v2_storage_engine_mapping_uses_proto_enum_values() {
        assert_eq!(
            crate::core::conversions::parse_storage_engine("sst").unwrap() as i32,
            crate::proto::proximadb_v1::StorageEngine::Sst as i32
        );
        assert_eq!(
            crate::core::conversions::parse_storage_engine("tst").unwrap() as i32,
            crate::proto::proximadb_v1::StorageEngine::Tst as i32
        );
        assert_eq!(
            collection_storage_engine_label(Some(
                crate::proto::proximadb_v1::StorageEngine::Viper as i32
            )),
            "viper"
        );
        assert_eq!(
            collection_storage_engine_label(Some(
                crate::proto::proximadb_v1::StorageEngine::Sst as i32
            )),
            "sst"
        );
        assert_eq!(collection_storage_engine_label(None), "auto");
    }

    #[test]
    fn test_v2_distance_metric_mapping_uses_proto_enum_values() {
        assert_eq!(
            crate::core::conversions::parse_distance_metric("dot_product").unwrap() as i32,
            crate::proto::proximadb_v1::DistanceMetric::DotProduct as i32
        );
        assert_eq!(
            collection_distance_metric_label(Some(
                crate::proto::proximadb_v1::DistanceMetric::Cosine as i32
            )),
            "cosine"
        );
        assert_eq!(
            collection_distance_metric_label(Some(
                crate::proto::proximadb_v1::DistanceMetric::Euclidean as i32
            )),
            "euclidean"
        );
        assert_eq!(collection_distance_metric_label(None), "cosine");
    }

    #[test]
    fn test_v2_stats_do_not_wrap_negative_proto_values() {
        assert_eq!(non_negative_stat(12), 12);
        assert_eq!(non_negative_stat(-1), 0);
    }

    // ------------------------------------------------------------------
    // Route-health builder tests. These lock the contract shape so future
    // capability flips are deliberate edits to both the code and the test,
    // not silent drifts.
    // ------------------------------------------------------------------

    #[test]
    fn route_health_builder_reports_resolved_collection_facts() {
        let h = build_route_health(
            "products".to_string(),
            "sst".to_string(),
            768,
            "cosine".to_string(),
            42,
            4096,
            1024,
        );
        assert_eq!(h.collection_id, "products");
        assert_eq!(h.engine, "sst");
        assert_eq!(h.dimension, 768);
        assert_eq!(h.distance_metric, "cosine");
        assert_eq!(h.record_count, 42);
        assert_eq!(h.storage_size_bytes, 4096);
        assert_eq!(h.index_size_bytes, 1024);
        assert_eq!(h.schema_version, "v1");
        assert_eq!(h.stability, "experimental");
    }

    #[test]
    fn route_health_filtered_ann_reports_td_064_state() {
        let h = build_route_health(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
        );
        assert!(h.filtered_ann.id_predicate_supported);
        assert!(
            h.filtered_ann.record_aware_predicates,
            "AxisManager query path evaluates ProximaRecord metadata during predicate traversal"
        );
        // Shortfall path is genuinely wired: predicate_diagnostics::scope in
        // REST records.rs + gRPC record_service.rs, captured shortfall set
        // via mark_predicate_shortfall on SearchPlanTrace, metrics counter
        // axis_predicate_shortfall_total fires on every event.
        assert!(
            h.filtered_ann.post_filter_shortfall_disclosure,
            "TD-064 shortfall path is wired (diagnostics + trace + metrics)"
        );
        assert_eq!(
            h.filtered_ann.td_064_status,
            "record_predicate_and_shortfall_wired"
        );
    }

    #[test]
    fn route_health_writes_match_current_v2_surface() {
        let h = build_route_health(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
        );
        assert!(h.writes.insert);
        assert!(h.writes.upsert);
        assert!(h.writes.update);
        assert!(h.writes.delete);
        // Unwired today — flip these only when REST/gRPC actually accepts them.
        assert!(!h.writes.conditional_write);
        assert!(!h.writes.filter_write);
        assert!(!h.writes.patch);
    }

    #[test]
    fn route_health_object_economy_is_explicitly_sst_only() {
        let sst = build_route_health(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
        );
        assert!(sst.object_economy.eligible);
        assert!(sst.object_economy.directory_format_present);
        assert_eq!(sst.object_economy.live_status, "not_checked");
        assert_eq!(sst.object_economy.route_hint, Some("object_economy"));

        let viper = build_route_health(
            "c".to_string(),
            "viper".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
        );
        assert!(!viper.object_economy.eligible);
        assert!(!viper.object_economy.directory_format_present);
        assert_eq!(viper.object_economy.live_status, "not_applicable");
        assert_eq!(viper.object_economy.route_hint, None);
    }

    #[test]
    fn route_health_degraded_reasons_serialize_as_screaming_snake() {
        let reasons = vec![
            DegradedReason::FilteredAnnRecordPredicateBridgePartial,
            DegradedReason::ObjectEconomyDirectoryDegraded,
            DegradedReason::RecallProbeNotWired,
            DegradedReason::ConditionalWritesUnsupported,
        ];
        let s = serde_json::to_string(&reasons).unwrap();
        assert!(s.contains("FILTERED_ANN_RECORD_PREDICATE_BRIDGE_PARTIAL"));
        assert!(s.contains("OBJECT_ECONOMY_DIRECTORY_DEGRADED"));
        assert!(s.contains("RECALL_PROBE_NOT_WIRED"));
        assert!(s.contains("CONDITIONAL_WRITES_UNSUPPORTED"));
    }

    #[test]
    fn route_health_v1_degraded_reasons_are_the_expected_five() {
        // Snapshot of the v1 reasons set. Adding/removing a reason without
        // updating this assertion would silently change the contract.
        let h = build_route_health(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
        );
        let expected = vec![
            DegradedReason::ObjectEconomyLiveStatusNotReachable,
            DegradedReason::RecallProbeNotWired,
            DegradedReason::FreshnessModesNotCollectionLevel,
            DegradedReason::ConditionalWritesUnsupported,
            DegradedReason::FilterWritesUnsupported,
        ];
        assert_eq!(h.degraded_reasons, expected);
    }

    #[test]
    fn route_health_json_shape_snapshot() {
        // Snapshot of the top-level JSON keys. Locks the contract so a
        // field rename surfaces as a test diff. Field *values* are
        // covered by the other tests; this one is shape-only.
        let h = build_route_health(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
        );
        let v: serde_json::Value = serde_json::to_value(&h).unwrap();
        let obj = v.as_object().expect("response is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "collection_id",
                "degraded_reasons",
                "dimension",
                "distance_metric",
                "engine",
                "filtered_ann",
                "freshness",
                "index_size_bytes",
                "object_economy",
                "recall_probe",
                "record_count",
                "schema_version",
                "stability",
                "storage_size_bytes",
                "writes",
            ]
        );
    }

    #[test]
    fn route_health_object_economy_reports_cached_loaded_status() {
        let h = build_route_health_with_live_state(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
            Some("loaded"),
            RecallProbeLiveState::Unwired,
        );

        assert!(h.object_economy.live_status_in_app_state);
        assert_eq!(h.object_economy.live_status, "loaded");
        assert!(
            !h.degraded_reasons
                .contains(&DegradedReason::ObjectEconomyLiveStatusNotReachable)
        );
        assert!(
            !h.degraded_reasons
                .contains(&DegradedReason::ObjectEconomyDirectoryDegraded)
        );
    }

    #[test]
    fn route_health_object_economy_reports_cached_degraded_status() {
        let h = build_route_health_with_live_state(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
            Some("missing"),
            RecallProbeLiveState::Unwired,
        );

        assert!(h.object_economy.live_status_in_app_state);
        assert_eq!(h.object_economy.live_status, "missing");
        assert!(
            !h.degraded_reasons
                .contains(&DegradedReason::ObjectEconomyLiveStatusNotReachable)
        );
        assert!(
            h.degraded_reasons
                .contains(&DegradedReason::ObjectEconomyDirectoryDegraded)
        );
    }

    // ------------------------------------------------------------------
    // RecallProbeLiveState branch coverage — exercises both the Unwired
    // path (default, no AppState slot) and the Wired path (gate reachable
    // from AppState, per-scope `gate_open` resolved). `wired_to_query_path`
    // intentionally stays `false` in both branches because no search code
    // consults the gate yet — that's a separate follow-up, and flipping
    // wired_to_query_path here would silently overclaim.
    // ------------------------------------------------------------------

    #[test]
    fn route_health_recall_probe_unwired_reports_none_gate_open() {
        let h = build_route_health(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
        );
        assert!(!h.recall_probe.live_state_in_app_state);
        assert_eq!(h.recall_probe.gate_open, None);
        // wired_to_query_path stays false until search code consults the gate.
        assert!(!h.recall_probe.wired_to_query_path);
        assert!(
            h.degraded_reasons
                .contains(&DegradedReason::RecallProbeNotWired),
            "RecallProbeNotWired must remain in the v1 set until the search \
             path consults the gate, regardless of AppState wiring"
        );
    }

    #[test]
    fn route_health_recall_probe_wired_reports_gate_open_true() {
        let h = build_route_health_with_live_state(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
            None,
            RecallProbeLiveState::Wired { gate_open: true },
        );
        assert!(h.recall_probe.live_state_in_app_state);
        assert_eq!(h.recall_probe.gate_open, Some(true));
        // Critical: AppState wiring alone does NOT flip wired_to_query_path.
        // Search code must call gate.is_open before that flips.
        assert!(!h.recall_probe.wired_to_query_path);
        assert!(
            h.degraded_reasons
                .contains(&DegradedReason::RecallProbeNotWired),
            "AppState reachability is necessary but not sufficient for the \
             gate to be considered fully wired"
        );
    }

    #[test]
    fn route_health_recall_probe_wired_reports_gate_open_false() {
        let h = build_route_health_with_live_state(
            "c".to_string(),
            "sst".to_string(),
            8,
            "cosine".to_string(),
            0,
            0,
            0,
            None,
            RecallProbeLiveState::Wired { gate_open: false },
        );
        assert!(h.recall_probe.live_state_in_app_state);
        assert_eq!(h.recall_probe.gate_open, Some(false));
    }

    // ------------------------------------------------------------------
    // compute_degraded_reasons branch coverage — exercises BOTH sides of
    // every `if !flag` so coverage isn't skewed by the v1 hardcoded
    // constants. These tests treat the function as the source of truth
    // for the flag → reason mapping; build_route_health is then a thin
    // assembler whose v1 set is checked by
    // `route_health_v1_degraded_reasons_are_the_expected_five`.
    // ------------------------------------------------------------------

    fn all_wired_filtered_ann() -> FilteredAnnHealth {
        FilteredAnnHealth {
            id_predicate_supported: true,
            record_aware_predicates: true,
            predicate_pushdown_infrastructure_present: true,
            predicate_pushdown_default_wired: true,
            post_filter_shortfall_disclosure: true,
            td_064_status: "resolved",
        }
    }
    fn all_wired_writes() -> WriteContractHealth {
        WriteContractHealth {
            insert: true,
            upsert: true,
            update: true,
            delete: true,
            conditional_write: true,
            filter_write: true,
            patch: true,
        }
    }
    fn all_wired_freshness() -> FreshnessHealth {
        FreshnessHealth {
            scope: "collection_level",
            collection_level_modes_wired: true,
            notes: "",
        }
    }
    fn all_wired_object_economy() -> ObjectEconomyHealth {
        ObjectEconomyHealth {
            eligible: true,
            directory_format_present: true,
            live_status_in_app_state: true,
            live_status: "loaded",
            route_hint: Some("object_economy"),
            notes: "",
        }
    }
    fn all_wired_recall_probe() -> RecallProbeHealth {
        RecallProbeHealth {
            implementation_present: true,
            wired_to_query_path: true,
            live_state_in_app_state: true,
            gate_open: Some(true),
            notes: "",
        }
    }

    #[test]
    fn compute_degraded_reasons_returns_empty_when_everything_wired() {
        let reasons = compute_degraded_reasons(
            &all_wired_filtered_ann(),
            &all_wired_writes(),
            &all_wired_freshness(),
            &all_wired_object_economy(),
            &all_wired_recall_probe(),
        );
        assert!(
            reasons.is_empty(),
            "fully-wired capability set must produce no degraded reasons; got {reasons:?}"
        );
    }

    #[test]
    fn compute_degraded_reasons_flips_record_aware_predicate_reason() {
        let mut fa = all_wired_filtered_ann();
        fa.record_aware_predicates = false;
        let reasons = compute_degraded_reasons(
            &fa,
            &all_wired_writes(),
            &all_wired_freshness(),
            &all_wired_object_economy(),
            &all_wired_recall_probe(),
        );
        assert_eq!(
            reasons,
            vec![DegradedReason::FilteredAnnRecordPredicateBridgePartial]
        );
    }

    #[test]
    fn compute_degraded_reasons_flips_post_filter_shortfall_reason() {
        let mut fa = all_wired_filtered_ann();
        fa.post_filter_shortfall_disclosure = false;
        let reasons = compute_degraded_reasons(
            &fa,
            &all_wired_writes(),
            &all_wired_freshness(),
            &all_wired_object_economy(),
            &all_wired_recall_probe(),
        );
        assert_eq!(
            reasons,
            vec![DegradedReason::PostFilterShortfallNotDisclosed]
        );
    }

    #[test]
    fn compute_degraded_reasons_flips_object_economy_reason() {
        let mut oe = all_wired_object_economy();
        oe.live_status_in_app_state = false;
        let reasons = compute_degraded_reasons(
            &all_wired_filtered_ann(),
            &all_wired_writes(),
            &all_wired_freshness(),
            &oe,
            &all_wired_recall_probe(),
        );
        assert_eq!(
            reasons,
            vec![DegradedReason::ObjectEconomyLiveStatusNotReachable]
        );
    }

    #[test]
    fn compute_degraded_reasons_flips_object_economy_directory_degraded_reason() {
        let mut oe = all_wired_object_economy();
        oe.live_status = "missing";
        let reasons = compute_degraded_reasons(
            &all_wired_filtered_ann(),
            &all_wired_writes(),
            &all_wired_freshness(),
            &oe,
            &all_wired_recall_probe(),
        );
        assert_eq!(
            reasons,
            vec![DegradedReason::ObjectEconomyDirectoryDegraded]
        );
    }

    #[test]
    fn compute_degraded_reasons_flips_recall_probe_reason() {
        let mut rp = all_wired_recall_probe();
        rp.wired_to_query_path = false;
        let reasons = compute_degraded_reasons(
            &all_wired_filtered_ann(),
            &all_wired_writes(),
            &all_wired_freshness(),
            &all_wired_object_economy(),
            &rp,
        );
        assert_eq!(reasons, vec![DegradedReason::RecallProbeNotWired]);
    }

    #[test]
    fn compute_degraded_reasons_flips_freshness_reason() {
        let mut f = all_wired_freshness();
        f.collection_level_modes_wired = false;
        let reasons = compute_degraded_reasons(
            &all_wired_filtered_ann(),
            &all_wired_writes(),
            &f,
            &all_wired_object_economy(),
            &all_wired_recall_probe(),
        );
        assert_eq!(
            reasons,
            vec![DegradedReason::FreshnessModesNotCollectionLevel]
        );
    }

    #[test]
    fn compute_degraded_reasons_flips_conditional_write_reason() {
        let mut w = all_wired_writes();
        w.conditional_write = false;
        let reasons = compute_degraded_reasons(
            &all_wired_filtered_ann(),
            &w,
            &all_wired_freshness(),
            &all_wired_object_economy(),
            &all_wired_recall_probe(),
        );
        assert_eq!(reasons, vec![DegradedReason::ConditionalWritesUnsupported]);
    }

    #[test]
    fn compute_degraded_reasons_flips_filter_write_reason() {
        let mut w = all_wired_writes();
        w.filter_write = false;
        let reasons = compute_degraded_reasons(
            &all_wired_filtered_ann(),
            &w,
            &all_wired_freshness(),
            &all_wired_object_economy(),
            &all_wired_recall_probe(),
        );
        assert_eq!(reasons, vec![DegradedReason::FilterWritesUnsupported]);
    }

    #[test]
    fn compute_degraded_reasons_preserves_order_when_all_unwired() {
        // Order must match the v1 set exactly. Reordering the if-chain
        // in compute_degraded_reasons would silently reshuffle JSON
        // output for clients that iterate the array.
        let fa = FilteredAnnHealth {
            id_predicate_supported: false,
            record_aware_predicates: false,
            predicate_pushdown_infrastructure_present: false,
            predicate_pushdown_default_wired: false,
            post_filter_shortfall_disclosure: false,
            td_064_status: "open",
        };
        let writes = WriteContractHealth {
            insert: false,
            upsert: false,
            update: false,
            delete: false,
            conditional_write: false,
            filter_write: false,
            patch: false,
        };
        let freshness = FreshnessHealth {
            scope: "",
            collection_level_modes_wired: false,
            notes: "",
        };
        let oe = ObjectEconomyHealth {
            eligible: false,
            directory_format_present: false,
            live_status_in_app_state: false,
            live_status: "not_applicable",
            route_hint: None,
            notes: "",
        };
        let rp = RecallProbeHealth {
            implementation_present: false,
            wired_to_query_path: false,
            live_state_in_app_state: false,
            gate_open: None,
            notes: "",
        };
        let reasons = compute_degraded_reasons(&fa, &writes, &freshness, &oe, &rp);
        assert_eq!(
            reasons,
            vec![
                DegradedReason::FilteredAnnRecordPredicateBridgePartial,
                DegradedReason::PostFilterShortfallNotDisclosed,
                DegradedReason::ObjectEconomyLiveStatusNotReachable,
                DegradedReason::RecallProbeNotWired,
                DegradedReason::FreshnessModesNotCollectionLevel,
                DegradedReason::ConditionalWritesUnsupported,
                DegradedReason::FilterWritesUnsupported,
            ]
        );
    }

    #[test]
    fn each_degraded_reason_variant_serializes_to_unique_screaming_snake() {
        // Per-variant rename check. If a variant is added without a
        // serde rename matching SCREAMING_SNAKE_CASE convention, this
        // fails before the contract leaks to clients.
        let cases = [
            (
                DegradedReason::FilteredAnnRecordPredicateBridgePartial,
                "\"FILTERED_ANN_RECORD_PREDICATE_BRIDGE_PARTIAL\"",
            ),
            (
                DegradedReason::PostFilterShortfallNotDisclosed,
                "\"POST_FILTER_SHORTFALL_NOT_DISCLOSED\"",
            ),
            (
                DegradedReason::ObjectEconomyLiveStatusNotReachable,
                "\"OBJECT_ECONOMY_LIVE_STATUS_NOT_REACHABLE\"",
            ),
            (
                DegradedReason::ObjectEconomyDirectoryDegraded,
                "\"OBJECT_ECONOMY_DIRECTORY_DEGRADED\"",
            ),
            (
                DegradedReason::RecallProbeNotWired,
                "\"RECALL_PROBE_NOT_WIRED\"",
            ),
            (
                DegradedReason::FreshnessModesNotCollectionLevel,
                "\"FRESHNESS_MODES_NOT_COLLECTION_LEVEL\"",
            ),
            (
                DegradedReason::ConditionalWritesUnsupported,
                "\"CONDITIONAL_WRITES_UNSUPPORTED\"",
            ),
            (
                DegradedReason::FilterWritesUnsupported,
                "\"FILTER_WRITES_UNSUPPORTED\"",
            ),
        ];
        for (variant, expected) in cases {
            assert_eq!(
                serde_json::to_string(&variant).unwrap(),
                expected,
                "variant {variant:?} did not serialize to {expected}"
            );
        }
    }

    #[test]
    fn build_route_health_passes_through_engine_and_distance_labels() {
        // Engine/distance strings flow through unchanged — no canonicalization,
        // no casing change. This pins the contract with `get_collection_v2`
        // which uses the same enum→string mapping.
        for (engine, distance) in [
            ("sst", "cosine"),
            ("helix", "euclidean"),
            ("viper", "dot_product"),
            ("auto", "cosine"),
        ] {
            let h = build_route_health(
                "k".to_string(),
                engine.to_string(),
                384,
                distance.to_string(),
                7,
                11,
                13,
            );
            assert_eq!(h.engine, engine);
            assert_eq!(h.distance_metric, distance);
            assert_eq!(h.dimension, 384);
            assert_eq!(h.record_count, 7);
            assert_eq!(h.storage_size_bytes, 11);
            assert_eq!(h.index_size_bytes, 13);
        }
    }
}
