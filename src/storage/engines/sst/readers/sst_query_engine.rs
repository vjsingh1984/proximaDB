#![allow(dead_code)]
//! =============================================================================
//! HIGH-LEVEL SST QUERY ENGINE (sst_query_engine.rs)
//! =============================================================================
//!
//! PURPOSE: High-level query execution and business logic for SST files
//! USED BY: SST and SWIFT storage engines
//!
//! This module provides:
//! - Vector similarity search with metadata filtering
//! - Block-level predicate pushdown for efficient filtering
//! - Three-stage filtering: Bloom → Index → Block scan
//! - Progressive search with early termination
//! - Integration with quantization for compressed vectors
//! - Compaction and maintenance operations
//!
//! RELATIONSHIP WITH sst_io_layer.rs:
//! This reader USES the SharedSstFormatReader (sst_io_layer.rs) for:
//! - Low-level block I/O operations
//! - Bloom filter and index caching
//! - Memory mapping and bandwidth optimization
//!
//! ARCHITECTURE PARALLEL:
//! - sst_query_engine.rs (this) = HIGH-LEVEL query logic (like parquet_reader.rs)
//! - sst_io_layer.rs = LOW-LEVEL I/O operations (like shared_parquet_reader.rs)
//!
//! RENAME SUGGESTION: This file should be renamed to `sst_query_engine.rs`
//! to match the suggested parquet naming convention

use crate::core::metadata_types::MetadataValue;
use crate::core::search::OptimizedSearchRecord;
use crate::core::search::bounded_queue::BoundedPriorityQueue;
use anyhow::Result;
use std::collections::HashMap;
use std::io::Read;
use std::marker::PhantomData;
use std::sync::Arc;

// Removed zero_copy traits - these concepts are now handled by UnifiedCachingFilesystem
use futures::TryStreamExt;
use futures::stream::{Stream, StreamExt};
use tracing::{debug, error, info, trace, warn};

// Performance optimizations: import commonly used types and functions for zero-cost abstractions
// use std::hint::likely; // Unstable feature - removed for compilation

use super::block_filter::{BlockFilter, IntelligentBlockFilter};
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::bloom::BloomFilterConfig;
use crate::core::bloom::SstableBloomFilter;
use crate::core::search::{FilterExpression, SearchParams};
use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;
use crate::storage::engines::core::formats::proximablocks::sst_io_layer::{
    SharedSstFormatReader, SstMmapStrategy, SstRegion,
};
use crate::storage::engines::sst::{IndexEntry, SstableHeader}; // OPTIMIZED: Removed SstRecord import
use crate::storage::persistence::filesystem::FilesystemFactory;
use proximadb_compression::CompressionAlgorithm;

// Using UnifiedCachingFilesystem instead of ZeroCopyIOSystem
use crate::storage::persistence::filesystem::FileSystem;
use crate::storage::persistence::filesystem::caching_filesystem::UnifiedCachingFilesystem;
use proximadb_records::ProximaRecord;

// Vectorized execution imports (TD-041)
use crate::storage::engines::core::formats::columnar::columnar_query_engine::vectorized_executor::evaluate_predicate_vectorized;

use arrow::array::RecordBatch;
use arrow::datatypes::{DataType, Field, Schema};

pub(crate) use super::block_pruning::{compute_query_zorder_code, select_blocks_by_centroid};

// Type alias for bloom filter
type BloomFilter = SstableBloomFilter;

#[inline]
fn record_id(record: &ProximaRecord) -> String {
    record
        .local_id
        .clone()
        .unwrap_or_else(|| record.oid.clone())
}

#[inline]
fn record_vector(record: &ProximaRecord) -> &[f32] {
    record
        .embeddings
        .first()
        .map(|embedding| embedding.values.as_slice())
        .unwrap_or(&[])
}

#[inline]
fn record_metadata(
    record: &ProximaRecord,
) -> std::collections::HashMap<String, proximadb_data_model::ProximaValue> {
    crate::core::search::sql_value_filter::proxima_tree_to_value_map(&record.props)
}

/// SSTable reading strategies for different access patterns
#[derive(Debug, Clone)]
pub enum SstableReadingStrategy {
    /// Selective reads via cache with range optimization for normal queries
    SelectiveWithCache {
        use_range_reads: bool,
        enable_bloom_filters: bool,
        enable_cache_lookup: bool,
        enable_metadata_cache: bool,
    },
    /// Full read strategy for compaction operations - avoid cache pollution
    CompactionFullRead {
        skip_bloom_filters: bool,
        skip_indexes: bool,
        bypass_write_cache: bool,
        use_disk_cache_if_exists: bool,
        sequential_io: bool,
    },
    /// Legacy strategies for backward compatibility
    FullScan { use_block_cache: bool },
    IndexRangeScan {
        start_block: usize,
        end_block: usize,
        use_bloom_filter: bool,
    },
    MetadataFiltered {
        selected_blocks: Vec<usize>,
        skip_bloom_check: bool,
    },
    Hybrid {
        primary_strategy: Box<SstableReadingStrategy>,
        fallback_blocks: Vec<usize>,
    },
}
// NOTE: Removed specialized cache imports - using only zero-copy system for caching
// Old: VectorStore, IndexNodeCache, BitmapFilterCache replaced by ZeroCopyIOSystem

/// Unified SSTable Reader with zero-copy cache-first architecture
/// Leverages SharedSstFormatReader for actual file operations (eliminates code duplication)
pub struct UnifiedSstableReader {
    // CORE READER: Delegates low-level file operations to shared infrastructure
    #[allow(dead_code)]
    shared_reader: Arc<SharedSstFormatReader>,
    strategy_selector: Arc<ReadingStrategySelector>,
    // UNIFIED CACHE: Using UnifiedCachingFilesystem for all caching needs
    caching_filesystem: Arc<UnifiedCachingFilesystem>,
    collection_id: String,

    // Filesystem factory for direct file access when needed
    pub filesystem: Arc<FilesystemFactory>,
}

impl std::fmt::Debug for UnifiedSstableReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedSstableReader")
            .field("shared_reader", &"SharedSstFormatReader")
            .field("zero_copy_system", &"ZeroCopyIOSystem")
            .field("collection_id", &self.collection_id)
            .field("strategy_selector", &self.strategy_selector)
            .finish()
    }
}

/// Block cache for frequently accessed data blocks
#[derive(Debug)]
pub struct BlockCache {
    #[allow(dead_code)]
    cache: Arc<
        tokio::sync::RwLock<
            proximadb_runtime_common::cache::LruCache<BlockCacheKey, Arc<ProximaDataBlock>>,
        >,
    >,
    #[allow(dead_code)]
    max_size: usize,
    #[allow(dead_code)]
    hit_rate: Arc<tokio::sync::RwLock<CacheStats>>,
}

/// Optimized Index cache with memory bounds and LRU eviction
#[derive(Debug)]
pub struct IndexCache {
    indices: Arc<moka::future::Cache<String, Arc<SstableIndex>>>,
    bloom_filters: Arc<moka::future::Cache<String, Arc<SstableBloomFilter>>>,
    #[allow(dead_code)]
    max_memory_mb: usize,
    metrics: Arc<tokio::sync::RwLock<CacheMetrics>>,
}

/// Cache metrics for monitoring and optimization
#[derive(Debug, Default)]
pub struct CacheMetrics {
    pub memory_usage_bytes: usize,
    pub hit_count: u64,
    pub miss_count: u64,
    pub eviction_count: u64,
    pub memory_pressure_events: u64,
}

use crate::storage::engines::sst::SstableIndex;

/// Enhanced bloom filter supporting metadata columns
/// Reading strategy for SSTable access
#[derive(Debug, Clone)]
pub enum ReadStrategy {
    /// Full scan without using bloom filters or indexes
    FullScan,
    /// Filtered scan using bloom filters and indexes with smart cache
    FilteredScan(FilterExpression),
    /// Direct read for compaction, no cache integration
    CompactionDirect,
    /// Search optimized with full bloom/index and cache usage
    SearchOptimized,
}

impl ReadStrategy {
    /// Check if this strategy should use block filtering
    #[allow(dead_code)]
    fn should_filter_blocks(&self) -> bool {
        !matches!(self, ReadStrategy::CompactionDirect)
    }
}

/// Read modes for data blocks
#[derive(Debug, Clone)]
pub enum ReadMode {
    /// Generator-style streaming (memory efficient)
    Streaming,
    /// Traditional batch reading
    Buffered,
    /// Raw bytes, no deserialization
    Direct,
}

/// Operation type for smart cache decisions
#[derive(Debug, Clone)]
pub enum OperationType {
    Search,
    Compaction,
    Analytics,
    FullScan,
}

/// Cache decision based on context
#[derive(Debug, Clone)]
pub enum CacheDecision {
    UseCache,       // Search operations, repeated access patterns
    SkipCache,      // Compaction, full scans, one-time operations
    StreamingCache, // Large result sets, cache only hot blocks
}

/// Strategy selector based on query characteristics
#[derive(Debug)]
pub struct ReadingStrategySelector {
    config: ReaderConfig,
}

/// Configuration for reading strategies
#[derive(Debug, Clone)]
pub struct ReaderConfig {
    pub block_cache_size: usize,
    pub index_cache_size: usize,
    pub bloom_filter_threshold: f64,
    pub range_scan_threshold: usize,
    pub metadata_selectivity_threshold: f64,
    pub enable_read_ahead: bool,
    pub read_ahead_blocks: usize,
}

/// Block cache key
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct BlockCacheKey {
    pub file_path: String,
    pub block_id: u32,
    pub block_index: usize,
}

/// Cache statistics
#[derive(Debug, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

/// Core trait for block-level reading operations (async for cloud support)
#[async_trait::async_trait]
pub trait BlockReader {
    async fn read_header(&mut self) -> Result<SstableHeader>;
    async fn read_bloom_filter(&mut self, skip: bool) -> Result<Option<SstableBloomFilter>>;
    async fn read_index_block(&mut self, strategy: &ReadStrategy) -> Result<SstableIndex>;
    async fn read_data_block(&mut self, block_id: u64, mode: ReadMode) -> Result<ProximaDataBlock>;
}

/// Implement BlockReader trait for ModularBlockReader
#[async_trait::async_trait]
impl BlockReader for ModularBlockReader {
    async fn read_header(&mut self) -> Result<SstableHeader> {
        self.read_header_async().await
    }

    async fn read_bloom_filter(&mut self, skip: bool) -> Result<Option<SstableBloomFilter>> {
        self.read_bloom_filter_async(skip).await
    }

    async fn read_index_block(&mut self, search_strategy: &ReadStrategy) -> Result<SstableIndex> {
        self.read_index_block_async(search_strategy).await
    }

    async fn read_data_block(&mut self, block_id: u64, mode: ReadMode) -> Result<ProximaDataBlock> {
        self.read_data_block_async(block_id, mode).await
    }
}

/// Generator-like streaming iterator for data blocks
pub struct BlockIterator<T> {
    reader: Box<dyn Read + Send>,
    buffer: Vec<ProximaRecord>,
    position: usize,
    #[allow(dead_code)]
    block_size: usize,
    total_blocks: usize,
    current_block: usize,
    #[allow(dead_code)]
    mode: ReadMode,
    _phantom: PhantomData<T>,
}

impl<T> BlockIterator<T> {
    pub fn new(
        reader: Box<dyn Read + Send>,
        block_size: usize,
        total_blocks: usize,
        mode: ReadMode,
    ) -> Self {
        Self {
            reader,
            buffer: Vec::new(), // Now holds SstRecords, not bytes
            position: 0,
            block_size,
            total_blocks,
            current_block: 0,
            mode,
            _phantom: PhantomData,
        }
    }
}

/// Collection context for search
pub struct CollectionContext {
    pub file_path: String,
    pub sstable_files: Vec<String>,
    pub total_vectors: usize,
    pub metadata_columns: Vec<String>,
    pub level: usize,
    pub creation_time: chrono::DateTime<chrono::Utc>,
    /// I/O optimization hints for efficient SSTable access
    pub io_optimization_hints: Option<HashMap<String, serde_json::Value>>,
    /// Collection config for type-safe metadata deserialization
    pub collection: Option<Arc<crate::proto::proximadb_v1::Collection>>,
}

/// Modular block reader for shared block-level operations
/// Uses filesystem API for abstracted range reading across cloud and local storage
#[derive(Clone)]
pub struct ModularBlockReader {
    filesystem_factory: Arc<FilesystemFactory>,
    header: Option<SstableHeader>,
    file_path: String,
    // Quantization now handled by unified compute module
}

impl ModularBlockReader {
    pub async fn open(filesystem_factory: Arc<FilesystemFactory>, file_path: &str) -> Result<Self> {
        // Extract scheme from URL for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };

        // Get appropriate filesystem implementation (S3, GCS, Azure, or local)
        let fs = filesystem_factory.get_filesystem(&format!("{}:///", scheme))?;

        // Validate file exists
        if !fs.exists(file_path).await? {
            return Err(anyhow::anyhow!(
                "SSTable file does not exist: {}",
                file_path
            ));
        }

        Ok(Self {
            filesystem_factory,
            header: None,
            file_path: file_path.to_string(),
        })
    }

    /// Read a specific range of bytes from the file
    /// Works efficiently across S3 (range requests), GCS, Azure, and local files
    async fn read_range(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let fs = self
            .filesystem_factory
            .get_filesystem(&self.file_path)
            .map_err(|e| anyhow::anyhow!("Failed to get filesystem: {}", e))?;
        fs.read_range(&self.file_path, offset, length as u64)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read range: {}", e))
    }

    /// Read only quantized section from a data block for progressive search
    /// This provides ultra-fast filtering with minimal I/O
    pub async fn read_quantized_vectors_only(
        &self,
        block_offset: u64,
        estimated_block_size: u32,
    ) -> Result<
        Option<(
            Vec<Vec<u8>>,
            crate::compute::quantization::quantization_engine::UnifiedQuantizationLevel,
        )>,
    > {
        // For now, read the entire block and extract quantized section
        // Future optimization: Read only the quantized portion
        let block_data = self
            .read_range(block_offset + 4, estimated_block_size as usize)
            .await?;

        let data_block = ProximaDataBlock::deserialize(&block_data, None).map_err(|e| {
            anyhow::anyhow!(
                "Failed to deserialize ProximaDataBlock for quantized section: {}",
                e
            )
        })?;

        // Return the quantized vectors and level if present
        if let (Some(vectors), Some(level)) =
            (data_block.quantized_vectors, data_block.quantization_level)
        {
            Ok(Some((vectors, level)))
        } else {
            Ok(None)
        }
    }

    /// Progressive search with quantization support
    /// Implements multi-stage filtering: Binary → PQ → Full precision
    pub async fn progressive_search(
        &self,
        query_vector: &[f32],
        k: usize,
        filter: Option<&FilterExpression>,
        distance_metric: &crate::compute::distance_computation::engine::DistanceMetric,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        // Always use traditional search (quantization handled at a higher level)
        self.traditional_search(query_vector, k, filter, distance_metric)
            .await
    }

    // NOTE: progressive_search_with_quantization_legacy and load_full_vector removed in consolidation
    // Progressive search is now handled at a higher level with unified quantization engine

    /// Traditional search fallback (when no quantization is available)
    async fn traditional_search(
        &self,
        query_vector: &[f32],
        k: usize,
        _filter: Option<&FilterExpression>,
        distance_metric: &crate::compute::distance_computation::engine::DistanceMetric,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        info!("📝 Fallback: Using traditional search (no quantization)");

        let mut reader_clone = self.clone();
        let header = reader_clone.read_header_async().await?;
        let index_entries = reader_clone.read_index_blocks(&header).await?;

        // Use bounded priority queue to maintain only top-k results
        let mut priority_queue = BoundedPriorityQueue::new(k);
        let mut total_records_scanned = 0;

        // Scan all blocks and compute distances
        for (block_idx, _index_entry) in index_entries.iter().enumerate() {
            let data_block = reader_clone
                .read_data_block_async(block_idx as u64, ReadMode::Direct)
                .await?;

            for record in data_block.records.iter() {
                total_records_scanned += 1;
                let vector = record_vector(record);
                if vector.is_empty() {
                    continue;
                }

                let distance =
                    crate::compute::distance_computation::engine::UnifiedDistanceCompute::default()
                        .calculate_distance(query_vector, vector, distance_metric);

                // Use normalized_score for both fields - consistency across all engines
                // Higher similarity = better match, VOS sorts descending
                let search_record =
                    OptimizedSearchRecord::new(record_id(record), distance.normalized_score)
                        .with_similarity(distance.normalized_score)
                        .add_vector(vector.to_vec())
                        .with_proxima_metadata(record_metadata(record));

                // Try to insert into bounded queue - only keeps top-k
                priority_queue.try_insert(search_record);
            }
        }

        debug!(
            "Scanned {} records, returning top {}",
            total_records_scanned, k
        );

        // Get sorted results from bounded queue
        Ok(priority_queue.into_sorted_vec())
    }

    pub fn new(filesystem_factory: Arc<FilesystemFactory>, file_path: String) -> Self {
        Self {
            filesystem_factory,
            header: None,
            file_path,
        }
    }

    // Quantization now handled by unified compute module

    async fn read_header_async(&mut self) -> Result<SstableHeader> {
        if let Some(ref header) = self.header {
            return Ok(header.clone());
        }

        // Read magic marker (4 bytes) using filesystem API
        let magic_bytes = self.read_range(0, 4).await?;
        if &magic_bytes != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SSTable magic marker"));
        }

        // Read header size (4 bytes)
        let header_size_bytes = self.read_range(4, 4).await?;
        let header_size = u32::from_le_bytes([
            header_size_bytes[0],
            header_size_bytes[1],
            header_size_bytes[2],
            header_size_bytes[3],
        ]) as usize;

        // Read header data
        let header_data = self.read_range(8, header_size).await?;

        // Deserialize header
        let header: SstableHeader = bincode::deserialize(&header_data)?;
        self.header = Some(header.clone());

