//! Collection-level DR / CRR policy schema — engine contract P1.
//!
//! Mirrors the SQL DDL and Rust types locked in
//! `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc`. The catalog
//! persists one row per `(tenant_id, namespace_id, collection_id)` tuple
//! in `xcatalog_collection_dr_policies` plus an append-only event log in
//! `xcatalog_collection_dr_events`.
//!
//! Layering: this module is contract-only. It owns the types, the DDL
//! constants, and the `ProviderError` taxonomy. The catalog backend
//! (sqlx or filestore) translates rows to/from these structs. The DR
//! reconciler (P3) drives state transitions; the provider adapter (P4)
//! turns policies into provider rules.
//!
//! Authority: `CollectionDrPolicy` is the metadata authority for DR.
//! Provider rules are rebuildable projections of that policy; if
//! provider state diverges, xCatalog wins.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// DDL constants
// ---------------------------------------------------------------------------

/// DDL string locked by the engine contract. Catalog backends (Postgres,
/// MySQL, SQLite, filestore-emulated) all materialize the same shape so
/// reconcilers and operators see one schema regardless of backend choice.
///
/// The `policy_version BIGINT` is signed in most SQL dialects but is
/// treated as unsigned by the engine; values stay well below `i64::MAX`
/// in practice (it counts provider-rule-changing edits per policy).
pub const COLLECTION_DR_POLICIES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS xcatalog_collection_dr_policies (
    policy_id                       TEXT PRIMARY KEY,
    tenant_id                       TEXT NOT NULL,
    namespace_id                    TEXT NOT NULL,
    collection_id                   TEXT NOT NULL,
    tier                            TEXT NOT NULL,

    state                           TEXT NOT NULL,
    provider                        TEXT NOT NULL,
    source_region                   TEXT NOT NULL,
    destination_region              TEXT NOT NULL,
    region_pair_id                  TEXT NOT NULL,

    source_pool_class               TEXT NOT NULL,
    destination_pool_class          TEXT NOT NULL,
    source_bucket_or_account        TEXT NOT NULL,
    destination_bucket_or_account   TEXT NOT NULL,
    source_container                TEXT,
    destination_container           TEXT,
    source_prefix                   TEXT NOT NULL,
    destination_prefix              TEXT NOT NULL,

    rpo_target_seconds              INTEGER NOT NULL,
    replicate_existing_objects      BOOLEAN NOT NULL DEFAULT FALSE,
    replicate_deletes               BOOLEAN NOT NULL DEFAULT FALSE,
    destination_retention_days      INTEGER NOT NULL,

    cost_binding_ref                     TEXT NOT NULL,
    cost_owner_tenant_id            TEXT NOT NULL,
    billing_approval_id             TEXT,
    operator_estimate_cents    BIGINT,

    provider_policy_id              TEXT,
    provider_rule_id                TEXT,
    provider_role_arn               TEXT,
    provider_kms_key_id             TEXT,

    last_reconciled_at_ns           BIGINT,
    last_health_state               TEXT NOT NULL DEFAULT 'unknown',
    last_health_reason              TEXT,
    last_provider_lag_seconds       INTEGER,

    requested_by                    TEXT NOT NULL,
    approved_by                     TEXT,
    created_at_ns                   BIGINT NOT NULL,
    updated_at_ns                   BIGINT NOT NULL,
    policy_version                  BIGINT NOT NULL,

    UNIQUE (tenant_id, namespace_id, collection_id),
    UNIQUE (provider, source_bucket_or_account, source_container, source_prefix)
);
"#;

/// Append-only event log for DR policy state transitions and reconciler
/// actions. Every state change, provider call, drift detection, and
/// blocked-delete attempt writes a row.
pub const COLLECTION_DR_EVENTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS xcatalog_collection_dr_events (
    event_id        TEXT PRIMARY KEY,
    policy_id       TEXT NOT NULL,
    tenant_id       TEXT NOT NULL,
    collection_id   TEXT NOT NULL,
    event_type      TEXT NOT NULL,
    actor           TEXT NOT NULL,
    reason          TEXT,
    before_state    TEXT,
    after_state     TEXT,
    provider_state  TEXT,
    created_at_ns   BIGINT NOT NULL
);
"#;

// ---------------------------------------------------------------------------
// Enums — DrState, ObjectProvider, DrHealthState, StoragePoolClass alias
// ---------------------------------------------------------------------------

/// Policy intent — what the customer/operator says should be true. State
/// transitions are monotonic for normal flows
/// (`Disabled → PendingBillingApproval → PendingProviderProvisioning →
/// Active → PendingRetirement → Retired`); the engine refuses invalid
/// transitions like `Active → Disabled`.
///
/// Orthogonal to [`DrHealthState`] (what the reconciler last observed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrState {
    Disabled,
    PendingBillingApproval,
    PendingProviderProvisioning,
    Active,
    SuspendedByOps,
    PendingRetirement,
    Retired,
}

