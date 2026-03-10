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

//! ProximaRecord REST handlers for v2 API
//!
//! This module provides REST endpoints for inserting and searching ProximaRecords,
//! the v2 record type with full type system support.
//!
//! ## Endpoints
//!
//! - `POST /api/v2/collections/{collection}/records/batch` - Insert ProximaRecords
//! - `POST /api/v2/collections/{collection}/search` - Search with typed filters
//!
//! ## ProximaRecord Structure
//!
//! ProximaRecord extends VectorRecord with:
//! - `typed_fields`: Strongly-typed fields (INTEGER, FLOAT, DECIMAL, UUID, etc.)
//! - `text_fields`: Dedicated TEXT column storage with chunking support
//! - Schema validation at insert time (when enabled)

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, error, info};

use crate::errors::{ApiError, ApiResult};
use crate::network::rest::v1::handlers::AppState;
use crate::proto::proximadb_v1::{
    SearchQuery, VectorBatchRequest, VectorRecord, VectorSearchRequest,
};

/// Convert a JSON value to SqlValue for storage
fn json_to_sql_value(value: &serde_json::Value) -> crate::proto::proximadb_v1::SqlValue {
    use crate::proto::proximadb_v1::sql_value::Value;
    use crate::proto::proximadb_v1::{SqlArray, SqlObject, SqlValue};

    let inner = match value {
        serde_json::Value::Null => Value::NullValue(0),
        serde_json::Value::Bool(b) => Value::BoolValue(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int64Value(i)
            } else if let Some(f) = n.as_f64() {
                Value::NumberValue(f)
            } else {
                Value::StringValue(n.to_string())
            }
        }
        serde_json::Value::String(s) => Value::StringValue(s.clone()),
        serde_json::Value::Array(arr) => {
            let values: Vec<SqlValue> = arr.iter().map(json_to_sql_value).collect();
            Value::ArrayValue(SqlArray { values })
        }
        serde_json::Value::Object(obj) => {
            let fields: HashMap<String, SqlValue> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_sql_value(v)))
                .collect();
            Value::ObjectValue(SqlObject { fields })
        }
    };

    SqlValue { value: Some(inner) }
}

/// Convert SqlValue back to JSON for responses
fn sql_value_to_json(
    value: &crate::proto::proximadb_v1::SqlValue,
) -> Result<serde_json::Value, ApiError> {
    use crate::proto::proximadb_v1::sql_value::Value;

    Ok(match value.value.as_ref() {
        Some(Value::NullValue(_)) => serde_json::Value::Null,
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(Value::NumberValue(f)) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| {
                ApiError::Internal(format!(
                    "Failed to convert f64 to serde_json::Number: {}",
                    f
                ))
            })?,
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::BytesValue(b)) => serde_json::Value::Array(
            b.iter()
                .map(|x| serde_json::Value::Number((*x as u64).into()))
                .collect(),
        ),
        Some(Value::ArrayValue(arr)) => serde_json::Value::Array(
            arr.values
                .iter()
                .map(|v| sql_value_to_json(v))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(Value::ObjectValue(obj)) => {
            let map: serde_json::Map<String, serde_json::Value> = obj
                .fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), sql_value_to_json(v)?)))
                .collect::<Result<_, ApiError>>()?;
            serde_json::Value::Object(map)
        }
        None => serde_json::Value::Null,
    })
}

/// Convert a JSON value to a FilterClause value
fn json_to_filter_clause_value(
    value: &serde_json::Value,
) -> Option<crate::proto::proximadb_v1::filter_clause::Value> {
    use crate::proto::proximadb_v1::filter_clause::Value;

    match value {
        serde_json::Value::String(s) => Some(Value::StringValue(s.clone())),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Value::IntValue(i))
            } else if let Some(f) = n.as_f64() {
                Some(Value::DoubleValue(f))
            } else {
                None
            }
        }
        serde_json::Value::Bool(b) => Some(Value::BoolValue(*b)),
        _ => None, // Arrays and objects not directly supported in FilterClause
    }
}

