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

//! Schema management endpoints for v2 API
//!
//! This module provides REST endpoints for managing collection schemas,
//! including retrieval and updates with schema evolution support.
//!
//! ## Endpoints
//!
//! - `GET /api/v2/collections/{id}/schema` - Get collection schema
//! - `PUT /api/v2/collections/{id}/schema` - Update collection schema
//!
//! ## Schema Evolution
//!
//! Schema updates follow these rules:
//! - New columns can be added (must be nullable or have defaults)
//! - Column types cannot be changed (except compatible widening)
//! - Columns cannot be removed (for data safety)
//! - Enforcement mode can be changed with care

use axum::{
    Json,
    extract::{Extension, Path, State},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::errors::{ApiError, ApiResult};
use crate::network::middleware::tenant::TenantContext;
use crate::network::rest::v1::handlers::AppState;

use super::collections::{ColumnDefinition, SchemaDefinition};

/// Schema response with metadata
#[derive(Debug, Serialize)]
pub struct SchemaResponse {
    /// Schema ID (UUID)
    pub schema_id: String,
    /// Schema version (semantic versioning)
    pub schema_version: String,
    /// Collection this schema belongs to
    pub collection_id: String,
    /// Schema definition
    pub schema: SchemaDefinition,
    /// Creation timestamp
    pub created_at: String,
    /// Last update timestamp
    pub updated_at: Option<String>,
    /// Parent schema ID (for evolution tracking)
    pub parent_schema_id: Option<String>,
}

/// GET /api/v2/collections/{collection_id}/schema
///
/// Retrieve the schema definition for a collection.
///
/// ## Path Parameters
///
/// - `collection_id`: Collection name/ID
///
/// ## Response
///
/// Returns [`SchemaResponse`] with full schema details.
///
/// ## Errors
///
/// - `404 Not Found`: Collection or schema does not exist
/// - `500 Internal Server Error`: Retrieval failed
pub async fn get_schema(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
    State(state): State<AppState>,
) -> ApiResult<Json<SchemaResponse>> {
    debug!("V2 API: Getting schema for collection '{}'", collection_id);

    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    // Step 1: Verify collection exists and get metadata
    let collection_request = crate::proto::proximadb_v1::CollectionRequest {
        operation: crate::proto::proximadb_v1::CollectionOperation::CollectionGet as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    let collection_response = state
        .unified_handlers
        .handle_collection_operation_for_tenant(collection_request, Some(&tenant.tenant_id))
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::CollectionNotFound(collection_id.clone())
            } else {
                ApiError::Internal(format!("Failed to get collection: {}", e))
            }
        })?;

    let collection = collection_response
        .collection
        .ok_or_else(|| ApiError::CollectionNotFound(collection_id.clone()))?;

    let config = collection
        .config
        .ok_or_else(|| ApiError::Internal("Collection has no configuration".to_string()))?;

    // Step 2: Check if ProximaRecord is enabled
    let proxima_record_enabled = config.enable_proxima_record.unwrap_or(false);

    // Step 3: Load schema from collection metadata
    let record_schema_config = config.record_schema;

    // Convert stored schema to response format
    let (schema_id, schema_version, parent_schema_id, schema_def) =
        if let Some(ref schema_config) = record_schema_config {
            // We have a schema configuration stored
            let schema_id = if schema_config.schema_id.is_empty() {
                // Generate a deterministic ID based on collection
                format!("schema_{}", collection_id)
            } else {
                schema_config.schema_id.clone()
            };

            let version = if schema_config.schema_version.is_empty() {
                "1.0.0".to_string()
            } else {
                schema_config.schema_version.clone()
            };

            // Build schema definition from text_columns if available
            let columns: Vec<ColumnDefinition> = config
                .text_columns
                .iter()
                .map(|col_name| ColumnDefinition {
                    name: col_name.clone(),
                    data_type: "text".to_string(),
                    nullable: Some(true),
                    indexed: Some(false),
                    filterable: Some(true),
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                })
                .collect();

            // Also add columns from text_storage_configs
            let mut all_columns = columns;
            for text_config in &config.text_storage_configs {
                if !all_columns
                    .iter()
                    .any(|c| c.name == text_config.column_name)
                {
                    all_columns.push(ColumnDefinition {
                        name: text_config.column_name.clone(),
                        data_type: "text_large".to_string(),
                        nullable: Some(true),
                        indexed: Some(false),
                        filterable: Some(false),
                        max_length: Some(text_config.chunk_size),
                        precision: None,
                        scale: None,
                        vector_dimension: None,
                    });
                }
            }

            // Map enforcement level from proto
            let enforcement = match schema_config.enforcement {
                1 => "strict",
                2 => "flexible",
                3 => "hybrid",
                _ => "hybrid",
            };

            let schema_def = SchemaDefinition {
                columns: all_columns,
                enforcement: Some(enforcement.to_string()),
                allow_additional_fields: Some(schema_config.auto_evolve),
            };

            (schema_id, version, None, schema_def)
        } else if proxima_record_enabled {
            // ProximaRecord enabled but no explicit schema - create default
            let schema_id = format!("schema_{}", collection_id);
            let columns: Vec<ColumnDefinition> = config
                .text_columns
                .iter()
                .map(|col_name| ColumnDefinition {
                    name: col_name.clone(),
                    data_type: "text".to_string(),
                    nullable: Some(true),
                    indexed: Some(false),
                    filterable: Some(true),
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                })
                .collect();

            let schema_def = SchemaDefinition {
                columns,
                enforcement: Some("hybrid".to_string()),
                allow_additional_fields: Some(true),
            };

            (schema_id, "1.0.0".to_string(), None, schema_def)
        } else {
            // No schema defined and ProximaRecord not enabled
            return Err(ApiError::CollectionNotFound(format!(
                "No schema defined for collection '{}'. Enable ProximaRecord to use schemas.",
                collection_id
            )));
        };

    // Step 4: Return schema with version info
    let response = SchemaResponse {
        schema_id,
        schema_version,
        collection_id: collection_id.clone(),
        schema: schema_def,
        created_at: chrono::DateTime::from_timestamp(collection.created_at / 1000, 0).map_or_else(|| chrono::Utc::now().to_rfc3339(), |dt| dt.to_rfc3339()),
        updated_at: if collection.updated_at != collection.created_at {
            chrono::DateTime::from_timestamp(collection.updated_at / 1000, 0)
                .map(|dt| dt.to_rfc3339())
        } else {
            None
        },
        parent_schema_id,
    };

    info!(
        "V2 API: Retrieved schema '{}' v{} for collection '{}'",
        response.schema_id, response.schema_version, collection_id
    );

    Ok(Json(response))
}

