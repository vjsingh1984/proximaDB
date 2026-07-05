// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! v2 REST time-series surface (TD-TS-1).
//!
//! Matches the contract the Python SDK already expects
//! (`clients/python/src/proximadb_sdk/adapters/rest_adapter.py`):
//! `POST /api/v2/timeseries/collections` (create), `.../{c}/ingest`, `.../{c}/query`,
//! `.../{c}/aggregate`, `GET /api/v2/timeseries/collections` (list),
//! `DELETE .../{c}` (delete). Backed by the process-global [`TimeSeriesService`] over
//! the native TST engine — never the stubbed vector-shaped trait methods. Tenant
//! isolation is structural (the tenant is folded into the collection key).

use axum::extract::Path;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::errors::{ApiError, ApiResult};
use crate::network::middleware::tenant::TenantContext;
use crate::services::timeseries_service::{
    TimeSeriesService, TsCollectionConfig, TsPoint, timeseries_service,
};

fn effective_collection(tenant: &TenantContext, collection_id: &str) -> String {
    if !tenant.tenant_id.is_empty() {
        format!("{}::{}", tenant.tenant_id, collection_id)
    } else {
        collection_id.to_string()
    }
}

fn service() -> ApiResult<Arc<TimeSeriesService>> {
    timeseries_service()
        .ok_or_else(|| ApiError::Internal("timeseries service is not available".to_string()))
}

#[derive(Debug, Deserialize)]
pub struct IngestRequest {
    pub points: Vec<TsPoint>,
}

#[derive(Debug, Serialize)]
pub struct IngestResponse {
    pub ingested: usize,
}

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub start_time: i64,
    pub end_time: i64,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub points: Vec<TsPoint>,
}

#[derive(Debug, Deserialize)]
pub struct AggregateRequest {
    pub start_time: i64,
    pub end_time: i64,
    #[serde(default = "default_aggregation")]
    pub aggregation: String,
    #[serde(default = "default_bucket_ms")]
    pub bucket_ms: i64,
}

fn default_aggregation() -> String {
    "avg".to_string()
}
fn default_bucket_ms() -> i64 {
    60_000
}

#[derive(Debug, Serialize)]
pub struct AggregateResponse {
    pub buckets: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub collections: Vec<TsCollectionConfig>,
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub success: bool,
}

/// `POST /api/v2/timeseries/collections`
pub async fn create_timeseries_collection(
    Extension(tenant): Extension<TenantContext>,
    Json(mut config): Json<TsCollectionConfig>,
) -> ApiResult<Json<CreateResponse>> {
    let name = effective_collection(&tenant, &config.name);
    config.name = name.clone();
    service()?
        .create_collection(config)
        .await
        .map_err(|e| ApiError::Internal(format!("create timeseries collection: {e}")))?;
    Ok(Json(CreateResponse { name }))
}

/// `POST /api/v2/timeseries/collections/{collection_id}/ingest`
pub async fn ingest_timeseries(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
    Json(request): Json<IngestRequest>,
) -> ApiResult<Json<IngestResponse>> {
    let collection = effective_collection(&tenant, &collection_id);
    let ingested = service()?
        .ingest(&collection, request.points)
        .await
        .map_err(|e| ApiError::Internal(format!("ingest timeseries: {e}")))?;
    Ok(Json(IngestResponse { ingested }))
}

/// `POST /api/v2/timeseries/collections/{collection_id}/query`
pub async fn query_timeseries(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
    Json(request): Json<QueryRequest>,
) -> ApiResult<Json<QueryResponse>> {
    let collection = effective_collection(&tenant, &collection_id);
    let points = service()?
        .query(
            &collection,
            request.start_time,
            request.end_time,
            request.limit,
        )
        .await
        .map_err(|e| ApiError::Internal(format!("query timeseries: {e}")))?;
    Ok(Json(QueryResponse { points }))
}

/// `POST /api/v2/timeseries/collections/{collection_id}/aggregate`
pub async fn aggregate_timeseries(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
    Json(request): Json<AggregateRequest>,
) -> ApiResult<Json<AggregateResponse>> {
    let collection = effective_collection(&tenant, &collection_id);
    let buckets = service()?
        .aggregate(
            &collection,
            request.start_time,
            request.end_time,
            &request.aggregation,
            request.bucket_ms,
        )
        .await
        .map_err(|e| ApiError::Internal(format!("aggregate timeseries: {e}")))?;
    Ok(Json(AggregateResponse { buckets }))
}

/// `GET /api/v2/timeseries/collections`
pub async fn list_timeseries_collections(
    Extension(_tenant): Extension<TenantContext>,
) -> ApiResult<Json<ListResponse>> {
    let collections = service()?.list_collections().await;
    Ok(Json(ListResponse { collections }))
}

/// `DELETE /api/v2/timeseries/collections/{collection_id}`
pub async fn delete_timeseries_collection(
    Path(collection_id): Path<String>,
    Extension(tenant): Extension<TenantContext>,
) -> ApiResult<Json<DeleteResponse>> {
    let collection = effective_collection(&tenant, &collection_id);
    let success = service()?.delete_collection(&collection).await;
    Ok(Json(DeleteResponse { success }))
}
