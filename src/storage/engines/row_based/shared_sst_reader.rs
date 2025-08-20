// Shared SST Format Reader for SST and SWIFT engines
// Optimized for bandwidth reduction and cache-aware operations

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use dashmap::DashMap;
use memmap2::{Mmap, MmapOptions};
use tokio::sync::RwLock;

use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
use crate::storage::cache::memory_pressure::MemoryPressureMonitor;
use crate::storage::cache::access_pattern::AccessPatternTracker;
use crate::common::errors::ProximaDBError;

const BLOOM_FILTER_SIZE: usize = 4096;  // 4KB bloom filters
const INDEX_BLOCK_SIZE: usize = 61440;  // 60KB index blocks
const DATA_BLOCK_SIZE: usize = 65536;   // 64KB data blocks

/// Shared SST format reader used by both SST and SWIFT engines
pub struct SharedSstFormatReader {
    /// Filesystem for I/O operations
    filesystem: Arc<FilesystemFactory>,
    
    /// Memory mapping strategy
    mmap_strategy: SstMmapStrategy,
    
    /// Cache for bloom filters (always hot)
    bloom_cache: Arc<DashMap<String, Arc<Vec<u8>>>>,
    
    /// Cache for index blocks (usually hot)
    index_cache: Arc<DashMap<String, Arc<Vec<u8>>>>,
    
    /// Local disk cache for downloaded data blocks
    local_cache: Arc<LocalDiskCache>,
    
    /// Memory pressure monitor
    memory_monitor: Arc<MemoryPressureMonitor>,
    
    /// Access pattern tracker
    access_tracker: Arc<AccessPatternTracker>,
    
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

/// Local disk cache for downloaded blocks
pub struct LocalDiskCache {
    cache_dir: PathBuf,
    max_cache_size: u64,
    current_size: AtomicU64,
    
    /// Track cached ranges per file
    cached_ranges: DashMap<String, Vec<Range<u64>>>,
    
