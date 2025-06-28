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

//! Service layer for ProximaDB
//!
//! The UnifiedAvroService is the single source of truth for all operations.
//! Old VectorService and CollectionService have been removed in favor of the unified approach.

pub mod collection_service;
pub mod migration;
pub mod storage_path_service;
pub mod unified_avro_service;
// pub mod vector_storage_coordinator; // REMOVED: Coordinator eliminated in optimization

/// Service result type
pub type ServiceResult<T> = Result<T, ServiceError>;

/// Service layer errors
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),
    #[error("Vector not found: {0}")]
    VectorNotFound(String),
    #[error("Invalid dimension: expected {expected}, got {actual}")]
    InvalidDimension { expected: u32, actual: usize },
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Internal error: {0}")]
    Internal(String),
}
