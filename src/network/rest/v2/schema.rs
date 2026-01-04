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
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::errors::{ApiError, ApiResult};
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
    State(_state): State<AppState>,
) -> ApiResult<Json<SchemaResponse>> {
    debug!("V2 API: Getting schema for collection '{}'", collection_id);

    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    // Placeholder implementation
    // TODO: Implement actual schema retrieval
    //
    // Implementation steps:
    // 1. Verify collection exists
    // 2. Check if ProximaRecord is enabled
    // 3. Load schema from metadata store
    // 4. Return schema with version info

    // For now, return not found since this is a placeholder
    Err(ApiError::CollectionNotFound(format!(
        "Schema not found for collection '{}'",
        collection_id
    )))
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
    State(_state): State<AppState>,
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
        if column.data_type == "decimal" {
            if column.precision.is_none() || column.scale.is_none() {
                return Err(ApiError::InvalidArgument(format!(
                    "Column '{}' with type 'decimal' requires precision and scale",
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

    // Placeholder implementation
    // TODO: Implement actual schema update with evolution validation
    //
    // Implementation steps:
    // 1. Load current schema
    // 2. Compare with new schema for evolution rules:
    //    - Check for removed columns (error if not force)
    //    - Check for type changes (error unless compatible widening)
    //    - Check new columns are nullable or have defaults
    // 3. Generate schema diff and changes list
    // 4. Create new schema version
    // 5. Store updated schema with parent reference
    // 6. Update collection metadata

    // For now, return not found since collection doesn't exist in placeholder
    Err(ApiError::CollectionNotFound(format!(
        "Collection '{}' not found",
        collection_id
    )))
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
        if let Some(max_length) = column.max_length {
            if max_length > 4096 && column.indexed.unwrap_or(false) {
                warnings.push(format!(
                    "Column '{}' has large max_length ({}) with indexing enabled",
                    column.name, max_length
                ));
            }
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
            if let Some(existing_col) = existing_columns.get(name) {
                if existing_col.data_type != new_col.data_type {
                    errors.push(format!(
                        "Cannot change type of column '{}' from '{}' to '{}'",
                        name, existing_col.data_type, new_col.data_type
                    ));
                }
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
}