impl DrState {
    /// The state machine permitted transitions per the engine contract.
    /// Returns `true` when `self → next` is allowed. Invalid transitions
    /// are rejected at the catalog layer so the reconciler never sees a
    /// stranded provider state.
    pub fn can_transition_to(self, next: DrState) -> bool {
        use DrState::*;
        matches!(
            (self, next),
            (Disabled, PendingBillingApproval)
                | (PendingBillingApproval, PendingProviderProvisioning)
                | (PendingBillingApproval, Disabled) // cancel before provider
                | (PendingProviderProvisioning, Active)
                | (PendingProviderProvisioning, Disabled) // provisioning failed, no rule exists
                | (Active, SuspendedByOps)
                | (SuspendedByOps, Active)
                | (Active, PendingRetirement)
                | (SuspendedByOps, PendingRetirement)
                | (PendingRetirement, Retired)
        )
    }
}

/// Object-storage provider backing a DR policy. The engine treats each
/// provider abstractly via [`crate::collection_dr_policy::ProviderError`]
/// and an external adapter trait; concrete rule shapes live in the
/// operator layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectProvider {
    AwsS3,
    AzureBlob,
    /// Azure Data Lake Storage Gen2 source accounts with hierarchical
    /// namespace enabled. Native Azure object replication is not
    /// available; routes to a ProximaDB-managed worker adapter.
    AzureAdlsHns,
    /// Reserved for a future GCS adapter. Not implemented at contract
    /// v1; the engine API returns `UnsupportedProvider` if used.
    GcsFuture,
}

/// Reconciler observation — what the provider rule actually looks like
/// vs the policy intent. Updated by the reconciler only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DrHealthState {
    #[default]
    Unknown,
    Healthy,
    /// Lag rising but still inside RPO.
    Degraded,
    /// Provider state diverged from policy; reconciler will repair.
    Drifted,
    /// Provider rule exists without an active billing binding, or
    /// approval was revoked. No further provider calls until cleared.
    BillingBlocked,
    /// Provider returned `Misconfiguration`, `QuotaExceeded`, or
    /// `AuthDenied`. Pages ops; no auto-retry.
    ProviderBlocked,
}

// ---------------------------------------------------------------------------
// Policy struct family
// ---------------------------------------------------------------------------

/// The full DR policy row materialized in xCatalog. Operator code edits
/// this through the [`CollectionDrPolicyStore`] trait (P3); the
/// reconciler reads it and projects state to provider rules.
///
/// `tier` is stored as a `String` because the catalog crate sits below
/// the root crate where `Tier` (`src/catalog/tenant_tier.rs`) lives;
/// callers serialize their tier name in. Recommended values match
/// `Tier::as_str()` exactly (`"free_trial"`, `"team"`, `"pro"`,
/// `"business"`, `"enterprise"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionDrPolicy {
    pub policy_id: String,
    pub tenant_id: String,
    pub namespace_id: String,
    pub collection_id: String,
    pub tier: String,

    pub state: DrState,
    pub provider: ObjectProvider,
    pub source_region: String,
    pub destination_region: String,
    /// Opaque operator-curated region pair id. Recommended canonical
    /// form `{provider}:{source_region}:{destination_region}`, e.g.
    /// `aws:us-east-1:us-west-2`. Engine treats it as opaque and uses
    /// it only as a metric label.
    pub region_pair_id: String,

    pub placement: DrPlacement,
    pub replication: DrReplicationBehavior,
    pub billing: DrBillingBinding,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_binding: Option<ProviderReplicationBinding>,
    pub health: DrHealth,

    pub requested_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    pub created_at_ns: i64,
    pub updated_at_ns: i64,
    /// Optimistic-concurrency counter. Bumps on every change that
    /// requires a provider adapter call (prefix changes, retention
    /// changes that touch provider, KMS key change, state transition);
    /// does NOT bump on observation-only changes (RPO target, observed
    /// lag, billing approval id by itself).
    pub policy_version: u64,
}

/// Source and destination object-storage coordinates for the policy.
/// The path resolver guarantees `source_prefix` is exactly
/// `data/{tenant_id}/{namespace_id}/{collection_id}/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrPlacement {
    pub source_pool_class: crate::StoragePoolClass,
    pub destination_pool_class: crate::StoragePoolClass,
    pub source_bucket_or_account: String,
    pub destination_bucket_or_account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_container: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_container: Option<String>,
    pub source_prefix: String,
    pub destination_prefix: String,
}

/// Replication behaviour knobs. Conservative defaults: do not backfill
/// existing objects (expensive one-time charge), do not mirror deletes
/// (destination retention drives lifecycle), 35-day destination
/// retention as a safe placeholder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrReplicationBehavior {
    pub rpo_target_seconds: u32,
    #[serde(default)]
    pub replicate_existing_objects: bool,
    #[serde(default)]
    pub replicate_deletes: bool,
    pub destination_retention_days: u32,
}

impl Default for DrReplicationBehavior {
    fn default() -> Self {
        Self {
            rpo_target_seconds: 900, // 15 minutes
            replicate_existing_objects: false,
            replicate_deletes: false,
            destination_retention_days: 35,
        }
    }
}

/// Opaque operator cost/governance binding the engine requires before
/// provisioning a provider rule. The engine treats these as pass-through values
/// (no pricing logic in OSS — chargeback/billing is the operator's concern):
/// `cost_binding_ref` and `cost_owner_tenant_id` are mandatory at row creation;
/// `approval_id` must be set before transitioning out of `PendingApproval`.
/// serde aliases preserve the pre-neutralization wire names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrBillingBinding {
    #[serde(alias = "billing_sku")]
    pub cost_binding_ref: String,
    pub cost_owner_tenant_id: String,
    #[serde(default, alias = "billing_approval_id", skip_serializing_if = "Option::is_none")]
    pub billing_approval_id: Option<String>,
    #[serde(
        default,
        alias = "estimated_monthly_cost_cents",
        skip_serializing_if = "Option::is_none"
    )]
    pub operator_estimate_cents: Option<i64>,
}

