//! TD-OBJSTORE-4 S1 gates (ADR-063 D3/D4/D5): WAL discovery is LIST-authority.
//!
//! The batch-3 loss chain included `list_collection_files` probing a directory
//! PREFIX with `exists()` before LISTing — HEAD-on-key is false for prefixes on
//! flat keyspaces, so durable orphan WAL batches were invisible to recovery.
//! These tests pin the corrected semantics on the local backend (the emulator
//! restart test `zero_vector_kv_record_survives_restart` covers the object-store
//! end-to-end path), plus a source-guard for the forbidden pattern itself.

use std::sync::Arc;

use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb::storage::persistence::write_ahead_log::WriteAheadLogDiskManager;

/// Valid `<base62-batch-id>.bcwal` name (same shape as the batch-3 repro object).
const ORPHAN_BATCH_FILE: &str = "8dWbZDDUPo.bcwal";

async fn disk_manager_over(base: &std::path::Path) -> WriteAheadLogDiskManager {
    let factory = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("filesystem factory"),
    );
    WriteAheadLogDiskManager::new(factory, format!("file://{}", base.display()))
}

/// A fresh collection has no WAL directory yet: that is the ONE benign absence —
/// it must read as "nothing to recover", not as an error (and, per ADR-063 D5,
/// without any exists() pre-probe).
#[tokio::test]
async fn missing_wal_dir_lists_empty_not_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dm = disk_manager_over(tmp.path()).await;

    let files = dm
        .list_collection_files("never_written")
        .await
        .expect("missing WAL dir is benign absence, not an error");
    assert!(files.is_empty(), "expected no WAL files, got {files:?}");
}

/// A durable batch object with NO manifest entry and NO directory marker must be
/// discovered by LIST alone — this is the orphan that recovery replays.
#[tokio::test]
async fn orphan_batch_discovered_by_list_without_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wal_dir = tmp.path().join("kv_coll").join("wal");
    std::fs::create_dir_all(&wal_dir).expect("mk wal dir");
    std::fs::write(wal_dir.join(ORPHAN_BATCH_FILE), b"batch-bytes").expect("write orphan");

    let dm = disk_manager_over(tmp.path()).await;
    let files = dm
        .list_collection_files("kv_coll")
        .await
        .expect("list orphan batch");
    assert_eq!(
        files.len(),
        1,
        "orphan .bcwal must be discovered: {files:?}"
    );
    assert!(
        files[0].file_url.ends_with(ORPHAN_BATCH_FILE),
        "unexpected file url: {}",
        files[0].file_url
    );
}

/// The legacy `{collection}/write_buffer/` fallback must also discover by LIST.
#[tokio::test]
async fn legacy_write_buffer_fallback_discovers_by_list() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let legacy_dir = tmp.path().join("old_coll").join("write_buffer");
    std::fs::create_dir_all(&legacy_dir).expect("mk legacy dir");
    std::fs::write(legacy_dir.join(ORPHAN_BATCH_FILE), b"batch-bytes").expect("write legacy");

    let dm = disk_manager_over(tmp.path()).await;
    let files = dm
        .list_collection_files("old_coll")
        .await
        .expect("list legacy batch");
    assert_eq!(files.len(), 1, "legacy batch must be discovered: {files:?}");
}

/// ADR-063 D3: a LIST failure that is NOT simple absence (here: the WAL path is a
/// FILE, so read_dir fails with a non-NotFound error) must fail recovery closed —
/// never be swallowed into "empty collection".
#[tokio::test]
async fn list_error_fails_closed_not_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let coll_dir = tmp.path().join("broken_coll");
    std::fs::create_dir_all(&coll_dir).expect("mk coll dir");
    // `{coll}/wal` is a FILE: listing it errors with NotADirectory-class IO error.
    std::fs::write(coll_dir.join("wal"), b"not a directory").expect("write blocker file");

    let dm = disk_manager_over(tmp.path()).await;
    let result = dm.list_collection_files("broken_coll").await;
    assert!(
        result.is_err(),
        "non-absence LIST failure must fail closed, got {result:?}"
    );
}

/// Source-guard (ADR-063 D5): the WAL disk manager must not reintroduce ANY
/// `exists()` probe — prefix-exists-before-LIST is the exact gate that made
/// durable batches invisible on flat keyspaces (TD-OBJSTORE-1 batch 3). Exact-key
/// exists() probes belong elsewhere; discovery in this file is LIST-only.
#[test]
fn disk_manager_has_no_exists_probe() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/storage/persistence/write_ahead_log/disk_manager.rs"
    ))
    .expect("read disk_manager.rs");
    assert!(
        !src.contains(".exists("),
        "disk_manager.rs must not probe existence — LIST is the discovery \
         authority (ADR-063 D5); found a `.exists(` call"
    );
}
