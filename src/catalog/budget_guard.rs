// Budget guard — wraps the per-tenant scan/ef-search check into one call.
//
// Every search call site needs the same precondition:
//   1. Look up the resolved tenant tier.
//   2. Apply the caller's `scan_budget_gb` (defaulting to the tier's hard
//      cap when omitted).
//   3. Apply the caller's `ef_search` (defaulting to the tier's hard cap
//      when omitted).
//   4. Reject with a structured explain when either ceiling trips.
//
// The Phase 0 router already exposes `RouteContext::check_tenant_caps` for
// the hard-cap rejection inside the data-plane. This helper covers the
// gateway-side soft-cap path — same explain JSON shape, but the error
// can be mapped to HTTP 429 by the caller without reaching into the
// router-internal `TenantBudgetExceeded` type.

use crate::catalog::tenant_tier::TenantTierRecord;

/// Outcome of a guard call. The runtime substitutes the resolved values
/// into the search request (defaults pick up when the caller omitted a
/// param) and the rejection carries the structured explain JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct EnforcedBudget {
    /// Scan budget the runtime should pass through to the router.
    pub effective_scan_gb: f64,
    /// ef_search the runtime should pass through to the router.
    pub effective_ef_search: u32,
    /// Echo of the tier's prometheus label so the runtime can attach it
    /// to bounded-cardinality counters without re-deriving.
    pub tier_label: &'static str,
}

/// Rejection — caller maps to HTTP 429 + serialized JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetRejection {
    /// Which ceiling tripped: "scan_budget_gb" | "ef_search_cap".
    pub which: &'static str,
    /// The ceiling value (in GB or unitless ef count).
    pub limit: f64,
    /// The requested value.
    pub requested: f64,
    /// Tenant id — echoed so the gateway can log the rejection target.
    pub tenant_id: String,
    /// Bounded tier label.
    pub tier_label: &'static str,
}

impl BudgetRejection {
    /// Serialize to the JSON shape the AnvaiOps gateway emits. Matches
    /// `services::tier_cache::BudgetExceeded.explain()` so customer-facing
    /// payloads are identical whether the gateway or the data-plane
    /// rejected.
    pub fn to_explain_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error":     "budget_exceeded",
            "which":     self.which,
            "limit":     self.limit,
            "requested": self.requested,
            "tenant_id": self.tenant_id,
            "tier":      self.tier_label,
            "hint":      "Lower scan_budget_gb / ef_search or upgrade tier.",
        })
    }
}

