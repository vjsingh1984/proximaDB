// Plan cache — LLD §6.4.
//
// Caches `PlanOutput` keyed on the shape of the incoming search request
// so identical query shapes skip the SelectivityEstimator + GLS work.
// Phase 1 emits PlanOutput in ~20-40 µs per call; this cache turns that
// into one DashMap lookup for the common case where a single agent loops
// hundreds of variations of the same predicate set.
//
// Cache key:
//   - tenant_id          → strict tenant isolation (LLD multi-tenant rule).
//   - collection         → distinct collections always plan separately.
//   - predicate_digest   → hash of the normalized predicate set.
//   - dim                → vector dimensionality (route choice depends on it).
//   - recall_target_bits → recall target encoded as u32 so we can hash it.
//
// Cache entries carry the `corpus_version` they were computed at; a bump
// in the segment manifest's version forces every entry for the collection
// to be invalidated en masse (mass-flush by version comparison on lookup).
//
// EMA-tracked hit rate so observability dashboards can flag a collection
// that's churning through plan choices (often a sign the predicate-digest
// hash is mis-classifying queries).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::query::federated::optimizer::plan_builder::PlanOutput;

/// Composite key for a plan cache lookup. Hash + Eq derive so it can index
/// a HashMap directly.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PlanCacheKey {
    pub tenant_id: String,
    pub collection: String,
    pub predicate_digest: u64,
    pub dim: u32,
    /// Recall target × 1000, rounded to nearest int — keeps key hashable
    /// without precision games.
    pub recall_target_bits: u32,
}

impl PlanCacheKey {
    /// Build a key from the natural inputs. `predicate_digest` should be a
    /// stable hash of the normalized predicate set — callers can use the
    /// `digest_predicates` helper below.
    pub fn new(
        tenant_id: impl Into<String>,
        collection: impl Into<String>,
        predicate_digest: u64,
        dim: u32,
        recall_target: f64,
    ) -> Self {
        let bits = (recall_target.clamp(0.0, 1.0) * 1000.0).round() as u32;
        Self {
            tenant_id: tenant_id.into(),
            collection: collection.into(),
            predicate_digest,
            dim,
            recall_target_bits: bits,
        }
    }
}

/// Stable digest of a slice of `(column, op_label, value_repr)` triples.
/// Callers supply the slice in the normalized order Phase 1's estimator
/// already uses; this helper does no sorting so the digest reflects the
/// exact predicate sequence the planner saw.
pub fn digest_predicates(predicates: &[(String, String, String)]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (col, op, val) in predicates {
        col.hash(&mut hasher);
        0u8.hash(&mut hasher);
        op.hash(&mut hasher);
        0u8.hash(&mut hasher);
        val.hash(&mut hasher);
        0u8.hash(&mut hasher);
    }
    hasher.finish()
}

/// Stored cache value — the plan plus its corpus version + timestamp.
#[derive(Debug, Clone)]
struct PlanCacheEntry {
    plan: PlanOutput,
    corpus_version: u64,
    cached_at: Instant,
}

/// Configuration knobs.
#[derive(Debug, Clone, Copy)]
pub struct PlanCacheConfig {
    /// Hard capacity ceiling. Hitting it forces LRU eviction.
    pub max_entries: usize,
    /// Per-entry TTL. Entries past their TTL miss on lookup and are
    /// evicted en masse on the next sweep.
    pub ttl: Duration,
    /// EMA smoothing factor in (0.0, 1.0). Lower values give a smoother
    /// hit-rate signal. Default 0.05 (= ~20-query smoothing).
    pub ema_alpha: f64,
}

impl Default for PlanCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            ttl: Duration::from_secs(300),
            ema_alpha: 0.05,
        }
    }
}

/// EMA-tracked plan-cache counters surfaced for observability.
///
/// Naming note: this type used to be called `PlanCacheStats` and collided
/// with the federated/execution/proximadb-query variants. Renamed because
/// the field set is observability-specific — only this variant tracks
/// the EMA hit rate and invalidation count. The canonical
/// `proximadb_query::PlanCacheStats` is what generic callers should use.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ObservabilityPlanCacheStats {
    pub entries: usize,
    pub total_lookups: u64,
    pub total_hits: u64,
    pub total_evictions: u64,
    pub total_invalidations: u64,
    /// Exponentially-weighted moving average of hit_rate in [0.0, 1.0].
    pub ema_hit_rate: f64,
}

