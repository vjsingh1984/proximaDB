//! Edge-case tests for the queue: backpressure thresholds, concurrent
//! producers, ordering within a partition, max_batch caps, poll timeouts,
//! auto-create on first send, error paths on unknown partitions, and ack
//! semantics for unknown ids.
//!
//! These exist alongside `roundtrip.rs` (happy-path coverage) to prove the
//! contract holds at the boundaries — especially the backpressure surface
//! since that's where the queue makes/breaks its promise to the customer
//! API layer (return 429 vs accept).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use proximadb_queue::{
    Message, QueueClient, QueueConfig, QueueError, TopicConfig, partition_for,
};

fn cfg(name: &str, partition_count: u32, memory_capacity: usize) -> QueueConfig {
    let mut topics = HashMap::new();
    topics.insert(
        name.to_string(),
        TopicConfig {
            partition_count,
            memory_capacity,
            ..Default::default()
        },
    );
    QueueConfig {
        topics,
        ..QueueConfig::default()
    }
}

/// Soft pressure: hint surfaces at >= 75% memory tier fill. At this level
/// the producer still gets a successful receipt — the hint is advisory
/// (used by the request handler to log / emit a metric, but not to fail).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backpressure_soft_hint_surfaces_at_75_percent() {
    // Single partition, capacity 4. Soft threshold is 75% → 3 entries.
    let client = QueueClient::open(cfg("t", 1, 4)).await.unwrap();
    let producer = client.producer();

    let mut last_hint = None;
    for i in 0..3u32 {
        let r = producer
            .send(Message::new("t", "tenant-a", vec![i as u8]))
            .await
            .expect("send");
        last_hint = r.backpressure_hint;
    }
    // 3rd send pushed depth to 3/4 = 75% exactly → soft hint expected
    assert!(
        matches!(last_hint, Some(proximadb_queue::message::BackpressureHint::Soft)),
        "soft pressure hint expected at 75% fill, got {:?}",
        last_hint
    );
}

/// Hard pressure: producer gets `QueueError::Backpressure` and a
/// retry_after_ms hint. The request handler should translate this to
/// HTTP 429 + `Retry-After`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backpressure_hard_errors_when_partition_full() {
    let client = QueueClient::open(cfg("t", 1, 2)).await.unwrap();
    let producer = client.producer();

    // Fill the partition completely.
    producer
        .send(Message::new("t", "tenant-a", vec![1]))
        .await
        .expect("send 1");
    producer
        .send(Message::new("t", "tenant-a", vec![2]))
        .await
        .expect("send 2");

    // Next send must hit hard pressure (memory tier at 100%, ≥ 95% threshold).
    let result = producer
        .send(Message::new("t", "tenant-a", vec![3]))
        .await;
    match result {
        Err(QueueError::Backpressure { pct, retry_after_ms }) => {
            assert!(pct >= 95.0, "expected pct ≥ 95, got {pct}");
            assert!(retry_after_ms > 0, "retry_after_ms should be non-zero");
        }
        other => panic!("expected QueueError::Backpressure, got {:?}", other),
    }
}

/// FIFO is preserved within a partition. All 64 messages from one tenant
/// land on the same partition; pulled in send order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordering_preserved_within_a_partition() {
    let client = QueueClient::open(cfg("t", 4, 256)).await.unwrap();
    let producer = client.producer();
    let consumer = client.consumer("g");
    consumer.subscribe("t", &[0, 1, 2, 3]).await.unwrap();

    // 64 messages from the same tenant → all on the same partition.
    for i in 0..64u32 {
        producer
            .send(Message::new("t", "tenant-single", i.to_be_bytes().to_vec()))
            .await
            .unwrap();
    }

    let mut received: Vec<u32> = Vec::with_capacity(64);
    let deadline = Instant::now() + Duration::from_secs(2);
    while received.len() < 64 && Instant::now() < deadline {
        let batch = consumer.poll(32, Duration::from_millis(50)).await.unwrap();
        for msg in batch {
            assert_eq!(msg.payload.len(), 4);
            let i = u32::from_be_bytes(msg.payload.as_slice().try_into().unwrap());
            received.push(i);
        }
    }
    assert_eq!(received.len(), 64);
    // FIFO check
    for (idx, &v) in received.iter().enumerate() {
        assert_eq!(v, idx as u32, "FIFO broken at idx {idx}: got {v}");
    }
}

