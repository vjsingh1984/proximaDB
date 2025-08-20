// Refined Integrated Cache System
// Implements file-level granular caching with compaction awareness
// Maintains consistency between cloud storage, disk cache, and mmap

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use dashmap::DashMap;
use memmap2::{Mmap, MmapOptions};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::common::errors::ProximaDBError;
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};

/// Cache configuration from server.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Enable/disable caching globally
    pub enabled: bool,
    
    /// Base cache directory
    pub cache_dir: PathBuf,
    
    /// Maximum cache size in bytes
    pub max_cache_size: u64,
    
    /// Maximum memory cache size in bytes
    pub max_memory_cache_size: usize,
    
    /// Enable memory mapping for local files
    pub enable_mmap: bool,
    
    /// Enable disk caching for cloud files
    pub enable_disk_cache: bool,
    
    /// Compression for disk cache
    pub disk_compression: CompressionType,
    
    /// Eviction policy
    pub eviction_policy: EvictionPolicy,
    
    /// Start evicting when cache is this full (0.0-1.0)
    pub eviction_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Lz4,
    Snappy,
    Zstd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvictionPolicy {
    LRU,
    LFU,
    FIFO,
}

/// Collection-specific cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionCacheConfig {
    /// Override global cache enable/disable
    pub enabled: Option<bool>,
    
    /// Maximum memory for this collection's metadata
    pub max_memory_mb: Option<usize>,
    
    /// Maximum disk cache for this collection
    pub max_disk_gb: Option<u64>,
    
    /// Prefer mmap for local files
    pub prefer_mmap: Option<bool>,
}

/// Main cache implementation
pub struct RefinedIntegratedCache {
    /// Global configuration
    config: CacheConfig,
    
    /// Filesystem factory for I/O
    filesystem: Arc<FilesystemFactory>,
    
    /// File-level metadata cache (global, in memory)
    file_metadata: Arc<FileMetadataCache>,
    
    /// Block-level metadata cache (per-block, in memory)
    block_metadata: Arc<BlockMetadataCache>,
    
    /// Memory-mapped files by collection
    mmap_cache: Arc<MmapCache>,
    
    /// Disk cache for cloud files
    disk_cache: Arc<DiskCache>,
    
    /// Cache index tracking what's cached
    cache_index: Arc<CacheIndex>,
    
    /// Global statistics
    stats: Arc<CacheStatistics>,
}

/// File-level metadata cache
pub struct FileMetadataCache {
    /// SST file metadata
    sst_metadata: DashMap<FileCacheKey, Arc<SstFileMetadata>>,
    
    /// Parquet file metadata
    parquet_metadata: DashMap<FileCacheKey, Arc<ParquetFileMetadata>>,
    
    /// Memory usage tracking
    memory_used: AtomicUsize,
}

/// Block-level metadata cache
pub struct BlockMetadataCache {
    /// SST block metadata
    sst_blocks: DashMap<BlockCacheKey, Arc<SstBlockMetadata>>,
    
    /// Parquet page metadata
    parquet_pages: DashMap<BlockCacheKey, Arc<ParquetPageMetadata>>,
    
    /// Memory usage tracking
    memory_used: AtomicUsize,
}

/// Memory-mapped file cache
pub struct MmapCache {
    /// Collections with their mmap'd files
    collections: DashMap<String, Arc<CollectionMmapCache>>,
    
    /// Total mmap'd memory
    total_mapped: AtomicUsize,
}

/// Per-collection mmap cache
pub struct CollectionMmapCache {
    /// Mapped files for this collection
    files: DashMap<String, Arc<MmappedFile>>,
    
    /// Memory used by this collection
    memory_used: AtomicUsize,
}

/// Memory-mapped file
pub struct MmappedFile {
    /// File path
    path: String,
    
    /// Memory map
    mmap: Arc<Mmap>,
    
    /// File metadata (cached)
    file_metadata: Option<Arc<dyn std::any::Any + Send + Sync>>,
    
    /// Block metadata (lazy loaded)
    block_metadata: DashMap<u32, Arc<dyn std::any::Any + Send + Sync>>,
    
    /// Access tracking
    last_access: AtomicU64,
    access_count: AtomicU32,
}

/// Disk cache for cloud files
pub struct DiskCache {
    /// Base cache directory
    cache_dir: PathBuf,
    
