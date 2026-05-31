/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! REST v2 External Collection endpoints (Phase 8 F5 / TD-090).
//!
//! - `POST /api/v2/external-collections` — register an external table un-copied
//! - `GET  /api/v2/external-collections` — list registered external collections
//! - `GET  /api/v2/external-collections/:id` — get one
//! - `POST /api/v2/external-collections/:id/build` — build the index in place
//! - `POST /api/v2/external-collections/:id/search` — search the built index
//!
//! Experimental, v2-only. Handlers reach the `ExternalCollectionService` via
//! `AppState`. See `src/services/external_collection/`.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::errors::{ApiError, ApiResult};
use crate::network::rest::v1::handlers::AppState;
use crate::network::rest::v2::records::{RestProximaValue, proxima_value_to_rest_value};
use crate::services::external_collection::{
    ExternalCollection, ExternalCollectionService, ExternalCollectionSpec, ExternalFormat,
};
use proximadb_records::ProximaTreeNode;

/// Request body for registering an external collection.
#[derive(Debug, Deserialize)]
pub struct RegisterExternalCollectionRequest {
    pub name: String,
    pub location: String,
    #[serde(default)]
    pub format: ExternalFormat,
    pub id_column: String,
    pub vector_column: String,
    pub dimension: usize,
    #[serde(default = "default_metric")]
    pub distance_metric: String,
    /// Optional Utf8 column to build a BM25 index over (enables hybrid search).
    #[serde(default)]
    pub text_column: Option<String>,
}

fn default_metric() -> String {
    "cosine".to_string()
}

/// Request body for searching an external collection. `text` (optional) enables
/// hybrid (vector + BM25) retrieval when the collection has a `text_column`.
#[derive(Debug, Deserialize)]
pub struct ExternalSearchRequest {
    pub vector: Vec<f32>,
    #[serde(default = "default_k")]
    pub k: usize,
    #[serde(default)]
    pub text: Option<String>,
}

fn default_k() -> usize {
    10
}

/// Single external-collection response.
#[derive(Debug, Serialize)]
pub struct ExternalCollectionResponse {
    pub collection: ExternalCollection,
}

/// List response.
#[derive(Debug, Serialize)]
pub struct ExternalCollectionListResponse {
    pub collections: Vec<ExternalCollection>,
}

/// Build response.
#[derive(Debug, Serialize)]
pub struct BuildExternalCollectionResponse {
    pub collection: ExternalCollection,
    pub indexed_record_count: usize,
}

/// One search hit, carrying the full record federated from the external source
/// (`props` = non-vector columns), matching the native v2 search-hit shape.
#[derive(Debug, Serialize)]
pub struct ExternalSearchHit {
    pub id: String,
    pub score: f32,
    pub props: HashMap<String, RestProximaValue>,
}

/// Search response.
#[derive(Debug, Serialize)]
pub struct ExternalSearchResponse {
    pub id: String,
    pub hits: Vec<ExternalSearchHit>,
}

/// Refresh response (staleness check + optional rebuild).
#[derive(Debug, Serialize)]
pub struct RefreshExternalCollectionResponse {
    pub collection: ExternalCollection,
    pub stale_detected: bool,
    pub rebuilt: bool,
}

fn service(state: &AppState) -> ApiResult<&std::sync::Arc<ExternalCollectionService>> {
    state.external_collection_service.as_ref().ok_or_else(|| {
        ApiError::NotImplemented("External Collection service is not enabled".to_string())
    })
}

fn require(collection: Option<ExternalCollection>, id: &str) -> ApiResult<ExternalCollection> {
    collection.ok_or_else(|| ApiError::NotFound(format!("external collection '{id}' not found")))
}

/// `POST /api/v2/external-collections`
pub async fn register_external_collection_v2(
    State(state): State<AppState>,
    Json(req): Json<RegisterExternalCollectionRequest>,
) -> ApiResult<Json<ExternalCollectionResponse>> {
    if req.name.is_empty() || req.location.is_empty() {
        return Err(ApiError::InvalidArgument(
            "name and location are required".to_string(),
        ));
    }
    let spec = ExternalCollectionSpec {
        name: req.name,
        location: req.location,
        format: req.format,
        id_column: req.id_column,
        vector_column: req.vector_column,
        dimension: req.dimension,
        distance_metric: req.distance_metric,
        text_column: req.text_column,
    };
    let collection = service(&state)?
        .register(spec)
        .await
        .map_err(|e| ApiError::Internal(format!("register external collection: {e:#}")))?;
    info!(
        "V2 API: registered external collection '{}' ({})",
        collection.spec.name, collection.id
    );
    Ok(Json(ExternalCollectionResponse { collection }))
}

/// `GET /api/v2/external-collections`
pub async fn list_external_collections_v2(
    State(state): State<AppState>,
) -> ApiResult<Json<ExternalCollectionListResponse>> {
    let collections = service(&state)?.list();
    Ok(Json(ExternalCollectionListResponse { collections }))
}

/// `GET /api/v2/external-collections/:id`
pub async fn get_external_collection_v2(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<ExternalCollectionResponse>> {
    let collection = require(service(&state)?.get(&id), &id)?;
    Ok(Json(ExternalCollectionResponse { collection }))
}

/// `POST /api/v2/external-collections/:id/build`
pub async fn build_external_collection_v2(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<BuildExternalCollectionResponse>> {
    let svc = service(&state)?;
    let indexed_record_count = svc
        .build(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("build external collection '{id}': {e:#}")))?;
    let collection = require(svc.get(&id), &id)?;
    info!(
        "V2 API: built external collection '{}' ({} records)",
        collection.spec.name, indexed_record_count
    );
    Ok(Json(BuildExternalCollectionResponse {
        collection,
        indexed_record_count,
    }))
}

/// `POST /api/v2/external-collections/:id/search`
pub async fn search_external_collection_v2(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<ExternalSearchRequest>,
) -> ApiResult<Json<ExternalSearchResponse>> {
    if req.vector.is_empty() {
        return Err(ApiError::InvalidArgument(
            "query vector is required".to_string(),
        ));
    }
    let hits = service(&state)?
        .hybrid_search(&id, req.vector, req.text, req.k)
        .await
        .map_err(|e| ApiError::Internal(format!("search external collection '{id}': {e:#}")))?
        .into_iter()
        .map(|hit| {
            // Flatten the record's scalar props to the native RestProximaValue
            // shape (nested Object nodes are dropped — handled in a later slice).
            let props = hit
                .record
                .props
                .iter()
                .filter_map(|(k, node)| match node {
                    ProximaTreeNode::Value(v) => Some((k.clone(), proxima_value_to_rest_value(v))),
                    ProximaTreeNode::Object(_) => None,
                })
                .collect();
            ExternalSearchHit {
                id: hit.id,
                score: hit.score,
                props,
            }
        })
        .collect();
    Ok(Json(ExternalSearchResponse { id, hits }))
}

/// `POST /api/v2/external-collections/:id/refresh`
pub async fn refresh_external_collection_v2(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<RefreshExternalCollectionResponse>> {
    let svc = service(&state)?;
    let outcome = svc
        .refresh(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("refresh external collection '{id}': {e:#}")))?;
    let collection = require(svc.get(&id), &id)?;
    info!(
        "V2 API: refreshed external collection '{}' (stale={}, rebuilt={})",
        collection.spec.name, outcome.stale_detected, outcome.rebuilt
    );
    Ok(Json(RefreshExternalCollectionResponse {
        collection,
        stale_detected: outcome.stale_detected,
        rebuilt: outcome.rebuilt,
    }))
}
