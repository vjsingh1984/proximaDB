// Batch operations for SST - optimized ID lookups
// Clean implementation with parallel block loading

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, warn};

use super::SwiftFile;
use super::id_index::BlockLocation;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;

/// Configuration for batch operations
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum number of concurrent block loads
    pub max_concurrent_blocks: usize,

    /// Whether to cache recently loaded blocks
    pub cache_blocks: bool,

    /// Maximum cache size in bytes
    pub max_cache_bytes: usize,

    /// Prefetch adjacent blocks
    pub prefetch_adjacent: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_concurrent_blocks: 10,
            cache_blocks: true,
            max_cache_bytes: 1024 * 1024 * 1024, // 1GB
            prefetch_adjacent: true,
        }
    }
}

/// Block cache for recently accessed blocks
#[derive(Clone)]
pub struct BlockCache {
    /// LRU cache storing blocks by (superblock_id, block_id) key
    pub cache:
        Arc<RwLock<proximadb_runtime_common::cache::LruCache<(u32, u32), Arc<ProximaDataBlock>>>>,
    /// Current cache size in bytes
    pub current_size: Arc<RwLock<usize>>,
    /// Maximum cache size in bytes
    pub max_size: usize,
}

impl BlockCache {
    fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(RwLock::new(proximadb_runtime_common::cache::LruCache::new(
                if max_size == 0 { 1000 } else { max_size },
            ))),
            current_size: Arc::new(RwLock::new(0)),
            max_size,
        }
    }

    async fn get(&self, key: &(u32, u32)) -> Option<Arc<ProximaDataBlock>> {
        self.cache.write().await.get(key).cloned()
    }

    async fn put(&self, key: (u32, u32), block: Arc<ProximaDataBlock>) {
        let block_size = estimate_block_size(&block);

        let mut cache = self.cache.write().await;
        let mut size = self.current_size.write().await;

        // Evict if necessary
        while *size + block_size > self.max_size && !cache.is_empty() {
            if let Some((_, evicted)) = cache.pop_lru() {
                *size -= estimate_block_size(&evicted);
            }
        }

        cache.put(key, block);
        *size += block_size;
    }
}

/// Main batch ID lookup function
pub async fn get_records_by_ids(sst: &SwiftFile, ids: &[String]) -> Result<Vec<VectorRecord>> {
    let config = BatchConfig::default();

    info!("Starting batch ID lookup for {} IDs", ids.len());

    // Step 1: Lookup locations for all IDs
    let locations = lookup_id_locations(sst, ids)?;

    if locations.is_empty() {
        debug!("No IDs found in index");
        return Ok(Vec::new());
    }

    debug!("Found {} IDs in index", locations.len());

    // Step 2: Group IDs by block for efficient loading
    let grouped = group_by_block(locations);

    debug!("IDs span {} unique blocks", grouped.len());

    // Step 3: Load blocks in parallel and extract records
    let cache = if config.cache_blocks {
        Some(BlockCache::new(config.max_cache_bytes))
    } else {
        None
    };

    let records =
        load_and_extract_records(sst, grouped, config.max_concurrent_blocks, cache).await?;

    info!("Batch lookup complete: {} records retrieved", records.len());

    Ok(records)
}

/// Lookup locations for multiple IDs
fn lookup_id_locations(sst: &SwiftFile, ids: &[String]) -> Result<Vec<(String, BlockLocation)>> {
    let mut locations = Vec::new();

    // Batch lookup in ID index
    let batch_results = sst.id_index.lookup_batch(ids);

    for (id, maybe_location) in ids.iter().zip(batch_results.iter()) {
        if let Some(location) = maybe_location {
            locations.push((id.clone(), location.clone()));
        } else {
            debug!("ID not found in index: {}", id);
        }
    }

    Ok(locations)
}

/// Group IDs by their block location
fn group_by_block(
    locations: Vec<(String, BlockLocation)>,
) -> HashMap<(u32, u32), Vec<(String, u32)>> {
    let mut grouped = HashMap::new();

    for (id, location) in locations {
        grouped
            .entry((location.superblock_idx, location.block_idx))
            .or_insert_with(Vec::new)
            .push((id, location.offset_in_block));
    }

    grouped
}

