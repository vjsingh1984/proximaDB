//! Memory-tier round-trip: produce → consume → ack across multiple tenants.
//!
//! Exercises the public API surface without touching disk/object tiers,
//! which are deliberately stubbed in this Phase 1B scaffold commit.

use std::collections::HashMap;
use std::time::Duration;

use proximadb_queue::{Message, QueueClient, QueueConfig, SyncMode, TopicConfig, partition_for};
use tempfile::TempDir;

fn cfg_with_topic(name: &str, partition_count: u32) -> (TempDir, QueueConfig) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut topics = HashMap::new();
    topics.insert(
        name.to_string(),
        TopicConfig {
            partition_count,
            ..Default::default()
        },
    );
    let cfg = QueueConfig {
        root: format!("file://{}", tmp.path().display()),
        topics,
        ..QueueConfig::default()
    };
    (tmp, cfg)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn produce_and_consume_round_trip() {
    let (_tmp, cfg) = cfg_with_topic("embed-ingest", 4);
    let client = QueueClient::open(cfg).await.expect("open");
    let producer = client.producer();
    let consumer = client.consumer("test-group");

    // Subscribe to all 4 partitions.
    consumer
        .subscribe("embed-ingest", &[0, 1, 2, 3])
        .await
        .expect("subscribe");

    // Send 8 messages across 2 distinct tenants. Tenants land on
    // deterministic partitions; we don't assume which.
    let mut sent_ids = Vec::new();
    for i in 0..8 {
        let tenant = if i % 2 == 0 { "tenant-a" } else { "tenant-b" };
        let payload = format!("payload-{i}").into_bytes();
        let receipt = producer
            .send(Message::new("embed-ingest", tenant, payload))
            .await
            .expect("send");
        sent_ids.push(receipt.message_id);
        // Strict-mode default — fsynced_at populated (Phase 1B approximate).
        assert!(receipt.fsynced_at.is_some());
    }

    // Poll until we've received all 8.
    let mut received = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while received.len() < 8 && std::time::Instant::now() < deadline {
        let batch = consumer
            .poll(16, Duration::from_millis(50))
            .await
            .expect("poll");
        received.extend(batch);
    }
    assert_eq!(received.len(), 8, "should receive all 8 sent messages");

    // Ack everything; subsequent polls should return empty.
    consumer.ack(&sent_ids).await.expect("ack");
    let empty = consumer
        .poll(16, Duration::from_millis(50))
        .await
        .expect("poll empty");
    assert!(empty.is_empty(), "no messages should remain after ack");

    client.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lazy_mode_returns_immediately_without_fsynced_at() {
    let (_tmp, mut cfg) = cfg_with_topic("billing", 2);
    cfg.default_sync_mode = SyncMode::Lazy;
    let client = QueueClient::open(cfg).await.expect("open");
    let producer = client.producer();

    let receipt = producer
        .send(Message::new("billing", "tenant-a", vec![1, 2, 3]))
        .await
        .expect("send");
    assert!(
        receipt.fsynced_at.is_none(),
        "Lazy mode should not stamp fsynced_at"
    );
}

#[test]
fn partition_assignment_is_stable_per_tenant() {
    let pc = 16u32;
    let a = partition_for("tenant-acme", pc);
    let b = partition_for("tenant-acme", pc);
    let c = partition_for("tenant-beta", pc);
    assert_eq!(a, b, "same tenant → same partition");
    // Different tenants MAY land on different partitions (not guaranteed,
    // but extremely likely for these two strings).
    let _ = c;
}
