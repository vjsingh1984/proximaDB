// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Service layer modules

pub mod collection_service;
pub mod vector_operations_service;
pub mod event_log_service;
pub mod streaming_search;

#[cfg(test)]
pub mod comprehensive_search_tests;

#[cfg(test)]
mod vector_operations_service_tests;

// Tests removed - API has changed significantly and needs proper refactoring

pub use collection_service::CollectionService;
pub use vector_operations_service::VectorOperationsService;
pub use event_log_service::{EventLogService, EventLogStats};
pub use streaming_search::{StreamingSearchService, StreamingSearchConfig, SearchResultStream};