/// Run the soft-cap check. `requested_scan_gb` and `requested_ef_search`
/// are `None` when the caller didn't supply them; the helper substitutes
/// the tier's default in that case rather than rejecting.
pub fn enforce(
    record: &TenantTierRecord,
    requested_scan_gb: Option<f64>,
    requested_ef_search: Option<u32>,
) -> Result<EnforcedBudget, BudgetRejection> {
    let scan_limit = record.effective_scan_budget_gb();
    let ef_limit = record.effective_ef_search_cap();
    let tier_label = record.tier.prometheus_label();

    // Step 1: scan budget.
    let effective_scan_gb = match requested_scan_gb {
        None => scan_limit,
        Some(r) if r.is_nan() => {
            // NaN can't be compared meaningfully — reject explicitly so the
            // caller gets a clear signal rather than a silently substituted
            // default.
            return Err(BudgetRejection {
                which: "scan_budget_gb",
                limit: scan_limit,
                requested: r,
                tenant_id: record.tenant_id.clone(),
                tier_label,
            });
        }
        Some(r) if r > scan_limit => {
            return Err(BudgetRejection {
                which: "scan_budget_gb",
                limit: scan_limit,
                requested: r,
                tenant_id: record.tenant_id.clone(),
                tier_label,
            });
        }
        Some(r) => r.max(0.0),
    };

    // Step 2: ef_search.
    let effective_ef_search = match requested_ef_search {
        None => ef_limit,
        Some(r) if r > ef_limit => {
            return Err(BudgetRejection {
                which: "ef_search_cap",
                limit: f64::from(ef_limit),
                requested: f64::from(r),
                tenant_id: record.tenant_id.clone(),
                tier_label,
            });
        }
        Some(r) => r,
    };

    Ok(EnforcedBudget {
        effective_scan_gb,
        effective_ef_search,
        tier_label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tenant_tier::{FeatureFlags, Tier};

    fn record(tier: Tier, scan: Option<f64>, ef: Option<u32>) -> TenantTierRecord {
        TenantTierRecord {
            tenant_id: "tenant-a".into(),
            tier,
            scan_budget_gb_hard: scan,
            ef_search_cap: ef,
            freshness_sla_seconds: None,
            feature_flags: FeatureFlags::default(),
        }
    }

    #[test]
    fn no_request_substitutes_tier_defaults() {
        let r = record(Tier::Business, None, None);
        let ok = enforce(&r, None, None).expect("should succeed");
        assert_eq!(
            ok.effective_scan_gb,
            Tier::Business.default_scan_budget_gb()
        );
        assert_eq!(
            ok.effective_ef_search,
            Tier::Business.default_ef_search_cap()
        );
        assert_eq!(ok.tier_label, "business");
    }

    #[test]
    fn within_budget_passes() {
        let r = record(Tier::Community, None, None);
        let ok = enforce(&r, Some(1.0), Some(64)).expect("should succeed");
        assert_eq!(ok.effective_scan_gb, 1.0);
        assert_eq!(ok.effective_ef_search, 64);
    }

    #[test]
    fn scan_exceeded_returns_structured_rejection() {
        let r = record(Tier::Community, None, None);
        let err = enforce(&r, Some(100.0), None).expect_err("should reject");
        assert_eq!(err.which, "scan_budget_gb");
        assert_eq!(err.limit, Tier::Community.default_scan_budget_gb());
        assert_eq!(err.requested, 100.0);
        assert_eq!(err.tier_label, "community");
        assert_eq!(err.tenant_id, "tenant-a");
    }

    #[test]
    fn ef_search_exceeded_returns_structured_rejection() {
        let r = record(Tier::Community, None, None);
        let err = enforce(&r, None, Some(10_000)).expect_err("should reject");
        assert_eq!(err.which, "ef_search_cap");
        assert_eq!(
            err.limit,
            f64::from(Tier::Community.default_ef_search_cap())
        );
        assert_eq!(err.requested, 10_000.0);
    }

    #[test]
    fn negative_scan_clamps_to_zero() {
        let r = record(Tier::Community, None, None);
        let ok = enforce(&r, Some(-1.0), None).expect("negative scan is clamped, not rejected");
        assert_eq!(ok.effective_scan_gb, 0.0);
    }

    #[test]
    fn nan_scan_is_rejected_explicitly() {
        let r = record(Tier::Community, None, None);
        let err = enforce(&r, Some(f64::NAN), None).expect_err("NaN must reject");
        assert_eq!(err.which, "scan_budget_gb");
        assert!(err.requested.is_nan());
    }

    #[test]
    fn per_tenant_override_takes_precedence_over_default() {
        // Custom hard cap of 0.1 GB on a community tenant.
        let r = record(Tier::Community, Some(0.1), Some(96));
        // 0.2 exceeds the per-tenant override even though it's well below
        // the tier default.
        let err = enforce(&r, Some(0.2), None).expect_err("override should reject");
        assert_eq!(err.limit, 0.1);
        // 0.05 fits the override.
        let ok = enforce(&r, Some(0.05), None).expect("within override");
        assert_eq!(ok.effective_scan_gb, 0.05);
    }

    #[test]
    fn explain_json_shape_is_stable() {
        let r = record(Tier::Enterprise, None, None);
        let err = enforce(&r, Some(10_000.0), None).expect_err("reject");
        let json = err.to_explain_json();
        // Pin the exact shape the gateway responds with.
        assert_eq!(json["error"], "budget_exceeded");
        assert_eq!(json["which"], "scan_budget_gb");
        assert_eq!(json["tier"], "enterprise");
        assert_eq!(json["tenant_id"], "tenant-a");
        assert!(json.get("limit").is_some());
        assert!(json.get("requested").is_some());
        assert!(json.get("hint").is_some());
    }

    #[test]
    fn tier_label_is_bounded_string_set() {
        // The label must always be one of the four bounded values so
        // Prometheus cardinality stays safe.
        for tier in [
            Tier::FreeTrial,
            Tier::Community,
            Tier::Business,
            Tier::Enterprise,
        ] {
            let r = record(tier, None, None);
            let ok = enforce(&r, None, None).expect("default succeeds");
            assert!(
                matches!(
                    ok.tier_label,
                    "free" | "community" | "business" | "enterprise"
                ),
                "label {} must be in the bounded set",
                ok.tier_label
            );
        }
    }

    #[test]
    fn at_exact_limit_passes() {
        // Requested == limit must succeed — strict less-than would be a
        // surprising customer-visible boundary.
        let r = record(Tier::Community, None, None);
        let scan_limit = Tier::Community.default_scan_budget_gb();
        let ef_limit = Tier::Community.default_ef_search_cap();
        let ok = enforce(&r, Some(scan_limit), Some(ef_limit)).expect("exact limit passes");
        assert_eq!(ok.effective_scan_gb, scan_limit);
        assert_eq!(ok.effective_ef_search, ef_limit);
    }

    #[test]
    fn rejection_carries_specific_tenant_id() {
        // The rejection must echo the actual tenant id so observability
        // can attribute it; a generic "<unknown>" would be a bug.
        let mut r = record(Tier::Community, None, None);
        r.tenant_id = "tenant-zzz".into();
        let err = enforce(&r, Some(100.0), None).expect_err("reject");
        assert_eq!(err.tenant_id, "tenant-zzz");
    }
}
