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

//! Unified handlers module for shared business logic between REST and gRPC

#[cfg(feature = "ai_endpoints")]
pub mod ai_endpoints;
pub mod enterprise;
#[cfg(feature = "sales_endpoints")]
pub mod sales_endpoints;
pub mod unified_handlers;

#[cfg(test)]
mod unified_handlers_tests;

pub use crate::services::operations::vectors::{
    RichFilterCondition, RichFilterOperator, RichRecordBatchRequest, RichSearchRequest,
    RichSearchResponse, RichSearchResult,
};
pub use unified_handlers::UnifiedHandlers;
