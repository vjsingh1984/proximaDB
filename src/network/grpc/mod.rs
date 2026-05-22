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
//!
//! ## Status
//!
//! These port-implementing services remain in the root crate. Collection, vector, and SQL
//! protocol adapters live in `crates/platform/proximadb-api`. Port traits live in
//! `crates/platform/proximadb-runtime`.

/// Shared gRPC authentication and data-plane capability enforcement.
pub mod auth;

// Port implementations: root crate provides concrete types that implement runtime port traits
/// gRPC service for document CRUD operations (implements DocumentPort)
pub mod document_service;
/// gRPC service for SKS entity operations (implements EntityPort)
pub mod entity_service;
/// gRPC service for graph database operations (implements GraphPort)
pub mod graph_service;
/// gRPC service for hybrid (vector + BM25) search (implements HybridPort)
pub mod hybrid_search_service;
/// gRPC service for observability (logs, metrics, traces) (implements ObservabilityPort)
pub mod observability_service;
/// gRPC service for security and authentication management (implements SecurityPort)
pub mod security_service;
/// gRPC service for bidirectional streaming operations (implements StreamingPort)
pub mod streaming_service;

// V2 services (new API with ProximaRecord support)
/// V2 gRPC services with ProximaRecord typed field support
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
