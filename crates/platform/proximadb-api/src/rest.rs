//! # REST API Handlers
//!
//! HTTP/JSON API handlers via Axum framework.

pub mod errors;
pub mod state;
pub mod v1;
pub mod v2;

use serde::Serialize;

// Re-export errors and state for consumers
pub use errors::{RestError, RestResult};
pub use state::{RestAppState, TenantContext};

// Re-export v1 handlers
pub use v1::{
    AnalyticsHandler, AqlHandler, CatalogHandler, CollectionHandler, DocumentHandler,
    DocumentQueryHandler, EntityHandler, GraphHandler, GraphTraversalHandler, HybridSearchHandler,
    LogsHandler, MetricsHandler, ProgressiveSearchHandler, VectorHandler,
};

/// REST API request context
#[derive(Debug, Clone)]
pub struct RestRequest {
    pub path: String,
    pub method: String,
    pub body: Option<serde_json::Value>,
    pub headers: Vec<(String, String)>,
}

/// REST API response
#[derive(Debug, Clone, Serialize)]
pub struct RestResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

/// REST API handler
pub struct RestApiHandler {
    // Service dependencies will be added here
}

impl RestApiHandler {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for RestApiHandler {
    fn default() -> Self {
        Self::new()
    }
}

// TODO: Move REST handlers from src/network/rest and src/api_handlers