impl DrBillingBinding {
    /// Is the billing binding strong enough to leave
    /// `PendingBillingApproval`?
    pub fn is_approved(&self) -> bool {
        self.billing_approval_id.is_some()
    }
}

/// What the provider returned when we last issued
/// `ensure_rule`/`fetch_state`. Populated by the provider adapter (P4)
/// via the reconciler; `None` means no provider call has succeeded yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderReplicationBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_policy_id: Option<String>,
    pub provider_rule_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_role_arn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kms_key_id: Option<String>,
}

/// Reconciler observation. `state` is observation; `reason` is a
/// short human-readable hint surfaced through the engine API and
/// metrics labels.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DrHealth {
    #[serde(default)]
    pub state: DrHealthState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reconciled_at_ns: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_provider_lag_seconds: Option<u32>,
}

// ---------------------------------------------------------------------------
// Event log
// ---------------------------------------------------------------------------

/// Event types written to `xcatalog_collection_dr_events`. Stored as
/// snake_case strings; mirrors the contract event list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrEventType {
    Requested,
    BillingApproved,
    ProviderProvisionStarted,
    ProviderRuleCreated,
    Active,
    DriftDetected,
    DriftRepaired,
    BillingBlocked,
    RetirementRequested,
    ProviderRuleDisabled,
    Retired,
}

/// Append-only row in `xcatalog_collection_dr_events`. Every state
/// transition, provider call, and blocked delete attempt writes one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionDrEvent {
    pub event_id: String,
    pub policy_id: String,
    pub tenant_id: String,
    pub collection_id: String,
    pub event_type: DrEventType,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_state: Option<DrState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_state: Option<DrState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_state: Option<String>,
    pub created_at_ns: i64,
}

// ---------------------------------------------------------------------------
// Provider error taxonomy
// ---------------------------------------------------------------------------

/// Errors returned by the provider adapter (P4). Four canonical kinds
/// so the reconciler can decide retry vs escalate without parsing
/// provider-specific strings.
///
/// Retry policy (reconciler):
/// - `Transient` → exponential backoff (default 30s → 30m, jittered),
///   max 12 attempts before treating as `ProviderBlocked`.
/// - `Misconfiguration`, `QuotaExceeded`, `AuthDenied` → no retry until
///   a human ack or follow-up xCatalog event clears the block.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// Transient provider failure — network blip, 5xx, throttling that
    /// the SDK already retried past. Reconciler retries with backoff.
    /// Does not change `DrHealthState`.
    #[error("transient provider error: {0}")]
    Transient(String),

    /// Provider rejected the call because preconditions are wrong
    /// (bucket missing, versioning disabled, KMS key unusable, IAM
    /// policy invalid). Reconciler marks `ProviderBlocked` and pages
    /// ops. No further provider calls until human action.
    #[error("provider misconfiguration: {0}")]
    Misconfiguration(String),

    /// Provider account hit a hard limit (S3 replication rule count,
    /// Azure object replication policy count, request-rate ceiling).
    /// Reconciler marks `ProviderBlocked` and alerts ops with the
    /// limit name.
    #[error("provider quota exceeded: {0}")]
    QuotaExceeded(String),

    /// Provider returned 401/403 / role assumption failed. Reconciler
    /// marks `ProviderBlocked` and pages immediately because this
    /// usually means a credential rotation is mid-flight or an IAM
    /// policy regressed.
    #[error("provider auth denied: {0}")]
    AuthDenied(String),
}

impl ProviderError {
    /// Should the reconciler retry this error with backoff?
    pub fn is_retryable(&self) -> bool {
        matches!(self, ProviderError::Transient(_))
    }

    /// Does this error require human intervention before further
    /// provider calls?
    pub fn requires_ops_ack(&self) -> bool {
        matches!(
            self,
            ProviderError::Misconfiguration(_)
                | ProviderError::QuotaExceeded(_)
                | ProviderError::AuthDenied(_)
        )
    }
}

// ---------------------------------------------------------------------------
// Provider adapter contract (P4)
// ---------------------------------------------------------------------------

/// Provider observation snapshot returned by
/// [`DrProviderAdapter::fetch_state`]. The reconciler diffs this against
/// the policy's intent to detect drift, mark `DrHealth`, and decide
/// repair vs escalate.
///
/// All fields except `rule_exists` are `Option` because providers vary
/// in what they report; absent values mean "could not observe", not
/// "observed as missing".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderObservedState {
    /// True iff a provider rule keyed on this policy currently exists.
    pub rule_exists: bool,
    /// Observed prefix filter on the provider rule. Drift if it differs
    /// from `policy.placement.source_prefix`.
    pub observed_prefix: Option<String>,
    /// Observed destination bucket/account on the provider rule. Drift
    /// if it differs from `policy.placement.destination_bucket_or_account`.
    pub observed_destination: Option<String>,
    /// Observed destination container (Azure) or `None` (S3 — bucket-only).
    pub observed_destination_container: Option<String>,
    /// True iff the provider rule status is "enabled" / "active".
    pub rule_enabled: bool,
    /// True iff source-side prerequisites (S3 versioning, Azure change
    /// feed) are still active. Drift if false.
    pub source_versioning_enabled: bool,
    /// True iff the destination bucket/container rejects application
    /// writes. Drift if false.
    pub destination_write_protected: bool,
    /// Last lag observed by the provider, in seconds. `None` if
    /// unknown.
    pub observed_lag_seconds: Option<u32>,
    /// Opaque provider-side rule ID. Cross-correlates with the catalog's
    /// `ProviderReplicationBinding::provider_rule_id`.
    pub provider_rule_id: Option<String>,
    /// Optional KMS key ID the provider rule is using. Drift if it
    /// differs from `policy.provider_binding.provider_kms_key_id`.
    pub provider_kms_key_id: Option<String>,
}

