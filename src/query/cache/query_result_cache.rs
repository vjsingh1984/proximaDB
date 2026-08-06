//! # Query Result Cache Implementation
//!
//! Core types and cache implementation for query result caching. The cache is
//! generic over the cached result type `T` (any [`CacheableResult`]) so the
//! same machinery serves both the federated query path (caching
//! `ExecutionResult`) and the pgwire OLAP path (caching
//! `ExecutionPipelineResult`).
//!
//! ## Multi-tenant structural keying
//!
//! The cache is keyed STRUCTURALLY on `(tenant, namespace, query)` — tenant is
//! a *leading* key component, never a predicate on the value (multi-tenant
//! co-design mandate). [`StructuralKey`] folds all three into a
//! [`CompositeMapKey`] used as the DashMap key, and every stored
//! [`CachedResult`] additionally records the full `(tenant, namespace,
//! query_fingerprint)` triple which is re-verified on lookup. A cross-tenant
//! collision would require a simultaneous 192-bit hash collision; the
//! stored-triple check defends against it regardless.
//!
//! ## Read-after-write freshness
//!
//! [`QueryResultCache::get_fresh`] gates serving on [`VectorFreshnessMode`]:
//!
//! - **Strong** (ADR-051 D2): serve iff `computed_at_lsn == Some(current_lsn)`
//!   and `current_lsn != 0`. Any write bumps the canonical-WAL LSN → guaranteed
//!   miss → read-after-write correct (mandate #16c).
//! - **BoundedStale**: serve iff `age <= max_staleness_ms`.
//! - **StaleOk**: serve iff not TTL-expired.
//!
//! TTL is the universal entry lifetime: an entry older than its TTL is dead for
//! every mode and is evicted. See [`freshness_eligible`].

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use thiserror::Error;
use tracing::{debug, info};

use crate::core::search::VectorFreshnessMode;
use crate::query::federated::ExecutionResult;

/// Unique identifier for a cached query result
pub type QueryCacheKey = u64;

/// Error types for query cache operations
#[derive(Debug, Error)]
pub enum QueryCacheError {
    /// Query result not found in cache
    #[error("Query result not found: {0}")]
    NotFound(QueryCacheKey),

    /// Cache entry has expired
    #[error("Query result has expired: {0}")]
    Expired(QueryCacheKey),

    /// Cache is full and cannot accept new entries
    #[error("Cache is full (max: {0})")]
    CacheFull(usize),

    /// Failed to compute query key
    #[error("Failed to compute query fingerprint: {0}")]
    FingerprintError(String),

    /// Internal cache error
    #[error("Internal cache error: {0}")]
    Internal(String),
}

/// Result type for query cache operations
pub type QueryCacheResult<T> = Result<T, QueryCacheError>;

/// What the cache requires of a cached result type so it can estimate the
/// in-memory footprint of an entry (for size limits / eviction). Implemented
/// by the concrete result types that instantiate [`QueryResultCache`].
pub trait CacheableResult: Send + Sync {
    /// Rough estimated size of this result in bytes.
    fn estimated_size_bytes(&self) -> usize;
}

impl CacheableResult for ExecutionResult {
    fn estimated_size_bytes(&self) -> usize {
        // Rough estimation based on row count and schema width — same heuristic
        // the old hardcoded estimator used.
        let row_count = self.row_count();
        let field_count = self.schema.fields().len();
        // Assume average of 100 bytes per field per row.
        let estimated_data = row_count * field_count * 100;
        // Add overhead for schema and metadata.
        let schema_overhead = field_count * 64;
        estimated_data + schema_overhead + 256 // Base overhead
    }
}

/// Configuration for the query result cache
#[derive(Debug, Clone)]
pub struct QueryResultCacheConfig {
    /// Maximum number of cached results (default: 10000)
    pub max_entries: usize,
    /// Default TTL for cached results (default: 5 minutes)
    pub default_ttl: Duration,
    /// Enable automatic cleanup of expired entries (default: true)
    pub enable_cleanup: bool,
    /// Cleanup interval (default: 1 minute)
    pub cleanup_interval: Duration,
    /// Maximum size per cached result in bytes (default: 10MB)
    pub max_result_size_bytes: usize,
    /// Enable cache hit/miss metrics (default: true)
    pub enable_metrics: bool,
}

impl Default for QueryResultCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            default_ttl: Duration::from_secs(300), // 5 minutes
            enable_cleanup: true,
            cleanup_interval: Duration::from_secs(60), // 1 minute
            max_result_size_bytes: 10 * 1024 * 1024,   // 10MB
            enable_metrics: true,
        }
    }
}

/// A key that uniquely identifies a query for caching purposes
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QueryKey {
    /// The computed fingerprint of the query
    pub fingerprint: u64,
    /// Original SQL or query string (for debugging)
    pub query_string: String,
}

