//! Pluggable durable home for the system catalog's snapshot blob (Phase 5c of
//! the two-tier operator/account system-catalog redesign).
//!
//! The catalog's bounded-restart machinery (Phase 3) periodically writes a
//! point-in-time snapshot of the in-RAM authority, then compacts the WAL up to
//! that watermark. This trait abstracts **only** the snapshot blob's durable
//! read/write so the snapshot can live on the local filesystem (the default,
//! behaviour-identical to before) or on object storage
//! (`s3://`/`gs://`/`az://`/`memory://`) under the tenant/operator `DrPathBuilder`
//! prefix — without touching the crash-consistency ordering or the local WAL.
//!
//! Object-store-*native* WAL durability (per-DDL delta objects) is deferred to
//! Phase 6, where that delta-object log doubles as the multi-pod CAS substrate;
//! building it here would mean building it twice. Until then an object-store
//! deployment keeps its per-DDL WAL on the local working volume and replicates
//! the snapshot to object storage.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

use proximadb_iceberg_engine::manifest::{CommitOutcome, ManifestCommitter};
use proximadb_object_store::ProximaObjectStore;

/// Durable, atomically-replaceable home for the catalog snapshot blob.
///
/// `write_atomic` must guarantee a reader concurrent with the write observes
/// either the previous blob or the complete new one — never a torn write. This
/// is the load-bearing property the Phase-3 crash-consistency ordering relies
/// on (snapshot made durable *before* the WAL is compacted).
///
/// Writes carry a monotonic **fencing generation** (Phase 6a). A local store
/// ignores it (single-pod, no contention). An object-store store routes the
/// write through a generation-fenced commit so a *stale* writer — one whose
/// generation is lower than a newer pod that has taken the catalog over — is
/// rejected instead of clobbering the newer pod's snapshot.
#[async_trait]
pub trait CatalogSnapshotStore: Send + Sync {
    /// Read the current snapshot blob with its fencing generation, or `None` if
    /// none has been written yet. Local (unfenced) stores report generation `0`.
    async fn read(&self) -> Result<Option<(u64, Vec<u8>)>>;

    /// Durably + atomically publish the snapshot blob stamped with `generation`.
    ///
    /// Returns `Ok(true)` when the write landed, `Ok(false)` when it was
    /// **fenced** — a newer writer (higher generation) already owns the blob, so
    /// this (stale) writer must step down rather than overwrite it. Local stores
    /// never fence and always return `Ok(true)`.
    async fn write_atomic(&self, generation: u64, bytes: &[u8]) -> Result<bool>;

    /// Human-readable location, for logs and error context.
    fn describe(&self) -> String;
}

/// Local-filesystem snapshot store: temp file → fsync → atomic rename →
/// best-effort parent-dir fsync. The default; behaviour-identical to the
/// pre-5c inline `write_atomic`.
pub struct LocalSnapshotStore {
    path: PathBuf,
}

impl LocalSnapshotStore {
    /// Snapshot blob at `path` (conventionally the WAL path with a `.snapshot`
    /// extension).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The blob path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl CatalogSnapshotStore for LocalSnapshotStore {
    async fn read(&self) -> Result<Option<(u64, Vec<u8>)>> {
        if tokio::fs::try_exists(&self.path).await.unwrap_or(false) {
            let bytes = tokio::fs::read(&self.path)
                .await
                .with_context(|| format!("reading catalog snapshot {}", self.path.display()))?;
            // Local single-pod store is unfenced — generation is always 0.
            Ok(Some((0, bytes)))
        } else {
            Ok(None)
        }
    }