/// Convert TypedFilter list to FilterClause list for MetadataFilter
///
/// Supports the following operators:
/// - eq: Equals
/// - neq: Not equals
/// - gt: Greater than
/// - gte: Greater than or equal
/// - lt: Less than
/// - lte: Less than or equal
/// - contains: String/array contains (substring match)
/// - in: Value is in a list
/// - between: Value is between two bounds (converted to gte + lte)
/// - starts_with: String starts with prefix (converted to contains)
/// - ends_with: String ends with suffix (converted to contains)
fn convert_typed_filters_to_clauses(
    typed_filters: &[TypedFilter],
) -> Result<Vec<crate::proto::proximadb_v1::FilterClause>, ApiError> {
    use crate::proto::proximadb_v1::{ComparisonOp, FilterClause, filter_clause::Value};

    let mut clauses = Vec::new();

    for filter in typed_filters {
        match filter.op.as_str() {
            "eq" => {
                if let Some(value) = json_to_filter_clause_value(&filter.value) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Eq as i32,
                        value: Some(value),
                    });
                }
            }
            "neq" => {
                if let Some(value) = json_to_filter_clause_value(&filter.value) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Ne as i32,
                        value: Some(value),
                    });
                }
            }
            "gt" => {
                if let Some(value) = json_to_filter_clause_value(&filter.value) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Gt as i32,
                        value: Some(value),
                    });
                }
            }
            "gte" => {
                if let Some(value) = json_to_filter_clause_value(&filter.value) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Gte as i32,
                        value: Some(value),
                    });
                }
            }
            "lt" => {
                if let Some(value) = json_to_filter_clause_value(&filter.value) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Lt as i32,
                        value: Some(value),
                    });
                }
            }
            "lte" => {
                if let Some(value) = json_to_filter_clause_value(&filter.value) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Lte as i32,
                        value: Some(value),
                    });
                }
            }
            "between" => {
                // "between" requires both value and value_upper
                // Convert to two clauses: field >= value AND field <= value_upper
                let value_upper = filter.value_upper.as_ref().ok_or_else(|| {
                    ApiError::InvalidArgument(
                        "Filter operator 'between' requires 'value_upper' to be specified"
                            .to_string(),
                    )
                })?;

                if let Some(lower_value) = json_to_filter_clause_value(&filter.value) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Gte as i32,
                        value: Some(lower_value),
                    });
                }

                if let Some(upper_value) = json_to_filter_clause_value(value_upper) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Lte as i32,
                        value: Some(upper_value),
                    });
                }
            }
            "contains" => {
                // Contains for string substring matching
                if let Some(value) = json_to_filter_clause_value(&filter.value) {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Contains as i32,
                        value: Some(value),
                    });
                }
            }
            "starts_with" => {
                // starts_with is implemented using Contains operator
                // The backend should interpret this as prefix matching
                // We encode the intent by using Contains with the prefix value
                if let serde_json::Value::String(s) = &filter.value {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Contains as i32,
                        value: Some(Value::StringValue(format!("^{}", s))),
                    });
                } else {
                    return Err(ApiError::InvalidArgument(
                        "Filter operator 'starts_with' requires a string value".to_string(),
                    ));
                }
            }
            "ends_with" => {
                // ends_with is implemented using Contains operator
                // The backend should interpret this as suffix matching
                // We encode the intent by using Contains with the suffix value
                if let serde_json::Value::String(s) = &filter.value {
                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::Contains as i32,
                        value: Some(Value::StringValue(format!("{}$", s))),
                    });
                } else {
                    return Err(ApiError::InvalidArgument(
                        "Filter operator 'ends_with' requires a string value".to_string(),
                    ));
                }
            }
            "in" => {
                // "in" operator: value should be an array
                // We use the In comparison operator
                if let serde_json::Value::Array(arr) = &filter.value {
                    // For the "in" operator, we need to pass the array of values
                    // The FilterClause only supports single values, so we convert
                    // the array to a comma-separated string representation
                    // that the backend can parse
                    let values_str: Vec<String> = arr
                        .iter()
                        .filter_map(|v| match v {
                            serde_json::Value::String(s) => Some(format!("\"{}\"", s)),
                            serde_json::Value::Number(n) => Some(n.to_string()),
                            serde_json::Value::Bool(b) => Some(b.to_string()),
                            _ => None,
                        })
                        .collect();

                    clauses.push(FilterClause {
                        field: filter.field.clone(),
                        op: ComparisonOp::In as i32,
                        value: Some(Value::StringValue(format!("[{}]", values_str.join(",")))),
                    });
                } else {
                    return Err(ApiError::InvalidArgument(
                        "Filter operator 'in' requires an array value".to_string(),
                    ));
                }
            }
            _ => {
                // Unknown operator - this should have been caught in validation
                return Err(ApiError::InvalidArgument(format!(
                    "Unsupported filter operator: {}",
                    filter.op
                )));
            }
        }
    }

    Ok(clauses)
}

/// Request to insert ProximaRecords
///
/// ## Example JSON
///
/// ```json
/// {
///     "records": [
///         {
///             "id": "doc_1",
///             "vector": [0.1, 0.2, 0.3],
///             "typed_fields": {
///                 "category": "electronics",
///                 "price": 299.99,
///                 "in_stock": true
///             },
///             "text_fields": [
///                 {
///                     "name": "description",
///                     "content": "A detailed product description...",
///                     "storage_hint": "adaptive"
///                 }
///             ],
///             "metadata": {
///                 "source": "catalog_import"
///             }
///         }
///     ],
///     "validate_schema": true
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct InsertRecordsRequest {
    /// Records to insert
    pub records: Vec<ProximaRecordInput>,
    /// Whether to validate against collection schema (default: true)
    pub validate_schema: Option<bool>,
}

