//! # Query Result Cache Implementation
//!
//! Core types and cache implementation for query result caching.
//! This cache benefits agentic AI workloads with repetitive queries.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use thiserror::Error;
use tracing::{debug, info};

use crate::query::federated::ExecutionResult;

/// Unique identifier for a cached query result
pub type QueryCacheKey = u64;

/// Error types for query cache operations
#[derive(Debug, Error)]
pub enum QueryCacheError {
    /// Query result not found in cache
    #[error("Query result not found: {0}")]
    NotFound(QueryCacheKey),

    /// Cache entry has expired
    #[error("Query result has expired: {0}")]
    Expired(QueryCacheKey),

    /// Cache is full and cannot accept new entries
    #[error("Query cache is full (max: {0})")]
    CacheFull(usize),

    /// Failed to compute query key
    #[error("Failed to compute query fingerprint: {0}")]
    FingerprintError(String),

    /// Internal cache error
    #[error("Internal cache error: {0}")]
    Internal(String),
}

/// Result type for query cache operations
pub type QueryCacheResult<T> = Result<T, QueryCacheError>;

/// Configuration for the query result cache
#[derive(Debug, Clone)]
pub struct QueryResultCacheConfig {
    /// Maximum number of cached results (default: 10000)
    pub max_entries: usize,
    /// Default TTL for cached results (default: 5 minutes)
    pub default_ttl: Duration,
    /// Enable automatic cleanup of expired entries (default: true)
    pub enable_cleanup: bool,
    /// Cleanup interval (default: 1 minute)
    pub cleanup_interval: Duration,
    /// Maximum size per cached result in bytes (default: 10MB)
    pub max_result_size_bytes: usize,
    /// Enable cache hit/miss metrics (default: true)
    pub enable_metrics: bool,
}

impl Default for QueryResultCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            default_ttl: Duration::from_secs(300), // 5 minutes
            enable_cleanup: true,
            cleanup_interval: Duration::from_secs(60), // 1 minute
            max_result_size_bytes: 10 * 1024 * 1024,   // 10MB
            enable_metrics: true,
        }
    }
}

/// A key that uniquely identifies a query for caching purposes
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QueryKey {
    /// The computed fingerprint of the query
    pub fingerprint: u64,
    /// Original SQL or query string (for debugging)
    pub query_string: String,
}

impl QueryKey {
    /// Create a new query key from a SQL string
    pub fn from_sql(sql: &str) -> Self {
        let fingerprint = Self::compute_fingerprint(sql);
        Self {
            fingerprint,
            query_string: sql.to_string(),
        }
    }

    /// Create a new query key from a SQL string with parameters
    pub fn from_sql_with_params(sql: &str, params: &[&str]) -> Self {
        let mut combined = sql.to_string();
        for param in params {
            combined.push('\0'); // Use null separator
            combined.push_str(param);
        }
        let fingerprint = Self::compute_fingerprint(&combined);
        Self {
            fingerprint,
            query_string: sql.to_string(),
        }
    }

    /// Compute a stable fingerprint using DefaultHasher
    fn compute_fingerprint(input: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        hasher.finish()
    }

    /// Get the cache key for DashMap lookup
    pub fn cache_key(&self) -> QueryCacheKey {
        self.fingerprint
    }
}

/// A cached query result with metadata
#[derive(Debug)]
pub struct CachedResult {
    /// The cached execution result
    pub result: ExecutionResult,
    /// Collections/tables that this result depends on
    pub dependencies: Vec<String>,
    /// Query fingerprint for verification
    pub query_fingerprint: u64,
    /// Time-to-live for this entry
    pub ttl: Duration,
    /// Creation timestamp
    pub created_at: Instant,
    /// Last access timestamp
    pub last_accessed: Instant,
    /// Number of times this result was accessed
    pub access_count: AtomicU64,
    /// Estimated size in bytes
    pub size_bytes: usize,
}

impl CachedResult {
    /// Check if this cached result has expired
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }

    /// Update the last access time and increment access count
    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
        self.access_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the age of this cached result
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Get the access count
    pub fn get_access_count(&self) -> u64 {
        self.access_count.load(Ordering::Relaxed)
    }
}

