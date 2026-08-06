// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! TD-DELVEC-1 WI-3a-remaining-A: the per-segment **deletion-vector store** —
//! the durable, CAS'd home for the post-seal-mutable DVs.
//!
//! Immutable cold segments are sealed once and never rewritten; a cold delete
//! *overlays* a position bit on them. That mutable overlay cannot live in the
//! immutable segment, so it lives **here** — one `VersionedDeletionVector` per
//! segment, keyed by segment path, persisted as a co-located `{path}.dv` file
//! (TD §9.4/§10: the *resolver* is immutable → a footer region; the *DV* is
//! post-write-mutable → this external CAS'd store).
//!
//! Mirrors three landed patterns:
//! - `OidResolverCache` — the sharded `HashMap` keyed by segment path.
//! - `SstManifest` — `write_atomic` (temp+rename) durability + the monotonic
//!   manifest-version bump.
//! - `VersionedDeletionVector` — the MVCC bits themselves.
//!
//! **CAS = the in-process shard lock + `write_atomic`.** A `mark_deleted` holds
//! the shard's **write** lock across mutate→persist, so concurrent deletes on the
//! same segment serialize through the I/O (no lost bit). This is the same
//! single-replica model `SstManifest` uses (its `TransactionCoordinator` is
//! `#[allow(dead_code)]`); a true cross-replica CAS (object-store conditional
//! PUT) is the TD-0007 shared-ledger open item.
//!
//! **`tokio::sync::RwLock`** (not `std::sync`, as the sibling caches use) because
//! the lock is held across the async `write_atomic` — a std guard held across an
//! await trips `clippy::await_holding_lock` and can stall the runtime.
//!
//! **Disk-authoritative**: every mutation durably `write_atomic`s the `.dv`, so
//! the in-memory map is a lazy-loaded cache of disk ⇒ restart-safe (reload via
//! `load`) and could evict+reload. No eviction/budget for the MVP — the store is
//! bounded by segment count (unlike the byte-budgeted resolver cache).
//!
//! Wired end-to-end (TD-DELVEC-1 WI-3b..WI-6): the delete path sets bits
//! (`legacy.rs`), recovery re-marks them (`reconcile_deletion_vectors`),
//! merge-on-read consults them (`search_pax_file_exact` + `try_pax_cascade`),
//! and compaction reads + retires them (`build_deleted_oids` + retire loop).
//! Feature-gated `cold-deletion-vectors`.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::RwLock;

use proximadb_storage_common::deletion_vector::VersionedDeletionVector;

use crate::storage::persistence::filesystem::FilesystemFactory;

/// Shard count — a delete on segment S contends only with other deletes on S,
/// never with deletes on other segments (TD §10.4: "one contention point for
/// every delete" is the anti-pattern a single global lock recreates).
const DV_STORE_SHARDS: usize = 16;

/// Durable, CAS'd, per-segment store of `VersionedDeletionVector`s, keyed by
/// segment path — the single external, mutable artifact F3v needs.
pub struct DeletionVectorStore {
    shards: Box<[RwLock<HashMap<String, DvEntry>>]>,
    filesystem: Arc<FilesystemFactory>,
}

struct DvEntry {
    dv: VersionedDeletionVector,
    /// Monotonic manifest version (§7.2-3), bumped per mutation. The in-process
    /// lock is the CAS today; this version is the surface a future cross-replica
    /// CAS / manifest-reporting keys on (TD-0007).
    version: u64,
}

