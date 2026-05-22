// Cross-tenant audit guardrail — LLD §6.3 risk mitigation.
//
// Several primitives in the retrieval-cost stack write tenant-scoped data
// keyed on (tenant_id, collection, …). The CatapultTable, PlanCache,
// BatchGroupCache, and RecordBufferPool all maintain tenant isolation by
// construction — the keys never cross tenants. But the LLD §6.3 risk row
// explicitly calls out:
//
//   "Catapult cross-tenant leakage. Catapult tables are per-collection
//    and per-tenant; never share. Audit hook in the CDC sink emits a
//    guardrail event if a catapult write crosses tenants."
//
// This module is the typed event + decision logic that hook calls. The
// CDC sink fan-out lives at the call site (it talks to the actual logger /
// SIEM); the data plane here just decides:
//
//   1. Is this a cross-tenant operation? (`expected_tenant` != `actual_tenant`)
//   2. Is it explicitly allowed? (an operator-defined exemption list)
//   3. What severity? (Info / Warn / Critical based on operation type)
//
// And returns a typed `CrossTenantEvent` the caller emits to the audit log.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Operation type that may cross tenants. Each variant has a default
/// severity assignment in `Operation::default_severity`. Variants that
/// touch durable graph state (catapult writes) are Critical; in-memory
/// cache touches are Warn; trace emissions are Info (still logged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Catapult shortcut write — modifies durable graph state.
    CatapultWrite,
    /// Catapult shortcut lookup — read-only but still tenant-scoped.
    CatapultLookup,
    /// Plan cache read or write.
    PlanCache,
    /// Result cache (query result cache) read or write.
    ResultCache,
    /// Batch-group cache touch.
    BatchGroupCache,
    /// SearchPlanTrace emission to a downstream sink.
    TraceEmit,
    /// Field-statistics refresh observing a tenant's rows.
    StatsObserve,
}

impl Operation {
    pub const fn default_severity(self) -> Severity {
        match self {
            // Durable graph mutation across tenants is the worst case —
            // could leak query trajectories across customers.
            Operation::CatapultWrite => Severity::Critical,
            // Stats observation crossing tenants would taint the
            // shared estimator inputs — quiet but high-impact.
            Operation::StatsObserve => Severity::Critical,
            // Read paths are warn-level: they reveal that the boundary
            // *was* probed even if no data crossed.
            Operation::CatapultLookup => Severity::Warn,
            Operation::PlanCache => Severity::Warn,
            Operation::ResultCache => Severity::Warn,
            Operation::BatchGroupCache => Severity::Warn,
            // Trace emissions are logged because a misconfigured sink
            // could fan a trace to the wrong tenant's CDC stream.
            Operation::TraceEmit => Severity::Info,
        }
    }

    /// Bounded label for Prometheus / SIEM ingest — keeps cardinality safe.
    pub const fn label(self) -> &'static str {
        match self {
            Operation::CatapultWrite => "catapult_write",
            Operation::CatapultLookup => "catapult_lookup",
            Operation::PlanCache => "plan_cache",
            Operation::ResultCache => "result_cache",
            Operation::BatchGroupCache => "batch_group_cache",
            Operation::TraceEmit => "trace_emit",
            Operation::StatsObserve => "stats_observe",
        }
    }
}

/// Bounded severity ladder — the audit sink uses this to decide whether
/// to page on-call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Critical,
}

impl Severity {
    pub const fn label(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Critical => "critical",
        }
    }
}

/// Structured event emitted when a cross-tenant operation is detected.
/// Serializes to JSON for SIEM ingest; fields are deliberately flat to
/// match the audit logger's existing schema.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CrossTenantEvent {
    /// Always "cross_tenant" — pinned constant for SIEM filtering.
    pub event: &'static str,
    /// Operation that tripped the guard.
    pub operation: &'static str,
    /// Bounded severity label.
    pub severity: &'static str,
    /// Tenant the caller declared they were operating on.
    pub expected_tenant: String,
    /// Tenant the data actually belonged to.
    pub actual_tenant: String,
    /// Optional collection scope — populated for cache + catapult events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    /// Whether the operation was blocked (true) or merely audited (false).
    /// Reads that were attempted across tenants are blocked at the type
    /// level by the scope keys; this flag distinguishes "we caught it
    /// before any data moved" from "we caught it and the data was
    /// already in flight to the wrong sink".
    pub blocked: bool,
}