/// LRU + version-based plan cache.
#[derive(Clone)]
pub struct PlanCache {
    inner: Arc<RwLock<Inner>>,
    config: PlanCacheConfig,
}

struct Inner {
    map: HashMap<PlanCacheKey, PlanCacheEntry>,
    /// LRU order — front is least-recently-used. Stored as a Vec because
    /// the touch-on-lookup pattern is O(n) worst case but the cap is small
    /// (defaults to 10k entries) and avoids the doubly-linked-list dance.
    lru: Vec<PlanCacheKey>,
    stats: ObservabilityPlanCacheStats,
}

/// Process-wide PlanCache singleton — lazy-initialized on first
/// access. The cache is `Clone`-able (it wraps an `Arc<RwLock<…>>`)
/// so the singleton stays cheap to hand to per-request handlers.
///
/// Production call sites use `global()` for a shared cache; tests
/// construct their own with `default()` to keep test isolation.
static GLOBAL_PLAN_CACHE: OnceLock<PlanCache> = OnceLock::new();

impl PlanCache {
    pub fn new(config: PlanCacheConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                map: HashMap::new(),
                lru: Vec::new(),
                stats: ObservabilityPlanCacheStats::default(),
            })),
            config,
        }
    }

    /// Process-wide singleton. First call constructs with
    /// `PlanCacheConfig::default()`; subsequent calls return the same
    /// instance.
    pub fn global() -> &'static PlanCache {
        GLOBAL_PLAN_CACHE.get_or_init(PlanCache::default)
    }

    /// Look up the plan for a key. Returns `None` on miss or when the
    /// cached entry's `corpus_version` differs from the supplied version
    /// (in which case the entry is dropped en passant).
    pub async fn get(&self, key: &PlanCacheKey, current_corpus_version: u64) -> Option<PlanOutput> {
        let mut g = self.inner.write().await;
        g.stats.total_lookups += 1;

        let hit_count_before = g.stats.total_hits;
        let mut hit_plan: Option<PlanOutput> = None;
        let mut version_drift = false;

        if let Some(entry) = g.map.get(key).cloned() {
            if entry.corpus_version != current_corpus_version {
                version_drift = true;
            } else if entry.cached_at.elapsed() > self.config.ttl {
                // Expired — treat as a miss + sweep this entry.
                g.map.remove(key);
                g.lru.retain(|k| k != key);
                g.stats.total_evictions += 1;
                g.stats.entries = g.map.len();
            } else {
                // Hit.
                hit_plan = Some(entry.plan.clone());
                g.stats.total_hits += 1;
                // Touch LRU.
                g.lru.retain(|k| k != key);
                g.lru.push(key.clone());
            }
        }
        if version_drift {
            g.map.remove(key);
            g.lru.retain(|k| k != key);
            g.stats.total_invalidations += 1;
            g.stats.entries = g.map.len();
        }
        // EMA update — 1.0 for hit, 0.0 for miss.
        let hit = g.stats.total_hits > hit_count_before;
        let alpha = self.config.ema_alpha.clamp(0.0, 1.0);
        g.stats.ema_hit_rate = alpha * (hit as u64 as f64) + (1.0 - alpha) * g.stats.ema_hit_rate;
        hit_plan
    }

    /// Insert a plan output. If the cache is at capacity, evict the LRU
    /// entry.
    pub async fn put(&self, key: PlanCacheKey, plan: PlanOutput, corpus_version: u64) {
        let mut g = self.inner.write().await;
        if g.map.len() >= self.config.max_entries && !g.map.contains_key(&key) {
            if let Some(victim) = g.lru.first().cloned() {
                g.lru.remove(0);
                g.map.remove(&victim);
                g.stats.total_evictions += 1;
            }
        }
        // Drop any existing LRU entry for this key (it's about to move).
        g.lru.retain(|k| k != &key);
        g.map.insert(
            key.clone(),
            PlanCacheEntry {
                plan,
                corpus_version,
                cached_at: Instant::now(),
            },
        );
        g.lru.push(key);
        g.stats.entries = g.map.len();
    }

    /// Drop every entry for a `(tenant, collection)` pair. Called when the
    /// catalog publishes a new corpus version — versions on stale entries
    /// would also force per-lookup invalidation, but this is the bulk-flush
    /// path that keeps memory bounded after schema-evolving DDL.
    pub async fn invalidate_collection(&self, tenant_id: &str, collection: &str) -> u64 {
        let mut g = self.inner.write().await;
        let to_remove: Vec<PlanCacheKey> = g
            .map
            .keys()
            .filter(|k| k.tenant_id == tenant_id && k.collection == collection)
            .cloned()
            .collect();
        let n = to_remove.len() as u64;
        for k in &to_remove {
            g.map.remove(k);
        }
        g.lru.retain(|k| !to_remove.contains(k));
        g.stats.total_invalidations += n;
        g.stats.entries = g.map.len();
        n
    }

    /// Snapshot the counters.
    pub async fn stats(&self) -> ObservabilityPlanCacheStats {
        let g = self.inner.read().await;
        let mut s = g.stats.clone();
        s.entries = g.map.len();
        s
    }
}

