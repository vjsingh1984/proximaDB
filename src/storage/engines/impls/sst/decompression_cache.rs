//! SST Decompression Cache
//!
//! This module provides a caching layer for decompressed SSTable blocks to avoid
//! repeated decompression of frequently accessed data. It uses an LRU eviction
//! policy with configurable size limits.

use crate::utils::cache::LruCache;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::storage::engines::core::formats::fastlanes_blocks::FastLanesDataBlock;
use crate::storage::cache::orchestrator::{CacheStatsProvider, UsageStats};

/// Cache key for decompressed blocks
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct BlockCacheKey {
    /// SSTable file path
    pub file_path: String,
    /// Block ID within the file
    pub block_id: u32,
    /// Block offset in file
    pub block_offset: u64,
}

/// Cached block data
#[derive(Debug, Clone)]
pub struct CachedBlock {
    /// Decompressed block data
    pub data: FastLanesDataBlock,
    /// Size in bytes
    pub size_bytes: usize,
    /// Timestamp when cached
    pub cached_at: i64,
    /// Number of times accessed
    pub access_count: u64,
    /// Original compression algorithm
    pub compression_algorithm: Option<crate::core::compression::CompressionAlgorithm>,
}

/// Decompression cache for SSTable blocks with automatic invalidation
pub struct DecompressionCache {
    /// LRU cache for decompressed blocks
    block_cache: Arc<RwLock<LruCache<BlockCacheKey, CachedBlock>>>,
    /// Maximum cache size in bytes
    max_size_bytes: usize,
    /// Current cache size in bytes
    current_size_bytes: Arc<RwLock<usize>>,
    /// Cache statistics
    stats: Arc<RwLock<CacheStats>>,
    /// Compression-specific sub-caches for better locality
    compression_caches:
        Arc<RwLock<HashMap<crate::core::compression::CompressionAlgorithm, Vec<BlockCacheKey>>>>,
    /// File modification timestamps for invalidation
    file_timestamps: Arc<dashmap::DashMap<String, i64>>,
    /// Configuration from TOML
    config: CacheConfig,
    /// Invalidation task handle
    invalidation_task: Option<tokio::task::JoinHandle<()>>,
}

/// Cache statistics
#[derive(Debug, Default, Clone)]
pub struct CacheStats {
    /// Total cache hits
    pub hits: u64,
    /// Total cache misses
    pub misses: u64,
    /// Total bytes saved from decompression
    pub bytes_saved: u64,
    /// Total decompression time saved (microseconds)
    pub time_saved_us: u64,
    /// Number of evictions
    pub evictions: u64,
    /// Peak cache size in bytes
    pub peak_size_bytes: usize,
}

impl std::fmt::Debug for DecompressionCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecompressionCache")
            .field("max_size_bytes", &self.max_size_bytes)
            .field("config", &self.config)
            .field("has_invalidation_task", &self.invalidation_task.is_some())
            .finish()
    }
}

impl DecompressionCache {
    /// Create a new decompression cache from configuration
    pub fn from_config(config: CacheConfig) -> Self {
        // Apply configurable limits
        let max_size_mb = config
            .max_size_mb
            .min(config.max_cap_mb) // Apply cap
            .max(config.min_size_mb); // Apply minimum

        let max_size_bytes = max_size_mb * 1024 * 1024;
        let capacity = 1000; // Start with 1000 entries

        info!(
            "🗂️ SST Decompression Cache initialized: max_size={}MB, prefetch={}, ttl={}s",
            max_size_mb, config.enable_prefetch, config.ttl_seconds
        );

        let mut cache = Self {
            block_cache: Arc::new(RwLock::new(LruCache::new(capacity))),
            max_size_bytes,
            current_size_bytes: Arc::new(RwLock::new(0)),
            stats: Arc::new(RwLock::new(CacheStats::default())),
            compression_caches: Arc::new(RwLock::new(HashMap::new())),
            file_timestamps: Arc::new(dashmap::DashMap::new()),
            config: config.clone(),
            invalidation_task: None,
        };

        // Start invalidation task if configured
        if config.invalidation_check_interval_seconds > 0 {
            cache.start_invalidation_task();
        }

        cache
    }

    /// Create with default configuration
    pub fn new(max_size_mb: usize) -> Self {
        let config = CacheConfig {
            max_size_mb,
            ..Default::default()
        };
        Self::from_config(config)
    }

