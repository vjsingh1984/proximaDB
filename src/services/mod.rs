// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Services Module - Business Logic and Coordination Layer
//!
//! This module provides ProximaDB's service layer that implements business logic and
//! coordinates between storage engines, indexes, and other system components. Services
//! handle high-level operations while delegating low-level tasks to appropriate subsystems.
//!
//! ## Role in ProximaDB Architecture
//!
//! The services layer acts as the orchestration hub:
//! ```text
//! API Handlers (REST/gRPC)
//!         ↓
//! ┌───────────────────────────────────────┐
//! │          Services Layer                │
//! ├───────────────────────────────────────┤
//! │ Collections │ Operations │ Search │ Events │
//! └───────────────────────────────────────┘
//!         ↓           ↓          ↓        ↓
//!    Storage     AXIS Index   Compute   WAL
//!    Engines     (HNSW/IVF)   (SIMD)   System
//! ```
//!
//! ## Core Services
//!
//! ### 1. **Collection Service** (`collection/`)
//! Manages vector collections and metadata:
//! - Collection lifecycle (create, delete, update)
//! - Metadata management and schema validation
//! - Storage engine selection and configuration
//! - Statistics tracking and optimization hints
//!
//! ### 2. **Vector Operations Service** (`operations/`)
//! Handles vector CRUD operations:
//! - Direct memtable access for low latency
//! - Batch insertions with automatic flushing
//! - Update and delete operations
//! - Transaction coordination
//!
//! ### 3. **Search Service** (`search/`)
//! Orchestrates vector similarity search:
//! - Query planning and optimization
//! - Index and storage coordination
//! - Result streaming and pagination
//! - Hybrid search with metadata filtering
//!
//! ### 4. **Event Log Service** (`events/`)
//! Provides persistent event logging:
//! - Operation logging for recovery
//! - Event replay for consistency
//! - Compaction and flush notifications
//! - Cross-component coordination
//!
//! ## Service Characteristics
//!
//! ### Design Principles
//! - **Stateless**: Services don't maintain state between requests
//! - **Thread-Safe**: All services safe for concurrent access
//! - **Async-First**: Built on Tokio for async operations
//! - **Fault-Tolerant**: Graceful degradation on failures
//!
//! ### Performance Goals
//! - **Latency**: < 1ms service overhead
//! - **Throughput**: 100K+ ops/sec per service
//! - **Concurrency**: Lock-free where possible
//! - **Memory**: Bounded memory usage
//!
//! ## Service Interactions
//!
//! ```text
//! Insert Flow:
//! API → VectorOps → Memtable → EventLog → WAL
//!                      ↓
//!                   Storage
//!                   (on flush)
//!
//! Search Flow:
//! API → Search → Query Optimizer
//!         ↓            ↓
//!     AXIS Index   Storage Engine
//!         ↓            ↓
//!       Merge & Rank Results
//! ```
//!
//! ## Configuration
//!
//! Services are configured through the main config:
//! ```toml
//! [services]
//! # Collection service
//! [services.collection]
//! max_collections = 1000
//! metadata_cache_size = 100
//!
//! # Operations service
//! [services.operations]
//! batch_size = 1000
//! flush_interval_ms = 5000
//!
//! # Search service
//! [services.search]
//! max_concurrent_searches = 100
//! result_cache_size = 1000
//! stream_buffer_size = 100
//!
//! # Event log service
//! [services.events]
//! retention_hours = 168  # 7 days
//! compaction_interval_hours = 24
//! ```
//!
//! ## Error Handling
//!
//! Services use unified error types:
//! - `ServiceError::NotFound` - Resource doesn't exist
//! - `ServiceError::AlreadyExists` - Duplicate resource
//! - `ServiceError::InvalidInput` - Validation failed
//! - `ServiceError::Internal` - System error
//! - `ServiceError::Unavailable` - Service temporarily down
//!
//! ## Usage Example
//!
//! ```rust
//! use proximadb::services::{Collections, VectorOps, StreamingSearch};
//!
//! // Initialize services
//! let collections = Collections::new(storage.clone());
//! let operations = VectorOps::new(storage.clone());
//! let search = StreamingSearch::new(storage.clone());
//!
//! // Create collection
//! collections.create_collection("products", config).await?;
//!
//! // Insert vectors
//! operations.insert_batch("products", vectors).await?;
//!
//! // Search with streaming
//! let stream = search.search_stream(
//!     "products",
//!     query_vector,
//!     SearchConfig::default()
//! ).await?;
//!
//! // Process results
//! while let Some(result) = stream.next().await {
//!     println!("Found: {:?}", result?);
//! }
//! ```
//!
//! ## Service Lifecycle
//!
//! 1. **Initialization**: Services created with storage references
//! 2. **Operation**: Handle requests asynchronously
//! 3. **Cleanup**: Graceful shutdown on drop
//! 4. **Recovery**: Automatic recovery from EventLog
//!
//! ## Monitoring
//!
//! Each service exports metrics:
//! - Request count and latency
//! - Error rates by type
//! - Resource usage
//! - Queue depths
//! - Cache hit rates

pub mod collection;
pub mod events;
pub mod graph_collection;
pub mod operations;
pub mod search;

// Legacy test module (to be reorganized)
#[cfg(test)]
pub mod tests;

// Re-export main service types with cleaner names
pub use collection::Collections;
pub use events::EventLog;
pub use graph_collection::GraphCollectionService;
pub use operations::VectorOps;
pub use search::StreamingSearch;

// Legacy compatibility exports (will be removed)
pub use collection::manager as collection_service;
pub use events::log as event_log_service;
pub use events::persistence as event_log_persistence;
pub use operations::vectors as vector_operations_service;
pub use search::streaming as streaming_search;

// Legacy type aliases for compatibility
pub use collection::Collections as CollectionService;
pub use events::EventLog as EventLogService;
pub use events::Stats as EventLogStats;
pub use operations::VectorOps as VectorOperationsService;
pub use search::{
    ResultStream as SearchResultStream, StreamConfig as StreamingSearchConfig,
    StreamingSearch as StreamingSearchService,
};