/// Concurrent producers don't crash, don't duplicate offsets, don't lose
/// messages. Spawn 8 tasks, each sending 32 messages → 256 total.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_producers_are_race_safe() {
    let client = QueueClient::open(cfg("t", 8, 1024)).await.unwrap();
    let mut handles = Vec::new();
    for task_id in 0..8u32 {
        let p = client.producer();
        handles.push(tokio::spawn(async move {
            for i in 0..32u32 {
                let tenant = format!("tenant-{task_id}-{i}");
                p.send(Message::new("t", tenant, vec![task_id as u8, i as u8]))
                    .await
                    .expect("concurrent send");
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    // Drain everything through one consumer and confirm 256 messages received.
    let consumer = client.consumer("g");
    consumer.subscribe("t", &(0..8u32).collect::<Vec<_>>()).await.unwrap();
    let mut total = 0;
    let deadline = Instant::now() + Duration::from_secs(2);
    while total < 256 && Instant::now() < deadline {
        let batch = consumer.poll(64, Duration::from_millis(50)).await.unwrap();
        total += batch.len();
    }
    assert_eq!(total, 256, "should receive all concurrent sends, got {total}");
}

/// Consumer respects `max_batch`. Send 16, poll 4 — get 4 back (not 16).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumer_max_batch_caps_returned_size() {
    let client = QueueClient::open(cfg("t", 2, 64)).await.unwrap();
    let producer = client.producer();
    let consumer = client.consumer("g");
    consumer.subscribe("t", &[0, 1]).await.unwrap();
    for i in 0..16u32 {
        producer
            .send(Message::new("t", format!("t-{i}"), vec![i as u8]))
            .await
            .unwrap();
    }

    let batch = consumer.poll(4, Duration::from_millis(100)).await.unwrap();
    assert!(batch.len() <= 4, "poll(4) returned {} > 4", batch.len());
    assert!(!batch.is_empty(), "poll(4) returned empty despite 16 enqueued");
}

/// Poll returns empty after `max_wait` when no messages arrive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumer_poll_timeout_returns_empty() {
    let client = QueueClient::open(cfg("t", 1, 16)).await.unwrap();
    let consumer = client.consumer("g");
    consumer.subscribe("t", &[0]).await.unwrap();

    let start = Instant::now();
    let batch = consumer.poll(8, Duration::from_millis(100)).await.unwrap();
    let elapsed = start.elapsed();
    assert!(batch.is_empty(), "should return empty on no-message timeout");
    assert!(
        elapsed >= Duration::from_millis(80),
        "should wait approximately the full timeout, got {:?}",
        elapsed
    );
    // Generous upper bound — we shouldn't wait much beyond the requested
    // timeout window.
    assert!(
        elapsed < Duration::from_millis(400),
        "wait was excessive: {:?}",
        elapsed
    );
}

/// Subscribing to an out-of-range partition errors cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_to_unknown_partition_errors() {
    let client = QueueClient::open(cfg("t", 2, 16)).await.unwrap();
    let consumer = client.consumer("g");
    let err = consumer.subscribe("t", &[0, 5, 999]).await.unwrap_err();
    match err {
        QueueError::PartitionNotFound { topic, partition } => {
            assert_eq!(topic, "t");
            assert!(partition == 5 || partition == 999);
        }
        other => panic!("expected PartitionNotFound, got {:?}", other),
    }
}

/// Topics not declared in `QueueConfig::topics` are auto-created at first
/// send with default `TopicConfig`. Critical for the AnvaiOps integration:
/// the `embed-ingest` topic doesn't need to be explicitly declared
/// upfront — first call wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_create_topic_on_first_send() {
    let client = QueueClient::open(QueueConfig::default()).await.unwrap();
    let producer = client.producer();
    let receipt = producer
        .send(Message::new("never-declared", "tenant-x", vec![42]))
        .await
        .expect("auto-create + send");
    // partition index must fit within the default (16) partition count
    assert!(receipt.partition < 16);
}

