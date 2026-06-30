//! # manifest — Iceberg-style atomic manifest commits over object storage
//!
//! The warehouse base tier needs a way to publish a new table snapshot **atomically**
//! over decoupled object storage, where there is no transaction manager — only
//! per-object operations. This module supplies the optimistic-concurrency commit
//! primitive built on [`ProximaObjectStore::put_if_absent`] (create-only put):
//!
//! - Snapshots are immutable, **monotonically-versioned** manifest objects named
//!   `{prefix}/v{version}.manifest` (zero-padded so a lexical `list` is in numeric order).
//! - To publish snapshot `parent + 1`, a committer **claims the successor slot** with a
//!   create-only put. If the slot already exists, another committer won the race: the
//!   loser gets a [`CommitOutcome::Conflict`] carrying the latest version to rebase onto
//!   and retry — exactly the Iceberg compare-and-swap commit protocol, minus a catalog.
//!
//! The manifest **bytes are opaque** to the committer: the caller serializes whatever
//! snapshot it needs (typically the set of data-file paths from
//! [`ObjectStoreBridge::list_objects`](crate::IcebergObjectStoreBridge), plus row
//! counts / column stats). This crate owns the *atomicity*, not the manifest schema.

use bytes::Bytes;
use chrono::{DateTime, Utc};
use object_store::path::Path;
use proximadb_kernel::error::StorageError;
use proximadb_object_store::ProximaObjectStore;
pub use proximadb_storage_common::object_store_bridge::CommitOutcome;

/// Floor for [`ManifestCommitter::prune_retention`]'s `keep_k`. Below this the log
/// would collapse to little more than the tip, eliminating the concurrency window in
/// which a reader may still hold a just-read parent and leaving a single point of
/// failure for the generation fence.
const MIN_PRUNE_KEEP_K: usize = 2;

/// Atomic, optimistic-concurrency manifest committer over a [`ProximaObjectStore`].
///
/// One committer serves one manifest log (selected by `prefix`); construct another for
/// a different table/log.
pub struct ManifestCommitter {
    store: ProximaObjectStore,
    prefix: String,
}

impl ManifestCommitter {
    /// Create a committer writing manifests under `prefix` (e.g. `"data/<tenant>/<ns>/_manifests"`).
    pub fn new(store: ProximaObjectStore, prefix: impl Into<String>) -> Self {
        let mut prefix = prefix.into();
        // Normalize so `{prefix}/v..` never produces a double slash or a leading one.
        while prefix.ends_with('/') {
            prefix.pop();
        }
        Self { store, prefix }
    }

    /// Object path of the manifest for `version`. Zero-padded to 20 digits (covers all
    /// of `u64`) so the lexical order returned by `list` matches numeric version order.
    fn manifest_path(&self, version: u64) -> Path {
        Path::from(format!("{}/v{version:020}.manifest", self.prefix))
    }

    /// Parse the version out of a manifest object key's final segment, ignoring any
    /// other objects that happen to live under the prefix.
    fn parse_version(name: &str) -> Option<u64> {
        name.strip_prefix('v')?
            .strip_suffix(".manifest")?
            .parse::<u64>()
            .ok()
    }

    /// The highest committed version, or `None` if the log is empty.
    pub async fn latest_version(&self) -> Result<Option<u64>, StorageError> {
        let prefix = Path::from(self.prefix.as_str());
        let metas = self.store.list(Some(&prefix)).await?;
        Ok(metas
            .iter()
            .filter_map(|m| m.location.filename().and_then(Self::parse_version))
            .max())
    }

    /// Read the raw manifest bytes for a specific `version`.
    pub async fn read_manifest(&self, version: u64) -> Result<Bytes, StorageError> {
        self.store.get(&self.manifest_path(version)).await
    }