/// Allow-list of tenant pairs that may legitimately operate together
/// (e.g. a parent-org / sub-org relationship the control plane
/// declared). The decision logic skips emission for these pairs.
#[derive(Debug, Clone, Default)]
pub struct AllowList {
    pairs: HashSet<(String, String)>,
}

impl AllowList {
    pub fn new() -> Self {
        Self {
            pairs: HashSet::new(),
        }
    }

    /// Declare that `expected` may operate on data scoped to `actual`.
    /// The relationship is symmetric — registering A→B also allows B→A.
    pub fn allow(&mut self, expected: impl Into<String>, actual: impl Into<String>) {
        let a = expected.into();
        let b = actual.into();
        self.pairs.insert((a.clone(), b.clone()));
        self.pairs.insert((b, a));
    }

    /// Check whether the pair is allowed.
    pub fn permits(&self, expected: &str, actual: &str) -> bool {
        self.pairs
            .contains(&(expected.to_string(), actual.to_string()))
    }
}

/// Decide whether to emit an event. Returns `None` when no cross-tenant
/// boundary was crossed (the common case) OR when the pair is on the
/// allow-list; returns `Some(event)` otherwise.
pub fn evaluate(
    operation: Operation,
    expected_tenant: &str,
    actual_tenant: &str,
    collection: Option<&str>,
    blocked: bool,
    allow_list: &AllowList,
) -> Option<CrossTenantEvent> {
    if expected_tenant == actual_tenant {
        return None;
    }
    if allow_list.permits(expected_tenant, actual_tenant) {
        return None;
    }
    Some(CrossTenantEvent {
        event: "cross_tenant",
        operation: operation.label(),
        severity: operation.default_severity().label(),
        expected_tenant: expected_tenant.to_string(),
        actual_tenant: actual_tenant.to_string(),
        collection: collection.map(str::to_string),
        blocked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_tenant_never_emits() {
        let allow = AllowList::default();
        for op in [
            Operation::CatapultWrite,
            Operation::CatapultLookup,
            Operation::PlanCache,
            Operation::ResultCache,
            Operation::BatchGroupCache,
            Operation::TraceEmit,
            Operation::StatsObserve,
        ] {
            assert!(
                evaluate(op, "tenant-a", "tenant-a", None, true, &allow).is_none(),
                "same-tenant must never emit for {:?}",
                op
            );
        }
    }

    #[test]
    fn cross_tenant_emits_event_with_pinned_shape() {
        let allow = AllowList::default();
        let ev = evaluate(
            Operation::CatapultWrite,
            "tenant-a",
            "tenant-b",
            Some("kb"),
            true,
            &allow,
        )
        .expect("must emit");
        assert_eq!(ev.event, "cross_tenant");
        assert_eq!(ev.operation, "catapult_write");
        assert_eq!(ev.severity, "critical");
        assert_eq!(ev.expected_tenant, "tenant-a");
        assert_eq!(ev.actual_tenant, "tenant-b");
        assert_eq!(ev.collection.as_deref(), Some("kb"));
        assert!(ev.blocked);
    }

    #[test]
    fn allow_list_suppresses_emission() {
        let mut allow = AllowList::new();
        allow.allow("parent-org", "sub-org-1");
        // Both directions are permitted.
        assert!(
            evaluate(
                Operation::PlanCache,
                "parent-org",
                "sub-org-1",
                None,
                false,
                &allow
            )
            .is_none()
        );
        assert!(
            evaluate(
                Operation::PlanCache,
                "sub-org-1",
                "parent-org",
                None,
                false,
                &allow
            )
            .is_none()
        );
        // Unrelated cross-tenant pair still emits.
        assert!(
            evaluate(
                Operation::PlanCache,
                "parent-org",
                "other",
                None,
                false,
                &allow
            )
            .is_some()
        );
    }

    #[test]
    fn severity_mapping_pins_to_lld_classes() {
        // Durable graph mutation = critical
        assert_eq!(
            Operation::CatapultWrite.default_severity(),
            Severity::Critical
        );
        // Stats observation = critical (taints shared estimator)
        assert_eq!(
            Operation::StatsObserve.default_severity(),
            Severity::Critical
        );
        // Read paths = warn
        assert_eq!(Operation::CatapultLookup.default_severity(), Severity::Warn);
        assert_eq!(Operation::PlanCache.default_severity(), Severity::Warn);
        assert_eq!(Operation::ResultCache.default_severity(), Severity::Warn);
        assert_eq!(
            Operation::BatchGroupCache.default_severity(),
            Severity::Warn
        );
        // Trace emit = info
        assert_eq!(Operation::TraceEmit.default_severity(), Severity::Info);
    }

    #[test]
    fn operation_labels_are_bounded_static_strings() {
        // Bounded-cardinality invariant — the label must be a `&'static str`
        // for safe Prometheus registration without per-call allocation.
        let labels = [
            Operation::CatapultWrite.label(),
            Operation::CatapultLookup.label(),
            Operation::PlanCache.label(),
            Operation::ResultCache.label(),
            Operation::BatchGroupCache.label(),
            Operation::TraceEmit.label(),
            Operation::StatsObserve.label(),
        ];
        for l in &labels {
            assert!(!l.is_empty());
            assert!(l.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        // No two operations share a label.
        let unique: std::collections::HashSet<_> = labels.iter().copied().collect();
        assert_eq!(unique.len(), labels.len());
    }

    #[test]
    fn severity_labels_are_bounded() {
        let labels = [
            Severity::Info.label(),
            Severity::Warn.label(),
            Severity::Critical.label(),
        ];
        assert_eq!(labels, ["info", "warn", "critical"]);
    }

    #[test]
    fn collection_field_is_optional() {
        let allow = AllowList::default();
        let ev = evaluate(
            Operation::TraceEmit,
            "tenant-a",
            "tenant-b",
            None,
            false,
            &allow,
        )
        .unwrap();
        assert!(ev.collection.is_none());
        let json = serde_json::to_value(&ev).unwrap();
        assert!(
            json.get("collection").is_none(),
            "None collection must skip serialization"
        );
    }

    #[test]
    fn collection_field_serializes_when_present() {
        let allow = AllowList::default();
        let ev = evaluate(
            Operation::ResultCache,
            "tenant-a",
            "tenant-b",
            Some("kb"),
            true,
            &allow,
        )
        .unwrap();
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["collection"], serde_json::json!("kb"));
    }

    #[test]
    fn allow_list_is_symmetric() {
        let mut allow = AllowList::new();
        allow.allow("a", "b");
        assert!(allow.permits("a", "b"));
        assert!(allow.permits("b", "a"));
        assert!(!allow.permits("a", "c"));
    }

    #[test]
    fn unrelated_pairs_do_not_match_after_allow() {
        // Adding (a, b) must not permit (a, c) — independence between
        // allow-list entries.
        let mut allow = AllowList::new();
        allow.allow("a", "b");
        allow.allow("c", "d");
        assert!(allow.permits("c", "d"));
        assert!(!allow.permits("a", "d"));
        assert!(!allow.permits("b", "c"));
    }

    #[test]
    fn blocked_field_flows_through() {
        let allow = AllowList::default();
        let blocked = evaluate(
            Operation::CatapultWrite,
            "tenant-a",
            "tenant-b",
            None,
            true,
            &allow,
        )
        .unwrap();
        assert!(blocked.blocked);
        let leaked = evaluate(
            Operation::TraceEmit,
            "tenant-a",
            "tenant-b",
            None,
            false,
            &allow,
        )
        .unwrap();
        assert!(!leaked.blocked);
    }

    #[test]
    fn json_event_field_is_always_cross_tenant() {
        // SIEM ingest filters on the "event" field — pin the constant.
        let allow = AllowList::default();
        let ev = evaluate(
            Operation::CatapultLookup,
            "tenant-a",
            "tenant-b",
            None,
            true,
            &allow,
        )
        .unwrap();
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event"], "cross_tenant");
    }
}
