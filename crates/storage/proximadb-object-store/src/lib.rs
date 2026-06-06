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
use object_store::{ObjectMeta, ObjectStore, PutMode, PutOptions};
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
/// `file://` and `memory://` always work. `s3://`/`gs://`/`az://` (incl. `adls://`/`abfs://`)
/// require the matching crate feature (`aws`/`gcp`/`azure`); without it the underlying
/// `object_store` builder returns an error (no panic).
///
/// Credentials come from the standard `object_store` environment variables. We forward the
/// process environment (lower-cased so the upper-case `AZURE_*` / `AWS_*` / `GOOGLE_*` names
/// the deployment sets match `object_store`'s lower-case config keys) into `parse_url_opts`;
/// `object_store` applies the keys it recognises for the URL's scheme and silently ignores the
/// rest. This is what makes **secret-less cloud auth** work here: e.g. the AKS workload-identity
/// trio `AZURE_CLIENT_ID` / `AZURE_TENANT_ID` / `AZURE_FEDERATED_TOKEN_FILE`, or AWS web-identity
/// (`AWS_ROLE_ARN` / `AWS_WEB_IDENTITY_TOKEN_FILE`), authenticate ADLS/S3 with no static key —
/// matching the `FileSystem` Azure backend's posture. `file://`/`memory://` ignore the options.
pub fn store_for_url(url: &str) -> Result<(Arc<dyn ObjectStore>, Path), StorageError> {
    let parsed = Url::parse(url).map_err(|e| url_err(url, e))?;
    let env_opts = std::env::vars().map(|(k, v)| (k.to_ascii_lowercase(), v));
    let (store, path) = object_store::parse_url_opts(&parsed, env_opts)
        .map_err(|e| os_err(&format!("parse_url({url})"), e))?;
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
    pub fn full_path(&self, path: &Path) -> Path {
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
            .put(&self.full_path(path), bytes.into())
            .await
            .map(|_| ())
            .map_err(|e| os_err("put", e))
    }

    /// Atomically write `bytes` to `path` ONLY if no object already exists there
    /// (`PutMode::Create`). Returns [`StorageError::AlreadyExists`] if the object
    /// is already present, and never overwrites it.
    ///
    /// This is the optimistic-concurrency primitive for Iceberg-style manifest
    /// commits (the warehouse base tier): a committer writes a new
    /// manifest/metadata object under a fresh name with create-only semantics,
    /// so two concurrent committers cannot clobber each other's commit — the
    /// loser gets `AlreadyExists` and retries against the winner's snapshot.
    /// (Supported by the `memory` and local-file backends; cloud backends need
    /// conditional-put support.)
    pub async fn put_if_absent(&self, path: &Path, bytes: Bytes) -> Result<(), StorageError> {
        self.store
            .put_opts(
                &self.full_path(path),
                bytes.into(),
                PutOptions::from(PutMode::Create),
            )
            .await
            .map(|_| ())
            .map_err(|e| os_err("put_if_absent", e))
    }

    /// Read the whole object at `path`.
    pub async fn get(&self, path: &Path) -> Result<Bytes, StorageError> {
        let result = self
            .store
            .get(&self.full_path(path))
            .await
            .map_err(|e| os_err("get", e))?;
        result.bytes().await.map_err(|e| os_err("get(bytes)", e))
    }

    /// Read a byte range of the object at `path` (the warehouse footer/row-group read path).
    pub async fn get_range(&self, path: &Path, range: Range<u64>) -> Result<Bytes, StorageError> {
        self.store
            .get_range(&self.full_path(path), range)
            .await
            .map_err(|e| os_err("get_range", e))
    }

    /// Fetch object metadata (size, last-modified, e-tag) for `path` WITHOUT
    /// reading the body.
    ///
    /// This is the prerequisite for the footer/row-group range-read path
    /// ([`get_range`]): a Parquet reader must know the object length to compute
    /// the trailing footer's byte range before it can range-read it. `head` is a
    /// metadata-only request (cheap on cloud stores — an HTTP HEAD), so callers
    /// avoid a full-object GET just to learn the size.
    pub async fn head(&self, path: &Path) -> Result<ObjectMeta, StorageError> {
        self.store
            .head(&self.full_path(path))
            .await
            .map_err(|e| os_err("head", e))
    }

    /// The byte length of the object at `path` (metadata-only; see [`head`]).
    /// Returns the value that bounds a `get_range` over the whole object.
    pub async fn object_size(&self, path: &Path) -> Result<u64, StorageError> {
        Ok(self.head(path).await?.size)
    }

    /// Read the last `n` bytes of the object at `path` — the Parquet-footer read
    /// pattern (a reader fetches the trailing bytes to locate the footer length,
    /// then the footer itself).
    ///
    /// Implemented as a metadata [`head`] (to learn the size) followed by a
    /// bounded [`get_range`]. `n` is clamped to the object size, so requesting
    /// more bytes than the object holds returns the whole object; `n == 0`
    /// returns empty without any request. (A future optimization could use the
    /// object store's native suffix range to save the `head` round-trip.)
    pub async fn get_suffix(&self, path: &Path, n: u64) -> Result<Bytes, StorageError> {
        if n == 0 {
            return Ok(Bytes::new());
        }
        let size = self.object_size(path).await?;
        if size == 0 {
            return Ok(Bytes::new());
        }
        let start = size.saturating_sub(n);
        self.get_range(path, start..size).await
    }

    /// List objects under an optional caller-relative prefix. A `None` prefix lists under
    /// the base (NOT the whole store/filesystem root — `file://` parses to root `/`).
    pub async fn list(&self, prefix: Option<&Path>) -> Result<Vec<ObjectMeta>, StorageError> {
        let resolved = match prefix {
            Some(p) => self.full_path(p),
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
            .delete(&self.full_path(path))
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

    /// `head`/`object_size` report the object length without a body GET, and
    /// that length is exactly what bounds a trailing-footer `get_range` — the
    /// warehouse Parquet-footer read pattern. A missing object errors (no panic).
    #[tokio::test]
    async fn head_reports_size_and_enables_footer_range_read() {
        let os = ProximaObjectStore::new(Arc::new(object_store::memory::InMemory::new()));
        let p = Path::from("t/data.parquet");
        os.put(&p, Bytes::from_static(b"0123456789")).await.unwrap();

        let meta = os.head(&p).await.unwrap();
        assert_eq!(meta.size, 10, "head reports the exact byte length");
        assert_eq!(os.object_size(&p).await.unwrap(), 10);

        // The size drives a "last 4 bytes" footer-style range read.
        let n = os.object_size(&p).await.unwrap();
        let footer = os.get_range(&p, (n - 4)..n).await.unwrap();
        assert_eq!(&footer[..], b"6789");

        // Metadata on a missing object surfaces an error, not a panic.
        assert!(os.head(&Path::from("missing.parquet")).await.is_err());
    }

    /// `get_suffix` reads the trailing N bytes (the Parquet-footer pattern),
    /// clamps N to the object size, treats N==0 as empty, and errors on a
    /// missing object.
    #[tokio::test]
    async fn get_suffix_reads_trailing_bytes() {
        let os = ProximaObjectStore::new(Arc::new(object_store::memory::InMemory::new()));
        let p = Path::from("t/data.parquet");
        os.put(&p, Bytes::from_static(b"0123456789")).await.unwrap(); // 10 bytes

        assert_eq!(&os.get_suffix(&p, 4).await.unwrap()[..], b"6789"); // last 4
        assert_eq!(
            &os.get_suffix(&p, 100).await.unwrap()[..],
            b"0123456789",
            "n >= size returns the whole object"
        );
        assert!(
            os.get_suffix(&p, 0).await.unwrap().is_empty(),
            "n == 0 returns empty"
        );
        assert!(
            os.get_suffix(&Path::from("missing.parquet"), 4).await.is_err(),
            "missing object errors"
        );
    }

    /// `put_if_absent` creates a new object but rejects (and does not overwrite)
    /// an existing one — the Iceberg-style commit-atomicity primitive.
    #[tokio::test]
    async fn put_if_absent_is_create_only() {
        let os = ProximaObjectStore::new(Arc::new(object_store::memory::InMemory::new()));
        let p = Path::from("manifest/v1.json");

        // First create succeeds.
        os.put_if_absent(&p, Bytes::from_static(b"first"))
            .await
            .unwrap();
        assert_eq!(&os.get(&p).await.unwrap()[..], b"first");

        // Second create is rejected and leaves the existing object untouched.
        let err = os.put_if_absent(&p, Bytes::from_static(b"second")).await;
        assert!(
            matches!(err, Err(StorageError::AlreadyExists(_))),
            "create-only must reject an existing key, got {err:?}"
        );
        assert_eq!(
            &os.get(&p).await.unwrap()[..],
            b"first",
            "rejected create must not overwrite"
        );
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