/// Thread-safe cache for query results
///
/// This cache provides high-performance caching of query results for
/// agentic AI workloads with repetitive queries. It features:
///
/// - Thread-safe concurrent access using DashMap
/// - TTL-based expiration
/// - Dependency tracking for invalidation
/// - LRU-like eviction when full
/// - Metrics for monitoring cache performance
pub struct QueryResultCache {
    /// The result cache using DashMap for concurrent access
    cache: DashMap<QueryCacheKey, Arc<CachedResult>>,
    /// Registry mapping collection names to affected cache keys
    /// Used for efficient invalidation when a collection is modified
    invalidation_registry: DashMap<String, HashSet<QueryCacheKey>>,
    /// Configuration
    config: QueryResultCacheConfig,
    /// Cache statistics
    stats: CacheStatistics,
}

/// Statistics for cache monitoring
#[derive(Debug, Default)]
pub struct CacheStatistics {
    /// Total cache hits
    pub hits: AtomicU64,
    /// Total cache misses
    pub misses: AtomicU64,
    /// Total entries inserted
    pub inserts: AtomicU64,
    /// Total entries evicted
    pub evictions: AtomicU64,
    /// Total entries invalidated
    pub invalidations: AtomicU64,
    /// Total entries expired
    pub expirations: AtomicU64,
}

impl CacheStatistics {
    /// Get the cache hit rate (0.0 to 1.0)
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }
}

impl QueryResultCache {
    /// Create a new query result cache with the given configuration
    pub fn new(config: QueryResultCacheConfig) -> Self {
        Self {
            cache: DashMap::new(),
            invalidation_registry: DashMap::new(),
            config,
            stats: CacheStatistics::default(),
        }
    }

    /// Create a new cache with default configuration
    pub fn with_defaults() -> Self {
        Self::new(QueryResultCacheConfig::default())
    }

