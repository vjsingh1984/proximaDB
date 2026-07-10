//! # REST API v1 Handlers
//!
//! Version 1 REST endpoints for ProximaDB.
//!
//! These endpoints are compatibility adapters. New clients should use canonical
//! record/table surfaces instead of targeting v1 vector-shaped payloads.
//!
//! The deprecation-header machinery these adapters emit is centralized in
//! `crate::rest::deprecation` (TD-V1SUNSET-1 step 2) and re-exported below so
//! the per-adapter `super::with_v1_compatibility_headers` call sites resolve.

pub mod analytics;
pub mod catalog;
pub mod document;
pub mod entities;
pub mod graph;
pub mod hybrid;
pub mod multimodal_query;
pub mod observability;

// Deprecation-header utilities live outside this deprecated dir; re-exported
// here so the adapters' `super::with_v1_compatibility_headers` sites compile.
// See `crate::rest::deprecation` (TD-V1SUNSET-1 step 2).
pub use crate::rest::deprecation::{
    REST_V1_DEPRECATION_MESSAGE, REST_V1_REPLACEMENT_SURFACE,
    add_compatibility_deprecation_headers, add_rest_v1_deprecation_headers,
    apply_v1_deprecation_headers, with_v1_compatibility_headers,
};

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
pub use graph::GraphRestState;
pub use hybrid::HybridRestState;
pub use multimodal_query::UnifiedQueryRestState;
pub use observability::ObservabilityRestState;

// Re-exports — router builders
pub use analytics::create_analytics_router;
pub use document::create_document_router;
pub use graph::create_graph_router;
pub use hybrid::{
    create_health_router, create_hybrid_search_router, execute_sql, sql_value_to_json,
};
pub use multimodal_query::create_multimodal_router;
pub use observability::create_observability_router;

// Re-exports — handler functions (for direct registration in existing root-crate routers)
pub use catalog::{collection_operation, delete_collection, get_collection, list_collections};
pub use hybrid::{liveness_check, readiness_check};
