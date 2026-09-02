// Tenant tier store — durable tier policy for multi-tenant SaaS enforcement.
use serde::{Deserialize, Serialize};

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

use std::sync::OnceLock;

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
// built from runtime data. The startup assertion `validate_tier_config_matches_enum`
// panics if the chosen layer's tier set diverges from the enum variants,
// surfacing the drift at process start rather than at first soft-cap
// rejection. Validation is alias-aware: overlay JSONs may use any tier id
// that maps to a Tier variant via its serde aliases (so legacy ids like
// `free_trial`/`team`/`pro`/`business`/`enterprise` continue to validate
// against the post-B-4 `tier1`..`tier5` enum without a data migration).

const BAKED_TIER_CONFIG_BYTES: &str = include_str!("../../../../config/tier-config.json");
const RUNTIME_TIER_CONFIG_ENV: &str = "PROXIMADB_TIER_CONFIG_PATH";
const DEFAULT_RUNTIME_TIER_CONFIG_PATH: &str = "/config/tier-config.json";
const LEGACY_RUNTIME_TIER_CONFIG_PATH: &str = "/config/pricing.json";

static TIER_CONFIG: OnceLock<TierConfig> = OnceLock::new();

#[derive(Debug, Deserialize)]
pub struct TierConfig {
    pub schema_version: u32,
    /// Fallback tier for tenants with no explicit stamp — READ cross-crate by
    /// `catalog::tenant_tier::default_tier()`, which prefers the overlay's
    /// value over the compiled `Tier::default()`.
    pub default_tier: String,
    pub tiers: Vec<TierSpec>,
}

