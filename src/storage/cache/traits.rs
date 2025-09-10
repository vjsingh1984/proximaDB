use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::hash::Hash;
use std::time::SystemTime;

use crate::storage::cache::backend::CacheTier;
use crate::storage::cache::metrics::CacheMetrics;

/// Base trait for cache keys
pub trait CacheKey: Hash + Eq + Clone + Send + Sync + Debug + 'static {}

// Implement CacheKey for common types
impl CacheKey for String {}
impl CacheKey for u64 {}

/// Base trait for cache values
pub trait CacheValue: Clone + Send + Sync + Debug + 'static {
    /// Get the approximate size in bytes
    fn size_bytes(&self) -> usize;
}

/// Base cache trait with template methods for all cache implementations
#[async_trait]
pub trait BaseCache: Send + Sync {
    type Key: CacheKey;
    type Value: CacheValue;

    /// Template method - defines algorithm structure for cache retrieval
    async fn get_with_hooks(&self, key: &Self::Key) -> Option<Self::Value> {
        // Pre-get hook for custom logic
        self.pre_get_hook(key).await;

        // Check each tier in order
        if let Some(value) = self.check_l1(key).await {
            self.record_hit(CacheTier::L1);
            return Some(value);
        }

        if let Some(value) = self.check_l2(key).await {
            // Promote to L1 for faster future access
            self.promote_to_l1(key, &value).await;
            self.record_hit(CacheTier::L2);
            return Some(value);
        }

        if let Some(value) = self.check_l3(key).await {
            // Promote to L2 (and potentially L1)
            self.promote_to_l2(key, &value).await;
            self.record_hit(CacheTier::L3);
            return Some(value);
        }

        self.record_miss();
        self.post_miss_hook(key).await;
        None
    }

    /// Put value into cache with automatic tier placement
    async fn put_with_hooks(&self, key: Self::Key, value: Self::Value) {
        self.pre_put_hook(&key, &value).await;

        // Determine appropriate tier based on value size and access patterns
        let tier = self.select_tier(&key, &value).await;

        match tier {
            CacheTier::L1 => self.put_l1(key.clone(), key.clone()).await,
            CacheTier::L2 => self.put_l2(key.clone(), key.clone()).await,
            CacheTier::L3 => self.put_l3(key.clone(), key.clone()).await,
        }

        self.post_put_hook(&key, &value).await;
    }

    /// Invalidate a cache entry across all tiers
    async fn invalidate(&self, key: &Self::Key) -> bool {
        let mut invalidated = false;

        invalidated |= self.invalidate_l1(key).await;
        invalidated |= self.invalidate_l2(key).await;
        invalidated |= self.invalidate_l3(key).await;

        if invalidated {
            self.post_invalidate_hook(key).await;
        }

        invalidated
    }

    // Hooks for specialization - default implementations do nothing
    async fn pre_get_hook(&self, _key: &Self::Key) {}
    async fn post_miss_hook(&self, _key: &Self::Key) {}
    async fn pre_put_hook(&self, _key: &Self::Key, _value: &Self::Value) {}
    async fn post_put_hook(&self, _key: &Self::Key, _value: &Self::Value) {}
    async fn post_invalidate_hook(&self, _key: &Self::Key) {}

    // Tier-specific operations - must be implemented
    async fn check_l1(&self, key: &Self::Key) -> Option<Self::Value>;
    async fn check_l2(&self, key: &Self::Key) -> Option<Self::Value>;
    async fn check_l3(&self, key: &Self::Key) -> Option<Self::Value>;

    async fn put_l1(&self, key: Self::Key, value: Self::Value);
    async fn put_l2(&self, key: Self::Key, value: Self::Value);
    async fn put_l3(&self, key: Self::Key, value: Self::Value);

    async fn invalidate_l1(&self, key: &Self::Key) -> bool;
    async fn invalidate_l2(&self, key: &Self::Key) -> bool;
    async fn invalidate_l3(&self, key: &Self::Key) -> bool;

    async fn promote_to_l1(&self, key: &Self::Key, value: &Self::Value);
    async fn promote_to_l2(&self, key: &Self::Key, value: &Self::Value);

    async fn select_tier(&self, key: &Self::Key, value: &Self::Value) -> CacheTier;

    // Metrics operations
    fn record_hit(&self, tier: CacheTier) {
        self.metrics().record_hit(tier);
    }

    fn record_miss(&self) {
        self.metrics().record_miss();
    }

    fn metrics(&self) -> &CacheMetrics;
}

/// Cache entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry<V: CacheValue> {
    pub value: V,
    pub inserted_at: SystemTime,
    pub last_accessed: SystemTime,
    pub access_count: u64,
    pub size_bytes: usize,
}

impl<V: CacheValue> CacheEntry<V> {
    pub fn new(value: V) -> Self {
        let size_bytes = value.size_bytes();
        let now = SystemTime::now();
        Self {
            value,
            inserted_at: now,
            last_accessed: now,
            access_count: 1,
            size_bytes,
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = SystemTime::now();
        self.access_count += 1;
    }

    pub fn age(&self) -> std::time::Duration {
        SystemTime::now()
            .duration_since(self.inserted_at)
            .unwrap_or_default()
    }
}

// Implement CacheValue for CacheEntry so it can be stored properly
impl<V: CacheValue> CacheValue for CacheEntry<V> {
    fn size_bytes(&self) -> usize {
        // The size was already calculated when the entry was created
        self.size_bytes + std::mem::size_of::<Self>()
    }
}