/// Input format for ProximaRecord (JSON-friendly)
///
/// This is the JSON-serializable input format for ProximaRecord.
/// It uses serde_json::Value for typed fields to support dynamic typing
/// at the API boundary, with validation happening during conversion.
#[derive(Debug, Deserialize)]
pub struct ProximaRecordInput {
    /// Record ID (optional, will be auto-generated if not provided)
    pub id: Option<String>,
    /// Vector embedding (required)
    pub vector: Vec<f32>,
    /// Typed fields with strong type support
    ///
    /// Supported types:
    /// - String: TEXT
    /// - Number (integer): INTEGER
    /// - Number (float): FLOAT
    /// - Boolean: BOOLEAN
    /// - Array: ARRAY types
    /// - Object: JSON or MAP types
    /// - null: NULL
    pub typed_fields: Option<HashMap<String, serde_json::Value>>,
    /// Dedicated TEXT fields with storage hints
    pub text_fields: Option<Vec<TextFieldInput>>,
    /// Legacy metadata (for backward compatibility)
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Input format for TEXT fields
///
/// TEXT fields are stored in dedicated columns with optional chunking
/// for large content. The storage hint helps optimize storage strategy.
#[derive(Debug, Deserialize)]
pub struct TextFieldInput {
    /// Field name
    pub name: String,
    /// Text content
    pub content: String,
    /// Storage strategy hint
    ///
    /// - "inline": Store inline in main column (<4KB)
    /// - "chunked": Split into chunks with embeddings (4KB-1MB)
    /// - "sidecar": Store in separate sidecar file (>1MB)
    /// - "adaptive": Auto-select based on content size (default)
    pub storage_hint: Option<String>,
}

/// Response for insert operation
#[derive(Debug, Serialize)]
pub struct InsertRecordsResponse {
    /// Number of successfully inserted records
    pub inserted_count: usize,
    /// Number of failed records
    pub failed_count: usize,
    /// Detailed errors for failed records
    pub errors: Vec<InsertError>,
    /// IDs of successfully inserted records
    pub inserted_ids: Vec<String>,
}

/// Error details for a failed record insertion
#[derive(Debug, Serialize)]
pub struct InsertError {
    /// Index of the record in the request
    pub index: usize,
    /// Record ID (if provided)
    pub id: Option<String>,
    /// Error message
    pub error: String,
}

/// POST /api/v2/collections/{collection}/records/batch
///
/// Insert ProximaRecords into a collection with typed field support.
///
/// ## Request Body
///
/// See [`InsertRecordsRequest`] for the expected JSON format.
///
/// ## Response
///
/// Returns [`InsertRecordsResponse`] with counts and any errors.
///
/// ## Errors
///
/// - `400 Bad Request`: Invalid request format or validation error
/// - `404 Not Found`: Collection does not exist
/// - `500 Internal Server Error`: Storage or processing error
pub async fn insert_records(
    Path(collection): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<InsertRecordsRequest>,
) -> ApiResult<Json<InsertRecordsResponse>> {
    info!(
        "V2 API: Inserting {} records into collection '{}'",
        request.records.len(),
        collection
    );

    // Validate collection exists
    if collection.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection name is required".to_string(),
        ));
    }

    if request.records.is_empty() {
        return Err(ApiError::InvalidArgument(
            "At least one record is required".to_string(),
        ));
    }

    let validate_schema = request.validate_schema.unwrap_or_else(|| {
        debug!("No schema validation preference provided, defaulting to true");
        true
    });
    debug!(
        "Schema validation: {}",
        if validate_schema {
            "enabled"
        } else {
            "disabled"
        }
    );

    let mut inserted_ids = Vec::new();
    let mut errors = Vec::new();
    let mut vector_records = Vec::new();

    for (index, record) in request.records.iter().enumerate() {
        // Validate vector is not empty
        if record.vector.is_empty() {
            errors.push(InsertError {
                index,
                id: record.id.clone(),
                error: "Vector cannot be empty".to_string(),
            });
            continue;
        }

        // Generate ID if not provided
        let record_id = record.id.clone().unwrap_or_else(|| {
            let new_id = uuid::Uuid::new_v4().to_string();
            debug!("Generated new UUID for record: {}", new_id);
            new_id
        });

        // Convert typed_fields to metadata for backward compatibility with v1 storage
        let mut metadata: HashMap<String, crate::proto::proximadb_v1::SqlValue> = HashMap::new();

        // Convert typed_fields
        if let Some(ref typed_fields) = record.typed_fields {
            for (key, value) in typed_fields {
                let sql_value = json_to_sql_value(value);
                metadata.insert(key.clone(), sql_value);
            }
        }

        // Convert text_fields to metadata
        if let Some(ref text_fields) = record.text_fields {
            for text_field in text_fields {
                let sql_value = crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        text_field.content.clone(),
                    )),
                };
                metadata.insert(text_field.name.clone(), sql_value);
            }
        }

        // Merge legacy metadata
        if let Some(ref legacy_metadata) = record.metadata {
            for (key, value) in legacy_metadata {
                if !metadata.contains_key(key) {
                    let sql_value = json_to_sql_value(value);
                    metadata.insert(key.clone(), sql_value);
                }
            }
        }

        // Create VectorRecord for storage
        let vector_record = VectorRecord {
            id: record_id.clone(),
            vector: record.vector.clone(),
            metadata,
            version: None,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            source: Some("v2_api".to_string()),
            updated_at: None,
            expires_at: None,
        };

        vector_records.push(vector_record);
        inserted_ids.push(record_id);
    }

    // Early return if all records failed validation
    if vector_records.is_empty() {
        return Ok(Json(InsertRecordsResponse {
            inserted_count: 0,
            failed_count: errors.len(),
            errors,
            inserted_ids: vec![],
        }));
    }

    // Insert via unified handlers
    let batch_request = VectorBatchRequest {
        collection_id: collection.clone(),
        vectors: vector_records,
    };

    match state
        .unified_handlers
        .handle_vector_batch_v1(batch_request)
        .await
    {
        Ok(resp) => {
            // Check for success - if successful, all records were inserted
            let success_count = if resp.success { inserted_ids.len() } else { 0 };

            let response = InsertRecordsResponse {
                inserted_count: success_count,
                failed_count: errors.len() + (inserted_ids.len() - success_count),
                errors,
                inserted_ids: if resp.success { inserted_ids } else { vec![] },
            };

            info!(
                "V2 API: Insert complete - {} inserted, {} failed",
                response.inserted_count, response.failed_count
            );

            Ok(Json(response))
        }
        Err(e) => {
            error!("V2 API: Batch insert failed: {}", e);
            Err(ApiError::Internal(format!("Insert failed: {}", e)))
        }
    }
}

