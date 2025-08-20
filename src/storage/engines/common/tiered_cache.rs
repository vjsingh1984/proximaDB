// Tiered Cache System for Storage Engines
// Implements a three-tier caching strategy:
// Tier 1 (Hot): Memory - Metadata, bloom filters, indexes
// Tier 2 (Warm): Disk Cache - Frequently accessed data blocks
// Tier 3 (Cold): Cloud/Remote Storage - Full files

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::common::errors::ProximaDBError;

/// Tiered cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredCacheConfig {
    /// Memory tier configuration
    pub memory_tier: MemoryTierConfig,
    
    /// Disk tier configuration
    pub disk_tier: DiskTierConfig,
    
    /// Eviction policies
    pub eviction: EvictionConfig,
    
    /// Prefetch configuration
    pub prefetch: PrefetchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTierConfig {
    /// Maximum memory for metadata (bloom, index, superblock)
    pub max_metadata_memory: usize,
    
    /// Maximum memory for hot data blocks
    pub max_data_memory: usize,
    
    /// Memory pressure threshold to start eviction
    pub pressure_threshold: f32,
    
    /// Items to always keep in memory
    pub pinned_items: Vec<CacheItemType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskTierConfig {
    /// Base directory for disk cache
    pub cache_directory: PathBuf,
    
    /// Maximum disk cache size
    pub max_disk_size: u64,
    
    /// Block size for disk writes
    pub block_size: usize,
    
    /// Enable compression for disk cache
    pub compression_enabled: bool,
    
    /// Compression algorithm
    pub compression_algorithm: CompressionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictionConfig {
    /// Eviction strategy
    pub strategy: EvictionStrategy,
    
    /// Time-based eviction TTL
    pub ttl_seconds: u64,
    
    /// Access count threshold for promotion
    pub promotion_threshold: u32,
    
    /// Access count threshold for demotion
    pub demotion_threshold: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchConfig {
    /// Enable predictive prefetching
    pub enabled: bool,
    
    /// Prefetch adjacent blocks
    pub prefetch_adjacent: bool,
    
    /// Number of blocks to prefetch
    pub prefetch_count: usize,
    
    /// Prefetch based on access patterns
    pub pattern_based: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CacheItemType {
    // SST metadata types
    BloomFilter,
    IndexBlock,
    SuperBlock,
    CompressionDictionary,
    
    // Parquet metadata types
    ParquetFooter,
    ColumnIndex,
    PageIndex,
    RowGroupMetadata,
    
    // Data types
    DataBlock,
    ColumnChunk,
    Page,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvictionStrategy {
    LRU,
    LFU,
    FIFO,
    TwoQueue,
    ARC,  // Adaptive Replacement Cache
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Lz4,
    Snappy,
    Zstd,
}

/// Tiered cache implementation
pub struct TieredCache {
    config: TieredCacheConfig,
    
    /// Tier 1: Memory cache for hot metadata
    memory_metadata: Arc<MemoryCache>,
    
    /// Tier 1: Memory cache for hot data
    memory_data: Arc<MemoryCache>,
    
    /// Tier 2: Disk cache for warm data
    disk_cache: Arc<DiskCache>,
    
    /// Access pattern tracker
    access_tracker: Arc<AccessPatternTracker>,
    
    /// Statistics
    stats: Arc<CacheStatistics>,
}

/// Memory cache for hot data
pub struct MemoryCache {
    /// Cached items
    items: DashMap<CacheKey, CacheEntry>,
    
    /// Current size in bytes
    current_size: AtomicUsize,
    
    /// Maximum size in bytes
    max_size: usize,
    
    /// Access queue for LRU
    access_queue: Arc<RwLock<AccessQueue>>,
}

/// Disk cache for warm data
pub struct DiskCache {
    /// Cache directory
    cache_dir: PathBuf,
    
    /// Index of cached items
    index: DashMap<CacheKey, DiskCacheEntry>,
    
    /// Current disk usage
    current_size: AtomicU64,
    
    /// Maximum disk size
    max_size: u64,
    
    /// Compression settings
    compression: Option<CompressionType>,
}

/// Cache key
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct CacheKey {
    /// File identifier (path or ID)
    pub file_id: String,
    
    /// Item type
    pub item_type: CacheItemType,
    
    /// Optional offset for data blocks
    pub offset: Option<u64>,
    
    /// Optional size
    pub size: Option<u64>,
}

/// Memory cache entry
pub struct CacheEntry {
    /// Cached data
    pub data: Arc<Vec<u8>>,
    
    /// Entry metadata
    pub metadata: EntryMetadata,
}

/// Disk cache entry
#[derive(Debug, Clone)]
pub struct DiskCacheEntry {
    /// Path to cached file
    pub file_path: PathBuf,
    
    /// Entry metadata
    pub metadata: EntryMetadata,
    
    /// Compression used
    pub compressed: bool,
    
    /// Original size if compressed
    pub original_size: Option<usize>,
}

/// Entry metadata
#[derive(Debug, Clone)]
pub struct EntryMetadata {
    /// Creation time
    pub created_at: Instant,
    
    /// Last access time
    pub last_access: Instant,
    
    /// Access count
    pub access_count: u32,
    
    /// Size in bytes
    pub size: usize,
    
    /// Priority (higher = more important)
    pub priority: u8,
}

/// Access queue for LRU/LFU
struct AccessQueue {
    /// Queue of keys in access order
    queue: Vec<CacheKey>,
    
    /// Position index for O(1) updates
    positions: HashMap<CacheKey, usize>,
}

/// Access pattern tracker
pub struct AccessPatternTracker {
    /// Access history
    history: DashMap<String, Vec<AccessRecord>>,
    
    /// Pattern predictions
    predictions: DashMap<String, PredictedAccess>,
}

#[derive(Debug, Clone)]
struct AccessRecord {
    key: CacheKey,
    timestamp: Instant,
    hit: bool,
}

#[derive(Debug, Clone)]
struct PredictedAccess {
    next_keys: Vec<CacheKey>,
    confidence: f32,
}

/// Cache statistics
pub struct CacheStatistics {
    // Memory tier stats
    pub memory_hits: AtomicU64,
    pub memory_misses: AtomicU64,
    pub memory_evictions: AtomicU64,
    
    // Disk tier stats
    pub disk_hits: AtomicU64,
    pub disk_misses: AtomicU64,
    pub disk_evictions: AtomicU64,
    
    // Remote/cloud stats
    pub remote_fetches: AtomicU64,
    pub bytes_downloaded: AtomicU64,
    pub bytes_saved: AtomicU64,
    
    // Promotion/demotion stats
    pub promotions: AtomicU64,
    pub demotions: AtomicU64,
}

impl TieredCache {
    pub fn new(config: TieredCacheConfig) -> Result<Self, ProximaDBError> {
        // Create cache directory if needed
        std::fs::create_dir_all(&config.disk_tier.cache_directory)?;
        
        let memory_metadata = Arc::new(MemoryCache::new(config.memory_tier.max_metadata_memory));
        let memory_data = Arc::new(MemoryCache::new(config.memory_tier.max_data_memory));
        let disk_cache = Arc::new(DiskCache::new(
            config.disk_tier.cache_directory.clone(),
            config.disk_tier.max_disk_size,
            config.disk_tier.compression_algorithm.clone(),
        )?);
        
        Ok(Self {
            config,
            memory_metadata,
            memory_data,
            disk_cache,
            access_tracker: Arc::new(AccessPatternTracker::new()),
            stats: Arc::new(CacheStatistics::default()),
        })
    }
    
    /// Get item from cache (checks all tiers)
    pub async fn get(&self, key: &CacheKey) -> Result<Option<Arc<Vec<u8>>>, ProximaDBError> {
        // Record access
        self.access_tracker.record_access(key.clone(), false);
        
        // Check if this is metadata that should be in memory
        if self.is_metadata(&key.item_type) {
            // Check memory metadata cache
            if let Some(data) = self.memory_metadata.get(key).await {
                self.stats.memory_hits.fetch_add(1, Ordering::Relaxed);
                self.access_tracker.record_access(key.clone(), true);
                return Ok(Some(data));
            }
            self.stats.memory_misses.fetch_add(1, Ordering::Relaxed);
        } else {
            // Check memory data cache for hot data
            if let Some(data) = self.memory_data.get(key).await {
                self.stats.memory_hits.fetch_add(1, Ordering::Relaxed);
                self.access_tracker.record_access(key.clone(), true);
                return Ok(Some(data));
            }
            self.stats.memory_misses.fetch_add(1, Ordering::Relaxed);
        }
        
        // Check disk cache
        if let Some(data) = self.disk_cache.get(key).await? {
            self.stats.disk_hits.fetch_add(1, Ordering::Relaxed);
            
            // Consider promotion to memory
            if self.should_promote(key).await {
                self.promote_to_memory(key, data.clone()).await?;
            }
            
            return Ok(Some(data));
        }
        
        self.stats.disk_misses.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    
    /// Put item into appropriate tier
    pub async fn put(
        &self,
        key: CacheKey,
        data: Vec<u8>,
        priority: u8,
    ) -> Result<(), ProximaDBError> {
        let size = data.len();
        let data_arc = Arc::new(data);
        
        // Determine target tier based on item type and size
        if self.is_metadata(&key.item_type) {
            // Metadata always goes to memory
            self.memory_metadata.put(key.clone(), data_arc.clone(), priority).await?;
        } else if self.is_hot_data(&key).await {
            // Hot data goes to memory
            self.memory_data.put(key.clone(), data_arc.clone(), priority).await?;
        } else {
            // Everything else goes to disk
            self.disk_cache.put(key.clone(), data_arc, priority).await?;
        }
        
        // Prefetch related items if configured
        if self.config.prefetch.enabled {
            self.prefetch_related(&key).await;
        }
        
        Ok(())
    }
    
    /// Invalidate all cache entries for a collection
    pub async fn invalidate_collection(&self, collection_id: &str) -> Result<u64, ProximaDBError> {
        let mut invalidated = 0;
        
        // Invalidate memory caches
        invalidated += self.memory_metadata.invalidate_pattern(collection_id).await;
        invalidated += self.memory_data.invalidate_pattern(collection_id).await;
        
        // Invalidate disk cache
        invalidated += self.disk_cache.invalidate_pattern(collection_id).await?;
        
        log::info!("Invalidated {} cache entries for collection {}", invalidated, collection_id);
        
        Ok(invalidated)
    }
    
    /// Check if item type is metadata
    fn is_metadata(&self, item_type: &CacheItemType) -> bool {
        matches!(
            item_type,
            CacheItemType::BloomFilter |
            CacheItemType::IndexBlock |
            CacheItemType::SuperBlock |
            CacheItemType::CompressionDictionary |
            CacheItemType::ParquetFooter |
            CacheItemType::ColumnIndex |
            CacheItemType::PageIndex |
            CacheItemType::RowGroupMetadata
        )
    }
    
    /// Check if data should be in memory (hot)
    async fn is_hot_data(&self, key: &CacheKey) -> bool {
        // Check access patterns
        if let Some(pattern) = self.access_tracker.get_pattern(&key.file_id) {
            return pattern.is_hot();
        }
        false
    }
    
    /// Check if item should be promoted to memory
    async fn should_promote(&self, key: &CacheKey) -> bool {
        if let Some(entry) = self.disk_cache.get_metadata(key).await {
            return entry.access_count >= self.config.eviction.promotion_threshold;
        }
        false
    }
    
    /// Promote item from disk to memory
    async fn promote_to_memory(&self, key: &CacheKey, data: Arc<Vec<u8>>) -> Result<(), ProximaDBError> {
        if self.is_metadata(&key.item_type) {
            self.memory_metadata.put(key.clone(), data, 10).await?;
        } else {
            self.memory_data.put(key.clone(), data, 5).await?;
        }
        
        self.stats.promotions.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    
    /// Prefetch related items
    async fn prefetch_related(&self, key: &CacheKey) {
        if !self.config.prefetch.enabled {
            return;
        }
        
        // Get predicted next accesses
        if let Some(predictions) = self.access_tracker.predict_next(&key.file_id) {
            for predicted_key in predictions.iter().take(self.config.prefetch.prefetch_count) {
                // Trigger async prefetch (don't wait)
                let cache = self.clone();
                let key = predicted_key.clone();
                tokio::spawn(async move {
                    let _ = cache.prefetch_item(key).await;
                });
            }
        }
    }
    
    /// Prefetch a single item
    async fn prefetch_item(&self, key: CacheKey) -> Result<(), ProximaDBError> {
        // This would fetch from remote storage and cache
        // Implementation depends on storage backend
        Ok(())
    }
    
    /// Get cache statistics
    pub fn get_stats(&self) -> CacheStats {
        CacheStats {
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
            memory_size: self.memory_metadata.current_size.load(Ordering::Relaxed) +
                        self.memory_data.current_size.load(Ordering::Relaxed),
            disk_size: self.disk_cache.current_size.load(Ordering::Relaxed),
            promotions: self.stats.promotions.load(Ordering::Relaxed),
            demotions: self.stats.demotions.load(Ordering::Relaxed),
            bytes_saved: self.stats.bytes_saved.load(Ordering::Relaxed),
        }
    }
}

impl MemoryCache {
    fn new(max_size: usize) -> Self {
        Self {
            items: DashMap::new(),
            current_size: AtomicUsize::new(0),
            max_size,
            access_queue: Arc::new(RwLock::new(AccessQueue::new())),
        }
    }
    
    async fn get(&self, key: &CacheKey) -> Option<Arc<Vec<u8>>> {
        if let Some(mut entry) = self.items.get_mut(key) {
            entry.metadata.last_access = Instant::now();
            entry.metadata.access_count += 1;
            
            // Update access queue
            self.access_queue.write().await.touch(key.clone());
            
            return Some(entry.data.clone());
        }
        None
    }
    
    async fn put(&self, key: CacheKey, data: Arc<Vec<u8>>, priority: u8) -> Result<(), ProximaDBError> {
        let size = data.len();
        
        // Check if we need to evict
        while self.current_size.load(Ordering::Relaxed) + size > self.max_size {
            self.evict_one().await?;
        }
        
        let entry = CacheEntry {
            data,
            metadata: EntryMetadata {
                created_at: Instant::now(),
                last_access: Instant::now(),
                access_count: 1,
                size,
                priority,
            },
        };
        
        self.items.insert(key.clone(), entry);
        self.current_size.fetch_add(size, Ordering::Relaxed);
        self.access_queue.write().await.push(key);
        
        Ok(())
    }
    
    async fn evict_one(&self) -> Result<(), ProximaDBError> {
        let key_to_evict = self.access_queue.write().await.pop_lru();
        
        if let Some(key) = key_to_evict {
            if let Some((_, entry)) = self.items.remove(&key) {
                self.current_size.fetch_sub(entry.metadata.size, Ordering::Relaxed);
            }
        }
        
        Ok(())
    }
    
    async fn invalidate_pattern(&self, pattern: &str) -> u64 {
        let mut count = 0;
        let keys_to_remove: Vec<_> = self.items.iter()
            .filter(|entry| entry.key().file_id.contains(pattern))
            .map(|entry| entry.key().clone())
            .collect();
        
        for key in keys_to_remove {
            if let Some((_, entry)) = self.items.remove(&key) {
                self.current_size.fetch_sub(entry.metadata.size, Ordering::Relaxed);
                count += 1;
            }
        }
        
        count
    }
}

impl DiskCache {
    fn new(cache_dir: PathBuf, max_size: u64, compression: CompressionType) -> Result<Self, ProximaDBError> {
        let compression = match compression {
            CompressionType::None => None,
            other => Some(other),
        };
        
        Ok(Self {
            cache_dir,
            index: DashMap::new(),
            current_size: AtomicU64::new(0),
            max_size,
            compression,
        })
    }
    
    async fn get(&self, key: &CacheKey) -> Result<Option<Arc<Vec<u8>>>, ProximaDBError> {
        if let Some(mut entry) = self.index.get_mut(key) {
            entry.metadata.last_access = Instant::now();
            entry.metadata.access_count += 1;
            
            // Read from disk
            let data = tokio::fs::read(&entry.file_path).await?;
            
            // Decompress if needed
            let data = if entry.compressed {
                self.decompress(&data)?
            } else {
                data
            };
            
            return Ok(Some(Arc::new(data)));
        }
        
        Ok(None)
    }
    
    async fn put(&self, key: CacheKey, data: Arc<Vec<u8>>, priority: u8) -> Result<(), ProximaDBError> {
        let file_path = self.cache_path_for(&key);
        
        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        // Compress if configured
        let (data_to_write, compressed, original_size) = if self.compression.is_some() {
            let compressed = self.compress(&data)?;
            let original = data.len();
            (compressed, true, Some(original))
        } else {
            (data.to_vec(), false, None)
        };
        
        // Write to disk
        tokio::fs::write(&file_path, &data_to_write).await?;
        
        let entry = DiskCacheEntry {
            file_path: file_path.clone(),
            metadata: EntryMetadata {
                created_at: Instant::now(),
                last_access: Instant::now(),
                access_count: 1,
                size: data_to_write.len(),
                priority,
            },
            compressed,
            original_size,
        };
        
        self.index.insert(key, entry);
        self.current_size.fetch_add(data_to_write.len() as u64, Ordering::Relaxed);
        
        Ok(())
    }
    
    async fn get_metadata(&self, key: &CacheKey) -> Option<EntryMetadata> {
        self.index.get(key).map(|entry| entry.metadata.clone())
    }
    
    async fn invalidate_pattern(&self, pattern: &str) -> Result<u64, ProximaDBError> {
        let mut count = 0;
        let keys_to_remove: Vec<_> = self.index.iter()
            .filter(|entry| entry.key().file_id.contains(pattern))
            .map(|entry| entry.key().clone())
            .collect();
        
        for key in keys_to_remove {
            if let Some((_, entry)) = self.index.remove(&key) {
                // Delete file from disk
                tokio::fs::remove_file(&entry.file_path).await.ok();
                self.current_size.fetch_sub(entry.metadata.size as u64, Ordering::Relaxed);
                count += 1;
            }
        }
        
        Ok(count)
    }
    
    fn cache_path_for(&self, key: &CacheKey) -> PathBuf {
        let safe_name = format!(
            "{}_{:?}_{}_{}",
            key.file_id.replace('/', "_").replace(':', "_"),
            key.item_type,
            key.offset.unwrap_or(0),
            key.size.unwrap_or(0)
        );
        
        self.cache_dir.join(safe_name)
    }
    
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>, ProximaDBError> {
        match self.compression.as_ref().unwrap() {
            CompressionType::Lz4 => {
                Ok(lz4::block::compress(data, None, false)?)
            },
            CompressionType::Snappy => {
                let mut encoder = snap::raw::Encoder::new();
                Ok(encoder.compress_vec(data)?)
            },
            CompressionType::Zstd => {
                Ok(zstd::encode_all(data, 3)?)
            },
            _ => Ok(data.to_vec()),
        }
    }
    
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>, ProximaDBError> {
        match self.compression.as_ref().unwrap() {
            CompressionType::Lz4 => {
                Ok(lz4::block::decompress(data, None)?)
            },
            CompressionType::Snappy => {
                let mut decoder = snap::raw::Decoder::new();
                Ok(decoder.decompress_vec(data)?)
            },
            CompressionType::Zstd => {
                Ok(zstd::decode_all(data)?)
            },
            _ => Ok(data.to_vec()),
        }
    }
}

impl AccessQueue {
    fn new() -> Self {
        Self {
            queue: Vec::new(),
            positions: HashMap::new(),
        }
    }
    
    fn touch(&mut self, key: CacheKey) {
        if let Some(&pos) = self.positions.get(&key) {
            // Move to end (most recently used)
            self.queue.remove(pos);
            self.queue.push(key.clone());
            
            // Update positions
            for (i, k) in self.queue.iter().enumerate() {
                self.positions.insert(k.clone(), i);
            }
        } else {
            // New key
            self.positions.insert(key.clone(), self.queue.len());
            self.queue.push(key);
        }
    }
    
    fn push(&mut self, key: CacheKey) {
        self.positions.insert(key.clone(), self.queue.len());
        self.queue.push(key);
    }
    
    fn pop_lru(&mut self) -> Option<CacheKey> {
        if !self.queue.is_empty() {
            let key = self.queue.remove(0);
            self.positions.remove(&key);
            
            // Update positions
            for (i, k) in self.queue.iter().enumerate() {
                self.positions.insert(k.clone(), i);
            }
            
            return Some(key);
        }
        None
    }
}

impl AccessPatternTracker {
    fn new() -> Self {
        Self {
            history: DashMap::new(),
            predictions: DashMap::new(),
        }
    }
    
    fn record_access(&self, key: CacheKey, hit: bool) {
        let record = AccessRecord {
            key: key.clone(),
            timestamp: Instant::now(),
            hit,
        };
        
        self.history.entry(key.file_id.clone())
            .or_insert_with(Vec::new)
            .push(record);
        
        // TODO: Update predictions based on patterns
    }
    
    fn get_pattern(&self, file_id: &str) -> Option<AccessPattern> {
        if let Some(history) = self.history.get(file_id) {
            // Analyze access pattern
            let recent_accesses = history.iter()
                .filter(|r| r.timestamp.elapsed() < Duration::from_secs(60))
                .count();
            
            if recent_accesses > 10 {
                return Some(AccessPattern::Hot);
            } else if recent_accesses > 3 {
                return Some(AccessPattern::Warm);
            }
        }
        
        Some(AccessPattern::Cold)
    }
    
    fn predict_next(&self, file_id: &str) -> Option<Vec<CacheKey>> {
        self.predictions.get(file_id)
            .map(|p| p.next_keys.clone())
    }
}

#[derive(Debug, Clone)]
enum AccessPattern {
    Hot,
    Warm,
    Cold,
}

impl AccessPattern {
    fn is_hot(&self) -> bool {
        matches!(self, AccessPattern::Hot)
    }
}

impl Clone for TieredCache {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            memory_metadata: self.memory_metadata.clone(),
            memory_data: self.memory_data.clone(),
            disk_cache: self.disk_cache.clone(),
            access_tracker: self.access_tracker.clone(),
            stats: self.stats.clone(),
        }
    }
}

impl Default for CacheStatistics {
    fn default() -> Self {
        Self {
            memory_hits: AtomicU64::new(0),
            memory_misses: AtomicU64::new(0),
            memory_evictions: AtomicU64::new(0),
            disk_hits: AtomicU64::new(0),
            disk_misses: AtomicU64::new(0),
            disk_evictions: AtomicU64::new(0),
            remote_fetches: AtomicU64::new(0),
            bytes_downloaded: AtomicU64::new(0),
            bytes_saved: AtomicU64::new(0),
            promotions: AtomicU64::new(0),
            demotions: AtomicU64::new(0),
        }
    }
}

#[derive(Debug)]
pub struct CacheStats {
    pub memory_hit_rate: f64,
    pub disk_hit_rate: f64,
    pub memory_size: usize,
    pub disk_size: u64,
    pub promotions: u64,
    pub demotions: u64,
    pub bytes_saved: u64,
}

impl Default for TieredCacheConfig {
    fn default() -> Self {
        Self {
            memory_tier: MemoryTierConfig {
                max_metadata_memory: 512 * 1024 * 1024,  // 512MB for metadata
                max_data_memory: 1024 * 1024 * 1024,     // 1GB for hot data
                pressure_threshold: 0.8,
                pinned_items: vec![
                    CacheItemType::BloomFilter,
                    CacheItemType::IndexBlock,
                    CacheItemType::ParquetFooter,
                ],
            },
            disk_tier: DiskTierConfig {
                cache_directory: PathBuf::from("/var/cache/proximadb"),
                max_disk_size: 100 * 1024 * 1024 * 1024,  // 100GB
                block_size: 64 * 1024,  // 64KB
                compression_enabled: true,
                compression_algorithm: CompressionType::Lz4,
            },
            eviction: EvictionConfig {
                strategy: EvictionStrategy::ARC,
                ttl_seconds: 3600,  // 1 hour
                promotion_threshold: 3,
                demotion_threshold: 1,
            },
            prefetch: PrefetchConfig {
                enabled: true,
                prefetch_adjacent: true,
                prefetch_count: 2,
                pattern_based: true,
            },
        }
    }
}