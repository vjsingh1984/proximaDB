// Document API REST handlers
//
// REST API for MongoDB-like document operations:
// - CRUD operations on JSON documents
// - Document queries with JSON path filtering
// - Index management

use axum::{
    extract::{Json, Path, Query, State},
    response::Json as JsonResponse,
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::errors::{ApiError, ApiResult};
use crate::proto::proximadb_v1::{SqlObject, SqlValue, IndexDefinition, DocIndexType, DocumentCollectionConfig};
use crate::storage::document::{DocumentService, DocumentRecord, DocumentQueryParams as ServiceQueryParams};

/// Document API state
#[derive(Clone)]
pub struct DocumentApiState {
    /// Document service
    pub document_service: Arc<DocumentService>,
}

/// Query parameters for document operations
#[derive(Debug, Deserialize)]
pub struct DocumentQueryParams {
    /// Fields to return (projection)
    #[serde(default)]
    pub projection: Option<String>,
    /// Maximum results
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    100
}

/// Create document request
#[derive(Debug, Deserialize)]
pub struct CreateDocumentRequest {
    /// Document ID (optional, will be generated if not provided)
    pub id: Option<String>,
    /// Document content
    pub document: serde_json::Value,
}

/// Create collection request
#[derive(Debug, Deserialize)]
pub struct CreateCollectionRequest {
    /// Collection name
    pub name: String,
    /// Initial indexes to create
    #[serde(default)]
    pub indexes: Vec<IndexDefinitionRequest>,
}

/// Index definition request
#[derive(Debug, Clone, Deserialize)]
pub struct IndexDefinitionRequest {
    /// Index name
    pub name: Option<String>,
    /// Path to index
    pub path: String,
    /// Index type (btree, hash, inverted, fulltext)
    #[serde(default = "default_index_type")]
    pub index_type: String,
    /// Unique constraint
    #[serde(default)]
    pub unique: bool,
    /// Sparse index (skip null values)
    #[serde(default)]
    pub sparse: bool,
}

fn default_index_type() -> String {
    "btree".to_string()
}

/// Document response
#[derive(Debug, Serialize)]
pub struct DocumentResponse {
    /// Document ID
    pub id: String,
    /// Document content
    pub document: serde_json::Value,
    /// Version
    pub version: u64,
    /// Updated timestamp (ns)
    pub updated_at_ns: i64,
}

/// Query response
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    /// Documents matching the query
    pub documents: Vec<DocumentResponse>,
    /// Total count (if available)
    pub total_count: Option<u64>,
    /// Whether there are more results
    pub has_more: bool,
}

/// Create document collection router
pub fn create_document_router() -> Router<DocumentApiState> {
    Router::new()
        // Collection operations
        .route("/collections", post(create_collection))
        .route("/collections", get(list_collections))
        .route("/collections/:collection", get(get_collection_info))
        .route("/collections/:collection", delete(delete_collection))
        // Document CRUD
        .route("/collections/:collection/documents", post(insert_document))
        .route("/collections/:collection/documents", get(query_documents))
        .route("/collections/:collection/documents/:id", get(get_document))
        .route("/collections/:collection/documents/:id", delete(delete_document))
        // Index operations
        .route("/collections/:collection/indexes", post(create_index))
        .route("/collections/:collection/indexes", get(list_indexes))
}

/// Create a document collection
async fn create_collection(
    State(state): State<DocumentApiState>,
    Json(request): Json<CreateCollectionRequest>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    info!("Creating document collection: {}", request.name);

    // Create indexes from request
    let index_definitions: Vec<IndexDefinition> = request
        .indexes
        .into_iter()
        .map(|idx| IndexDefinition {
            name: idx.name,
            path: idx.path,
            index_type: match idx.index_type.to_lowercase().as_str() {
                "btree" => DocIndexType::Btree as i32,
                "hash" => DocIndexType::Hash as i32,
                "inverted" => DocIndexType::Inverted as i32,
                "fulltext" => DocIndexType::Fulltext as i32,
                "geo" => DocIndexType::Geo as i32,
                _ => DocIndexType::Btree as i32,
            },
            unique: idx.unique,
            sparse: idx.sparse,
        })
        .collect();

    // Create collection config
    let config = DocumentCollectionConfig {
        name: request.name.clone(),
        json_schema: None,
        indexes: index_definitions,
        enable_fulltext: false,
        fulltext_paths: Vec::new(),
        ttl_seconds: 0, // No expiry by default
        compression: None,
    };

    state
        .document_service
        .create_collection(&request.name, config)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create collection: {}", e)))?;

    Ok(JsonResponse(serde_json::json!({
        "success": true,
        "collection": request.name
    })))
}

