// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC protocol implementation with thin handlers
//!
//! ## Module Organization
//!
//! - **V1 Services** (root level): Original gRPC services for vector, collection, graph operations
//! - **V2 Services** (`v2/`): New V2 API with ProximaRecord, typed fields, and schema support

// V1 services (original API)
pub mod collection_service;
pub mod document_service;
pub mod entity_service;
pub mod graph_service;
pub mod hybrid_search_service;
pub mod observability_service;
pub mod security_service;
pub mod sql_service;
pub mod streaming_service;
pub mod vector_service;

// V2 services (new API with ProximaRecord support)
pub mod v2;

// Re-export the entity service for SKS
pub use entity_service::EntityServiceImpl;
// Re-export the graph service
pub use graph_service::GraphServiceImpl;
// Re-export hybrid search service
pub use hybrid_search_service::HybridSearchServiceImpl;
// Re-export document and observability services
pub use document_service::DocumentServiceImpl;
pub use observability_service::ObservabilityServiceImpl;
// Re-export streaming service
pub use streaming_service::StreamingServiceImpl;
// Re-export security service
pub use security_service::SecurityServiceImpl;
// Re-export V2 services
pub use v2::ProximaRecordServiceImpl;
