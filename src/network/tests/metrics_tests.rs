use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::monitoring::MetricsCollector;
use crate::network::metrics_service::{MetricsService, MetricsServiceConfig};

#[tokio::test]
async fn prometheus_metrics_endpoint_returns_payload() {
    let collector = std::sync::Arc::new(MetricsCollector::new());
    let service = MetricsService::new(MetricsServiceConfig::default(), collector);
    let app = service.create_router();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/prometheus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let body = hyper::body::to_bytes(response.into_body())
        .await
        .expect("body read");
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("proximadb"),
        "expected proximadb metrics in prometheus output, got: {}",
        body_str
    );
}

#[tokio::test]
async fn prometheus_metrics_endpoint_returns_valid_format() {
    let collector = std::sync::Arc::new(MetricsCollector::new());
    let service = MetricsService::new(MetricsServiceConfig::default(), collector);
    let app = service.create_router();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/prometheus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let body = hyper::body::to_bytes(response.into_body())
        .await
        .expect("body read");
    let body_str = String::from_utf8_lossy(&body);

    // Verify key metrics are present
    assert!(
        body_str.contains("proximadb_cpu_usage_percent"),
        "expected cpu metric"
    );
    assert!(
        body_str.contains("proximadb_memory_used_bytes"),
        "expected memory metric"
    );
    assert!(
        body_str.contains("proximadb_uptime_seconds"),
        "expected uptime metric"
    );
    assert!(
        body_str.contains("proximadb_vectors_total"),
        "expected vectors metric"
    );
    assert!(
        body_str.contains("proximadb_queries_total"),
        "expected queries metric"
    );

    // Verify HELP and TYPE comments are present
    assert!(body_str.contains("# HELP"), "expected HELP comments");
    assert!(body_str.contains("# TYPE"), "expected TYPE comments");
}
