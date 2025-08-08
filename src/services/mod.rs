// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Service layer modules

pub mod collection_service;
pub mod vector_operations_service;
// 🔴 UNUSED SERVICES - Never imported or used
// pub mod migration;
// pub mod storage_path_service;
pub mod streaming_search;

#[cfg(test)]
pub mod comprehensive_search_tests;

#[cfg(test)]
mod vector_operations_service_tests;

#[cfg(test)]
pub mod tests {
    // Test modules will be added here as needed
}

pub use collection_service::CollectionService;
pub use vector_operations_service::VectorOperationsService;
pub use streaming_search::{StreamingSearchService, StreamingSearchConfig, SearchResultStream};