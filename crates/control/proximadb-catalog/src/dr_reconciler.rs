//! DR reconciler decision logic — engine contract P3a (pure core).
//!
//! This module is the "what should happen" half of the DR reconciler. It
//! takes a `(CollectionDrPolicy, ProviderObservedState)` pair and returns
//! a [`ReconcileDecision`] that the async loop (P3b) will dispatch on.
//!
//! It is intentionally pure — no async, no I/O, no time, no metrics —
//! so the state-machine and drift-detection logic can be unit-tested
//! exhaustively for every `(state × observation)` combination. The
//! async driver layer wraps this with lease acquisition, adapter
//! dispatch, event emission, and metric updates.
//!
//! See `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc` "LLD:
//! Reconciler Interface".
//!
//! Layering: depends only on the types in
//! [`crate::collection_dr_policy`]; no provider/adapter coupling. The
//! adapter trait is consumed by the dispatch layer, not by the decision
//! function itself — keeps the decision logic free of async lifetimes.

use crate::collection_dr_policy::{
    CollectionDrPolicy, DrHealthState, DrState, ProviderObservedState,
};

/// What the reconciler should do for a single policy on this poll.
///
/// The async driver maps each variant to a sequence of adapter calls
/// and xCatalog writes. The variant choice is deterministic from the
/// (policy, observed) pair plus the policy's own state — no time, no
/// hidden inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileDecision {
    /// Nothing to do this poll. Used for terminal/idle states
    /// (`Disabled`, `Retired`, `SuspendedByOps`) or for active policies
    /// that are healthy and within RPO. The driver still emits a
    /// `last_reconciled_at_ns` heartbeat.
    Idle { reason: IdleReason },

    /// The policy is in `PendingProviderProvisioning` or its provider
    /// rule needs to be created/updated. Driver calls
    /// `adapter.ensure_rule(policy)` and persists the returned binding.
    EnsureRule,

    /// The policy is in `PendingRetirement`. Driver calls
    /// `adapter.retire_rule(policy)` and transitions the row to
    /// `Retired` once the call succeeds.
    RetireRule,

    /// Safe drift detected — driver re-issues `ensure_rule` to repair.
    /// Examples: prefix label out of sync, rule disabled but should
    /// be enabled, observed lag exceeded RPO but provider can catch up.
    /// Driver records a `drift_repaired` event after success.
    RepairDrift { reason: DriftReason },

    /// Unsafe drift — driver flips `DrHealthState::Drifted` and does
    /// NOT make any provider calls. Pages ops. Examples: destination
    /// bucket/account changed out from under us, unknown provider rule
    /// claiming the same policy, KMS key revocation.
    MarkDrifted { reason: DriftReason },

    /// Provider returned a non-retryable error or precondition failure
    /// (versioning disabled, IAM regression, change-feed off). Driver
    /// sets `DrHealthState::ProviderBlocked`, emits an event, and
    /// pages ops. No further provider calls until a follow-up event
    /// clears the block.
    MarkProviderBlocked { reason: BlockReason },

    /// Provider rule exists or is being provisioned without an active
    /// billing binding. Driver sets `DrHealthState::BillingBlocked`
    /// and refuses provider mutations. Distinct from
    /// `ProviderBlocked` because the resolution path is contractual,
    /// not technical (operator must restore the billing approval).
    MarkBillingBlocked { reason: BlockReason },
}

/// Reason a policy is idle — small enum so metric labels stay bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleReason {
    /// State is `Disabled` — no work until the customer enables.
    Disabled,
    /// State is `PendingBillingApproval` — waiting for the operator
    /// approval API to be called.
    AwaitingBillingApproval,
    /// State is `SuspendedByOps` — emergency suspension, no provider
    /// mutations until ops resumes.
    SuspendedByOps,
    /// State is `Retired` — terminal.
    Retired,
    /// Active and healthy.
    HealthyActive,
}

