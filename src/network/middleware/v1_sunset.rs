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

//! REST `/api/v1/*` sunset gate (ADR-049 M0-c).
//!
//! ProximaDB is retiring the v1 wire model in favor of the native `proximadb.v2`
//! surface (ADR-049, closes TD-121). This middleware is the *enabling slice* for
//! that retirement on the REST edge: when the `PROXIMADB_REST_V1_COMPAT` flag is
//! off, requests to the deprecated `/api/v1/*` surface are answered with
//! `410 Gone` + RFC 8594 deprecation headers instead of being served (the gRPC
//! v1 compat gate was removed in TD-V1SUNSET-1 step 4).
//!
//! The flag is **off by default** (TD-V1SUNSET-1 step 1: the breaking cutover —
//! `/api/v1/*` returns `410 Gone` unless explicitly re-enabled). Set
//! `PROXIMADB_REST_V1_COMPAT=1` to temporarily re-enable v1 during migration.
//! The `/api/v2/*` surface is never affected.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

/// Env flag gating the deprecated REST v1 surface. Default ON (v1 served); set
/// to `0` / `false` / `off` / `no` (case-insensitive) to disable v1 and return
/// `410 Gone`.
pub const REST_V1_COMPAT_ENV: &str = "PROXIMADB_REST_V1_COMPAT";

/// Optional RFC 8594 `Sunset` HTTP-date advertised when v1 is disabled (e.g.
/// `Wed, 01 Jul 2026 00:00:00 GMT`). Emitted verbatim; unset ⇒ no `Sunset`
/// header.
pub const REST_V1_SUNSET_ENV: &str = "PROXIMADB_REST_V1_SUNSET";

/// Whether the deprecated `/api/v1/*` surface should still be served. Defaults
/// to `false` (off) when the env var is unset (TD-V1SUNSET-1 step 1).
pub fn rest_v1_compat_enabled() -> bool {
    parse_compat_flag(std::env::var(REST_V1_COMPAT_ENV).ok().as_deref())
}

/// Pure parser for the compat flag (testable without touching the environment).
fn parse_compat_flag(value: Option<&str>) -> bool {
    match value {
        Some(v) => {
            let v = v.trim();
            !(v == "0"
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("no"))
        }
        None => false,
    }
}

/// Middleware that answers `/api/v1/*` with `410 Gone` + deprecation headers when
/// the v1 compat gate (`compat_enabled`) is off. Every other path — and every
/// path when the gate is on — passes through unchanged.
pub async fn v1_sunset_middleware(
    State(compat_enabled): State<bool>,
    req: Request,
    next: Next,
) -> Response {
    if req.uri().path().starts_with("/api/v1/") {
        // TD-V1SUNSET-1 reconciliation (2026-08-28): the REST metrics layer is
        // a post-routing route_layer, so unmatched /api/v1/* paths were never
        // metered and the sunset window could not be verified from metrics.
        // Count every decision HERE — the gate for Step 5 (remove this
        // middleware) is `increase(...{outcome="gone"}[N])` against the
        // traffic baseline, and residual compat usage shows as compat_served.
        if compat_enabled {
            record_sunset_response("compat_served");
        } else {
            record_sunset_response("gone");
            return v1_gone_response(req.uri().path());
        }
    }
    next.run(req).await
}

lazy_static::lazy_static! {
    /// Per-decision sunset counter (TD-V1SUNSET-1 Step-5 gate; see the
    /// middleware above). Labels are the two compile-time outcomes — no
    /// request data. Panic-free registration (same shape as claim_metrics /
    /// operational_metrics): fall back to an unregistered vec on the
    /// pathological double-register rather than expect().
    static ref SUNSET_RESPONSES: prometheus::CounterVec = {
        prometheus::register_counter_vec!(
            "proximadb_v1_sunset_responses_total",
            "/api/v1 sunset middleware decisions: gone = answered 410 \
             (compat off), compat_served = request passed through under \
             PROXIMADB_REST_V1_COMPAT. TD-V1SUNSET-1 Step 5's window gate.",
            &["outcome"]
        )
        .unwrap_or_else(|_| {
            prometheus::CounterVec::new(
                prometheus::Opts::new(
                    "proximadb_v1_sunset_responses_total",
                    "v1 sunset decisions",
                ),
                &["outcome"],
            )
            .unwrap_or_else(|_| unreachable!("valid counter descriptor"))
        })
    };
}

