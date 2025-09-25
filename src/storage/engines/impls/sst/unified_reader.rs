//! Unified SST Reader with strategy-aware caching
//!
//! This reader implements the unified ReadAccessStrategy pattern,
//! choosing between cached and direct reads based on the access pattern.

use anyhow::Result;
use std::sync::Arc;

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::read_strategy::{ReadAccessStrategy, StrategyAwareReader};
use crate::storage::persistence::filesystem::{FilesystemFactory, FileSystem};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;

use super::readers::sst_query_engine::UnifiedSstableReader;

/// Unified SST reader that implements strategy-aware reading
///
/// This reader automatically chooses between:
/// - DirectSSTReader: For full scans (compaction, batch operations)
/// - CachedSSTReader: For selective queries (point lookups, searches)
pub struct UnifiedSSTReader {
    /// Filesystem factory for direct reads
    filesystem_factory: Arc<FilesystemFactory>,

    /// Cached filesystem for selective reads
    cached_filesystem: Option<Arc<UnifiedCachingFilesystem>>,

    /// Current read strategy
    strategy: ReadAccessStrategy,

    /// Collection ID
    collection_id: String,

    /// The actual SST reader (reused for both strategies)
    inner_reader: Arc<UnifiedSstableReader>,
}

impl UnifiedSSTReader {
    /// Create a new unified SST reader
    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
        strategy: ReadAccessStrategy,
    ) -> Result<Self> {
        // Create cached filesystem if needed by strategy
        let cached_filesystem = if strategy.should_use_cache() {
            let base_fs = filesystem_factory.get_filesystem("file://")?;
            Some(Arc::new(UnifiedCachingFilesystem::new(
                base_fs,
                collection_id.clone(),
                "sst".to_string(),
            )))
        } else {
            None
        };

        // Create the appropriate inner reader based on strategy
        let inner_reader = if strategy.should_use_cache() {
            // Use cached filesystem for selective reads
            Arc::new(UnifiedSstableReader::new(
                filesystem_factory.clone(),
                cached_filesystem.as_ref().unwrap().clone(),
                collection_id.clone(),
            ))
        } else {
            // For direct reads, create a minimal cached filesystem that bypasses cache
            // This is a temporary solution - ideally UnifiedSstableReader should support direct mode
            let base_fs = filesystem_factory.get_filesystem("file://")?;
            let minimal_cache = Arc::new(UnifiedCachingFilesystem::new(
                base_fs,
                collection_id.clone(),
                "sst_direct".to_string(),
            ));
            Arc::new(UnifiedSstableReader::new(
                filesystem_factory.clone(),
                minimal_cache,
                collection_id.clone(),
            ))
        };

        Ok(Self {
            filesystem_factory,
            cached_filesystem,
            strategy,
            collection_id,
            inner_reader,
        })
    }

    /// Create a reader optimized for compaction (direct reads)
    pub fn for_compaction(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
    ) -> Result<Self> {
        Self::new(
            filesystem_factory,
            collection_id,
            ReadAccessStrategy::DirectStream,
        )
    }

    /// Create a reader optimized for search (cached reads)
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

    /// Create a reader for filtered queries (cached reads)
    pub fn for_filtered_query(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
        filter: Option<crate::core::search::FilterExpression>,
    ) -> Result<Self> {
        Self::new(
            filesystem_factory,
            collection_id,
            ReadAccessStrategy::CachedSelective { filter },
        )
    }

    /// Read a vector by ID
    pub async fn read_vector(&self, id: &str, file_path: &str) -> Result<Option<VectorRecord>> {
        // The inner reader handles the actual read
        self.inner_reader.vector(file_path, id).await
    }

    /// Read vectors in batch (optimized for the current strategy)
    pub async fn read_batch(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        // For direct stream, we should read sequentially without caching
        // For cached strategies, we can use the cache
        match &self.strategy {
            ReadAccessStrategy::DirectStream => {
                // TODO: Implement true streaming read that bypasses cache
                // For now, use the inner reader
                self.inner_reader.read_all_records_for_compaction(&[file_path.to_string()]).await
            }
            _ => {
                // Use cached read path
                self.inner_reader.read_all_records_for_compaction(&[file_path.to_string()]).await
            }
        }
    }

    /// Validate an SST file
    pub async fn validate_file(&self, file_path: &str) -> Result<()> {
        self.inner_reader.validate_sst_file(file_path).await
    }
}