    /// Track file versions for cache invalidation
    file_versions: DashMap<String, u64>,
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
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            filesystem,
            mmap_strategy,
            bloom_cache: Arc::new(DashMap::new()),
            index_cache: Arc::new(DashMap::new()),
            local_cache: Arc::new(LocalDiskCache::new(cache_dir)),
            memory_monitor: Arc::new(MemoryPressureMonitor::new()),
            access_tracker: Arc::new(AccessPatternTracker::new()),
            stats: Arc::new(ReaderStats::default()),
        }
    }
    
    /// Smart read that minimizes bandwidth usage
    pub async fn read_record(
        &self,
        file_path: &str,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, ProximaDBError> {
        // Step 1: Check bloom filter BEFORE downloading anything
        let bloom_data = self.get_bloom_filter_smart(file_path).await?;
        if !self.check_bloom(&bloom_data, key) {
            // Key definitely not in file - saved bandwidth!
            self.stats.bytes_saved.fetch_add(DATA_BLOCK_SIZE as u64, Ordering::Relaxed);
            return Ok(None);
        }
        
        // Step 2: Check index block to find data block location
        let index_data = self.get_index_block_smart(file_path).await?;
        let block_info = match self.find_block_for_key(&index_data, key)? {
            Some(info) => info,
            None => {
                // Key not in index - saved bandwidth!
                self.stats.bytes_saved.fetch_add(DATA_BLOCK_SIZE as u64, Ordering::Relaxed);
                return Ok(None);
            }
        };
        
        // Step 3: NOW download the data block since we know it's needed
        let data = self.read_data_block_smart(file_path, &block_info).await?;
        
        self.find_in_block(&data, key)
    }
    
    /// Get bloom filter with smart bandwidth optimization
    async fn get_bloom_filter_smart(&self, file_path: &str) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        // Check memory cache first
        if let Some(cached) = self.bloom_cache.get(file_path) {
            self.stats.bloom_hits.fetch_add(1, Ordering::Relaxed);
            self.access_tracker.track_hit(file_path, SstRegion::BloomFilter);
            return Ok(cached.clone());
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
            
            // Cache in memory (tiny, always fits)
            let bloom_arc = Arc::new(bloom_data);
            self.bloom_cache.insert(file_path.to_string(), bloom_arc.clone());
            
            return Ok(bloom_arc);
        }
        
        // For local files, try mmap if memory allows
        self.get_local_bloom_with_mmap(file_path).await
    }
    
    /// Get index block with smart bandwidth optimization
    async fn get_index_block_smart(&self, file_path: &str) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        // Check memory cache first
        if let Some(cached) = self.index_cache.get(file_path) {
            self.stats.index_hits.fetch_add(1, Ordering::Relaxed);
            self.access_tracker.track_hit(file_path, SstRegion::IndexBlock);
            return Ok(cached.clone());
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
            
            // Cache in memory if pressure allows
            if self.memory_monitor.get_pressure() < 0.8 {
                let index_arc = Arc::new(index_data);
                self.index_cache.insert(file_path.to_string(), index_arc.clone());
                return Ok(index_arc);
            }
            
            return Ok(Arc::new(index_data));
        }
        
        // For local files, use mmap if possible
        self.get_local_index_with_mmap(file_path).await
    }
    
    /// Read data block only after confirming it's needed
    async fn read_data_block_smart(
        &self,
        file_path: &str,
        block_info: &BlockInfo,
    ) -> Result<Vec<u8>, ProximaDBError> {
        // Check if we have this block in local cache
        if let Some(cached_data) = self.local_cache.get_block(file_path, block_info).await? {
            return Ok(cached_data);
        }
        
        // Download the specific block (not the whole file!)
        let data = if self.is_cloud_file(file_path) {
            // Cloud file - download just this block
            let block_data = self.filesystem
                .get_filesystem(file_path)?
                .read_range(file_path, block_info.offset, block_info.size)
                .await?;
            
            self.stats.bytes_downloaded.fetch_add(block_info.size, Ordering::Relaxed);
            
            // Cache locally for future reads
            self.local_cache.put_block(file_path, block_info, &block_data).await?;
            
            block_data
        } else {
            // Local file - just read the range
            self.filesystem
                .get_filesystem(file_path)?
                .read_range(file_path, block_info.offset, block_info.size)
                .await?
        };
        
        Ok(data)
    }
    
    /// Batch read optimization with smart filtering
    pub async fn batch_read_with_filtering(
        &self,
        file_path: &str,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>, ProximaDBError> {
        // Step 1: Get bloom filter once for all keys
        let bloom_data = self.get_bloom_filter_smart(file_path).await?;
        
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
        let index_data = self.get_index_block_smart(file_path).await?;
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
            let block_data = self.read_data_block_smart(file_path, &block_info).await?;
            
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
        // Remove from memory caches
        let mut invalidated = 0;
        
        self.bloom_cache.retain(|path, _| {
            if path.contains(collection_id) {
                invalidated += 1;
                false
            } else {
                true
            }
        });
        
        self.index_cache.retain(|path, _| {
            if path.contains(collection_id) {
                invalidated += 1;
                false
            } else {
                true
            }
        });
        
        // Invalidate local disk cache
        self.local_cache.invalidate_collection(collection_id).await?;
        
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
    async fn get_local_bloom_with_mmap(&self, file_path: &str) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        // Implementation for local files
        Ok(Arc::new(vec![0; BLOOM_FILTER_SIZE]))
    }
    
    /// Get local index block with mmap
    async fn get_local_index_with_mmap(&self, file_path: &str) -> Result<Arc<Vec<u8>>, ProximaDBError> {
        // Implementation for local files
        Ok(Arc::new(vec![0; INDEX_BLOCK_SIZE]))
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

impl LocalDiskCache {
    pub fn new(cache_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&cache_dir).ok();
        
        Self {
            cache_dir,
            max_cache_size: 100 * 1024 * 1024 * 1024, // 100GB default
            current_size: AtomicU64::new(0),
            cached_ranges: DashMap::new(),
            file_versions: DashMap::new(),
        }
    }
    
    /// Get cached block if available
    pub async fn get_block(
        &self,
        file_path: &str,
        block_info: &BlockInfo,
    ) -> Result<Option<Vec<u8>>, ProximaDBError> {
        // Check if we have this range cached
        if let Some(ranges) = self.cached_ranges.get(file_path) {
            for range in ranges.iter() {
                if range.start <= block_info.offset && 
                   range.end >= block_info.offset + block_info.size {
                    // We have this block cached
                    let cache_file = self.cache_path_for(file_path);
                    if cache_file.exists() {
                        // Read from local cache
                        let file = std::fs::File::open(&cache_file)?;
                        let mut buffer = vec![0; block_info.size as usize];
                        use std::io::{Read, Seek, SeekFrom};
                        let mut file = std::io::BufReader::new(file);
                        file.seek(SeekFrom::Start(block_info.offset))?;
                        file.read_exact(&mut buffer)?;
                        return Ok(Some(buffer));
                    }
                }
            }
        }
        
        Ok(None)
    }
    
    /// Cache a block locally
    pub async fn put_block(
        &self,
        file_path: &str,
        block_info: &BlockInfo,
        data: &[u8],
    ) -> Result<(), ProximaDBError> {
        let cache_file = self.cache_path_for(file_path);
        
        // Ensure parent directory exists
        if let Some(parent) = cache_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        // Write block to cache file
        use std::io::{Write, Seek, SeekFrom};
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&cache_file)?;
        
        let mut file = std::io::BufWriter::new(file);
        file.seek(SeekFrom::Start(block_info.offset))?;
        file.write_all(data)?;
        file.flush()?;
        
        // Update cached ranges
        self.cached_ranges.entry(file_path.to_string())
            .or_insert_with(Vec::new)
            .push(block_info.offset..block_info.offset + block_info.size);
        
        // Update size tracking
        self.current_size.fetch_add(data.len() as u64, Ordering::Relaxed);
        
        // Evict if over limit
        if self.current_size.load(Ordering::Relaxed) > self.max_cache_size {
            self.evict_lru().await?;
        }
        
        Ok(())
    }
    
    /// Invalidate cache for a collection
    pub async fn invalidate_collection(&self, collection_id: &str) -> Result<(), ProximaDBError> {
        let mut files_to_remove = Vec::new();
        
        // Find all cached files for this collection
        for entry in self.cached_ranges.iter() {
            if entry.key().contains(collection_id) {
                files_to_remove.push(entry.key().clone());
            }
        }
        
        // Remove from cache
        for file_path in files_to_remove {
            self.cached_ranges.remove(&file_path);
            
            // Delete from disk
            let cache_file = self.cache_path_for(&file_path);
            if cache_file.exists() {
                std::fs::remove_file(cache_file)?;
            }
        }
        
        log::info!("Invalidated disk cache for collection {}", collection_id);
        
        Ok(())
    }
    
    /// Get cache file path for a given file
    fn cache_path_for(&self, file_path: &str) -> PathBuf {
        // Convert file path to safe cache filename
        let safe_name = file_path
            .replace('/', "_")
            .replace(':', "_")
            .replace("\\", "_");
        
        self.cache_dir.join(safe_name)
    }
    
    /// Evict least recently used entries
    async fn evict_lru(&self) -> Result<(), ProximaDBError> {
        // Simple LRU eviction
        // In production, track access times and evict oldest
        log::warn!("Cache size exceeded, performing LRU eviction");
        Ok(())
    }
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

/// Memory pressure monitor placeholder
pub struct MemoryPressureMonitor;

impl MemoryPressureMonitor {
    pub fn new() -> Self {
        Self
    }
    
    pub fn get_pressure(&self) -> f32 {
        // Get system memory pressure (0.0 to 1.0)
        0.5 // Placeholder
    }
}

/// Access pattern tracker placeholder
pub struct AccessPatternTracker;

impl AccessPatternTracker {
    pub fn new() -> Self {
        Self
    }
    
    pub fn track_hit(&self, _file: &str, _region: SstRegion) {
        // Track access patterns
    }
}