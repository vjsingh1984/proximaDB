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

/// Per-tenant elasticity limits: a guaranteed `floor`, an absolute `hard_ceiling`
/// (runaway guard), and a `weight` for the contended fair share (higher = larger
/// share). Between floor and ceiling, capacity is borrowed from the idle pool.
#[derive(Debug, Clone, Copy)]
pub struct TenantLimits {
    pub floor_bytes: u64,
    pub hard_ceiling_bytes: u64,
    pub weight: u32,
}

/// One tier's cache shares, expressed as **fractions of the global pool** so the
/// same policy scales to any `total_bytes`. `weight` drives the contended fair
/// share (higher tier = larger share); `floor_frac` is the protected working
/// set; `ceiling_frac` is the absolute runaway cap.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct TierSpec {
    pub weight: u32,
    pub floor_frac: f64,
    pub ceiling_frac: f64,
}

/// JSON-loadable per-tier cache policy — a **generic, operator-supplied** config
/// schema (string-keyed tier ids; an unknown tenant tier falls back to
/// `default_tier`). OSS ships no commercial tiers: the actual tier→share data is
/// deployment config provided by the control plane (e.g. anvaiops ships a
/// `cache_tiers.json` keyed by its pricing tiers). This struct is just the Rust
/// parser + the resolver factory (the adapter that turns config into a
/// [`LimitsResolver`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TierPolicy {
    pub default_tier: String,
    pub tiers: HashMap<String, TierSpec>,
}

impl TierPolicy {
    /// Parse a tier policy from operator-supplied JSON (`cache_tiers.json`).
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Neutral OSS default: a single `"default"` tier that may use the whole pool
    /// (uniform fair share, no tier preference). Production tier shares come from
    /// operator config — OSS bakes in no commercial tiers.
    pub fn single_default() -> Self {
        let mut tiers = HashMap::new();
        tiers.insert(
            "default".into(),
            TierSpec { weight: 1, floor_frac: 0.0, ceiling_frac: 1.0 },
        );
        Self { default_tier: "default".into(), tiers }
    }

    /// Absolute [`TenantLimits`] for `tier` at a given pool `total_bytes`
    /// (falls back to `default_tier`, then to a permissive default).
    pub fn limits(&self, tier: &str, total_bytes: u64) -> TenantLimits {
        let spec = self
            .tiers
            .get(tier)
            .or_else(|| self.tiers.get(&self.default_tier));
        match spec {
            Some(s) => TenantLimits {
                floor_bytes: (total_bytes as f64 * s.floor_frac) as u64,
                hard_ceiling_bytes: ((total_bytes as f64 * s.ceiling_frac) as u64).max(1),
                weight: s.weight.max(1),
            },
            None => TenantLimits {
                floor_bytes: 0,
                hard_ceiling_bytes: total_bytes,
                weight: 1,
            },
        }
    }

