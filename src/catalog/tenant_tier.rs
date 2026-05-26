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

// ── Tier config (compile-time baseline + runtime overlay) ───────────────────
//
// Three resolution layers (first hit wins):
//
//   1. **Runtime overlay** at `PROXIMADB_TIER_CONFIG_PATH` (default
//      `/config/tier-config.json`). Operators who want to ship per-deployment
//      tier definitions (different caps per environment, AnvaiOps's
//      commercial pricing baked into the AnvaiOps overlay image, etc.) write
//      to this path either via Dockerfile `COPY` at image build time or via
//      the entrypoint script's optional URL fetch at container boot. Phase
//      B-5: this layer is now actually CONSUMED by the engine (prior to B-5
//      the file was written but never read, making the overlay cosmetic).
//
//   2. **Legacy runtime overlay** at `/config/pricing.json`. Backward-
//      compat for image overlays + entrypoint fetches that targeted the
//      pre-B-5 path. Will be removed in a future major version once the
//      ecosystem has migrated.
//
//   3. **Compile-time baseline** baked via `include_str!` from
//      `config/tier-config.json`. Self-contained fallback so the server
//      binary works for offline / air-gapped deployments and OSS adopters
//      who haven't supplied an overlay.
//
// The numeric defaults (scan_budget_gb, ef_search_cap, freshness_sla_seconds,
// prom_label) load from the chosen layer at first access. The `Tier` enum
// variants themselves stay compile-time exhaustive — Rust enums cannot be
// built from runtime data. The startup assertion `validate_pricing_matches_enum`
// panics if the chosen layer's tier set diverges from the enum variants,
// surfacing the drift at process start rather than at first soft-cap
// rejection. Validation is alias-aware: overlay JSONs may use any tier id
// that maps to a Tier variant via its serde aliases (so legacy ids like
// `free_trial`/`team`/`pro`/`business`/`enterprise` continue to validate
// against the post-B-4 `tier1`..`tier5` enum without a data migration).

const BAKED_TIER_CONFIG_BYTES: &str = include_str!("../../config/tier-config.json");
const RUNTIME_TIER_CONFIG_ENV: &str = "PROXIMADB_TIER_CONFIG_PATH";
const DEFAULT_RUNTIME_TIER_CONFIG_PATH: &str = "/config/tier-config.json";
const LEGACY_RUNTIME_TIER_CONFIG_PATH: &str = "/config/pricing.json";

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

/// Resolved tier-config source: where it came from + its raw JSON bytes.
/// The `label` flows into log lines + panic messages so the source is
/// always identifiable in failure modes.
struct TierConfigSource {
    label: String,
    content: String,
}

/// Resolve the tier-config source per the three-layer precedence:
/// runtime-overlay (env-configured or default) → legacy-overlay → baked.
fn resolve_tier_config_source() -> TierConfigSource {
    let runtime_path = std::env::var(RUNTIME_TIER_CONFIG_ENV)
        .unwrap_or_else(|_| DEFAULT_RUNTIME_TIER_CONFIG_PATH.to_string());

    // Layer 1: runtime overlay at the configured path.
    if let Ok(content) = std::fs::read_to_string(&runtime_path) {
        return TierConfigSource {
            label: format!("runtime overlay {runtime_path}"),
            content,
        };
    }

    // Layer 2: legacy overlay (pre-B-5 path), only if the configured path
    // is the new default. If operator explicitly set PROXIMADB_TIER_CONFIG_PATH
    // to something else and it was missing, don't silently fall back to the
    // legacy path — that would mask a config error.
    if runtime_path == DEFAULT_RUNTIME_TIER_CONFIG_PATH {
        if let Ok(content) = std::fs::read_to_string(LEGACY_RUNTIME_TIER_CONFIG_PATH) {
            return TierConfigSource {
                label: format!(
                    "legacy runtime overlay {LEGACY_RUNTIME_TIER_CONFIG_PATH} \
                     (deprecated; rename to {DEFAULT_RUNTIME_TIER_CONFIG_PATH})"
                ),
                content,
            };
        }
    }

    // Layer 3: compile-time baked baseline. Self-contained fallback for
    // air-gapped / OSS-default deployments.
    TierConfigSource {
        label: "compile-time baked config/tier-config.json".to_string(),
        content: BAKED_TIER_CONFIG_BYTES.to_string(),
    }
}

/// Parse + validate a tier-config JSON body from a labeled source.
/// Panics on parse failure, unsupported schema_version, or enum-mismatch —
/// the engine refuses to start with a malformed tier config so the operator
/// catches the problem at process start rather than at first soft-cap
/// rejection.
fn parse_and_validate(source: &TierConfigSource) -> PricingConfig {
    let parsed: PricingConfig = serde_json::from_str(&source.content).unwrap_or_else(|e| {
        panic!(
            "tier config from {} is malformed — proximaDB cannot start: {e}",
            source.label
        )
    });
    assert_eq!(
        parsed.schema_version, 1,
        "tier config from {} has unsupported schema_version {}",
        source.label, parsed.schema_version
    );
    validate_pricing_matches_enum(&parsed, &source.label);
    parsed
}

fn pricing() -> &'static PricingConfig {
    PRICING.get_or_init(|| {
        let source = resolve_tier_config_source();
        let parsed = parse_and_validate(&source);
        tracing::info!(
            tier_config_source = %source.label,
            tier_count = parsed.tiers.len(),
            "loaded tier config"
        );
        parsed
    })
}

