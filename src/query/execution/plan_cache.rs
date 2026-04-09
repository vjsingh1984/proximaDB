//! Query Plan Cache for Reusing Optimized Execution Plans
//!
//! This module implements a cache for query execution plans to eliminate the overhead
//! of query planning and optimization for repeated or similar queries.

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, info};

use crate::query::execution::ExecutionPlan;

/// Unique identifier for a query plan
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanKey {
    /// Query fingerprint (hash of query text and parameters)
    pub query_fingerprint: u64,
    /// Target collection name
    pub collection_id: String,
    /// Query type (vector search, SQL, graph, etc.)
    pub query_type: String,
}

impl PlanKey {
    /// Create a new plan key
    pub fn new(query_fingerprint: u64, collection_id: String, query_type: String) -> Self {
        Self {
            query_fingerprint,
            collection_id,
            query_type,
        }
    }

    /// Create a plan key from a query string
    pub fn from_query(query: &str, collection_id: &str, query_type: &str) -> Self {
        let query_fingerprint = Self::hash_query(query);
        Self {
            query_fingerprint,
            collection_id: collection_id.to_string(),
            query_type: query_type.to_string(),
        }
    }

    /// Hash a query string to create a fingerprint
    fn hash_query(query: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        query.hash(&mut hasher);
        hasher.finish()
    }
}

/// Cached query plan with performance metrics
#[derive(Debug, Clone)]
pub struct CachedPlan {
    /// The execution plan
    pub plan: ExecutionPlan,
    /// When this plan was created
    pub created_at: Instant,
    /// How many times this plan has been reused
    pub reuse_count: u64,
    /// Average execution time for this plan
    pub avg_execution_time_ms: f64,
    /// Estimated cost of this plan
    pub estimated_cost: f64,
    /// Whether this plan has been validated
    pub is_validated: bool,
}

impl CachedPlan {
    /// Create a new cached plan
    pub fn new(plan: ExecutionPlan) -> Self {
        Self {
            plan,
            created_at: Instant::now(),
            reuse_count: 0,
            avg_execution_time_ms: 0.0,
            estimated_cost: 0.0,
            is_validated: false,
        }
    }

    /// Record that this plan was reused
    pub fn record_reuse(&mut self) {
        self.reuse_count += 1;
    }

    /// Update average execution time based on new measurement
    pub fn update_execution_time(&mut self, new_time_ms: f64) {
        // Exponential moving average with alpha = 0.2
        let alpha = 0.2;
        if self.avg_execution_time_ms == 0.0 {
            self.avg_execution_time_ms = new_time_ms;
        } else {
            self.avg_execution_time_ms =
                alpha * new_time_ms + (1.0 - alpha) * self.avg_execution_time_ms;
        }
    }

    /// Check if this plan is stale (should be re-optimized)
    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.created_at.elapsed() > max_age
    }
}

/// Query plan cache configuration
#[derive(Debug, Clone)]
pub struct PlanCacheConfig {
    /// Maximum number of plans to cache
    pub max_plans: usize,
    /// Maximum age of a cached plan before it's considered stale
    pub max_plan_age: Duration,
    /// Enable automatic plan validation
    pub enable_validation: bool,
    /// Threshold for plan reuse (reuse below this doesn't justify caching)
    pub min_reuse_threshold: u32,
}

impl Default for PlanCacheConfig {
    fn default() -> Self {
        Self {
            max_plans: 1000,
            max_plan_age: Duration::from_secs(300), // 5 minutes
            enable_validation: true,
            min_reuse_threshold: 3,
        }
    }
}

/// Query plan cache
pub struct QueryPlanCache {
    /// Cache storage
    cache: DashMap<PlanKey, CachedPlan>,
    /// Configuration
    config: PlanCacheConfig,
    /// Cache hits
    hits: Arc<std::sync::atomic::AtomicU64>,
    /// Cache misses
    misses: Arc<std::sync::atomic::AtomicU64>,
}