    /// Atomically publish `manifest` as the successor of `parent` (`None` ⇒ the first
    /// commit, version `0`).
    ///
    /// Returns [`CommitOutcome::Committed`] with the new version on success, or
    /// [`CommitOutcome::Conflict`] (carrying the current latest version) if another
    /// committer already claimed the target slot. Real I/O errors propagate as `Err`.
    pub async fn commit(
        &self,
        parent: Option<u64>,
        manifest: Bytes,
    ) -> Result<CommitOutcome, StorageError> {
        let target = match parent {
            Some(p) => p.checked_add(1).ok_or_else(|| {
                StorageError::Serialization("manifest: version counter overflow".into())
            })?,
            None => 0,
        };
        match self
            .store
            .put_if_absent(&self.manifest_path(target), manifest)
            .await
        {
            Ok(()) => Ok(CommitOutcome::Committed(target)),
            Err(StorageError::AlreadyExists(_)) => Ok(CommitOutcome::Conflict {
                latest: self.latest_version().await?,
            }),
            Err(other) => Err(other),
        }
    }

    /// Commit `payload` as a **generation-fenced** successor of `parent` (TD-117/TD-119).
    ///
    /// On top of the version CAS in [`commit`](Self::commit), this fences a *stale
    /// writer*: a `generation` strictly lower than the generation embedded in the latest
    /// committed manifest is rejected with [`CommitOutcome::Conflict`] **before** the slot
    /// is claimed. This is the object-store analog of Neon's `index_part.json` +
    /// generation single-writer guarantee — a resurrected/forked writer carrying an old
    /// generation cannot clobber a branch that a newer writer has taken over.
    ///
    /// The generation is stored as an 8-byte big-endian header prepended to `payload`;
    /// read it back with [`read_fenced`](Self::read_fenced). Existing manifests written by
    /// plain [`commit`](Self::commit) decode as generation `0`, so an unfenced log upgrades
    /// transparently.
    pub async fn commit_fenced(
        &self,
        parent: Option<u64>,
        generation: u64,
        payload: Bytes,
    ) -> Result<CommitOutcome, StorageError> {
        if let Some(latest) = self.latest_version().await? {
            let (existing_generation, _) = decode_fenced(&self.read_manifest(latest).await?);
            if generation < existing_generation {
                // Stale writer: a newer generation already owns this log.
                return Ok(CommitOutcome::Conflict {
                    latest: Some(latest),
                });
            }
        }
        self.commit(parent, encode_fenced(generation, &payload))
            .await
    }

    /// Read a generation-fenced manifest, returning `(generation, payload)`.
    /// Manifests written by plain [`commit`](Self::commit) decode as generation `0`.
    pub async fn read_fenced(&self, version: u64) -> Result<(u64, Bytes), StorageError> {
        Ok(decode_fenced(&self.read_manifest(version).await?))
    }

