// Tenant Prometheus label primitive — bundles tenant_id → bounded
// label resolution with the LLD's cardinality-safety guardrail.
//
// The LLD `Multi-Tenant + SaaS Posture` row warns:
//
//   "Per-tenant Prometheus labels. Bounded cardinality: only the tenant
//    tier (free/pooled/dedicated) goes on the high-cardinality counters;
//    the raw tenant_id only on rollup gauges."
//
// Today every metric emit site re-derives `Tier::prometheus_label`
// inline and the discipline of "only bounded label on the hot counter"
// lives in code review. This module makes the discipline typed: the
// `TenantLabel` resolver exposes two methods with different intent —
// `high_cardinality_label()` returns the bounded `&'static str` and is
// always safe to attach to per-second counters; `rollup_label()`
// returns the raw `tenant_id` and is intentionally not `&'static`,
// signaling that it must only land on metrics scraped at >=1m.
//
// Bounded set: free, team, pro, business, enterprise — derived at startup
// from `config/tier-config.json` via `Tier::prometheus_label()`. Operators
// adding tiers via the runtime overlay (see `config/TIER_CONFIG.md`)
// automatically widen this bounded set on the next process restart.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::catalog::tenant_tier::{TenantTierRecord, Tier};

/// Resolver that takes a `TenantTierRecord` and exposes
/// cardinality-safe labels.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantLabel {
    /// Raw tenant id — only safe for low-frequency rollup metrics.
    pub tenant_id: String,
    /// Bounded tier label — safe for any metric.
    pub tier_label: String,
}

impl TenantLabel {
    /// Build from a tier record.
    pub fn from_record(record: &TenantTierRecord) -> Self {
        Self {
            tenant_id: record.tenant_id.clone(),
            tier_label: record.tier.prometheus_label().to_string(),
        }
    }

    /// Build with a specific tier (when the caller doesn't have a full
    /// record handy — e.g. the gateway middleware before tier resolution
    /// completes).
    pub fn from_parts(tenant_id: impl Into<String>, tier: Tier) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            tier_label: tier.prometheus_label().to_string(),
        }
    }

    /// Safe label for high-cardinality counters (per-second scrape).
    /// Returns a bounded `&'static str` sourced from the loaded pricing
    /// config via the canonical `Tier::prometheus_label()` route.
    ///
    /// Alias-aware: the stored `tier_label` is parsed through serde
    /// (honoring every serde alias declared on the `Tier` enum — both
    /// the current canonical names like `tier1`/`tier2`/... and every
    /// historical alias like `free_trial`/`team`/`community`/`starter`/
    /// `standard`/`pro`/`business`/`enterprise_pooled`/`enterprise`/
    /// `enterprise_dedicated`). The deserialized `Tier` then yields its
    /// canonical prometheus label from the bundled pricing config (or
    /// the operator overlay, when an overlay is loaded).
    ///
    /// Returns `"unknown"` if the stored string doesn't deserialize to
    /// any known `Tier`.
    pub fn high_cardinality_label(&self) -> &'static str {
        let v = serde_json::Value::String(self.tier_label.clone());
        serde_json::from_value::<Tier>(v)
            .ok()
            .map(|t| t.prometheus_label())
            .unwrap_or("unknown")
    }

    /// Label for rollup metrics scraped at >=1m. Returns the raw
    /// tenant_id — intentionally `&str` (not `&'static str`) so the
    /// caller can see the cardinality cost in the type.
    pub fn rollup_label(&self) -> &str {
        &self.tenant_id
    }

    /// JSON shape for the audit log — both labels present, no cardinality
    /// constraint at write time (the trace store is bounded by sampling
    /// + retention, not by Prometheus discipline).
    pub fn to_audit_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tenant_id": self.tenant_id,
            "tier": self.tier_label,
        })
    }
}

static BOUNDED_LABELS_CACHE: OnceLock<Vec<&'static str>> = OnceLock::new();

/// Bounded tier-label set, derived at startup from `Tier::prometheus_label()`
/// (which itself loads from `config/tier-config.json`). This is the single source
/// of truth callers should hit when registering Prometheus metric families
/// or asserting cardinality safety.
pub fn bounded_tier_labels() -> &'static [&'static str] {
    BOUNDED_LABELS_CACHE
        .get_or_init(|| Tier::all().iter().map(|t| t.prometheus_label()).collect())
        .as_slice()
}