/// Provider adapter trait — engine surface only. The reconciler (P3)
/// calls this; concrete adapters (S3, Azure Blob, ADLS HNS worker)
/// live in the operator layer.
///
/// All methods are async because real provider SDKs (`aws-sdk-s3`,
/// `azure_storage_blobs`) are async-only; making the trait async avoids
/// `block_on` smuggling inside the reconciler.
///
/// Idempotency contract:
/// - `ensure_rule` must be safe to retry against the same
///   `(policy_id, policy_version)`. Calling it twice with the same
///   policy version must converge to the same provider binding, not
///   create a duplicate rule.
/// - `retire_rule` must be safe to retry; calling it after the rule is
///   already disabled returns `Ok(())`.
///
/// See `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc` "LLD:
/// Provider Adapter Contract".
#[async_trait::async_trait]
pub trait DrProviderAdapter: Send + Sync {
    /// Stable identifier for logs and metric labels, e.g. `"aws_s3"`,
    /// `"azure_blob"`, `"azure_adls_hns"`, `"mock"`.
    fn name(&self) -> &'static str;

    /// Create or update the provider rule to match `policy`. Returns
    /// the binding the reconciler should persist on the catalog row.
    /// Must be idempotent against `(policy_id, policy_version)`.
    async fn ensure_rule(
        &self,
        policy: &CollectionDrPolicy,
    ) -> Result<ProviderReplicationBinding, ProviderError>;

    /// Fetch current provider state. Read-only — never mutates.
    async fn fetch_state(
        &self,
        policy: &CollectionDrPolicy,
    ) -> Result<ProviderObservedState, ProviderError>;

    /// Disable and tombstone the provider rule. Idempotent. Returns
    /// `Ok(())` even if no rule exists.
    async fn retire_rule(&self, policy: &CollectionDrPolicy) -> Result<(), ProviderError>;
}

// ---------------------------------------------------------------------------
// Reference test-double adapter (P4)
// ---------------------------------------------------------------------------

/// In-memory reference `DrProviderAdapter` for tests. Simulates the
/// idempotent provider-rule lifecycle without any provider SDK access.
///
/// Use in:
/// - Reconciler tests in this crate (P3, when it lands).
/// - Integration tests in dependent crates.
/// - Operator-side wiring tests to validate engine plumbing before the
///   real S3/Azure adapter is plugged in.
///
/// Supports error injection so tests can drive the full retry/escalate
/// matrix.
pub struct MockDrProviderAdapter {
    inner: parking_lot::Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    /// Provider-side rules keyed by `(policy_id, policy_version)` so
    /// retry against the same version returns the same binding.
    rules: std::collections::HashMap<String, ProviderReplicationBinding>,
    /// Observed states keyed by `policy_id` (whatever the adapter
    /// would report on the next `fetch_state` for that policy).
    observed: std::collections::HashMap<String, ProviderObservedState>,
    /// Injected error consumed by the next call. `None` means normal
    /// operation.
    next_error: Option<ProviderError>,
    /// Counter of `ensure_rule` calls — lets tests verify idempotency.
    ensure_calls: usize,
    /// Counter of `retire_rule` calls.
    retire_calls: usize,
    /// Counter of `fetch_state` calls.
    fetch_calls: usize,
}

impl MockDrProviderAdapter {
    /// Build an empty mock adapter. No rules, no observed state, no
    /// injected errors.
    pub fn new() -> Self {
        Self {
            inner: parking_lot::Mutex::new(MockState::default()),
        }
    }

    /// Inject an error to be returned by the next adapter call. Cleared
    /// after one consumption. Useful for exercising the reconciler's
    /// retry vs escalate decision.
    pub fn inject_error(&self, err: ProviderError) {
        self.inner.lock().next_error = Some(err);
    }

    /// Pre-seed the observed state for a policy so `fetch_state` can
    /// return a synthetic drift scenario.
    pub fn seed_observed(&self, policy_id: &str, state: ProviderObservedState) {
        self.inner
            .lock()
            .observed
            .insert(policy_id.to_string(), state);
    }

    /// Number of times `ensure_rule` has been called.
    pub fn ensure_call_count(&self) -> usize {
        self.inner.lock().ensure_calls
    }

    /// Number of times `retire_rule` has been called.
    pub fn retire_call_count(&self) -> usize {
        self.inner.lock().retire_calls
    }

    /// Number of times `fetch_state` has been called.
    pub fn fetch_call_count(&self) -> usize {
        self.inner.lock().fetch_calls
    }