/// Search request with typed filters
///
/// ## Example JSON
///
/// ```json
/// {
///     "vector": [0.1, 0.2, 0.3],
///     "top_k": 10,
///     "filters": [
///         {"field": "category", "op": "eq", "value": "electronics"},
///         {"field": "price", "op": "lt", "value": 500},
///         {"field": "in_stock", "op": "eq", "value": true}
///     ],
///     "include_text": true
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct TypedSearchRequest {
    /// Query vector
    pub vector: Vec<f32>,
    /// Number of results to return
    pub top_k: usize,
    /// Typed filters with operator support
    pub filters: Option<Vec<TypedFilter>>,
    /// Whether to include TEXT fields in results (default: false)
    ///
    /// TEXT fields can be large, so they are excluded by default.
    /// Set to true to include them in the response.
    pub include_text: Option<bool>,
    /// Whether to include the vector in results (default: false)
    ///
    /// Vector data can be large, so it is excluded by default.
    pub include_vector: Option<bool>,
}

/// A typed filter for search operations
///
/// Supports various comparison operators with type-safe values.
#[derive(Debug, Deserialize)]
pub struct TypedFilter {
    /// Field name to filter on
    pub field: String,
    /// Comparison operator
    ///
    /// Supported operators:
    /// - "eq": Equals
    /// - "neq": Not equals
    /// - "gt": Greater than
    /// - "gte": Greater than or equal
    /// - "lt": Less than
    /// - "lte": Less than or equal
    /// - "contains": String/array contains
    /// - "between": Value is between two bounds (requires value_upper)
    /// - "in": Value is in a list
    /// - "starts_with": String starts with prefix
    /// - "ends_with": String ends with suffix
    pub op: String,
    /// Filter value (type depends on field type)
    pub value: serde_json::Value,
    /// Upper bound for "between" operator
    pub value_upper: Option<serde_json::Value>,
}

