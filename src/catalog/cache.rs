//! Catalog Cache
//!
//! High-performance metadata caching for catalog operations.
//! Designed for distributed, serverless environments with TTL-based invalidation.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tracing::{debug, trace};

use super::types::{CatalogIndex, CatalogNamespace, CatalogTableSchema, CatalogTableStatistics};
use super::TableIdentifier;

/// Cache entry with TTL tracking
#[derive(Debug, Clone)]
struct CacheEntry<T> {
    value: T,
    created_at: Instant,
    last_accessed: Instant,
    access_count: u64,
}

impl<T: Clone> CacheEntry<T> {
    fn new(value: T) -> Self {
        let now = Instant::now();
        Self {
            value,
            created_at: now,
            last_accessed: now,
            access_count: 1,
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.created_at.elapsed() > ttl
    }

    fn access(&mut self) -> &T {
        self.last_accessed = Instant::now();
        self.access_count += 1;
        &self.value
    }
}

/// Catalog metadata cache
pub struct CatalogCache {
    /// Maximum number of entries
    max_entries: usize,
    /// Time-to-live in seconds
    ttl: Duration,
    /// Namespace cache: namespace_path -> CatalogNamespace
    namespaces: RwLock<HashMap<String, CacheEntry<CatalogNamespace>>>,
    /// Table schema cache: catalog.namespace.table -> CatalogTableSchema
    tables: RwLock<HashMap<String, CacheEntry<CatalogTableSchema>>>,
    /// Index cache: catalog.namespace.table -> Vec<CatalogIndex>
    indexes: RwLock<HashMap<String, CacheEntry<Vec<CatalogIndex>>>>,
    /// Statistics cache: catalog.namespace.table -> CatalogTableStatistics
    statistics: RwLock<HashMap<String, CacheEntry<CatalogTableStatistics>>>,
    /// Cache statistics
    stats: RwLock<CacheStats>,
}

/// Cache performance statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub namespace_hits: u64,
    pub namespace_misses: u64,
    pub table_hits: u64,
    pub table_misses: u64,
    pub index_hits: u64,
    pub index_misses: u64,
    pub stats_hits: u64,
    pub stats_misses: u64,
    pub evictions: u64,
    pub invalidations: u64,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total_hits = self.namespace_hits + self.table_hits + self.index_hits + self.stats_hits;
        let total_misses = self.namespace_misses + self.table_misses + self.index_misses + self.stats_misses;
        let total = total_hits + total_misses;
        if total == 0 {
            0.0
        } else {
            total_hits as f64 / total as f64
        }
    }
}

impl CatalogCache {
    /// Create a new catalog cache
    pub fn new(max_entries: usize, ttl_seconds: u64) -> Self {
        Self {
            max_entries,
            ttl: Duration::from_secs(ttl_seconds),
            namespaces: RwLock::new(HashMap::new()),
            tables: RwLock::new(HashMap::new()),
            indexes: RwLock::new(HashMap::new()),
            statistics: RwLock::new(HashMap::new()),
            stats: RwLock::new(CacheStats::default()),
        }
    }

    /// Create a cache with default settings (10K entries, 5 min TTL)
    pub fn default_cache() -> Self {
        Self::new(10000, 300)
    }

    // ========================
    // Namespace Cache
    // ========================

    /// Get a namespace from cache
    pub fn get_namespace(&self, catalog: &str, namespace: &[String]) -> Option<CatalogNamespace> {
        let key = format_namespace_key(catalog, namespace);
        let mut cache = self.namespaces.write();

        if let Some(entry) = cache.get_mut(&key) {
            if entry.is_expired(self.ttl) {
                cache.remove(&key);
                self.stats.write().namespace_misses += 1;
                trace!("Namespace cache miss (expired): {}", key);
                return None;
            }
            self.stats.write().namespace_hits += 1;
            trace!("Namespace cache hit: {}", key);
            Some(entry.access().clone())
        } else {
            self.stats.write().namespace_misses += 1;
            trace!("Namespace cache miss: {}", key);
            None
        }
    }

    /// Put a namespace in cache
    pub fn put_namespace(&self, catalog: &str, namespace: &[String], ns: CatalogNamespace) {
        let key = format_namespace_key(catalog, namespace);
        let mut cache = self.namespaces.write();

        self.maybe_evict(&mut cache);
        cache.insert(key, CacheEntry::new(ns));
    }

    /// Invalidate a namespace
    pub async fn invalidate_namespace(&self, catalog: &str, namespace: &[String]) {
        let key = format_namespace_key(catalog, namespace);
        self.namespaces.write().remove(&key);
        self.stats.write().invalidations += 1;
        debug!("Invalidated namespace: {}", key);
    }

