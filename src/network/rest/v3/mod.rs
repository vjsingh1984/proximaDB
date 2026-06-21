/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! REST API v3 — native server-side embedding surface.
//!
//! Adds the document-ingest endpoint for text-only records. Unlike v1/v2
//! which require a `vector` field, v3 accepts text-only records and
//! dispatches them through the in-process EmbeddingService singleton
//! (proximadb-embedding crate) before they reach WAL + index. Suitable as
//! a target for upstream gateways that proxy a `/v1/ingest`-style API.
//!
//! ## Endpoints
//!
//! - `POST /api/v3/collections/{collection}/documents` — text-only ingest;
//!   the server populates vectors via the tenant's configured embedding route.
//!
//! ## Headers
//!
//! - `X-Tenant-ID` — required; resolves the embedding route from the tenant
//!   registry. Falls back to per-record `tenant_id` if the header is missing.
//! - `X-Ingest-Mode` — `sync` (default) or `async`. Async ack returns 202
//!   immediately after WAL fsync; sync waits for the index commit.
//! - `X-Embed-Source` — `native` (default; server embeds) or `sdk-vector`
//!   (client provided the vector; server skips embedding).
//!
//! See the operator-side architecture docs for the finalized API contract
//! consumed by upstream gateways.

pub mod documents;

use axum::{
    Router,
    extract::Path,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use tracing::warn;

use super::v1::handlers::AppState;

/// Sunset date after which the v3 document-ingest alias is removed; clients
/// must migrate to `POST /api/v2/collections/{id}/documents`.
const V3_DOCUMENTS_SUNSET_DATE: &str = "2026-09-30";

/// Create the v3 API router with all endpoints. Mounted under `/api/v3`
/// in `server.rs` alongside `/api/v1` and `/api/v2`.
///
/// NOTE: v3 is now an alias. The document-ingest endpoint folds into v2
/// (`POST /api/v2/collections/{id}/documents`); this route returns a
/// `308 Permanent Redirect` to the canonical v2 path (mirrors the v1 graph
/// 308 redirect pattern). Sunset: see `V3_DOCUMENTS_SUNSET_DATE`.
pub fn create_v3_router() -> Router<AppState> {
    use axum::routing::post;

    Router::new().route(
        "/collections/{collection_id}/documents",
        post(redirect_documents_to_v2),
    )
}

/// 308 redirect from the deprecated v3 document-ingest endpoint to the
/// canonical v2 surface. Preserves the request method/body semantics (308),
/// and stamps `deprecation` + `sunset` headers like the v1 graph redirects.
async fn redirect_documents_to_v2(Path(collection_id): Path<String>) -> Response {
    let canonical_path = format!("/api/v2/collections/{}/documents", collection_id);

    warn!(
        canonical_route = %canonical_path,
        sunset_date = V3_DOCUMENTS_SUNSET_DATE,
        "v3 document-ingest endpoint is deprecated; redirecting to canonical v2 route"
    );

    let mut response = StatusCode::PERMANENT_REDIRECT.into_response();

    if let Ok(location_value) = HeaderValue::from_str(&canonical_path) {
        response
            .headers_mut()
            .insert(header::LOCATION, location_value.clone());
        response.headers_mut().insert(
            header::HeaderName::from_static("x-proximadb-canonical-route"),
            location_value,
        );
    }

    response.headers_mut().insert(
        header::HeaderName::from_static("deprecation"),
        HeaderValue::from_static("true"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("sunset"),
        HeaderValue::from_static(V3_DOCUMENTS_SUNSET_DATE),
    );

    response
}
