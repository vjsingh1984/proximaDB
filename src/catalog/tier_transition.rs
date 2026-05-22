// Tier transition detector — classifies a TenantTierRecord change.
//
// When a tenant changes tiers mid-billing-cycle (upgrade, downgrade,
// or a lateral re-classification), the gateway needs to:
//   1. Emit an audit event so finance can reconcile prorated billing.
//   2. Invalidate caches that held the old budget (PlanCache,
//      ResultCache via invalidation_coordinator).
//   3. Surface the change in observability so on-call sees the new
//      headroom on the rollup metrics.
//
// This module produces a typed `TierTransition` event from a before /
// after snapshot pair. Pure-data — the audit emitter, the cache
// invalidator, and the metric emitter all consume the same struct.
//
// The classification considers:
//   - `Tier` change (strongest signal — class change always wins).
//   - Effective scan budget delta (when tier is identical, an
//     override change still moves the tenant up/down).
//   - Effective ef_search_cap delta.
//   - Effective freshness SLA delta.
//
// Direction rules:
//   - Higher tier OR higher scan budget OR higher ef cap OR LOWER
//     freshness SLA (faster) → `Upgrade`.
//   - Lower tier OR lower scan budget OR lower ef cap OR HIGHER
//     freshness SLA (slower) → `Downgrade`.
//   - Mixed (some up, some down) → `Lateral`.
//   - All identical → `NoChange`.

use serde::{Deserialize, Serialize};

use crate::catalog::tenant_tier::{TenantTierRecord, Tier};

/// Direction the transition moved on a single axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisDirection {
    Up,
    Down,
    Flat,
}

impl AxisDirection {
    pub const fn label(self) -> &'static str {
        match self {
            AxisDirection::Up => "up",
            AxisDirection::Down => "down",
            AxisDirection::Flat => "flat",
        }
    }
}

/// Per-axis delta. `delta` is signed: positive = the field's value
/// increased, negative = decreased.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AxisDelta {
    pub before: f64,
    pub after: f64,
    pub direction: AxisDirection,
}

impl AxisDelta {
    fn classify(before: f64, after: f64) -> AxisDirection {
        if !before.is_finite() || !after.is_finite() {
            return AxisDirection::Flat;
        }
        if (after - before).abs() < f64::EPSILON {
            AxisDirection::Flat
        } else if after > before {
            AxisDirection::Up
        } else {
            AxisDirection::Down
        }
    }

    pub fn new(before: f64, after: f64) -> Self {
        Self {
            before,
            after,
            direction: Self::classify(before, after),
        }
    }
}

/// Overall transition class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionClass {
    Upgrade,
    Downgrade,
    Lateral,
    NoChange,
}

impl TransitionClass {
    pub const fn label(self) -> &'static str {
        match self {
            TransitionClass::Upgrade => "upgrade",
            TransitionClass::Downgrade => "downgrade",
            TransitionClass::Lateral => "lateral",
            TransitionClass::NoChange => "no_change",
        }
    }

    pub fn is_no_change(self) -> bool {
        matches!(self, TransitionClass::NoChange)
    }
}

/// Structured audit event for the transition.
///
/// The `tier_before` / `tier_after` fields are owned `String` so the
/// event round-trips through JSON; the values themselves are always
/// from the bounded `{free, team, pro, business, enterprise}` set, so
/// observability cardinality stays safe. Use `event.class.label()`
/// when a `&'static str` of the class is needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierTransitionEvent {
    pub tenant_id: String,
    pub class: TransitionClass,
    pub tier_before: String,
    pub tier_after: String,
    pub scan_budget_gb: AxisDelta,
    pub ef_search_cap: AxisDelta,
    pub freshness_sla_seconds: AxisDelta,
}

