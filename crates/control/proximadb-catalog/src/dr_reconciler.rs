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
    CollectionDrEvent, CollectionDrPolicy, DrEventType, DrHealth, DrHealthState,
    DrProviderAdapter, DrState, ProviderError, ProviderObservedState,
    ProviderReplicationBinding,
};
use async_trait::async_trait;
use std::sync::Arc;

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

// ---------------------------------------------------------------------------
// Driver (P3b) — async dispatch layer
// ---------------------------------------------------------------------------

/// Errors surfaced by the engine API surface and the reconciler
/// dispatch layer. Per the contract §"Engine API Surface" / S14.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DrApiError {
    /// Caller passed an invalid input (unknown policy_id, missing
    /// required field, malformed value).
    #[error("validation failed: {0}")]
    ValidationFailed(String),

    /// Caller lacks the operator service-account capability required
    /// for this mutation. Customer-facing surfaces wrap this as 401/403.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// The requested state transition is not permitted by the state
    /// machine (e.g. `Active -> Disabled`).
    #[error("invalid state transition: {from:?} -> {to:?}")]
    InvalidStateTransition { from: DrState, to: DrState },

    /// The owning collection's authority mode is `ExternalAuthoritative`
    /// (Iceberg / Delta / Hudi); per D8 the engine refuses DR for those.
    #[error("external-authoritative collection refused: {0}")]
    ExternalAuthoritativeRefused(String),

    /// `ObjectProvider` variant has no adapter implementation in this
    /// build (per S13, `GcsFuture` is reserved-not-implemented).
    #[error("unsupported provider: {0}")]
    UnsupportedProvider(String),

    /// xCatalog store could not be reached. Retryable.
    #[error("store unavailable: {0}")]
    StoreUnavailable(String),

    /// Optimistic concurrency: caller's `expected_version` did not match
    /// the current `policy_version`. Caller should reload and retry.
    #[error(
        "policy version conflict for {policy_id}: \
         expected {expected}, got {actual}"
    )]
    VersionConflict {
        policy_id: String,
        expected: u64,
        actual: u64,
    },
}

/// Narrow store surface the reconciler needs to drive a policy through
/// one tick. Implementations are operator-owned (sqlx/filestore); the
/// `MockDrPolicyStore` ships here for tests.
#[async_trait]
pub trait DrPolicyStore: Send + Sync {
    /// Persist a state transition. Returns the new `policy_version`.
    /// Bumps `policy_version` because the contract S2 rule says state
    /// transitions always bump.
    async fn transition_state(
        &self,
        policy_id: &str,
        next: DrState,
        expected_version: u64,
    ) -> Result<u64, DrApiError>;

    /// Persist a provider binding after a successful `ensure_rule`.
    /// Bumps `policy_version` per S2 (binding change is a
    /// provider-rule-touching change).
    async fn set_provider_binding(
        &self,
        policy_id: &str,
        binding: ProviderReplicationBinding,
        expected_version: u64,
    ) -> Result<u64, DrApiError>;

    /// Update the row's health. Health updates do NOT bump
    /// `policy_version` — they are reconciler observations only.
    async fn update_health(
        &self,
        policy_id: &str,
        health: DrHealth,
    ) -> Result<(), DrApiError>;

    /// Append a row to `xcatalog_collection_dr_events`. Append-only;
    /// the caller (driver) generates the `event_id`.
    async fn record_event(&self, event: CollectionDrEvent) -> Result<(), DrApiError>;
}

/// One-shot outcome of `DrReconcilerDriver::reconcile_one`. The async
/// loop layer aggregates these and feeds them to metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// No action taken.
    Idle(IdleReason),
    /// Provider rule was successfully created/updated and the binding
    /// was persisted. Carries the new `policy_version`.
    EnsuredRule { policy_version: u64 },
    /// Provider rule retired and the row transitioned to `Retired`.
    Retired,
    /// Safe drift was repaired via `ensure_rule`.
    RepairedDrift(DriftReason),
    /// Unsafe drift recorded; health flipped to `Drifted`.
    MarkedDrifted(DriftReason),
    /// Provider blocked; health flipped, ops paged.
    MarkedProviderBlocked(BlockReason),
    /// Billing binding missing/revoked; health flipped.
    MarkedBillingBlocked(BlockReason),
    /// Adapter call failed transiently. Driver does NOT escalate;
    /// caller's retry logic decides next.
    AdapterTransient(String),
    /// Adapter call failed with a non-retryable error. Driver flipped
    /// health to `ProviderBlocked`.
    AdapterEscalated(BlockReason),
}