impl QueryKey {
    /// Create a new query key from a SQL string
    pub fn from_sql(sql: &str) -> Self {
        let fingerprint = Self::compute_fingerprint(sql);
        Self {
            fingerprint,
            query_string: sql.to_string(),
        }
    }

    /// Create a new query key from a SQL string with parameters
    pub fn from_sql_with_params(sql: &str, params: &[&str]) -> Self {
        let mut combined = sql.to_string();
        for param in params {
            combined.push('\0'); // Use null separator
            combined.push_str(param);
        }
        let fingerprint = Self::compute_fingerprint(&combined);
        Self {
            fingerprint,
            query_string: sql.to_string(),
        }
    }

    /// Compute a stable fingerprint using DefaultHasher
    fn compute_fingerprint(input: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        hasher.finish()
    }

    /// Get the cache key for DashMap lookup
    pub fn cache_key(&self) -> QueryCacheKey {
        self.fingerprint
    }
}

/// Composite DashMap key folding tenant + namespace + query fingerprint.
/// Deriving `Hash`/`Eq` over all three makes tenant a STRUCTURAL key
/// component (not a predicate on the value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompositeMapKey {
    pub tenant_hash: u64,
    pub ns_hash: u64,
    pub query_hash: u64,
}

/// Structural cache key: tenant and namespace are the LEADING components; the
/// query is last. This is the multi-tenant isolation surface — tenant is part
/// of the key, never a predicate.
#[derive(Debug, Clone)]
pub struct StructuralKey {
    /// Tenant identifier (structural leading component).
    pub tenant: String,
    /// Namespace / schema identifier (structural component).
    pub namespace: String,
    /// The query fingerprint + SQL.
    pub query: QueryKey,
}

impl StructuralKey {
    /// Build a structural key from its three components.
    pub fn new(tenant: impl Into<String>, namespace: impl Into<String>, query: QueryKey) -> Self {
        Self {
            tenant: tenant.into(),
            namespace: namespace.into(),
            query,
        }
    }

    fn hash_str(s: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }

    /// Fold the three components into the DashMap composite key.
    pub fn composite(&self) -> CompositeMapKey {
        CompositeMapKey {
            tenant_hash: Self::hash_str(&self.tenant),
            ns_hash: Self::hash_str(&self.namespace),
            query_hash: self.query.fingerprint,
        }
    }
}

/// A cached query result with metadata
#[derive(Debug)]
pub struct CachedResult<T> {
    /// The cached execution result
    pub result: T,
    /// Collections/tables that this result depends on (normalized table keys)
    pub dependencies: Vec<String>,
    /// Query fingerprint for verification
    pub query_fingerprint: u64,
    /// Tenant this entry belongs to (structural isolation re-check).
    pub tenant: String,
    /// Namespace this entry belongs to (structural isolation re-check).
    pub namespace: String,
    /// Time-to-live for this entry
    pub ttl: Duration,
    /// Creation timestamp
    pub created_at: Instant,
    /// Last access timestamp
    pub last_accessed: Instant,
    /// Number of times this result was accessed
    pub access_count: AtomicU64,
    /// Estimated size in bytes
    pub size_bytes: usize,
    /// Canonical-WAL LSN the result was computed at — the Strong-freshness
    /// anchor (ADR-051 D2). `None` = LSN not tracked (never served to Strong).
    pub computed_at_lsn: Option<u64>,
}

impl<T> CachedResult<T> {
    /// Check if this cached result has expired (age > TTL).
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }

    /// Get the age of this cached result
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Get the access count
    pub fn get_access_count(&self) -> u64 {
        self.access_count.load(Ordering::Relaxed)
    }
}

/// Decide whether a cached entry is eligible to serve under the given
/// freshness mode. Pure + deterministic so it is unit-tested directly.
///
/// Policy (ADR-051 D2 / mandate #16c):
/// - TTL is the universal entry lifetime — an expired entry is never eligible.
/// - **Strong**: eligible iff `computed_at_lsn == Some(current_lsn)` and
///   `current_lsn != 0`. Any write bumps the LSN → guaranteed miss.
/// - **BoundedStale**: eligible iff `age <= max_staleness_ms`.
/// - **StaleOk**: eligible (within TTL, already checked above).
pub fn freshness_eligible(
    mode: &VectorFreshnessMode,
    age: Duration,
    ttl: Duration,
    computed_at_lsn: Option<u64>,
    current_lsn: u64,
) -> bool {
    // Universal expiry: an entry older than its TTL is dead for every mode.
    if age > ttl {
        return false;
    }
    match mode {
        VectorFreshnessMode::Strong => current_lsn != 0 && computed_at_lsn == Some(current_lsn),
        VectorFreshnessMode::BoundedStale { max_staleness_ms } => {
            age <= Duration::from_millis(*max_staleness_ms)
        }
        VectorFreshnessMode::StaleOk => true,
    }
}

