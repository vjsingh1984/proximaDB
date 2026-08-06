//! Narrow filesystem abstraction the queue uses for its disk + object tiers.
//!
//! We deliberately do NOT depend on ProximaDB's `FilesystemFactory` here -
//! that lives in the main `proximadb` crate which depends on us, so taking
//! a hard dep back would be circular. Instead the queue defines this small
//! trait, ships a `LocalFs` (tokio::fs) impl, and lets the main crate
//! provide an adapter that wraps `FilesystemFactory`'s output for
//! production deployments (object stores etc.) when Phase 2D lands.
//!
//! Methods are async (the disk tier runs on tokio); paths are `&Path` so
//! callers can choose `PathBuf` ownership freely.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::QueueError;

pub type Result<T> = std::result::Result<T, QueueError>;

/// Operations the queue needs from a backing filesystem.
#[async_trait]
pub trait QueueFs: Send + Sync + std::fmt::Debug {
    async fn create_dir_all(&self, path: &Path) -> Result<()>;

    /// Append bytes to the file, creating it if absent. NOT durable on
    /// its own - call `fsync` to durably persist.
    async fn append(&self, path: &Path, data: &[u8]) -> Result<()>;

    /// Force the file's bytes (and its containing directory entry) to
    /// stable storage. On `LocalFs` this is `tokio::fs::File::sync_all`.
    /// On object stores, this is a no-op (durability is at write time).
    async fn fsync(&self, path: &Path) -> Result<()>;

    async fn read(&self, path: &Path) -> Result<Vec<u8>>;

    /// List files in `dir`. Returns absolute paths. Order is unspecified.
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>>;

    /// Rename `from` to `to`. On local FS this is atomic when both paths
    /// are on the same mount.
    async fn rename(&self, from: &Path, to: &Path) -> Result<()>;

    async fn delete(&self, path: &Path) -> Result<()>;

    async fn metadata(&self, path: &Path) -> Result<Metadata>;
}

#[derive(Debug, Clone, Copy)]
pub struct Metadata {
    pub size_bytes: u64,
}

/// Real-filesystem implementation. Production default.
#[derive(Debug)]
pub struct LocalFs;

impl LocalFs {
    pub fn new_arc() -> Arc<dyn QueueFs> {
        Arc::new(Self)
    }
}

#[async_trait]
impl QueueFs for LocalFs {
    async fn create_dir_all(&self, path: &Path) -> Result<()> {
        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| QueueError::Persistence(format!("create_dir_all {path:?}: {e}")))
    }

    async fn append(&self, path: &Path, data: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|e| QueueError::Persistence(format!("open {path:?}: {e}")))?;
        f.write_all(data)
            .await
            .map_err(|e| QueueError::Persistence(format!("write {path:?}: {e}")))?;
        Ok(())
    }

    async fn fsync(&self, path: &Path) -> Result<()> {
        let f = tokio::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .await
            .map_err(|e| QueueError::Persistence(format!("open-for-fsync {path:?}: {e}")))?;
        f.sync_all()
            .await
            .map_err(|e| QueueError::Persistence(format!("fsync {path:?}: {e}")))?;
        Ok(())
    }

    async fn read(&self, path: &Path) -> Result<Vec<u8>> {
        tokio::fs::read(path)
            .await
            .map_err(|e| QueueError::Persistence(format!("read {path:?}: {e}")))
    }

    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let mut entries = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| QueueError::Persistence(format!("read_dir {dir:?}: {e}")))?;
        let mut out = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| QueueError::Persistence(format!("read_dir-next {dir:?}: {e}")))?
        {
            out.push(entry.path());
        }
        Ok(out)
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        tokio::fs::rename(from, to)
            .await
            .map_err(|e| QueueError::Persistence(format!("rename {from:?} -> {to:?}: {e}")))
    }

    async fn delete(&self, path: &Path) -> Result<()> {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| QueueError::Persistence(format!("delete {path:?}: {e}")))
    }

    async fn metadata(&self, path: &Path) -> Result<Metadata> {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| QueueError::Persistence(format!("metadata {path:?}: {e}")))?;
        Ok(Metadata {
            size_bytes: meta.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_fs_round_trips_append_read_metadata_list_rename_and_delete() {
        let fs = LocalFs;
        let dir = tempfile::tempdir().unwrap();
        let partition_dir = dir.path().join("topic").join("0");
        let segment = partition_dir.join("0000000000.qseg");

        fs.create_dir_all(&partition_dir).await.unwrap();
        fs.append(&segment, b"hello").await.unwrap();
        fs.append(&segment, b" world").await.unwrap();
        fs.fsync(&segment).await.unwrap();

        assert_eq!(fs.read(&segment).await.unwrap(), b"hello world");
        assert_eq!(fs.metadata(&segment).await.unwrap().size_bytes, 11);

        let listed = fs.list(&partition_dir).await.unwrap();
        assert!(listed.iter().any(|path| path == &segment));

        let renamed = partition_dir.join("0000000001.qseg");
        fs.rename(&segment, &renamed).await.unwrap();
        assert_eq!(fs.read(&renamed).await.unwrap(), b"hello world");

        fs.delete(&renamed).await.unwrap();
        let error = fs.metadata(&renamed).await.unwrap_err();
        assert!(error.to_string().contains("metadata"));
    }

    #[tokio::test]
    async fn local_fs_surfaces_context_on_missing_paths() {
        let fs = LocalFs;
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.qseg");

        let error = fs.read(&missing).await.unwrap_err();

        assert!(error.to_string().contains("read"));
        assert!(error.to_string().contains("missing.qseg"));
    }
}
