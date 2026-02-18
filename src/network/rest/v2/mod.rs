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

//! REST API v2 - ProximaRecord and typed schema support
//!
//! This module provides the v2 REST API handlers for ProximaDB, introducing
//! ProximaRecord as the primary record type with full type system support.
//!
//! ## New Endpoints
//!
//! - `POST /api/v2/collections` - Create collection with schema
//! - `PUT /api/v2/collections/{id}/schema` - Update collection schema
//! - `GET /api/v2/collections/{id}/schema` - Get collection schema
//! - `POST /api/v2/collections/{collection}/records/batch` - Insert ProximaRecords
//! - `POST /api/v2/collections/{collection}/search` - Search with typed filters
//!
//! ## Key Features
//!
//! - **Typed Fields**: Full support for TEXT, INTEGER, FLOAT, DECIMAL, UUID, etc.
//! - **Schema Enforcement**: Strict, Flexible, or Hybrid modes
//! - **TEXT Column Storage**: Dedicated columnar storage for large text fields
//! - **Typed Filtering**: Range, equality, and CONTAINS filters with type safety
//!
//! ## Migration from v1
//!
//! The v2 API is designed for gradual migration. Collections can enable
//! ProximaRecord support via the `enable_proxima_record` flag, allowing
//! mixed v1/v2 operations during transition.

pub mod collections;
pub mod records;
pub mod schema;

pub use collections::*;
pub use records::*;
pub use schema::*;

use axum::Router;

use super::v1::handlers::AppState;

/// Create the v2 API router with all endpoints
///
/// This router provides the v2 REST API endpoints for ProximaRecord operations.
/// It should be nested under `/api/v2` in the main application router.
///
/// ## Endpoints
///
/// ### Collections
/// - `POST /collections` - Create collection with schema
/// - `GET /collections` - List collections with pagination
/// - `GET /collections/:collection_id` - Get collection details
///
/// ### Schema
/// - `GET /collections/:collection_id/schema` - Get collection schema
/// - `PUT /collections/:collection_id/schema` - Update collection schema
///
/// ### Records
/// - `POST /collections/:collection_id/records/batch` - Batch insert ProximaRecords
/// - `GET /collections/:collection_id/records/:record_id` - Get single record
/// - `POST /collections/:collection_id/search` - Search with typed filters
pub fn create_v2_router() -> Router<AppState> {
    use axum::routing::{get, post, put};

    Router::new()
        // Collection operations with schema support
        .route("/collections", post(collections::create_collection_v2))
        .route("/collections", get(collections::list_collections_v2))
        .route(
            "/collections/:collection_id",
            get(collections::get_collection_v2),
        )
        // Schema management - separate routes for GET and PUT
        .route(
            "/collections/:collection_id/schema",
            get(schema::get_schema),
        )
        .route(
            "/collections/:collection_id/schema",
            put(schema::update_schema),
        )
        // Record operations
        .route(
            "/collections/:collection_id/records/batch",
            post(records::insert_records),
        )
        .route(
            "/collections/:collection_id/records/:record_id",
            get(records::get_record_v2),
        )
        // Search with typed filters
        .route(
            "/collections/:collection_id/search",
            post(records::search_with_typed_filters),
        )
}
