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

//! API-handler support modules.
//!
//! The legacy root `UnifiedHandlers` (4k+ lines, formerly `request_handlers.rs`)
//! was retired in TD-104 S3-f: every network/embedded surface now routes through
//! the runtime `proximadb_runtime::UnifiedHandlers` port handler, and the record
//! write path lives in [`record_ops_service::RecordOpsService`]. This module
//! keeps the surviving handler-support crates/modules.

pub mod record_ops_service;

pub use crate::services::operations::vectors::{
    RichFilterCondition, RichFilterOperator, RichRecordBatchRequest, RichRecordDeleteBatchRequest,
    RichRecordGetRequest, RichRecordGetResponse, RichSearchRequest, RichSearchResponse,
    RichSearchResult,
};
