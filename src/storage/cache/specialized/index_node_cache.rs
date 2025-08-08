use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};
use crate::storage::cache::metrics::CacheMetrics;
use serde::{Deserialize, Serialize};

/// Index node that can be cached
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexNode {
    pub id: String,
    pub level: u32,
    pub children: Vec<String>,
    pub data: Vec<u8>,
}

// String already implements CacheKey elsewhere
impl CacheValue for IndexNode {
    fn size_bytes(&self) -> usize {
        self.data.len() + self.children.len() * 32 + 64 // Approximate size
    }
}

/// Specialized cache for index structures with hot path optimization
pub struct IndexNodeCache {
    base: BaseCacheImpl<String, IndexNode>,
}

impl IndexNodeCache {
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            base: BaseCacheImpl::new(max_memory_mb * 1024 * 1024),
        }
    }
    
    /// Delegate put_with_hooks to base cache
    pub async fn put_with_hooks(&self, key: String, value: IndexNode) {
        BaseCache::put_with_hooks(&self.base, key, value).await;
    }
    
    /// Delegate get_with_hooks to base cache
    pub async fn get_with_hooks(&self, key: &String) -> Option<IndexNode> {
        BaseCache::get_with_hooks(&self.base, key).await
    }
    
    /// Prefetch index path for a vector
    pub async fn prefetch_vector_index_path(&self, _vector_id: &str) {
        // TODO: Implement prefetching logic based on index structure
        // This would traverse the index tree and cache hot nodes
    }
    
    /// Invalidate a cached index node
    pub async fn invalidate(&self, key: &str) -> bool {
        BaseCache::invalidate(&self.base, &key.to_string()).await
    }
    
    /// Get cache metrics
    pub fn metrics(&self) -> &CacheMetrics {
        self.base.metrics()
    }
}