    /// Get a cached result by query key
    ///
    /// Returns `Some(result)` if found and not expired, `None` otherwise.
    pub fn get(&self, key: &QueryKey) -> Option<Arc<CachedResult>> {
        let cache_key = key.cache_key();

        if let Some(entry) = self.cache.get_mut(&cache_key) {
            // Check expiration
            if entry.is_expired() {
                drop(entry);
                self.remove_entry(cache_key);
                self.stats.expirations.fetch_add(1, Ordering::Relaxed);
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            // Verify fingerprint matches (collision check)
            if entry.query_fingerprint != key.fingerprint {
                debug!(
                    expected = key.fingerprint,
                    actual = entry.query_fingerprint,
                    "Cache key collision detected"
                );
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            // Update access tracking (need to get mutable reference)
            let result = Arc::clone(&*entry);
            // We cannot call touch() on Arc directly, but we track access via stats
            drop(entry);

            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some(result)
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Insert a query result into the cache
    ///
    /// # Arguments
    /// * `key` - The query key
    /// * `result` - The execution result to cache
    /// * `dependencies` - Collection names that this result depends on
    pub fn insert(
        &self,
        key: QueryKey,
        result: ExecutionResult,
        dependencies: Vec<String>,
    ) -> QueryCacheResult<()> {
        self.insert_with_ttl(key, result, dependencies, self.config.default_ttl)
    }

    /// Insert a query result with a custom TTL
    pub fn insert_with_ttl(
        &self,
        key: QueryKey,
        result: ExecutionResult,
        dependencies: Vec<String>,
        ttl: Duration,
    ) -> QueryCacheResult<()> {
        // Estimate result size
        let size_bytes = self.estimate_result_size(&result);

        // Check size limit
        if size_bytes > self.config.max_result_size_bytes {
            debug!(
                size = size_bytes,
                max = self.config.max_result_size_bytes,
                "Query result too large to cache"
            );
            return Ok(()); // Don't cache, but not an error
        }

        // Check cache capacity
        if self.cache.len() >= self.config.max_entries {
            // Try to evict expired entries first
            let expired_count = self.cleanup_expired();

            // If still full, evict oldest entries
            if self.cache.len() >= self.config.max_entries {
                let evicted = self.evict_oldest(1);
                if evicted == 0 {
                    return Err(QueryCacheError::CacheFull(self.config.max_entries));
                }
            }

            if expired_count > 0 {
                debug!(
                    expired = expired_count,
                    "Evicted expired entries to make room"
                );
            }
        }

        let cache_key = key.cache_key();
        let now = Instant::now();

        let cached = Arc::new(CachedResult {
            result,
            dependencies: dependencies.clone(),
            query_fingerprint: key.fingerprint,
            ttl,
            created_at: now,
            last_accessed: now,
            access_count: AtomicU64::new(0),
            size_bytes,
        });

        // Insert into cache
        self.cache.insert(cache_key, cached);

        // Register dependencies for invalidation
        for dep in dependencies {
            self.invalidation_registry
                .entry(dep)
                .or_insert_with(HashSet::new)
                .insert(cache_key);
        }

        self.stats.inserts.fetch_add(1, Ordering::Relaxed);

        debug!(
            key = cache_key,
            ttl_secs = ttl.as_secs(),
            size_bytes,
            "Cached query result"
        );

        Ok(())
    }

    /// Remove a cached entry by key
    pub fn remove(&self, key: &QueryKey) -> bool {
        self.remove_entry(key.cache_key())
    }

    /// Internal method to remove an entry and clean up invalidation registry
    fn remove_entry(&self, cache_key: QueryCacheKey) -> bool {
        if let Some((_, cached)) = self.cache.remove(&cache_key) {
            // Remove from invalidation registry
            for dep in &cached.dependencies {
                if let Some(mut keys) = self.invalidation_registry.get_mut(dep) {
                    keys.remove(&cache_key);
                }
            }
            true
        } else {
            false
        }
    }

    /// Invalidate all cached results that depend on a collection
    ///
    /// This should be called when data in a collection is modified.
    pub fn invalidate_collection(&self, collection: &str) -> usize {
        let keys_to_remove: Vec<QueryCacheKey> = self
            .invalidation_registry
            .get(collection)
            .map(|keys| keys.iter().copied().collect())
            .unwrap_or_default();

        let count = keys_to_remove.len();

        for key in keys_to_remove {
            self.remove_entry(key);
        }

        // Clean up the registry entry
        self.invalidation_registry.remove(collection);

        if count > 0 {
            self.stats
                .invalidations
                .fetch_add(count as u64, Ordering::Relaxed);
            info!(
                collection,
                invalidated = count,
                "Invalidated cached query results"
            );
        }

        count
    }

    /// Invalidate all cached results that depend on any of the given collections
    pub fn invalidate_collections(&self, collections: &[&str]) -> usize {
        let mut total = 0;
        for collection in collections {
            total += self.invalidate_collection(collection);
        }
        total
    }

    /// Cleanup expired entries
    pub fn cleanup_expired(&self) -> usize {
        let expired_keys: Vec<QueryCacheKey> = self
            .cache
            .iter()
            .filter(|entry| entry.value().is_expired())
            .map(|entry| *entry.key())
            .collect();

        let count = expired_keys.len();

        for key in expired_keys {
            self.remove_entry(key);
        }

        if count > 0 {
            self.stats
                .expirations
                .fetch_add(count as u64, Ordering::Relaxed);
            debug!(expired = count, "Cleaned up expired cache entries");
        }

        count
    }

    /// Evict the oldest entries to make room
    fn evict_oldest(&self, count: usize) -> usize {
        // Collect entries with their creation times
        let mut entries: Vec<(QueryCacheKey, Instant)> = self
            .cache
            .iter()
            .map(|entry| (*entry.key(), entry.value().created_at))
            .collect();

        // Sort by age (oldest first)
        entries.sort_by(|a, b| a.1.cmp(&b.1));

        let to_evict = entries
            .into_iter()
            .take(count)
            .map(|(k, _)| k)
            .collect::<Vec<_>>();
        let evicted = to_evict.len();

        for key in to_evict {
            self.remove_entry(key);
        }

        if evicted > 0 {
            self.stats
                .evictions
                .fetch_add(evicted as u64, Ordering::Relaxed);
            debug!(evicted, "Evicted oldest cache entries");
        }

        evicted
    }

    /// Clear all cached entries
    pub fn clear(&self) {
        let count = self.cache.len();
        self.cache.clear();
        self.invalidation_registry.clear();

        if count > 0 {
            info!(cleared = count, "Cleared query result cache");
        }
    }

    /// Get the number of cached entries
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Check if a query is cached
    pub fn contains(&self, key: &QueryKey) -> bool {
        let cache_key = key.cache_key();
        if let Some(entry) = self.cache.get(&cache_key) {
            !entry.is_expired() && entry.query_fingerprint == key.fingerprint
        } else {
            false
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> QueryCacheStats {
        QueryCacheStats {
            entries: self.cache.len(),
            max_entries: self.config.max_entries,
            hits: self.stats.hits.load(Ordering::Relaxed),
            misses: self.stats.misses.load(Ordering::Relaxed),
            hit_rate: self.stats.hit_rate(),
            inserts: self.stats.inserts.load(Ordering::Relaxed),
            evictions: self.stats.evictions.load(Ordering::Relaxed),
            invalidations: self.stats.invalidations.load(Ordering::Relaxed),
            expirations: self.stats.expirations.load(Ordering::Relaxed),
            total_size_bytes: self.total_size_bytes(),
            tracked_collections: self.invalidation_registry.len(),
        }
    }

    /// Estimate the size of an execution result
    fn estimate_result_size(&self, result: &ExecutionResult) -> usize {
        // Rough estimation based on row count and schema
        let row_count = result.row_count();
        let field_count = result.schema.fields().len();

        // Assume average of 100 bytes per field per row
        let estimated_data = row_count * field_count * 100;

        // Add overhead for schema and metadata
        let schema_overhead = field_count * 64;

        estimated_data + schema_overhead + 256 // Base overhead
    }

    /// Get total size of all cached entries
    fn total_size_bytes(&self) -> usize {
        self.cache
            .iter()
            .map(|entry| entry.value().size_bytes)
            .sum()
    }

    /// Get configuration
    pub fn config(&self) -> &QueryResultCacheConfig {
        &self.config
    }
}

impl Default for QueryResultCache {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Public cache statistics
#[derive(Debug, Clone)]
pub struct QueryCacheStats {
    /// Number of cached entries
    pub entries: usize,
    /// Maximum allowed entries
    pub max_entries: usize,
    /// Total cache hits
    pub hits: u64,
    /// Total cache misses
    pub misses: u64,
    /// Cache hit rate (0.0 to 1.0)
    pub hit_rate: f64,
    /// Total inserts
    pub inserts: u64,
    /// Total evictions
    pub evictions: u64,
    /// Total invalidations
    pub invalidations: u64,
    /// Total expirations
    pub expirations: u64,
    /// Total size of cached data in bytes
    pub total_size_bytes: usize,
    /// Number of tracked collections for invalidation
    pub tracked_collections: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, RecordBatch, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn create_test_result() -> ExecutionResult {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, true),
        ]));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["1", "2"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
            ],
        )
        .unwrap();

        ExecutionResult::from_batch(batch)
    }

    #[test]
    fn test_query_key_creation() {
        let key1 = QueryKey::from_sql("SELECT * FROM test");
        let key2 = QueryKey::from_sql("SELECT * FROM test");
        let key3 = QueryKey::from_sql("SELECT * FROM other");

        assert_eq!(key1.fingerprint, key2.fingerprint);
        assert_ne!(key1.fingerprint, key3.fingerprint);
    }

    #[test]
    fn test_query_key_with_params() {
        let key1 = QueryKey::from_sql_with_params("SELECT * FROM $1", &["test"]);
        let key2 = QueryKey::from_sql_with_params("SELECT * FROM $1", &["test"]);
        let key3 = QueryKey::from_sql_with_params("SELECT * FROM $1", &["other"]);

        assert_eq!(key1.fingerprint, key2.fingerprint);
        assert_ne!(key1.fingerprint, key3.fingerprint);
    }

    #[test]
    fn test_cache_insert_and_get() {
        let cache = QueryResultCache::with_defaults();
        let key = QueryKey::from_sql("SELECT * FROM test");
        let result = create_test_result();

        cache
            .insert(key.clone(), result, vec!["test".to_string()])
            .unwrap();

        assert!(cache.contains(&key));
        assert_eq!(cache.len(), 1);

        let cached = cache.get(&key);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().result.row_count(), 2);
    }

    #[test]
    fn test_cache_miss() {
        let cache = QueryResultCache::with_defaults();
        let key = QueryKey::from_sql("SELECT * FROM nonexistent");

        assert!(!cache.contains(&key));
        assert!(cache.get(&key).is_none());

        let stats = cache.stats();
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn test_cache_invalidation() {
        let cache = QueryResultCache::with_defaults();

        // Insert entries depending on different collections
        let key1 = QueryKey::from_sql("SELECT * FROM test1");
        let key2 = QueryKey::from_sql("SELECT * FROM test2");
        let key3 = QueryKey::from_sql("SELECT * FROM test1 JOIN test2");

        cache
            .insert(
                key1.clone(),
                create_test_result(),
                vec!["test1".to_string()],
            )
            .unwrap();
        cache
            .insert(
                key2.clone(),
                create_test_result(),
                vec!["test2".to_string()],
            )
            .unwrap();
        cache
            .insert(
                key3.clone(),
                create_test_result(),
                vec!["test1".to_string(), "test2".to_string()],
            )
            .unwrap();

        assert_eq!(cache.len(), 3);

        // Invalidate test1 - should remove key1 and key3
        let invalidated = cache.invalidate_collection("test1");
        assert_eq!(invalidated, 2);
        assert_eq!(cache.len(), 1);
        assert!(!cache.contains(&key1));
        assert!(cache.contains(&key2));
        assert!(!cache.contains(&key3));
    }

    #[test]
    fn test_cache_expiration() {
        let config = QueryResultCacheConfig {
            default_ttl: Duration::from_millis(1),
            ..Default::default()
        };
        let cache = QueryResultCache::new(config);
        let key = QueryKey::from_sql("SELECT * FROM test");

        cache
            .insert(key.clone(), create_test_result(), vec!["test".to_string()])
            .unwrap();
        assert!(cache.contains(&key));

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(10));

        // Should not be found (expired)
        assert!(cache.get(&key).is_none());

        let stats = cache.stats();
        assert!(stats.expirations > 0 || stats.misses > 0);
    }

    #[test]
    fn test_cache_remove() {
        let cache = QueryResultCache::with_defaults();
        let key = QueryKey::from_sql("SELECT * FROM test");

        cache
            .insert(key.clone(), create_test_result(), vec!["test".to_string()])
            .unwrap();
        assert_eq!(cache.len(), 1);

        let removed = cache.remove(&key);
        assert!(removed);
        assert_eq!(cache.len(), 0);
        assert!(!cache.contains(&key));
    }

    #[test]
    fn test_cache_clear() {
        let cache = QueryResultCache::with_defaults();

        for i in 0..5 {
            let key = QueryKey::from_sql(&format!("SELECT * FROM test{}", i));
            cache
                .insert(key, create_test_result(), vec![format!("test{}", i)])
                .unwrap();
        }

        assert_eq!(cache.len(), 5);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_stats() {
        let cache = QueryResultCache::with_defaults();
        let key = QueryKey::from_sql("SELECT * FROM test");

        cache
            .insert(key.clone(), create_test_result(), vec!["test".to_string()])
            .unwrap();

        // Hit
        let _ = cache.get(&key);
        let _ = cache.get(&key);

        // Miss
        let missing = QueryKey::from_sql("SELECT * FROM missing");
        let _ = cache.get(&missing);

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.inserts, 1);
        assert!((stats.hit_rate - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_cache_capacity_eviction() {
        let config = QueryResultCacheConfig {
            max_entries: 3,
            ..Default::default()
        };
        let cache = QueryResultCache::new(config);

        // Insert 3 entries
        for i in 0..3 {
            let key = QueryKey::from_sql(&format!("SELECT * FROM test{}", i));
            cache
                .insert(key, create_test_result(), vec![format!("test{}", i)])
                .unwrap();
        }

        assert_eq!(cache.len(), 3);

        // Insert 4th entry - should evict oldest
        let key4 = QueryKey::from_sql("SELECT * FROM test4");
        cache
            .insert(
                key4.clone(),
                create_test_result(),
                vec!["test4".to_string()],
            )
            .unwrap();

        assert_eq!(cache.len(), 3);
        assert!(cache.contains(&key4));
    }

    #[test]
    fn test_cleanup_expired() {
        let config = QueryResultCacheConfig {
            default_ttl: Duration::from_millis(1),
            ..Default::default()
        };
        let cache = QueryResultCache::new(config);

        for i in 0..5 {
            let key = QueryKey::from_sql(&format!("SELECT * FROM test{}", i));
            cache
                .insert(key, create_test_result(), vec![format!("test{}", i)])
                .unwrap();
        }

        assert_eq!(cache.len(), 5);

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(10));

        let cleaned = cache.cleanup_expired();
        assert_eq!(cleaned, 5);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_multiple_dependencies() {
        let cache = QueryResultCache::with_defaults();

        // Insert with multiple dependencies
        let key = QueryKey::from_sql("SELECT * FROM a JOIN b JOIN c");
        cache
            .insert(
                key.clone(),
                create_test_result(),
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
            )
            .unwrap();

        assert!(cache.contains(&key));

        // Invalidating any dependency should remove the entry
        cache.invalidate_collection("b");
        assert!(!cache.contains(&key));
    }
}
