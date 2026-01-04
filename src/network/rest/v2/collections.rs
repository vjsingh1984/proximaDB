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
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::errors::{ApiError, ApiResult};
use crate::network::rest::v1::handlers::AppState;
use crate::proto::proximadb_v1::{CollectionConfig, CollectionOperation, CollectionRequest};

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
    /// Options: "auto", "sst", "helix", "viper", "swift", "nova", "raptor"
    /// Default: "auto" (system selects optimal engine)
    pub engine: Option<String>,
    /// Schema definition with column types
    pub schema: Option<SchemaDefinition>,
    /// Enable ProximaRecord support for this collection
    ///
    /// When enabled:
    /// - Records can use typed_fields and text_fields
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
}

/// Schema definition for a collection
///
/// Defines the typed columns and enforcement rules for ProximaRecord support.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SchemaDefinition {
    /// Column definitions
    pub columns: Vec<ColumnDefinition>,
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

/// Column definition for schema
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ColumnDefinition {
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
    let valid_engines = ["auto", "sst", "helix", "viper", "swift", "nova", "raptor"];
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
    let storage_engine_value = match engine {
        "sst" => 1,     // StorageEngine::Sst
        "helix" => 2,   // StorageEngine::Helix
        "viper" => 3,   // StorageEngine::Viper
        "swift" => 4,   // StorageEngine::Swift
        "nova" => 5,    // StorageEngine::Nova
        "raptor" => 6,  // StorageEngine::Raptor
        _ => 0,         // StorageEngine::Auto
    };

    // Map distance metric name to DistanceMetric enum value
    let distance_metric_value = match request.distance_metric.as_deref() {
        Some("euclidean") => Some(1),  // DistanceMetric::Euclidean
        Some("dot_product") => Some(2), // DistanceMetric::DotProduct
        _ => Some(0),                   // DistanceMetric::Cosine (default)
    };

    let collection_config = CollectionConfig {
        name: request.name.clone(),
        dimension: request.dimension,
        storage_engine: Some(storage_engine_value),
        distance_metric: distance_metric_value,
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
        .unified_handlers
        .handle_collection_operation(collection_request)
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
        .unified_handlers
        .handle_collection_operation(request)
        .await
    {
        Ok(resp) => {
            // Extract collection info from response
            let collection = resp.collection.unwrap_or_default();
            let config = collection.config.unwrap_or_default();
            let stats = collection.stats.unwrap_or_default();

            // Map storage engine enum to string
            let engine_str = match config.storage_engine.unwrap_or(0) {
                1 => "sst",
                2 => "helix",
                3 => "viper",
                4 => "swift",
                5 => "nova",
                6 => "raptor",
                _ => "auto",
            };

            // Map distance metric enum to string
            let distance_metric_str = match config.distance_metric.unwrap_or(0) {
                1 => "euclidean",
                2 => "dot_product",
                _ => "cosine",
            };

            let response = CollectionV2Response {
                collection_id: collection_id.clone(),
                name: collection_id,
                dimension: config.dimension,
                engine: engine_str.to_string(),
                distance_metric: distance_metric_str.to_string(),
                proxima_record_enabled: false, // Would be stored in metadata
                schema: None,                  // Would be loaded from metadata
                stats: CollectionStatsV2 {
                    record_count: stats.vector_count as u64,
                    storage_size_bytes: stats.data_size_bytes as u64,
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
        .unified_handlers
        .handle_collection_operation(request)
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
                        dimension: cfg.map(|cfg| cfg.dimension).unwrap_or(0),
                        engine: engine_str.to_string(),
                        proxima_record_enabled: false,
                        record_count: if include_stats {
                            Some(c.stats.as_ref().map(|s| s.vector_count as u64).unwrap_or(0))
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
            columns: vec![ColumnDefinition {
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

        let column: ColumnDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(column.precision, Some(10));
        assert_eq!(column.scale, Some(2));
    }
}
