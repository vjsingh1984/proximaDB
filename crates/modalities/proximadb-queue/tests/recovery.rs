//! Recovery integration tests — proves the queue's durability promise
//! across `QueueClient` open/close cycles.
//!
//! Each test uses a single `TempDir` that persists across both the
//! sending client and the recovering client, simulating a process
//! restart pointed at the same on-disk root.

use std::collections::HashMap;
use std::time::Duration;

use proximadb_queue::{Message, QueueClient, QueueConfig, TopicConfig};
use tempfile::TempDir;

/// Topic-declared config bound to a specific root path so a follow-on
/// `QueueClient::open` against the same TempDir resumes from disk.
fn cfg_with_topic(root: &std::path::Path, name: &str, partition_count: u32) -> QueueConfig {
    let mut topics = HashMap::new();
    topics.insert(
        name.to_string(),
        TopicConfig {
            partition_count,
            ..Default::default()
        },
    );
    QueueConfig {
        root: format!("file://{}", root.display()),
        topics,
        ..QueueConfig::default()
    }
}

/// The single most important durability guarantee: messages persisted on
/// the first client come back on the second client. Open client → send
/// 50 messages from one tenant → drop client → open new client pointed
/// at the same root → poll → all 50 messages return in FIFO order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_recovers_unconsumed_messages() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    // Session 1: send 50 messages and shut down. Each Strict-mode send
    // returns only after the group-commit fsync, so by the time the
    // loop exits all messages are on disk.
    {
        let client = QueueClient::open(cfg_with_topic(root, "embed-ingest", 1))
            .await
            .expect("session1 open");
        let producer = client.producer();
        for i in 0..50u32 {
            producer
                .send(Message::new("embed-ingest", "tenant-a", i.to_be_bytes().to_vec()))
                .await
                .expect("send");
        }
        client.shutdown().await.expect("shutdown");
    }

    // Session 2: open again at the same root. Recovery replays segments
    // into the memory tier before QueueClient::open returns.
    let client = QueueClient::open(cfg_with_topic(root, "embed-ingest", 1))
        .await
        .expect("session2 open");
    let consumer = client.consumer("g");
    consumer
        .subscribe("embed-ingest", &[0])
        .await
        .expect("subscribe");

    let mut received: Vec<u32> = Vec::with_capacity(50);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while received.len() < 50 && std::time::Instant::now() < deadline {
        let batch = consumer.poll(32, Duration::from_millis(50)).await.unwrap();
        for msg in batch {
            assert_eq!(msg.payload.len(), 4);
            let i = u32::from_be_bytes(msg.payload.as_slice().try_into().unwrap());
            received.push(i);
        }
    }

    assert_eq!(received.len(), 50, "all 50 persisted messages must be recovered");
    // FIFO across the restart boundary.
    for (idx, &v) in received.iter().enumerate() {
        assert_eq!(v, idx as u32, "recovery broke FIFO at idx {idx}: got {v}");
    }
}

/// Recovery on a fresh empty root is a clean no-op — no errors, no
/// surprises. Useful for the first deployment / dev-container case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_is_a_noop_for_empty_root() {
    let tmp = TempDir::new().expect("tempdir");
    let client = QueueClient::open(cfg_with_topic(tmp.path(), "topic-x", 4))
        .await
        .expect("open on empty root must succeed");
    // No segments → memory tier should be empty after open.
    let consumer = client.consumer("g");
    consumer.subscribe("topic-x", &[0, 1, 2, 3]).await.unwrap();
    let batch = consumer.poll(8, Duration::from_millis(50)).await.unwrap();
    assert!(batch.is_empty(), "empty root should yield no messages");
}