    /// Best-effort retention prune of superseded manifest objects.
    ///
    /// The manifest log is append-only — every commit creates a new `v{N}.manifest`
    /// via a create-only put, and historically **nothing reclaimed the stale tail**.
    /// A long-lived lease renewed every few seconds therefore grows without bound
    /// (observed: ~48k objects per collection, ~562 MB), which made every
    /// [`latest_version`](Self::latest_version) a full O(n) `list` that pinned a CPU
    /// core; on a cloud store each such `list` is a paginated HTTP LIST that grows
    /// slower and costlier as `n` grows. Pruning the tail caps `n` — which is *itself*
    /// the read-path fix, so no mutable tip pointer is needed (a pointer would
    /// reintroduce the lost-update/ABA hazard the create-only-put protocol avoids).
    ///
    /// # Safety
    ///
    /// Only the **tip** (the max version) is ever read — by
    /// [`commit_fenced`](Self::commit_fenced), [`read_fenced`](Self::read_fenced) and
    /// the version CAS — so it is **never deleted**. A version becomes eligible only
    /// when it is **both**:
    ///
    /// - ranked `keep_k` or more behind the tip (rank is the position in the sorted
    ///   version list, **not** `tip - v` arithmetic, so a log with gaps from a prior
    ///   partial prune is still ranked correctly), **and**
    /// - at least `min_age` old (a grace window that protects the recent burst and the
    ///   tombstone a release publishes at `generation + 1`).
    ///
    /// `keep_k` is clamped to [`MIN_PRUNE_KEEP_K`]. A future-dated `last_modified`
    /// (cloud clock skew) is treated as not-yet-of-age and never reaped early. Deletes
    /// are best-effort: a transient error — including [`StorageError::NotFound`] for an
    /// object a concurrent pass already removed — is logged and the pass continues. A
    /// crash mid-pass leaves the log half-pruned; the next pass recomputes from a fresh
    /// `list`. Returns the number of objects deleted.
    pub async fn prune_retention(
        &self,
        keep_k: usize,
        min_age: std::time::Duration,
    ) -> Result<usize, StorageError> {
        // Defense in depth — the call sites clamp too, but never let the log collapse
        // past the tip-plus-predecessor concurrency window.
        let keep_k = keep_k.max(MIN_PRUNE_KEEP_K);
        let min_age = chrono::Duration::from_std(min_age).unwrap_or(chrono::Duration::zero());

        let prefix = Path::from(self.prefix.as_str());
        let mut entries: Vec<(u64, DateTime<Utc>)> = self
            .store
            .list(Some(&prefix))
            .await?
            .into_iter()
            .filter_map(|m| {
                let version = Self::parse_version(m.location.filename()?)?;
                Some((version, m.last_modified))
            })
            .collect();
        if entries.len() <= keep_k {
            return Ok(0);
        }

        // Oldest first; the tip is the last element and is never eligible.
        entries.sort_unstable_by_key(|(v, _)| *v);
        let now = Utc::now();
        let count = entries.len();

        let mut deleted = 0usize;
        for (idx, (version, last_modified)) in entries.iter().enumerate() {
            // rank 0 == the tip; rank grows toward the head of the list. Newer entries
            // have smaller rank, so once rank drops below `keep_k` everything that
            // remains is within the keep window — stop early.
            let rank_from_tip = count - 1 - idx;
            if rank_from_tip < keep_k {
                break;
            }
            // `age < min_age` also covers a future-dated mtime (negative age) from cloud
            // clock skew — such an object is never reaped early.
            let age = now.signed_duration_since(*last_modified);
            if age < min_age {
                continue;
            }
            // Delete via the canonical `manifest_path(version)` — the same form `get` /
            // `put_if_absent` address. A list-returned `location` is not always
            // delete-safe across backends (the local store returns a root-relative form
            // that `delete` would double-prefix), but the constructed path round-trips.
            match self.store.delete(&self.manifest_path(*version)).await {
                Ok(()) => deleted += 1,
                // A concurrent pass (or a cross-pod owner) already removed it.
                Err(StorageError::NotFound(_)) => {}
                Err(e) => tracing::warn!(
                    target: "proximadb::manifest::prune",
                    prefix = %self.prefix,
                    version,
                    error = %e,
                    "best-effort manifest delete failed; continuing pass"
                ),
            }
        }
        Ok(deleted)
    }
}

/// Prepend the 8-byte big-endian `generation` header to `payload`.
fn encode_fenced(generation: u64, payload: &[u8]) -> Bytes {
    let mut buf = Vec::with_capacity(8 + payload.len());
    buf.extend_from_slice(&generation.to_be_bytes());
    buf.extend_from_slice(payload);
    Bytes::from(buf)
}

