// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Catalog metadata cache — TTL-based in-memory LRU cache for xCatalog operations.
//!
//! Caches namespace, table schema, index, and statistics lookups to reduce
//! round-trips to the backing store. Shared by all catalog backend implementations
//! via `Arc<CatalogCache>`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tracing::{debug, trace};

use crate::{
    CatalogIndex, CatalogNamespace, CatalogTableSchema, CatalogTableStatistics, TableIdentifier,
};

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

/// Catalog metadata cache shared by all catalog backend implementations.
pub struct CatalogCache {
    max_entries: usize,
    ttl: Duration,
    namespaces: RwLock<HashMap<String, CacheEntry<CatalogNamespace>>>,
    tables: RwLock<HashMap<String, CacheEntry<CatalogTableSchema>>>,
    indexes: RwLock<HashMap<String, CacheEntry<Vec<CatalogIndex>>>>,
    statistics: RwLock<HashMap<String, CacheEntry<CatalogTableStatistics>>>,
    stats: RwLock<CacheStats>,
}

/// Cache hit/miss/eviction counters.
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
        let hits = self.namespace_hits + self.table_hits + self.index_hits + self.stats_hits;
        let misses =
            self.namespace_misses + self.table_misses + self.index_misses + self.stats_misses;
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }
}

impl CatalogCache {
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

    pub fn default_cache() -> Self {
        Self::new(10_000, 300)
    }

    // ---- Namespace ----

    pub fn get_namespace(&self, catalog: &str, namespace: &[String]) -> Option<CatalogNamespace> {
        let key = ns_key(catalog, namespace);
        let mut cache = self.namespaces.write();
        if let Some(e) = cache.get_mut(&key) {
            if e.is_expired(self.ttl) {
                cache.remove(&key);
                self.stats.write().namespace_misses += 1;
                trace!("namespace cache miss (expired): {key}");
                return None;
            }
            self.stats.write().namespace_hits += 1;
            trace!("namespace cache hit: {key}");
            Some(e.access().clone())
        } else {
            self.stats.write().namespace_misses += 1;
            None
        }
    }

    pub fn put_namespace(&self, catalog: &str, namespace: &[String], ns: CatalogNamespace) {
        let key = ns_key(catalog, namespace);
        let mut cache = self.namespaces.write();
        self.maybe_evict(&mut cache);
        cache.insert(key, CacheEntry::new(ns));
    }

    pub fn invalidate_namespace(&self, catalog: &str, namespace: &[String]) {
        let key = ns_key(catalog, namespace);
        self.namespaces.write().remove(&key);
        self.stats.write().invalidations += 1;
        debug!("invalidated namespace: {key}");
    }

    // ---- Table ----

    pub fn get_table(&self, catalog: &str, id: &TableIdentifier) -> Option<CatalogTableSchema> {
        let key = tbl_key(catalog, id);
        let mut cache = self.tables.write();
        if let Some(e) = cache.get_mut(&key) {
            if e.is_expired(self.ttl) {
                cache.remove(&key);
                self.stats.write().table_misses += 1;
                return None;
            }
            self.stats.write().table_hits += 1;
            Some(e.access().clone())
        } else {
            self.stats.write().table_misses += 1;
            None
        }
    }

    pub fn put_table(&self, catalog: &str, id: &TableIdentifier, schema: CatalogTableSchema) {
        let key = tbl_key(catalog, id);
        let mut cache = self.tables.write();
        self.maybe_evict(&mut cache);
        cache.insert(key, CacheEntry::new(schema));
    }

    pub fn invalidate_table_in_catalog(&self, catalog: &str, id: &TableIdentifier) {
        let key = tbl_key(catalog, id);
        self.tables.write().remove(&key);
        self.indexes.write().remove(&key);
        self.statistics.write().remove(&key);
        self.stats.write().invalidations += 1;
        debug!("invalidated table {id} in catalog {catalog}");
    }

    pub fn invalidate_table(&self, id: &TableIdentifier) {
        let pattern = format!(".{}", id.to_fqn());
        self.tables.write().retain(|k, _| !k.ends_with(&pattern));
        self.indexes.write().retain(|k, _| !k.ends_with(&pattern));
        self.statistics
            .write()
            .retain(|k, _| !k.ends_with(&pattern));
        self.stats.write().invalidations += 1;
    }

    // ---- Indexes ----

    pub fn get_indexes(&self, catalog: &str, id: &TableIdentifier) -> Option<Vec<CatalogIndex>> {
        let key = tbl_key(catalog, id);
        let mut cache = self.indexes.write();
        if let Some(e) = cache.get_mut(&key) {
            if e.is_expired(self.ttl) {
                cache.remove(&key);
                self.stats.write().index_misses += 1;
                return None;
            }
            self.stats.write().index_hits += 1;
            Some(e.access().clone())
        } else {
            self.stats.write().index_misses += 1;
            None
        }
    }

