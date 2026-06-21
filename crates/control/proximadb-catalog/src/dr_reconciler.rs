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
    CollectionDrEvent, CollectionDrPolicy, DrEventType, DrHealth, DrHealthState, DrProviderAdapter,
    DrState, ObjectProvider, ProviderError, ProviderObservedState, ProviderReplicationBinding,
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
    ) && observed.rule_exists
        && expected != actual
    {
        return ReconcileDecision::MarkProviderBlocked {
            reason: BlockReason::ProviderMisconfiguration,
        };
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
    if let Some(obs_dest) = observed.observed_destination.as_deref()
        && obs_dest != policy.placement.destination_bucket_or_account
    {
        return ReconcileDecision::MarkDrifted {
            reason: DriftReason::DestinationMismatch,
        };
    }

    // 5. KMS binding moved.
    if let (Some(expected_kms), Some(actual_kms)) = (
        policy
            .provider_binding
            .as_ref()
            .and_then(|b| b.provider_kms_key_id.as_deref()),
        observed.provider_kms_key_id.as_deref(),
    ) && expected_kms != actual_kms
    {
        return ReconcileDecision::MarkDrifted {
            reason: DriftReason::KmsBindingChanged,
        };
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
    if let Some(obs_prefix) = observed.observed_prefix.as_deref()
        && obs_prefix != policy.placement.source_prefix
    {
        return ReconcileDecision::RepairDrift {
            reason: DriftReason::PrefixMismatch,
        };
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
// Backoff, rate limit, shard pause (P3c1) — pure state machinery
// ---------------------------------------------------------------------------

/// Configurable backoff knobs. Defaults mirror the contract spec
/// (30s → 30m, jitter 25%, 12 attempts before escalation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackoffPolicy {
    /// First retry delay after the initial failure, in seconds.
    pub initial_delay_secs: u32,
    /// Hard ceiling on the per-attempt delay, in seconds. The
    /// exponential growth caps at this value.
    pub max_delay_secs: u32,
    /// Jitter fraction in `[0.0, 1.0)`. Each attempt's delay is
    /// `base * uniform(1.0 - jitter, 1.0 + jitter)`.
    pub jitter_factor: f64,
    /// Number of consecutive transient failures before the entry
    /// escalates to `ProviderBlocked`.
    pub max_attempts: u32,
    /// Per-policy minimum interval between provider calls, in
    /// seconds. Bound to keep one well-behaved policy from
    /// monopolising a shard's outbound rate.
    pub min_call_interval_secs: u32,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        // Contract values — keep in sync with §"Scheduling And
        // Backpressure" in COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc.
        Self {
            initial_delay_secs: 30,
            max_delay_secs: 30 * 60,
            jitter_factor: 0.25,
            max_attempts: 12,
            min_call_interval_secs: 5,
        }
    }
}

/// Per-policy retry state held by the shard loop. The pure functions
/// below mutate it without any async or real time.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BackoffEntry {
    /// Consecutive transient-failure count. 0 means healthy.
    pub attempt: u32,
    /// Earliest nanosecond timestamp the policy may be retried at. 0
    /// means "ready now".
    pub earliest_retry_ns: i64,
}

/// Outcome category the backoff layer reacts to. Distinct from
/// [`ReconcileOutcome`] because backoff doesn't care about
/// drift/idle vs success — it only cares about retry vs success vs
/// escalate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffSignal {
    /// Adapter call returned a transient error — schedule another try.
    TransientFailure,
    /// Adapter call succeeded — reset to healthy.
    Success,
    /// Adapter call returned a non-retryable error — clear backoff and
    /// let the driver flip health to `ProviderBlocked`.
    Escalate,
}

/// Compute the next earliest-retry time and updated attempt count
/// after a signal. Pure: caller supplies `now_ns` and a
/// `jitter_uniform_01` value in `[0.0, 1.0)` (production wires a
/// real RNG; tests inject `0.5` for the un-jittered midpoint).
///
/// Returns `Some(new_entry)` for `TransientFailure` (capped at
/// `max_attempts`; the caller is responsible for treating
/// `attempt >= max_attempts` as escalation).
///
/// Returns `None` for `Success` (caller deletes the entry) and for
/// `Escalate` (caller drops the entry — escalation is handled by the
/// driver, not by suppressing retries).
pub fn apply_backoff_signal(
    policy: BackoffPolicy,
    current: BackoffEntry,
    signal: BackoffSignal,
    now_ns: i64,
    jitter_uniform_01: f64,
) -> Option<BackoffEntry> {
    match signal {
        BackoffSignal::Success | BackoffSignal::Escalate => None,
        BackoffSignal::TransientFailure => {
            let next_attempt = current.attempt.saturating_add(1);
            let delay_ns = jittered_delay_ns(policy, next_attempt, jitter_uniform_01);
            Some(BackoffEntry {
                attempt: next_attempt,
                earliest_retry_ns: now_ns.saturating_add(delay_ns),
            })
        }
    }
}

/// True if the entry's escalation threshold has been reached. Caller
/// flips the policy to `ProviderBlocked` and clears the entry.
pub fn should_escalate(policy: BackoffPolicy, entry: &BackoffEntry) -> bool {
    entry.attempt >= policy.max_attempts
}

/// Is the policy ready to be polled at `now_ns`?
pub fn is_ready(entry: &BackoffEntry, now_ns: i64) -> bool {
    entry.earliest_retry_ns <= now_ns
}

/// Compute the jittered delay for `attempt` (1-indexed). Pure helper
/// — exponential base * 2^(attempt-1), capped at `max_delay_secs`,
/// then multiplied by `uniform(1 - jitter, 1 + jitter)`.
fn jittered_delay_ns(policy: BackoffPolicy, attempt: u32, jitter_uniform_01: f64) -> i64 {
    // Cap the shift to avoid overflow on absurd attempt counts.
    let shift = attempt.saturating_sub(1).min(30);
    let base_secs = (policy.initial_delay_secs as u64)
        .saturating_mul(1u64 << shift)
        .min(policy.max_delay_secs as u64) as f64;
    let jitter = policy.jitter_factor.clamp(0.0, 1.0);
    let scale = 1.0 - jitter + 2.0 * jitter * jitter_uniform_01.clamp(0.0, 1.0);
    let secs = base_secs * scale;
    (secs * 1_000_000_000.0) as i64
}

/// Per-policy rate limiter. The shard loop uses this to enforce the
/// contract's "one provider call per 5 seconds per policy" floor.
///
/// Pure — no clock, caller supplies `now_ns`. Tracks the last call
/// timestamp; `try_acquire` returns whether the policy may issue a
/// provider call now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PerPolicyRateLimit {
    /// Last successful `try_acquire` timestamp. `None` until the
    /// first call — using `Option` avoids the sentinel-vs-actual-zero
    /// ambiguity that a bare `i64 = 0` would create at the epoch.
    pub last_call_ns: Option<i64>,
}

impl PerPolicyRateLimit {
    /// Attempt to reserve a provider call slot at `now_ns`. Returns
    /// true if the call is permitted (and updates `last_call_ns`),
    /// false if the caller should defer until later.
    pub fn try_acquire(&mut self, policy: BackoffPolicy, now_ns: i64) -> bool {
        let min_interval_ns = (policy.min_call_interval_secs as i64).saturating_mul(1_000_000_000);
        let allowed = match self.last_call_ns {
            None => true,
            Some(last) => now_ns.saturating_sub(last) >= min_interval_ns,
        };
        if allowed {
            self.last_call_ns = Some(now_ns);
        }
        allowed
    }
}

/// Why the shard is paused. The async loop reads this and skips its
/// provider-mutation pass while the pause is active. Read-only
/// `fetch_state` calls are still permitted per the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardPauseReason {
    /// `ProviderError::QuotaExceeded` was observed on the shard.
    QuotaExceeded,
}

/// Shard-level pause state. Distinct from per-policy backoff: a
/// quota refusal in one policy implies the shard's entire provider
/// account may be saturated, so every other policy waits too.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShardPauseState {
    /// `Some` while paused; `None` when free to mutate.
    pub paused: Option<ShardPause>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardPause {
    pub reason: ShardPauseReason,
    /// Wall-clock nanos when the pause expires. Loop layer compares
    /// to its clock; tests inject explicit values.
    pub until_ns: i64,
}

impl ShardPauseState {
    /// Begin a shard pause. Contract: `QuotaExceeded` pauses for at
    /// least 60 seconds.
    pub fn pause_for_quota(&mut self, now_ns: i64) {
        const SIXTY_SECS_NS: i64 = 60 * 1_000_000_000;
        let until = now_ns.saturating_add(SIXTY_SECS_NS);
        // Extend an existing pause rather than truncate.
        if let Some(existing) = &self.paused
            && existing.until_ns >= until
        {
            return;
        }
        self.paused = Some(ShardPause {
            reason: ShardPauseReason::QuotaExceeded,
            until_ns: until,
        });
    }

    /// True if the shard is currently paused at `now_ns`. Clears the
    /// pause if it has expired.
    pub fn is_paused(&mut self, now_ns: i64) -> bool {
        match &self.paused {
            Some(p) if p.until_ns > now_ns => true,
            Some(_) => {
                self.paused = None;
                false
            }
            None => false,
        }
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

/// Result of [`DrPolicyStore::acquire_lease`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseAcquireResult {
    /// Lease granted. Carries the row's current `policy_version` so
    /// the caller can detect a version conflict before mutating.
    Acquired { policy_version: u64 },
    /// Lease is held by another holder and has not expired.
    HeldElsewhere {
        current_holder: String,
        until_ns: i64,
    },
}

/// Narrow store surface the reconciler needs to drive a policy through
/// one tick. Implementations are operator-owned (sqlx/filestore); the
/// `MockDrPolicyStore` ships here for tests.
///
/// Lease contract: implementations must perform the
/// "acquire if free or expired" check atomically (single UPDATE for
/// sqlx backends; mutex-guarded swap for the filestore). The shard
/// relies on this atomicity for multi-runner safety.
#[async_trait]
pub trait DrPolicyStore: Send + Sync {
    /// Return the set of policies the shard should consider this tick.
    /// Implementations typically filter out `Disabled` and `Retired`
    /// rows server-side. The shard applies backoff/rate-limit gates on
    /// top.
    async fn pending_reconcile(&self) -> Result<Vec<CollectionDrPolicy>, DrApiError>;