/// Split a fenced manifest into `(generation, payload)`. Bytes too short to carry a
/// header decode as generation `0` with the whole buffer as payload (back-compat).
fn decode_fenced(bytes: &Bytes) -> (u64, Bytes) {
    if bytes.len() < 8 {
        return (0, bytes.clone());
    }
    let mut header = [0u8; 8];
    header.copy_from_slice(&bytes[..8]);
    (u64::from_be_bytes(header), bytes.slice(8..))
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::ObjectStoreExt;
    use object_store::memory::InMemory;
    use std::sync::Arc;
    use std::time::Duration;

    fn committer() -> ManifestCommitter {
        ManifestCommitter::new(
            ProximaObjectStore::new(Arc::new(InMemory::new())),
            "data/t/ns/_manifests",
        )
    }

    /// An empty log has no latest version; the first commit lands at v0 and is readable.
    #[tokio::test]
    async fn first_commit_is_version_zero() {
        let c = committer();
        assert_eq!(c.latest_version().await.unwrap(), None);

        let out = c
            .commit(None, Bytes::from_static(b"snapshot-0"))
            .await
            .unwrap();
        assert_eq!(out, CommitOutcome::Committed(0));
        assert_eq!(c.latest_version().await.unwrap(), Some(0));
        assert_eq!(
            c.read_manifest(0).await.unwrap(),
            Bytes::from_static(b"snapshot-0")
        );
    }

    /// Sequential commits advance the version monotonically and each is independently readable.
    #[tokio::test]
    async fn sequential_commits_advance_version() {
        let c = committer();
        assert_eq!(
            c.commit(None, Bytes::from_static(b"s0")).await.unwrap(),
            CommitOutcome::Committed(0)
        );
        assert_eq!(
            c.commit(Some(0), Bytes::from_static(b"s1")).await.unwrap(),
            CommitOutcome::Committed(1)
        );
        assert_eq!(
            c.commit(Some(1), Bytes::from_static(b"s2")).await.unwrap(),
            CommitOutcome::Committed(2)
        );
        assert_eq!(c.latest_version().await.unwrap(), Some(2));
        assert_eq!(c.read_manifest(1).await.unwrap(), Bytes::from_static(b"s1"));
    }

    /// Two committers racing from the SAME parent: one wins the slot, the other is told
    /// it conflicted and given the latest version to rebase onto. The winner's bytes
    /// are never clobbered.
    #[tokio::test]
    async fn concurrent_commit_from_same_parent_conflicts() {
        let c = committer();
        c.commit(None, Bytes::from_static(b"s0")).await.unwrap();

        // Both attempt to publish v1 as the successor of v0.
        let winner = c
            .commit(Some(0), Bytes::from_static(b"winner"))
            .await
            .unwrap();
        let loser = c
            .commit(Some(0), Bytes::from_static(b"loser"))
            .await
            .unwrap();

        assert_eq!(winner, CommitOutcome::Committed(1));
        assert_eq!(loser, CommitOutcome::Conflict { latest: Some(1) });
        // The slot holds the winner's manifest, untouched by the loser.
        assert_eq!(
            c.read_manifest(1).await.unwrap(),
            Bytes::from_static(b"winner")
        );
    }

    /// A trailing slash in the prefix must not change where manifests land.
    #[tokio::test]
    async fn prefix_trailing_slash_is_normalized() {
        let c = ManifestCommitter::new(
            ProximaObjectStore::new(Arc::new(InMemory::new())),
            "a/b/_manifests/",
        );
        assert_eq!(
            c.commit(None, Bytes::from_static(b"x")).await.unwrap(),
            CommitOutcome::Committed(0)
        );
        assert_eq!(
            c.manifest_path(0).as_ref(),
            "a/b/_manifests/v00000000000000000000.manifest"
        );
        assert_eq!(c.latest_version().await.unwrap(), Some(0));
    }

    /// Non-manifest objects sharing the prefix are ignored by version discovery.
    #[tokio::test]
    async fn unrelated_objects_under_prefix_are_ignored() {
        let store = ProximaObjectStore::new(Arc::new(InMemory::new()));
        store
            .put(
                &Path::from("data/t/ns/_manifests/README.txt"),
                Bytes::from_static(b"hi"),
            )
            .await
            .unwrap();
        let c = ManifestCommitter::new(store, "data/t/ns/_manifests");
        assert_eq!(
            c.latest_version().await.unwrap(),
            None,
            "non-manifest objects don't count"
        );
        assert_eq!(
            c.commit(None, Bytes::from_static(b"s0")).await.unwrap(),
            CommitOutcome::Committed(0)
        );
        assert_eq!(c.latest_version().await.unwrap(), Some(0));
    }

    /// A stale writer (generation lower than the latest committed) is fenced before it
    /// can claim a slot; a current/newer generation commits and round-trips.
    #[tokio::test]
    async fn fenced_commit_rejects_stale_generation() {
        let c = committer();
        // Generation 5 takes ownership of the log at v0.
        assert_eq!(
            c.commit_fenced(None, 5, Bytes::from_static(b"g5"))
                .await
                .unwrap(),
            CommitOutcome::Committed(0)
        );
        let (generation, payload) = c.read_fenced(0).await.unwrap();
        assert_eq!(generation, 5);
        assert_eq!(payload, Bytes::from_static(b"g5"));

        // A resurrected writer with an older generation is fenced; no slot is claimed.
        assert_eq!(
            c.commit_fenced(Some(0), 3, Bytes::from_static(b"stale"))
                .await
                .unwrap(),
            CommitOutcome::Conflict { latest: Some(0) }
        );
        assert_eq!(c.latest_version().await.unwrap(), Some(0));

        // The current generation (>= latest) advances the log normally.
        assert_eq!(
            c.commit_fenced(Some(0), 5, Bytes::from_static(b"g5-next"))
                .await
                .unwrap(),
            CommitOutcome::Committed(1)
        );
        assert_eq!(c.read_fenced(1).await.unwrap().0, 5);
    }

    /// A plain (unfenced) manifest decodes as generation 0 for forward compatibility.
    #[tokio::test]
    async fn plain_commit_decodes_as_generation_zero() {
        let c = committer();
        c.commit(None, Bytes::from_static(b"plain")).await.unwrap();
        assert_eq!(c.read_fenced(0).await.unwrap().0, 0);
    }

    // ---- prune_retention ----

    fn committer_with_store() -> (ManifestCommitter, Arc<InMemory>) {
        let backend = Arc::new(InMemory::new());
        let c = ManifestCommitter::new(
            ProximaObjectStore::new(backend.clone()),
            "data/t/ns/_manifests",
        );
        (c, backend)
    }

    /// Seed `n` fenced commits at a fixed generation; returns the final version.
    async fn seed_fenced(c: &ManifestCommitter, n: u64, generation: u64) -> u64 {
        let mut parent: Option<u64> = None;
        let mut last = 0;
        for v in 0..n {
            assert_eq!(
                c.commit_fenced(parent, generation, Bytes::from_static(b"g"))
                    .await
                    .unwrap(),
                CommitOutcome::Committed(v)
            );
            parent = Some(v);
            last = v;
        }
        last
    }

    /// Pruning deletes only the stale tail; the tip is always retained and readable.
    #[tokio::test]
    async fn prune_never_deletes_tip() {
        let (c, _backend) = committer_with_store();
        let tip = seed_fenced(&c, 50, 5).await;
        assert_eq!(tip, 49);

        let deleted = c.prune_retention(10, Duration::ZERO).await.unwrap();
        assert_eq!(deleted, 40, "keep newest 10 of 50");
        assert_eq!(c.latest_version().await.unwrap(), Some(49));
        let (tip_gen, _bytes) = c.read_fenced(49).await.unwrap();
        assert_eq!(tip_gen, 5, "tip still decodes after prune");
    }

    /// After pruning, the owner can still commit a fenced successor — the tip's
    /// generation header survives. This is the load-bearing fence test.
    #[tokio::test]
    async fn prune_preserves_fenced_commit_after_prune() {
        let (c, _backend) = committer_with_store();
        let tip = seed_fenced(&c, 50, 5).await;
        assert_eq!(c.prune_retention(5, Duration::ZERO).await.unwrap(), 45);

        let out = c
            .commit_fenced(Some(tip), 5, Bytes::from_static(b"next"))
            .await
            .unwrap();
        assert_eq!(out, CommitOutcome::Committed(50));
    }

    /// A stale writer (generation below the tip's) is still fenced after pruning.
    #[tokio::test]
    async fn prune_then_stale_writer_still_fenced() {
        let (c, _backend) = committer_with_store();
        let tip = seed_fenced(&c, 50, 5).await;
        c.prune_retention(5, Duration::ZERO).await.unwrap();

        let out = c
            .commit_fenced(Some(tip), 4, Bytes::from_static(b"stale"))
            .await
            .unwrap();
        // Fenced at the generation check *before* any slot is claimed — the tip (still
        // 49 after pruning) is returned, and no v50 is created.
        assert_eq!(out, CommitOutcome::Conflict { latest: Some(tip) });
        assert_eq!(c.latest_version().await.unwrap(), Some(tip));
    }

    /// `min_age` grants recent history a grace window: a large `min_age` reaps nothing,
    /// then dropping it to zero reaps the stale tail while the keep window stays intact.
    #[tokio::test]
    async fn prune_respects_min_age_grace_window() {
        let (c, _backend) = committer_with_store();
        seed_fenced(&c, 50, 5).await;

        // Everything was written moments ago → all within the grace window.
        assert_eq!(
            c.prune_retention(2, Duration::from_secs(3600))
                .await
                .unwrap(),
            0,
            "min_age=1h keeps the recent burst"
        );
        // Drop the age gate; keep only the newest 2.
        assert_eq!(
            c.prune_retention(2, Duration::ZERO).await.unwrap(),
            48,
            "keep newest 2 of 50"
        );
        assert_eq!(c.latest_version().await.unwrap(), Some(49));
        assert!(c.read_manifest(48).await.is_ok(), "predecessor retained");
    }

    /// Rank is the position in the sorted version list, so a log with gaps (from a
    /// prior partial prune) is still ranked correctly — not by `tip - v` arithmetic.
    #[tokio::test]
    async fn prune_rank_robust_to_gaps() {
        let (c, backend) = committer_with_store();
        seed_fenced(&c, 50, 5).await;
        // Simulate a prior partial prune that already removed v25.
        let gap_path = c.manifest_path(25);
        backend.delete(&gap_path).await.unwrap();
        assert!(c.read_manifest(25).await.is_err());

        // 49 remaining; keep newest 5 by rank (v45..v49) → delete 44.
        let deleted = c.prune_retention(5, Duration::ZERO).await.unwrap();
        assert_eq!(deleted, 44);
        for v in 45..50u64 {
            assert!(c.read_manifest(v).await.is_ok(), "v{v} retained");
        }
        assert_eq!(c.latest_version().await.unwrap(), Some(49));
    }

    /// Pruning concurrently with commits never breaks the fence: the commit outcome is
    /// always well-formed and the tip stays readable.
    #[tokio::test]
    async fn prune_concurrent_with_commit_fenced_is_safe() {
        let (c, _backend) = committer_with_store();
        let mut tip = seed_fenced(&c, 40, 5).await;

        for _ in 0..20 {
            let (pruned, committed) = tokio::join!(
                c.prune_retention(5, Duration::ZERO),
                c.commit_fenced(Some(tip), 5, Bytes::from_static(b"r")),
            );
            let pruned = pruned.unwrap();
            let committed = committed.unwrap();
            assert!(
                matches!(committed, CommitOutcome::Committed(_)),
                "commit must not error under concurrent prune (got {committed:?}, pruned {pruned})"
            );
            if let CommitOutcome::Committed(v) = committed {
                tip = v;
            }
        }
        assert_eq!(c.latest_version().await.unwrap(), Some(tip));
        let (tip_gen, _) = c.read_fenced(tip).await.unwrap();
        assert_eq!(tip_gen, 5);
    }

    /// An empty log is a no-op (never errors, deletes nothing).
    #[tokio::test]
    async fn prune_empty_log_is_noop() {
        let (c, _backend) = committer_with_store();
        assert_eq!(
            c.prune_retention(10, Duration::from_secs(60))
                .await
                .unwrap(),
            0
        );
        assert_eq!(c.latest_version().await.unwrap(), None);
    }

    /// `keep_k` below the floor is clamped, not honored — the log never collapses past
    /// the tip-plus-predecessor window.
    #[tokio::test]
    async fn prune_clamps_keep_k_below_minimum() {
        let (c, _backend) = committer_with_store();
        seed_fenced(&c, 10, 5).await;
        // keep_k=0 → clamped to MIN_PRUNE_KEEP_K (2): delete 8, keep newest 2.
        let deleted = c.prune_retention(0, Duration::ZERO).await.unwrap();
        assert_eq!(deleted, 8);
        assert_eq!(c.latest_version().await.unwrap(), Some(9));
        assert!(c.read_manifest(8).await.is_ok(), "predecessor retained");
        assert!(c.read_manifest(7).await.is_err(), "v7 reaped");
    }
}
