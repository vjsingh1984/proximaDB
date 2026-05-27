//! T3.1 Slice 8 — Durable branch-merge integration test (canonical-WAL roundtrip).
//!
//! Verifies the full ADR-012 branch-merge lifecycle end-to-end through the
//! canonical WAL: seed divergent branch mutations, run `merge_branches` +
//! `write_back_merge`, then re-read the WAL to confirm the merge entries are
//! durably visible with the correct outcome and `origin = "branch_merge:<a>:<b>"`.
//!
//! Slice 7 (commit `0c87e4640`) landed the durable write-back primitive; this
//! suite proves that subsequent canonical-WAL readers observe merged state.
//!
//! REST-router-level integration (axum `oneshot`) is deferred — needs an
//! `AppState::for_test` constructor first.

use anyhow::Result;
use proximadb::graph::merge::{merge_branches, write_back_merge};
use proximadb::services::FramedTableWalAppender;
use proximadb::services::record_store::TableWalAppender;
use proximadb_records::{LabelSet, ProximaRecord};
use proximadb_storage_common::{CanonicalOperation, CanonicalWalEntry};
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::time::{sleep, Duration};

const COLLECTION: &str = "col";
const BRANCH_A: &str = "a";
const BRANCH_B: &str = "b";
const EXPECTED_ORIGIN: &str = "branch_merge:a:b";

/// Per-test fixture. Returns the temp dir (keeps it alive) and the canonical
/// WAL path nested under `pgwire/canonical-records.wal` so the layout mirrors
/// the REST handler's `graph_branch_merge_wal_path`.
fn fresh_wal() -> Result<(TempDir, PathBuf)> {
    let tmp = tempfile::tempdir()?;
    let wal_path = tmp.path().join("pgwire").join("canonical-records.wal");
    Ok((tmp, wal_path))
}

fn upsert_op(oid: &str, branch: Option<&str>) -> CanonicalOperation {
    let mut record = ProximaRecord::default();
    record.oid = oid.to_string();
    record.branch_id = branch.map(String::from);
    CanonicalOperation::RecordUpsert {
        collection_id: COLLECTION.to_string(),
        record: Box::new(record),
        projections: vec![],
    }
}

fn upsert_with_record(record: ProximaRecord) -> CanonicalOperation {
    CanonicalOperation::RecordUpsert {
        collection_id: COLLECTION.to_string(),
        record: Box::new(record),
        projections: vec![],
    }
}

fn branch_record(oid: &str, branch: Option<&str>) -> ProximaRecord {
    let mut record = ProximaRecord::default();
    record.oid = oid.to_string();
    record.branch_id = branch.map(String::from);
    record
}

fn tombstone_record(oid: &str, branch: Option<&str>) -> ProximaRecord {
    let mut record = branch_record(oid, branch);
    record.valid_to_ns = Some(0);
    record.embeddings = Vec::new();
    record.origin = Some("delete".to_string());
    record
}

/// Append `ops` to the WAL and drop the appender so the next reader sees a
/// fully flushed file.
async fn append_and_drop(
    wal_path: &PathBuf,
    ops: Vec<CanonicalOperation>,
) -> Result<Vec<CanonicalWalEntry>> {
    let appender = FramedTableWalAppender::open(wal_path).await?;
    let entries = appender.append_operations(ops, None).await?;
    drop(appender);
    Ok(entries)
}

/// Run the full lifecycle: read WAL → merge → durable write-back → re-read.
async fn merge_and_read_back(
    wal_path: &PathBuf,
) -> Result<(
    Vec<CanonicalWalEntry>,
    Vec<CanonicalWalEntry>,
    usize, // count of newly written merge entries
)> {
    let pre = FramedTableWalAppender::read_entries_from_path(wal_path).await?;
    let report = merge_branches(&pre, BRANCH_A, BRANCH_B)
        .expect("seed produced mergeable branches");
    let result = write_back_merge(
        &pre,
        &report,
        wal_path,
        COLLECTION,
        BRANCH_A,
        BRANCH_B,
        None,
    )
    .await?
    .expect("write_back_merge should return Some when seed has mutations");
    let post = FramedTableWalAppender::read_entries_from_path(wal_path).await?;
    Ok((pre, post, result.written_entries.len()))
}