impl DeletionVectorStore {
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        let shards = (0..DV_STORE_SHARDS)
            .map(|_| RwLock::new(HashMap::new()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self { shards, filesystem }
    }

    fn shard_for(&self, path: &str) -> &RwLock<HashMap<String, DvEntry>> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut h);
        &self.shards[(h.finish() as usize) % self.shards.len()]
    }

    /// The `.dv` file path co-located with the segment.
    fn dv_path(segment_path: &str) -> String {
        format!("{segment_path}.dv")
    }

    /// Read a segment's DV from disk, if a `.dv` exists (`None` if absent — no
    /// deletes yet). Propagates a real I/O / deserialize error: the DV is
    /// *authoritative*, so a transient read failure must NOT be treated as
    /// "empty" or a subsequent mark would silently clobber real bits.
    async fn load_dv_from_disk(
        &self,
        segment_path: &str,
    ) -> Result<Option<VersionedDeletionVector>> {
        let dv_path = Self::dv_path(segment_path);
        let fs = self
            .filesystem
            .get_filesystem(&dv_path)
            .map_err(|e| anyhow!("dv load: filesystem for {dv_path}: {e:?}"))?;
        let exists = fs
            .exists(&dv_path)
            .await
            .map_err(|e| anyhow!("dv load: exists for {dv_path}: {e:?}"))?;
        if !exists {
            return Ok(None);
        }
        let bytes = fs
            .read(&dv_path)
            .await
            .map_err(|e| anyhow!("dv load: read for {dv_path}: {e:?}"))?;
        let dv = VersionedDeletionVector::deserialize(&bytes)
            .map_err(|e| anyhow!("dv load: deserialize for {dv_path}: {e}"))?;
        Ok(Some(dv))
    }

    /// Atomically persist a segment's DV bytes (temp+rename, crash-safe).
    async fn persist_dv(&self, dv_path: &str, data: Vec<u8>) -> Result<()> {
        let fs = self
            .filesystem
            .get_filesystem(dv_path)
            .map_err(|e| anyhow!("dv persist: filesystem for {dv_path}: {e:?}"))?;
        let strategy =
            crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory::create_metadata_strategy(
                &*fs, None,
            )
            .map_err(|e| anyhow!("dv persist: write strategy: {e}"))?;
        let write_options = strategy
            .create_file_options(&*fs, dv_path)
            .map_err(|e| anyhow!("dv persist: file options: {e}"))?;
        fs.write_atomic(dv_path, &data, Some(write_options))
            .await
            .map_err(|e| anyhow!("dv persist: write_atomic for {dv_path}: {e:?}"))?;
        Ok(())
    }

    /// Mark `position` deleted at generation `generation` (the delete's WAL LSN)
    /// in the segment's DV, persisting atomically. Returns `true` if newly marked
    /// (idempotent: a row is deleted once, earliest generation wins). Holds the
    /// shard write lock across mutate→persist so same-segment deletes serialize
    /// (CAS).
    pub async fn mark_deleted(
        &self,
        segment_path: &str,
        position: u32,
        generation: u64,
    ) -> Result<bool> {
        let dv_path = Self::dv_path(segment_path);
        let mut shard = self.shard_for(segment_path).write().await;
        // Lazy-load the on-disk DV if not resident, so a mark never clobbers
        // existing bits with a fresh empty DV.
        if !shard.contains_key(segment_path)
            && let Some(dv) = self.load_dv_from_disk(segment_path).await?
        {
            shard.insert(segment_path.to_string(), DvEntry { dv, version: 0 });
        }
        let entry = shard
            .entry(segment_path.to_string())
            .or_insert_with(|| DvEntry {
                dv: VersionedDeletionVector::new(),
                version: 0,
            });
        let newly = entry.dv.mark_deleted(position, generation);
        entry.version = entry.version.wrapping_add(1);
        let data = entry
            .dv
            .serialize()
            .map_err(|e| anyhow!("dv mark_deleted: serialize for {dv_path}: {e}"))?;
        // Persist under the held lock → same-segment deletes serialize through
        // the I/O. A write_atomic failure propagates as Err (fail-closed: the
        // delete path does not report success with an un-persisted bit).
        self.persist_dv(&dv_path, data).await?;
        Ok(newly)
    }

    /// Warm a segment's DV into memory from disk (the read-path pre-scan step).
    /// Idempotent; a no-op if no `.dv` exists. WI-4 merge-on-read calls this once
    /// per segment before `is_deleted_as_of` probes.
    pub async fn load(&self, segment_path: &str) -> Result<()> {
        let mut shard = self.shard_for(segment_path).write().await;
        if shard.contains_key(segment_path) {
            return Ok(());
        }
        if let Some(dv) = self.load_dv_from_disk(segment_path).await? {
            shard.insert(segment_path.to_string(), DvEntry { dv, version: 0 });
        }
        Ok(())
    }

    /// Whether `position` is deleted as of `snapshot_lsn` (MVCC). Takes the shard
    /// READ lock and does NO I/O (the entry must be warmed via `load` /
    /// `mark_deleted` first); returns `false` for a not-yet-loaded segment.
    pub async fn is_deleted_as_of(
        &self,
        segment_path: &str,
        position: u32,
        snapshot_lsn: u64,
    ) -> bool {
        let shard = self.shard_for(segment_path).read().await;
        match shard.get(segment_path) {
            Some(entry) => entry.dv.is_deleted_as_of(position, snapshot_lsn),
            None => false,
        }
    }

    /// A clone of the segment's DV (for compaction carry, WI-6), if resident.
    pub async fn get(&self, segment_path: &str) -> Option<VersionedDeletionVector> {
        let shard = self.shard_for(segment_path).read().await;
        shard.get(segment_path).map(|e| e.dv.clone())
    }

    /// The segment's manifest version (mutations applied) — diagnostics + the
    /// surface a future cross-replica CAS keys on. `None` if not resident.
    pub async fn manifest_version(&self, segment_path: &str) -> Option<u64> {
        let shard = self.shard_for(segment_path).read().await;
        shard.get(segment_path).map(|e| e.version)
    }

    /// Retire a segment's DV — drop the in-memory entry and delete the `.dv` file
    /// (compaction retire, after the carry). Idempotent: a missing file is OK.
    pub async fn remove(&self, segment_path: &str) -> Result<()> {
        {
            let mut shard = self.shard_for(segment_path).write().await;
            shard.remove(segment_path);
        }
        let dv_path = Self::dv_path(segment_path);
        let fs = self
            .filesystem
            .get_filesystem(&dv_path)
            .map_err(|e| anyhow!("dv remove: filesystem for {dv_path}: {e:?}"))?;
        match fs.delete(&dv_path).await {
            Ok(()) => Ok(()),
            Err(_) => {
                // A missing file (segment never had deletes, or already retired)
                // is success; anything still present is a real failure.
                let still_exists = fs.exists(&dv_path).await.unwrap_or(false);
                if still_exists {
                    Err(anyhow!("dv remove: delete failed for {dv_path}"))
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A store over a fresh temp dir (mirrors `create_test_manifest`,
    /// manifest.rs:466-475). The temp dir is returned so the caller keeps it
    /// alive for the test's lifetime.
    async fn make_store() -> (DeletionVectorStore, TempDir, String) {
        let temp_dir = TempDir::new().expect("tempdir");
        let filesystem = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("filesystem"),
        );
        let base = format!("file://{}", temp_dir.path().display());
        (DeletionVectorStore::new(filesystem), temp_dir, base)
    }

    #[tokio::test]
    async fn mark_persists_and_survives_reload() {
        let (store, tmp, base) = make_store().await;
        let seg = format!("{base}/seg.pax");
        assert!(store.mark_deleted(&seg, 3, 10).await.expect("mark"));
        assert!(store.is_deleted_as_of(&seg, 3, 10).await);

        // Simulate restart: a fresh store over the SAME dir (temp dir kept alive).
        let filesystem = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("filesystem"),
        );
        let store2 = DeletionVectorStore::new(filesystem);
        // Not loaded yet → returns false (entry not resident).
        assert!(
            !store2.is_deleted_as_of(&seg, 3, 10).await,
            "not loaded yet → not deleted"
        );
        // After load, the persisted bit is visible (disk-authoritative).
        store2.load(&seg).await.expect("load");
        assert!(
            store2.is_deleted_as_of(&seg, 3, 10).await,
            "loaded from disk → visible"
        );
        drop(tmp);
    }

    #[tokio::test]
    async fn is_deleted_as_of_is_mvcc_snapshot_correct() {
        let (store, _tmp, base) = make_store().await;
        let seg = format!("{base}/seg.pax");
        // pos 2 deleted @ LSN 10, pos 5 @ LSN 20.
        assert!(store.mark_deleted(&seg, 2, 10).await.expect("mark"));
        assert!(store.mark_deleted(&seg, 5, 20).await.expect("mark"));

        store.load(&seg).await.expect("load"); // resident
        // snapshot 15 sees pos 2 (10 ≤ 15) but NOT pos 5 (20 > 15).
        assert!(store.is_deleted_as_of(&seg, 2, 15).await);
        assert!(!store.is_deleted_as_of(&seg, 5, 15).await);
        // snapshot 25 sees both.
        assert!(store.is_deleted_as_of(&seg, 2, 25).await);
        assert!(store.is_deleted_as_of(&seg, 5, 25).await);
        // snapshot 9 sees neither.
        assert!(!store.is_deleted_as_of(&seg, 2, 9).await);
    }

    #[tokio::test]
    async fn concurrent_same_segment_deletes_lose_no_bit() {
        let (store, _tmp, base) = make_store().await;
        let store = Arc::new(store);
        let seg = format!("{base}/seg.pax");
        // 50 concurrent deletes of distinct positions at gens 1..=50.
        let mut handles = Vec::new();
        for i in 0..50u32 {
            let s = store.clone();
            let seg = seg.clone();
            handles.push(tokio::spawn(async move {
                s.mark_deleted(&seg, i, (i as u64) + 1).await
            }));
        }
        for h in handles {
            assert!(h.await.expect("join").is_ok(), "mark failed");
        }
        // All 50 bits present — no lost bit.
        for i in 0..50u32 {
            assert!(
                store.is_deleted_as_of(&seg, i, u64::MAX).await,
                "pos {i} lost"
            );
        }
        // Idempotent re-delete of pos 0 at a later gen is a no-op (earliest wins).
        assert!(!store.mark_deleted(&seg, 0, 999).await.expect("mark"));
        assert_eq!(store.get(&seg).await.expect("dv").delete_gen(0), Some(1));
    }

    #[tokio::test]
    async fn segments_are_independent() {
        let (store, _tmp, base) = make_store().await;
        let seg_a = format!("{base}/a.pax");
        let seg_b = format!("{base}/b.pax");
        store.mark_deleted(&seg_a, 1, 5).await.expect("mark");
        store.mark_deleted(&seg_b, 2, 5).await.expect("mark");
        assert!(store.is_deleted_as_of(&seg_a, 1, 5).await);
        assert!(
            !store.is_deleted_as_of(&seg_a, 2, 5).await,
            "pos 2 not in seg_a"
        );
        assert!(store.is_deleted_as_of(&seg_b, 2, 5).await);
        assert!(
            !store.is_deleted_as_of(&seg_b, 1, 5).await,
            "pos 1 not in seg_b"
        );
    }

    #[tokio::test]
    async fn remove_retires_inmemory_and_disk() {
        let (store, tmp, base) = make_store().await;
        let seg = format!("{base}/seg.pax");
        let dv_path = format!("{seg}.dv");
        store.mark_deleted(&seg, 1, 5).await.expect("mark");
        assert!(store.is_deleted_as_of(&seg, 1, 5).await);

        store.remove(&seg).await.expect("remove");
        // in-memory entry gone.
        assert!(!store.is_deleted_as_of(&seg, 1, 5).await);
        // disk file gone (re-open over the same dir).
        let checker = FilesystemFactory::create(Default::default())
            .await
            .expect("fs");
        assert!(
            !checker.exists(&dv_path).await.expect("exists"),
            ".dv file retired from disk"
        );
        drop(tmp);
    }

    #[tokio::test]
    async fn corrupt_dv_file_is_rejected_not_misread() {
        let (store, tmp, base) = make_store().await;
        let seg = format!("{base}/seg.pax");
        let dv_path = format!("{seg}.dv");
        // Plant garbage at the .dv path (bad magic — not a valid VDV2 blob).
        let planter = FilesystemFactory::create(Default::default())
            .await
            .expect("fs");
        planter
            .write(&dv_path, b"not-a-valid-VDV2-blob", None)
            .await
            .expect("write");
        // load must error (bad magic), NOT silently treat as empty.
        let res = store.load(&seg).await;
        assert!(
            res.is_err(),
            "a corrupt .dv is rejected, not misread as empty"
        );
        drop(tmp);
    }

    #[tokio::test]
    async fn manifest_version_increments_per_mutation() {
        let (store, _tmp, base) = make_store().await;
        let seg = format!("{base}/seg.pax");
        assert_eq!(store.manifest_version(&seg).await, None);
        store.mark_deleted(&seg, 1, 10).await.expect("mark");
        assert_eq!(store.manifest_version(&seg).await, Some(1));
        store.mark_deleted(&seg, 2, 11).await.expect("mark");
        assert_eq!(store.manifest_version(&seg).await, Some(2));
        // an idempotent re-mark still bumps the version (it was a write attempt).
        store.mark_deleted(&seg, 1, 99).await.expect("mark");
        assert_eq!(store.manifest_version(&seg).await, Some(3));
    }
}
