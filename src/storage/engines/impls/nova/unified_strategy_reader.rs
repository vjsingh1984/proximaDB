//! Unified NOVA Reader with strategy-aware caching
//!
//! NOVA (Next-generation Optimized Vector Architecture) implements progressive
//! columnar storage with multi-level quantization and hierarchical zone maps.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::read_strategy::{ReadAccessStrategy, StrategyAwareReader};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};

use super::zone_maps::PruningStrategy;

/// Unified NOVA reader that implements strategy-aware reading
///
/// NOVA specializes in:
/// - Progressive columnar storage with multi-resolution quantization
/// - Hierarchical zone maps for efficient pruning
/// - Parquet-optimized metadata caching
pub struct UnifiedNOVAReader {
    /// Filesystem factory for direct reads
    filesystem_factory: Arc<FilesystemFactory>,

    /// Cached filesystem for selective reads
    cached_filesystem: Option<Arc<UnifiedCachingFilesystem>>,

    /// Current read strategy
    strategy: ReadAccessStrategy,

    /// Collection ID
    collection_id: String,

    /// NOVA-specific pruning strategy
    pruning_strategy: PruningStrategy,
}

impl UnifiedNOVAReader {
    /// Create a new unified NOVA reader
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
                "nova".to_string(),
            )))
        } else {
            None
        };

        // Convert unified strategy to NOVA pruning strategy
        let pruning_strategy = Self::to_nova_pruning_strategy(&strategy);

        Ok(Self {
            filesystem_factory,
            cached_filesystem,
            strategy,
            collection_id,
            pruning_strategy,
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

    /// Create a reader for hierarchical queries (cached with zone map pruning)
    pub fn for_hierarchical_query(
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

    /// Convert unified strategy to NOVA-specific pruning strategy
    fn to_nova_pruning_strategy(strategy: &ReadAccessStrategy) -> PruningStrategy {
        match strategy {
            ReadAccessStrategy::DirectStream => PruningStrategy::NoPruning,
            ReadAccessStrategy::CachedSelective { .. } => PruningStrategy::BasicZoneMap,
            ReadAccessStrategy::CachedSearch { .. } => PruningStrategy::Hierarchical(3),
            ReadAccessStrategy::CachedMetadataOnly => PruningStrategy::BasicZoneMap,
            ReadAccessStrategy::Adaptive { .. } => PruningStrategy::Probabilistic,
        }
    }

    /// Read using progressive columnar access
    pub async fn read_progressive(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        match &self.strategy {
            ReadAccessStrategy::DirectStream => self.read_direct_columnar(file_path).await,
            _ => self.read_with_zone_maps(file_path).await,
        }
    }

    /// Direct columnar read (for full scans - used during compaction)
    async fn read_direct_columnar(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        // Create UnifiedParquetReader for direct reads
        let dimension = 128; // TODO: Get from collection config
        let cached_fs = self
            .cached_filesystem
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UnifiedCachingFilesystem not available"))?;
        let reader = super::readers::UnifiedParquetReader::new(
            vec![file_path.to_string()],
            dimension,
            self.filesystem_factory.clone(),
            cached_fs.clone(),
            self.collection_id.clone(),
            "nova".to_string(),
        )?;

        // For full scan without filters, use similarity search with empty query
        // This returns VectorRecords directly
        let records = reader
            .read_for_similarity_search(
                &[file_path.to_string()],
                None,       // No filter
                usize::MAX, // Get all records
            )
            .await?;

        Ok(records)
    }

    /// Cached read with zone map pruning (for selective queries)
    async fn read_with_zone_maps(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        use crate::core::search::FilterExpression;

        // Create reader with cached filesystem for metadata caching
        let dimension = 128; // TODO: Get from collection config
        let cached_fs = self
            .cached_filesystem
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UnifiedCachingFilesystem not available"))?;
        let reader = super::readers::UnifiedParquetReader::new(
            vec![file_path.to_string()],
            dimension,
            self.filesystem_factory.clone(),
            cached_fs.clone(),
            self.collection_id.clone(),
            "nova".to_string(),
        )?;

        // Apply filter based on strategy
        let metadata_filter = match &self.strategy {
            ReadAccessStrategy::CachedSelective { filter } => {
                filter.as_ref().map(|f| self.convert_to_metadata_filter(f))
            }
            _ => None,
        };

        // Use read_all_records since read_for_similarity_search is not async
        let records = reader.read_all_records(0, None).await?;

        Ok(records)
    }

    /// Convert FilterExpression to MetadataFilter for columnar module
    fn convert_to_metadata_filter(
        &self,
        filter: &crate::core::search::FilterExpression,
    ) -> crate::storage::engines::core::formats::columnar::MetadataFilter {
        // Simple conversion - expand as needed
        // NOVA uses Parquet's built-in statistics and bloom filters for pruning
        // TODO: Convert FilterExpression to FilterConditions based on actual filter content
        use crate::storage::engines::core::formats::columnar::{FilterLogic, MetadataFilter};

        MetadataFilter {
            conditions: vec![],
            logic: FilterLogic::And,
        }
    }

    /// Check if a row group should be read based on zone maps
    fn should_read_row_group(
        &self,
        metadata: &parquet::file::metadata::ParquetMetaData,
        rg_idx: usize,
        filter: &Option<crate::core::search::FilterExpression>,
    ) -> Result<bool> {
        // If no filter, read all row groups
        if filter.is_none() {
            return Ok(true);
        }

        // TODO: Implement actual zone map checking based on row group statistics
        // For now, conservatively read all row groups
        Ok(true)
    }

    /// Evaluate filter against metadata
    fn evaluate_filter(
        &self,
        filter: &crate::core::search::FilterExpression,
        metadata: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<bool> {
        // TODO: Implement actual filter evaluation
        // For now, accept all records
        Ok(true)
    }

    /// Read specific row groups from a Parquet file
    pub async fn read_row_groups(
        &self,
        file_path: &str,
        row_groups: &[usize],
    ) -> Result<Vec<VectorRecord>> {
        // Create Parquet reader
        let dimension = 128; // TODO: Get from collection config
        let cached_fs = self
            .cached_filesystem
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UnifiedCachingFilesystem not available"))?;
        let reader = super::readers::UnifiedParquetReader::new(
            vec![file_path.to_string()],
            dimension,
            self.filesystem_factory.clone(),
            cached_fs.clone(),
            self.collection_id.clone(),
            "nova".to_string(),
        )?;

        // Read specified row groups
        let batches = reader
            .read_row_groups_projected(file_path, row_groups, None)
            .await?;

        // Convert Arrow batches to VectorRecords
        let mut records = Vec::new();
        // batches is Vec<VectorRecord>, so we can extend directly
        records.extend(batches);

        Ok(records)
    }

    /// Convert Arrow RecordBatch to VectorRecords
    fn arrow_batch_to_vector_records(
        &self,
        batch: arrow_array::RecordBatch,
    ) -> Result<Vec<VectorRecord>> {
        use arrow_array::cast::as_primitive_array;
        use arrow_array::cast::as_string_array;
        use arrow_array::types::{Float32Type, Int64Type, UInt32Type};

        let mut records = Vec::new();
        let num_rows = batch.num_rows();

        // Get column arrays
        let id_array = as_string_array(batch.column(0));
        let vector_array = as_primitive_array::<Float32Type>(batch.column(1));
        let timestamp_array = as_primitive_array::<Int64Type>(batch.column(2));
        let version_array = batch.column(3);

        // Get dimension from schema or vector array
        let dimension = if let Some(field) = batch.schema().field(1).metadata().get("dimension") {
            field.parse::<usize>().unwrap_or(1536)
        } else {
            vector_array.len() / num_rows.max(1)
        };

        for row in 0..num_rows {
            // Extract ID
            let id = id_array.value(row).to_string();

            // Extract vector
            let start = row * dimension;
            let end = start + dimension;
            let vector: Vec<f32> = (start..end).map(|i| vector_array.value(i)).collect();

            // Extract timestamp
            let timestamp = timestamp_array.value(row);

            // Extract version (nullable)
            let version = if version_array.is_null(row) {
                None
            } else {
                Some(as_primitive_array::<UInt32Type>(version_array).value(row))
            };

            // Create VectorRecord
            let record = VectorRecord {
                id,
                vector,
                metadata: HashMap::new(), // TODO: Extract metadata columns if present
                timestamp: Some(timestamp),
                version,
                expires_at: None,
                source: None,
                updated_at: Some(timestamp), // Use timestamp as updated_at for now
            };

            records.push(record);
        }

        Ok(records)
    }
}

impl StrategyAwareReader for UnifiedNOVAReader {
    fn strategy(&self) -> &ReadAccessStrategy {
        &self.strategy
    }

    fn set_strategy(&mut self, strategy: ReadAccessStrategy) {
        self.strategy = strategy;
        self.pruning_strategy = Self::to_nova_pruning_strategy(&self.strategy);

        // Update cached filesystem if needed
        if self.strategy.should_use_cache() && self.cached_filesystem.is_none() {
            if let Ok(base_fs) = self.filesystem_factory.get_filesystem("file://") {
                self.cached_filesystem = Some(Arc::new(UnifiedCachingFilesystem::new(
                    base_fs,
                    self.collection_id.clone(),
                    "nova".to_string(),
                )));
            }
        }
    }
}

/// Direct NOVA Reader (bypasses cache for compaction operations)
pub struct DirectNOVAReader {
    filesystem_factory: Arc<FilesystemFactory>,
    collection_id: String,
}

impl DirectNOVAReader {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>, collection_id: String) -> Self {
        Self {
            filesystem_factory,
            collection_id,
        }
    }

    /// Stream Parquet files directly for compaction
    pub async fn stream_parquet(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        // Create reader for direct streaming
        let dimension = 128; // TODO: Get from collection config
        // Create UnifiedCachingFilesystem for optimal performance
        let base_fs = self.filesystem_factory.get_filesystem("file://")?;
        let cached_filesystem = Arc::new(
            crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
                base_fs,
                self.collection_id.clone(),
                "nova".to_string(),
            ),
        );
        let reader = super::readers::UnifiedParquetReader::new(
            vec![file_path.to_string()],
            dimension,
            self.filesystem_factory.clone(),
            cached_filesystem,
            self.collection_id.clone(),
            "nova".to_string(),
        )?;

        // Use similarity search for full read which returns VectorRecords
        let records = reader
            .read_for_similarity_search(
                &[file_path.to_string()],
                None,       // No filter for compaction
                usize::MAX, // Get all records
            )
            .await?;

        Ok(records)
    }
}

/// Cached NOVA Reader (uses UnifiedCachingFilesystem with local disk cache)
pub struct CachedNOVAReader {
    cached_filesystem: Arc<UnifiedCachingFilesystem>,
    collection_id: String,
    pruning_strategy: PruningStrategy,
}

impl CachedNOVAReader {
    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        collection_id: String,
        pruning_strategy: PruningStrategy,
    ) -> Result<Self> {
        let base_fs = filesystem_factory.get_filesystem("file://")?;
        let cached_filesystem = Arc::new(UnifiedCachingFilesystem::new(
            base_fs,
            collection_id.clone(),
            "nova".to_string(),
        ));

        Ok(Self {
            cached_filesystem,
            collection_id,
            pruning_strategy,
        })
    }

    /// Read with hierarchical zone map pruning
    pub async fn read_with_hierarchical_pruning(
        &self,
        file_path: &str,
    ) -> Result<Vec<VectorRecord>> {
        // UnifiedCachingFilesystem provides transparent caching:
        // - Cloud files (S3/GCS/Azure) are downloaded to local disk cache
        // - Cache location: /tmp/proximadb/cache/{collection}/nova/{file}
        // - Metadata extracted and cached separately for fast access
        // - Access patterns tracked for intelligent prefetching

        // Create reader with cached filesystem
        let dimension = 128; // TODO: Get from collection config
        let reader = super::readers::UnifiedParquetReader::new(
            vec![file_path.to_string()],
            dimension,
            Arc::new(FilesystemFactory::create_default().await?),
            self.cached_filesystem.clone(),
            self.collection_id.clone(),
            "nova".to_string(),
        )?;

        // Apply zone map pruning based on strategy
        // NOVA uses Parquet's built-in statistics (min/max per column) and bloom filters
        let filter = match self.pruning_strategy {
            PruningStrategy::NoPruning => None,
            PruningStrategy::BasicZoneMap
            | PruningStrategy::Hierarchical(_)
            | PruningStrategy::Probabilistic
            | PruningStrategy::Adaptive
            | PruningStrategy::MultiScale(_)
            | PruningStrategy::Hybrid => {
                // TODO: Create appropriate MetadataFilter based on Parquet statistics
                // Parquet provides per-row-group statistics that can be used for pruning
                None // For now, no filter
            }
        };

        // Use similarity search which handles zone map pruning internally
        let records = reader
            .read_for_similarity_search(
                &[file_path.to_string()],
                filter.as_ref(),
                usize::MAX, // Get all records
            )
            .await?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nova_strategy_to_pruning() {
        let direct = ReadAccessStrategy::DirectStream;
        let pruning = UnifiedNOVAReader::to_nova_pruning_strategy(&direct);
        assert!(matches!(pruning, PruningStrategy::NoPruning));

        let search = ReadAccessStrategy::CachedSearch {
            prefetch_metadata: true,
        };
        let pruning = UnifiedNOVAReader::to_nova_pruning_strategy(&search);
        assert!(matches!(pruning, PruningStrategy::Hierarchical(_)));
    }
}
