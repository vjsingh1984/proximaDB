//! REST v1 deprecation-header utilities.
//!
//! Centralized home for the deprecation headers ProximaDB emits on compatibility
//! (`/api/v1`) traffic, owned *outside* the deprecated `rest::v1` adapter
//! directory. This relocation is TD-V1SUNSET-1 step 2 — the prerequisite for
//! hard-deleting that directory: the live root-crate handler
//! (`src/network/rest/v1/handlers.rs`) imports `add_rest_v1_deprecation_headers`
//! from here, so deleting `rest::v1` no longer breaks it. The v1 adapters
//! re-export `with_v1_compatibility_headers` from here for their per-request
//! header layer.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Request, header},
    middleware::Next,
    response::Response,
};

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

/// Path-scoped: stamps deprecation headers only on `/api/v1/` responses. Applied
/// as an axum middleware layer on the live canonical router.
pub async fn add_rest_v1_deprecation_headers(request: Request<Body>, next: Next) -> Response {
    let is_rest_v1 = request.uri().path().starts_with("/api/v1/");
    let mut response = next.run(request).await;

    if is_rest_v1 {
        apply_v1_deprecation_headers(response.headers_mut());
    }

    response
}

/// Unscoped: stamps deprecation headers on every response. Used by the v1
/// compatibility adapters that are known to serve only v1 traffic.
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