    /// Atomically acquire the reconcile lease on `policy_id` for
    /// `holder_id`. The lease is granted when either:
    /// - no lease is currently held, or
    /// - the held lease's `until_ns` has expired at `now_ns`, or
    /// - the caller already holds the lease (renew).
    ///
    /// Implementations MUST perform this as a single atomic
    /// compare-and-swap; otherwise two shards may race past the
    /// "free" check and both think they hold it.
    async fn acquire_lease(
        &self,
        policy_id: &str,
        holder_id: &str,
        ttl_ns: i64,
        now_ns: i64,
    ) -> Result<LeaseAcquireResult, DrApiError>;

    /// Release the lease if `holder_id` currently holds it. Idempotent
    /// — releasing a lease the caller doesn't hold is a no-op (so a
    /// crashed shard's stale release doesn't kick a fresh holder).
    async fn release_lease(&self, policy_id: &str, holder_id: &str) -> Result<(), DrApiError>;

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
    async fn update_health(&self, policy_id: &str, health: DrHealth) -> Result<(), DrApiError>;

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
        self.reconcile_one_with_observation(policy)
            .await
            .map(|(o, _)| o)
    }

    /// Same as [`reconcile_one`] but also returns the
    /// [`ProviderObservedState`] the adapter produced this tick (or
    /// `None` if the fetch failed). The shard layer uses this to
    /// surface observed lag to the metric sink without paying for a
    /// second `fetch_state` round-trip.
    pub async fn reconcile_one_with_observation(
        &self,
        policy: &CollectionDrPolicy,
    ) -> Result<(ReconcileOutcome, Option<ProviderObservedState>), DrApiError> {
        // 1. Fetch observed state. A transient adapter failure here
        //    returns AdapterTransient — caller decides retry cadence.
        let observed = match self.adapter.fetch_state(policy).await {
            Ok(o) => o,
            Err(e) => {
                let outcome = self.handle_adapter_error(policy, e, "fetch_state").await?;
                return Ok((outcome, None));
            }
        };

        // 2. Decide.
        let decision = reconcile_step(policy, &observed);

        // 3. Dispatch.
        let outcome = self.dispatch(policy, decision).await?;
        Ok((outcome, Some(observed)))
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
                self.emit_event(
                    policy,
                    DrEventType::DriftDetected,
                    Some(format!("{reason:?}")),
                )
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
                self.set_health(policy, DrHealthState::Drifted, Some(format!("{reason:?}")))
                    .await?;
                self.emit_event(
                    policy,
                    DrEventType::DriftDetected,
                    Some(format!("{reason:?}")),
                )
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
                    .set_provider_binding(&policy.policy_id, binding, policy.policy_version)
                    .await?;
                // Drive to Active if we were in PendingProviderProvisioning.
                let next_state = if policy.state == DrState::PendingProviderProvisioning {
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
                self.set_health(policy, DrHealthState::Healthy, None)
                    .await?;
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

    async fn do_retire(&self, policy: &CollectionDrPolicy) -> Result<ReconcileOutcome, DrApiError> {
        match self.adapter.retire_rule(policy).await {
            Ok(()) => {
                self.store
                    .transition_state(&policy.policy_id, DrState::Retired, policy.policy_version)
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
            return Ok(ReconcileOutcome::AdapterTransient(format!("{op}: {err}")));
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

// ---------------------------------------------------------------------------
// Metric sink (P3c4) — abstract observability hook
// ---------------------------------------------------------------------------

/// Bounded label set the shard hands to [`DrMetrics`] for every tick
/// outcome. Strings are owned to keep the trait object-safe and
/// avoid lifetime gymnastics. Cardinality is bounded by the contract:
/// `tier` is one of 5 canonical values, `provider` is one of 4,
/// `region_pair_id` is operator-curated, and `state` is one of 7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyLabels {
    pub tenant_id: String,
    pub tier: String,
    pub provider: ObjectProvider,
    pub region_pair_id: String,
    /// Snapshot of the policy's `state` at the start of this tick.
    pub state_at_tick_start: DrState,
}

impl PolicyLabels {
    /// Snapshot a policy's label set. Cheap clone of the small
    /// string fields; no allocation if the caller owns the policy.
    pub fn from_policy(policy: &CollectionDrPolicy) -> Self {
        Self {
            tenant_id: policy.tenant_id.clone(),
            tier: policy.tier.clone(),
            provider: policy.provider,
            region_pair_id: policy.region_pair_id.clone(),
            state_at_tick_start: policy.state,
        }
    }
}

/// Engine-side metric sink. The shard calls this once per policy per
/// tick. The catalog crate ships an abstract trait and two reference
/// implementations (Noop + Recording); concrete Prometheus families
/// live outside this crate (root crate or the operator layer) so the
/// catalog stays free of the `prometheus` dependency.
///
/// Implementations must be `Send + Sync` so the shard can hold
/// `Arc<dyn DrMetrics>` and share it across concurrent ticks.
///
/// Forward-compatibility: future passes may add methods (e.g.
/// `observe_lag` for `proximadb_dr_provider_lag_seconds`); all
/// additions are default-implemented to keep existing implementations
/// source-compatible.
pub trait DrMetrics: Send + Sync {
    /// Record the result of one policy's tick. Implementations
    /// typically increment per-outcome counters and update gauges
    /// keyed on `labels`.
    fn observe_tick(&self, labels: &PolicyLabels, outcome: &TickOutcome);

    /// Record a shard-wide pause event. Called once each time the
    /// shard transitions from "running" to "paused", not on every
    /// tick during the pause window.
    fn observe_shard_paused(&self, reason: ShardPauseReason) {
        let _ = reason;
    }

    /// Record an observed provider lag in seconds. Called for every
    /// policy whose `ProviderObservedState::observed_lag_seconds` is
    /// `Some` after a successful `fetch_state`. Implementations set
    /// the `proximadb_dr_provider_lag_seconds` gauge keyed on
    /// `{provider, region_pair}` per contract §"Observability".
    fn observe_lag(&self, labels: &PolicyLabels, lag_seconds: u32) {
        let _ = (labels, lag_seconds);
    }
}

/// Default no-op sink. Use in tests that don't care about metrics
/// and in production deployments that haven't wired Prometheus yet.
#[derive(Debug, Default)]
pub struct NoopDrMetrics;

impl DrMetrics for NoopDrMetrics {
    fn observe_tick(&self, _labels: &PolicyLabels, _outcome: &TickOutcome) {}
}

// ---------------------------------------------------------------------------
// Shard loop (P3c2) — async iteration with gates
// ---------------------------------------------------------------------------

/// Per-policy result of one [`DrReconcilerShard::tick`] pass. The loop
/// layer aggregates these for metric emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickOutcome {
    /// Policy was reconciled. Carries the driver's outcome.
    Reconciled(ReconcileOutcome),
    /// Backoff entry not yet ready — skipped this tick.
    DeferredBackoff,
    /// Per-policy rate limit refused — skipped this tick.
    DeferredRateLimit,
    /// Shard is paused (quota); mutations skipped. Health
    /// observations are also skipped in this simplified design — see
    /// `DrReconcilerShard::tick` for the trade-off note.
    SkippedShardPaused,
    /// Backoff hit `max_attempts`; the driver was directed to flip
    /// health to `ProviderBlocked` and the backoff was cleared.
    EscalatedAfterMaxAttempts,
    /// Another shard holds the reconcile lease; defer to next tick
    /// after the lease expires.
    DeferredLeaseHeldElsewhere {
        current_holder: String,
        until_ns: i64,
    },
}

/// Reconciler shard — one async loop per shard, one shard per range
/// of `policy_id` ULIDs. Default deployment is a single shard.
///
/// Holds the per-policy backoff and rate-limit state plus the
/// shard-wide pause state. The `tick` method does one synchronous
/// pass over the store's pending list; `run` wraps it in a
/// `tokio::time::interval` with a shutdown signal.
pub struct DrReconcilerShard<S, A> {
    driver: Arc<DrReconcilerDriver<S, A>>,
    store: Arc<S>,
    config: BackoffPolicy,
    backoffs: parking_lot::Mutex<std::collections::HashMap<String, BackoffEntry>>,
    rate_limits: parking_lot::Mutex<std::collections::HashMap<String, PerPolicyRateLimit>>,
    pause: parking_lot::Mutex<ShardPauseState>,
    now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Sampled per `apply_backoff_signal` call. Production wires
    /// `rand::random`; tests inject a constant.
    jitter_source: Arc<dyn Fn() -> f64 + Send + Sync>,
    /// Per-process identifier this shard uses when acquiring a
    /// reconcile lease. Two shards in the same deployment must have
    /// distinct holder IDs for the lease gate to work.
    holder_id: String,
    /// Lease lifetime in nanoseconds. Default 5 minutes — long
    /// enough that a tick under load doesn't expire mid-flight,
    /// short enough that a crashed shard's lease frees within one
    /// recovery interval.
    lease_ttl_ns: i64,
    /// Metric sink. Defaults to [`NoopDrMetrics`]; production wires a
    /// Prometheus-backed implementation from outside the catalog
    /// crate via `with_metrics`.
    metrics: Arc<dyn DrMetrics>,
}

impl<S, A> DrReconcilerShard<S, A>
where
    S: DrPolicyStore + 'static,
    A: DrProviderAdapter + 'static,
{
    /// Construct a shard with default config and explicit clock /
    /// jitter sources. Tests use this; production callers can call
    /// `new` instead. Default holder ID is `"shard-default"` and the
    /// default lease TTL is 5 minutes.
    pub fn with_clocks(
        driver: Arc<DrReconcilerDriver<S, A>>,
        store: Arc<S>,
        config: BackoffPolicy,
        now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
        jitter_source: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> Self {
        Self {
            driver,
            store,
            config,
            backoffs: parking_lot::Mutex::new(std::collections::HashMap::new()),
            rate_limits: parking_lot::Mutex::new(std::collections::HashMap::new()),
            pause: parking_lot::Mutex::new(ShardPauseState::default()),
            now_ns,
            jitter_source,
            holder_id: "shard-default".into(),
            lease_ttl_ns: 5 * 60 * 1_000_000_000,
            metrics: Arc::new(NoopDrMetrics),
        }
    }

    /// Override the lease holder ID. Multi-shard deployments must
    /// give each shard a distinct ID (typically `"shard-{ordinal}-{pid}"`).
    pub fn with_holder_id(mut self, holder_id: impl Into<String>) -> Self {
        self.holder_id = holder_id.into();
        self
    }

    /// Override the lease TTL. Default is 5 minutes; tests use shorter
    /// values to exercise expiry without long waits.
    pub fn with_lease_ttl_ns(mut self, ttl_ns: i64) -> Self {
        self.lease_ttl_ns = ttl_ns;
        self
    }

    /// Inject a [`DrMetrics`] implementation. Production wires a
    /// Prometheus-backed sink from the root or operator crate;
    /// tests use the recording double.
    pub fn with_metrics(mut self, metrics: Arc<dyn DrMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// One synchronous-from-the-shard's-perspective pass: load the
    /// pending list, gate each policy on shard-pause/backoff/rate-
    /// limit, run `reconcile_one`, feed the outcome back into local
    /// state.
    ///
    /// Trade-off note: under shard pause we skip the policy entirely
    /// instead of running `fetch_state` for drift observation. The
    /// contract permits read-only observation during pause; we trade
    /// that responsiveness for a simpler dispatch path. P3c2.1 can
    /// add the observe-only branch if drift visibility during quota
    /// pause matters.
    pub async fn tick(&self) -> Result<Vec<(String, TickOutcome)>, DrApiError> {
        let now = (self.now_ns)();
        let paused = self.pause.lock().is_paused(now);
        let pending = self.store.pending_reconcile().await?;
        let mut results = Vec::with_capacity(pending.len());

        for policy in pending {
            let labels = PolicyLabels::from_policy(&policy);

            if paused {
                let outcome = TickOutcome::SkippedShardPaused;
                self.metrics.observe_tick(&labels, &outcome);
                results.push((policy.policy_id.clone(), outcome));
                continue;
            }

            // Escalation check FIRST so a stuck entry doesn't sit at
            // max_attempts forever.
            let backoff_now = self.backoffs.lock().get(&policy.policy_id).cloned();
            if let Some(entry) = &backoff_now {
                if should_escalate(self.config, entry) {
                    // The driver's health update happens through the
                    // store directly so the loop layer doesn't need
                    // to know the driver internals.
                    let mut h = policy.health.clone();
                    h.state = DrHealthState::ProviderBlocked;
                    h.reason = Some(format!(
                        "escalated after {} transient attempts",
                        entry.attempt
                    ));
                    h.last_reconciled_at_ns = Some(now);
                    self.store.update_health(&policy.policy_id, h).await?;
                    self.backoffs.lock().remove(&policy.policy_id);
                    let outcome = TickOutcome::EscalatedAfterMaxAttempts;
                    self.metrics.observe_tick(&labels, &outcome);
                    results.push((policy.policy_id.clone(), outcome));
                    continue;
                }
                if !is_ready(entry, now) {
                    let outcome = TickOutcome::DeferredBackoff;
                    self.metrics.observe_tick(&labels, &outcome);
                    results.push((policy.policy_id.clone(), outcome));
                    continue;
                }
            }

            // Rate-limit gate.
            let proceed = {
                let mut rl_map = self.rate_limits.lock();
                let entry = rl_map.entry(policy.policy_id.clone()).or_default();
                entry.try_acquire(self.config, now)
            };
            if !proceed {
                let outcome = TickOutcome::DeferredRateLimit;
                self.metrics.observe_tick(&labels, &outcome);
                results.push((policy.policy_id.clone(), outcome));
                continue;
            }

            // Lease gate — atomically acquire the reconcile lease
            // before any mutating work. Multi-runner safety from the
            // contract's "Concurrency And Provider Rule Locking"
            // section. Single-runner deployments still go through
            // this path; the store always returns Acquired.
            let lease = self
                .store
                .acquire_lease(&policy.policy_id, &self.holder_id, self.lease_ttl_ns, now)
                .await?;
            match lease {
                LeaseAcquireResult::HeldElsewhere {
                    current_holder,
                    until_ns,
                } => {
                    let outcome = TickOutcome::DeferredLeaseHeldElsewhere {
                        current_holder,
                        until_ns,
                    };
                    self.metrics.observe_tick(&labels, &outcome);
                    results.push((policy.policy_id.clone(), outcome));
                    continue;
                }
                LeaseAcquireResult::Acquired { .. } => {
                    // Fall through to dispatch.
                }
            }

            // Dispatch. The richer entry point also returns the
            // observation so we can surface lag without a second
            // fetch_state round-trip.
            let (outcome, observation) =
                self.driver.reconcile_one_with_observation(&policy).await?;
            if let Some(lag) = observation.as_ref().and_then(|o| o.observed_lag_seconds) {
                self.metrics.observe_lag(&labels, lag);
            }
            // Record the shard pause event exactly once on the
            // transition, not on subsequent ticks during the pause.
            let was_paused_before = self.pause.lock().paused.is_some();
            self.feedback(&policy.policy_id, &outcome, now);
            let is_paused_after = self.pause.lock().paused.is_some();
            if !was_paused_before && is_paused_after {
                self.metrics
                    .observe_shard_paused(ShardPauseReason::QuotaExceeded);
            }
            // Best-effort release; the lease will expire naturally if
            // the release call fails (the store impl can return a
            // transient error). Don't surface release errors back to
            // the caller — they shouldn't fail the tick.
            let _ = self
                .store
                .release_lease(&policy.policy_id, &self.holder_id)
                .await;
            let tick_outcome = TickOutcome::Reconciled(outcome);
            self.metrics.observe_tick(&labels, &tick_outcome);
            results.push((policy.policy_id, tick_outcome));
        }

        Ok(results)
    }

    /// Map a [`ReconcileOutcome`] back into shard state.
    fn feedback(&self, policy_id: &str, outcome: &ReconcileOutcome, now_ns: i64) {
        match outcome {
            ReconcileOutcome::AdapterTransient(_) => {
                let mut bs = self.backoffs.lock();
                let current = bs.get(policy_id).cloned().unwrap_or_default();
                let jitter = (self.jitter_source)();
                if let Some(new_entry) = apply_backoff_signal(
                    self.config,
                    current,
                    BackoffSignal::TransientFailure,
                    now_ns,
                    jitter,
                ) {
                    bs.insert(policy_id.to_string(), new_entry);
                }
            }
            ReconcileOutcome::AdapterEscalated(BlockReason::ProviderQuotaExceeded) => {
                self.pause.lock().pause_for_quota(now_ns);
                self.backoffs.lock().remove(policy_id);
            }
            ReconcileOutcome::AdapterEscalated(_) => {
                // Misconfig / AuthDenied → driver already flipped
                // health; just clear backoff so we stop retrying.
                self.backoffs.lock().remove(policy_id);
            }
            // Any success path clears the backoff entry.
            ReconcileOutcome::EnsuredRule { .. }
            | ReconcileOutcome::Retired
            | ReconcileOutcome::RepairedDrift(_)
            | ReconcileOutcome::Idle(_) => {
                self.backoffs.lock().remove(policy_id);
            }
            // Marked-* outcomes leave backoff as-is. They reflect a
            // catalog-side condition (drift/billing) the operator
            // must resolve; reconciler doesn't retry on its own.
            ReconcileOutcome::MarkedDrifted(_)
            | ReconcileOutcome::MarkedProviderBlocked(_)
            | ReconcileOutcome::MarkedBillingBlocked(_) => {}
        }
    }

    /// Expose the current backoff state for metric emission and
    /// observability. Caller must not mutate.
    pub fn backoffs_snapshot(&self) -> std::collections::HashMap<String, BackoffEntry> {
        self.backoffs.lock().clone()
    }

    /// Is the shard currently paused at `now_ns`? Mutating: clears
    /// the pause if expired.
    pub fn is_paused(&self, now_ns: i64) -> bool {
        self.pause.lock().is_paused(now_ns)
    }
}

// ---------------------------------------------------------------------------
// Public testing surface
// ---------------------------------------------------------------------------

/// Reference test doubles for the DR engine. Operators wiring real
/// stores and adapters should treat these as the canonical way to
/// drive the contract in integration tests — keeps assertions about
/// the reconciler's behaviour identical across in-process tests and
/// the operator's CI.
///
/// The catalog crate ships these in a `pub` module rather than
/// behind a `testing` feature flag because they are zero-cost when
/// unused (no compile-time impact on production binaries — Rust
/// tree-shakes unreferenced types).
pub mod testing {
    use super::{
        CollectionDrEvent, CollectionDrPolicy, DrApiError, DrHealth, DrMetrics, DrPolicyStore,
        DrState, LeaseAcquireResult, PolicyLabels, ProviderReplicationBinding, ShardPauseReason,
        TickOutcome,
    };
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// In-memory `DrPolicyStore` for unit and integration tests.
    /// Records every mutation so assertions can pin the order and
    /// contents. Atomicity around the lease compare-and-swap is via
    /// the per-field `parking_lot::Mutex` guards.
    #[derive(Default)]
    pub struct MockDrPolicyStore {
        events: Mutex<Vec<CollectionDrEvent>>,
        health_updates: Mutex<Vec<(String, DrHealth)>>,
        state_transitions: Mutex<Vec<(String, DrState, u64)>>,
        bindings: Mutex<Vec<(String, ProviderReplicationBinding, u64)>>,
        next_version: AtomicU64,
        inject_error: Mutex<Option<DrApiError>>,
        pending: Mutex<Vec<CollectionDrPolicy>>,
        leases: Mutex<std::collections::HashMap<String, (String, i64)>>,
    }

    impl MockDrPolicyStore {
        /// Build a store that allocates new policy versions starting
        /// from `starting_version`.
        pub fn new(starting_version: u64) -> Arc<Self> {
            let s = Self::default();
            s.next_version.store(starting_version, Ordering::Relaxed);
            Arc::new(s)
        }

        /// Snapshot of every event the reconciler recorded.
        pub fn events_snapshot(&self) -> Vec<CollectionDrEvent> {
            self.events.lock().clone()
        }
        /// Snapshot of every `update_health` call.
        pub fn health_snapshot(&self) -> Vec<(String, DrHealth)> {
            self.health_updates.lock().clone()
        }
        /// Snapshot of every state transition.
        pub fn transitions_snapshot(&self) -> Vec<(String, DrState, u64)> {
            self.state_transitions.lock().clone()
        }
        /// Snapshot of every `set_provider_binding` call.
        pub fn bindings_snapshot(&self) -> Vec<(String, ProviderReplicationBinding, u64)> {
            self.bindings.lock().clone()
        }

        /// Replace the pending list returned by the next
        /// `pending_reconcile` call.
        pub fn seed_pending(&self, policies: Vec<CollectionDrPolicy>) {
            *self.pending.lock() = policies;
        }

        /// Inject a one-shot error returned by the next store call
        /// (whichever method runs first consumes it). Useful for
        /// driving the reconciler's failure paths.
        pub fn inject_error(&self, err: DrApiError) {
            *self.inject_error.lock() = Some(err);
        }
    }

    #[async_trait]
    impl DrPolicyStore for MockDrPolicyStore {
        async fn pending_reconcile(&self) -> Result<Vec<CollectionDrPolicy>, DrApiError> {
            if let Some(e) = self.inject_error.lock().take() {
                return Err(e);
            }
            Ok(self.pending.lock().clone())
        }

        async fn acquire_lease(
            &self,
            policy_id: &str,
            holder_id: &str,
            ttl_ns: i64,
            now_ns: i64,
        ) -> Result<LeaseAcquireResult, DrApiError> {
            if let Some(e) = self.inject_error.lock().take() {
                return Err(e);
            }
            let mut leases = self.leases.lock();
            let free_or_mine = match leases.get(policy_id) {
                None => true,
                Some((existing_holder, until)) => existing_holder == holder_id || *until <= now_ns,
            };
            if free_or_mine {
                leases.insert(
                    policy_id.to_string(),
                    (holder_id.to_string(), now_ns.saturating_add(ttl_ns)),
                );
                Ok(LeaseAcquireResult::Acquired {
                    policy_version: self.next_version.load(Ordering::Relaxed),
                })
            } else {
                let (current_holder, until_ns) = leases.get(policy_id).unwrap().clone();
                Ok(LeaseAcquireResult::HeldElsewhere {
                    current_holder,
                    until_ns,
                })
            }
        }

        async fn release_lease(&self, policy_id: &str, holder_id: &str) -> Result<(), DrApiError> {
            if let Some(e) = self.inject_error.lock().take() {
                return Err(e);
            }
            let mut leases = self.leases.lock();
            if let Some((existing, _)) = leases.get(policy_id)
                && existing == holder_id
            {
                leases.remove(policy_id);
            }
            Ok(())
        }

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

        async fn update_health(&self, policy_id: &str, health: DrHealth) -> Result<(), DrApiError> {
            if let Some(e) = self.inject_error.lock().take() {
                return Err(e);
            }
            self.health_updates.lock().push((policy_id.into(), health));
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

    /// Recording `DrMetrics` for tests. Captures every
    /// `observe_tick`, `observe_shard_paused`, and `observe_lag`
    /// call so tests can assert that the reconciler hit the metric
    /// layer correctly.
    #[derive(Default)]
    pub struct RecordingDrMetrics {
        ticks: Mutex<Vec<(PolicyLabels, TickOutcome)>>,
        pauses: Mutex<Vec<ShardPauseReason>>,
        lags: Mutex<Vec<(PolicyLabels, u32)>>,
    }

    impl RecordingDrMetrics {
        /// Build an empty recorder, returned as `Arc` so it can be
        /// shared between the shard and the test that asserts.
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        /// Snapshot of every tick observation.
        pub fn ticks(&self) -> Vec<(PolicyLabels, TickOutcome)> {
            self.ticks.lock().clone()
        }
        /// Snapshot of every pause observation.
        pub fn pauses(&self) -> Vec<ShardPauseReason> {
            self.pauses.lock().clone()
        }
        /// Snapshot of every lag observation.
        pub fn lags(&self) -> Vec<(PolicyLabels, u32)> {
            self.lags.lock().clone()
        }
    }

    impl DrMetrics for RecordingDrMetrics {
        fn observe_tick(&self, labels: &PolicyLabels, outcome: &TickOutcome) {
            self.ticks.lock().push((labels.clone(), outcome.clone()));
        }
        fn observe_shard_paused(&self, reason: ShardPauseReason) {
            self.pauses.lock().push(reason);
        }
        fn observe_lag(&self, labels: &PolicyLabels, lag_seconds: u32) {
            self.lags.lock().push((labels.clone(), lag_seconds));
        }
    }
}

// ---------------------------------------------------------------------------
// Async runner (P3c5) — interval-driven loop + shutdown
// ---------------------------------------------------------------------------

/// Runner config. Mirrors the contract's
/// `[dr.reconciler] poll_interval_seconds` setting; defaults to 60s
/// per §"Scheduling And Backpressure".
#[derive(Debug, Clone, Copy)]
pub struct RunnerConfig {
    pub poll_interval: std::time::Duration,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(60),
        }
    }
}

/// Stats returned by the runner on shutdown — useful for shutdown
/// diagnostics and integration assertions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunnerStats {
    /// Number of `shard.tick()` calls that returned successfully.
    pub successful_ticks: u64,
    /// Number of `shard.tick()` calls that returned an error. The
    /// loop logs and continues after each.
    pub failed_ticks: u64,
}

/// Long-running wrapper around [`DrReconcilerShard::tick`]. Calls
/// `tick()` on a `tokio::time::interval` cadence; exits cleanly when
/// the watch receiver flips to `true`.
///
/// Errors from `tick()` are tracing-logged and counted but do not
/// terminate the loop — a transient store failure should not bring
/// the reconciler down.
///
/// Usage:
/// ```ignore
/// let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
/// let runner = DrReconcilerRunner::new(shard, RunnerConfig::default());
/// let handle = tokio::spawn(runner.run(shutdown_rx));
/// // ... later ...
/// shutdown_tx.send(true).unwrap();
/// let stats = handle.await.unwrap();
/// ```
pub struct DrReconcilerRunner<S, A> {
    shard: Arc<DrReconcilerShard<S, A>>,
    config: RunnerConfig,
}

impl<S, A> DrReconcilerRunner<S, A>
where
    S: DrPolicyStore + 'static,
    A: DrProviderAdapter + 'static,
{
    pub fn new(shard: Arc<DrReconcilerShard<S, A>>, config: RunnerConfig) -> Self {
        Self { shard, config }
    }

    /// Drive the loop until `shutdown_rx` flips to `true`. Returns
    /// the tick counters at exit. The first tick fires immediately
    /// (tokio::time::interval's MissedTickBehavior defaults to Burst,
    /// which we override to Delay so a slow tick doesn't compound
    /// into a tick storm).
    pub async fn run(self, mut shutdown_rx: tokio::sync::watch::Receiver<bool>) -> RunnerStats {
        let mut interval = tokio::time::interval(self.config.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut stats = RunnerStats::default();
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match self.shard.tick().await {
                        Ok(_) => stats.successful_ticks += 1,
                        Err(e) => {
                            stats.failed_ticks += 1;
                            tracing::warn!(
                                target: "proximadb::dr::reconciler",
                                error = ?e,
                                "DR reconciler tick failed; continuing"
                            );
                        }
                    }
                }
                changed = shutdown_rx.changed() => {
                    // Channel closed or value changed; either way,
                    // exit cleanly. If changed errored (sender
                    // dropped), treat it as shutdown.
                    if changed.is_err() || *shutdown_rx.borrow() {
                        return stats;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoragePoolClass;
    use crate::collection_dr_policy::{
        CollectionDrPolicy, DrBillingBinding, DrHealth, DrPlacement, DrReplicationBehavior,
        ObjectProvider, ProviderObservedState, ProviderReplicationBinding,
    };

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
                source_pool_class: StoragePoolClass::Standard,
                destination_pool_class: StoragePoolClass::Standard,
                source_bucket_or_account: "src-bucket".into(),
                destination_bucket_or_account: "dst-bucket".into(),
                source_container: None,
                destination_container: None,
                source_prefix: "data/tnt_acme/ns_1/col_orders/".into(),
                destination_prefix: "data/tnt_acme/ns_1/col_orders/".into(),
            },
            replication: DrReplicationBehavior::default(),
            billing: DrBillingBinding {
                cost_binding_ref: "dr-standard-binding".into(),
                cost_owner_tenant_id: "tnt_acme".into(),
                billing_approval_id: Some("appr_1".into()),
                operator_estimate_cents: Some(10_000),
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
            observed_destination: Some(policy.placement.destination_bucket_or_account.clone()),
            observed_destination_container: policy.placement.destination_container.clone(),
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
    use crate::dr_reconciler::testing::MockDrPolicyStore;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

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
        let now_ns: Arc<dyn Fn() -> i64 + Send + Sync> =
            Arc::new(move || clock_clone.fetch_add(1_000, Ordering::Relaxed));
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
        assert!(
            events
                .iter()
                .any(|e| e.event_type == DrEventType::DriftDetected)
        );
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
            ReconcileOutcome::MarkedProviderBlocked(BlockReason::ProviderMisconfiguration)
        );
        assert_eq!(adapter.ensure_call_count(), 0);
        let h = store.health_snapshot();
        assert!(
            h.iter()
                .any(|(_, h)| h.state == DrHealthState::ProviderBlocked)
        );
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
        let event_types: Vec<_> = store
            .events_snapshot()
            .iter()
            .map(|e| e.event_type)
            .collect();
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
        let event_types: Vec<_> = store
            .events_snapshot()
            .iter()
            .map(|e| e.event_type)
            .collect();
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
        assert!(
            h.iter()
                .any(|(_, h)| h.state == DrHealthState::ProviderBlocked)
        );
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

    // --- Backoff / rate limit / shard pause (P3c1) --------------------

    fn test_policy() -> BackoffPolicy {
        BackoffPolicy {
            initial_delay_secs: 30,
            max_delay_secs: 30 * 60,
            jitter_factor: 0.0, // no jitter for deterministic tests
            max_attempts: 4,
            min_call_interval_secs: 5,
        }
    }

    #[test]
    fn backoff_default_matches_contract_spec() {
        let d = BackoffPolicy::default();
        // §"Scheduling And Backpressure": 30s → 30m, 12 attempts.
        assert_eq!(d.initial_delay_secs, 30);
        assert_eq!(d.max_delay_secs, 30 * 60);
        assert_eq!(d.max_attempts, 12);
        assert_eq!(d.min_call_interval_secs, 5);
        // Default jitter is 25%.
        assert!((d.jitter_factor - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn backoff_grows_exponentially_until_cap_no_jitter() {
        let p = test_policy(); // jitter = 0 → deterministic
        let now = 1_000_000_000_000_i64;
        let cap_ns = (p.max_delay_secs as i64) * 1_000_000_000;

        let mut delays = Vec::new();
        let mut entry = BackoffEntry::default();
        for _ in 0..6 {
            entry = apply_backoff_signal(p, entry, BackoffSignal::TransientFailure, now, 0.5)
                .expect("transient yields a new entry");
            delays.push(entry.earliest_retry_ns - now);
        }
        // 30s → 60s → 120s → 240s → 480s → 960s
        assert_eq!(delays[0], 30 * 1_000_000_000);
        assert_eq!(delays[1], 60 * 1_000_000_000);
        assert_eq!(delays[2], 120 * 1_000_000_000);
        assert_eq!(delays[3], 240 * 1_000_000_000);
        assert_eq!(delays[4], 480 * 1_000_000_000);
        assert_eq!(delays[5], 960 * 1_000_000_000);
        // None exceed the cap (30m = 1800s).
        for d in &delays {
            assert!(*d <= cap_ns);
        }
    }

    #[test]
    fn backoff_respects_max_delay_cap() {
        let p = BackoffPolicy {
            initial_delay_secs: 30,
            max_delay_secs: 120, // unrealistically tight cap
            jitter_factor: 0.0,
            max_attempts: 12,
            min_call_interval_secs: 5,
        };
        let now = 0_i64;
        let mut entry = BackoffEntry::default();
        for _ in 0..6 {
            entry =
                apply_backoff_signal(p, entry, BackoffSignal::TransientFailure, now, 0.5).unwrap();
        }
        // Once past the cap, every subsequent delay equals 120s.
        assert_eq!(entry.earliest_retry_ns, 120 * 1_000_000_000);
    }

    #[test]
    fn backoff_jitter_scales_within_band() {
        let p = BackoffPolicy {
            initial_delay_secs: 100,
            max_delay_secs: 10_000,
            jitter_factor: 0.5, // ±50% band
            max_attempts: 5,
            min_call_interval_secs: 1,
        };
        let now = 0_i64;
        // u01 = 0.0 → multiplier 0.5; u01 = 1.0 → multiplier 1.5;
        // u01 = 0.5 → multiplier 1.0.
        let low = apply_backoff_signal(
            p,
            BackoffEntry::default(),
            BackoffSignal::TransientFailure,
            now,
            0.0,
        )
        .unwrap();
        let mid = apply_backoff_signal(
            p,
            BackoffEntry::default(),
            BackoffSignal::TransientFailure,
            now,
            0.5,
        )
        .unwrap();
        let high = apply_backoff_signal(
            p,
            BackoffEntry::default(),
            BackoffSignal::TransientFailure,
            now,
            1.0,
        )
        .unwrap();
        let base_ns = 100_i64 * 1_000_000_000;
        assert_eq!(low.earliest_retry_ns, base_ns / 2);
        assert_eq!(mid.earliest_retry_ns, base_ns);
        assert_eq!(high.earliest_retry_ns, base_ns * 3 / 2);
    }

    #[test]
    fn backoff_success_drops_entry() {
        let p = test_policy();
        let entry = BackoffEntry {
            attempt: 3,
            earliest_retry_ns: 999_999,
        };
        // Success → caller deletes (None).
        assert!(apply_backoff_signal(p, entry, BackoffSignal::Success, 0, 0.5).is_none());
    }

    #[test]
    fn backoff_escalate_drops_entry() {
        let p = test_policy();
        let entry = BackoffEntry {
            attempt: 3,
            earliest_retry_ns: 999_999,
        };
        // Escalate → caller deletes (driver flips health separately).
        assert!(apply_backoff_signal(p, entry, BackoffSignal::Escalate, 0, 0.5).is_none());
    }

    #[test]
    fn should_escalate_fires_at_max_attempts() {
        let p = test_policy(); // max_attempts = 4
        assert!(!should_escalate(
            p,
            &BackoffEntry {
                attempt: 3,
                earliest_retry_ns: 0
            }
        ));
        assert!(should_escalate(
            p,
            &BackoffEntry {
                attempt: 4,
                earliest_retry_ns: 0
            }
        ));
        assert!(should_escalate(
            p,
            &BackoffEntry {
                attempt: 99,
                earliest_retry_ns: 0
            }
        ));
    }

    #[test]
    fn is_ready_compares_against_now() {
        let entry = BackoffEntry {
            attempt: 2,
            earliest_retry_ns: 1_000,
        };
        assert!(!is_ready(&entry, 999));
        assert!(is_ready(&entry, 1_000));
        assert!(is_ready(&entry, 1_001));
    }

    #[test]
    fn rate_limit_first_call_always_succeeds() {
        let p = test_policy();
        let mut rl = PerPolicyRateLimit::default();
        assert!(rl.try_acquire(p, 1_000_000));
    }

    #[test]
    fn rate_limit_refuses_within_min_interval() {
        let p = test_policy(); // 5s
        let mut rl = PerPolicyRateLimit::default();
        let t0 = 1_000_000_000_000_i64;
        assert!(rl.try_acquire(p, t0));
        // 4.99s later — still inside window, refused.
        assert!(!rl.try_acquire(p, t0 + 4_990_000_000));
        // Exactly 5s later — allowed.
        assert!(rl.try_acquire(p, t0 + 5_000_000_000));
        // Right after — refused again.
        assert!(!rl.try_acquire(p, t0 + 5_001_000_000));
    }

    #[test]
    fn shard_pause_starts_unset() {
        let s = ShardPauseState::default();
        assert!(s.paused.is_none());
    }

    #[test]
    fn shard_pause_quota_sets_for_at_least_60_seconds() {
        let mut s = ShardPauseState::default();
        let t0 = 5_000_000_000_000_i64;
        s.pause_for_quota(t0);
        let p = s.paused.expect("paused after quota");
        assert_eq!(p.reason, ShardPauseReason::QuotaExceeded);
        assert_eq!(p.until_ns - t0, 60 * 1_000_000_000);
    }

    #[test]
    fn shard_pause_is_paused_returns_true_then_clears_on_expiry() {
        let mut s = ShardPauseState::default();
        let t0 = 5_000_000_000_000_i64;
        s.pause_for_quota(t0);
        assert!(s.is_paused(t0));
        assert!(s.is_paused(t0 + 59 * 1_000_000_000));
        // After the 60s mark — pause expires and clears.
        assert!(!s.is_paused(t0 + 60 * 1_000_000_000 + 1));
        assert!(s.paused.is_none());
    }

    #[test]
    fn shard_pause_extends_overlapping_quota_events() {
        // A second quota refusal during an active pause should
        // extend the window, not truncate it. The reconciler runs in
        // ticks; if quota persists, we don't want the pause shrinking
        // because the clock advanced past the second event's "until".
        let mut s = ShardPauseState::default();
        let t0 = 0_i64;
        s.pause_for_quota(t0);
        let first_until = s.paused.as_ref().unwrap().until_ns;
        // Second event 10s later — would naively expire at t0+70s,
        // which IS later than first_until (t0+60s), so extend.
        s.pause_for_quota(t0 + 10 * 1_000_000_000);
        let second_until = s.paused.as_ref().unwrap().until_ns;
        assert!(second_until > first_until);
        // But a second event 1s later (until = t0+61s, also later
        // than t0+60s) extends by 1s only.
        let mut s2 = ShardPauseState::default();
        s2.pause_for_quota(t0);
        s2.pause_for_quota(t0 + 1_000_000_000);
        let until = s2.paused.as_ref().unwrap().until_ns;
        assert_eq!(until, t0 + 61 * 1_000_000_000);
    }

    #[test]
    fn shard_pause_does_not_shrink_on_stale_event() {
        // A "later" quota event whose until is BEFORE the current
        // pause's until must NOT shrink the window. Defends against
        // a clock-skew or out-of-order event flipping the pause off
        // early.
        let mut s = ShardPauseState::default();
        let t0 = 100_000_000_000_i64;
        s.pause_for_quota(t0);
        let first_until = s.paused.as_ref().unwrap().until_ns;
        // Simulate a stale event at t0 - 30s (its until = t0 + 30s,
        // earlier than first_until = t0 + 60s).
        s.pause_for_quota(t0 - 30 * 1_000_000_000);
        let second_until = s.paused.as_ref().unwrap().until_ns;
        assert_eq!(second_until, first_until, "pause must not shrink");
    }

    // --- Shard loop (P3c2) --------------------------------------------

    fn make_shard(
        store: Arc<MockDrPolicyStore>,
        adapter: Arc<MockDrProviderAdapter>,
        config: BackoffPolicy,
        clock: Arc<AtomicI64>,
        jitter: f64,
    ) -> DrReconcilerShard<MockDrPolicyStore, MockDrProviderAdapter> {
        let driver = Arc::new(make_driver(store.clone(), adapter.clone()));
        let clock_clone = clock.clone();
        let now: Arc<dyn Fn() -> i64 + Send + Sync> =
            Arc::new(move || clock_clone.load(Ordering::Relaxed));
        let jitter_src: Arc<dyn Fn() -> f64 + Send + Sync> = Arc::new(move || jitter);
        DrReconcilerShard::with_clocks(driver, store, config, now, jitter_src)
    }

    #[tokio::test]
    async fn shard_tick_empty_pending_list_yields_no_results() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let clock = Arc::new(AtomicI64::new(0));
        let shard = make_shard(store, adapter, BackoffPolicy::default(), clock, 0.5);
        let results = shard.tick().await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn shard_tick_reconciles_healthy_policy() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        adapter.seed_observed(&p.policy_id, healthy_observed(&p));
        store.seed_pending(vec![p.clone()]);
        let clock = Arc::new(AtomicI64::new(0));
        let shard = make_shard(store.clone(), adapter, BackoffPolicy::default(), clock, 0.5);

        let results = shard.tick().await.unwrap();
        assert_eq!(results.len(), 1);
        match &results[0].1 {
            TickOutcome::Reconciled(ReconcileOutcome::Idle(IdleReason::HealthyActive)) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn shard_tick_transient_increments_backoff_and_defers_next_tick() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        store.seed_pending(vec![p.clone()]);
        // First call (fetch_state) returns transient.
        adapter.inject_error(ProviderError::Transient("blip".into()));
        let clock = Arc::new(AtomicI64::new(0));
        let shard = make_shard(
            store.clone(),
            adapter.clone(),
            BackoffPolicy::default(),
            clock.clone(),
            0.5,
        );

        let results = shard.tick().await.unwrap();
        assert!(matches!(
            results[0].1,
            TickOutcome::Reconciled(ReconcileOutcome::AdapterTransient(_))
        ));
        // Backoff was recorded for this policy.
        let bs = shard.backoffs_snapshot();
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[&p.policy_id].attempt, 1);

        // Second tick at the SAME timestamp: backoff not ready yet
        // (next retry is ~30s away with default policy).
        // But rate limit would also trip first since clock didn't
        // advance. To isolate the backoff gate, advance past the
        // rate-limit window AND keep within the backoff window.
        // Default min_call_interval=5s, initial_delay=30s. Tick at
        // 6s: rate limit OK, backoff NOT OK.
        clock.store(6 * 1_000_000_000, Ordering::Relaxed);
        let results2 = shard.tick().await.unwrap();
        assert_eq!(results2[0].1, TickOutcome::DeferredBackoff);
    }

    #[tokio::test]
    async fn shard_tick_rate_limit_defers_repeated_call_within_window() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        adapter.seed_observed(&p.policy_id, healthy_observed(&p));
        store.seed_pending(vec![p.clone()]);
        let clock = Arc::new(AtomicI64::new(0));
        let shard = make_shard(
            store.clone(),
            adapter,
            BackoffPolicy::default(),
            clock.clone(),
            0.5,
        );

        // First tick: reconciles (success → no backoff entry).
        let r1 = shard.tick().await.unwrap();
        assert!(matches!(r1[0].1, TickOutcome::Reconciled(_)));
        // Same clock value → rate limit deferral.
        let r2 = shard.tick().await.unwrap();
        assert_eq!(r2[0].1, TickOutcome::DeferredRateLimit);
        // Advance past 5s → rate limit clears.
        clock.store(5 * 1_000_000_000, Ordering::Relaxed);
        let r3 = shard.tick().await.unwrap();
        assert!(matches!(r3[0].1, TickOutcome::Reconciled(_)));
    }

    #[tokio::test]
    async fn shard_tick_quota_exceeded_pauses_shard() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        store.seed_pending(vec![p.clone()]);
        adapter.inject_error(ProviderError::QuotaExceeded("rule cap".into()));
        let clock = Arc::new(AtomicI64::new(1_000));
        let shard = make_shard(
            store.clone(),
            adapter,
            BackoffPolicy::default(),
            clock.clone(),
            0.5,
        );

        let r1 = shard.tick().await.unwrap();
        assert!(matches!(
            r1[0].1,
            TickOutcome::Reconciled(ReconcileOutcome::AdapterEscalated(
                BlockReason::ProviderQuotaExceeded
            ))
        ));
        // Shard is now paused.
        assert!(shard.is_paused(clock.load(Ordering::Relaxed)));

        // Next tick: subsequent policies skipped (here just the same
        // policy still in pending).
        let r2 = shard.tick().await.unwrap();
        assert_eq!(r2[0].1, TickOutcome::SkippedShardPaused);
    }

    #[tokio::test]
    async fn shard_tick_escalates_after_max_transient_attempts() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        store.seed_pending(vec![p.clone()]);
        // Small max_attempts to keep the test tight.
        let config = BackoffPolicy {
            max_attempts: 3,
            initial_delay_secs: 1, // tiny so test clock advance is easy
            max_delay_secs: 1,
            jitter_factor: 0.0,
            min_call_interval_secs: 1,
        };
        let clock = Arc::new(AtomicI64::new(1_000));
        let shard = make_shard(store.clone(), adapter.clone(), config, clock.clone(), 0.5);

        // Drive 3 transient failures with clock advances between them.
        for _ in 0..3 {
            adapter.inject_error(ProviderError::Transient("blip".into()));
            let _ = shard.tick().await.unwrap();
            // Advance past 1s rate limit + 1s backoff.
            let cur = clock.load(Ordering::Relaxed);
            clock.store(cur + 2_500_000_000, Ordering::Relaxed);
        }
        // Backoff state now has attempt = 3 == max_attempts.
        let bs = shard.backoffs_snapshot();
        assert_eq!(bs[&p.policy_id].attempt, 3);

        // Next tick: escalation fires.
        let r = shard.tick().await.unwrap();
        assert_eq!(r[0].1, TickOutcome::EscalatedAfterMaxAttempts);
        // Backoff entry cleared.
        assert!(!shard.backoffs_snapshot().contains_key(&p.policy_id));
        // Store recorded a health flip to ProviderBlocked.
        let h = store.health_snapshot();
        assert!(
            h.iter()
                .any(|(_, h)| h.state == DrHealthState::ProviderBlocked)
        );
    }

    #[tokio::test]
    async fn shard_tick_success_clears_backoff_entry() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        adapter.seed_observed(&p.policy_id, healthy_observed(&p));
        store.seed_pending(vec![p.clone()]);
        let clock = Arc::new(AtomicI64::new(0));
        let shard = make_shard(
            store.clone(),
            adapter.clone(),
            BackoffPolicy::default(),
            clock.clone(),
            0.5,
        );

        // Inject a transient failure first.
        adapter.inject_error(ProviderError::Transient("blip".into()));
        let _ = shard.tick().await.unwrap();
        assert!(shard.backoffs_snapshot().contains_key(&p.policy_id));

        // Advance past rate-limit + backoff windows, then tick
        // cleanly. Success should clear the backoff.
        clock.store(60 * 1_000_000_000, Ordering::Relaxed);
        let r = shard.tick().await.unwrap();
        assert!(matches!(
            r[0].1,
            TickOutcome::Reconciled(ReconcileOutcome::Idle(IdleReason::HealthyActive))
        ));
        assert!(!shard.backoffs_snapshot().contains_key(&p.policy_id));
    }

    // --- Lease / TTL (P3c3) -------------------------------------------

    #[tokio::test]
    async fn lease_acquire_on_free_row_returns_acquired() {
        let store = MockDrPolicyStore::new(7);
        let result = store
            .acquire_lease("drp_1", "shard-a", 60_000_000_000, 1_000)
            .await
            .unwrap();
        assert!(matches!(
            result,
            LeaseAcquireResult::Acquired { policy_version: 7 }
        ));
    }

    #[tokio::test]
    async fn lease_acquire_by_same_holder_renews() {
        let store = MockDrPolicyStore::new(1);
        let _ = store
            .acquire_lease("drp_1", "shard-a", 60_000_000_000, 1_000)
            .await
            .unwrap();
        // Same holder may re-acquire — used for renewal mid-tick.
        let result = store
            .acquire_lease("drp_1", "shard-a", 60_000_000_000, 2_000)
            .await
            .unwrap();
        assert!(matches!(result, LeaseAcquireResult::Acquired { .. }));
    }

    #[tokio::test]
    async fn lease_acquire_by_other_holder_blocks_until_expiry() {
        let store = MockDrPolicyStore::new(1);
        // shard-a takes a 10s lease at t=1000.
        let _ = store
            .acquire_lease("drp_1", "shard-a", 10_000_000_000, 1_000)
            .await
            .unwrap();
        // shard-b tries at t=2000 — still held.
        let r = store
            .acquire_lease("drp_1", "shard-b", 10_000_000_000, 2_000)
            .await
            .unwrap();
        match r {
            LeaseAcquireResult::HeldElsewhere {
                current_holder,
                until_ns,
            } => {
                assert_eq!(current_holder, "shard-a");
                assert_eq!(until_ns, 1_000 + 10_000_000_000);
            }
            other => panic!("expected HeldElsewhere, got {other:?}"),
        }
        // shard-b retries past the expiry — grants.
        let r2 = store
            .acquire_lease("drp_1", "shard-b", 10_000_000_000, 1_000 + 10_000_000_001)
            .await
            .unwrap();
        assert!(matches!(r2, LeaseAcquireResult::Acquired { .. }));
    }

    #[tokio::test]
    async fn lease_release_is_idempotent_and_holder_scoped() {
        let store = MockDrPolicyStore::new(1);
        let _ = store
            .acquire_lease("drp_1", "shard-a", 60_000_000_000, 1_000)
            .await
            .unwrap();
        // shard-b cannot release shard-a's lease.
        store.release_lease("drp_1", "shard-b").await.unwrap();
        let r = store
            .acquire_lease("drp_1", "shard-c", 60_000_000_000, 1_500)
            .await
            .unwrap();
        assert!(matches!(r, LeaseAcquireResult::HeldElsewhere { .. }));
        // shard-a can release its own, freeing the row.
        store.release_lease("drp_1", "shard-a").await.unwrap();
        let r2 = store
            .acquire_lease("drp_1", "shard-c", 60_000_000_000, 1_600)
            .await
            .unwrap();
        assert!(matches!(r2, LeaseAcquireResult::Acquired { .. }));
        // Releasing twice is fine (idempotent).
        store.release_lease("drp_1", "shard-c").await.unwrap();
        store.release_lease("drp_1", "shard-c").await.unwrap();
    }

    #[tokio::test]
    async fn shard_tick_with_held_lease_defers_with_holder_info() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        adapter.seed_observed(&p.policy_id, healthy_observed(&p));
        store.seed_pending(vec![p.clone()]);

        // Pre-acquire the lease as a different holder.
        let _ = store
            .acquire_lease(&p.policy_id, "other-shard", 60_000_000_000, 0)
            .await
            .unwrap();

        let clock = Arc::new(AtomicI64::new(0));
        let shard = make_shard(
            store.clone(),
            adapter.clone(),
            BackoffPolicy::default(),
            clock,
            0.5,
        )
        .with_holder_id("this-shard");

        let results = shard.tick().await.unwrap();
        match &results[0].1 {
            TickOutcome::DeferredLeaseHeldElsewhere {
                current_holder,
                until_ns: _,
            } => {
                assert_eq!(current_holder, "other-shard");
            }
            other => panic!("expected DeferredLeaseHeldElsewhere, got {other:?}"),
        }
        // No adapter mutation should have happened — driver wasn't
        // called because the lease gate fired first.
        assert_eq!(adapter.ensure_call_count(), 0);
    }

    #[tokio::test]
    async fn shard_tick_acquires_lease_and_releases_after_dispatch() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        adapter.seed_observed(&p.policy_id, healthy_observed(&p));
        store.seed_pending(vec![p.clone()]);

        let clock = Arc::new(AtomicI64::new(100));
        let shard = make_shard(store.clone(), adapter, BackoffPolicy::default(), clock, 0.5)
            .with_holder_id("shard-x");

        let results = shard.tick().await.unwrap();
        assert!(matches!(results[0].1, TickOutcome::Reconciled(_)));

        // The lease was released after dispatch — a different holder
        // can immediately acquire it. (We test this via the store
        // directly because the shard owns its own holder_id.)
        let r = store
            .acquire_lease(&p.policy_id, "shard-y", 60_000_000_000, 101)
            .await
            .unwrap();
        assert!(matches!(r, LeaseAcquireResult::Acquired { .. }));
    }

    // --- DrMetrics sink (P3c4) ----------------------------------------
    // `RecordingDrMetrics` is now part of the public testing surface;
    // tests import it from there rather than redefining it locally.
    use crate::dr_reconciler::testing::RecordingDrMetrics;

    #[test]
    fn policy_labels_snapshot_carries_label_set() {
        let p = base_policy();
        let labels = PolicyLabels::from_policy(&p);
        assert_eq!(labels.tenant_id, "tnt_acme");
        assert_eq!(labels.tier, "business");
        assert_eq!(labels.provider, ObjectProvider::AwsS3);
        assert_eq!(labels.region_pair_id, "aws:us-east-1:us-west-2");
        assert_eq!(labels.state_at_tick_start, DrState::Active);
    }

    #[test]
    fn noop_metrics_is_safe_to_share_as_default() {
        let m: Arc<dyn DrMetrics> = Arc::new(NoopDrMetrics);
        let p = base_policy();
        let labels = PolicyLabels::from_policy(&p);
        // Calling the trait methods must not panic.
        m.observe_tick(&labels, &TickOutcome::DeferredBackoff);
        m.observe_shard_paused(ShardPauseReason::QuotaExceeded);
    }

    #[tokio::test]
    async fn shard_with_metrics_records_each_outcome_once() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        adapter.seed_observed(&p.policy_id, healthy_observed(&p));
        store.seed_pending(vec![p.clone()]);
        let clock = Arc::new(AtomicI64::new(0));
        let metrics = RecordingDrMetrics::new();
        let shard = make_shard(store.clone(), adapter, BackoffPolicy::default(), clock, 0.5)
            .with_metrics(metrics.clone());

        let _ = shard.tick().await.unwrap();
        let recorded = metrics.ticks();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0.tenant_id, "tnt_acme");
        assert!(matches!(
            recorded[0].1,
            TickOutcome::Reconciled(ReconcileOutcome::Idle(IdleReason::HealthyActive))
        ));
        // No pause event during normal operation.
        assert!(metrics.pauses().is_empty());
    }

    #[tokio::test]
    async fn shard_with_metrics_records_rate_limit_defer() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        adapter.seed_observed(&p.policy_id, healthy_observed(&p));
        store.seed_pending(vec![p.clone()]);
        let clock = Arc::new(AtomicI64::new(100));
        let metrics = RecordingDrMetrics::new();
        let shard = make_shard(store.clone(), adapter, BackoffPolicy::default(), clock, 0.5)
            .with_metrics(metrics.clone());

        // Tick 1: reconciles successfully.
        let _ = shard.tick().await.unwrap();
        // Tick 2 at same clock: rate-limit defer.
        let _ = shard.tick().await.unwrap();

        let kinds: Vec<_> = metrics
            .ticks()
            .into_iter()
            .map(|(_, o)| match o {
                TickOutcome::Reconciled(_) => "reconciled",
                TickOutcome::DeferredRateLimit => "rate_limit",
                TickOutcome::DeferredBackoff => "backoff",
                TickOutcome::DeferredLeaseHeldElsewhere { .. } => "lease",
                TickOutcome::SkippedShardPaused => "shard_paused",
                TickOutcome::EscalatedAfterMaxAttempts => "escalated",
            })
            .collect();
        assert_eq!(kinds, vec!["reconciled", "rate_limit"]);
    }

