// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! `TenantCache` — a multitenant, byte-budgeted, cache-aware primitive.
//!
//! One **global** [`moka`] cache (so capacity is pooled — economies of scale, no
//! per-tenant stranding) whose admission is moka's TinyLFU (scan-resistant and
//! frequency-aware, so a flooding tenant's cold entries are rejected *at
//! admission* and hot small-tenant entries are protected). On top of that:
//!
//! * every entry is **tenant-namespaced** by [`CacheKey`] — the isolation key;
//! * capacity is **byte-weighted** (not entry-count) via [`CachedValue::weight`];
//! * each tenant has a **soft byte ceiling** — once exceeded, that tenant's new
//!   entries bypass the cache (the value is still returned) so one tenant cannot
//!   monopolize the pool;
//! * **per-tenant stats** (hits/misses/bytes/evictions) are exposed for billing
//!   labels — this crate stays metrics-free and just returns the numbers.
//!
//! Eviction is reconciled into per-tenant byte gauges by a moka eviction
//! listener, so accounting stays correct no matter which tenant's entry moka
//! chooses to evict under global pressure.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use moka::future::Cache;
use moka::notification::RemovalCause;

/// The category of a cached artifact. Drives key-namespacing and per-kind stats;
/// extend as new consumers adopt the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum CacheKind {
    /// Parsed PAX block footer / column metadata (`BlockLayout`).
    Footer,
    /// Parsed PAX segment index (block offsets).
    SegmentIndex,
    /// Quantized vector codes (SQ8 / RaBitQ) hot tier.
    QuantizedCodes,
    /// Cached SQL query result.
    QueryResult,
    /// Catalog schema / namespace metadata.
    CatalogSchema,
    /// Anything not yet categorized.
    Other,
}

/// A tenant-namespaced cache key. `tenant` is the isolation boundary; `kind`
/// separates artifact classes; `key` is the per-artifact identifier (e.g. a
/// segment path + block offset).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub tenant: Arc<str>,
    pub kind: CacheKind,
    pub key: Arc<str>,
}

impl CacheKey {
    pub fn new(tenant: impl AsRef<str>, kind: CacheKind, key: impl AsRef<str>) -> Self {
        Self {
            tenant: Arc::from(tenant.as_ref()),
            kind,
            key: Arc::from(key.as_ref()),
        }
    }
}

/// A cached value carrying its byte weight (so moka's weigher budgets by bytes,
/// not entry count).
#[derive(Debug, Clone)]
struct CachedValue<V> {
    weight: u32,
    value: V,
}

/// Cache sizing policy: a global byte pool plus per-tenant byte ceilings.
#[derive(Debug, Clone)]
pub struct CacheBudget {
    /// Total bytes across all tenants (the shared pool).
    pub total_bytes: u64,
    /// Default per-tenant ceiling when not overridden.
    pub default_tenant_ceiling_bytes: u64,
    /// Per-tenant ceiling overrides (e.g. larger tenants).
    pub per_tenant_ceiling: HashMap<String, u64>,
    /// Optional time-to-live for entries (None = no expiry; rely on size).
    pub ttl: Option<Duration>,
}

impl CacheBudget {
    /// A simple budget: `total_bytes` pool, `per_tenant_ceiling_bytes` cap each.
    pub fn new(total_bytes: u64, per_tenant_ceiling_bytes: u64) -> Self {
        Self {
            total_bytes,
            default_tenant_ceiling_bytes: per_tenant_ceiling_bytes,
            per_tenant_ceiling: HashMap::new(),
            ttl: None,
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    pub fn with_tenant_ceiling(mut self, tenant: impl Into<String>, bytes: u64) -> Self {
        self.per_tenant_ceiling.insert(tenant.into(), bytes);
        self
    }
}

#[derive(Default)]
struct TenantUsage {
    bytes: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    inserts: AtomicU64,
    evictions: AtomicU64,
}

/// A point-in-time per-tenant stats snapshot for metrics emission.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TenantCacheStat {
    pub tenant: String,
    pub bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub evictions: u64,
    pub hit_ratio: f64,
}

/// A multitenant, byte-budgeted cache over values of type `V`.
pub struct TenantCache<V: Clone + Send + Sync + 'static> {
    inner: Cache<CacheKey, CachedValue<V>>,
    usage: Arc<DashMap<Arc<str>, TenantUsage>>,
    default_ceiling: u64,
    per_tenant_ceiling: Arc<HashMap<String, u64>>,
}

