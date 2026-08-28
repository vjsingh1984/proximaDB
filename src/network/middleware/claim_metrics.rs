// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Counters for the security gates whose decisions were previously visible
//! only as **warn-once-per-process log lines** — which made the gates
//! unnoperable at fleet scale.
//!
//! Two gates, one shared defect shape:
//!
//! * **Deprecated claim use** (TD-TENANT-3 S4): Arrow Flight's legacy tenant
//!   aliases warn once per process. A long-lived pod therefore goes log-silent
//!   while clients keep using the alias every second — "quiet warnings" would
//!   falsely read as "clients migrated", and the alias would be retired while
//!   still in active use. A *cumulative per-use* counter makes the retirement
//!   gate a query: **retire when `increase(proximadb_deprecated_claim_uses_total[7d]) == 0`**.
//! * **Dropped tier claims** (ADR-0053 W8): a rejected entitlement claim is
//!   dropped and warned once per tenant. How many claims a deployment drops,
//!   on which surfaces and why, was invisible in aggregate — you could not
//!   tell a healthy deployment from one silently stripping every claim.
//!
//! Labels are compile-time-bounded (3 aliases × 4 surfaces × 2 reasons); no
//! request data enters a label, so cardinality cannot explode.

use lazy_static::lazy_static;
use prometheus::{CounterVec, register_counter_vec};

/// Panic-policy-safe `CounterVec` registration (same shape as
/// `metrics/operational_metrics.rs`): fall back to a fresh unregistered vec on
/// the pathological double-register rather than `.expect()` — registration
/// failure must not take the process down (the gate degrades to uncounted,
/// matching the `.map(inc)`-style guards at the recording sites).
fn counter_vec(name: &str, help: &str, labels: &[&str]) -> CounterVec {
    register_counter_vec!(name, help, labels).unwrap_or_else(|_| {
        CounterVec::new(prometheus::Opts::new(name, help), labels)
            .unwrap_or_else(|_| unreachable!("valid counter metric descriptor"))
    })
}

lazy_static! {
    /// Every use of a deprecated claim name (per use, NOT per process — this
    /// is the counter the once-per-process warn deliberately is not).
    pub static ref DEPRECATED_CLAIM_USES: CounterVec = counter_vec(
        "proximadb_deprecated_claim_uses_total",
        "Total uses of deprecated claim names (e.g. Arrow Flight legacy tenant-alias headers). \
         Cumulative per use; the TD-TENANT-3 S4 retirement gate is \
         increase(...) == 0 over the observation window.",
        &["surface", "name"],
    );

    /// Every entitlement claim dropped by the tier trust gate.
    pub static ref TIER_CLAIMS_DROPPED: CounterVec = counter_vec(
        "proximadb_tier_claims_dropped_total",
        "Total X-Tenant-Tier/x-tenant-tier/proximadb_tier claims DROPPED by \
         PROXIMADB_TIER_HEADER_TRUST (ADR-0053 W8), by ingress surface and \
         rejection reason. The request proceeds at the default tier.",
        &["surface", "reason"],
    );
}

/// Record one use of a deprecated claim name (TD-TENANT-3).
pub fn record_deprecated_claim_use(surface: &'static str, name: &'static str) {
    // Unwrap-free: label values are &'static compile-time strings, so
    // get_metric_with_label_values cannot fail on cardinality grounds; on the
    // pathological double-register the fallback is to skip counting rather
    // than fail a request over telemetry.
    let _ = DEPRECATED_CLAIM_USES
        .get_metric_with_label_values(&[surface, name])
        .map(|c| c.inc());
}

/// Record one tier-claim drop, with a bounded reason label (ADR-0053 W8).
pub fn record_tier_claim_dropped(
    surface: &'static str,
    rejection: &proximadb_tenant::TierClaimRejection,
) {
    let reason: &'static str = match rejection {
        proximadb_tenant::TierClaimRejection::Unauthenticated { .. } => "unauthenticated",
        proximadb_tenant::TierClaimRejection::NonGatewayPrincipal { .. } => "non_gateway_principal",
    };
    let _ = TIER_CLAIMS_DROPPED
        .get_metric_with_label_values(&[surface, reason])
        .map(|c| c.inc());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecated_use_counter_is_per_use_not_once_per_process() {
        let read = || {
            DEPRECATED_CLAIM_USES
                .get_metric_with_label_values(&["flight", "tenant_id"])
                .map(|c| c.get())
                .unwrap_or(0.0)
        };
        let before = read();
        record_deprecated_claim_use("flight", "tenant_id");
        record_deprecated_claim_use("flight", "tenant_id");
        // Two increments move the counter by exactly two — the property the
        // once-per-process warn lacks and the retirement gate depends on.
        assert_eq!(read() - before, 2.0);
    }

    #[test]
    fn tier_drop_reasons_map_to_bounded_labels() {
        let unauth = proximadb_tenant::TierClaimRejection::Unauthenticated {
            tenant: "t".into(),
            claim: "enterprise".into(),
        };
        record_tier_claim_dropped("rest", &unauth);
        assert!(
            TIER_CLAIMS_DROPPED
                .get_metric_with_label_values(&["rest", "unauthenticated"])
                .is_ok()
        );
    }
}