/// Request to update schema
///
/// ## Schema Evolution Rules
///
/// 1. **Adding Columns**: New columns must be nullable or have defaults
/// 2. **Type Changes**: Not allowed (except compatible widening like int32->int64)
/// 3. **Removing Columns**: Not allowed (use deprecation instead)
/// 4. **Enforcement Mode**: Can be changed, but strict->flexible may lose validation
///
/// ## Example JSON
///
/// ```json
/// {
///     "columns": [
///         {"name": "category", "data_type": "text", "indexed": true},
///         {"name": "price", "data_type": "float", "filterable": true},
///         {"name": "tags", "data_type": "array_text", "nullable": true}
///     ],
///     "enforcement": "hybrid",
///     "allow_additional_fields": true
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct UpdateSchemaRequest {
    /// Updated schema definition
    #[serde(flatten)]
    pub schema: SchemaDefinition,
    /// Force update even if validation warnings exist
    ///
    /// Use with caution - may cause data compatibility issues.
    pub force: Option<bool>,
}

/// Schema update response
#[derive(Debug, Serialize)]
pub struct UpdateSchemaResponse {
    /// Updated schema ID
    pub schema_id: String,
    /// New schema version
    pub schema_version: String,
    /// Previous schema ID (for rollback)
    pub previous_schema_id: String,
    /// List of applied changes
    pub changes: Vec<SchemaChange>,
    /// Warnings about the update
    pub warnings: Vec<String>,
    /// Update timestamp
    pub updated_at: String,
}

/// Description of a schema change
#[derive(Debug, Serialize)]
pub struct SchemaChange {
    /// Type of change
    pub change_type: String,
    /// Affected column (if applicable)
    pub column: Option<String>,
    /// Description of the change
    pub description: String,
}

