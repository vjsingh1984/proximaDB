mod fake_fs;

use std::path::PathBuf;
use std::time::Duration;

use fake_fs::{FakeFs, FakeFsConfig};
use proximadb_queue::disk_tier::PartitionDiskWriter;
use proximadb_queue::fs::QueueFs;
use proximadb_queue::{Message, TopicConfig};

fn cfg(max_batch: usize) -> TopicConfig {
    TopicConfig {
        group_commit_max_wait: Duration::from_millis(25),
        group_commit_max_batch: max_batch,
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_writes_framed_message_and_waits_for_fsync() {
    let fs = FakeFs::new();
    let writer = PartitionDiskWriter::open(
        "embed-ingest".to_string(),
        0,
        PathBuf::from("/queue"),
        fs.clone(),
        cfg(1),
    )
    .await
    .expect("open writer");

    let msg = Message::new("embed-ingest", "tenant-a", vec![1, 2, 3]);
    let out = writer.append(&msg).await.expect("append");
    writer
        .wait_for_fsync(out.segment_path.clone())
        .await
        .expect("fsync");

    assert_eq!(fs.fsync_calls(), 1);
    // Frame format: [4 BE len][8 BE offset][len bytes bincode payload]
    let bytes = fs.read(&out.segment_path).await.expect("read segment");
    assert!(bytes.len() > 12);
    let payload_len = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
    assert_eq!(
        payload_len,
        bytes.len() - 12,
        "frame body should be bytes after 12-byte header"
    );
    let frame_offset = u64::from_be_bytes(bytes[4..12].try_into().unwrap());
    assert_eq!(
        frame_offset, out.offset,
        "frame offset must match AppendOutcome"
    );
    assert_eq!(frame_offset, 0, "first message gets offset 0");
    let decoded: Message = bincode::deserialize(&bytes[12..]).expect("decode message");
    assert_eq!(decoded.topic, "embed-ingest");
    assert_eq!(decoded.tenant_id, "tenant-a");
    assert_eq!(decoded.payload, vec![1, 2, 3]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn group_commit_coalesces_waiters_for_same_segment() {
    let fs = FakeFs::new();
    let writer = PartitionDiskWriter::open(
        "events".to_string(),
        1,
        PathBuf::from("/queue"),
        fs.clone(),
        cfg(2),
    )
    .await
    .expect("open writer");

    let path = writer
        .append(&Message::new("events", "tenant-a", vec![1]))
        .await
        .expect("append")
        .segment_path;

    let w1 = writer.clone();
    let p1 = path.clone();
    let h1 = tokio::spawn(async move { w1.wait_for_fsync(p1).await });
    let w2 = writer.clone();
    let h2 = tokio::spawn(async move { w2.wait_for_fsync(path).await });

    h1.await.expect("join waiter 1").expect("waiter 1");
    h2.await.expect("join waiter 2").expect("waiter 2");
    assert_eq!(fs.fsync_calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writer_rotates_when_active_segment_exceeds_threshold() {
    let fs = FakeFs::new();
    let mut topic_cfg = cfg(1);
    topic_cfg.disk_rotation_size_mb = 0;
    let writer = PartitionDiskWriter::open(
        "rotate".to_string(),
        2,
        PathBuf::from("/queue"),
        fs,
        topic_cfg,
    )
    .await
    .expect("open writer");

    writer
        .append(&Message::new("rotate", "tenant-a", vec![1]))
        .await
        .expect("append first");
    writer
        .append(&Message::new("rotate", "tenant-a", vec![2]))
        .await
        .expect("append second");

    let mut segments = writer.segments().await.expect("segments");
    segments.sort_by_key(|s| s.segment_id);
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].segment_id, 0);
    assert_eq!(segments[0].topic, "rotate");
    assert_eq!(segments[0].partition, 2);
    assert_eq!(segments[1].segment_id, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fsync_failure_is_returned_to_waiter() {
    let fs = FakeFs::with_config(FakeFsConfig {
        fsync_failure_rate: 1.0,
        ..Default::default()
    });
    let writer =
        PartitionDiskWriter::open("fail".to_string(), 0, PathBuf::from("/queue"), fs, cfg(1))
            .await
            .expect("open writer");

    let path = writer
        .append(&Message::new("fail", "tenant-a", vec![1]))
        .await
        .expect("append")
        .segment_path;
    let err = writer
        .wait_for_fsync(path)
        .await
        .expect_err("fsync failure must surface");
    assert!(err.to_string().contains("fake fsync failure"));
}
