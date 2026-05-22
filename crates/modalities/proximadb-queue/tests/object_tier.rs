//! Object-tier integration tests — proves sealed segments are
//! asynchronously mirrored to the archive root so the queue survives
//! ECS node-loss / k8s pod eviction onto fresh local disk.

use std::collections::HashMap;
use std::time::Duration;

use proximadb_queue::{Message, QueueClient, QueueConfig, TopicConfig};
use tempfile::TempDir;

fn cfg(
    disk_root: &std::path::Path,
    archive_root: &std::path::Path,
    name: &str,
    rotation_mb: u32,
) -> QueueConfig {
    let mut topics = HashMap::new();
    topics.insert(
        name.to_string(),
        TopicConfig {
            partition_count: 1,
            memory_capacity: 4096,
            // Tiny rotation threshold so a few small sends seal a segment.
            disk_rotation_size_mb: rotation_mb,
            ..Default::default()
        },
    );
    QueueConfig {
        root: format!("file://{}", disk_root.display()),
        object_archive: Some(format!("file://{}", archive_root.display())),
        topics,
        ..QueueConfig::default()
    }
}

/// Send enough to force segment rotation (so segment 0 is sealed),
/// wait briefly, then verify the uploader copied segment 0 to the
/// archive root and dropped an `.uploaded` marker next to the disk copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sealed_segment_uploads_to_object_archive() {
    let disk_tmp = TempDir::new().expect("disk tempdir");
    let archive_tmp = TempDir::new().expect("archive tempdir");
    // disk_rotation_size_mb = 0 forces rotation on every append after
    // the first (active_segment_size > 0 check); the second send
    // creates segment 1, leaving segment 0 sealed.
    let client = QueueClient::open(cfg(disk_tmp.path(), archive_tmp.path(), "embed-ingest", 0))
        .await
        .expect("open");

    let producer = client.producer();
    for i in 0..3u32 {
        producer
            .send(Message::new("embed-ingest", "tenant-a", vec![i as u8]))
            .await
            .expect("send");
    }

    // Wait up to 2s for the uploader to catch up (default poll
    // interval is 500ms). Look for both the archived segment AND its
    // .uploaded marker.
    let archived_seg = archive_tmp
        .path()
        .join("embed-ingest")
        .join("0")
        .join("0000000000.qseg");
    let marker = disk_tmp
        .path()
        .join("embed-ingest")
        .join("0")
        .join("0000000000.qseg.uploaded");

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !archived_seg.exists() || !marker.exists() {
        if std::time::Instant::now() > deadline {
            panic!(
                "uploader didn't archive segment within 3s. archived={} marker={}",
                archived_seg.exists(),
                marker.exists(),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Sanity: archived bytes match disk bytes (frame-for-frame).
    let disk_bytes = std::fs::read(
        disk_tmp
            .path()
            .join("embed-ingest")
            .join("0")
            .join("0000000000.qseg"),
    )
    .expect("read disk");
    let archived_bytes = std::fs::read(&archived_seg).expect("read archived");
    assert_eq!(disk_bytes, archived_bytes, "archived bytes must match disk");

    client.shutdown().await.expect("shutdown");
}

/// Active (still-growing) segment is NOT uploaded. Send one message —
/// segment 0 is the active segment — wait, then assert the archive is
/// empty. Only sealed segments get mirrored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_segment_is_not_uploaded() {
    let disk_tmp = TempDir::new().expect("disk tempdir");
    let archive_tmp = TempDir::new().expect("archive tempdir");
    // disk_rotation_size_mb = 16 (default-ish), and we send one tiny
    // message — segment 0 stays active forever.
    let client = QueueClient::open(cfg(disk_tmp.path(), archive_tmp.path(), "t", 16))
        .await
        .expect("open");

    let producer = client.producer();
    producer
        .send(Message::new("t", "tenant-a", vec![1, 2, 3]))
        .await
        .expect("send");

    // Wait for at least two uploader scan cycles.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let archive_dir = archive_tmp.path().join("t").join("0");
    let entries: Vec<_> = std::fs::read_dir(&archive_dir)
        .map(|rd| rd.collect::<Result<Vec<_>, _>>().unwrap_or_default())
        .unwrap_or_default();
    let qseg_count = entries
        .iter()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("qseg"))
        .count();
    assert_eq!(qseg_count, 0, "active segment must not be uploaded");

    client.shutdown().await.expect("shutdown");
}
