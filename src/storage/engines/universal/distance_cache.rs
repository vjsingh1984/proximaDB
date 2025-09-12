//! Distance Table Cache for PQ operations
//!
//! This module provides caching for pre-computed distance tables used in
//! Product Quantization operations.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, trace};

use super::config::CacheConfig;
use crate::compute::distance_computation::DistanceMetric;

/// Cache key for distance tables
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct DistanceTableKey {
    /// Query vector hash
    pub query_hash: u64,
    /// Number of segments
    pub segments: usize,
    /// Number of bits
    pub bits: usize,
    /// Distance metric
    pub metric: DistanceMetric,
}

/// Cached distance table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedDistanceTable {
    /// Pre-computed distances
    pub distances: Vec<Vec<f32>>,
    /// Cache timestamp
    pub timestamp: std::time::Instant,
    /// Access count
    pub access_count: u64,
}

/// Distance table cache for PQ operations
#[derive(Debug)]
pub struct DistanceTableCache {
    /// Cache storage
    cache: Arc<RwLock<HashMap<DistanceTableKey, CachedDistanceTable>>>,
    /// Cache configuration
    config: CacheConfig,
    /// Cache statistics
    stats: Arc<RwLock<CacheStats>>,
}

/// Cache statistics
#[derive(Debug, Default, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub total_requests: u64,
    pub size_mb: usize,
    pub hit_rate_percent: f32,
}

impl DistanceTableCache {
    /// Create new distance table cache
    pub async fn new(config: &CacheConfig) -> Result<Self> {
        Ok(Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            config: config.clone(),
            stats: Arc::new(RwLock::new(CacheStats::default())),
        })
    }

    /// Get or compute distance table
    pub async fn get_or_compute<F>(
        &self,
        key: DistanceTableKey,
        compute_fn: F,
    ) -> Result<Vec<Vec<f32>>>
    where
        F: FnOnce() -> Result<Vec<Vec<f32>>>,
    {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(table) = cache.get(&key) {
                let mut stats = self.stats.write().await;
                stats.hits += 1;
                stats.total_requests += 1;

                trace!("Cache hit for distance table");
                return Ok(table.distances.clone());
            }
        }

        // Cache miss - compute new table
        let mut stats = self.stats.write().await;
        stats.misses += 1;
        stats.total_requests += 1;
        drop(stats);

        debug!("Cache miss - computing distance table");
        let distances = compute_fn()?;

        // Store in cache
        let cached_table = CachedDistanceTable {
            distances: distances.clone(),
            timestamp: std::time::Instant::now(),
            access_count: 1,
        };

        let mut cache = self.cache.write().await;

        // Check cache size and evict if needed
        if cache.len() >= self.config.max_entries {
            self.evict_lru(&mut cache).await;
        }

        cache.insert(key, cached_table);

        Ok(distances)
    }

    /// Evict least recently used entry
    async fn evict_lru(&self, cache: &mut HashMap<DistanceTableKey, CachedDistanceTable>) {
        if let Some((key, _)) = cache
            .iter()
            .min_by_key(|(_, table)| table.timestamp)
            .map(|(k, v)| (k.clone(), v.clone()))
        {
            cache.remove(&key);
            let mut stats = self.stats.write().await;
            stats.evictions += 1;
        }
    }

    /// Get cache statistics
    pub async fn get_statistics(&self) -> CacheStats {
        let stats = self.stats.read().await;
        let mut result = (*stats).clone();

        if result.total_requests > 0 {
            result.hit_rate_percent = (result.hits as f32 / result.total_requests as f32) * 100.0;
        }

        // Estimate size
        let cache = self.cache.read().await;
        let entry_size = std::mem::size_of::<DistanceTableKey>()
            + std::mem::size_of::<CachedDistanceTable>()
            + 1024 * 4; // Estimate for distance data
        result.size_mb = (cache.len() * entry_size) / (1024 * 1024);

        result
    }

    /// Clear cache
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();

        let mut stats = self.stats.write().await;
        stats.evictions += cache.len() as u64;
    }
}