    /// Cached files by collection
    collections: DashMap<String, Arc<CollectionDiskCache>>,
    
    /// Total disk usage
    total_disk_used: AtomicU64,
}

/// Per-collection disk cache
pub struct CollectionDiskCache {
    /// Collection ID
    collection_id: String,
    
    /// Cache directory for this collection
    cache_dir: PathBuf,
    
    /// Cached files
    files: DashMap<String, CachedFileInfo>,
    
    /// Disk usage for this collection
    disk_used: AtomicU64,
}

/// Information about a cached file
#[derive(Debug, Clone)]
pub struct CachedFileInfo {
    /// Original path (s3://, gs://, etc.)
    pub original_path: String,
    
    /// Local cache path
    pub cache_path: PathBuf,
    
    /// File size
    pub size: u64,
    
    /// Is staging file (being written)
    pub is_staging: bool,
    
    /// Creation time
    pub created_at: SystemTime,
    
    /// Last access time
    pub last_access: Instant,
    
    /// Access count
    pub access_count: u32,
}

/// Cache index tracking what's cached
pub struct CacheIndex {
    /// Files in cache by collection
    cached_files: DashMap<String, HashSet<String>>,
    
    /// Recently flushed files (protect from eviction)
    recent_flushes: DashMap<String, Instant>,
    
    /// Recently compacted files (protect from eviction)
    recent_compactions: DashMap<String, Instant>,
}

