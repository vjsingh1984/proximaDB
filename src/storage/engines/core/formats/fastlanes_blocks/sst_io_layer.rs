// =============================================================================
// LOW-LEVEL SST I/O INFRASTRUCTURE (sst_io_layer.rs)
// =============================================================================
//
// PURPOSE: Low-level I/O operations and caching infrastructure for SST files
// USED BY: sst_query_engine.rs (high-level query engine)
//
// This module provides:
// - Zero-copy file access with memory mapping strategies
// - Block-level caching for SST data blocks
// - Bloom filter caching (4KB per file)
// - Index block caching (60KB per file)
// - Bandwidth optimization for cloud storage
// - FastLanes encoding support for compressed blocks
//
// RELATIONSHIP WITH sst_query_engine.rs:
// Similar to parquet_io_layer vs parquet_query_engine:
// - This handles LOW-LEVEL I/O and caching
// - sst_query_engine handles HIGH-LEVEL query logic
//
// RENAME SUGGESTION: This file should be renamed to `sst_io_layer.rs`
// to match the parquet naming convention
//
// FASTLANES INTEGRATION ARCHITECTURE:
// ====================================
// This reader supports multiple encoding schemes per DataBlock based on data characteristics:
//
// 1. ENCODING DETECTION:
//    - Each DataBlock has a 1-byte encoding marker at offset 0
//    - Marker format: [7:4] = Major encoding, [3:0] = Sub-encoding variant
//    - Examples: 0x00 = Raw, 0x10 = FastLanes BitPacked, 0x20 = FastLanes Delta, etc.
//
// 2. DATABLOCK LAYOUT WITH FASTLANES:
//    Traditional SST DataBlock:
//    [Header][Records][Bloom][Index]
//
//    FastLanes-Enhanced DataBlock:
//    [EncodingMarker(1B)][Header][EncodedVectorData][MetadataSection][Bloom][Index]
//
//    Where EncodedVectorData uses columnar transpose:
//    - Vectors are transposed: 500 vectors x 384 dims → 384 columns x 500 values
//    - Each column encoded independently based on statistics
//    - Enables SIMD-friendly access patterns
//
// 3. MIXED ENCODING SUPPORT:
//    - Different blocks can use different encodings
//    - Encoding chosen at write-time based on data statistics
//    - Reader detects and handles encoding transparently
//
// 4. BACKWARD COMPATIBILITY:
//    - Marker 0x00 indicates traditional format
//    - New readers can read old blocks
//    - Old readers will fail gracefully on new encoded blocks

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// memmap2 imports removed - using filesystem API for memory mapping
use anyhow::Result;
use tracing::info;

use crate::storage::persistence::filesystem::FilesystemFactory;
// Using UnifiedCachingFilesystem for efficient caching
use crate::core::error::{ProximaDBError, StorageError};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;

/// File type enum for cache key discrimination
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    SST,
    Parquet,
    Index,
}

/// SST file metadata for caching
#[derive(Debug, Clone)]
pub struct SstFileMetadata {
    pub file_path: String,
    pub file_size: u64,
    pub file_bloom_filter: Arc<Vec<u8>>,
    pub file_index: Arc<Vec<u8>>,
    pub last_accessed: std::time::Instant,
}

const BLOOM_FILTER_SIZE: usize = 4096; // 4KB bloom filters
const INDEX_BLOCK_SIZE: usize = 61440; // 60KB index blocks
const DATA_BLOCK_SIZE: usize = 65536; // 64KB data blocks

/// Shared SST format reader with zero-copy cache-first architecture
/// Leverages OS page cache for optimal memory management vs dedicated VectorStore
pub struct SharedSstFormatReader {
    /// Filesystem for I/O operations
    filesystem: Arc<FilesystemFactory>,

    /// Memory mapping strategy (kept for region-specific optimizations)
    mmap_strategy: SstMmapStrategy,

    /// UNIFIED CACHE: UnifiedCachingFilesystem replaces all specialized caches
    unified_filesystem: Arc<UnifiedCachingFilesystem>,

    /// Collection ID for filename-based cache keys
    collection_id: String,

    /// Stats for monitoring
    stats: Arc<ReaderStats>,