/// `true` when `s` is one of the bounded tier labels. Used by metric
/// registration code that wants to assert label safety at startup.
pub fn is_bounded_label(s: &str) -> bool {
    bounded_tier_labels().contains(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tenant_tier::FeatureFlags;

    fn record(tenant_id: &str, tier: Tier) -> TenantTierRecord {
        TenantTierRecord {
            tenant_id: tenant_id.into(),
            tier,
            scan_budget_gb_hard: None,
            ef_search_cap: None,
            freshness_sla_seconds: None,
            feature_flags: FeatureFlags::default(),
        }
    }

    #[test]
    fn from_record_carries_tenant_and_tier_label() {
        let r = record("tenant-a", Tier::Tier4);
        let label = TenantLabel::from_record(&r);
        assert_eq!(label.tenant_id, "tenant-a");
        assert_eq!(label.tier_label, "tier4");
    }

    #[test]
    fn from_parts_constructs_without_record() {
        let label = TenantLabel::from_parts("tenant-b", Tier::Tier5);
        assert_eq!(label.tenant_id, "tenant-b");
        assert_eq!(label.tier_label, "tier5");
    }

    #[test]
    fn high_cardinality_label_returns_static_strings() {
        // The label must be `&'static str` so callers can register
        // metric families with the bounded set at startup. This test
        // pins each tier's label.
        for (tier, expected) in [
            (Tier::Tier1, "tier1"),
            (Tier::Tier2, "tier2"),
            (Tier::Tier3, "tier3"),
            (Tier::Tier4, "tier4"),
            (Tier::Tier5, "tier5"),
        ] {
            let label = TenantLabel::from_parts("any", tier);
            assert_eq!(label.high_cardinality_label(), expected);
        }
    }

    #[test]
    fn legacy_tier_labels_map_to_canonical_bounded_labels() {
        // 2026 Q2 consolidation: historical TenantLabel JSON with the
        // pre-rename `tier_label` values must still produce a bounded
        // metric label rather than collapsing to "unknown".
        for (raw, expected) in [
            ("community", "tier2"),
            ("starter", "tier2"),
            ("standard", "tier2"),
            ("enterprise_pooled", "tier4"),
            ("enterprise_dedicated", "tier5"),
        ] {
            let label = TenantLabel {
                tenant_id: "any".into(),
                tier_label: raw.into(),
            };
            assert_eq!(label.high_cardinality_label(), expected);
        }
    }

    #[test]
    fn high_cardinality_label_falls_back_to_unknown() {
        // Misconfigured TenantLabel with a tier_label outside the
        // bounded set — must not panic; returns "unknown" so metric
        // emission stays safe.
        let label = TenantLabel {
            tenant_id: "any".into(),
            tier_label: "platinum".into(),
        };
        assert_eq!(label.high_cardinality_label(), "unknown");
    }

    #[test]
    fn rollup_label_returns_raw_tenant_id() {
        let label = TenantLabel::from_parts("tenant-xyz", Tier::Tier4);
        assert_eq!(label.rollup_label(), "tenant-xyz");
    }

    #[test]
    fn bounded_tier_labels_contains_exactly_five() {
        let labels = bounded_tier_labels();
        assert_eq!(labels.len(), 5);
        assert!(labels.contains(&"tier1"));
        assert!(labels.contains(&"tier2"));
        assert!(labels.contains(&"tier3"));
        assert!(labels.contains(&"tier4"));
        assert!(labels.contains(&"tier5"));
    }

    #[test]
    fn bounded_tier_labels_derived_from_pricing_config() {
        // The label set must match what Tier::prometheus_label() returns
        // for each declared variant — proving the JSON-driven derivation
        // didn't drift from the enum-driven side.
        let derived: Vec<&str> = Tier::all().iter().map(|t| t.prometheus_label()).collect();
        assert_eq!(bounded_tier_labels(), derived.as_slice());
    }

    #[test]
    fn is_bounded_label_recognizes_each_tier() {
        assert!(is_bounded_label("tier1"));
        assert!(is_bounded_label("tier2"));
        assert!(is_bounded_label("tier3"));
        assert!(is_bounded_label("tier4"));
        assert!(is_bounded_label("tier5"));
    }

    #[test]
    fn is_bounded_label_rejects_off_set_strings() {
        assert!(!is_bounded_label("platinum"));
        assert!(!is_bounded_label("FREE")); // case-sensitive
        assert!(!is_bounded_label(""));
        assert!(!is_bounded_label("unknown"));
    }

    #[test]
    fn audit_json_carries_both_fields() {
        let label = TenantLabel::from_parts("tenant-a", Tier::Tier4);
        let json = label.to_audit_json();
        assert_eq!(json["tenant_id"], "tenant-a");
        assert_eq!(json["tier"], "tier4");
    }

    #[test]
    fn label_round_trips_via_json() {
        let label = TenantLabel::from_parts("tenant-a", Tier::Tier1);
        let s = serde_json::to_string(&label).unwrap();
        let back: TenantLabel = serde_json::from_str(&s).unwrap();
        assert_eq!(label, back);
    }

    #[test]
    fn label_is_hash_eq_for_map_use() {
        // The resolver is Hash + Eq so it can key a per-tenant counter
        // map directly (e.g. DashMap<TenantLabel, u64>).
        use std::collections::HashMap;
        let a = TenantLabel::from_parts("t1", Tier::Tier4);
        let b = TenantLabel::from_parts("t1", Tier::Tier4);
        let mut map: HashMap<TenantLabel, u32> = HashMap::new();
        *map.entry(a.clone()).or_insert(0) += 1;
        *map.entry(b).or_insert(0) += 1;
        // Same key → one entry, count 2.
        assert_eq!(map.len(), 1);
        assert_eq!(map[&a], 2);
    }

    #[test]
    fn distinct_tenants_produce_distinct_map_keys() {
        use std::collections::HashMap;
        let a = TenantLabel::from_parts("t1", Tier::Tier4);
        let b = TenantLabel::from_parts("t2", Tier::Tier4);
        let mut map: HashMap<TenantLabel, u32> = HashMap::new();
        map.insert(a, 1);
        map.insert(b, 2);
        assert_eq!(map.len(), 2);
    }
}