    /// Build a [`LimitsResolver`] from this policy: `tenant_id → tier`
    /// (host-supplied, e.g. catalog/billing lookup) → [`TenantLimits`].
    pub fn resolver(
        self: Arc<Self>,
        total_bytes: u64,
        tenant_to_tier: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> Arc<LimitsResolver> {
        Arc::new(move |tenant: &str| {
            let tier = tenant_to_tier(tenant);
            self.limits(&tier, total_bytes)
        })
    }
}

/// Pluggable per-tenant limits policy — the Strategy seam for tier-driven
/// preference. The host supplies a `tenant_id → TenantLimits` function (e.g.
/// [`TierPolicy::resolver`]); when unset, the cache falls back to the
/// `CacheBudget`'s static per-tenant map / defaults.
pub type LimitsResolver = dyn Fn(&str) -> TenantLimits + Send + Sync;

/// Cache sizing policy: a global byte pool with **work-conserving** per-tenant
/// elasticity — tenants borrow idle pool capacity up to their hard ceiling, and
/// are reclaimed toward a weighted fair share only when the pool is under
/// pressure. Nothing is pre-partitioned, so idle capacity is never stranded
/// (no fragmentation), while the hard ceiling bounds any single tenant.
#[derive(Debug, Clone)]
pub struct CacheBudget {
    /// Total bytes across all tenants (the shared pool).
    pub total_bytes: u64,
    /// Default guaranteed working set per tenant (protected from reclaim).
    pub default_floor_bytes: u64,
    /// Default absolute per-tenant cap (runaway guard).
    pub default_hard_ceiling_bytes: u64,
    /// Pool-pressure threshold as a fraction of `total_bytes`; below it tenants
    /// borrow freely (elastic), above it the fair share is enforced. Default 0.9.
    pub high_watermark_frac: f64,
    /// Per-tenant overrides (paid tiers, large tenants).
    pub per_tenant: HashMap<String, TenantLimits>,
    /// Optional time-to-live for entries (None = no expiry; rely on size).
    pub ttl: Option<Duration>,
}

impl CacheBudget {
    /// A budget with a `total_bytes` pool and a default per-tenant
    /// `hard_ceiling_bytes` (absolute cap). Floor defaults to 0, high-watermark
    /// to 0.9, and the contended fair share is equal (`total / active_tenants`).
    pub fn new(total_bytes: u64, hard_ceiling_bytes: u64) -> Self {
        Self {
            total_bytes,
            default_floor_bytes: 0,
            default_hard_ceiling_bytes: hard_ceiling_bytes,
            high_watermark_frac: 0.9,
            per_tenant: HashMap::new(),
            ttl: None,
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Set the default guaranteed floor every tenant is protected up to.
    pub fn with_floor(mut self, floor_bytes: u64) -> Self {
        self.default_floor_bytes = floor_bytes;
        self
    }

    /// Override the pool-pressure threshold (fraction of `total_bytes`).
    pub fn with_high_watermark(mut self, frac: f64) -> Self {
        self.high_watermark_frac = frac;
        self
    }

    /// Override limits for a specific tenant (floor / hard ceiling / weight).
    pub fn with_tenant_limits(mut self, tenant: impl Into<String>, limits: TenantLimits) -> Self {
        self.per_tenant.insert(tenant.into(), limits);
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

/// A multitenant, byte-budgeted, **work-conserving elastic** cache over `V`.
pub struct TenantCache<V: Clone + Send + Sync + 'static> {
    inner: Cache<CacheKey, CachedValue<V>>,
    usage: Arc<DashMap<Arc<str>, TenantUsage>>,
    /// Live sum of admitted bytes across all tenants (pressure signal).
    global_bytes: Arc<AtomicU64>,
    total_bytes: u64,
    high_watermark: u64,
    default_floor: u64,
    default_hard_ceiling: u64,
    per_tenant: Arc<HashMap<String, TenantLimits>>,
    /// Optional tier-driven limits policy (Strategy seam); overrides the static
    /// per-tenant map when set.
    limits_resolver: Option<Arc<LimitsResolver>>,
}

impl<V: Clone + Send + Sync + 'static> TenantCache<V> {
    /// Build a cache for the given byte `budget`. The eviction listener keeps the
    /// per-tenant and global byte gauges accurate under pressure.
    pub fn new(budget: CacheBudget) -> Self {
        let usage: Arc<DashMap<Arc<str>, TenantUsage>> = Arc::new(DashMap::new());
        let global_bytes = Arc::new(AtomicU64::new(0));
        let listener_usage = usage.clone();
        let listener_global = global_bytes.clone();

        let mut builder = Cache::builder()
            .max_capacity(budget.total_bytes)
            .weigher(|_k: &CacheKey, v: &CachedValue<V>| v.weight)
            .eviction_listener(move |k: Arc<CacheKey>, v: CachedValue<V>, cause: RemovalCause| {
                listener_global.fetch_sub(v.weight as u64, Ordering::Relaxed);
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

        let high_watermark =
            ((budget.total_bytes as f64) * budget.high_watermark_frac.clamp(0.0, 1.0)) as u64;

        Self {
            inner: builder.build(),
            usage,
            global_bytes,
            total_bytes: budget.total_bytes,
            high_watermark,
            default_floor: budget.default_floor_bytes,
            default_hard_ceiling: budget.default_hard_ceiling_bytes,
            per_tenant: Arc::new(budget.per_tenant),
            limits_resolver: None,
        }
    }

    /// Plug a tier-driven limits policy (the Strategy seam): a
    /// `tenant_id → TenantLimits` resolver, e.g. catalog tier lookup +
    /// [`TenantTier::limits`]. Overrides the static per-tenant map.
    pub fn with_limits_resolver(mut self, resolver: Arc<LimitsResolver>) -> Self {
        self.limits_resolver = Some(resolver);
        self
    }

    fn usage_for(&self, tenant: &Arc<str>) -> dashmap::mapref::one::Ref<'_, Arc<str>, TenantUsage> {
        if let Some(u) = self.usage.get(tenant) {
            return u;
        }
        self.usage.entry(tenant.clone()).or_default();
        self.usage.get(tenant).expect("just inserted")
    }

    fn limits_for(&self, tenant: &str) -> TenantLimits {
        if let Some(resolver) = &self.limits_resolver {
            return resolver(tenant);
        }
        self.per_tenant.get(tenant).copied().unwrap_or(TenantLimits {
            floor_bytes: self.default_floor,
            hard_ceiling_bytes: self.default_hard_ceiling,
            weight: 1,
        })
    }

    /// Sum of weights over tenants currently holding bytes (for the contended
    /// weighted fair share). O(active tenants); fine for a footer/metadata cache.
    fn active_weight(&self) -> u64 {
        self.usage
            .iter()
            .filter(|e| e.value().bytes.load(Ordering::Relaxed) > 0)
            .map(|e| self.limits_for(e.key()).weight.max(1) as u64)
            .sum()
    }

    /// The work-conserving elastic admission decision for `tenant` adding
    /// `weight` bytes: absolute hard-ceiling cap always; below the high watermark
    /// borrow freely from the idle pool; above it, enforce the weighted fair
    /// share (`total * w_t / Σ active w`) clamped to `[floor, hard_ceiling]`.
    fn should_admit(&self, tenant: &Arc<str>, weight: u64) -> bool {
        let limits = self.limits_for(tenant);
        let u = self.usage_for(tenant).bytes.load(Ordering::Relaxed);
        // 1. absolute runaway guard — always enforced.
        if u.saturating_add(weight) > limits.hard_ceiling_bytes {
            return false;
        }
        // 2. pool not under pressure → elastic borrow up to the hard ceiling.
        let g = self.global_bytes.load(Ordering::Relaxed);
        if g.saturating_add(weight) <= self.high_watermark {
            return true;
        }
        // 3. under pressure → weighted fair share (with floor guarantee).
        let active_w = self.active_weight();
        let my_w = limits.weight.max(1) as u64;
        let fair = if active_w == 0 {
            limits.hard_ceiling_bytes
        } else {
            ((self.total_bytes as u128 * my_w as u128 / active_w as u128) as u64)
                .clamp(limits.floor_bytes, limits.hard_ceiling_bytes)
        };
        u.saturating_add(weight) <= fair
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

        if self.should_admit(&key.tenant, weight as u64) {
            self.account_insert(&key.tenant, weight);
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

    /// Explicitly insert/replace a value, subject to the elastic admission policy.
    pub async fn insert(&self, key: CacheKey, weight: u32, value: V) {
        if !self.should_admit(&key.tenant, weight as u64) {
            return;
        }
        self.account_insert(&key.tenant, weight);
        self.inner.insert(key, CachedValue { weight, value }).await;
    }

    fn account_insert(&self, tenant: &Arc<str>, weight: u32) {
        let u = self.usage_for(tenant);
        u.bytes.fetch_add(weight as u64, Ordering::Relaxed);
        u.inserts.fetch_add(1, Ordering::Relaxed);
        self.global_bytes.fetch_add(weight as u64, Ordering::Relaxed);
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
    async fn solo_tenant_borrows_idle_pool_up_to_hard_ceiling() {
        // Elasticity: with the pool mostly idle, a solo tenant borrows well past
        // the pressure watermark, up to its hard ceiling — no stranding.
        let budget = CacheBudget::new(1000, /*hard*/ 800).with_high_watermark(0.5); // hwm=500
        let c: TenantCache<u64> = TenantCache::new(budget);
        for i in 0..200u64 {
            c.insert(CacheKey::new("A", CacheKind::Footer, format!("k{i}")), 10, i)
                .await;
        }
        c.sync().await;
        let a = c.tenant_bytes("A");
        assert!(a > 500, "solo tenant {a} did not borrow past watermark (500)");
        assert!(a <= 800, "solo tenant {a} exceeded hard ceiling (800)");
    }

    #[tokio::test]
    async fn fair_share_bounds_tenants_under_pressure() {
        // Under contention, the weighted fair share (total/active) bounds every
        // tenant — no monopoly — while the pool stays fully used (no fragmentation).
        let budget = CacheBudget::new(1000, /*hard*/ 800).with_high_watermark(0.5);
        let c: TenantCache<u64> = TenantCache::new(budget);
        let tenants = ["A", "B", "C", "D"];
        // Round-robin so no tenant gets a head start.
        for round in 0..60u64 {
            for t in tenants {
                c.insert(CacheKey::new(t, CacheKind::Footer, format!("k{round}")), 10, round)
                    .await;
            }
        }
        c.sync().await;
        let total: u64 = tenants.iter().map(|t| c.tenant_bytes(t)).sum();
        assert!(total <= 1000 + 40, "pool over budget: {total}");
        // Fair share with 4 active ≈ 250; assert no tenant hoards (≤ ~share+slack).
        for t in tenants {
            let b = c.tenant_bytes(t);
            assert!(b <= 250 + 60, "tenant {t} hoarded {b} (> fair share)");
        }
        // And capacity is actually used (not stranded): pool well-filled.
        assert!(total >= 700, "pool under-utilized: {total}");
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

    #[tokio::test]
    async fn json_tier_policy_gives_higher_tier_preference_under_pressure() {
        // Operator-supplied JSON tier policy (the open-core seam): a higher-tier
        // tenant wins the contended fair share. Tier ids are arbitrary strings
        // (here mimicking the control plane's pricing tiers) — OSS bakes in none.
        let total = 1600u64;
        let json = r#"{
            "default_tier": "free_trial",
            "tiers": {
                "free_trial": {"weight": 1, "floor_frac": 0.0,    "ceiling_frac": 0.0625},
                "enterprise": {"weight": 8, "floor_frac": 0.0625, "ceiling_frac": 0.5}
            }
        }"#;
        let policy = Arc::new(TierPolicy::from_json(json).unwrap());
        // Host-supplied tenant→tier authority (in prod: TenantContext.tier).
        let tenant_to_tier: Arc<dyn Fn(&str) -> String + Send + Sync> =
            Arc::new(|t: &str| if t == "ent" { "enterprise".into() } else { "free_trial".into() });
        let resolver = policy.resolver(total, tenant_to_tier);

        let budget = CacheBudget::new(total, total).with_high_watermark(0.5); // hwm=800
        let c: TenantCache<u64> = TenantCache::new(budget).with_limits_resolver(resolver);

        for round in 0..200u64 {
            for t in ["ent", "free"] {
                c.insert(CacheKey::new(t, CacheKind::Footer, format!("k{round}")), 10, round)
                    .await;
            }
        }
        c.sync().await;
        let ent = c.tenant_bytes("ent");
        let free = c.tenant_bytes("free");
        assert!(
            ent > free * 3,
            "enterprise {ent} not strongly preferred over free {free}"
        );
        // Free is bounded by its tier ceiling (total/16 = 100).
        assert!(free <= 100 + 20, "free {free} exceeded its tier ceiling");
        // Enterprise bounded by its tier ceiling (total/2 = 800).
        assert!(ent <= 800 + 20, "enterprise {ent} exceeded its tier ceiling");
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
