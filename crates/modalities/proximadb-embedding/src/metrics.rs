//! Prometheus instruments for the embedding service.
//!
//! Exposed under the existing `/metrics` endpoint in `proximadb-server`.
//! Metric names follow the `proximadb_embed_*` namespace so existing
//! dashboards and alerts can be extended without churn.

use once_cell::sync::Lazy;
use prometheus::{
    HistogramVec, IntCounterVec, IntGaugeVec, register_histogram_vec, register_int_counter_vec,
    register_int_gauge_vec,
};

pub static EMBED_LATENCY_MS: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "proximadb_embed_latency_ms",
        "Latency of a single embedding batch in milliseconds.",
        &["route", "priority"],
        // 1ms .. 10s
        vec![
            1.0, 5.0, 10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0
        ]
    )
    .expect("metric registration")
});

pub static EMBED_BATCH_SIZE: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "proximadb_embed_batch_size",
        "Records per embedding batch dispatched to the scheduler.",
        &["route", "priority"],
        vec![1.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0]
    )
    .expect("metric registration")
});

pub static EMBED_QUEUE_DEPTH: Lazy<IntGaugeVec> = Lazy::new(|| {
    register_int_gauge_vec!(
        "proximadb_embed_queue_depth",
        "Pending embedding tasks in each scheduler queue.",
        &["queue"]
    )
    .expect("metric registration")
});

pub static EMBED_FAILURES: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "proximadb_embed_failures_total",
        "Embedding task failures by route + cause.",
        &["route", "cause"]
    )
    .expect("metric registration")
});

pub static EMBED_WORK_STEALS: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "proximadb_embed_work_steals_total",
        "Count of async batches stolen by idle sync workers.",
        &[]
    )
    .expect("metric registration")
});

/// Record a successful embedding batch.
pub fn record_embed_success(route: &str, priority: &str, latency_ms: f64, batch_size: usize) {
    EMBED_LATENCY_MS
        .with_label_values(&[route, priority])
        .observe(latency_ms);
    EMBED_BATCH_SIZE
        .with_label_values(&[route, priority])
        .observe(batch_size as f64);
}

/// Record a failed embedding batch.
pub fn record_embed_failure(route: &str, cause: &str) {
    EMBED_FAILURES.with_label_values(&[route, cause]).inc();
}

/// Set the current queue depth for the named queue.
pub fn set_queue_depth(queue: &str, depth: i64) {
    EMBED_QUEUE_DEPTH.with_label_values(&[queue]).set(depth);
}
