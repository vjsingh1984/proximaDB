//! TD-066 (c) Part 1 — read-side observability of the canonical WAL
//! checkpoint on ORION recovery.
//!
//! Today's emission half (commit `b36a24b17`) makes
//! `GraphOperationsService::flush_wal` persist
//! `CanonicalOperation::Checkpoint(SnapshotManifest)` to the shared
//! canonical WAL. The production wiring (commit `4ece74250`) ensures
//! both graph checkpoint emission and pgwire writes share a single
//! `FramedTableWalAppender` to avoid sequence-number collisions.
//!
//! This slice adds the read-side: `OrionPersistence::canonical_checkpoint_lsn`
//! scans the canonical WAL and returns the latest checkpoint
//! `SnapshotManifest.sequence_number` whose `collection_ids` contains
//! the graph being recovered. `OrionGraphEngine::recover` logs the
//! result via `tracing::info!` for operator visibility.
//!
//! **Important**: this is observability only. Recovery behavior is
//! unchanged in Part 1 — Part 2 will use the LSN to scope engine WAL
//! replay once the LSN-correlation semantics are designed.

use anyhow::Result;
use proximadb::graph::engines::orion::OrionGraphEngine;
use proximadb::services::FramedTableWalAppender;
use proximadb::services::record_store::TableWalAppender;
use proximadb_storage_common::{CanonicalOperation, SnapshotManifest};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

/// Returns (TempDir guard, base_path, canonical_wal_path). The temp dir
/// must outlive the test for cleanup; `base_path` is the engine's
/// base storage URL, `canonical_wal_path` is the shared canonical WAL
/// file location.
fn fresh_layout() -> Result<(TempDir, PathBuf, PathBuf)> {
    let tmp = tempfile::tempdir()?;
    let base = tmp.path().join("orion-data");
    std::fs::create_dir_all(&base)?;
    let wal = tmp.path().join("pgwire").join("canonical-records.wal");
    Ok((tmp, base, wal))
}

/// Seed the canonical WAL with `entries`, then drop the appender so the
/// file is flushed before the next reader sees it.
async fn seed_canonical_wal(
    wal_path: &PathBuf,
    operations: Vec<CanonicalOperation>,
) -> Result<()> {
    let appender = FramedTableWalAppender::open(wal_path).await?;
    appender.append_operations(operations, None).await?;
    drop(appender);
    Ok(())
}

fn checkpoint_op(sequence_number: u64, collection_ids: Vec<&str>) -> CanonicalOperation {
    CanonicalOperation::Checkpoint(SnapshotManifest {
        sequence_number,
        timestamp_ms: 0,
        collection_ids: collection_ids.into_iter().map(String::from).collect(),
        projection_freshness: vec![],
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. No canonical WAL path → None (today's pre-TD-066 behavior).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn canonical_checkpoint_lsn_returns_none_when_no_wal_path() -> Result<()> {
    let (_tmp, base, _wal) = fresh_layout()?;
    let base_url = format!("file://{}", base.display());
    let engine = OrionGraphEngine::with_persistence_for_graph(
        "graph-a".to_string(),
        base_url,
        true,
    )
    .await?;

    let lsn = engine
        .persistence()
        .expect("engine should have persistence")
        .canonical_checkpoint_lsn()
        .await;

    assert!(
        lsn.is_none(),
        "no canonical WAL path → canonical_checkpoint_lsn must return None; got {:?}",
        lsn
    );

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Returns the MAX checkpoint LSN for this graph (not just the last appended).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn canonical_checkpoint_lsn_returns_max_for_graph() -> Result<()> {
    let (_tmp, base, wal_path) = fresh_layout()?;
    let base_url = format!("file://{}", base.display());

    // Three checkpoints for the same graph — the middle one (200) is the
    // largest LSN, but it's NOT the last-appended. canonical_checkpoint_lsn
    // must return 200 (max), not 50 (last).
    seed_canonical_wal(
        &wal_path,
        vec![
            checkpoint_op(100, vec!["graph-a"]),
            checkpoint_op(200, vec!["graph-a"]),
            checkpoint_op(50, vec!["graph-a"]),
        ],
    )
    .await?;

    let engine = OrionGraphEngine::with_persistence_for_graph_and_canonical_wal(
        "graph-a".to_string(),
        base_url,
        true,
        Some(wal_path.clone()),
    )
    .await?;

    let lsn = engine
        .persistence()
        .expect("engine should have persistence")
        .canonical_checkpoint_lsn()
        .await;

    assert_eq!(
        lsn,
        Some(200),
        "must return the MAX checkpoint LSN for the graph; got {:?}",
        lsn
    );

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Filters by graph_id — checkpoints for OTHER graphs are ignored.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn canonical_checkpoint_lsn_filters_by_graph_id() -> Result<()> {
    let (_tmp, base, wal_path) = fresh_layout()?;
    let base_url = format!("file://{}", base.display());

    // Mix of graph-a and graph-b checkpoints, plus a multi-graph
    // checkpoint that should match both.
    seed_canonical_wal(
        &wal_path,
        vec![
            checkpoint_op(100, vec!["graph-a"]),
            checkpoint_op(500, vec!["graph-b"]),
            checkpoint_op(300, vec!["graph-a", "graph-c"]),
            checkpoint_op(50, vec!["graph-b"]),
        ],
    )
    .await?;

    let engine_a = OrionGraphEngine::with_persistence_for_graph_and_canonical_wal(
        "graph-a".to_string(),
        base_url.clone(),
        true,
        Some(wal_path.clone()),
    )
    .await?;
    let lsn_a = engine_a
        .persistence()
        .unwrap()
        .canonical_checkpoint_lsn()
        .await;
    assert_eq!(
        lsn_a,
        Some(300),
        "graph-a should see {{100, 300}} → max = 300; got {:?}",
        lsn_a
    );

    let engine_b = OrionGraphEngine::with_persistence_for_graph_and_canonical_wal(
        "graph-b".to_string(),
        base_url.clone(),
        true,
        Some(wal_path.clone()),
    )
    .await?;
    let lsn_b = engine_b
        .persistence()
        .unwrap()
        .canonical_checkpoint_lsn()
        .await;
    assert_eq!(
        lsn_b,
        Some(500),
        "graph-b should see {{500, 50}} → max = 500; got {:?}",
        lsn_b
    );

    let engine_c = OrionGraphEngine::with_persistence_for_graph_and_canonical_wal(
        "graph-c".to_string(),
        base_url,
        true,
        Some(wal_path.clone()),
    )
    .await?;
    let lsn_c = engine_c
        .persistence()
        .unwrap()
        .canonical_checkpoint_lsn()
        .await;
    assert_eq!(
        lsn_c,
        Some(300),
        "graph-c should see {{300 (multi-graph)}} → 300; got {:?}",
        lsn_c
    );

    Ok(())
}

// Drop an unused import warning the compiler would emit otherwise.
#[allow(dead_code)]
fn _force_link() {
    let _: Option<Arc<dyn TableWalAppender>> = None;
}
