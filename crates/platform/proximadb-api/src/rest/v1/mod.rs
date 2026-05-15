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

// Re-exports — handler types
pub use analytics::{AnalyticsHandler, AqlHandler};
pub use catalog::{CatalogHandler, CollectionHandler};
pub use document::{DocumentHandler, DocumentQueryHandler};
pub use entities::{EntityHandler, VectorHandler};
pub use graph::{GraphHandler, GraphTraversalHandler};
pub use hybrid::{HybridSearchHandler, ProgressiveSearchHandler};
pub use observability::{LogsHandler, MetricsHandler};

// Re-exports — state types
pub use analytics::AnalyticsRestState;
pub use document::DocumentRestState;
pub use observability::ObservabilityRestState;

// Re-exports — router builders
pub use analytics::create_analytics_router;
pub use catalog::create_collection_router;
pub use document::create_document_router;
pub use entities::{create_vector_router, parse_batch_request, parse_search_request};
pub use hybrid::{create_health_router, create_sql_router, execute_sql, sql_value_to_json};
pub use observability::create_observability_router;

// Re-exports — handler functions (for direct registration in existing root-crate routers)
pub use catalog::{collection_operation, delete_collection, get_collection, list_collections};
pub use entities::{
    delete_vector, get_vector, vector_batch, vector_search, vector_search_with_metadata,
};
pub use hybrid::{liveness_check, readiness_check};
