//! Unified HELIX Reader with strategy-aware caching
//!
//! HELIX (High-Efficiency Locality-Indexed eXecution) implements locality-optimized
//! storage with Hilbert curve spatial clustering and PCA-based dimensionality reduction.

use anyhow::Result;
use std::sync::Arc;

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::read_strategy::{ReadAccessStrategy, StrategyAwareReader};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};

use super::SStableMetadata;
use super::clustering::HilbertKey;

/// Unified HELIX reader that implements strategy-aware reading
///
/// HELIX specializes in:
/// - Spatial locality optimization with Hilbert curve clustering
/// - PCA-based clustering for dimensional reduction
/// - Proxima columnar blocks for SIMD optimization
/// - Liquid clustering for adaptive query patterns
pub struct UnifiedHELIXReader {
    /// Filesystem factory for direct reads
    filesystem_factory: Arc<FilesystemFactory>,

    /// Cached filesystem for selective reads
    cached_filesystem: Option<Arc<UnifiedCachingFilesystem>>,

    /// Current read strategy
    strategy: ReadAccessStrategy,

    /// Collection ID
    collection_id: String,

    /// HELIX-specific search strategy
    search_strategy: HelixSearchStrategy,
}

/// HELIX-specific search strategy for different access patterns
#[derive(Debug, Clone)]
pub enum HelixSearchStrategy {
    /// No pruning - full scan (for compaction)
    NoPruning,
    /// Basic Hilbert range pruning
    HilbertRange,
    /// Advanced zone map pruning with PCA projection
    ZoneMapPruning,
    /// Liquid clustering with adaptive patterns
    LiquidClustering { pattern_threshold: usize },
}

impl UnifiedHELIXReader {
    /// Create a new unified HELIX reader
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
                "helix".to_string(),
            )))
        } else {
            None
        };

        // Convert unified strategy to HELIX search strategy
        let search_strategy = Self::to_helix_search_strategy(&strategy);

        Ok(Self {
            filesystem_factory,
            cached_filesystem,
            strategy,
            collection_id,
            search_strategy,
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
            ReadAccessStrategy::CachedSearch {
                prefetch_metadata: true,
            },
        )
    }

    /// Create a reader for spatial locality queries (cached with liquid clustering)
    pub fn for_spatial_locality_query(
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

    /// Convert unified strategy to HELIX-specific search strategy
    fn to_helix_search_strategy(strategy: &ReadAccessStrategy) -> HelixSearchStrategy {
        match strategy {
            ReadAccessStrategy::DirectStream => HelixSearchStrategy::NoPruning,
            ReadAccessStrategy::CachedSelective { .. } => HelixSearchStrategy::HilbertRange,
            ReadAccessStrategy::CachedSearch { .. } => HelixSearchStrategy::ZoneMapPruning,
            ReadAccessStrategy::CachedMetadataOnly => HelixSearchStrategy::HilbertRange,
            ReadAccessStrategy::Adaptive { .. } => HelixSearchStrategy::LiquidClustering {
                pattern_threshold: 100,
            },
        }
    }

    /// Read using spatial locality optimized access
    pub async fn read_with_locality(
        &self,
        file_path: &str,
        query_hilbert: Option<HilbertKey>,
    ) -> Result<Vec<VectorRecord>> {
        match &self.strategy {
            ReadAccessStrategy::DirectStream => self.read_direct_streaming(file_path).await,
            _ => {
                self.read_with_hilbert_pruning(file_path, query_hilbert)
                    .await
            }
        }
    }

    /// Direct streaming read (for full scans and compaction)
    async fn read_direct_streaming(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        // Use filesystem factory directly for streaming reads
        let fs = self.filesystem_factory.get_filesystem("file://")?;
        let _data = fs.read(file_path).await?;

        // TODO: Parse HELIX Proxima format directly without caching
        // Use Proxima blocks for SIMD optimization
        Ok(vec![])
    }

    /// Cached read with Hilbert curve pruning
    async fn read_with_hilbert_pruning(
        &self,
        file_path: &str,
        query_hilbert: Option<HilbertKey>,
    ) -> Result<Vec<VectorRecord>> {
        let cached_fs = self
            .cached_filesystem
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Cached filesystem not initialized"))?;

        let _data = cached_fs.read(file_path).await?;

        // TODO: Use cached Proxima metadata and Hilbert ranges for pruning
        // Apply zone map pruning based on search strategy
        match self.search_strategy {
            HelixSearchStrategy::NoPruning => {
                // Read all data
            }
            HelixSearchStrategy::HilbertRange => {
                // Apply basic Hilbert range pruning
            }
            HelixSearchStrategy::ZoneMapPruning => {
                // Apply advanced zone map pruning
            }
            HelixSearchStrategy::LiquidClustering { .. } => {
                // Apply adaptive clustering patterns
            }
        }

        Ok(vec![])
    }

    /// Check if reader is using cache
    pub fn is_using_cache(&self) -> bool {
        self.cached_filesystem.is_some()
    }
}