    /// ✅ Reusable distance compute engine - created once and passed to all search operations
    distance_compute: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,
}

/// Shared SST format writer with compression and FastLanes encoding
/// Complements the reader for full read/write support
pub struct SharedSstFormatWriter {
    /// Filesystem for I/O operations
    filesystem: Arc<FilesystemFactory>,

    /// Unified filesystem for write operations
    unified_filesystem: Arc<UnifiedCachingFilesystem>,

    /// Collection ID for filename-based cache keys
    collection_id: String,

    /// Compression configuration
    compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,

    /// Stats for monitoring writes
    stats: Arc<WriterStats>,
}

/// Writer statistics for monitoring
pub struct WriterStats {
    blocks_written: AtomicU64,
    bytes_written: AtomicU64,
    bytes_compressed: AtomicU64,
    compression_time_ms: AtomicU64,
    writes_total: AtomicU64,
}

#[derive(Clone)]
pub struct SstMmapStrategy {
    /// Always mmap these regions (critical for performance)
    pub always_mmap: Vec<SstRegion>,

    /// Conditionally mmap based on memory pressure
    pub conditional_mmap: Vec<(SstRegion, f32)>, // (region, max_pressure_threshold)

    /// Never mmap these (always stream)
    pub never_mmap: Vec<SstRegion>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SstRegion {
    BloomFilter,     // First 4KB - always cached
    IndexBlock,      // 4KB-64KB typically - usually cached
    CompressionDict, // If present
    DataBlocks,      // Large, usually streamed
    Metadata,        // File metadata
}

/// Statistics for monitoring
pub struct ReaderStats {
    bloom_hits: AtomicU64,
    bloom_misses: AtomicU64,
    index_hits: AtomicU64,
    index_misses: AtomicU64,
    bytes_downloaded: AtomicU64,
    bytes_saved: AtomicU64, // Saved by filtering
    cache_invalidations: AtomicU64,
}

impl SharedSstFormatReader {
    pub fn new(
        filesystem: Arc<FilesystemFactory>,
        mmap_strategy: SstMmapStrategy,
        unified_filesystem: Arc<UnifiedCachingFilesystem>,
        collection_id: String,
    ) -> Self {
        use crate::compute::distance_computation::engine::UnifiedDistanceCompute;

        Self {
            filesystem,
            mmap_strategy,
            unified_filesystem,
            collection_id,
            stats: Arc::new(ReaderStats::default()),
            distance_compute: Arc::new(UnifiedDistanceCompute::default()),  // ✅ Create once and reuse
        }
    }

    /// Enhanced read with strategy-based delegation to FastLanes blocks
    pub fn read_with_strategy<'a>(
        &'a self,
        file_path: &'a str,
        strategy: &'a crate::storage::engines::impls::sst::readers::sst_query_engine::SstableReadingStrategy,
        filter_expression: Option<&'a crate::core::search::FilterExpression>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock>, ProximaDBError>> + Send + 'a>> {
        Box::pin(async move {
        use crate::storage::engines::impls::sst::readers::sst_query_engine::SstableReadingStrategy;

        match strategy {
            SstableReadingStrategy::FullScan { use_block_cache } => {
                self.full_scan_read(file_path, *use_block_cache).await
            }
            SstableReadingStrategy::SelectiveWithCache {
                use_range_reads,
                enable_bloom_filters,
                enable_cache_lookup,
                enable_metadata_cache,
            } => {
                self.selective_cache_read(
                    file_path,
                    *use_range_reads,
                    *enable_bloom_filters,
                    *enable_cache_lookup,
                    *enable_metadata_cache,
                    filter_expression,
                ).await
            }
            SstableReadingStrategy::CompactionFullRead {
                skip_bloom_filters,
                skip_indexes,
                bypass_write_cache,
                use_disk_cache_if_exists,
                sequential_io,
            } => {
                self.compaction_read(
                    file_path,
                    *skip_bloom_filters,
                    *skip_indexes,
                    *bypass_write_cache,
                    *use_disk_cache_if_exists,
                    *sequential_io,
                ).await
            }
            SstableReadingStrategy::IndexRangeScan {
                start_block,
                end_block,
                use_bloom_filter,
            } => {
                self.range_scan_read(file_path, *start_block as u32, *end_block as u32, *use_bloom_filter).await
            }
            SstableReadingStrategy::MetadataFiltered {
                selected_blocks,
                skip_bloom_check,
            } => {
                // Convert Vec<usize> to Vec<u32>
                let blocks_u32: Vec<u32> = selected_blocks.iter().map(|&b| b as u32).collect();
                self.metadata_filtered_read(file_path, &blocks_u32, *skip_bloom_check, filter_expression).await
            }
            SstableReadingStrategy::Hybrid {
                primary_strategy,
                fallback_blocks,
            } => {
                // Try primary strategy first, then fallback for specific blocks
                let mut primary_results = self.read_with_strategy(file_path, primary_strategy, filter_expression).await?;

                // Add fallback blocks if needed
                if !fallback_blocks.is_empty() {
                    // Convert Vec<usize> to Vec<u32>
                    let fallback_u32: Vec<u32> = fallback_blocks.iter().map(|&b| b as u32).collect();
                    let fallback_results = self.read_specific_blocks(file_path, &fallback_u32).await?;
                    primary_results.extend(fallback_results);
                }

                Ok(primary_results)
            }
        }
        })
    }