impl TierTransitionEvent {
    /// Serialize to JSON suitable for the audit log.
    pub fn to_audit_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// Compute the transition between two snapshots. The tenant_id must
/// match — caller responsibility.
pub fn detect(before: &TenantTierRecord, after: &TenantTierRecord) -> TierTransitionEvent {
    let scan_budget = AxisDelta::new(
        before.effective_scan_budget_gb(),
        after.effective_scan_budget_gb(),
    );
    let ef_cap = AxisDelta::new(
        f64::from(before.effective_ef_search_cap()),
        f64::from(after.effective_ef_search_cap()),
    );
    // Freshness SLA semantics flip: a LOWER number = faster = "upgrade
    // direction". Invert the after/before pair before constructing the
    // delta so `direction` reflects user-experience direction, not raw
    // numeric direction.
    let freshness = AxisDelta::new(
        f64::from(after.effective_freshness_sla_seconds()), // smaller = better
        f64::from(before.effective_freshness_sla_seconds()),
    );

    let tier_dir = compare_tiers(before.tier, after.tier);
    let class = classify(
        tier_dir,
        scan_budget.direction,
        ef_cap.direction,
        freshness.direction,
    );

    TierTransitionEvent {
        tenant_id: after.tenant_id.clone(),
        class,
        tier_before: before.tier.prometheus_label().to_string(),
        tier_after: after.tier.prometheus_label().to_string(),
        scan_budget_gb: scan_budget,
        ef_search_cap: ef_cap,
        freshness_sla_seconds: freshness,
    }
}

/// Tier ordering for the dominant-axis check. Higher tier = up.
fn compare_tiers(before: Tier, after: Tier) -> AxisDirection {
    let bi = tier_rank(before);
    let ai = tier_rank(after);
    if ai > bi {
        AxisDirection::Up
    } else if ai < bi {
        AxisDirection::Down
    } else {
        AxisDirection::Flat
    }
}

fn tier_rank(t: Tier) -> u8 {
    match t {
        Tier::FreeTrial => 0,
        Tier::Team => 1,
        Tier::Pro => 2,
        Tier::Business => 3,
        Tier::Enterprise => 4,
    }
}

/// Combine the per-axis directions into an overall class.
///
/// Rules (tier change dominates):
///   - tier_dir == Up   → Upgrade
///   - tier_dir == Down → Downgrade
///   - tier_dir == Flat:
///       all-flat → NoChange
///       any axis Up + no Down → Upgrade
///       any axis Down + no Up → Downgrade
///       mixed → Lateral
fn classify(
    tier_dir: AxisDirection,
    scan_dir: AxisDirection,
    ef_dir: AxisDirection,
    fresh_dir: AxisDirection,
) -> TransitionClass {
    match tier_dir {
        AxisDirection::Up => return TransitionClass::Upgrade,
        AxisDirection::Down => return TransitionClass::Downgrade,
        AxisDirection::Flat => {}
    }
    let axes = [scan_dir, ef_dir, fresh_dir];
    let any_up = axes.iter().any(|d| matches!(d, AxisDirection::Up));
    let any_down = axes.iter().any(|d| matches!(d, AxisDirection::Down));
    match (any_up, any_down) {
        (false, false) => TransitionClass::NoChange,
        (true, false) => TransitionClass::Upgrade,
        (false, true) => TransitionClass::Downgrade,
        (true, true) => TransitionClass::Lateral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tenant_tier::{FeatureFlags, Tier};

    fn record(
        tier: Tier,
        scan: Option<f64>,
        ef: Option<u32>,
        fresh: Option<u32>,
    ) -> TenantTierRecord {
        TenantTierRecord {
            tenant_id: "tenant-a".into(),
            tier,
            scan_budget_gb_hard: scan,
            ef_search_cap: ef,
            freshness_sla_seconds: fresh,
            feature_flags: FeatureFlags::default(),
        }
    }

    #[test]
    fn no_change_when_records_identical() {
        let a = record(Tier::Business, None, None, None);
        let b = record(Tier::Business, None, None, None);
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::NoChange);
        assert!(ev.class.is_no_change());
    }

    #[test]
    fn tier_up_is_upgrade() {
        let a = record(Tier::Team, None, None, None);
        let b = record(Tier::Business, None, None, None);
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Upgrade);
        assert_eq!(ev.tier_before, "team");
        assert_eq!(ev.tier_after, "business");
    }