/// Cache keys
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct FileCacheKey {
    pub collection_id: String,
    pub filename: String,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct BlockCacheKey {
    pub collection_id: String,
    pub filename: String,
    pub block_id: u32,
}

/// SST file metadata
#[derive(Debug, Clone)]
pub struct SstFileMetadata {
    /// Global bloom filter for entire file
    pub file_bloom_filter: Arc<Vec<u8>>,
    
    /// File-level index
    pub file_index: Arc<Vec<u8>>,
    
    /// Superblock metadata (for SWIFT)
    pub superblock_index: Option<Arc<Vec<u8>>>,
    
    /// File size
    pub file_size: u64,
    
    /// Number of blocks
    pub num_blocks: u32,
}

/// SST block metadata
#[derive(Debug, Clone)]
pub struct SstBlockMetadata {
    /// Per-block bloom filter
    pub block_bloom: Arc<Vec<u8>>,
    
    /// Block index
    pub block_index: Arc<Vec<u8>>,
    
    /// Block offset in file
    pub offset: u64,
    
    /// Block size
    pub size: u32,
}

/// Parquet file metadata
#[derive(Debug, Clone)]
pub struct ParquetFileMetadata {
    /// Parquet footer
    pub footer: Arc<Vec<u8>>,
    
    /// File statistics
    pub file_stats: Arc<Vec<u8>>,
    
    /// Number of row groups
    pub num_row_groups: u32,
}

/// Parquet page metadata
#[derive(Debug, Clone)]
pub struct ParquetPageMetadata {
    /// Page statistics
    pub page_stats: Arc<Vec<u8>>,
    
    /// Column bloom filters
    pub column_blooms: Arc<Vec<Vec<u8>>>,
    
    /// Page offset
    pub offset: u64,
    
    /// Page size
    pub size: u32,
}

/// Cache statistics
pub struct CacheStatistics {
    // Hit rates
    pub file_metadata_hits: AtomicU64,
    pub file_metadata_misses: AtomicU64,
    pub block_metadata_hits: AtomicU64,
    pub block_metadata_misses: AtomicU64,
    pub disk_cache_hits: AtomicU64,
    pub disk_cache_misses: AtomicU64,
    
    // Operations
    pub files_cached: AtomicU64,
    pub files_evicted: AtomicU64,
    pub staging_operations: AtomicU64,
    
    // Space
    pub memory_used: AtomicUsize,
    pub disk_used: AtomicU64,
    pub mmap_count: AtomicU32,
}

impl RefinedIntegratedCache {
    /// Create new cache instance
    pub fn new(
        config: CacheConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self, ProximaDBError> {
        // Create cache directory if needed
        if config.enable_disk_cache {
            std::fs::create_dir_all(&config.cache_dir)?;
        }
        
        Ok(Self {
            config,
            filesystem,
            file_metadata: Arc::new(FileMetadataCache::new()),
            block_metadata: Arc::new(BlockMetadataCache::new()),
            mmap_cache: Arc::new(MmapCache::new()),
            disk_cache: Arc::new(DiskCache::new(config.cache_dir.clone())),
            cache_index: Arc::new(CacheIndex::new()),
            stats: Arc::new(CacheStatistics::default()),
        })
    }
    
    /// Create staging file for atomic write
    pub async fn create_staging_file(
        &self,
        collection_id: &str,
        filename: &str,
    ) -> Result<PathBuf, ProximaDBError> {
        let staging_path = self.disk_cache.get_staging_path(collection_id, filename);
        
        // Ensure directory exists
        if let Some(parent) = staging_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        self.stats.staging_operations.fetch_add(1, Ordering::Relaxed);
        
        Ok(staging_path)
    }
    
    /// Move staging file to final location (atomic)
    pub async fn commit_staging_file(
        &self,
        collection_id: &str,
        filename: &str,
    ) -> Result<PathBuf, ProximaDBError> {
        let staging_path = self.disk_cache.get_staging_path(collection_id, filename);
        let final_path = self.disk_cache.get_cache_path(collection_id, filename);
        
        // Atomic rename
        tokio::fs::rename(&staging_path, &final_path).await?;
        
        // Update cache index
        self.cache_index.add_file(collection_id, filename);
        self.cache_index.mark_recent_flush(filename);
        
        // Update disk cache tracking
        self.disk_cache.register_cached_file(collection_id, filename, &final_path).await?;
        
        self.stats.files_cached.fetch_add(1, Ordering::Relaxed);
        
        Ok(final_path)
    }
    
    /// Remove specific files from cache (for compaction)
    pub async fn remove_files(
        &self,
        collection_id: &str,
        filenames: Vec<String>,
    ) -> Result<(), ProximaDBError> {
        for filename in filenames {
            // Remove from disk cache
            self.disk_cache.remove_file(collection_id, &filename).await?;
            
            // Remove from mmap cache
            self.mmap_cache.unmap_file(collection_id, &filename);
            
            // Remove from metadata caches
            self.file_metadata.remove(collection_id, &filename);
            self.block_metadata.remove_file(collection_id, &filename);
            
            // Update cache index
            self.cache_index.remove_file(collection_id, &filename);
            
            self.stats.files_evicted.fetch_add(1, Ordering::Relaxed);
        }
        
        Ok(())
    }
    
    /// Get file metadata from cache
    pub fn get_file_metadata(
        &self,
        collection_id: &str,
        filename: &str,
        file_type: FileType,
    ) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        let key = FileCacheKey {
            collection_id: collection_id.to_string(),
            filename: filename.to_string(),
        };
        
        match file_type {
            FileType::SST => {
                if let Some(metadata) = self.file_metadata.sst_metadata.get(&key) {
                    self.stats.file_metadata_hits.fetch_add(1, Ordering::Relaxed);
                    return Some(metadata.clone() as Arc<dyn std::any::Any + Send + Sync>);
                }
            },
            FileType::Parquet => {
                if let Some(metadata) = self.file_metadata.parquet_metadata.get(&key) {
                    self.stats.file_metadata_hits.fetch_add(1, Ordering::Relaxed);
                    return Some(metadata.clone() as Arc<dyn std::any::Any + Send + Sync>);
                }
            },
        }
        
        self.stats.file_metadata_misses.fetch_add(1, Ordering::Relaxed);
        None
    }
    
    /// Put file metadata into cache
    pub fn put_file_metadata(
        &self,
        collection_id: &str,
        filename: &str,
        metadata: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), ProximaDBError> {
        let key = FileCacheKey {
            collection_id: collection_id.to_string(),
            filename: filename.to_string(),
        };
        
        // Downcast and store based on type
        if let Some(sst_metadata) = metadata.downcast_ref::<SstFileMetadata>() {
            let size = std::mem::size_of_val(sst_metadata);
            self.file_metadata.sst_metadata.insert(key, Arc::new(sst_metadata.clone()));
            self.file_metadata.memory_used.fetch_add(size, Ordering::Relaxed);
        } else if let Some(parquet_metadata) = metadata.downcast_ref::<ParquetFileMetadata>() {
            let size = std::mem::size_of_val(parquet_metadata);
            self.file_metadata.parquet_metadata.insert(key, Arc::new(parquet_metadata.clone()));
            self.file_metadata.memory_used.fetch_add(size, Ordering::Relaxed);
        }
        
        Ok(())
    }
    
    /// Get block metadata from cache
    pub fn get_block_metadata(
        &self,
        collection_id: &str,
        filename: &str,
        block_id: u32,
        file_type: FileType,
    ) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        let key = BlockCacheKey {
            collection_id: collection_id.to_string(),
            filename: filename.to_string(),
            block_id,
        };
        
        match file_type {
            FileType::SST => {
                if let Some(metadata) = self.block_metadata.sst_blocks.get(&key) {
                    self.stats.block_metadata_hits.fetch_add(1, Ordering::Relaxed);
                    return Some(metadata.clone() as Arc<dyn std::any::Any + Send + Sync>);
                }
            },
            FileType::Parquet => {
                if let Some(metadata) = self.block_metadata.parquet_pages.get(&key) {
                    self.stats.block_metadata_hits.fetch_add(1, Ordering::Relaxed);
                    return Some(metadata.clone() as Arc<dyn std::any::Any + Send + Sync>);
                }
            },
        }
        
        self.stats.block_metadata_misses.fetch_add(1, Ordering::Relaxed);
        None
    }
    
    /// Put block metadata into cache
    pub fn put_block_metadata(
        &self,
        collection_id: &str,
        filename: &str,
        block_id: u32,
        metadata: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), ProximaDBError> {
        let key = BlockCacheKey {
            collection_id: collection_id.to_string(),
            filename: filename.to_string(),
            block_id,
        };
        
        // Downcast and store based on type
        if let Some(sst_block) = metadata.downcast_ref::<SstBlockMetadata>() {
            let size = std::mem::size_of_val(sst_block);
            self.block_metadata.sst_blocks.insert(key, Arc::new(sst_block.clone()));
            self.block_metadata.memory_used.fetch_add(size, Ordering::Relaxed);
        } else if let Some(parquet_page) = metadata.downcast_ref::<ParquetPageMetadata>() {
            let size = std::mem::size_of_val(parquet_page);
            self.block_metadata.parquet_pages.insert(key, Arc::new(parquet_page.clone()));
            self.block_metadata.memory_used.fetch_add(size, Ordering::Relaxed);
        }
        
        Ok(())
    }
    
    /// Get or create mmap for local file
    pub async fn get_or_create_mmap(
        &self,
        collection_id: &str,
        filename: &str,
        file_path: &str,
    ) -> Result<Arc<Mmap>, ProximaDBError> {
        // Check if already mapped
        if let Some(mmap) = self.mmap_cache.get_mmap(collection_id, filename) {
            return Ok(mmap);
        }
        
        // Don't mmap cloud files
        if is_cloud_url(file_path) {
            return Err(ProximaDBError::InvalidArgument("Cannot mmap cloud files".into()));
        }
        
        // Create new mmap
        let file = std::fs::File::open(file_path)?;
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        let mmap_arc = Arc::new(mmap);
        
        // Cache it
        self.mmap_cache.add_mmap(collection_id, filename, mmap_arc.clone())?;
        self.stats.mmap_count.fetch_add(1, Ordering::Relaxed);
        
        Ok(mmap_arc)
    }
    
    /// Get cached file path or download if needed
    pub async fn get_cached_file_path(
        &self,
        collection_id: &str,
        filename: &str,
        original_path: &str,
    ) -> Result<PathBuf, ProximaDBError> {
        // For local files, return original path
        if !is_cloud_url(original_path) {
            return Ok(PathBuf::from(original_path));
        }
        
        // Check if already cached
        if let Some(cache_path) = self.disk_cache.get_cached_path(collection_id, filename) {
            self.stats.disk_cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(cache_path);
        }
        
        self.stats.disk_cache_misses.fetch_add(1, Ordering::Relaxed);
        
        // Download to cache
        let cache_path = self.disk_cache.get_cache_path(collection_id, filename);
        
        // Ensure directory exists
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        // Download file
        let fs = self.filesystem.get_filesystem(original_path)?;
        let data = fs.read(original_path).await?;
        tokio::fs::write(&cache_path, data).await?;
        
        // Register in cache
        self.disk_cache.register_cached_file(collection_id, filename, &cache_path).await?;
        self.cache_index.add_file(collection_id, filename);
        
        Ok(cache_path)
    }
    
    /// Evict files if needed to make space
    pub async fn evict_if_needed(&self, required_space: u64) -> Result<(), ProximaDBError> {
        let current_size = self.disk_cache.get_total_size();
        let max_size = self.config.max_cache_size;
        let threshold_size = (max_size as f64 * self.config.eviction_threshold as f64) as u64;
        
        if current_size + required_space > threshold_size {
            let to_free = (current_size + required_space) - threshold_size;
            self.evict_files(to_free).await?;
        }
        
        Ok(())
    }
    
    /// Evict files to free space
    async fn evict_files(&self, space_to_free: u64) -> Result<(), ProximaDBError> {
        let candidates = self.get_eviction_candidates();
        let mut freed = 0u64;
        
        for (collection_id, filename, size) in candidates {
            // Skip recently flushed/compacted files
            if self.cache_index.is_recent(&filename) {
                continue;
            }
            
            // Remove file
            self.remove_files(&collection_id, vec![filename]).await?;
            freed += size;
            
            if freed >= space_to_free {
                break;
            }
        }
        
        Ok(())
    }
    
    /// Get eviction candidates based on policy
    fn get_eviction_candidates(&self) -> Vec<(String, String, u64)> {
        match self.config.eviction_policy {
            EvictionPolicy::LRU => self.disk_cache.get_lru_candidates(),
            EvictionPolicy::LFU => self.disk_cache.get_lfu_candidates(),
            EvictionPolicy::FIFO => self.disk_cache.get_fifo_candidates(),
        }
    }
}