/// Assert that every freshly appended entry's operation matches the expected
/// branch-merge shape.
fn assert_merge_entries(
    post: &[CanonicalWalEntry],
    pre_len: usize,
    written: usize,
    mut check: impl FnMut(&CanonicalOperation),
) {
    assert_eq!(
        post.len(),
        pre_len + written,
        "post-merge WAL should contain seed + write-back entries"
    );
    for entry in &post[pre_len..] {
        check(&entry.operation);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. LWW conflict — later append wins.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_branch_merge_cycle_lww_picks_later_branch_via_wal_roundtrip() -> Result<()> {
    let (_tmp, wal_path) = fresh_wal()?;

    // Base record on no branch.
    append_and_drop(&wal_path, vec![upsert_op("shared", None)]).await?;
    sleep(Duration::from_millis(8)).await;

    // Branch a — earlier append, earlier timestamp_ms.
    let mut left = branch_record("shared", Some(BRANCH_A));
    left.labels = LabelSet::from(vec!["from-left".to_string()]);
    append_and_drop(&wal_path, vec![upsert_with_record(left)]).await?;
    sleep(Duration::from_millis(8)).await;

    // Branch b — later append, later timestamp_ms. LWW selects this side.
    let mut right = branch_record("shared", Some(BRANCH_B));
    right.labels = LabelSet::from(vec!["from-right".to_string()]);
    append_and_drop(&wal_path, vec![upsert_with_record(right)]).await?;

    let (pre, post, written) = merge_and_read_back(&wal_path).await?;
    assert_eq!(written, 1, "single shared OID should produce one merge entry");

    assert_merge_entries(&post, pre.len(), written, |op| match op {
        CanonicalOperation::RecordUpsert { record, .. } => {
            assert_eq!(record.oid, "shared");
            assert_eq!(record.origin.as_deref(), Some(EXPECTED_ORIGIN));
            let labels: Vec<&str> = record.labels.iter().map(|s| s.as_str()).collect();
            assert!(
                labels.contains(&"from-right"),
                "LWW should keep the later (right-branch) record; got labels {:?}",
                labels
            );
        }
        other => panic!("expected RecordUpsert, got {:?}", other),
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Delete-wins — tombstone propagates as a RecordDelete merge entry.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_branch_merge_cycle_tombstone_for_both_deleted_via_wal_roundtrip() -> Result<()> {
    let (_tmp, wal_path) = fresh_wal()?;

    // Base + left tombstone + right live record on the same OID.
    append_and_drop(&wal_path, vec![upsert_op("shared", None)]).await?;
    sleep(Duration::from_millis(8)).await;
    append_and_drop(
        &wal_path,
        vec![upsert_with_record(tombstone_record("shared", Some(BRANCH_A)))],
    )
    .await?;
    sleep(Duration::from_millis(8)).await;
    append_and_drop(
        &wal_path,
        vec![upsert_with_record(branch_record("shared", Some(BRANCH_B)))],
    )
    .await?;

    let (pre, post, written) = merge_and_read_back(&wal_path).await?;
    assert_eq!(written, 1, "tombstone resolution should produce one entry");

    assert_merge_entries(&post, pre.len(), written, |op| match op {
        CanonicalOperation::RecordDelete { collection_id, oid, .. } => {
            assert_eq!(collection_id, COLLECTION);
            assert_eq!(oid, "shared");
        }
        other => panic!("expected RecordDelete tombstone, got {:?}", other),
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Add-wins / UnionLabels — both branches contribute distinct labels.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_branch_merge_cycle_unions_labels_via_wal_roundtrip() -> Result<()> {
    let (_tmp, wal_path) = fresh_wal()?;

    let mut base = branch_record("shared", None);
    base.labels = LabelSet::from(vec!["base".to_string()]);
    let mut left = branch_record("shared", Some(BRANCH_A));
    left.labels = LabelSet::from(vec!["base".to_string(), "left".to_string()]);
    let mut right = branch_record("shared", Some(BRANCH_B));
    right.labels = LabelSet::from(vec!["base".to_string(), "right".to_string()]);

    append_and_drop(&wal_path, vec![upsert_with_record(base)]).await?;
    sleep(Duration::from_millis(8)).await;
    append_and_drop(&wal_path, vec![upsert_with_record(left)]).await?;
    sleep(Duration::from_millis(8)).await;
    append_and_drop(&wal_path, vec![upsert_with_record(right)]).await?;

    let (pre, post, written) = merge_and_read_back(&wal_path).await?;
    assert_eq!(written, 1, "label union should produce one merge entry");

    assert_merge_entries(&post, pre.len(), written, |op| match op {
        CanonicalOperation::RecordUpsert { record, .. } => {
            assert_eq!(record.oid, "shared");
            assert_eq!(record.origin.as_deref(), Some(EXPECTED_ORIGIN));
            let labels: Vec<&str> = record.labels.iter().map(|s| s.as_str()).collect();
            assert_eq!(labels.len(), 3, "union should be {{base,left,right}}; got {:?}", labels);
            assert!(labels.contains(&"base"));
            assert!(labels.contains(&"left"));
            assert!(labels.contains(&"right"));
        }
        other => panic!("expected RecordUpsert, got {:?}", other),
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Unilateral mutations — disjoint OIDs on each branch propagate unchanged.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_branch_merge_cycle_preserves_unilateral_mutations_via_wal_roundtrip() -> Result<()> {
    let (_tmp, wal_path) = fresh_wal()?;

    append_and_drop(&wal_path, vec![upsert_op("left-only", Some(BRANCH_A))]).await?;
    sleep(Duration::from_millis(8)).await;
    append_and_drop(&wal_path, vec![upsert_op("right-only", Some(BRANCH_B))]).await?;

    let (pre, post, written) = merge_and_read_back(&wal_path).await?;
    assert_eq!(written, 2, "two disjoint OIDs should both propagate");

    let mut oids_seen = Vec::new();
    assert_merge_entries(&post, pre.len(), written, |op| match op {
        CanonicalOperation::RecordUpsert { record, .. } => {
            assert_eq!(
                record.origin.as_deref(),
                Some(EXPECTED_ORIGIN),
                "every merge entry must carry the branch-merge origin"
            );
            oids_seen.push(record.oid.clone());
        }
        other => panic!("expected RecordUpsert, got {:?}", other),
    });

    oids_seen.sort();
    assert_eq!(oids_seen, vec!["left-only".to_string(), "right-only".to_string()]);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Sequence-number monotonicity after durable write-back.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_branch_merge_cycle_sequences_monotonic_after_write_back() -> Result<()> {
    let (_tmp, wal_path) = fresh_wal()?;

    append_and_drop(&wal_path, vec![upsert_op("shared", None)]).await?;
    sleep(Duration::from_millis(8)).await;
    append_and_drop(&wal_path, vec![upsert_op("shared", Some(BRANCH_A))]).await?;
    sleep(Duration::from_millis(8)).await;
    append_and_drop(&wal_path, vec![upsert_op("shared", Some(BRANCH_B))]).await?;

    let (pre, post, written) = merge_and_read_back(&wal_path).await?;
    assert!(written >= 1);
    assert_eq!(post.len(), pre.len() + written);

    let mut prev = 0u64;
    for entry in &post {
        assert!(
            entry.sequence_number > prev,
            "sequence numbers must be strictly increasing; prev={}, got={}",
            prev,
            entry.sequence_number
        );
        prev = entry.sequence_number;
    }

    // The merge entries must come after every seed entry.
    let max_pre_seq = pre.iter().map(|e| e.sequence_number).max().unwrap_or(0);
    for entry in &post[pre.len()..] {
        assert!(
            entry.sequence_number > max_pre_seq,
            "merge entry seq={} must exceed max seed seq={}",
            entry.sequence_number,
            max_pre_seq
        );
    }

    Ok(())
}
