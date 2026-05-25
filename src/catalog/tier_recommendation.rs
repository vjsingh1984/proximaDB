// Tier recommendation — turns a workload mix + signal counts into an
// actionable upgrade / hold / downgrade hint.
//
// `observability::workload_mix::WorkloadMix` summarizes WHAT the
// tenant's traffic looks like. `TenantTierRecord` summarizes what
// their current ceiling is. This module combines the two with a small
// set of signal counts to recommend whether the gateway should suggest
// a tier change.
//
// The output is typed and bounded so the gateway can:
//   - emit a customer-facing in-app banner ("you've been bursting on
//     enterprise-shaped workloads — upgrade?")
//   - notify Sales via the existing audit channel
//   - record the recommendation against the tenant for trend analysis
//
// Pure-data: the policy itself is a `RecommendationPolicy` struct with
// thresholds the caller can tune. Defaults match the LLD's pricing
// sketch.

use serde::{Deserialize, Serialize};

use crate::catalog::tenant_tier::{TenantTierRecord, Tier};
use crate::observability::workload_mix::{ConcentrationClass, WorkloadMix};

/// Recommendation direction. Bounded enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationKind {
    /// Recommend the tenant move up a tier.
    Upgrade,
    /// Recommend they stay on the current tier.
    Hold,
    /// Recommend they move down a tier (over-provisioned).
    Downgrade,
}

impl RecommendationKind {
    pub const fn label(self) -> &'static str {
        match self {
            RecommendationKind::Upgrade => "upgrade",
            RecommendationKind::Hold => "hold",
            RecommendationKind::Downgrade => "downgrade",
        }
    }
}

/// Bounded reason labels. Pinned strings — the gateway maps these to
/// localized customer copy.
pub mod reason {
    pub const HIGH_OVER_BUDGET_RATE: &str = "high_over_budget_rate";
    pub const CONCENTRATED_HOT_WORKLOAD: &str = "concentrated_hot_workload";
    pub const LATENCY_STALLS: &str = "latency_stalls";
    pub const STEADY_STATE: &str = "steady_state";
    pub const UNDERUTILIZED: &str = "underutilized";
    pub const INSUFFICIENT_SIGNAL: &str = "insufficient_signal";
    pub const CEILING_REACHED: &str = "ceiling_reached";
}

/// Per-window signal counts the gateway feeds in alongside the mix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignalCounts {
    /// Fraction of requests in the window that hit `budget_exceeded`.
    pub over_budget_rate: f64,
    /// Fraction of requests stalled past the tier's latency target.
    pub latency_stall_rate: f64,
    /// Fraction of requests served from the result cache. High hit
    /// rate + low over-budget = candidate for downgrade.
    pub cache_hit_rate: f64,
    /// Total request count in the window. Below `min_samples` the
    /// recommendation is `Hold (insufficient_signal)` regardless of
    /// other signals.
    pub request_count: u64,
}

impl Default for SignalCounts {
    fn default() -> Self {
        Self {
            over_budget_rate: 0.0,
            latency_stall_rate: 0.0,
            cache_hit_rate: 0.0,
            request_count: 0,
        }
    }
}

/// Policy thresholds. Defaults match the LLD §3 (1%/60%) + risk-row
/// guidance; the gateway can override per-window if desired.
#[derive(Debug, Clone, Copy)]
pub struct RecommendationPolicy {
    /// Minimum request count required before recommending anything
    /// other than `Hold (insufficient_signal)`.
    pub min_samples: u64,
    /// Over-budget fraction above which we suggest Upgrade. Default 0.10.
    pub upgrade_over_budget_rate: f64,
    /// Latency stall fraction above which we suggest Upgrade. Default 0.20.
    pub upgrade_latency_stall_rate: f64,
    /// Cache-hit rate above which a tenant qualifies for Downgrade
    /// candidacy. Default 0.85.
    pub downgrade_cache_hit_rate: f64,
    /// Over-budget rate ceiling for a Downgrade candidate. Default 0.01.
    pub downgrade_over_budget_rate_max: f64,
    /// Latency stall ceiling for a Downgrade candidate. Default 0.02.
    pub downgrade_latency_stall_rate_max: f64,
}

impl Default for RecommendationPolicy {
    fn default() -> Self {
        Self {
            min_samples: 100,
            upgrade_over_budget_rate: 0.10,
            upgrade_latency_stall_rate: 0.20,
            downgrade_cache_hit_rate: 0.85,
            downgrade_over_budget_rate_max: 0.01,
            downgrade_latency_stall_rate_max: 0.02,
        }
    }
}

/// Structured recommendation event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    pub tenant_id: String,
    pub current_tier: String,
    pub kind: RecommendationKind,
    pub reason: String,
    /// Suggested target tier label, or `None` when the recommendation
    /// is `Hold` or the current tier is already at the ceiling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_tier: Option<String>,
    /// Echo of the dominant fingerprint (when present) so the audit
    /// log can group by workload shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dominant_fingerprint: Option<String>,
}

