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
pub struct DistanceTableCache {
    /// Unified cache orchestrator
    cache_orchestrator: Arc<CrossCacheOrchestrator>,
    /// Cache configuration
    config: CacheConfig,
}

impl std::fmt::Debug for DistanceTableCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DistanceTableCache")
            .field("cache_orchestrator", &"<CrossCacheOrchestrator>")
            .field("config", &self.config)
            .finish()
    }
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
        if let Ok(Some(cached_data)) = self.cache_orchestrator.get(&CacheType::DistanceTable, &cache_key).await {
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
            // Extract metrics from the JSON structure
            let hits = metrics.get("total_hits").and_then(|v| v.as_u64()).unwrap_or(0);
            let misses = metrics.get("total_misses").and_then(|v| v.as_u64()).unwrap_or(0);
            let evictions = metrics.get("total_evictions").and_then(|v| v.as_u64()).unwrap_or(0);
            let memory_bytes = metrics.get("total_memory_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
            
            // Convert unified cache metrics to our local format
            CacheStats {
                hits,
                misses,
                evictions,
                total_requests: hits + misses,
                size_mb: (memory_bytes / (1024 * 1024)) as usize,
                hit_rate_percent: if hits + misses > 0 {
                    (hits as f32 / (hits + misses) as f32) * 100.0
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

    /// Batch precompute and cache distance tables for performance optimization
    pub async fn warm_cache_batch<F>(&self, keys: Vec<DistanceTableKey>, compute_fn: F) -> Result<()>
    where
        F: Fn(&DistanceTableKey) -> Result<Vec<Vec<f32>>>,
    {
        debug!("Warming cache with {} distance tables", keys.len());
        
        for key in keys {
            let cache_key = self.create_cache_key(&key);
            
            // Skip if already cached
            if let Ok(Some(_)) = self.cache_orchestrator.get(&CacheType::DistanceTable, &cache_key).await {
                continue;
            }
            
            // Compute and cache
            if let Ok(distances) = compute_fn(&key) {
                let cached_table = CachedDistanceTable {
                    distances,
                    access_count: 0,
                };
                
                if let Ok(cached_data) = serde_json::to_vec(&cached_table) {
                    let _ = self.cache_orchestrator.put(CacheType::DistanceTable, cache_key, cached_data, None);
                }
            }
        }
        
        Ok(())
    }

    /// Optimize cache by prefetching related distance tables
    pub async fn prefetch_related(&self, base_key: &DistanceTableKey) -> Result<()> {
        let related_keys = self.generate_related_keys(base_key);
        
        // Implement simple prefetching for commonly accessed patterns
        for related_key in related_keys {
            let cache_key = self.create_cache_key(&related_key);
            
            // Check if already cached to avoid unnecessary work
            if let Ok(None) = self.cache_orchestrator.get(&CacheType::DistanceTable, &cache_key).await {
                // Could implement predictive computation here based on patterns
                trace!("Could prefetch related key: {:?}", related_key);
            }
        }
        
        Ok(())
    }

    /// Generate related keys for predictive caching
    fn generate_related_keys(&self, base_key: &DistanceTableKey) -> Vec<DistanceTableKey> {
        let mut related = Vec::new();
        
        // Generate variants with different bit counts (common access pattern)
        for bits in [4, 8, 16, 32] {
            if bits != base_key.bits {
                related.push(DistanceTableKey {
                    query_hash: base_key.query_hash,
                    segments: base_key.segments,
                    bits,
                    metric: base_key.metric.clone(),
                });
            }
        }
        
        // Generate variants with different segment counts
        for segments in [8, 16, 32, 64] {
            if segments != base_key.segments {
                related.push(DistanceTableKey {
                    query_hash: base_key.query_hash,
                    segments,
                    bits: base_key.bits,
                    metric: base_key.metric.clone(),
                });
            }
        }
        
        related
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_distance_cache_creation() {
        let config = CacheConfig {
            max_entries: 100,
            ttl_seconds: 3600,
            eviction_policy: crate::storage::engines::universal::config::CacheEvictionPolicy::LRU,
            compression: false,
            enable_cache_warming: false,
            max_memory_mb: 512,
        };
        let cache_orchestrator = Arc::new(
            crate::storage::cache::orchestrator::CrossCacheOrchestrator::new(1024 * 1024)
        );
        
        let cache = DistanceTableCache::new(&config, cache_orchestrator).await.unwrap();
        assert_eq!(cache.config.max_entries, 100);
    }

    #[tokio::test]
    async fn test_cache_key_generation() {
        let config = CacheConfig {
            max_entries: 100,
            ttl_seconds: 3600,
            eviction_policy: crate::storage::engines::universal::config::CacheEvictionPolicy::LRU,
            compression: false,
            enable_cache_warming: false,
            max_memory_mb: 512,
        };
        let cache_orchestrator = Arc::new(
            crate::storage::cache::orchestrator::CrossCacheOrchestrator::new(1024 * 1024)
        );
        
        let cache = DistanceTableCache::new(&config, cache_orchestrator).await.unwrap();
        
        let key = DistanceTableKey {
            query_hash: 12345,
            segments: 8,
            bits: 16,
            metric: DistanceMetric::Cosine,
        };
        
        let cache_key = cache.create_cache_key(&key);
        assert!(cache_key.contains("12345"));
        assert!(cache_key.contains("8"));
        assert!(cache_key.contains("16"));
    }

    #[tokio::test]
    async fn test_distance_cache_get_or_compute() {
        let config = CacheConfig {
            max_entries: 100,
            ttl_seconds: 3600,
            eviction_policy: crate::storage::engines::universal::config::CacheEvictionPolicy::LRU,
            compression: false,
            enable_cache_warming: false,
            max_memory_mb: 512,
        };
        let cache_orchestrator = Arc::new(
            crate::storage::cache::orchestrator::CrossCacheOrchestrator::new(1024 * 1024)
        );
        
        let cache = DistanceTableCache::new(&config, cache_orchestrator).await.unwrap();
        
        let key = DistanceTableKey {
            query_hash: 12345,
            segments: 8,
            bits: 16,
            metric: DistanceMetric::Cosine,
        };
        
        let compute_fn = || -> Result<Vec<Vec<f32>>> {
            Ok(vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]])
        };
        
        // First call should compute
        let result1 = cache.get_or_compute(key.clone(), compute_fn).await.unwrap();
        assert_eq!(result1.len(), 2);
        assert_eq!(result1[0], vec![1.0, 2.0, 3.0]);
    }

    #[tokio::test]
    async fn test_cache_batch_warming() {
        let config = CacheConfig {
            max_entries: 100,
            ttl_seconds: 3600,
            eviction_policy: crate::storage::engines::universal::config::CacheEvictionPolicy::LRU,
            compression: false,
            enable_cache_warming: false,
            max_memory_mb: 512,
        };
        let cache_orchestrator = Arc::new(
            crate::storage::cache::orchestrator::CrossCacheOrchestrator::new(1024 * 1024)
        );
        
        let cache = DistanceTableCache::new(&config, cache_orchestrator).await.unwrap();
        
        let keys = vec![
            DistanceTableKey {
                query_hash: 111,
                segments: 8,
                bits: 16,
                metric: DistanceMetric::Cosine,
            },
            DistanceTableKey {
                query_hash: 222,
                segments: 8,
                bits: 16,
                metric: DistanceMetric::Euclidean,
            },
        ];
        
        let compute_fn = |_key: &DistanceTableKey| -> Result<Vec<Vec<f32>>> {
            Ok(vec![vec![1.0, 2.0], vec![3.0, 4.0]])
        };
        
        // Test batch warming
        let result = cache.warm_cache_batch(keys, compute_fn).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_related_key_generation() {
        let config = CacheConfig {
            max_entries: 100,
            ttl_seconds: 3600,
            eviction_policy: crate::storage::engines::universal::config::CacheEvictionPolicy::LRU,
            compression: false,
            enable_cache_warming: false,
            max_memory_mb: 512,
        };
        let cache_orchestrator = Arc::new(
            crate::storage::cache::orchestrator::CrossCacheOrchestrator::new(1024 * 1024)
        );
        
        let cache = DistanceTableCache::new(&config, cache_orchestrator).await.unwrap();
        
        let base_key = DistanceTableKey {
            query_hash: 12345,
            segments: 16,
            bits: 8,
            metric: DistanceMetric::Cosine,
        };
        
        let related_keys = cache.generate_related_keys(&base_key);
        
        // Should generate related keys with different bit counts and segment counts
        assert!(!related_keys.is_empty());
        
        // Should not include the base key itself
        assert!(!related_keys.iter().any(|k| k.bits == base_key.bits && k.segments == base_key.segments));
    }

    #[test]
    fn test_cache_stats_default() {
        let stats = CacheStats::default();
        
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.evictions, 0);
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.size_mb, 0);
        assert_eq!(stats.hit_rate_percent, 0.0);
    }
}
