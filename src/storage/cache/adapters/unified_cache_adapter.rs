//! Compatibility adapter for UnifiedCrossEngineCache
//! 
//! This adapter allows the existing UnifiedCrossEngineCache to work with
//! the new specialized cache architecture while maintaining backward compatibility.

use std::sync::Arc;
use async_trait::async_trait;
use anyhow::Result;

use crate::storage::unified_cache::{UnifiedCrossEngineCache, CacheKey as OldCacheKey};
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};
use crate::storage::cache::specialized::{VectorDataCache, MetadataCache};
use crate::storage::cache::backend::CacheTier;
use crate::proto::proximadb::VectorRecord;

/// Adapter that bridges the old UnifiedCrossEngineCache with new specialized caches
pub struct UnifiedCacheAdapter {
    /// Original unified cache for backward compatibility
    legacy_cache: Arc<UnifiedCrossEngineCache>,
    /// New specialized vector cache
    vector_cache: Arc<VectorDataCache>,
    /// New specialized metadata cache
    metadata_cache: Arc<MetadataCache>,
    /// Migration mode flag
    migration_mode: bool,
}

impl UnifiedCacheAdapter {
    /// Create a new adapter with migration support
    pub fn new(
        legacy_cache: Arc<UnifiedCrossEngineCache>,
        vector_cache: Arc<VectorDataCache>,
        metadata_cache: Arc<MetadataCache>,
    ) -> Self {
        Self {
            legacy_cache,
            vector_cache,
            metadata_cache,
            migration_mode: true,
        }
    }

    /// Migrate data from legacy cache to specialized caches
    pub async fn migrate_data(&self) -> Result<()> {
        // Migration logic to move data from unified cache to specialized caches
        // This happens gradually during normal operations
        
        // Get all vector data from legacy cache
        let vectors = self.legacy_cache.get_all_vectors().await?;
        for (key, vector) in vectors {
            // Convert old key to new format
            let new_key = self.convert_key(&key);
            // Insert into specialized cache using put_with_hooks
            self.vector_cache.put_with_hooks(new_key, vector).await;
        }

        // Get all metadata from legacy cache
        let metadata = self.legacy_cache.get_all_metadata().await?;
        for (key, meta) in metadata {
            let new_key = self.convert_key(&key);
            self.metadata_cache.put_with_hooks(new_key, meta).await;
        }

        Ok(())
    }

    /// Convert old cache key format to new format
    fn convert_key(&self, old_key: &OldCacheKey) -> String {
        format!("{}_{}_{}", old_key.collection_id, old_key.engine, old_key.item_id)
    }

    /// Check if we should use legacy or new cache
    async fn should_use_legacy(&self, key: &str) -> bool {
        if !self.migration_mode {
            return false;
        }
        
        // During migration, check if data exists in new cache first
        // If not found, fall back to legacy cache
        let vec_exists = self.vector_cache.get_with_hooks(key).await.is_some();
        let meta_exists = self.metadata_cache.get_with_hooks(key).await.is_some();
        !vec_exists && !meta_exists
    }

    /// Parse new key format back to old format
    fn parse_old_key(&self, key: &str) -> OldCacheKey {
        let parts: Vec<&str> = key.split('_').collect();
        OldCacheKey {
            collection_id: parts.get(0).unwrap_or("").to_string(),
            engine: parts.get(1).unwrap_or("").to_string(),
            data_type: crate::storage::unified_cache::CacheDataType::Vector,
            item_id: parts.get(2).unwrap_or("").to_string(),
        }
    }

    /// Check if key exists in legacy cache
    async fn legacy_cache_contains(&self, key: &str) -> bool {
        let old_key = self.parse_old_key(key);
        self.legacy_cache.contains(&old_key).await
    }

    /// Complete migration and disable legacy cache
    pub fn complete_migration(&mut self) {
        self.migration_mode = false;
    }
    
    /// Get cache metrics
    pub fn metrics(&self) -> &crate::storage::cache::metrics::CacheMetrics {
        self.vector_cache.metrics()
    }
    
    /// Helper methods for public API
    pub async fn get(&self, key: &str) -> Option<VectorRecord> {
        self.get_with_hooks(&key.to_string()).await
    }
    
    pub async fn put(&self, key: &str, value: VectorRecord) -> Result<()> {
        self.put_with_hooks(key.to_string(), value).await;
        Ok(())
    }
    
