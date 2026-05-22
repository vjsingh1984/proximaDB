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
///
/// Uses a SHORT lease_duration so that the prior session's leftover
/// `lease.meta` expires fast — these tests simulate sequential
/// restart, not multi-replica contention. Multi-replica lease behavior
/// is exercised in `tests/leases.rs`.
fn cfg_with_topic(root: &std::path::Path, name: &str, partition_count: u32) -> QueueConfig {
    let mut topics = HashMap::new();
    topics.insert(
        name.to_string(),
        TopicConfig {
            partition_count,
            lease_duration: Duration::from_millis(200),
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
                .send(Message::new(
                    "embed-ingest",
                    "tenant-a",
                    i.to_be_bytes().to_vec(),
                ))
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

    assert_eq!(
        received.len(),
        50,
        "all 50 persisted messages must be recovered"
    );
    // FIFO across the restart boundary.
    for (idx, &v) in received.iter().enumerate() {
        assert_eq!(v, idx as u32, "recovery broke FIFO at idx {idx}: got {v}");
    }
}

/// Recovery skips messages already acked on the previous session. Send
/// 50, ack 30, drop, reopen → poll returns exactly 20.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_skips_already_acked_messages() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    // Session 1: send 50, poll all, ack the first 30.
    {
        let client = QueueClient::open(cfg_with_topic(root, "embed-ingest", 1))
            .await
            .expect("session1 open");
        let producer = client.producer();
        for i in 0..50u32 {
            producer
                .send(Message::new(
                    "embed-ingest",
                    "tenant-a",
                    i.to_be_bytes().to_vec(),
                ))
                .await
                .expect("send");
        }
        let consumer = client.consumer("g");
        consumer
            .subscribe("embed-ingest", &[0])
            .await
            .expect("subscribe");

        // Drain at least 30 from the memory tier.
        let mut polled = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while polled < 30 && std::time::Instant::now() < deadline {
            let batch = consumer
                .poll(32, std::time::Duration::from_millis(50))
                .await
                .unwrap();
            polled += batch.len();
        }
        assert!(polled >= 30, "should poll at least 30; got {polled}");

        // Ack the first 30 offsets (0..29). Consumer.ack will commit
        // max=29 to offset.meta.
        let ack_ids: Vec<proximadb_queue::MessageId> = (0..30u64)
            .map(|o| proximadb_queue::MessageId::new(0, 0, o))
            .collect();
        consumer.ack(&ack_ids).await.expect("ack");
        client.shutdown().await.expect("shutdown");
    }

    // Session 2: open at the same root. Recovery must skip offsets
    // 0..=29 and only enqueue 30..=49 → poll returns exactly 20.
    let client = QueueClient::open(cfg_with_topic(root, "embed-ingest", 1))
        .await
        .expect("session2 open");
    let consumer = client.consumer("g");
    consumer
        .subscribe("embed-ingest", &[0])
        .await
        .expect("subscribe");

    let mut received: Vec<u32> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while received.len() < 21 && std::time::Instant::now() < deadline {
        let batch = consumer
            .poll(32, std::time::Duration::from_millis(50))
            .await
            .unwrap();
        for msg in batch {
            assert_eq!(msg.payload.len(), 4);
            let v = u32::from_be_bytes(msg.payload.as_slice().try_into().unwrap());
            received.push(v);
        }
    }

    assert_eq!(
        received.len(),
        20,
        "should recover exactly 20 unacked messages"
    );
    // FIFO across the restart, starting at payload value 30.
    for (idx, &v) in received.iter().enumerate() {
        assert_eq!(v, (idx + 30) as u32, "expected {} got {v}", idx + 30);
    }
}

/// Phase 2D-c: fresh-node rebuild. Simulates ECS pod reschedule onto a
/// new node with empty local disk but a populated object archive.
/// Recovery must load segments straight from the archive when the disk
/// has nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_node_recovers_from_archive_when_disk_is_empty() {
    let disk_tmp = TempDir::new().expect("disk");
    let archive_tmp = TempDir::new().expect("archive");
    let disk_root = disk_tmp.path();
    let archive_root = archive_tmp.path();

    let mut topics = HashMap::new();
    topics.insert(
        "embed-ingest".to_string(),
        TopicConfig {
            partition_count: 1,
            disk_rotation_size_mb: 0, // force rotation to seal segments
            ..Default::default()
        },
    );
    let cfg = |root: &std::path::Path| QueueConfig {
        root: format!("file://{}", root.display()),
        object_archive: Some(format!("file://{}", archive_root.display())),
        topics: topics.clone(),
        ..QueueConfig::default()
    };

    // Session 1: send messages on the original "node", wait for upload
    // to mirror sealed segments to the archive.
    {
        let client = QueueClient::open(cfg(disk_root)).await.expect("session1");
        let producer = client.producer();
        for i in 0..5u32 {
            producer
                .send(Message::new(
                    "embed-ingest",
                    "tenant-a",
                    i.to_be_bytes().to_vec(),
                ))
                .await
                .expect("send");
        }
        // Wait for at least segment 0 to mirror.
        let arch_seg0 = archive_root
            .join("embed-ingest")
            .join("0")
            .join("0000000000.qseg");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !arch_seg0.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(arch_seg0.exists(), "upload must mirror segment 0");
        client.shutdown().await.expect("shutdown");
    }

    // Simulate "fresh node": brand-new disk_root with NOTHING on it.
    // The archive_root persists (it's a separate TempDir). Recovery on
    // the new disk_root must pull segments from the archive.
    let fresh_disk_tmp = TempDir::new().expect("fresh disk");
    let fresh_disk = fresh_disk_tmp.path();

    let client = QueueClient::open(cfg(fresh_disk))
        .await
        .expect("fresh node open");
    let consumer = client.consumer("g");
    consumer
        .subscribe("embed-ingest", &[0])
        .await
        .expect("subscribe");

    // Drain until we've received the messages from the archive. Only
    // segment 0 was guaranteed mirrored above; the test asserts at
    // least 1 message comes back — proving the archive fallback works.
    let mut received: Vec<u32> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while received.is_empty() && std::time::Instant::now() < deadline {
        let batch = consumer
            .poll(8, Duration::from_millis(100))
            .await
            .unwrap();
        for msg in batch {
            assert_eq!(msg.payload.len(), 4);
            let v = u32::from_be_bytes(msg.payload.as_slice().try_into().unwrap());
            received.push(v);
        }
    }
    // The core archive-fallback proof: with an empty local disk,
    // recovery still surfaced messages — they could only have come
    // from the archive. Exactly which subset of [0..5) we get depends
    // on the upload+rotation race in session 1 (this test doesn't
    // attempt to control that; it only asserts the fallback path
    // works at all).
    assert!(
        !received.is_empty(),
        "fresh-node recovery must pull messages from archive when disk is empty",
    );
    for (idx, &v) in received.iter().enumerate() {
        assert!(
            (v as usize) < 5,
            "recovered value {v} out of range; received={received:?}",
        );
        if idx > 0 {
            assert!(
                v > received[idx - 1],
                "FIFO broken at idx {idx}: {} → {v}; received={received:?}",
                received[idx - 1],
            );
        }
    }

    client.shutdown().await.expect("shutdown");
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
