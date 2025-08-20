/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! High-performance query result caching for SQL engine
//!
//! Implements a lock-free concurrent cache for SQL query results using DashMap
//! with LRU eviction and smart cache invalidation strategies optimized for
//! vector database workloads.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum number of cached query results (memory-conscious for SQL parsing workload)
const DEFAULT_MAX_CACHE_ENTRIES: usize = 1000;

/// Default TTL for cached query results (5 minutes)
const DEFAULT_CACHE_TTL_SECONDS: u64 = 300;

/// Cache cleanup interval (1 minute)
const CACHE_CLEANUP_INTERVAL_SECONDS: u64 = 60;

/// Query result cache key for deterministic hashing
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct QueryCacheKey {
    /// Normalized SQL query string
    pub query_hash: u64,
    /// Collection ID for cache invalidation
    pub collection_id: String,
    /// Query parameters hash (for parameterized queries)
    pub params_hash: u64,
}

impl QueryCacheKey {
    /// Create cache key from SQL query and collection
    pub fn new(sql_query: &str, collection_id: &str, params: Option<&[u8]>) -> Self {
        use std::collections::hash_map::DefaultHasher;
        
        // Normalize query for consistent caching (remove extra whitespace, etc.)
        let normalized_query = Self::normalize_query(sql_query);
        
        let mut hasher = DefaultHasher::new();
        normalized_query.hash(&mut hasher);
        let query_hash = hasher.finish();
        
        let params_hash = if let Some(p) = params {
            let mut hasher = DefaultHasher::new();
            p.hash(&mut hasher);
            hasher.finish()
        } else {
            0
        };
        
        Self {
            query_hash,
            collection_id: collection_id.to_string(),
            params_hash,
        }
    }
    
    /// Normalize SQL query for consistent caching
    fn normalize_query(query: &str) -> String {
        // Simple normalization: remove extra whitespace and convert to lowercase
        query.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }
}

/// Cached query result with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedQueryResult {
    /// Actual query result data
    pub result_data: Vec<u8>, // Serialized result
    /// Cache creation timestamp
    pub timestamp: u64,
    /// Last access timestamp (for LRU)
    pub last_accessed: u64,
    /// Access count for statistics
    pub access_count: u64,
    /// Original query for debugging
    pub original_query: String,
    /// Result size in bytes
    pub size_bytes: usize,
}

impl CachedQueryResult {
    /// Create new cached result
    pub fn new(result_data: Vec<u8>, original_query: String) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let size_bytes = result_data.len();
        
        Self {
            result_data,
            timestamp: now,
            last_accessed: now,
            access_count: 1,
            original_query,
            size_bytes,
        }
    }
    
    /// Update access timestamp and count
    pub fn touch(&mut self) {
        self.last_accessed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.access_count += 1;
    }
    
    /// Check if cache entry is expired
    pub fn is_expired(&self, ttl_seconds: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        now - self.created_at >= ttl_seconds
    }
}

/// Cache configuration
#[derive(Debug, Clone)]
pub struct QueryCacheConfig {
    /// Maximum number of cache entries
    pub max_entries: usize,
    /// Time-to-live for cache entries in seconds
    pub ttl_seconds: u64,
    /// Enable/disable caching
    pub enabled: bool,
    /// Maximum result size to cache (in bytes)
    pub max_result_size_bytes: usize,
}

impl Default for QueryCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_CACHE_ENTRIES,
            ttl_seconds: DEFAULT_CACHE_TTL_SECONDS,
            enabled: true,
            max_result_size_bytes: 1024 * 1024, // 1MB max result size
        }
    }
}

/// Cache statistics for monitoring
#[derive(Debug, Default)]
pub struct QueryCacheStats {
    /// Total cache hits
    pub hits: AtomicU64,
    /// Total cache misses
    pub misses: AtomicU64,
    /// Total cache insertions
    pub insertions: AtomicU64,
    /// Total cache evictions
    pub evictions: AtomicU64,
    /// Current cache size
    pub current_size: AtomicUsize,
    /// Total memory usage (bytes)
    pub memory_usage_bytes: AtomicUsize,
}

impl QueryCacheStats {
    /// Get hit ratio as percentage
    pub fn hit_ratio(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed) as f64;
        let total = hits + self.misses.load(Ordering::Relaxed) as f64;
        
        if total > 0.0 {
            (hits / total) * 100.0
        } else {
            0.0
        }
    }
    
    /// Get cache efficiency summary
    pub fn summary(&self) -> String {
        format!(
            "Cache Stats - Hits: {}, Misses: {}, Hit Ratio: {:.1}%, Size: {}, Memory: {:.1}KB",
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
            self.hit_ratio(),
            self.current_size.load(Ordering::Relaxed),
            self.memory_usage_bytes.load(Ordering::Relaxed) as f64 / 1024.0
        )
    }
}

