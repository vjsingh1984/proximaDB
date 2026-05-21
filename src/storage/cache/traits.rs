//! Core traits for cache implementations
//!
//! This module defines the foundational traits used by all cache implementations
//! in ProximaDB, including key/value constraints and the base cache interface.

use async_trait::async_trait;
use std::fmt::Debug;
use std::hash::Hash;
use std::time::SystemTime;

use crate::storage::cache::backend::CacheTier;
use crate::storage::traits::UnifiedMetricsCollector;

/// Base trait for cache keys
///
/// Any type used as a cache key must implement this trait, which requires
/// standard traits for hashing, equality, cloning, and thread safety.
pub trait CacheKey: Hash + Eq + Clone + Send + Sync + Debug + 'static {}

// Implement CacheKey for common types
impl CacheKey for String {}
impl CacheKey for u64 {}

/// Tenant-scoped cache key — enforces the LLD's multi-tenant cache isolation
/// invariant: every cache entry must be addressable only with the tenant +
/// collection that produced it.
///
/// Caches that hold tenant-derived data (result, vector, plan, hot block,
/// batch group — see LLD §6) MUST key on this wrapper rather than on the raw
/// inner key. Caches that hold only tenant-agnostic global data (e.g. the
/// embedding-model byte cache) are exempt.
///
/// # Why a wrapper rather than rewriting every call site
///
/// `Hash`/`Eq` derive over the prefix tuple makes "no tenant prefix" a type
/// error rather than a code-review hazard. Existing callers migrate at their
/// own pace; new code must use this type.
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct TenantScopedKey<K>
where
    K: CacheKey,
{
    /// Tenant identifier — required, never empty.
    pub tenant_id: String,
    /// Collection name within the tenant's namespace — required.
    pub collection: String,
    /// Inner key (query digest, block id, vector id, etc.).
    pub key: K,
}

impl<K: CacheKey> TenantScopedKey<K> {
    /// Build a tenant-scoped cache key. Empty tenant_id or collection name is
    /// rejected at construction so a misuse fails fast instead of silently
    /// producing a cross-tenant key.
    pub fn new(tenant_id: impl Into<String>, collection: impl Into<String>, key: K) -> Self {
        let tenant_id = tenant_id.into();
        let collection = collection.into();
        debug_assert!(!tenant_id.is_empty(), "tenant_id must not be empty");
        debug_assert!(!collection.is_empty(), "collection must not be empty");
        Self {
            tenant_id,
            collection,
            key,
        }
    }
}

impl<K: CacheKey> CacheKey for TenantScopedKey<K> {}

#[cfg(test)]
mod tenant_scoped_key_tests {
    use super::*;

    #[test]
    fn different_tenants_produce_distinct_keys() {
        let a = TenantScopedKey::new("tenant-a", "knowledge", "q-1".to_string());
        let b = TenantScopedKey::new("tenant-b", "knowledge", "q-1".to_string());
        assert_ne!(a, b);
        // And they hash differently — guarded so cross-tenant cache collisions
        // are impossible by construction.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);
        assert_ne!(ha.finish(), hb.finish());
    }

    #[test]
    fn different_collections_in_same_tenant_are_distinct() {
        let a = TenantScopedKey::new("t", "kb-a", 7u64);
        let b = TenantScopedKey::new("t", "kb-b", 7u64);
        assert_ne!(a, b);
    }

    #[test]
    #[should_panic(expected = "tenant_id must not be empty")]
    fn empty_tenant_panics_in_debug() {
        // Debug-only assertion; in release builds the key would be malformed
        // but still typed, so the higher-level audit guardrail (CDC sink) is
        // the second line of defense.
        let _ = TenantScopedKey::new("", "kb", "k".to_string());
    }
}

/// Base trait for cache values
///
/// Any type stored in the cache must implement this trait, which requires
/// the ability to estimate its size in bytes for memory management.
pub trait CacheValue: Clone + Send + Sync + Debug + 'static {
    /// Get the approximate size in bytes
    ///
    /// This is used for cache capacity management and eviction decisions.
    /// The implementation should provide a reasonable approximation of
    /// the memory footprint of the value.
    fn size_bytes(&self) -> usize;
}

/// Base cache trait with template methods for all cache implementations
///
/// This trait provides a complete cache implementation with tiered storage support.
/// It uses the template method pattern to define the algorithm structure while
/// allowing subclasses to customize specific behaviors through hooks.
///
/// # Type Parameters
///
/// * `Key` - The cache key type (must implement `CacheKey`)
/// * `Value` - The cache value type (must implement `CacheValue`)
///
/// # Template Methods
///
/// The trait provides default implementations for:
/// - `get_with_hooks()` - Retrieves values with automatic promotion
/// - `put_with_hooks()` - Stores values with automatic tier selection
/// - `invalidate()` - Removes values from all tiers
///
/// # Hooks
///
/// Subclasses can override these hooks to customize behavior:
/// - `pre_get_hook()` - Called before get operations
/// - `post_miss_hook()` - Called after a cache miss
/// - `pre_put_hook()` - Called before put operations
/// - `post_put_hook()` - Called after put operations
/// - `post_invalidate_hook()` - Called after invalidation
#[async_trait]
pub trait BaseCache: Send + Sync {
    /// Cache key type
    type Key: CacheKey;
    /// Cache value type
    type Value: CacheValue;