/// Thread-safe cache for query results, generic over the result type.
///
/// This cache provides high-performance caching of query results with:
///
/// - Structural `(tenant, namespace, query)` keying for multi-tenant isolation
/// - TTL-based expiration + LRU-like eviction
/// - Tenant-scoped dependency tracking for invalidation
/// - Freshness gating via [`VectorFreshnessMode`] (Strong LSN-pinning)
pub struct QueryResultCache<T: Clone + CacheableResult + Send + Sync + 'static> {
    /// The result cache using DashMap for concurrent access
    cache: DashMap<CompositeMapKey, Arc<CachedResult<T>>>,
    /// Registry mapping `(tenant, collection)` to affected cache keys.
    /// Tenant-scoped so invalidating `(tenant-A, coll)` never touches
    /// tenant-B's entries.
    invalidation_registry: DashMap<(String, String), HashSet<CompositeMapKey>>,
    /// Configuration
    config: QueryResultCacheConfig,
    /// Cache statistics
    stats: QueryResultCacheStatistics,
}

/// Statistics for cache monitoring
#[derive(Debug, Default)]
pub struct QueryResultCacheStatistics {
    /// Total cache hits
    pub hits: AtomicU64,
    /// Total cache misses
    pub misses: AtomicU64,
    /// Total entries inserted
    pub inserts: AtomicU64,
    /// Total entries evicted
    pub evictions: AtomicU64,
    /// Total entries invalidated
    pub invalidations: AtomicU64,
    /// Total entries expired
    pub expirations: AtomicU64,
}

impl QueryResultCacheStatistics {
    /// Get the cache hit rate (0.0 to 1.0)
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }
}