    #[tokio::test]
    async fn shard_with_metrics_records_pause_transition_once() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.state = DrState::PendingProviderProvisioning;
        store.seed_pending(vec![p.clone()]);
        adapter.inject_error(ProviderError::QuotaExceeded("rule cap".into()));
        let clock = Arc::new(AtomicI64::new(1_000));
        let metrics = RecordingDrMetrics::new();
        let shard = make_shard(store.clone(), adapter, BackoffPolicy::default(), clock, 0.5)
            .with_metrics(metrics.clone());

        // First tick triggers the pause.
        let _ = shard.tick().await.unwrap();
        // Second tick happens during pause — the policy is skipped.
        let _ = shard.tick().await.unwrap();
        // Pause was observed exactly once at the transition, not on
        // every tick during the pause window.
        assert_eq!(metrics.pauses(), vec![ShardPauseReason::QuotaExceeded]);
    }

    // --- Lifecycle integration test (P6.1) ----------------------------

    /// Walk one policy through the contract's full state machine via
    /// the shard, asserting that each phase produces the expected
    /// outcome + event log entries. The mock store doesn't auto-apply
    /// state transitions to its pending list, so the test re-seeds
    /// between phases to mirror what a real store would project on
    /// the next `pending_reconcile` call.
    ///
    /// This is the engine-side proof that every primitive we landed
    /// in P1..P3c4 + P4 composes into the documented lifecycle. If a
    /// future refactor breaks the contract, this test names the
    /// regression at the phase boundary.
    #[tokio::test]
    async fn lifecycle_full_state_machine_walk() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let metrics = RecordingDrMetrics::new();
        let clock = Arc::new(AtomicI64::new(1_000_000_000_000));
        let shard = make_shard(
            store.clone(),
            adapter.clone(),
            BackoffPolicy::default(),
            clock.clone(),
            0.5,
        )
        .with_metrics(metrics.clone());

        // Each phase advances 6s — past the 5s per-policy rate-limit
        // floor — so successive ticks aren't deferred.
        let advance = |clock: &Arc<AtomicI64>| {
            clock.fetch_add(6_000_000_000, Ordering::Relaxed);
        };

        // ---------- Phase 1: PendingProviderProvisioning ----------
        let mut policy = base_policy();
        policy.state = DrState::PendingProviderProvisioning;
        policy.provider_binding = None;
        store.seed_pending(vec![policy.clone()]);

        let phase1 = shard.tick().await.unwrap();
        match &phase1[0].1 {
            TickOutcome::Reconciled(ReconcileOutcome::EnsuredRule { .. }) => {}
            other => panic!("phase 1 expected EnsuredRule, got {other:?}"),
        }
        // The driver: ensure_rule → binding write → state transition
        // to Active → `active` event → health = Healthy.
        assert_eq!(adapter.ensure_call_count(), 1);
        let bindings = store.bindings_snapshot();
        assert_eq!(bindings.len(), 1);
        let new_binding = bindings[0].1.clone();
        assert_eq!(
            store
                .transitions_snapshot()
                .iter()
                .filter(|(_, s, _)| *s == DrState::Active)
                .count(),
            1
        );
        assert!(
            store
                .events_snapshot()
                .iter()
                .any(|e| e.event_type == DrEventType::Active)
        );

        // ---------- Phase 2: Active + healthy ----------
        // Mirror what the store would project: state = Active, binding set.
        policy.state = DrState::Active;
        policy.provider_binding = Some(new_binding.clone());
        store.seed_pending(vec![policy.clone()]);
        adapter.seed_observed(&policy.policy_id, healthy_observed(&policy));
        advance(&clock);

        let phase2 = shard.tick().await.unwrap();
        match &phase2[0].1 {
            TickOutcome::Reconciled(ReconcileOutcome::Idle(IdleReason::HealthyActive)) => {}
            other => panic!("phase 2 expected Idle(HealthyActive), got {other:?}"),
        }

        // ---------- Phase 3: drift detected (rule missing) ----------
        let mut drift = healthy_observed(&policy);
        drift.rule_exists = false;
        adapter.seed_observed(&policy.policy_id, drift);
        advance(&clock);

        let phase3 = shard.tick().await.unwrap();
        match &phase3[0].1 {
            TickOutcome::Reconciled(ReconcileOutcome::RepairedDrift(DriftReason::RuleMissing)) => {}
            other => panic!("phase 3 expected RepairedDrift(RuleMissing), got {other:?}"),
        }
        let event_types: Vec<_> = store
            .events_snapshot()
            .iter()
            .map(|e| e.event_type)
            .collect();
        let detected = event_types
            .iter()
            .filter(|t| **t == DrEventType::DriftDetected)
            .count();
        let repaired = event_types
            .iter()
            .filter(|t| **t == DrEventType::DriftRepaired)
            .count();
        assert_eq!(detected, 1, "exactly one drift_detected event after repair");
        assert_eq!(repaired, 1, "exactly one drift_repaired event");

        // ---------- Phase 4: PendingRetirement ----------
        policy.state = DrState::PendingRetirement;
        // The mock adapter's ensure_rule reflected the rule back into
        // observed state, so a fresh healthy read is what fetch_state
        // returns; nothing further to seed here.
        store.seed_pending(vec![policy.clone()]);
        advance(&clock);

        let phase4 = shard.tick().await.unwrap();
        match &phase4[0].1 {
            TickOutcome::Reconciled(ReconcileOutcome::Retired) => {}
            other => panic!("phase 4 expected Retired, got {other:?}"),
        }
        // retire_rule was called once; transition to Retired recorded.
        assert_eq!(adapter.retire_call_count(), 1);
        assert!(
            store
                .transitions_snapshot()
                .iter()
                .any(|(_, s, _)| *s == DrState::Retired)
        );
        let event_types: Vec<_> = store
            .events_snapshot()
            .iter()
            .map(|e| e.event_type)
            .collect();
        assert!(event_types.contains(&DrEventType::ProviderRuleDisabled));
        assert!(event_types.contains(&DrEventType::Retired));

        // ---------- Phase 5: terminal Retired ----------
        policy.state = DrState::Retired;
        store.seed_pending(vec![policy.clone()]);
        advance(&clock);

        let phase5 = shard.tick().await.unwrap();
        match &phase5[0].1 {
            TickOutcome::Reconciled(ReconcileOutcome::Idle(IdleReason::Retired)) => {}
            other => panic!("phase 5 expected Idle(Retired), got {other:?}"),
        }
        // No further mutations after Retired.
        let bindings_after = store.bindings_snapshot().len();
        let transitions_after = store.transitions_snapshot().len();
        advance(&clock);
        let phase5b = shard.tick().await.unwrap();
        assert!(matches!(
            phase5b[0].1,
            TickOutcome::Reconciled(ReconcileOutcome::Idle(IdleReason::Retired))
        ));
        assert_eq!(
            store.bindings_snapshot().len(),
            bindings_after,
            "no new bindings after Retired"
        );
        assert_eq!(
            store.transitions_snapshot().len(),
            transitions_after,
            "no new transitions after Retired"
        );

        // ---------- Metric layer sanity check ----------
        // Every tick produced exactly one observe_tick call.
        let recorded = metrics.ticks();
        assert_eq!(
            recorded.len(),
            6,
            "5 phases + 1 phase-5b idempotency tick = 6 observed ticks"
        );
        // No shard pauses occurred (no QuotaExceeded errors).
        assert!(metrics.pauses().is_empty());
    }

    // --- Async runner -------------------------------------------------

    /// Helper: build a shard whose store starts with no pending
    /// policies. The runner exercises pure cadence and shutdown
    /// semantics; the per-policy gates already have coverage in
    /// the dedicated tick tests.
    fn make_runner_shard() -> (
        Arc<MockDrPolicyStore>,
        Arc<DrReconcilerShard<MockDrPolicyStore, MockDrProviderAdapter>>,
    ) {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let clock = Arc::new(AtomicI64::new(0));
        let shard = Arc::new(make_shard(
            store.clone(),
            adapter,
            BackoffPolicy::default(),
            clock,
            0.5,
        ));
        (store, shard)
    }

    #[test]
    fn runner_config_default_matches_contract_60s() {
        let c = RunnerConfig::default();
        assert_eq!(c.poll_interval, std::time::Duration::from_secs(60));
    }

    #[tokio::test(start_paused = true)]
    async fn runner_ticks_at_configured_cadence_and_exits_on_shutdown() {
        let (store, shard) = make_runner_shard();
        // Seed one policy so each tick has work to record (verifies
        // the loop actually calls shard.tick).
        let p = base_policy();
        store.seed_pending(vec![p]);

        let cfg = RunnerConfig {
            poll_interval: std::time::Duration::from_millis(100),
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let runner = DrReconcilerRunner::new(shard.clone(), cfg);
        let handle = tokio::spawn(runner.run(shutdown_rx));

        // Let three intervals elapse: 0ms (first tick fires
        // immediately), then 100ms and 200ms.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        shutdown_tx.send(true).unwrap();
        let stats = handle.await.unwrap();
        // Expect 3 successful ticks: at 0, 100, 200ms.
        assert!(
            stats.successful_ticks >= 3,
            "expected ≥3 ticks, got {}",
            stats.successful_ticks
        );
        assert_eq!(stats.failed_ticks, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn runner_exits_immediately_on_initial_shutdown_signal() {
        // If the shutdown signal flips before any interval tick
        // fires, the runner should still exit (within ~one tick
        // window) without waiting forever.
        let (_store, shard) = make_runner_shard();
        let cfg = RunnerConfig {
            poll_interval: std::time::Duration::from_secs(60),
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let runner = DrReconcilerRunner::new(shard, cfg);
        let handle = tokio::spawn(runner.run(shutdown_rx));

        // Yield to let the loop reach select!.
        tokio::task::yield_now().await;
        // Now signal shutdown.
        shutdown_tx.send(true).unwrap();
        // First tick of an interval fires at construction time
        // (immediately). Advance microscopically so the runner
        // reaches its select! and observes the shutdown.
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        let stats = handle.await.unwrap();
        // The immediate-first-tick property means we may get 1 tick
        // before shutdown wins the select; anything more would
        // indicate the shutdown wasn't observed.
        assert!(
            stats.successful_ticks <= 1,
            "expected ≤1 tick, got {}",
            stats.successful_ticks
        );
    }

    #[tokio::test(start_paused = true)]
    async fn runner_continues_loop_after_tick_failure() {
        // The store returns an error from pending_reconcile on the
        // first call; subsequent calls succeed. The runner must log
        // and continue, not exit on the failure.
        let (store, shard) = make_runner_shard();
        store.inject_error(DrApiError::StoreUnavailable("test injected".into()));
        let cfg = RunnerConfig {
            poll_interval: std::time::Duration::from_millis(50),
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let runner = DrReconcilerRunner::new(shard, cfg);
        let handle = tokio::spawn(runner.run(shutdown_rx));

        // Let several intervals pass — first will fail, rest succeed
        // (the error injection is one-shot per MockDrPolicyStore
        // semantics).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        shutdown_tx.send(true).unwrap();
        let stats = handle.await.unwrap();
        assert_eq!(stats.failed_ticks, 1, "exactly one error injected");
        assert!(
            stats.successful_ticks >= 2,
            "loop continued after the error: {} ticks",
            stats.successful_ticks
        );
    }

    #[tokio::test(start_paused = true)]
    async fn runner_handles_sender_drop_as_shutdown() {
        // Dropping the watch sender should terminate the loop
        // (tokio::sync::watch::Receiver::changed returns Err once
        // all senders are dropped).
        let (_store, shard) = make_runner_shard();
        let cfg = RunnerConfig {
            poll_interval: std::time::Duration::from_secs(60),
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let runner = DrReconcilerRunner::new(shard, cfg);
        let handle = tokio::spawn(runner.run(shutdown_rx));

        tokio::task::yield_now().await;
        drop(shutdown_tx);
        // Tiny advance so the runner's select! has a chance to wake.
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        let stats = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("runner exited within timeout")
            .unwrap();
        // At most one immediate tick before shutdown won the select.
        assert!(stats.successful_ticks <= 1);
    }

    // --- observe_lag --------------------------------------------------

    #[tokio::test]
    async fn shard_records_observed_lag_when_present() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        // Seed an observation that reports 42s of lag.
        let mut obs = healthy_observed(&p);
        obs.observed_lag_seconds = Some(42);
        adapter.seed_observed(&p.policy_id, obs);
        store.seed_pending(vec![p.clone()]);
        let clock = Arc::new(AtomicI64::new(0));
        let metrics = RecordingDrMetrics::new();
        let shard = make_shard(store.clone(), adapter, BackoffPolicy::default(), clock, 0.5)
            .with_metrics(metrics.clone());

        let _ = shard.tick().await.unwrap();
        let lags = metrics.lags();
        assert_eq!(lags.len(), 1, "exactly one lag observation");
        assert_eq!(lags[0].1, 42, "lag value passed through");
        assert_eq!(lags[0].0.tenant_id, "tnt_acme");
        assert_eq!(lags[0].0.provider, ObjectProvider::AwsS3);
    }

    #[tokio::test]
    async fn shard_skips_lag_when_observation_has_none() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        let mut obs = healthy_observed(&p);
        obs.observed_lag_seconds = None;
        adapter.seed_observed(&p.policy_id, obs);
        store.seed_pending(vec![p.clone()]);
        let clock = Arc::new(AtomicI64::new(0));
        let metrics = RecordingDrMetrics::new();
        let shard = make_shard(store.clone(), adapter, BackoffPolicy::default(), clock, 0.5)
            .with_metrics(metrics.clone());

        let _ = shard.tick().await.unwrap();
        assert!(metrics.lags().is_empty(), "no lag observation");
    }

    #[tokio::test]
    async fn driver_with_observation_returns_state_alongside_outcome() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        let mut p = base_policy();
        p.provider_binding = Some(ProviderReplicationBinding {
            provider_policy_id: None,
            provider_rule_id: "dr-drp_1-v1".into(),
            provider_role_arn: None,
            provider_kms_key_id: None,
        });
        let mut obs = healthy_observed(&p);
        obs.observed_lag_seconds = Some(7);
        adapter.seed_observed(&p.policy_id, obs);
        let driver = make_driver(store.clone(), adapter);

        let (outcome, observation) = driver.reconcile_one_with_observation(&p).await.unwrap();
        assert!(matches!(
            outcome,
            ReconcileOutcome::Idle(IdleReason::HealthyActive)
        ));
        let observation = observation.expect("fetch_state succeeded");
        assert_eq!(observation.observed_lag_seconds, Some(7));
    }

    #[tokio::test]
    async fn driver_with_observation_returns_none_on_fetch_failure() {
        let store = MockDrPolicyStore::new(1);
        let adapter = Arc::new(MockDrProviderAdapter::new());
        adapter.inject_error(ProviderError::Transient("blip".into()));
        let driver = make_driver(store.clone(), adapter);

        let p = base_policy();
        let (outcome, observation) = driver.reconcile_one_with_observation(&p).await.unwrap();
        assert!(matches!(outcome, ReconcileOutcome::AdapterTransient(_)));
        assert!(observation.is_none(), "no observation when fetch failed");
    }
}