    // ========================
    // Table Cache
    // ========================

    /// Get a table schema from cache
    pub fn get_table(&self, catalog: &str, identifier: &TableIdentifier) -> Option<CatalogTableSchema> {
        let key = format_table_key(catalog, identifier);
        let mut cache = self.tables.write();

        if let Some(entry) = cache.get_mut(&key) {
            if entry.is_expired(self.ttl) {
                cache.remove(&key);
                self.stats.write().table_misses += 1;
                trace!("Table cache miss (expired): {}", key);
                return None;
            }
            self.stats.write().table_hits += 1;
            trace!("Table cache hit: {}", key);
            Some(entry.access().clone())
        } else {
            self.stats.write().table_misses += 1;
            trace!("Table cache miss: {}", key);
            None
        }
    }

    /// Put a table schema in cache
    pub fn put_table(&self, catalog: &str, identifier: &TableIdentifier, schema: CatalogTableSchema) {
        let key = format_table_key(catalog, identifier);
        let mut cache = self.tables.write();

        self.maybe_evict(&mut cache);
        cache.insert(key, CacheEntry::new(schema));
    }

    /// Invalidate a table and its related entries
    pub async fn invalidate_table(&self, identifier: &TableIdentifier) {
        // Invalidate from all catalogs (when catalog is unknown)
        let pattern = format!(".{}", identifier.to_fqn());

        // Invalidate table cache
        {
            let mut cache = self.tables.write();
            cache.retain(|k, _| !k.ends_with(&pattern));
        }

        // Invalidate index cache
        {
            let mut cache = self.indexes.write();
            cache.retain(|k, _| !k.ends_with(&pattern));
        }

        // Invalidate statistics cache
        {
            let mut cache = self.statistics.write();
            cache.retain(|k, _| !k.ends_with(&pattern));
        }

        self.stats.write().invalidations += 1;
        debug!("Invalidated table: {}", identifier);
    }

    /// Invalidate a table for a specific catalog
    pub async fn invalidate_table_in_catalog(&self, catalog: &str, identifier: &TableIdentifier) {
        let key = format_table_key(catalog, identifier);

        self.tables.write().remove(&key);
        self.indexes.write().remove(&key);
        self.statistics.write().remove(&key);

        self.stats.write().invalidations += 1;
        debug!("Invalidated table in catalog {}: {}", catalog, identifier);
    }

    // ========================
    // Index Cache
    // ========================

    /// Get indexes from cache
    pub fn get_indexes(&self, catalog: &str, identifier: &TableIdentifier) -> Option<Vec<CatalogIndex>> {
        let key = format_table_key(catalog, identifier);
        let mut cache = self.indexes.write();

        if let Some(entry) = cache.get_mut(&key) {
            if entry.is_expired(self.ttl) {
                cache.remove(&key);
                self.stats.write().index_misses += 1;
                return None;
            }
            self.stats.write().index_hits += 1;
            Some(entry.access().clone())
        } else {
            self.stats.write().index_misses += 1;
            None
        }
    }

    /// Put indexes in cache
    pub fn put_indexes(&self, catalog: &str, identifier: &TableIdentifier, indexes: Vec<CatalogIndex>) {
        let key = format_table_key(catalog, identifier);
        let mut cache = self.indexes.write();

        self.maybe_evict(&mut cache);
        cache.insert(key, CacheEntry::new(indexes));
    }

    // ========================
    // Statistics Cache
    // ========================

    /// Get statistics from cache
    pub fn get_statistics(&self, catalog: &str, identifier: &TableIdentifier) -> Option<CatalogTableStatistics> {
        let key = format_table_key(catalog, identifier);
        let mut cache = self.statistics.write();

        if let Some(entry) = cache.get_mut(&key) {
            if entry.is_expired(self.ttl) {
                cache.remove(&key);
                self.stats.write().stats_misses += 1;
                return None;
            }
            self.stats.write().stats_hits += 1;
            Some(entry.access().clone())
        } else {
            self.stats.write().stats_misses += 1;
            None
        }
    }

    /// Put statistics in cache
    pub fn put_statistics(&self, catalog: &str, identifier: &TableIdentifier, stats: CatalogTableStatistics) {
        let key = format_table_key(catalog, identifier);
        let mut cache = self.statistics.write();

        self.maybe_evict(&mut cache);
        cache.insert(key, CacheEntry::new(stats));
    }

    // ========================
    // Cache Management
    // ========================