    /// Start background invalidation task
    fn start_invalidation_task(&mut self) {
        let block_cache = Arc::clone(&self.block_cache);
        let file_timestamps = Arc::clone(&self.file_timestamps);
        let current_size = Arc::clone(&self.current_size_bytes);
        let stats = Arc::clone(&self.stats);
        let interval = self.config.invalidation_check_interval_seconds;

        let handle = tokio::spawn(async move {
            let mut interval_timer =
                tokio::time::interval(tokio::time::Duration::from_secs(interval));

            loop {
                interval_timer.tick().await;

                // Check for stale entries
                let mut invalidated = 0;
                let mut cache = block_cache.write().await;
                let mut size = current_size.write().await;

                // Get current file timestamps (would check actual files in production)
                let keys_to_remove: Vec<BlockCacheKey> = cache
                    .iter()
                    .filter_map(|(key, cached_block)| {
                        // Check if file has been modified
                        if let Some(last_modified) = file_timestamps.get(&key.file_path) {
                            if *last_modified > cached_block.cached_at {
                                return Some(key.clone());
                            }
                        }
                        None
                    })
                    .collect();

                // Remove invalidated entries
                for key in keys_to_remove {
                    if let Some(removed) = cache.pop(&key) {
                        *size -= removed.size_bytes;
                        invalidated += 1;
                    }
                }

                if invalidated > 0 {
                    let mut s = stats.write().await;
                    s.evictions += invalidated;
                    info!(
                        "🔄 Cache invalidation: removed {} stale entries",
                        invalidated
                    );
                }
            }
        });

        self.invalidation_task = Some(handle);
    }

