//! offset_store integration tests — proves Consumer::ack writes the
//! committed offset to disk atomically and concurrently-safe.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use proximadb_queue::{Message, QueueClient, QueueConfig, TopicConfig};
use serde_json::Value;
use tempfile::TempDir;

fn cfg(root: &std::path::Path, name: &str, partition_count: u32) -> QueueConfig {
    let mut topics = HashMap::new();
    topics.insert(
        name.to_string(),
        TopicConfig {
            partition_count,
            memory_capacity: 256,
            ..Default::default()
        },
    );
    QueueConfig {
        root: format!("file://{}", root.display()),
        topics,
        ..QueueConfig::default()
    }
}

/// After acking 7 of 10 messages, the on-disk `offset.meta` for the
/// partition that holds them contains a committed_offset of at least 6
/// (offsets are 0-indexed, so the 7th message has offset 6).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ack_commits_offset_to_disk() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    let client = QueueClient::open(cfg(root, "t", 1)).await.expect("open");

    let producer = client.producer();
    for i in 0..10u32 {
        producer
            .send(Message::new("t", "tenant-a", vec![i as u8]))
            .await
            .expect("send");
    }

    let consumer = client.consumer("g");
    consumer.subscribe("t", &[0]).await.expect("subscribe");
    let batch = consumer
        .poll(7, Duration::from_millis(200))
        .await
        .expect("poll");
    assert_eq!(batch.len(), 7, "should poll 7 messages");

    // To ack, we need the MessageIds. The consumer's in_flight tracker
    // has them but isn't part of the public API; we reconstruct them
    // from the deterministic memory-tier offset assignment (partition
    // 0, segment 0, offsets 0..7).
    let ack_ids: Vec<proximadb_queue::MessageId> = (0..7u64)
        .map(|offset| proximadb_queue::MessageId::new(0, 0, offset))
        .collect();
    consumer.ack(&ack_ids).await.expect("ack");

    // offset.meta must now exist on disk.
    let meta_path = root.join("t").join("0").join("offset.meta");
    assert!(meta_path.exists(), "offset.meta should be written");
    let body = std::fs::read_to_string(&meta_path).expect("read meta");
    let parsed: Value = serde_json::from_str(&body).expect("parse json");
    assert_eq!(parsed["group"], "g");
    let committed = parsed["committed_offset"].as_u64().expect("u64");
    // 7 acks of offsets 0..6 → max = 6.
    assert_eq!(committed, 6, "committed_offset should be 6 after 7 acks");
}

/// 100 concurrent acks against the same partition must not corrupt
/// `offset.meta`. The final on-disk file must be parseable and contain
/// a valid (monotonically reachable) committed_offset.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offset_commit_is_atomic() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    let client = QueueClient::open(cfg(root, "t", 1)).await.expect("open");

    let producer = client.producer();
    for i in 0..100u32 {
        producer
            .send(Message::new("t", "tenant-a", vec![i as u8]))
            .await
            .expect("send");
    }

    let consumer = Arc::new(client.consumer("g"));
    consumer.subscribe("t", &[0]).await.expect("subscribe");
    // Drain so the in_flight tracker has all 100.
    let mut polled = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while polled < 100 && std::time::Instant::now() < deadline {
        let batch = consumer
            .poll(32, Duration::from_millis(50))
            .await
            .expect("poll");
        polled += batch.len();
    }
    assert_eq!(polled, 100, "should poll all 100 first");

    // 100 ack tasks, one per offset. Each task acks a single MessageId.
    let mut handles = Vec::new();
    for offset in 0..100u64 {
        let c = consumer.clone();
        handles.push(tokio::spawn(async move {
            let id = proximadb_queue::MessageId::new(0, 0, offset);
            c.ack(&[id]).await
        }));
    }
    for h in handles {
        h.await.unwrap().expect("ack");
    }

    // After the storm settles, offset.meta must be parseable and the
    // committed_offset must be a reachable value (≤ 99). The exact
    // value depends on scheduling — what matters is that we never
    // observe a corrupted file.
    let meta_path = root.join("t").join("0").join("offset.meta");
    assert!(meta_path.exists(), "offset.meta should exist");
    let body = std::fs::read_to_string(&meta_path).expect("read meta");
    let parsed: Value = serde_json::from_str(&body).expect("parse json (no corruption)");
    let committed = parsed["committed_offset"].as_u64().expect("u64");
    assert!(
        committed <= 99,
        "committed_offset {committed} must be within range 0..=99"
    );
    assert_eq!(parsed["group"], "g");
}