/// The dispatch layer that turns one [`ReconcileDecision`] into
/// adapter + store calls. Generic over store + adapter so tests can
/// inject mocks.
///
/// One driver per shard; the caller handles iteration and scheduling
/// (P3c). This struct is `Clone`-able through `Arc` so multiple loops
/// can share one configuration.
pub struct DrReconcilerDriver<S, A> {
    store: Arc<S>,
    adapter: Arc<A>,
    /// Actor identity recorded on every emitted event.
    actor: String,
    /// Source of monotonically-increasing event IDs. Tests supply a
    /// deterministic counter; production wires a ULID generator.
    event_id_source: Arc<dyn Fn() -> String + Send + Sync>,
    /// Wall-clock source for `created_at_ns`. Pluggable for tests.
    now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl<S, A> DrReconcilerDriver<S, A>
where
    S: DrPolicyStore + 'static,
    A: DrProviderAdapter + 'static,
{
    /// Build a driver with the given store, adapter, and actor label.
    /// Uses ULID-ish event IDs from the system clock; tests should
    /// prefer [`with_clocks`].
    pub fn new(store: Arc<S>, adapter: Arc<A>, actor: impl Into<String>) -> Self {
        let now_ns: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0)
        });
        let counter = std::sync::atomic::AtomicU64::new(0);
        let counter = Arc::new(counter);
        let event_id_source: Arc<dyn Fn() -> String + Send + Sync> = Arc::new(move || {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("evt_{n:020}")
        });
        Self {
            store,
            adapter,
            actor: actor.into(),
            event_id_source,
            now_ns,
        }
    }

    /// Build a driver with explicit clock + event-id source. Tests use
    /// this to make outcomes deterministic.
    pub fn with_clocks(
        store: Arc<S>,
        adapter: Arc<A>,
        actor: impl Into<String>,
        event_id_source: Arc<dyn Fn() -> String + Send + Sync>,
        now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            store,
            adapter,
            actor: actor.into(),
            event_id_source,
            now_ns,
        }
    }

    /// Drive one policy through one tick: fetch state, decide, dispatch,
    /// persist health, emit event. Returns the outcome for metric
    /// aggregation.
    ///
    /// This method is `&self` so a single driver can be shared across
    /// concurrent policy ticks (the underlying adapter/store traits
    /// are `Send + Sync`).
    pub async fn reconcile_one(
        &self,
        policy: &CollectionDrPolicy,
    ) -> Result<ReconcileOutcome, DrApiError> {
        // 1. Fetch observed state. A transient adapter failure here
        //    returns AdapterTransient — caller decides retry cadence.
        let observed = match self.adapter.fetch_state(policy).await {
            Ok(o) => o,
            Err(e) => return self.handle_adapter_error(policy, e, "fetch_state").await,
        };

        // 2. Decide.
        let decision = reconcile_step(policy, &observed);

        // 3. Dispatch.
        self.dispatch(policy, decision).await
    }

    async fn dispatch(
        &self,
        policy: &CollectionDrPolicy,
        decision: ReconcileDecision,
    ) -> Result<ReconcileOutcome, DrApiError> {
        match decision {
            ReconcileDecision::Idle { reason } => {
                // Heartbeat-only — record observation but no event spam
                // for idle ticks beyond first.
                let mut health = policy.health.clone();
                health.state = next_health_state(&ReconcileDecision::Idle { reason });
                health.last_reconciled_at_ns = Some((self.now_ns)());
                self.store.update_health(&policy.policy_id, health).await?;
                Ok(ReconcileOutcome::Idle(reason))
            }

            ReconcileDecision::EnsureRule => self.do_ensure(policy, None).await,

            ReconcileDecision::RetireRule => self.do_retire(policy).await,

            ReconcileDecision::RepairDrift { reason } => {
                self.emit_event(policy, DrEventType::DriftDetected, Some(format!("{reason:?}")))
                    .await?;
                let outcome = self.do_ensure(policy, Some(reason)).await?;
                // Successful repair → emit drift_repaired.
                if matches!(outcome, ReconcileOutcome::RepairedDrift(_)) {
                    self.emit_event(
                        policy,
                        DrEventType::DriftRepaired,
                        Some(format!("{reason:?}")),
                    )
                    .await?;
                }
                Ok(outcome)
            }

            ReconcileDecision::MarkDrifted { reason } => {
                self.set_health(
                    policy,
                    DrHealthState::Drifted,
                    Some(format!("{reason:?}")),
                )
                .await?;
                self.emit_event(policy, DrEventType::DriftDetected, Some(format!("{reason:?}")))
                    .await?;
                Ok(ReconcileOutcome::MarkedDrifted(reason))
            }

            ReconcileDecision::MarkProviderBlocked { reason } => {
                self.set_health(
                    policy,
                    DrHealthState::ProviderBlocked,
                    Some(format!("{reason:?}")),
                )
                .await?;
                Ok(ReconcileOutcome::MarkedProviderBlocked(reason))
            }

            ReconcileDecision::MarkBillingBlocked { reason } => {
                self.set_health(
                    policy,
                    DrHealthState::BillingBlocked,
                    Some(format!("{reason:?}")),
                )
                .await?;
                self.emit_event(
                    policy,
                    DrEventType::BillingBlocked,
                    Some(format!("{reason:?}")),
                )
                .await?;
                Ok(ReconcileOutcome::MarkedBillingBlocked(reason))
            }
        }
    }

    async fn do_ensure(
        &self,
        policy: &CollectionDrPolicy,
        drift_reason: Option<DriftReason>,
    ) -> Result<ReconcileOutcome, DrApiError> {
        match self.adapter.ensure_rule(policy).await {
            Ok(binding) => {
                let new_version = self
                    .store
                    .set_provider_binding(
                        &policy.policy_id,
                        binding,
                        policy.policy_version,
                    )
                    .await?;
                // Drive to Active if we were in PendingProviderProvisioning.
                let next_state = if policy.state == DrState::PendingProviderProvisioning
                {
                    Some(DrState::Active)
                } else {
                    None
                };
                if let Some(next) = next_state {
                    self.store
                        .transition_state(&policy.policy_id, next, new_version)
                        .await?;
                    self.emit_event(policy, DrEventType::Active, None).await?;
                }
                self.set_health(policy, DrHealthState::Healthy, None).await?;
                Ok(match drift_reason {
                    Some(r) => ReconcileOutcome::RepairedDrift(r),
                    None => ReconcileOutcome::EnsuredRule {
                        policy_version: new_version,
                    },
                })
            }
            Err(e) => self.handle_adapter_error(policy, e, "ensure_rule").await,
        }
    }

    async fn do_retire(
        &self,
        policy: &CollectionDrPolicy,
    ) -> Result<ReconcileOutcome, DrApiError> {
        match self.adapter.retire_rule(policy).await {
            Ok(()) => {
                self.store
                    .transition_state(
                        &policy.policy_id,
                        DrState::Retired,
                        policy.policy_version,
                    )
                    .await?;
                self.emit_event(policy, DrEventType::ProviderRuleDisabled, None)
                    .await?;
                self.emit_event(policy, DrEventType::Retired, None).await?;
                Ok(ReconcileOutcome::Retired)
            }
            Err(e) => self.handle_adapter_error(policy, e, "retire_rule").await,
        }
    }

    async fn handle_adapter_error(
        &self,
        policy: &CollectionDrPolicy,
        err: ProviderError,
        op: &'static str,
    ) -> Result<ReconcileOutcome, DrApiError> {
        if err.is_retryable() {
            // Transient: do not flip health, do not emit a blocking
            // event. The async loop's backoff layer (P3c) will retry.
            return Ok(ReconcileOutcome::AdapterTransient(format!(
                "{op}: {err}"
            )));
        }
        let reason = match err {
            ProviderError::Misconfiguration(_) => BlockReason::ProviderMisconfiguration,
            ProviderError::QuotaExceeded(_) => BlockReason::ProviderQuotaExceeded,
            ProviderError::AuthDenied(_) => BlockReason::ProviderAuthDenied,
            // Transient already handled above; the catch-all keeps the
            // match exhaustive without a panic path.
            ProviderError::Transient(_) => BlockReason::ProviderMisconfiguration,
        };
        self.set_health(
            policy,
            DrHealthState::ProviderBlocked,
            Some(format!("{op}: {reason:?}")),
        )
        .await?;
        Ok(ReconcileOutcome::AdapterEscalated(reason))
    }

    async fn set_health(
        &self,
        policy: &CollectionDrPolicy,
        state: DrHealthState,
        reason: Option<String>,
    ) -> Result<(), DrApiError> {
        let mut h = policy.health.clone();
        h.state = state;
        h.reason = reason;
        h.last_reconciled_at_ns = Some((self.now_ns)());
        self.store.update_health(&policy.policy_id, h).await
    }

    async fn emit_event(
        &self,
        policy: &CollectionDrPolicy,
        event_type: DrEventType,
        reason: Option<String>,
    ) -> Result<(), DrApiError> {
        let event = CollectionDrEvent {
            event_id: (self.event_id_source)(),
            policy_id: policy.policy_id.clone(),
            tenant_id: policy.tenant_id.clone(),
            collection_id: policy.collection_id.clone(),
            event_type,
            actor: self.actor.clone(),
            reason,
            before_state: Some(policy.state),
            after_state: Some(policy.state),
            provider_state: None,
            created_at_ns: (self.now_ns)(),
        };
        self.store.record_event(event).await
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

    // --- Driver (P3b) -------------------------------------------------

    use crate::collection_dr_policy::MockDrProviderAdapter;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    /// In-memory store for driver tests. Records every store mutation
    /// so assertions can pin the order and contents.
    #[derive(Default)]
    struct MockDrPolicyStore {
        events: Mutex<Vec<CollectionDrEvent>>,
        health_updates: Mutex<Vec<(String, DrHealth)>>,
        state_transitions: Mutex<Vec<(String, DrState, u64)>>,
        bindings: Mutex<Vec<(String, ProviderReplicationBinding, u64)>>,
        next_version: AtomicU64,
        inject_error: Mutex<Option<DrApiError>>,
    }

    impl MockDrPolicyStore {
        fn new(starting_version: u64) -> Arc<Self> {
            let s = Self::default();
            s.next_version.store(starting_version, Ordering::Relaxed);
            Arc::new(s)
        }

        fn events_snapshot(&self) -> Vec<CollectionDrEvent> {
            self.events.lock().clone()
        }
        fn health_snapshot(&self) -> Vec<(String, DrHealth)> {
            self.health_updates.lock().clone()
        }
        fn transitions_snapshot(&self) -> Vec<(String, DrState, u64)> {
            self.state_transitions.lock().clone()
        }
        fn bindings_snapshot(&self) -> Vec<(String, ProviderReplicationBinding, u64)> {
            self.bindings.lock().clone()
        }
    }

    #[async_trait]
    impl DrPolicyStore for MockDrPolicyStore {
        async fn transition_state(
            &self,
            policy_id: &str,
            next: DrState,
            expected_version: u64,
        ) -> Result<u64, DrApiError> {
            if let Some(e) = self.inject_error.lock().take() {
                return Err(e);
            }
            self.state_transitions
                .lock()
                .push((policy_id.into(), next, expected_version));
            Ok(self.next_version.fetch_add(1, Ordering::Relaxed) + 1)
        }

        async fn set_provider_binding(
            &self,
            policy_id: &str,
            binding: ProviderReplicationBinding,
            expected_version: u64,
        ) -> Result<u64, DrApiError> {
            if let Some(e) = self.inject_error.lock().take() {
                return Err(e);
            }
            self.bindings
                .lock()
                .push((policy_id.into(), binding, expected_version));
            Ok(self.next_version.fetch_add(1, Ordering::Relaxed) + 1)
        }

        async fn update_health(
            &self,
            policy_id: &str,
            health: DrHealth,
        ) -> Result<(), DrApiError> {
            if let Some(e) = self.inject_error.lock().take() {
                return Err(e);
            }
            self.health_updates
                .lock()
                .push((policy_id.into(), health));
            Ok(())
        }

        async fn record_event(&self, event: CollectionDrEvent) -> Result<(), DrApiError> {
            if let Some(e) = self.inject_error.lock().take() {
                return Err(e);
            }
            self.events.lock().push(event);
            Ok(())
        }
    }

    fn make_driver(
        store: Arc<MockDrPolicyStore>,
        adapter: Arc<MockDrProviderAdapter>,
    ) -> DrReconcilerDriver<MockDrPolicyStore, MockDrProviderAdapter> {
        // Deterministic clocks for assertable outcomes.
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        let event_id: Arc<dyn Fn() -> String + Send + Sync> = Arc::new(move || {
            let n = counter_clone.fetch_add(1, Ordering::Relaxed);
            format!("evt_{n:04}")
        });
        let clock_counter = Arc::new(AtomicI64::new(1_700_000_000_000_000_000));
        let clock_clone = clock_counter.clone();
        let now_ns: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(move || {
            clock_clone.fetch_add(1_000, Ordering::Relaxed)
        });
        DrReconcilerDriver::with_clocks(store, adapter, "reconciler", event_id, now_ns)
    }

    #[tokio::test]
    async fn driver_active_healthy_updates_health_and_emits_no_event() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        // Pre-seed adapter observation to "healthy" for the policy.
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        let healthy = healthy_observed(&p);
        adapter.seed_observed(&p.policy_id, healthy);
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert_eq!(outcome, ReconcileOutcome::Idle(IdleReason::HealthyActive));

        // Health update recorded, no events fired.
        let h = store.health_snapshot();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].1.state, DrHealthState::Healthy);
        assert!(h[0].1.last_reconciled_at_ns.is_some());
        assert!(store.events_snapshot().is_empty());
    }

    #[tokio::test]
    async fn driver_pending_provisioning_drives_to_active_via_ensure() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        p.provider_binding = None;
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert!(matches!(
            outcome,
            ReconcileOutcome::EnsuredRule { policy_version: _ }
        ));

        // Adapter ensure_rule was called.
        assert_eq!(adapter.ensure_call_count(), 1);
        // Store recorded a binding write AND a transition to Active.
        assert_eq!(store.bindings_snapshot().len(), 1);
        let transitions = store.transitions_snapshot();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].1, DrState::Active);
        // An `active` event was emitted.
        let events = store.events_snapshot();
        assert!(events.iter().any(|e| e.event_type == DrEventType::Active));
        // Health was set to Healthy.
        let h = store.health_snapshot();
        assert!(h.iter().any(|(_, h)| h.state == DrHealthState::Healthy));
    }

    #[tokio::test]
    async fn driver_repair_drift_emits_detected_then_repaired() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let p = base_policy();
        // Seed adapter to report a missing rule (drift).
        let mut obs = healthy_observed(&p);
        obs.rule_exists = false;
        adapter.seed_observed(&p.policy_id, obs);
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert_eq!(
            outcome,
            ReconcileOutcome::RepairedDrift(DriftReason::RuleMissing)
        );

        // Both drift_detected and drift_repaired events fired, in order.
        let events = store.events_snapshot();
        let drift_types: Vec<_> = events.iter().map(|e| e.event_type).collect();
        let detected_idx = drift_types
            .iter()
            .position(|t| *t == DrEventType::DriftDetected)
            .expect("drift_detected fired");
        let repaired_idx = drift_types
            .iter()
            .position(|t| *t == DrEventType::DriftRepaired)
            .expect("drift_repaired fired");
        assert!(
            detected_idx < repaired_idx,
            "detected must precede repaired"
        );
        assert_eq!(adapter.ensure_call_count(), 1);
    }

    #[tokio::test]
    async fn driver_mark_drifted_does_not_call_adapter_ensure() {
        // Destination mismatch is unsafe drift — must NOT trigger
        // ensure_rule (which could overwrite into the hostile dest).
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.observed_destination = Some("hostile-bucket".into());
        adapter.seed_observed(&p.policy_id, obs);
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert_eq!(
            outcome,
            ReconcileOutcome::MarkedDrifted(DriftReason::DestinationMismatch)
        );
        assert_eq!(adapter.ensure_call_count(), 0);
        assert_eq!(adapter.retire_call_count(), 0);

        // Health flipped to Drifted.
        let h = store.health_snapshot();
        assert!(h.iter().any(|(_, h)| h.state == DrHealthState::Drifted));
        // drift_detected event recorded.
        let events = store.events_snapshot();
        assert!(events
            .iter()
            .any(|e| e.event_type == DrEventType::DriftDetected));
    }

    #[tokio::test]
    async fn driver_provider_blocked_skips_provider_calls() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let p = base_policy();
        let mut obs = healthy_observed(&p);
        obs.source_versioning_enabled = false;
        adapter.seed_observed(&p.policy_id, obs);
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert_eq!(
            outcome,
            ReconcileOutcome::MarkedProviderBlocked(
                BlockReason::ProviderMisconfiguration
            )
        );
        assert_eq!(adapter.ensure_call_count(), 0);
        let h = store.health_snapshot();
        assert!(h
            .iter()
            .any(|(_, h)| h.state == DrHealthState::ProviderBlocked));
    }

    #[tokio::test]
    async fn driver_pending_retirement_calls_retire_and_emits_two_events() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::PendingRetirement;
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert_eq!(outcome, ReconcileOutcome::Retired);

        assert_eq!(adapter.retire_call_count(), 1);
        let transitions = store.transitions_snapshot();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].1, DrState::Retired);
        let event_types: Vec<_> =
            store.events_snapshot().iter().map(|e| e.event_type).collect();
        assert!(event_types.contains(&DrEventType::ProviderRuleDisabled));
        assert!(event_types.contains(&DrEventType::Retired));
    }

    #[tokio::test]
    async fn driver_billing_revoked_marks_billing_blocked_with_event() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.billing.billing_approval_id = None;
        // Observation doesn't matter — billing gate fires first.
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert_eq!(
            outcome,
            ReconcileOutcome::MarkedBillingBlocked(BlockReason::BillingApprovalMissing)
        );
        assert_eq!(adapter.ensure_call_count(), 0);
        let event_types: Vec<_> =
            store.events_snapshot().iter().map(|e| e.event_type).collect();
        assert!(event_types.contains(&DrEventType::BillingBlocked));
    }

    #[tokio::test]
    async fn driver_transient_adapter_failure_does_not_flip_health() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        adapter.inject_error(ProviderError::Transient("blip".into()));
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert!(matches!(outcome, ReconcileOutcome::AdapterTransient(_)));
        // No health flip — the retry layer will try again.
        assert!(store.health_snapshot().is_empty());
        assert!(store.events_snapshot().is_empty());
    }

    #[tokio::test]
    async fn driver_auth_denied_escalates_to_provider_blocked() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        // First call is fetch_state; second is ensure_rule. Inject the
        // auth error on fetch_state since it runs first.
        adapter.inject_error(ProviderError::AuthDenied("403".into()));
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert_eq!(
            outcome,
            ReconcileOutcome::AdapterEscalated(BlockReason::ProviderAuthDenied)
        );
        let h = store.health_snapshot();
        assert!(h
            .iter()
            .any(|(_, h)| h.state == DrHealthState::ProviderBlocked));
    }

    #[tokio::test]
    async fn driver_disabled_policy_records_heartbeat_only() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::Disabled;
        let driver = make_driver(store.clone(), adapter.clone());

        let outcome = driver.reconcile_one(&p).await.unwrap();
        assert_eq!(outcome, ReconcileOutcome::Idle(IdleReason::Disabled));
        // Heartbeat only — no events, no adapter calls.
        assert_eq!(adapter.ensure_call_count(), 0);
        assert!(store.events_snapshot().is_empty());
        // BUT fetch_state still runs (read-only, cheap) and health is
        // updated for the heartbeat.
        assert_eq!(adapter.fetch_call_count(), 1);
        assert_eq!(store.health_snapshot().len(), 1);
    }
}