    /// Template method - defines algorithm structure for cache retrieval
    ///
    /// Checks each cache tier in order (L1 → L2 → L3), automatically promoting
    /// values to faster tiers when they're found in slower tiers.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to look up
    ///
    /// # Returns
    ///
    /// `Some(value)` if found in any tier, `None` otherwise
    async fn get_with_hooks(&self, key: &Self::Key) -> Option<Self::Value> {
        // Pre-get hook for custom logic
        self.pre_get_hook(key).await;

        // Check each tier in order
        if let Some(value) = self.check_l1(key).await {
            self.record_hit(CacheTier::L1).await;
            return Some(value);
        }

        if let Some(value) = self.check_l2(key).await {
            // Promote to L1 for faster future access
            self.promote_to_l1(key, &value).await;
            self.record_hit(CacheTier::L2).await;
            return Some(value);
        }

        if let Some(value) = self.check_l3(key).await {
            // Promote to L2 (and potentially L1)
            self.promote_to_l2(key, &value).await;
            self.record_hit(CacheTier::L3).await;
            return Some(value);
        }

        self.record_miss().await;
        self.post_miss_hook(key).await;
        None
    }

    /// Put value into cache with automatic tier placement
    ///
    /// Determines the appropriate tier based on value size and access patterns,
    /// then stores the value in that tier.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to store
    /// * `value` - The value to store
    async fn put_with_hooks(&self, key: Self::Key, value: Self::Value) {
        self.pre_put_hook(&key, &value).await;

        // Determine appropriate tier based on value size and access patterns
        let tier = self.select_tier(&key, &value).await;

        match tier {
            CacheTier::L1 => self.put_l1(key.clone(), value.clone()).await,
            CacheTier::L2 => self.put_l2(key.clone(), value.clone()).await,
            CacheTier::L3 => self.put_l3(key.clone(), value.clone()).await,
        }

        self.post_put_hook(&key, &value).await;
    }

    /// Invalidate a cache entry across all tiers
    ///
    /// Removes the entry from L1, L2, and L3 tiers if present.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to invalidate
    ///
    /// # Returns
    ///
    /// `true` if the entry was found in any tier, `false` otherwise
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
    /// Called before get operations (override for custom behavior)
    async fn pre_get_hook(&self, _key: &Self::Key) {}
    /// Called after a cache miss (override for custom behavior)
    async fn post_miss_hook(&self, _key: &Self::Key) {}
    /// Called before put operations (override for custom behavior)
    async fn pre_put_hook(&self, _key: &Self::Key, _value: &Self::Value) {}
    /// Called after put operations (override for custom behavior)
    async fn post_put_hook(&self, _key: &Self::Key, _value: &Self::Value) {}
    /// Called after invalidation (override for custom behavior)
    async fn post_invalidate_hook(&self, _key: &Self::Key) {}

    // Tier-specific operations - must be implemented
    /// Check L1 (memory) cache for a key
    async fn check_l1(&self, key: &Self::Key) -> Option<Self::Value>;
    /// Check L2 (NVMe/SSD) cache for a key
    async fn check_l2(&self, key: &Self::Key) -> Option<Self::Value>;
    /// Check L3 (network) cache for a key
    async fn check_l3(&self, key: &Self::Key) -> Option<Self::Value>;

    /// Put a value into L1 (memory) cache
    async fn put_l1(&self, key: Self::Key, value: Self::Value);
    /// Put a value into L2 (NVMe/SSD) cache
    async fn put_l2(&self, key: Self::Key, value: Self::Value);
    /// Put a value into L3 (network) cache
    async fn put_l3(&self, key: Self::Key, value: Self::Value);

    /// Invalidate a key from L1 (memory) cache
    async fn invalidate_l1(&self, key: &Self::Key) -> bool;
    /// Invalidate a key from L2 (NVMe/SSD) cache
    async fn invalidate_l2(&self, key: &Self::Key) -> bool;
    /// Invalidate a key from L3 (network) cache
    async fn invalidate_l3(&self, key: &Self::Key) -> bool;

    /// Promote a value to L1 (memory) cache
    async fn promote_to_l1(&self, key: &Self::Key, value: &Self::Value);
    /// Promote a value to L2 (NVMe/SSD) cache
    async fn promote_to_l2(&self, key: &Self::Key, value: &Self::Value);

    /// Select the appropriate tier for a key-value pair
    async fn select_tier(&self, key: &Self::Key, value: &Self::Value) -> CacheTier;

    // Metrics operations
    /// Record a cache hit for metrics tracking
    async fn record_hit(&self, _tier: CacheTier) {
        // Record cache hit using unified metrics
        self.metrics()
            .record_operation(
                crate::storage::traits::MetricsOperationType::CacheHit,
                true, // success
                0,    // bytes
                std::time::Duration::from_millis(0),
            )
            .await;
    }

    /// Record a cache miss for metrics tracking
    async fn record_miss(&self) {
        // Record cache miss using unified metrics
        self.metrics()
            .record_operation(
                crate::storage::traits::MetricsOperationType::CacheMiss,
                true, // success (miss is a valid outcome, not a failure)
                0,    // bytes
                std::time::Duration::from_millis(0),
            )
            .await;
    }

    /// Get the metrics collector for this cache
    fn metrics(&self) -> &UnifiedMetricsCollector;
}

/// Cache entry with metadata
///
/// Wraps a cached value with metadata for tracking access patterns,
/// which is used for eviction decisions and performance optimization.
///
/// # Type Parameters
///
/// * `V` - The cached value type (must implement `CacheValue`)
#[derive(Debug, Clone)]
pub struct CacheEntry<V: CacheValue> {
    /// The cached value
    pub value: V,
    /// When this entry was inserted into the cache
    pub inserted_at: SystemTime,
    /// When this entry was last accessed
    pub last_accessed: SystemTime,
    /// Number of times this entry has been accessed
    pub access_count: u64,
    /// Size of the value in bytes
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
