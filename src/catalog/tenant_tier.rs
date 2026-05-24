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
// later phases hydrate it from a system-internal tenant-tier collection via
// the regular SDK read path. The collection name is operator-configurable
// (defaults to `proximadb_tenant_tier`). Callers depend on `TenantTierStore`,
// not the concrete backing, so the swap is transparent.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, warn};

// ── Tier config (embedded baseline; runtime overlay loaded by entrypoint) ───
//
// The compile-time-embedded `config/pricing.json` provides the baseline
// numeric defaults so the server binary stays self-contained for offline /
// air-gapped deployments. Runtime deployments overlay this via the entrypoint
// script (`deploy/docker/entrypoint.sh`) which fetches from
// `PROXIMADB_TIER_CONFIG_URL` and atomic-replaces the on-disk config before
// the server starts. See `config/TIER_CONFIG.md` for the operator-neutral
// schema; the embedded baseline currently uses the richer legacy schema
// during the migration window (Phase B-4 will flip the embedded file to
// the stripped tier-config.json shape).
//
// The numeric defaults (scan_budget_gb, ef_search_cap, freshness_sla_seconds,
// prom_label) load from this JSON at first access. The `Tier` enum variants
// themselves stay compile-time exhaustive — Rust enums cannot be built from
// runtime data. The startup assertion `pricing_matches_enum()` panics if the
// JSON tier set diverges from the enum variants, surfacing the drift at
// process start rather than at the first soft-cap rejection.

const PRICING_JSON_BYTES: &str = include_str!("../../config/pricing.json");

static PRICING: OnceLock<PricingConfig> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct PricingConfig {
    schema_version: u32,
    #[allow(dead_code)]
    default_tier: String,
    tiers: Vec<PricingTier>,
}

#[derive(Debug, Deserialize)]
struct PricingTier {
    id: String,
    prom_label: String,
    soft_caps: PricingSoftCaps,
}

#[derive(Debug, Deserialize)]
struct PricingSoftCaps {
    scan_budget_gb: f64,
    ef_search_cap: u32,
    freshness_sla_seconds: u32,
}

fn pricing() -> &'static PricingConfig {
    PRICING.get_or_init(|| {
        let parsed: PricingConfig = serde_json::from_str(PRICING_JSON_BYTES)
            .expect("config/pricing.json is malformed — proximaDB cannot start");
        assert_eq!(
            parsed.schema_version, 1,
            "config/pricing.json has unsupported schema_version {}",
            parsed.schema_version
        );
        validate_pricing_matches_enum(&parsed);
        parsed
    })
}

fn validate_pricing_matches_enum(p: &PricingConfig) {
    use std::collections::HashSet;
    let json_ids: HashSet<&str> = p.tiers.iter().map(|t| t.id.as_str()).collect();
    let enum_ids: HashSet<&str> = Tier::all().iter().map(|t| t.id()).collect();
    assert_eq!(
        json_ids, enum_ids,
        "config/pricing.json tier ids must match Tier enum variants exactly; \
         add/remove a Tier variant or update pricing/tiers.json. \
         json={json_ids:?} enum={enum_ids:?}"
    );
}

fn pricing_row(tier: Tier) -> &'static PricingTier {
    let id = tier.id();
    pricing()
        .tiers
        .iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("tier {id} missing from config/pricing.json"))
}

/// Tier identifier — the baseline tier set shipped with ProximaDB. Operator
/// deployments can override the numeric caps per tier via the runtime
/// tier-config overlay (see `config/TIER_CONFIG.md`), but the variant set
/// itself is compile-time exhaustive because Rust enums can't be built from
/// runtime data.
///
/// Legacy stored values (`"community"`, `"starter"`, `"standard"`,
/// `"enterprise_pooled"`, `"enterprise_dedicated"`) deserialize transparently
/// via the serde aliases below so existing tenant-tier rows stay readable
/// across a tier-naming consolidation without a data migration.
///
/// A future major version (Phase B-4) may rename these variants to fully
/// operator-neutral names (e.g., `Tier1` / `Tier2` / `Tier3`); the current
/// names are kept stable to avoid breaking external callers in this
/// release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Shared pool, capped resources, evaluation usage.
    #[default]
    FreeTrial,
    /// Entry pooled tier — replaces the legacy `community` / `starter` /
    /// `standard` SKUs from earlier naming conventions.
    #[serde(alias = "community", alias = "starter", alias = "standard")]
    Team,
    /// Production pooled tier with sync ingest support.
    Pro,
    /// Pooled tier with full connector set + webhook integration support.
    #[serde(alias = "enterprise_pooled")]
    Business,
    /// Dedicated single-tenant infrastructure tier.
    #[serde(alias = "enterprise_dedicated")]
    Enterprise,
}