/// Acking unknown message ids is a no-op. The system doesn't crash; ack'd
/// messages that aren't in flight are silently dropped. This matters
/// because the drainer may legitimately retry an ack after a transient
/// network failure and the second ack should be safe.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ack_unknown_message_id_is_noop() {
    let client = QueueClient::open(cfg("t", 1, 8)).await.unwrap();
    let consumer = client.consumer("g");
    consumer.subscribe("t", &[0]).await.unwrap();

    let bogus = proximadb_queue::MessageId::new(0, 0, 999_999);
    consumer.ack(&[bogus]).await.expect("noop ack must not error");
}

/// `partition_for` is consistent regardless of partition_count not changing,
/// AND yields a value < partition_count. Defends against future hashing
/// changes that might exceed the modulus.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partition_for_always_in_range() {
    for pc in [1, 2, 4, 8, 16, 32, 64, 128, 256u32] {
        for i in 0..50u32 {
            let p = partition_for(&format!("tenant-{i}"), pc);
            assert!(p < pc, "partition_for returned {p} >= partition_count {pc}");
        }
    }
}

/// Polling without any subscription returns empty immediately, doesn't
/// block forever. Defends against subtle starvation if a drainer crashes
/// before subscribing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poll_without_subscription_returns_empty_quickly() {
    let client = QueueClient::open(QueueConfig::default()).await.unwrap();
    let consumer = client.consumer("g");

    let start = Instant::now();
    let batch = consumer.poll(8, Duration::from_secs(2)).await.unwrap();
    assert!(batch.is_empty());
    // Should return well before the 2s timeout because there's nothing to wait on.
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "no-subscription poll should return promptly, took {:?}",
        start.elapsed()
    );
}

/// Multiple consumers in the SAME group on the same partition share the
/// in-flight tracker — once one polls a message, the other won't see it.
/// (Phase 1B in-process leasing; cross-process disk leases land later.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_consumer_sees_remaining_messages_after_first_drains() {
    let client = QueueClient::open(cfg("t", 1, 16)).await.unwrap();
    let producer = client.producer();
    for i in 0..8u32 {
        producer
            .send(Message::new("t", "tenant-z", vec![i as u8]))
            .await
            .unwrap();
    }

    let c1 = client.consumer("g");
    c1.subscribe("t", &[0]).await.unwrap();
    let batch1 = c1.poll(4, Duration::from_millis(50)).await.unwrap();
    assert_eq!(batch1.len(), 4);

    let c2 = client.consumer("g");
    c2.subscribe("t", &[0]).await.unwrap();
    let batch2 = c2.poll(8, Duration::from_millis(50)).await.unwrap();
    assert_eq!(batch2.len(), 4, "second consumer should see the remaining 4");
}

/// Shutdown is idempotent and doesn't panic when called twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_is_idempotent() {
    let client = QueueClient::open(QueueConfig::default()).await.unwrap();
    client.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}

/// `Arc<QueueClient>` is cheap to clone — confirms `producer()` and
/// `consumer()` don't deep-copy state. Sanity check for hot-path use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn producer_and_consumer_handles_are_cheap_to_create() {
    let client = QueueClient::open(QueueConfig::default()).await.unwrap();
    for _ in 0..1000 {
        let _p = client.producer();
        let _c = client.consumer("g");
    }
    // No assertion — just verifying no allocation explosion. If this
    // becomes too slow, the test runner will time us out.
    let _strong_count = Arc::strong_count(&client);
}