impl QueryPlanCache {
    /// Create a new query plan cache
    pub fn new(config: PlanCacheConfig) -> Self {
        info!("Creating query plan cache with {:?}", config);
        Self {
            cache: DashMap::new(),
            config,
            hits: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            misses: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Get a cached execution plan for the given key
    pub fn get(&self, key: &PlanKey) -> Option<ExecutionPlan> {
        if let Some(mut cached_plan) = self.cache.get_mut(key) {
            // Check if plan is stale
            if cached_plan.is_stale(self.config.max_plan_age) {
                debug!("Cached plan is stale, removing from cache");
                self.cache.remove(key);
                self.misses
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return None;
            }

            // Record reuse
            cached_plan.record_reuse();
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            debug!(
                "Cache hit for plan (reused {} times)",
                cached_plan.reuse_count
            );

            Some(cached_plan.plan.clone())
        } else {
            self.misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        }
    }

    /// Insert a new execution plan into the cache
    pub fn insert(&self, key: PlanKey, plan: ExecutionPlan) {
        // Check cache size limit
        if self.cache.len() >= self.config.max_plans {
            // Remove least recently used plans
            self.evict_lru(1);
        }

        let cached_plan = CachedPlan::new(plan);
        self.cache.insert(key, cached_plan);
        debug!("Inserted new plan into cache (size: {})", self.cache.len());
    }

    /// Get or create a plan for the given key
    pub fn get_or_create<F>(
        &self,
        key: PlanKey,
        plan_creator: F,
    ) -> Result<ExecutionPlan, anyhow::Error>
    where
        F: FnOnce() -> Result<ExecutionPlan, anyhow::Error>,
    {
        // Try to get from cache first
        if let Some(plan) = self.get(&key) {
            return Ok(plan);
        }

        // Create new plan
        debug!("Cache miss, creating new plan");
        let plan = plan_creator()?;

        // Insert into cache
        self.insert(key, plan.clone());

        Ok(plan)
    }

    /// Remove least recently used plans
    fn evict_lru(&self, count: usize) {
        let mut removed = 0;
        self.cache.retain(|_key, plan| {
            if removed < count {
                // Remove plans with low reuse count
                if plan.reuse_count < self.config.min_reuse_threshold as u64 {
                    removed += 1;
                    false
                } else {
                    true
                }
            } else {
                true
            }
        });

        if removed > 0 {
            info!("Evicted {} plans from cache", removed);
        }
    }

    /// Cleanup stale plans
    pub fn cleanup_stale(&self) -> usize {
        let mut removed = 0;
        self.cache.retain(|_key, plan| {
            if plan.is_stale(self.config.max_plan_age) {
                removed += 1;
                false
            } else {
                true
            }
        });
        removed
    }

    /// Get cache statistics
    pub fn stats(&self) -> PlanCacheStats {
        let hits = self.hits.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.misses.load(std::sync::atomic::Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };

        PlanCacheStats {
            total_plans: self.cache.len(),
            hits,
            misses,
            hit_rate,
            max_plans: self.config.max_plans,
        }
    }

    /// Clear all cached plans
    pub fn clear(&self) {
        self.cache.clear();
        info!("Cleared all cached plans");
    }
}

/// Plan cache statistics
#[derive(Debug, Clone)]
pub struct PlanCacheStats {
    /// Total number of cached plans
    pub total_plans: usize,
    /// Total cache hits
    pub hits: u64,
    /// Total cache misses
    pub misses: u64,
    /// Cache hit rate (0.0 to 1.0)
    pub hit_rate: f64,
    /// Maximum plan cache size
    pub max_plans: usize,
}

impl fmt::Display for PlanCacheStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PlanCacheStats{{ plans: {}, hit_rate: {:.1}%, hits: {}, misses: {} }}",
            self.total_plans,
            self.hit_rate * 100.0,
            self.hits,
            self.misses
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_key_creation() {
        let key = PlanKey::from_query("SELECT * FROM test", "test_collection", "sql");
        assert_eq!(key.collection_id, "test_collection");
        assert_eq!(key.query_type, "sql");
        assert_ne!(key.query_fingerprint, 0);
    }

    #[test]
    fn test_cached_plan_reuse() {
        use crate::query::execution::ExecutionStrategy;
        let plan = ExecutionPlan {
            execution_strategy: ExecutionStrategy::VectorOnly,
            operations: vec![],
            estimated_cost: 0.0,
            optimizations: vec![],
            performance_hints: vec![],
            seeding_strategy: crate::query::execution::SeedingStrategy::Average,
            limit: None,
            offset: None,
        };
        let mut cached_plan = CachedPlan::new(plan);

        assert_eq!(cached_plan.reuse_count, 0);
        cached_plan.record_reuse();
        assert_eq!(cached_plan.reuse_count, 1);
        cached_plan.record_reuse();
        assert_eq!(cached_plan.reuse_count, 2);
    }

    #[test]
    fn test_execution_time_update() {
        use crate::query::execution::ExecutionStrategy;
        let plan = ExecutionPlan {
            execution_strategy: ExecutionStrategy::VectorOnly,
            operations: vec![],
            estimated_cost: 0.0,
            optimizations: vec![],
            performance_hints: vec![],
            seeding_strategy: crate::query::execution::SeedingStrategy::Average,
            limit: None,
            offset: None,
        };
        let mut cached_plan = CachedPlan::new(plan);

        assert_eq!(cached_plan.avg_execution_time_ms, 0.0);

        // First measurement sets the value
        cached_plan.update_execution_time(100.0);
        assert_eq!(cached_plan.avg_execution_time_ms, 100.0);

        // Second measurement uses exponential moving average
        cached_plan.update_execution_time(200.0);
        assert!(cached_plan.avg_execution_time_ms > 100.0);
        assert!(cached_plan.avg_execution_time_ms < 200.0);
    }

    #[test]
    fn test_plan_cache_stats() {
        let config = PlanCacheConfig::default();
        let cache = QueryPlanCache::new(config);

        // Initially empty
        let stats = cache.stats();
        assert_eq!(stats.total_plans, 0);
        assert_eq!(stats.hit_rate, 0.0);

        // Add a plan
        use crate::query::execution::ExecutionStrategy;
        let key = PlanKey::from_query("SELECT 1", "test", "sql");
        let plan = ExecutionPlan {
            execution_strategy: ExecutionStrategy::VectorOnly,
            operations: vec![],
            estimated_cost: 0.0,
            optimizations: vec![],
            performance_hints: vec![],
            seeding_strategy: crate::query::execution::SeedingStrategy::Average,
            limit: None,
            offset: None,
        };
        cache.insert(key.clone(), plan);

        // Should now have one plan
        let stats = cache.stats();
        assert_eq!(stats.total_plans, 1);
    }
}