    async fn write_atomic(&self, _generation: u64, bytes: &[u8]) -> Result<bool> {
        let path = &self.path;
        let tmp = path.with_extension("snapshot-tmp");
        {
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)
                .await
                .with_context(|| format!("opening snapshot temp {}", tmp.display()))?;
            file.write_all(bytes)
                .await
                .with_context(|| format!("writing snapshot temp {}", tmp.display()))?;
            file.flush().await?;
            file.sync_data()
                .await
                .with_context(|| format!("fsync snapshot temp {}", tmp.display()))?;
        }
        tokio::fs::rename(&tmp, path)
            .await
            .with_context(|| format!("atomically replacing snapshot {}", path.display()))?;
        if let Some(dir) = path.parent()
            && let Ok(handle) = std::fs::File::open(dir)
        {
            let _ = handle.sync_all();
        }
        Ok(true)
    }

    fn describe(&self) -> String {
        format!("file:{}", self.path.display())
    }
}

/// Object-store snapshot store with **generation-fencing** (Phase 6a).
///
/// Persists the catalog snapshot as a generation-fenced versioned manifest via
/// [`ManifestCommitter`] (the same `commit_fenced` primitive the warehouse
/// manifest log uses). A write carries the writer's monotonic generation; a
/// *stale* writer — one whose generation is below the generation a newer pod
/// already committed — is rejected (`Conflict`) instead of clobbering the newer
/// pod's snapshot. This is the multi-pod single-writer guarantee (the Neon
/// `index_part.json` + generation model). The per-DDL WAL stays local; only the
/// snapshot pointer is fenced here.
///
/// Works for `memory://` (deterministic tests) and, behind the
/// `proximadb-object-store` crate's cloud features, `s3://`/`gs://`/`az://`.
pub struct ObjectStoreSnapshotStore {
    committer: ManifestCommitter,
    label: String,
}

impl ObjectStoreSnapshotStore {
    /// Build from an object-store base URL (e.g. `s3://bucket/prefix`,
    /// `memory:///`) and the relative prefix that holds the catalog snapshot's
    /// fenced manifest log (e.g. `_operator/catalog/_manifests/`).
    pub fn from_url(base_url: &str, manifests_prefix: impl Into<String>) -> Result<Self> {
        let store = ProximaObjectStore::from_url(base_url)
            .with_context(|| format!("opening object store at {base_url}"))?;
        Ok(Self::new(store, base_url, manifests_prefix))
    }

    /// Build from an already-open store, its base URL (for labelling), and the
    /// fenced-manifest-log prefix. Used by tests and by callers that share one
    /// backing store across catalog opens.
    pub fn new(
        store: ProximaObjectStore,
        base_url: &str,
        manifests_prefix: impl Into<String>,
    ) -> Self {
        let manifests_prefix = manifests_prefix.into();
        let label = format!("{base_url}::{manifests_prefix}");
        Self {
            committer: ManifestCommitter::new(store, manifests_prefix),
            label,
        }
    }
}

#[async_trait]
impl CatalogSnapshotStore for ObjectStoreSnapshotStore {
    async fn read(&self) -> Result<Option<(u64, Vec<u8>)>> {
        match self
            .committer
            .latest_version()
            .await
            .with_context(|| format!("reading catalog snapshot log {}", self.label))?
        {
            Some(version) => {
                let (generation, payload) =
                    self.committer.read_fenced(version).await.with_context(|| {
                        format!("reading catalog snapshot {}@{version}", self.label)
                    })?;
                Ok(Some((generation, payload.to_vec())))
            }
            // No manifest yet → empty-catalog boot; the caller replays the WAL.
            None => Ok(None),
        }
    }

