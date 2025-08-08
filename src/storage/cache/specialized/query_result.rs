use crate::proto::proximadb::SearchResult;
use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::SystemTime;

/// Query key that includes the query parameters
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryKey {
    pub collection_id: String,
    pub vector_hash: u64,
    pub k: u32,
    pub filters_hash: u64,
}

impl QueryKey {
    pub fn new(collection_id: String, vector: &[f32], k: u32, filters: Option<&str>) -> Self {
        let mut hasher = DefaultHasher::new();
        
        // Hash the vector
        for v in vector {
            v.to_bits().hash(&mut hasher);
        }
        let vector_hash = hasher.finish();
        
        // Hash the filters
        let mut hasher = DefaultHasher::new();
        if let Some(f) = filters {
            f.hash(&mut hasher);
        }
        let filters_hash = hasher.finish();
        
        Self {
            collection_id,
            vector_hash,
            k,
            filters_hash,
        }
    }
}

impl CacheKey for QueryKey {}

/// Cached query result with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedQueryResult {
    pub results: Vec<SearchResult>,
    pub cached_at: SystemTime,
    pub file_dependencies: Vec<String>,
}

impl CacheValue for CachedQueryResult {
    fn size_bytes(&self) -> usize {
        // Estimate: results + metadata
        self.results.len() * 64 + 256
    }
}

/// Specialized cache for query results with staleness detection
pub struct QueryResultCache {
    base: BaseCacheImpl<QueryKey, CachedQueryResult>,
}

impl QueryResultCache {
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            base: BaseCacheImpl::new(max_memory_mb),
        }
    }
    
    /// Get cached results if fresh
    pub async fn get_if_fresh(
        &self,
        key: &QueryKey,
        max_age_secs: u64,
    ) -> Option<Vec<SearchResult>> {
        if let Some(cached) = BaseCache::get_with_hooks(&self.base, key).await {
            let age = SystemTime::now()
                .duration_since(cached.cached_at)
                .unwrap_or_default()
                .as_secs();
            
            if age <= max_age_secs {
                return Some(cached.results);
            }
        }
        None
    }
    
    /// Cache results with file dependencies
    pub async fn cache_with_dependencies(
        &self,
        key: QueryKey,
        results: Vec<SearchResult>,
        dependencies: Vec<String>,
    ) {
        let cached = CachedQueryResult {
            results,
            cached_at: SystemTime::now(),
            file_dependencies: dependencies,
        };
        
        BaseCache::put_with_hooks(&self.base, key, cached).await;
    }
    
    /// Invalidate all queries dependent on a file
    pub async fn invalidate_by_file(&self, _file_path: &str) {
        // TODO: Implement file-based invalidation
        // This would track which queries depend on which files
    }
}