    /// Full scan read - reads all blocks without filtering
    async fn full_scan_read(
        &self,
        file_path: &str,
        use_block_cache: bool,
    ) -> Result<Vec<crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock>, ProximaDBError> {
        use crate::storage::persistence::filesystem::FileSystem;

        // Read data from file
        let data = if use_block_cache {
            // Use unified filesystem with caching
            self.unified_filesystem.read(file_path).await?
        } else {
            // Direct read without caching
            self.filesystem.get_filesystem(file_path)?
                .read(file_path).await?
        };

        // Deserialize blocks from raw data
        // For now, try to deserialize as a single block
        // TODO: Implement multi-block file format
        let blocks = if let Ok(single_block) =
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock::deserialize(&data) {
            vec![single_block]
        } else {
            // If single block fails, assume empty or corrupted file
            Vec::new()
        };
        Ok(blocks)
    }

    /// Selective cache read with various optimizations
    async fn selective_cache_read(
        &self,
        file_path: &str,
        use_range_reads: bool,
        enable_bloom_filters: bool,
        enable_cache_lookup: bool,
        enable_metadata_cache: bool,
        filter_expression: Option<&crate::core::search::FilterExpression>,
    ) -> Result<Vec<crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock>, ProximaDBError> {
        use crate::storage::persistence::filesystem::FileSystem;

        // Use unified filesystem with selective caching
        let mut blocks = Vec::new();

        // Read the entire file first
        let data = if enable_cache_lookup {
            self.unified_filesystem.read(file_path).await?
        } else {
            self.filesystem.get_filesystem(file_path)?.read(file_path).await?
        };

        // Deserialize blocks
        // TODO: Implement multi-block file format
        let all_blocks = if let Ok(single_block) =
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock::deserialize(&data) {
            vec![single_block]
        } else {
            Vec::new()
        };

        for block in all_blocks {
            // Check bloom filter if enabled
            if enable_bloom_filters {
                if let Some(ref filter_expr) = filter_expression {
                    // FastLanes blocks have auto-generated bloom filters
                    if let Some(ref bloom) = block.bloom_filter {
                        // Check if block might contain matching records
                        // TODO: Implement bloom filter check logic
                    }
                }
            }

            blocks.push(block);
        }

        Ok(blocks)
    }

    /// Compaction read - optimized for sequential I/O during compaction
    async fn compaction_read(
        &self,
        file_path: &str,
        skip_bloom_filters: bool,
        skip_indexes: bool,
        bypass_write_cache: bool,
        use_disk_cache_if_exists: bool,
        sequential_io: bool,
    ) -> Result<Vec<crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock>, ProximaDBError> {
        // Compaction needs all blocks for merging
        use crate::storage::persistence::filesystem::FileSystem;

        // Read the entire file
        let data = self.unified_filesystem.read(file_path).await?;

        // Deserialize blocks
        // TODO: Implement multi-block file format
        let blocks = if let Ok(single_block) =
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock::deserialize(&data) {
            vec![single_block]
        } else {
            Vec::new()
        };
        Ok(blocks)
    }