    /// Clear all caches
    pub fn clear(&self) {
        self.namespaces.write().clear();
        self.tables.write().clear();
        self.indexes.write().clear();
        self.statistics.write().clear();
        debug!("Catalog cache cleared");
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> CacheStats {
        self.stats.read().clone()
    }

    /// Get current cache size
    pub fn size(&self) -> usize {
        self.namespaces.read().len()
            + self.tables.read().len()
            + self.indexes.read().len()
            + self.statistics.read().len()
    }

    /// Evict expired entries
    pub fn evict_expired(&self) {
        let ttl = self.ttl;

        let mut evicted = 0;

        {
            let mut cache = self.namespaces.write();
            let before = cache.len();
            cache.retain(|_, v| !v.is_expired(ttl));
            evicted += before - cache.len();
        }

        {
            let mut cache = self.tables.write();
            let before = cache.len();
            cache.retain(|_, v| !v.is_expired(ttl));
            evicted += before - cache.len();
        }

        {
            let mut cache = self.indexes.write();
            let before = cache.len();
            cache.retain(|_, v| !v.is_expired(ttl));
            evicted += before - cache.len();
        }

        {
            let mut cache = self.statistics.write();
            let before = cache.len();
            cache.retain(|_, v| !v.is_expired(ttl));
            evicted += before - cache.len();
        }

        if evicted > 0 {
            self.stats.write().evictions += evicted as u64;
            debug!("Evicted {} expired cache entries", evicted);
        }
    }

    /// Maybe evict entries if over capacity
    fn maybe_evict<T>(&self, cache: &mut HashMap<String, CacheEntry<T>>) {
        if cache.len() >= self.max_entries {
            // Simple LRU: remove oldest accessed entry
            if let Some(oldest_key) = cache
                .iter()
                .min_by_key(|(_, v)| v.last_accessed)
                .map(|(k, _)| k.clone())
            {
                cache.remove(&oldest_key);
                self.stats.write().evictions += 1;
            }
        }
    }
}

fn format_namespace_key(catalog: &str, namespace: &[String]) -> String {
    format!("{}.{}", catalog, namespace.join("."))
}

fn format_table_key(catalog: &str, identifier: &TableIdentifier) -> String {
    format!("{}.{}", catalog, identifier.to_fqn())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_new() {
        let cache = CatalogCache::new(100, 60);
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_cache_put_get_table() {
        let cache = CatalogCache::new(100, 60);
        let identifier = TableIdentifier::new(vec!["db".to_string()], "users".to_string());

        let schema = CatalogTableSchema {
            name: "users".to_string(),
            ..Default::default()
        };

        cache.put_table("default", &identifier, schema.clone());

        let result = cache.get_table("default", &identifier);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "users");
    }

    #[test]
    fn test_cache_miss() {
        let cache = CatalogCache::new(100, 60);
        let identifier = TableIdentifier::new(vec!["db".to_string()], "nonexistent".to_string());

        let result = cache.get_table("default", &identifier);
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_stats() {
        let cache = CatalogCache::new(100, 60);
        let identifier = TableIdentifier::new(vec!["db".to_string()], "test".to_string());

        // Miss
        let _ = cache.get_table("default", &identifier);

        // Put
        cache.put_table("default", &identifier, CatalogTableSchema::default());

        // Hit
        let _ = cache.get_table("default", &identifier);

        let stats = cache.get_stats();
        assert_eq!(stats.table_misses, 1);
        assert_eq!(stats.table_hits, 1);
    }

    #[tokio::test]
    async fn test_cache_invalidation() {
        let cache = CatalogCache::new(100, 60);
        let identifier = TableIdentifier::new(vec!["db".to_string()], "test".to_string());

        cache.put_table("default", &identifier, CatalogTableSchema::default());
        assert!(cache.get_table("default", &identifier).is_some());

        cache.invalidate_table_in_catalog("default", &identifier).await;
        assert!(cache.get_table("default", &identifier).is_none());
    }

    #[test]
    fn test_cache_clear() {
        let cache = CatalogCache::new(100, 60);
        let id1 = TableIdentifier::new(vec!["db".to_string()], "t1".to_string());
        let id2 = TableIdentifier::new(vec!["db".to_string()], "t2".to_string());

        cache.put_table("default", &id1, CatalogTableSchema::default());
        cache.put_table("default", &id2, CatalogTableSchema::default());

        assert_eq!(cache.size(), 2);

        cache.clear();
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_cache_hit_rate() {
        let mut stats = CacheStats::default();
        assert_eq!(stats.hit_rate(), 0.0);

        stats.table_hits = 3;
        stats.table_misses = 1;
        assert!((stats.hit_rate() - 0.75).abs() < 0.001);
    }
}
