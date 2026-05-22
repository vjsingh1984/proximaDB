// Tenant tier store — durable tier policy for multi-tenant SaaS enforcement.
//
// Holds the per-tenant policy that the LLD §3 planner and the §4 router both
// consult on every search:
//   - tier (free / pooled / dedicated)
//   - scan_budget_gb_hard       — router refuses to exceed this
//   - ef_search_cap             — beam-width hard ceiling
//   - freshness_sla_seconds     — async-ingest aging cap for the tenant
//   - feature_flags             — per-tenant + per-collection rollout switches
//
// Phase 0 backs the store with an in-process TTL cache populated from config;
// later phases hydrate it from the `anvaiops_tenant_tier` ProximaDB collection
// via the regular SDK read path. Callers depend on `TenantTierStore`, not the
// concrete backing, so the swap is transparent.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Tier identifier. Names match `docs/PRICING_INTERNAL.md` in the AnvaiOps repo.
///
/// **2026 Q2 consolidation** — the prior `Community` variant maps to `Team`.
/// Legacy stored values (`"community"`, `"starter"`, `"standard"`,
/// `"enterprise_pooled"`, `"enterprise_dedicated"`) deserialize transparently
/// via the serde aliases below so existing `anvaiops_tenant_tier` rows stay
/// readable without a data migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Shared pool, capped resources, evaluation usage.
    #[default]
    FreeTrial,
    /// Team — $19/mo pooled entry tier. Replaces the legacy `community`
    /// (and the prior `starter`/`standard` SKUs on the AnvaiOps side).
    #[serde(alias = "community", alias = "starter", alias = "standard")]
    Team,
    /// Pro — $199/mo pooled production tier with sync ingest + DR add-on.
    Pro,
    /// Business — $599/mo pooled tier with all 21 connectors + webhook bots.
    #[serde(alias = "enterprise_pooled")]
    Business,
    /// Enterprise — $1,500+/mo dedicated infrastructure, custom commits.
    #[serde(alias = "enterprise_dedicated")]
    Enterprise,
}

impl Tier {
    /// Default scan budget (GB) for the tier — the soft cap the gateway uses
    /// when the request omits `scan_budget_gb` and the per-tenant override is
    /// absent. Values mirror the Python `tier_cache._TIER_DEFAULTS` in the
    /// AnvaiOps repo and must move in lockstep with it.
    pub const fn default_scan_budget_gb(self) -> f64 {
        match self {
            Tier::FreeTrial => 1.0,
            Tier::Team => 4.0,
            Tier::Pro => 15.0,
            Tier::Business => 50.0,
            Tier::Enterprise => 256.0,
        }
    }

    /// Default beam-width / ef_search ceiling — hard ceiling at the router.
    pub const fn default_ef_search_cap(self) -> u32 {
        match self {
            Tier::FreeTrial => 64,
            Tier::Team => 160,
            Tier::Pro => 256,
            Tier::Business => 384,
            Tier::Enterprise => 1024,
        }
    }

    /// Default freshness SLA for async ingest, in seconds.
    pub const fn default_freshness_sla_seconds(self) -> u32 {
        match self {
            Tier::FreeTrial => 900,
            Tier::Team => 300,
            Tier::Pro => 120,
            Tier::Business => 60,
            Tier::Enterprise => 15,
        }
    }

    /// Label used on bounded-cardinality Prometheus counters. Must stay in the
    /// fixed set {free, team, pro, business, enterprise} to keep cardinality
    /// safe (see LLD `Multi-Tenant + SaaS Posture`).
    pub const fn prometheus_label(self) -> &'static str {
        match self {
            Tier::FreeTrial => "free",
            Tier::Team => "team",
            Tier::Pro => "pro",
            Tier::Business => "business",
            Tier::Enterprise => "enterprise",
        }
    }
}

/// Per-tenant feature rollout switches. Phase 0 ships with conservative
/// defaults; LLD §5 explicitly requires per-tenant **and** per-collection
/// gating for the quantized route, so this is intentionally bitfield-shaped
/// rather than a coarse enum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureFlags {
    /// QuIVer-style 2-bit Sign-Magnitude quantized search route.
    #[serde(default)]
    pub quantized_route: bool,
    /// Block-aware AXIS runtime (BAMG layout + graph tunneling).
    #[serde(default)]
    pub block_aware_axis: bool,
    /// Per-category cache policy (heterogeneous LLM workload separation).
    #[serde(default)]
    pub per_category_cache: bool,
    /// Catapult shortcut edges on warm collections.
    #[serde(default)]
    pub catapult_shortcuts: bool,
    /// Utility-aware reranker (UAE distillation path).
    #[serde(default)]
    pub utility_reranker: bool,
    /// Retrieval repair controller (SURE / Skill / S2G / Doctor-RAG).
    #[serde(default)]
    pub repair_controller: bool,
}

