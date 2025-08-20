// Integrated Collection-Partitioned Cache System
// Combines tiered caching with collection partitioning
// Focuses on file-level and metadata caching, NOT VectorRecord caching
//
// Key Design Decisions:
// 1. NO VectorRecord caching - engines read directly from cached files
// 2. Metadata always in memory (bloom filters, indexes, footers)
// 3. Data blocks cached on disk or accessed directly from EBS
// 4. Collection-level partitioning for efficient similarity search
// 5. Simple collection-wide eviction for compaction

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use dashmap::DashMap;
use memmap2::{Mmap, MmapOptions};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::common::errors::ProximaDBError;
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};

/// Integrated cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegratedCacheConfig {
    /// Base cache directory
    pub base_cache_dir: PathBuf,
    
    /// Memory allocation strategy
    pub memory_config: MemoryConfig,
    
    /// Disk cache configuration
    pub disk_config: DiskConfig,
    
    /// Storage location hints
    pub storage_hints: StorageHints,
    
    /// Collection eviction policy
    pub eviction_policy: CollectionEvictionPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Maximum memory for metadata per collection
    pub max_metadata_per_collection: usize,
    
    /// Total memory limit for all metadata
    pub total_metadata_limit: usize,
    
    /// Enable mmap for local files
    pub enable_mmap: bool,
    
    /// Memory pressure threshold
    pub pressure_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskConfig {
    /// Maximum disk cache per collection
    pub max_disk_per_collection: u64,
    
    /// Total disk cache limit
    pub total_disk_limit: u64,
    
    /// Enable disk caching (false for EBS volumes)
    pub enable_disk_cache: bool,
    
    /// Compression for cached files
    pub compression: CompressionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageHints {
    /// Files are on fast local storage (NVMe, EBS)
    pub fast_local_storage: bool,
    
    /// Files are on slow network storage (S3, GCS)
    pub remote_storage: bool,
    
    /// EBS volume paths (no disk caching needed)
    pub ebs_paths: Vec<PathBuf>,
    
    /// Local SSD paths (can use mmap)
    pub local_ssd_paths: Vec<PathBuf>,
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
    LRU,
    LFU,
    LargestFirst,
    OldestFirst,
}

/// Main integrated cache
pub struct IntegratedCache {
    config: IntegratedCacheConfig,
    
    /// Per-collection cache partitions
    collections: DashMap<String, Arc<CollectionCache>>,
    
    /// Filesystem for I/O
    filesystem: Arc<FilesystemFactory>,
    
    /// Global memory usage
    total_memory_used: AtomicUsize,
    
    /// Global disk usage
    total_disk_used: AtomicU64,
    
    /// Statistics
    stats: Arc<CacheStats>,
}

/// Cache for a single collection
pub struct CollectionCache {
    collection_id: String,
    
    /// Metadata cache (always in memory)
    metadata: Arc<MetadataCache>,
    
    /// File cache (mmap or disk cache based on storage type)
    files: Arc<FileCache>,
    
    /// Collection statistics
    stats: Arc<CollectionStats>,
    
    /// Last access time
    last_access: RwLock<Instant>,
}

/// Metadata cache (always in memory)
pub struct MetadataCache {
    /// SST metadata
    bloom_filters: DashMap<String, Arc<Vec<u8>>>,
    index_blocks: DashMap<String, Arc<Vec<u8>>>,
    superblocks: DashMap<String, Arc<Vec<u8>>>,
    
    /// Parquet metadata
    footers: DashMap<String, Arc<Vec<u8>>>,
    column_indexes: DashMap<String, Arc<Vec<u8>>>,
    page_indexes: DashMap<String, Arc<Vec<u8>>>,
    
    /// Memory usage
    memory_used: AtomicUsize,
}

/// File cache (adaptive based on storage type)
pub struct FileCache {
    /// Memory-mapped files (for local SSD)
    mmap_files: DashMap<String, Arc<MmappedFile>>,
    
    /// Disk cached files (for remote storage)
    disk_cache: Option<Arc<DiskCache>>,
    
    /// Direct file handles (for EBS volumes)
    file_handles: DashMap<String, Arc<FileHandle>>,
}

/// Memory-mapped file
pub struct MmappedFile {
    path: String,
    mmap: Arc<Mmap>,
    regions: Vec<FileRegion>,
    last_access: RwLock<Instant>,
}

/// Direct file handle (for EBS volumes)
pub struct FileHandle {
    path: String,
    file: Arc<tokio::fs::File>,
    last_access: RwLock<Instant>,
}

/// Disk cache for remote files
pub struct DiskCache {
    cache_dir: PathBuf,
    cached_files: DashMap<String, CachedFile>,
    disk_used: AtomicU64,
}

#[derive(Clone)]
pub struct CachedFile {
    original_path: String,
    cache_path: PathBuf,
    size: u64,
    ranges: Vec<(u64, u64)>,
    last_access: Instant,
}

#[derive(Debug, Clone)]
pub struct FileRegion {
    pub region_type: RegionType,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RegionType {
    // SST regions
    SstBloom,
    SstIndex,
    SstSuperblock,
    SstDataBlock,
    
    // Parquet regions
    ParquetFooter,
    ParquetColumnIndex,
    ParquetRowGroup,
}

/// Cache statistics
pub struct CacheStats {
    metadata_hits: AtomicU64,
    metadata_misses: AtomicU64,
    file_hits: AtomicU64,
    file_misses: AtomicU64,
    mmap_count: AtomicU64,
    disk_cache_size: AtomicU64,
    collections_evicted: AtomicU64,
}

pub struct CollectionStats {
    access_count: AtomicU64,
    metadata_size: AtomicUsize,
    cache_size: AtomicU64,
}

impl IntegratedCache {
    pub fn new(
        config: IntegratedCacheConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self, ProximaDBError> {
        // Create base cache directory if disk caching is enabled
        if config.disk_config.enable_disk_cache {
            std::fs::create_dir_all(&config.base_cache_dir)?;
        }
        
        Ok(Self {
            config,
            collections: DashMap::new(),
            filesystem,
            total_memory_used: AtomicUsize::new(0),
            total_disk_used: AtomicU64::new(0),
            stats: Arc::new(CacheStats::default()),
        })
    }
    
    /// Get metadata from cache (bloom filter, index, footer, etc.)
    pub async fn get_metadata(
        &self,
        collection_id: &str,
        file_id: &str,
        region_type: RegionType,
    ) -> Result<Option<Arc<Vec<u8>>>, ProximaDBError> {
        let collection = self.get_or_create_collection(collection_id).await?;
        
        // Metadata is always cached in memory
        if let Some(data) = collection.metadata.get(file_id, &region_type) {
            self.stats.metadata_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(data));
        }
        
        self.stats.metadata_misses.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    
    /// Put metadata into cache (always goes to memory)
    pub async fn put_metadata(
        &self,
        collection_id: &str,
        file_id: &str,
        region_type: RegionType,
        data: Vec<u8>,
    ) -> Result<(), ProximaDBError> {
        let collection = self.get_or_create_collection(collection_id).await?;
        
        let size = data.len();
        collection.metadata.put(file_id, region_type, Arc::new(data))?;
        
        self.total_memory_used.fetch_add(size, Ordering::Relaxed);
        collection.stats.metadata_size.fetch_add(size, Ordering::Relaxed);
        
        // Check memory pressure
        self.maybe_evict_collection().await?;
        
        Ok(())
    }
    
    /// Get data block from cache or storage
    pub async fn get_data_block(
        &self,
        collection_id: &str,
        file_path: &str,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>, ProximaDBError> {
        let collection = self.get_or_create_collection(collection_id).await?;
        
        // Determine storage type
        let storage_type = self.determine_storage_type(file_path);
        
        match storage_type {
            StorageType::LocalSSD => {
                // Use mmap for local SSD
                self.read_via_mmap(&collection, file_path, offset, size).await
            },
            StorageType::EBS => {
                // Direct read from EBS (no caching needed)
                self.read_direct(file_path, offset, size).await
            },
            StorageType::Remote => {
                // Check disk cache, download if needed
                self.read_via_cache(&collection, file_path, offset, size).await
            },
        }
    }
    
    /// Invalidate entire collection cache (for compaction)
    pub async fn invalidate_collection(&self, collection_id: &str) -> Result<(), ProximaDBError> {
        log::info!("Invalidating cache for collection: {}", collection_id);
        
        if let Some((_, collection)) = self.collections.remove(collection_id) {
            // Clear metadata
            let metadata_freed = collection.metadata.clear();
            self.total_memory_used.fetch_sub(metadata_freed, Ordering::Relaxed);
            
            // Clear file cache
            let disk_freed = collection.files.clear().await?;
            self.total_disk_used.fetch_sub(disk_freed, Ordering::Relaxed);
            
            self.stats.collections_evicted.fetch_add(1, Ordering::Relaxed);
            
            log::info!(
                "Evicted collection {}: freed {}MB memory, {}GB disk",
                collection_id,
                metadata_freed / (1024 * 1024),
                disk_freed / (1024 * 1024 * 1024)
            );
        }
        
        Ok(())
    }
    
    /// Get or create collection cache
    async fn get_or_create_collection(&self, collection_id: &str) -> Result<Arc<CollectionCache>, ProximaDBError> {
        if let Some(collection) = self.collections.get(collection_id) {
            collection.update_access().await;
            return Ok(collection.clone());
        }
        
        // Create new collection cache
        let collection = Arc::new(CollectionCache::new(
            collection_id.to_string(),
            &self.config,
        )?);
        
        self.collections.insert(collection_id.to_string(), collection.clone());
        
        Ok(collection)
    }
    
    /// Determine storage type from path
    fn determine_storage_type(&self, path: &str) -> StorageType {
        // Check if remote storage
        if path.starts_with("s3://") || path.starts_with("gs://") || path.starts_with("azure://") {
            return StorageType::Remote;
        }
        
        let path_buf = PathBuf::from(path);
        
        // Check if on EBS
        for ebs_path in &self.config.storage_hints.ebs_paths {
            if path_buf.starts_with(ebs_path) {
                return StorageType::EBS;
            }
        }
        
        // Check if on local SSD
        for ssd_path in &self.config.storage_hints.local_ssd_paths {
            if path_buf.starts_with(ssd_path) {
                return StorageType::LocalSSD;
            }
        }
        
        // Default based on hints
        if self.config.storage_hints.fast_local_storage {
            StorageType::EBS
        } else {
            StorageType::Remote
        }
    }
    
    /// Read via memory mapping
    async fn read_via_mmap(
        &self,
        collection: &CollectionCache,
        file_path: &str,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>, ProximaDBError> {
        if let Some(mmap_file) = collection.files.get_mmap(file_path).await {
            // Read from mmap
            let data = mmap_file.read_range(offset, size)?;
            self.stats.file_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(data);
        }
        
        // Create new mmap
        let file = std::fs::File::open(file_path)?;
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        
        let mmap_file = Arc::new(MmappedFile {
            path: file_path.to_string(),
            mmap: Arc::new(mmap),
            regions: Self::identify_regions(file_path)?,
            last_access: RwLock::new(Instant::now()),
        });
        
        collection.files.add_mmap(file_path, mmap_file.clone()).await;
        self.stats.mmap_count.fetch_add(1, Ordering::Relaxed);
        
        Ok(mmap_file.read_range(offset, size)?)
    }
    
    /// Direct read from EBS
    async fn read_direct(
        &self,
        file_path: &str,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>, ProximaDBError> {
        // Direct read - no caching needed for EBS
        let fs = self.filesystem.get_filesystem(file_path)?;
        fs.read_range(file_path, offset, size).await
    }
    
    /// Read via disk cache (for remote storage)
    async fn read_via_cache(
        &self,
        collection: &CollectionCache,
        file_path: &str,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>, ProximaDBError> {
        if let Some(cache) = &collection.files.disk_cache {
            // Check cache
            if let Some(data) = cache.get(file_path, offset, size).await? {
                self.stats.file_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(data);
            }
            
            // Download and cache
            let fs = self.filesystem.get_filesystem(file_path)?;
            let data = fs.read_range(file_path, offset, size).await?;
            
            cache.put(file_path, offset, &data).await?;
            self.stats.disk_cache_size.fetch_add(data.len() as u64, Ordering::Relaxed);
            
            Ok(data)
        } else {
            // No disk cache - direct read
            self.read_direct(file_path, offset, size).await
        }
    }
    
    /// Check if eviction is needed
    async fn maybe_evict_collection(&self) -> Result<(), ProximaDBError> {
        let memory_used = self.total_memory_used.load(Ordering::Relaxed);
        
        if memory_used > self.config.memory_config.total_metadata_limit {
            // Select collection to evict
            let collection_id = self.select_collection_to_evict()?;
            self.invalidate_collection(&collection_id).await?;
        }
        
        Ok(())
    }
    
    /// Select collection to evict based on policy
    fn select_collection_to_evict(&self) -> Result<String, ProximaDBError> {
        match self.config.eviction_policy {
            CollectionEvictionPolicy::LRU => {
                // Find least recently used
                let mut oldest = Instant::now();
                let mut oldest_id = String::new();
                
                for entry in self.collections.iter() {
                    let last_access = entry.value().get_last_access();
                    if last_access < oldest {
                        oldest = last_access;
                        oldest_id = entry.key().clone();
                    }
                }
                
                Ok(oldest_id)
            },
            _ => {
                // Simple: evict first found
                self.collections.iter()
                    .next()
                    .map(|e| e.key().clone())
                    .ok_or_else(|| ProximaDBError::Internal("No collections to evict".into()))
            }
        }
    }
    
    /// Identify file regions
    fn identify_regions(file_path: &str) -> Result<Vec<FileRegion>, ProximaDBError> {
        if file_path.ends_with(".sst") {
            Ok(vec![
                FileRegion { region_type: RegionType::SstBloom, offset: 0, size: 4096 },
                FileRegion { region_type: RegionType::SstIndex, offset: 4096, size: 61440 },
            ])
        } else if file_path.ends_with(".parquet") {
            Ok(vec![
                FileRegion { region_type: RegionType::ParquetFooter, offset: 0, size: 8 * 1024 * 1024 },
            ])
        } else {
            Ok(Vec::new())
        }
    }
    
    /// Get cache statistics
    pub fn get_stats(&self) -> CacheStatsSummary {
        CacheStatsSummary {
            collections: self.collections.len(),
            memory_used: self.total_memory_used.load(Ordering::Relaxed),
            disk_used: self.total_disk_used.load(Ordering::Relaxed),
            metadata_hit_rate: calculate_hit_rate(
                self.stats.metadata_hits.load(Ordering::Relaxed),
                self.stats.metadata_misses.load(Ordering::Relaxed),
            ),
            file_hit_rate: calculate_hit_rate(
                self.stats.file_hits.load(Ordering::Relaxed),
                self.stats.file_misses.load(Ordering::Relaxed),
            ),
            mmap_files: self.stats.mmap_count.load(Ordering::Relaxed),
            collections_evicted: self.stats.collections_evicted.load(Ordering::Relaxed),
        }
    }
}

impl CollectionCache {
    fn new(collection_id: String, config: &IntegratedCacheConfig) -> Result<Self, ProximaDBError> {
        let disk_cache = if config.disk_config.enable_disk_cache {
            Some(Arc::new(DiskCache::new(
                config.base_cache_dir.join(&collection_id),
            )?))
        } else {
            None
        };
        
        Ok(Self {
            collection_id,
            metadata: Arc::new(MetadataCache::new()),
            files: Arc::new(FileCache::new(disk_cache)),
            stats: Arc::new(CollectionStats::default()),
            last_access: RwLock::new(Instant::now()),
        })
    }
    
    async fn update_access(&self) {
        *self.last_access.write().await = Instant::now();
        self.stats.access_count.fetch_add(1, Ordering::Relaxed);
    }
    
    fn get_last_access(&self) -> Instant {
        *self.last_access.blocking_read()
    }
}

impl MetadataCache {
    fn new() -> Self {
        Self {
            bloom_filters: DashMap::new(),
            index_blocks: DashMap::new(),
            superblocks: DashMap::new(),
            footers: DashMap::new(),
            column_indexes: DashMap::new(),
            page_indexes: DashMap::new(),
            memory_used: AtomicUsize::new(0),
        }
    }
    
    fn get(&self, file_id: &str, region_type: &RegionType) -> Option<Arc<Vec<u8>>> {
        match region_type {
            RegionType::SstBloom => self.bloom_filters.get(file_id).map(|e| e.clone()),
            RegionType::SstIndex => self.index_blocks.get(file_id).map(|e| e.clone()),
            RegionType::SstSuperblock => self.superblocks.get(file_id).map(|e| e.clone()),
            RegionType::ParquetFooter => self.footers.get(file_id).map(|e| e.clone()),
            RegionType::ParquetColumnIndex => self.column_indexes.get(file_id).map(|e| e.clone()),
            _ => None,
        }
    }
    
    fn put(&self, file_id: &str, region_type: RegionType, data: Arc<Vec<u8>>) -> Result<(), ProximaDBError> {
        let size = data.len();
        
        match region_type {
            RegionType::SstBloom => self.bloom_filters.insert(file_id.to_string(), data),
            RegionType::SstIndex => self.index_blocks.insert(file_id.to_string(), data),
            RegionType::SstSuperblock => self.superblocks.insert(file_id.to_string(), data),
            RegionType::ParquetFooter => self.footers.insert(file_id.to_string(), data),
            RegionType::ParquetColumnIndex => self.column_indexes.insert(file_id.to_string(), data),
            _ => return Err(ProximaDBError::InvalidArgument("Data blocks should not be in metadata cache".into())),
        };
        
        self.memory_used.fetch_add(size, Ordering::Relaxed);
        Ok(())
    }
    
    fn clear(&self) -> usize {
        let mut freed = 0;
        
        for entry in self.bloom_filters.iter() {
            freed += entry.value().len();
        }
        self.bloom_filters.clear();
        
        for entry in self.index_blocks.iter() {
            freed += entry.value().len();
        }
        self.index_blocks.clear();
        
        for entry in self.footers.iter() {
            freed += entry.value().len();
        }
        self.footers.clear();
        
        self.memory_used.store(0, Ordering::Relaxed);
        freed
    }
}

impl FileCache {
    fn new(disk_cache: Option<Arc<DiskCache>>) -> Self {
        Self {
            mmap_files: DashMap::new(),
            disk_cache,
            file_handles: DashMap::new(),
        }
    }
    
    async fn get_mmap(&self, file_path: &str) -> Option<Arc<MmappedFile>> {
        self.mmap_files.get(file_path).map(|e| e.clone())
    }
    
    async fn add_mmap(&self, file_path: &str, mmap: Arc<MmappedFile>) {
        self.mmap_files.insert(file_path.to_string(), mmap);
    }
    
    async fn clear(&self) -> Result<u64, ProximaDBError> {
        self.mmap_files.clear();
        self.file_handles.clear();
        
        if let Some(cache) = &self.disk_cache {
            cache.clear().await
        } else {
            Ok(0)
        }
    }
}

impl MmappedFile {
    fn read_range(&self, offset: u64, size: u64) -> Result<Vec<u8>, ProximaDBError> {
        let start = offset as usize;
        let end = (offset + size) as usize;
        
        if end > self.mmap.len() {
            return Err(ProximaDBError::InvalidArgument("Range out of bounds".into()));
        }
        
        Ok(self.mmap[start..end].to_vec())
    }
}

impl DiskCache {
    fn new(cache_dir: PathBuf) -> Result<Self, ProximaDBError> {
        std::fs::create_dir_all(&cache_dir)?;
        
        Ok(Self {
            cache_dir,
            cached_files: DashMap::new(),
            disk_used: AtomicU64::new(0),
        })
    }
    
    async fn get(&self, file_path: &str, offset: u64, size: u64) -> Result<Option<Vec<u8>>, ProximaDBError> {
        // Check if we have this file cached
        if let Some(cached) = self.cached_files.get(file_path) {
            // Check if range is cached
            for (start, end) in &cached.ranges {
                if *start <= offset && *end >= offset + size {
                    // Read from cache
                    let data = tokio::fs::read(&cached.cache_path).await?;
                    let range_start = (offset - start) as usize;
                    let range_end = range_start + size as usize;
                    return Ok(Some(data[range_start..range_end].to_vec()));
                }
            }
        }
        
        Ok(None)
    }
    
    async fn put(&self, file_path: &str, offset: u64, data: &[u8]) -> Result<(), ProximaDBError> {
        let cache_file = self.cache_dir.join(format!("{}_{}", 
            file_path.replace('/', "_"), offset));
        
        tokio::fs::write(&cache_file, data).await?;
        
        self.cached_files.entry(file_path.to_string())
            .and_modify(|e| {
                e.ranges.push((offset, offset + data.len() as u64));
                e.last_access = Instant::now();
            })
            .or_insert(CachedFile {
                original_path: file_path.to_string(),
                cache_path: cache_file,
                size: data.len() as u64,
                ranges: vec![(offset, offset + data.len() as u64)],
                last_access: Instant::now(),
            });
        
        self.disk_used.fetch_add(data.len() as u64, Ordering::Relaxed);
        
        Ok(())
    }
    
    async fn clear(&self) -> Result<u64, ProximaDBError> {
        let size = self.disk_used.load(Ordering::Relaxed);
        
        // Remove all cached files
        for entry in self.cached_files.iter() {
            tokio::fs::remove_file(&entry.value().cache_path).await.ok();
        }
        
        self.cached_files.clear();
        self.disk_used.store(0, Ordering::Relaxed);
        
        // Remove cache directory
        tokio::fs::remove_dir_all(&self.cache_dir).await.ok();
        
        Ok(size)
    }
}

#[derive(Debug, Clone)]
enum StorageType {
    LocalSSD,  // Use mmap
    EBS,       // Direct read
    Remote,    // Use disk cache
}

fn calculate_hit_rate(hits: u64, misses: u64) -> f64 {
    let total = hits + misses;
    if total > 0 {
        hits as f64 / total as f64
    } else {
        0.0
    }
}

impl Default for CacheStats {
    fn default() -> Self {
        Self {
            metadata_hits: AtomicU64::new(0),
            metadata_misses: AtomicU64::new(0),
            file_hits: AtomicU64::new(0),
            file_misses: AtomicU64::new(0),
            mmap_count: AtomicU64::new(0),
            disk_cache_size: AtomicU64::new(0),
            collections_evicted: AtomicU64::new(0),
        }
    }
}

impl Default for CollectionStats {
    fn default() -> Self {
        Self {
            access_count: AtomicU64::new(0),
            metadata_size: AtomicUsize::new(0),
            cache_size: AtomicU64::new(0),
        }
    }
}

impl Default for IntegratedCacheConfig {
    fn default() -> Self {
        Self {
            base_cache_dir: PathBuf::from("/var/cache/proximadb"),
            memory_config: MemoryConfig {
                max_metadata_per_collection: 512 * 1024 * 1024,  // 512MB
                total_metadata_limit: 4 * 1024 * 1024 * 1024,    // 4GB
                enable_mmap: true,
                pressure_threshold: 0.8,
            },
            disk_config: DiskConfig {
                max_disk_per_collection: 10 * 1024 * 1024 * 1024,  // 10GB
                total_disk_limit: 100 * 1024 * 1024 * 1024,        // 100GB
                enable_disk_cache: true,  // Set to false for EBS
                compression: CompressionType::Lz4,
            },
            storage_hints: StorageHints {
                fast_local_storage: false,
                remote_storage: true,
                ebs_paths: vec![],
                local_ssd_paths: vec![],
            },
            eviction_policy: CollectionEvictionPolicy::LRU,
        }
    }
}

#[derive(Debug)]
pub struct CacheStatsSummary {
    pub collections: usize,
    pub memory_used: usize,
    pub disk_used: u64,
    pub metadata_hit_rate: f64,
    pub file_hit_rate: f64,
    pub mmap_files: u64,
    pub collections_evicted: u64,
}