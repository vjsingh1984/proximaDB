//! # Vector and Entity Handlers
//!
//! REST endpoints for vector CRUD and similarity search.  All handler functions
//! delegate exclusively to `ApiHandlersPort` so this module compiles without any
//! dependency on root-crate concrete service types.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    routing::{get, post},
    Router,
};
use proximadb_proto::v1::{
    SearchQuery, VectorBatchRequest, VectorRecord, VectorSearchRequest,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::rest::errors::{RestError, RestResult};
use crate::rest::state::{RestAppState, TenantContext};

// ── Legacy stub types kept for backward-compat re-exports ─────────────────────

/// Entity handler stub (kept for re-export compatibility).
pub struct EntityHandler;

impl EntityHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EntityHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Vector handler stub (kept for re-export compatibility).
pub struct VectorHandler;

impl VectorHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VectorHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Request / Response types ──────────────────────────────────────────────────

/// Query parameters for the `GET /api/v1/vectors/:collection_id/:vector_id` endpoint.
#[derive(Debug, Deserialize)]
pub struct GetVectorParams {
    pub include_vector: Option<bool>,
    pub include_metadata: Option<bool>,
}

/// Serde-compatible entity data (for callers that prefer the simple REST format).
#[derive(Debug, Deserialize, Serialize)]
pub struct EntityData {
    pub id: Option<String>,
    pub vector: Option<Vec<f32>>,
    pub properties: Option<serde_json::Value>,
}

// ── Parse helpers (pure functions, no port deps) ──────────────────────────────

/// Parse a vector search request from JSON.
///
/// Supports both the proto format
/// `{ "collection_id": "...", "queries": [{…}], "top_k": 10 }`
/// and the simple SDK-friendly format
/// `{ "collection": "...", "vector": [..], "top_k": 10 }`.
pub fn parse_search_request(value: serde_json::Value) -> Result<VectorSearchRequest, String> {
    if let Some(obj) = value.as_object() {
        let has_simple = obj.contains_key("collection") || obj.contains_key("vector");
        if has_simple {
            let collection_id = obj
                .get("collection")
                .or_else(|| obj.get("collection_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let vector: Vec<f32> = obj
                .get("vector")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect())
                .unwrap_or_default();

            let top_k = obj.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as u32;

            let filters: HashMap<String, proximadb_proto::v1::SqlValue> = obj
                .get("filters")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            return Ok(VectorSearchRequest {
                collection_id,
                queries: vec![SearchQuery {
                    vector,
                    filters,
                    advanced_filter: None,
                }],
                top_k,
                include_fields: None,
                search_params: None,
                distance_metric_override: None,
                search_optimization: None,
            });
        }
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Parse a vector batch request from JSON.
///
/// Supports both the proto format `{ "collection_id": "...", "vectors": [..] }`
/// and the simple format `{ "collection": "...", "vectors": [..] }`.
pub fn parse_batch_request(value: serde_json::Value) -> Result<VectorBatchRequest, String> {
    if let Some(obj) = value.as_object() {
        if obj.contains_key("collection") {
            let collection_id = obj
                .get("collection")
                .or_else(|| obj.get("collection_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let vectors: Vec<proximadb_proto::v1::VectorRecord> = obj
                .get("vectors")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            return Ok(VectorBatchRequest {
                collection_id,
                vectors,
            });
        }
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}

// ── Handler functions ──────────────────────────────────────────────────────────

/// `POST /api/v1/search` — vector similarity search.
///
/// Accepts both the full proto format and the simple SDK-friendly format.
pub async fn vector_search(
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> RestResult<Json<proximadb_proto::v1::VectorOperationResponse>> {
    let request = parse_search_request(value)
        .map_err(|e| RestError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    if request.collection_id.is_empty() {
        return Err(RestError::InvalidArgument("collection_id is required".to_string()));
    }

    debug!(
        "Vector search: collection='{}', tenant='{}'",
        request.collection_id, tenant.tenant_id
    );

    state
        .handlers
        .handle_vector_search_v1_for_tenant(request.clone(), Some(&tenant.tenant_id))
        .await
        .map(Json)
        .map_err(|e| {
            error!("Vector search failed for '{}': {}", request.collection_id, e);
            RestError::Internal(format!("Search failed: {}", e))
        })
}

/// `POST /api/v1/vectors/batch` — upsert/delete a batch of vectors.
pub async fn vector_batch(
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> RestResult<Json<proximadb_proto::v1::VectorOperationResponse>> {
    let request = parse_batch_request(value)
        .map_err(|e| RestError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    if request.collection_id.is_empty() {
        return Err(RestError::InvalidArgument("collection_id is required".to_string()));
    }
    if request.vectors.is_empty() {
        return Err(RestError::InvalidArgument(
            "At least one vector record is required".to_string(),
        ));
    }

    info!(
        "Vector batch: collection='{}', {} records, tenant='{}'",
        request.collection_id,
        request.vectors.len(),
        tenant.tenant_id
    );

    state
        .handlers
        .handle_vector_batch_v1_for_tenant(request, Some(&tenant.tenant_id))
        .await
        .map(Json)
        .map_err(|e| {
            error!("Vector batch failed: {}", e);
            RestError::Internal(e.to_string())
        })
}

/// `GET /api/v1/vectors/:collection_id/:vector_id` — fetch a single vector.
pub async fn get_vector(
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
    Path((collection_id, vector_id)): Path<(String, String)>,
    Query(params): Query<GetVectorParams>,
) -> RestResult<Json<proximadb_proto::v1::VectorOperationResponse>> {
    if collection_id.is_empty() || vector_id.is_empty() {
        return Err(RestError::InvalidArgument(
            "collection_id and vector_id are required".to_string(),
        ));
    }

    debug!("Get vector: collection='{}', id='{}'", collection_id, vector_id);

    state
        .handlers
        .handle_vector_v1_for_tenant(
            &collection_id,
            &vector_id,
            params.include_vector.unwrap_or(true),
            params.include_metadata.unwrap_or(true),
            Some(&tenant.tenant_id),
        )
        .await
        .map(Json)
        .map_err(|e| {
            error!("Get vector {}/{} failed: {}", collection_id, vector_id, e);
            RestError::Internal(e.to_string())
        })
}

/// `DELETE /api/v1/vectors/:collection_id/:vector_id` — delete a single vector.
///
/// Implemented as a tombstone batch upsert (expires_at = 0).
pub async fn delete_vector(
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
    Path((collection_id, vector_id)): Path<(String, String)>,
) -> RestResult<Json<proximadb_proto::v1::VectorOperationResponse>> {
    if collection_id.is_empty() || vector_id.is_empty() {
        return Err(RestError::InvalidArgument(
            "collection_id and vector_id are required".to_string(),
        ));
    }

    info!("Delete vector: collection='{}', id='{}'", collection_id, vector_id);

    let tombstone = VectorBatchRequest {
        collection_id: collection_id.clone(),
        vectors: vec![VectorRecord {
            id: vector_id.clone(),
            vector: vec![],
            metadata: HashMap::new(),
            version: None,
            timestamp: None,
            source: None,
            updated_at: None,
            expires_at: Some(0),
        }],
    };

    state
        .handlers
        .handle_vector_batch_v1_for_tenant(tombstone, Some(&tenant.tenant_id))
        .await
        .map(Json)
        .map_err(|e| {
            error!("Delete vector {}/{} failed: {}", collection_id, vector_id, e);
            RestError::Internal(e.to_string())
        })
}

/// `POST /api/v1/search/with_metadata` — vector search with metadata filtering.
pub async fn vector_search_with_metadata(
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> RestResult<Json<proximadb_proto::v1::VectorOperationResponse>> {
    let request = parse_search_request(value)
        .map_err(|e| RestError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    let request_id = Uuid::new_v4().to_string();
    let start = std::time::Instant::now();

    info!(
        "Vector search+metadata {} for collection='{}', tenant='{}'",
        request_id, request.collection_id, tenant.tenant_id
    );

    state
        .handlers
        .handle_vector_search_v1_for_tenant(request, Some(&tenant.tenant_id))
        .await
        .map(|resp| {
            info!("Search+metadata {} done in {}ms", request_id, start.elapsed().as_millis());
            Json(resp)
        })
        .map_err(|e| {
            error!("Search+metadata {} failed: {}", request_id, e);
            RestError::Internal(e.to_string())
        })
}

// ── Router configuration ──────────────────────────────────────────────────────

/// Build the vector operations router.
///
/// All routes are registered against `RestAppState`; callers `.with_state(state)` this
/// before nesting it into the main application router.
pub fn create_vector_router() -> Router<RestAppState> {
    Router::new()
        .route("/api/v1/search", post(vector_search))
        .route("/api/v1/search/with_metadata", post(vector_search_with_metadata))
        .route("/api/v1/vectors/batch", post(vector_batch))
        .route(
            "/api/v1/vectors/:collection_id/:vector_id",
            get(get_vector).delete(delete_vector),
        )
}
