//! Query Cache for Parquet Reads
//!
//! This module provides caching strategies for Parquet queries
//! to improve performance for repeated reads.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use proximadb_records::ProximaRecord;

/// Cache strategy for queries
#[derive(Debug, Clone, Copy)]
pub enum CacheStrategy {
    /// No caching
    None,
    /// LRU cache with fixed size
    LRU,
    /// LFU cache (Least Frequently Used)
    LFU,
    /// Adaptive cache that switches strategies
    Adaptive,
}

/// Query cache for storing results
pub struct QueryCache {
    strategy: CacheStrategy,
    cache: Arc<RwLock<HashMap<String, Vec<ProximaRecord>>>>,
    capacity: usize,
    stats: ColumnarResultCacheStats,
}

impl QueryCache {
    /// Create new cache with strategy
    pub fn new(strategy: CacheStrategy, capacity: usize) -> Self {
        Self {
            strategy,
            cache: Arc::new(RwLock::new(HashMap::new())),
            capacity,
            stats: ColumnarResultCacheStats::default(),
        }
    }

    /// Get cached results
    pub fn get(&mut self, key: &str) -> Option<Vec<ProximaRecord>> {
        match self.strategy {
            CacheStrategy::None => None,
            CacheStrategy::LRU => {
                let cache = self
                    .cache
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let result = cache.get(key).cloned();
                if result.is_some() {
                    self.stats.hits += 1;
                } else {
                    self.stats.misses += 1;
                }
                result
            }
            _ => None, // Other strategies not implemented yet
        }
    }

    /// Put results in cache
    pub fn put(&mut self, key: String, records: Vec<ProximaRecord>) {
        match self.strategy {
            CacheStrategy::None => {}
            CacheStrategy::LRU => {
                let mut cache = self
                    .cache
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());

                // Simple eviction: if cache is full, clear some entries
                if cache.len() >= self.capacity {
                    // Remove oldest entries (simple eviction, not true LRU)
                    let keys_to_remove: Vec<String> =
                        cache.keys().take(cache.len() / 2).cloned().collect();
                    for k in keys_to_remove {
                        cache.remove(&k);
                    }
                }

                cache.insert(key, records);
                self.stats.insertions += 1;
            }
            _ => {} // Other strategies not implemented yet
        }
    }

    /// Clear cache
    pub fn clear(&mut self) {
        let mut cache = self
            .cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let len = cache.len();
        cache.clear();
        self.stats.evictions += len;
    }

    /// Get cache statistics
    pub fn stats(&self) -> &ColumnarResultCacheStats {
        &self.stats
    }
}

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct ColumnarResultCacheStats {
    pub hits: usize,
    pub misses: usize,
    pub insertions: usize,
    pub evictions: usize,
}

impl ColumnarResultCacheStats {
    /// Get hit rate
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}