// Helper implementations

impl FileMetadataCache {
    fn new() -> Self {
        Self {
            sst_metadata: DashMap::new(),
            parquet_metadata: DashMap::new(),
            memory_used: AtomicUsize::new(0),
        }
    }
    
    fn remove(&self, collection_id: &str, filename: &str) {
        let key = FileCacheKey {
            collection_id: collection_id.to_string(),
            filename: filename.to_string(),
        };
        
        if let Some((_, metadata)) = self.sst_metadata.remove(&key) {
            let size = std::mem::size_of_val(&*metadata);
            self.memory_used.fetch_sub(size, Ordering::Relaxed);
        }
        
        if let Some((_, metadata)) = self.parquet_metadata.remove(&key) {
            let size = std::mem::size_of_val(&*metadata);
            self.memory_used.fetch_sub(size, Ordering::Relaxed);
        }
    }
}

impl BlockMetadataCache {
    fn new() -> Self {
        Self {
            sst_blocks: DashMap::new(),
            parquet_pages: DashMap::new(),
            memory_used: AtomicUsize::new(0),
        }
    }
    
    fn remove_file(&self, collection_id: &str, filename: &str) {
        // Remove all blocks for this file
        let to_remove: Vec<_> = self.sst_blocks
            .iter()
            .filter(|entry| {
                entry.key().collection_id == collection_id &&
                entry.key().filename == filename
            })
            .map(|entry| entry.key().clone())
            .collect();
        
        for key in to_remove {
            self.sst_blocks.remove(&key);
        }
        
        // Same for Parquet pages
        let to_remove: Vec<_> = self.parquet_pages
            .iter()
            .filter(|entry| {
                entry.key().collection_id == collection_id &&
                entry.key().filename == filename
            })
            .map(|entry| entry.key().clone())
            .collect();
        
        for key in to_remove {
            self.parquet_pages.remove(&key);
        }
    }
}

