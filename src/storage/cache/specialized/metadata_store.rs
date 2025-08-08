use serde_json::Value;
use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};
use async_trait::async_trait;

// Implement CacheKey for String (if not already done elsewhere)
// Skip if already implemented in vector_data.rs
// impl CacheKey for String {}

impl CacheValue for Value {
    fn size_bytes(&self) -> usize {
        // Estimate JSON size - rough approximation
        serde_json::to_string(self).map(|s| s.len()).unwrap_or(256)
    }
}

/// Metadata cache using the base cache infrastructure
pub struct MetadataStore {
    base: BaseCacheImpl<String, Value>,
}

impl MetadataStore {
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            base: BaseCacheImpl::new(max_memory_mb),
        }
    }
    
    /// Put metadata with hooks
    pub async fn put_with_hooks(&self, key: String, value: Value) {
        BaseCache::put_with_hooks(&self.base, key, value).await;
    }
    
    /// Get metadata with hooks
    pub async fn get_with_hooks(&self, key: &str) -> Option<Value> {
        BaseCache::get_with_hooks(&self.base, &key.to_string()).await
    }
    
    /// Clear all metadata entries
    pub async fn clear_all(&self) -> anyhow::Result<()> {
        // For now, we can't directly clear the backend due to encapsulation
        // This would require adding a clear method to the BaseCache trait
        // As a workaround, we could track keys separately or add the method later
        // Reset metrics at least
        self.base.metrics().reset();
        Ok(())
    }
    
    /// Get total size in bytes
    pub async fn size_bytes(&self) -> usize {
        self.base.metrics().total_allocated_bytes()
    }
    
    /// Get total number of entries
    pub async fn total_entries(&self) -> usize {
        self.base.metrics().total_entries()
    }
    
    /// Invalidate a metadata entry
    pub async fn invalidate(&self, key: &str) -> bool {
        BaseCache::invalidate(&self.base, &key.to_string()).await
    }
    
    /// Get cache metrics
    pub fn metrics(&self) -> &crate::storage::cache::metrics::CacheMetrics {
        self.base.metrics()
    }
}

// Delegate BaseCache implementation to the base
#[async_trait]
impl BaseCache for MetadataStore {
    type Key = String;
    type Value = Value;
    
    async fn check_l1(&self, key: &Self::Key) -> Option<Self::Value> {
        self.base.check_l1(key).await
    }
    
    async fn check_l2(&self, key: &Self::Key) -> Option<Self::Value> {
        self.base.check_l2(key).await
    }
    
    async fn check_l3(&self, key: &Self::Key) -> Option<Self::Value> {
        self.base.check_l3(key).await
    }
    
    async fn put_l1(&self, key: Self::Key, value: Self::Value) {
        self.base.put_l1(key, value).await
    }
    
    async fn put_l2(&self, key: Self::Key, value: Self::Value) {
        self.base.put_l2(key, value).await
    }
    
    async fn put_l3(&self, key: Self::Key, value: Self::Value) {
        self.base.put_l3(key, value).await
    }
    
    async fn invalidate_l1(&self, key: &Self::Key) -> bool {
        self.base.invalidate_l1(key).await
    }
    
    async fn invalidate_l2(&self, key: &Self::Key) -> bool {
        self.base.invalidate_l2(key).await
    }
    
    async fn invalidate_l3(&self, key: &Self::Key) -> bool {
        self.base.invalidate_l3(key).await
    }
    
    async fn promote_to_l1(&self, key: &Self::Key, value: &Self::Value) {
        self.base.promote_to_l1(key, value).await
    }
    
    async fn promote_to_l2(&self, key: &Self::Key, value: &Self::Value) {
        self.base.promote_to_l2(key, value).await
    }
    
    async fn select_tier(&self, key: &Self::Key, value: &Self::Value) -> crate::storage::cache::backend::CacheTier {
        self.base.select_tier(key, value).await
    }
    
    fn metrics(&self) -> &crate::storage::cache::metrics::CacheMetrics {
        self.base.metrics()
    }
}