    /// Key used internally for idempotency: `(policy_id, policy_version)`.
    fn idem_key(policy: &CollectionDrPolicy) -> String {
        format!("{}@{}", policy.policy_id, policy.policy_version)
    }
}

impl Default for MockDrProviderAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl DrProviderAdapter for MockDrProviderAdapter {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn ensure_rule(
        &self,
        policy: &CollectionDrPolicy,
    ) -> Result<ProviderReplicationBinding, ProviderError> {
        let mut state = self.inner.lock();
        state.ensure_calls += 1;
        if let Some(err) = state.next_error.take() {
            return Err(err);
        }
        let key = Self::idem_key(policy);
        // Idempotency: same (policy_id, policy_version) returns the
        // same binding rather than creating a new one. Build the
        // binding eagerly then clone it so we can release the
        // `rules` borrow before touching `observed`.
        let binding = state
            .rules
            .entry(key.clone())
            .or_insert_with(|| ProviderReplicationBinding {
                provider_policy_id: Some(format!("mock_policy_{key}")),
                provider_rule_id: format!("dr-{}-v{}", policy.policy_id, policy.policy_version),
                provider_role_arn: None,
                provider_kms_key_id: policy
                    .provider_binding
                    .as_ref()
                    .and_then(|b| b.provider_kms_key_id.clone()),
            })
            .clone();
        // Reflect the rule into observed state so the next fetch_state
        // call sees it.
        state.observed.insert(
            policy.policy_id.clone(),
            ProviderObservedState {
                rule_exists: true,
                observed_prefix: Some(policy.placement.source_prefix.clone()),
                observed_destination: Some(policy.placement.destination_bucket_or_account.clone()),
                observed_destination_container: policy.placement.destination_container.clone(),
                rule_enabled: true,
                source_versioning_enabled: true,
                destination_write_protected: true,
                observed_lag_seconds: Some(0),
                provider_rule_id: Some(binding.provider_rule_id.clone()),
                provider_kms_key_id: binding.provider_kms_key_id.clone(),
            },
        );
        Ok(binding)
    }

