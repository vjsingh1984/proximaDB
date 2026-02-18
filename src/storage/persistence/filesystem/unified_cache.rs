//! Unified Metadata Cache Implementation
//!
//! A high-performance, lock-free metadata cache that consolidates all metadata caching
//! into a single, efficient structure.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::RwLock;
use tracing::{debug, trace};

use crate::storage::persistence::filesystem::FileMetadata;

/// Unified metadata cache with lock-free reads
pub struct UnifiedMetadataCache {
    /// Main cache storage using DashMap for lock-free concurrent access
    cache: Arc<DashMap<String, Arc<CachedMetadata>>>,

    /// Configuration
    #[allow(dead_code)]
    max_entries: usize,
    max_memory_mb: usize,
    default_ttl: Duration,

    /// Statistics
    stats: Arc<CacheStatistics>,

    /// Memory tracker
    memory_usage: Arc<RwLock<usize>>,
}

/// Cached metadata entry
#[derive(Debug, Clone)]
pub struct CachedMetadata {
    /// File metadata
    pub metadata: FileMetadata,

    /// Parquet footer (if applicable)
    pub parquet_footer: Option<Vec<u8>>,

    /// Bloom filter (if applicable)
    pub bloom_filter: Option<Vec<u8>>,

    /// Cache timestamp
    pub cached_at: Instant,

    /// TTL for this entry
    pub ttl: Duration,

    /// Access count
    pub access_count: u64,

    /// Size in bytes (for memory tracking)
    pub size_bytes: usize,
}

/// Cache statistics
#[derive(Debug, Default)]
pub struct CacheStatistics {
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
    evictions: std::sync::atomic::AtomicU64,
    #[allow(dead_code)]
    insertions: std::sync::atomic::AtomicU64,
}

