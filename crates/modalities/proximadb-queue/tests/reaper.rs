//! Reaper integration tests — verifies on-disk sealed segments are
//! deleted only after (a) upload to archive (when configured) and
//! (b) every consumer has committed past the segment's last offset.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use proximadb_queue::{Message, MessageId, QueueClient, QueueConfig, TopicConfig};
use tempfile::TempDir;

fn cfg(
    disk_root: &std::path::Path,
    archive_root: Option<&std::path::Path>,
    name: &str,
    rotation_mb: u32,
) -> QueueConfig {
    let mut topics = HashMap::new();
    topics.insert(
        name.to_string(),
        TopicConfig {
            partition_count: 1,
            memory_capacity: 4096,
            disk_rotation_size_mb: rotation_mb,
            ..Default::default()
        },
    );
    QueueConfig {
        root: format!("file://{}", disk_root.display()),
        object_archive: archive_root.map(|p| format!("file://{}", p.display())),
        topics,
        ..QueueConfig::default()
    }
}

fn segment_path(root: &std::path::Path, topic: &str, segment_id: u64) -> std::path::PathBuf {
    root.join(topic)
        .join("0")
        .join(format!("{segment_id:010}.qseg"))
}

/// Happy path: archive uploads sealed segment 0, consumer acks past
/// its last offset, reaper deletes segment 0 from disk. The archived
/// copy stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reaper_deletes_disk_after_upload_and_full_ack() {
    let disk_tmp = TempDir::new().expect("disk");
    let archive_tmp = TempDir::new().expect("archive");
    let client = QueueClient::open(cfg(
        disk_tmp.path(),
        Some(archive_tmp.path()),
        "t",
        0, // rotate aggressively
    ))
    .await
    .expect("open");

    let producer = client.producer();
    // Send 3 messages — rotation_mb=0 forces rotation, so segment 0
    // ends up sealed holding the first message (offset 0).
    for i in 0..3u32 {
        producer
            .send(Message::new("t", "tenant-a", vec![i as u8]))
            .await
            .expect("send");
    }

    let seg0 = segment_path(disk_tmp.path(), "t", 0);
    let arch_seg0 = segment_path(archive_tmp.path(), "t", 0);

    // Wait for the uploader to mirror segment 0.
    let deadline = Instant::now() + Duration::from_secs(3);
    while !arch_seg0.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(arch_seg0.exists(), "uploader should mirror segment 0");

    // Ack offset 0 (the segment's only and last frame). Consumer
    // group "g" matches what reaper queries.
    let consumer = client.consumer("g");
    consumer.subscribe("t", &[0]).await.expect("subscribe");
    // Drain so the in_flight tracker has the messages.
    let mut polled = 0;
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    while polled < 3 && Instant::now() < drain_deadline {
        let batch = consumer
            .poll(8, Duration::from_millis(50))
            .await
            .expect("poll");
        polled += batch.len();
    }
    assert_eq!(polled, 3, "should drain all 3");
    // Ack offsets 0, 1, 2 → committed_offset = 2, well past segment 0's
    // last_offset which is 0.
    let acks: Vec<MessageId> = (0..3u64).map(|o| MessageId::new(0, 0, o)).collect();
    consumer.ack(&acks).await.expect("ack");

    // Wait for reaper to delete segment 0 from disk.
    let reap_deadline = Instant::now() + Duration::from_secs(4);
    while seg0.exists() && Instant::now() < reap_deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(!seg0.exists(), "reaper should have deleted disk segment 0");
    assert!(arch_seg0.exists(), "archive copy must survive the reap");

    client.shutdown().await.expect("shutdown");
}

/// Reaper must NOT delete when no consumer has caught up to the
/// segment's last offset. Send→seal→wait→assert disk segment still
/// present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reaper_keeps_disk_when_consumers_lag() {
    let disk_tmp = TempDir::new().expect("disk");
    let archive_tmp = TempDir::new().expect("archive");
    let client = QueueClient::open(cfg(
        disk_tmp.path(),
        Some(archive_tmp.path()),
        "t",
        0,
    ))
    .await
    .expect("open");

    let producer = client.producer();
    for i in 0..3u32 {
        producer
            .send(Message::new("t", "tenant-a", vec![i as u8]))
            .await
            .expect("send");
    }

    let seg0 = segment_path(disk_tmp.path(), "t", 0);
    let arch_seg0 = segment_path(archive_tmp.path(), "t", 0);

    // Wait for archive upload to complete (otherwise the test's
    // "keeps disk" assertion is meaningless — would have been kept
    // for the upload-pending reason).
    let upload_deadline = Instant::now() + Duration::from_secs(3);
    while !arch_seg0.exists() && Instant::now() < upload_deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(arch_seg0.exists(), "uploader must complete first");

    // Wait two reaper poll cycles WITHOUT acking. Segment must stay.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(
        seg0.exists(),
        "reaper must not delete when consumers haven't acked",
    );

    client.shutdown().await.expect("shutdown");
}

/// Reaper still reclaims disk when NO archive is configured — the
/// upload condition is vacuous ("local-disk-only" durability mode).
/// Consumer commit is the sole reap gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reaper_works_without_archive_configured() {
    let disk_tmp = TempDir::new().expect("disk");
    // No archive — object_archive = None.
    let client = QueueClient::open(cfg(disk_tmp.path(), None, "t", 0))
        .await
        .expect("open");

    let producer = client.producer();
    for i in 0..3u32 {
        producer
            .send(Message::new("t", "tenant-a", vec![i as u8]))
            .await
            .expect("send");
    }

    let seg0 = segment_path(disk_tmp.path(), "t", 0);
    assert!(seg0.exists(), "segment 0 should be sealed on disk");

    // Ack all 3 → committed_offset = 2 ≥ segment 0's last_offset (0).
    let consumer = client.consumer("g");
    consumer.subscribe("t", &[0]).await.expect("subscribe");
    let mut polled = 0;
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    while polled < 3 && Instant::now() < drain_deadline {
        let batch = consumer
            .poll(8, Duration::from_millis(50))
            .await
            .expect("poll");
        polled += batch.len();
    }
    assert_eq!(polled, 3);
    let acks: Vec<MessageId> = (0..3u64).map(|o| MessageId::new(0, 0, o)).collect();
    consumer.ack(&acks).await.expect("ack");

    // Wait for reaper to delete (poll interval = 1s).
    let reap_deadline = Instant::now() + Duration::from_secs(4);
    while seg0.exists() && Instant::now() < reap_deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        !seg0.exists(),
        "reaper must delete sealed segment when no archive + consumer past last_offset",
    );

    client.shutdown().await.expect("shutdown");
}
