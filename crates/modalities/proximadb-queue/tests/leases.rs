//! Cross-process partition lease tests.
//!
//! Each test simulates two consumer replicas by opening two distinct
//! `QueueClient` instances at the same `root` path — their separate
//! `instance_id` values mean the lease.meta CAS is the authority on
//! which one "owns" each partition.

use std::collections::HashMap;
use std::time::Duration;

use proximadb_queue::error::QueueError;
use proximadb_queue::{QueueClient, QueueConfig, TopicConfig};
use tempfile::TempDir;

fn cfg(root: &std::path::Path, lease_duration: Duration) -> QueueConfig {
    let mut topics = HashMap::new();
    topics.insert(
        "t".to_string(),
        TopicConfig {
            partition_count: 2,
            lease_duration,
            ..Default::default()
        },
    );
    QueueConfig {
        root: format!("file://{}", root.display()),
        topics,
        ..QueueConfig::default()
    }
}

/// A fresh subscribe writes the `lease.meta` file and the consumer
/// holds an enforceable lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_subscriber_acquires_lease() {
    let tmp = TempDir::new().expect("tempdir");
    let client = QueueClient::open(cfg(tmp.path(), Duration::from_secs(30)))
        .await
        .expect("open");
    let consumer = client.consumer("g");
    consumer.subscribe("t", &[0]).await.expect("subscribe");

    let lease_path = tmp.path().join("t").join("0").join("lease.meta");
    assert!(lease_path.exists(), "lease.meta should be written on subscribe");

    let body = std::fs::read_to_string(&lease_path).expect("read lease");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("parse lease");
    let holder = parsed["holder_id"].as_str().expect("holder_id");
    assert!(holder.starts_with("inst-"), "holder should be QueueClient instance id; got {holder}");
    assert!(parsed["expires_at_unix_nanos"].as_u64().is_some());

    client.shutdown().await.expect("shutdown");
}

/// Two QueueClients pointed at the same root simulate two replicas.
/// The second to subscribe to a held partition must get
/// `QueueError::LeaseConflict`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_subscriber_gets_lease_conflict() {
    let tmp = TempDir::new().expect("tempdir");
    let client_a = QueueClient::open(cfg(tmp.path(), Duration::from_secs(30)))
        .await
        .expect("open A");
    let consumer_a = client_a.consumer("g");
    consumer_a.subscribe("t", &[0]).await.expect("subscribe A");

    // Different instance_id; tries to subscribe to the same partition.
    let client_b = QueueClient::open(cfg(tmp.path(), Duration::from_secs(30)))
        .await
        .expect("open B");
    let consumer_b = client_b.consumer("g");
    let err = consumer_b
        .subscribe("t", &[0])
        .await
        .expect_err("B must fail");
    match err {
        QueueError::LeaseConflict { topic, partition, holder } => {
            assert_eq!(topic, "t");
            assert_eq!(partition, 0);
            assert!(holder.starts_with("inst-"), "conflict reports A's instance id");
        }
        other => panic!("expected LeaseConflict, got {other:?}"),
    }

    client_a.shutdown().await.expect("shutdown A");
    client_b.shutdown().await.expect("shutdown B");
}

/// A's lease expires; B can take it over via try_acquire. We drop A
/// (renewer task stops), wait past the lease duration, then subscribe
/// from B and verify success + holder switch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_lease_is_reclaimable() {
    let tmp = TempDir::new().expect("tempdir");
    let client_a = QueueClient::open(cfg(tmp.path(), Duration::from_millis(800)))
        .await
        .expect("open A");
    let consumer_a = client_a.consumer("g");
    consumer_a.subscribe("t", &[0]).await.expect("subscribe A");

    // Read A's holder_id from the freshly-written lease.meta. We
    // capture it BEFORE dropping A so we can later assert that B's
    // takeover replaced it with a different id.
    let lease_path = tmp.path().join("t").join("0").join("lease.meta");
    let a_body = std::fs::read_to_string(&lease_path).expect("read A's lease");
    let a_parsed: serde_json::Value = serde_json::from_str(&a_body).expect("parse A");
    let a_instance = a_parsed["holder_id"]
        .as_str()
        .expect("A holder_id")
        .to_string();

    // Drop A — the renewer task aborts. The lease.meta still exists
    // on disk; its expires_at_unix_nanos passes in ~800ms.
    drop(consumer_a);
    drop(client_a);

    // Wait past the lease duration (plus a small safety margin).
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // B now subscribes successfully because A's lease expired.
    let client_b = QueueClient::open(cfg(tmp.path(), Duration::from_millis(800)))
        .await
        .expect("open B");
    let consumer_b = client_b.consumer("g");
    consumer_b.subscribe("t", &[0]).await.expect("B should reclaim");

    // Verify the holder switched.
    let lease_body = std::fs::read_to_string(tmp.path().join("t").join("0").join("lease.meta"))
        .expect("read lease");
    let parsed: serde_json::Value = serde_json::from_str(&lease_body).expect("parse");
    let new_holder = parsed["holder_id"].as_str().expect("holder_id");
    assert_ne!(new_holder, a_instance, "B's instance must differ from A's");

    client_b.shutdown().await.expect("shutdown B");
}

/// A's renewer keeps the lease alive past its initial expiry. B,
/// trying to subscribe after that initial expiry would have lapsed,
/// should still get LeaseConflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lease_renewer_keeps_holder_alive() {
    let tmp = TempDir::new().expect("tempdir");
    // 600ms lease; renewer fires at 300ms.
    let client_a = QueueClient::open(cfg(tmp.path(), Duration::from_millis(600)))
        .await
        .expect("open A");
    let consumer_a = client_a.consumer("g");
    consumer_a.subscribe("t", &[0]).await.expect("subscribe A");

    // Sleep past 600ms initial expiry; renewer must have refreshed.
    tokio::time::sleep(Duration::from_millis(900)).await;

    let client_b = QueueClient::open(cfg(tmp.path(), Duration::from_millis(600)))
        .await
        .expect("open B");
    let consumer_b = client_b.consumer("g");
    let err = consumer_b
        .subscribe("t", &[0])
        .await
        .expect_err("renewer must have kept lease alive");
    assert!(matches!(err, QueueError::LeaseConflict { .. }));

    client_a.shutdown().await.expect("shutdown A");
    client_b.shutdown().await.expect("shutdown B");
}

