//! TD-WLP-2 (ADR-061 D5) integration gate: per-collection compaction override.
//!
//! Compaction arming must resolve per-collection through the tag cascade while
//! the global env (`PROXIMADB_L0_COMPACTION_ENABLED`) stays the master
//! kill-switch:
//!
//! * an `AppendBulk` collection with an explicit `compaction:on` opt-in arms
//!   compaction (at its per-collection `l0_threshold:N`) while the global gate
//!   is OFF;
//! * an untagged collection under the same global-off never arms (today's
//!   default, byte-for-byte);
//! * an explicitly falsy global env hard-disables even opted-in collections.
//!
//! Env-sensitive tests rely on nextest's process-per-test isolation.

use anyhow::Result;
use proximadb::storage::engines::sst::SstEngine;
use proximadb::storage::traits::{FlushParameters, FlushResult, UnifiedStorageEngine};
use proximadb_proto::v1::{Collection, CollectionConfig, StorageAssignment};
use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaRecord};

const GLOBAL_GATE_ENV: &str = "PROXIMADB_L0_COMPACTION_ENABLED";

fn record(id: &str) -> ProximaRecord {
    ProximaRecord {
        oid: id.to_string(),
        created_at_ns: 12_345_000_000,
        updated_at_ns: 12_345_000_000,
        record_version: 1,
        embeddings: vec![EmbeddingCell {
            model_id: "test".to_string(),
            modality: "dense_vector".to_string(),
            dim: 4,
            values: EmbeddingValues::Fp32(vec![1.0, 0.0, 0.0, 0.0]),
            ..Default::default()
        }],
        ..ProximaRecord::default()
    }
}

fn collection(id: &str, base_location: &str, tags: &[&str]) -> Collection {
    Collection {
        id: id.to_string(),
        config: Some(CollectionConfig {
            name: id.to_string(),
            dimension: 4,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            base_location: base_location.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn flush_once(engine: &SstEngine, coll: &Collection, ids: &[&str]) -> Result<FlushResult> {
    let params = FlushParameters {
        collection_id: Some(coll.id.clone()),
        vector_records: ids.iter().map(|id| record(id)).collect(),
        force: true,
        synchronous: true,
        collection_config: Some(coll.clone()),
        ..Default::default()
    };
    engine.do_flush(&params).await
}

/// TD-WLP-2 TDD gate: with the global env unset, a collection tagged
/// `workload_profile:append` + `compaction:on` reaching L0 >= its
/// per-collection threshold fires compaction; an untagged collection under the
/// same global-off does NOT — even past the legacy threshold of 5.
#[tokio::test]
async fn test_append_collection_arms_compaction_while_global_off() -> Result<()> {
    unsafe {
        std::env::remove_var(GLOBAL_GATE_ENV);
        std::env::remove_var("PROXIMADB_STORAGE_PROFILE");
    }
    let engine = SstEngine::new().await?;

    // Opted-in AppendBulk collection: arms once L0 reaches its own threshold.
    let opted_dir = tempfile::tempdir()?;
    let opted = collection(
        "wlp2_append_opted",
        opted_dir.path().to_str().expect("utf8 tempdir"),
        &["workload_profile:append", "compaction:on", "l0_threshold:2"],
    );
    let first = flush_once(&engine, &opted, &["a0", "a1"]).await?;
    assert!(first.success, "first flush must succeed");
    assert!(
        !first.compaction_triggered,
        "1 L0 segment < l0_threshold:2 — compaction must not fire yet"
    );
    let second = flush_once(&engine, &opted, &["a2", "a3"]).await?;
    assert!(second.success, "second flush must succeed");
    assert!(
        second.compaction_triggered,
        "opted-in collection at L0 >= per-collection threshold must arm \
         compaction while the global gate is OFF (TD-WLP-2)"
    );

    // Untagged collection: the global gate governs and it is OFF — never arms,
    // even once past the legacy L0_COMPACTION_THRESHOLD of 5.
    let untagged_dir = tempfile::tempdir()?;
    let untagged = collection(
        "wlp2_untagged",
        untagged_dir.path().to_str().expect("utf8 tempdir"),
        &[],
    );
    for i in 0..6 {
        let ids = [format!("u{i}")];
        let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let result = flush_once(&engine, &untagged, &id_refs).await?;
        assert!(result.success, "untagged flush {i} must succeed");
        assert!(
            !result.compaction_triggered,
            "untagged collection must keep today's default-OFF behaviour \
             (flush {i})"
        );
    }
    Ok(())
}

/// The global env explicitly falsy is the master kill-switch: a per-collection
/// `compaction:on` opt-in cannot arm compaction under it (ADR-061 D5).
#[tokio::test]
async fn test_global_hard_disable_wins_over_collection_opt_in() -> Result<()> {
    unsafe {
        std::env::set_var(GLOBAL_GATE_ENV, "0");
    }
    let engine = SstEngine::new().await?;
    let dir = tempfile::tempdir()?;
    let opted = collection(
        "wlp2_hard_disable",
        dir.path().to_str().expect("utf8 tempdir"),
        &["workload_profile:append", "compaction:on", "l0_threshold:1"],
    );
    for i in 0..2 {
        let ids = [format!("h{i}")];
        let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let result = flush_once(&engine, &opted, &id_refs).await?;
        assert!(result.success, "flush {i} must succeed");
        assert!(
            !result.compaction_triggered,
            "global hard-disable must win over the per-collection opt-in \
             (flush {i})"
        );
    }
    unsafe {
        std::env::remove_var(GLOBAL_GATE_ENV);
    }
    Ok(())
}
