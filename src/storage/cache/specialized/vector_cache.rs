//! # Vector Cache Implementation
//!
//! Dedicated cache for individual vector storage, separate from query result caching.
//! This fixes the type mismatch issue where storage engines were incorrectly using
//! QueryCache for individual vectors.

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::SystemTime;

/// Simple string key for vector caching
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct VectorCacheKey(pub String);

impl CacheKey for VectorCacheKey {}

impl From<String> for VectorCacheKey {
    fn from(s: String) -> Self {
        VectorCacheKey(s)
    }
}

impl From<&str> for VectorCacheKey {
    fn from(s: &str) -> Self {
        VectorCacheKey(s.to_string())
    }
}

/// Cached vector with metadata
#[derive(Debug, Clone)]
pub struct CachedVector {
    pub vector: VectorRecord,
    pub cached_at: SystemTime,
    pub access_count: u64,
}

impl CacheValue for CachedVector {
    fn size_bytes(&self) -> usize {
        // Calculate actual size
        let mut size = std::mem::size_of::<Self>();

        // Add vector data size
        size += self.vector.id.len();
        size += self.vector.vector.len() * std::mem::size_of::<f32>();

        // Add metadata size
        for (key, _value) in &self.vector.metadata {
            size += key.len() + 64; // Estimate 64 bytes per SqlValue
        }

        size
    }
}

impl CachedVector {
    fn is_expired(&self, ttl: std::time::Duration) -> bool {
        SystemTime::now()
            .duration_since(self.cached_at)
            .map(|age| age > ttl)
            .unwrap_or(false)
    }
}

/// Dedicated cache for individual vectors
pub struct VectorCache {
    base: BaseCacheImpl<VectorCacheKey, CachedVector>,
    ttl_seconds: u64,
}

impl VectorCache {
    /// Create new vector cache with specified memory limit in MB
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            base: BaseCacheImpl::new(max_memory_mb),
            ttl_seconds: 3600, // 1 hour default TTL
        }
    }

    /// Create with custom TTL
    pub fn with_ttl(max_memory_mb: usize, ttl_seconds: u64) -> Self {
        Self {
            base: BaseCacheImpl::new(max_memory_mb),
            ttl_seconds,
        }
    }

    /// Get vector by string key (used by storage engines)
    pub async fn get(&self, key: &str) -> Option<VectorRecord> {
        let cache_key = VectorCacheKey::from(key);

        if let Some(mut cached) = self.base.get_with_hooks(&cache_key).await {
            // Check TTL
            if cached.is_expired(std::time::Duration::from_secs(self.ttl_seconds)) {
                // Remove expired entry
                self.base.invalidate(&cache_key).await;
                return None;
            }

            // Update access count
            cached.access_count += 1;

            // Re-insert to update access count
            let vector = cached.vector.clone();
            self.base.put_with_hooks(cache_key, cached).await;

            Some(vector)
        } else {
            None
        }
    }

    /// Put vector with string key
    pub async fn put(&self, key: String, vector: VectorRecord) -> anyhow::Result<()> {
        let cache_key = VectorCacheKey::from(key);
        let cached = CachedVector {
            vector,
            cached_at: SystemTime::now(),
            access_count: 0,
        };

        self.base.put_with_hooks(cache_key, cached).await;
        Ok(())
    }

    /// Remove vector from cache
    pub async fn remove(&self, key: &str) -> bool {
        let cache_key = VectorCacheKey::from(key);
        self.base.invalidate(&cache_key).await
    }

    /// Clear all cached vectors
    pub async fn clear(&self) {
        // Clear is not directly available, would need to be implemented
        // For now, we can't clear all entries at once through BaseCache
    }

    /// Get cache statistics
    pub async fn statistics(&self) -> CacheStatistics {
        CacheStatistics {
            total_items: 0,  // Size not directly available through BaseCache
            memory_usage_bytes: 0,  // Memory usage not directly available
            hit_count: 0, // TODO: Track hits
            miss_count: 0, // TODO: Track misses
        }
    }

    /// Get cache size in items
    pub async fn size(&self) -> usize {
        0  // Size not directly available through BaseCache
    }

    /// Get memory usage in bytes
    pub async fn memory_usage(&self) -> usize {
        0  // Memory usage not directly available through BaseCache
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStatistics {
    pub total_items: usize,
    pub memory_usage_bytes: usize,
    pub hit_count: u64,
    pub miss_count: u64,
}

impl CacheStatistics {
    /// Calculate hit rate
    pub fn hit_rate(&self) -> f64 {
        let total = self.hit_count + self.miss_count;
        if total > 0 {
            self.hit_count as f64 / total as f64
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vector_cache_basic_operations() {
        let cache = VectorCache::new(10); // 10MB cache

        // Create test vector
        let mut vector = VectorRecord::default();
        vector.id = "test_vector".to_string();
        vector.values = vec![1.0, 2.0, 3.0];

        // Test put and get
        cache.put("key1".to_string(), vector.clone()).await.unwrap();
        let retrieved = cache.get("key1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "test_vector");

        // Test remove
        assert!(cache.remove("key1").await);
        assert!(cache.get("key1").await.is_none());
    }

    #[tokio::test]
    async fn test_vector_cache_ttl() {
        let cache = VectorCache::with_ttl(10, 0); // 0 second TTL (instant expiry)

        let mut vector = VectorRecord::default();
        vector.id = "test_ttl".to_string();

        cache.put("key1".to_string(), vector.clone()).await.unwrap();

        // Should be expired immediately
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(cache.get("key1").await.is_none());
    }
}