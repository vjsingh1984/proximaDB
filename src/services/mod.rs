// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Services layer
//! 
//! High-level services that coordinate storage engines, indexes, and operations

pub mod collection;
pub mod operations;  
pub mod search;
pub mod events;

// Legacy test module (to be reorganized)
#[cfg(test)]
pub mod tests;

// Re-export main service types with cleaner names
pub use collection::Collections;
pub use operations::VectorOps;
pub use search::StreamingSearch;
pub use events::EventLog;

// Legacy compatibility exports (will be removed)
pub use collection::manager as collection_service;
pub use operations::vectors as vector_operations_service;
pub use search::streaming as streaming_search;
pub use events::log as event_log_service;
pub use events::persistence as event_log_persistence;

// Legacy type aliases for compatibility
pub use collection::Collections as CollectionService;
pub use operations::VectorOps as VectorOperationsService;
pub use events::EventLog as EventLogService;
pub use events::Stats as EventLogStats;
pub use search::{StreamingSearch as StreamingSearchService, StreamConfig as StreamingSearchConfig, ResultStream as SearchResultStream};