fn validate_pricing_matches_enum(p: &PricingConfig, source_label: &str) {
    use std::collections::HashSet;
    // Phase B-4: validation is now alias-aware. Each JSON tier id is parsed
    // through serde (which honors the serde aliases on the Tier enum), and
    // we assert the resulting Tier set exactly equals Tier::all(). This lets
    // operator overlay JSONs use either the canonical operator-neutral ids
    // (tier1..tier5) or their own legacy ids (free_trial, team, pro, business,
    // enterprise — recognized via serde aliases) without needing a wire-format
    // migration.
    let mut parsed: HashSet<Tier> = HashSet::new();
    for json_tier in &p.tiers {
        let v = serde_json::Value::String(json_tier.id.clone());
        let tier: Tier = serde_json::from_value(v).unwrap_or_else(|_| {
            panic!(
                "tier config from {source_label} has unknown tier id {:?}; \
                 expected one of {:?} or a recognized alias",
                json_tier.id,
                Tier::all().iter().map(|t| t.id()).collect::<Vec<_>>()
            )
        });
        parsed.insert(tier);
    }
    let all_variants: HashSet<Tier> = Tier::all().iter().copied().collect();
    if parsed != all_variants {
        let missing: Vec<_> = all_variants.difference(&parsed).collect();
        let extra: Vec<_> = parsed.difference(&all_variants).collect();
        panic!(
            "tier config from {source_label} doesn't cover all Tier variants. \
             missing={missing:?}, extra={extra:?}"
        );
    }
}

fn pricing_row(tier: Tier) -> &'static PricingTier {
    // Phase B-4: alias-aware lookup. The JSON id may be the canonical id
    // (`tier.id()`) OR any of the serde aliases — both must locate the row.
    pricing()
        .tiers
        .iter()
        .find(|t| {
            let v = serde_json::Value::String(t.id.clone());
            serde_json::from_value::<Tier>(v).ok() == Some(tier)
        })
        .unwrap_or_else(|| panic!("tier {tier:?} missing from loaded tier config"))
}

/// Tier identifier — the baseline tier set shipped with ProximaDB.
///
/// **Phase B-4 rename (operator-neutral positional names).** The variants
/// are now `Tier1` through `Tier5` rather than the prior commercial names
/// (`FreeTrial`/`Team`/`Pro`/`Business`/`Enterprise`). Positional names
/// avoid any naming-collision trap with the legacy serde aliases listed
/// below: a stored `"tier": "standard"` continues to deserialize to
/// `Tier2` (its prior `Team` mapping) rather than getting silently
/// upgraded to a fictional `Tier::Standard`. See `config/TIER_CONFIG.md`
/// for the operator-neutral schema; operator overlays can carry their
/// own display names without the engine ever caring.
///
/// Legacy stored values (`"free_trial"`, `"team"`, `"pro"`, `"business"`,
/// `"enterprise"`, `"community"`, `"starter"`, `"standard"`,
/// `"enterprise_pooled"`, `"enterprise_dedicated"`) deserialize via the
/// serde aliases below so existing tenant-tier rows + operator overlay
/// JSONs keep working without a data migration.
///
/// Operator deployments can override the numeric caps per tier via the
/// runtime tier-config overlay (see `config/TIER_CONFIG.md`), but the
/// variant set itself is compile-time exhaustive because Rust enums can't
/// be built from runtime data. Adding a sixth tier requires a recompile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Lowest tier — evaluation / capped shared-pool resources.
    #[default]
    #[serde(alias = "free_trial", alias = "free")]
    Tier1,
    /// Entry pooled tier — accepts a wide alias set so legacy SKUs from
    /// earlier naming generations all map here.
    #[serde(
        alias = "team",
        alias = "community",
        alias = "starter",
        alias = "standard",
        alias = "basic"
    )]
    Tier2,
    /// Production pooled tier with sync ingest support.
    #[serde(alias = "pro")]
    Tier3,
    /// Pooled tier with full connector set + webhook integration support.
    #[serde(
        alias = "business",
        alias = "enterprise_pooled",
        alias = "premium"
    )]
    Tier4,
    /// Dedicated single-tenant infrastructure tier.
    #[serde(alias = "enterprise", alias = "enterprise_dedicated")]
    Tier5,
}

impl Tier {
    /// Canonical operator-neutral id for this tier (`tier1`..`tier5`).
    /// Stored tenant records may use any of the serde aliases above; this
    /// function always returns the canonical name regardless of how the
    /// value was deserialized.
    pub const fn id(self) -> &'static str {
        match self {
            Tier::Tier1 => "tier1",
            Tier::Tier2 => "tier2",
            Tier::Tier3 => "tier3",
            Tier::Tier4 => "tier4",
            Tier::Tier5 => "tier5",
        }
    }

    /// All declared tier variants in increasing-capacity order. Kept in
    /// sync with the embedded JSON via `validate_pricing_matches_enum()`
    /// at startup.
    pub const fn all() -> &'static [Tier] {
        &[
            Tier::Tier1,
            Tier::Tier2,
            Tier::Tier3,
            Tier::Tier4,
            Tier::Tier5,
        ]
    }

    /// Default scan budget (GB). Loaded from the embedded baseline
    /// `config/tier-config.json` at first access. Operators who overlay a
    /// runtime tier-config replace these defaults at process start (see
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
    /// from the embedded baseline `config/tier-config.json`; operators who
    /// add a tier via the runtime overlay (see `config/TIER_CONFIG.md`) widen the
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
            tier: Tier::Tier1,
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
    fn pricing_config_loads_without_panic_and_matches_enum() {
        // First access of `pricing()` deserializes the embedded JSON, asserts
        // schema_version == 1, and runs `validate_pricing_matches_enum`. If
        // any of those checks fail we panic here — surfaces a malformed or
        // drifted `config/tier-config.json` at test time rather than first
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
        assert_eq!(
            labels,
            vec!["tier1", "tier2", "tier3", "tier4", "tier5"]
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
}