impl MmapCache {
    fn new() -> Self {
        Self {
            collections: DashMap::new(),
            total_mapped: AtomicUsize::new(0),
        }
    }
    
    fn get_mmap(&self, collection_id: &str, filename: &str) -> Option<Arc<Mmap>> {
        self.collections.get(collection_id)
            .and_then(|collection| {
                collection.files.get(filename)
                    .map(|file| {
                        file.last_access.store(
                            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap().as_secs(),
                            Ordering::Relaxed
                        );
                        file.access_count.fetch_add(1, Ordering::Relaxed);
                        file.mmap.clone()
                    })
            })
    }
    
    fn add_mmap(
        &self,
        collection_id: &str,
        filename: &str,
        mmap: Arc<Mmap>,
    ) -> Result<(), ProximaDBError> {
        let collection = self.collections.entry(collection_id.to_string())
            .or_insert_with(|| Arc::new(CollectionMmapCache {
                files: DashMap::new(),
                memory_used: AtomicUsize::new(0),
            }));
        
        let mmap_file = Arc::new(MmappedFile {
            path: filename.to_string(),
            mmap: mmap.clone(),
            file_metadata: None,
            block_metadata: DashMap::new(),
            last_access: AtomicU64::new(
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap().as_secs()
            ),
            access_count: AtomicU32::new(1),
        });
        
        let size = mmap.len();
        collection.files.insert(filename.to_string(), mmap_file);
        collection.memory_used.fetch_add(size, Ordering::Relaxed);
        self.total_mapped.fetch_add(size, Ordering::Relaxed);
        
        Ok(())
    }
    