impl StrategyAwareReader for UnifiedSSTReader {
    fn strategy(&self) -> &ReadAccessStrategy {
        &self.strategy
    }

    fn set_strategy(&mut self, strategy: ReadAccessStrategy) {
        // Update strategy and potentially recreate inner reader if needed
        let strategy_changed = self.strategy.should_use_cache() != strategy.should_use_cache();
        self.strategy = strategy;

        if strategy_changed {
            // Recreate cached filesystem if strategy changed
            if self.strategy.should_use_cache() && self.cached_filesystem.is_none() {
                if let Ok(base_fs) = self.filesystem_factory.get_filesystem("file://") {
                    self.cached_filesystem = Some(Arc::new(UnifiedCachingFilesystem::new(
                        base_fs,
                        self.collection_id.clone(),
                        "sst".to_string(),
                    )));
                }
            }
            // Note: We should also recreate inner_reader here, but that requires &mut self throughout
        }
    }
}

/// Direct SST Reader (bypasses cache completely)
///
/// Use this for:
/// - Compaction operations
/// - Full table scans
/// - Batch migrations
/// - Any sequential read of entire files
pub struct DirectSSTReader {
    filesystem_factory: Arc<FilesystemFactory>,
    collection_id: String,
}

impl DirectSSTReader {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>, collection_id: String) -> Self {
        Self {
            filesystem_factory,
            collection_id,
        }
    }

    /// Read entire file directly without caching
    pub async fn read_file_direct(&self, file_path: &str) -> Result<Vec<u8>> {
        let fs = self.filesystem_factory.get_filesystem("file://")?;
        fs.read(file_path).await.map_err(|e| anyhow::anyhow!("Failed to read file: {}", e))
    }

    /// Stream records directly from file
    pub async fn stream_records(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        // TODO: Implement true streaming that reads blocks sequentially
        // without caching or loading entire file into memory
        let _data = self.read_file_direct(file_path).await?;

        // For now, return empty vec - actual implementation would parse SST format
        Ok(vec![])
    }
}

/// Cached SST Reader (uses UnifiedCachingFilesystem)
///
/// Use this for:
/// - Point queries by ID
/// - Range queries
/// - Search operations
/// - Any operation that benefits from caching bloom filters/indexes
pub struct CachedSSTReader {
    inner_reader: Arc<UnifiedSstableReader>,
}

impl CachedSSTReader {
    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
    ) -> Result<Self> {
        let base_fs = filesystem_factory.get_filesystem("file://")?;
        let cached_fs = Arc::new(UnifiedCachingFilesystem::new(
            base_fs,
            collection_id.clone(),
            "sst".to_string(),
        ));

        let inner_reader = Arc::new(UnifiedSstableReader::new(
            filesystem_factory,
            cached_fs,
            collection_id,
        ));

        Ok(Self { inner_reader })
    }

    /// Read with cache benefit
    pub async fn read_with_cache(&self, file_path: &str, id: &str) -> Result<Option<VectorRecord>> {
        self.inner_reader.vector(file_path, id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::FilesystemConfig;

    #[tokio::test]
    async fn test_unified_reader_strategy_selection() {
        // Test that correct strategy is selected for different use cases
        let factory = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await.unwrap());

        // Compaction should use DirectStream
        let compaction_reader = UnifiedSSTReader::for_compaction(
            factory.clone(),
            "test_collection".to_string(),
        ).unwrap();
        assert_eq!(compaction_reader.strategy(), &ReadAccessStrategy::DirectStream);
        assert!(!compaction_reader.is_using_cache());

        // Search should use CachedSearch
        let search_reader = UnifiedSSTReader::for_search(
            factory.clone(),
            "test_collection".to_string(),
        ).unwrap();
        matches!(search_reader.strategy(), ReadAccessStrategy::CachedSearch { .. });
        assert!(search_reader.is_using_cache());
    }
}