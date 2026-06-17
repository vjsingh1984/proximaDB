/*
 * Copyright 2026 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Bridge between the queue's narrow `QueueFs` trait and the main
//! crate's `FilesystemFactory`. Lives here (not in `proximadb-queue`)
//! because the factory pulls in cache + transaction orchestration that
//! the queue can't depend on without going circular.
//!
//! Wiring:
//!   1. `database.rs` already constructs a `FilesystemFactory` for the
//!      storage engine layer (it knows how to resolve `file://`,
//!      `adls://`, `s3://`, `gcs://`, `hdfs://`).
//!   2. At queue-init time, `database.rs` constructs a
//!      [`FactoryQueueFs`] anchored at the queue root URL.
//!   3. `QueueClient::open_with_fs(config, Some(adapter))` injects it.
//!   4. Queue's disk tier + object_tier uploader call through the
//!      adapter, which translates `&Path` → URL by joining with the
//!      configured root URL prefix.
//!
//! ## Path-to-URL translation
//!
//! The queue uses `PathBuf` internally (inherited from its
//! `LocalFs`-only origins). The adapter holds a `root_url: String`;
//! incoming paths are interpreted as RELATIVE to that root and joined
//! with `/` separators. So if
//! `root_url = "adls://acct.dfs.core.windows.net/queue"` and the queue
//! asks for `path = "embed-ingest/0/0000000000.qseg"`, the adapter calls
//! `factory.get_filesystem("adls://.../queue/embed-ingest/0/0000000000.qseg")`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use proximadb_queue::error::QueueError;
use proximadb_queue::fs::{Metadata, QueueFs, Result as QueueResult};

use crate::storage::persistence::filesystem::FilesystemFactory;

/// `QueueFs` impl backed by `FilesystemFactory`. Resolves URLs for
/// any scheme the factory knows (`file`, `adls`, `s3`, `gcs`, `hdfs`).
#[derive(Debug)]
pub struct FactoryQueueFs {
    factory: Arc<FilesystemFactory>,
    /// Root URL the queue's relative paths are joined under. Includes
    /// scheme + authority + base path, no trailing slash.
    root_url: String,
}

impl FactoryQueueFs {
    // Returns `Arc<dyn QueueFs>` directly because every caller stores the
    // adapter behind a trait object; exposing the concrete type would force
    // every call site to add a redundant `.as_queue_fs()` cast.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(factory: Arc<FilesystemFactory>, root_url: impl Into<String>) -> Arc<dyn QueueFs> {
        let mut root_url: String = root_url.into();
        while root_url.ends_with('/') {
            root_url.pop();
        }
        Arc::new(Self { factory, root_url })
    }

    /// Translate a queue-supplied `&Path` to a URL the factory accepts.
    fn url_for(&self, path: &Path) -> String {
        let s = path.to_string_lossy();
        let suffix = s.trim_start_matches('/');
        if suffix.is_empty() {
            self.root_url.clone()
        } else {
            format!("{}/{}", self.root_url, suffix)
        }
    }

    fn map_err(e: impl std::fmt::Display) -> QueueError {
        QueueError::Persistence(e.to_string())
    }
}

#[async_trait]
impl QueueFs for FactoryQueueFs {
    async fn create_dir_all(&self, path: &Path) -> QueueResult<()> {
        let url = self.url_for(path);
        let fs = self.factory.get_filesystem(&url).map_err(Self::map_err)?;
        fs.create_dir_all(&url).await.map_err(Self::map_err)
    }

    async fn append(&self, path: &Path, data: &[u8]) -> QueueResult<()> {
        let url = self.url_for(path);
        let fs = self.factory.get_filesystem(&url).map_err(Self::map_err)?;
        fs.append(&url, data).await.map_err(Self::map_err)
    }

    async fn fsync(&self, path: &Path) -> QueueResult<()> {
        let url = self.url_for(path);
        let fs = self.factory.get_filesystem(&url).map_err(Self::map_err)?;
        // Object-store backends no-op sync_file (PUTs are durable on
        // success); local filesystem invokes File::sync_all.
        fs.sync_file(&url).await.map_err(Self::map_err)
    }

    async fn read(&self, path: &Path) -> QueueResult<Vec<u8>> {
        let url = self.url_for(path);
        let fs = self.factory.get_filesystem(&url).map_err(Self::map_err)?;
        fs.read(&url).await.map_err(Self::map_err)
    }

    async fn list(&self, dir: &Path) -> QueueResult<Vec<PathBuf>> {
        let url = self.url_for(dir);
        let fs = self.factory.get_filesystem(&url).map_err(Self::map_err)?;
        let entries = fs.list(&url).await.map_err(Self::map_err)?;
        // Strip the root URL prefix so returned PathBufs look like
        // relative paths to the queue (which then re-joins under its
        // own PathBuf-flavored walkers).
        let prefix = format!("{}/", self.root_url);
        Ok(entries
            .into_iter()
            .map(|e| {
                if let Some(rest) = e.url.strip_prefix(&prefix) {
                    PathBuf::from(rest)
                } else {
                    PathBuf::from(e.url)
                }
            })
            .collect())
    }

    async fn rename(&self, from: &Path, to: &Path) -> QueueResult<()> {
        let from_url = self.url_for(from);
        let to_url = self.url_for(to);
        let fs = self
            .factory
            .get_filesystem(&from_url)
            .map_err(Self::map_err)?;
        fs.move_file(&from_url, &to_url)
            .await
            .map_err(Self::map_err)
    }

    async fn delete(&self, path: &Path) -> QueueResult<()> {
        let url = self.url_for(path);
        let fs = self.factory.get_filesystem(&url).map_err(Self::map_err)?;
        fs.delete(&url).await.map_err(Self::map_err)
    }

    async fn metadata(&self, path: &Path) -> QueueResult<Metadata> {
        let url = self.url_for(path);
        let fs = self.factory.get_filesystem(&url).map_err(Self::map_err)?;
        let m = fs.metadata(&url).await.map_err(Self::map_err)?;
        Ok(Metadata { size_bytes: m.size })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `url_for` joins relative paths under the root with exactly one
    /// slash, regardless of leading slashes on the input.
    #[test]
    fn url_for_joins_relative_path_with_single_slash() {
        struct Probe(String);
        impl Probe {
            fn url_for(&self, path: &Path) -> String {
                let s = path.to_string_lossy();
                let suffix = s.trim_start_matches('/');
                if suffix.is_empty() {
                    self.0.clone()
                } else {
                    format!("{}/{}", self.0, suffix)
                }
            }
        }
        let p = Probe("adls://acct.dfs.core.windows.net/queue".to_string());
        assert_eq!(
            p.url_for(Path::new("embed-ingest/0/0000000000.qseg")),
            "adls://acct.dfs.core.windows.net/queue/embed-ingest/0/0000000000.qseg"
        );
        assert_eq!(
            p.url_for(Path::new("/embed-ingest/0/0000000000.qseg")),
            "adls://acct.dfs.core.windows.net/queue/embed-ingest/0/0000000000.qseg",
            "leading slash in relative path is stripped"
        );
        assert_eq!(p.url_for(Path::new("")), p.0);
    }

    /// Trailing slashes on root_url are normalized away.
    #[test]
    fn root_url_normalization_strips_trailing_slashes() {
        fn normalize(mut s: String) -> String {
            while s.ends_with('/') {
                s.pop();
            }
            s
        }
        assert_eq!(
            normalize("adls://acct.dfs.core.windows.net/queue/".to_string()),
            "adls://acct.dfs.core.windows.net/queue"
        );
        assert_eq!(
            normalize("s3://bucket/queue///".to_string()),
            "s3://bucket/queue"
        );
    }
}
