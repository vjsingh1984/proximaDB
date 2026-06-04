//! # proximadb-object-store — decoupled object-storage plumbing (F1)
//!
//! The bottom layer of the ProximaDB warehouse base tier (course-correction
//! `DATA_WAREHOUSE_AND_ENGINEERING_COURSE_CORRECTION_2026_06_04.adoc` §6 P3): a thin,
//! write-capable wrapper over the `object_store` crate.
//!
//! - [`store_for_url`] maps a URL (`file://`, `memory://`; `s3://`/`gs://`/`az://` behind
//!   the `aws`/`gcp`/`azure` features) to an `Arc<dyn object_store::ObjectStore>` + the base
//!   `Path`, reusing `object_store::parse_url` (the canonical scheme dispatch) — it does NOT
//!   fork a fourth storage abstraction.
//! - [`ProximaObjectStore`] bundles a store + base prefix with `put`/`get`/`get_range`/
//!   `list`/`delete` helpers. Cloud writes are the gap this fills: the existing `FileSystem`
//!   cloud backends are read-only.
//!
//! Plumbing ONLY. Parquet/Iceberg encoding and the concrete `ObjectStoreBridge`
//! implementation live in `proximadb-iceberg-engine` (F2), built on top of this. The
//! `object_store` version is pinned via the workspace dep so the `Arc<dyn ObjectStore>` here
//! is the exact type `proximadb-storage-common`'s `ObjectStoreBridge::inner_store()` expects.

use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use object_store::path::Path;
use object_store::{ObjectMeta, ObjectStore};
use proximadb_kernel::error::StorageError;
use url::Url;

/// Map an `object_store`/url failure to the canonical `StorageError` (the same error the
/// `ObjectStoreBridge` seam returns), preserving NotFound/AlreadyExists where possible.
fn os_err(context: &str, e: object_store::Error) -> StorageError {
    match &e {
        object_store::Error::NotFound { .. } => StorageError::NotFound(format!("{context}: {e}")),
        object_store::Error::AlreadyExists { .. } => {
            StorageError::AlreadyExists(format!("{context}: {e}"))
        }
        _ => StorageError::DiskIO(std::io::Error::other(format!(
            "object_store {context}: {e}"
        ))),
    }
}

fn url_err(url: &str, e: impl std::fmt::Display) -> StorageError {
    StorageError::DiskIO(std::io::Error::other(format!(
        "object-store url `{url}`: {e}"
    )))
}

/// Build an object store backend + base `Path` from a URL.
///
/// `file://` and `memory://` always work. `s3://`/`gs://`/`az://` require the matching
/// crate feature (`aws`/`gcp`/`azure`); without it `object_store::parse_url` returns an
/// error (no panic), and credentials come from the standard `object_store` environment
/// variables (aligned with `FilesystemFactory`'s env conventions).
pub fn store_for_url(url: &str) -> Result<(Arc<dyn ObjectStore>, Path), StorageError> {
    let parsed = Url::parse(url).map_err(|e| url_err(url, e))?;
    let (store, path) =
        object_store::parse_url(&parsed).map_err(|e| os_err(&format!("parse_url({url})"), e))?;
    Ok((Arc::from(store), path))
}

/// A store handle bundled with a base prefix; all helper paths are taken relative to it.
#[derive(Clone)]
pub struct ProximaObjectStore {
    store: Arc<dyn ObjectStore>,
    base: Path,
}

impl ProximaObjectStore {
    /// Open a store from a URL (see [`store_for_url`]).
    pub fn from_url(url: &str) -> Result<Self, StorageError> {
        let (store, base) = store_for_url(url)?;
        Ok(Self { store, base })
    }