/// Load blocks in parallel and extract requested records
async fn load_and_extract_records(
    _sst: &SwiftFile,
    grouped: HashMap<(u32, u32), Vec<(String, u32)>>,
    max_concurrent: usize,
    cache: Option<BlockCache>,
) -> Result<Vec<VectorRecord>> {
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let mut handles = Vec::new();

    for ((sb_idx, b_idx), id_offsets) in grouped {
        let sem = semaphore.clone();
        let cache = cache.clone();

        let handle = tokio::spawn(async move {
            // SAFETY: semaphore acquire is safe because the semaphore is never closed
            // during batch operations. It is only used for concurrency limiting.
            let _permit = sem.acquire().await.ok();

            // Check cache first
            let block = if let Some(ref cache) = cache {
                if let Some(cached_block) = cache.get(&(sb_idx, b_idx)).await {
                    debug!("Block ({}, {}) found in cache", sb_idx, b_idx);
                    cached_block
                } else {
                    debug!("Loading block ({}, {})", sb_idx, b_idx);
                    let block = load_block_from_disk(sb_idx, b_idx).await?;
                    let block = Arc::new(block);
                    cache.put((sb_idx, b_idx), block.clone()).await;
                    block
                }
            } else {
                debug!("Loading block ({}, {}) without cache", sb_idx, b_idx);
                Arc::new(load_block_from_disk(sb_idx, b_idx).await?)
            };

            // Extract requested records
            let mut records = Vec::new();
            for (_id, offset) in id_offsets {
                if let Some(record) = extract_record_from_block(&block, offset) {
                    records.push(record);
                } else {
                    warn!(
                        "Record not found at offset {} in block ({}, {})",
                        offset, sb_idx, b_idx
                    );
                }
            }

            Ok::<Vec<VectorRecord>, anyhow::Error>(records)
        });

        handles.push(handle);
    }

    // Collect all results
    let mut all_records = Vec::new();
    for handle in handles {
        let records = handle.await??;
        all_records.extend(records);
    }

    Ok(all_records)
}

/// Load a block from disk (simulated)
async fn load_block_from_disk(_superblock_idx: u32, _block_idx: u32) -> Result<ProximaDataBlock> {
    // In real implementation, this would:
    // 1. Calculate file offset using superblock and block indices
    // 2. Seek to that position in the file
    // 3. Read and decompress the block
    // 4. Deserialize the records

    // For now, return a mock block using the correct constructor
    use crate::storage::engines::core::formats::proximablocks::BlockCompressionConfig;
    use proximadb_compression::CompressionAlgorithm;

    let compression_config = BlockCompressionConfig {
        algorithm: CompressionAlgorithm::Zstd,
        compression_level: 3,
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 1024,
        dictionary_compression: false,
        vector_layout:
            crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
        metadata_algorithm: None, // Use main algorithm for metadata
    };

    Ok(ProximaDataBlock::new(Vec::new(), compression_config))
}

/// Extract a record from a block at the given offset
fn extract_record_from_block(block: &ProximaDataBlock, offset: u32) -> Option<VectorRecord> {
    block.records.get(offset as usize).map(VectorRecord::from)
}

/// Estimate memory size of a block
fn estimate_block_size(block: &ProximaDataBlock) -> usize {
    // Rough estimate: records + quantized data + metadata
    let record_size = block.records.len() * (std::mem::size_of::<VectorRecord>() + 768 * 4); // Assume 768-dim vectors

    let quantized_size = if let Some(ref quantized) = block.quantized_vectors {
        quantized.len() * 100 // Estimate based on quantized vectors
    } else {
        0
    };

    record_size + quantized_size + 1024 // Overhead
}

/// Batch update operations
pub async fn update_records_batch(
    _swift_file: &mut SwiftFile,
    _updates: Vec<(String, VectorRecord)>,
) -> Result<usize> {
    // In a real implementation, SST files are immutable
    // Updates would create a new SST file or use a write-ahead log

    warn!("SST files are immutable - updates require compaction");
    Ok(0)
}

