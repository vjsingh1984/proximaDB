// Collection-Partitioned Tiered Cache System
// Partitions cache by collection_id for efficient access and eviction
// Tier 1: Memory (mmap) - Per-collection metadata and hot data
// Tier 2: Disk Cache - Per-collection downloaded files
// Supports atomic collection-level eviction for compaction

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use dashmap::DashMap;
use memmap2::{Mmap, MmapOptions};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::common::errors::ProximaDBError;

/// Configuration for collection-partitioned cache
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionCacheConfig {
    /// Base directory for cache
    pub base_cache_dir: PathBuf,
    
    /// Maximum memory per collection
    pub max_memory_per_collection: usize,
    
    /// Maximum disk cache per collection
    pub max_disk_per_collection: u64,
    
    /// Total memory limit across all collections
    pub total_memory_limit: usize,
    
    /// Total disk limit across all collections
    pub total_disk_limit: u64,
    
    /// Enable memory mapping for metadata
    pub enable_mmap: bool,
    
    /// Compression for disk cache
    pub disk_compression: CompressionType,
    
    /// Collection eviction policy
    pub eviction_policy: CollectionEvictionPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Lz4,
    Snappy,
    Zstd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollectionEvictionPolicy {
    /// Evict least recently used collection
    LRU,
    /// Evict collection with lowest access count
    LFU,
    /// Evict largest collection first
    LargestFirst,
    /// Evict oldest collection first
    OldestFirst,
}

/// Main collection-partitioned cache
pub struct CollectionPartitionedCache {
    /// Configuration
    config: CollectionCacheConfig,
    
    /// Per-collection cache partitions
    partitions: DashMap<String, Arc<CollectionPartition>>,
    
    /// Collection access tracking for eviction
    access_tracker: Arc<CollectionAccessTracker>,
    
    /// Global statistics
    stats: Arc<GlobalCacheStats>,
    
    /// Total memory usage across all collections
    total_memory_used: AtomicUsize,
    
    /// Total disk usage across all collections
    total_disk_used: AtomicU64,
}

/// Cache partition for a single collection
pub struct CollectionPartition {
    /// Collection ID
    collection_id: String,
    
    /// Memory cache for this collection
    memory_cache: Arc<CollectionMemoryCache>,
    
    /// Disk cache for this collection
    disk_cache: Arc<CollectionDiskCache>,
    
    /// Collection-specific statistics
    stats: Arc<CollectionCacheStats>,
    
    /// Last access time
    last_access: RwLock<Instant>,
}

/// Memory cache for a collection (with mmap support)
pub struct CollectionMemoryCache {
    /// Collection ID
    collection_id: String,
    
    /// Memory-mapped files for this collection
    mmap_files: DashMap<String, Arc<MmappedFile>>,
    
    /// In-memory metadata cache (bloom filters, indexes)
    metadata_cache: DashMap<String, Arc<Vec<u8>>>,
    
    /// Current memory usage
    memory_used: AtomicUsize,
    
    /// Maximum memory for this collection
    max_memory: usize,
}

/// Memory-mapped file entry
pub struct MmappedFile {
    /// File path
    file_path: String,
    
    /// Memory map
    mmap: Arc<Mmap>,
    
    /// File regions
    regions: Vec<FileRegion>,
    
    /// Access count
    access_count: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct FileRegion {
    /// Region type (bloom, index, data, etc.)
    pub region_type: RegionType,
    
    /// Offset in file
    pub offset: u64,
    
    /// Size of region
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RegionType {
    // SST regions
    SstBloomFilter,
    SstIndexBlock,
    SstSuperBlock,
    SstDataBlock,
    
    // Parquet regions
    ParquetFooter,
    ParquetColumnIndex,
    ParquetRowGroup,
    ParquetColumnChunk,
}

/// Disk cache for a collection
pub struct CollectionDiskCache {
    /// Collection ID
    collection_id: String,
    
    /// Cache directory for this collection
    cache_dir: PathBuf,
    
    /// Cached files index
    cached_files: DashMap<String, CachedFileInfo>,
    
    /// Current disk usage
    disk_used: AtomicU64,
    
    /// Maximum disk space for this collection
    max_disk: u64,
}

#[derive(Debug, Clone)]
pub struct CachedFileInfo {
    /// Original file path (s3://, gs://, etc.)
    pub original_path: String,
    
    /// Local cache path
    pub cache_path: PathBuf,
    
    /// File size
    pub size: u64,
    
    /// Cached ranges (for partial downloads)
    pub cached_ranges: Vec<(u64, u64)>,
    
    /// Is complete file cached
    pub is_complete: bool,
    
    /// Compression used
    pub compressed: bool,
    
    /// Last access time
    pub last_access: Instant,
}

/// Collection access tracker for eviction decisions
pub struct CollectionAccessTracker {
    /// Access history per collection
    access_history: DashMap<String, CollectionAccessInfo>,
}

#[derive(Debug, Clone)]
pub struct CollectionAccessInfo {
    /// Total access count
    pub access_count: u64,
    
    /// Last access time
    pub last_access: Instant,
    
    /// Creation time
    pub created_at: Instant,
    
    /// Current size (memory + disk)
    pub total_size: usize,
}

/// Global cache statistics
pub struct GlobalCacheStats {
    pub total_collections: AtomicU64,
    pub memory_hits: AtomicU64,
    pub memory_misses: AtomicU64,
    pub disk_hits: AtomicU64,
    pub disk_misses: AtomicU64,
    pub collections_evicted: AtomicU64,
    pub bytes_downloaded: AtomicU64,
    pub bytes_saved: AtomicU64,
}

/// Per-collection cache statistics
pub struct CollectionCacheStats {
    pub memory_hits: AtomicU64,
    pub memory_misses: AtomicU64,
    pub disk_hits: AtomicU64,
    pub disk_misses: AtomicU64,
    pub mmap_count: AtomicU64,
    pub cached_files: AtomicU64,
}

impl CollectionPartitionedCache {
    pub fn new(config: CollectionCacheConfig) -> Result<Self, ProximaDBError> {
        std::fs::create_dir_all(&config.base_cache_dir)?;
        
        Ok(Self {
            config,
            partitions: DashMap::new(),
            access_tracker: Arc::new(CollectionAccessTracker::new()),
            stats: Arc::new(GlobalCacheStats::default()),
            total_memory_used: AtomicUsize::new(0),
            total_disk_used: AtomicU64::new(0),
        })
    }
    
    /// Get or create partition for a collection
    pub async fn get_partition(&self, collection_id: &str) -> Arc<CollectionPartition> {
        if let Some(partition) = self.partitions.get(collection_id) {
            // Update access tracking
            self.access_tracker.record_access(collection_id);
            partition.update_last_access().await;
            return partition.clone();
        }
        
        // Create new partition
        let partition = Arc::new(CollectionPartition::new(
            collection_id.to_string(),
            self.config.max_memory_per_collection,
            self.config.max_disk_per_collection,
            self.config.base_cache_dir.join(collection_id),
        ));
        
        self.partitions.insert(collection_id.to_string(), partition.clone());
        self.stats.total_collections.fetch_add(1, Ordering::Relaxed);
        
        // Check if we need to evict
        self.maybe_evict_collection().await;
        
        partition
    }
    
    /// Get data from cache (memory or disk)
    pub async fn get(
        &self,
        collection_id: &str,
        file_id: &str,
        region_type: RegionType,
        offset: u64,
        size: u64,
    ) -> Result<Option<Vec<u8>>, ProximaDBError> {
        let partition = self.get_partition(collection_id).await;
        
        // Try memory first (including mmap)
        if let Some(data) = partition.memory_cache.get(file_id, &region_type, offset, size).await {
            self.stats.memory_hits.fetch_add(1, Ordering::Relaxed);
            partition.stats.memory_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(data));
        }
        
        self.stats.memory_misses.fetch_add(1, Ordering::Relaxed);
        partition.stats.memory_misses.fetch_add(1, Ordering::Relaxed);
        
        // Try disk cache
        if let Some(data) = partition.disk_cache.get(file_id, offset, size).await? {
            self.stats.disk_hits.fetch_add(1, Ordering::Relaxed);
            partition.stats.disk_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(data));
        }
        
        self.stats.disk_misses.fetch_add(1, Ordering::Relaxed);
        partition.stats.disk_misses.fetch_add(1, Ordering::Relaxed);
        
        Ok(None)
    }
    
    /// Put data into cache
    pub async fn put(
        &self,
        collection_id: &str,
        file_id: &str,
        region_type: RegionType,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<(), ProximaDBError> {
        let partition = self.get_partition(collection_id).await;
        
        // Determine target tier based on region type
        if Self::is_metadata_region(&region_type) {
            // Metadata goes to memory
            partition.memory_cache.put(file_id, region_type, offset, data).await?;
            let size = data.len();
            self.total_memory_used.fetch_add(size, Ordering::Relaxed);
        } else {
            // Data blocks go to disk
            partition.disk_cache.put(file_id, offset, data).await?;
            let size = data.len() as u64;
            self.total_disk_used.fetch_add(size, Ordering::Relaxed);
        }
        
        Ok(())
    }
    
    /// Memory map a file for a collection
    pub async fn mmap_file(
        &self,
        collection_id: &str,
        file_path: &str,
    ) -> Result<Arc<Mmap>, ProximaDBError> {
        if !self.config.enable_mmap {
            return Err(ProximaDBError::NotSupported("mmap disabled".into()));
        }
        
        let partition = self.get_partition(collection_id).await;
        partition.memory_cache.mmap_file(file_path).await
    }
    
    /// Invalidate entire collection (for compaction)
    pub async fn invalidate_collection(&self, collection_id: &str) -> Result<(), ProximaDBError> {
        log::info!("Invalidating entire cache for collection: {}", collection_id);
        
        if let Some((_, partition)) = self.partitions.remove(collection_id) {
            // Clean up memory
            let memory_freed = partition.memory_cache.clear_all().await;
            self.total_memory_used.fetch_sub(memory_freed, Ordering::Relaxed);
            
            // Clean up disk
            let disk_freed = partition.disk_cache.clear_all().await?;
            self.total_disk_used.fetch_sub(disk_freed, Ordering::Relaxed);
            
            // Remove from access tracker
            self.access_tracker.remove_collection(collection_id);
            
            // Update stats
            self.stats.collections_evicted.fetch_add(1, Ordering::Relaxed);
            
            log::info!(
                "Invalidated collection {}: freed {}MB memory, {}GB disk",
                collection_id,
                memory_freed / (1024 * 1024),
                disk_freed / (1024 * 1024 * 1024)
            );
        }
        
        Ok(())
    }
    
    /// Check if eviction is needed and evict if necessary
    async fn maybe_evict_collection(&self) {
        let memory_used = self.total_memory_used.load(Ordering::Relaxed);
        let disk_used = self.total_disk_used.load(Ordering::Relaxed);
        
        // Check if we're over limits
        if memory_used > self.config.total_memory_limit || 
           disk_used > self.config.total_disk_limit {
            // Select collection to evict based on policy
            if let Some(collection_id) = self.select_collection_to_evict() {
                let _ = self.invalidate_collection(&collection_id).await;
            }
        }
    }
    
    /// Select which collection to evict based on policy
    fn select_collection_to_evict(&self) -> Option<String> {
        match self.config.eviction_policy {
            CollectionEvictionPolicy::LRU => {
                self.access_tracker.get_lru_collection()
            },
            CollectionEvictionPolicy::LFU => {
                self.access_tracker.get_lfu_collection()
            },
            CollectionEvictionPolicy::LargestFirst => {
                self.access_tracker.get_largest_collection()
            },
            CollectionEvictionPolicy::OldestFirst => {
                self.access_tracker.get_oldest_collection()
            },
        }
    }
    
    /// Check if region type is metadata
    fn is_metadata_region(region_type: &RegionType) -> bool {
        matches!(
            region_type,
            RegionType::SstBloomFilter |
            RegionType::SstIndexBlock |
            RegionType::SstSuperBlock |
            RegionType::ParquetFooter |
            RegionType::ParquetColumnIndex
        )
    }
    
    /// Get cache statistics
    pub fn get_stats(&self) -> CacheStatsSummary {
        CacheStatsSummary {
            total_collections: self.partitions.len(),
            total_memory_used: self.total_memory_used.load(Ordering::Relaxed),
            total_disk_used: self.total_disk_used.load(Ordering::Relaxed),
            memory_hit_rate: {
                let hits = self.stats.memory_hits.load(Ordering::Relaxed);
                let misses = self.stats.memory_misses.load(Ordering::Relaxed);
                let total = hits + misses;
                if total > 0 {
                    hits as f64 / total as f64
                } else {
                    0.0
                }
            },
            disk_hit_rate: {
                let hits = self.stats.disk_hits.load(Ordering::Relaxed);
                let misses = self.stats.disk_misses.load(Ordering::Relaxed);
                let total = hits + misses;
                if total > 0 {
                    hits as f64 / total as f64
                } else {
                    0.0
                }
            },
            collections_evicted: self.stats.collections_evicted.load(Ordering::Relaxed),
            bytes_saved: self.stats.bytes_saved.load(Ordering::Relaxed),
        }
    }
}

impl CollectionPartition {
    fn new(collection_id: String, max_memory: usize, max_disk: u64, cache_dir: PathBuf) -> Self {
        Self {
            collection_id: collection_id.clone(),
            memory_cache: Arc::new(CollectionMemoryCache::new(collection_id.clone(), max_memory)),
            disk_cache: Arc::new(CollectionDiskCache::new(collection_id, cache_dir, max_disk)),
            stats: Arc::new(CollectionCacheStats::default()),
            last_access: RwLock::new(Instant::now()),
        }
    }
    
    async fn update_last_access(&self) {
        *self.last_access.write().await = Instant::now();
    }
}

impl CollectionMemoryCache {
    fn new(collection_id: String, max_memory: usize) -> Self {
        Self {
            collection_id,
            mmap_files: DashMap::new(),
            metadata_cache: DashMap::new(),
            memory_used: AtomicUsize::new(0),
            max_memory,
        }
    }
    
    async fn get(
        &self,
        file_id: &str,
        region_type: &RegionType,
        offset: u64,
        size: u64,
    ) -> Option<Vec<u8>> {
        // Check metadata cache first
        let cache_key = format!("{}_{}_{}", file_id, offset, size);
        if let Some(cached) = self.metadata_cache.get(&cache_key) {
            return Some(cached.to_vec());
        }
        
        // Check mmap files
        if let Some(mmap_file) = self.mmap_files.get(file_id) {
            mmap_file.access_count.fetch_add(1, Ordering::Relaxed);
            
            // Find the region
            for region in &mmap_file.regions {
                if region.region_type == *region_type &&
                   region.offset <= offset &&
                   region.offset + region.size >= offset + size {
                    // Read from mmap
                    let start = (offset - region.offset) as usize;
                    let end = start + size as usize;
                    return Some(mmap_file.mmap[start..end].to_vec());
                }
            }
        }
        
        None
    }
    
    async fn put(
        &self,
        file_id: &str,
        region_type: RegionType,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<(), ProximaDBError> {
        let size = data.len();
        
        // Check memory limit
        if self.memory_used.load(Ordering::Relaxed) + size > self.max_memory {
            // Evict some entries
            self.evict_lru_metadata().await;
        }
        
        let cache_key = format!("{}_{}_{}", file_id, offset, size);
        self.metadata_cache.insert(cache_key, Arc::new(data));
        self.memory_used.fetch_add(size, Ordering::Relaxed);
        
        Ok(())
    }
    
    async fn mmap_file(&self, file_path: &str) -> Result<Arc<Mmap>, ProximaDBError> {
        // Check if already mapped
        if let Some(mmap_file) = self.mmap_files.get(file_path) {
            mmap_file.access_count.fetch_add(1, Ordering::Relaxed);
            return Ok(mmap_file.mmap.clone());
        }
        
        // Create new mmap
        let file = std::fs::File::open(file_path)?;
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        let mmap_arc = Arc::new(mmap);
        
        // Parse file to identify regions
        let regions = Self::identify_file_regions(file_path)?;
        
        let mmap_file = Arc::new(MmappedFile {
            file_path: file_path.to_string(),
            mmap: mmap_arc.clone(),
            regions,
            access_count: AtomicU64::new(1),
        });
        
        self.mmap_files.insert(file_path.to_string(), mmap_file);
        self.memory_used.fetch_add(mmap_arc.len(), Ordering::Relaxed);
        
        Ok(mmap_arc)
    }
    
    async fn clear_all(&self) -> usize {
        let mut total_freed = 0;
        
        // Clear mmap files
        for entry in self.mmap_files.iter() {
            total_freed += entry.mmap.len();
        }
        self.mmap_files.clear();
        
        // Clear metadata cache
        for entry in self.metadata_cache.iter() {
            total_freed += entry.value().len();
        }
        self.metadata_cache.clear();
        
        self.memory_used.store(0, Ordering::Relaxed);
        total_freed
    }
    
    async fn evict_lru_metadata(&self) {
        // Simple eviction: remove first item found
        // In production, implement proper LRU
        if let Some(entry) = self.metadata_cache.iter().next() {
            let key = entry.key().clone();
            let size = entry.value().len();
            drop(entry);
            
            self.metadata_cache.remove(&key);
            self.memory_used.fetch_sub(size, Ordering::Relaxed);
        }
    }
    
    fn identify_file_regions(file_path: &str) -> Result<Vec<FileRegion>, ProximaDBError> {
        // Identify regions based on file extension
        if file_path.ends_with(".sst") {
            Ok(vec![
                FileRegion {
                    region_type: RegionType::SstBloomFilter,
                    offset: 0,
                    size: 4096,
                },
                FileRegion {
                    region_type: RegionType::SstIndexBlock,
                    offset: 4096,
                    size: 61440,
                },
            ])
        } else if file_path.ends_with(".parquet") {
            // For Parquet, we'd need to read the footer to identify regions
            // Simplified for now
            Ok(vec![
                FileRegion {
                    region_type: RegionType::ParquetFooter,
                    offset: 0,  // Would be calculated from file size
                    size: 8 * 1024 * 1024,
                },
            ])
        } else {
            Ok(Vec::new())
        }
    }
}

impl CollectionDiskCache {
    fn new(collection_id: String, cache_dir: PathBuf, max_disk: u64) -> Self {
        std::fs::create_dir_all(&cache_dir).ok();
        
        Self {
            collection_id,
            cache_dir,
            cached_files: DashMap::new(),
            disk_used: AtomicU64::new(0),
            max_disk,
        }
    }
    
    async fn get(&self, file_id: &str, offset: u64, size: u64) -> Result<Option<Vec<u8>>, ProximaDBError> {
        if let Some(mut info) = self.cached_files.get_mut(file_id) {
            info.last_access = Instant::now();
            
            // Check if we have this range
            for (start, end) in &info.cached_ranges {
                if *start <= offset && *end >= offset + size {
                    // Read from cache file
                    let mut file = tokio::fs::File::open(&info.cache_path).await?;
                    use tokio::io::{AsyncReadExt, AsyncSeekExt};
                    file.seek(tokio::io::SeekFrom::Start(offset - start)).await?;
                    
                    let mut buffer = vec![0u8; size as usize];
                    file.read_exact(&mut buffer).await?;
                    
                    return Ok(Some(buffer));
                }
            }
        }
        
        Ok(None)
    }
    
    async fn put(&self, file_id: &str, offset: u64, data: Vec<u8>) -> Result<(), ProximaDBError> {
        let size = data.len() as u64;
        
        // Check disk limit
        if self.disk_used.load(Ordering::Relaxed) + size > self.max_disk {
            self.evict_lru_file().await?;
        }
        
        let cache_path = self.cache_dir.join(format!("{}_{}", file_id.replace('/', "_"), offset));
        
        // Write to disk
        tokio::fs::write(&cache_path, &data).await?;
        
        // Update index
        self.cached_files.entry(file_id.to_string())
            .and_modify(|info| {
                info.cached_ranges.push((offset, offset + size));
                info.last_access = Instant::now();
            })
            .or_insert_with(|| CachedFileInfo {
                original_path: file_id.to_string(),
                cache_path: cache_path.clone(),
                size,
                cached_ranges: vec![(offset, offset + size)],
                is_complete: false,
                compressed: false,
                last_access: Instant::now(),
            });
        
        self.disk_used.fetch_add(size, Ordering::Relaxed);
        
        Ok(())
    }
    
    async fn clear_all(&self) -> Result<u64, ProximaDBError> {
        let mut total_freed = 0u64;
        
        // Remove all cached files
        for entry in self.cached_files.iter() {
            let info = entry.value();
            if info.cache_path.exists() {
                tokio::fs::remove_file(&info.cache_path).await.ok();
                total_freed += info.size;
            }
        }
        
        self.cached_files.clear();
        
        // Remove cache directory
        tokio::fs::remove_dir_all(&self.cache_dir).await.ok();
        
        self.disk_used.store(0, Ordering::Relaxed);
        
        Ok(total_freed)
    }
    
    async fn evict_lru_file(&self) -> Result<(), ProximaDBError> {
        // Find oldest file
        let mut oldest_key = None;
        let mut oldest_time = Instant::now();
        
        for entry in self.cached_files.iter() {
            if entry.value().last_access < oldest_time {
                oldest_time = entry.value().last_access;
                oldest_key = Some(entry.key().clone());
            }
        }
        
        if let Some(key) = oldest_key {
            if let Some((_, info)) = self.cached_files.remove(&key) {
                tokio::fs::remove_file(&info.cache_path).await.ok();
                self.disk_used.fetch_sub(info.size, Ordering::Relaxed);
            }
        }
        
        Ok(())
    }
}

impl CollectionAccessTracker {
    fn new() -> Self {
        Self {
            access_history: DashMap::new(),
        }
    }
    
    fn record_access(&self, collection_id: &str) {
        self.access_history.entry(collection_id.to_string())
            .and_modify(|info| {
                info.access_count += 1;
                info.last_access = Instant::now();
            })
            .or_insert_with(|| CollectionAccessInfo {
                access_count: 1,
                last_access: Instant::now(),
                created_at: Instant::now(),
                total_size: 0,
            });
    }
    
    fn remove_collection(&self, collection_id: &str) {
        self.access_history.remove(collection_id);
    }
    
    fn get_lru_collection(&self) -> Option<String> {
        let mut oldest_key = None;
        let mut oldest_time = Instant::now();
        
        for entry in self.access_history.iter() {
            if entry.value().last_access < oldest_time {
                oldest_time = entry.value().last_access;
                oldest_key = Some(entry.key().clone());
            }
        }
        
        oldest_key
    }
    
    fn get_lfu_collection(&self) -> Option<String> {
        let mut least_accessed_key = None;
        let mut min_count = u64::MAX;
        
        for entry in self.access_history.iter() {
            if entry.value().access_count < min_count {
                min_count = entry.value().access_count;
                least_accessed_key = Some(entry.key().clone());
            }
        }
        
        least_accessed_key
    }
    
    fn get_largest_collection(&self) -> Option<String> {
        let mut largest_key = None;
        let mut max_size = 0;
        
        for entry in self.access_history.iter() {
            if entry.value().total_size > max_size {
                max_size = entry.value().total_size;
                largest_key = Some(entry.key().clone());
            }
        }
        
        largest_key
    }
    
    fn get_oldest_collection(&self) -> Option<String> {
        let mut oldest_key = None;
        let mut oldest_time = Instant::now();
        
        for entry in self.access_history.iter() {
            if entry.value().created_at < oldest_time {
                oldest_time = entry.value().created_at;
                oldest_key = Some(entry.key().clone());
            }
        }
        
        oldest_key
    }
}

impl Default for GlobalCacheStats {
    fn default() -> Self {
        Self {
            total_collections: AtomicU64::new(0),
            memory_hits: AtomicU64::new(0),
            memory_misses: AtomicU64::new(0),
            disk_hits: AtomicU64::new(0),
            disk_misses: AtomicU64::new(0),
            collections_evicted: AtomicU64::new(0),
            bytes_downloaded: AtomicU64::new(0),
            bytes_saved: AtomicU64::new(0),
        }
    }
}

impl Default for CollectionCacheStats {
    fn default() -> Self {
        Self {
            memory_hits: AtomicU64::new(0),
            memory_misses: AtomicU64::new(0),
            disk_hits: AtomicU64::new(0),
            disk_misses: AtomicU64::new(0),
            mmap_count: AtomicU64::new(0),
            cached_files: AtomicU64::new(0),
        }
    }
}

impl Default for CollectionCacheConfig {
    fn default() -> Self {
        Self {
            base_cache_dir: PathBuf::from("/var/cache/proximadb"),
            max_memory_per_collection: 1024 * 1024 * 1024,  // 1GB
            max_disk_per_collection: 10 * 1024 * 1024 * 1024,  // 10GB
            total_memory_limit: 10 * 1024 * 1024 * 1024,  // 10GB total
            total_disk_limit: 100 * 1024 * 1024 * 1024,  // 100GB total
            enable_mmap: true,
            disk_compression: CompressionType::Lz4,
            eviction_policy: CollectionEvictionPolicy::LRU,
        }
    }
}

#[derive(Debug)]
pub struct CacheStatsSummary {
    pub total_collections: usize,
    pub total_memory_used: usize,
    pub total_disk_used: u64,
    pub memory_hit_rate: f64,
    pub disk_hit_rate: f64,
    pub collections_evicted: u64,
    pub bytes_saved: u64,
}