    async fn fetch_state(
        &self,
        policy: &CollectionDrPolicy,
    ) -> Result<ProviderObservedState, ProviderError> {
        let mut state = self.inner.lock();
        state.fetch_calls += 1;
        if let Some(err) = state.next_error.take() {
            return Err(err);
        }
        Ok(state
            .observed
            .get(&policy.policy_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn retire_rule(&self, policy: &CollectionDrPolicy) -> Result<(), ProviderError> {
        let mut state = self.inner.lock();
        state.retire_calls += 1;
        if let Some(err) = state.next_error.take() {
            return Err(err);
        }
        // Remove all bindings for this policy_id regardless of version.
        state
            .rules
            .retain(|key, _| !key.starts_with(&format!("{}@", policy.policy_id)));
        // Reflect retirement in observed state.
        state.observed.insert(
            policy.policy_id.clone(),
            ProviderObservedState {
                rule_exists: false,
                ..ProviderObservedState::default()
            },
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoragePoolClass;

    // --- DDL constants -----------------------------------------------------

    #[test]
    fn policies_ddl_mentions_every_locked_column() {
        let ddl = COLLECTION_DR_POLICIES_DDL;
        // Sanity-check that the table name and every column referenced in
        // the contract appears in the DDL string. If the contract grows a
        // new required field, this test catches a doc-vs-DDL drift.
        for col in [
            "xcatalog_collection_dr_policies",
            "policy_id",
            "tenant_id",
            "namespace_id",
            "collection_id",
            "tier",
            "state",
            "provider",
            "source_region",
            "destination_region",
            "region_pair_id",
            "source_pool_class",
            "destination_pool_class",
            "source_bucket_or_account",
            "destination_bucket_or_account",
            "source_container",
            "destination_container",
            "source_prefix",
            "destination_prefix",
            "rpo_target_seconds",
            "replicate_existing_objects",
            "replicate_deletes",
            "destination_retention_days",
            "cost_binding_ref",
            "cost_owner_tenant_id",
            "billing_approval_id",
            "operator_estimate_cents",
            "provider_policy_id",
            "provider_rule_id",
            "provider_role_arn",
            "provider_kms_key_id",
            "last_reconciled_at_ns",
            "last_health_state",
            "last_health_reason",
            "last_provider_lag_seconds",
            "requested_by",
            "approved_by",
            "created_at_ns",
            "updated_at_ns",
            "policy_version",
        ] {
            assert!(
                ddl.contains(col),
                "policies DDL is missing locked column `{col}`"
            );
        }
    }

    #[test]
    fn events_ddl_mentions_every_locked_column() {
        let ddl = COLLECTION_DR_EVENTS_DDL;
        for col in [
            "xcatalog_collection_dr_events",
            "event_id",
            "policy_id",
            "tenant_id",
            "collection_id",
            "event_type",
            "actor",
            "reason",
            "before_state",
            "after_state",
            "provider_state",
            "created_at_ns",
        ] {
            assert!(
                ddl.contains(col),
                "events DDL is missing locked column `{col}`"
            );
        }
    }

    // --- State machine -----------------------------------------------------

    #[test]
    fn state_machine_allows_full_enable_path() {
        use DrState::*;
        assert!(Disabled.can_transition_to(PendingBillingApproval));
        assert!(PendingBillingApproval.can_transition_to(PendingProviderProvisioning));
        assert!(PendingProviderProvisioning.can_transition_to(Active));
        assert!(Active.can_transition_to(PendingRetirement));
        assert!(PendingRetirement.can_transition_to(Retired));
    }

    #[test]
    fn state_machine_allows_ops_suspend_cycle() {
        use DrState::*;
        assert!(Active.can_transition_to(SuspendedByOps));
        assert!(SuspendedByOps.can_transition_to(Active));
        assert!(SuspendedByOps.can_transition_to(PendingRetirement));
    }

    #[test]
    fn state_machine_allows_cancel_before_provider_rule_exists() {
        use DrState::*;
        // Cancel paths only work while no provider rule exists.
        assert!(PendingBillingApproval.can_transition_to(Disabled));
        assert!(PendingProviderProvisioning.can_transition_to(Disabled));
    }

    #[test]
    fn state_machine_rejects_active_to_disabled() {
        // The whole point of monotonic retirement: customers cannot
        // toggle DR off with a single API call.
        assert!(!DrState::Active.can_transition_to(DrState::Disabled));
    }

    #[test]
    fn state_machine_rejects_retired_to_active() {
        // Retired is terminal — re-enabling requires a new policy.
        for next in [
            DrState::Disabled,
            DrState::PendingBillingApproval,
            DrState::PendingProviderProvisioning,
            DrState::Active,
            DrState::SuspendedByOps,
            DrState::PendingRetirement,
        ] {
            assert!(
                !DrState::Retired.can_transition_to(next),
                "Retired must not transition to {next:?}"
            );
        }
    }

    #[test]
    fn state_machine_rejects_suspended_to_disabled() {
        // SuspendedByOps must go through retirement; it cannot snap
        // back to Disabled, which would strand the provider rule.
        assert!(!DrState::SuspendedByOps.can_transition_to(DrState::Disabled));
        assert!(!DrState::SuspendedByOps.can_transition_to(DrState::Retired));
    }

    // --- Serde -------------------------------------------------------------

    #[test]
    fn dr_state_serde_uses_snake_case() {
        let pairs = [
            (DrState::Disabled, "\"disabled\""),
            (
                DrState::PendingBillingApproval,
                "\"pending_billing_approval\"",
            ),
            (
                DrState::PendingProviderProvisioning,
                "\"pending_provider_provisioning\"",
            ),
            (DrState::Active, "\"active\""),
            (DrState::SuspendedByOps, "\"suspended_by_ops\""),
            (DrState::PendingRetirement, "\"pending_retirement\""),
            (DrState::Retired, "\"retired\""),
        ];
        for (variant, expected) in pairs {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s, expected, "variant {variant:?}");
            let back: DrState = serde_json::from_str(expected).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn object_provider_serde_uses_snake_case() {
        let pairs = [
            (ObjectProvider::AwsS3, "\"aws_s3\""),
            (ObjectProvider::AzureBlob, "\"azure_blob\""),
            (ObjectProvider::AzureAdlsHns, "\"azure_adls_hns\""),
            (ObjectProvider::GcsFuture, "\"gcs_future\""),
        ];
        for (variant, expected) in pairs {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s, expected, "variant {variant:?}");
        }
    }

    #[test]
    fn dr_health_state_default_is_unknown() {
        assert_eq!(DrHealthState::default(), DrHealthState::Unknown);
    }

    #[test]
    fn replication_behavior_defaults_match_contract() {
        let r = DrReplicationBehavior::default();
        assert_eq!(r.rpo_target_seconds, 900);
        assert!(!r.replicate_existing_objects);
        assert!(!r.replicate_deletes);
        assert_eq!(r.destination_retention_days, 35);
    }

    // --- Policy round-trip -------------------------------------------------

    fn sample_policy() -> CollectionDrPolicy {
        CollectionDrPolicy {
            policy_id: "drp_01HX7Q8K2N5R9P3M1B2C3D4E5F".into(),
            tenant_id: "tnt_acme".into(),
            namespace_id: "ns_01HX7Q8K2N5R9P3M1B2C3D4E5G".into(),
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
                source_bucket_or_account: "proximadb-prod-us-east-1-business-data".into(),
                destination_bucket_or_account: "proximadb-prod-us-west-2-business-dr".into(),
                source_container: None,
                destination_container: None,
                source_prefix: "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5G/col_orders/".into(),
                destination_prefix: "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5G/col_orders/"
                    .into(),
            },
            replication: DrReplicationBehavior::default(),
            billing: DrBillingBinding {
                cost_binding_ref: "dr-standard-binding".into(),
                cost_owner_tenant_id: "tnt_acme".into(),
                billing_approval_id: Some("appr_01HX...".into()),
                operator_estimate_cents: Some(18_400),
            },
            provider_binding: Some(ProviderReplicationBinding {
                provider_policy_id: None,
                provider_rule_id: "dr-tnt_acme-ns-col_orders-v1".into(),
                provider_role_arn: Some(
                    "arn:aws:iam::123456789012:role/proximadb-dr-replication".into(),
                ),
                provider_kms_key_id: Some("arn:aws:kms:us-east-1:123456789012:key/abc".into()),
            }),
            health: DrHealth {
                state: DrHealthState::Healthy,
                reason: None,
                last_reconciled_at_ns: Some(1_700_000_000_000_000_000),
                last_provider_lag_seconds: Some(120),
            },
            requested_by: "user_123".into(),
            approved_by: Some("user_456".into()),
            created_at_ns: 1_700_000_000_000_000_000,
            updated_at_ns: 1_700_000_000_000_000_000,
            policy_version: 1,
        }
    }

    #[test]
    fn policy_round_trips_through_json() {
        let p = sample_policy();
        let json = serde_json::to_string(&p).unwrap();
        let back: CollectionDrPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.policy_id, p.policy_id);
        assert_eq!(back.tenant_id, p.tenant_id);
        assert_eq!(back.state, DrState::Active);
        assert_eq!(back.provider, ObjectProvider::AwsS3);
        assert_eq!(back.tier, "business");
        assert_eq!(back.health.state, DrHealthState::Healthy);
        assert_eq!(back.policy_version, 1);
    }

    #[test]
    fn billing_binding_is_approved_requires_approval_id() {
        let mut b = DrBillingBinding {
            cost_binding_ref: "dr-standard-binding".into(),
            cost_owner_tenant_id: "tnt_acme".into(),
            billing_approval_id: None,
            operator_estimate_cents: None,
        };
        assert!(!b.is_approved());
        b.billing_approval_id = Some("appr_x".into());
        assert!(b.is_approved());
    }

    // --- Event log ---------------------------------------------------------

    #[test]
    fn event_round_trips_with_optional_fields_omitted() {
        let e = CollectionDrEvent {
            event_id: "evt_1".into(),
            policy_id: "drp_1".into(),
            tenant_id: "tnt_acme".into(),
            collection_id: "col_orders".into(),
            event_type: DrEventType::DriftDetected,
            actor: "reconciler".into(),
            reason: None,
            before_state: Some(DrState::Active),
            after_state: Some(DrState::Active),
            provider_state: None,
            created_at_ns: 1_700_000_000_000_000_000,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("\"reason\""));
        assert!(!json.contains("\"provider_state\""));
        let back: CollectionDrEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, DrEventType::DriftDetected);
        assert_eq!(back.before_state, Some(DrState::Active));
    }

    #[test]
    fn event_type_serde_uses_snake_case() {
        let pairs = [
            (DrEventType::Requested, "\"requested\""),
            (DrEventType::BillingApproved, "\"billing_approved\""),
            (
                DrEventType::ProviderProvisionStarted,
                "\"provider_provision_started\"",
            ),
            (
                DrEventType::ProviderRuleCreated,
                "\"provider_rule_created\"",
            ),
            (DrEventType::Active, "\"active\""),
            (DrEventType::DriftDetected, "\"drift_detected\""),
            (DrEventType::DriftRepaired, "\"drift_repaired\""),
            (DrEventType::BillingBlocked, "\"billing_blocked\""),
            (DrEventType::RetirementRequested, "\"retirement_requested\""),
            (
                DrEventType::ProviderRuleDisabled,
                "\"provider_rule_disabled\"",
            ),
            (DrEventType::Retired, "\"retired\""),
        ];
        for (variant, expected) in pairs {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s, expected, "variant {variant:?}");
        }
    }

    // --- ProviderError -----------------------------------------------------

    #[test]
    fn provider_error_classifies_retry_vs_escalate() {
        let transient = ProviderError::Transient("5xx".into());
        let misconfig = ProviderError::Misconfiguration("bucket missing".into());
        let quota = ProviderError::QuotaExceeded("rule cap".into());
        let auth = ProviderError::AuthDenied("403".into());

        assert!(transient.is_retryable());
        assert!(!misconfig.is_retryable());
        assert!(!quota.is_retryable());
        assert!(!auth.is_retryable());

        assert!(!transient.requires_ops_ack());
        assert!(misconfig.requires_ops_ack());
        assert!(quota.requires_ops_ack());
        assert!(auth.requires_ops_ack());
    }

    // --- DrProviderAdapter / MockDrProviderAdapter -------------------------

    #[tokio::test]
    async fn mock_adapter_name_is_stable() {
        let m = MockDrProviderAdapter::new();
        assert_eq!(m.name(), "mock");
    }

    #[tokio::test]
    async fn mock_ensure_rule_creates_binding_and_observable_state() {
        let m = MockDrProviderAdapter::new();
        let p = sample_policy();

        let binding = m.ensure_rule(&p).await.unwrap();
        assert!(binding.provider_rule_id.contains(&p.policy_id));
        assert_eq!(m.ensure_call_count(), 1);

        // The rule is now visible to fetch_state.
        let state = m.fetch_state(&p).await.unwrap();
        assert!(state.rule_exists);
        assert!(state.rule_enabled);
        assert_eq!(
            state.observed_prefix.as_deref(),
            Some(p.placement.source_prefix.as_str())
        );
        assert_eq!(
            state.observed_destination.as_deref(),
            Some(p.placement.destination_bucket_or_account.as_str()),
        );
        assert!(state.source_versioning_enabled);
        assert!(state.destination_write_protected);
    }

    #[tokio::test]
    async fn mock_ensure_rule_is_idempotent_against_policy_version() {
        let m = MockDrProviderAdapter::new();
        let p = sample_policy();

        let b1 = m.ensure_rule(&p).await.unwrap();
        let b2 = m.ensure_rule(&p).await.unwrap();
        let b3 = m.ensure_rule(&p).await.unwrap();

        // Same policy_version → same binding, no duplication.
        assert_eq!(b1.provider_rule_id, b2.provider_rule_id);
        assert_eq!(b2.provider_rule_id, b3.provider_rule_id);
        assert_eq!(b1.provider_policy_id, b2.provider_policy_id);
        assert_eq!(m.ensure_call_count(), 3);
    }

    #[tokio::test]
    async fn mock_ensure_rule_bumps_provider_rule_id_when_policy_version_changes() {
        let m = MockDrProviderAdapter::new();
        let mut p = sample_policy();

        let b_v1 = m.ensure_rule(&p).await.unwrap();
        // S2: policy_version bumps for changes that touch provider rule
        // (destination prefix change). The adapter must produce a new
        // rule for the new version.
        p.policy_version = 2;
        let b_v2 = m.ensure_rule(&p).await.unwrap();

        assert_ne!(b_v1.provider_rule_id, b_v2.provider_rule_id);
        assert!(b_v2.provider_rule_id.ends_with("-v2"));
    }

    #[tokio::test]
    async fn mock_retire_rule_removes_binding_and_marks_observed_missing() {
        let m = MockDrProviderAdapter::new();
        let p = sample_policy();

        m.ensure_rule(&p).await.unwrap();
        assert!(m.fetch_state(&p).await.unwrap().rule_exists);

        m.retire_rule(&p).await.unwrap();
        let state = m.fetch_state(&p).await.unwrap();
        assert!(!state.rule_exists);
        assert!(!state.rule_enabled);
        assert_eq!(m.retire_call_count(), 1);
    }

    #[tokio::test]
    async fn mock_retire_rule_is_idempotent() {
        let m = MockDrProviderAdapter::new();
        let p = sample_policy();

        // Retiring before ensure must still be Ok per the contract.
        m.retire_rule(&p).await.unwrap();
        m.retire_rule(&p).await.unwrap();
        m.retire_rule(&p).await.unwrap();
        assert_eq!(m.retire_call_count(), 3);

        let state = m.fetch_state(&p).await.unwrap();
        assert!(!state.rule_exists);
    }

    #[tokio::test]
    async fn mock_fetch_state_on_unknown_policy_returns_default() {
        let m = MockDrProviderAdapter::new();
        let p = sample_policy();
        let state = m.fetch_state(&p).await.unwrap();
        // Default observed state: rule_exists = false, everything else
        // bottomed out. This is what the reconciler sees the first time
        // it polls a brand-new policy before ensure_rule runs.
        assert!(!state.rule_exists);
        assert!(!state.rule_enabled);
        assert_eq!(state.observed_prefix, None);
        assert_eq!(m.fetch_call_count(), 1);
    }

    #[tokio::test]
    async fn mock_error_injection_returns_then_clears() {
        let m = MockDrProviderAdapter::new();
        let p = sample_policy();

        m.inject_error(ProviderError::Transient("flake".into()));
        let err = m.ensure_rule(&p).await.unwrap_err();
        assert!(matches!(err, ProviderError::Transient(_)));
        assert!(err.is_retryable());

        // Error is consumed — the next call succeeds.
        let binding = m.ensure_rule(&p).await.unwrap();
        assert!(!binding.provider_rule_id.is_empty());
    }

    #[tokio::test]
    async fn mock_error_injection_covers_full_taxonomy() {
        let m = MockDrProviderAdapter::new();
        let p = sample_policy();

        for err in [
            ProviderError::Transient("5xx".into()),
            ProviderError::Misconfiguration("no bucket".into()),
            ProviderError::QuotaExceeded("rule cap".into()),
            ProviderError::AuthDenied("403".into()),
        ] {
            m.inject_error(err.clone());
            let got = m.ensure_rule(&p).await.unwrap_err();
            assert_eq!(got, err);
        }
    }

    #[tokio::test]
    async fn mock_seed_observed_drives_drift_scenarios() {
        // Pre-seed an observed state that doesn't match the policy.
        // Lets reconciler tests assert drift detection without going
        // through a full ensure_rule first.
        let m = MockDrProviderAdapter::new();
        let p = sample_policy();
        let drift = ProviderObservedState {
            rule_exists: true,
            observed_prefix: Some("data/wrong/prefix/".into()),
            observed_destination: Some("wrong-bucket".into()),
            rule_enabled: true,
            source_versioning_enabled: false, // <-- drift!
            destination_write_protected: true,
            ..Default::default()
        };
        m.seed_observed(&p.policy_id, drift.clone());

        let got = m.fetch_state(&p).await.unwrap();
        assert_eq!(got, drift);
    }

    #[tokio::test]
    async fn mock_adapter_is_object_safe_via_trait_object() {
        // The reconciler will hold the adapter as `Arc<dyn DrProviderAdapter>`.
        // This test pins object safety so a future trait change can't
        // accidentally break dynamic dispatch.
        let mock: std::sync::Arc<dyn DrProviderAdapter> =
            std::sync::Arc::new(MockDrProviderAdapter::new());
        assert_eq!(mock.name(), "mock");
        let p = sample_policy();
        let binding = mock.ensure_rule(&p).await.unwrap();
        assert!(!binding.provider_rule_id.is_empty());
    }
}