/// Categorised drift reason. The driver uses this for events and
/// metric labels; the cardinality is finite and stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftReason {
    /// Provider rule does not exist but policy is `Active`.
    RuleMissing,
    /// Provider rule exists but is disabled.
    RuleDisabled,
    /// Provider rule's prefix filter does not match the policy's
    /// `source_prefix`.
    PrefixMismatch,
    /// Provider rule's destination does not match
    /// `destination_bucket_or_account` — unsafe; do not auto-repair.
    DestinationMismatch,
    /// Provider rule ID in the catalog does not match the rule the
    /// provider reports — unknown rule, do not auto-delete.
    UnknownProviderRule,
    /// Source-side versioning / change feed has been turned off.
    SourceVersioningDisabled,
    /// Destination bucket/container is accepting application writes.
    DestinationWritable,
    /// KMS key reference changed on the provider side.
    KmsBindingChanged,
}

/// Why the policy is blocked. Small enum to keep metric label
/// cardinality bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Billing approval is missing or has been revoked.
    BillingApprovalMissing,
    /// Cost owner tenant ID has been cleared on the row.
    CostOwnerMissing,
    /// Source/destination configuration is wrong; provider refuses.
    ProviderMisconfiguration,
    /// Provider account hit a quota limit.
    ProviderQuotaExceeded,
    /// Auth failed at the provider.
    ProviderAuthDenied,
}

/// Pure decision step. Given the catalog's policy intent and the
/// reconciler's last observation, return the next action.
///
/// The function is deterministic — every input maps to exactly one
/// output. No time, no randomness, no I/O. The driver layer wraps it
/// with the per-policy rate limit, lease, and adapter dispatch.
///
/// Drift-detection priority (higher precedence wins):
/// 1. Provider rule ID mismatch → `MarkProviderBlocked` (don't touch
///    unknown rules).
/// 2. Source versioning disabled → `MarkProviderBlocked` (config issue).
/// 3. Destination accepts writes → `MarkProviderBlocked` (data loss
///    risk).
/// 4. Destination mismatch → `MarkDrifted` (unsafe, ops ack needed).
/// 5. KMS binding changed → `MarkDrifted`.
/// 6. Rule missing or disabled (but policy `Active`) → `RepairDrift`.
/// 7. Prefix mismatch → `RepairDrift` (safe — re-issue ensure_rule).
/// 8. Active and healthy → `Idle { HealthyActive }`.
pub fn reconcile_step(
    policy: &CollectionDrPolicy,
    observed: &ProviderObservedState,
) -> ReconcileDecision {
    use DrState::*;

    // Billing gate: any state past PendingBillingApproval that lacks an
    // approval id is blocked. The state machine prevents reaching
    // PendingProviderProvisioning without one at create time, but a
    // mid-flight billing revocation can clear `billing_approval_id`
    // on an Active row.
    let billing_ok = policy.billing.billing_approval_id.is_some()
        && !policy.billing.cost_owner_tenant_id.is_empty();

    match policy.state {
        Disabled => ReconcileDecision::Idle {
            reason: IdleReason::Disabled,
        },

        PendingBillingApproval => ReconcileDecision::Idle {
            reason: IdleReason::AwaitingBillingApproval,
        },

        PendingProviderProvisioning => {
            if !billing_ok {
                return ReconcileDecision::MarkBillingBlocked {
                    reason: if policy.billing.billing_approval_id.is_none() {
                        BlockReason::BillingApprovalMissing
                    } else {
                        BlockReason::CostOwnerMissing
                    },
                };
            }
            // Drive toward Active.
            ReconcileDecision::EnsureRule
        }

        Active => {
            if !billing_ok {
                return ReconcileDecision::MarkBillingBlocked {
                    reason: if policy.billing.billing_approval_id.is_none() {
                        BlockReason::BillingApprovalMissing
                    } else {
                        BlockReason::CostOwnerMissing
                    },
                };
            }
            detect_drift_for_active(policy, observed)
        }

        SuspendedByOps => ReconcileDecision::Idle {
            reason: IdleReason::SuspendedByOps,
        },

        PendingRetirement => ReconcileDecision::RetireRule,

        Retired => ReconcileDecision::Idle {
            reason: IdleReason::Retired,
        },
    }
}