impl Default for PlanCache {
    fn default() -> Self {
        Self::new(PlanCacheConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::search_plan_trace::{FilterStrategy, IndexRoute};

    fn plan() -> PlanOutput {
        PlanOutput {
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            estimated_selectivity: Some(0.1),
            gls_score: None,
        }
    }

    fn key(tenant: &str, coll: &str, digest: u64) -> PlanCacheKey {
        PlanCacheKey::new(tenant, coll, digest, 384, 0.9)
    }

    fn cfg(max: usize, ttl_ms: u64) -> PlanCacheConfig {
        PlanCacheConfig {
            max_entries: max,
            ttl: Duration::from_millis(ttl_ms),
            ema_alpha: 0.5,
        }
    }

    #[tokio::test]
    async fn miss_then_put_then_hit() {
        let cache = PlanCache::default();
        let k = key("t", "kb", 1);
        assert!(cache.get(&k, 1).await.is_none());
        cache.put(k.clone(), plan(), 1).await;
        let got = cache.get(&k, 1).await;
        assert!(got.is_some());
        assert_eq!(got.unwrap(), plan());
        let stats = cache.stats().await;
        assert_eq!(stats.total_lookups, 2);
        assert_eq!(stats.total_hits, 1);
    }

    #[tokio::test]
    async fn corpus_version_drift_invalidates_on_lookup() {
        let cache = PlanCache::default();
        let k = key("t", "kb", 1);
        cache.put(k.clone(), plan(), 1).await;
        // Lookup with a different corpus version → miss + invalidation.
        let got = cache.get(&k, 2).await;
        assert!(got.is_none());
        let stats = cache.stats().await;
        assert!(stats.total_invalidations >= 1);
    }

    #[tokio::test]
    async fn ttl_expiry_drops_entry() {
        let cache = PlanCache::new(cfg(64, 1)); // 1 ms TTL
        let k = key("t", "kb", 1);
        cache.put(k.clone(), plan(), 1).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(cache.get(&k, 1).await.is_none());
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 0);
        assert!(stats.total_evictions >= 1);
    }

    #[tokio::test]
    async fn lru_evicts_oldest_at_capacity() {
        let cache = PlanCache::new(cfg(2, 60_000));
        let a = key("t", "kb", 1);
        let b = key("t", "kb", 2);
        let c = key("t", "kb", 3);
        cache.put(a.clone(), plan(), 1).await;
        cache.put(b.clone(), plan(), 1).await;
        // Touch a so b is now the LRU.
        cache.get(&a, 1).await;
        cache.put(c.clone(), plan(), 1).await;
        assert!(cache.get(&a, 1).await.is_some());
        assert!(
            cache.get(&b, 1).await.is_none(),
            "b should have been evicted"
        );
        assert!(cache.get(&c, 1).await.is_some());
    }

    #[tokio::test]
    async fn tenant_isolation_at_key_level() {
        let cache = PlanCache::default();
        let a = key("tenant-a", "kb", 1);
        let b = key("tenant-b", "kb", 1);
        cache.put(a.clone(), plan(), 1).await;
        // Same digest under a different tenant must not hit.
        assert!(cache.get(&b, 1).await.is_none());
        assert!(cache.get(&a, 1).await.is_some());
    }

    #[tokio::test]
    async fn collection_isolation_at_key_level() {
        let cache = PlanCache::default();
        let a = key("t", "kb-1", 1);
        let b = key("t", "kb-2", 1);
        cache.put(a.clone(), plan(), 1).await;
        assert!(cache.get(&b, 1).await.is_none());
    }

    #[tokio::test]
    async fn invalidate_collection_drops_matching_entries() {
        let cache = PlanCache::default();
        cache.put(key("t", "kb-1", 1), plan(), 1).await;
        cache.put(key("t", "kb-1", 2), plan(), 1).await;
        cache.put(key("t", "kb-2", 1), plan(), 1).await;
        let n = cache.invalidate_collection("t", "kb-1").await;
        assert_eq!(n, 2);
        assert!(cache.get(&key("t", "kb-1", 1), 1).await.is_none());
        assert!(cache.get(&key("t", "kb-1", 2), 1).await.is_none());
        // kb-2 still alive.
        assert!(cache.get(&key("t", "kb-2", 1), 1).await.is_some());
    }

    #[tokio::test]
    async fn put_replaces_existing_value_without_growing() {
        let cache = PlanCache::new(cfg(2, 60_000));
        let k = key("t", "kb", 1);
        cache.put(k.clone(), plan(), 1).await;
        cache.put(k.clone(), plan(), 2).await;
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 1);
        // Lookup at the new version hits; the old version doesn't.
        assert!(cache.get(&k, 2).await.is_some());
    }

