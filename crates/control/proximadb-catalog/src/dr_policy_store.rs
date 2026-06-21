//! Customer-facing DR policy mutation surface — contract §S14.
//!
//! The contract puts every DR policy mutation behind a single trait:
//! customers and operator code only mutate through
//! [`CollectionDrPolicyStore`]. Any code path that touches the
//! `xcatalog_collection_dr_policies` table directly is treated as a
//! bug — the trait is the only sanctioned surface.
//!
//! This module ships:
//! - The request types (`CreatePolicyRequest`, `BillingApproval`,
//!   `RetirementAck`).
//! - The async trait itself.
//! - An `InMemoryCollectionDrPolicyStore` reference implementation
//!   used in tests and embedded-mode operator deployments. Production
//!   wires sqlx- or filestore-backed impls.
//!
//! The in-memory impl enforces every contract gate:
//! - D8: refuses creation for `ExternalAuthoritative` collections.
//! - S13: refuses unsupported providers (e.g., `GcsFuture`).
//! - Namespace must carry `region_home` (per G4 region-home routing).
//! - State-machine transitions go through
//!   [`crate::collection_dr_policy::DrState::can_transition_to`].
//! - Retirement requires explicit replication-stop and retention-cost
//!   acknowledgements.
//! - One policy per `(tenant_id, namespace_id, collection_id)`.

use crate::CatalogAuthorityMode;
use crate::collection_dr_policy::{
    CollectionDrPolicy, DrBillingBinding, DrHealth, DrPlacement, DrReplicationBehavior, DrState,
    ObjectProvider,
};
use crate::dr_reconciler::DrApiError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Inputs to [`CollectionDrPolicyStore::create`]. The caller must
/// provide the collection's `CatalogAuthorityMode` and the owning
/// namespace's `region_home` because the engine refuses to look those
/// up — the resolution paths live in the runtime layer above the
/// catalog crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePolicyRequest {
    pub tenant_id: String,
    pub namespace_id: String,
    pub collection_id: String,
    pub tier: String,
    pub provider: ObjectProvider,
    pub source_region: String,
    pub destination_region: String,
    pub region_pair_id: String,
    pub placement: DrPlacement,
    pub replication: DrReplicationBehavior,
    pub billing: DrBillingBinding,
    pub requested_by: String,
    /// The collection's xCatalog authority mode. The store refuses
    /// creation for `ExternalAuthoritative` per D8 — those
    /// collections do not own their commits; replication is the
    /// table owner's problem.
    pub collection_authority_mode: CatalogAuthorityMode,
    /// The owning namespace's `region_home`. Must be `Some` —
    /// per the contract's "Region Home Routing" rule, a DR policy
    /// cannot be created against a namespace whose region home is
    /// unset.
    pub namespace_region_home: Option<String>,
}

/// Inputs to [`CollectionDrPolicyStore::approve_billing`]. The
/// operator carries the customer's explicit acknowledgement of the
/// estimated cost so the audit trail records who approved what.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingApproval {
    pub accepted_by: String,
    pub accepted_approval_id: String,
    pub accepted_estimate_cents: Option<i64>,
}

/// Inputs to [`CollectionDrPolicyStore::request_retire`]. Both
/// acknowledgement booleans must be `true` — the contract requires
/// the customer (or ops on the customer's behalf) to explicitly
/// confirm both the replication stop and the destination-retention
/// cost before the engine accepts the retirement request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetirementAck {
    pub requested_by: String,
    pub acknowledge_replication_stop: bool,
    pub acknowledge_destination_retention_cost: bool,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// The single sanctioned mutation surface for collection DR policies
/// per contract §"Engine API Surface" (S14). Concrete impls translate
/// to sqlx (Postgres/MySQL/SQLite) or filestore writes; tests use the
/// in-memory impl shipped below.
#[async_trait]
pub trait CollectionDrPolicyStore: Send + Sync {
    /// Create a new policy in `PendingBillingApproval`. Refuses:
    /// - `ExternalAuthoritative` collections (D8).
    /// - Unsupported providers (S13).
    /// - Empty tenant/namespace/collection IDs.
    /// - A namespace whose `region_home` is `None`.
    /// - Empty billing SKU or cost-owner.
    /// - Duplicate `(tenant_id, namespace_id, collection_id)`.
    async fn create(&self, req: CreatePolicyRequest) -> Result<CollectionDrPolicy, DrApiError>;

