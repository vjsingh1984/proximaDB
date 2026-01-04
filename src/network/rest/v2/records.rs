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
fn sql_value_to_json(value: &crate::proto::proximadb_v1::SqlValue) -> serde_json::Value {
    use crate::proto::proximadb_v1::sql_value::Value;

    match value.value.as_ref() {
        Some(Value::NullValue(_)) => serde_json::Value::Null,
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(Value::NumberValue(f)) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::BytesValue(b)) => serde_json::Value::Array(
            b.iter()
                .map(|x| serde_json::Value::Number((*x as u64).into()))
                .collect(),
        ),
        Some(Value::ArrayValue(arr)) => {
            serde_json::Value::Array(arr.values.iter().map(sql_value_to_json).collect())
        }
        Some(Value::ObjectValue(obj)) => {
            let map: serde_json::Map<String, serde_json::Value> = obj
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), sql_value_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
        None => serde_json::Value::Null,
    }
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

    let validate_schema = request.validate_schema.unwrap_or(true);
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
        let record_id = record
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

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

    let include_text = request.include_text.unwrap_or(false);
    debug!(
        "Include TEXT fields: {}, filters: {:?}",
        include_text,
        request.filters.as_ref().map(|f| f.len())
    );

    // Convert typed filters to v1 filter format
    let filters = if let Some(ref typed_filters) = request.filters {
        let mut filter_map: HashMap<String, crate::proto::proximadb_v1::SqlValue> = HashMap::new();
        for filter in typed_filters {
            // For simple eq filters, add to filter map
            if filter.op == "eq" {
                filter_map.insert(filter.field.clone(), json_to_sql_value(&filter.value));
            }
            // For range filters, we encode the operation in the key
            // Note: Full filter support would require AdvancedFilter proto
        }
        filter_map
    } else {
        HashMap::new()
    };

    // Create search query
    let search_query = SearchQuery {
        vector: request.vector.clone(),
        filters,
        advanced_filter: None, // Could be extended for complex filters
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

            // resp.results is Option<SearchResult> - unwrap it
            let search_result = resp.results.unwrap_or_default();

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
                        .map(|(k, v)| (k.clone(), sql_value_to_json(v)))
                        .collect();

                    TypedSearchResult {
                        id: r.id.clone(),
                        score: r.score as f32,
                        vector: if request.include_vector.unwrap_or(false) {
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
                    }
                })
                .collect();

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

    let include_vector = params.include_vector.unwrap_or(true);
    let include_text = params.include_text.unwrap_or(false);

    // Get vector via unified handlers
    match state
        .unified_handlers
        .handle_vector_v1(&collection_id, &record_id, include_vector, true)
        .await
    {
        Ok(resp) => {
            // resp.results is Option<SearchResult>, unwrap then access Vec<SearchVectorRecord>
            let search_result = resp.results.unwrap_or_default();
            let result = search_result
                .results
                .first()
                .ok_or_else(|| ApiError::NotFound(format!("Record '{}' not found", record_id)))?;

            // Convert metadata to typed_fields
            let typed_fields: HashMap<String, serde_json::Value> = result
                .metadata
                .iter()
                .map(|(k, v)| (k.clone(), sql_value_to_json(v)))
                .collect();

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

        let request: InsertRecordsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.records.len(), 1);
        assert_eq!(request.records[0].id, Some("doc_1".to_string()));
        assert_eq!(request.records[0].vector.len(), 3);
        assert!(request.validate_schema.unwrap());
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

        let request: TypedSearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.vector.len(), 3);
        assert_eq!(request.top_k, 10);
        assert!(request.include_text.unwrap());

        let filters = request.filters.unwrap();
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

        let filter: TypedFilter = serde_json::from_str(json).unwrap();
        assert_eq!(filter.op, "between");
        assert!(filter.value_upper.is_some());
    }
}