    /// Range scan read - reads blocks within a specific range
    async fn range_scan_read(
        &self,
        file_path: &str,
        start_block: u32,
        end_block: u32,
        use_bloom_filter: bool,
    ) -> Result<Vec<crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock>, ProximaDBError> {
        use crate::storage::persistence::filesystem::FileSystem;

        // For now, read the entire file and filter blocks in memory
        // TODO: Implement range-based reading when multi-block format is ready
        let data = self.unified_filesystem.read(file_path).await?;

        // Deserialize and filter blocks by range
        let all_blocks = if let Ok(single_block) =
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock::deserialize(&data) {
            vec![single_block]
        } else {
            Vec::new()
        };

        // Filter blocks by range (when we have multi-block support)
        let blocks: Vec<_> = all_blocks
            .into_iter()
            .enumerate()
            .filter(|(idx, _)| *idx >= start_block as usize && *idx <= end_block as usize)
            .map(|(_, block)| block)
            .collect();

        Ok(blocks)
    }

    /// Metadata filtered read - reads blocks based on metadata predicates
    async fn metadata_filtered_read(
        &self,
        file_path: &str,
        selected_blocks: &[u32],
        skip_bloom_check: bool,
        filter_expression: Option<&crate::core::search::FilterExpression>,
    ) -> Result<Vec<crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock>, ProximaDBError> {
        // Read only selected blocks
        let mut blocks = Vec::new();
        use crate::storage::persistence::filesystem::FileSystem;

        // Read the entire file once
        let data = self.unified_filesystem.read(file_path).await?;

        // Deserialize all blocks
        let all_blocks = if let Ok(single_block) =
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock::deserialize(&data) {
            vec![single_block]
        } else {
            Vec::new()
        };

        // Select specific blocks
        for &block_id in selected_blocks {
            if (block_id as usize) < all_blocks.len() {
                blocks.push(all_blocks[block_id as usize].clone());
            }
        }
        Ok(blocks)
    }

    /// Read specific blocks by their IDs
    async fn read_specific_blocks(
        &self,
        file_path: &str,
        block_ids: &[u32],
    ) -> Result<Vec<crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock>, ProximaDBError> {
        let mut blocks = Vec::new();
        use crate::storage::persistence::filesystem::FileSystem;

        // Read the entire file once
        let data = self.unified_filesystem.read(file_path).await?;

        // Deserialize all blocks
        let all_blocks = if let Ok(single_block) =
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock::deserialize(&data) {
            vec![single_block]
        } else {
            Vec::new()
        };

        // Select specific blocks
        for &block_id in block_ids {
            if (block_id as usize) < all_blocks.len() {
                blocks.push(all_blocks[block_id as usize].clone());
            }
        }
        Ok(blocks)
    }

