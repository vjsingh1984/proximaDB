use crate::proto::proximadb::VectorRecord;
use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Partitioned key for collection-aware storage
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionedVectorKey {
    pub collection_id: String,
    pub vector_id: String,
}

impl PartitionedVectorKey {
    pub fn new(collection_id: String, vector_id: String) -> Self {
        Self { collection_id, vector_id }
    }
}

impl CacheKey for String {}
impl CacheKey for PartitionedVectorKey {}

impl CacheValue for VectorRecord {
    fn size_bytes(&self) -> usize {
        // Estimate size: vector data + metadata
        self.vector.len() * 4 + 256 // 4 bytes per f32 + metadata overhead
    }
}

/// Specialized cache for vector data with optimizations for batch operations and collection partitioning
pub struct VectorStore {
    /// Collection identifier for partitioning (optional for backward compatibility)
    collection_id: Option<String>,
    /// Base cache with partitioned keys when collection_id is set
    base: BaseCacheImpl<String, VectorRecord>,
    /// Partitioned base cache for collection-aware operations
    partitioned_base: Option<BaseCacheImpl<PartitionedVectorKey, VectorRecord>>,
}

impl VectorStore {
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            collection_id: None,
            base: BaseCacheImpl::new(max_memory_mb),
            partitioned_base: None,
        }
    }
    
    /// Create a new VectorStore for a specific collection
    pub fn new_with_collection(collection_id: String, max_memory_mb: usize) -> Self {
        Self {
            collection_id: Some(collection_id),
            base: BaseCacheImpl::new(0), // Unused when partitioned
            partitioned_base: Some(BaseCacheImpl::new(max_memory_mb)),
        }
    }
    
    /// Batch get operation optimized for locality with collection awareness
    pub async fn batch_get(&self, ids: &[String]) -> Vec<Option<VectorRecord>> {
        let mut results = Vec::with_capacity(ids.len());
        
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                for id in ids {
                    let key = PartitionedVectorKey::new(coll_id.clone(), id.clone());
                    results.push(partitioned.get_with_hooks(&key).await);
                }
            } else {
                // Fallback if partitioned base not initialized
                return vec![None; ids.len()];
            }
        } else {
            // Use regular cache
            for id in ids {
                results.push(self.base.get_with_hooks(id).await);
            }
        }
        
        results
    }
    
    /// Batch put operation with collection awareness
    pub async fn batch_put(&self, records: Vec<(String, VectorRecord)>) {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                for (id, record) in records {
                    let key = PartitionedVectorKey::new(coll_id.clone(), id);
                    partitioned.put_with_hooks(key, record).await;
                }
            }
        } else {
            // Use regular cache
            for (id, record) in records {
                self.base.put_with_hooks(id, record).await;
            }
        }
    }
    
    /// Prefetch vectors that are likely to be accessed together
    pub async fn similarity_prefetch(&self, _query_vector: &[f32], _k: usize) {
        // TODO: Implement similarity-based prefetching
        // This would use an index to find similar vectors and prefetch them
    }
    
    /// Resize the cache
    pub async fn resize(&self, _new_size_mb: usize) -> anyhow::Result<()> {
        // TODO: Implement cache resizing
        Ok(())
    }
    
    /// Clear all cache entries
    pub async fn clear_all(&self) -> anyhow::Result<()> {
        // TODO: Implement cache clearing
        Ok(())
    }
    
    /// Check if a key exists in the cache
    pub async fn contains(&self, key: &str) -> bool {
        self.get(key).await.is_some()
    }
    
    /// Get a vector from the cache with collection awareness
    pub async fn get(&self, key: &str) -> Option<VectorRecord> {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key.to_string());
                partitioned.get_with_hooks(&partitioned_key).await
            } else {
                None
            }
        } else {
            // Use regular cache
            self.base.get_with_hooks(&key.to_string()).await
        }
    }
    
    /// Get a vector from the cache with hooks (alias for compatibility)
    pub async fn get_with_hooks(&self, key: &String) -> Option<VectorRecord> {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key.clone());
                partitioned.get_with_hooks(&partitioned_key).await
            } else {
                None
            }
        } else {
            // Use regular cache
            self.base.get_with_hooks(key).await
        }
    }
    
    /// Put a vector in the cache with collection awareness
    pub async fn put(&self, key: String, value: VectorRecord) {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key);
                partitioned.put_with_hooks(partitioned_key, value).await;
            }
        } else {
            // Use regular cache
            self.base.put_with_hooks(key, value).await;
        }
    }
    
    /// Put a vector in the cache with hooks (alias for compatibility)
    pub async fn put_with_hooks(&self, key: String, value: VectorRecord) {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key);
                partitioned.put_with_hooks(partitioned_key, value).await;
            }
        } else {
            // Use regular cache
            self.base.put_with_hooks(key, value).await;
        }
    }
    
    /// Access metrics from base cache
    pub fn metrics(&self) -> &crate::storage::cache::metrics::CacheMetrics {
        if let Some(ref partitioned) = self.partitioned_base {
            partitioned.metrics()
        } else {
            self.base.metrics()
        }
    }
    
    /// Invalidate a cache entry with collection awareness
    pub async fn invalidate(&self, key: &str) -> bool {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key.to_string());
                BaseCache::invalidate(partitioned, &partitioned_key).await
            } else {
                false
            }
        } else {
            // Use regular cache
            BaseCache::invalidate(&self.base, &key.to_string()).await
        }
    }
}