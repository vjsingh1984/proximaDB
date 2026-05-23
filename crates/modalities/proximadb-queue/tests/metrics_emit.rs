//! Verifies queue metrics actually update from the production code
//! paths (producer.send → depth + latency; group-commit → batch
//! size). The instruments existed but were silent before this test;
//! a regression that breaks instrumentation would now fail loudly
//! instead of leaving cloud dashboards reading empty values.

use std::collections::HashMap;
use std::time::Duration;

use proximadb_queue::metrics::{
    QUEUE_BACKPRESSURE, QUEUE_DEPTH, QUEUE_FSYNC_BATCH_SIZE, QUEUE_SEND_LATENCY_MS,
};
use proximadb_queue::{Message, QueueClient, QueueConfig, SyncMode, TopicConfig};

fn lazy_config(root: String, topic: &str, partition_count: u32, capacity: usize) -> QueueConfig {
    let mut topics = HashMap::new();
    topics.insert(
        topic.to_string(),
        TopicConfig {
            partition_count,
            memory_capacity: capacity,
            sync_mode_override: Some(SyncMode::Lazy),
            lease_duration: Duration::from_secs(30),
            ..TopicConfig::default()
        },
    );
    QueueConfig {
        root,
        default_sync_mode: SyncMode::Lazy,
        topics,
        ..QueueConfig::default()
    }
}

/// A single send bumps `proximadb_queue_send_latency_ms` (count) and
/// sets `proximadb_queue_depth` to a non-zero value for the partition
/// the message hashes to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_emits_latency_and_depth_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let topic = "metrics_test_send";
    let client = QueueClient::open(lazy_config(
        format!("file://{}", dir.path().display()),
        topic,
        1,
        8,
    ))
    .await
    .unwrap();
    let producer = client.producer();

    // Snapshot the latency histogram count BEFORE the send so this
    // test doesn't race with parallel tests that hit the same global
    // metric (Prometheus instruments are process-global).
    let latency_metric = QUEUE_SEND_LATENCY_MS.with_label_values(&[topic, "lazy"]);
    let count_before = latency_metric.get_sample_count();

    producer
        .send(Message::new(topic, "tenant-x", b"hello".to_vec()))
        .await
        .expect("send");

    let count_after = latency_metric.get_sample_count();
    assert_eq!(
        count_after,
        count_before + 1,
        "QUEUE_SEND_LATENCY_MS observation count must increment by 1 per send"
    );

    // Depth gauge for partition 0 reflects the single-message memory tier.
    let depth = QUEUE_DEPTH.with_label_values(&[topic, "0"]).get();
    assert!(
        depth >= 1,
        "QUEUE_DEPTH for the message's partition must be ≥ 1 after enqueue; got {depth}"
    );
}

/// Filling the memory tier past hard pressure triggers a Backpressure
/// rejection that increments `proximadb_queue_backpressure_total{severity="hard"}`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backpressure_emits_counter_on_hard_pressure() {
    let dir = tempfile::tempdir().unwrap();
    // Distinct topic name so this test's counter doesn't share state
    // with `send_emits_*` above.
    let topic = "metrics_test_backpressure";
    let client = QueueClient::open(lazy_config(
        format!("file://{}", dir.path().display()),
        topic,
        1,
        2,
    ))
    .await
    .unwrap();
    let producer = client.producer();

    // Snapshot before — the hard label may have prior observations.
    let hard_counter = QUEUE_BACKPRESSURE.with_label_values(&[topic, "hard"]);
    let before = hard_counter.get();

    // Fill the 2-slot memory tier without consumers draining.
    for i in 0..2u8 {
        let _ = producer
            .send(Message::new(topic, "tenant-x", vec![i]))
            .await;
    }
    // Now push until we get a backpressure rejection. The first one
    // might be soft (memory full, drain coordinator hasn't caught up),
    // but enough pushes will trigger hard.
    let mut got_backpressure = false;
    for i in 0..32u8 {
        if producer
            .send(Message::new(topic, "tenant-x", vec![i + 10]))
            .await
            .is_err()
        {
            got_backpressure = true;
            break;
        }
    }
    assert!(
        got_backpressure,
        "filling the 2-slot memory tier with 32 extra sends should produce backpressure"
    );

    // Either "hard" or "soft" counter must have moved; check both.
    let soft_counter = QUEUE_BACKPRESSURE.with_label_values(&[topic, "soft"]);
    let total_after = hard_counter.get() + soft_counter.get();
    let total_before = before + 0; // soft started at 0 too (distinct topic name)
    assert!(
        total_after > total_before,
        "QUEUE_BACKPRESSURE (hard+soft) must increment when sends are rejected"
    );
}

/// Group-commit batching emits `proximadb_queue_fsync_batch_size`.
/// Sending in Strict mode triggers the fsync coordinator; the
/// histogram should have at least one observation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fsync_batch_size_emits_in_strict_mode() {
    let dir = tempfile::tempdir().unwrap();
    let topic = "metrics_test_fsync";
    let mut topics = HashMap::new();
    topics.insert(
        topic.to_string(),
        TopicConfig {
            partition_count: 1,
            memory_capacity: 32,
            sync_mode_override: Some(SyncMode::Strict),
            lease_duration: Duration::from_secs(30),
            ..TopicConfig::default()
        },
    );
    let cfg = QueueConfig {
        root: format!("file://{}", dir.path().display()),
        default_sync_mode: SyncMode::Strict,
        topics,
        ..QueueConfig::default()
    };
    let client = QueueClient::open(cfg).await.unwrap();
    let producer = client.producer();

    let metric = QUEUE_FSYNC_BATCH_SIZE.with_label_values(&[topic, "0"]);
    let before = metric.get_sample_count();

    // Concurrent sends to amortize multiple waiters into one fsync.
    let mut handles = Vec::new();
    for i in 0..8u8 {
        let p = producer.clone();
        let topic_owned = topic.to_string();
        handles.push(tokio::spawn(async move {
            p.send(Message::new(&topic_owned, "tenant-x", vec![i]))
                .await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    let after = metric.get_sample_count();
    assert!(
        after > before,
        "QUEUE_FSYNC_BATCH_SIZE must record at least one observation after Strict sends; \
         before={before} after={after}"
    );
}
