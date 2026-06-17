//! Prometheus instruments for the queue subsystem. Mounted under the same
//! `/metrics` endpoint as the rest of the ProximaDB process via the main
//! server's metrics aggregator.

use once_cell::sync::Lazy;
use prometheus::{
    HistogramVec, IntCounterVec, IntGaugeVec, register_histogram_vec, register_int_counter_vec,
    register_int_gauge_vec,
};

pub static QUEUE_DEPTH: Lazy<IntGaugeVec> = Lazy::new(|| {
    register_int_gauge_vec!(
        "proximadb_queue_depth",
        "Current depth of the memory tier per (topic, partition).",
        &["topic", "partition"]
    )
    .expect("metric registration")
});

pub static QUEUE_SEND_LATENCY_MS: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "proximadb_queue_send_latency_ms",
        "Latency from Producer::send to receipt return (includes fsync in Strict mode).",
        &["topic", "sync_mode"],
        vec![0.5, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0]
    )
    .expect("metric registration")
});

pub static QUEUE_BACKPRESSURE: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "proximadb_queue_backpressure_total",
        "Backpressure events by topic + severity.",
        &["topic", "severity"]
    )
    .expect("metric registration")
});

pub static QUEUE_FSYNC_BATCH_SIZE: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "proximadb_queue_fsync_batch_size",
        "Number of messages amortized per group-commit fsync.",
        &["topic", "partition"],
        vec![1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0]
    )
    .expect("metric registration")
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_metrics_register_and_accept_labeled_observations() {
        let depth = QUEUE_DEPTH.with_label_values(&["orders", "0"]);
        depth.set(7);
        assert_eq!(depth.get(), 7);

        let backpressure = QUEUE_BACKPRESSURE.with_label_values(&["orders", "soft"]);
        let before = backpressure.get();
        backpressure.inc();
        assert_eq!(backpressure.get(), before + 1);

        QUEUE_SEND_LATENCY_MS
            .with_label_values(&["orders", "lazy"])
            .observe(2.5);
        QUEUE_FSYNC_BATCH_SIZE
            .with_label_values(&["orders", "0"])
            .observe(16.0);
    }
}
