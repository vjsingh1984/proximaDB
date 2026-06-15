/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Unified Cache Interface (TD-042)
//!
//! This module provides a unified interface for all cache types in ProximaDB,
//! enabling cross-cache operations, coordinated invalidation, and shared
//! infrastructure.
//!
//! ## Architecture
//!
//! ```text
//! UnifiedCache Trait
//!        ↓
//!    ┌───┴────┬─────────┬──────────┐
//!    ↓        ↓         ↓          ↓
//! Vector  Metadata  Query  BitmapFilter
//! Cache    Cache    Cache     Cache
//!    └────────┴─────────┴──────────┘
//!                ↓
//!       Shared Infrastructure:
//!       - String Interner
//!       - Eviction Policies
//!       - Metrics Collection
//!       - Cascade Invalidation
//! ```
//!
//! ## Benefits
//!
//! 1. **Type Safety**: Generic over cache key and value types
//! 2. **Shared Infrastructure**: Common operations implemented once
//! 3. **Cross-Cache Operations**: Invalidation and prefetching across caches
//! 4. **Unified Metrics**: Consistent monitoring across all cache types
//! 5. **Testing**: Mock implementations for testing

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use async_recursion::async_recursion;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use proximadb_kernel::error::VectorDBError;

/// Cache key trait - all cache keys must implement this
pub trait CacheKey: Send + Sync + Hash + Eq + Clone + Debug + ToString {}

// Implement CacheKey for common types
impl CacheKey for String {}
impl CacheKey for u64 {}
impl CacheKey for i64 {}
impl CacheKey for u32 {}

/// Cache value trait - all cache values must implement this
pub trait CacheValue: Send + Sync + Clone + Debug {}

// Implement CacheValue for common types
impl<T: Send + Sync + Clone + Debug> CacheValue for T {}

/// Cache identifier for cross-cache operations
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum CacheId {
    /// Vector data cache
    VectorData,
    /// Query result cache
    QueryResult,
    /// Metadata cache
    Metadata,
    /// Bitmap filter cache
    BitmapFilter,
    /// Index node cache
    IndexNode,
}

impl CacheId {
    /// Get all cache IDs
    pub fn all() -> Vec<CacheId> {
        vec![
            CacheId::VectorData,
            CacheId::QueryResult,
            CacheId::Metadata,
            CacheId::BitmapFilter,
            CacheId::IndexNode,
        ]
    }

    /// Get cache name for logging/metrics
    pub fn name(&self) -> &str {
        match self {
            CacheId::VectorData => "vector_data",
            CacheId::QueryResult => "query_result",
            CacheId::Metadata => "metadata",
            CacheId::BitmapFilter => "bitmap_filter",
            CacheId::IndexNode => "index_node",
        }
    }
}

/// Cache statistics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCoordinatorStats {
    /// Cache identifier
    pub cache_id: CacheId,
    /// Total number of entries
    pub entry_count: usize,
    /// Total memory usage in bytes
    pub memory_usage_bytes: u64,
    /// Number of hits since last reset
    pub hits: u64,
    /// Number of misses since last reset
    pub misses: u64,
    /// Hit rate (0.0 to 1.0)
    pub hit_rate: f64,
    /// Number of evictions since last reset
    pub evictions: u64,
}

/// Cache dependency for cascade invalidation
#[derive(Debug, Clone)]
pub struct CacheDependency {
    /// Cache that this entry depends on
    pub cache_id: CacheId,
    /// Key in the dependency cache
    pub dependency_key: String,
}

/// Unified cache interface
///
/// This trait provides a common interface for all cache types, enabling
/// shared infrastructure and cross-cache operations.
#[async_trait]
pub trait UnifiedCache: Send + Sync {
    /// Get value from cache
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key
    ///
    /// # Returns
    ///
    /// `Some(value)` if found, `None` if cache miss
    ///
    /// # Example
    ///
    /// ```ignore
    /// let value = cache.get(&"vector_123".to_string()).await?;
    /// ```
    async fn get<K: CacheKey + 'static, V: CacheValue + 'static>(
        &self,
        key: &K,
    ) -> Result<Option<V>, VectorDBError>;

    /// Put value in cache with cross-cache invalidation
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key
    /// * `value` - Value to cache
    /// * `dependencies` - Optional cache dependencies for cascade invalidation
    ///
    /// # Example
    ///
    /// ```ignore
    /// let deps = vec![CacheDependency {
    ///     cache_id: CacheId::Metadata,
    ///     dependency_key: "collection_123".to_string(),
    /// }];
    /// cache.put(&"vector_123".to_string(), vector_data, Some(&deps)).await?;
    /// ```
    async fn put<K: CacheKey + 'static, V: CacheValue + 'static>(
        &self,
        key: K,
        value: V,
        dependencies: Option<&[CacheDependency]>,
    ) -> Result<(), VectorDBError>;

