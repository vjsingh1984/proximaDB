/*
 * Copyright 2025 Vijaykumar Singh
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

//! REST API handlers for ProximaDB
//!
//! Provides HTTP/JSON endpoints that delegate to the unified service layer

pub use crate::api_handlers;
/// Health and readiness check endpoints
pub mod health;
/// Progressive multi-stage search with explain endpoint
pub mod progressive_search_handler;
/// Proto-JSON bidirectional serialization for REST responses
pub mod proto_json;
/// REST HTTP server setup and route configuration
pub mod server;
/// V1 REST API handlers (collections, vectors, graph, entities)
pub mod v1;
/// V2 REST API handlers (ProximaRecord, typed fields, schema)
pub mod v2;
/// V3 REST API handlers (native server-side embedding, text-only documents)
pub mod v3;
/// WebSocket support for real-time streaming
pub mod websocket;

pub use api_handlers::*;
pub use server::*;
// Re-export handlers from v1
pub use v1::handlers;
