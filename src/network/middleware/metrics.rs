//! Per-route REST metrics middleware.
//!
//! Emits Prometheus series for every REST request so operators get latency and
//! error-rate visibility per endpoint. Labels use the *matched route pattern*
//! (e.g. `/api/v2/collections/:collection_id`), not the raw path, to keep label
//! cardinality bounded. Scraped via the existing `/metrics/prometheus` surface.

use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::Request;
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
pub async fn metrics_middleware(request: Request<Body>, next: Next<Body>) -> Response {
    let method = request.method().as_str().to_owned();
    // Matched route pattern bounds cardinality; unmatched (fallback) requests
    // share a single `<unmatched>` bucket so random 404 paths can't explode it.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| "<unmatched>".to_owned());

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

    response
}
