//! T2.3 / TD-066 — `GraphOperationsService::flush_wal` persists canonical
//! Checkpoints to the canonical WAL when an appender is injected.
//!
//! Before this slice the checkpoint was constructed but only emitted via
//! `tracing::debug!`. Now, when `with_canonical_wal_appender` injects a
//! real `FramedTableWalAppender`, every `flush_wal(graph_id)` call appends
//! a `CanonicalOperation::Checkpoint(SnapshotManifest)` to the canonical
//! WAL. Recovery (separate follow-up slice) reads the latest checkpoint
//! and scopes engine-WAL replay accordingly — establishing the canonical
//! WAL as the durability authority per ADR-020.
//!
//! These tests prove the emission half end-to-end via the WAL roundtrip
//! pattern Slice 8 established for branch merges. They do NOT cover the
//! recovery half (separate slice).

use anyhow::Result;
use proximadb::graph::service::GraphOperationsService;
use proximadb::services::FramedTableWalAppender;
use proximadb::services::record_store::TableWalAppender;
use proximadb_storage_common::{CanonicalOperation, CanonicalWalEntry};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

const GRAPH_ID: &str = "test-graph";

/// Returns a fresh temp dir + canonical WAL path (mirrors the layout the
/// REST merge endpoint uses: `<data_dir>/pgwire/canonical-records.wal`).
fn fresh_wal_path() -> Result<(TempDir, PathBuf)> {
    let tmp = tempfile::tempdir()?;
    let wal_path = tmp.path().join("pgwire").join("canonical-records.wal");
    Ok((tmp, wal_path))
}

/// Count `CanonicalOperation::Checkpoint` entries in a WAL file.
async fn count_checkpoints(wal_path: &PathBuf) -> Result<Vec<CanonicalWalEntry>> {
    let entries = FramedTableWalAppender::read_entries_from_path(wal_path).await?;
    Ok(entries
        .into_iter()
        .filter(|e| matches!(&e.operation, CanonicalOperation::Checkpoint(_)))
        .collect())
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. flush_wal with injected appender appends a Checkpoint to the WAL.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn flush_wal_persists_canonical_checkpoint_to_disk() -> Result<()> {
    let (_tmp, wal_path) = fresh_wal_path()?;

    let appender: Arc<dyn TableWalAppender> =
        Arc::new(FramedTableWalAppender::open(&wal_path).await?);
    let service = GraphOperationsService::new().with_canonical_wal_appender(appender);

    // No nodes/edges inserted — flush still emits a checkpoint because the
    // method is idempotent and the LSN defaults to 0 when no edge epoch
    // exists for the graph.
    service
        .flush_wal(GRAPH_ID)
        .await
        .map_err(|e| anyhow::anyhow!("flush_wal failed: {:?}", e))?;

    let checkpoints = count_checkpoints(&wal_path).await?;
    assert_eq!(
        checkpoints.len(),
        1,
        "flush_wal should persist exactly one Checkpoint entry"
    );

    match &checkpoints[0].operation {
        CanonicalOperation::Checkpoint(manifest) => {
            assert_eq!(
                manifest.collection_ids,
                vec![GRAPH_ID.to_string()],
                "manifest must reference the flushed graph_id"
            );
            assert!(
                manifest.timestamp_ms > 0,
                "manifest timestamp_ms should be set from SystemTime::now()"
            );
        }
        other => panic!("expected Checkpoint, got {:?}", other),
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Repeated flushes append successive Checkpoints with monotonic WAL seqs.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn flush_wal_emits_one_checkpoint_per_call_with_monotonic_seq() -> Result<()> {
    let (_tmp, wal_path) = fresh_wal_path()?;

    let appender: Arc<dyn TableWalAppender> =
        Arc::new(FramedTableWalAppender::open(&wal_path).await?);
    let service = GraphOperationsService::new().with_canonical_wal_appender(appender);

    // Three successive flushes.
    for _ in 0..3 {
        service
            .flush_wal(GRAPH_ID)
            .await
            .map_err(|e| anyhow::anyhow!("flush_wal failed: {:?}", e))?;
    }

    let checkpoints = count_checkpoints(&wal_path).await?;
    assert_eq!(checkpoints.len(), 3, "three flushes → three checkpoints");

    // WAL-assigned sequence numbers must be strictly monotonic.
    let mut prev = 0u64;
    for entry in &checkpoints {
        assert!(
            entry.sequence_number > prev,
            "WAL sequence must increase per append; prev={}, got={}",
            prev,
            entry.sequence_number
        );
        prev = entry.sequence_number;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. flush_wal without the appender does NOT append (and does NOT error).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn flush_wal_without_appender_is_no_op_for_canonical_wal() -> Result<()> {
    let (_tmp, wal_path) = fresh_wal_path()?;

    // Service constructed WITHOUT the appender — mirrors today's
    // production wiring until the follow-up slice lands.
    let service = GraphOperationsService::new();

    service
        .flush_wal(GRAPH_ID)
        .await
        .map_err(|e| anyhow::anyhow!("flush_wal failed: {:?}", e))?;

    // Canonical WAL file must not exist — nothing was written.
    assert!(
        !wal_path.exists(),
        "no appender → canonical WAL file should not be created; found at {}",
        wal_path.display()
    );

    Ok(())
}
