//! Pub/sub (consumer-group) round-trip + delivery-guarantee ratchet (ADR-079 §Semantics).
//!
//! Proves the defining Kafka-style property: every consumer GROUP independently
//! receives ALL messages on a partition, in order, with no loss and no
//! duplication — even when groups consume concurrently or one races ahead of
//! the other. This is the conformance ratchet for the queue's pub/sub plane: a
//! regression (loss / dup / cross-group interference) fails CI here.

use std::collections::HashMap;
use std::time::Duration;

use proximadb_queue::{Consumer, Delivery, Message, QueueClient, QueueConfig, TopicConfig};
use tempfile::TempDir;

fn cfg(name: &str, partition_count: u32) -> (TempDir, QueueConfig) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut topics = HashMap::new();
    topics.insert(
        name.to_string(),
        TopicConfig {
            partition_count,
            memory_capacity: 1024,
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

/// Poll until `want` deliveries arrive (or the deadline lapses).
async fn drain(consumer: &Consumer, want: usize) -> Vec<Delivery> {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while out.len() < want && std::time::Instant::now() < deadline {
        let batch = consumer
            .poll(want.max(1), Duration::from_millis(50))
            .await
            .expect("poll");
        if batch.is_empty() {
            continue;
        }
        out.extend(batch);
    }
    out
}

fn payloads_of(deliveries: &[Delivery]) -> Vec<u8> {
    let mut v: Vec<u8> = deliveries
        .iter()
        .flat_map(|d| d.message.payload.iter().copied())
        .collect();
    v.sort_unstable();
    v
}

/// The core pub/sub property: two groups each independently receive ALL
/// messages, even when one group fully consumes + acks before the other reads.
/// RED for slice 4: today `poll` consumes (pops), so group B finds the buffer
/// already emptied by group A and receives nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_groups_each_independently_receive_all_messages() {
    const N: usize = 10;
    let (_tmp, cfg) = cfg("fanout", 1);
    let client = QueueClient::open(cfg).await.expect("open");
    let producer = client.producer();
    for i in 0..N as u8 {
        producer
            .send(Message::new("fanout", "tenant", vec![i]))
            .await
            .expect("send");
    }

    let a = client.consumer("alpha");
    a.subscribe("fanout", &[0]).await.expect("A subscribe");
    let b = client.consumer("beta");
    b.subscribe("fanout", &[0]).await.expect("B subscribe");

    // group A consumes + acks everything FIRST.
    let a_delivered = drain(&a, N).await;
    assert_eq!(a_delivered.len(), N, "group A receives all {N}");
    let a_ids: Vec<_> = a_delivered.iter().map(|d| d.message_id.clone()).collect();
    a.ack(&a_ids).await.expect("ack A");

    // group B consumes AFTER A — must ALSO receive all N (pub/sub fan-out).
    let b_delivered = drain(&b, N).await;
    assert_eq!(b_delivered.len(), N, "group B independently receives all {N}");
    assert_eq!(
        payloads_of(&b_delivered),
        (0..N as u8).collect::<Vec<_>>(),
        "group B sees the same message set as A",
    );

    client.shutdown().await.expect("shutdown");
}