impl Tier {
    /// Stable enum-id used as the JSON key in the embedded
    /// `config/pricing.json` baseline (and any runtime tier-config overlay).
    pub const fn id(self) -> &'static str {
        match self {
            Tier::FreeTrial => "free_trial",
            Tier::Team => "team",
            Tier::Pro => "pro",
            Tier::Business => "business",
            Tier::Enterprise => "enterprise",
        }
    }

    /// All declared tier variants in display order. Kept in sync with the
    /// canonical JSON via `validate_pricing_matches_enum()` at startup.
    pub const fn all() -> &'static [Tier] {
        &[
            Tier::FreeTrial,
            Tier::Team,
            Tier::Pro,
            Tier::Business,
            Tier::Enterprise,
        ]
    }

    /// Default scan budget (GB). Loaded from the embedded baseline
    /// `config/pricing.json` at first access. Operators who overlay a runtime
    /// tier-config replace these defaults at process start (see
    /// `config/TIER_CONFIG.md`); drift between operator gateway + engine is
    /// avoided because the engine sources its values from the same overlay
    /// the operator publishes.
    pub fn default_scan_budget_gb(self) -> f64 {
        pricing_row(self).soft_caps.scan_budget_gb
    }

    /// Default beam-width / ef_search ceiling — hard ceiling at the router.
    pub fn default_ef_search_cap(self) -> u32 {
        pricing_row(self).soft_caps.ef_search_cap
    }

    /// Default freshness SLA for async ingest, in seconds.
    pub fn default_freshness_sla_seconds(self) -> u32 {
        pricing_row(self).soft_caps.freshness_sla_seconds
    }

    /// Bounded Prometheus label — cardinality-safe. The label set is loaded
    /// from the embedded baseline `config/pricing.json`; operators who add a
    /// tier via the runtime overlay (see `config/TIER_CONFIG.md`) widen the
    /// metric label set automatically without a recompile.
    pub fn prometheus_label(self) -> &'static str {
        pricing_row(self).prom_label.as_str()
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

/// Policy record for a single tenant. Stored as a row in the
/// operator-configured tenant-tier collection and surfaced via
/// `TenantTierStore`.
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
    fn pricing_config_loads_without_panic_and_matches_enum() {
        // First access of `pricing()` deserializes the embedded JSON, asserts
        // schema_version == 1, and runs `validate_pricing_matches_enum`. If
        // any of those checks fail we panic here — surfaces a malformed or
        // drifted `config/pricing.json` at test time rather than first
        // production request.
        let cfg = pricing();
        assert_eq!(cfg.schema_version, 1);
        let json_ids: std::collections::HashSet<&str> =
            cfg.tiers.iter().map(|t| t.id.as_str()).collect();
        let enum_ids: std::collections::HashSet<&str> =
            Tier::all().iter().map(|t| t.id()).collect();
        assert_eq!(json_ids, enum_ids);
    }

    #[test]
    fn every_tier_id_round_trips_through_pricing_lookup() {
        for tier in Tier::all() {
            // Every variant must find a matching row, and the loaded numbers
            // must be positive / non-zero — guards against an incomplete
            // pricing.json that compiles but stalls the router with NaN.
            assert!(tier.default_scan_budget_gb() > 0.0, "{:?}", tier);
            assert!(tier.default_ef_search_cap() > 0, "{:?}", tier);
            assert!(tier.default_freshness_sla_seconds() > 0, "{:?}", tier);
            assert!(!tier.prometheus_label().is_empty(), "{:?}", tier);
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
        // Tier consolidation: stored tier values from the older ladder
        // (Starter / Standard / Community) must continue to load as Team
        // so deployments don't need a one-shot migration over the tenant
        // registry.
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