impl<T: Clone + CacheableResult + Send + Sync + 'static> QueryResultCache<T> {
    /// Create a new query result cache with the given configuration
    pub fn new(config: QueryResultCacheConfig) -> Self {
        Self {
            cache: DashMap::new(),
            invalidation_registry: DashMap::new(),
            config,
            stats: QueryResultCacheStatistics::default(),
        }
    }

    /// Create a new cache with default configuration
    pub fn with_defaults() -> Self {
        Self::new(QueryResultCacheConfig::default())
    }

    /// Freshness-gated lookup honoring [`VectorFreshnessMode`] (ADR-051 D2 /
    /// mandate #16c). Returns the cached entry only when the structural key
    /// matches AND the entry is fresh enough for the requested mode.
    pub fn get_fresh(
        &self,
        key: &StructuralKey,
        mode: &VectorFreshnessMode,
        current_lsn: u64,
    ) -> Option<Arc<CachedResult<T>>> {
        let map_key = key.composite();

        if let Some(entry) = self.cache.get(&map_key) {
            // Check expiration first.
            if entry.is_expired() {
                drop(entry);
                self.remove_entry(map_key);
                self.stats.expirations.fetch_add(1, Ordering::Relaxed);
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            // Structural isolation re-check: tenant + namespace + query must
            // ALL match the stored entry. Defends against a composite-key
            // collision and documents the cross-tenant guarantee explicitly.
            if entry.tenant != key.tenant
                || entry.namespace != key.namespace
                || entry.query_fingerprint != key.query.fingerprint
            {
                debug!(
                    expected_tenant = %key.tenant,
                    stored_tenant = %entry.tenant,
                    "Cache structural-key mismatch (cross-tenant guard)"
                );
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            // Freshness gate.
            if !freshness_eligible(
                mode,
                entry.age(),
                entry.ttl,
                entry.computed_at_lsn,
                current_lsn,
            ) {
                self.stats.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            let result = Arc::clone(&*entry);
            drop(entry);

            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some(result)
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Insert a query result with a structural key + the LSN it was computed at
    /// (the Strong-freshness anchor). Dependencies are registered under
    /// `(key.tenant, dep)` for tenant-scoped invalidation.
    pub fn insert_fresh(
        &self,
        key: StructuralKey,
        result: T,
        dependencies: Vec<String>,
        computed_at_lsn: Option<u64>,
    ) -> QueryCacheResult<()> {
        self.insert_entry(
            key,
            result,
            dependencies,
            self.config.default_ttl,
            computed_at_lsn,
        )
    }

    /// Core insert worker shared by the freshness-aware and legacy APIs.
    fn insert_entry(
        &self,
        key: StructuralKey,
        result: T,
        dependencies: Vec<String>,
        ttl: Duration,
        computed_at_lsn: Option<u64>,
    ) -> QueryCacheResult<()> {
        // Estimate result size
        let size_bytes = result.estimated_size_bytes();

        // Check size limit
        if size_bytes > self.config.max_result_size_bytes {
            debug!(
                size = size_bytes,
                max = self.config.max_result_size_bytes,
                "Query result too large to cache"
            );
            return Ok(()); // Don't cache, but not an error
        }

        // Check cache capacity
        if self.cache.len() >= self.config.max_entries {
            // Try to evict expired entries first
            let expired_count = self.cleanup_expired();

            // If still full, evict oldest entries
            if self.cache.len() >= self.config.max_entries {
                let evicted = self.evict_oldest(1);
                if evicted == 0 {
                    return Err(QueryCacheError::CacheFull(self.config.max_entries));
                }
            }

            if expired_count > 0 {
                debug!(
                    expired = expired_count,
                    "Evicted expired entries to make room"
                );
            }
        }

        let map_key = key.composite();
        let now = Instant::now();

        let cached = Arc::new(CachedResult {
            result,
            dependencies: dependencies.clone(),
            query_fingerprint: key.query.fingerprint,
            tenant: key.tenant.clone(),
            namespace: key.namespace.clone(),
            ttl,
            created_at: now,
            last_accessed: now,
            access_count: AtomicU64::new(0),
            size_bytes,
            computed_at_lsn,
        });

        // Insert into cache
        self.cache.insert(map_key, cached);

        // Register dependencies for tenant-scoped invalidation
        for dep in dependencies {
            self.invalidation_registry
                .entry((key.tenant.clone(), dep))
                .or_default()
                .insert(map_key);
        }

        self.stats.inserts.fetch_add(1, Ordering::Relaxed);

        debug!(
            key = ?map_key,
            ttl_secs = ttl.as_secs(),
            size_bytes,
            "Cached query result"
        );

        Ok(())
    }

    // ---------------------------------------------------------------------
    // Backward-compatible legacy API (no tenant, no freshness).
    //
    // These delegate to the structural/freshness API with `tenant=""`,
    // `namespace=""`, `computed_at_lsn=None`, and `StaleOk` eligibility,
    // preserving the original TTL-only semantics. Kept so the federated
    // query path and its existing tests compile unchanged.
    // ---------------------------------------------------------------------

    /// Legacy TTL-only lookup (no tenant context). Delegates with the empty
    /// tenant and `StaleOk` eligibility.
    pub fn get(&self, key: &QueryKey) -> Option<Arc<CachedResult<T>>> {
        let skey = StructuralKey::new("", "", key.clone());
        self.get_fresh(&skey, &VectorFreshnessMode::StaleOk, 0)
    }

    /// Insert a query result into the cache (legacy, no tenant context).
    pub fn insert(
        &self,
        key: QueryKey,
        result: T,
        dependencies: Vec<String>,
    ) -> QueryCacheResult<()> {
        self.insert_with_ttl(key, result, dependencies, self.config.default_ttl)
    }

    /// Insert a query result with a custom TTL (legacy, no tenant context).
    pub fn insert_with_ttl(
        &self,
        key: QueryKey,
        result: T,
        dependencies: Vec<String>,
        ttl: Duration,
    ) -> QueryCacheResult<()> {
        let skey = StructuralKey::new("", "", key);
        self.insert_entry(skey, result, dependencies, ttl, None)
    }

    /// Remove a cached entry by key (legacy).
    pub fn remove(&self, key: &QueryKey) -> bool {
        let skey = StructuralKey::new("", "", key.clone());
        self.remove_entry(skey.composite())
    }

    /// Tenant-scoped invalidation: drop every entry registered under
    /// `(tenant, collection)`. Never touches another tenant's entries.
    pub fn invalidate_tenant_collection(&self, tenant: &str, collection: &str) -> usize {
        let keys_to_remove: Vec<CompositeMapKey> = self
            .invalidation_registry
            .get(&(tenant.to_string(), collection.to_string()))
            .map(|keys| keys.iter().copied().collect())
            .unwrap_or_default();

        let count = keys_to_remove.len();

        for key in keys_to_remove {
            self.remove_entry(key);
        }

        // Clean up the registry entry
        self.invalidation_registry
            .remove(&(tenant.to_string(), collection.to_string()));

        if count > 0 {
            self.stats
                .invalidations
                .fetch_add(count as u64, Ordering::Relaxed);
            info!(
                tenant,
                collection,
                invalidated = count,
                "Invalidated cached query results"
            );
        }

        count
    }

    /// Legacy: invalidate by collection name under the empty tenant.
    pub fn invalidate_collection(&self, collection: &str) -> usize {
        self.invalidate_tenant_collection("", collection)
    }

    /// Invalidate cached results for any of the given collections (legacy,
    /// empty-tenant scope).
    pub fn invalidate_collections(&self, collections: &[&str]) -> usize {
        let mut total = 0;
        for collection in collections {
            total += self.invalidate_collection(collection);
        }
        total
    }

    /// Internal method to remove an entry and clean up invalidation registry
    fn remove_entry(&self, map_key: CompositeMapKey) -> bool {
        if let Some((_, cached)) = self.cache.remove(&map_key) {
            // Remove from invalidation registry (tenant-scoped)
            for dep in &cached.dependencies {
                if let Some(mut keys) = self
                    .invalidation_registry
                    .get_mut(&(cached.tenant.clone(), dep.clone()))
                {
                    keys.remove(&map_key);
                }
            }
            true
        } else {
            false
        }
    }

    /// Cleanup expired entries
    pub fn cleanup_expired(&self) -> usize {
        let expired_keys: Vec<CompositeMapKey> = self
            .cache
            .iter()
            .filter(|entry| entry.value().is_expired())
            .map(|entry| *entry.key())
            .collect();

        let count = expired_keys.len();

        for key in expired_keys {
            self.remove_entry(key);
        }

        if count > 0 {
            self.stats
                .expirations
                .fetch_add(count as u64, Ordering::Relaxed);
            debug!(expired = count, "Cleaned up expired cache entries");
        }

        count
    }

    /// Evict the oldest entries to make room
    fn evict_oldest(&self, count: usize) -> usize {
        // Collect entries with their creation times
        let mut entries: Vec<(CompositeMapKey, Instant)> = self
            .cache
            .iter()
            .map(|entry| (*entry.key(), entry.value().created_at))
            .collect();

        // Sort by age (oldest first)
        entries.sort_by_key(|e| e.1);

        let to_evict = entries
            .into_iter()
            .take(count)
            .map(|(k, _)| k)
            .collect::<Vec<_>>();
        let evicted = to_evict.len();

        for key in to_evict {
            self.remove_entry(key);
        }

        if evicted > 0 {
            self.stats
                .evictions
                .fetch_add(evicted as u64, Ordering::Relaxed);
            debug!(evicted, "Evicted oldest cache entries");
        }

        evicted
    }

    /// Clear all cached entries
    pub fn clear(&self) {
        let count = self.cache.len();
        self.cache.clear();
        self.invalidation_registry.clear();

        if count > 0 {
            info!(cleared = count, "Cleared query result cache");
        }
    }

    /// Get the number of cached entries
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Check if a query is cached (legacy, empty-tenant scope)
    pub fn contains(&self, key: &QueryKey) -> bool {
        let skey = StructuralKey::new("", "", key.clone());
        let map_key = skey.composite();
        if let Some(entry) = self.cache.get(&map_key) {
            !entry.is_expired()
                && entry.tenant.is_empty()
                && entry.namespace.is_empty()
                && entry.query_fingerprint == key.fingerprint
        } else {
            false
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> QueryCacheStats {
        QueryCacheStats {
            entries: self.cache.len(),
            max_entries: self.config.max_entries,
            hits: self.stats.hits.load(Ordering::Relaxed),
            misses: self.stats.misses.load(Ordering::Relaxed),
            hit_rate: self.stats.hit_rate(),
            inserts: self.stats.inserts.load(Ordering::Relaxed),
            evictions: self.stats.evictions.load(Ordering::Relaxed),
            invalidations: self.stats.invalidations.load(Ordering::Relaxed),
            expirations: self.stats.expirations.load(Ordering::Relaxed),
            total_size_bytes: self.total_size_bytes(),
            tracked_collections: self.invalidation_registry.len(),
        }
    }

    /// Get total size of all cached entries
    fn total_size_bytes(&self) -> usize {
        self.cache
            .iter()
            .map(|entry| entry.value().size_bytes)
            .sum()
    }

    /// Get configuration
    pub fn config(&self) -> &QueryResultCacheConfig {
        &self.config
    }
}

impl<T: Clone + CacheableResult + Send + Sync + 'static> Default for QueryResultCache<T> {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Alias for the cache instantiation used by the federated query path.
/// Letting `CacheInvalidator` / `FederatedQueryContext` reference the alias
/// keeps the generic-ization from rippling through every call site.
pub type FederatedQueryResultCache = QueryResultCache<ExecutionResult>;

/// Public cache statistics.
///
/// Part of the external API surface — appears in query-observability
/// REST/gRPC responses. Do NOT consolidate with
/// `proximadb_runtime_common::cache::CacheStats` without bumping the
/// public API version.
#[derive(Debug, Clone)]
pub struct QueryCacheStats {
    /// Number of cached entries
    pub entries: usize,
    /// Maximum allowed entries
    pub max_entries: usize,
    /// Total cache hits
    pub hits: u64,
    /// Total cache misses
    pub misses: u64,
    /// Cache hit rate (0.0 to 1.0)
    pub hit_rate: f64,
    /// Total inserts
    pub inserts: u64,
    /// Total evictions
    pub evictions: u64,
    /// Total invalidations
    pub invalidations: u64,
    /// Total expirations
    pub expirations: u64,
    /// Total size of cached data in bytes
    pub total_size_bytes: usize,
    /// Number of tracked (tenant, collection) pairs for invalidation
    pub tracked_collections: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, RecordBatch, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn create_test_result() -> ExecutionResult {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, true),
        ]));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["1", "2"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
            ],
        )
        .expect("Failed to create test RecordBatch");

        ExecutionResult::from_batch(batch)
    }

    #[test]
    fn test_query_key_creation() {
        let key1 = QueryKey::from_sql("SELECT * FROM test");
        let key2 = QueryKey::from_sql("SELECT * FROM test");
        let key3 = QueryKey::from_sql("SELECT * FROM other");

        assert_eq!(key1.fingerprint, key2.fingerprint);
        assert_ne!(key1.fingerprint, key3.fingerprint);
    }

    #[test]
    fn test_query_key_with_params() {
        let key1 = QueryKey::from_sql_with_params("SELECT * FROM $1", &["test"]);
        let key2 = QueryKey::from_sql_with_params("SELECT * FROM $1", &["test"]);
        let key3 = QueryKey::from_sql_with_params("SELECT * FROM $1", &["other"]);

        assert_eq!(key1.fingerprint, key2.fingerprint);
        assert_ne!(key1.fingerprint, key3.fingerprint);
    }

    #[test]
    fn test_cache_insert_and_get() {
        let cache = QueryResultCache::with_defaults();
        let key = QueryKey::from_sql("SELECT * FROM test");
        let result = create_test_result();

        cache
            .insert(key.clone(), result, vec!["test".to_string()])
            .expect("Failed to insert test result into cache");

        assert!(cache.contains(&key));
        assert_eq!(cache.len(), 1);

        let cached = cache.get(&key);
        assert!(cached.is_some());
        let cached_result = cached.expect("Cached result should exist");
        assert_eq!(cached_result.result.row_count(), 2);
    }

    #[test]
    fn test_cache_miss() {
        // Type-annotate via the alias so the bare `with_defaults()` (no insert
        // to infer `T`) resolves to the federated result type.
        let cache = FederatedQueryResultCache::with_defaults();
        let key = QueryKey::from_sql("SELECT * FROM nonexistent");

        assert!(!cache.contains(&key));
        assert!(cache.get(&key).is_none());

        let stats = cache.stats();
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn test_cache_invalidation() {
        let cache = QueryResultCache::with_defaults();

        // Insert entries depending on different collections
        let key1 = QueryKey::from_sql("SELECT * FROM test1");
        let key2 = QueryKey::from_sql("SELECT * FROM test2");
        let key3 = QueryKey::from_sql("SELECT * FROM test1 JOIN test2");

        cache
            .insert(
                key1.clone(),
                create_test_result(),
                vec!["test1".to_string()],
            )
            .expect("Failed to insert test1 result into cache");
        cache
            .insert(
                key2.clone(),
                create_test_result(),
                vec!["test2".to_string()],
            )
            .expect("Failed to insert test2 result into cache");
        cache
            .insert(
                key3.clone(),
                create_test_result(),
                vec!["test1".to_string(), "test2".to_string()],
            )
            .expect("Failed to insert test3 result into cache");

        assert_eq!(cache.len(), 3);

        // Invalidate test1 - should remove key1 and key3
        let invalidated = cache.invalidate_collection("test1");
        assert_eq!(invalidated, 2);
        assert_eq!(cache.len(), 1);
        assert!(!cache.contains(&key1));
        assert!(cache.contains(&key2));
        assert!(!cache.contains(&key3));
    }

    #[test]
    fn test_cache_expiration() {
        let config = QueryResultCacheConfig {
            default_ttl: Duration::from_millis(1),
            ..Default::default()
        };
        let cache = QueryResultCache::new(config);
        let key = QueryKey::from_sql("SELECT * FROM test");

        cache
            .insert(key.clone(), create_test_result(), vec!["test".to_string()])
            .expect("Failed to insert test result into cache");
        assert!(cache.contains(&key));

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(10));

        // Should not be found (expired)
        assert!(cache.get(&key).is_none());

        let stats = cache.stats();
        assert!(stats.expirations > 0 || stats.misses > 0);
    }

    #[test]
    fn test_cache_remove() {
        let cache = QueryResultCache::with_defaults();
        let key = QueryKey::from_sql("SELECT * FROM test");

        cache
            .insert(key.clone(), create_test_result(), vec!["test".to_string()])
            .expect("Failed to insert test result into cache");
        assert_eq!(cache.len(), 1);

        let removed = cache.remove(&key);
        assert!(removed);
        assert_eq!(cache.len(), 0);
        assert!(!cache.contains(&key));
    }

    #[test]
    fn test_cache_clear() {
        let cache = QueryResultCache::with_defaults();

        for i in 0..5 {
            let key = QueryKey::from_sql(&format!("SELECT * FROM test{}", i));
            cache
                .insert(key, create_test_result(), vec![format!("test{}", i)])
                .expect("Failed to insert test result into cache");
        }

        assert_eq!(cache.len(), 5);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_stats() {
        let cache = QueryResultCache::with_defaults();
        let key = QueryKey::from_sql("SELECT * FROM test");

        cache
            .insert(key.clone(), create_test_result(), vec!["test".to_string()])
            .expect("Failed to insert test result into cache");

        // Hit
        let _ = cache.get(&key);
        let _ = cache.get(&key);

        // Miss
        let missing = QueryKey::from_sql("SELECT * FROM missing");
        let _ = cache.get(&missing);

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.inserts, 1);
        assert!((stats.hit_rate - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_cache_capacity_eviction() {
        let config = QueryResultCacheConfig {
            max_entries: 3,
            ..Default::default()
        };
        let cache = QueryResultCache::new(config);

        // Insert 3 entries
        for i in 0..3 {
            let key = QueryKey::from_sql(&format!("SELECT * FROM test{}", i));
            cache
                .insert(key, create_test_result(), vec![format!("test{}", i)])
                .expect("Failed to insert test result into cache");
        }

        assert_eq!(cache.len(), 3);

        // Insert 4th entry - should evict oldest
        let key4 = QueryKey::from_sql("SELECT * FROM test4");
        cache
            .insert(
                key4.clone(),
                create_test_result(),
                vec!["test4".to_string()],
            )
            .expect("Failed to insert test4 result into cache");

        assert_eq!(cache.len(), 3);
        assert!(cache.contains(&key4));
    }

    #[test]
    fn test_cleanup_expired() {
        let config = QueryResultCacheConfig {
            default_ttl: Duration::from_millis(1),
            ..Default::default()
        };
        let cache = QueryResultCache::new(config);

        for i in 0..5 {
            let key = QueryKey::from_sql(&format!("SELECT * FROM test{}", i));
            cache
                .insert(key, create_test_result(), vec![format!("test{}", i)])
                .expect("Failed to insert test result into cache");
        }

        assert_eq!(cache.len(), 5);

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(10));

        let cleaned = cache.cleanup_expired();
        assert_eq!(cleaned, 5);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_multiple_dependencies() {
        let cache = QueryResultCache::with_defaults();

        // Insert with multiple dependencies
        let key = QueryKey::from_sql("SELECT * FROM a JOIN b JOIN c");
        cache
            .insert(
                key.clone(),
                create_test_result(),
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
            )
            .expect("Failed to insert test result with multiple dependencies into cache");

        assert!(cache.contains(&key));

        // Invalidating any dependency should remove the entry
        cache.invalidate_collection("b");
        assert!(!cache.contains(&key));
    }

    // =====================================================================
    // Structural keying + freshness tests (pgwire OLAP wiring, ADR-051 D2).
    // =====================================================================

    fn skey(tenant: &str, namespace: &str, sql: &str) -> StructuralKey {
        StructuralKey::new(tenant, namespace, QueryKey::from_sql(sql))
    }

    #[test]
    fn structural_key_folds_all_three_components() {
        // Same query + namespace, different tenant → different composite.
        let a = skey("tenant-a", "public", "SELECT 1");
        let b = skey("tenant-b", "public", "SELECT 1");
        assert_ne!(a.composite(), b.composite());
        // Same query + tenant, different namespace → different composite.
        let c = skey("tenant-a", "other", "SELECT 1");
        assert_ne!(a.composite(), c.composite());
        // Identical → same composite.
        let a2 = skey("tenant-a", "public", "SELECT 1");
        assert_eq!(a.composite(), a2.composite());
    }

    #[test]
    fn cross_tenant_key_isolation() {
        // Tenant A's write must NEVER serve tenant B, even with identical SQL
        // and namespace. Insert under A, lookup under B → miss.
        let cache = QueryResultCache::with_defaults();
        let key_a = skey("tenant-a", "public", "SELECT * FROM orders");
        let key_b = skey("tenant-b", "public", "SELECT * FROM orders");

        cache
            .insert_fresh(
                key_a.clone(),
                create_test_result(),
                vec!["orders".to_string()],
                Some(42),
            )
            .expect("insert");

        // A hits (Strong at the pinned LSN).
        assert!(
            cache
                .get_fresh(&key_a, &VectorFreshnessMode::Strong, 42)
                .is_some()
        );
        // B never hits A's entry — Strong or otherwise.
        assert!(
            cache
                .get_fresh(&key_b, &VectorFreshnessMode::Strong, 42)
                .is_none()
        );
        assert!(
            cache
                .get_fresh(&key_b, &VectorFreshnessMode::StaleOk, 0)
                .is_none()
        );
    }

    #[test]
    fn cross_tenant_invalidation_does_not_touch_other_tenant() {
        let cache = QueryResultCache::with_defaults();
        let key_a = skey("tenant-a", "public", "SELECT * FROM orders");
        let key_b = skey("tenant-b", "public", "SELECT * FROM orders");

        cache
            .insert_fresh(
                key_a.clone(),
                create_test_result(),
                vec!["orders".to_string()],
                Some(42),
            )
            .expect("insert a");
        cache
            .insert_fresh(
                key_b.clone(),
                create_test_result(),
                vec!["orders".to_string()],
                Some(42),
            )
            .expect("insert b");

        // Invalidate (tenant-a, orders) — must drop only A.
        let dropped = cache.invalidate_tenant_collection("tenant-a", "orders");
        assert_eq!(dropped, 1);
        assert!(
            cache
                .get_fresh(&key_a, &VectorFreshnessMode::Strong, 42)
                .is_none()
        );
        assert!(
            cache
                .get_fresh(&key_b, &VectorFreshnessMode::Strong, 42)
                .is_some()
        );
    }

    #[test]
    fn strong_lsn_pinned_serve() {
        // Strong serves iff computed_at_lsn == current_lsn && lsn != 0.
        let cache = QueryResultCache::with_defaults();
        let key = skey("t", "public", "SELECT * FROM orders");
        cache
            .insert_fresh(
                key.clone(),
                create_test_result(),
                vec!["orders".to_string()],
                Some(42),
            )
            .expect("insert");

        // Matching LSN → serve.
        assert!(
            cache
                .get_fresh(&key, &VectorFreshnessMode::Strong, 42)
                .is_some()
        );
        // Advanced LSN (simulates a write) → bypass.
        assert!(
            cache
                .get_fresh(&key, &VectorFreshnessMode::Strong, 43)
                .is_none()
        );
        // current_lsn == 0 (LSN tracking unavailable) → never serve Strong.
        assert!(
            cache
                .get_fresh(&key, &VectorFreshnessMode::Strong, 0)
                .is_none()
        );
        // An entry written without an LSN anchor is never Strong-eligible.
        let key2 = skey("t", "public", "SELECT * FROM other");
        cache
            .insert_fresh(
                key2.clone(),
                create_test_result(),
                vec!["other".to_string()],
                None,
            )
            .expect("insert no-lsn");
        assert!(
            cache
                .get_fresh(&key2, &VectorFreshnessMode::Strong, 42)
                .is_none()
        );
    }

    #[test]
    fn strong_serves_until_ttl_even_with_stable_lsn() {
        // TTL is the universal entry lifetime: even a stable LSN cannot serve
        // an expired entry.
        let config = QueryResultCacheConfig {
            default_ttl: Duration::from_millis(1),
            ..Default::default()
        };
        let cache = QueryResultCache::new(config);
        let key = skey("t", "public", "SELECT * FROM orders");
        cache
            .insert_fresh(
                key.clone(),
                create_test_result(),
                vec!["orders".to_string()],
                Some(42),
            )
            .expect("insert");
        std::thread::sleep(Duration::from_millis(10));
        assert!(
            cache
                .get_fresh(&key, &VectorFreshnessMode::Strong, 42)
                .is_none()
        );
    }

    #[test]
    fn bounded_stale_window() {
        let cache = QueryResultCache::with_defaults();
        let key = skey("t", "public", "SELECT * FROM orders");
        cache
            .insert_fresh(
                key.clone(),
                create_test_result(),
                vec!["orders".to_string()],
                None,
            )
            .expect("insert");

        // Within the staleness window → serve.
        let within = VectorFreshnessMode::BoundedStale {
            max_staleness_ms: 60_000,
        };
        assert!(cache.get_fresh(&key, &within, 0).is_some());

        // Past the window → bypass (age > max_staleness_ms).
        let past = VectorFreshnessMode::BoundedStale {
            max_staleness_ms: 0,
        };
        std::thread::sleep(Duration::from_millis(2));
        assert!(cache.get_fresh(&key, &past, 0).is_none());
    }

    #[test]
    fn freshness_eligible_predicate_units() {
        // Pure predicate: exercise each branch without a cache.
        let ttl = Duration::from_secs(60);
        // Strong
        assert!(freshness_eligible(
            &VectorFreshnessMode::Strong,
            Duration::from_secs(1),
            ttl,
            Some(7),
            7
        ));
        assert!(!freshness_eligible(
            &VectorFreshnessMode::Strong,
            Duration::from_secs(1),
            ttl,
            Some(7),
            8
        ));
        assert!(!freshness_eligible(
            &VectorFreshnessMode::Strong,
            Duration::from_secs(1),
            ttl,
            Some(7),
            0
        ));
        // BoundedStale
        assert!(freshness_eligible(
            &VectorFreshnessMode::BoundedStale {
                max_staleness_ms: 1_000
            },
            Duration::from_millis(500),
            ttl,
            None,
            0
        ));
        assert!(!freshness_eligible(
            &VectorFreshnessMode::BoundedStale {
                max_staleness_ms: 1_000
            },
            Duration::from_millis(2_000),
            ttl,
            None,
            0
        ));
        // StaleOk
        assert!(freshness_eligible(
            &VectorFreshnessMode::StaleOk,
            Duration::from_secs(1),
            ttl,
            None,
            0
        ));
        // Universal TTL expiry overrides every mode.
        assert!(!freshness_eligible(
            &VectorFreshnessMode::StaleOk,
            Duration::from_secs(120),
            ttl,
            Some(7),
            7
        ));
    }
}
