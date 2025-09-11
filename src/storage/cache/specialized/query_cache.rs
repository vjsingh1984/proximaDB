use crate::proto::proximadb_v1::SearchResult;
use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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
#[derive(Debug)]
pub struct QueryCache {
    base: BaseCacheImpl<QueryKey, CachedQueryResult>,
}

impl QueryCache {
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            base: BaseCacheImpl::new(max_memory_mb),
        }
    }

    /// Delegate put_with_hooks to base cache
    pub async fn put_with_hooks(&self, key: QueryKey, value: CachedQueryResult) {
        BaseCache::put_with_hooks(&self.base, key, value).await;
    }

    /// Delegate get_with_hooks to base cache
    pub async fn get_with_hooks(&self, key: &QueryKey) -> Option<CachedQueryResult> {
        BaseCache::get_with_hooks(&self.base, key).await
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

    /// Get cached results as v1 if fresh (converts legacy to v1 on read)
    pub async fn get_if_fresh_v1(
        &self,
        key: &QueryKey,
        max_age_secs: u64,
    ) -> Option<Vec<crate::proto::proximadb_v1::SearchResult>> {
        if let Some(cached) = BaseCache::get_with_hooks(&self.base, key).await {
            let age = SystemTime::now()
                .duration_since(cached.cached_at)
                .unwrap_or_default()
                .as_secs();

            if age <= max_age_secs {
                return Some(
                    cached
                        .results
                        .into_iter()
                        // Results are already v1, no conversion needed
                        .collect(),
                );
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

    /// Cache v1 results by converting to legacy for storage
    pub async fn cache_with_dependencies_v1(
        &self,
        key: QueryKey,
        results: Vec<crate::proto::proximadb_v1::SearchResult>,
        dependencies: Vec<String>,
    ) {
        let legacy: Vec<SearchResult> = results
            .into_iter()
            // Results are already v1, no conversion needed
            .collect();
        let cached = CachedQueryResult {
            results: legacy,
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

    /// Invalidate a specific query result
    pub async fn invalidate(&self, _key: &str) -> bool {
        // Convert string key to QueryKey if possible
        // For now, return false as we can't invalidate without proper QueryKey
        false
    }

    /// Resize the cache
    pub async fn resize(&self, _new_size_mb: usize) -> anyhow::Result<()> {
        // TODO: Implement cache resizing
        Ok(())
    }

    /// Get cache metrics
    pub fn metrics(&self) -> &crate::storage::cache::metrics::CacheMetrics {
        self.base.metrics()
    }
}