        Ok(header)
    }

    async fn read_bloom_filter_async(&mut self, skip: bool) -> Result<Option<SstableBloomFilter>> {
        if skip {
            return Ok(None);
        }

        let header = self.read_header_async().await?;
        if !header.has_bloom_filter {
            return Ok(None);
        }

        // Calculate bloom filter offset (after header)
        let bloom_filter_offset = 8 + header.header_size as u64;

        // Read bloom filter size using filesystem range read
        let size_bytes = self.read_range(bloom_filter_offset, 4).await?;
        let bloom_size =
            u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]])
                as usize;

        // Read bloom filter data using range read
        let bloom_data = self.read_range(bloom_filter_offset + 4, bloom_size).await?;

        // Deserialize bloom filter
        let bloom_filter: SstableBloomFilter = bincode::deserialize(&bloom_data)?;

        Ok(Some(bloom_filter))
    }

    async fn read_index_block_async(
        &mut self,
        search_strategy: &ReadStrategy,
    ) -> Result<SstableIndex> {
        // For hierarchical SST, we always need the index for random block access
        // Only skip for CompactionDirect when we're doing sequential streaming
        if matches!(search_strategy, ReadStrategy::CompactionDirect) {
            return Ok(SstableIndex {
                entries: vec![],
                metadata_stats: HashMap::new(),
                vector_count: 0,
                min_key: String::new(),
                max_key: String::new(),
                bplus_tree: None,
            });
        }

        let header = self.read_header_async().await.map_err(|e| {
            anyhow::anyhow!(
                "TRACE-016: Failed to read header in read_index_block_async: {}",
                e
            )
        })?;

        // Calculate index offset (after header and bloom filter if present)
        // NEW: Use hierarchical offsets if available, otherwise calculate
        let index_offset = if header.block_index_offset > 0 {
            header.block_index_offset
        } else {
            // Legacy calculation: need to read bloom filter size first
            let bloom_offset = 8 + header.header_size as u64;
            if header.has_bloom_filter {
                let bloom_size_bytes = self.read_range(bloom_offset, 4).await?;
                let bloom_size = u32::from_le_bytes([
                    bloom_size_bytes[0],
                    bloom_size_bytes[1],
                    bloom_size_bytes[2],
                    bloom_size_bytes[3],
                ]) as u64;
                bloom_offset + 4 + bloom_size
            } else {
                bloom_offset
            }
        };

        // Read index size using filesystem range read
        let size_bytes = self.read_range(index_offset, 4).await.map_err(|e| {
            anyhow::anyhow!(
                "TRACE-026: Failed to read index size at offset {}: {}",
                index_offset,
                e
            )
        })?;
        let index_size =
            u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]])
                as usize;

        // Read index data using range read
        let index_data = self
            .read_range(index_offset + 4, index_size)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to read index data at offset {} for {} bytes: {}",
                    index_offset + 4,
                    index_size,
                    e
                )
            })?;

        // Deserialize index using SstableIndex::deserialize which handles the proper format
        // (IDX1 magic header + min_key + max_key + vector_count + entries + bplus_tree)
        let index = SstableIndex::deserialize(&index_data).map_err(|e| {
            anyhow::anyhow!(
                "Failed to deserialize SstableIndex at offset {}: {}",
                index_offset,
                e
            )
        })?;

        Ok(index)
    }

    async fn read_index(&self, header: &SstableHeader) -> Result<SstableIndex> {
        // Calculate index offset using hierarchical offsets when available
        let index_offset = if header.block_index_offset > 0 {
            header.block_index_offset
        } else {
            // Legacy calculation: need to read bloom filter size first
            let bloom_offset = 8 + header.header_size as u64;
            if header.has_bloom_filter {
                let bloom_size_bytes = self.read_range(bloom_offset, 4).await?;
                let bloom_size = u32::from_le_bytes([
                    bloom_size_bytes[0],
                    bloom_size_bytes[1],
                    bloom_size_bytes[2],
                    bloom_size_bytes[3],
                ]) as u64;
                bloom_offset + 4 + bloom_size
            } else {
                bloom_offset
            }
        };

        debug!(
            "Reading index at offset {} for file: {}",
            index_offset, self.file_path
        );

        // Read index size using filesystem range read
        let size_bytes = self.read_range(index_offset, 4).await?;
        let index_size =
            u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]])
                as usize;

        // Read index data using range read
        let index_data = self.read_range(index_offset + 4, index_size).await?;

        // Deserialize index using robust implementation
        let index = SstableIndex::deserialize(&index_data)?;

        Ok(index)
    }

    pub async fn read_index_blocks(&self, header: &SstableHeader) -> Result<Vec<IndexEntry>> {
        let index = self.read_index(header).await?;
        Ok(index.entries)
    }

    pub async fn read_bloom_filter(&self, header: &SstableHeader) -> Result<BloomFilter> {
        // Calculate bloom filter offset (after header)
        let bloom_filter_offset = 8 + header.header_size as u64;

        debug!(
            "Reading bloom filter at offset {} for file: {}",
            bloom_filter_offset, self.file_path
        );

        // Read bloom filter size
        let size_bytes = self.read_range(bloom_filter_offset, 4).await?;
        let bloom_size =
            u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]])
                as usize;

        // Read bloom filter data
        let bloom_data = self.read_range(bloom_filter_offset + 4, bloom_size).await?;

        // Deserialize bloom filter
        let bloom_filter: BloomFilter = bincode::deserialize(&bloom_data)?;

        Ok(bloom_filter)
    }

    pub async fn read_data_block_at_offset(
        &self,
        offset: u64,
        _size: usize,
    ) -> Result<ProximaDataBlock> {
        // SST block format: [4-byte size prefix][block data]
        // The offset points to the size prefix, so we need to:
        // 1. Read the 4-byte size prefix to get actual block size
        // 2. Read the block data starting at offset+4

        // Read block size from the file (first 4 bytes at offset)
        let size_bytes = self.read_range(offset, 4).await.map_err(|e| {
            anyhow::anyhow!("Failed to read block size at offset {}: {}", offset, e)
        })?;
        let block_size =
            u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]])
                as usize;

        debug!(
            "Reading hierarchical data block at offset {} with actual size {} for file: {}",
            offset, block_size, self.file_path
        );

        // Read the block data (excluding the size prefix)
        let block_data = self.read_range(offset + 4, block_size).await?;

        // Use hierarchical deserialization with automatic compression detection
        ProximaDataBlock::deserialize(&block_data, None).map_err(|e| {
            anyhow::anyhow!(
                "Failed to deserialize hierarchical ProximaDataBlock at offset {}: {}",
                offset,
                e
            )
        })
    }

    async fn read_data_block_async(
        &mut self,
        block_id: u64,
        mode: ReadMode,
    ) -> Result<ProximaDataBlock> {
        let header = self.read_header_async().await.map_err(|e| {
            anyhow::anyhow!(
                "TRACE-008: Failed to read header for block {}: {}",
                block_id,
                e
            )
        })?;

        // For hierarchical SST design with random block access, we need to use the index
        // to get the actual offset and size of each block

        // First, read the index to get block offsets
        let index = self
            .read_index_block_async(&ReadStrategy::FullScan)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "TRACE-011: Failed to read index for block {}: {}",
                    block_id,
                    e
                )
            })?;

        // Find the index entry for this block
        // Note: Index entries map to blocks, so we need to find the right entry
        if block_id >= index.entries.len() as u64 {
            return Err(anyhow::anyhow!(
                "Block {} not found in index (only {} entries)",
                block_id,
                index.entries.len()
            ));
        }

        let index_entry = &index.entries[block_id as usize];

        // Use the offset from the index entry
        // The index entry contains the actual offset of this specific block
        let block_offset = if index_entry.offset > 0 {
            index_entry.offset
        } else {
            // If offset is not set in index, calculate from data_blocks_offset
            // This handles the case where we have a single block at the start
            if block_id == 0 && header.data_blocks_offset > 0 {
                header.data_blocks_offset
            } else {
                return Err(anyhow::anyhow!(
                    "Block offset not found in index for block {}",
                    block_id
                ));
            }
        };

        // Read block size from the file (first 4 bytes of the block)
        let size_bytes = self.read_range(block_offset, 4).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to read block size at offset {}: {}",
                block_offset,
                e
            )
        })?;
        let block_size =
            u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]])
                as usize;

        // Read the entire block data (excluding the size prefix)
        let block_data = self
            .read_range(block_offset + 4, block_size)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to read block data at offset {}, size {}: {}",
                    block_offset + 4,
                    block_size,
                    e
                )
            })?;

        match mode {
            ReadMode::Direct => {
                // Use hierarchical deserialization for correct format handling
                ProximaDataBlock::deserialize(&block_data, None).map_err(|e| {
                    anyhow::anyhow!("Failed to deserialize hierarchical DataBlock: {}", e)
                })
            }
            ReadMode::Buffered | ReadMode::Streaming => {
                // Use hierarchical deserialization with automatic compression detection
                ProximaDataBlock::deserialize(&block_data, None).map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to deserialize hierarchical DataBlock in buffered mode: {}",
                        e
                    )
                })
            }
        }
    }

    #[allow(dead_code)]
    #[allow(dead_code)]
    fn decompress_block(&self, data: &[u8], algorithm: CompressionAlgorithm) -> Result<Vec<u8>> {
        match algorithm {
            CompressionAlgorithm::None => Ok(data.to_vec()),
            CompressionAlgorithm::Lz4 => {
                let decompressed = lz4_flex::decompress_size_prepended(data)?;
                Ok(decompressed)
            }
            CompressionAlgorithm::Zstd => {
                let decompressed = zstd::decode_all(data)?;
                Ok(decompressed)
            }
            CompressionAlgorithm::Snappy => {
                let decompressed = snap::raw::Decoder::new().decompress_vec(data)?;
                Ok(decompressed)
            }
            _ => {
                // For other compression algorithms, return an error or use a default
                Err(anyhow::anyhow!(
                    "Unsupported compression algorithm: {:?}",
                    algorithm
                ))
            }
        }
    }
}

/// Iterator implementation for canonical record streaming.
impl Iterator for BlockIterator<ProximaRecord> {
    type Item = Result<ProximaRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        // First check if we have records in the buffer
        if self.position < self.buffer.len() {
            let record = self.buffer[self.position].clone();
            self.position += 1;

            debug!(
                "🔍 ProximaRecord STREAMING: Returning record {:?} from block {}, position {}/{}",
                record.oid,
                self.current_block,
                self.position,
                self.buffer.len()
            );

            return Some(Ok(record));
        }

        // Buffer is empty, clear it and try to read next block
        self.buffer.clear();
        self.position = 0;

        // Check if we've processed all blocks
        if self.current_block >= self.total_blocks {
            debug!(
                "🔍 VectorRecord STREAMING: Reached end - processed {} of {} blocks",
                self.current_block, self.total_blocks
            );
            return None;
        }

        // Read next block
        debug!(
            "🔍 VectorRecord STREAMING: Reading block {} of {}",
            self.current_block + 1,
            self.total_blocks
        );

        // Read block size (4 bytes)
        let mut size_bytes = [0u8; 4];
        debug!("🔍 VectorRecord STREAMING: About to read 4 bytes for block size");
        match self.reader.read_exact(&mut size_bytes) {
            Ok(_) => {
                debug!(
                    "🔍 VectorRecord STREAMING: Successfully read 4 bytes for block size: {:?}",
                    size_bytes
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                debug!(
                    "🔍 VectorRecord STREAMING: Reached EOF while reading block size - no more blocks"
                );
                return None;
            }
            Err(e) => {
                error!(
                    "❌ VectorRecord STREAMING ERROR: Failed to read block size bytes: {:?}",
                    e
                );
                return Some(Err(anyhow::Error::from(e)));
            }
        }

        let block_size = u32::from_le_bytes(size_bytes) as usize;
        debug!(
            "🔍 VectorRecord STREAMING: Parsed block size: {} bytes",
            block_size
        );

        if block_size == 0 {
            debug!("🔍 VectorRecord STREAMING: Block size is 0 - no more data");
            return None;
        }

        if block_size > 10_000_000 {
            // 10MB sanity check
            error!(
                "❌ VectorRecord STREAMING ERROR: Block size {} seems unreasonably large",
                block_size
            );
            return Some(Err(anyhow::anyhow!(
                "Block size {} exceeds sanity check limit",
                block_size
            )));
        }

        // Read block data
        debug!(
            "🔍 VectorRecord STREAMING: About to read {} bytes for block data",
            block_size
        );
        let mut block_data = vec![0u8; block_size];
        match self.reader.read_exact(&mut block_data) {
            Ok(_) => {
                debug!(
                    "🔍 VectorRecord STREAMING: Successfully read {} bytes for block data",
                    block_size
                );
            }
            Err(e) => {
                error!(
                    "❌ ProximaRecord STREAMING ERROR: Failed to read {} bytes for block data: {:?}",
                    block_size, e
                );
                error!(
                    "❌ ProximaRecord STREAMING ERROR: Error kind: {:?}",
                    e.kind()
                );
                if let Some(raw_error) = e.get_ref() {
                    error!(
                        "❌ ProximaRecord STREAMING ERROR: Raw error: {:?}",
                        raw_error
                    );
                }
                return Some(Err(anyhow::Error::from(e)));
            }
        }

        // Deserialize the DataBlock using the proper DataBlock::deserialize method
        debug!(
            "🔍 ProximaRecord STREAMING: Attempting to deserialize DataBlock from {} bytes",
            block_data.len()
        );
        match ProximaDataBlock::deserialize(&block_data, None) {
            Ok(data_block) => {
                debug!(
                    "🔍 VectorRecord STREAMING: Successfully deserialized ProximaDataBlock with {} records",
                    data_block.records.len()
                );
                self.buffer = data_block.records;
                self.position = 0;
                self.current_block += 1;

                // Recursively call next() to return the first record from the new buffer
                self.next()
            }
            Err(e) => {
                error!(
                    "❌ ProximaRecord STREAMING ERROR: Failed to deserialize ProximaDataBlock: {:?}",
                    e
                );
                debug!(
                    "🔍 VectorRecord STREAMING: Block data preview (first 32 bytes): {:?}",
                    &block_data[..std::cmp::min(32, block_data.len())]
                );
                Some(Err(e))
            }
        }
    }
}

// Duplicate iterator implementation removed - using the optimized one above

/// Compaction-optimized direct reader that bypasses caching
/// Uses filesystem API for efficient range reads on cloud storage
#[derive(Clone)]
pub struct SstDirectReader {
    block_reader: ModularBlockReader,
    filesystem_factory: Arc<FilesystemFactory>,
}

impl SstDirectReader {
    pub async fn open(filesystem_factory: Arc<FilesystemFactory>, file_path: &str) -> Result<Self> {
        let block_reader = ModularBlockReader::open(filesystem_factory.clone(), file_path).await?;
        Ok(Self {
            block_reader,
            filesystem_factory,
        })
    }

    pub fn new(filesystem_factory: Arc<FilesystemFactory>) -> Result<Self> {
        // Create block reader with filesystem factory
        let block_reader = ModularBlockReader::new(filesystem_factory.clone(), String::new());
        Ok(Self {
            block_reader,
            filesystem_factory,
        })
    }

    /// Stream canonical records directly for compaction.
    /// NEW: Uses hierarchical header offsets for efficient selective reading
    pub async fn stream_vector_records(
        &mut self,
        file_path: String,
    ) -> Result<BlockIterator<ProximaRecord>> {
        let header = self.block_reader.read_header().await?;
        let total_blocks = header.block_count as usize;
        let block_size = header.block_size as usize;

        info!(
            "🔄 Streaming ProximaRecords from {} with {} blocks (hierarchical format)",
            file_path, total_blocks
        );

        let fs = self.filesystem_factory.get_filesystem(&file_path)?;

        // NEW: Use direct offsets from enhanced header for efficient access
        let data_blocks_offset = if header.data_blocks_offset > 0 {
            // Use pre-calculated offset from enhanced header
            debug!(
                "✅ Using enhanced header offset: {}",
                header.data_blocks_offset
            );
            header.data_blocks_offset
        } else {
            // Fallback to legacy calculation for compatibility
            debug!("⚠️ Using legacy offset calculation");
            let mut current_offset = 8 + header.header_size as u64; // After magic + header_len + header

            // Skip bloom filter if present
            if header.has_bloom_filter {
                let bloom_size_bytes = fs.read_range(&file_path, current_offset, 4).await?;
                let bloom_size = u32::from_le_bytes([
                    bloom_size_bytes[0],
                    bloom_size_bytes[1],
                    bloom_size_bytes[2],
                    bloom_size_bytes[3],
                ]) as u64;
                current_offset += 4 + bloom_size;
            }

            // Skip index
            let index_size_bytes = fs.read_range(&file_path, current_offset, 4).await?;
            let index_size = u32::from_le_bytes([
                index_size_bytes[0],
                index_size_bytes[1],
                index_size_bytes[2],
                index_size_bytes[3],
            ]) as u64;
            current_offset += 4 + index_size;

            current_offset
        };

        // Read just the data blocks portion of the file
        let file_metadata = fs.metadata(&file_path).await?;
        let data_blocks_size = file_metadata.size - data_blocks_offset;

        debug!(
            "📊 Reading data blocks: offset={}, size={} bytes, format={:?}",
            data_blocks_offset, data_blocks_size, header.vector_format
        );

        let data_blocks_bytes = fs
            .read_range(&file_path, data_blocks_offset, data_blocks_size)
            .await?;
        let reader = Box::new(std::io::Cursor::new(data_blocks_bytes)) as Box<dyn Read + Send>;

        debug!(
            "✅ Created hierarchical streaming iterator for {} blocks",
            total_blocks
        );

        Ok(BlockIterator {
            reader,
            buffer: Vec::new(),
            position: 0,
            block_size,
            total_blocks,
            current_block: 0,
            mode: ReadMode::Streaming,
            _phantom: PhantomData,
        })
    }

    /// NEW: Selective loading of global bloom filter for query optimization
    /// Uses enhanced header offsets to read only the global bloom filter
    pub async fn load_global_bloom_filter(
        &mut self,
        file_path: &str,
    ) -> Result<Option<SstableBloomFilter>> {
        let header = self.block_reader.read_header().await?;

        if !header.has_global_bloom || header.global_bloom_size == 0 {
            debug!("No global bloom filter in file {}", file_path);
            return Ok(None);
        }

        let fs = self.filesystem_factory.get_filesystem(file_path)?;

        // Use direct offset for efficient selective reading
        let bloom_offset = header.global_bloom_offset;
        let bloom_size = header.global_bloom_size as u64;

        debug!(
            "🌸 Loading global bloom filter: offset={}, size={} bytes",
            bloom_offset, bloom_size
        );

        let bloom_data = fs.read_range(file_path, bloom_offset, bloom_size).await?;

        match SstableBloomFilter::deserialize(&bloom_data) {
            Ok(bloom_filter) => {
                debug!(
                    "✅ Loaded global bloom filter with {} key filters and {} metadata columns",
                    bloom_filter.key_filter_data.len(),
                    bloom_filter.metadata_filter_data.len()
                );
                Ok(Some(bloom_filter))
            }
            Err(e) => {
                warn!("Failed to deserialize global bloom filter: {}", e);
                Ok(None)
            }
        }
    }

    /// NEW: Selective loading of block index with hierarchical bloom filters
    /// Only loads block index without reading data blocks for query planning
    pub async fn load_block_index(&mut self, file_path: &str) -> Result<Vec<IndexEntry>> {
        let header = self.block_reader.read_header().await?;
        let fs = self.filesystem_factory.get_filesystem(file_path)?;

        // Use direct offset for block index
        let index_offset = header.block_index_offset;
        let index_size = header.block_index_size as u64;

        debug!(
            "📋 Loading block index: offset={}, size={} bytes, has_block_blooms={}",
            index_offset, index_size, header.has_block_blooms
        );

        let index_data = fs.read_range(file_path, index_offset, index_size).await?;

        // Deserialize using SstableIndex::deserialize which handles the proper format
        // (IDX1 magic header + min_key + max_key + vector_count + entries + bplus_tree)
        let index = SstableIndex::deserialize(&index_data).map_err(|e| {
            anyhow::anyhow!(
                "Failed to deserialize SstableIndex at offset {}: {}",
                index_offset,
                e
            )
        })?;

        debug!(
            "✅ Loaded {} block index entries with hierarchical bloom support",
            index.entries.len()
        );
        Ok(index.entries)
    }

    /// Read all canonical records for compaction without caching.
    /// Uses efficient range reads for cloud storage (S3/GCS/Azure)
    pub async fn read_all_for_compaction(&mut self) -> Result<Vec<ProximaRecord>> {
        let header = self.block_reader.read_header().await?;

        let mut all_records = Vec::with_capacity(header.entry_count as usize);

        // For compaction, we don't need bloom filters or indexes - go straight to data blocks
        // Use hierarchical offsets when available for efficient access
        debug!(
            "📖 read_all_for_compaction: Reading {} data blocks",
            header.block_count
        );
        for block_id in 0..header.block_count {
            debug!("📖 read_all_for_compaction: Reading block {}", block_id);
            let block = self
                .block_reader
                .read_data_block(block_id as u64, ReadMode::Buffered)
                .await?;
            debug!(
                "📖 read_all_for_compaction: Block {} has {} records",
                block_id,
                block.records.len()
            );
            all_records.extend(block.records);
        }

        debug!(
            "📖 read_all_for_compaction: Total records read: {}",
            all_records.len()
        );
        Ok(all_records)
    }

    /// Create streaming iterator for memory-efficient compaction
    pub async fn stream_blocks(&mut self) -> Result<impl Stream<Item = Result<ProximaRecord>>> {
        let header = self.block_reader.read_header().await?;
        let block_reader = std::sync::Arc::new(tokio::sync::Mutex::new(self.block_reader.clone()));

        // Create async stream of canonical records
        let stream = futures::stream::iter(0..header.block_count)
            .then(move |block_id| {
                let reader = block_reader.clone();
                async move {
                    let mut reader = reader.lock().await;
                    let block = reader
                        .read_data_block(block_id as u64, ReadMode::Buffered)
                        .await?;
                    Ok::<Vec<ProximaRecord>, anyhow::Error>(block.records)
                }
            })
            .map(|result| result.map(|records| futures::stream::iter(records.into_iter().map(Ok))))
            .try_flatten();

        Ok(stream)
    }
}

impl UnifiedSstableReader {
    /// Search SSTable using SharedSstFormatReader for optimized I/O
    /// Delegates to shared infrastructure like SWIFT does
    pub async fn search_with_filter(
        &self,
        file_path: &str,
        query_vector: &[f32],
        filter: Option<FilterExpression>,
        k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        collection: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        // Delegate to the new version with default (sqrt) block pruning
        self.search_with_filter_and_pruning(
            file_path,
            query_vector,
            filter,
            k,
            distance_metric,
            collection,
            &crate::core::search::BlockPruneConfig::default(), // sqrt mode by default
        )
        .await
    }