    #[tokio::test]
    async fn ema_hit_rate_moves_toward_one_under_steady_hits() {
        // EMA alpha 0.5 → after ~10 consecutive hits, EMA should be very
        // close to 1.0.
        let cache = PlanCache::new(cfg(64, 60_000));
        let k = key("t", "kb", 1);
        cache.put(k.clone(), plan(), 1).await;
        for _ in 0..15 {
            cache.get(&k, 1).await;
        }
        let stats = cache.stats().await;
        assert!(
            stats.ema_hit_rate > 0.95,
            "EMA should be near 1.0, got {}",
            stats.ema_hit_rate
        );
    }

    #[tokio::test]
    async fn ema_hit_rate_decays_after_misses() {
        let cache = PlanCache::new(cfg(64, 60_000));
        let k = key("t", "kb", 1);
        cache.put(k.clone(), plan(), 1).await;
        // 5 hits drive EMA toward 1.
        for _ in 0..5 {
            cache.get(&k, 1).await;
        }
        let after_hits = cache.stats().await.ema_hit_rate;
        // 10 misses (different keys) should drop EMA.
        for i in 0..10 {
            let m = key("t", "kb", 100 + i);
            cache.get(&m, 1).await;
        }
        let after_misses = cache.stats().await.ema_hit_rate;
        assert!(
            after_misses < after_hits,
            "EMA should drop after misses: {} -> {}",
            after_hits,
            after_misses
        );
    }

    #[test]
    fn digest_is_order_sensitive() {
        // Different orders → different digests so the runtime decides
        // whether to canonicalize the predicate sequence at planner entry.
        let a = digest_predicates(&[
            ("x".into(), "eq".into(), "1".into()),
            ("y".into(), "eq".into(), "2".into()),
        ]);
        let b = digest_predicates(&[
            ("y".into(), "eq".into(), "2".into()),
            ("x".into(), "eq".into(), "1".into()),
        ]);
        assert_ne!(a, b, "predicate order must matter so callers know to sort");
    }

    #[test]
    fn digest_identical_inputs_match() {
        let a = digest_predicates(&[
            ("x".into(), "eq".into(), "1".into()),
            ("y".into(), "gt".into(), "5".into()),
        ]);
        let b = digest_predicates(&[
            ("x".into(), "eq".into(), "1".into()),
            ("y".into(), "gt".into(), "5".into()),
        ]);
        assert_eq!(a, b);
    }

    #[test]
    fn recall_target_bits_round_consistently() {
        let a = PlanCacheKey::new("t", "kb", 1, 384, 0.9);
        let b = PlanCacheKey::new("t", "kb", 1, 384, 0.9001);
        // 0.9 * 1000 = 900, 0.9001 * 1000 = 900.1 → rounds to 900.
        assert_eq!(a.recall_target_bits, b.recall_target_bits);
    }

    #[test]
    fn recall_target_out_of_range_clamps() {
        let lo = PlanCacheKey::new("t", "kb", 1, 384, -0.5);
        let hi = PlanCacheKey::new("t", "kb", 1, 384, 1.5);
        assert_eq!(lo.recall_target_bits, 0);
        assert_eq!(hi.recall_target_bits, 1000);
    }
}