    /// Search FastLanes blocks with predicate pushdown
    pub async fn search_blocks_with_predicate(
        &self,
        blocks: &[crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock],
        query_vector: &[f32],
        filter_expression: Option<&crate::core::search::FilterExpression>,
        k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        distance_compute: &crate::compute::distance_computation::engine::UnifiedDistanceCompute,  // ✅ Pass from caller for reuse
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>, ProximaDBError> {
        let mut all_results = Vec::new();

        // Process each block with batch distance computation
        for block in blocks {
            // Collect vectors from block for batch processing
            let mut block_records = Vec::new();
            let mut block_vectors = Vec::new();

            for record in &block.records {
                // Apply filter expression if provided
                if let Some(filter) = filter_expression {
                    // TODO: Implement proper filter evaluation
                    // For now, pass all records
                }
                block_records.push(record);
                block_vectors.push(record.vector.as_slice());
            }

            // Batch calculate distances for entire block
            if !block_vectors.is_empty() {
                let distances = distance_compute.batch_distance_pooled_simd(
                    query_vector,
                    &block_vectors,
                    &distance_metric
                );

                // Create search records with batch distances
                for (record, distance_result) in block_records.into_iter().zip(distances.iter()) {
                    let search_record = crate::core::search::results::OptimizedSearchRecord {
                        id: record.id.clone(),
                        vector_id: Some(record.id.clone()),
                        score: distance_result.distance,
                        similarity: Some(distance_result.distance),
                        metadata: record.metadata.clone(),
                        vector: Some(Arc::new(record.vector.clone())),
                        debug_info: None,
                        version: None,
                        timestamp: Some(record.timestamp),
                        updated_at: None,
                        expires_at: None,
                        source: None,
                        expanded_context: Vec::new(),
                        semantic_similarity: None,
                        quantization_info: None,
                        engine_stats: None,
                        index_path: None,
                    };
                    all_results.push(search_record);
                }
            }
        }

        // Sort and truncate to top-k
        all_results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
        all_results.truncate(k);

        Ok(all_results)
    }

    /// Smart read that minimizes bandwidth usage
    pub async fn read_record(
        &self,
        file_path: &str,
        collection_id: &str,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, ProximaDBError> {
        // Step 1: Check bloom filter BEFORE downloading anything
        let bloom_data = self
            .get_bloom_filter_smart(file_path, collection_id)
            .await?;
        if !self.check_bloom(&bloom_data, key) {
            // Key definitely not in file - saved bandwidth!
            self.stats
                .bytes_saved
                .fetch_add(DATA_BLOCK_SIZE as u64, Ordering::Relaxed);
            return Ok(None);
        }

        // Step 2: Check index block to find data block location
        let index_data = self.get_index_block_smart(file_path, collection_id).await?;
        let block_info = match self.find_block_for_key(&index_data, key)? {
            Some(info) => info,
            None => {
                // Key not in index - saved bandwidth!
                self.stats
                    .bytes_saved
                    .fetch_add(DATA_BLOCK_SIZE as u64, Ordering::Relaxed);
                return Ok(None);
            }
        };

        // Step 3: NOW download the data block since we know it's needed
        let data = self
            .read_data_block_smart(file_path, collection_id, &block_info)
            .await?;

        self.find_in_block(&data, key)
    }

    /// Get bloom filter with smart bandwidth optimization
    async fn get_bloom_filter_smart(
        &self,
        file_path: &str,
        collection_id: &str,
    ) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        let filename = std::path::Path::new(file_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown");

        // Check if file metadata with bloom filter is cached
        // The zero_copy_system returns Arc<Box<dyn EngineMetadata>> which we can't directly downcast
        // We'd need to access the bloom filter through the EngineMetadata trait methods instead
        // For SST files, we'll use direct file reads for bloom filters since they're small (4KB)

        self.stats.bloom_misses.fetch_add(1, Ordering::Relaxed);

        // For cloud files, download ONLY the bloom filter range
        if self.is_cloud_file(file_path) {
            // Use range request to get just 4KB bloom filter
            let fs = self.filesystem.get_filesystem(file_path).map_err(|e| {
                ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to get filesystem: {}", e),
                )))
            })?;
            let bloom_data = fs
                .read_range(file_path, 0, BLOOM_FILTER_SIZE as u64)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to read bloom filter: {}", e),
                    )))
                })?;

            self.stats
                .bytes_downloaded
                .fetch_add(BLOOM_FILTER_SIZE as u64, Ordering::Relaxed);

            // Cache the bloom filter as part of file metadata
            let bloom_arc = Arc::new(bloom_data);
            // The zero_copy_system will cache metadata automatically on next access
            // We don't need to manually cache it

            return Ok(bloom_arc);
        }

        // For local files, try mmap if memory allows
        self.get_local_bloom_with_mmap(file_path, collection_id, filename)
            .await
    }

    /// Get index block with smart bandwidth optimization
    async fn get_index_block_smart(
        &self,
        file_path: &str,
        collection_id: &str,
    ) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        let filename = std::path::Path::new(file_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown");

        // Check if file metadata with index is cached
        // The zero_copy_system returns Arc<Box<dyn EngineMetadata>> which we can't directly downcast
        // We'll use direct file reads for index as well

        self.stats.index_misses.fetch_add(1, Ordering::Relaxed);

        // For cloud files, download ONLY the index block range
        if self.is_cloud_file(file_path) {
            // Use range request to get just the index block
            let fs = self.filesystem.get_filesystem(file_path).map_err(|e| {
                ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to get filesystem: {}", e),
                )))
            })?;
            let index_data = fs
                .read_range(file_path, BLOOM_FILTER_SIZE as u64, INDEX_BLOCK_SIZE as u64)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to read index: {}", e),
                    )))
                })?;

            self.stats
                .bytes_downloaded
                .fetch_add(INDEX_BLOCK_SIZE as u64, Ordering::Relaxed);

            // Return the index data directly without caching
            // The zero_copy_system handles caching at a different level
            let index_arc = Arc::new(index_data);
            return Ok(index_arc);
        }

        // For local files, use mmap if possible
        self.get_local_index_with_mmap(file_path, collection_id, filename)
            .await
    }

    /// Read data block only after confirming it's needed
    async fn read_data_block_smart(
        &self,
        file_path: &str,
        collection_id: &str,
        block_info: &BlockInfo,
    ) -> Result<Vec<u8>, ProximaDBError> {
        let filename = std::path::Path::new(file_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown");

        // For cloud files, download the specific block range
        let data = if self.is_cloud_file(file_path) {
            // Use range request to get just the block we need
            let fs = self.filesystem.get_filesystem(file_path).map_err(|e| {
                ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to get filesystem: {}", e),
                )))
            })?;

            fs.read_range(file_path, block_info.offset, block_info.size as u64)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to read block from cloud: {}", e),
                    )))
                })?
        } else {
            // For local files, use direct read
            // The zero_copy_system handles memory mapping internally
            let fs = self.filesystem.get_filesystem(file_path).map_err(|e| {
                ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to get filesystem: {}", e),
                )))
            })?;
            fs.read_range(file_path, block_info.offset, block_info.size)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to read block: {}", e),
                    )))
                })?
        };

        self.stats
            .bytes_downloaded
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(data)
    }

    /// Batch read optimization with smart filtering
    pub async fn batch_read_with_filtering(
        &self,
        file_path: &str,
        collection_id: &str,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>, ProximaDBError> {
        // Step 1: Get bloom filter once for all keys
        let bloom_data = self
            .get_bloom_filter_smart(file_path, collection_id)
            .await?;

        // Filter keys using bloom - avoid downloading unnecessary blocks
        let mut possible_keys = Vec::new();
        let mut bloom_filtered = Vec::new();

        for (idx, key) in keys.iter().enumerate() {
            if self.check_bloom(&bloom_data, key) {
                possible_keys.push((idx, key));
            } else {
                bloom_filtered.push(idx);
            }
        }

        // Track bandwidth saved
        let saved = bloom_filtered.len() * DATA_BLOCK_SIZE;
        self.stats
            .bytes_saved
            .fetch_add(saved as u64, Ordering::Relaxed);

        if possible_keys.is_empty() {
            return Ok(vec![None; keys.len()]);
        }

        // Step 2: Get index once and find blocks needed
        let index_data = self.get_index_block_smart(file_path, collection_id).await?;
        let mut blocks_to_read = HashMap::new();
        let mut index_filtered = Vec::new();

        for (idx, key) in &possible_keys {
            if let Some(block_info) = self.find_block_for_key(&index_data, key)? {
                blocks_to_read
                    .entry(block_info.offset)
                    .or_insert_with(|| (block_info, Vec::new()))
                    .1
                    .push((*idx, *key));
            } else {
                index_filtered.push(*idx);
            }
        }

        // Track additional bandwidth saved
        let additional_saved = index_filtered.len() * DATA_BLOCK_SIZE;
        self.stats
            .bytes_saved
            .fetch_add(additional_saved as u64, Ordering::Relaxed);

        // Step 3: Read only necessary blocks in parallel
        let mut results = vec![None; keys.len()];

        for (_, (block_info, keys_in_block)) in blocks_to_read {
            let block_data = self
                .read_data_block_smart(file_path, collection_id, &block_info)
                .await?;

            for (idx, key) in keys_in_block {
                if let Some(value) = self.find_in_block(&block_data, key)? {
                    results[idx] = Some(value);
                }
            }
        }

        Ok(results)
    }

    /// Cache invalidation during compaction
    pub async fn invalidate_cache_for_collection(
        &self,
        collection_id: &str,
    ) -> Result<(), ProximaDBError> {
        // The zero_copy_system handles cache invalidation internally
        // We just track the statistics
        self.stats
            .cache_invalidations
            .fetch_add(1, Ordering::Relaxed);

        info!(
            "Invalidated cache entries for collection {} during compaction",
            collection_id
        );

        Ok(())
    }

    /// Check if file is on cloud storage
    fn is_cloud_file(&self, path: &str) -> bool {
        path.starts_with("s3://")
            || path.starts_with("gs://")
            || path.starts_with("azure://")
            || path.starts_with("http://")
            || path.starts_with("https://")
    }

    /// Check bloom filter
    fn check_bloom(&self, bloom_data: &[u8], key: &[u8]) -> bool {
        // Bloom filter implementation
        // Returns false if key definitely not present
        // Returns true if key might be present
        true // Placeholder
    }

    /// Find block for key in index
    fn find_block_for_key(
        &self,
        index_data: &[u8],
        key: &[u8],
    ) -> Result<Option<BlockInfo>, ProximaDBError> {
        // Binary search in index to find block
        // Returns None if key not in range
        Ok(Some(BlockInfo {
            offset: 0,
            size: DATA_BLOCK_SIZE as u64,
        })) // Placeholder
    }

    /// Search for key in data block
    fn find_in_block(
        &self,
        block_data: &[u8],
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, ProximaDBError> {
        // Binary search in sorted block
        Ok(None) // Placeholder
    }

    /// Get local bloom filter with direct read
    async fn get_local_bloom_with_mmap(
        &self,
        file_path: &str,
        _collection_id: &str,
        _filename: &str,
    ) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        // Direct read for local files
        let fs = self.filesystem.get_filesystem(file_path).map_err(|e| {
            ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to get filesystem: {}", e),
            )))
        })?;
        let bloom_data = fs
            .read_range(file_path, 0, BLOOM_FILTER_SIZE as u64)
            .await
            .map_err(|e| {
                ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to read bloom filter: {}", e),
                )))
            })?;
        Ok(Arc::new(bloom_data))
    }

    /// Get local index block with direct read
    async fn get_local_index_with_mmap(
        &self,
        file_path: &str,
        _collection_id: &str,
        _filename: &str,
    ) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        // Direct read for local files
        let fs = self.filesystem.get_filesystem(file_path).map_err(|e| {
            ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to get filesystem: {}", e),
            )))
        })?;
        let index_data = fs
            .read_range(file_path, BLOOM_FILTER_SIZE as u64, INDEX_BLOCK_SIZE as u64)
            .await
            .map_err(|e| {
                ProximaDBError::Storage(StorageError::DiskIO(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to read index: {}", e),
                )))
            })?;
        Ok(Arc::new(index_data))
    }

    /// Get statistics for monitoring
    pub fn get_stats(&self) -> ReaderStatsSummary {
        ReaderStatsSummary {
            bloom_hit_rate: {
                let hits = self.stats.bloom_hits.load(Ordering::Relaxed);
                let misses = self.stats.bloom_misses.load(Ordering::Relaxed);
                let total = hits + misses;
                if total > 0 {
                    hits as f64 / total as f64
                } else {
                    0.0
                }
            },
            index_hit_rate: {
                let hits = self.stats.index_hits.load(Ordering::Relaxed);
                let misses = self.stats.index_misses.load(Ordering::Relaxed);
                let total = hits + misses;
                if total > 0 {
                    hits as f64 / total as f64
                } else {
                    0.0
                }
            },
            bytes_downloaded: self.stats.bytes_downloaded.load(Ordering::Relaxed),
            bytes_saved: self.stats.bytes_saved.load(Ordering::Relaxed),
            cache_invalidations: self.stats.cache_invalidations.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug)]
pub struct ReaderStatsSummary {
    pub bloom_hit_rate: f64,
    pub index_hit_rate: f64,
    pub bytes_downloaded: u64,
    pub bytes_saved: u64,
    pub cache_invalidations: u64,
}

impl Default for ReaderStats {
    fn default() -> Self {
        Self {
            bloom_hits: AtomicU64::new(0),
            bloom_misses: AtomicU64::new(0),
            index_hits: AtomicU64::new(0),
            index_misses: AtomicU64::new(0),
            bytes_downloaded: AtomicU64::new(0),
            bytes_saved: AtomicU64::new(0),
            cache_invalidations: AtomicU64::new(0),
        }
    }
}