impl StrategyAwareReader for UnifiedHELIXReader {
    fn strategy(&self) -> &ReadAccessStrategy {
        &self.strategy
    }

    fn set_strategy(&mut self, strategy: ReadAccessStrategy) {
        self.strategy = strategy;
        self.search_strategy = Self::to_helix_search_strategy(&self.strategy);

        // Update cached filesystem if needed
        if self.strategy.should_use_cache() && self.cached_filesystem.is_none() {
            if let Ok(base_fs) = self.filesystem_factory.get_filesystem("file://") {
                self.cached_filesystem = Some(Arc::new(UnifiedCachingFilesystem::new(
                    base_fs,
                    self.collection_id.clone(),
                    "helix".to_string(),
                )));
            }
        }
    }
}

/// Direct HELIX Reader (bypasses cache, streams Proxima blocks)
///
/// Use this for:
/// - Compaction operations
/// - Full SSTable scans
/// - Time-series batch processing
/// - PCA model training
pub struct DirectHELIXReader {
    filesystem_factory: Arc<FilesystemFactory>,
    collection_id: String,
}

impl DirectHELIXReader {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>, collection_id: String) -> Self {
        Self {
            filesystem_factory,
            collection_id,
        }
    }

    /// Stream Proxima blocks directly for compaction
    pub async fn stream_proximablocks(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        let fs = self.filesystem_factory.get_filesystem("file://")?;
        let _data = fs.read(file_path).await?;

        // TODO: Use Proxima block streaming for direct access
        Ok(vec![])
    }

    /// Stream SSTables with Hilbert sorting for compaction
    pub async fn stream_sorted_by_hilbert(
        &self,
        sstables: &[SStableMetadata],
    ) -> Result<Vec<VectorRecord>> {
        let _fs = self.filesystem_factory.get_filesystem("file://")?;
        let mut all_records = Vec::new();

        for sstable in sstables {
            let _data = _fs.read(&sstable.path.to_string_lossy()).await?;
            // TODO: Read Proxima blocks and extract records
            // For now, return empty vec - actual implementation would parse Proxima format
        }

        // TODO: Sort by Hilbert key for optimal compaction
        all_records.sort_by_key(|record: &VectorRecord| {
            // Placeholder: compute Hilbert key from record
            record.id.len()
        });

        Ok(all_records)
    }
}

/// Cached HELIX Reader (uses zone maps and Proxima metadata caching)
///
/// Use this for:
/// - Spatial locality queries with range filtering
/// - Point queries by ID with Hilbert locality
/// - Search operations with PCA-based pruning
/// - Liquid clustering adaptive patterns
pub struct CachedHELIXReader {
    cached_filesystem: Arc<UnifiedCachingFilesystem>,
    collection_id: String,
    search_strategy: HelixSearchStrategy,
}