    fn unmap_file(&self, collection_id: &str, filename: &str) {
        if let Some(collection) = self.collections.get(collection_id) {
            if let Some((_, file)) = collection.files.remove(filename) {
                let size = file.mmap.len();
                collection.memory_used.fetch_sub(size, Ordering::Relaxed);
                self.total_mapped.fetch_sub(size, Ordering::Relaxed);
            }
        }
    }
}

impl DiskCache {
    fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            collections: DashMap::new(),
            total_disk_used: AtomicU64::new(0),
        }
    }
    
    fn get_staging_path(&self, collection_id: &str, filename: &str) -> PathBuf {
        self.cache_dir
            .join(collection_id)
            .join(format!("{}.staging", filename))
    }
    
    fn get_cache_path(&self, collection_id: &str, filename: &str) -> PathBuf {
        self.cache_dir
            .join(collection_id)
            .join(filename)
    }
    
    fn get_cached_path(&self, collection_id: &str, filename: &str) -> Option<PathBuf> {
        self.collections.get(collection_id)
            .and_then(|collection| {
                collection.files.get(filename)
                    .map(|info| info.cache_path.clone())
            })
    }
    
    async fn register_cached_file(
        &self,
        collection_id: &str,
        filename: &str,
        path: &Path,
    ) -> Result<(), ProximaDBError> {
        let collection = self.collections.entry(collection_id.to_string())
            .or_insert_with(|| Arc::new(CollectionDiskCache {
                collection_id: collection_id.to_string(),
                cache_dir: self.cache_dir.join(collection_id),
                files: DashMap::new(),
                disk_used: AtomicU64::new(0),
            }));
        
        let metadata = tokio::fs::metadata(path).await?;
        let size = metadata.len();
        
        let info = CachedFileInfo {
            original_path: format!("s3://{}/{}", collection_id, filename),
            cache_path: path.to_path_buf(),
            size,
            is_staging: false,
            created_at: SystemTime::now(),
            last_access: Instant::now(),
            access_count: 1,
        };
        
        collection.files.insert(filename.to_string(), info);
        collection.disk_used.fetch_add(size, Ordering::Relaxed);
        self.total_disk_used.fetch_add(size, Ordering::Relaxed);
        
        Ok(())
    }
    
    async fn remove_file(
        &self,
        collection_id: &str,
        filename: &str,
    ) -> Result<(), ProximaDBError> {
        if let Some(collection) = self.collections.get(collection_id) {
            if let Some((_, info)) = collection.files.remove(filename) {
                // Delete file from disk
                tokio::fs::remove_file(&info.cache_path).await.ok();
                
                // Update size tracking
                collection.disk_used.fetch_sub(info.size, Ordering::Relaxed);
                self.total_disk_used.fetch_sub(info.size, Ordering::Relaxed);
            }
        }
        
        Ok(())
    }
    
    fn get_total_size(&self) -> u64 {
        self.total_disk_used.load(Ordering::Relaxed)
    }
    
    fn get_lru_candidates(&self) -> Vec<(String, String, u64)> {
        let mut candidates = Vec::new();
        
        for collection_entry in self.collections.iter() {
            let collection_id = collection_entry.key().clone();
            
            for file_entry in collection_entry.value().files.iter() {
                candidates.push((
                    collection_id.clone(),
                    file_entry.key().clone(),
                    file_entry.value().size,
                    file_entry.value().last_access,
                ));
            }
        }
        
        // Sort by last access (oldest first)
        candidates.sort_by_key(|(_,_,_,last_access)| *last_access);
        
        candidates.into_iter()
            .map(|(cid, fname, size, _)| (cid, fname, size))
            .collect()
    }
    
    fn get_lfu_candidates(&self) -> Vec<(String, String, u64)> {
        let mut candidates = Vec::new();
        
        for collection_entry in self.collections.iter() {
            let collection_id = collection_entry.key().clone();
            
            for file_entry in collection_entry.value().files.iter() {
                candidates.push((
                    collection_id.clone(),
                    file_entry.key().clone(),
                    file_entry.value().size,
                    file_entry.value().access_count,
                ));
            }
        }
        
        // Sort by access count (least accessed first)
        candidates.sort_by_key(|(_,_,_,count)| *count);
        
        candidates.into_iter()
            .map(|(cid, fname, size, _)| (cid, fname, size))
            .collect()
    }
    
    fn get_fifo_candidates(&self) -> Vec<(String, String, u64)> {
        let mut candidates = Vec::new();
        
        for collection_entry in self.collections.iter() {
            let collection_id = collection_entry.key().clone();
            
            for file_entry in collection_entry.value().files.iter() {
                candidates.push((
                    collection_id.clone(),
                    file_entry.key().clone(),
                    file_entry.value().size,
                    file_entry.value().created_at,
                ));
            }
        }
        
        // Sort by creation time (oldest first)
        candidates.sort_by_key(|(_,_,_,created)| *created);
        
        candidates.into_iter()
            .map(|(cid, fname, size, _)| (cid, fname, size))
            .collect()
    }
}

