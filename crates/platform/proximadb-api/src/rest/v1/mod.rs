//! # REST API v1 Handlers
//!
//! Version 1 REST endpoints for ProximaDB.
//!
//! These endpoints are compatibility adapters. New clients should use canonical
//! record/table surfaces instead of targeting v1 vector-shaped payloads.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Request, header},
    middleware::Next,
    response::Response,
};

pub mod analytics;
pub mod catalog;
pub mod document;
pub mod entities;
pub mod graph;
pub mod hybrid;
pub mod multimodal_query;
pub mod observability;

pub const REST_V1_REPLACEMENT_SURFACE: &str = "/api/v2, proximadb.v2.ProximaRecordService, pgwire";
pub const REST_V1_DEPRECATION_MESSAGE: &str =
    "REST /api/v1 is compatibility-only; use canonical ProximaRecord APIs for new clients";

pub fn apply_v1_deprecation_headers(headers: &mut HeaderMap) {
    let deprecation = header::HeaderName::from_static("deprecation");
    if !headers.contains_key(&deprecation) {
        headers.insert(deprecation, HeaderValue::from_static("true"));
    }

    if !headers.contains_key(header::LINK) {
        headers.insert(
            header::LINK,
            HeaderValue::from_static("</api/v2>; rel=\"successor-version\""),
        );
    }

    let status = header::HeaderName::from_static("x-proximadb-api-status");
    if !headers.contains_key(&status) {
        headers.insert(status, HeaderValue::from_static("deprecated-compatibility"));
    }

    let replacement = header::HeaderName::from_static("x-proximadb-replacement");
    if !headers.contains_key(&replacement) {
        headers.insert(
            replacement,
            HeaderValue::from_static(REST_V1_REPLACEMENT_SURFACE),
        );
    }

    let message = header::HeaderName::from_static("x-proximadb-deprecation-message");
    if !headers.contains_key(&message) {
        headers.insert(
            message,
            HeaderValue::from_static(REST_V1_DEPRECATION_MESSAGE),
        );
    }
}

pub async fn add_rest_v1_deprecation_headers(request: Request<Body>, next: Next) -> Response {
    let is_rest_v1 = request.uri().path().starts_with("/api/v1/");
    let mut response = next.run(request).await;

    if is_rest_v1 {
        apply_v1_deprecation_headers(response.headers_mut());
    }

    response
}

pub async fn add_compatibility_deprecation_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    apply_v1_deprecation_headers(response.headers_mut());
    response
}

pub fn with_v1_compatibility_headers<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn(
        add_compatibility_deprecation_headers,
    ))
}

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

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request, routing::get};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn compatibility_router_headers_mark_relative_v1_adapters() {
        let router =
            with_v1_compatibility_headers(Router::new().route("/relative", get(|| async { "ok" })));

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/relative")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response
                .headers()
                .get("deprecation")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
        assert_eq!(
            response
                .headers()
                .get("x-proximadb-replacement")
                .and_then(|value| value.to_str().ok()),
            Some(REST_V1_REPLACEMENT_SURFACE)
        );
    }
}