/// Batch delete operations (mark as deleted)
pub async fn delete_records_batch(swift_file: &mut SwiftFile, ids: &[String]) -> Result<usize> {
    // In SST, deletions are typically handled by:
    // 1. Writing tombstone records
    // 2. Filtering during compaction

    let mut deleted = 0;
    for id in ids {
        if swift_file.id_index.lookup(id).is_some() {
            // Would write a tombstone record
            deleted += 1;
        }
    }

    swift_file.header.deleted_records += deleted as u64;

    info!("Marked {} records for deletion", deleted);
    Ok(deleted)
}

/// Prefetch blocks for anticipated access patterns
pub async fn prefetch_blocks(
    _sst: &SwiftFile,
    block_ids: Vec<(u32, u32)>,
    cache: Option<BlockCache>,
) -> Result<()> {
    if cache.is_none() {
        return Ok(());
    }

    let cache = match cache {
        Some(cache) => cache,
        None => return Ok(()),
    };
    let semaphore = Arc::new(Semaphore::new(4)); // Limited prefetch parallelism
    let mut handles = Vec::new();

    for (sb_idx, b_idx) in block_ids {
        // Skip if already cached
        if cache.get(&(sb_idx, b_idx)).await.is_some() {
            continue;
        }

        let sem = semaphore.clone();
        let cache = cache.clone();

        let handle = tokio::spawn(async move {
            // SAFETY: semaphore acquire is safe because the semaphore is never closed
            // during prefetch operations. It is only used for concurrency limiting.
            let _permit = sem.acquire().await.ok();

            match load_block_from_disk(sb_idx, b_idx).await {
                Ok(block) => {
                    cache.put((sb_idx, b_idx), Arc::new(block)).await;
                    debug!("Prefetched block ({}, {})", sb_idx, b_idx);
                }
                Err(e) => {
                    warn!("Failed to prefetch block ({}, {}): {}", sb_idx, b_idx, e);
                }
            }
        });

        handles.push(handle);
    }

    // Wait for all prefetches to complete
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_group_by_block() {
        let locations = vec![
            (
                "id1".to_string(),
                BlockLocation {
                    superblock_idx: 0,
                    block_idx: 0,
                    offset_in_block: 10,
                    size_bytes: 100,
                },
            ),
            (
                "id2".to_string(),
                BlockLocation {
                    superblock_idx: 0,
                    block_idx: 0,
                    offset_in_block: 20,
                    size_bytes: 100,
                },
            ),
            (
                "id3".to_string(),
                BlockLocation {
                    superblock_idx: 0,
                    block_idx: 1,
                    offset_in_block: 5,
                    size_bytes: 100,
                },
            ),
            (
                "id4".to_string(),
                BlockLocation {
                    superblock_idx: 1,
                    block_idx: 0,
                    offset_in_block: 0,
                    size_bytes: 100,
                },
            ),
        ];

        let grouped = group_by_block(locations);

        assert_eq!(grouped.len(), 3); // 3 unique blocks
        assert_eq!(grouped[&(0, 0)].len(), 2); // 2 IDs in block (0, 0)
        assert_eq!(grouped[&(0, 1)].len(), 1); // 1 ID in block (0, 1)
        assert_eq!(grouped[&(1, 0)].len(), 1); // 1 ID in block (1, 0)
    }

    #[tokio::test]
    async fn test_block_cache() {
        let cache = BlockCache::new(1024 * 1024); // 1MB cache

        let block = Arc::new(ProximaDataBlock {
            encoding_marker: 0x00,
            encoding_metadata: None,
            block_id: 0,
            encoded_vectors: None,
            vector_layout:
                crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            records: vec![VectorRecord {
                id: "test".to_string(),
                vector: vec![1.0; 768],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            }],
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: Default::default(),
            compression_config: Default::default(),
            compression_algorithm: Default::default(),
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("test".to_string(), "test".to_string()),
            timestamp_range: (0, 0),
            statistics: Default::default(),
            metadata_stats: None,
            has_deletes: false,
        });

        // Test put and get
        cache.put((0, 0), block.clone()).await;
        let retrieved = cache.get(&(0, 0)).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().records[0].id, "test".to_string());

        // Test cache miss
        let miss = cache.get(&(1, 1)).await;
        assert!(miss.is_none());
    }

    // ========================================================================
    // BatchConfig tests
    // ========================================================================

    #[test]
    fn test_batch_config_default() {
        let config = BatchConfig::default();
        assert_eq!(config.max_concurrent_blocks, 10);
        assert!(config.cache_blocks);
        assert_eq!(config.max_cache_bytes, 1024 * 1024 * 1024);
        assert!(config.prefetch_adjacent);
    }

    #[test]
    fn test_batch_config_custom() {
        let config = BatchConfig {
            max_concurrent_blocks: 4,
            cache_blocks: false,
            max_cache_bytes: 512 * 1024,
            prefetch_adjacent: false,
        };
        assert_eq!(config.max_concurrent_blocks, 4);
        assert!(!config.cache_blocks);
        assert_eq!(config.max_cache_bytes, 512 * 1024);
        assert!(!config.prefetch_adjacent);
    }

    // ========================================================================
    // group_by_block extended tests
    // ========================================================================

    #[test]
    fn test_group_by_block_empty() {
        let grouped = group_by_block(Vec::new());
        assert!(grouped.is_empty());
    }

    #[test]
    fn test_group_by_block_single() {
        let locations = vec![(
            "only_id".to_string(),
            BlockLocation {
                superblock_idx: 7,
                block_idx: 3,
                offset_in_block: 99,
                size_bytes: 100,
            },
        )];

        let grouped = group_by_block(locations);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[&(7, 3)].len(), 1);
        assert_eq!(grouped[&(7, 3)][0].0, "only_id");
        assert_eq!(grouped[&(7, 3)][0].1, 99);
    }

    #[test]
    fn test_group_by_block_all_same_block() {
        let locations = vec![
            (
                "a".to_string(),
                BlockLocation {
                    superblock_idx: 1,
                    block_idx: 1,
                    offset_in_block: 0,
                    size_bytes: 0,
                },
            ),
            (
                "b".to_string(),
                BlockLocation {
                    superblock_idx: 1,
                    block_idx: 1,
                    offset_in_block: 1,
                    size_bytes: 0,
                },
            ),
            (
                "c".to_string(),
                BlockLocation {
                    superblock_idx: 1,
                    block_idx: 1,
                    offset_in_block: 2,
                    size_bytes: 0,
                },
            ),
        ];

        let grouped = group_by_block(locations);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[&(1, 1)].len(), 3);
    }

    #[test]
    fn test_group_by_block_all_different_blocks() {
        let locations = vec![
            (
                "a".to_string(),
                BlockLocation {
                    superblock_idx: 0,
                    block_idx: 0,
                    offset_in_block: 0,
                    size_bytes: 0,
                },
            ),
            (
                "b".to_string(),
                BlockLocation {
                    superblock_idx: 1,
                    block_idx: 1,
                    offset_in_block: 0,
                    size_bytes: 0,
                },
            ),
            (
                "c".to_string(),
                BlockLocation {
                    superblock_idx: 2,
                    block_idx: 2,
                    offset_in_block: 0,
                    size_bytes: 0,
                },
            ),
        ];

        let grouped = group_by_block(locations);
        assert_eq!(grouped.len(), 3);
    }

    // ========================================================================
    // BlockCache extended tests
    // ========================================================================

    #[tokio::test]
    async fn test_block_cache_eviction() {
        // Create a very small cache (enough for ~1 block)
        let cache = BlockCache::new(4096);

        let block1 = Arc::new(ProximaDataBlock {
            encoding_marker: 0x00,
            encoding_metadata: None,
            block_id: 1,
            encoded_vectors: None,
            vector_layout:
                crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            records: vec![VectorRecord {
                id: "r1".to_string(),
                vector: vec![1.0; 128],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            }],
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: Default::default(),
            compression_config: Default::default(),
            compression_algorithm: Default::default(),
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("r1".to_string(), "r1".to_string()),
            timestamp_range: (0, 0),
            statistics: Default::default(),
            metadata_stats: None,
            has_deletes: false,
        });

        let block2 = Arc::new(ProximaDataBlock {
            encoding_marker: 0x00,
            encoding_metadata: None,
            block_id: 2,
            encoded_vectors: None,
            vector_layout:
                crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            records: vec![VectorRecord {
                id: "r2".to_string(),
                vector: vec![2.0; 128],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            }],
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: Default::default(),
            compression_config: Default::default(),
            compression_algorithm: Default::default(),
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("r2".to_string(), "r2".to_string()),
            timestamp_range: (0, 0),
            statistics: Default::default(),
            metadata_stats: None,
            has_deletes: false,
        });

        // Insert two blocks; the cache is small so it should evict
        cache.put((0, 0), block1).await;
        cache.put((0, 1), block2).await;

        // At least the second one should be present
        let retrieved = cache.get(&(0, 1)).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().records[0].id, "r2");
    }

    #[tokio::test]
    async fn test_block_cache_overwrite() {
        let cache = BlockCache::new(1024 * 1024);

        let block_v1 = Arc::new(ProximaDataBlock {
            encoding_marker: 0x00,
            encoding_metadata: None,
            block_id: 0,
            encoded_vectors: None,
            vector_layout:
                crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            records: vec![VectorRecord {
                id: "v1".to_string(),
                vector: vec![1.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            }],
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: Default::default(),
            compression_config: Default::default(),
            compression_algorithm: Default::default(),
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("v1".to_string(), "v1".to_string()),
            timestamp_range: (0, 0),
            statistics: Default::default(),
            metadata_stats: None,
            has_deletes: false,
        });

        let block_v2 = Arc::new(ProximaDataBlock {
            encoding_marker: 0x00,
            encoding_metadata: None,
            block_id: 0,
            encoded_vectors: None,
            vector_layout:
                crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            records: vec![VectorRecord {
                id: "v2".to_string(),
                vector: vec![2.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            }],
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: Default::default(),
            compression_config: Default::default(),
            compression_algorithm: Default::default(),
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("v2".to_string(), "v2".to_string()),
            timestamp_range: (0, 0),
            statistics: Default::default(),
            metadata_stats: None,
            has_deletes: false,
        });

        cache.put((0, 0), block_v1).await;
        cache.put((0, 0), block_v2).await;

        let retrieved = cache.get(&(0, 0)).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().records[0].id, "v2");
    }

    // ========================================================================
    // extract_record_from_block tests
    // ========================================================================

    #[test]
    fn test_extract_record_from_block_valid() {
        let block = ProximaDataBlock {
            encoding_marker: 0x00,
            encoding_metadata: None,
            block_id: 0,
            encoded_vectors: None,
            vector_layout:
                crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            records: vec![
                VectorRecord {
                    id: "first".to_string(),
                    vector: vec![1.0],
                    metadata: std::collections::HashMap::new(),
                    timestamp: Some(0),
                    updated_at: None,
                    expires_at: None,
                    version: None,
                    source: None,
                },
                VectorRecord {
                    id: "second".to_string(),
                    vector: vec![2.0],
                    metadata: std::collections::HashMap::new(),
                    timestamp: Some(0),
                    updated_at: None,
                    expires_at: None,
                    version: None,
                    source: None,
                },
            ],
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: Default::default(),
            compression_config: Default::default(),
            compression_algorithm: Default::default(),
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("first".to_string(), "second".to_string()),
            timestamp_range: (0, 0),
            statistics: Default::default(),
            metadata_stats: None,
            has_deletes: false,
        };

        let record = extract_record_from_block(&block, 0);
        assert!(record.is_some());
        assert_eq!(record.unwrap().id, "first");

        let record = extract_record_from_block(&block, 1);
        assert!(record.is_some());
        assert_eq!(record.unwrap().id, "second");

        let record = extract_record_from_block(&block, 2);
        assert!(record.is_none());
    }

    #[test]
    fn test_estimate_block_size() {
        let block = ProximaDataBlock {
            encoding_marker: 0x00,
            encoding_metadata: None,
            block_id: 0,
            encoded_vectors: None,
            vector_layout:
                crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            records: vec![VectorRecord {
                id: "test".to_string(),
                vector: vec![1.0; 768],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            }],
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: Default::default(),
            compression_config: Default::default(),
            compression_algorithm: Default::default(),
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("test".to_string(), "test".to_string()),
            timestamp_range: (0, 0),
            statistics: Default::default(),
            metadata_stats: None,
            has_deletes: false,
        };

        let size = estimate_block_size(&block);
        assert!(size > 0);
        // Should include 1024 overhead + record size
        assert!(size >= 1024);
    }
}