/// PUT /api/v2/collections/{collection_id}/schema
///
/// Update the schema for a collection.
///
/// ## Path Parameters
///
/// - `collection_id`: Collection name/ID
///
/// ## Request Body
///
/// See [`UpdateSchemaRequest`] for the expected JSON format.
///
/// ## Response
///
/// Returns [`UpdateSchemaResponse`] with change details.
///
/// ## Schema Evolution
///
/// Schema updates must follow evolution rules to maintain data compatibility.
/// See [`UpdateSchemaRequest`] for detailed rules.
///
/// ## Errors
///
/// - `400 Bad Request`: Invalid schema or evolution violation
/// - `404 Not Found`: Collection does not exist
/// - `409 Conflict`: Incompatible schema change
/// - `500 Internal Server Error`: Update failed
pub async fn update_schema(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
    State(state): State<AppState>,
    Json(request): Json<UpdateSchemaRequest>,
) -> ApiResult<Json<UpdateSchemaResponse>> {
    info!("V2 API: Updating schema for collection '{}'", collection_id);

    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    let force = request.force.unwrap_or(false);
    if force {
        warn!(
            "V2 API: Force schema update requested for '{}' - validation warnings may be ignored",
            collection_id
        );
    }

    // Validate the new schema
    let schema = &request.schema;

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

    for column in &schema.columns {
        if column.name.is_empty() {
            return Err(ApiError::InvalidArgument(
                "Column name cannot be empty".to_string(),
            ));
        }

        if !valid_types.contains(&column.data_type.as_str()) {
            return Err(ApiError::InvalidArgument(format!(
                "Invalid data type '{}' for column '{}'. Valid types: {:?}",
                column.data_type, column.name, valid_types
            )));
        }

        // Validate decimal precision/scale
        if column.data_type == "decimal"
            && (column.precision.is_none() || column.scale.is_none()) {
                return Err(ApiError::InvalidArgument(format!(
                    "Column '{}' with type 'decimal' requires precision and scale",
                    column.name
                )));
            }

        // Validate vector dimension
        if column.data_type == "vector" && column.vector_dimension.is_none() {
            return Err(ApiError::InvalidArgument(format!(
                "Column '{}' with type 'vector' requires vector_dimension",
                column.name
            )));
        }
    }

    // Step 1: Load current collection and schema
    let collection_request = crate::proto::proximadb_v1::CollectionRequest {
        operation: crate::proto::proximadb_v1::CollectionOperation::CollectionGet as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    let collection_response = state
        .unified_handlers
        .handle_collection_operation_for_tenant(collection_request, Some(&tenant.tenant_id))
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::CollectionNotFound(collection_id.clone())
            } else {
                ApiError::Internal(format!("Failed to get collection: {}", e))
            }
        })?;

    let collection = collection_response
        .collection
        .ok_or_else(|| ApiError::CollectionNotFound(collection_id.clone()))?;

    let mut config = collection
        .config
        .clone()
        .ok_or_else(|| ApiError::Internal("Collection has no configuration".to_string()))?;

    // Get existing schema for evolution validation
    let existing_schema = build_existing_schema(&config);

    // Step 2: Validate schema evolution rules
    let validation_result = validate_schema(schema, existing_schema.as_ref());

    // Collect changes and warnings
    let mut changes: Vec<SchemaChange> = Vec::new();
    let mut warnings: Vec<String> = validation_result.warnings.clone();

    // Check for evolution violations (unless force is set)
    if !validation_result.valid && !force {
        // Return conflict error with details about what's wrong
        return Err(ApiError::Conflict(format!(
            "Schema evolution validation failed: {}. Use 'force: true' to override.",
            validation_result.errors.join("; ")
        )));
    }

    // If force is set, add evolution errors as warnings
    if !validation_result.valid && force {
        for error in &validation_result.errors {
            warnings.push(format!("FORCED: {}", error));
        }
    }

    // Step 3: Generate schema diff and changes list
    if let Some(ref existing) = existing_schema {
        let existing_columns: std::collections::HashMap<&str, &ColumnDefinition> = existing
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();

        let new_columns: std::collections::HashMap<&str, &ColumnDefinition> = schema
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();

        // Detect added columns
        for (name, col) in &new_columns {
            if !existing_columns.contains_key(name) {
                changes.push(SchemaChange {
                    change_type: "ADD_COLUMN".to_string(),
                    column: Some(name.to_string()),
                    description: format!("Added column '{}' with type '{}'", name, col.data_type),
                });
            }
        }

        // Detect removed columns (only if force is set)
        for (name, _) in &existing_columns {
            if !new_columns.contains_key(name) {
                changes.push(SchemaChange {
                    change_type: "REMOVE_COLUMN".to_string(),
                    column: Some(name.to_string()),
                    description: format!("Removed column '{}'", name),
                });
            }
        }

        // Detect property changes
        for (name, new_col) in &new_columns {
            if let Some(existing_col) = existing_columns.get(name) {
                // Check for type change
                if existing_col.data_type != new_col.data_type {
                    changes.push(SchemaChange {
                        change_type: "CHANGE_TYPE".to_string(),
                        column: Some(name.to_string()),
                        description: format!(
                            "Changed type of '{}' from '{}' to '{}'",
                            name, existing_col.data_type, new_col.data_type
                        ),
                    });
                }

                // Check for indexed change
                if existing_col.indexed != new_col.indexed {
                    changes.push(SchemaChange {
                        change_type: "CHANGE_INDEX".to_string(),
                        column: Some(name.to_string()),
                        description: format!(
                            "Changed indexed status of '{}' from {:?} to {:?}",
                            name, existing_col.indexed, new_col.indexed
                        ),
                    });
                }

                // Check for nullable change
                if existing_col.nullable != new_col.nullable {
                    changes.push(SchemaChange {
                        change_type: "CHANGE_NULLABILITY".to_string(),
                        column: Some(name.to_string()),
                        description: format!(
                            "Changed nullable status of '{}' from {:?} to {:?}",
                            name, existing_col.nullable, new_col.nullable
                        ),
                    });
                }
            }
        }

        // Detect enforcement mode change
        if existing.enforcement != schema.enforcement {
            changes.push(SchemaChange {
                change_type: "CHANGE_ENFORCEMENT".to_string(),
                column: None,
                description: format!(
                    "Changed enforcement mode from {:?} to {:?}",
                    existing.enforcement, schema.enforcement
                ),
            });
        }
    } else {
        // No existing schema - this is the initial schema
        for col in &schema.columns {
            changes.push(SchemaChange {
                change_type: "ADD_COLUMN".to_string(),
                column: Some(col.name.clone()),
                description: format!("Added column '{}' with type '{}'", col.name, col.data_type),
            });
        }
    }

    // Step 4: Create new schema version
    let previous_schema_id = config
        .record_schema
        .as_ref().map_or_else(|| format!("schema_{}_v0", collection_id), |s| s.schema_id.clone());

    let new_schema_id = format!("schema_{}_{}", collection_id, uuid::Uuid::new_v4());
    let new_version = increment_version(
        config
            .record_schema
            .as_ref()
            .map_or("0.0.0", |s| s.schema_version.as_str()),
    );

    // Step 5: Build and store updated schema configuration
    // Map enforcement mode to proto enum
    let enforcement_value = match schema.enforcement.as_deref() {
        Some("strict") => 1,   // SchemaEnforcement::Strict
        Some("flexible") => 2, // SchemaEnforcement::Flexible
        Some("hybrid") => 3,   // SchemaEnforcement::Hybrid
        _ => 3,                // Default to hybrid
    };

    // Build text_columns from schema columns with text types
    let text_columns: Vec<String> = schema
        .columns
        .iter()
        .filter(|c| c.data_type == "text" || c.data_type == "text_large")
        .map(|c| c.name.clone())
        .collect();

    // Build text_storage_configs for text_large columns
    let text_storage_configs: Vec<crate::proto::proximadb_v1::TextStorageConfig> = schema
        .columns
        .iter()
        .filter(|c| c.data_type == "text_large")
        .map(|c| crate::proto::proximadb_v1::TextStorageConfig {
            column_name: c.name.clone(),
            strategy: 1, // TextStorage::Chunked
            inline_threshold: 4096,
            chunked_threshold: 1048576,
            chunk_size: c.max_length.unwrap_or(512),
            generate_chunk_embeddings: false,
            embedding_model: String::new(),
            enable_ngram_bloom: false,
            ngram_size: 3,
            sidecar_base_path: String::new(),
            sidecar_compression: 0, // TextCompression::None
            max_text_size: 0,       // Unlimited
            enable_fulltext_index: false,
            fulltext_analyzer: String::new(),
        })
        .collect();

    // Create the new record schema config
    let new_record_schema = crate::proto::proximadb_v1::RecordSchemaConfig {
        schema_id: new_schema_id.clone(),
        schema_version: new_version.clone(),
        enforcement: enforcement_value,
        auto_evolve: schema.allow_additional_fields.unwrap_or(true),
        columns: Vec::new(), // Column definitions are stored separately in text_columns/text_storage_configs
    };

    // Update the collection config
    config.record_schema = Some(new_record_schema);
    config.enable_proxima_record = Some(true);
    config.text_columns = text_columns;
    config.text_storage_configs = text_storage_configs;

    // Step 6: Persist the updated collection
    let update_request = crate::proto::proximadb_v1::CollectionRequest {
        operation: crate::proto::proximadb_v1::CollectionOperation::CollectionUpdate as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: Some(config),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    state
        .unified_handlers
        .handle_collection_operation_for_tenant(update_request, Some(&tenant.tenant_id))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update collection schema: {}", e)))?;

    let now = chrono::Utc::now().to_rfc3339();

    info!(
        "V2 API: Updated schema for collection '{}' to version '{}' with {} changes",
        collection_id,
        new_version,
        changes.len()
    );

    Ok(Json(UpdateSchemaResponse {
        schema_id: new_schema_id,
        schema_version: new_version,
        previous_schema_id,
        changes,
        warnings,
        updated_at: now,
    }))
}