impl UnifiedMetadataCache {
    /// Create a new unified metadata cache
    pub fn new(max_memory_mb: usize, default_ttl_secs: u64) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            max_entries: 100_000, // Default max entries
            max_memory_mb,
            default_ttl: Duration::from_secs(default_ttl_secs),
            stats: Arc::new(CacheStatistics::default()),
            memory_usage: Arc::new(RwLock::new(0)),
        }
    }

    /// Get metadata from cache
    pub async fn get(&self, key: &str) -> Option<Arc<CachedMetadata>> {
        let entry = self.cache.get(key)?;
        let cached = entry.value();

        // Check TTL
        if cached.cached_at.elapsed() > cached.ttl {
            trace!("Cache entry expired for {}", key);
            drop(entry);
            self.cache.remove(key);
            self.stats.record_miss();
            return None;
        }

        // Update access count (we clone the Arc, so this is safe)
        self.stats.record_hit();

        Some(Arc::clone(cached))
    }

    /// Put metadata into cache
    pub async fn put(&self, key: String, mut metadata: CachedMetadata) {
        // Calculate size
        metadata.size_bytes = Self::calculate_size(&metadata);

        // Check memory limit
        let mut memory_usage = self.memory_usage.write().await;
        if *memory_usage + metadata.size_bytes > self.max_memory_mb * 1024 * 1024 {
            // Need to evict
            self.evict_lru().await;
        }

        *memory_usage += metadata.size_bytes;
        drop(memory_usage);

        // Set TTL if not specified
        if metadata.ttl == Duration::ZERO {
            metadata.ttl = self.default_ttl;
        }

        self.cache.insert(key, Arc::new(metadata));
        self.stats.record_insertion();
    }

    /// Put a negative cache entry (file doesn't exist)
    pub async fn put_negative(&self, key: &str, ttl: Duration) {
        let metadata = CachedMetadata {
            metadata: FileMetadata {
                path: key.to_string(),
                size: 0,
                modified: None,
                created: None,
                is_directory: false,
                permissions: None,
                etag: None,
                storage_class: None,
            },
            parquet_footer: None,
            bloom_filter: None,
            cached_at: Instant::now(),
            ttl,
            access_count: 0,
            size_bytes: 64, // Small size for negative entries
        };

        self.cache.insert(key.to_string(), Arc::new(metadata));
    }

    /// Invalidate a cache entry
    pub async fn invalidate(&self, key: &str) {
        if let Some((_, entry)) = self.cache.remove(key) {
            let mut memory_usage = self.memory_usage.write().await;
            *memory_usage = memory_usage.saturating_sub(entry.size_bytes);
        }
    }

    /// Invalidate all entries with a given prefix
    pub async fn invalidate_prefix(&self, prefix: &str) {
        let keys_to_remove: Vec<String> = self
            .cache
            .iter()
            .filter(|entry| entry.key().starts_with(prefix))
            .map(|entry| entry.key().clone())
            .collect();

        for key in keys_to_remove {
            self.invalidate(&key).await;
        }
    }

    /// Clear all cache entries
    pub async fn clear(&self) {
        self.cache.clear();
        *self.memory_usage.write().await = 0;
        self.stats.reset();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.stats.hits.load(std::sync::atomic::Ordering::Relaxed),
            misses: self.stats.misses.load(std::sync::atomic::Ordering::Relaxed),
            evictions: self
                .stats
                .evictions
                .load(std::sync::atomic::Ordering::Relaxed),
            insertions: self
                .stats
                .insertions
                .load(std::sync::atomic::Ordering::Relaxed),
            entries: self.cache.len(),
            memory_usage_bytes: 0, // Will be updated
        }
    }

    /// Evict least recently used entries
    async fn evict_lru(&self) {
        // Simple LRU eviction - find oldest entries
        let mut entries: Vec<(String, Instant)> = self
            .cache
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().cached_at))
            .collect();

        entries.sort_by_key(|(_, time)| *time);

        // Evict 10% of entries
        let evict_count = entries.len() / 10;
        for (key, _) in entries.iter().take(evict_count) {
            self.invalidate(key).await;
            self.stats.record_eviction();
        }
    }

    /// Calculate the size of a cached metadata entry
    fn calculate_size(metadata: &CachedMetadata) -> usize {
        let mut size = std::mem::size_of::<CachedMetadata>();

        if let Some(ref footer) = metadata.parquet_footer {
            size += footer.len();
        }

        if let Some(ref filter) = metadata.bloom_filter {
            size += filter.len();
        }

        size
    }

    /// Update Parquet footer for an existing entry
    pub async fn update_parquet_footer(&self, key: &str, footer: Vec<u8>) {
        if let Some(mut entry) = self.cache.get_mut(key) {
            let mut new_metadata = (**entry).clone();
            new_metadata.parquet_footer = Some(footer);
            new_metadata.size_bytes = Self::calculate_size(&new_metadata);
            *entry = Arc::new(new_metadata);
        }
    }

    /// Update bloom filter for an existing entry
    pub async fn update_bloom_filter(&self, key: &str, filter: Vec<u8>) {
        if let Some(mut entry) = self.cache.get_mut(key) {
            let mut new_metadata = (**entry).clone();
            new_metadata.bloom_filter = Some(filter);
            new_metadata.size_bytes = Self::calculate_size(&new_metadata);
            *entry = Arc::new(new_metadata);
        }
    }

    /// Extract and cache Parquet metadata from file data
    pub async fn extract_parquet_metadata(&self, key: &str, data: &[u8]) -> Option<Vec<u8>> {
        // Check if this is a Parquet file
        if !key.ends_with(".parquet") || data.len() < 12 {
            return None;
        }

        // Parquet files have "PAR1" magic bytes at start and end
        if &data[0..4] != b"PAR1" || &data[data.len() - 4..] != b"PAR1" {
            return None;
        }

        // Footer length is stored in last 8 bytes before final magic
        let footer_len_bytes = &data[data.len() - 8..data.len() - 4];
        let footer_len = u32::from_le_bytes([
            footer_len_bytes[0],
            footer_len_bytes[1],
            footer_len_bytes[2],
            footer_len_bytes[3],
        ]) as usize;

        if footer_len > data.len() - 12 {
            return None;
        }

        // Extract footer
        let footer_start = data.len() - 8 - footer_len;
        let footer = data[footer_start..data.len() - 8].to_vec();

        // Cache the footer
        self.update_parquet_footer(key, footer.clone()).await;

        debug!(
            "Extracted and cached Parquet footer for {}: {} bytes",
            key,
            footer.len()
        );
        Some(footer)
    }

    /// Check if we have cached Parquet metadata
    pub async fn get_parquet_footer(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(entry) = self.get(key).await {
            return entry.parquet_footer.clone();
        }
        None
    }

    /// Check if we have cached bloom filter
    pub async fn get_bloom_filter(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(entry) = self.get(key).await {
            return entry.bloom_filter.clone();
        }
        None
    }
}