    pub async fn remove(&self, key: &str) -> Result<()> {
        self.invalidate(&key.to_string()).await;
        Ok(())
    }
    
    pub async fn clear(&self) -> Result<()> {
        // Clear all caches
        self.vector_cache.clear_all().await?;
        self.metadata_cache.clear_all().await?;
        
        if self.migration_mode {
            self.legacy_cache.clear_all().await?;
        }
        
        Ok(())
    }
    
    pub async fn size(&self) -> usize {
        self.vector_cache.size().await + self.metadata_cache.size().await
    }
    
    pub async fn contains(&self, key: &str) -> bool {
        self.get_with_hooks(&key.to_string()).await.is_some() ||
        (self.migration_mode && self.legacy_cache_contains(key).await)
    }
}

// Implement CacheKey for String
impl CacheKey for String {}

// Implement CacheValue for VectorRecord
impl CacheValue for VectorRecord {
    fn size_bytes(&self) -> usize {
        // Calculate approximate size
        std::mem::size_of::<VectorRecord>() + 
        self.vector.len() * std::mem::size_of::<f32>() +
        self.id.as_ref().map(|s| s.len()).unwrap_or(0) +
        self.collection_id.len()
    }
}

/// Bridge trait implementation for vector operations
#[async_trait]
impl BaseCache for UnifiedCacheAdapter {
    type Key = String;
    type Value = VectorRecord;

    async fn check_l1(&self, key: &Self::Key) -> Option<Self::Value> {
        // Try new cache first
        if let Some(value) = self.vector_cache.get_with_hooks(key).await {
            return Some(value);
        }

        // Fall back to legacy cache during migration
        if self.migration_mode {
            let old_key = self.parse_old_key(key);
            self.legacy_cache.get_vector(&old_key).await.ok()
        } else {
            None
        }
    }
    
    async fn check_l2(&self, _key: &Self::Key) -> Option<Self::Value> {
        // L2 is handled by the underlying caches
        None
    }
    
    async fn check_l3(&self, _key: &Self::Key) -> Option<Self::Value> {
        // L3 is handled by the underlying caches
        None
    }

    async fn put_l1(&self, key: Self::Key, value: Self::Value) {
        // Write to new cache
        self.vector_cache.put_with_hooks(key.clone(), value.clone()).await;

        // Also write to legacy cache during migration
        if self.migration_mode {
            let old_key = self.parse_old_key(&key);
            let _ = self.legacy_cache.put_vector(&old_key, value).await;
        }
    }
    
    async fn put_l2(&self, _key: Self::Key, _value: Self::Value) {
        // L2 is handled by the underlying caches
    }
    
    async fn put_l3(&self, _key: Self::Key, _value: Self::Value) {
        // L3 is handled by the underlying caches
    }

    async fn invalidate_l1(&self, key: &Self::Key) -> bool {
        let mut invalidated = false;
        
        // Remove from new caches
        if self.vector_cache.invalidate(key).await {
            invalidated = true;
        }
        
        if self.migration_mode {
            let old_key = self.parse_old_key(key);
            if self.legacy_cache.remove_vector(&old_key).await.is_ok() {
                invalidated = true;
            }
        }
        
        invalidated
    }
    
    async fn invalidate_l2(&self, _key: &Self::Key) -> bool {
        // L2 is handled by the underlying caches
        false
    }
    
    async fn invalidate_l3(&self, _key: &Self::Key) -> bool {
        // L3 is handled by the underlying caches
        false
    }

    async fn promote_to_l1(&self, _key: &Self::Key, _value: &Self::Value) {
        // Promotion is handled by the underlying caches
    }
    
    async fn promote_to_l2(&self, _key: &Self::Key, _value: &Self::Value) {
        // Promotion is handled by the underlying caches
    }

    async fn select_tier(&self, _key: &Self::Key, _value: &Self::Value) -> CacheTier {
        // Always use L1 for the adapter
        CacheTier::L1
    }

    fn record_hit(&self, tier: CacheTier) {
        self.vector_cache.record_hit(tier);
    }
    
    fn record_miss(&self) {
        self.vector_cache.record_miss();
    }
    
    fn metrics(&self) -> &crate::storage::cache::metrics::CacheMetrics {
        self.vector_cache.metrics()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_adapter_migration() {
        // Test that adapter correctly migrates data from legacy to new cache
        // Implementation of test
    }

    #[tokio::test]
    async fn test_adapter_fallback() {
        // Test that adapter falls back to legacy cache when needed
        // Implementation of test
    }
}