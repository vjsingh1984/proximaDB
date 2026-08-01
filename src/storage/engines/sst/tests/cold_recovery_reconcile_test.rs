//! TD-DELVEC-1 WI-5 P1 integration test: the post-recovery DV-bit reconciliation
//! (`UnifiedStorageFormat::reconcile_deletion_vectors`) re-marks a deletion-vector
//! bit for a tombstone — closing the crash-strand resurface window.
//!
//! A crash between the WAL append (commit) and `mark_deleted` strands the DV bit.
//! Recovery calls `reconcile_deletion_vectors` after the materialization flush with
//! the replayed tombstones; the `SstEngine` impl resolves each oid → (segment,
//! position) and `mark_deleted`s at the tombstone's `manifest_lsn`. This test
//! drives that method directly: flush rows to a cold `.pax` → confirm no DV bit →
//! reconcile one oid → that row's position is deleted, the others are not. In-crate
//! so it can read the `pub(crate)` DV store + discovery, and call the feature-gated
//! trait override.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use anyhow::Result;
    use tempfile::TempDir;

    use crate::proto::proximadb_v1::{
        Collection, CollectionConfig, StorageAssignment, StorageEngine, VectorRecord,
    };
    use crate::storage::engines::sst::{SstConfig, core::SstEngine};
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use crate::storage::traits::{FlushParameters, UnifiedStorageFormat};
    use proximadb_distance_kernel::engine::UnifiedDistanceCompute;

    fn collection(id: &str, temp_dir: &TempDir) -> Collection {
        Collection {
            id: id.to_string(),
            config: Some(CollectionConfig {
                name: id.to_string(),
                dimension: 4,
                storage_engine: Some(StorageEngine::Sst as i32),
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                base_location: temp_dir.path().to_str().unwrap().to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn record(oid: &str, i: usize) -> VectorRecord {
        VectorRecord {
            id: oid.to_string(),
            vector: vec![i as f32, 0.0, 0.0, 0.0],
            metadata: HashMap::new(),
            version: Some(1),
            timestamp: Some(i as i64),
            updated_at: None,
            expires_at: None,
            source: None,
        }
    }

    async fn make_engine(base_path: &str) -> SstEngine {
        let mut fs_config = FilesystemConfig::default();
        fs_config.default_fs = Some(format!("file://{base_path}"));
        let filesystem = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        SstEngine::new_with_config(SstConfig::default(), filesystem, distance_compute)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn reconcile_deletion_vectors_marks_the_target_row_only() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base = temp_dir.path().to_str().unwrap().to_string();
        let collection = collection("cold_recovery_reconcile", &temp_dir);
        let engine = make_engine(&base).await;

        // Flush 3 records → an immutable cold `.pax` (OID resolver embedded).
        let records: Vec<VectorRecord> = ["r0", "r1", "r2"]
            .iter()
            .copied()
            .enumerate()
            .map(|(i, o)| record(o, i))
            .collect();
        let flush = FlushParameters {
            collection_id: Some(collection.id.clone()),
            vector_records: records.into_iter().map(Into::into).collect(),
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            batch_ids: vec![],
            collection_config: Some(collection.clone()),
            estimated_size: 0,
        };
        let res = engine.do_flush(&flush).await?;
        assert!(res.success, "flush should produce a cold segment");

        // Resolve the segment path + the on-disk row positions (the same space the
        // DV keys on).
        let storage_url = crate::storage::traits::StorageQueryContext::new(
            std::sync::Arc::new(crate::core::search::SearchParams::default()),
            std::sync::Arc::new(collection.clone()),
        )
        .collection_storage_path()
        .expect("ctx has a storage assignment");
        let files = engine.discover_sstable_files(&storage_url).await?;
        let seg = files
            .iter()
            .find(|f| f.ends_with(".pax"))
            .cloned()
            .expect("flush should have produced a .pax segment");
        let pax_local = seg.strip_prefix("file://").unwrap_or(&seg);
        let bytes = std::fs::read(pax_local)?;
        let on_disk = crate::storage::engines::sst::segment_format::read_segment_records(
            &bytes,
            &[],
            &[],
            None,
        )?;
        assert_eq!(on_disk.len(), 3, "segment should hold all 3 records");
        // Map oid -> on-disk position (the same space the DV keys on).
        let p0 = on_disk
            .iter()
            .position(|r| r.oid == "r0")
            .expect("r0 in segment") as u32;
        let p1 = on_disk
            .iter()
            .position(|r| r.oid == "r1")
            .expect("r1 in segment") as u32;

        let dv = engine
            .deletion_vector_store
            .as_ref()
            .expect("DV store is armed under cold-deletion-vectors");

        // Precondition: no DV bits set yet (the "stranded" state — the live
        // mark_deleted never ran, e.g. a crash stranded it).
        assert!(
            !dv.is_deleted_as_of(&seg, p1, u64::MAX).await,
            "r1 must not be deleted before reconciliation"
        );

        // Recovery reconciliation: re-mark r1's bit at its manifest_lsn (100).
        engine
            .reconcile_deletion_vectors(&collection.id, &[("r1".to_string(), 100)])
            .await?;

        // r1 is now deleted at gen 100 (visible at snapshot >= 100, invisible before
        // — correct MVCC, since the key is the durable manifest_lsn space).
        assert!(
            dv.is_deleted_as_of(&seg, p1, 100).await,
            "reconciliation must mark r1 @ gen 100"
        );
        assert!(
            dv.is_deleted_as_of(&seg, p1, u64::MAX).await,
            "r1 visible at a later snapshot"
        );
        assert!(
            !dv.is_deleted_as_of(&seg, p1, 99).await,
            "r1 invisible before its delete (MVCC snapshot-correct)"
        );
        // r0 (not reconciled) is untouched.
        assert!(
            !dv.is_deleted_as_of(&seg, p0, u64::MAX).await,
            "r0 must remain live (only r1 was reconciled)"
        );
        Ok(())
    }
}