impl Recommendation {
    pub fn to_audit_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// Inputs the recommender consumes.
#[derive(Debug, Clone)]
pub struct RecommendationInputs<'a> {
    pub tenant: &'a TenantTierRecord,
    pub mix: &'a WorkloadMix,
    pub signals: SignalCounts,
}

/// Run the recommender. Pure given the policy.
pub fn recommend(
    inputs: &RecommendationInputs<'_>,
    policy: &RecommendationPolicy,
) -> Recommendation {
    let tenant = inputs.tenant;
    let mix = inputs.mix;
    let signals = inputs.signals;

    let current_label = tenant.tier.prometheus_label().to_string();
    let dominant_fp = mix.dominant_shape.clone();
    let mut out = Recommendation {
        tenant_id: tenant.tenant_id.clone(),
        current_tier: current_label.clone(),
        kind: RecommendationKind::Hold,
        reason: reason::STEADY_STATE.to_string(),
        suggested_tier: None,
        dominant_fingerprint: dominant_fp.clone(),
    };

    // Step 1: insufficient signal short-circuit.
    if signals.request_count < policy.min_samples {
        out.reason = reason::INSUFFICIENT_SIGNAL.to_string();
        return out;
    }

    // Step 2: upgrade signal — over-budget rate above threshold.
    if signals.over_budget_rate >= policy.upgrade_over_budget_rate {
        return upgrade(out, tenant.tier, reason::HIGH_OVER_BUDGET_RATE);
    }

    // Step 3: upgrade signal — latency stalls above threshold.
    if signals.latency_stall_rate >= policy.upgrade_latency_stall_rate {
        return upgrade(out, tenant.tier, reason::LATENCY_STALLS);
    }

    // Step 4: upgrade signal — concentrated hot workload on a low tier.
    // A highly-concentrated workload on the free/community tiers
    // suggests the tenant has stable production traffic, not
    // evaluation usage — recommend the next tier up.
    if matches!(
        mix.concentration,
        ConcentrationClass::HighlyConcentrated | ConcentrationClass::Concentrated
    ) && matches!(tenant.tier, Tier::Tier1 | Tier::Tier2)
    {
        return upgrade(out, tenant.tier, reason::CONCENTRATED_HOT_WORKLOAD);
    }

    // Step 5: downgrade — high cache hit rate + low budget pressure +
    // low latency pressure suggests over-provisioning.
    if signals.cache_hit_rate >= policy.downgrade_cache_hit_rate
        && signals.over_budget_rate <= policy.downgrade_over_budget_rate_max
        && signals.latency_stall_rate <= policy.downgrade_latency_stall_rate_max
    {
        if let Some(below) = tier_below(tenant.tier) {
            out.kind = RecommendationKind::Downgrade;
            out.reason = reason::UNDERUTILIZED.to_string();
            out.suggested_tier = Some(below.prometheus_label().to_string());
            return out;
        }
        // Free tier can't go down.
        out.reason = reason::UNDERUTILIZED.to_string();
        return out;
    }

    // Default: hold.
    out
}

fn upgrade(mut out: Recommendation, current: Tier, reason_label: &str) -> Recommendation {
    out.reason = reason_label.to_string();
    match tier_above(current) {
        Some(t) => {
            out.kind = RecommendationKind::Upgrade;
            out.suggested_tier = Some(t.prometheus_label().to_string());
        }
        None => {
            // Already at the ceiling — keep Hold but signal the
            // operator that the customer is enterprise-bound.
            out.kind = RecommendationKind::Hold;
            out.reason = reason::CEILING_REACHED.to_string();
        }
    }
    out
}

fn tier_above(t: Tier) -> Option<Tier> {
    match t {
        Tier::Tier1 => Some(Tier::Tier2),
        Tier::Tier2 => Some(Tier::Tier3),
        Tier::Tier3 => Some(Tier::Tier4),
        Tier::Tier4 => Some(Tier::Tier5),
        Tier::Tier5 => None,
    }
}