/// List document collections
async fn list_collections(
    State(state): State<DocumentApiState>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    let collections = state
        .document_service
        .list_collections()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list collections: {}", e)))?;

    // Convert to serializable format
    let collection_info: Vec<serde_json::Value> = collections
        .iter()
        .map(|c| serde_json::json!({
            "name": c.name,
            "document_count": c.document_count,
            "storage_size_bytes": c.storage_size_bytes
        }))
        .collect();

    Ok(JsonResponse(serde_json::json!({
        "collections": collection_info
    })))
}

/// Get collection info
async fn get_collection_info(
    State(state): State<DocumentApiState>,
    Path(collection): Path<String>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    let info = state
        .document_service
        .get_collection(&collection)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to get collection: {}", e)))?
        .ok_or_else(|| ApiError::CollectionNotFound(collection.clone()))?;

    Ok(JsonResponse(serde_json::json!({
        "name": info.name,
        "document_count": info.document_count,
        "indexes": info.indexes.iter().map(|i| serde_json::json!({
            "name": i.name,
            "path": i.path,
            "unique": i.unique
        })).collect::<Vec<_>>()
    })))
}

/// Delete a collection
async fn delete_collection(
    State(state): State<DocumentApiState>,
    Path(collection): Path<String>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    info!("Deleting document collection: {}", collection);

    state
        .document_service
        .delete_collection(&collection)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete collection: {}", e)))?;

    Ok(JsonResponse(serde_json::json!({
        "success": true
    })))
}

/// Insert a document
async fn insert_document(
    State(state): State<DocumentApiState>,
    Path(collection): Path<String>,
    Json(request): Json<CreateDocumentRequest>,
) -> ApiResult<JsonResponse<DocumentResponse>> {
    debug!("Inserting document into collection: {}", collection);

    // Convert JSON to SqlObject
    let sql_object = json_to_sql_object(&request.document)?;

    // Pass id as Option<&str>
    let id_ref = request.id.as_deref();

    let record = state
        .document_service
        .insert_document(&collection, id_ref, sql_object)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to insert document: {}", e)))?;

    Ok(JsonResponse(document_record_to_response(&record)?))
}

/// Get a document by ID
async fn get_document(
    State(state): State<DocumentApiState>,
    Path((collection, id)): Path<(String, String)>,
    Query(params): Query<DocumentQueryParams>,
) -> ApiResult<JsonResponse<DocumentResponse>> {
    debug!("Getting document: {}/{}", collection, id);

    let projection = params.projection.map(|p| {
        p.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
    });

    let record = state
        .document_service
        .get_document(&collection, &id, projection)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to get document: {}", e)))?
        .ok_or_else(|| ApiError::CollectionNotFound(format!("Document {} not found", id)))?;

    Ok(JsonResponse(document_record_to_response(&record)?))
}