impl CachedHELIXReader {
    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
        search_strategy: HelixSearchStrategy,
    ) -> Result<Self> {
        let base_fs = filesystem_factory.get_filesystem("file://")?;
        let cached_filesystem = Arc::new(UnifiedCachingFilesystem::new(
            base_fs,
            collection_id.clone(),
            "helix".to_string(),
        ));

        Ok(Self {
            cached_filesystem,
            collection_id,
            search_strategy,
        })
    }

    /// Read with spatial locality optimization and Hilbert pruning
    pub async fn read_with_spatial_pruning(
        &self,
        file_path: &str,
        query_hilbert: Option<HilbertKey>,
        _range_filter: Option<(f64, f64)>, // (min_value, max_value) for any dimension
    ) -> Result<Vec<VectorRecord>> {
        let _data = self.cached_filesystem.read(file_path).await?;

        // TODO: Apply spatial locality optimized pruning
        match self.search_strategy {
            HelixSearchStrategy::NoPruning => {
                // Read all data for full scans
            }
            HelixSearchStrategy::HilbertRange => {
                // Apply Hilbert range pruning for locality
            }
            HelixSearchStrategy::ZoneMapPruning => {
                // Apply zone map pruning with PCA projection
            }
            HelixSearchStrategy::LiquidClustering {
                pattern_threshold: _,
            } => {
                // Apply adaptive clustering patterns
            }
        }

        Ok(vec![])
    }

    /// Read with liquid clustering adaptation
    pub async fn read_with_liquid_clustering(
        &self,
        file_path: &str,
        _access_pattern: &str, // Query pattern for adaptation
    ) -> Result<Vec<VectorRecord>> {
        let _data = self.cached_filesystem.read(file_path).await?;

        // TODO: Apply liquid clustering based on access pattern
        // Cache frequently accessed patterns for better performance

        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helix_strategy_to_search() {
        let direct = ReadAccessStrategy::DirectStream;
        let search_strategy = UnifiedHELIXReader::to_helix_search_strategy(&direct);
        assert!(matches!(search_strategy, HelixSearchStrategy::NoPruning));

        let search = ReadAccessStrategy::CachedSearch {
            prefetch_metadata: true,
        };
        let search_strategy = UnifiedHELIXReader::to_helix_search_strategy(&search);
        assert!(matches!(
            search_strategy,
            HelixSearchStrategy::ZoneMapPruning
        ));

        let adaptive = ReadAccessStrategy::Adaptive {
            initial_strategy: Box::new(ReadAccessStrategy::DirectStream),
            fallback_threshold: 100,
        };
        let search_strategy = UnifiedHELIXReader::to_helix_search_strategy(&adaptive);
        assert!(matches!(
            search_strategy,
            HelixSearchStrategy::LiquidClustering { .. }
        ));
    }

    #[tokio::test]
    async fn test_helix_reader_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _path = temp_dir.path().to_str().unwrap().to_string();

        let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        fs_config.default_fs = Some(format!("file://{}", _path));
        let factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config)
                .await
                .unwrap(),
        );

        // Compaction should use DirectStream
        let compaction_reader =
            UnifiedHELIXReader::for_compaction(factory.clone(), "test_collection".to_string())
                .unwrap();
        assert_eq!(
            compaction_reader.strategy(),
            &ReadAccessStrategy::DirectStream
        );
        assert!(!compaction_reader.is_using_cache());

        // Search should use CachedSearch
        let _search_reader =
            UnifiedHELIXReader::for_search(factory.clone(), "test_collection".to_string()).unwrap();
        assert!(matches!(
            _search_reader.strategy(),
            ReadAccessStrategy::CachedSearch { .. }
        ));
        assert!(_search_reader.is_using_cache());
    }

    #[tokio::test]
    async fn test_strategy_updates() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _path = temp_dir.path().to_str().unwrap().to_string();

        let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        fs_config.default_fs = Some(format!("file://{}", _path));
        let factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config)
                .await
                .unwrap(),
        );
        let mut reader = UnifiedHELIXReader::for_search(factory, "test".to_string()).unwrap();

        // Initially configured for search (cached)
        assert!(reader.is_using_cache());
        assert!(matches!(
            reader.search_strategy,
            HelixSearchStrategy::ZoneMapPruning
        ));

        // Change to direct stream
        reader.set_strategy(ReadAccessStrategy::DirectStream);
        assert!(matches!(
            reader.search_strategy,
            HelixSearchStrategy::NoPruning
        ));
    }
}
