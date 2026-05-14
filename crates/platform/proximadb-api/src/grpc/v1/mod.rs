//! # gRPC API v1 Services
//!
//! Version 1 gRPC services for ProximaDB.

pub mod collection;
pub mod document;
pub mod entity;
pub mod graph;
pub mod hybrid;
pub mod observability;
pub mod security;
pub mod sql;
pub mod streaming;
pub mod vector;

// Re-exports
pub use collection::CollectionServiceImpl;
pub use document::DocumentServiceImpl;
pub use entity::EntityServiceImpl;
pub use graph::GraphServiceImpl;
pub use hybrid::HybridSearchServiceImpl;
pub use observability::ObservabilityServiceImpl;
pub use security::SecurityServiceImpl;
pub use sql::SqlServiceImpl;
pub use streaming::StreamingServiceImpl;
pub use vector::VectorServiceImpl;
