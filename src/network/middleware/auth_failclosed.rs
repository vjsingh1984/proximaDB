/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Fail-closed guard for a security **misconfiguration**.
//!
//! When the REST server is built with authentication *enabled* but **no**
//! `SecurityCoordinator` wired, the old behavior logged a warning and then
//! served every request **unauthenticated** — i.e. it failed *open* on a
//! security misconfiguration. This layer is attached only in that case and
//! **fails closed**: data-plane requests (`/api/*`) are rejected with
//! `503 Service Unavailable` while non-data-plane paths (health / liveness /
//! metrics) still pass, so orchestrator probes keep working and the operator
//! sees an up-but-degraded server rather than a silent auth bypass.
//!
//! The fix is scoped to the request layer because the REST router constructor
//! returns `Self` (cannot return an error) and panicking in a critical module
//! is disallowed by the panic-policy guard.

use axum::{
    Json,
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

/// Path prefix of the authenticated data plane. Requests outside it (health,
/// liveness, metrics) are allowed through even when auth is misconfigured.
const DATA_PLANE_PREFIX: &str = "/api/";

/// Reject data-plane requests with `503` when auth is enabled-but-unconfigured.
/// This middleware is attached ONLY in the misconfigured case, so its presence
/// is the signal — it always denies `/api/*` and passes everything else.
pub async fn auth_misconfigured_deny_data_plane(req: Request, next: Next) -> Response {
    if req.uri().path().starts_with(DATA_PLANE_PREFIX) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "service_unavailable",
                "message": "Authentication is enabled but no SecurityCoordinator is \
                    configured; the data plane is failing closed. Wire a \
                    SecurityCoordinator or set auth.enabled=false.",
            })),
        )
            .into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, routing::get};
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/api/v1/search", get(|| async { "data" }))
            .route("/health", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(
                auth_misconfigured_deny_data_plane,
            ))
    }

    #[tokio::test]
    async fn data_plane_is_denied_503() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/search")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn health_still_passes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