impl<V: Clone + Send + Sync + 'static> TenantCache<V> {
    /// Build a cache for the given byte `budget`. The eviction listener keeps
    /// per-tenant byte/eviction gauges accurate under global pressure.
    pub fn new(budget: CacheBudget) -> Self {
        let usage: Arc<DashMap<Arc<str>, TenantUsage>> = Arc::new(DashMap::new());
        let listener_usage = usage.clone();

        let mut builder = Cache::builder()
            .max_capacity(budget.total_bytes)
            .weigher(|_k: &CacheKey, v: &CachedValue<V>| v.weight)
            .eviction_listener(move |k: Arc<CacheKey>, v: CachedValue<V>, cause: RemovalCause| {
                if let Some(u) = listener_usage.get(&k.tenant) {
                    u.bytes.fetch_sub(v.weight as u64, Ordering::Relaxed);
                    // Count true capacity/expiry evictions (not explicit removals).
                    if matches!(cause, RemovalCause::Size | RemovalCause::Expired) {
                        u.evictions.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        if let Some(ttl) = budget.ttl {
            builder = builder.time_to_live(ttl);
        }

        Self {
            inner: builder.build(),
            usage,
            default_ceiling: budget.default_tenant_ceiling_bytes,
            per_tenant_ceiling: Arc::new(budget.per_tenant_ceiling),
        }
    }

    fn usage_for(&self, tenant: &Arc<str>) -> dashmap::mapref::one::Ref<'_, Arc<str>, TenantUsage> {
        if let Some(u) = self.usage.get(tenant) {
            return u;
        }
        self.usage.entry(tenant.clone()).or_default();
        self.usage.get(tenant).expect("just inserted")
    }

    fn ceiling_for(&self, tenant: &str) -> u64 {
        self.per_tenant_ceiling
            .get(tenant)
            .copied()
            .unwrap_or(self.default_ceiling)
    }

    /// Look up a value, recording a per-tenant hit or miss.
    pub async fn get(&self, key: &CacheKey) -> Option<V> {
        let result = self.inner.get(key).await;
        let u = self.usage_for(&key.tenant);
        match result {
            Some(cv) => {
                u.hits.fetch_add(1, Ordering::Relaxed);
                Some(cv.value)
            }
            None => {
                u.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Get `key`, or run `loader` on miss and cache the result **iff** the
    /// tenant is under its byte ceiling (otherwise the value is returned without
    /// caching — bypass — so one tenant cannot monopolize the pool). `weight` is
    /// the value's byte size for the budget.
    pub async fn get_or_load<F, Fut, E>(
        &self,
        key: CacheKey,
        weight: u32,
        loader: F,
    ) -> Result<V, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, E>>,
    {
        if let Some(v) = self.get(&key).await {
            return Ok(v);
        }
        // `get` already recorded the miss.
        let value = loader().await?;

        let ceiling = self.ceiling_for(&key.tenant);
        let cur = self.usage_for(&key.tenant).bytes.load(Ordering::Relaxed);
        if cur.saturating_add(weight as u64) <= ceiling {
            {
                let u = self.usage_for(&key.tenant);
                u.bytes.fetch_add(weight as u64, Ordering::Relaxed);
                u.inserts.fetch_add(1, Ordering::Relaxed);
            }
            self.inner
                .insert(
                    key,
                    CachedValue {
                        weight,
                        value: value.clone(),
                    },
                )
                .await;
        }
        Ok(value)
    }

    /// Explicitly insert/replace a value if the tenant is under its ceiling.
    pub async fn insert(&self, key: CacheKey, weight: u32, value: V) {
        let ceiling = self.ceiling_for(&key.tenant);
        let cur = self.usage_for(&key.tenant).bytes.load(Ordering::Relaxed);
        if cur.saturating_add(weight as u64) > ceiling {
            return;
        }
        {
            let u = self.usage_for(&key.tenant);
            u.bytes.fetch_add(weight as u64, Ordering::Relaxed);
            u.inserts.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.insert(key, CachedValue { weight, value }).await;
    }

    /// Drain moka's pending maintenance (eviction listener, etc.). Call before
    /// reading [`Self::tenant_stats`] in tests for deterministic gauges.
    pub async fn sync(&self) {
        self.inner.run_pending_tasks().await;
    }

    /// Per-tenant stats snapshot for metrics emission.
    pub fn tenant_stats(&self) -> Vec<TenantCacheStat> {
        self.usage
            .iter()
            .map(|e| {
                let u = e.value();
                let hits = u.hits.load(Ordering::Relaxed);
                let misses = u.misses.load(Ordering::Relaxed);
                let total = hits + misses;
                TenantCacheStat {
                    tenant: e.key().to_string(),
                    bytes: u.bytes.load(Ordering::Relaxed),
                    hits,
                    misses,
                    inserts: u.inserts.load(Ordering::Relaxed),
                    evictions: u.evictions.load(Ordering::Relaxed),
                    hit_ratio: if total == 0 {
                        0.0
                    } else {
                        hits as f64 / total as f64
                    },
                }
            })
            .collect()
    }

    /// Current tracked byte usage for a tenant (post-`sync`).
    pub fn tenant_bytes(&self, tenant: &str) -> u64 {
        self.usage
            .get(&Arc::from(tenant))
            .map(|u| u.bytes.load(Ordering::Relaxed))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(tenant: &str, k: &str) -> CacheKey {
        CacheKey::new(tenant, CacheKind::Footer, k)
    }

    #[tokio::test]
    async fn key_namespacing_isolates_tenants() {
        let c: TenantCache<u64> = TenantCache::new(CacheBudget::new(1 << 20, 1 << 20));
        c.insert(key("A", "seg1"), 8, 100).await;
        c.insert(key("B", "seg1"), 8, 200).await;
        c.sync().await;
        assert_eq!(c.get(&key("A", "seg1")).await, Some(100));
        assert_eq!(c.get(&key("B", "seg1")).await, Some(200));
        // Same artifact key, different tenant → different entries.
        assert_eq!(c.get(&key("A", "missing")).await, None);
    }

    #[tokio::test]
    async fn get_or_load_records_hit_miss_and_gauges() {
        let c: TenantCache<u64> = TenantCache::new(CacheBudget::new(1 << 20, 1 << 20));
        let k = key("A", "seg1");
        // miss → loads → caches
        let v = c
            .get_or_load(k.clone(), 16, || async { Ok::<u64, ()>(42) })
            .await
            .unwrap();
        assert_eq!(v, 42);
        // hit → no reload
        let v2 = c
            .get_or_load(k.clone(), 16, || async { Ok::<u64, ()>(999) })
            .await
            .unwrap();
        assert_eq!(v2, 42, "second call must hit cache, not reload");
        c.sync().await;
        let s = c.tenant_stats();
        let a = s.iter().find(|s| s.tenant == "A").unwrap();
        assert_eq!(a.hits, 1);
        assert_eq!(a.misses, 1);
        assert_eq!(a.inserts, 1);
        assert_eq!(a.bytes, 16);
    }

    #[tokio::test]
    async fn per_tenant_ceiling_caps_one_tenant() {
        // Big global pool, tiny per-tenant ceiling.
        let c: TenantCache<u64> =
            TenantCache::new(CacheBudget::new(1 << 30, /*ceiling*/ 100));
        // Tenant A floods past its 100-byte ceiling.
        for i in 0..50u64 {
            c.insert(CacheKey::new("A", CacheKind::Footer, format!("k{i}")), 10, i)
                .await;
        }
        c.sync().await;
        // A is capped at/around the ceiling, NOT all 500 bytes.
        assert!(c.tenant_bytes("A") <= 100, "A bytes {} > ceiling", c.tenant_bytes("A"));
        // Tenant B (separate ceiling) is unaffected by A's flood.
        c.insert(CacheKey::new("B", CacheKind::Footer, "b1"), 10, 7).await;
        c.sync().await;
        assert_eq!(c.get(&CacheKey::new("B", CacheKind::Footer, "b1")).await, Some(7));
    }

    #[tokio::test]
    async fn byte_weighted_eviction_respects_total_budget() {
        // Global pool 100 bytes; ceiling high so the GLOBAL budget is what bites.
        let c: TenantCache<u64> = TenantCache::new(CacheBudget::new(100, 1 << 30));
        for i in 0..40u64 {
            c.insert(CacheKey::new("A", CacheKind::Footer, format!("k{i}")), 10, i)
                .await;
        }
        c.sync().await;
        // moka holds total weight ≈ max_capacity, not all 400 bytes.
        assert!(
            c.tenant_bytes("A") <= 100 + 10,
            "tracked bytes {} exceed pool",
            c.tenant_bytes("A")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_get_or_load_is_safe() {
        let c: Arc<TenantCache<u64>> =
            Arc::new(TenantCache::new(CacheBudget::new(1 << 20, 1 << 20)));
        let mut handles = Vec::new();
        for t in 0..8u64 {
            let cc = c.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..100u64 {
                    let k = CacheKey::new("A", CacheKind::Footer, format!("k{}", i % 10));
                    let _ = cc
                        .get_or_load(k, 8, || async move { Ok::<u64, ()>(t + i) })
                        .await;
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        c.sync().await;
        // 10 distinct keys × 8 bytes = 80 bytes tracked (no double counting).
        assert!(c.tenant_bytes("A") <= 80, "bytes {}", c.tenant_bytes("A"));
    }
}