/// Diff policy intent vs observed provider state for an `Active`
/// policy. Encodes the precedence rules listed on `reconcile_step`.
fn detect_drift_for_active(
    policy: &CollectionDrPolicy,
    observed: &ProviderObservedState,
) -> ReconcileDecision {
    // 1. Unknown rule ID in flight: the provider reports a rule whose
    //    ID does not match our catalog binding. Per the contract, do
    //    not delete the unknown rule; flag ProviderBlocked.
    if let (Some(expected), Some(actual)) = (
        policy
            .provider_binding
            .as_ref()
            .map(|b| b.provider_rule_id.as_str()),
        observed.provider_rule_id.as_deref(),
    ) {
        if observed.rule_exists && expected != actual {
            return ReconcileDecision::MarkProviderBlocked {
                reason: BlockReason::ProviderMisconfiguration,
            };
        }
    }

    // 2. Source-side prerequisite drift — provider can't replicate
    //    without versioning/change feed. Operator must repair.
    if observed.rule_exists && !observed.source_versioning_enabled {
        return ReconcileDecision::MarkProviderBlocked {
            reason: BlockReason::ProviderMisconfiguration,
        };
    }

    // 3. Destination accepting writes — direct data-loss risk. Same
    //    treatment: do not touch provider, page ops.
    if observed.rule_exists && !observed.destination_write_protected {
        return ReconcileDecision::MarkProviderBlocked {
            reason: BlockReason::ProviderMisconfiguration,
        };
    }

    // 4. Destination bucket/account changed under us — unsafe drift,
    //    do not auto-repair (could rewrite into wrong account).
    if let Some(obs_dest) = observed.observed_destination.as_deref() {
        if obs_dest != policy.placement.destination_bucket_or_account {
            return ReconcileDecision::MarkDrifted {
                reason: DriftReason::DestinationMismatch,
            };
        }
    }

    // 5. KMS binding moved.
    if let (Some(expected_kms), Some(actual_kms)) = (
        policy
            .provider_binding
            .as_ref()
            .and_then(|b| b.provider_kms_key_id.as_deref()),
        observed.provider_kms_key_id.as_deref(),
    ) {
        if expected_kms != actual_kms {
            return ReconcileDecision::MarkDrifted {
                reason: DriftReason::KmsBindingChanged,
            };
        }
    }

    // 6. Rule missing — Active policy expects an enabled rule.
    if !observed.rule_exists {
        return ReconcileDecision::RepairDrift {
            reason: DriftReason::RuleMissing,
        };
    }

    // 7. Rule disabled — safe to re-issue ensure_rule.
    if !observed.rule_enabled {
        return ReconcileDecision::RepairDrift {
            reason: DriftReason::RuleDisabled,
        };
    }

    // 8. Prefix mismatch — provider filter labels drifted. Safe repair.
    if let Some(obs_prefix) = observed.observed_prefix.as_deref() {
        if obs_prefix != policy.placement.source_prefix {
            return ReconcileDecision::RepairDrift {
                reason: DriftReason::PrefixMismatch,
            };
        }
    }

    // 9. Healthy.
    ReconcileDecision::Idle {
        reason: IdleReason::HealthyActive,
    }
}