    #[test]
    fn tier_down_is_downgrade() {
        let a = record(Tier::Enterprise, None, None, None);
        let b = record(Tier::FreeTrial, None, None, None);
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Downgrade);
    }

    #[test]
    fn same_tier_higher_scan_override_is_upgrade() {
        // No tier change, but the override doubled the scan budget.
        let a = record(Tier::Business, Some(10.0), None, None);
        let b = record(Tier::Business, Some(20.0), None, None);
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Upgrade);
        assert_eq!(ev.scan_budget_gb.direction, AxisDirection::Up);
    }

    #[test]
    fn same_tier_lower_scan_override_is_downgrade() {
        let a = record(Tier::Business, Some(20.0), None, None);
        let b = record(Tier::Business, Some(10.0), None, None);
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Downgrade);
    }

    #[test]
    fn lateral_when_scan_up_and_ef_down() {
        // Scan budget doubled, ef cap halved → lateral.
        let a = record(Tier::Business, Some(10.0), Some(256), None);
        let b = record(Tier::Business, Some(20.0), Some(128), None);
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Lateral);
    }

    #[test]
    fn freshness_lower_value_classified_as_upgrade() {
        // 60s SLA → 15s SLA = faster = upgrade direction even though
        // the raw numeric direction is "down".
        let a = record(Tier::Business, None, None, Some(60));
        let b = record(Tier::Business, None, None, Some(15));
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Upgrade);
        assert_eq!(ev.freshness_sla_seconds.direction, AxisDirection::Up);
    }

    #[test]
    fn freshness_higher_value_classified_as_downgrade() {
        // 15s → 300s = slower = downgrade direction.
        let a = record(Tier::Business, None, None, Some(15));
        let b = record(Tier::Business, None, None, Some(300));
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Downgrade);
    }

    #[test]
    fn tier_class_dominates_axis_mix() {
        // Tier moved up; one of the axes moved down — overall still
        // Upgrade because tier_dir wins.
        let a = record(Tier::Team, Some(2.0), Some(128), None);
        let b = record(Tier::Business, Some(1.5), None, None);
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Upgrade);
    }

    #[test]
    fn tier_class_dominates_even_when_axes_disagree() {
        // Tier moved down; raw scan-budget value went up because the
        // user removed an override that was tighter than the new tier
        // default. The class is still Downgrade because tier wins.
        let a = record(Tier::Business, Some(1.0), None, None);
        let b = record(Tier::Team, None, None, None);
        let ev = detect(&a, &b);
        assert_eq!(ev.class, TransitionClass::Downgrade);
    }

    #[test]
    fn class_label_pinned_to_snake_case() {
        let labels = [
            TransitionClass::Upgrade.label(),
            TransitionClass::Downgrade.label(),
            TransitionClass::Lateral.label(),
            TransitionClass::NoChange.label(),
        ];
        assert_eq!(labels, ["upgrade", "downgrade", "lateral", "no_change"]);
    }

    #[test]
    fn axis_direction_label_pinned() {
        assert_eq!(AxisDirection::Up.label(), "up");
        assert_eq!(AxisDirection::Down.label(), "down");
        assert_eq!(AxisDirection::Flat.label(), "flat");
    }

    #[test]
    fn audit_json_carries_every_field() {
        let a = record(Tier::Team, None, None, None);
        let b = record(Tier::Business, None, None, None);
        let json = detect(&a, &b).to_audit_json();
        // Top-level field presence.
        assert!(json.get("tenant_id").is_some());
        assert!(json.get("class").is_some());
        assert!(json.get("tier_before").is_some());
        assert!(json.get("tier_after").is_some());
        assert!(json.get("scan_budget_gb").is_some());
        assert!(json.get("ef_search_cap").is_some());
        assert!(json.get("freshness_sla_seconds").is_some());
        // Each axis carries before/after/direction.
        let scan = &json["scan_budget_gb"];
        assert!(scan.get("before").is_some());
        assert!(scan.get("after").is_some());
        assert!(scan.get("direction").is_some());
    }

    #[test]
    fn event_round_trips_via_json() {
        let a = record(Tier::FreeTrial, None, None, None);
        let b = record(Tier::Team, Some(3.0), None, None);
        let ev = detect(&a, &b);
        let s = serde_json::to_string(&ev).expect("serialize");
        let back: TierTransitionEvent = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(ev, back);
    }

    #[test]
    fn delta_carries_before_and_after_values() {
        // Spot-check: a 2 → 16 GB upgrade keeps both values in the delta.
        let a = record(Tier::Team, None, None, None); // default 2.0
        let b = record(Tier::Business, None, None, None); // default 16.0
        let ev = detect(&a, &b);
        assert_eq!(ev.scan_budget_gb.before, 2.0);
        assert_eq!(ev.scan_budget_gb.after, 16.0);
        assert_eq!(ev.scan_budget_gb.direction, AxisDirection::Up);
    }

    #[test]
    fn axis_delta_classify_is_robust_against_non_finite() {
        // NaN / inf values collapse to Flat rather than crashing the
        // classifier when a fail-open record sneaks in.
        let d = AxisDelta::new(f64::NAN, 5.0);
        assert_eq!(d.direction, AxisDirection::Flat);
        let d = AxisDelta::new(5.0, f64::INFINITY);
        assert_eq!(d.direction, AxisDirection::Flat);
    }

    #[test]
    fn tenant_id_taken_from_after_record() {
        // If the after snapshot has a different tenant_id (caller bug),
        // the event reflects the after value — caller is responsible for
        // pre-checking the IDs match.
        let mut a = record(Tier::Team, None, None, None);
        a.tenant_id = "tenant-a".into();
        let mut b = record(Tier::Business, None, None, None);
        b.tenant_id = "tenant-b".into();
        let ev = detect(&a, &b);
        assert_eq!(ev.tenant_id, "tenant-b");
    }

    #[test]
    fn class_label_helper_returns_bounded_string() {
        let a = record(Tier::Team, None, None, None);
        let b = record(Tier::Business, None, None, None);
        let ev = detect(&a, &b);
        // The class enum exposes a bounded `label()` for observability;
        // callers use it directly rather than reading a struct field.
        assert_eq!(ev.class.label(), "upgrade");
    }
}