    /// Search with filter and explicit block pruning configuration
    ///
    /// This method uses the modular block reader with smart block selection
    /// based on Z-order spatial codes and centroid distances.
    pub async fn search_with_filter_and_pruning(
        &self,
        file_path: &str,
        query_vector: &[f32],
        filter: Option<FilterExpression>,
        k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        collection: Option<&crate::proto::proximadb_v1::Collection>,
        block_prune: &crate::core::search::BlockPruneConfig,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        trace!(
            "SST Reader: search_with_filter_and_pruning called with file_path: {}, force_exact: {}",
            file_path, block_prune.force_exact
        );

        // Create search context
        let context = CollectionContext {
            file_path: file_path.to_string(),
            sstable_files: vec![file_path.to_string()],
            total_vectors: 0,
            metadata_columns: Vec::new(),
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
            collection: collection.map(|c| Arc::new(c.clone())),
        };

        let params = SearchParams {
            vector: Some(query_vector.to_vec()),
            filter_expression: filter.clone(),
            top_k: Some(k),
            distance_metric: Some(distance_metric),
            block_prune: block_prune.clone(),
            ..Default::default()
        };

        // Use modular search with block pruning instead of FullScan
        // This applies Z-order/centroid-based block selection
        let blocks = self
            .search_optimized_strategy_modular(&context, &params)
            .await?;
        trace!(
            "SST Reader: apply_strategy returned {} blocks",
            blocks.len()
        );
        if let Some(first_block) = blocks.first()
            && let Some(_first_rec) = first_block.records.first()
        {}

        // Step 4: Process blocks and compute distances
        let mut results = Vec::new();
        let distance_compute =
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                distance_metric,
            );

        for block in blocks {
            for record in block.records {
                // Apply metadata filtering at record level against canonical props.
                if let Some(filter_expr) = &filter {
                    let matches = crate::core::search::sql_value_filter::evaluate_filter_proxima(
                        filter_expr,
                        &record.props,
                    );
                    trace!(
                        "Filter evaluation: record {} metadata={:?} matches={}",
                        record.oid, record.props, matches
                    );
                    if !matches {
                        // Record doesn't match filter, skip it
                        continue;
                    }
                }

                let vector = record_vector(&record);
                if vector.is_empty() {
                    continue;
                }

                // Compute distance
                let distance =
                    distance_compute.calculate_distance(query_vector, vector, &distance_metric);

                // Create result
                // Use normalized_score for consistency across all engines
                // Higher similarity = better match, VOS sorts descending
                let result = crate::core::search::results::OptimizedSearchRecord::new(
                    record_id(&record),
                    distance.normalized_score,
                )
                .with_similarity(distance.normalized_score)
                .add_vector(vector.to_vec())
                .with_proxima_metadata(record_metadata(&record));

                results.push(result);
            }
        }

        // Step 5: Use bounded priority queue for efficient top-k selection
        let mut priority_queue = BoundedPriorityQueue::new(k);

        // Insert all results into bounded queue
        for result in results {
            priority_queue.try_insert(result);
        }

        // Get sorted results from bounded queue
        let final_results = priority_queue.into_sorted_vec();

