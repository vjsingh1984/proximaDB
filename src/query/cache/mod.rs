//! # Query Result Cache Module
//!
//! Provides query result caching for ProximaDB, enabling high-performance caching
//! of query results for agentic AI workloads with repetitive queries.
//!
//! ## Key Features
//!
//! - **Thread-safe concurrent access**: Uses DashMap for lock-free concurrent operations
//! - **TTL-based expiration**: Automatic cleanup of stale cache entries
//! - **Dependency tracking**: Track which collections a query depends on
//! - **Real-time invalidation**: Hook into WAL/CDC for automatic cache invalidation
//! - **Transaction-aware**: Defer invalidation until transactions commit
//! - **LRU-like eviction**: When cache is full, evict oldest entries
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Query Result Cache                           │
//! │  ┌─────────────────────────────────────────────────────────┐    │
//! │  │           DashMap<QueryKey, Arc<CachedResult>>          │    │
//! │  │                                                          │    │
//! │  │  key_1: {result, dependencies, fingerprint, ttl}        │    │
//! │  │  key_2: {result, dependencies, fingerprint, ttl}        │    │
//! │  │  ...                                                     │    │
//! │  └─────────────────────────────────────────────────────────┘    │
//! │                                                                  │
//! │  ┌─────────────────────────────────────────────────────────┐    │
//! │  │      Invalidation Registry (collection -> keys)          │    │
//! │  │                                                          │    │
//! │  │  "products" -> [key_1, key_3, key_7]                    │    │
//! │  │  "users"    -> [key_2, key_5]                           │    │
//! │  └─────────────────────────────────────────────────────────┘    │
//! │                                                                  │
//! │  CacheInvalidator (hooks into WAL/CDC)                          │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ### Basic Caching
//!
//! ```rust,ignore
//! use proximadb::query::cache::{QueryResultCache, QueryKey};
//!
//! // Create cache with default settings
//! let cache = QueryResultCache::with_defaults();
//!
//! // Create a query key
//! let key = QueryKey::from_sql("SELECT * FROM VECTOR_SEARCH('products', '[0.1,0.2]', 10)");
//!
//! // Check cache first
//! if let Some(cached) = cache.get(&key) {
//!     return cached.result.clone();
//! }
//!
//! // Execute query and cache result
//! let result = execute_query(...);
//! cache.insert(key, result, vec!["products".to_string()])?;
//! ```
//!
//! ### With Invalidation
//!
//! ```rust,ignore
//! use proximadb::query::cache::{QueryResultCache, CacheInvalidator, InvalidationEvent, ChangeOperation};
//! use std::sync::Arc;
//!
//! // Create cache and invalidator
//! let cache = Arc::new(QueryResultCache::with_defaults());
//! let invalidator = CacheInvalidator::new(cache.clone());
//!
//! // On data modification
//! fn on_collection_write(collection: &str, op: ChangeOperation, invalidator: &CacheInvalidator) {
//!     let event = InvalidationEvent::new(collection, op);
//!     invalidator.on_change_event(event);
//! }
//! ```
//!
//! ### Transaction-Aware Invalidation
//!
//! ```rust,ignore
//! use proximadb::query::cache::{CacheInvalidator, InvalidationEvent, ChangeOperation, InvalidationConfig};
//!
//! let config = InvalidationConfig {
//!     transaction_aware: true,
//!     ..Default::default()
//! };
//! let invalidator = CacheInvalidator::with_config(cache, config);
//!
//! // During transaction
//! let event = InvalidationEvent::new("products", ChangeOperation::Update)
//!     .with_transaction_id("txn_123");
//! invalidator.on_change_event(event); // Deferred
//!
//! // On commit
//! invalidator.on_transaction_commit("txn_123"); // Now invalidates
//!
//! // On rollback
//! invalidator.on_transaction_rollback("txn_456"); // Discards pending
//! ```
//!
//! ## Performance Benefits
//!
//! | Metric | Without Cache | With Cache (hit) |
//! |--------|---------------|------------------|
//! | Query Parse | ~1ms | 0ms |
//! | Query Optimize | ~2-5ms | 0ms |
//! | Query Execute | varies | 0ms |
//! | Result Serialize | varies | 0ms |
//!
//! For agentic AI workloads with repetitive query patterns, query result caching
//! can provide >80% hit rate and 10-100x speedup for cached queries.
//!
//! ## Cache Hit Rate Targets
//!
//! - **Agentic AI Workloads**: >80% hit rate expected
//! - **Interactive Analytics**: 40-60% hit rate expected
//! - **Real-time OLTP**: <20% hit rate (invalidation-heavy)

pub mod adaptive_cache;
pub mod batch_group;
pub mod invalidation;
pub mod mismatch_cost;
pub mod per_category_policy;
pub mod plan_cache;
pub mod query_result_cache;

// Re-export main types
pub use adaptive_cache::{
    AccessPattern, AdaptiveCacheConfig, AdaptiveCacheEntry, AdaptiveQueryCache, CacheStats,
};
pub use query_result_cache::{
    CachedResult, QueryCacheError, QueryCacheKey, QueryCacheResult, QueryCacheStats, QueryKey,
    QueryResultCache, QueryResultCacheConfig,
};

pub use invalidation::{
    BroadcastInvalidator, CacheInvalidator, ChangeOperation, InvalidationConfig, InvalidationEvent,
    InvalidationListener, InvalidationStats, InvalidationStatsSnapshot,
};