impl CacheStatistics {
    fn record_hit(&self) {
        self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_miss(&self) {
        self.misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_eviction(&self) {
        self.evictions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_insertion(&self) {
        self.insertions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn reset(&self) {
        self.hits.store(0, std::sync::atomic::Ordering::Relaxed);
        self.misses.store(0, std::sync::atomic::Ordering::Relaxed);
        self.evictions
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.insertions
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Public cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub insertions: u64,
    pub entries: usize,
    pub memory_usage_bytes: usize,
}

impl CacheStats {
    /// Calculate hit rate
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_cache_operations() {
        let cache = UnifiedMetadataCache::new(10, 300);

        let metadata = CachedMetadata {
            metadata: FileMetadata {
                path: "test_key".to_string(),
                size: 1024,
                modified: None,
                created: None,
                is_directory: false,
                permissions: None,
                etag: None,
                storage_class: None,
            },
            parquet_footer: None,
            bloom_filter: None,
            cached_at: Instant::now(),
            ttl: Duration::from_secs(60),
            access_count: 0,
            size_bytes: 0,
        };

        // Test put and get
        cache.put("test_key".to_string(), metadata.clone()).await;
        let retrieved = cache.get("test_key").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().metadata.size, 1024);

        // Test invalidation
        cache.invalidate("test_key").await;
        let retrieved = cache.get("test_key").await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_ttl_expiration() {
        let cache = UnifiedMetadataCache::new(10, 1); // 1 second TTL

        let metadata = CachedMetadata {
            metadata: FileMetadata {
                path: "expired_key".to_string(),
                size: 2048,
                modified: None,
                created: None,
                is_directory: false,
                permissions: None,
                etag: None,
                storage_class: None,
            },
            parquet_footer: None,
            bloom_filter: None,
            cached_at: Instant::now() - Duration::from_secs(2), // Already expired
            ttl: Duration::from_secs(1),
            access_count: 0,
            size_bytes: 0,
        };

        cache.put("expired_key".to_string(), metadata).await;

        // Should not retrieve expired entry
        let retrieved = cache.get("expired_key").await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_prefix_invalidation() {
        let cache = UnifiedMetadataCache::new(10, 300);

        let metadata = CachedMetadata {
            metadata: FileMetadata::default(),
            parquet_footer: None,
            bloom_filter: None,
            cached_at: Instant::now(),
            ttl: Duration::from_secs(60),
            access_count: 0,
            size_bytes: 0,
        };

        // Add multiple entries with same prefix
        cache.put("/data/file1".to_string(), metadata.clone()).await;
        cache.put("/data/file2".to_string(), metadata.clone()).await;
        cache
            .put("/other/file3".to_string(), metadata.clone())
            .await;

        // Invalidate by prefix
        cache.invalidate_prefix("/data/").await;

        assert!(cache.get("/data/file1").await.is_none());
        assert!(cache.get("/data/file2").await.is_none());
        assert!(cache.get("/other/file3").await.is_some());
    }
}

impl Default for FileMetadata {
    fn default() -> Self {
        Self {
            path: String::new(),
            size: 0,
            modified: None,
            created: None,
            is_directory: false,
            permissions: None,
            etag: None,
            storage_class: None,
        }
    }
}
