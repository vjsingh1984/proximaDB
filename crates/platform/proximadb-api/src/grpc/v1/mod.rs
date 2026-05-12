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
pub mod streaming;
pub mod vector;

// Re-exports
pub use collection::CollectionService;
pub use document::DocumentService;
pub use entity::EntityService;
pub use graph::{GraphService, GraphTraversalService};
pub use hybrid::HybridSearchService;
pub use observability::{LogsService, MetricsService};
pub use security::SecurityService;
pub use streaming::StreamingService;
pub use vector::VectorService;