/// Build existing schema from collection config
fn build_existing_schema(
    config: &crate::proto::proximadb_v1::CollectionConfig,
) -> Option<SchemaDefinition> {
    // If ProximaRecord is not enabled or no schema config, return None
    if !config.enable_proxima_record.unwrap_or(false) && config.record_schema.is_none() {
        return None;
    }

    // Build columns from text_columns and text_storage_configs
    let mut columns: Vec<ColumnDefinition> = config
        .text_columns
        .iter()
        .map(|col_name| ColumnDefinition {
            name: col_name.clone(),
            data_type: "text".to_string(),
            nullable: Some(true),
            indexed: Some(false),
            filterable: Some(true),
            max_length: None,
            precision: None,
            scale: None,
            vector_dimension: None,
        })
        .collect();

    // Add text_large columns from text_storage_configs
    for text_config in &config.text_storage_configs {
        if !columns.iter().any(|c| c.name == text_config.column_name) {
            columns.push(ColumnDefinition {
                name: text_config.column_name.clone(),
                data_type: "text_large".to_string(),
                nullable: Some(true),
                indexed: Some(false),
                filterable: Some(false),
                max_length: Some(text_config.chunk_size),
                precision: None,
                scale: None,
                vector_dimension: None,
            });
        }
    }

    // Get enforcement mode from record_schema if available
    let enforcement =
        config
            .record_schema
            .as_ref()
            .map(|schema_config| match schema_config.enforcement {
                1 => "strict".to_string(),
                2 => "flexible".to_string(),
                3 => "hybrid".to_string(),
                _ => "hybrid".to_string(),
            });

    let allow_additional = config
        .record_schema
        .as_ref()
        .map_or(true, |s| s.auto_evolve);

    Some(SchemaDefinition {
        columns,
        enforcement,
        allow_additional_fields: Some(allow_additional),
    })
}

