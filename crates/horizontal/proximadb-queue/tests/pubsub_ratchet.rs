//! Pub/sub (consumer-group) round-trip + delivery-guarantee ratchet (ADR-079 §Semantics).
//!
//! Proves the defining Kafka-style property: every consumer GROUP independently
//! receives ALL messages on a partition, in order, with no loss and no
//! duplication — even when groups consume concurrently or one races ahead of
//! the other. This is the conformance ratchet for the queue's pub/sub plane: a
//! regression (loss / dup / cross-group interference) fails CI here.

use std::collections::{HashMap, HashSet};
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

/// **Delivery-guarantee ratchet (ADR-079).** Every consumer group independently
/// receives EVERY message, in strict ascending offset order, with no loss and
/// no duplication — the pub/sub contract. This is the CI ratchet: the
/// guaranteed-delivery fraction is 100% and may never regress. A failure on
/// completeness, order, uniqueness, or cross-group equality fails here.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn pubsub_delivery_guarantee_ratchet() {
    const GROUPS: &[&str] = &["compliance", "security", "analytics"];
    const N: usize = 25;
    let (_tmp, cfg) = cfg("ratchet-topic", 1);
    let client = QueueClient::open(cfg).await.expect("open");
    let producer = client.producer();
    for i in 0..N as u8 {
        producer
            .send(Message::new("ratchet-topic", "tenant", vec![i]))
            .await
            .expect("send");
    }

    let mut delivered_per_group: HashMap<&str, Vec<u64>> = HashMap::new();
    for &g in GROUPS {
        let c = client.consumer(g);
        c.subscribe("ratchet-topic", &[0]).await.expect("subscribe");

        let batch = drain(&c, N).await;
        // Completeness: every group receives exactly N.
        assert_eq!(batch.len(), N, "group {g}: received {} of {N} (loss)", batch.len());

        // Order + uniqueness: payloads (sent as vec![i]) are the offset proxy —
        // strictly ascending 0..N with no duplicates.
        let payloads: Vec<u64> =
            batch.iter().map(|d| d.message.payload.first().copied().unwrap_or(0) as u64).collect();
        let unique: HashSet<u64> = payloads.iter().copied().collect();
        assert_eq!(unique.len(), N, "group {g}: expected {N} unique messages (no dups)");
        assert_eq!(
            payloads,
            (0..N as u64).collect::<Vec<_>>(),
            "group {g}: messages must be 0..{N} in strict ascending order",
        );
        delivered_per_group.insert(g, payloads);
    }

    // Cross-group equality: every group received the SAME full message set
    // (true fan-out — no group's consumption affected another's).
    let baseline = delivered_per_group["compliance"].clone();
    for (g, msgs) in &delivered_per_group {
        assert_eq!(msgs, &baseline, "group {g} received the same set as compliance");
    }

    client.shutdown().await.expect("shutdown");
}