    /// Move from `PendingBillingApproval` to
    /// `PendingProviderProvisioning`. Requires the approval to carry
    /// a non-empty `accepted_approval_id`; the engine treats it as
    /// opaque (the operator's billing system supplies the value).
    async fn approve_billing(
        &self,
        policy_id: &str,
        approval: BillingApproval,
    ) -> Result<CollectionDrPolicy, DrApiError>;

    /// Customer-initiated retirement. Both ack booleans must be
    /// `true`; otherwise the store returns
    /// `DrApiError::ValidationFailed`.
    async fn request_retire(
        &self,
        policy_id: &str,
        ack: RetirementAck,
    ) -> Result<CollectionDrPolicy, DrApiError>;

    /// Ops-only emergency suspend. Caller is expected to have the
    /// operator service-account capability; the engine does not
    /// authorize here — that's the operator's customer-facing layer
    /// job. Returns `InvalidStateTransition` if not in `Active`.
    async fn suspend(
        &self,
        policy_id: &str,
        reason: &str,
    ) -> Result<CollectionDrPolicy, DrApiError>;

    /// Ops-only resume from suspension. Returns
    /// `InvalidStateTransition` if not in `SuspendedByOps`.
    async fn resume(&self, policy_id: &str) -> Result<CollectionDrPolicy, DrApiError>;

    /// Read-only fetch by primary key.
    async fn get(&self, policy_id: &str) -> Result<Option<CollectionDrPolicy>, DrApiError>;

    /// All policies owned by a tenant.
    async fn list_by_tenant(&self, tenant_id: &str) -> Result<Vec<CollectionDrPolicy>, DrApiError>;

    /// The policy for a specific `(tenant, collection)` if any.
    /// Returns `None` if the collection has no policy. The trait
    /// asserts at most one policy per collection via the
    /// `(tenant_id, namespace_id, collection_id)` uniqueness rule
    /// from §"LLD: xCatalog Schema".
    async fn list_by_collection(
        &self,
        tenant_id: &str,
        collection_id: &str,
    ) -> Result<Option<CollectionDrPolicy>, DrApiError>;
}

// ---------------------------------------------------------------------------
// In-memory reference implementation
// ---------------------------------------------------------------------------

/// In-memory `CollectionDrPolicyStore`. Used in:
/// - Tests across catalog crate consumers.
/// - Embedded-mode deployments that don't run a database.
/// - The operator's wiring tests before plugging in a real backend.
///
/// Thread-safe via a single `parking_lot::Mutex`. Internal cardinality
/// (policies-per-shard) is low enough that a single mutex is fine;
/// production sqlx backends use the database for concurrency control.
///
/// Clock and ID source are injectable so tests run deterministically.
pub struct InMemoryCollectionDrPolicyStore {
    inner: parking_lot::Mutex<InMemoryState>,
    now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
    id_source: Arc<dyn Fn() -> String + Send + Sync>,
}

#[derive(Default)]
struct InMemoryState {
    /// policy_id → policy
    policies: std::collections::HashMap<String, CollectionDrPolicy>,
    /// (tenant_id, namespace_id, collection_id) → policy_id; the
    /// uniqueness key from §"LLD: xCatalog Schema".
    by_collection: std::collections::HashMap<(String, String, String), String>,
}

impl InMemoryCollectionDrPolicyStore {
    /// Build a store with system-clock-derived IDs and timestamps.
    pub fn new() -> Self {
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter_clone = counter.clone();
        let id_source: Arc<dyn Fn() -> String + Send + Sync> = Arc::new(move || {
            let n = counter_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("drp_{n:020}")
        });
        let now_ns: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0)
        });
        Self {
            inner: parking_lot::Mutex::new(InMemoryState::default()),
            now_ns,
            id_source,
        }
    }

    /// Build a store with explicit clock + ID source. Tests use this
    /// for deterministic policy IDs and timestamps.
    pub fn with_clocks(
        now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
        id_source: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Self {
        Self {
            inner: parking_lot::Mutex::new(InMemoryState::default()),
            now_ns,
            id_source,
        }
    }
}

impl Default for InMemoryCollectionDrPolicyStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CollectionDrPolicyStore for InMemoryCollectionDrPolicyStore {
    async fn create(&self, req: CreatePolicyRequest) -> Result<CollectionDrPolicy, DrApiError> {
        // D8: refuse ExternalAuthoritative.
        if req.collection_authority_mode.is_external_authoritative() {
            return Err(DrApiError::ExternalAuthoritativeRefused(format!(
                "collection {}/{}/{}",
                req.tenant_id, req.namespace_id, req.collection_id
            )));
        }
        // S13: refuse providers we don't ship adapters for.
        if matches!(req.provider, ObjectProvider::GcsFuture) {
            return Err(DrApiError::UnsupportedProvider(format!(
                "{:?}",
                req.provider
            )));
        }
        // ID validation.
        if req.tenant_id.is_empty() {
            return Err(DrApiError::ValidationFailed("tenant_id is empty".into()));
        }
        if req.namespace_id.is_empty() {
            return Err(DrApiError::ValidationFailed("namespace_id is empty".into()));
        }
        if req.collection_id.is_empty() {
            return Err(DrApiError::ValidationFailed(
                "collection_id is empty".into(),
            ));
        }
        // G4: namespace must have region_home set.
        if req.namespace_region_home.is_none() {
            return Err(DrApiError::ValidationFailed(
                "namespace has no region_home; \
                 cannot create DR policy until operator sets it"
                    .into(),
            ));
        }
        // Billing: SKU + cost owner must be set at create time.
        if req.billing.cost_binding_ref.is_empty() {
            return Err(DrApiError::ValidationFailed(
                "billing.cost_binding_ref is empty".into(),
            ));
        }
        if req.billing.cost_owner_tenant_id.is_empty() {
            return Err(DrApiError::ValidationFailed(
                "billing.cost_owner_tenant_id is empty".into(),
            ));
        }
        // Region pair must be non-empty for the metric label.
        if req.region_pair_id.is_empty() {
            return Err(DrApiError::ValidationFailed(
                "region_pair_id is empty".into(),
            ));
        }

        let mut state = self.inner.lock();
        let key = (
            req.tenant_id.clone(),
            req.namespace_id.clone(),
            req.collection_id.clone(),
        );
        if state.by_collection.contains_key(&key) {
            return Err(DrApiError::ValidationFailed(format!(
                "policy already exists for {}/{}/{}",
                req.tenant_id, req.namespace_id, req.collection_id
            )));
        }

        let now = (self.now_ns)();
        let policy_id = (self.id_source)();
        let policy = CollectionDrPolicy {
            policy_id: policy_id.clone(),
            tenant_id: req.tenant_id.clone(),
            namespace_id: req.namespace_id.clone(),
            collection_id: req.collection_id.clone(),
            tier: req.tier,
            state: DrState::PendingBillingApproval,
            provider: req.provider,
            source_region: req.source_region,
            destination_region: req.destination_region,
            region_pair_id: req.region_pair_id,
            placement: req.placement,
            replication: req.replication,
            billing: req.billing,
            provider_binding: None,
            health: DrHealth::default(),
            requested_by: req.requested_by,
            approved_by: None,
            created_at_ns: now,
            updated_at_ns: now,
            policy_version: 1,
        };
        state.by_collection.insert(key, policy_id.clone());
        state.policies.insert(policy_id, policy.clone());
        Ok(policy)
    }

    async fn approve_billing(
        &self,
        policy_id: &str,
        approval: BillingApproval,
    ) -> Result<CollectionDrPolicy, DrApiError> {
        if approval.accepted_approval_id.is_empty() {
            return Err(DrApiError::ValidationFailed(
                "accepted_approval_id is empty".into(),
            ));
        }
        if approval.accepted_by.is_empty() {
            return Err(DrApiError::ValidationFailed("accepted_by is empty".into()));
        }
        let mut state = self.inner.lock();
        let policy = state
            .policies
            .get_mut(policy_id)
            .ok_or_else(|| DrApiError::ValidationFailed(format!("unknown policy {policy_id}")))?;
        let next = DrState::PendingProviderProvisioning;
        if !policy.state.can_transition_to(next) {
            return Err(DrApiError::InvalidStateTransition {
                from: policy.state,
                to: next,
            });
        }
        policy.billing.billing_approval_id = Some(approval.accepted_approval_id);
        policy.billing.operator_estimate_cents = approval.accepted_estimate_cents;
        policy.approved_by = Some(approval.accepted_by);
        policy.state = next;
        policy.policy_version = policy.policy_version.saturating_add(1);
        policy.updated_at_ns = (self.now_ns)();
        Ok(policy.clone())
    }