/// Policy record for a single tenant. Stored as a row in
/// `anvaiops_tenant_tier` and surfaced via `TenantTierStore`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantTierRecord {
    /// Tenant identifier (matches `X-Tenant-ID`).
    pub tenant_id: String,
    /// Tier classification.
    pub tier: Tier,
    /// Absolute scan ceiling enforced by the router (GB). Defaults to the
    /// tier's `default_scan_budget_gb` when unset.
    #[serde(default)]
    pub scan_budget_gb_hard: Option<f64>,
    /// Absolute beam-width / ef_search ceiling. Defaults to the tier's value.
    #[serde(default)]
    pub ef_search_cap: Option<u32>,
    /// Freshness SLA for async ingest in seconds.
    #[serde(default)]
    pub freshness_sla_seconds: Option<u32>,
    /// Feature rollout flags.
    #[serde(default)]
    pub feature_flags: FeatureFlags,
}

impl TenantTierRecord {
    /// Build a default record for a tenant the store has never seen — used as
    /// the fail-open path when the tier collection is unavailable. Conservative
    /// (FreeTrial) so an outage of the tier store can't lift caps.
    pub fn fail_safe(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            tier: Tier::FreeTrial,
            scan_budget_gb_hard: None,
            ef_search_cap: None,
            freshness_sla_seconds: None,
            feature_flags: FeatureFlags::default(),
        }
    }

    /// Effective hard scan budget — respects per-tenant override, otherwise
    /// uses the tier default.
    pub fn effective_scan_budget_gb(&self) -> f64 {
        self.scan_budget_gb_hard
            .unwrap_or_else(|| self.tier.default_scan_budget_gb())
    }

    /// Effective hard beam-width cap.
    pub fn effective_ef_search_cap(&self) -> u32 {
        self.ef_search_cap
            .unwrap_or_else(|| self.tier.default_ef_search_cap())
    }

    /// Effective async-ingest freshness SLA in seconds.
    pub fn effective_freshness_sla_seconds(&self) -> u32 {
        self.freshness_sla_seconds
            .unwrap_or_else(|| self.tier.default_freshness_sla_seconds())
    }
}

/// Outcome of evaluating an incoming search request against the tier policy.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetDecision {
    /// Request fits within the tenant tier ceiling.
    WithinBudget,
    /// Request exceeded the hard cap. The router must refuse with the
    /// reason embedded; the gateway will surface it as a 429 + explain.
    Exceeded {
        /// Which ceiling tripped — e.g. "scan_budget_gb_hard", "ef_search_cap".
        which: &'static str,
        /// The ceiling value.
        limit: f64,
        /// The requested value.
        requested: f64,
    },
}

/// Pluggable backing store for tenant tier records.
#[async_trait]
pub trait TenantTierStore: Send + Sync + 'static {
    /// Look up the tier record for a tenant. Returns `Ok(None)` when the
    /// store has no row — callers fall back to `TenantTierRecord::fail_safe`.
    async fn get(&self, tenant_id: &str) -> Result<Option<TenantTierRecord>>;

    /// Upsert a tier record (admin path). Defaults to unsupported; concrete
    /// backings may override.
    async fn put(&self, _record: TenantTierRecord) -> Result<()> {
        Err(anyhow::anyhow!("tenant tier store does not support writes"))
    }
}

/// In-process TTL cache wrapping any backing store. This is what the
/// request path actually depends on, so a hot read is one DashMap lookup.
///
/// The cache deliberately separates "miss in cache" from "miss in source":
/// a backing-store outage returns the last cached value past its TTL with a
/// `warn!` log, rather than collapsing to fail-safe, since collapsing every
/// in-flight request to FreeTrial during a brief outage would be a
/// self-inflicted incident.
pub struct CachedTenantTierStore {
    inner: Arc<dyn TenantTierStore>,
    ttl: Duration,
    cache: RwLock<HashMap<String, (Instant, TenantTierRecord)>>,
}

