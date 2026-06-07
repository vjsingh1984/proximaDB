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
use object_store::path::Path;
use proximadb_kernel::error::StorageError;
use proximadb_object_store::ProximaObjectStore;

/// Outcome of an attempted [`ManifestCommitter::commit`].
///
/// `Conflict` is an expected control-flow result of optimistic concurrency, not an
/// error — only genuine I/O failures surface as `Err(StorageError)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// The manifest was atomically published as this new version.
    Committed(u64),
    /// Another committer already claimed the target slot. `latest` is the highest
    /// version currently present — the snapshot to rebase onto and retry from.
    Conflict { latest: Option<u64> },
}

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use std::sync::Arc;

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

        let out = c.commit(None, Bytes::from_static(b"snapshot-0")).await.unwrap();
        assert_eq!(out, CommitOutcome::Committed(0));
        assert_eq!(c.latest_version().await.unwrap(), Some(0));
        assert_eq!(c.read_manifest(0).await.unwrap(), Bytes::from_static(b"snapshot-0"));
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
        let winner = c.commit(Some(0), Bytes::from_static(b"winner")).await.unwrap();
        let loser = c.commit(Some(0), Bytes::from_static(b"loser")).await.unwrap();

        assert_eq!(winner, CommitOutcome::Committed(1));
        assert_eq!(loser, CommitOutcome::Conflict { latest: Some(1) });
        // The slot holds the winner's manifest, untouched by the loser.
        assert_eq!(c.read_manifest(1).await.unwrap(), Bytes::from_static(b"winner"));
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
        assert_eq!(c.manifest_path(0).as_ref(), "a/b/_manifests/v00000000000000000000.manifest");
        assert_eq!(c.latest_version().await.unwrap(), Some(0));
    }

    /// Non-manifest objects sharing the prefix are ignored by version discovery.
    #[tokio::test]
    async fn unrelated_objects_under_prefix_are_ignored() {
        let store = ProximaObjectStore::new(Arc::new(InMemory::new()));
        store
            .put(&Path::from("data/t/ns/_manifests/README.txt"), Bytes::from_static(b"hi"))
            .await
            .unwrap();
        let c = ManifestCommitter::new(store, "data/t/ns/_manifests");
        assert_eq!(c.latest_version().await.unwrap(), None, "non-manifest objects don't count");
        assert_eq!(
            c.commit(None, Bytes::from_static(b"s0")).await.unwrap(),
            CommitOutcome::Committed(0)
        );
        assert_eq!(c.latest_version().await.unwrap(), Some(0));
    }
}