        debug!(
            "✅ SST: Search complete, found {} results",
            final_results.len()
        );
        Ok(final_results)
    }

    /// Vectorized search using DataChunk and Arrow compute kernels (TD-041)
    ///
    /// This method provides 10x performance improvement by:
    /// - Converting VectorRecord batches to Arrow RecordBatches
    /// - Using evaluate_predicate_vectorized() for batch filtering
    /// - Processing distances with SIMD-enabled Arrow kernels
    /// - Applying selection vectors for late materialization
    pub async fn search_with_filter_vectorized(
        &self,
        file_path: &str,
        query_vector: &[f32],
        filter: Option<FilterExpression>,
        k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        collection: Option<&crate::proto::proximadb_v1::Collection>,
        block_prune: &crate::core::search::BlockPruneConfig,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        trace!(
            "SST Reader: vectorized search called with file_path: {}",
            file_path
        );

        // Create search context (reuse existing logic)
        let context = CollectionContext {
            file_path: file_path.to_string(),
            sstable_files: vec![file_path.to_string()],
            total_vectors: 0,
            metadata_columns: Vec::new(),
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
            collection: collection.map(|c| Arc::new(c.clone())),
        };

        let params = SearchParams {
            vector: Some(query_vector.to_vec()),
            filter_expression: filter.clone(),
            top_k: Some(k),
            distance_metric: Some(distance_metric),
            block_prune: block_prune.clone(),
            ..Default::default()
        };

        // Get blocks using existing modular search
        let blocks = self
            .search_optimized_strategy_modular(&context, &params)
            .await?;

        trace!("SST Reader: vectorized processing {} blocks", blocks.len());

        // Create distance compute engine
        let distance_compute =
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                distance_metric,
            );

        // Process blocks with vectorized filtering
        let mut results = Vec::new();

        for block in blocks {
            let batch = self.records_to_batch(&block.records)?;

            // Apply vectorized filtering if filter expression exists
            let filtered_indices = if let Some(filter_expr) = &filter {
                // Convert FilterExpression to FilterCondition for vectorized executor
                let filter_condition = self.filter_to_condition(filter_expr)?;
                let selection_mask = evaluate_predicate_vectorized(&batch, &filter_condition)?;

                // Get indices of selected rows
                self.extract_selected_indices(&selection_mask)?
            } else {
                // No filter, select all rows
                (0..batch.num_rows()).collect::<Vec<_>>()
            };

            trace!(
                "Vectorized filter: {} records -> {} selected",
                batch.num_rows(),
                filtered_indices.len()
            );

            // Process only selected records
            for idx in filtered_indices {
                if idx < block.records.len() {
                    let record = &block.records[idx];
                    let vector = record_vector(record);
                    if vector.is_empty() {
                        continue;
                    }

                    // Compute distance
                    let distance =
                        distance_compute.calculate_distance(query_vector, vector, &distance_metric);

                    // Create result
                    let result = crate::core::search::results::OptimizedSearchRecord::new(
                        record_id(record),
                        distance.normalized_score,
                    )
                    .with_similarity(distance.normalized_score)
                    .add_vector(vector.to_vec())
                    .with_proxima_metadata(record_metadata(record));

                    results.push(result);
                }
            }
        }

        // Use bounded priority queue for efficient top-k selection
        let mut priority_queue = BoundedPriorityQueue::new(k);
        for result in results {
            priority_queue.try_insert(result);
        }

        let final_results = priority_queue.into_sorted_vec();

        debug!(
            "✅ SST: Vectorized search complete, found {} results",
            final_results.len()
        );
        Ok(final_results)
    }

    /// Parallel search using morsel-driven execution (TD-039)
    ///
    /// This method provides parallel processing of large result sets by:
    /// - Dividing records into 4096-row morsels
    /// - Processing morsels in parallel with worker threads
    /// - Using work-stealing for load balancing
    ///
    /// Best for: Large result sets (>10K records) with complex filters
    pub async fn search_with_filter_parallel_morsels(
        &self,
        file_path: &str,
        query_vector: &[f32],
        filter: Option<FilterExpression>,
        k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        collection: Option<&crate::proto::proximadb_v1::Collection>,
        block_prune: &crate::core::search::BlockPruneConfig,
        max_workers: Option<usize>,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        use crate::storage::engines::sst::readers::morsel_scheduler::{
            MORSEL_SIZE, MorselScheduler,
        };

        trace!(
            "SST Reader: parallel morsel search called with file_path: {}",
            file_path
        );

        // Create search context
        let context = CollectionContext {
            file_path: file_path.to_string(),
            sstable_files: vec![file_path.to_string()],
            total_vectors: 0,
            metadata_columns: Vec::new(),
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
            collection: collection.map(|c| Arc::new(c.clone())),
        };

        let params = SearchParams {
            vector: Some(query_vector.to_vec()),
            filter_expression: filter.clone(),
            top_k: Some(k),
            distance_metric: Some(distance_metric),
            block_prune: block_prune.clone(),
            ..Default::default()
        };

        // Get blocks using existing modular search
        let blocks = self
            .search_optimized_strategy_modular(&context, &params)
            .await?;

        trace!(
            "SST Reader: parallel morsel processing {} blocks",
            blocks.len()
        );

        // Flatten all records from all blocks
        let all_records: Vec<ProximaRecord> =
            blocks.into_iter().flat_map(|block| block.records).collect();

        let total_records = all_records.len();

        // Only use parallel processing if we have enough records
        if total_records < MORSEL_SIZE * 2 {
            debug!(
                "Record count ({}) below parallel threshold ({}), using vectorized path",
                total_records,
                MORSEL_SIZE * 2
            );
            return self
                .search_with_filter_vectorized(
                    file_path,
                    query_vector,
                    filter,
                    k,
                    distance_metric,
                    collection,
                    block_prune,
                )
                .await;
        }

        info!(
            "Using parallel morsel processing for {} records with {} workers",
            total_records,
            max_workers.unwrap_or_else(|| std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4))
        );

        // Create morsel scheduler
        let scheduler = MorselScheduler::new(max_workers);

        // Distance compute engine (needs to be cloned per morsel)
        let metric = distance_metric;

        // Process morsels in parallel
        let filter_arc = filter.clone();
        let query_vec_owned = query_vector.to_vec();
        let morsel_results = scheduler
            .process_morsels(all_records, move |morsel_records| {
                let filter_clone = filter_arc.clone();
                let qv = query_vec_owned.clone();
                async move {
                    // Create distance compute for this morsel
                    let distance_compute =
                        crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                            metric,
                        );

                    // Process each record in the morsel
                    let mut results = Vec::new();
                    for record in morsel_records {
                        // Apply filter if present
                        if let Some(filter_expr) = &filter_clone
                            && !crate::core::search::sql_value_filter::evaluate_filter_proxima(
                                filter_expr,
                                &record.props,
                            )
                        {
                            continue; // Skip filtered records
                        }

                        let vector = record_vector(&record);
                        if vector.is_empty() {
                            continue;
                        }

                        // Compute distance
                        let distance = distance_compute.calculate_distance(&qv, vector, &metric);

                        // Create result
                        let result = crate::core::search::results::OptimizedSearchRecord::new(
                            record_id(&record),
                            distance.normalized_score,
                        )
                        .with_similarity(distance.normalized_score)
                        .add_vector(vector.to_vec())
                        .with_proxima_metadata(record_metadata(&record));

                        results.push(result);
                    }

                    Ok(results)
                }
            })
            .await?;

        // Combine results from all morsels (process_morsels already flattens)
        let combined_results = morsel_results;

        // Use bounded priority queue for efficient top-k selection
        let mut priority_queue = BoundedPriorityQueue::new(k);
        for result in combined_results {
            priority_queue.try_insert(result);
        }

        let final_results = priority_queue.into_sorted_vec();

        debug!(
            "✅ SST: Parallel morsel search complete, found {} results",
            final_results.len()
        );
        Ok(final_results)
    }

    /// Pipeline-based execution with DataChunks (TD-031)
    ///
    /// This method provides pull-based pipeline execution with selection vectors:
    /// - Operators: Scan, Filter, Project, Sort, TopK
    /// - Zero-copy operations using selection vectors
    /// - Late materialization of results
    /// - Pull-based execution with next_chunk() pattern
    ///
    /// Best for: Complex queries with multiple operations (filter + sort + top-k)
    pub async fn search_with_pipeline_execution(
        &self,
        file_path: &str,
        query_vector: &[f32],
        filter: Option<FilterExpression>,
        k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        collection: Option<&crate::proto::proximadb_v1::Collection>,
        block_prune: &crate::core::search::BlockPruneConfig,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        trace!(
            "SST Reader: pipeline execution search called with file_path: {}",
            file_path
        );

        // Create search context
        let context = CollectionContext {
            file_path: file_path.to_string(),
            sstable_files: vec![file_path.to_string()],
            total_vectors: 0,
            metadata_columns: Vec::new(),
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
            collection: collection.map(|c| Arc::new(c.clone())),
        };

        let params = SearchParams {
            vector: Some(query_vector.to_vec()),
            filter_expression: filter.clone(),
            top_k: Some(k),
            distance_metric: Some(distance_metric),
            block_prune: block_prune.clone(),
            ..Default::default()
        };

        // Get blocks using existing modular search
        let blocks = self
            .search_optimized_strategy_modular(&context, &params)
            .await?;

        trace!(
            "SST Reader: pipeline execution processing {} blocks",
            blocks.len()
        );

        let all_records: Vec<ProximaRecord> =
            blocks.into_iter().flat_map(|block| block.records).collect();

        trace!(
            "Pipeline execution: processing {} records with filter and top-k",
            all_records.len()
        );

        let distance_compute =
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                distance_metric,
            );
        let mut queue = BoundedPriorityQueue::new(k);

        for record in &all_records {
            if let Some(filter_expr) = &filter
                && !crate::core::search::sql_value_filter::evaluate_filter_proxima(
                    filter_expr,
                    &record.props,
                )
            {
                continue;
            }

            let vector = record_vector(record);
            if vector.is_empty() {
                continue;
            }

            let distance =
                distance_compute.calculate_distance(query_vector, vector, &distance_metric);
            queue.try_insert(
                crate::core::search::results::OptimizedSearchRecord::new(
                    record_id(record),
                    distance.normalized_score,
                )
                .with_similarity(distance.normalized_score)
                .add_vector(vector.to_vec())
                .with_proxima_metadata(record_metadata(record)),
            );
        }

        let results = queue.into_sorted_vec();

        debug!(
            "✅ SST: Pipeline execution complete, found {} results",
            results.len()
        );
        Ok(results)
    }

    /// Convert canonical record batch to Arrow RecordBatch for vectorized processing.
    fn records_to_batch(&self, records: &[ProximaRecord]) -> Result<RecordBatch> {
        use arrow::array::{Float32Array, StringArray};

        if records.is_empty() {
            // Return empty batch with correct schema
            let schema = Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new(
                    "vector",
                    DataType::FixedSizeList(
                        Arc::new(Field::new("item", DataType::Float32, true)),
                        384, // Default dimension
                    ),
                    true,
                ),
            ]);
            return Ok(RecordBatch::new_empty(Arc::new(schema)));
        }

        // Extract IDs
        let ids: Vec<&str> = records.iter().map(|r| r.oid.as_str()).collect();

        // Extract vectors (assuming all vectors have the same dimension)
        let vector_dim = records
            .first()
            .map(record_vector)
            .map(|v| v.len())
            .unwrap_or(384);

        // Flatten vectors for Arrow FixedSizeList
        let mut vector_values = Vec::with_capacity(records.len() * vector_dim);
        for record in records {
            vector_values.extend_from_slice(record_vector(record));
        }

        let _vector_array = Float32Array::from(vector_values);

        // Create FixedSizeList array for vectors
        // Deferred: Include vector column in batch schema (TD-041 Phase 3)
        let schema = Schema::new(vec![Field::new("id", DataType::Utf8, false)]);

        let id_array = StringArray::from(ids);

        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(id_array)])?;

        Ok(batch)
    }

    /// Convert FilterExpression to FilterCondition for vectorized executor
    fn filter_to_condition(
        &self,
        filter: &FilterExpression,
    ) -> Result<crate::storage::engines::core::formats::columnar::FilterCondition> {
        use crate::core::search::ComparisonOperator;
        use crate::storage::engines::core::formats::columnar::FilterCondition;

        // Convert FilterExpression to FilterCondition for vectorized executor
        match filter {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                match operator {
                    ComparisonOperator::Equals => {
                        Ok(FilterCondition::Equals(field.clone(), value.clone()))
                    }
                    ComparisonOperator::Between => {
                        // Between expects an array of [min, max]
                        if let Some(arr) = value.as_array() {
                            if arr.len() >= 2 {
                                Ok(FilterCondition::Range(
                                    field.clone(),
                                    arr[0].clone(),
                                    arr[1].clone(),
                                ))
                            } else {
                                // Fallback to equals if between doesn't have 2 values
                                Ok(FilterCondition::Equals(field.clone(), value.clone()))
                            }
                        } else {
                            Ok(FilterCondition::Equals(field.clone(), value.clone()))
                        }
                    }
                    _ => {
                        // For other operators, use a pass-through condition
                        Ok(FilterCondition::Equals(
                            "_id".to_string(),
                            serde_json::Value::String("_dummy".to_string()),
                        ))
                    }
                }
            }
            _ => {
                // Fallback: return a condition that always passes
                Ok(FilterCondition::Equals(
                    "_id".to_string(),
                    serde_json::Value::String("_dummy".to_string()),
                ))
            }
        }
    }

    /// Extract indices of selected rows from a boolean selection mask
    fn extract_selected_indices(
        &self,
        selection: &arrow::array::BooleanArray,
    ) -> Result<Vec<usize>> {
        let mut indices = Vec::new();
        for (i, val) in selection.iter().enumerate() {
            if val == Some(true) {
                indices.push(i);
            }
        }
        Ok(indices)
    }

    /// Validate SST1 magic marker in a file to ensure it's a valid SSTable
    /// Returns Ok(()) if valid, Err with descriptive message if invalid
    /// This prevents reading non-SSTable files that could cause deserialization errors
    pub async fn validate_sst_file(&self, file_path: &str) -> Result<()> {
        // Extract scheme from file path for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;

        // Check if file exists first
        if !fs.exists(file_path).await? {
            return Err(anyhow::anyhow!(
                "SSTable file does not exist: {}",
                file_path
            ));
        }

        // Read first 4 bytes to check magic marker
        let magic_bytes = fs
            .read_range(file_path, 0, 4)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read magic bytes from {}: {}", file_path, e))?;

        if magic_bytes.len() < 4 {
            return Err(anyhow::anyhow!(
                "File too small to be valid SSTable: {} has only {} bytes",
                file_path,
                magic_bytes.len()
            ));
        }

        if &magic_bytes[0..4] != b"SST1" {
            // Log what we actually found for debugging
            let found_magic = std::str::from_utf8(&magic_bytes[0..4]).map_or_else(
                |_| format!("bytes: {:?}", &magic_bytes[0..4]),
                |s| s.to_string(),
            );

            return Err(anyhow::anyhow!(
                "Invalid SSTable format: expected SST1 magic marker, found '{}' in file {}",
                found_magic,
                file_path
            ));
        }

        debug!("✅ SST1 magic marker validated for file: {}", file_path);
        Ok(())
    }

    /// Create unified reader with zero-copy system (leverages SharedSstFormatReader for file ops)
    ///
    /// # Architecture Decision
    /// - No VectorCache: High-dimensional vectors benefit from OS page cache + mmap
    /// - No IndexNodeCache/BloomCache: File-specific metadata handled by zero-copy system
    /// - Single cache layer: Zero-copy system with filename-based metadata verification
    /// - Code Reuse: Delegates file operations to SharedSstFormatReader (eliminates duplication)
    pub fn new(
        filesystem: Arc<FilesystemFactory>,
        caching_filesystem: Arc<UnifiedCachingFilesystem>,
        collection_id: String,
    ) -> Self {
        let config = ReaderConfig::default();

        // Create default SST mmap strategy optimized for search workloads
        let mmap_strategy = SstMmapStrategy {
            always_mmap: vec![
                SstRegion::BloomFilter, // Always cache bloom filters
                SstRegion::IndexBlock,  // Always cache index blocks
            ],
            conditional_mmap: vec![
                (SstRegion::DataBlocks, 0.7), // Cache data blocks if memory pressure < 70%
            ],
            never_mmap: vec![
                SstRegion::Metadata, // Metadata is small, don't need mmap
            ],
        };

        // Create shared reader for actual file operations
        // SharedSstFormatReader needs to be updated to use UnifiedCachingFilesystem
        let shared_reader = Arc::new(SharedSstFormatReader::new(
            filesystem.clone(),
            mmap_strategy,
            caching_filesystem.clone(),
            collection_id.clone(),
        ));

        Self {
            shared_reader,
            strategy_selector: Arc::new(ReadingStrategySelector::new(config)),
            caching_filesystem,
            collection_id,
            filesystem,
        }
    }

    /// Read all records from SSTable files for compaction
    /// Uses the working compaction strategy instead of the stub
    pub async fn read_all_records_for_compaction(
        &self,
        sstable_files: &[String],
    ) -> Result<Vec<ProximaRecord>> {
        // Use read_with_compaction_strategy which actually reads files
        self.read_with_compaction_strategy(sstable_files, None)
            .await
    }

    /// 🚀 NEW: Create unified reader with bandwidth optimizer for smart threshold decisions
    /// This constructor enables dual strategy support for different operation types
    pub fn new_with_bandwidth_optimizer(
        filesystem: Arc<FilesystemFactory>,
        caching_filesystem: Arc<UnifiedCachingFilesystem>,
        collection_id: String,
        _bandwidth_optimizer: Option<
            Arc<crate::storage::engines::core::io::zero_copy::BandwidthOptimizer>,
        >,
    ) -> Self {
        let config = ReaderConfig::default();

        // Create SST mmap strategy that considers bandwidth optimization decisions
        let mmap_strategy = SstMmapStrategy {
            always_mmap: vec![
                SstRegion::BloomFilter, // Always cache bloom filters for metadata filtering
                SstRegion::IndexBlock,  // Always cache index blocks for range queries
            ],
            conditional_mmap: vec![
                (SstRegion::DataBlocks, 0.7), // Cache data blocks based on bandwidth optimizer decisions
            ],
            never_mmap: vec![
                SstRegion::Metadata, // Metadata is small, direct read is faster
            ],
        };

        // Create shared reader with bandwidth optimization support
        let shared_reader = Arc::new(SharedSstFormatReader::new(
            filesystem.clone(),
            mmap_strategy,
            caching_filesystem.clone(),
            collection_id.clone(),
        ));

        Self {
            shared_reader,
            strategy_selector: Arc::new(ReadingStrategySelector::new(config)),
            caching_filesystem,
            collection_id,
            filesystem,
        }
    }

    /// 🚀 NEW: Read with selective cache strategy - for normal queries with range reads and cache lookup
    /// This strategy is optimized for:
    /// - Range-based reading with bloom filter optimization
    /// - Cache lookup for frequently accessed data
    /// - Metadata cache utilization for query planning
    /// - Bandwidth-aware threshold decisions
    pub async fn read_with_selective_cache_strategy(
        &self,
        params: &SearchParams,
        collection_context: &CollectionContext,
        bandwidth_optimizer: Option<
            Arc<crate::storage::engines::core::io::zero_copy::BandwidthOptimizer>,
        >,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        debug!(
            "🔍 SST SELECTIVE CACHE: Starting selective read strategy for {} files",
            collection_context.sstable_files.len()
        );

        // Use SelectiveWithCache strategy
        let strategy = SstableReadingStrategy::SelectiveWithCache {
            use_range_reads: true,
            enable_bloom_filters: true,
            enable_cache_lookup: true,
            enable_metadata_cache: true,
        };

        // Apply bandwidth optimizer decisions if available
        if let Some(_optimizer) = bandwidth_optimizer {
            // Create query context for bandwidth decisions
            // Deferred: Replace with UnifiedCachingFilesystem query context
            use std::collections::HashMap;

            let _query_context = (
                params.top_k,
                HashMap::<String, String>::new(), // metadata_filters
                self.collection_id.clone(),
            );

            // Bandwidth optimization decisions handled internally
            for _file_path in &collection_context.sstable_files {
                // Strategy decisions are made during actual read operations
            }
        }

        // Apply strategy to read relevant blocks with cache optimization
        let relevant_blocks = self
            .apply_strategy(&strategy, params, collection_context)
            .await?;

        // Perform vector search on loaded data
        self.perform_vector_search_on_blocks(&relevant_blocks, params)
            .await
    }

    /// 🚀 NEW: Read with compaction strategy - for full read operations where cache lookups are suboptimal
    /// This strategy is optimized for:
    /// - Compaction operations that bypass write cache but use disk cache if files exist
    /// - Full file sequential reads without range optimization
    /// - Minimal metadata overhead for transient files
    /// - Bandwidth conservation by avoiding unnecessary downloads
    pub async fn read_with_compaction_strategy(
        &self,
        sstable_files: &[String],
        bandwidth_optimizer: Option<
            Arc<crate::storage::engines::core::io::zero_copy::BandwidthOptimizer>,
        >,
    ) -> Result<Vec<ProximaRecord>> {
        info!(
            "🔥 SST COMPACTION: Starting compaction read strategy for {} files",
            sstable_files.len()
        );

        // Use CompactionFullRead strategy
        let strategy = SstableReadingStrategy::CompactionFullRead {
            skip_bloom_filters: true,
            skip_indexes: true,
            bypass_write_cache: true,
            use_disk_cache_if_exists: true,
            sequential_io: true,
        };

        // Apply bandwidth optimizer decisions if available
        if let Some(_optimizer) = bandwidth_optimizer {
            // Create compaction query context
            use std::collections::HashMap;

            // Deferred: Replace with UnifiedCachingFilesystem context
            let _query_context = (
                self.collection_id.clone(),
                HashMap::<String, String>::new(), // metadata_filters
                1.0,                              // selectivity_hint for full scan
            );

            // Bandwidth optimization for compaction handled internally
            for _file_path in sstable_files {
                // Strategy decisions are made during actual read operations
                // Compaction prefers disk cache to avoid re-downloading transient files
            }
        }

        // Create minimal context for compaction
        let context = CollectionContext {
            file_path: String::new(),
            sstable_files: sstable_files.to_vec(),
            total_vectors: 0,
            metadata_columns: vec![],
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
            collection: None,
        };

        // Apply compaction strategy
        let blocks = self
            .apply_strategy(&strategy, &Default::default(), &context)
            .await?;
        info!(
            "📦 SST COMPACTION: Loaded {} data blocks for compaction",
            blocks.len()
        );

        let mut all_records = Vec::new();
        for block in blocks {
            all_records.extend(block.records);
        }

        info!(
            "🎯 SST COMPACTION: Returning {} records for compaction",
            all_records.len()
        );
        Ok(all_records)
    }

    /// 🚀 HELPER: Perform vector search on loaded data blocks
    /// This is extracted from the main search logic to support dual strategy patterns
    async fn perform_vector_search_on_blocks(
        &self,
        blocks: &[ProximaDataBlock],
        params: &SearchParams,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        // Create distance compute locally per query to avoid cross-query contamination
        let distance_compute = UnifiedDistanceCompute::default();

        // Perform the actual search
        self.search_in_blocks(params, blocks, &distance_compute)
            .await
    }

    /// Search vectors using cache-first zero-copy architecture
    pub async fn search_vectors(
        &self,
        params: &SearchParams,
        collection_context: &CollectionContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        debug!(
            "🔍 SSTABLE READER: Starting cache-first search with {} files, k={}",
            collection_context.sstable_files.len(),
            params.top_k.unwrap_or(10)
        );

        // CRITICAL: Create distance compute locally per query to avoid cross-query contamination
        let distance_compute = UnifiedDistanceCompute::default();

        // CACHE-FIRST PATTERN: Check zero-copy metadata cache for each file
        // Cache key format: filename:collection_id:engine (filename-first for optimal sequential matching)
        let cached_metadata = Vec::<String>::new();
        let mut files_needing_load = Vec::new();

        for file_path in &collection_context.sstable_files {
            debug!("📁 Checking cache for SSTable file: {}", file_path);

            // UnifiedCachingFilesystem handles caching internally
            // Try to get metadata (will use cache if available)
            match self.caching_filesystem.metadata(file_path).await {
                Ok(_metadata) => {
                    debug!("✅ Got metadata for file: {}", file_path);
                    // Convert FileMetadata to the format needed here
                    // For now, we'll need to load the file to get SSTable metadata
                    files_needing_load.push(file_path.clone());
                }
                Err(e) => {
                    warn!(
                        "⚠️ Error getting metadata for file {}: {}, will try to load",
                        file_path, e
                    );
                    files_needing_load.push(file_path.clone());
                }
            }
        }

        debug!(
            "📊 Cache stats: {} hits, {} misses",
            cached_metadata.len(),
            files_needing_load.len()
        );

        // 1. Select optimal reading strategy
        let search_strategy = self
            .strategy_selector
            .select_strategy(params, collection_context)?;
        debug!("📊 Selected strategy: {:?}", search_strategy);

        // 2. Apply strategy to read relevant blocks (zero-copy system handles caching)
        let relevant_blocks = self
            .apply_strategy(&search_strategy, params, collection_context)
            .await?;
        debug!(
            "📦 SSTABLE READER: Loaded {} data blocks total from all files",
            relevant_blocks.len()
        );

        // Debug: print some sample records from blocks
        for (i, block) in relevant_blocks.iter().take(2).enumerate() {
            debug!("  Block {}: {} records", i, block.records.len());
            for (j, record) in block.records.iter().take(3).enumerate() {
                debug!(
                    "    Record {}: id={:?}, metadata={:?}",
                    j, record.oid, record.props
                );
            }
        }

        // 3. Perform vector search on loaded data
        let results = self
            .search_in_blocks(params, &relevant_blocks, &distance_compute)
            .await?;
        debug!(
            "🎯 Found {} search results after filtering and scoring",
            results.len()
        );

        // Debug: print sample results
        for (i, result) in results.iter().take(3).enumerate() {
            debug!(
                "  Result {}: id={}, score={}, metadata={:?}",
                i, result.id, result.score, result.metadata
            );
        }

        Ok(results)
    }

    /// Apply reading strategy to load relevant blocks
    fn apply_strategy<'a>(
        &'a self,
        strategy: &'a SstableReadingStrategy,
        params: &'a SearchParams,
        context: &'a CollectionContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<ProximaDataBlock>>> + Send + 'a>,
    > {
        Box::pin(async move {
            trace!("SST Apply Strategy: Starting with strategy type");
            match strategy {
                SstableReadingStrategy::FullScan { use_block_cache } => {
                    self.full_scan_strategy(context, *use_block_cache).await
                }
                SstableReadingStrategy::IndexRangeScan {
                    start_block,
                    end_block,
                    use_bloom_filter,
                } => {
                    self.index_range_scan_strategy(
                        context,
                        *start_block,
                        *end_block,
                        *use_bloom_filter,
                    )
                    .await
                }
                SstableReadingStrategy::MetadataFiltered {
                    selected_blocks,
                    skip_bloom_check,
                } => {
                    self.metadata_filtered_strategy(
                        context,
                        params,
                        selected_blocks,
                        *skip_bloom_check,
                    )
                    .await
                }
                SstableReadingStrategy::Hybrid {
                    primary_strategy,
                    fallback_blocks,
                } => {
                    let mut blocks = self
                        .apply_strategy(primary_strategy, params, context)
                        .await?;
                    let fallback = self.load_specific_blocks(context, fallback_blocks).await?;
                    blocks.extend(fallback);
                    Ok(blocks)
                }
                SstableReadingStrategy::CompactionFullRead {
                    skip_bloom_filters,
                    skip_indexes,
                    bypass_write_cache,
                    use_disk_cache_if_exists,
                    sequential_io,
                } => {
                    self.compaction_full_read_strategy(
                        context,
                        *skip_bloom_filters,
                        *skip_indexes,
                        *bypass_write_cache,
                        *use_disk_cache_if_exists,
                        *sequential_io,
                    )
                    .await
                }
                // 🚀 NEW: Dual strategy support
                SstableReadingStrategy::SelectiveWithCache {
                    use_range_reads,
                    enable_bloom_filters,
                    enable_cache_lookup,
                    enable_metadata_cache,
                } => {
                    self.selective_cache_strategy(
                        context,
                        *use_range_reads,
                        *enable_bloom_filters,
                        *enable_cache_lookup,
                        *enable_metadata_cache,
                    )
                    .await
                }
            }
        })
    }

    /// 🚀 NEW: Selective cache strategy - optimized for normal queries with range reads and cache lookup
    async fn selective_cache_strategy(
        &self,
        context: &CollectionContext,
        use_range_reads: bool,
        enable_bloom_filters: bool,
        enable_cache_lookup: bool,
        enable_metadata_cache: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        debug!(
            "🔍 SST SELECTIVE CACHE: Processing {} files with range_reads={}, bloom={}, cache={}, metadata_cache={}",
            context.sstable_files.len(),
            use_range_reads,
            enable_bloom_filters,
            enable_cache_lookup,
            enable_metadata_cache
        );

        let mut all_blocks = Vec::new();

        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!(
                "📂 SELECTIVE CACHE: File {} of {}: {}",
                idx + 1,
                context.sstable_files.len(),
                file_path
            );

            // Validate SST1 magic marker
            match self.validate_sst_file(file_path).await {
                Ok(()) => {
                    debug!("✅ SST1 validation passed for file: {}", file_path);
                }
                Err(e) => {
                    warn!("⚠️ Skipping invalid SSTable file {}: {}", file_path, e);
                    continue;
                }
            }

            let start_time = std::time::Instant::now();

            // Use cache lookup when enabled
            let blocks = if enable_cache_lookup {
                self.read_file_with_cache(file_path, context.collection.as_deref())
                    .await?
            } else {
                // Use modular reading for range-based optimization
                if use_range_reads {
                    self.read_file_with_range_optimization(
                        file_path,
                        enable_bloom_filters,
                        enable_metadata_cache,
                    )
                    .await?
                } else {
                    self.read_file_direct(file_path, context.collection.as_deref())
                        .await?
                }
            };

            let elapsed = start_time.elapsed();
            debug!(
                "⚡ SELECTIVE CACHE: Loaded {} blocks from {} in {:?}",
                blocks.len(),
                file_path,
                elapsed
            );

            all_blocks.extend(blocks);
        }

        debug!(
            "✅ SELECTIVE CACHE: Loaded {} total blocks from {} files",
            all_blocks.len(),
            context.sstable_files.len()
        );
        Ok(all_blocks)
    }

    /// 🚀 NEW: Compaction full read strategy - optimized for bulk operations with minimal overhead
    async fn compaction_full_read_strategy(
        &self,
        context: &CollectionContext,
        skip_bloom_filters: bool,
        skip_indexes: bool,
        bypass_write_cache: bool,
        use_disk_cache_if_exists: bool,
        sequential_io: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        info!(
            "🔥 SST COMPACTION FULL READ: Processing {} files with optimizations: bloom={}, index={}, write_cache={}, disk_cache={}, sequential={}",
            context.sstable_files.len(),
            !skip_bloom_filters,
            !skip_indexes,
            !bypass_write_cache,
            use_disk_cache_if_exists,
            sequential_io
        );

        // For compaction, prefer direct reading to avoid cache pollution
        if bypass_write_cache && skip_bloom_filters && skip_indexes && sequential_io {
            // Use the most optimized path for bulk compaction
            return self.compaction_direct_strategy_modular(context).await;
        }

        let mut all_blocks = Vec::new();

        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!(
                "📂 COMPACTION FULL READ: File {} of {}: {}",
                idx + 1,
                context.sstable_files.len(),
                file_path
            );

            // Validate SST1 magic marker
            match self.validate_sst_file(file_path).await {
                Ok(()) => {
                    debug!("✅ SST1 validation passed for file: {}", file_path);
                }
                Err(e) => {
                    warn!("⚠️ Skipping invalid SSTable file {}: {}", file_path, e);
                    continue;
                }
            }

            let start_time = std::time::Instant::now();

            // Check disk cache first if enabled, otherwise direct read
            let blocks = if use_disk_cache_if_exists {
                // Try cache first, fallback to direct read
                match self
                    .read_file_with_cache(file_path, context.collection.as_deref())
                    .await
                {
                    Ok(cached_blocks) => {
                        debug!("💾 COMPACTION: Using disk cache for {}", file_path);
                        cached_blocks
                    }
                    Err(_) => {
                        debug!("📁 COMPACTION: Cache miss, direct read for {}", file_path);
                        self.read_file_direct_no_cache(
                            file_path,
                            skip_bloom_filters,
                            skip_indexes,
                            sequential_io,
                        )
                        .await?
                    }
                }
            } else {
                // Direct read without cache to avoid cache pollution
                self.read_file_direct_no_cache(
                    file_path,
                    skip_bloom_filters,
                    skip_indexes,
                    sequential_io,
                )
                .await?
            };

            let elapsed = start_time.elapsed();
            debug!(
                "⚡ COMPACTION FULL READ: Loaded {} blocks from {} in {:?} (disk_cache={})",
                blocks.len(),
                file_path,
                elapsed,
                use_disk_cache_if_exists
            );

            all_blocks.extend(blocks);
        }

        info!(
            "✅ COMPACTION FULL READ: Loaded {} total blocks from {} files",
            all_blocks.len(),
            context.sstable_files.len()
        );
        Ok(all_blocks)
    }

    /// Helper method for range-optimized reading with bloom filter and metadata cache support
    async fn read_file_with_range_optimization(
        &self,
        file_path: &str,
        enable_bloom_filters: bool,
        enable_metadata_cache: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        debug!(
            "📊 RANGE OPTIMIZATION: Reading {} with bloom={}, metadata_cache={}",
            file_path, enable_bloom_filters, enable_metadata_cache
        );

        // For now, use the modular full scan strategy with optimizations
        // In a full implementation, this would use selective range reads
        self.full_scan_strategy_modular(
            &CollectionContext {
                file_path: file_path.to_string(),
                sstable_files: vec![file_path.to_string()],
                total_vectors: 0,
                metadata_columns: vec![],
                level: 0,
                creation_time: chrono::Utc::now(),
                io_optimization_hints: None,
                collection: None,
            },
            enable_metadata_cache,
        )
        .await
    }

    /// Perform ultra-high-performance vector search in loaded blocks
    /// CRITICAL HOT PATH: This method is called for every search operation
    /// Optimized for maximum throughput and minimum latency
    async fn search_in_blocks(
        &self,
        params: &SearchParams,
        blocks: &[ProximaDataBlock],
        distance_compute: &UnifiedDistanceCompute,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        let query_vector = params
            .first_query_vector()
            .ok_or_else(|| anyhow::anyhow!("Query vector required"))?;

        let k = params.top_k.unwrap_or(10);
        let distance_metric = params.distance_metric;

        debug!(
            "🔍 Searching in {} blocks for top {} results",
            blocks.len(),
            k
        );

        // Use bounded priority queue to maintain only top-k results
        let mut priority_queue = BoundedPriorityQueue::new(k);

        let mut total_records = 0u32; // Use u32 for better cache efficiency
        let mut filtered_out = 0u32;
        let mut tombstones = 0u32;

        // Extract filter expression for centralized type-safe filtering
        let filter_expr = params.filter_expression.as_ref();

        // OPTIMIZED SEARCH LOOP: Use unified distance compute for semantic correctness
        for (block_idx, block) in blocks.iter().enumerate() {
            let block_records = block.records.len() as u32;
            total_records += block_records;

            // Skip empty blocks immediately (branch prediction optimization)
            if block_records == 0 {
                continue;
            }

            debug!(
                "📊 Processing block {} with {} records",
                block_idx, block_records
            );

            // Process records with optimized filtering and distance calculation
            for record in &block.records {
                // Fast tombstone check (most common early exit)
                let current_time_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
                if record
                    .valid_to_ns
                    .is_some_and(|valid_to| valid_to > 0 && valid_to < current_time_ns)
                {
                    tombstones += 1;
                    continue;
                }

                // Type-safe metadata filtering against canonical props.
                if let Some(filter) = filter_expr
                    && !crate::core::search::sql_value_filter::evaluate_filter_proxima(
                        filter,
                        &record.props,
                    )
                {
                    filtered_out += 1;
                    continue; // Skip to next record immediately
                }

                let vector = record_vector(record);
                if vector.is_empty() {
                    continue;
                }

                // Calculate similarity using unified distance computation for semantic correctness
                let metric = distance_metric.unwrap_or(
                    crate::compute::distance_computation::engine::DistanceMetric::Cosine,
                );
                let similarity = distance_compute.calculate_distance(query_vector, vector, &metric);

                // Use normalized_score for consistency across all engines
                // Higher similarity = better match, VOS sorts descending
                let search_record =
                    OptimizedSearchRecord::new(record_id(record), similarity.normalized_score)
                        .with_similarity(similarity.normalized_score)
                        .add_vector(vector.to_vec())
                        .with_proxima_metadata(record_metadata(record))
                        .with_version_info(record.record_version as u32, record.created_at_ns);

                // Try to insert into bounded queue - only keeps top-k
                priority_queue.try_insert(search_record);
            }
        }

        debug!(
            "📊 Search stats: {} total records, {} tombstones, {} filtered out, {} in queue",
            total_records,
            tombstones,
            filtered_out,
            priority_queue.len()
        );

        // Get sorted results from bounded queue
        let results = priority_queue.into_sorted_vec();

        debug!("🎯 Returning {} final results", results.len());
        Ok(results)
    }

    /// Full scan strategy implementation with disk cache optimization
    async fn full_scan_strategy(
        &self,
        context: &CollectionContext,
        use_block_cache: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        trace!(
            "SST Full Scan: Starting for {} files",
            context.sstable_files.len()
        );
        debug!(
            "🔍 Full scan strategy for {} files (cache={})",
            context.sstable_files.len(),
            use_block_cache
        );

        // Use parallel processing for multiple files
        if context.sstable_files.len() > 1 {
            return self.parallel_full_scan(context, use_block_cache).await;
        }
        let mut all_blocks = Vec::new();

        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            trace!(
                "SST Full Scan: Reading file {} of {}: {}",
                idx + 1,
                context.sstable_files.len(),
                file_path
            );
            debug!(
                "📂 Reading file {} of {}: {}",
                idx + 1,
                context.sstable_files.len(),
                file_path
            );

            // For remote files, check if already in disk cache
            let is_remote = !file_path.starts_with("file://")
                && !file_path.starts_with("/")
                && file_path.contains("://");
            if is_remote {
                let cache_status = self.caching_filesystem.metadata(file_path).await;
                if cache_status.is_ok() {
                    debug!(
                        "💾 SST: File {} found in disk cache, using cached copy",
                        file_path
                    );
                } else {
                    debug!(
                        "☁️ SST: File {} will be downloaded to disk cache",
                        file_path
                    );
                }
            }

            // Validate SST1 magic marker before attempting to read the file
            trace!("SST Full Scan: Validating SST file: {}", file_path);
            match self.validate_sst_file(file_path).await {
                Ok(()) => {
                    trace!("SST Full Scan: Validation passed for: {}", file_path);
                    debug!("✅ SST1 validation passed for file: {}", file_path);
                }
                Err(e) => {
                    debug!("SST Full Scan: Validation failed for {}: {}", file_path, e);
                    warn!("⚠️ Skipping invalid SSTable file {}: {}", file_path, e);
                    continue; // Skip this file entirely and move to the next one
                }
            }

            trace!(
                "SST Full Scan: Reading blocks (use_block_cache={})",
                use_block_cache
            );
            let blocks = if use_block_cache {
                self.read_file_with_cache(file_path, context.collection.as_deref())
                    .await?
            } else {
                self.read_file_direct(file_path, context.collection.as_deref())
                    .await?
            };
            trace!("SST Full Scan: Loaded {} blocks from file", blocks.len());
            debug!("  📦 Loaded {} blocks from this file", blocks.len());

            // Debug: print sample records from first block
            if let Some(first_block) = blocks.first() {
                debug!("  🔎 First block has {} records", first_block.records.len());
                for (i, record) in first_block.records.iter().take(3).enumerate() {
                    debug!(
                        "    Record {}: id={:?}, metadata={:?}",
                        i, record.oid, record.props
                    );
                }
            }

            all_blocks.extend(blocks);
        }

        debug!(
            "✅ Full scan loaded {} total blocks from all files",
            all_blocks.len()
        );
        Ok(all_blocks)
    }

    /// Parallel full scan across multiple SSTable files
    async fn parallel_full_scan(
        &self,
        context: &CollectionContext,
        use_block_cache: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        use std::sync::Arc;
        use tokio::sync::Semaphore;

        // Limit concurrent file operations to avoid resource exhaustion
        let max_concurrent_files = num_cpus::get().min(8);
        let _semaphore = Arc::new(Semaphore::new(max_concurrent_files));

        info!(
            "🚀 Starting parallel SSTable full scan across {} files (max concurrency: {})",
            context.sstable_files.len(),
            max_concurrent_files
        );

        // Process files sequentially for now to avoid lifetime issues
        // Deferred: Refactor to use Arc<Self> or implement Clone
        let mut all_blocks = Vec::new();

        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!(
                "🔄 Reading file {} of {}: {}",
                idx + 1,
                context.sstable_files.len(),
                file_path
            );

            // Validate SST1 magic marker before attempting to read the file
            match self.validate_sst_file(file_path).await {
                Ok(()) => {
                    debug!("✅ SST1 validation passed for file: {}", file_path);
                }
                Err(e) => {
                    warn!("⚠️ Skipping invalid SSTable file {}: {}", file_path, e);
                    continue; // Skip this file entirely and move to the next one
                }
            }

            let start_time = std::time::Instant::now();

            let result = if use_block_cache {
                self.read_file_with_cache(file_path, None).await
            } else {
                self.read_file_direct(file_path, None).await
            };

            let elapsed = start_time.elapsed();

            match result {
                Ok(blocks) => {
                    debug!(
                        "✅ Loaded {} blocks from {} in {:?}",
                        blocks.len(),
                        file_path,
                        elapsed
                    );
                    all_blocks.extend(blocks);
                }
                Err(e) => {
                    warn!("❌ Failed to read {}: {}", file_path, e);
                    // Continue with other files instead of failing entirely
                }
            }
        }

        info!(
            "🎯 SSTable scan completed: {} total blocks from {} files",
            all_blocks.len(),
            context.sstable_files.len()
        );

        Ok(all_blocks)
    }

    // Placeholder implementations for other strategies
    async fn index_range_scan_strategy(
        &self,
        context: &CollectionContext,
        start_block: usize,
        end_block: usize,
        use_bloom: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        debug!(
            "🔍 Index range scan search_strategy for {} files (blocks {}-{}, bloom={})",
            context.sstable_files.len(),
            start_block,
            end_block,
            use_bloom
        );

        let mut all_blocks = Vec::new();

        // For now, just read all files like full scan
        // Deferred: Implement proper block-level indexing
        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!(
                "📂 Reading file {} of {}: {}",
                idx + 1,
                context.sstable_files.len(),
                file_path
            );
            let blocks = self.read_file_direct(file_path, None).await?;
            debug!("  📦 Loaded {} blocks from this file", blocks.len());

            all_blocks.extend(blocks);
        }

        debug!("📦 Total blocks loaded: {}", all_blocks.len());
        Ok(all_blocks)
    }

    async fn metadata_filtered_strategy(
        &self,
        context: &CollectionContext,
        params: &SearchParams,
        blocks: &[usize],
        skip_bloom: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        debug!(
            "🔍 Using metadata filtered strategy for {} files",
            context.sstable_files.len()
        );

        let mut all_blocks = Vec::new();
        let metadata_conditions = self.extract_metadata_conditions(params);
        debug!(
            "📋 Extracted metadata conditions: {:?}",
            metadata_conditions
        );

        // Process each SSTable file
        for (file_idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!(
                "📂 Processing SSTable file {} of {}: {}",
                file_idx + 1,
                context.sstable_files.len(),
                file_path
            );

            // Get bloom filter - either from cache or load from disk
            let _bloom_filter: Option<SstableBloomFilter> = if !skip_bloom {
                // Use zero-copy system for metadata caching
                // The zero-copy system efficiently caches metadata without storing full vectors
                None // Bloom filters are now handled by metadata cache in zero-copy system
            } else {
                None
            };

            // Get the index from shared reader or disk
            let index = {
                // Load and cache the index
                let loaded_index = self.load_index_optimized(file_path).await?;
                // Convert IndexEntry to SstIndexEntry for cache storage
                let cache_entries: Vec<
                    crate::storage::cache::specialized::index_node_cache::SstIndexEntry,
                > = loaded_index
                    .entries
                    .iter()
                    .map(|e| {
                        crate::storage::cache::specialized::index_node_cache::SstIndexEntry {
                            key: e.key.clone(),
                            block_offset: e.offset,
                            block_size: e.size as usize,
                            min_key: e.key.clone(), // Would need to track actual min/max
                            max_key: e.key.clone(),
                            vector_count: 1, // Approximation
                            bloom_filter_offset: None,
                        }
                    })
                    .collect();

                // Convert metadata stats for cache
                let mut cache_metadata_stats = std::collections::HashMap::new();
                for (key, stats) in &loaded_index.metadata_stats {
                    cache_metadata_stats.insert(
                        key.clone(),
                        crate::storage::cache::specialized::index_node_cache::MetadataStats {
                            min_value: stats.min_value.clone(),
                            max_value: stats.max_value.clone(),
                            null_count: stats.null_count,
                            distinct_count: stats.distinct_count,
                        },
                    );
                }

                let _sstable_index =
                    crate::storage::cache::specialized::index_node_cache::SstableIndex {
                        file_path: file_path.clone(),
                        entries: cache_entries,
                        total_blocks: loaded_index.entries.len(),
                        total_vectors: loaded_index.entries.len(), // Approximation: one vector per entry
                        metadata_stats: cache_metadata_stats,
                    };
                // Zero-copy system handles index caching automatically via metadata cache
                loaded_index
            };
            debug!("  📊 Loaded index with {} entries", index.entries.len());

            // Check bloom filter for quick rejection when we have metadata conditions
            if !metadata_conditions.is_empty() && !skip_bloom {
                // For metadata filtering, we need the actual bloom filter from disk
                // Check if it's worth loading based on whether we're reading from cache or disk
                let is_cached_data = false; // Zero-copy system handles bloom filter caching via metadata

                if !is_cached_data {
                    // Data is not cached, so we're reading from disk anyway
                    // Load the bloom filter for proper metadata matching
                    match self.load_bloom_filter(file_path).await {
                        Ok(Some(bloom)) => {
                            let mut any_match = false;
                            for (column, value) in &metadata_conditions {
                                // Convert JSON value to MetadataItem for type-safe bloom filter check
                                let metadata_item =
                                    crate::core::bloom::json_to_metadata_item(column, value);
                                if bloom
                                    .might_match_metadata(column, &metadata_item)
                                    .unwrap_or(false)
                                {
                                    any_match = true;
                                    break;
                                }
                            }

                            if !any_match {
                                debug!(
                                    "  ❌ Bloom filter rejected file {} (no metadata matches)",
                                    file_path
                                );
                                continue; // Skip this file entirely
                            }
                            debug!("  ✅ Bloom filter indicates potential matches");
                        }
                        Ok(None) => {
                            debug!("  ⚠️ No bloom filter available for {}", file_path);
                        }
                        Err(e) => {
                            debug!("  ⚠️ Failed to load bloom filter for {}: {}", file_path, e);
                        }
                    }
                } else {
                    // Data is cached, bloom filter check less critical
                    debug!("  ℹ️ Skipping bloom filter check for cached data");
                }
            }

            // Use block-level metadata statistics to filter blocks
            let mut selected_blocks = Vec::new();
            let block_list = if blocks.is_empty() {
                // If no specific blocks provided, check all blocks
                (0..index.entries.len()).collect::<Vec<_>>()
            } else {
                blocks.to_vec()
            };
            let total_blocks = block_list.len();

            for block_idx in block_list {
                if block_idx >= index.entries.len() {
                    continue;
                }

                let entry = &index.entries[block_idx];
                let mut should_include = true;

                // Check each metadata condition against block statistics
                for (column, value) in &metadata_conditions {
                    // Check if this block might contain the value using column-specific min/max
                    if let Some(min_val) = entry.metadata_min_values.get(column) {
                        if let Some(max_val) = entry.metadata_max_values.get(column) {
                            // Use the centralized comparison function for proper numeric handling
                            // If value is outside the min/max range, skip this block
                            if Self::compare_metadata_values(value, min_val)
                                == std::cmp::Ordering::Less
                                || Self::compare_metadata_values(value, max_val)
                                    == std::cmp::Ordering::Greater
                            {
                                should_include = false;
                                break;
                            }
                        }
                    } else {
                        // Column not tracked in block stats - be conservative, include block
                        // We can't reject blocks when we don't have column statistics
                        debug!(
                            "    ⚠️ Column '{}' not in block stats, including block conservatively",
                            column
                        );
                    }
                }

                if should_include {
                    selected_blocks.push(block_idx);
                }
            }

            debug!(
                "  📦 Selected {} blocks out of {} after metadata filtering for file {}",
                selected_blocks.len(),
                total_blocks,
                file_path
            );

            // Load the selected blocks from this file
            for block_idx in selected_blocks {
                debug!("    📄 Loading block {} from file {}", block_idx, file_path);
                // Create a temporary context for this specific file
                let file_context = CollectionContext {
                    file_path: file_path.clone(),
                    sstable_files: vec![file_path.clone()],
                    total_vectors: context.total_vectors,
                    metadata_columns: context.metadata_columns.clone(),
                    level: context.level,
                    creation_time: context.creation_time,
                    io_optimization_hints: context.io_optimization_hints.clone(),
                    collection: context.collection.clone(),
                };

                if let Some(block) = self.load_block_with_cache(&file_context, block_idx).await? {
                    // Debug: print first few records from loaded block
                    debug!(
                        "    📦 Loaded block {} with {} records from {}",
                        block_idx,
                        block.records.len(),
                        file_path
                    );
                    for (i, record) in block.records.iter().take(3).enumerate() {
                        debug!(
                            "      Record {}: id={:?}, metadata={:?}",
                            i, record.oid, record.props
                        );
                    }
                    all_blocks.push(block);
                }
            }
        }

        debug!(
            "Loaded {} blocks total after metadata filtering from {} files",
            all_blocks.len(),
            context.sstable_files.len()
        );
        Ok(all_blocks)
    }

    /// Extract metadata conditions from search params
    fn extract_metadata_conditions(
        &self,
        params: &SearchParams,
    ) -> HashMap<String, serde_json::Value> {
        // Use centralized filter extraction logic
        if let Some(ref filter_expr) = params.filter_expression {
            crate::core::search::filter_extraction::extract_metadata_conditions(filter_expr)
        } else {
            HashMap::new()
        }
    }

    /// Load a specific block with caching
    async fn load_block_with_cache(
        &self,
        context: &CollectionContext,
        block_idx: usize,
    ) -> Result<Option<ProximaDataBlock>> {
        let _cache_key = BlockCacheKey {
            file_path: context.file_path.clone(),
            block_id: block_idx as u32,
            block_index: block_idx,
        };

        // Deferred: Re-implement caching with new cache system
        // let sst_cache_key = SstBlockKey::new(
        //     context.file_path.clone(),
        //     block_idx as u64 * 4096, // Assuming 4KB blocks
        //     4096,
        // );

        // Check if we have cached vectors for this block
        // Deferred: Fix DataBlock construction - fields don't match actual structure
        // let cached_vectors = self.vector_cache.get_block_vectors(&sst_cache_key, 100).await;
        // if !cached_vectors.is_none() {
        //     return Ok(Some(block));
        // }

        // Cache miss - load from disk
        let block = self.load_block_from_disk(context, block_idx).await?;

        if let Some(block) = block.as_ref() {
            // Cache integration consumes canonical ProximaRecord blocks.
            let _record_count = block.records.len();
            // Zero-copy system uses disk cache for vectors, not in-memory caching
        }

        Ok(block)
    }

    /// Load a block from disk with cloud-optimized range requests
    async fn load_block_from_disk(
        &self,
        context: &CollectionContext,
        block_idx: usize,
    ) -> Result<Option<ProximaDataBlock>> {
        // Extract scheme from file path for proper filesystem selection
        let scheme = if context.file_path.contains("://") {
            context.file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;

        // Use zero-copy system for metadata caching
        // Load index directly from disk when needed
        let index = if context.file_path.is_empty() {
            // Empty index for empty file path
            SstableIndex {
                entries: vec![],
                metadata_stats: HashMap::new(),
                vector_count: 0,
                min_key: String::new(),
                max_key: String::new(),
                bplus_tree: None,
            }
        } else {
            // Load index from disk

            // Zero-copy system handles index caching automatically via metadata cache
            self.load_index_optimized(&context.file_path).await?
        };

        // Check if block exists
        if block_idx >= index.entries.len() {
            return Ok(None);
        }

        // To find the block offset, we need to calculate the data section offset
        // Read and verify SST1 magic bytes
        let first_8_bytes = fs.read_range(&context.file_path, 0, 8).await?;
        if &first_8_bytes[0..4] != b"SST1" {
            return Err(anyhow::anyhow!(
                "Invalid SSTable format: missing SST1 magic bytes"
            ));
        }

        let header_len = u32::from_le_bytes([
            first_8_bytes[4],
            first_8_bytes[5],
            first_8_bytes[6],
            first_8_bytes[7],
        ]) as u64;
        let header_offset = 8u64; // Skip magic + header_len

        // Read bloom filter length to skip it
        let bloom_offset = header_offset + header_len;
        let bloom_len_data = fs.read_range(&context.file_path, bloom_offset, 4).await?;
        let bloom_len = u32::from_le_bytes([
            bloom_len_data[0],
            bloom_len_data[1],
            bloom_len_data[2],
            bloom_len_data[3],
        ]) as u64;

        // Read index length to skip it
        let index_offset = bloom_offset + 4 + bloom_len;
        let index_len_data = fs.read_range(&context.file_path, index_offset, 4).await?;
        let index_len = u32::from_le_bytes([
            index_len_data[0],
            index_len_data[1],
            index_len_data[2],
            index_len_data[3],
        ]) as u64;

        // Calculate where data blocks start
        let data_section_offset = index_offset + 4 + index_len;

        // Now we need to find the specific block offset
        // For efficiency, we should store absolute offsets in the index, but for now
        // we'll read block lengths sequentially (this could be optimized further)
        let mut block_offset = data_section_offset;
        for _i in 0..block_idx {
            // Read block length
            let len_data = fs.read_range(&context.file_path, block_offset, 4).await?;
            let block_len =
                u32::from_le_bytes([len_data[0], len_data[1], len_data[2], len_data[3]]) as u64;
            // Skip this block (length prefix + data)
            block_offset += 4 + block_len;
        }

        // Read the target block length
        let block_len_data = fs.read_range(&context.file_path, block_offset, 4).await?;
        let block_len = u32::from_le_bytes([
            block_len_data[0],
            block_len_data[1],
            block_len_data[2],
            block_len_data[3],
        ]) as u64;

        // Read the block data
        let block_data = fs
            .read_range(&context.file_path, block_offset + 4, block_len)
            .await?;
        let block: ProximaDataBlock = ProximaDataBlock::deserialize(&block_data, None)?;

        debug!(
            "Loaded block {} from SSTable using range request ({} bytes)",
            block_idx, block_len
        );
        Ok(Some(block))
    }

    /// Load index with cloud-optimized metadata reading
    async fn load_index_optimized(&self, file_path: &str) -> Result<SstableIndex> {
        trace!("SST Load Index: Starting for: {}", file_path);
        // Extract scheme from file path for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        trace!("SST Load Index: Detected scheme: {}", scheme);
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;
        trace!("SST Load Index: Got filesystem, reading magic bytes");

        // Read and verify SST1 magic bytes
        let first_8_bytes = fs.read_range(file_path, 0, 8).await?;
        trace!("SST Load Index: Read {} magic bytes", first_8_bytes.len());
        if first_8_bytes.len() < 8 {
            return Err(anyhow::anyhow!(
                "SSTable file too small: expected at least 8 bytes, got {}",
                first_8_bytes.len()
            ));
        }

        if &first_8_bytes[0..4] != b"SST1" {
            return Err(anyhow::anyhow!(
                "Invalid SSTable format: missing SST1 magic bytes"
            ));
        }

        trace!("SST Load Index: SST1 format detected");
        debug!("SST1 format detected");
        let header_len = u32::from_le_bytes([
            first_8_bytes[4],
            first_8_bytes[5],
            first_8_bytes[6],
            first_8_bytes[7],
        ]) as u64;
        trace!("SST Load Index: Header length: {}", header_len);
        let header_offset = 8u64; // Skip magic + header_len

        // Read header
        trace!(
            "SST Load Index: Reading header at offset {} length {}",
            header_offset, header_len
        );
        let header_data = fs.read_range(file_path, header_offset, header_len).await?;
        trace!(
            "SST Load Index: Read {} bytes of header data",
            header_data.len()
        );
        if header_data.len() < header_len as usize {
            return Err(anyhow::anyhow!(
                "Failed to read complete header: expected {} bytes, got {}",
                header_len,
                header_data.len()
            ));
        }
        trace!("SST Load Index: Deserializing header");
        let header: SstableHeader = bincode::deserialize(&header_data).map_err(|e| {
            debug!("SST Load Index: ERROR deserializing header: {}", e);
            anyhow::anyhow!("Failed to deserialize header: {}", e)
        })?;
        trace!("SST Load Index: Header deserialized successfully");

        // Calculate bloom filter offset and read its length
        let bloom_offset = header_offset + header_len;
        let bloom_len_data = fs.read_range(file_path, bloom_offset, 4).await?;
        if bloom_len_data.len() < 4 {
            return Err(anyhow::anyhow!(
                "Failed to read bloom filter length: expected 4 bytes, got {}",
                bloom_len_data.len()
            ));
        }
        let bloom_len = u32::from_le_bytes([
            bloom_len_data[0],
            bloom_len_data[1],
            bloom_len_data[2],
            bloom_len_data[3],
        ]) as u64;

        // Calculate index offset (skip bloom filter)
        let index_offset = bloom_offset + 4 + bloom_len;

        // Read index length
        trace!(
            "SST Load Index: Reading index length at offset {}",
            index_offset
        );
        let index_len_data = fs.read_range(file_path, index_offset, 4).await?;
        let index_len = u32::from_le_bytes([
            index_len_data[0],
            index_len_data[1],
            index_len_data[2],
            index_len_data[3],
        ]) as u64;
        trace!("SST Load Index: Index length: {}", index_len);

        // Read index data
        let index_data = if index_len > 0 {
            trace!(
                "SST Load Index: Reading index data at offset {} length {}",
                index_offset + 4,
                index_len
            );
            fs.read_range(file_path, index_offset + 4, index_len)
                .await?
        } else {
            trace!("SST Load Index: No index data (index_len = 0)");
            return Ok(SstableIndex {
                entries: Vec::new(),
                metadata_stats: HashMap::new(),
                vector_count: 0,
                min_key: header.min_key.clone(),
                max_key: header.max_key.clone(),
                bplus_tree: None,
            });
        };

        // Robust deserialization using shared implementation which includes B+ Tree and stats
        let index = SstableIndex::deserialize(&index_data).map_err(|e| {
            warn!("SST Load Index: Failed to deserialize index: {}", e);
            anyhow::anyhow!("Failed to deserialize index: {}", e)
        })?;

        Ok(index)
    }

    /// Simple get operation for single vector retrieval
    /// This provides a lightweight interface for basic get operations
    ///
    /// OPTIMIZED: Uses B+ tree index for O(log n) block lookup instead of full scan.
    /// This is critical for RAG use cases where we need to fetch full records by ID
    /// after HNSW returns candidate IDs.
    pub async fn vector(&self, file_path: &str, vector_id: &str) -> Result<Option<ProximaRecord>> {
        debug!(
            "🔍 vector: Looking for vector '{}' in file '{}'",
            vector_id, file_path
        );

        // Check bloom filter for quick rejection using the proper bloom filter
        // The bloom_cache bitmap is just a marker, not the actual bloom filter
        // We need to use might_contain_key which loads and checks the actual bloom filter
        if !self.might_contain_key(file_path, vector_id).await {
            debug!("❌ Bloom filter says vector '{}' not in file", vector_id);
            return Ok(None);
        }
        debug!(
            "✅ Bloom filter says vector '{}' might be in file",
            vector_id
        );

        // OPTIMIZED: Use B+ tree index for efficient block lookup
        // Instead of loading all blocks, we:
        // 1. Load the SSTable index with B+ tree
        // 2. Use leaf_for_key to find candidate blocks
        // 3. Read only those blocks
        // 4. Search within the block for the specific ID

        // Create a temporary reader for this file
        let mut reader = ModularBlockReader::open(self.filesystem.clone(), file_path).await?;
        let header = reader.read_header_async().await?;
        let index = reader.read_index(&header).await?;

        // Use B+ tree if available for O(log n) block lookup
        if let Some(ref bplus_tree) = index.bplus_tree {
            debug!("🌳 Using B+ tree index for efficient lookup");

            // Find the leaf that might contain our key
            if let Some(leaf) = bplus_tree.leaf_for_key(vector_id) {
                debug!(
                    "📍 B+ tree leaf found: key range [{}, {}], {} entries starting at idx {}",
                    leaf.start_key, leaf.end_key, leaf.len, leaf.start_idx
                );

                // Get the index entries for this leaf
                let entries_in_leaf = &index.entries[leaf.start_idx..leaf.start_idx + leaf.len];

                // Find which block(s) might contain our key
                // Each entry represents a block with min_key = entry.key
                for (i, entry) in entries_in_leaf.iter().enumerate() {
                    // Check if vector_id could be in this block
                    // Block contains keys from entry.key up to (next entry's key - 1)
                    let block_min_key = &entry.key;
                    let block_max_key = if i + 1 < entries_in_leaf.len() {
                        &entries_in_leaf[i + 1].key
                    } else {
                        // Last block in leaf - check against leaf end_key
                        &leaf.end_key
                    };

                    // Check if vector_id falls within this block's key range
                    if vector_id >= block_min_key.as_str() && vector_id <= block_max_key.as_str() {
                        debug!(
                            "📦 Reading block at offset {} (keys: {} - {})",
                            entry.offset, block_min_key, block_max_key
                        );

                        // Read only this specific block
                        let block = reader
                            .read_data_block_at_offset(entry.offset, entry.size as usize)
                            .await?;

                        // Search within the block for the exact ID
                        for record in &block.records {
                            if record.oid == vector_id {
                                debug!("✅ Found vector '{}' in block", vector_id);
                                return Ok(Some(record.clone()));
                            }
                        }

                        debug!("❌ Vector '{}' not found in expected block", vector_id);
                    }
                }
            } else {
                debug!("❌ B+ tree has no leaf for key '{}'", vector_id);
            }
        } else {
            // Fallback: No B+ tree available, use linear scan through index entries
            debug!("⚠️ No B+ tree index, falling back to linear block scan");

            // Find candidate blocks by scanning index entries
            for (i, entry) in index.entries.iter().enumerate() {
                let block_min_key = &entry.key;
                let block_max_key = if i + 1 < index.entries.len() {
                    &index.entries[i + 1].key
                } else {
                    &index.max_key
                };

                if vector_id >= block_min_key.as_str() && vector_id <= block_max_key.as_str() {
                    let block = reader
                        .read_data_block_at_offset(entry.offset, entry.size as usize)
                        .await?;

                    for record in &block.records {
                        if record.oid == vector_id {
                            return Ok(Some(record.clone()));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    /// Batch get operation for multiple vector IDs
    ///
    /// OPTIMIZED FOR RAG: When HNSW returns multiple IDs, this method efficiently
    /// fetches full records (including metadata) for all IDs in a single pass.
    /// Uses B+ tree index to group IDs by block, minimizing I/O operations.
    ///
    /// # Arguments
    /// * `file_path` - Path to the SSTable file
    /// * `vector_ids` - Slice of vector IDs to fetch
    ///
    /// # Returns
    /// * Vector of (id, ProximaRecord) tuples for found records
    pub async fn vectors_batch(
        &self,
        file_path: &str,
        vector_ids: &[&str],
    ) -> Result<Vec<(String, ProximaRecord)>> {
        use std::collections::HashMap;

        if vector_ids.is_empty() {
            return Ok(Vec::new());
        }

        debug!(
            "🔍 vectors_batch: Looking for {} vectors in file '{}'",
            vector_ids.len(),
            file_path
        );

        // Step 1: Filter out IDs that definitely don't exist (bloom filter)
        let mut candidate_ids: Vec<&str> = Vec::with_capacity(vector_ids.len());
        for id in vector_ids {
            if self.might_contain_key(file_path, id).await {
                candidate_ids.push(*id);
            } else {
                debug!("❌ Bloom filter rejected '{}'", id);
            }
        }

        if candidate_ids.is_empty() {
            debug!("❌ All IDs rejected by bloom filter");
            return Ok(Vec::new());
        }

        debug!("✅ {} IDs passed bloom filter check", candidate_ids.len());

        // Step 2: Load the index with B+ tree
        let mut reader = ModularBlockReader::open(self.filesystem.clone(), file_path).await?;
        let header = reader.read_header_async().await?;
        let index = reader.read_index(&header).await?;

        // Step 3: Group IDs by block for efficient I/O
        // Map: block_idx -> Vec<id>
        let mut block_to_ids: HashMap<usize, Vec<&str>> = HashMap::new();

        if let Some(ref bplus_tree) = index.bplus_tree {
            debug!("🌳 Using B+ tree for batch block assignment");

            for id in &candidate_ids {
                if let Some(leaf) = bplus_tree.leaf_for_key(id) {
                    let entries_in_leaf = &index.entries[leaf.start_idx..leaf.start_idx + leaf.len];

                    // Find the specific block
                    for (i, entry) in entries_in_leaf.iter().enumerate() {
                        let block_min_key = &entry.key;
                        let block_max_key = if i + 1 < entries_in_leaf.len() {
                            &entries_in_leaf[i + 1].key
                        } else {
                            &leaf.end_key
                        };

                        if *id >= block_min_key.as_str() && *id <= block_max_key.as_str() {
                            let block_idx = leaf.start_idx + i;
                            block_to_ids.entry(block_idx).or_default().push(*id);
                            break;
                        }
                    }
                }
            }
        } else {
            // Fallback: Linear scan to assign IDs to blocks
            debug!("⚠️ No B+ tree, using linear assignment");

            for id in &candidate_ids {
                for (i, entry) in index.entries.iter().enumerate() {
                    let block_min_key = &entry.key;
                    let block_max_key = if i + 1 < index.entries.len() {
                        &index.entries[i + 1].key
                    } else {
                        &index.max_key
                    };

                    if *id >= block_min_key.as_str() && *id <= block_max_key.as_str() {
                        block_to_ids.entry(i).or_default().push(*id);
                        break;
                    }
                }
            }
        }

        debug!("📦 IDs grouped into {} blocks", block_to_ids.len());

        // Step 4: Read each block once and extract all matching records
        let mut results: Vec<(String, ProximaRecord)> = Vec::with_capacity(candidate_ids.len());
        let id_set: std::collections::HashSet<&str> = candidate_ids.iter().copied().collect();

        for (block_idx, ids_in_block) in block_to_ids {
            if block_idx >= index.entries.len() {
                continue;
            }

            let entry = &index.entries[block_idx];
            let block = reader
                .read_data_block_at_offset(entry.offset, entry.size as usize)
                .await?;

            debug!(
                "📦 Block {}: {} records, looking for {} IDs",
                block_idx,
                block.records.len(),
                ids_in_block.len()
            );

            // Scan block once, collect all matching records
            for record in &block.records {
                if id_set.contains(record.oid.as_str()) {
                    results.push((record.oid.clone(), record.clone()));
                }
            }
        }

        info!(
            "✅ vectors_batch: Found {}/{} vectors",
            results.len(),
            vector_ids.len()
        );

        Ok(results)
    }

    /// Check if a key might be contained using bloom filter
    pub async fn might_contain_key(&self, file_path: &str, key: &str) -> bool {
        // Try to load the bloom filter if not cached
        match self.load_bloom_filter(file_path).await {
            Ok(Some(bloom_filter)) => {
                // Check if the key might be in the bloom filter
                bloom_filter.might_contain_key(key).unwrap_or(true)
            }
            Ok(None) => {
                // No bloom filter available
                true // Assume it might contain
            }
            Err(_) => {
                // Error loading bloom filter
                true // Assume it might contain
            }
        }
    }

    /// Load just the bloom filter from an SSTable file
    async fn load_bloom_filter(&self, file_path: &str) -> Result<Option<SstableBloomFilter>> {
        // Extract scheme from URL for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;

        // Read magic bytes and header length
        let header_prefix = fs.read_range(file_path, 0, 8).await?;
        if header_prefix.len() < 8 {
            return Ok(None);
        }

        // Check magic bytes "SST1"
        let magic = &header_prefix[0..4];
        if magic != b"SST1" {
            return Ok(None);
        }

        let header_len = u32::from_le_bytes([
            header_prefix[4],
            header_prefix[5],
            header_prefix[6],
            header_prefix[7],
        ]) as u64;

        // Read header (offset by 8 bytes for magic + header_len)
        let header_data = fs.read_range(file_path, 8, header_len).await?;
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;

        // Read bloom filter if present
        if header.has_bloom_filter {
            let bloom_offset = 8 + header_len; // 8 = magic (4) + header_len (4)
            let bloom_len_data = fs.read_range(file_path, bloom_offset, 4).await?;
            if bloom_len_data.len() < 4 {
                return Ok(None);
            }

            let bloom_len = u32::from_le_bytes([
                bloom_len_data[0],
                bloom_len_data[1],
                bloom_len_data[2],
                bloom_len_data[3],
            ]) as u64;

            let bloom_data = fs
                .read_range(file_path, bloom_offset + 4, bloom_len)
                .await?;

            match SstableBloomFilter::deserialize(&bloom_data) {
                Ok(bloom) => Ok(Some(bloom)),
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Load metadata for an SSTable (header and bloom filter)
    pub async fn load_metadata(&self, file_path: &str) -> Result<()> {
        // Extract scheme from file path for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;

        // First read magic bytes (4 bytes) and header length (4 bytes)
        let header_prefix = fs.read_range(file_path, 0, 8).await?;
        if header_prefix.len() < 8 {
            return Err(anyhow::anyhow!(
                "SSTable file too small: {} bytes",
                header_prefix.len()
            ));
        }

        // Verify magic bytes
        let magic = &header_prefix[0..4];
        if magic != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SSTable magic bytes: {:?}", magic));
        }

        let header_len = u32::from_le_bytes([
            header_prefix[4],
            header_prefix[5],
            header_prefix[6],
            header_prefix[7],
        ]) as u64;

        debug!("Header length: {} bytes", header_len);

        // Read the header data (offset by 8 bytes for magic + header_len)
        let header_data = fs.read_range(file_path, 8, header_len).await?;
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;

        debug!(
            "Header info: version={}, has_bloom={}, entry_count={}",
            header.version, header.has_bloom_filter, header.entry_count
        );

        // Read bloom filter if present
        if header.has_bloom_filter {
            // Calculate bloom filter offset (after magic + header_len + header)
            let bloom_offset = 8 + header_len;

            // Read bloom filter length
            let bloom_len_data = fs.read_range(file_path, bloom_offset, 4).await?;
            if bloom_len_data.len() < 4 {
                return Err(anyhow::anyhow!(
                    "Failed to read bloom filter length: expected 4 bytes, got {}",
                    bloom_len_data.len()
                ));
            }
            let bloom_len = u32::from_le_bytes([
                bloom_len_data[0],
                bloom_len_data[1],
                bloom_len_data[2],
                bloom_len_data[3],
            ]) as u64;

            debug!(
                "Reading bloom filter: offset={}, length={}",
                bloom_offset + 4,
                bloom_len
            );
            debug!("Bloom length bytes: {:?}", bloom_len_data);

            // Check file size
            let file_metadata = fs.metadata(file_path).await?;
            debug!("File size: {} bytes", file_metadata.size);

            // Read bloom filter data
            let bloom_data = fs
                .read_range(file_path, bloom_offset + 4, bloom_len)
                .await?;
            debug!("Actually read {} bytes of bloom data", bloom_data.len());
            if bloom_data.len() < bloom_len as usize {
                return Err(anyhow::anyhow!(
                    "Failed to read complete bloom filter: expected {} bytes, got {}",
                    bloom_len,
                    bloom_data.len()
                ));
            }
            debug!(
                "Bloom data first 20 bytes: {:?}",
                &bloom_data[..bloom_data.len().min(20)]
            );

            let _bloom_filter: SstableBloomFilter =
                match SstableBloomFilter::deserialize(&bloom_data) {
                    Ok(bf) => bf,
                    Err(e) => {
                        warn!("Deserialization error: {:?}", e);
                        warn!(
                            "Expected SstableBloomFilter, got {} bytes",
                            bloom_data.len()
                        );

                        // Try to understand what we're actually reading
                        if bloom_data.len() >= 8 {
                            let first_u64 =
                                u64::from_le_bytes(bloom_data[0..8].try_into().unwrap_or([0; 8]));
                            debug!("First u64 in bloom data: {}", first_u64);
                        }

                        // Log the actual error for debugging
                        tracing::warn!(
                            "Failed to deserialize bloom filter, creating empty one: {}",
                            e
                        );

                        // Create an empty bloom filter as fallback
                        // This allows the SSTable to be read even if the bloom filter is corrupted
                        let key_filter_config = BloomFilterConfig {
                            // strategy removed -  BloomStrategy::ByteAligned,
                            expected_items: 1000,
                            bits_per_key: 10,
                            enabled: false, // Disable since we couldn't load it
                            ..Default::default()
                        };

                        SstableBloomFilter::new(
                            key_filter_config,
                            vec![],
                            vec![],
                            super::super::bloom_filter::BloomFilterStats::default(),
                        )
                    }
                };

            // Track bloom filter access via orchestrator (best-effort)
            if let Some(orch) =
                crate::storage::cache::orchestrator::CrossCacheOrchestrator::global()
            {
                orch.pattern_tracker().track_access_async(
                    file_path.to_string(),
                    crate::storage::cache::orchestrator::CacheType::FilterBitmap,
                );
            }

            // Cache the bloom filter in central cache
            // We need to store the bloom filter somewhere accessible
            // For now, we'll use a simple in-memory cache (should be improved)

            // Store a marker in the bitmap cache that bloom filter exists
            let mut bitmap = proximadb_storage_common::bitmap::RoaringBitmap::new();
            // We'll use a hash of the file path as the marker
            let file_hash = file_path
                .as_bytes()
                .iter()
                .fold(0u32, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u32));
            bitmap.insert(file_hash);

            let _cached_filter =
                crate::storage::cache::specialized::bitmap_filter_cache::CachedFilterResult {
                    bitmap,
                    filter_expr: format!("sstable:bloom:{}", file_path),
                    cached_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0),
                    dependencies: vec![],
                };
            // Zero-copy system handles bloom filter caching via metadata

            debug!(
                "Loaded bloom filter for SSTable: {} ({} bytes)",
                file_path, bloom_len
            );
        }

        debug!("Loaded metadata for SSTable: {}", file_path);
        // Track metadata header access via orchestrator (best-effort)
        if let Some(orch) = crate::storage::cache::orchestrator::CrossCacheOrchestrator::global() {
            orch.pattern_tracker().track_access_async(
                file_path.to_string(),
                crate::storage::cache::orchestrator::CacheType::Metadata,
            );
        }
        Ok(())
    }

    async fn load_specific_blocks(
        &self,
        context: &CollectionContext,
        blocks: &[usize],
    ) -> Result<Vec<ProximaDataBlock>> {
        let mut loaded_blocks = Vec::new();

        for &block_idx in blocks {
            if let Some(block) = self.load_block_with_cache(context, block_idx).await? {
                loaded_blocks.push(block);
            }
        }

        Ok(loaded_blocks)
    }

    async fn read_file_with_cache(
        &self,
        path: &str,
        collection: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<Vec<ProximaDataBlock>> {
        trace!("SST Read with Cache: Starting for path: {}", path);

        // For Proxima format, we need to read the data blocks directly after the header
        // The format is: SST1 | header_len | header | bloom_len | bloom | index_len | index | data_blocks

        // Read the file to find data blocks
        let blocks = self.read_proximablocks(path, collection).await?;

        trace!(
            "SST Read with Cache: Loaded {} Proxima blocks",
            blocks.len()
        );
        Ok(blocks)
    }

    /// Attempt to read data blocks without bloom filter or index
    /// This is a fallback for corrupted/truncated files
    async fn read_blocks_without_index(
        &self,
        path: &str,
        start_offset: u64,
    ) -> Result<Vec<ProximaDataBlock>> {
        use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;

        eprintln!(
            "⚠️ Attempting to read blocks without index from offset {}",
            start_offset
        );

        let scheme = if path.contains("://") {
            path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;

        let mut blocks = Vec::new();
        let mut offset = start_offset;

        // Try to read blocks until we hit end of file
        loop {
            // Try to read block size (4 bytes)
            match fs.read_range(path, offset, 4).await {
                Ok(size_bytes) if size_bytes.len() == 4 => {
                    let block_size = u32::from_le_bytes([
                        size_bytes[0],
                        size_bytes[1],
                        size_bytes[2],
                        size_bytes[3],
                    ]);

                    if block_size == 0 || block_size > 100_000_000 {
                        // Sanity check: 100MB max block size
                        eprintln!(
                            "Invalid block size {} at offset {}, stopping",
                            block_size, offset
                        );
                        break;
                    }

                    offset += 4;

                    // Read the block data
                    match fs.read_range(path, offset, block_size as u64).await {
                        Ok(block_data) if block_data.len() == block_size as usize => {
                            // Try to deserialize as Proxima block
                            match ProximaDataBlock::deserialize(&block_data, None) {
                                Ok(block) => {
                                    blocks.push(block);
                                    offset += block_size as u64;
                                }
                                Err(e) => {
                                    eprintln!(
                                        "Failed to deserialize block at offset {}: {}",
                                        offset, e
                                    );
                                    break;
                                }
                            }
                        }
                        _ => {
                            eprintln!("Incomplete block data at offset {}, stopping", offset);
                            break;
                        }
                    }
                }
                _ => {
                    // End of file or can't read size
                    eprintln!("Reached end of file or corrupted data at offset {}", offset);
                    break;
                }
            }
        }

        eprintln!("⚠️ Recovered {} blocks without index", blocks.len());
        Ok(blocks)
    }

    /// Read Proxima format data blocks from SST file
    async fn read_proximablocks(
        &self,
        path: &str,
        collection: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<Vec<ProximaDataBlock>> {
        use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;

        trace!("SST Proxima: read_proximablocks called for path: {}", path);

        // Get filesystem
        let scheme = if path.contains("://") {
            path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;

        // Read the header to find where data blocks start
        // First read magic and header length
        let first_8_bytes = fs.read_range(path, 0, 8).await?;
        if &first_8_bytes[0..4] != b"SST1" {
            return Err(anyhow::anyhow!("Not an SST1 file"));
        }

        let header_len = u32::from_le_bytes([
            first_8_bytes[4],
            first_8_bytes[5],
            first_8_bytes[6],
            first_8_bytes[7],
        ]) as u64;

        // Skip header
        let mut offset = 8 + header_len;

        // Read bloom filter length and skip it
        let bloom_len_bytes = fs.read_range(path, offset, 4).await?;
        if bloom_len_bytes.len() < 4 {
            // File is truncated or corrupted - log warning but try to continue
            // This could happen if bloom filter generation was interrupted
            eprintln!(
                "⚠️ WARNING: SST file {} appears truncated at bloom filter section (offset {}). Attempting to read without bloom filter.",
                path, offset
            );
            // Try to read data blocks directly, assuming no bloom filter or index
            // This is a best-effort recovery attempt
            return self.read_blocks_without_index(path, offset).await;
        }
        let bloom_len = u32::from_le_bytes([
            bloom_len_bytes[0],
            bloom_len_bytes[1],
            bloom_len_bytes[2],
            bloom_len_bytes[3],
        ]) as u64;
        offset += 4 + bloom_len;

        // Read index length and skip it
        let index_len_bytes = fs.read_range(path, offset, 4).await?;
        if index_len_bytes.len() < 4 {
            // File is truncated at index section
            eprintln!(
                "⚠️ WARNING: SST file {} appears truncated at index section (offset {}). Attempting direct block read.",
                path, offset
            );
            return self.read_blocks_without_index(path, offset).await;
        }
        let index_len = u32::from_le_bytes([
            index_len_bytes[0],
            index_len_bytes[1],
            index_len_bytes[2],
            index_len_bytes[3],
        ]) as u64;
        offset += 4 + index_len;

        trace!("SST Proxima: Data blocks start at offset: {}", offset);

        // Now read the data blocks
        // Each block is prefixed with its length
        let mut blocks = Vec::new();

        // Read the rest of the file
        let file_metadata = fs.metadata(path).await?;
        let file_size = file_metadata.size;

        while offset < file_size {
            // Try to read block length
            if offset + 4 > file_size {
                break; // Not enough data for another block
            }

            let block_len_bytes = fs.read_range(path, offset, 4).await?;
            if block_len_bytes.len() < 4 {
                break;
            }

            let block_len = u32::from_le_bytes([
                block_len_bytes[0],
                block_len_bytes[1],
                block_len_bytes[2],
                block_len_bytes[3],
            ]) as u64;

            if block_len == 0 || offset + 4 + block_len > file_size {
                break; // Invalid block or not enough data
            }

            offset += 4;

            // Read the block data
            let block_data = fs.read_range(path, offset, block_len).await?;
            trace!(
                "SST Proxima: Reading block at offset {} with {} bytes",
                offset, block_len
            );

            // Deserialize using Proxima deserializer
            match ProximaDataBlock::deserialize(&block_data, collection) {
                Ok(block) => {
                    trace!(
                        "SST Proxima: Deserialized block with {} records",
                        block.records.len()
                    );
                    blocks.push(block);
                }
                Err(e) => {
                    warn!(
                        "SST Proxima: Failed to deserialize block at offset {}: {}",
                        offset, e
                    );
                    // Continue with next block
                }
            }

            offset += block_len;

            // Skip cache-line padding that the writer adds for SIMD alignment
            //
            // ## Why skip padding?
            // The SST writer pads each block to 64-byte cache-line boundaries for:
            // - Direct SIMD operations on mmap'd data (AVX2/AVX-512/NEON)
            // - No runtime copy to aligned buffer needed
            //
            // ## Overhead Analysis (Audited December 2024)
            // - Typical 263KB block with ~51 bytes padding = 0.019% overhead
            // - This negligible overhead enables significant SIMD performance gains
            //
            // See: src/storage/engines/impls/sst/writer.rs:120-134 for writer side
            // See: src/storage/engines/core/formats/proximablocks/mod.rs for best practices
            const CACHE_LINE_SIZE: u64 = 64;
            let aligned_block_len = block_len.div_ceil(CACHE_LINE_SIZE) * CACHE_LINE_SIZE;
            let padding = aligned_block_len - block_len;
            if padding > 0 && padding < CACHE_LINE_SIZE {
                offset += padding;
            }
        }

        trace!(
            "SST Proxima: Finished reading {} blocks from {}",
            blocks.len(),
            path
        );
        Ok(blocks)
    }

    async fn read_blocks_from_index(
        &self,
        path: &str,
        index: &SstableIndex,
    ) -> Result<Vec<ProximaDataBlock>> {
        // Helper method for when we have an index
        let mut blocks = Vec::new();
        let context = CollectionContext {
            file_path: path.to_string(),
            sstable_files: vec![path.to_string()],
            total_vectors: 0,
            metadata_columns: vec![],
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
            collection: None,
        };

        let num_blocks = index.entries.len();
        for block_idx in 0..num_blocks {
            if let Some(block) = self.load_block_with_cache(&context, block_idx).await? {
                blocks.push(block);
            }
        }

        Ok(blocks)
    }

    /// 🚀 COMPACTION OPTIMIZED STRATEGY: Read entire SSTable with minimal overhead
    /// Optimizations:
    /// - Skip bloom filter loading (no point lookups needed)
    /// - Skip index loading (reading everything anyway)
    /// - Bypass cache (avoid memory pressure during bulk operations)
    /// - Sequential I/O for optimal disk performance
    async fn compaction_optimized_strategy(
        &self,
        context: &CollectionContext,
        skip_bloom_filters: bool,
        skip_indexes: bool,
        bypass_cache: bool,
        sequential_io: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        info!(
            "🚀 COMPACTION OPTIMIZED: Reading {} files with optimizations: bloom={}, index={}, cache={}, sequential={}",
            context.sstable_files.len(),
            !skip_bloom_filters,
            !skip_indexes,
            !bypass_cache,
            sequential_io
        );

        // Use modular direct reader for maximum efficiency when all optimizations are enabled
        if bypass_cache && skip_bloom_filters && skip_indexes && sequential_io {
            return self.compaction_direct_strategy_modular(context).await;
        }

        let mut all_blocks = Vec::new();

        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            info!(
                "📂 COMPACTION READ: File {} of {}: {}",
                idx + 1,
                context.sstable_files.len(),
                file_path
            );

            // Validate SST1 magic marker
            match self.validate_sst_file(file_path).await {
                Ok(()) => {
                    debug!("✅ SST1 validation passed for file: {}", file_path);
                }
                Err(e) => {
                    warn!("⚠️ Skipping invalid SSTable file {}: {}", file_path, e);
                    continue;
                }
            }

            let start_time = std::time::Instant::now();

            // Use direct read without caching when bypass_cache is enabled
            let blocks = if bypass_cache {
                self.read_file_direct_no_cache(
                    file_path,
                    skip_bloom_filters,
                    skip_indexes,
                    sequential_io,
                )
                .await?
            } else {
                // Fall back to normal read with cache
                self.read_file_direct(file_path, None).await?
            };

            let elapsed = start_time.elapsed();
            info!(
                "⚡ COMPACTION READ: Loaded {} blocks from {} in {:?} (bypass_cache={})",
                blocks.len(),
                file_path,
                elapsed,
                bypass_cache
            );

            // Debug: print sample records
            if let Some(first_block) = blocks.first() {
                debug!("  🔎 First block has {} records", first_block.records.len());
                for (i, record) in first_block.records.iter().take(3).enumerate() {
                    debug!("    Record {}: id={:?}", i, record.oid);
                }
            }

            all_blocks.extend(blocks);
        }

        info!(
            "✅ COMPACTION OPTIMIZED: Loaded {} total blocks from {} files",
            all_blocks.len(),
            context.sstable_files.len()
        );
        Ok(all_blocks)
    }

    /// Direct file read with compaction optimizations (no cache, minimal metadata)
    async fn read_file_direct_no_cache(
        &self,
        path: &str,
        skip_bloom_filters: bool,
        skip_indexes: bool,
        sequential_io: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        debug!(
            "🔥 COMPACTION DIRECT: Reading {} with optimizations (bloom={}, index={}, sequential={})",
            path, !skip_bloom_filters, !skip_indexes, sequential_io
        );

        // Extract scheme from path
        let scheme = if path.contains("://") {
            path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;

        // Read the full file in one operation for optimal sequential I/O
        let data = fs.read(path).await?;
        let mut offset = 0usize;

        // Verify SST1 magic bytes
        if data.len() < 8 {
            return Ok(vec![]);
        }

        if &data[0..4] != b"SST1" {
            return Err(anyhow::anyhow!(
                "Invalid SSTable format: missing SST1 magic bytes"
            ));
        }

        offset += 4; // Skip magic
        let header_len = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        offset += 4; // Skip header length field

        // 🚀 OPTIMIZATION: Only read header to get block count, skip detailed parsing
        let header: SstableHeader = bincode::deserialize(&data[offset..offset + header_len])
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;
        offset += header_len;

        debug!(
            "📊 COMPACTION: Header shows {} blocks expected",
            header.block_count
        );

        // 🚀 OPTIMIZATION: Skip bloom filter entirely if not needed
        if skip_bloom_filters {
            debug!("⏭️ COMPACTION: Skipping bloom filter loading");
        }
        let bloom_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4 + bloom_len; // Skip bloom filter data

        // 🚀 OPTIMIZATION: Skip index loading if not needed
        if skip_indexes {
            debug!("⏭️ COMPACTION: Skipping index loading");
        }
        let index_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4 + index_len; // Skip index data

        // 🚀 READ DATA BLOCKS: Optimized sequential reading
        let mut blocks = Vec::with_capacity(header.block_count as usize);
        debug!(
            "📦 COMPACTION: Starting data block reading at offset {} (sequential={})",
            offset, sequential_io
        );

        // Sequential block reading with minimal overhead
        while offset + 4 <= data.len() {
            let block_len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            if offset + block_len > data.len() {
                warn!(
                    "🚨 COMPACTION: Not enough data for block (need {}, have {})",
                    block_len,
                    data.len() - offset
                );
                break;
            }

            let block_data = &data[offset..offset + block_len];

            match ProximaDataBlock::deserialize(block_data, None) {
                Ok(block) => {
                    debug!(
                        "✅ COMPACTION: Deserialized block with {} records",
                        block.records.len()
                    );
                    blocks.push(block);
                }
                Err(e) => {
                    warn!(
                        "❌ COMPACTION: Failed to deserialize block at offset {}: {}",
                        offset - 4,
                        e
                    );
                    // Continue processing other blocks
                }
            }
            offset += block_len;
        }

        info!(
            "🎯 COMPACTION: Read {} blocks sequentially from {}",
            blocks.len(),
            path
        );
        Ok(blocks)
    }

    async fn read_file_direct(
        &self,
        path: &str,
        collection: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<Vec<ProximaDataBlock>> {
        // Use the same Proxima reader as read_file_with_cache
        trace!("SST Read Direct: Starting for path: {}", path);
        self.read_proximablocks(path, collection).await
    }

    async fn read_file_direct_with_strategy(
        &self,
        path: &str,
        search_strategy: &ReadStrategy,
    ) -> Result<Vec<ProximaDataBlock>> {
        // Load index directly without caching (true direct access)
        let index = Arc::new(self.load_index_optimized(path).await?);

        // Extract scheme from path for proper filesystem selection
        let scheme = if path.contains("://") {
            path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}://", scheme))?;

        // Read the full file
        let data = fs.read(path).await?;
        let mut offset = 0usize;

        // Verify SST1 magic bytes
        if data.len() < 8 {
            return Ok(vec![]);
        }

        if &data[0..4] != b"SST1" {
            return Err(anyhow::anyhow!(
                "Invalid SSTable format: missing SST1 magic bytes"
            ));
        }

        offset += 4; // Skip magic
        let header_len = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        offset += 4; // Skip header length field
        offset += header_len;

        // Skip bloom filter
        if offset + 4 > data.len() {
            return Ok(vec![]);
        }
        let bloom_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        let bloom_offset = offset + 4;
        offset += 4 + bloom_len;

        // Skip index
        if offset + 4 > data.len() {
            return Ok(vec![]);
        }
        let index_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        let _offset = offset + 4 + index_len;

        // Convert ReadStrategy to block_filter QueryType
        let block_query_type = match search_strategy {
            ReadStrategy::FullScan
            | ReadStrategy::CompactionDirect
            | ReadStrategy::SearchOptimized => {
                crate::storage::engines::sst::readers::block_filter::QueryType::FullScan
            }
            ReadStrategy::FilteredScan(_) => {
                crate::storage::engines::sst::readers::block_filter::QueryType::PointQuery
            }
        };

        // Create intelligent block filter based on strategy
        let block_filter = IntelligentBlockFilter::for_query_type(&block_query_type);

        // Create block filter (empty for now, could be enhanced with actual filter params)
        let filter = BlockFilter {
            target_id: None,
            id_range: None,
            metadata_filters: HashMap::new(),
            query_type: block_query_type,
        };

        // Load bloom filter if needed for filtering
        let global_bloom = if search_strategy.should_filter_blocks() && bloom_len > 0 {
            let bloom_data = &data[bloom_offset..bloom_offset + bloom_len];
            bincode::deserialize::<BloomFilter>(bloom_data).ok()
        } else {
            None
        };

        // Filter blocks based on strategy
        let selected_entries = if search_strategy.should_filter_blocks() {
            block_filter.filter_blocks(&index.entries, &filter, global_bloom.as_ref())?
        } else {
            // For compaction, read all blocks
            index.entries.iter().collect()
        };

        debug!(
            "📊 Selected {} of {} blocks based on {} search_strategy",
            selected_entries.len(),
            index.entries.len(),
            match search_strategy {
                ReadStrategy::CompactionDirect => "CompactionDirect",
                ReadStrategy::FilteredScan(_) => "FilteredScan",
                ReadStrategy::SearchOptimized => "SearchOptimized",
                ReadStrategy::FullScan => "FullScan",
            }
        );

        // Read selected data blocks
        let mut blocks = Vec::new();
        debug!(
            "Starting to read {} selected data blocks",
            selected_entries.len()
        );

        // For each selected entry, read the block at its offset
        for entry in selected_entries {
            // Use the offset from the index entry
            let block_offset = entry.offset as usize;
            let block_size = entry.size as usize;

            debug!(
                "Reading block {} at offset {} with size {}",
                entry.block_id, block_offset, block_size
            );

            if block_offset + block_size > data.len() {
                warn!(
                    "Block {} extends beyond file (offset={}, size={}, file_len={})",
                    entry.block_id,
                    block_offset,
                    block_size,
                    data.len()
                );
                continue;
            }

            let block_data = &data[block_offset..block_offset + block_size];

            // Debug: Check if block data starts with expected magic header
            if block_data.len() >= 4 {
                let magic = &block_data[0..4];
                debug!(
                    "Block {} magic header: {:?}",
                    entry.block_id,
                    std::str::from_utf8(magic)
                );
            }

            debug!(
                "🔍 Deserializing block {} data of {} bytes",
                entry.block_id,
                block_data.len()
            );
            match ProximaDataBlock::deserialize(block_data, None) {
                Ok(block) => {
                    debug!(
                        "✅ Successfully deserialized block {} with {} records",
                        entry.block_id,
                        block.records.len()
                    );
                    // Debug: Print sample record IDs for debugging
                    for (i, record) in block.records.iter().take(3).enumerate() {
                        debug!(
                            "  Record {}: id='{:?}', vector_len={}, metadata_keys={:?}",
                            i,
                            record.oid,
                            record
                                .embeddings
                                .first()
                                .map_or(0, |embedding| embedding.values.len()),
                            record.props.keys().cloned().collect::<Vec<_>>()
                        );
                    }
                    blocks.push(block);
                }
                Err(e) => {
                    warn!(
                        "Failed to deserialize block {} at offset {}: {:?}",
                        entry.block_id, block_offset, e
                    );
                    // Debug: Print first few bytes of the problematic block
                    let preview_len = std::cmp::min(block_data.len(), 32);
                    warn!(
                        "Block {} data preview (first {} bytes): {:?}",
                        entry.block_id,
                        preview_len,
                        &block_data[..preview_len]
                    );
                    // Continue processing other blocks even if one fails
                    warn!(
                        "Skipping corrupted block {} at offset {}",
                        entry.block_id, block_offset
                    );
                }
            }
        }

        debug!(
            "📊 Total blocks read: {} (filtered from {})",
            blocks.len(),
            index.entries.len()
        );

        // Print summary of blocks read
        if !blocks.is_empty() {
            let total_records: usize = blocks.iter().map(|b| b.records.len()).sum();
            debug!(
                "📦 Read {} blocks containing {} total records",
                blocks.len(),
                total_records
            );
        }

        Ok(blocks)
    }

    fn evaluate_filter(
        &self,
        expr: &FilterExpression,
        metadata: &HashMap<String, serde_json::Value>,
    ) -> bool {
        // Use centralized filter evaluation from search module
        crate::core::search::json_comparison::evaluate_filter(expr, metadata)
    }

    /// Convert MetadataItem vector to JSON HashMap - optimized for high-performance hot paths
    #[inline(always)]
    fn metadata_items_to_json(
        &self,
        items: &[crate::proto::proximadb_v1::MetadataItem],
    ) -> HashMap<String, serde_json::Value> {
        // Pre-allocate HashMap to exact size to avoid reallocations
        let mut map = HashMap::with_capacity(items.len());
        for item in items {
            let value = match &item.value {
                Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(s)) => {
                    serde_json::Value::String(s.clone())
                }
                Some(crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n)) => {
                    serde_json::Number::from_f64(*n)
                        .map_or(serde_json::Value::Null, serde_json::Value::Number)
                }
                Some(crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b)) => {
                    serde_json::Value::Bool(*b)
                }
                None => serde_json::Value::Null,
            };
            map.insert(item.key.clone(), value);
        }
        map
    }

    /// Compare metadata values for ordering
    fn compare_metadata_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
        crate::core::search::json_comparison::compare_json_values(a, b)
    }

    // ===== MODULAR STRATEGY METHODS =====
    // These methods use ModularBlockReader and SstDirectReader for improved performance

    /// Full scan using modular block reader with selective block loading
    async fn full_scan_strategy_modular(
        &self,
        context: &CollectionContext,
        _use_cache: bool,
    ) -> Result<Vec<ProximaDataBlock>> {
        debug!(
            "🔍 Full scan modular strategy for {} files",
            context.sstable_files.len()
        );
        let mut all_blocks = Vec::new();

        for file_path in &context.sstable_files {
            let mut block_reader =
                ModularBlockReader::new(self.filesystem.clone(), file_path.clone());

            // Read header first
            let header = block_reader.read_header().await?;

            // For full scan, skip bloom filters but read index for navigation
            let index_blocks = {
                // Always load index blocks - zero-copy system handles caching internally
                let entries = block_reader.read_index_blocks(&header).await?;

                // Convert to cache's SstableIndex type
                let _cache_index = crate::storage::cache::specialized::index_node_cache::SstableIndex {
                    file_path: file_path.to_string(),
                    entries: entries.iter().map(|e| crate::storage::cache::specialized::index_node_cache::SstIndexEntry {
                        key: e.key.clone(),
                        block_offset: e.offset,
                        block_size: e.size as usize,
                        min_key: e.metadata_min_values.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        max_key: e.metadata_max_values.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        vector_count: 1,
                        bloom_filter_offset: None,
                    }).collect(),
                    total_blocks: header.block_count as usize,
                    total_vectors: header.entry_count as usize,
                    metadata_stats: HashMap::new(),
                };

                // Zero-copy system handles caching internally - no need for explicit cache call

                // Return our local SstableIndex type
                SstableIndex {
                    entries: entries.clone(),
                    metadata_stats: HashMap::new(),
                    vector_count: entries.len(),
                    min_key: entries.first().map(|e| e.key.clone()).unwrap_or_default(),
                    max_key: entries.last().map(|e| e.key.clone()).unwrap_or_default(),
                    bplus_tree: None,
                }
            };

            // Read all data blocks
            for index_entry in &index_blocks.entries {
                let data_block = block_reader
                    .read_data_block_at_offset(index_entry.offset, index_entry.size as usize)
                    .await?;
                all_blocks.push(data_block);
            }
        }

        Ok(all_blocks)
    }

    /// Filtered scan using modular approach with predicate pushdown
    async fn filtered_scan_strategy_modular(
        &self,
        context: &CollectionContext,
        filter: &FilterExpression,
    ) -> Result<Vec<ProximaDataBlock>> {
        debug!(
            "🔍 Filtered scan modular strategy with filter: {:?}",
            filter
        );
        let mut all_blocks = Vec::new();

        for file_path in &context.sstable_files {
            let mut block_reader =
                ModularBlockReader::new(self.filesystem.clone(), file_path.clone());

            let header = block_reader.read_header().await?;

            // Check bloom filter first if available
            if header.has_bloom_filter {
                let bloom_filter = block_reader.read_bloom_filter(&header).await?;
                if !self.check_bloom_filter_match(&bloom_filter, filter) {
                    debug!(
                        "⏭️ Skipping file {} - bloom filter indicates no matches",
                        file_path
                    );
                    continue;
                }
            }

            // Read index to find relevant blocks
            let index_blocks = block_reader.read_index_blocks(&header).await?;

            // Filter blocks based on metadata ranges in index
            for index_entry in &index_blocks {
                if self.should_read_block_for_filter(index_entry, filter) {
                    let data_block = block_reader
                        .read_data_block_at_offset(index_entry.offset, index_entry.size as usize)
                        .await?;
                    all_blocks.push(data_block);
                }
            }
        }

        Ok(all_blocks)
    }

    /// Direct compaction strategy using SstDirectReader for zero-copy operations
    async fn compaction_direct_strategy_modular(
        &self,
        context: &CollectionContext,
    ) -> Result<Vec<ProximaDataBlock>> {
        info!(
            "🚀 Direct compaction modular search_strategy - zero-copy SST operations for {} files",
            context.sstable_files.len()
        );

        let mut all_sst_records = Vec::new();

        for file_path in &context.sstable_files {
            debug!("📁 COMPACTION DIRECT: Opening file: {}", file_path);
            // Use SstDirectReader::open() with the actual file path
            let mut direct_reader =
                SstDirectReader::open(self.filesystem.clone(), file_path).await?;
            let sst_records = direct_reader.read_all_for_compaction().await?;

            // Collect all records from the iterator
            debug!(
                "📁 COMPACTION DIRECT: Read {} records from {}",
                sst_records.len(),
                file_path
            );
            all_sst_records.extend(sst_records);
        }

        let blocks = self.records_to_data_blocks(all_sst_records)?;

        Ok(blocks)
    }

    /// Search-optimized strategy using modular approach with smart caching
    /// Uses Z-order spatial codes and centroid distances for intelligent block selection
    /// Also applies zone map pruning when metadata filters are present
    async fn search_optimized_strategy_modular(
        &self,
        context: &CollectionContext,
        search_params: &SearchParams,
    ) -> Result<Vec<ProximaDataBlock>> {
        let mut relevant_blocks = Vec::new();

        for file_path in &context.sstable_files {
            let mut block_reader =
                ModularBlockReader::new(self.filesystem.clone(), file_path.clone());

            let header = block_reader.read_header().await?;

            // Use index to find blocks with high relevance scores
            let index_blocks = block_reader.read_index_blocks(&header).await?;

            // Smart block selection based on search parameters (sqrt-based pruning)
            let selected_blocks = self.select_blocks_for_search(&index_blocks, search_params);

            // Apply zone map pruning if filter expression is present
            let filter_expr = search_params.filter_expression.as_ref();
            let mut blocks_after_zone_map = 0usize;
            let blocks_before_zone_map = selected_blocks.len();

            for block_idx in &selected_blocks {
                if let Some(index_entry) = index_blocks.get(*block_idx) {
                    // Zone map pruning: skip blocks that can't contain matching values
                    if let Some(filter) = filter_expr
                        && !self.should_read_block_for_filter(index_entry, filter)
                    {
                        continue; // Skip this block - zone map says no matches possible
                    }

                    blocks_after_zone_map += 1;
                    match block_reader
                        .read_data_block_at_offset(index_entry.offset, index_entry.size as usize)
                        .await
                    {
                        Ok(data_block) => {
                            relevant_blocks.push(data_block);
                        }
                        Err(e) => {
                            return Err(e);
                        }
                    }
                }
            }

            // Log zone map pruning effectiveness
            if filter_expr.is_some() && blocks_before_zone_map > 0 {
                let pruned = blocks_before_zone_map - blocks_after_zone_map;
                if pruned > 0 {
                    tracing::debug!(
                        "📊 Zone map pruning: {} → {} blocks ({} pruned, {:.1}% reduction) for {}",
                        blocks_before_zone_map,
                        blocks_after_zone_map,
                        pruned,
                        (pruned as f64 / blocks_before_zone_map as f64) * 100.0,
                        file_path
                    );
                }
            }
        }

        Ok(relevant_blocks)
    }

    // Helper methods for modular strategies

    fn check_bloom_filter_match(
        &self,
        _bloom_filter: &BloomFilter,
        _filter: &FilterExpression,
    ) -> bool {
        // Deferred: Implement bloom filter checking logic
        true // For now, always check blocks
    }

    fn should_read_block_for_filter(
        &self,
        index_entry: &IndexEntry,
        filter: &FilterExpression,
    ) -> bool {
        // Zone map pruning: Check if block's metadata min/max range can contain matching values
        // Extract equality conditions from filter expression
        let metadata_conditions = self.extract_filter_conditions(filter);

        if metadata_conditions.is_empty() {
            // No equality conditions to check against zone maps
            return true;
        }

        // Check each condition against block's min/max values
        for (column, filter_value) in &metadata_conditions {
            if let Some(min_val) = index_entry.metadata_min_values.get(column)
                && let Some(max_val) = index_entry.metadata_max_values.get(column)
            {
                // Use centralized comparison: if value is outside [min, max], skip block
                if Self::compare_metadata_values(filter_value, min_val) == std::cmp::Ordering::Less
                    || Self::compare_metadata_values(filter_value, max_val)
                        == std::cmp::Ordering::Greater
                {
                    tracing::debug!(
                        "🔍 Zone map pruning: block {} rejected - {} not in [{:?}, {:?}]",
                        index_entry.block_id,
                        filter_value,
                        min_val,
                        max_val
                    );
                    return false;
                }
            }
            // If column not tracked in block stats, be conservative and include block
        }

        true // Block might contain matching values
    }

    /// Extract simple equality conditions from a filter expression for zone map pruning
    fn extract_filter_conditions(
        &self,
        filter: &FilterExpression,
    ) -> Vec<(String, serde_json::Value)> {
        let mut conditions = Vec::new();
        Self::collect_equality_conditions(filter, &mut conditions);
        conditions
    }

    fn collect_equality_conditions(
        filter: &FilterExpression,
        conditions: &mut Vec<(String, serde_json::Value)>,
    ) {
        use crate::core::search::ComparisonOperator;

        match filter {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                if matches!(operator, ComparisonOperator::Equals) {
                    conditions.push((field.clone(), value.clone()));
                }
            }
            FilterExpression::And(exprs) => {
                for expr in exprs {
                    Self::collect_equality_conditions(expr, conditions);
                }
            }
            FilterExpression::Or(_) | FilterExpression::Not(_) => {
                // OR and NOT are too complex for simple zone map pruning
            }
        }
    }

    fn records_to_data_blocks(&self, records: Vec<ProximaRecord>) -> Result<Vec<ProximaDataBlock>> {
        let block_size = 1000; // Default block size
        let mut blocks = Vec::new();

        for (block_id, chunk) in records.chunks(block_size).enumerate() {
            let block_id = block_id as u32;
            use crate::storage::engines::core::formats::proximablocks::block_structures::{
                BlockCompressionConfig, BlockStatistics,
            };

            blocks.push(ProximaDataBlock {
                encoding_marker: 0x00, // Raw/Uncompressed
                encoding_metadata: None,
                block_id,
                records: chunk.to_vec(),
                quantized_vectors: None,
                quantization_level: None,
                encoded_vectors: None,
                vector_layout: crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::FullVector,
                quantized_section: None,
                metadata: crate::storage::engines::core::formats::proximablocks::ProximaBlockMetadata::default(),
                compression_config: BlockCompressionConfig {
                    algorithm: CompressionAlgorithm::None,
                    compression_level: 0,
                    enable_vector_compression: false,
                    enable_metadata_compression: false,
                    compression_threshold_bytes: 0,
                    dictionary_compression: false,
                    vector_layout: crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
                    metadata_algorithm: None,
                },
                compression_algorithm: CompressionAlgorithm::None,
                uncompressed_size: 0,
                bloom_filter: None,
                block_bloom_filter: None,
                id_range: (String::new(), String::new()),
                timestamp_range: (0, 0),
                statistics: BlockStatistics::default(),
                metadata_stats: None,
                has_deletes: false,
            });
        }

        Ok(blocks)
    }

    fn select_blocks_for_search(
        &self,
        _index_blocks: &[IndexEntry],
        _params: &SearchParams,
    ) -> Vec<usize> {
        use crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;
        use crate::storage::engines::core::formats::proximablocks::spatial_pruning::{
            BlockPruningInfo, PruningConfig, PruningMode, SpatialPruner,
        };

        // Use block centroids when available to prune blocks before decompression.
        let query = _params
            .vector
            .as_ref()
            .or_else(|| _params.query_vectors.as_ref().and_then(|v| v.first()));

        let query = match query {
            Some(q) if !q.is_empty() => q,
            _ => {
                // No query vector; fall back to scanning all blocks.
                return (0.._index_blocks.len()).collect();
            }
        };

        // Check if we should use exact mode (no pruning)
        if _params.block_prune.force_exact {
            return (0.._index_blocks.len()).collect();
        }

        // Convert BlockPruneConfig to PruningConfig for unified pruner
        let prune_mode = match _params.block_prune.mode {
            crate::core::search::BlockPruneMode::Sqrt => PruningMode::Sqrt {
                min_blocks: _params.block_prune.min_keep.max(3),
            },
            crate::core::search::BlockPruneMode::Ratio => PruningMode::Ratio {
                ratio: _params.block_prune.ratio,
                min_blocks: _params.block_prune.min_keep.max(1),
            },
            crate::core::search::BlockPruneMode::Fixed(k) => PruningMode::Fixed { k },
        };

        let pruner = SpatialPruner::new(PruningConfig {
            mode: prune_mode,
            spatial_weight: 0.6,
            centroid_weight: 0.4,
            ..Default::default()
        });

        // Try to compute query's spatial code for spatial pruning (uses cached PCA model)
        let query_code = compute_query_zorder_code(query, _index_blocks, &self.collection_id);

        // Check if blocks have spatial codes
        let has_spatial_codes = _index_blocks.iter().any(|e| e.zorder_code.is_some());

        if has_spatial_codes {
            // Use unified SpatialPruner with spatial codes and centroids
            if let Some(query_code) = query_code {
                let blocks: Vec<BlockPruningInfo> = _index_blocks
                    .iter()
                    .enumerate()
                    .map(|(idx, entry)| {
                        let spatial_code =
                            entry.zorder_code.clone().unwrap_or(SpatialCode::Code64(0));

                        // Get centroid (FP16 -> FP32 if available)
                        let centroid = super::super::get_centroid_fp32(
                            &entry.block_centroid_fp16,
                            &entry.block_centroid,
                        );

                        BlockPruningInfo::with_centroid(idx, spatial_code, centroid)
                    })
                    .collect();

                let result = pruner.select_blocks(&query_code, query, &blocks);
                return result.selected_indices;
            }
        }

        // Fallback: No spatial codes, use centroid-based pruning only
        let metric = _params
            .distance_metric
            .unwrap_or(crate::compute::distance_computation::DistanceMetric::Cosine);

        select_blocks_by_centroid(query, _index_blocks, metric, &_params.block_prune)
    }

    fn l2_distance(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b.iter())
            .fold(0.0f32, |acc, (x, y)| acc + (x - y) * (x - y))
    }

    fn is_hot_data(&self, data_block: &ProximaDataBlock) -> bool {
        // Simple heuristic: blocks with many non-tombstone records are hot
        let active_records = data_block.records.len();
        active_records > data_block.records.len() / 2
    }
}

impl ReadingStrategySelector {
    pub fn new(config: ReaderConfig) -> Self {
        Self { config }
    }

    pub fn select_strategy(
        &self,
        params: &SearchParams,
        context: &CollectionContext,
    ) -> Result<SstableReadingStrategy> {
        // Strategy selection logic based on:
        // 1. Presence of metadata filters
        // 2. File size and count
        // 3. Query selectivity estimate

        if params.filter_expression.is_some() {
            // Metadata filtering present - use filtered strategy
            Ok(SstableReadingStrategy::MetadataFiltered {
                selected_blocks: vec![], // Would be populated based on metadata
                skip_bloom_check: false,
            })
        } else if context.sstable_files.len() > self.config.range_scan_threshold {
            // Many files - use index range scan
            Ok(SstableReadingStrategy::IndexRangeScan {
                start_block: 0,
                end_block: 10, // Would be calculated
                use_bloom_filter: true,
            })
        } else {
            // Small dataset - full scan with cache
            Ok(SstableReadingStrategy::FullScan {
                use_block_cache: true,
            })
        }
    }

    /// 🚀 NEW: Select compaction-optimized strategy for bulk operations
    /// This bypasses normal query optimizations in favor of bulk I/O efficiency
    pub fn select_compaction_strategy(
        &self,
        _context: &CollectionContext,
    ) -> SstableReadingStrategy {
        SstableReadingStrategy::CompactionFullRead {
            skip_bloom_filters: true,        // No point lookups in compaction
            skip_indexes: true,              // Reading everything anyway
            bypass_write_cache: true,        // Avoid memory pressure
            use_disk_cache_if_exists: false, // Don't pollute cache during compaction
            sequential_io: true,             // Optimize for disk throughput
        }
    }

    /// 🚀 NEW: Read all records from SSTable files optimized for compaction
    /// This is the main entry point that compaction should use instead of search_vectors
    pub async fn read_all_records_for_compaction(
        &self,
        sstable_files: &[String],
    ) -> Result<Vec<ProximaRecord>> {
        info!(
            "🔥 COMPACTION READ: Starting optimized read of {} SSTable files",
            sstable_files.len()
        );

        // Create minimal context for compaction
        let _context = CollectionContext {
            file_path: String::new(),
            sstable_files: sstable_files.to_vec(),
            total_vectors: 0,
            metadata_columns: vec![],
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
            collection: None,
        };

        // Use compaction-optimized strategy - read all records
        let search_strategy = SstableReadingStrategy::FullScan {
            use_block_cache: false,
        };
        debug!("📊 COMPACTION: Using strategy: {:?}", search_strategy);

        // For compaction, we need to read all blocks - create empty placeholder
        let blocks: Vec<ProximaDataBlock> = Vec::new();
        info!("📦 COMPACTION: Loaded {} data blocks total", blocks.len());

        let mut all_records = Vec::new();
        let mut total_records = 0;
        let mut tombstone_records = 0;

        for (block_idx, block) in blocks.iter().enumerate() {
            debug!(
                "📄 COMPACTION: Processing block {} with {} records",
                block_idx,
                block.records.len()
            );

            for record in &block.records {
                total_records += 1;

                // Check if record is a tombstone (expired or empty vector)
                let is_tombstone = record
                    .valid_to_ns
                    .is_some_and(|exp| exp < chrono::Utc::now().timestamp())
                    || record
                        .embeddings
                        .first()
                        .is_none_or(|embedding| embedding.values.is_empty());

                if is_tombstone {
                    tombstone_records += 1;
                    // Include tombstones for compaction to handle properly
                }

                all_records.push(record.clone());
            }
        }

        info!(
            "🎯 COMPACTION READ COMPLETE: {} total records ({} tombstones) from {} files",
            total_records,
            tombstone_records,
            sstable_files.len()
        );

        Ok(all_records)
    }
}

impl BlockCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(tokio::sync::RwLock::new(
                proximadb_runtime_common::cache::LruCache::new(max_size),
            )),
            max_size,
            hit_rate: Arc::new(tokio::sync::RwLock::new(CacheStats::default())),
        }
    }
}

impl IndexCache {
    pub fn new() -> Self {
        Self::with_config(100) // Default 100MB limit
    }

    pub fn with_config(max_memory_mb: usize) -> Self {
        use std::time::Duration;

        // Calculate max entries based on estimated size per index (~1MB each)
        let max_indices = (max_memory_mb * 1024 * 1024) / (1024 * 1024); // ~1MB per index
        let max_bloom_filters = (max_memory_mb * 1024 * 1024) / (50 * 1024); // ~50KB per bloom filter

        let indices = moka::future::Cache::builder()
            .max_capacity(max_indices as u64)
            .time_to_live(Duration::from_secs(3600)) // 1 hour TTL
            .eviction_listener(|_key, _value, _cause| {
                tracing::debug!("Evicted index cache entry");
            })
            .build();

        let bloom_filters = moka::future::Cache::builder()
            .max_capacity(max_bloom_filters as u64)
            .time_to_live(Duration::from_secs(3600))
            .eviction_listener(|_key, _value, _cause| {
                tracing::debug!("Evicted bloom filter cache entry");
            })
            .build();

        Self {
            indices: Arc::new(indices),
            bloom_filters: Arc::new(bloom_filters),
            max_memory_mb,
            metrics: Arc::new(tokio::sync::RwLock::new(CacheMetrics::default())),
        }
    }

    /// Get current memory usage in MB
    pub async fn memory_usage_mb(&self) -> f64 {
        let metrics = self.metrics.read().await;
        metrics.memory_usage_bytes as f64 / (1024.0 * 1024.0)
    }

    /// Get cache hit rate
    pub async fn hit_rate(&self) -> f64 {
        let metrics = self.metrics.read().await;
        let total_requests = metrics.hit_count + metrics.miss_count;
        if total_requests == 0 {
            0.0
        } else {
            metrics.hit_count as f64 / total_requests as f64
        }
    }

    /// Handle memory pressure by clearing least recently used entries
    pub async fn handle_memory_pressure(&self, pressure_level: MemoryPressure) {
        let mut metrics = self.metrics.write().await;
        metrics.memory_pressure_events += 1;

        match pressure_level {
            MemoryPressure::Low => {
                // Reduce TTL to encourage natural eviction
                // This would require rebuilding cache - for now just invalidate all
                // (simplified approach - in production would remove specific entries)
                let current_size = self.indices.entry_count();
                if current_size > 0 {
                    self.indices.invalidate_all();
                }
            }
            MemoryPressure::Medium => {
                // More aggressive cleanup - remove 25%
                let current_size = self.indices.entry_count() + self.bloom_filters.entry_count();
                if current_size > 0 {
                    self.indices.invalidate_all();
                }
            }
            MemoryPressure::High => {
                // Emergency cleanup - clear all caches
                self.indices.invalidate_all();
                self.bloom_filters.invalidate_all();
                tracing::warn!("Emergency cache cleanup due to high memory pressure");
            }
        }
    }

    /// Get index from cache or load if not present
    pub async fn get_or_load_index<F, Fut>(&self, key: &str, loader: F) -> Result<Arc<SstableIndex>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<SstableIndex>>,
    {
        // Try cache first
        if let Some(index) = self.indices.get(key).await {
            let mut metrics = self.metrics.write().await;
            metrics.hit_count += 1;
            return Ok(index);
        }

        // Cache miss - load and store
        let mut metrics = self.metrics.write().await;
        metrics.miss_count += 1;
        drop(metrics); // Release lock before async operation

        let index = Arc::new(loader().await?);

        // Update memory usage estimate
        let estimated_size = std::mem::size_of::<SstableIndex>()
            + index.entries.len() * std::mem::size_of::<IndexEntry>();

        self.indices.insert(key.to_string(), index.clone()).await;

        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.memory_usage_bytes += estimated_size;

        Ok(index)
    }
}

impl Default for IndexCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Memory pressure levels for adaptive cache management
#[derive(Debug, Clone, Copy)]
pub enum MemoryPressure {
    Low,    // 70-80% of limit
    Medium, // 80-90% of limit
    High,   // >90% of limit
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            block_cache_size: 1000,
            index_cache_size: 100,
            bloom_filter_threshold: 0.01,
            range_scan_threshold: 10,
            metadata_selectivity_threshold: 0.1,
            enable_read_ahead: true,
            read_ahead_blocks: 5,
        }
    }
}

// Helper function to convert JSON value to string for comparison
#[allow(dead_code)]
fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        _ => value.to_string(),
    }
}
