//! # REST API v1 Handlers
//!
//! Version 1 REST endpoints for ProximaDB.

pub mod analytics;
pub mod catalog;
pub mod document;
pub mod entities;
pub mod graph;
pub mod hybrid;
pub mod observability;

// Re-exports
pub use analytics::{AnalyticsHandler, AqlHandler};
pub use catalog::{CatalogHandler, CollectionHandler};
pub use document::{DocumentHandler, DocumentQueryHandler};
pub use entities::{EntityHandler, VectorHandler};
pub use graph::{GraphHandler, GraphTraversalHandler};
pub use hybrid::{HybridSearchHandler, ProgressiveSearchHandler};
pub use observability::{LogsHandler, MetricsHandler};
