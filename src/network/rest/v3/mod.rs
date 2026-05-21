/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! REST API v3 — native server-side embedding surface.
//!
//! Adds the document-ingest endpoint that the AnvaiOps `/v1/ingest` proxy
//! targets. Unlike v1/v2 which require a `vector` field, v3 accepts text-only
//! records and dispatches them through the in-process EmbeddingService
//! singleton (proximadb-embedding crate) before they reach WAL + index.
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
//! See `docs/architecture/EMBEDDING.md` in the AnvaiOps repo for the
//! finalized API contract.

pub mod documents;

use axum::Router;

use super::v1::handlers::AppState;

/// Create the v3 API router with all endpoints. Mounted under `/api/v3`
/// in `server.rs` alongside `/api/v1` and `/api/v2`.
pub fn create_v3_router() -> Router<AppState> {
    use axum::routing::post;

    Router::new().route(
        "/collections/:collection_id/documents",
        post(documents::ingest_documents),
    )
}