#[derive(Debug, Deserialize)]
pub struct TierSpec {
    pub id: String,
    pub prom_label: String,
    pub soft_caps: TierSoftCaps,
    /// C5 governance tier-entitlement multiplier (Dimension 5). Authored by the
    /// control plane (anvaiops `tiers.json` → `/config/tier-config.json`); absent
    /// in the OSS baseline overlay → `None` → neutral `1.0`. Other overlay fields
    /// (pricing, display, …) are ignored by serde — this reads only the scalar.
    #[serde(default)]
    pub cost_multiplier: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct TierSoftCaps {
    pub scan_budget_gb: f64,
    pub ef_search_cap: u32,
    pub freshness_sla_seconds: u32,
}

/// Resolved tier-config source: where it came from + its raw JSON bytes.
/// The `label` flows into log lines + panic messages so the source is
/// always identifiable in failure modes.
pub struct TierConfigSource {
    pub label: String,
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
    if runtime_path == DEFAULT_RUNTIME_TIER_CONFIG_PATH
        && let Ok(content) = std::fs::read_to_string(LEGACY_RUNTIME_TIER_CONFIG_PATH)
    {
        return TierConfigSource {
            label: format!(
                "legacy runtime overlay {LEGACY_RUNTIME_TIER_CONFIG_PATH} \
                 (deprecated; rename to {DEFAULT_RUNTIME_TIER_CONFIG_PATH})"
            ),
            content,
        };
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
///
/// The four `panic!()` calls below are intentional startup-time fail-fast
/// — a malformed tier config is a fatal configuration error and the
/// engine cannot proceed safely. They're allowed for `clippy::panic`
/// because the function's whole point IS to crash early with a clear
/// operator-facing message rather than degrade silently.
#[allow(clippy::panic)]
fn parse_and_validate(source: &TierConfigSource) -> TierConfig {
    let parsed: TierConfig = serde_json::from_str(&source.content).unwrap_or_else(|e| {
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
    validate_tier_config_matches_enum(&parsed, &source.label);
    parsed
}

pub fn tier_config() -> &'static TierConfig {
    TIER_CONFIG.get_or_init(|| {
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

// Startup-only invariant check on the loaded tier config. A malformed
// config means the operator overlay JSON is broken — failing fast at
// boot is the right answer; downstream code assumes every Tier variant
// has a tier-config row wired up.
#[allow(clippy::panic)]
fn validate_tier_config_matches_enum(p: &TierConfig, source_label: &str) {
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

fn tier_row(tier: Tier) -> &'static TierSpec {
    // Phase B-4: alias-aware lookup. The JSON id may be the canonical id
    // (`tier.id()`) OR any of the serde aliases — both must locate the row.
    // The panic on miss is intentional: the loaded tier config is
    // already validated to cover every Tier variant at startup
    // (`parse_and_validate`), so a missing tier here is a startup-time
    // invariant violation that should crash, not be silently masked.
    #[allow(clippy::panic)]
    tier_config()
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
    #[serde(alias = "business", alias = "enterprise_pooled", alias = "premium")]
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
    /// sync with the embedded JSON via `validate_tier_config_matches_enum()`
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
        tier_row(self).soft_caps.scan_budget_gb
    }

    /// Default beam-width / ef_search ceiling — hard ceiling at the router.
    pub fn default_ef_search_cap(self) -> u32 {
        tier_row(self).soft_caps.ef_search_cap
    }

    /// Default freshness SLA for async ingest, in seconds.
    pub fn default_freshness_sla_seconds(self) -> u32 {
        tier_row(self).soft_caps.freshness_sla_seconds
    }

    /// Parse a tier *claim* string (e.g. the `X-Tenant-Tier` header the control
    /// plane stamps) into a `Tier`, honoring the serde aliases — so both the
    /// canonical ids (`tier1`..`tier5`) and the legacy/commercial ids
    /// (`free_trial`/`free`/`team`/`pro`/`business`/`enterprise`/…) resolve.
    /// `None` for an unrecognized claim (caller falls back to the default tier).
    pub fn from_claim(claim: &str) -> Option<Tier> {
        let trimmed = claim.trim();
        if trimmed.is_empty() {
            return None;
        }
        serde_json::from_value::<Tier>(serde_json::Value::String(trimmed.to_string())).ok()
    }

    /// C5 governance tier-entitlement multiplier (Dimension 5) for this tier,
    /// read from the tier-config overlay. Neutral `1.0` when the overlay omits it
    /// (the OSS baseline) or supplies a non-finite/non-positive value (rejected
    /// fail-safe — a non-positive multiplier could invert the reported cost). The
    /// $ values are control-plane policy; OSS only reads the configured scalar.
    pub fn cost_multiplier(self) -> f64 {
        match tier_row(self).cost_multiplier {
            Some(m) if m.is_finite() && m > 0.0 => m,
            _ => 1.0,
        }
    }

    /// Bounded Prometheus label — cardinality-safe. The label set is loaded
    /// from the embedded baseline `config/tier-config.json`; operators who
    /// add a tier via the runtime overlay (see `config/TIER_CONFIG.md`) widen the
    /// metric label set automatically without a recompile.
    pub fn prometheus_label(self) -> &'static str {
        tier_row(self).prom_label.as_str()
    }

    /// Object economy routing configuration for this tier.
    /// Returns tier-specific caps for block-level routing based on
    /// object economy metadata (centroids, Z-order codes, zone maps).
    /// Cheap tiers are capped at lower precision and smaller block budgets;
    /// premium tiers are uncapped.
    pub fn object_economy_config(self) -> TierObjectEconomyConfig {
        match self {
            Tier::Tier1 => TierObjectEconomyConfig {
                allow_centroid_routing: true,
                allow_zorder_pruning: true,
                max_blocks_per_query: 100,
                quantization_ceiling: ObjectEconomyQuantizationCeiling::INT8,
            },
            Tier::Tier2 => TierObjectEconomyConfig {
                allow_centroid_routing: true,
                allow_zorder_pruning: true,
                max_blocks_per_query: 500,
                quantization_ceiling: ObjectEconomyQuantizationCeiling::INT8,
            },
            Tier::Tier3 => TierObjectEconomyConfig {
                allow_centroid_routing: true,
                allow_zorder_pruning: true,
                max_blocks_per_query: 2000,
                quantization_ceiling: ObjectEconomyQuantizationCeiling::FP16,
            },
            Tier::Tier4 => TierObjectEconomyConfig {
                allow_centroid_routing: true,
                allow_zorder_pruning: true,
                max_blocks_per_query: 10000,
                quantization_ceiling: ObjectEconomyQuantizationCeiling::FP16,
            },
            Tier::Tier5 => TierObjectEconomyConfig {
                allow_centroid_routing: true,
                allow_zorder_pruning: true,
                max_blocks_per_query: u32::MAX,
                quantization_ceiling: ObjectEconomyQuantizationCeiling::FP32,
            },
        }
    }
}

/// Object economy routing caps per tier.
///
/// Encodes the upper bounds a tenant in this tier can request. Cheap tiers
/// have tighter caps to control resource usage; premium tiers are uncapped.
/// The planner consults this to clamp request-side asks before route
/// selection — it does not raise quality for queries that asked for less.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierObjectEconomyConfig {
    /// Whether centroid-based block routing is allowed for this tier.
    pub allow_centroid_routing: bool,
    /// Whether Z-order code based pruning is allowed for this tier.
    pub allow_zorder_pruning: bool,
    /// Maximum number of blocks that can be scanned per query.
    /// Lower tiers have tighter limits to control I/O costs.
    pub max_blocks_per_query: u32,
    /// Maximum precision a query in this tier may request. Cheap tiers are
    /// capped at lower precision so a Tier1 tenant cannot demand FP32 over
    /// object storage. Premium tiers (Tier5) are effectively uncapped via
    /// [`ObjectEconomyQuantizationCeiling::FP32`].
    pub quantization_ceiling: ObjectEconomyQuantizationCeiling,
}

/// Highest quantization precision a tier is allowed to request.
///
/// This is a *ceiling*, not a floor: a Tier1 query capped at INT8 may still
/// run at lower precision (Binary, INT8) but cannot escalate to FP16/FP32.
/// `FP32` means "no effective cap" — used by the premium tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectEconomyQuantizationCeiling {
    /// No cap — full FP32 precision available (Tier5 / Enterprise).
    FP32,
    /// FP16 half precision — Tier3/Tier4 balance of quality and cost.
    FP16,
    /// INT8 cap — Tier1/Tier2 aggressive compression for cost control.
    INT8,
}

impl std::fmt::Display for ObjectEconomyQuantizationCeiling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FP32 => write!(f, "FP32"),
            Self::FP16 => write!(f, "FP16"),
            Self::INT8 => write!(f, "INT8"),
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

#[cfg(test)]
mod overlay_tests {
    //! ADR-0053: the exact artifact a commercial operator ships — a runtime
    //! overlay at `PROXIMADB_TIER_CONFIG_PATH` whose tier ids are the
    //! operator's own (commercial aliases), not the canonical `tier1..tier5`.
    //! Exercises the full chain: `resolve_tier_config_source` →
    //! `parse_and_validate` (alias-aware enum coverage) → `tier_row` alias
    //! lookup. Exactly ONE test in this crate may initialize `TIER_CONFIG`
    //! (the OnceLock has no reset); nextest's process-per-test makes that
    //! safe, and no other test in this crate touches `tier_config()`.
    use super::*;

    /// The AnvaiOps-shaped projection (ADR-0053 W1): mechanics only — ids,
    /// prom labels, soft caps, cost multiplier. Distinct values from the OSS
    /// baseline so overlay-preference is observable.
    const OVERLAY_JSON: &str = r#"{
        "schema_version": 1,
        "default_tier": "free_trial",
        "tiers": [
            { "id": "free_trial", "prom_label": "overlay-free",
              "soft_caps": { "scan_budget_gb": 2.0, "ef_search_cap": 70, "freshness_sla_seconds": 800 },
              "cost_multiplier": 1.25 },
            { "id": "team", "prom_label": "overlay-team",
              "soft_caps": { "scan_budget_gb": 5.0, "ef_search_cap": 170, "freshness_sla_seconds": 290 } },
            { "id": "pro", "prom_label": "overlay-pro",
              "soft_caps": { "scan_budget_gb": 42.0, "ef_search_cap": 300, "freshness_sla_seconds": 100 },
              "cost_multiplier": 2.5 },
            { "id": "business", "prom_label": "overlay-business",
              "soft_caps": { "scan_budget_gb": 51.0, "ef_search_cap": 390, "freshness_sla_seconds": 50 } },
            { "id": "enterprise", "prom_label": "overlay-enterprise",
              "soft_caps": { "scan_budget_gb": 257.0, "ef_search_cap": 1030, "freshness_sla_seconds": 10 } }
        ]
    }"#;

    #[test]
    fn runtime_overlay_with_commercial_alias_ids_is_loaded_and_preferred() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tier-config.json");
        std::fs::write(&path, OVERLAY_JSON).expect("write overlay");

        // SAFETY: nextest runs process-per-test, so the env mutation cannot
        // leak into sibling tests (same justification as
        // config_loader_tests.rs).
        unsafe { std::env::set_var("PROXIMADB_TIER_CONFIG_PATH", &path) };

        // Alias-aware validation passed: all five commercial ids deserialized
        // onto the Tier enum and covered every variant.
        assert_eq!(tier_config().tiers.len(), 5);

        // The overlay's row won over the baked baseline for the aliased tier.
        assert_eq!(Tier::Tier3.prometheus_label(), "overlay-pro");
        // The C5 governance scalar round-trips from the overlay (Tier3 = pro).
        assert!((Tier::Tier3.cost_multiplier() - 2.5).abs() < 1e-9);
        // Soft caps come from the overlay row, not the compiled defaults.
        assert!((Tier::Tier3.default_scan_budget_gb() - 42.0).abs() < 1e-9);
        assert_eq!(Tier::Tier3.default_ef_search_cap(), 300);
        // Tier1 = free_trial via alias.
        assert_eq!(Tier::Tier1.prometheus_label(), "overlay-free");
        assert!((Tier::Tier1.cost_multiplier() - 1.25).abs() < 1e-9);
    }
}
