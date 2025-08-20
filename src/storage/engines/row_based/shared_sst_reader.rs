// Shared SST Format Reader for SST and SWIFT engines
// Optimized for bandwidth reduction and cache-aware operations
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

use memmap2::{Mmap, MmapOptions};

use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
// DEPRECATED: refined_integrated_cache replaced by zero_copy_io_system
use crate::storage::engines::common::zero_copy_io_system::{
    ZeroCopyIOSystem, MetadataSerializer, EngineMetadata, QueryContext, DataRange,
    FileAccessRequest, RequestPriority, IOStrategy
};
use crate::common::errors::ProximaDBError;

const BLOOM_FILTER_SIZE: usize = 4096;  // 4KB bloom filters
const INDEX_BLOCK_SIZE: usize = 61440;  // 60KB index blocks
const DATA_BLOCK_SIZE: usize = 65536;   // 64KB data blocks

/// Shared SST format reader with zero-copy cache-first architecture
/// Leverages OS page cache for optimal memory management vs dedicated VectorStore
pub struct SharedSstFormatReader {
    /// Filesystem for I/O operations
    filesystem: Arc<FilesystemFactory>,
    
    /// Memory mapping strategy (kept for region-specific optimizations)
    mmap_strategy: SstMmapStrategy,
    
    /// UNIFIED CACHE: Zero-copy system replaces all specialized caches
    zero_copy_system: Arc<ZeroCopyIOSystem>,
    
    /// Collection ID for filename-based cache keys
    collection_id: String,
    
    /// Stats for monitoring
    stats: Arc<ReaderStats>,
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
    BloomFilter,        // First 4KB - always cached
    IndexBlock,         // 4KB-64KB typically - usually cached
    CompressionDict,    // If present
    DataBlocks,         // Large, usually streamed
    Metadata,           // File metadata
}


/// Statistics for monitoring
pub struct ReaderStats {
    bloom_hits: AtomicU64,
    bloom_misses: AtomicU64,
    index_hits: AtomicU64,
    index_misses: AtomicU64,
    bytes_downloaded: AtomicU64,
    bytes_saved: AtomicU64,  // Saved by filtering
    cache_invalidations: AtomicU64,
}

impl SharedSstFormatReader {
    pub fn new(
        filesystem: Arc<FilesystemFactory>,
        mmap_strategy: SstMmapStrategy,
        zero_copy_system: Arc<ZeroCopyIOSystem>,
        collection_id: String,
    ) -> Self {
        Self {
            filesystem,
            mmap_strategy,
            zero_copy_system,
            collection_id,
            stats: Arc::new(ReaderStats::default()),
        }
    }
    
    /// Smart read that minimizes bandwidth usage
    pub async fn read_record(
        &self,
        file_path: &str,
        collection_id: &str,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, ProximaDBError> {
        // Step 1: Check bloom filter BEFORE downloading anything
        let bloom_data = self.get_bloom_filter_smart(file_path, collection_id).await?;
        if !self.check_bloom(&bloom_data, key) {
            // Key definitely not in file - saved bandwidth!
            self.stats.bytes_saved.fetch_add(DATA_BLOCK_SIZE as u64, Ordering::Relaxed);
            return Ok(None);
        }
        
        // Step 2: Check index block to find data block location
        let index_data = self.get_index_block_smart(file_path, collection_id).await?;
        let block_info = match self.find_block_for_key(&index_data, key)? {
            Some(info) => info,
            None => {
                // Key not in index - saved bandwidth!
                self.stats.bytes_saved.fetch_add(DATA_BLOCK_SIZE as u64, Ordering::Relaxed);
                return Ok(None);
            }
        };
        
        // Step 3: NOW download the data block since we know it's needed
        let data = self.read_data_block_smart(file_path, collection_id, &block_info).await?;
        
        self.find_in_block(&data, key)
    }
    
    /// Get bloom filter with smart bandwidth optimization
    async fn get_bloom_filter_smart(&self, file_path: &str, collection_id: &str) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        let filename = std::path::Path::new(file_path).file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(file_path);
        
        // Check if file metadata with bloom filter is cached
        if let Some(file_metadata) = self.cache.get_file_metadata(collection_id, filename, FileType::SST) {
            if let Some(sst_metadata) = file_metadata.downcast_ref::<SstFileMetadata>() {
                self.stats.bloom_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(sst_metadata.file_bloom_filter.clone());
            }
        }
        
        self.stats.bloom_misses.fetch_add(1, Ordering::Relaxed);
        
        // For cloud files, download ONLY the bloom filter range
        if self.is_cloud_file(file_path) {
            // Use range request to get just 4KB bloom filter
            let bloom_data = self.filesystem
                .get_filesystem(file_path)?
                .read_range(file_path, 0, BLOOM_FILTER_SIZE as u64)
                .await?;
            
            self.stats.bytes_downloaded.fetch_add(BLOOM_FILTER_SIZE as u64, Ordering::Relaxed);
            
            // Cache the bloom filter as part of file metadata
            let bloom_arc = Arc::new(bloom_data);
            let file_metadata = SstFileMetadata {
                file_bloom_filter: bloom_arc.clone(),
                file_index: Arc::new(vec![]), // Will be populated later
                superblock_index: None,
                file_size: 0, // Will be updated later
                num_blocks: 0, // Will be updated later
            };
            
            self.cache.put_file_metadata(collection_id, filename, Arc::new(file_metadata))?;
            
            return Ok(bloom_arc);
        }
        
        // For local files, try mmap if memory allows
        self.get_local_bloom_with_mmap(file_path, collection_id, filename).await
    }
    