    async fn request_retire(
        &self,
        policy_id: &str,
        ack: RetirementAck,
    ) -> Result<CollectionDrPolicy, DrApiError> {
        if !ack.acknowledge_replication_stop {
            return Err(DrApiError::ValidationFailed(
                "acknowledge_replication_stop must be true".into(),
            ));
        }
        if !ack.acknowledge_destination_retention_cost {
            return Err(DrApiError::ValidationFailed(
                "acknowledge_destination_retention_cost must be true".into(),
            ));
        }
        if ack.requested_by.is_empty() {
            return Err(DrApiError::ValidationFailed("requested_by is empty".into()));
        }
        let mut state = self.inner.lock();
        let policy = state
            .policies
            .get_mut(policy_id)
            .ok_or_else(|| DrApiError::ValidationFailed(format!("unknown policy {policy_id}")))?;
        let next = DrState::PendingRetirement;
        if !policy.state.can_transition_to(next) {
            return Err(DrApiError::InvalidStateTransition {
                from: policy.state,
                to: next,
            });
        }
        policy.state = next;
        policy.policy_version = policy.policy_version.saturating_add(1);
        policy.updated_at_ns = (self.now_ns)();
        Ok(policy.clone())
    }

    async fn suspend(
        &self,
        policy_id: &str,
        _reason: &str,
    ) -> Result<CollectionDrPolicy, DrApiError> {
        let mut state = self.inner.lock();
        let policy = state
            .policies
            .get_mut(policy_id)
            .ok_or_else(|| DrApiError::ValidationFailed(format!("unknown policy {policy_id}")))?;
        let next = DrState::SuspendedByOps;
        if !policy.state.can_transition_to(next) {
            return Err(DrApiError::InvalidStateTransition {
                from: policy.state,
                to: next,
            });
        }
        policy.state = next;
        policy.policy_version = policy.policy_version.saturating_add(1);
        policy.updated_at_ns = (self.now_ns)();
        Ok(policy.clone())
    }

    async fn resume(&self, policy_id: &str) -> Result<CollectionDrPolicy, DrApiError> {
        let mut state = self.inner.lock();
        let policy = state
            .policies
            .get_mut(policy_id)
            .ok_or_else(|| DrApiError::ValidationFailed(format!("unknown policy {policy_id}")))?;
        let next = DrState::Active;
        if !policy.state.can_transition_to(next) {
            return Err(DrApiError::InvalidStateTransition {
                from: policy.state,
                to: next,
            });
        }
        policy.state = next;
        policy.policy_version = policy.policy_version.saturating_add(1);
        policy.updated_at_ns = (self.now_ns)();
        Ok(policy.clone())
    }

    async fn get(&self, policy_id: &str) -> Result<Option<CollectionDrPolicy>, DrApiError> {
        Ok(self.inner.lock().policies.get(policy_id).cloned())
    }

    async fn list_by_tenant(&self, tenant_id: &str) -> Result<Vec<CollectionDrPolicy>, DrApiError> {
        let state = self.inner.lock();
        Ok(state
            .policies
            .values()
            .filter(|p| p.tenant_id == tenant_id)
            .cloned()
            .collect())
    }