    async fn write_atomic(&self, generation: u64, bytes: &[u8]) -> Result<bool> {
        let parent = self
            .committer
            .latest_version()
            .await
            .with_context(|| format!("reading catalog snapshot head {}", self.label))?;
        match self
            .committer
            .commit_fenced(parent, generation, bytes::Bytes::copy_from_slice(bytes))
            .await
            .with_context(|| format!("committing catalog snapshot {}", self.label))?
        {
            CommitOutcome::Committed(_) => Ok(true),
            // Fenced (stale generation) or lost the version CAS race — either way
            // this writer did not publish; it must re-read and step down/retry.
            CommitOutcome::Conflict { .. } => Ok(false),
        }
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(opt: Option<(u64, Vec<u8>)>) -> Option<Vec<u8>> {
        opt.map(|(_gen, bytes)| bytes)
    }

    #[tokio::test]
    async fn local_store_round_trips_and_reports_absence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cat.snapshot");
        let store = LocalSnapshotStore::new(&path);

        // Absent before any write.
        assert!(store.read().await.unwrap().is_none());

        // Local store is unfenced: it ignores the generation and never fences.
        assert!(store.write_atomic(0, b"hello-catalog").await.unwrap());
        let (generation, bytes) = store.read().await.unwrap().unwrap();
        assert_eq!(generation, 0);
        assert_eq!(bytes, b"hello-catalog");

        // Atomic replace — even an "older" generation still writes locally.
        assert!(store.write_atomic(0, b"v2").await.unwrap());
        assert_eq!(
            body(store.read().await.unwrap()).as_deref(),
            Some(&b"v2"[..])
        );
    }

    #[tokio::test]
    async fn object_store_round_trips_and_reports_absence() {
        // `memory://` exercises the real ManifestCommitter commit/read path
        // deterministically (no cloud creds).
        let store =
            ObjectStoreSnapshotStore::from_url("memory:///", "_operator/catalog/_manifests/")
                .expect("memory store");

        assert!(store.read().await.unwrap().is_none());

        assert!(store.write_atomic(1, b"snap-bytes").await.unwrap());
        let (generation, bytes) = store.read().await.unwrap().unwrap();
        assert_eq!(generation, 1);
        assert_eq!(bytes, b"snap-bytes");

        // A same-or-higher generation succeeds and supersedes.
        assert!(store.write_atomic(1, b"snap-bytes-2").await.unwrap());
        assert_eq!(
            body(store.read().await.unwrap()).as_deref(),
            Some(&b"snap-bytes-2"[..])
        );
    }

    /// Phase 6a multi-instance fence: two stores over ONE shared backing object
    /// store (two pods). After a newer-generation writer takes over, the older
    /// (stale) writer's commit is fenced — it cannot clobber the newer snapshot.
    #[tokio::test]
    async fn fenced_commit_rejects_stale_writer() {
        use object_store::memory::InMemory;
        use std::sync::Arc;

        // One backing store shared by both "pods" (a fresh `from_url("memory://")`
        // would be a different, empty store — we need the same instance).
        let backing: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let prefix = "_operator/catalog/_manifests/";

        let pod_a = ObjectStoreSnapshotStore::new(
            ProximaObjectStore::new(backing.clone()),
            "memory:///",
            prefix,
        );
        let pod_b = ObjectStoreSnapshotStore::new(
            ProximaObjectStore::new(backing.clone()),
            "memory:///",
            prefix,
        );

        // Pod A owns generation 1 and commits.
        assert!(pod_a.write_atomic(1, b"from-A").await.unwrap());

        // Pod B takes over: it reads the current generation (1) and claims the
        // next one (2), then commits successfully (2 >= 1).
        let (seen, _) = pod_b.read().await.unwrap().unwrap();
        assert_eq!(seen, 1);
        assert!(pod_b.write_atomic(2, b"from-B").await.unwrap());

        // Pod A is now stale (generation 1 < 2). Its next commit is FENCED.
        assert!(
            !pod_a.write_atomic(1, b"from-A-again").await.unwrap(),
            "stale generation-1 writer must be fenced once generation 2 owns the log"
        );

        // The newer pod's snapshot is intact — A did not clobber it.
        let (generation, bytes) = pod_b.read().await.unwrap().unwrap();
        assert_eq!(generation, 2);
        assert_eq!(bytes, b"from-B");
    }
}