fn record_sunset_response(outcome: &'static str) {
    let _ = SUNSET_RESPONSES
        .get_metric_with_label_values(&[outcome])
        .map(|c| c.inc());
}

fn v1_gone_response(path: &str) -> Response {
    let body = Json(json!({
        "error": "gone",
        "message": format!(
            "REST {path} is part of the deprecated /api/v1 surface, which is disabled \
             ({REST_V1_COMPAT_ENV} is off). Migrate to /api/v2. See ADR-049."
        ),
        "successor_version": "/api/v2",
    }));
    let mut resp = (StatusCode::GONE, body).into_response();
    let headers = resp.headers_mut();
    // RFC 8594 deprecation signalling.
    headers.insert("deprecation", HeaderValue::from_static("true"));
    headers.insert(
        header::LINK,
        HeaderValue::from_static("</api/v2>; rel=\"successor-version\""),
    );
    if let Ok(sunset) = std::env::var(REST_V1_SUNSET_ENV)
        && let Ok(val) = HeaderValue::from_str(sunset.trim())
    {
        headers.insert("sunset", val);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, routing::get};
    use tower::ServiceExt;

    fn app(compat: bool) -> Router {
        Router::new()
            .route("/api/v1/ping", get(|| async { "v1" }))
            .route("/api/v2/ping", get(|| async { "v2" }))
            .layer(axum::middleware::from_fn_with_state(
                compat,
                v1_sunset_middleware,
            ))
    }

    #[test]
    fn compat_flag_parsing() {
        assert!(!parse_compat_flag(None)); // default off (TD-V1SUNSET-1 step 1)
        assert!(parse_compat_flag(Some("1")));
        assert!(parse_compat_flag(Some("true")));
        assert!(parse_compat_flag(Some("on")));
        assert!(!parse_compat_flag(Some("0")));
        assert!(!parse_compat_flag(Some("false")));
        assert!(!parse_compat_flag(Some("OFF")));
        assert!(!parse_compat_flag(Some(" no ")));
    }

    #[tokio::test]
    async fn v1_returns_410_with_headers_when_gate_off() {
        let resp = app(false)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::GONE);
        assert_eq!(resp.headers().get("deprecation").unwrap(), "true");
        assert!(
            resp.headers()
                .get(header::LINK)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("successor-version")
        );
    }

    #[tokio::test]
    async fn v2_unaffected_when_gate_off() {
        let resp = app(false)
            .oneshot(
                Request::builder()
                    .uri("/api/v2/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn v1_served_when_gate_on() {
        let resp = app(true)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// TD-V1SUNSET-1 census (2026-08-28): pin the property the sunset's contract
    /// depends on — production registers NO /api/v1 routes, so this middleware
    /// only matters if `.layer()` fires for UNMATCHED paths. It does (probed
    /// empirically during the census after axum fallback semantics were
    /// questioned); this test keeps it true across axum upgrades.
    #[tokio::test]
    async fn unmatched_v1_path_still_gets_410_not_404() {
        use axum::{Router, body::Body, routing::get};
        use tower::ServiceExt;
        // NO /api/v1 routes — mirrors production post-deletion.
        let app: Router = Router::new()
            .route("/api/v2/ping", get(|| async { "v2" }))
            .layer(axum::middleware::from_fn_with_state(
                false,
                v1_sunset_middleware,
            ));
        let req = axum::http::Request::builder()
            .uri("/api/v1/collections")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status().as_u16(),
            410,
            "unmatched v1 path must be 410, else the sunset middleware is dead in production"
        );
    }

    /// The Step-5 gate counter counts both outcomes through the real middleware.
    #[tokio::test]
    async fn sunset_decisions_are_counted() {
        let read = |outcome: &'static str| {
            SUNSET_RESPONSES
                .get_metric_with_label_values(&[outcome])
                .map(|c| c.get())
                .unwrap_or(0.0)
        };
        let gone_before = read("gone");
        let served_before = read("compat_served");

        // compat OFF: /api/v1 -> gone+1; /api/v2 untouched.
        let resp = app(false)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::GONE);

        // compat ON: /api/v1 -> compat_served+1 (passes through to the route).
        let resp = app(true)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        assert_eq!(read("gone") - gone_before, 1.0);
        assert_eq!(read("compat_served") - served_before, 1.0);
    }
}