/// Delete a document
async fn delete_document(
    State(state): State<DocumentApiState>,
    Path((collection, id)): Path<(String, String)>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    debug!("Deleting document: {}/{}", collection, id);

    state
        .document_service
        .delete_document(&collection, &id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete document: {}", e)))?;

    Ok(JsonResponse(serde_json::json!({
        "success": true,
        "id": id
    })))
}

/// Query documents
async fn query_documents(
    State(state): State<DocumentApiState>,
    Path(collection): Path<String>,
    Query(params): Query<DocumentQueryParams>,
) -> ApiResult<JsonResponse<QueryResponse>> {
    debug!("Querying documents in collection: {}", collection);

    // Build service query params
    let projection = params.projection.map(|p| {
        p.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
    }).unwrap_or_default();

    let query_params = ServiceQueryParams {
        filter: None,
        projection,
        sort: Vec::new(),
        limit: params.limit,
        offset: 0,
        include_count: true,
    };

    let result = state
        .document_service
        .query_documents(&collection, query_params)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query documents: {}", e)))?;

    let documents: Vec<DocumentResponse> = result
        .documents
        .iter()
        .map(document_record_to_response)
        .collect::<Result<Vec<_>, _>>()?;

    let has_more = documents.len() >= params.limit as usize;

    Ok(JsonResponse(QueryResponse {
        documents,
        total_count: result.total_count,
        has_more,
    }))
}

/// Create an index
/// Note: Indexes are created during collection creation via the config.
/// This endpoint is for adding indexes to existing collections (TODO: implement)
async fn create_index(
    State(_state): State<DocumentApiState>,
    Path(collection): Path<String>,
    Json(request): Json<IndexDefinitionRequest>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    info!("Creating index on {}: {:?}", collection, request.path);

    // TODO: Implement adding indexes to existing collections
    // For now, indexes must be specified at collection creation time
    Err(ApiError::InvalidArgument(
        "Creating indexes on existing collections is not yet supported. Specify indexes when creating the collection.".to_string()
    ))
}

/// List indexes
async fn list_indexes(
    State(state): State<DocumentApiState>,
    Path(collection): Path<String>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    let info = state
        .document_service
        .get_collection(&collection)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to get collection: {}", e)))?
        .ok_or_else(|| ApiError::CollectionNotFound(collection.clone()))?;

    Ok(JsonResponse(serde_json::json!({
        "indexes": info.indexes.iter().map(|i| serde_json::json!({
            "name": i.name,
            "path": i.path,
            "unique": i.unique
        })).collect::<Vec<_>>()
    })))
}

// Helper functions

/// Convert JSON Value to SqlObject
fn json_to_sql_object(value: &serde_json::Value) -> ApiResult<SqlObject> {
    match value {
        serde_json::Value::Object(map) => {
            let fields: HashMap<String, SqlValue> = map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_sql_value(v)))
                .collect();
            Ok(SqlObject { fields })
        }
        _ => Err(ApiError::InvalidArgument(
            "Document must be a JSON object".to_string(),
        )),
    }
}

/// Convert JSON Value to SqlValue
fn json_to_sql_value(value: &serde_json::Value) -> SqlValue {
    use crate::proto::proximadb_v1::sql_value::Value;
    use crate::proto::proximadb_v1::SqlArray;

    let inner = match value {
        serde_json::Value::Null => Some(Value::NullValue(0)),
        serde_json::Value::Bool(b) => Some(Value::BoolValue(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Value::Int64Value(i))
            } else if let Some(f) = n.as_f64() {
                Some(Value::NumberValue(f))
            } else {
                Some(Value::StringValue(n.to_string()))
            }
        }
        serde_json::Value::String(s) => Some(Value::StringValue(s.clone())),
        serde_json::Value::Array(arr) => {
            let values: Vec<SqlValue> = arr.iter().map(json_to_sql_value).collect();
            Some(Value::ArrayValue(SqlArray { values }))
        }
        serde_json::Value::Object(map) => {
            let fields: HashMap<String, SqlValue> = map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_sql_value(v)))
                .collect();
            Some(Value::ObjectValue(SqlObject { fields }))
        }
    };

    SqlValue { value: inner }
}

/// Convert SqlValue to JSON
fn sql_value_to_json(value: &SqlValue) -> serde_json::Value {
    use crate::proto::proximadb_v1::sql_value::Value;

    match &value.value {
        Some(Value::NullValue(_)) => serde_json::Value::Null,
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::Int64Value(i)) => serde_json::json!(*i),
        Some(Value::NumberValue(f)) => serde_json::json!(*f),
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::BytesValue(b)) => {
            // Convert bytes to hex string
            serde_json::Value::String(b.iter().map(|byte| format!("{:02x}", byte)).collect())
        }
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

/// Convert SqlObject to JSON
fn sql_object_to_json(obj: &SqlObject) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = obj
        .fields
        .iter()
        .map(|(k, v)| (k.clone(), sql_value_to_json(v)))
        .collect();
    serde_json::Value::Object(map)
}

/// Convert DocumentRecord to response
fn document_record_to_response(record: &DocumentRecord) -> ApiResult<DocumentResponse> {
    Ok(DocumentResponse {
        id: record.id.clone(),
        document: sql_object_to_json(&record.document),
        version: record.version,
        updated_at_ns: record.updated_at_ns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_to_sql_value() {
        let json = serde_json::json!({"name": "test", "count": 42, "active": true});
        let obj = json_to_sql_object(&json).unwrap();
        assert!(obj.fields.contains_key("name"));
        assert!(obj.fields.contains_key("count"));
        assert!(obj.fields.contains_key("active"));
    }
}