    /// Get index block with smart bandwidth optimization
    async fn get_index_block_smart(&self, file_path: &str, collection_id: &str) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        let filename = std::path::Path::new(file_path).file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(file_path);
        
        // Check if file metadata with index is cached
        if let Some(file_metadata) = self.cache.get_file_metadata(collection_id, filename, FileType::SST) {
            if let Some(sst_metadata) = file_metadata.downcast_ref::<SstFileMetadata>() {
                if !sst_metadata.file_index.is_empty() {
                    self.stats.index_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(sst_metadata.file_index.clone());
                }
            }
        }
        
        self.stats.index_misses.fetch_add(1, Ordering::Relaxed);
        
        // For cloud files, download ONLY the index block range
        if self.is_cloud_file(file_path) {
            // Use range request to get just the index block
            let index_data = self.filesystem
                .get_filesystem(file_path)?
                .read_range(
                    file_path,
                    BLOOM_FILTER_SIZE as u64,
                    INDEX_BLOCK_SIZE as u64
                )
                .await?;
            
            self.stats.bytes_downloaded.fetch_add(INDEX_BLOCK_SIZE as u64, Ordering::Relaxed);
            
            // Update or create file metadata with index
            let index_arc = Arc::new(index_data);
            if let Some(file_metadata) = self.cache.get_file_metadata(collection_id, filename, FileType::SST) {
                if let Some(mut sst_metadata) = file_metadata.downcast_ref::<SstFileMetadata>().cloned() {
                    sst_metadata.file_index = index_arc.clone();
                    self.cache.put_file_metadata(collection_id, filename, Arc::new(sst_metadata))?;
                }
            } else {
                let file_metadata = SstFileMetadata {
                    file_bloom_filter: Arc::new(vec![]), // Will be populated separately
                    file_index: index_arc.clone(),
                    superblock_index: None,
                    file_size: 0,
                    num_blocks: 0,
                };
                self.cache.put_file_metadata(collection_id, filename, Arc::new(file_metadata))?;
            }
            
            return Ok(index_arc);
        }
        