/// Decide which `DrHealthState` the row should be set to after the
/// driver applies a decision. Pure helper used by the dispatch layer.
pub fn next_health_state(decision: &ReconcileDecision) -> DrHealthState {
    match decision {
        ReconcileDecision::Idle {
            reason: IdleReason::HealthyActive,
        } => DrHealthState::Healthy,
        ReconcileDecision::Idle { .. } => DrHealthState::Unknown,
        ReconcileDecision::EnsureRule | ReconcileDecision::RetireRule => {
            // Health is updated AFTER the adapter call returns. The
            // mid-flight state is whatever it was before this poll;
            // the driver leaves it alone until success/failure is
            // observed.
            DrHealthState::Unknown
        }
        ReconcileDecision::RepairDrift { .. } => DrHealthState::Drifted,
        ReconcileDecision::MarkDrifted { .. } => DrHealthState::Drifted,
        ReconcileDecision::MarkProviderBlocked { .. } => DrHealthState::ProviderBlocked,
        ReconcileDecision::MarkBillingBlocked { .. } => DrHealthState::BillingBlocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection_dr_policy::{
        CollectionDrPolicy, DrBillingBinding, DrHealth, DrPlacement,
        DrReplicationBehavior, ObjectProvider, ProviderObservedState,
        ProviderReplicationBinding,
    };
    use crate::StoragePoolClass;

    fn base_policy() -> CollectionDrPolicy {
        CollectionDrPolicy {
            policy_id: "drp_1".into(),
            tenant_id: "tnt_acme".into(),
            namespace_id: "ns_1".into(),
            collection_id: "col_orders".into(),
            tier: "business".into(),
            state: DrState::Active,
            provider: ObjectProvider::AwsS3,
            source_region: "us-east-1".into(),
            destination_region: "us-west-2".into(),
            region_pair_id: "aws:us-east-1:us-west-2".into(),
            placement: DrPlacement {
                source_pool_class: StoragePoolClass::Business,
                destination_pool_class: StoragePoolClass::Business,
                source_bucket_or_account: "src-bucket".into(),
                destination_bucket_or_account: "dst-bucket".into(),
                source_container: None,
                destination_container: None,
                source_prefix: "data/tnt_acme/ns_1/col_orders/".into(),
                destination_prefix: "data/tnt_acme/ns_1/col_orders/".into(),
            },
            replication: DrReplicationBehavior::default(),
            billing: DrBillingBinding {
                billing_sku: "collection-dr-business".into(),
                cost_owner_tenant_id: "tnt_acme".into(),
                billing_approval_id: Some("appr_1".into()),
                estimated_monthly_cost_cents: Some(10_000),
            },
            provider_binding: Some(ProviderReplicationBinding {
                provider_policy_id: None,
                provider_rule_id: "dr-drp_1-v1".into(),
                provider_role_arn: None,
                provider_kms_key_id: None,
            }),
            health: DrHealth::default(),
            requested_by: "user".into(),
            approved_by: Some("ops".into()),
            created_at_ns: 0,
            updated_at_ns: 0,
            policy_version: 1,
        }
    }

    fn healthy_observed(policy: &CollectionDrPolicy) -> ProviderObservedState {
        ProviderObservedState {
            rule_exists: true,
            observed_prefix: Some(policy.placement.source_prefix.clone()),
            observed_destination: Some(
                policy.placement.destination_bucket_or_account.clone(),
            ),
            observed_destination_container: policy
                .placement
                .destination_container
                .clone(),
            rule_enabled: true,
            source_versioning_enabled: true,
            destination_write_protected: true,
            observed_lag_seconds: Some(60),
            provider_rule_id: policy
                .provider_binding
                .as_ref()
                .map(|b| b.provider_rule_id.clone()),
            provider_kms_key_id: None,
        }
    }

    // -- Terminal/idle states -------------------------------------------

    #[test]
    fn disabled_policy_is_idle_disabled() {
        let mut p = base_policy();
        p.state = DrState::Disabled;
        assert_eq!(
            reconcile_step(&p, &ProviderObservedState::default()),
            ReconcileDecision::Idle {
                reason: IdleReason::Disabled
            }
        );
    }

    #[test]
    fn retired_policy_is_idle_retired() {
        let mut p = base_policy();
        p.state = DrState::Retired;
        assert_eq!(
            reconcile_step(&p, &ProviderObservedState::default()),
            ReconcileDecision::Idle {
                reason: IdleReason::Retired
            }
        );
    }

    #[test]
    fn suspended_policy_is_idle_suspended() {
        let mut p = base_policy();
        p.state = DrState::SuspendedByOps;
        // Even with healthy observation, suspended state means hands off.
        assert_eq!(
            reconcile_step(&p, &healthy_observed(&p.clone())),
            ReconcileDecision::Idle {
                reason: IdleReason::SuspendedByOps
            }
        );
    }

    #[test]
    fn pending_billing_approval_is_idle_awaiting() {
        let mut p = base_policy();
        p.state = DrState::PendingBillingApproval;
        assert_eq!(
            reconcile_step(&p, &ProviderObservedState::default()),
            ReconcileDecision::Idle {
                reason: IdleReason::AwaitingBillingApproval
            }
        );
    }

    // -- Drive states ---------------------------------------------------

    #[test]
    fn pending_provider_provisioning_drives_ensure_rule() {
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        assert_eq!(
            reconcile_step(&p, &ProviderObservedState::default()),
            ReconcileDecision::EnsureRule
        );
    }

    #[test]
    fn pending_retirement_drives_retire_rule() {
        let mut p = base_policy();
        p.state = DrState::PendingRetirement;
        // Retire works even when the rule still looks healthy from the
        // last observation.
        let obs = healthy_observed(&p.clone());
        assert_eq!(reconcile_step(&p, &obs), ReconcileDecision::RetireRule);
    }

    // -- Billing gate ---------------------------------------------------

    #[test]
    fn provisioning_without_billing_approval_marks_billing_blocked() {
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        p.billing.billing_approval_id = None;
        assert_eq!(
            reconcile_step(&p, &ProviderObservedState::default()),
            ReconcileDecision::MarkBillingBlocked {
                reason: BlockReason::BillingApprovalMissing
            }
        );
    }

    #[test]
    fn active_with_revoked_billing_approval_marks_billing_blocked() {
        let mut p = base_policy();
        p.billing.billing_approval_id = None;
        let obs = healthy_observed(&p.clone());
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkBillingBlocked {
                reason: BlockReason::BillingApprovalMissing
            }
        );
    }

    #[test]
    fn active_with_empty_cost_owner_marks_billing_blocked_cost_owner() {
        let mut p = base_policy();
        p.billing.cost_owner_tenant_id = String::new();
        let obs = healthy_observed(&p.clone());
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkBillingBlocked {
                reason: BlockReason::CostOwnerMissing
            }
        );
    }

    // -- Active happy path ---------------------------------------------

    #[test]
    fn active_with_healthy_observation_is_idle_healthy() {
        let p = base_policy();
        let obs = healthy_observed(&p);
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::Idle {
                reason: IdleReason::HealthyActive
            }
        );
    }

    // -- Drift: repairable ---------------------------------------------

    #[test]
    fn active_with_missing_rule_repairs_via_ensure() {
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.rule_exists = false;
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::RepairDrift {
                reason: DriftReason::RuleMissing
            }
        );
    }

    #[test]
    fn active_with_disabled_rule_repairs() {
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.rule_enabled = false;
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::RepairDrift {
                reason: DriftReason::RuleDisabled
            }
        );
    }

    #[test]
    fn active_with_prefix_drift_repairs() {
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.observed_prefix = Some("data/wrong/prefix/".into());
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::RepairDrift {
                reason: DriftReason::PrefixMismatch
            }
        );
    }

    // -- Drift: unsafe → MarkDrifted -----------------------------------

    #[test]
    fn active_with_destination_drift_marks_drifted_not_repair() {
        // Destination mismatch must NOT trigger an auto-repair — the
        // ensure_rule call would rewrite into the wrong account.
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.observed_destination = Some("hostile-bucket".into());
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkDrifted {
                reason: DriftReason::DestinationMismatch
            }
        );
    }

    #[test]
    fn active_with_kms_change_marks_drifted() {
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: Some("arn:aws:kms:.../expected".into()),
        });
        let mut obs = healthy_observed(&p.clone());
        obs.provider_kms_key_id = Some("arn:aws:kms:.../surprise".into());
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkDrifted {
                reason: DriftReason::KmsBindingChanged
            }
        );
    }

    // -- Drift: ProviderBlocked ----------------------------------------

    #[test]
    fn active_with_unknown_provider_rule_marks_provider_blocked() {
        // Provider reports a rule whose ID doesn't match our catalog
        // binding. Per the contract, do NOT auto-delete; block.
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.provider_rule_id = Some("dr-someone-else-v9".into());
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkProviderBlocked {
                reason: BlockReason::ProviderMisconfiguration
            }
        );
    }

    #[test]
    fn active_with_versioning_disabled_marks_provider_blocked() {
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.source_versioning_enabled = false;
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkProviderBlocked {
                reason: BlockReason::ProviderMisconfiguration
            }
        );
    }

    #[test]
    fn active_with_writable_destination_marks_provider_blocked() {
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.destination_write_protected = false;
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkProviderBlocked {
                reason: BlockReason::ProviderMisconfiguration
            }
        );
    }

    // -- Precedence ----------------------------------------------------

    #[test]
    fn provider_blocked_takes_precedence_over_repairable_drift() {
        // Two faults at once: prefix drift (repairable) AND versioning
        // disabled (block). The block wins because attempting to
        // repair would still hit the configuration error.
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.observed_prefix = Some("data/wrong/prefix/".into());
        obs.source_versioning_enabled = false;
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkProviderBlocked {
                reason: BlockReason::ProviderMisconfiguration
            }
        );
    }

    #[test]
    fn destination_mismatch_takes_precedence_over_prefix_repair() {
        // Destination mismatch is unsafe, prefix mismatch is safe-
        // repairable. We must not call ensure_rule against a hostile
        // destination just because the prefix label drifted too.
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.observed_prefix = Some("data/wrong/prefix/".into());
        obs.observed_destination = Some("hostile-bucket".into());
        assert_eq!(
            reconcile_step(&p, &obs),
            ReconcileDecision::MarkDrifted {
                reason: DriftReason::DestinationMismatch
            }
        );
    }

    // -- next_health_state ---------------------------------------------

    #[test]
    fn next_health_state_maps_decisions_correctly() {
        let cases = [
            (
                ReconcileDecision::Idle {
                    reason: IdleReason::HealthyActive,
                },
                DrHealthState::Healthy,
            ),
            (
                ReconcileDecision::Idle {
                    reason: IdleReason::Disabled,
                },
                DrHealthState::Unknown,
            ),
            (ReconcileDecision::EnsureRule, DrHealthState::Unknown),
            (ReconcileDecision::RetireRule, DrHealthState::Unknown),
            (
                ReconcileDecision::RepairDrift {
                    reason: DriftReason::RuleMissing,
                },
                DrHealthState::Drifted,
            ),
            (
                ReconcileDecision::MarkDrifted {
                    reason: DriftReason::DestinationMismatch,
                },
                DrHealthState::Drifted,
            ),
            (
                ReconcileDecision::MarkProviderBlocked {
                    reason: BlockReason::ProviderMisconfiguration,
                },
                DrHealthState::ProviderBlocked,
            ),
            (
                ReconcileDecision::MarkBillingBlocked {
                    reason: BlockReason::BillingApprovalMissing,
                },
                DrHealthState::BillingBlocked,
            ),
        ];
        for (decision, expected) in cases {
            assert_eq!(
                next_health_state(&decision),
                expected,
                "{decision:?} → {expected:?}"
            );
        }
    }
}
