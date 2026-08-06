//! Root-side [`CatalogFilesystemResolver`] impl.
//!
//! `CatalogManager` now lives in `proximadb-catalog` and resolves object-store
//! catalog URLs (s3://, gs://, az://) through the injected
//! [`CatalogFilesystemResolver`] port. This file is the root half of that
//! dependency inversion: a lazily-initialized wrapper around the root
//! [`FilesystemFactory`]. The factory is created on first use (never for
//! local-only setups), preserving the original lazy-create behavior, and stays
//! out of the catalog crate so there is no catalog→storage up-edge.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use proximadb_catalog::CatalogFilesystemResolver;
use proximadb_storage_filesystem_types::FileSystem;
use tokio::sync::OnceCell;

use crate::storage::persistence::filesystem::FilesystemFactory;

/// Lazily-initialized filesystem resolver backed by the root [`FilesystemFactory`].
///
/// The factory is constructed once, on the first object-store catalog URL
/// resolution; local-only deployments never construct it.
pub struct LazyFilesystemResolver {
    factory: OnceCell<FilesystemFactory>,
}

impl LazyFilesystemResolver {
    /// Create a resolver that will build the [`FilesystemFactory`] on first use.
    pub fn new() -> Self {
        Self {
            factory: OnceCell::new(),
        }
    }
}

impl Default for LazyFilesystemResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CatalogFilesystemResolver for LazyFilesystemResolver {
    async fn get_filesystem(&self, url: &str) -> Result<Arc<dyn FileSystem>> {
        let factory = self
            .factory
            .get_or_try_init(|| async { FilesystemFactory::create_default().await })
            .await?;
        factory.get_filesystem(url).map_err(anyhow::Error::from)
    }
}