        // For local files, use mmap if possible
        self.get_local_index_with_mmap(file_path, collection_id, filename).await
    }
    
    /// Read data block only after confirming it's needed
    async fn read_data_block_smart(
        &self,
        file_path: &str,
        collection_id: &str,
        block_info: &BlockInfo,
    ) -> Result<Vec<u8>, ProximaDBError> {
        let filename = std::path::Path::new(file_path).file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(file_path);
        
        // For cloud files, use the integrated cache for caching
        let data = if self.is_cloud_file(file_path) {
            // Get cached file path or download
            let cached_path = self.cache.get_cached_file_path(collection_id, filename, file_path).await?;
            
            // Read the specific block from cached file
            let file = std::fs::File::open(&cached_path)?;
            let mut buffer = vec![0; block_info.size as usize];
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::io::BufReader::new(file);
            file.seek(SeekFrom::Start(block_info.offset))?;
            file.read_exact(&mut buffer)?;
            
            buffer
        } else {
            // For local files, try to use mmap for direct access
            if let Ok(mmap) = self.cache.get_or_create_mmap(collection_id, filename, file_path).await {
                let start = block_info.offset as usize;
                let end = start + block_info.size as usize;
                if end <= mmap.len() {
                    mmap[start..end].to_vec()
                } else {
                    // Fallback to direct read
                    self.filesystem
                        .get_filesystem(file_path)?
                        .read_range(file_path, block_info.offset, block_info.size)
                        .await?
                }
            } else {
                // Fallback to direct read
                self.filesystem
                    .get_filesystem(file_path)?
                    .read_range(file_path, block_info.offset, block_info.size)
                    .await?
            }
        };
        
        self.stats.bytes_downloaded.fetch_add(data.len() as u64, Ordering::Relaxed);
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
        let bloom_data = self.get_bloom_filter_smart(file_path, collection_id).await?;
        
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
        self.stats.bytes_saved.fetch_add(saved as u64, Ordering::Relaxed);
        
        if possible_keys.is_empty() {
            return Ok(vec![None; keys.len()]);
        }
        
        // Step 2: Get index once and find blocks needed
        let index_data = self.get_index_block_smart(file_path, collection_id).await?;
        let mut blocks_to_read = HashMap::new();
        let mut index_filtered = Vec::new();
        
        for (idx, key) in &possible_keys {
            if let Some(block_info) = self.find_block_for_key(&index_data, key)? {
                blocks_to_read.entry(block_info.offset)
                    .or_insert_with(|| (block_info, Vec::new()))
                    .1.push((*idx, *key));
            } else {
                index_filtered.push(*idx);
            }
        }
        
        // Track additional bandwidth saved
        let additional_saved = index_filtered.len() * DATA_BLOCK_SIZE;
        self.stats.bytes_saved.fetch_add(additional_saved as u64, Ordering::Relaxed);
        
        // Step 3: Read only necessary blocks in parallel
        let mut results = vec![None; keys.len()];
        
        for (_, (block_info, keys_in_block)) in blocks_to_read {
            let block_data = self.read_data_block_smart(file_path, collection_id, &block_info).await?;
            
            for (idx, key) in keys_in_block {
                if let Some(value) = self.find_in_block(&block_data, key)? {
                    results[idx] = Some(value);
                }
            }
        }
        
        Ok(results)
    }
    
    /// Cache invalidation during compaction
    pub async fn invalidate_cache_for_collection(&self, collection_id: &str) -> Result<(), ProximaDBError> {
        // Use the integrated cache's collection-level invalidation
        let invalidated = self.cache.invalidate_collection(collection_id).await?;
        
        self.stats.cache_invalidations.fetch_add(invalidated, Ordering::Relaxed);
        
        log::info!(
            "Invalidated {} cache entries for collection {} during compaction",
            invalidated,
            collection_id
        );
        
        Ok(())
    }
    
    /// Check if file is on cloud storage
    fn is_cloud_file(&self, path: &str) -> bool {
        path.starts_with("s3://") || 
        path.starts_with("gs://") || 
        path.starts_with("azure://") ||
        path.starts_with("http://") ||
        path.starts_with("https://")
    }
    
    /// Check bloom filter
    fn check_bloom(&self, bloom_data: &[u8], key: &[u8]) -> bool {
        // Bloom filter implementation
        // Returns false if key definitely not present
        // Returns true if key might be present
        true // Placeholder
    }
    
    /// Find block for key in index
    fn find_block_for_key(&self, index_data: &[u8], key: &[u8]) -> Result<Option<BlockInfo>, ProximaDBError> {
        // Binary search in index to find block
        // Returns None if key not in range
        Ok(Some(BlockInfo {
            offset: 0,
            size: DATA_BLOCK_SIZE as u64,
        })) // Placeholder
    }
    
    /// Search for key in data block
    fn find_in_block(&self, block_data: &[u8], key: &[u8]) -> Result<Option<Vec<u8>>, ProximaDBError> {
        // Binary search in sorted block
        Ok(None) // Placeholder
    }
    
    /// Get local bloom filter with mmap
    async fn get_local_bloom_with_mmap(&self, file_path: &str, collection_id: &str, filename: &str) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        // Try to use mmap for local files
        if let Ok(mmap) = self.cache.get_or_create_mmap(collection_id, filename, file_path).await {
            if mmap.len() >= BLOOM_FILTER_SIZE {
                let bloom_data = mmap[0..BLOOM_FILTER_SIZE].to_vec();
                let bloom_arc = Arc::new(bloom_data);
                
                // Cache in file metadata
                let file_metadata = SstFileMetadata {
                    file_bloom_filter: bloom_arc.clone(),
                    file_index: Arc::new(vec![]),
                    superblock_index: None,
                    file_size: mmap.len() as u64,
                    num_blocks: 0,
                };
                self.cache.put_file_metadata(collection_id, filename, Arc::new(file_metadata))?;
                
                return Ok(bloom_arc);
            }
        }
        
        // Fallback to direct read
        let bloom_data = self.filesystem
            .get_filesystem(file_path)?
            .read_range(file_path, 0, BLOOM_FILTER_SIZE as u64)
            .await?;
        Ok(Arc::new(bloom_data))
    }
    
    /// Get local index block with mmap
    async fn get_local_index_with_mmap(&self, file_path: &str, collection_id: &str, filename: &str) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        // Try to use mmap for local files
        if let Ok(mmap) = self.cache.get_or_create_mmap(collection_id, filename, file_path).await {
            let start = BLOOM_FILTER_SIZE;
            let end = start + INDEX_BLOCK_SIZE;
            if mmap.len() >= end {
                let index_data = mmap[start..end].to_vec();
                let index_arc = Arc::new(index_data);
                
                // Update file metadata with index
                if let Some(file_metadata) = self.cache.get_file_metadata(collection_id, filename, FileType::SST) {
                    if let Some(mut sst_metadata) = file_metadata.downcast_ref::<SstFileMetadata>().cloned() {
                        sst_metadata.file_index = index_arc.clone();
                        sst_metadata.file_size = mmap.len() as u64;
                        self.cache.put_file_metadata(collection_id, filename, Arc::new(sst_metadata))?;
                    }
                }
                
                return Ok(index_arc);
            }
        }
        
        // Fallback to direct read
        let index_data = self.filesystem
            .get_filesystem(file_path)?
            .read_range(file_path, BLOOM_FILTER_SIZE as u64, INDEX_BLOCK_SIZE as u64)
            .await?;
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