    /// Invalidate entry and its dependents
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key to invalidate
    ///
    /// # Cascade Invalidation
    ///
    /// When an entry is invalidated, all dependent entries across caches
    /// are also invalidated. For example, invalidating metadata should
    /// invalidate all vector data that depends on that metadata.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Invalidate metadata entry
    /// cache.invalidate(&"collection_123".to_string()).await?;
    /// // This also invalidates all vectors in that collection
    /// ```
    async fn invalidate<K: CacheKey + 'static>(&self, key: &K) -> Result<(), VectorDBError>;

    /// Check if key exists in cache
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key
    ///
    /// # Returns
    ///
    /// `true` if key exists, `false` otherwise
    async fn contains<K: CacheKey + 'static>(&self, key: &K) -> Result<bool, VectorDBError>;

    /// Clear all entries from cache
    ///
    /// # Example
    ///
    /// ```ignore
    /// cache.clear().await?;
    /// ```
    async fn clear(&self) -> Result<(), VectorDBError>;

    /// Get cache statistics
    ///
    /// # Returns
    ///
    /// Snapshot of cache statistics
    ///
    /// # Example
    ///
    /// ```ignore
    /// let stats = cache.stats().await?;
    /// println!("Hit rate: {:.2}%", stats.hit_rate * 100.0);
    /// ```
    async fn stats(&self) -> Result<CacheCoordinatorStats, VectorDBError>;

    /// Get cache identifier
    ///
    /// # Returns
    ///
    /// Cache ID for this cache instance
    fn cache_id(&self) -> CacheId;

    /// Prefetch value for predicted future access
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key to prefetch
    ///
    /// # Use Case
    ///
    /// Called by access pattern tracker when predictive prefetching
    /// indicates a high probability of future access.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Access pattern tracker predicts this vector will be accessed soon
    /// cache.prefetch(&"vector_456".to_string()).await?;
    /// ```
    async fn prefetch<K: CacheKey + 'static>(&self, key: &K) -> Result<(), VectorDBError>;

    /// Resize cache to new capacity
    ///
    /// # Arguments
    ///
    /// * `new_capacity_bytes` - New cache capacity in bytes
    ///
    /// # Behavior
    ///
    /// If shrinking, evict entries until under new capacity.
    /// If growing, simply update capacity limit.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Reduce cache size to 1GB
    /// cache.resize(1_000_000_000).await?;
    /// ```
    async fn resize(&self, new_capacity_bytes: u64) -> Result<(), VectorDBError>;
}

/// Unified cache coordinator for cross-cache operations
///
/// This coordinator manages cross-cache dependencies, cascade invalidation,
/// and coordinated eviction across all cache instances.
pub struct UnifiedCacheCoordinator {
    /// Registered cache instances
    caches: HashMap<CacheId, Arc<dyn UnifiedCacheCoordinatorInternal>>,
    /// Dependency graph: cache_key -> list of dependent entries
    dependency_graph: Arc<tokio::sync::RwLock<DependencyGraph>>,
    /// Global string interner (shared across all caches)
    string_interner: Arc<crate::storage::cache::orchestrator::StringInterner>,
}

/// Internal trait for coordinator operations
#[async_trait]
pub(crate) trait UnifiedCacheCoordinatorInternal: Send + Sync {
    /// Invalidate entry in this cache
    async fn invalidate_entry(&self, key: &str) -> Result<(), VectorDBError>;
}

/// Dependency graph for cascade invalidation
#[derive(Debug, Clone, Default)]
struct DependencyGraph {
    /// Map from cache_id+key to list of dependent cache_id+keys
    /// Format: "cache_id:key" -> Vec<("dependent_cache_id:dependent_key")>
    dependencies: HashMap<String, Vec<String>>,
}

impl UnifiedCacheCoordinator {
    /// Create new unified cache coordinator
    pub fn new() -> Self {
        Self {
            caches: HashMap::new(),
            dependency_graph: Arc::new(tokio::sync::RwLock::new(DependencyGraph::default())),
            string_interner: Arc::new(crate::storage::cache::orchestrator::StringInterner::new()),
        }
    }

    /// Register a cache instance
    ///
    /// # Arguments
    ///
    /// * `cache` - Cache instance implementing internal interface
    #[allow(dead_code)]
    pub(crate) fn register_cache(
        &mut self,
        cache_id: CacheId,
        cache: Arc<dyn UnifiedCacheCoordinatorInternal>,
    ) {
        self.caches.insert(cache_id, cache);
    }

    /// Register dependency between cache entries
    ///
    /// # Arguments
    ///
    /// * `cache_id` - Cache containing the dependent entry
    /// * `key` - Key of the dependent entry
    /// * `dependency` - Cache dependency
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Vector data depends on metadata
    /// coordinator.register_dependency(
    ///     CacheId::VectorData,
    ///     "vector_123",
    ///     &CacheDependency {
    ///         cache_id: CacheId::Metadata,
    ///         dependency_key: "collection_123".to_string(),
    ///     }
    /// ).await?;
    /// ```
    pub async fn register_dependency(
        &self,
        cache_id: CacheId,
        key: &str,
        dependency: &CacheDependency,
    ) -> Result<(), VectorDBError> {
        let mut graph = self.dependency_graph.write().await;

        // Create dependency key
        let dep_key = format!(
            "{}:{}",
            dependency.cache_id.name(),
            dependency.dependency_key
        );

        // Create dependent key
        let dependent_key = format!("{}:{}", cache_id.name(), key);

        // Add to dependency graph
        graph
            .dependencies
            .entry(dep_key)
            .or_insert_with(Vec::new)
            .push(dependent_key);

        Ok(())
    }