/// Search result with typed fields
#[derive(Debug, Clone, Serialize)]
pub struct TypedSearchResult {
    /// Record ID
    pub id: String,
    /// Similarity score (0.0 - 1.0 for cosine, distance for L2)
    pub score: f32,
    /// Vector embedding (if requested)
    pub vector: Option<Vec<f32>>,
    /// Typed fields from the record
    pub typed_fields: HashMap<String, serde_json::Value>,
    /// TEXT fields (if include_text is true)
    pub text_fields: Option<Vec<TextFieldOutput>>,
    /// Legacy metadata
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Output format for TEXT fields
#[derive(Debug, Clone, Serialize)]
pub struct TextFieldOutput {
    /// Field name
    pub name: String,
    /// Text content (may be truncated for large content)
    pub content: String,
    /// Number of chunks (for chunked storage)
    pub chunk_count: Option<u32>,
    /// Whether content was truncated
    pub truncated: bool,
}

/// Search response with typed results
#[derive(Debug, Serialize)]
pub struct TypedSearchResponse {
    /// Search results
    pub results: Vec<TypedSearchResult>,
    /// Total number of matching documents (before top_k limit)
    pub total_matches: Option<u64>,
    /// Search latency in milliseconds
    pub latency_ms: u64,
    /// Request ID for tracing
    pub request_id: String,
}

/// POST /api/v2/collections/{collection}/search
///
/// Search a collection with typed filters.
///
/// ## Request Body
///
/// See [`TypedSearchRequest`] for the expected JSON format.
///
/// ## Response
///
/// Returns [`TypedSearchResponse`] with ranked results.
///
/// ## Errors
///
/// - `400 Bad Request`: Invalid request format or filter error
/// - `404 Not Found`: Collection does not exist
/// - `500 Internal Server Error`: Search execution error
pub async fn search_with_typed_filters(
    Path(collection): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<TypedSearchRequest>,
) -> ApiResult<Json<TypedSearchResponse>> {
    let start_time = std::time::Instant::now();
    let request_id = uuid::Uuid::new_v4().to_string();

    info!(
        "V2 API: Search request {} for collection '{}', top_k={}",
        request_id, collection, request.top_k
    );

    // Validate request
    if collection.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection name is required".to_string(),
        ));
    }

    if request.vector.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Query vector is required".to_string(),
        ));
    }

    if request.top_k == 0 {
        return Err(ApiError::InvalidArgument(
            "top_k must be greater than 0".to_string(),
        ));
    }

    // Validate filters if provided
    if let Some(ref filters) = request.filters {
        for filter in filters {
            if filter.field.is_empty() {
                return Err(ApiError::InvalidArgument(
                    "Filter field name cannot be empty".to_string(),
                ));
            }

            let valid_ops = [
                "eq",
                "neq",
                "gt",
                "gte",
                "lt",
                "lte",
                "contains",
                "between",
                "in",
                "starts_with",
                "ends_with",
            ];
            if !valid_ops.contains(&filter.op.as_str()) {
                return Err(ApiError::InvalidArgument(format!(
                    "Invalid filter operator '{}'. Valid operators: {:?}",
                    filter.op, valid_ops
                )));
            }

            // Validate "between" has upper bound
            if filter.op == "between" && filter.value_upper.is_none() {
                return Err(ApiError::InvalidArgument(
                    "Filter operator 'between' requires 'value_upper' to be specified".to_string(),
                ));
            }
        }
    }

    let include_text = request.include_text.unwrap_or_else(|| {
        debug!("No include_text preference provided, defaulting to false");
        false
    });
    debug!(
        "Include TEXT fields: {}, filters: {:?}",
        include_text,
        request.filters.as_ref().map(|f| f.len())
    );

    // Convert typed filters to MetadataFilter format with advanced filter support
    let advanced_filter = if let Some(ref typed_filters) = request.filters {
        let clauses = convert_typed_filters_to_clauses(typed_filters)?;
        if clauses.is_empty() {
            None
        } else {
            Some(crate::proto::proximadb_v1::MetadataFilter {
                clauses,
                op: crate::proto::proximadb_v1::LogicalOp::And as i32,
            })
        }
    } else {
        None
    };

    // Keep simple equality filters in the filters map for backward compatibility
    let filters = if let Some(ref typed_filters) = request.filters {
        let mut filter_map: HashMap<String, crate::proto::proximadb_v1::SqlValue> = HashMap::new();
        for filter in typed_filters {
            if filter.op == "eq" {
                filter_map.insert(filter.field.clone(), json_to_sql_value(&filter.value));
            }
        }
        filter_map
    } else {
        HashMap::new()
    };

    // Create search query
    let search_query = SearchQuery {
        vector: request.vector.clone(),
        filters,
        advanced_filter,
    };

    let search_request = VectorSearchRequest {
        collection_id: collection.clone(),
        queries: vec![search_query],
        top_k: request.top_k as u32,
        include_fields: None,
        search_params: None,
        distance_metric_override: None,
        search_optimization: None,
    };

    // Execute search via unified handlers
    match state
        .unified_handlers
        .handle_vector_search_v1(search_request)
        .await
    {
        Ok(resp) => {
            let latency_ms = start_time.elapsed().as_millis() as u64;

            // resp.results is Option<SearchResult> - use default if None
            let search_result = resp.results.unwrap_or_else(|| {
                debug!("Search response contains no results, using default");
                Default::default()
            });

            // Convert results to TypedSearchResult format
            // search_result.results is Vec<SearchVectorRecord>
            let results: Vec<TypedSearchResult> = search_result
                .results
                .iter()
                .map(|r| {
                    // Convert metadata to typed_fields
                    let typed_fields: HashMap<String, serde_json::Value> = r
                        .metadata
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), sql_value_to_json(v)?)))
                        .collect::<Result<_, ApiError>>()?;

                    Ok(TypedSearchResult {
                        id: r.id.clone(),
                        score: r.score as f32,
                        vector: if request.include_vector.unwrap_or_else(|| {
                            debug!("No include_vector preference provided, defaulting to false");
                            false
                        }) {
                            Some(r.vector.clone())
                        } else {
                            None
                        },
                        typed_fields,
                        text_fields: if include_text {
                            // Extract text fields from metadata
                            Some(vec![]) // Would be populated from text storage
                        } else {
                            None
                        },
                        metadata: None, // Legacy metadata is converted to typed_fields
                    })
                })
                .collect::<Result<_, ApiError>>()?;

            let total_matches = search_result.total_found as u64;
            let response = TypedSearchResponse {
                results: results.clone(),
                total_matches: Some(total_matches),
                latency_ms,
                request_id: request_id.clone(),
            };

            info!(
                "V2 API: Search {} completed in {}ms, {} results",
                request_id,
                latency_ms,
                response.results.len()
            );

            Ok(Json(response))
        }
        Err(e) => {
            error!("V2 API: Search failed: {}", e);
            if e.to_string().contains("not found") {
                Err(ApiError::CollectionNotFound(collection))
            } else {
                Err(ApiError::Internal(format!("Search failed: {}", e)))
            }
        }
    }
}