    async fn list_by_collection(
        &self,
        tenant_id: &str,
        collection_id: &str,
    ) -> Result<Option<CollectionDrPolicy>, DrApiError> {
        let state = self.inner.lock();
        Ok(state
            .policies
            .values()
            .find(|p| p.tenant_id == tenant_id && p.collection_id == collection_id)
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoragePoolClass;

    fn make_store() -> InMemoryCollectionDrPolicyStore {
        // Deterministic clocks for predictable assertions.
        let now_counter = Arc::new(std::sync::atomic::AtomicI64::new(1_700_000_000_000_000_000));
        let now_clone = now_counter.clone();
        let now_ns: Arc<dyn Fn() -> i64 + Send + Sync> =
            Arc::new(move || now_clone.fetch_add(1_000, std::sync::atomic::Ordering::Relaxed));
        let id_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let id_clone = id_counter.clone();
        let id_source: Arc<dyn Fn() -> String + Send + Sync> = Arc::new(move || {
            let n = id_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("drp_test_{n:04}")
        });
        InMemoryCollectionDrPolicyStore::with_clocks(now_ns, id_source)
    }

    fn valid_request() -> CreatePolicyRequest {
        CreatePolicyRequest {
            tenant_id: "tnt_acme".into(),
            namespace_id: "ns_1".into(),
            collection_id: "col_orders".into(),
            tier: "business".into(),
            provider: ObjectProvider::AwsS3,
            source_region: "us-east-1".into(),
            destination_region: "us-west-2".into(),
            region_pair_id: "aws:us-east-1:us-west-2".into(),
            placement: DrPlacement {
                source_pool_class: StoragePoolClass::Standard,
                destination_pool_class: StoragePoolClass::Standard,
                source_bucket_or_account: "src".into(),
                destination_bucket_or_account: "dst".into(),
                source_container: None,
                destination_container: None,
                source_prefix: "data/tnt_acme/ns_1/col_orders/".into(),
                destination_prefix: "data/tnt_acme/ns_1/col_orders/".into(),
            },
            replication: DrReplicationBehavior::default(),
            billing: DrBillingBinding {
                cost_binding_ref: "dr-standard-binding".into(),
                cost_owner_tenant_id: "tnt_acme".into(),
                billing_approval_id: None,
                operator_estimate_cents: None,
            },
            requested_by: "user_1".into(),
            collection_authority_mode: CatalogAuthorityMode::InternalCanonical,
            namespace_region_home: Some("us-east-1".into()),
        }
    }

    // -- create ---------------------------------------------------------

    #[tokio::test]
    async fn create_valid_request_lands_in_pending_billing_approval() {
        let store = make_store();
        let policy = store.create(valid_request()).await.unwrap();
        assert_eq!(policy.state, DrState::PendingBillingApproval);
        assert_eq!(policy.policy_id, "drp_test_0000");
        assert_eq!(policy.policy_version, 1);
        assert!(policy.provider_binding.is_none());
        // Round-trip via get / list.
        let by_id = store.get(&policy.policy_id).await.unwrap().unwrap();
        assert_eq!(by_id.policy_id, policy.policy_id);
        let by_collection = store
            .list_by_collection("tnt_acme", "col_orders")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_collection.policy_id, policy.policy_id);
        let by_tenant = store.list_by_tenant("tnt_acme").await.unwrap();
        assert_eq!(by_tenant.len(), 1);
    }

    #[tokio::test]
    async fn create_refuses_external_authoritative_collection() {
        let store = make_store();
        let mut req = valid_request();
        req.collection_authority_mode = CatalogAuthorityMode::ExternalAuthoritative;
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, DrApiError::ExternalAuthoritativeRefused(_)));
    }

    #[tokio::test]
    async fn create_refuses_unsupported_provider() {
        let store = make_store();
        let mut req = valid_request();
        req.provider = ObjectProvider::GcsFuture;
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, DrApiError::UnsupportedProvider(_)));
    }

    #[tokio::test]
    async fn create_refuses_empty_ids() {
        let store = make_store();
        for clear in ["tenant_id", "namespace_id", "collection_id"] {
            let mut req = valid_request();
            match clear {
                "tenant_id" => req.tenant_id = String::new(),
                "namespace_id" => req.namespace_id = String::new(),
                "collection_id" => req.collection_id = String::new(),
                _ => unreachable!(),
            }
            let err = store.create(req).await.unwrap_err();
            assert!(matches!(err, DrApiError::ValidationFailed(_)), "{clear}");
        }
    }

    #[tokio::test]
    async fn create_refuses_namespace_without_region_home() {
        let store = make_store();
        let mut req = valid_request();
        req.namespace_region_home = None;
        let err = store.create(req).await.unwrap_err();
        match err {
            DrApiError::ValidationFailed(msg) => assert!(msg.contains("region_home")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_refuses_missing_billing_fields() {
        let store = make_store();
        let mut req = valid_request();
        req.billing.cost_binding_ref = String::new();
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, DrApiError::ValidationFailed(_)));

        let mut req = valid_request();
        req.billing.cost_owner_tenant_id = String::new();
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, DrApiError::ValidationFailed(_)));
    }

    #[tokio::test]
    async fn create_refuses_empty_region_pair_id() {
        let store = make_store();
        let mut req = valid_request();
        req.region_pair_id = String::new();
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, DrApiError::ValidationFailed(_)));
    }

    #[tokio::test]
    async fn create_refuses_duplicate_collection_policy() {
        let store = make_store();
        store.create(valid_request()).await.unwrap();
        let err = store.create(valid_request()).await.unwrap_err();
        assert!(matches!(err, DrApiError::ValidationFailed(_)));
    }

    // -- approve_billing -----------------------------------------------

    #[tokio::test]
    async fn approve_billing_drives_to_pending_provider_provisioning() {
        let store = make_store();
        let p = store.create(valid_request()).await.unwrap();
        let approval = BillingApproval {
            accepted_by: "user_2".into(),
            accepted_approval_id: "appr_001".into(),
            accepted_estimate_cents: Some(18_400),
        };
        let p2 = store.approve_billing(&p.policy_id, approval).await.unwrap();
        assert_eq!(p2.state, DrState::PendingProviderProvisioning);
        assert_eq!(p2.billing.billing_approval_id.as_deref(), Some("appr_001"));
        assert_eq!(p2.billing.operator_estimate_cents, Some(18_400));
        assert_eq!(p2.approved_by.as_deref(), Some("user_2"));
        // Version bumps on transition.
        assert_eq!(p2.policy_version, p.policy_version + 1);
    }

    #[tokio::test]
    async fn approve_billing_refuses_empty_approval_id() {
        let store = make_store();
        let p = store.create(valid_request()).await.unwrap();
        let approval = BillingApproval {
            accepted_by: "u".into(),
            accepted_approval_id: String::new(),
            accepted_estimate_cents: None,
        };
        let err = store
            .approve_billing(&p.policy_id, approval)
            .await
            .unwrap_err();
        assert!(matches!(err, DrApiError::ValidationFailed(_)));
    }

    #[tokio::test]
    async fn approve_billing_refuses_unknown_policy() {
        let store = make_store();
        let approval = BillingApproval {
            accepted_by: "u".into(),
            accepted_approval_id: "appr".into(),
            accepted_estimate_cents: None,
        };
        let err = store
            .approve_billing("drp_nope", approval)
            .await
            .unwrap_err();
        assert!(matches!(err, DrApiError::ValidationFailed(_)));
    }

    #[tokio::test]
    async fn approve_billing_refuses_wrong_state() {
        // After billing already approved, re-approving fails because
        // PendingProviderProvisioning -> PendingProviderProvisioning
        // is not a valid transition.
        let store = make_store();
        let p = store.create(valid_request()).await.unwrap();
        let approval = BillingApproval {
            accepted_by: "u".into(),
            accepted_approval_id: "a".into(),
            accepted_estimate_cents: None,
        };
        store
            .approve_billing(&p.policy_id, approval.clone())
            .await
            .unwrap();
        let err = store
            .approve_billing(&p.policy_id, approval)
            .await
            .unwrap_err();
        assert!(matches!(err, DrApiError::InvalidStateTransition { .. }));
    }

    // -- suspend / resume ----------------------------------------------

    async fn drive_to_active(store: &InMemoryCollectionDrPolicyStore) -> CollectionDrPolicy {
        let p = store.create(valid_request()).await.unwrap();
        let approval = BillingApproval {
            accepted_by: "u".into(),
            accepted_approval_id: "a".into(),
            accepted_estimate_cents: None,
        };
        let p = store.approve_billing(&p.policy_id, approval).await.unwrap();
        // Mock the reconciler's transition to Active by going through
        // a separate `transition_state` call. The customer-facing
        // store doesn't expose that — only the reconciler does — so
        // we cheat for test setup by suspending+resuming after the
        // reconciler would have moved it. Simpler path: drop down
        // and mutate the policy directly via the inner mutex.
        {
            let mut state = store.inner.lock();
            let row = state.policies.get_mut(&p.policy_id).unwrap();
            row.state = DrState::Active;
        }
        p
    }

    #[tokio::test]
    async fn suspend_from_active_succeeds() {
        let store = make_store();
        let p = drive_to_active(&store).await;
        let p2 = store.suspend(&p.policy_id, "oncall test").await.unwrap();
        assert_eq!(p2.state, DrState::SuspendedByOps);
    }

    #[tokio::test]
    async fn resume_from_suspended_returns_to_active() {
        let store = make_store();
        let p = drive_to_active(&store).await;
        store.suspend(&p.policy_id, "test").await.unwrap();
        let p2 = store.resume(&p.policy_id).await.unwrap();
        assert_eq!(p2.state, DrState::Active);
    }

    #[tokio::test]
    async fn suspend_refuses_non_active_state() {
        let store = make_store();
        let p = store.create(valid_request()).await.unwrap();
        let err = store.suspend(&p.policy_id, "test").await.unwrap_err();
        assert!(matches!(err, DrApiError::InvalidStateTransition { .. }));
    }

    #[tokio::test]
    async fn resume_refuses_non_suspended_state() {
        let store = make_store();
        let p = drive_to_active(&store).await;
        let err = store.resume(&p.policy_id).await.unwrap_err();
        assert!(matches!(err, DrApiError::InvalidStateTransition { .. }));
    }

    // -- request_retire ------------------------------------------------

    #[tokio::test]
    async fn request_retire_from_active_with_both_acks() {
        let store = make_store();
        let p = drive_to_active(&store).await;
        let ack = RetirementAck {
            requested_by: "u".into(),
            acknowledge_replication_stop: true,
            acknowledge_destination_retention_cost: true,
        };
        let p2 = store.request_retire(&p.policy_id, ack).await.unwrap();
        assert_eq!(p2.state, DrState::PendingRetirement);
    }

    #[tokio::test]
    async fn request_retire_refuses_missing_replication_ack() {
        let store = make_store();
        let p = drive_to_active(&store).await;
        let ack = RetirementAck {
            requested_by: "u".into(),
            acknowledge_replication_stop: false,
            acknowledge_destination_retention_cost: true,
        };
        let err = store.request_retire(&p.policy_id, ack).await.unwrap_err();
        assert!(matches!(err, DrApiError::ValidationFailed(_)));
    }

    #[tokio::test]
    async fn request_retire_refuses_missing_retention_ack() {
        let store = make_store();
        let p = drive_to_active(&store).await;
        let ack = RetirementAck {
            requested_by: "u".into(),
            acknowledge_replication_stop: true,
            acknowledge_destination_retention_cost: false,
        };
        let err = store.request_retire(&p.policy_id, ack).await.unwrap_err();
        assert!(matches!(err, DrApiError::ValidationFailed(_)));
    }

    #[tokio::test]
    async fn request_retire_refuses_pending_billing_state() {
        // PendingBillingApproval -> PendingRetirement is not a valid
        // transition; customer must approve billing first OR cancel
        // explicitly (different code path).
        let store = make_store();
        let p = store.create(valid_request()).await.unwrap();
        let ack = RetirementAck {
            requested_by: "u".into(),
            acknowledge_replication_stop: true,
            acknowledge_destination_retention_cost: true,
        };
        let err = store.request_retire(&p.policy_id, ack).await.unwrap_err();
        assert!(matches!(err, DrApiError::InvalidStateTransition { .. }));
    }

    // -- get / list ----------------------------------------------------

    #[tokio::test]
    async fn get_returns_none_for_unknown_policy() {
        let store = make_store();
        assert!(store.get("drp_nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_by_tenant_returns_empty_when_none() {
        let store = make_store();
        assert!(store.list_by_tenant("tnt_nope").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_by_collection_returns_none_when_missing() {
        let store = make_store();
        assert!(
            store
                .list_by_collection("tnt_acme", "col_nope")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn list_by_tenant_filters_correctly() {
        let store = make_store();
        store.create(valid_request()).await.unwrap();
        let mut req = valid_request();
        req.tenant_id = "tnt_other".into();
        req.cost_owner_override();
        store.create(req).await.unwrap();
        let acme = store.list_by_tenant("tnt_acme").await.unwrap();
        let other = store.list_by_tenant("tnt_other").await.unwrap();
        assert_eq!(acme.len(), 1);
        assert_eq!(other.len(), 1);
        assert_eq!(acme[0].tenant_id, "tnt_acme");
        assert_eq!(other[0].tenant_id, "tnt_other");
    }

    impl CreatePolicyRequest {
        // Test helper — used by `list_by_tenant_filters_correctly` to
        // align billing fields with a switched tenant_id.
        fn cost_owner_override(&mut self) {
            self.billing.cost_owner_tenant_id = self.tenant_id.clone();
        }
    }
}