impl CachedTenantTierStore {
    /// Wrap a backing store with a TTL cache. 60 s is the default — short
    /// enough that tier upgrades take effect within a minute, long enough
    /// that the hot path is one RwLock read on the cached map.
    pub fn new(inner: Arc<dyn TenantTierStore>, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Fetch the tenant tier record. Returns the fail-safe record when the
    /// tenant is genuinely unknown to the backing store.
    pub async fn fetch(&self, tenant_id: &str) -> TenantTierRecord {
        let now = Instant::now();
        if let Some((stored_at, record)) = self.cache.read().await.get(tenant_id).cloned() {
            if now.duration_since(stored_at) < self.ttl {
                return record;
            }
        }

        match self.inner.get(tenant_id).await {
            Ok(Some(record)) => {
                self.cache
                    .write()
                    .await
                    .insert(tenant_id.to_string(), (now, record.clone()));
                record
            }
            Ok(None) => {
                debug!(tenant_id, "tier store has no row; returning fail-safe");
                TenantTierRecord::fail_safe(tenant_id)
            }
            Err(err) => {
                // Soft-fail: if we have a stale cache entry, keep using it.
                if let Some((_, record)) = self.cache.read().await.get(tenant_id).cloned() {
                    warn!(
                        tenant_id,
                        error = %err,
                        "tier store outage; serving stale cached record",
                    );
                    return record;
                }
                warn!(
                    tenant_id,
                    error = %err,
                    "tier store outage and no cached record; using fail-safe",
                );
                TenantTierRecord::fail_safe(tenant_id)
            }
        }
    }

    /// Evaluate a search request against the tenant's hard scan-budget cap.
    /// The gateway should also apply a soft cap before this is reached.
    pub fn check_scan_budget(
        &self,
        record: &TenantTierRecord,
        requested_scan_gb: f64,
    ) -> BudgetDecision {
        let limit = record.effective_scan_budget_gb();
        if requested_scan_gb > limit {
            BudgetDecision::Exceeded {
                which: "scan_budget_gb_hard",
                limit,
                requested: requested_scan_gb,
            }
        } else {
            BudgetDecision::WithinBudget
        }
    }

    /// Evaluate a search request against the tenant's hard ef_search cap.
    pub fn check_ef_search(&self, record: &TenantTierRecord, requested_ef: u32) -> BudgetDecision {
        let limit = record.effective_ef_search_cap();
        if requested_ef > limit {
            BudgetDecision::Exceeded {
                which: "ef_search_cap",
                limit: f64::from(limit),
                requested: f64::from(requested_ef),
            }
        } else {
            BudgetDecision::WithinBudget
        }
    }
}

/// In-memory backing for tests, local dev, and the Phase 0 default. Populated
/// from `config.toml` at startup; mutated only by the admin API.
pub struct InMemoryTenantTierStore {
    rows: RwLock<HashMap<String, TenantTierRecord>>,
}

impl InMemoryTenantTierStore {
    /// Empty store — every lookup returns `None`, so callers fall back to
    /// `TenantTierRecord::fail_safe`.
    pub fn empty() -> Self {
        Self {
            rows: RwLock::new(HashMap::new()),
        }
    }

    /// Build with an initial set of rows.
    pub fn with_rows(rows: Vec<TenantTierRecord>) -> Self {
        let map = rows.into_iter().map(|r| (r.tenant_id.clone(), r)).collect();
        Self {
            rows: RwLock::new(map),
        }
    }
}

#[async_trait]
impl TenantTierStore for InMemoryTenantTierStore {
    async fn get(&self, tenant_id: &str) -> Result<Option<TenantTierRecord>> {
        Ok(self.rows.read().await.get(tenant_id).cloned())
    }