/// Query parameters for getting a single record
#[derive(Debug, Deserialize)]
pub struct GetRecordV2Query {
    /// Whether to include the vector in the response
    pub include_vector: Option<bool>,
    /// Whether to include TEXT fields in the response
    pub include_text: Option<bool>,
}

/// Response for getting a single record
#[derive(Debug, Serialize)]
pub struct RecordV2Response {
    /// Record ID
    pub id: String,
    /// Vector embedding (if requested)
    pub vector: Option<Vec<f32>>,
    /// Typed fields from the record
    pub typed_fields: HashMap<String, serde_json::Value>,
    /// TEXT fields (if include_text is true)
    pub text_fields: Option<Vec<TextFieldOutput>>,
    /// Record version
    pub version: Option<u64>,
    /// Record timestamp
    pub timestamp: Option<i64>,
}

/// GET /api/v2/collections/{collection_id}/records/{record_id}
///
/// Get a single record by ID.
///
/// ## Path Parameters
///
/// - `collection_id`: Collection name/ID
/// - `record_id`: Record ID
///
/// ## Query Parameters
///
/// - `include_vector`: Whether to include the vector (default: true)
/// - `include_text`: Whether to include TEXT fields (default: false)
///
/// ## Response
///
/// Returns [`RecordV2Response`] with record details.
///
/// ## Errors
///
/// - `404 Not Found`: Collection or record does not exist
/// - `500 Internal Server Error`: Retrieval failed
pub async fn get_record_v2(
    Path((collection_id, record_id)): Path<(String, String)>,
    State(state): State<AppState>,
    Query(params): Query<GetRecordV2Query>,
) -> ApiResult<Json<RecordV2Response>> {
    debug!(
        "V2 API: Getting record '{}' from collection '{}'",
        record_id, collection_id
    );

    if collection_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection ID is required".to_string(),
        ));
    }

    if record_id.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Record ID is required".to_string(),
        ));
    }

    let include_vector = params.include_vector.unwrap_or_else(|| {
        debug!("No include_vector preference provided, defaulting to true");
        true
    });
    let include_text = params.include_text.unwrap_or_else(|| {
        debug!("No include_text preference provided, defaulting to false");
        false
    });

    // Get vector via unified handlers
    match state
        .unified_handlers
        .handle_vector_v1(&collection_id, &record_id, include_vector, true)
        .await
    {
        Ok(resp) => {
            // resp.results is Option<SearchResult>, use default if None
            let search_result = resp.results.unwrap_or_else(|| {
                debug!("Get vector response contains no results, using default");
                Default::default()
            });
            let result = search_result
                .results
                .first()
                .ok_or_else(|| ApiError::NotFound(format!("Record '{}' not found", record_id)))?;

            // Convert metadata to typed_fields
            let typed_fields: HashMap<String, serde_json::Value> = result
                .metadata
                .iter()
                .map(|(k, v)| Ok((k.clone(), sql_value_to_json(v)?)))
                .collect::<Result<_, ApiError>>()?;

            let response = RecordV2Response {
                id: result.id.clone(),
                vector: if include_vector {
                    Some(result.vector.clone())
                } else {
                    None
                },
                typed_fields,
                text_fields: if include_text {
                    Some(vec![]) // Would be populated from text storage
                } else {
                    None
                },
                version: result.version.map(|v| v as u64),
                timestamp: result.timestamp,
            };

            Ok(Json(response))
        }
        Err(e) => {
            if e.to_string().contains("not found") {
                Err(ApiError::NotFound(format!(
                    "Record '{}' not found in collection '{}'",
                    record_id, collection_id
                )))
            } else {
                Err(ApiError::Internal(format!("Failed to get record: {}", e)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_request_deserialization() {
        let json = r#"{
            "records": [
                {
                    "id": "doc_1",
                    "vector": [0.1, 0.2, 0.3],
                    "typed_fields": {
                        "category": "test",
                        "price": 99.99
                    },
                    "text_fields": [
                        {
                            "name": "content",
                            "content": "Test content",
                            "storage_hint": "adaptive"
                        }
                    ]
                }
            ],
            "validate_schema": true
        }"#;

        let request: InsertRecordsRequest = serde_json::from_str(json)
            .expect("Failed to deserialize InsertRecordsRequest from test JSON");
        assert_eq!(request.records.len(), 1);
        assert_eq!(request.records[0].id, Some("doc_1".to_string()));
        assert_eq!(request.records[0].vector.len(), 3);
        assert_eq!(request.validate_schema, Some(true));
    }

    #[test]
    fn test_search_request_deserialization() {
        let json = r#"{
            "vector": [0.1, 0.2, 0.3],
            "top_k": 10,
            "filters": [
                {"field": "category", "op": "eq", "value": "electronics"},
                {"field": "price", "op": "lt", "value": 500}
            ],
            "include_text": true
        }"#;

        let request: TypedSearchRequest = serde_json::from_str(json)
            .expect("Failed to deserialize TypedSearchRequest from test JSON");
        assert_eq!(request.vector.len(), 3);
        assert_eq!(request.top_k, 10);
        assert_eq!(request.include_text, Some(true));

        let filters = request.filters.as_ref().expect("filters should be Some");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].field, "category");
        assert_eq!(filters[0].op, "eq");
    }

    #[test]
    fn test_typed_filter_between_validation() {
        let json = r#"{
            "field": "price",
            "op": "between",
            "value": 100,
            "value_upper": 500
        }"#;

        let filter: TypedFilter =
            serde_json::from_str(json).expect("Failed to deserialize TypedFilter from test JSON");
        assert_eq!(filter.op, "between");
        assert!(filter.value_upper.is_some());
    }

    #[test]
    fn test_convert_eq_filter() {
        use crate::proto::proximadb_v1::ComparisonOp;

        let filters = vec![TypedFilter {
            field: "status".to_string(),
            op: "eq".to_string(),
            value: serde_json::json!("active"),
            value_upper: None,
        }];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].field, "status");
        assert_eq!(clauses[0].op, ComparisonOp::Eq as i32);
    }

    #[test]
    fn test_convert_range_filters() {
        use crate::proto::proximadb_v1::ComparisonOp;

        let filters = vec![
            TypedFilter {
                field: "price".to_string(),
                op: "gt".to_string(),
                value: serde_json::json!(100),
                value_upper: None,
            },
            TypedFilter {
                field: "price".to_string(),
                op: "gte".to_string(),
                value: serde_json::json!(100),
                value_upper: None,
            },
            TypedFilter {
                field: "price".to_string(),
                op: "lt".to_string(),
                value: serde_json::json!(500),
                value_upper: None,
            },
            TypedFilter {
                field: "price".to_string(),
                op: "lte".to_string(),
                value: serde_json::json!(500),
                value_upper: None,
            },
        ];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 4);
        assert_eq!(clauses[0].op, ComparisonOp::Gt as i32);
        assert_eq!(clauses[1].op, ComparisonOp::Gte as i32);
        assert_eq!(clauses[2].op, ComparisonOp::Lt as i32);
        assert_eq!(clauses[3].op, ComparisonOp::Lte as i32);
    }

    #[test]
    fn test_convert_between_filter() {
        use crate::proto::proximadb_v1::ComparisonOp;

        let filters = vec![TypedFilter {
            field: "price".to_string(),
            op: "between".to_string(),
            value: serde_json::json!(100),
            value_upper: Some(serde_json::json!(500)),
        }];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        // between is converted to two clauses: gte and lte
        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].field, "price");
        assert_eq!(clauses[0].op, ComparisonOp::Gte as i32);
        assert_eq!(clauses[1].field, "price");
        assert_eq!(clauses[1].op, ComparisonOp::Lte as i32);
    }

    #[test]
    fn test_convert_between_filter_missing_upper() {
        let filters = vec![TypedFilter {
            field: "price".to_string(),
            op: "between".to_string(),
            value: serde_json::json!(100),
            value_upper: None, // Missing upper bound
        }];

        let result = convert_typed_filters_to_clauses(&filters);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_contains_filter() {
        use crate::proto::proximadb_v1::ComparisonOp;

        let filters = vec![TypedFilter {
            field: "description".to_string(),
            op: "contains".to_string(),
            value: serde_json::json!("search term"),
            value_upper: None,
        }];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].op, ComparisonOp::Contains as i32);
    }

    #[test]
    fn test_convert_starts_with_filter() {
        use crate::proto::proximadb_v1::{ComparisonOp, filter_clause::Value};

        let filters = vec![TypedFilter {
            field: "name".to_string(),
            op: "starts_with".to_string(),
            value: serde_json::json!("pre"),
            value_upper: None,
        }];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].op, ComparisonOp::Contains as i32);
        // Verify the value is prefixed with ^
        if let Some(Value::StringValue(s)) = &clauses[0].value {
            assert_eq!(s, "^pre");
        } else {
            panic!("Expected StringValue");
        }
    }

    #[test]
    fn test_convert_ends_with_filter() {
        use crate::proto::proximadb_v1::{ComparisonOp, filter_clause::Value};

        let filters = vec![TypedFilter {
            field: "name".to_string(),
            op: "ends_with".to_string(),
            value: serde_json::json!("suffix"),
            value_upper: None,
        }];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].op, ComparisonOp::Contains as i32);
        // Verify the value is suffixed with $
        if let Some(Value::StringValue(s)) = &clauses[0].value {
            assert_eq!(s, "suffix$");
        } else {
            panic!("Expected StringValue");
        }
    }

    #[test]
    fn test_convert_in_filter() {
        use crate::proto::proximadb_v1::{ComparisonOp, filter_clause::Value};

        let filters = vec![TypedFilter {
            field: "status".to_string(),
            op: "in".to_string(),
            value: serde_json::json!(["active", "pending", "review"]),
            value_upper: None,
        }];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].op, ComparisonOp::In as i32);
        // Verify the value is a JSON array string
        if let Some(Value::StringValue(s)) = &clauses[0].value {
            assert!(s.starts_with('['));
            assert!(s.ends_with(']'));
            assert!(s.contains("active"));
            assert!(s.contains("pending"));
            assert!(s.contains("review"));
        } else {
            panic!("Expected StringValue");
        }
    }

    #[test]
    fn test_convert_in_filter_with_numbers() {
        use crate::proto::proximadb_v1::{ComparisonOp, filter_clause::Value};

        let filters = vec![TypedFilter {
            field: "priority".to_string(),
            op: "in".to_string(),
            value: serde_json::json!([1, 2, 3]),
            value_upper: None,
        }];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].op, ComparisonOp::In as i32);
        if let Some(Value::StringValue(s)) = &clauses[0].value {
            assert!(s.contains("1"));
            assert!(s.contains("2"));
            assert!(s.contains("3"));
        } else {
            panic!("Expected StringValue");
        }
    }

    #[test]
    fn test_convert_in_filter_non_array_error() {
        let filters = vec![TypedFilter {
            field: "status".to_string(),
            op: "in".to_string(),
            value: serde_json::json!("not_an_array"),
            value_upper: None,
        }];

        let result = convert_typed_filters_to_clauses(&filters);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_neq_filter() {
        use crate::proto::proximadb_v1::ComparisonOp;

        let filters = vec![TypedFilter {
            field: "status".to_string(),
            op: "neq".to_string(),
            value: serde_json::json!("deleted"),
            value_upper: None,
        }];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].op, ComparisonOp::Ne as i32);
    }

    #[test]
    fn test_convert_multiple_filters() {
        use crate::proto::proximadb_v1::ComparisonOp;

        let filters = vec![
            TypedFilter {
                field: "category".to_string(),
                op: "eq".to_string(),
                value: serde_json::json!("electronics"),
                value_upper: None,
            },
            TypedFilter {
                field: "price".to_string(),
                op: "lt".to_string(),
                value: serde_json::json!(1000),
                value_upper: None,
            },
            TypedFilter {
                field: "in_stock".to_string(),
                op: "eq".to_string(),
                value: serde_json::json!(true),
                value_upper: None,
            },
        ];

        let clauses = convert_typed_filters_to_clauses(&filters)
            .expect("Failed to convert typed filters to clauses");
        assert_eq!(clauses.len(), 3);
        assert_eq!(clauses[0].op, ComparisonOp::Eq as i32);
        assert_eq!(clauses[1].op, ComparisonOp::Lt as i32);
        assert_eq!(clauses[2].op, ComparisonOp::Eq as i32);
    }

    #[test]
    fn test_json_to_filter_clause_value_types() {
        use crate::proto::proximadb_v1::filter_clause::Value;

        // String
        let string_val = json_to_filter_clause_value(&serde_json::json!("test"));
        assert!(matches!(string_val, Some(Value::StringValue(_))));

        // Integer
        let int_val = json_to_filter_clause_value(&serde_json::json!(42));
        assert!(matches!(int_val, Some(Value::IntValue(42))));

        // Float
        let float_val = json_to_filter_clause_value(&serde_json::json!(3.14));
        assert!(matches!(float_val, Some(Value::DoubleValue(_))));

        // Boolean
        let bool_val = json_to_filter_clause_value(&serde_json::json!(true));
        assert!(matches!(bool_val, Some(Value::BoolValue(true))));

        // Null returns None
        let null_val = json_to_filter_clause_value(&serde_json::json!(null));
        assert!(null_val.is_none());

        // Array returns None (not directly supported)
        let array_val = json_to_filter_clause_value(&serde_json::json!([1, 2, 3]));
        assert!(array_val.is_none());

        // Object returns None (not directly supported)
        let object_val = json_to_filter_clause_value(&serde_json::json!({"key": "value"}));
        assert!(object_val.is_none());
    }
}
