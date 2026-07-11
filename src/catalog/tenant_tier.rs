// Tier definition system pre-extracted to foundation proximadb-tenant (Slice D).
use anyhow::Result;
use async_trait::async_trait;
pub use proximadb_tenant::{
    FeatureFlags, ObjectEconomyQuantizationCeiling, TenantTierRecord, Tier,
    TierObjectEconomyConfig, tier_config,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

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
        if let Some((stored_at, record)) = self.cache.read().await.get(tenant_id).cloned()
            && now.duration_since(stored_at) < self.ttl
        {
            return record;
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

// ── C5: governance tier cost-multiplier startup adapter (Dimension 5) ───────
//
// The route cost model exposes a tenant→multiplier Port
// (`route_cost_model::set_tier_multiplier_resolver`, default 1.0). This is the
// OSS adapter that fills it from the tier-config overlay (anvaiops policy values)
// without leaking the `Tier` enum into the routing/cost path:
//
//   tenant_id ──(tenant→tier Port)──▶ Tier ──(tier-config)──▶ cost_multiplier
//
// The tenant→tier half is itself a Port: OSS standalone has no per-tenant tier
// authority (that lives in the control plane's registry), so the default
// resolves every tenant to the configured default tier — whose multiplier is
// 1.0 in the OSS baseline, keeping the whole objective inert until a deployment
// both registers a tenant→tier resolver AND ships non-neutral tier multipliers.

type TenantTierFn = dyn Fn(&str) -> Tier + Send + Sync;

static TENANT_TIER_RESOLVER: Mutex<Option<Box<TenantTierFn>>> = Mutex::new(None);

/// Install (or clear with `None`) the sync tenant→tier resolver Port. The
/// control-plane integration registers one mapping a tenant id to its `Tier`
/// from its authority (e.g. a warmed snapshot of the tenant registry); OSS
/// standalone leaves it unset, so every tenant resolves to the configured
/// default tier. Sync by contract — the route cost model consults it inline.
pub fn set_tenant_tier_resolver(resolver: Option<Box<TenantTierFn>>) {
    *TENANT_TIER_RESOLVER
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = resolver;
}

/// The configured default tier (`tier_config().default_tier`, alias-aware), or
/// [`Tier::default`] if it somehow fails to parse.
pub fn default_tier() -> Tier {
    let raw = serde_json::Value::String(tier_config().default_tier.clone());
    serde_json::from_value::<Tier>(raw).unwrap_or_default()
}

/// Resolve a tenant to its `Tier` via the registered Port, falling back to the
/// configured default tier when none is installed (OSS standalone).
pub fn resolve_tier_for(tenant_id: &str) -> Tier {
    match TENANT_TIER_RESOLVER
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        Some(resolve) => resolve(tenant_id),
        None => default_tier(),
    }
}

/// C5 startup adapter: install the config-driven tier cost-multiplier resolver
/// into the route cost model. Called once at startup (after
/// `route_cost_model::install_route_cost_observer`). Maps tenant → tier →
/// `Tier::cost_multiplier`, so `route_cost_model::final_cost` reports the real
/// per-tenant `Cost(q)`. Default-inert: with no tenant→tier resolver and the
/// neutral baseline multipliers, every tenant resolves to `1.0`.
pub fn install_tier_cost_multiplier_resolver() {
    crate::query::route_cost_model::set_tier_multiplier_resolver(Some(Box::new(|tenant_id| {
        resolve_tier_for(tenant_id).cost_multiplier()
    })));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fail_safe_is_free_trial() {
        let r = TenantTierRecord::fail_safe("unknown-tenant");
        assert_eq!(r.tier, Tier::Tier1);
        assert_eq!(r.effective_scan_budget_gb(), 1.0);
        assert_eq!(r.effective_ef_search_cap(), 64);
        assert!(!r.feature_flags.quantized_route);
    }

    #[tokio::test]
    async fn ttl_cache_serves_known_tenant() {
        let store = Arc::new(InMemoryTenantTierStore::with_rows(vec![TenantTierRecord {
            tenant_id: "tenant-acme".into(),
            tier: Tier::Tier4,
            scan_budget_gb_hard: Some(8.0),
            ef_search_cap: None,
            freshness_sla_seconds: None,
            feature_flags: FeatureFlags::default(),
        }]));
        let cache = CachedTenantTierStore::new(store, Duration::from_secs(60));
        let record = cache.fetch("tenant-acme").await;
        assert_eq!(record.tier, Tier::Tier4);
        assert_eq!(record.effective_scan_budget_gb(), 8.0);
        // Override only set the budget; ef_search defaults to Business tier.
        assert_eq!(
            record.effective_ef_search_cap(),
            Tier::Tier4.default_ef_search_cap()
        );
    }

    #[tokio::test]
    async fn unknown_tenant_returns_fail_safe() {
        let store = Arc::new(InMemoryTenantTierStore::empty());
        let cache = CachedTenantTierStore::new(store, Duration::from_secs(60));
        let record = cache.fetch("never-seen").await;
        assert_eq!(record.tier, Tier::Tier1);
    }

    #[tokio::test]
    async fn budget_exceeded_emits_structured_decision() {
        let store = Arc::new(InMemoryTenantTierStore::with_rows(vec![TenantTierRecord {
            tenant_id: "t".into(),
            tier: Tier::Tier2,
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
    fn tier_config_loads_without_panic_and_matches_enum() {
        // First access of `tier_config()` deserializes the embedded JSON, asserts
        // schema_version == 1, and runs `validate_tier_config_matches_enum`. If
        // any of those checks fail we panic here — surfaces a malformed or
        // drifted `config/tier-config.json` at test time rather than first
        // production request.
        let cfg = tier_config();
        assert_eq!(cfg.schema_version, 1);
        let json_ids: std::collections::HashSet<&str> =
            cfg.tiers.iter().map(|t| t.id.as_str()).collect();
        let enum_ids: std::collections::HashSet<&str> =
            Tier::all().iter().map(|t| t.id()).collect();
        assert_eq!(json_ids, enum_ids);
    }

    #[test]
    fn every_tier_id_round_trips_through_tier_lookup() {
        for tier in Tier::all() {
            // Every variant must find a matching row, and the loaded numbers
            // must be positive / non-zero — guards against an incomplete
            // tier-config.json that compiles but stalls the router with NaN.
            assert!(tier.default_scan_budget_gb() > 0.0, "{:?}", tier);
            assert!(tier.default_ef_search_cap() > 0, "{:?}", tier);
            assert!(tier.default_freshness_sla_seconds() > 0, "{:?}", tier);
            assert!(!tier.prometheus_label().is_empty(), "{:?}", tier);
        }
    }

    #[test]
    fn prometheus_labels_are_bounded() {
        // Bounded-cardinality contract: every tier must produce a fixed label
        // from a small enumerated set so per-second Prometheus counters never
        // grow with tenant count. After the Phase B-4 rename the bundled
        // baseline produces operator-neutral positional labels
        // (`tier1`..`tier5`); operators who overlay a runtime tier-config
        // with their own `prom_label` per tier override these values
        // without changing the cardinality contract.
        let labels: Vec<_> = [
            Tier::Tier1,
            Tier::Tier2,
            Tier::Tier3,
            Tier::Tier4,
            Tier::Tier5,
        ]
        .iter()
        .map(|t| t.prometheus_label())
        .collect();
        assert_eq!(labels, vec!["tier1", "tier2", "tier3", "tier4", "tier5"]);
    }

    #[test]
    fn legacy_tier_aliases_deserialize_to_team() {
        // Tier consolidation: stored tier values from the older ladder
        // (Starter / Standard / Community) must continue to load as Team
        // so deployments don't need a one-shot migration over the tenant
        // registry.
        for raw in ["\"community\"", "\"starter\"", "\"standard\""] {
            let parsed: Tier = serde_json::from_str(raw).expect("deserialize legacy tier");
            assert_eq!(parsed, Tier::Tier2, "expected {raw} → Team");
        }
    }

    #[test]
    fn legacy_enterprise_aliases_deserialize_to_canonical() {
        let pooled: Tier =
            serde_json::from_str("\"enterprise_pooled\"").expect("enterprise_pooled");
        assert_eq!(pooled, Tier::Tier4);
        let dedicated: Tier =
            serde_json::from_str("\"enterprise_dedicated\"").expect("enterprise_dedicated");
        assert_eq!(dedicated, Tier::Tier5);
    }

    #[test]
    fn paid_tier_scan_budgets_grow_monotonically() {
        // Ratio rule: scan budget at tier (x+1) must be ≥ tier (x). Without
        // this an upgrade could leave a customer with less budget than they
        // had on the prior tier.
        let ladder = [
            Tier::Tier1,
            Tier::Tier2,
            Tier::Tier3,
            Tier::Tier4,
            Tier::Tier5,
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

    #[test]
    fn object_economy_config_increases_with_tier() {
        // Lower tiers should have stricter limits
        let t1 = Tier::Tier1.object_economy_config();
        let t2 = Tier::Tier2.object_economy_config();
        let t3 = Tier::Tier3.object_economy_config();
        let t4 = Tier::Tier4.object_economy_config();
        let t5 = Tier::Tier5.object_economy_config();

        // max_blocks_per_query should increase monotonically
        assert!(t2.max_blocks_per_query > t1.max_blocks_per_query);
        assert!(t3.max_blocks_per_query > t2.max_blocks_per_query);
        assert!(t4.max_blocks_per_query > t3.max_blocks_per_query);
        assert_eq!(t5.max_blocks_per_query, u32::MAX);

        // quantization ceiling should rise (cap loosens) with tier
        assert_eq!(
            t1.quantization_ceiling,
            ObjectEconomyQuantizationCeiling::INT8
        );
        assert_eq!(
            t2.quantization_ceiling,
            ObjectEconomyQuantizationCeiling::INT8
        );
        assert_eq!(
            t3.quantization_ceiling,
            ObjectEconomyQuantizationCeiling::FP16
        );
        assert_eq!(
            t4.quantization_ceiling,
            ObjectEconomyQuantizationCeiling::FP16
        );
        assert_eq!(
            t5.quantization_ceiling,
            ObjectEconomyQuantizationCeiling::FP32
        );
    }

    #[test]
    fn tier1_has_strictest_object_economy_limits() {
        let config = Tier::Tier1.object_economy_config();
        assert!(config.allow_centroid_routing); // Enabled to reduce costs
        assert!(config.allow_zorder_pruning);
        assert_eq!(config.max_blocks_per_query, 100);
        assert_eq!(
            config.quantization_ceiling,
            ObjectEconomyQuantizationCeiling::INT8
        );
    }

    #[test]
    fn tier5_has_no_object_economy_limits() {
        let config = Tier::Tier5.object_economy_config();
        assert!(config.allow_centroid_routing);
        assert!(config.allow_zorder_pruning);
        assert_eq!(config.max_blocks_per_query, u32::MAX);
        assert_eq!(
            config.quantization_ceiling,
            ObjectEconomyQuantizationCeiling::FP32
        );
    }

    // ── C5 tier cost-multiplier adapter ─────────────────────────────────────

    #[test]
    fn cost_multiplier_is_neutral_in_oss_baseline() {
        // The baked OSS baseline overlay ships no cost_multiplier → neutral 1.0
        // on every tier (the objective stays inert until policy ships values).
        for &t in Tier::all() {
            assert_eq!(t.cost_multiplier(), 1.0, "{t:?} baseline multiplier");
        }
    }

    #[test]
    fn from_claim_parses_canonical_and_legacy_ids() {
        // Canonical ids and the control-plane / legacy aliases both resolve.
        assert_eq!(Tier::from_claim("tier1"), Some(Tier::Tier1));
        assert_eq!(Tier::from_claim("free_trial"), Some(Tier::Tier1));
        assert_eq!(Tier::from_claim("free"), Some(Tier::Tier1));
        assert_eq!(Tier::from_claim("pro"), Some(Tier::Tier3));
        assert_eq!(Tier::from_claim("business"), Some(Tier::Tier4));
        assert_eq!(Tier::from_claim("enterprise"), Some(Tier::Tier5));
        assert_eq!(Tier::from_claim("  tier5  "), Some(Tier::Tier5));
        // Unknown / empty → None (caller uses the default tier).
        assert_eq!(Tier::from_claim("nonsense"), None);
        assert_eq!(Tier::from_claim(""), None);
    }

    #[test]
    fn tenant_tier_resolver_port_defaults_to_config_default_then_honors_override() {
        // No resolver installed → the configured default tier.
        set_tenant_tier_resolver(None);
        assert_eq!(resolve_tier_for("anyone"), default_tier());
        // A registered resolver (the control-plane adapter) maps per tenant.
        set_tenant_tier_resolver(Some(Box::new(|t: &str| {
            if t == "vip" { Tier::Tier5 } else { Tier::Tier1 }
        })));
        assert_eq!(resolve_tier_for("vip"), Tier::Tier5);
        assert_eq!(resolve_tier_for("other"), Tier::Tier1);
        set_tenant_tier_resolver(None); // reset process-global for sibling tests
    }

    #[test]
    fn install_wires_the_route_cost_model_tier_multiplier() {
        use crate::query::route_cost_model::{
            set_tier_multiplier_resolver, tier_entitlement_multiplier,
        };
        // Map a tenant to a concrete tier, install the adapter, and confirm the
        // route cost model resolves that tenant's multiplier through it. Baseline
        // multipliers are the neutral 1.0, so the assertion is also race-robust.
        set_tenant_tier_resolver(Some(Box::new(|_| Tier::Tier5)));
        install_tier_cost_multiplier_resolver();
        assert_eq!(
            tier_entitlement_multiplier(Some("vip")),
            Tier::Tier5.cost_multiplier()
        );
        // reset both process-globals so other tests are unaffected.
        set_tier_multiplier_resolver(None);
        set_tenant_tier_resolver(None);
    }
}
