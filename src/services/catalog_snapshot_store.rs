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

use object_store::path::Path as ObjectPath;
use proximadb_kernel::error::StorageError;
use proximadb_object_store::ProximaObjectStore;

/// Durable, atomically-replaceable home for the catalog snapshot blob.
///
/// `write_atomic` must guarantee a reader concurrent with the write observes
/// either the previous blob or the complete new one — never a torn write. This
/// is the load-bearing property the Phase-3 crash-consistency ordering relies
/// on (snapshot made durable *before* the WAL is compacted).
#[async_trait]
pub trait CatalogSnapshotStore: Send + Sync {
    /// Read the current snapshot blob, or `None` if none has been written yet.
    async fn read(&self) -> Result<Option<Vec<u8>>>;

    /// Durably + atomically replace the snapshot blob.
    async fn write_atomic(&self, bytes: &[u8]) -> Result<()>;

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
    async fn read(&self) -> Result<Option<Vec<u8>>> {
        if tokio::fs::try_exists(&self.path).await.unwrap_or(false) {
            let bytes = tokio::fs::read(&self.path)
                .await
                .with_context(|| format!("reading catalog snapshot {}", self.path.display()))?;
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    }

    async fn write_atomic(&self, bytes: &[u8]) -> Result<()> {
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
        Ok(())
    }

    fn describe(&self) -> String {
        format!("file:{}", self.path.display())
    }
}

/// Object-store snapshot store: a single whole-object PUT/GET under `key`.
///
/// Object stores give atomic whole-object PUT semantics, so no temp/rename
/// dance is needed — a reader sees either the old object or the new one, never
/// a partial write. Works for `memory://` (deterministic tests) and, behind the
/// `proximadb-object-store` crate's cloud features, `s3://`/`gs://`/`az://`.
pub struct ObjectStoreSnapshotStore {
    store: ProximaObjectStore,
    key: ObjectPath,
    label: String,
}

impl ObjectStoreSnapshotStore {
    /// Build from an object-store base URL (e.g. `s3://bucket/prefix`,
    /// `memory:///`) and the snapshot object's relative key under it (e.g.
    /// `_operator/catalog/system-catalog.snapshot`).
    pub fn from_url(base_url: &str, key: impl Into<String>) -> Result<Self> {
        let store = ProximaObjectStore::from_url(base_url)
            .with_context(|| format!("opening object store at {base_url}"))?;
        Ok(Self::new(store, base_url, key))
    }

    /// Build from an already-open store, its base URL (for labelling), and the
    /// snapshot object's relative key. Used by tests and by callers that share
    /// one store across catalog opens.
    pub fn new(store: ProximaObjectStore, base_url: &str, key: impl Into<String>) -> Self {
        let key = key.into();
        let label = format!("{base_url}::{key}");
        Self {
            store,
            key: ObjectPath::from(key),
            label,
        }
    }
}

#[async_trait]
impl CatalogSnapshotStore for ObjectStoreSnapshotStore {
    async fn read(&self) -> Result<Option<Vec<u8>>> {
        match self.store.get(&self.key).await {
            Ok(bytes) => Ok(Some(bytes.to_vec())),
            // A not-yet-written snapshot is the empty-catalog boot case, not an
            // error — the caller then replays the WAL from empty.
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading catalog snapshot {}", self.label)),
        }
    }

    async fn write_atomic(&self, bytes: &[u8]) -> Result<()> {
        self.store
            .put(&self.key, bytes::Bytes::copy_from_slice(bytes))
            .await
            .with_context(|| format!("writing catalog snapshot {}", self.label))?;
        Ok(())
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_store_round_trips_and_reports_absence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cat.snapshot");
        let store = LocalSnapshotStore::new(&path);

        // Absent before any write.
        assert!(store.read().await.unwrap().is_none());

        store.write_atomic(b"hello-catalog").await.unwrap();
        assert_eq!(
            store.read().await.unwrap().as_deref(),
            Some(&b"hello-catalog"[..])
        );

        // Atomic replace.
        store.write_atomic(b"v2").await.unwrap();
        assert_eq!(store.read().await.unwrap().as_deref(), Some(&b"v2"[..]));
    }

    #[tokio::test]
    async fn object_store_round_trips_and_reports_absence() {
        // `memory://` exercises the real ProximaObjectStore PUT/GET path
        // deterministically (no cloud creds).
        let store = ObjectStoreSnapshotStore::from_url(
            "memory:///",
            "_operator/catalog/system-catalog.snapshot",
        )
        .expect("memory store");

        assert!(store.read().await.unwrap().is_none());

        store.write_atomic(b"snap-bytes").await.unwrap();
        assert_eq!(
            store.read().await.unwrap().as_deref(),
            Some(&b"snap-bytes"[..])
        );

        store.write_atomic(b"snap-bytes-2").await.unwrap();
        assert_eq!(
            store.read().await.unwrap().as_deref(),
            Some(&b"snap-bytes-2"[..])
        );
    }
}
