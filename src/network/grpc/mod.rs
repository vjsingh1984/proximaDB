// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC protocol implementation with thin handlers

// pub mod v1; // Removed - not needed
pub mod collection_service;
pub mod document_service;
pub mod entity_service;
pub mod graph_service;
pub mod observability_service;
pub mod sql_service;
pub mod streaming_service;
pub mod vector_service;

// Re-export the entity service for SKS
pub use entity_service::EntityServiceImpl;
// Re-export the graph service
pub use graph_service::GraphServiceImpl;
// Re-export document and observability services
pub use document_service::DocumentServiceImpl;
pub use observability_service::ObservabilityServiceImpl;
// Re-export streaming service
pub use streaming_service::StreamingServiceImpl;
