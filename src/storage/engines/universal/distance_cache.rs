//! Distance Table Cache for PQ operations
//!
//! This module provides caching for pre-computed distance tables used in
//! Product Quantization operations.

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, trace};

use super::config::CacheConfig;
use crate::compute::distance_computation::DistanceMetric;
use crate::storage::cache::orchestrator::{CrossCacheOrchestrator, CacheType};

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedDistanceTable {
    /// Pre-computed distances
    pub distances: Vec<Vec<f32>>,
    /// Access count
    pub access_count: u64,
}

/// Distance table cache for PQ operations using unified cache infrastructure
#[derive(Debug)]
pub struct DistanceTableCache {
    /// Unified cache orchestrator
    cache_orchestrator: Arc<CrossCacheOrchestrator>,
    /// Cache configuration
    config: CacheConfig,
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
    /// Create new distance table cache with unified cache orchestrator
    pub async fn new(config: &CacheConfig, cache_orchestrator: Arc<CrossCacheOrchestrator>) -> Result<Self> {
        Ok(Self {
            cache_orchestrator,
            config: config.clone(),
        })
    }

    /// Create cache key from DistanceTableKey
    fn create_cache_key(&self, key: &DistanceTableKey) -> String {
        format!("dist_{}_{}_{}_{:?}", key.query_hash, key.segments, key.bits, key.metric)
    }

    /// Get or compute distance table using unified cache
    pub async fn get_or_compute<F>(
        &self,
        key: DistanceTableKey,
        compute_fn: F,
    ) -> Result<Vec<Vec<f32>>>
    where
        F: FnOnce() -> Result<Vec<Vec<f32>>>,
    {
        let cache_key = self.create_cache_key(&key);
        
        // Check unified cache first
        if let Ok(Some(cached_data)) = self.cache_orchestrator.get(&CacheType::DistanceTable, &cache_key) {
            if let Ok(cached_table) = serde_json::from_slice::<CachedDistanceTable>(&cached_data) {
                trace!("Cache hit for distance table");
                
                // Update access count
                let updated_table = CachedDistanceTable {
                    distances: cached_table.distances.clone(),
                    access_count: cached_table.access_count + 1,
                };
                
                if let Ok(updated_data) = serde_json::to_vec(&updated_table) {
                    let _ = self.cache_orchestrator.put(CacheType::DistanceTable, cache_key, updated_data, None);
                }
                
                return Ok(cached_table.distances);
            }
        }

        // Cache miss - compute new table
        debug!("Cache miss - computing distance table");
        let distances = compute_fn()?;

        // Store in unified cache
        let cached_table = CachedDistanceTable {
            distances: distances.clone(),
            access_count: 1,
        };

        if let Ok(cached_data) = serde_json::to_vec(&cached_table) {
            let _ = self.cache_orchestrator.put(CacheType::DistanceTable, cache_key, cached_data, None);
        }

        Ok(distances)
    }


    /// Get cache statistics from unified cache orchestrator
    pub async fn get_statistics(&self) -> CacheStats {
        // Get statistics from unified cache orchestrator
        if let Ok(metrics) = self.cache_orchestrator.get_metrics().await {
            // Convert unified cache metrics to our local format
            CacheStats {
                hits: metrics.total_hits,
                misses: metrics.total_misses,
                evictions: metrics.total_evictions,
                total_requests: metrics.total_hits + metrics.total_misses,
                size_mb: (metrics.total_memory_bytes / (1024 * 1024)) as usize,
                hit_rate_percent: if metrics.total_hits + metrics.total_misses > 0 {
                    (metrics.total_hits as f32 / (metrics.total_hits + metrics.total_misses) as f32) * 100.0
                } else {
                    0.0
                },
            }
        } else {
            CacheStats::default()
        }
    }

    /// Clear distance tables from unified cache
    pub async fn clear(&self) {
        // Note: Unified cache orchestrator handles eviction policies
        // Individual cache clearing is managed by the orchestrator
        debug!("Distance table cache clear requested - managed by unified orchestrator");
    }
}
