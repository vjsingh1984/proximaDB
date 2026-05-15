//! # Collection and Catalog Handlers
//!
//! REST endpoints for vector collection lifecycle management.  All handler functions
//! delegate to `ApiHandlersPort` so this module compiles without any dependency on
//! root-crate concrete service types.
//!
//! External catalog management (Iceberg, Delta Lake, AWS Glue) is tracked via the
//! `CatalogHandler` stub but the implementation remains in `src/network/rest/v1/catalog.rs`
//! until a `CatalogPort` trait is defined in `proximadb-runtime`.

use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use proximadb_proto::v1::{CollectionOperation, CollectionRequest};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::rest::errors::{RestError, RestResult};
use crate::rest::state::{RestAppState, TenantContext};

// ── Legacy stub types kept for re-export compatibility ────────────────────────

/// External catalog handler stub.  Full implementation lives in the root crate
/// until a `CatalogPort` is defined in `proximadb-runtime`.
pub struct CatalogHandler;

impl CatalogHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CatalogHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Collection lifecycle handler stub.
pub struct CollectionHandler;

impl CollectionHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CollectionHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Request / Response types ──────────────────────────────────────────────────

/// Simple collection creation / update request (REST-only body shape).
#[derive(Debug, Deserialize)]
pub struct CreateCollectionRequest {
    pub name: String,
    pub dimension: Option<usize>,
    pub metric: Option<String>,
}

/// Simple collection response body shape.
#[derive(Debug, Serialize)]
pub struct CollectionResponse {
    pub name: String,
    pub dimension: usize,
    pub metric: String,
}

/// Query parameters for the `GET /api/v1/collections` list endpoint.
#[derive(Debug, Deserialize)]
pub struct ListCollectionsQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub include_stats: Option<bool>,
}

// ── Prost-enum normalisation helper ──────────────────────────────────────────

/// Normalise `distance_metric` and `storage_engine` enum values from the raw JSON.
///
/// The prost-derived `Deserialize` cannot handle string enum names sent by the Python
/// SDK ("manhattan", "viper", …) — it expects integer wire values.  This function
/// reads the raw JSON and back-patches the already-parsed `CollectionRequest` so the
/// enum integers match the proto-defined constants regardless of whether the client
/// sent a string name or an integer.
fn apply_proto_enum_workarounds(
    request: &mut CollectionRequest,
    raw: &serde_json::Value,
) {
    let Some(ref mut config) = request.collection_config else {
        return;
    };

    // distance_metric: try both wrapped and flat JSON paths
    let dm_paths = [
        raw.get("collection_config")
            .and_then(|v| v.get("distance_metric")),
        raw.get("distance_metric"),
    ];
    for dm_value in dm_paths.iter().filter_map(|&v| v) {
        let int = if let Some(s) = dm_value.as_str() {
            match s {
                "unspecified" => 0,
                "cosine" => 1,
                "euclidean" => 2,
                "dot_product" => 3,
                "hamming" => 4,
                "manhattan" => 5,
                "jaccard" => 6,
                "angular" => 7,
                "chebyshev" => 8,
                "canberra" => 9,
                "minkowski" => 10,
                "bray_curtis" => 11,
                "hellinger" => 12,
                "custom" => 13,
                _ => 1,
            }
        } else if let Some(i) = dm_value.as_i64() {
            i as i32
        } else if let Some(u) = dm_value.as_u64() {
            u as i32
        } else {
            continue;
        };
        config.distance_metric = Some(int);
        break;
    }

    // storage_engine: try both wrapped and flat JSON paths
    let se_paths = [
        raw.get("collection_config")
            .and_then(|v| v.get("storage_engine")),
        raw.get("storage_engine"),
    ];
    for se_value in se_paths.iter().filter_map(|&v| v) {
        let int = if let Some(s) = se_value.as_str() {
            match s {
                "unspecified" => 0,
                "viper" => 1,
                "sst" => 2,
                "nova" => 3,
                "helix" => 4,
                "swift" => 5,
                "raptor" => 6,
                "mmap" => 7,
                "hybrid" => 8,
                "tst" => 9,
                "cedar" => 10,
                "titan" => 11,
                "chrono" => 12,
                _ => 2,
            }
        } else if let Some(i) = se_value.as_i64() {
            i as i32
        } else if let Some(u) = se_value.as_u64() {
            u as i32
        } else {
            continue;
        };
        config.storage_engine = Some(int);
        break;
    }
}