fn tier_below(t: Tier) -> Option<Tier> {
    match t {
        Tier::Tier1 => None,
        Tier::Tier2 => Some(Tier::Tier1),
        Tier::Tier3 => Some(Tier::Tier2),
        Tier::Tier4 => Some(Tier::Tier3),
        Tier::Tier5 => Some(Tier::Tier4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tenant_tier::FeatureFlags;
    use crate::observability::workload_mix::{ConcentrationClass, WorkloadRow};

    fn tenant(tier: Tier) -> TenantTierRecord {
        TenantTierRecord {
            tenant_id: "tenant-a".into(),
            tier,
            scan_budget_gb_hard: None,
            ef_search_cap: None,
            freshness_sla_seconds: None,
            feature_flags: FeatureFlags::default(),
        }
    }

    fn mix(concentration: ConcentrationClass, dominant: Option<&str>, total: u64) -> WorkloadMix {
        WorkloadMix {
            total,
            distinct_shapes: 1,
            dominant_shape: dominant.map(|s| s.to_string()),
            dominant_fraction: match concentration {
                ConcentrationClass::HighlyConcentrated => 0.9,
                ConcentrationClass::Concentrated => 0.6,
                ConcentrationClass::Diverse => 0.3,
                ConcentrationClass::Broad => 0.05,
            },
            concentration,
            top: dominant
                .map(|s| {
                    vec![WorkloadRow {
                        fingerprint: s.to_string(),
                        count: total,
                        fraction: 1.0,
                    }]
                })
                .unwrap_or_default(),
        }
    }

    fn signals_with_count(n: u64) -> SignalCounts {
        SignalCounts {
            over_budget_rate: 0.0,
            latency_stall_rate: 0.0,
            cache_hit_rate: 0.0,
            request_count: n,
        }
    }

    fn cfg() -> RecommendationPolicy {
        RecommendationPolicy::default()
    }

    #[test]
    fn below_min_samples_returns_insufficient_signal() {
        let t = tenant(Tier::Tier2);
        let m = mix(ConcentrationClass::HighlyConcentrated, Some("fp"), 10);
        let s = signals_with_count(10);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Hold);
        assert_eq!(r.reason, reason::INSUFFICIENT_SIGNAL);
    }

    #[test]
    fn high_over_budget_rate_recommends_upgrade() {
        let t = tenant(Tier::Tier2);
        let m = mix(ConcentrationClass::Broad, Some("fp"), 1_000);
        let mut s = signals_with_count(1_000);
        s.over_budget_rate = 0.25; // well above 0.10 threshold
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Upgrade);
        assert_eq!(r.reason, reason::HIGH_OVER_BUDGET_RATE);
        assert_eq!(r.suggested_tier.as_deref(), Some("tier3"));
    }

    #[test]
    fn latency_stalls_recommend_upgrade() {
        let t = tenant(Tier::Tier4);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 5_000);
        let mut s = signals_with_count(5_000);
        s.latency_stall_rate = 0.30;
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Upgrade);
        assert_eq!(r.reason, reason::LATENCY_STALLS);
        assert_eq!(r.suggested_tier.as_deref(), Some("tier5"));
    }

    #[test]
    fn concentrated_workload_on_low_tier_recommends_upgrade() {
        let t = tenant(Tier::Tier1);
        let m = mix(ConcentrationClass::HighlyConcentrated, Some("fp"), 1_000);
        let s = signals_with_count(1_000);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Upgrade);
        assert_eq!(r.reason, reason::CONCENTRATED_HOT_WORKLOAD);
        assert_eq!(r.suggested_tier.as_deref(), Some("tier2"));
    }

    #[test]
    fn concentrated_workload_on_business_tier_holds() {
        // The concentrated-workload signal only fires on free/team
        // tiers — a business tenant with concentrated traffic is the
        // normal case, not a tier-recommendation trigger.
        let t = tenant(Tier::Tier4);
        let m = mix(ConcentrationClass::HighlyConcentrated, Some("fp"), 1_000);
        let s = signals_with_count(1_000);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Hold);
        assert_eq!(r.reason, reason::STEADY_STATE);
    }

    #[test]
    fn enterprise_ceiling_collapses_upgrade_to_hold() {
        // High over-budget rate on enterprise — can't go higher.
        let t = tenant(Tier::Tier5);
        let m = mix(ConcentrationClass::Broad, Some("fp"), 1_000);
        let mut s = signals_with_count(1_000);
        s.over_budget_rate = 0.50;
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Hold);
        assert_eq!(r.reason, reason::CEILING_REACHED);
        assert!(r.suggested_tier.is_none());
    }

    #[test]
    fn high_cache_hit_low_pressure_recommends_downgrade() {
        let t = tenant(Tier::Tier4);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 1_000);
        let mut s = signals_with_count(1_000);
        s.cache_hit_rate = 0.95;
        s.over_budget_rate = 0.0;
        s.latency_stall_rate = 0.0;
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Downgrade);
        assert_eq!(r.reason, reason::UNDERUTILIZED);
        assert_eq!(r.suggested_tier.as_deref(), Some("tier3"));
    }

    #[test]
    fn downgrade_blocked_at_free_tier() {
        let t = tenant(Tier::Tier1);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 1_000);
        let mut s = signals_with_count(1_000);
        s.cache_hit_rate = 0.95;
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        // Underutilized but no lower tier — stays Hold but with the
        // underutilized reason so the gateway can show the customer
        // they're paying for capacity they don't use (mostly for the
        // sales surface to suggest scoping).
        assert_eq!(r.kind, RecommendationKind::Hold);
        assert_eq!(r.reason, reason::UNDERUTILIZED);
    }

    #[test]
    fn high_cache_hit_with_some_budget_pressure_holds() {
        // Cache hit rate is high but over-budget is nonzero → not a
        // clean downgrade candidate.
        let t = tenant(Tier::Tier4);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 1_000);
        let mut s = signals_with_count(1_000);
        s.cache_hit_rate = 0.95;
        s.over_budget_rate = 0.05; // above downgrade ceiling 0.01
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Hold);
    }

    #[test]
    fn steady_state_is_default_hold() {
        let t = tenant(Tier::Tier4);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 1_000);
        let s = signals_with_count(1_000);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Hold);
        assert_eq!(r.reason, reason::STEADY_STATE);
    }

    #[test]
    fn upgrade_signal_takes_precedence_over_downgrade() {
        // Both signals present — high cache hit rate AND high latency
        // stall. Upgrade wins (latency pain dominates over-provisioning).
        let t = tenant(Tier::Tier2);
        let m = mix(ConcentrationClass::Broad, Some("fp"), 1_000);
        let mut s = signals_with_count(1_000);
        s.cache_hit_rate = 0.95;
        s.latency_stall_rate = 0.30;
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Upgrade);
        assert_eq!(r.reason, reason::LATENCY_STALLS);
    }

    #[test]
    fn dominant_fingerprint_propagates_to_recommendation() {
        let t = tenant(Tier::Tier4);
        let m = mix(ConcentrationClass::Diverse, Some("abc123"), 1_000);
        let s = signals_with_count(1_000);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.dominant_fingerprint.as_deref(), Some("abc123"));
    }

    #[test]
    fn missing_dominant_fingerprint_omitted_in_json() {
        let t = tenant(Tier::Tier4);
        let mut m = mix(ConcentrationClass::Broad, None, 1_000);
        m.dominant_shape = None;
        let s = signals_with_count(1_000);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        let json = r.to_audit_json();
        assert!(json.get("dominant_fingerprint").is_none());
    }

    #[test]
    fn hold_omits_suggested_tier() {
        let t = tenant(Tier::Tier4);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 1_000);
        let s = signals_with_count(1_000);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert!(r.suggested_tier.is_none());
    }

    #[test]
    fn recommendation_kind_labels_are_bounded_snake_case() {
        let labels = [
            RecommendationKind::Upgrade.label(),
            RecommendationKind::Hold.label(),
            RecommendationKind::Downgrade.label(),
        ];
        assert_eq!(labels, ["upgrade", "hold", "downgrade"]);
    }

    #[test]
    fn reason_labels_are_pinned_snake_case() {
        for label in [
            reason::HIGH_OVER_BUDGET_RATE,
            reason::CONCENTRATED_HOT_WORKLOAD,
            reason::LATENCY_STALLS,
            reason::STEADY_STATE,
            reason::UNDERUTILIZED,
            reason::INSUFFICIENT_SIGNAL,
            reason::CEILING_REACHED,
        ] {
            assert!(!label.is_empty());
            assert!(label.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }

    #[test]
    fn recommendation_round_trips_via_json() {
        let t = tenant(Tier::Tier2);
        let m = mix(ConcentrationClass::HighlyConcentrated, Some("fp"), 1_000);
        let s = signals_with_count(1_000);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        let s_json = serde_json::to_string(&r).unwrap();
        let back: Recommendation = serde_json::from_str(&s_json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn over_budget_threshold_exactly_at_boundary_triggers_upgrade() {
        // 0.10 is the upgrade threshold — at-boundary should fire
        // (>=, not >).
        let t = tenant(Tier::Tier2);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 1_000);
        let mut s = signals_with_count(1_000);
        s.over_budget_rate = 0.10;
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.kind, RecommendationKind::Upgrade);
    }

    #[test]
    fn min_samples_at_exact_boundary_does_not_short_circuit() {
        // min_samples is "below" → strict less-than. Exactly at the
        // threshold proceeds to the real classifier.
        let t = tenant(Tier::Tier4);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 100);
        let s = signals_with_count(100);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_ne!(r.reason, reason::INSUFFICIENT_SIGNAL);
    }

    #[test]
    fn current_tier_label_propagates() {
        let t = tenant(Tier::Tier5);
        let m = mix(ConcentrationClass::Diverse, Some("fp"), 1_000);
        let s = signals_with_count(1_000);
        let r = recommend(
            &RecommendationInputs {
                tenant: &t,
                mix: &m,
                signals: s,
            },
            &cfg(),
        );
        assert_eq!(r.current_tier, "tier5");
    }
}