    /// Invalidate entry and all dependents
    ///
    /// # Arguments
    ///
    /// * `cache_id` - Cache containing the entry
    /// * `key` - Key to invalidate
    ///
    /// # Cascade Invalidation
    ///
    /// 1. Invalidate entry in specified cache
    /// 2. Look up dependent entries in dependency graph
    /// 3. Recursively invalidate all dependents
    #[async_recursion]
    pub async fn invalidate_with_dependents(
        &self,
        cache_id: CacheId,
        key: &str,
    ) -> Result<(), VectorDBError> {
        // Invalidate in specified cache
        if let Some(cache) = self.caches.get(&cache_id) {
            cache.invalidate_entry(key).await?;
        }

        // Invalidate dependents
        let dep_key = format!("{}:{}", cache_id.name(), key);
        let mut graph = self.dependency_graph.write().await;

        if let Some(dependents) = graph.dependencies.remove(&dep_key) {
            for dependent_key in dependents {
                // Parse cache_id:key
                let parts: Vec<&str> = dependent_key.split(':').collect();
                if parts.len() == 2 {
                    let dep_cache_id = Self::parse_cache_id(parts[0])?;
                    let dep_key = parts[1];

                    // Recursively invalidate dependent
                    drop(graph); // Release write lock before recursion
                    self.invalidate_with_dependents(dep_cache_id, dep_key)
                        .await?;
                    graph = self.dependency_graph.write().await;
                }
            }
        }

        Ok(())
    }

    /// Parse cache ID from name
    fn parse_cache_id(name: &str) -> Result<CacheId, VectorDBError> {
        match name {
            "vector_data" => Ok(CacheId::VectorData),
            "query_result" => Ok(CacheId::QueryResult),
            "metadata" => Ok(CacheId::Metadata),
            "bitmap_filter" => Ok(CacheId::BitmapFilter),
            "index_node" => Ok(CacheId::IndexNode),
            _ => Err(VectorDBError::Internal(format!(
                "Unknown cache ID: {}",
                name
            ))),
        }
    }

    /// Get global string interner
    pub fn string_interner(&self) -> Arc<crate::storage::cache::orchestrator::StringInterner> {
        self.string_interner.clone()
    }

    /// Get all cache statistics
    ///
    /// # Returns
    ///
    /// Statistics for all registered caches
    pub async fn get_all_stats(&self) -> HashMap<CacheId, CacheCoordinatorStats> {
        let mut stats = HashMap::new();

        for cache_id in self.caches.keys() {
            // TODO: Get stats from each cache
            // For now, add placeholder
            stats.insert(
                *cache_id,
                CacheCoordinatorStats {
                    cache_id: *cache_id,
                    entry_count: 0,
                    memory_usage_bytes: 0,
                    hits: 0,
                    misses: 0,
                    hit_rate: 0.0,
                    evictions: 0,
                },
            );
        }

        stats
    }
}

impl Default for UnifiedCacheCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_id_name() {
        assert_eq!(CacheId::VectorData.name(), "vector_data");
        assert_eq!(CacheId::QueryResult.name(), "query_result");
        assert_eq!(CacheId::Metadata.name(), "metadata");
    }

    #[test]
    fn test_cache_id_all() {
        let all = CacheId::all();
        assert_eq!(all.len(), 5);
        assert!(all.contains(&CacheId::VectorData));
        assert!(all.contains(&CacheId::QueryResult));
    }

    #[test]
    fn test_metadata_cache_coordinator_new() {
        let coordinator = UnifiedCacheCoordinator::new();
        assert!(coordinator.caches.is_empty());
    }

    #[tokio::test]
    async fn test_dependency_registration() {
        let coordinator = UnifiedCacheCoordinator::new();

        let dep = CacheDependency {
            cache_id: CacheId::Metadata,
            dependency_key: "collection_123".to_string(),
        };

        coordinator
            .register_dependency(CacheId::VectorData, "vector_123", &dep)
            .await
            .unwrap();

        let graph = coordinator.dependency_graph.read().await;
        assert!(graph.dependencies.len() > 0);
    }

    #[tokio::test]
    async fn test_coordinator_string_interner() {
        let coordinator = UnifiedCacheCoordinator::new();
        let interner = coordinator.string_interner();

        // Intern a string
        let arc_str = interner.intern("test_string").await;
        assert_eq!(arc_str.as_ref(), "test_string");
    }
}