    /// Wrap an existing `object_store` handle (base = root). Useful when the store is built
    /// elsewhere (e.g. a shared `Arc<dyn ObjectStore>` from the bridge).
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self {
            store,
            base: Path::default(),
        }
    }

    /// The underlying object store (for `ObjectStoreBridge::inner_store()` and direct use).
    pub fn store(&self) -> Arc<dyn ObjectStore> {
        self.store.clone()
    }

    /// The base prefix this handle resolves relative paths against.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Join the base prefix with a caller-relative path.
    fn full(&self, path: &Path) -> Path {
        let base = self.base.as_ref();
        if base.is_empty() {
            path.clone()
        } else {
            Path::from(format!("{base}/{path}"))
        }
    }

    /// Write `bytes` to `path` (atomic for stores that support it). Overwrites.
    pub async fn put(&self, path: &Path, bytes: Bytes) -> Result<(), StorageError> {
        self.store
            .put(&self.full(path), bytes.into())
            .await
            .map(|_| ())
            .map_err(|e| os_err("put", e))
    }

    /// Read the whole object at `path`.
    pub async fn get(&self, path: &Path) -> Result<Bytes, StorageError> {
        let result = self
            .store
            .get(&self.full(path))
            .await
            .map_err(|e| os_err("get", e))?;
        result.bytes().await.map_err(|e| os_err("get(bytes)", e))
    }

    /// Read a byte range of the object at `path` (the warehouse footer/row-group read path).
    pub async fn get_range(&self, path: &Path, range: Range<u64>) -> Result<Bytes, StorageError> {
        self.store
            .get_range(&self.full(path), range)
            .await
            .map_err(|e| os_err("get_range", e))
    }

    /// List objects under an optional caller-relative prefix. A `None` prefix lists under
    /// the base (NOT the whole store/filesystem root — `file://` parses to root `/`).
    pub async fn list(&self, prefix: Option<&Path>) -> Result<Vec<ObjectMeta>, StorageError> {
        let resolved = match prefix {
            Some(p) => self.full(p),
            None => self.base.clone(),
        };
        let mut stream = self.store.list(Some(&resolved));
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            out.push(item.map_err(|e| os_err("list", e))?);
        }
        Ok(out)
    }

    /// Delete the object at `path`.
    pub async fn delete(&self, path: &Path) -> Result<(), StorageError> {
        self.store
            .delete(&self.full(path))
            .await
            .map_err(|e| os_err("delete", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_store_roundtrip() {
        let store: Arc<dyn ObjectStore> = Arc::new(object_store::memory::InMemory::new());
        let os = ProximaObjectStore::new(store);
        let p = Path::from("a/b.bin");

        os.put(&p, Bytes::from_static(b"hello world"))
            .await
            .unwrap();
        assert_eq!(&os.get(&p).await.unwrap()[..], b"hello world");
        assert_eq!(&os.get_range(&p, 0..5).await.unwrap()[..], b"hello");
        assert_eq!(os.list(None).await.unwrap().len(), 1);

        os.delete(&p).await.unwrap();
        assert!(os.get(&p).await.is_err());
    }

    #[tokio::test]
    async fn file_url_roundtrip_under_base_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("file://{}", dir.path().display());
        let os = ProximaObjectStore::from_url(&url).unwrap();
        let p = Path::from("sub/x.parquet");

        os.put(&p, Bytes::from_static(b"parquet-bytes"))
            .await
            .unwrap();
        assert_eq!(&os.get(&p).await.unwrap()[..], b"parquet-bytes");
        assert_eq!(&os.get_range(&p, 0..7).await.unwrap()[..], b"parquet");
        assert_eq!(os.list(None).await.unwrap().len(), 1);
        // The write landed under the URL's directory (base prefix honored).
        assert!(dir.path().join("sub/x.parquet").exists());
    }

    #[test]
    fn memory_scheme_dispatches() {
        assert!(store_for_url("memory:///").is_ok());
    }

    #[test]
    fn cloud_scheme_without_feature_errors_not_panics() {
        // s3:// needs the `aws` feature; the default build must error gracefully.
        let result = store_for_url("s3://bucket/key");
        #[cfg(not(feature = "aws"))]
        assert!(result.is_err());
        let _ = result;
    }
}