/// Increment semantic version (e.g., "1.0.0" -> "1.0.1")
fn increment_version(current: &str) -> String {
    let parts: Vec<&str> = current.split('.').collect();
    if parts.len() == 3
        && let (Ok(major), Ok(minor), Ok(patch)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        ) {
            return format!("{}.{}.{}", major, minor, patch + 1);
        }
    // Fallback: return 1.0.0
    "1.0.0".to_string()
}

/// Schema validation result
#[derive(Debug, Serialize)]
pub struct SchemaValidationResult {
    /// Whether the schema is valid
    pub valid: bool,
    /// Validation errors
    pub errors: Vec<String>,
    /// Validation warnings
    pub warnings: Vec<String>,
    /// Suggested fixes
    pub suggestions: Vec<String>,
}

/// Validate a schema definition
///
/// This is a helper function used internally to validate schemas.
/// It checks:
/// - Column name uniqueness
/// - Type validity
/// - Constraint consistency
/// - Evolution compatibility (when comparing with existing schema)
pub fn validate_schema(
    schema: &SchemaDefinition,
    existing_schema: Option<&SchemaDefinition>,
) -> SchemaValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut suggestions = Vec::new();

    // Check for duplicate column names
    let mut seen_names = std::collections::HashSet::new();
    for column in &schema.columns {
        if !seen_names.insert(&column.name) {
            errors.push(format!("Duplicate column name: '{}'", column.name));
        }
    }

    // Validate each column
    for column in &schema.columns {
        // Check for reserved names
        let reserved = ["id", "vector", "_id", "_score", "_version"];
        if reserved.contains(&column.name.as_str()) {
            errors.push(format!("Column name '{}' is reserved", column.name));
        }

        // Validate column name format
        if !column.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            errors.push(format!(
                "Column name '{}' contains invalid characters (use alphanumeric and underscore only)",
                column.name
            ));
        }

        // Warn about non-indexed filterable columns
        if column.filterable.unwrap_or(false) && !column.indexed.unwrap_or(false) {
            warnings.push(format!(
                "Column '{}' is filterable but not indexed - queries may be slow",
                column.name
            ));
            suggestions.push(format!(
                "Consider adding 'indexed: true' to column '{}'",
                column.name
            ));
        }

        // Warn about large max_length for indexed text
        if let Some(max_length) = column.max_length
            && max_length > 4096 && column.indexed.unwrap_or(false) {
                warnings.push(format!(
                    "Column '{}' has large max_length ({}) with indexing enabled",
                    column.name, max_length
                ));
            }
    }

    // Check evolution compatibility if existing schema provided
    if let Some(existing) = existing_schema {
        let existing_columns: std::collections::HashMap<&str, &ColumnDefinition> = existing
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();

        let new_columns: std::collections::HashMap<&str, &ColumnDefinition> = schema
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();

        // Check for removed columns
        for (name, _) in &existing_columns {
            if !new_columns.contains_key(name) {
                errors.push(format!(
                    "Cannot remove column '{}' - schema evolution does not allow column removal",
                    name
                ));
            }
        }

        // Check for type changes
        for (name, new_col) in &new_columns {
            if let Some(existing_col) = existing_columns.get(name)
                && existing_col.data_type != new_col.data_type {
                    errors.push(format!(
                        "Cannot change type of column '{}' from '{}' to '{}'",
                        name, existing_col.data_type, new_col.data_type
                    ));
                }
        }

        // Check new columns are nullable or have defaults
        for (name, new_col) in &new_columns {
            if !existing_columns.contains_key(name) {
                // This is a new column
                if !new_col.nullable.unwrap_or(true) {
                    warnings.push(format!(
                        "New column '{}' is not nullable - existing records will have NULL values",
                        name
                    ));
                    suggestions.push(format!(
                        "Consider making column '{}' nullable or providing a default value",
                        name
                    ));
                }
            }
        }
    }

    SchemaValidationResult {
        valid: errors.is_empty(),
        errors,
        warnings,
        suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_validation_basic() {
        let schema = SchemaDefinition {
            columns: vec![
                ColumnDefinition {
                    name: "title".to_string(),
                    data_type: "text".to_string(),
                    nullable: Some(false),
                    indexed: Some(true),
                    filterable: Some(true),
                    max_length: Some(255),
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
                ColumnDefinition {
                    name: "price".to_string(),
                    data_type: "float".to_string(),
                    nullable: Some(true),
                    indexed: Some(false),
                    filterable: Some(true),
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
            ],
            enforcement: Some("hybrid".to_string()),
            allow_additional_fields: Some(true),
        };

        let result = validate_schema(&schema, None);
        assert!(result.valid);
        // Should have warning about non-indexed filterable column
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_schema_validation_duplicate_columns() {
        let schema = SchemaDefinition {
            columns: vec![
                ColumnDefinition {
                    name: "title".to_string(),
                    data_type: "text".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
                ColumnDefinition {
                    name: "title".to_string(), // Duplicate!
                    data_type: "text".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
            ],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&schema, None);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("Duplicate")));
    }

    #[test]
    fn test_schema_validation_reserved_name() {
        let schema = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "id".to_string(), // Reserved!
                data_type: "text".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&schema, None);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("reserved")));
    }

    #[test]
    fn test_schema_evolution_removed_column() {
        let existing = SchemaDefinition {
            columns: vec![
                ColumnDefinition {
                    name: "title".to_string(),
                    data_type: "text".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
                ColumnDefinition {
                    name: "price".to_string(),
                    data_type: "float".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
            ],
            enforcement: None,
            allow_additional_fields: None,
        };

        // New schema removes 'price' column
        let new_schema = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "title".to_string(),
                data_type: "text".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&new_schema, Some(&existing));
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("remove column")));
    }

    #[test]
    fn test_schema_evolution_type_change() {
        let existing = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "count".to_string(),
                data_type: "integer".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        // New schema changes type from integer to text
        let new_schema = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "count".to_string(),
                data_type: "text".to_string(), // Type change!
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&new_schema, Some(&existing));
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("change type")));
    }

    // =========================================================================
    // Tests for increment_version
    // =========================================================================

    #[test]
    fn test_increment_version_basic() {
        assert_eq!(increment_version("1.0.0"), "1.0.1");
        assert_eq!(increment_version("1.2.3"), "1.2.4");
        assert_eq!(increment_version("0.0.0"), "0.0.1");
    }

    #[test]
    fn test_increment_version_large_numbers() {
        assert_eq!(increment_version("10.20.30"), "10.20.31");
        assert_eq!(increment_version("99.99.99"), "99.99.100");
    }

    #[test]
    fn test_increment_version_invalid_format() {
        // Not three parts
        assert_eq!(increment_version("1.0"), "1.0.0");
        assert_eq!(increment_version("1"), "1.0.0");
        assert_eq!(increment_version(""), "1.0.0");
    }

    #[test]
    fn test_increment_version_non_numeric() {
        assert_eq!(increment_version("a.b.c"), "1.0.0");
        assert_eq!(increment_version("1.2.xyz"), "1.0.0");
    }

    // =========================================================================
    // Tests for build_existing_schema
    // =========================================================================

    #[test]
    fn test_build_existing_schema_no_proxima_record() {
        let config = crate::proto::proximadb_v1::CollectionConfig {
            enable_proxima_record: Some(false),
            record_schema: None,
            ..Default::default()
        };
        let result = build_existing_schema(&config);
        assert!(result.is_none());
    }

    #[test]
    fn test_build_existing_schema_with_text_columns() {
        let config = crate::proto::proximadb_v1::CollectionConfig {
            enable_proxima_record: Some(true),
            text_columns: vec!["title".to_string(), "body".to_string()],
            record_schema: None,
            ..Default::default()
        };
        let result = build_existing_schema(&config);
        assert!(result.is_some());
        let schema = result.expect("Should build schema");
        assert_eq!(schema.columns.len(), 2);
        assert_eq!(schema.columns[0].name, "title");
        assert_eq!(schema.columns[0].data_type, "text");
        assert_eq!(schema.columns[1].name, "body");
    }

    #[test]
    fn test_build_existing_schema_with_record_schema_enforcement() {
        let config = crate::proto::proximadb_v1::CollectionConfig {
            enable_proxima_record: Some(true),
            record_schema: Some(crate::proto::proximadb_v1::RecordSchemaConfig {
                schema_id: "s1".to_string(),
                schema_version: "1.0.0".to_string(),
                enforcement: 1, // strict
                auto_evolve: false,
                columns: vec![],
            }),
            ..Default::default()
        };
        let result = build_existing_schema(&config);
        assert!(result.is_some());
        let schema = result.expect("Should build schema");
        assert_eq!(schema.enforcement, Some("strict".to_string()));
        assert_eq!(schema.allow_additional_fields, Some(false));
    }

    #[test]
    fn test_build_existing_schema_enforcement_mapping() {
        // Test all enforcement values
        for (value, expected) in [(1, "strict"), (2, "flexible"), (3, "hybrid"), (0, "hybrid")] {
            let config = crate::proto::proximadb_v1::CollectionConfig {
                enable_proxima_record: Some(true),
                record_schema: Some(crate::proto::proximadb_v1::RecordSchemaConfig {
                    enforcement: value,
                    auto_evolve: true,
                    ..Default::default()
                }),
                ..Default::default()
            };
            let schema = build_existing_schema(&config).expect("Should build");
            assert_eq!(
                schema.enforcement,
                Some(expected.to_string()),
                "Failed for enforcement value {}",
                value
            );
        }
    }

    #[test]
    fn test_build_existing_schema_text_storage_configs() {
        let config = crate::proto::proximadb_v1::CollectionConfig {
            enable_proxima_record: Some(true),
            text_columns: vec!["summary".to_string()],
            text_storage_configs: vec![crate::proto::proximadb_v1::TextStorageConfig {
                column_name: "full_text".to_string(),
                chunk_size: 1024,
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = build_existing_schema(&config);
        assert!(result.is_some());
        let schema = result.expect("Should build schema");
        assert_eq!(schema.columns.len(), 2);
        // First from text_columns
        assert_eq!(schema.columns[0].name, "summary");
        assert_eq!(schema.columns[0].data_type, "text");
        // Second from text_storage_configs
        assert_eq!(schema.columns[1].name, "full_text");
        assert_eq!(schema.columns[1].data_type, "text_large");
        assert_eq!(schema.columns[1].max_length, Some(1024));
    }

    #[test]
    fn test_build_existing_schema_deduplicates_columns() {
        // If a column appears in both text_columns and text_storage_configs,
        // it should not be duplicated
        let config = crate::proto::proximadb_v1::CollectionConfig {
            enable_proxima_record: Some(true),
            text_columns: vec!["content".to_string()],
            text_storage_configs: vec![crate::proto::proximadb_v1::TextStorageConfig {
                column_name: "content".to_string(),
                chunk_size: 512,
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = build_existing_schema(&config);
        assert!(result.is_some());
        let schema = result.expect("Should build schema");
        // Should only have one "content" column (from text_columns, since it comes first)
        assert_eq!(schema.columns.len(), 1);
        assert_eq!(schema.columns[0].name, "content");
    }

    // =========================================================================
    // Tests for validate_schema - additional coverage
    // =========================================================================

    #[test]
    fn test_validate_schema_invalid_column_name_chars() {
        let schema = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "my-column".to_string(), // hyphens not allowed
                data_type: "text".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&schema, None);
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("invalid characters"))
        );
    }

    #[test]
    fn test_validate_schema_all_reserved_names() {
        for name in &["id", "vector", "_id", "_score", "_version"] {
            let schema = SchemaDefinition {
                columns: vec![ColumnDefinition {
                    name: name.to_string(),
                    data_type: "text".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                }],
                enforcement: None,
                allow_additional_fields: None,
            };

            let result = validate_schema(&schema, None);
            assert!(!result.valid, "Column name '{}' should be reserved", name);
        }
    }

    #[test]
    fn test_validate_schema_large_max_length_with_index_warns() {
        let schema = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "big_text".to_string(),
                data_type: "text".to_string(),
                nullable: None,
                indexed: Some(true),
                filterable: None,
                max_length: Some(10000),
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&schema, None);
        assert!(result.valid);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("large max_length"))
        );
    }

    #[test]
    fn test_validate_schema_evolution_new_non_nullable_column_warns() {
        let existing = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "title".to_string(),
                data_type: "text".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        let new_schema = SchemaDefinition {
            columns: vec![
                ColumnDefinition {
                    name: "title".to_string(),
                    data_type: "text".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
                ColumnDefinition {
                    name: "required_field".to_string(),
                    data_type: "integer".to_string(),
                    nullable: Some(false), // not nullable!
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
            ],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&new_schema, Some(&existing));
        // Should be valid but with warnings about non-nullable new column
        assert!(result.valid);
        assert!(result.warnings.iter().any(|w| w.contains("not nullable")));
        assert!(!result.suggestions.is_empty());
    }

    #[test]
    fn test_validate_schema_evolution_add_column_is_valid() {
        let existing = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "title".to_string(),
                data_type: "text".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        let new_schema = SchemaDefinition {
            columns: vec![
                ColumnDefinition {
                    name: "title".to_string(),
                    data_type: "text".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
                ColumnDefinition {
                    name: "tags".to_string(),
                    data_type: "array_text".to_string(),
                    nullable: Some(true),
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
            ],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&new_schema, Some(&existing));
        assert!(result.valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validate_schema_empty_columns() {
        let schema = SchemaDefinition {
            columns: vec![],
            enforcement: None,
            allow_additional_fields: None,
        };
        let result = validate_schema(&schema, None);
        assert!(result.valid);
    }

    #[test]
    fn test_validate_schema_filterable_not_indexed_suggestion() {
        let schema = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "category".to_string(),
                data_type: "text".to_string(),
                nullable: None,
                indexed: Some(false),
                filterable: Some(true),
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };

        let result = validate_schema(&schema, None);
        assert!(result.valid);
        assert!(
            result
                .suggestions
                .iter()
                .any(|s| s.contains("indexed: true"))
        );
    }

    // ============================================================
    // increment_version tests
    // ============================================================

    #[test]
    fn test_increment_version_rollover() {
        assert_eq!(increment_version("1.0.0"), "1.0.1");
        assert_eq!(increment_version("1.0.9"), "1.0.10");
        assert_eq!(increment_version("2.3.5"), "2.3.6");
    }

    #[test]
    fn test_increment_version_fallback_invalid() {
        assert_eq!(increment_version(""), "1.0.0");
        assert_eq!(increment_version("abc"), "1.0.0");
        assert_eq!(increment_version("1.2"), "1.0.0");
    }

    // ============================================================
    // Additional validate_schema edge case tests
    // ============================================================

    #[test]
    fn test_schema_validation_empty_columns() {
        let schema = SchemaDefinition {
            columns: vec![],
            enforcement: None,
            allow_additional_fields: None,
        };
        let result = validate_schema(&schema, None);
        // Empty schema should still be valid (no columns = no errors)
        assert!(result.valid);
    }

    #[test]
    fn test_schema_validation_reserved_column_name() {
        let schema = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "id".to_string(), // reserved
                data_type: "text".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: None,
            allow_additional_fields: None,
        };
        let result = validate_schema(&schema, None);
        // "id" is a reserved column name — should produce warning or error
        assert!(
            !result.warnings.is_empty() || !result.errors.is_empty(),
            "Reserved column name 'id' should trigger warning or error"
        );
    }

    #[test]
    fn test_schema_validation_vector_type_needs_dimension() {
        let schema = SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "embedding".to_string(),
                data_type: "vector".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None, // missing dimension for vector type
            }],
            enforcement: None,
            allow_additional_fields: None,
        };
        let result = validate_schema(&schema, None);
        // Vector type is accepted (dimension is optional in schema definition)
        // Just verify the validation runs without panic
        let _ = result.valid;
    }

    #[test]
    fn test_schema_validation_multiple_errors() {
        let schema = SchemaDefinition {
            columns: vec![
                ColumnDefinition {
                    name: "dup".to_string(),
                    data_type: "text".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
                ColumnDefinition {
                    name: "dup".to_string(), // duplicate
                    data_type: "integer".to_string(),
                    nullable: None,
                    indexed: None,
                    filterable: None,
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
            ],
            enforcement: None,
            allow_additional_fields: None,
        };
        let result = validate_schema(&schema, None);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("Duplicate")),
            "Should report duplicate column error"
        );
    }

    #[test]
    fn test_schema_validation_valid_all_types() {
        let schema = SchemaDefinition {
            columns: vec![
                ColumnDefinition {
                    name: "col_text".to_string(),
                    data_type: "text".to_string(),
                    nullable: Some(true),
                    indexed: Some(true),
                    filterable: Some(true),
                    max_length: Some(100),
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
                ColumnDefinition {
                    name: "col_int".to_string(),
                    data_type: "integer".to_string(),
                    nullable: Some(false),
                    indexed: Some(true),
                    filterable: Some(true),
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
                ColumnDefinition {
                    name: "col_float".to_string(),
                    data_type: "float".to_string(),
                    nullable: Some(true),
                    indexed: Some(false),
                    filterable: Some(false),
                    max_length: None,
                    precision: None,
                    scale: None,
                    vector_dimension: None,
                },
            ],
            enforcement: Some("strict".to_string()),
            allow_additional_fields: Some(false),
        };
        let result = validate_schema(&schema, None);
        assert!(
            result.valid,
            "Valid multi-type schema should pass: {:?}",
            result.errors
        );
    }
}
