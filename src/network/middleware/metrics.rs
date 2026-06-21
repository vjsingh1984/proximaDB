#![allow(clippy::unwrap_used, clippy::expect_used)]
// dedicated metric-registration module: static registration is infallible / fail-fast at startup; lazy_static can't carry per-site allows
//! Per-route REST metrics middleware.
//!
//! Emits Prometheus series for every REST request so operators get latency and
//! error-rate visibility per endpoint. Labels use the *matched route pattern*
//! (e.g. `/api/v2/collections/:collection_id`), not the raw path, to keep label
//! cardinality bounded. Scraped via the existing `/metrics/prometheus` surface.
use axum::extract::MatchedPath;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;

use lazy_static::lazy_static;
use prometheus::{CounterVec, HistogramVec, register_counter_vec, register_histogram_vec};

lazy_static! {
    static ref HTTP_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "proximadb_http_requests_total",
        "Total ProximaDB REST requests by route, method, and status",
        &["route", "method", "status"]
    )
    .expect("register proximadb_http_requests_total");
    static ref HTTP_REQUEST_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "proximadb_http_request_duration_seconds",
        "ProximaDB REST request latency (seconds) by route and method",
        &["route", "method"]
    )
    .expect("register proximadb_http_request_duration_seconds");
}

/// Records latency + count for each request, labelled by matched route.
pub async fn metrics_middleware(request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    // Matched route pattern bounds cardinality; unmatched (fallback) requests
    // share a single `<unmatched>` bucket so random 404 paths can't explode it.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| "<unmatched>".to_owned());

    // KOU result-egress: classify the client once, before the request is
    // consumed by the handler. Same IP-extraction seam as the rate limiter.
    let edge = crate::metrics::consumption_metrics::EdgePolicyContext::classify(
        crate::network::middleware::rate_limit::get_client_ip(&request),
    );

    let start = Instant::now();
    let response = next.run(request).await;
    let elapsed = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    HTTP_REQUEST_DURATION_SECONDS
        .with_label_values(&[&route, &method])
        .observe(elapsed);
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[&route, &method, &status])
        .inc();

    // Meter response-body egress to the KOU meter (direction=result) when the
    // client is genuinely remote (the direct data-plane case) and the body size
    // is known. Buffered responses (e.g. `Json`) carry `Content-Length`;
    // streamed/chunked bodies are not buffered here and are left uncounted. The
    // common in-cluster gateway client classifies free → this is a no-op for it.
    if edge.kou_locality.is_chargeable_egress()
        && let Some(len) = response
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
    {
        edge.record_result_egress(None, len);
    }

    response
}