    /// Notify cache of file modification
    pub async fn invalidate_file(&self, file_path: &str) {
        // Update file timestamp
        self.file_timestamps
            .insert(file_path.to_string(), chrono::Utc::now().timestamp());

        // Remove all blocks from this file
        let mut cache = self.block_cache.write().await;
        let mut current_size = self.current_size_bytes.write().await;
        let mut stats = self.stats.write().await;

        let keys_to_remove: Vec<BlockCacheKey> = cache
            .iter()
            .filter_map(|(key, _)| {
                if key.file_path == file_path {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect();

        let mut invalidated = 0;
        let mut freed_bytes = 0;

        for key in keys_to_remove {
            if let Some(removed) = cache.pop(&key) {
                freed_bytes += removed.size_bytes;
                invalidated += 1;
            }
        }

        *current_size -= freed_bytes;
        stats.evictions += invalidated;

        if invalidated > 0 {
            info!(
                "🔄 Invalidated {} cache entries for modified file: {} (freed {}KB)",
                invalidated,
                file_path,
                freed_bytes / 1024
            );
        }
    }

    /// Invalidate cache entries for a collection
    pub async fn invalidate_collection(&self, collection_id: &str) {
        let mut cache = self.block_cache.write().await;
        let mut current_size = self.current_size_bytes.write().await;

        let keys_to_remove: Vec<BlockCacheKey> = cache
            .iter()
            .filter_map(|(key, _)| {
                if key.file_path.contains(collection_id) {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect();

        let mut freed_bytes = 0;
        for key in keys_to_remove {
            if let Some(removed) = cache.pop(&key) {
                freed_bytes += removed.size_bytes;
            }
        }

        *current_size -= freed_bytes;

        info!(
            "🔄 Invalidated cache for collection: {} (freed {}MB)",
            collection_id,
            freed_bytes / (1024 * 1024)
        );
    }

    /// Get a decompressed block from cache
    pub async fn get(&self, key: &BlockCacheKey) -> Option<FastLanesDataBlock> {
        let mut cache = self.block_cache.write().await;
        let mut stats = self.stats.write().await;

        if let Some(cached_block) = cache.get_mut(key) {
            // Update access count
            cached_block.access_count += 1;
            stats.hits += 1;

            // Estimate decompression time saved based on compression algorithm
            let time_saved = Self::estimate_decompression_time(
                cached_block.size_bytes,
                cached_block.compression_algorithm.clone(),
            );
            stats.time_saved_us += time_saved;
            stats.bytes_saved += cached_block.size_bytes as u64;

            debug!(
                "🎯 Cache hit for block {}:{} ({}KB, {} accesses)",
                key.file_path,
                key.block_id,
                cached_block.size_bytes / 1024,
                cached_block.access_count
            );

            Some(cached_block.data.clone())
        } else {
            stats.misses += 1;
            debug!("❌ Cache miss for block {}:{}", key.file_path, key.block_id);
            None
        }
    }

    /// Put a decompressed block into cache
    pub async fn put(
        &self,
        key: BlockCacheKey,
        data: FastLanesDataBlock,
        compression_algorithm: Option<crate::core::compression::CompressionAlgorithm>,
    ) -> Result<()> {
        // Calculate block size
        let size_bytes = Self::calculate_block_size(&data);

        // Check if adding this block would exceed cache size
        let mut current_size = self.current_size_bytes.write().await;

        if *current_size + size_bytes > self.max_size_bytes {
            // Need to evict blocks
            let bytes_to_free = *current_size + size_bytes - self.max_size_bytes;
            drop(current_size); // Release the lock before calling evict_blocks
            self.evict_blocks(bytes_to_free).await?;
            current_size = self.current_size_bytes.write().await; // Re-acquire the lock
        }

        // Create cached block
        let cached_block = CachedBlock {
            data,
            size_bytes,
            cached_at: chrono::Utc::now().timestamp(),
            access_count: 0,
            compression_algorithm: compression_algorithm.clone(),
        };

        // Add to cache
        let mut cache = self.block_cache.write().await;
        if let Some(evicted) = cache.put(key.clone(), cached_block) {
            *current_size -= evicted.size_bytes;
        }
        *current_size += size_bytes;

        // Update compression-specific cache index
        if let Some(algo) = compression_algorithm {
            let mut comp_caches = self.compression_caches.write().await;
            comp_caches
                .entry(algo.clone())
                .or_insert_with(Vec::new)
                .push(key.clone());
        }

        // Update statistics
        let mut stats = self.stats.write().await;
        if *current_size > stats.peak_size_bytes {
            stats.peak_size_bytes = *current_size;
        }

        debug!(
            "📥 Cached block {}:{} ({}KB), cache size: {}MB/{}MB",
            key.file_path,
            key.block_id,
            size_bytes / 1024,
            *current_size / (1024 * 1024),
            self.max_size_bytes / (1024 * 1024)
        );

        Ok(())
    }

    /// Evict blocks to free up space
    async fn evict_blocks(&self, bytes_to_free: usize) -> Result<()> {
        let mut cache = self.block_cache.write().await;
        let mut current_size = self.current_size_bytes.write().await;
        let mut stats = self.stats.write().await;

        let mut freed_bytes = 0;
        let mut evicted_count = 0;

        // LRU eviction - the cache automatically evicts least recently used items
        while freed_bytes < bytes_to_free && cache.len() > 0 {
            // Pop the least recently used item
            if let Some((key, cached_block)) = cache.pop_lru() {
                freed_bytes += cached_block.size_bytes;
                *current_size -= cached_block.size_bytes;
                evicted_count += 1;

                debug!(
                    "🗑️ Evicted block {}:{} ({}KB, {} accesses)",
                    key.file_path,
                    key.block_id,
                    cached_block.size_bytes / 1024,
                    cached_block.access_count
                );
            } else {
                break;
            }
        }

        stats.evictions += evicted_count;

        if freed_bytes > 0 {
            info!(
                "🗑️ Evicted {} blocks, freed {}MB",
                evicted_count,
                freed_bytes / (1024 * 1024)
            );
        }

        Ok(())
    }

    /// Calculate the size of a data block
    fn calculate_block_size(block: &FastLanesDataBlock) -> usize {
        // Estimate based on VectorRecords in the block
        block
            .records
            .iter()
            .map(|r| {
                std::mem::size_of::<crate::core::VectorRecord>()
                    + r.id.len()
                    + r.vector.len() * std::mem::size_of::<f32>()
                    + r.metadata.iter().map(|(k, _)| k.len() + 8).sum::<usize>() // Rough metadata size
            })
            .sum()
    }

    /// Estimate decompression time based on algorithm and size
    fn estimate_decompression_time(
        size_bytes: usize,
        algorithm: Option<crate::core::compression::CompressionAlgorithm>,
    ) -> u64 {
        // Rough estimates based on typical decompression speeds
        match algorithm {
            Some(crate::core::compression::CompressionAlgorithm::Zstd) => {
                (size_bytes as u64) / 1000
            } // ~1GB/s
            Some(crate::core::compression::CompressionAlgorithm::Lz4) => (size_bytes as u64) / 2000, // ~2GB/s
            Some(crate::core::compression::CompressionAlgorithm::Snappy) => {
                (size_bytes as u64) / 1500
            } // ~1.5GB/s
            _ => 0,
        }
    }

    /// Clear the entire cache
    pub async fn clear(&self) {
        let mut cache = self.block_cache.write().await;
        cache.clear();

        let mut current_size = self.current_size_bytes.write().await;
        *current_size = 0;

        let mut comp_caches = self.compression_caches.write().await;
        comp_caches.clear();

        info!("🗑️ Decompression cache cleared");
    }

    /// Get cache statistics
    pub async fn get_stats(&self) -> CacheStats {
        self.stats.read().await.clone()
    }

    /// Get current cache size in bytes
    pub async fn get_current_size(&self) -> usize {
        *self.current_size_bytes.read().await
    }

    /// Get cache hit rate
    pub async fn get_hit_rate(&self) -> f64 {
        let stats = self.stats.read().await;
        let total = stats.hits + stats.misses;
        if total == 0 {
            0.0
        } else {
            stats.hits as f64 / total as f64
        }
    }

    /// Prefetch blocks for a file (warming the cache)
    pub async fn prefetch_file_blocks(
        &self,
        file_path: &str,
        blocks: Vec<(
            u32,
            FastLanesDataBlock,
            Option<crate::core::compression::CompressionAlgorithm>,
        )>,
    ) -> Result<()> {
        info!(
            "📥 Prefetching {} blocks for file {}",
            blocks.len(),
            file_path
        );

        for (block_id, data, algo) in blocks {
            let key = BlockCacheKey {
                file_path: file_path.to_string(),
                block_id,
                block_offset: 0, // Will be set properly in actual usage
            };

            self.put(key, data, algo).await?;
        }

        Ok(())
    }

    /// Get blocks by compression algorithm (for optimization)
    pub async fn get_blocks_by_algorithm(
        &self,
        algorithm: crate::core::compression::CompressionAlgorithm,
    ) -> Vec<BlockCacheKey> {
        let comp_caches = self.compression_caches.read().await;
        comp_caches.get(&algorithm).cloned().unwrap_or_default()
    }
}

/// Orchestrator stats provider for the SST DecompressionCache
pub struct DecompressionCacheStatsProvider {
    cache: Arc<DecompressionCache>,
}

impl DecompressionCacheStatsProvider {
    pub fn new(cache: Arc<DecompressionCache>) -> Self { Self { cache } }
}

impl CacheStatsProvider for DecompressionCacheStatsProvider {
    fn snapshot(&self) -> UsageStats {
        // Attempt to get an instantaneous snapshot using the Tokio runtime
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let stats = handle.block_on(self.cache.get_stats());
            let total = stats.hits + stats.misses;
            let hit_rate = if total == 0 { 0.0 } else { stats.hits as f64 / total as f64 };
            // Approximate avg entry size using bytes_saved per hit when available
            let avg_entry_size = if stats.hits > 0 {
                (stats.bytes_saved / stats.hits) as usize
            } else {
                64 * 1024
            };
            return UsageStats {
                hit_rate,
                avg_entry_size,
                access_frequency: total as f64,
                last_rebalance: std::time::SystemTime::now(),
            };
        }
        UsageStats { hit_rate: 0.0, avg_entry_size: 64 * 1024, access_frequency: 0.0, last_rebalance: std::time::SystemTime::now() }
    }
}

/// Cache configuration
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct CacheConfig {
    /// Maximum cache size in MB
    pub max_size_mb: usize,
    /// Minimum cache size in MB (0 = no minimum)
    #[serde(default)]
    pub min_size_mb: usize,
    /// Maximum cache size cap in MB (0 = no cap)
    #[serde(default = "CacheConfig::default_max_cap")]
    pub max_cap_mb: usize,
    /// Enable prefetching
    pub enable_prefetch: bool,
    /// Prefetch threshold (number of accesses before prefetching related blocks)
    pub prefetch_threshold: u64,
    /// TTL for cached entries in seconds (0 = no TTL)
    pub ttl_seconds: u64,
    /// Cache invalidation check interval in seconds
    pub invalidation_check_interval_seconds: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size_mb: 512, // 512MB default
            min_size_mb: 64,  // 64MB minimum in production
            max_cap_mb: 8192, // 8GB cap
            enable_prefetch: true,
            prefetch_threshold: 3, // Prefetch after 3 accesses
            ttl_seconds: 0,        // No TTL by default
            invalidation_check_interval_seconds: 60, // Check every minute
        }
    }
}

impl CacheConfig {
    /// Default maximum cap value
    fn default_max_cap() -> usize {
        8192 // 8GB
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test cache config with minimal values
    fn test_cache_config(max_size_mb: usize) -> CacheConfig {
        CacheConfig {
            max_size_mb,
            min_size_mb: 0,   // No minimum for tests
            max_cap_mb: 8192, // Keep cap at 8GB
            enable_prefetch: false,
            prefetch_threshold: 3,
            ttl_seconds: 0,
            invalidation_check_interval_seconds: 0,
        }
    }

    #[tokio::test]
    async fn test_cache_basic_operations() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let cache = DecompressionCache::from_config(test_cache_config(10)); // 10MB cache

        let key = BlockCacheKey {
            file_path: "test.sstable".to_string(),
            block_id: 1,
            block_offset: 0,
        };

        // Test miss
        assert!(cache.get(&key).await.is_some());

        // Test put and hit
        let block = DataBlock::new(1, vec![]);
        cache
            .put(
                key.clone(),
                block.clone(),
                Some(crate::core::compression::CompressionAlgorithm::Zstd),
            )
            .await
            .unwrap();

        assert!(cache.get(&key).await.is_some());

        // Check stats
        let stats = cache.stats().await;
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let cache = DecompressionCache::from_config(test_cache_config(1)); // 1MB cache - very small for testing

        // Fill cache with blocks - create fewer but larger blocks to ensure we exceed cache size
        for i in 0..20 {
            let key = BlockCacheKey {
                file_path: "test.sstable".to_string(),
                block_id: i,
                block_offset: 0,
            };

            // Create a large block with 500 VectorRecords, each with 256-dim vectors
            // This should be approximately 500 * (256 * 4 + overhead) = ~512KB per block
            let mut records = vec![];
            for j in 0..500 {
                records.push(crate::core::VectorRecord {
                    id: Some(format!("id_long_name_for_testing_{}", j)),
                    vector: vec![0.0; 256], // 256-dim vector = 1KB per vector
                    metadata: vec![],
                    timestamp: 0,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    quantized_vector: None,
                });
            }

            let block = DataBlock::new(i, records);
            cache.put(key, block, None).await.unwrap();
        }

        // Check that evictions happened
        let stats = cache.stats().await;
        assert!(
            stats.evictions > 0,
            "Expected evictions but got none. Cache stats: hits={}, misses={}, evictions={}, peak_size={}",
            stats.hits,
            stats.misses,
            stats.evictions,
            stats.peak_size_bytes
        );

        // Cache size should be under limit
        let current_size = cache.get_current_size().await;
        assert!(current_size <= 1024 * 1024);
    }
}

/// Stats provider implementation for Cross-Cache Orchestrator integration
pub struct DecompressionCacheStatsProvider {
    cache: Arc<DecompressionCache>,
}

impl DecompressionCacheStatsProvider {
    pub fn new(cache: Arc<DecompressionCache>) -> Self {
        Self { cache }
    }
}

impl CacheStatsProvider for DecompressionCacheStatsProvider {
    fn snapshot(&self) -> UsageStats {
        // Since we can't await in a sync trait method, we'll use try_lock 
        // or provide default stats if cache is busy
        let stats = if let Ok(stats) = self.cache.stats.try_read() {
            let hit_rate = if stats.hits + stats.misses > 0 {
                stats.hits as f64 / (stats.hits + stats.misses) as f64
            } else {
                0.0
            };
            
            let access_frequency = if stats.hits + stats.misses > 0 {
                (stats.hits + stats.misses) as f64 / 60.0  // requests per minute approximation
            } else {
                0.0
            };
            
            // Estimate average entry size from peak usage
            let avg_entry_size = if let Ok(cache) = self.cache.block_cache.try_read() {
                if cache.len() > 0 {
                    stats.peak_size_bytes / cache.len()
                } else {
                    8192  // Default 8KB per entry
                }
            } else {
                8192
            };
            
            UsageStats {
                hit_rate,
                avg_entry_size,
                access_frequency,
                last_rebalance: std::time::SystemTime::now(),
            }
        } else {
            // Fallback stats if cache is busy
            UsageStats {
                hit_rate: 0.5,  // Assume moderate hit rate
                avg_entry_size: 8192,  // 8KB default
                access_frequency: 1.0,  // Moderate frequency
                last_rebalance: std::time::SystemTime::now(),
            }
        };
        
        stats
    }
}