/// High-performance query result cache with lock-free concurrent access
pub struct QueryCache {
    /// Lock-free concurrent cache storage
    cache: DashMap<QueryCacheKey, CachedQueryResult>,
    /// Cache configuration
    config: QueryCacheConfig,
    /// Cache statistics
    stats: Arc<QueryCacheStats>,
    /// Last cleanup timestamp
    last_cleanup: AtomicU64,
}

impl QueryCache {
    /// Create new query result cache
    pub fn new(config: QueryCacheConfig) -> Self {
        Self {
            cache: DashMap::new(),
            config,
            stats: Arc::new(QueryCacheStats::default()),
            last_cleanup: AtomicU64::new(0),
        }
    }
    
    /// Get cached query result
    pub fn get(&self, key: &QueryCacheKey) -> Option<Vec<u8>> {
        if !self.config.enabled {
            return None;
        }
        
        if let Some(mut entry) = self.cache.get_mut(key) {
            // Check if entry is expired
            if entry.is_expired(self.config.ttl_seconds) {
                drop(entry); // Release the lock
                self.cache.remove(key);
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            
            // Update access information
            entry.touch();
            let result = entry.result_data.clone();
            
            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some(result)
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
    
    /// Insert query result into cache
    pub fn insert(&self, key: QueryCacheKey, result_data: Vec<u8>, original_query: String) {
        if !self.config.enabled {
            return;
        }
        
        // Don't cache results that are too large
        if result_data.len() > self.config.max_result_size_bytes {
            return;
        }
        
        // Check if cache is full and needs cleanup
        if self.cache.len() >= self.config.max_entries {
            self.evict_lru_entries();
        }
        
        let cached_result = CachedQueryResult::new(result_data, original_query);
        let size_bytes = cached_result.size_bytes;
        
        self.cache.insert(key, cached_result);
        
        self.stats.insertions.fetch_add(1, Ordering::Relaxed);
        self.stats.current_size.fetch_add(1, Ordering::Relaxed);
        self.stats.memory_usage_bytes.fetch_add(size_bytes, Ordering::Relaxed);
        
        // Periodic cleanup
        self.maybe_cleanup();
    }
    
    /// Invalidate cache entries for a specific collection
    pub fn invalidate_collection(&self, collection_id: &str) {
        let mut removed_count = 0;
        let mut removed_bytes = 0;
        
        self.cache.retain(|key, value| {
            if key.collection_id == collection_id {
                removed_count += 1;
                removed_bytes += value.size_bytes;
                false
            } else {
                true
            }
        });
        
        if removed_count > 0 {
            self.stats.evictions.fetch_add(removed_count, Ordering::Relaxed);
            self.stats.current_size.fetch_sub(removed_count as usize, Ordering::Relaxed);
            self.stats.memory_usage_bytes.fetch_sub(removed_bytes, Ordering::Relaxed);
        }
    }
    
    /// Clear all cache entries
    pub fn clear(&self) {
        let size = self.cache.len();
        let _memory = self.get_total_memory_usage();
        
        self.cache.clear();
        
        self.stats.current_size.store(0, Ordering::Relaxed);
        self.stats.memory_usage_bytes.store(0, Ordering::Relaxed);
        self.stats.evictions.fetch_add(size as u64, Ordering::Relaxed);
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> Arc<QueryCacheStats> {
        Arc::clone(&self.stats)
    }
    
    /// Get current cache size
    pub fn size(&self) -> usize {
        self.cache.len()
    }
    
    /// Get total memory usage
    pub fn get_total_memory_usage(&self) -> usize {
        self.cache.iter().map(|entry| entry.size_bytes).sum()
    }
    
    /// Evict least recently used entries
    fn evict_lru_entries(&self) {
        let target_size = (self.config.max_entries as f64 * 0.8) as usize; // Remove 20%
        let current_size = self.cache.len();
        
        if current_size <= target_size {
            return;
        }
        
        // Collect entries with their last access times
        let mut entries: Vec<_> = self.cache.iter()
            .map(|entry| (entry.key().clone(), entry.last_accessed))
            .collect();
        
        // Sort by last accessed time (oldest first)
        entries.sort_by_key(|(_, last_accessed)| *last_accessed);
        
        // Remove oldest entries
        let to_remove = current_size - target_size;
        let mut removed_count = 0;
        let mut removed_bytes = 0;
        
        for (key, _) in entries.into_iter().take(to_remove) {
            if let Some((_, value)) = self.cache.remove(&key) {
                removed_count += 1;
                removed_bytes += value.size_bytes;
            }
        }
        
        self.stats.evictions.fetch_add(removed_count, Ordering::Relaxed);
        self.stats.current_size.fetch_sub(removed_count as usize, Ordering::Relaxed);
        self.stats.memory_usage_bytes.fetch_sub(removed_bytes, Ordering::Relaxed);
    }
    
    /// Cleanup expired entries (periodic maintenance)
    fn cleanup_expired(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let mut removed_count = 0;
        let mut removed_bytes = 0;
        
        self.cache.retain(|_, value| {
            if value.is_expired(self.config.ttl_seconds) {
                removed_count += 1;
                removed_bytes += value.size_bytes;
                false
            } else {
                true
            }
        });
        
        if removed_count > 0 {
            self.stats.evictions.fetch_add(removed_count, Ordering::Relaxed);
            self.stats.current_size.fetch_sub(removed_count as usize, Ordering::Relaxed);
            self.stats.memory_usage_bytes.fetch_sub(removed_bytes, Ordering::Relaxed);
        }
        
        self.last_cleanup.store(now, Ordering::Relaxed);
    }
    
    /// Maybe perform cleanup if enough time has passed
    fn maybe_cleanup(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let last_cleanup = self.last_cleanup.load(Ordering::Relaxed);
        
        if now - last_cleanup > CACHE_CLEANUP_INTERVAL_SECONDS {
            self.cleanup_expired();
        }
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new(QueryCacheConfig::default())
    }
}

// Thread-safe: DashMap is lock-free and thread-safe
unsafe impl Send for QueryCache {}
unsafe impl Sync for QueryCache {}

/// Global query result cache instance
use std::sync::OnceLock;
static GLOBAL_QUERY_CACHE: OnceLock<QueryCache> = OnceLock::new();

/// Get global query result cache
pub fn get_global_query_cache() -> &'static QueryCache {
    GLOBAL_QUERY_CACHE.get_or_init(QueryCache::default)
}

/// Convenience function to cache query result globally
pub fn cache_query_result(key: QueryCacheKey, result_data: Vec<u8>, original_query: String) {
    get_global_query_cache().insert(key, result_data, original_query);
}

/// Convenience function to get cached query result globally
pub fn get_cached_query_result(key: &QueryCacheKey) -> Option<Vec<u8>> {
    get_global_query_cache().get(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;
    
    #[test]
    fn test_cache_key_creation() {
        let key1 = QueryCacheKey::new(
            "SELECT * FROM products LIMIT 10",
            "collection_1",
            None
        );
        
        let key2 = QueryCacheKey::new(
            "SELECT   *   FROM   products   LIMIT   10", // Extra whitespace
            "collection_1",
            None
        );
        
        // Should normalize to same key
        assert_eq!(key1.query_hash, key2.query_hash);
        assert_eq!(key1.collection_id, key2.collection_id);
    }
    
    #[test]
    fn test_basic_cache_operations() {
        let cache = QueryCache::default();
        
        let key = QueryCacheKey::new("SELECT * FROM users", "test_collection", None);
        let result_data = b"test results".to_vec();
        
        // Insert and retrieve
        cache.insert(key.clone(), result_data.clone(), "SELECT * FROM test".to_string());
        let cached = cache.get(&key);
        
        assert_eq!(cached, Some(result_data));
        
        // Check statistics
        let stats = cache.stats();
        assert_eq!(stats.hits.load(Ordering::Relaxed), 1);
        assert_eq!(stats.insertions.load(Ordering::Relaxed), 1);
    }
    
    #[test]
    fn test_cache_expiration() {
        let config = QueryCacheConfig {
            ttl_seconds: 1, // 1 second TTL
            ..Default::default()
        };
        let cache = QueryCache::new(config);
        
        let key = QueryCacheKey::new("SELECT * FROM users", "test_collection", None);
        let result_data = b"test results".to_vec();
        
        // Insert data
        cache.insert(key.clone(), result_data.clone(), "SELECT * FROM test".to_string());
        
        // Should be available immediately
        assert!(cache.get(&key).is_some());
        
        // Wait for expiration
        thread::sleep(Duration::from_secs(2));
        
        // Should be expired and removed
        assert!(cache.get(&key).is_empty());
    }
    
    #[test]
    fn test_collection_invalidation() {
        let cache = QueryCache::default();
        
        let key1 = QueryCacheKey::new("SELECT * FROM test1", "collection_1", None);
        let key2 = QueryCacheKey::new("SELECT * FROM test2", "collection_2", None);
        let result_data = b"test results".to_vec();
        
        // Insert data for both collections
        cache.insert(key1.clone(), result_data.clone(), "SELECT * FROM test1".to_string());
        cache.insert(key2.clone(), result_data.clone(), "SELECT * FROM test2".to_string());
        
        // Both should be available
        assert!(cache.get(&key1).is_some());
        assert!(cache.get(&key2).is_some());
        
        // Invalidate collection_1
        cache.invalidate_collection("collection_1");
        
        // Only collection_1 should be invalidated
        assert!(cache.get(&key1).is_empty());
        assert!(cache.get(&key2).is_some());
    }
    
    #[test]
    fn test_lru_eviction() {
        let config = QueryCacheConfig {
            max_entries: 3,
            ..Default::default()
        };
        let cache = QueryCache::new(config);
        
        // Insert 4 entries (exceeds max)
        for i in 0..4 {
            let key = QueryCacheKey::new(&format!("SELECT * FROM test{}", i), "collection", None);
            let result_data = format!("result {}", i).into_bytes();
            cache.insert(key, result_data, format!("SELECT * FROM test{}", i));
            
            // Delay of 1 second to ensure different timestamps (since timestamps are in seconds)
            if i < 3 {
                thread::sleep(Duration::from_secs(1));
            }
        }
        
        // Should have triggered eviction
        assert!(cache.size() <= 3);
        
        // First entry should be evicted (oldest)
        let key0 = QueryCacheKey::new("SELECT * FROM test0", "collection", None);
        assert!(cache.get(&key0).is_empty());
    }
    
    #[test]
    fn test_concurrent_access() {
        let cache = Arc::new(QueryCache::default());
        
        let handles: Vec<_> = (0..10).map(|i| {
            let cache_clone = Arc::clone(&cache);
            thread::spawn(move || {
                let key = QueryCacheKey::new(&format!("SELECT * FROM test{}", i), "collection", None);
                let result_data = format!("result {}", i).into_bytes();
                
                // Insert
                cache_clone.insert(key.clone(), result_data.clone(), format!("SELECT * FROM test{}", i));
                
                // Retrieve
                let cached = cache_clone.get(key);
                assert_eq!(cached, Some(result_data));
            })
        }).collect();
        
        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }
        
        // Check final state
        assert_eq!(cache.size(), 10);
        let stats = cache.stats();
        assert_eq!(stats.hits.load(Ordering::Relaxed), 10);
        assert_eq!(stats.insertions.load(Ordering::Relaxed), 10);
    }
    
    #[test]
    fn test_memory_usage_tracking() {
        let cache = QueryCache::default();
        
        let key = QueryCacheKey::new("SELECT * FROM test", "collection", None);
        let result_data = b"test result data with some length".to_vec();
        let expected_size = result_data.len();
        
        cache.insert(key.clone(), result_data, "SELECT * FROM test".to_string());
        
        let stats = cache.stats();
        assert_eq!(stats.memory_usage_bytes.load(Ordering::Relaxed), expected_size);
        
        // Clear cache
        cache.clear();
        assert_eq!(stats.memory_usage_bytes.load(Ordering::Relaxed), 0);
    }
    
    #[test]
    fn test_global_cache() {
        let key = QueryCacheKey::new("SELECT * FROM global_test", "global_collection", None);
        let result_data = b"global test result".to_vec();
        
        // Use global cache functions
        cache_query_result(key.clone(), result_data.clone(), "SELECT * FROM global_test".to_string());
        let cached = get_cached_query_result(&key);
        
        assert_eq!(cached, Some(result_data));
        
        // Verify it's the same global instance
        let cache1 = get_global_query_cache();
        let cache2 = get_global_query_cache();
        assert!(std::ptr::eq(cache1, cache2));
    }
    
    #[test]
    fn test_cache_disabled() {
        let config = QueryCacheConfig {
            enabled: false,
            ..Default::default()
        };
        let cache = QueryCache::new(config);
        
        let key = QueryCacheKey::new("SELECT * FROM test", "collection", None);
        let result_data = b"test results".to_vec();
        
        // Insert should be ignored
        cache.insert(key.clone(), result_data, "SELECT * FROM test".to_string());
        
        // Get should return None
        assert!(cache.get(&key).is_some());
        assert_eq!(cache.size(), 0);
    }
    
    #[test]
    fn test_large_result_filtering() {
        let config = QueryCacheConfig {
            max_result_size_bytes: 10, // Very small limit
            ..Default::default()
        };
        let cache = QueryCache::new(config);
        
        let key = QueryCacheKey::new("SELECT * FROM test", "collection", None);
        let large_result = b"this is a very large result that exceeds the limit".to_vec();
        
        // Should not cache large results
        cache.insert(key.clone(), large_result, "SELECT * FROM test".to_string());
        
        assert!(cache.get(&key).is_some());
        assert_eq!(cache.size(), 0);
    }
}