    pub fn put_indexes(&self, catalog: &str, id: &TableIdentifier, indexes: Vec<CatalogIndex>) {
        let key = tbl_key(catalog, id);
        let mut cache = self.indexes.write();
        self.maybe_evict(&mut cache);
        cache.insert(key, CacheEntry::new(indexes));
    }

    // ---- Statistics ----

    pub fn get_statistics(
        &self,
        catalog: &str,
        id: &TableIdentifier,
    ) -> Option<CatalogTableStatistics> {
        let key = tbl_key(catalog, id);
        let mut cache = self.statistics.write();
        if let Some(e) = cache.get_mut(&key) {
            if e.is_expired(self.ttl) {
                cache.remove(&key);
                self.stats.write().stats_misses += 1;
                return None;
            }
            self.stats.write().stats_hits += 1;
            Some(e.access().clone())
        } else {
            self.stats.write().stats_misses += 1;
            None
        }
    }

    pub fn put_statistics(
        &self,
        catalog: &str,
        id: &TableIdentifier,
        stats: CatalogTableStatistics,
    ) {
        let key = tbl_key(catalog, id);
        let mut cache = self.statistics.write();
        self.maybe_evict(&mut cache);
        cache.insert(key, CacheEntry::new(stats));
    }

    // ---- Maintenance ----

    pub fn clear(&self) {
        self.namespaces.write().clear();
        self.tables.write().clear();
        self.indexes.write().clear();
        self.statistics.write().clear();
        debug!("catalog cache cleared");
    }

    pub fn get_stats(&self) -> CacheStats {
        self.stats.read().clone()
    }

    pub fn size(&self) -> usize {
        self.namespaces.read().len()
            + self.tables.read().len()
            + self.indexes.read().len()
            + self.statistics.read().len()
    }

    pub fn evict_expired(&self) {
        let ttl = self.ttl;
        let mut evicted = 0usize;
        {
            let mut c = self.namespaces.write();
            let before = c.len();
            c.retain(|_, v| !v.is_expired(ttl));
            evicted += before - c.len();
        }
        {
            let mut c = self.tables.write();
            let before = c.len();
            c.retain(|_, v| !v.is_expired(ttl));
            evicted += before - c.len();
        }
        {
            let mut c = self.indexes.write();
            let before = c.len();
            c.retain(|_, v| !v.is_expired(ttl));
            evicted += before - c.len();
        }
        {
            let mut c = self.statistics.write();
            let before = c.len();
            c.retain(|_, v| !v.is_expired(ttl));
            evicted += before - c.len();
        }
        if evicted > 0 {
            self.stats.write().evictions += evicted as u64;
        }
    }

    fn maybe_evict<T>(&self, cache: &mut HashMap<String, CacheEntry<T>>) {
        if cache.len() >= self.max_entries {
            if let Some(k) = cache
                .iter()
                .min_by_key(|(_, v)| v.last_accessed)
                .map(|(k, _)| k.clone())
            {
                cache.remove(&k);
                self.stats.write().evictions += 1;
            }
        }
    }
}

fn ns_key(catalog: &str, namespace: &[String]) -> String {
    format!("{}.{}", catalog, namespace.join("."))
}

fn tbl_key(catalog: &str, id: &TableIdentifier) -> String {
    format!("{}.{}", catalog, id.to_fqn())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_table() {
        let cache = CatalogCache::new(100, 60);
        let id = TableIdentifier::new(vec!["db".into()], "users");
        let schema = CatalogTableSchema {
            name: "users".into(),
            ..Default::default()
        };
        cache.put_table("default", &id, schema);
        let result = cache.get_table("default", &id);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "users");
    }

    #[test]
    fn cache_miss() {
        let cache = CatalogCache::new(100, 60);
        let id = TableIdentifier::new(vec!["db".into()], "missing");
        assert!(cache.get_table("default", &id).is_none());
    }

    #[test]
    fn cache_stats_hit_miss() {
        let cache = CatalogCache::new(100, 60);
        let id = TableIdentifier::new(vec!["db".into()], "t");
        let _ = cache.get_table("default", &id); // miss
        cache.put_table("default", &id, CatalogTableSchema::default());
        let _ = cache.get_table("default", &id); // hit
        let s = cache.get_stats();
        assert_eq!(s.table_misses, 1);
        assert_eq!(s.table_hits, 1);
    }

    #[test]
    fn invalidate_clears_entry() {
        let cache = CatalogCache::new(100, 60);
        let id = TableIdentifier::new(vec!["db".into()], "t");
        cache.put_table("default", &id, CatalogTableSchema::default());
        assert!(cache.get_table("default", &id).is_some());
        cache.invalidate_table_in_catalog("default", &id);
        assert!(cache.get_table("default", &id).is_none());
    }
}