impl CacheIndex {
    fn new() -> Self {
        Self {
            cached_files: DashMap::new(),
            recent_flushes: DashMap::new(),
            recent_compactions: DashMap::new(),
        }
    }
    
    fn add_file(&self, collection_id: &str, filename: &str) {
        self.cached_files.entry(collection_id.to_string())
            .or_insert_with(HashSet::new)
            .insert(filename.to_string());
    }
    
    fn remove_file(&self, collection_id: &str, filename: &str) {
        if let Some(mut files) = self.cached_files.get_mut(collection_id) {
            files.remove(filename);
        }
    }
    
    fn mark_recent_flush(&self, filename: &str) {
        self.recent_flushes.insert(filename.to_string(), Instant::now());
    }
    
    fn mark_recent_compaction(&self, filename: &str) {
        self.recent_compactions.insert(filename.to_string(), Instant::now());
    }
    
    fn is_recent(&self, filename: &str) -> bool {
        let threshold = Duration::from_secs(300); // 5 minutes
        
        if let Some(flush_time) = self.recent_flushes.get(filename) {
            if flush_time.elapsed() < threshold {
                return true;
            }
        }
        
        if let Some(compact_time) = self.recent_compactions.get(filename) {
            if compact_time.elapsed() < threshold {
                return true;
            }
        }
        
        false
    }
}

// Helper functions

fn is_cloud_url(path: &str) -> bool {
    path.starts_with("s3://") ||
    path.starts_with("gs://") ||
    path.starts_with("azure://") ||
    path.starts_with("http://") ||
    path.starts_with("https://")
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_dir: PathBuf::from("/tmp/proximadb_cache"),
            max_cache_size: 10 * 1024 * 1024 * 1024, // 10GB
            max_memory_cache_size: 512 * 1024 * 1024, // 512MB
            enable_mmap: true,
            enable_disk_cache: true,
            disk_compression: CompressionType::Lz4,
            eviction_policy: EvictionPolicy::LRU,
            eviction_threshold: 0.9,
        }
    }
}

impl Default for CacheStatistics {
    fn default() -> Self {
        Self {
            file_metadata_hits: AtomicU64::new(0),
            file_metadata_misses: AtomicU64::new(0),
            block_metadata_hits: AtomicU64::new(0),
            block_metadata_misses: AtomicU64::new(0),
            disk_cache_hits: AtomicU64::new(0),
            disk_cache_misses: AtomicU64::new(0),
            files_cached: AtomicU64::new(0),
            files_evicted: AtomicU64::new(0),
            staging_operations: AtomicU64::new(0),
            memory_used: AtomicUsize::new(0),
            disk_used: AtomicU64::new(0),
            mmap_count: AtomicU32::new(0),
        }
    }
}

#[derive(Debug, Clone)]
pub enum FileType {
    SST,
    Parquet,
}