    async fn put(&self, record: TenantTierRecord) -> Result<()> {
        self.rows
            .write()
            .await
            .insert(record.tenant_id.clone(), record);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fail_safe_is_free_trial() {
        let r = TenantTierRecord::fail_safe("unknown-tenant");
        assert_eq!(r.tier, Tier::FreeTrial);
        assert_eq!(r.effective_scan_budget_gb(), 1.0);
        assert_eq!(r.effective_ef_search_cap(), 64);
        assert!(!r.feature_flags.quantized_route);
    }

    #[tokio::test]
    async fn ttl_cache_serves_known_tenant() {
        let store = Arc::new(InMemoryTenantTierStore::with_rows(vec![TenantTierRecord {
            tenant_id: "tenant-acme".into(),
            tier: Tier::Business,
            scan_budget_gb_hard: Some(8.0),
            ef_search_cap: None,
            freshness_sla_seconds: None,
            feature_flags: FeatureFlags::default(),
        }]));
        let cache = CachedTenantTierStore::new(store, Duration::from_secs(60));
        let record = cache.fetch("tenant-acme").await;
        assert_eq!(record.tier, Tier::Business);
        assert_eq!(record.effective_scan_budget_gb(), 8.0);
        // Override only set the budget; ef_search defaults to Business tier.
        assert_eq!(
            record.effective_ef_search_cap(),
            Tier::Business.default_ef_search_cap()
        );
    }

    #[tokio::test]
    async fn unknown_tenant_returns_fail_safe() {
        let store = Arc::new(InMemoryTenantTierStore::empty());
        let cache = CachedTenantTierStore::new(store, Duration::from_secs(60));
        let record = cache.fetch("never-seen").await;
        assert_eq!(record.tier, Tier::FreeTrial);
    }

    #[tokio::test]
    async fn budget_exceeded_emits_structured_decision() {
        let store = Arc::new(InMemoryTenantTierStore::with_rows(vec![TenantTierRecord {
            tenant_id: "t".into(),
            tier: Tier::Team,
            scan_budget_gb_hard: Some(1.0),
            ef_search_cap: Some(96),
            freshness_sla_seconds: None,
            feature_flags: FeatureFlags::default(),
        }]));
        let cache = CachedTenantTierStore::new(store, Duration::from_secs(60));
        let record = cache.fetch("t").await;
        match cache.check_scan_budget(&record, 2.5) {
            BudgetDecision::Exceeded {
                which,
                limit,
                requested,
            } => {
                assert_eq!(which, "scan_budget_gb_hard");
                assert!((limit - 1.0).abs() < 1e-9);
                assert!((requested - 2.5).abs() < 1e-9);
            }
            other => panic!("expected Exceeded, got {other:?}"),
        }
        assert_eq!(
            cache.check_scan_budget(&record, 0.5),
            BudgetDecision::WithinBudget
        );
        match cache.check_ef_search(&record, 256) {
            BudgetDecision::Exceeded { which, .. } => assert_eq!(which, "ef_search_cap"),
            other => panic!("expected Exceeded for ef_search, got {other:?}"),
        }
    }

    #[test]
    fn prometheus_labels_are_bounded() {
        // Bounded-cardinality contract: every tier must produce a fixed label
        // from the {free, team, pro, business, enterprise} set so per-second
        // Prometheus counters never grow with tenant count.
        let labels: Vec<_> = [
            Tier::FreeTrial,
            Tier::Team,
            Tier::Pro,
            Tier::Business,
            Tier::Enterprise,
        ]
        .iter()
        .map(|t| t.prometheus_label())
        .collect();
        assert_eq!(
            labels,
            vec!["free", "team", "pro", "business", "enterprise"]
        );
    }

    #[test]
    fn legacy_tier_aliases_deserialize_to_team() {
        // 2026 Q2 consolidation: stored tier values from the old AnvaiOps
        // ladder (Starter/Standard/Community) must continue to load as Team
        // so we don't need a one-shot migration over the tenant registry.
        for raw in ["\"community\"", "\"starter\"", "\"standard\""] {
            let parsed: Tier = serde_json::from_str(raw).expect("deserialize legacy tier");
            assert_eq!(parsed, Tier::Team, "expected {raw} → Team");
        }
    }

    #[test]
    fn legacy_enterprise_aliases_deserialize_to_canonical() {
        let pooled: Tier =
            serde_json::from_str("\"enterprise_pooled\"").expect("enterprise_pooled");
        assert_eq!(pooled, Tier::Business);
        let dedicated: Tier =
            serde_json::from_str("\"enterprise_dedicated\"").expect("enterprise_dedicated");
        assert_eq!(dedicated, Tier::Enterprise);
    }

    #[test]
    fn paid_tier_scan_budgets_grow_monotonically() {
        // Ratio rule: scan budget at tier (x+1) must be ≥ tier (x). Without
        // this an upgrade could leave a customer with less budget than they
        // had on the prior tier.
        let ladder = [
            Tier::FreeTrial,
            Tier::Team,
            Tier::Pro,
            Tier::Business,
            Tier::Enterprise,
        ];
        for w in ladder.windows(2) {
            let (lo, hi) = (w[0], w[1]);
            assert!(
                hi.default_scan_budget_gb() >= lo.default_scan_budget_gb(),
                "{hi:?} scan budget must be >= {lo:?}"
            );
            assert!(
                hi.default_ef_search_cap() >= lo.default_ef_search_cap(),
                "{hi:?} ef_search cap must be >= {lo:?}"
            );
            assert!(
                hi.default_freshness_sla_seconds() <= lo.default_freshness_sla_seconds(),
                "{hi:?} freshness SLA must be tighter than {lo:?}"
            );
        }
    }
}
