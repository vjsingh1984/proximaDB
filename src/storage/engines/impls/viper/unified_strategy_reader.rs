//! Unified VIPER Reader with strategy-aware caching
//!
//! VIPER (Vector Infrastructure for Production-Efficient Retrieval) implements
//! production-ready columnar storage with Parquet optimization.

use anyhow::Result;
use std::sync::Arc;

use crate::core::VectorRecord;
use crate::storage::engines::core::read_strategy::{ReadAccessStrategy, StrategyAwareReader};
use crate::storage::persistence::filesystem::{FilesystemFactory, FileSystem};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;

/// Unified VIPER reader that implements strategy-aware reading
pub struct UnifiedVIPERReader {
    filesystem_factory: Arc<FilesystemFactory>,
    cached_filesystem: Option<Arc<UnifiedCachingFilesystem>>,
    strategy: ReadAccessStrategy,
    collection_id: String,
}

impl UnifiedVIPERReader {
    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
        strategy: ReadAccessStrategy,
    ) -> Result<Self> {
        let cached_filesystem = if strategy.should_use_cache() {
            let base_fs = filesystem_factory.get_filesystem("file://")?;
            Some(Arc::new(UnifiedCachingFilesystem::new(
                base_fs,
                collection_id.clone(),
                "viper".to_string(),
            )))
        } else {
            None
        };

        Ok(Self {
            filesystem_factory,
            cached_filesystem,
            strategy,
            collection_id,
        })
    }

    /// Create a reader optimized for compaction (direct Parquet streaming)
    pub fn for_compaction(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
    ) -> Result<Self> {
        Self::new(filesystem_factory, collection_id, ReadAccessStrategy::DirectStream)
    }

    /// Create a reader optimized for search (cached Parquet metadata)
    pub fn for_search(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
    ) -> Result<Self> {
        Self::new(
            filesystem_factory,
            collection_id,
            ReadAccessStrategy::CachedSearch { prefetch_metadata: true },
        )
    }

    /// Read using Parquet-optimized access
    pub async fn read_parquet(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        match &self.strategy {
            ReadAccessStrategy::DirectStream => {
                // Direct Parquet read for compaction
                let fs = self.filesystem_factory.get_filesystem("file://")?;
                let _data = fs.read(file_path).await?;
                // TODO: Use arrow-rs for direct streaming
                Ok(vec![])
            }
            _ => {
                // Cached Parquet read with footer caching
                let cached_fs = self.cached_filesystem.as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Cached filesystem not initialized"))?;
                let _data = cached_fs.read(file_path).await?;
                // TODO: Use cached Parquet metadata
                Ok(vec![])
            }
        }
    }
}

impl StrategyAwareReader for UnifiedVIPERReader {
    fn strategy(&self) -> &ReadAccessStrategy {
        &self.strategy
    }

    fn set_strategy(&mut self, strategy: ReadAccessStrategy) {
        self.strategy = strategy;
        // Update cached filesystem if needed
        if self.strategy.should_use_cache() && self.cached_filesystem.is_none() {
            if let Ok(base_fs) = self.filesystem_factory.get_filesystem("file://") {
                self.cached_filesystem = Some(Arc::new(UnifiedCachingFilesystem::new(
                    base_fs,
                    self.collection_id.clone(),
                    "viper".to_string(),
                )));
            }
        }
    }
}

/// Direct VIPER Reader (streams Parquet without caching)
pub struct DirectVIPERReader {
    filesystem_factory: Arc<FilesystemFactory>,
}

impl DirectVIPERReader {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>) -> Self {
        Self { filesystem_factory }
    }

    pub async fn stream_parquet_direct(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        let fs = self.filesystem_factory.get_filesystem("file://")?;
        let _data = fs.read(file_path).await?;
        // TODO: Direct Parquet streaming
        Ok(vec![])
    }
}

/// Cached VIPER Reader (caches Parquet footers and metadata)
pub struct CachedVIPERReader {
    cached_filesystem: Arc<UnifiedCachingFilesystem>,
}

impl CachedVIPERReader {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>, collection_id: String) -> Result<Self> {
        let base_fs = filesystem_factory.get_filesystem("file://")?;
        let cached_filesystem = Arc::new(UnifiedCachingFilesystem::new(
            base_fs,
            collection_id,
            "viper".to_string(),
        ));

        Ok(Self { cached_filesystem })
    }

    pub async fn read_with_footer_cache(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        let _data = self.cached_filesystem.read(file_path).await?;
        // TODO: Use cached Parquet footer and row group metadata
        Ok(vec![])
    }
}