// ── Handler functions ──────────────────────────────────────────────────────────

/// `POST /api/v1/collections` — create or operate on a collection.
///
/// Accepts a full proto `CollectionRequest` JSON body.  Applies enum-string normalisation
/// for Python SDK compatibility before delegating to `ApiHandlersPort`.
pub async fn collection_operation(
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> RestResult<Json<proximadb_proto::v1::CollectionResponse>> {
    let mut request: CollectionRequest = serde_json::from_value(value.clone()).map_err(|e| {
        RestError::InvalidArgument(format!("Invalid request format: {}", e))
    })?;

    apply_proto_enum_workarounds(&mut request, &value);

    let operation = CollectionOperation::try_from(request.operation).map_err(|_| {
        RestError::InvalidArgument("Invalid collection operation".to_string())
    })?;

    info!(
        "Collection operation {:?} for {:?}, tenant='{}'",
        operation, request.collection_id, tenant.tenant_id
    );

    state
        .handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
        .map(Json)
        .map_err(|e| RestError::Internal(e.to_string()))
}

/// `GET /api/v1/collections/:collection_id` — fetch a single collection.
pub async fn get_collection(
    Path(collection_id): Path<String>,
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
) -> impl IntoResponse {
    debug!("Get collection '{}', tenant='{}'", collection_id, tenant.tenant_id);

    if collection_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "collection_id is required").into_response();
    }

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionGet as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    match state
        .handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, format!("Collection not found: {}", collection_id))
                    .into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
            }
        }
    }
}

/// `GET /api/v1/collections` — list collections with optional pagination.
pub async fn list_collections(
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
    Query(params): Query<ListCollectionsQuery>,
) -> impl IntoResponse {
    debug!("List collections, tenant='{}'", tenant.tenant_id);

    let mut query_params = std::collections::HashMap::new();
    if let Some(limit) = params.limit {
        query_params.insert("limit".to_string(), limit.to_string());
    }
    if let Some(offset) = params.offset {
        query_params.insert("offset".to_string(), offset.to_string());
    }

    let mut options = std::collections::HashMap::new();
    if let Some(include_stats) = params.include_stats {
        options.insert("include_stats".to_string(), include_stats);
    }

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionList as i32,
        collection_id: None,
        collection_config: None,
        query_params,
        options,
        migration_config: Default::default(),
    };

    match state
        .handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `DELETE /api/v1/collections/:collection_id` — delete a collection.
pub async fn delete_collection(
    Path(collection_id): Path<String>,
    State(state): State<RestAppState>,
    Extension(tenant): Extension<TenantContext>,
) -> impl IntoResponse {
    info!("Delete collection '{}', tenant='{}'", collection_id, tenant.tenant_id);

    if collection_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "collection_id is required").into_response();
    }

    let request = CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_id.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    };

    match state
        .handlers
        .handle_collection_operation_for_tenant(request, Some(&tenant.tenant_id))
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, format!("Collection not found: {}", collection_id))
                    .into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
            }
        }
    }
}

// ── Router configuration ──────────────────────────────────────────────────────

/// Build the collection lifecycle router.
pub fn create_collection_router() -> Router<RestAppState> {
    Router::new()
        .route("/api/v1/collections", post(collection_operation).get(list_collections))
        .route(
            "/api/v1/collections/:collection_id",
            get(get_collection).delete(delete_collection),
        )
}
