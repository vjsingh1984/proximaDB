//! DR restore-readiness primitives — engine contract P5.
//!
//! The engine answers one question: "given this DR policy and a
//! target LSN, what would block a successful restore right now?".
//! Failover orchestration (the operator-layer runbook) calls this
//! checker before promoting the DR region; quarterly drills call it
//! to validate RPO.
//!
//! Layering: this module is the abstract surface. Concrete bridges
//! sit outside the catalog crate:
//!
//! - The manifest source — wraps `GlobalManifestService` in the
//!   root crate (`src/storage/persistence/write_ahead_log/manifest/`)
//!   to produce `ManifestEntryRef` rows. The catalog crate cannot
//!   depend on the root crate, so the bridge lives upstream.
//! - The destination object-presence check — uses the provider
//!   adapter trait from [`crate::collection_dr_policy`].
//! - The KMS accessibility check — uses the provider adapter's
//!   binding observation.
//!
//! See `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc`
//! §"LLD: Restore Primitives".

use crate::collection_dr_policy::{CollectionDrPolicy, ProviderError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Manifest anchor types
// ---------------------------------------------------------------------------

/// Status reported by a manifest entry. Mirrors the catalog-side
/// projection of `GlobalManifestEntry::status` so the restore
/// checker can be written without depending on the root crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestEntryStatus {
    /// Batch is currently being written (not durable yet).
    Active,
    /// Batch is durably written and visible to readers.
    Flushed,
    /// Batch has been compacted away; the file referenced may have
    /// been deleted from the source.
    Archived,
    /// Batch was rolled back; its file must not be replayed.
    RolledBack,
}

impl ManifestEntryStatus {
    /// Can this entry contribute to a restore? Active rows are not
    /// yet durable; RolledBack rows must be skipped; Flushed and
    /// Archived rows are valid restore inputs.
    pub fn is_restorable(self) -> bool {
        matches!(
            self,
            ManifestEntryStatus::Flushed | ManifestEntryStatus::Archived
        )
    }
}

/// Engine-side projection of a single manifest row. The root-crate
/// bridge translates from `GlobalManifestEntry` to this type so the
/// restore checker can live in the catalog crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntryRef {
    /// Monotonic LSN. The restore checker walks entries up to
    /// `target_lsn` inclusive.
    pub global_lsn: u64,
    /// Collection this entry belongs to. Must match the policy.
    pub collection_id: String,
    /// Identifier of the batch this entry committed. Matches the
    /// base62-encoded `BatchId` the engine emits in
    /// `GlobalManifestEntry::batch_id`; treated as opaque here.
    pub batch_id: String,
    /// Storage object key the batch lives at. The destination
    /// presence check uses this verbatim.
    pub file_path: String,
    /// Optional storage URL (e.g. `s3://bucket/key`). Set when the
    /// engine knows the full provider URI; readers prefer
    /// `file_path` for the relative key.
    pub storage_url: Option<String>,
    pub status: ManifestEntryStatus,
    /// Optional checkpoint identifier. Multiple entries may share
    /// a checkpoint. Matches `GlobalManifestEntry::checkpoint_id`.
    pub checkpoint_id: Option<u64>,
    /// Source-side commit time (millis since epoch). RPO observation
    /// uses this; see contract §"RPO Observation".
    pub timestamp_ms: i64,
}

// ---------------------------------------------------------------------------
// Restore readiness result types
// ---------------------------------------------------------------------------

/// Top-level summary the operator's failover runbook consumes. Maps
/// directly to the contract's `RestoreReadiness` struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreReadiness {
    pub policy_id: String,
    pub target_lsn: u64,
    /// Lag in seconds between the source commit time of the latest
    /// replicated manifest entry and `now`. Per contract §"RPO
    /// Observation", this is a manifest-clock difference, not wall
    /// clock.
    pub observed_rpo_seconds: u32,
    pub latest_replicated_manifest_entry: Option<ManifestEntryRef>,
    /// Object keys referenced by manifests ≤ target_lsn that are NOT
    /// present at the destination. Populated by the caller's
    /// destination-presence check; the assembler does not invent
    /// these.
    pub missing_objects: Vec<String>,
    /// Metadata exports (tenant catalog snapshot, namespace export)
    /// that are NOT present at the destination.
    pub missing_metadata: Vec<String>,
    /// True iff a destination-region KMS decryption check succeeded.
    pub kms_accessible: bool,
    /// Top-level classification — derived from the fields above by
    /// [`RestoreReadiness::classify`].
    pub status: RestoreStatus,
}

/// Coarse classification used by alerts, dashboards, and the
/// operator's "is the DR region ready to promote?" gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreStatus {
    /// Every restorable entry up to `target_lsn` has its object
    /// present, observed lag is within `rpo_target_seconds`, and KMS
    /// is accessible.
    Ready,
    /// Within RPO but some objects/metadata still missing. The
    /// operator may choose to wait or to fail over with partial
    /// data, depending on contract.
    Incomplete,
    /// All objects present but `observed_rpo_seconds` exceeds the
    /// policy's RPO target. Operator chooses to wait for catch-up
    /// or to accept the stale checkpoint.
    Stale,
    /// KMS check failed — the destination cannot decrypt objects
    /// even if they are present. Always blocks restore.
    KmsBlocked,
    /// No replicated entries exist yet; failover would lose all
    /// recent writes. Typical for newly-created policies.
    NoReplicatedManifest,
}

impl RestoreReadiness {
    /// Classify the readiness based on populated fields plus the
    /// policy's RPO target. Pure — no I/O, no clock. The caller
    /// determines `observed_rpo_seconds` separately.
    pub fn classify(
        rpo_target_seconds: u32,
        latest_entry: Option<&ManifestEntryRef>,
        observed_rpo_seconds: u32,
        missing_objects_count: usize,
        missing_metadata_count: usize,
        kms_accessible: bool,
    ) -> RestoreStatus {
        if !kms_accessible {
            return RestoreStatus::KmsBlocked;
        }
        if latest_entry.is_none() {
            return RestoreStatus::NoReplicatedManifest;
        }
        if missing_objects_count > 0 || missing_metadata_count > 0 {
            return RestoreStatus::Incomplete;
        }
        if observed_rpo_seconds > rpo_target_seconds {
            return RestoreStatus::Stale;
        }
        RestoreStatus::Ready
    }
}

/// Pure assembly function. Given the policy, the manifest entries
/// the caller fetched, and the caller's observations about
/// destination presence + KMS, build a [`RestoreReadiness`] summary.
///
/// Encapsulates the latest-restorable-entry pick (highest LSN ≤
/// target_lsn whose status is restorable) and the RPO subtraction
/// in seconds. The caller provides `now_ms` so the function stays
/// time-injectable and exhaustively testable.
pub fn assemble_readiness(
    policy: &CollectionDrPolicy,
    target_lsn: u64,
    entries: &[ManifestEntryRef],
    missing_objects: Vec<String>,
    missing_metadata: Vec<String>,
    kms_accessible: bool,
    now_ms: i64,
) -> RestoreReadiness {
    // Pick the highest-LSN entry that:
    // - belongs to this policy's collection,
    // - is at or below `target_lsn`,
    // - has a restorable status (Flushed or Archived).
    // Iterating the slice is fine — manifest reads are bounded by
    // the WAL retention horizon, which is configured to stay within
    // a few minutes' worth of segments for hot collections.
    let latest_entry = entries
        .iter()
        .filter(|e| e.collection_id == policy.collection_id)
        .filter(|e| e.global_lsn <= target_lsn)
        .filter(|e| e.status.is_restorable())
        .max_by_key(|e| e.global_lsn)
        .cloned();

    let observed_rpo_seconds = match &latest_entry {
        Some(e) => {
            let delta_ms = now_ms.saturating_sub(e.timestamp_ms).max(0);
            (delta_ms / 1_000).min(u32::MAX as i64) as u32
        }
        None => u32::MAX,
    };

    let status = RestoreReadiness::classify(
        policy.replication.rpo_target_seconds,
        latest_entry.as_ref(),
        observed_rpo_seconds,
        missing_objects.len(),
        missing_metadata.len(),
        kms_accessible,
    );

    RestoreReadiness {
        policy_id: policy.policy_id.clone(),
        target_lsn,
        observed_rpo_seconds,
        latest_replicated_manifest_entry: latest_entry,
        missing_objects,
        missing_metadata,
        kms_accessible,
        status,
    }
}

// ---------------------------------------------------------------------------
// Checker trait
// ---------------------------------------------------------------------------

/// The async surface a failover runbook calls. Implementations
/// (root-crate bridge, operator-side bridge) wire `assemble_readiness`
/// to the live manifest service + provider adapter.
///
/// The trait is read-only — `check` never mutates xCatalog state,
/// never makes provider write calls, and is safe to invoke at any
/// time, including from a quarterly drill running against a clone
/// of the destination bucket.
#[async_trait]
pub trait DrRestoreReadinessChecker: Send + Sync {
    /// Build a readiness report for `policy` up to `target_lsn`.
    /// Most production callers leave `target_lsn` set to "the
    /// source's current LSN" and the implementation looks it up
    /// internally; tests pass an explicit value to pin the
    /// observation point.
    async fn check(
        &self,
        policy: &CollectionDrPolicy,
        target_lsn: u64,
    ) -> Result<RestoreReadiness, ProviderError>;
}

// ---------------------------------------------------------------------------
// Reference impl pluggable concerns
// ---------------------------------------------------------------------------

/// Source the reference checker queries for manifest entries. The
/// root crate wraps `GlobalManifestService` with an impl of this
/// trait; tests use the in-module mock.
///
/// Implementations should return every restorable entry for the
/// collection — the checker filters by status and target_lsn
/// internally.
#[async_trait]
pub trait ManifestSource: Send + Sync {
    async fn entries_for_collection(
        &self,
        collection_id: &str,
    ) -> Result<Vec<ManifestEntryRef>, ProviderError>;
}

/// Destination-presence probe. Given a list of candidate object
/// keys, returns the subset that are NOT present at the policy's
/// destination. Operator-side impls typically dispatch to
/// `DrProviderAdapter::fetch_state` or a HEAD request per key.
#[async_trait]
pub trait DestinationPresenceCheck: Send + Sync {
    async fn missing_objects(
        &self,
        policy: &CollectionDrPolicy,
        candidate_keys: &[String],
    ) -> Result<Vec<String>, ProviderError>;
}

/// KMS accessibility probe. Returns `true` if the destination
/// region can decrypt objects under the policy's KMS binding.
/// Implementations typically issue a `Decrypt` call against a
/// zero-byte canary blob.
#[async_trait]
pub trait KmsAccessibilityCheck: Send + Sync {
    async fn is_accessible(&self, policy: &CollectionDrPolicy) -> Result<bool, ProviderError>;
}

// ---------------------------------------------------------------------------
// EngineRestoreReadinessChecker
// ---------------------------------------------------------------------------

/// Reference `DrRestoreReadinessChecker` that wires the three
/// pluggable concerns through `assemble_readiness`. Generic so
/// operators substitute their own backing types; the type-erased
/// trait object is also fine.
///
/// Usage:
/// ```ignore
/// let checker = EngineRestoreReadinessChecker::new(
///     manifest_source, destination_check, kms_check,
/// );
/// let readiness = checker.check(&policy, target_lsn).await?;
/// ```
pub struct EngineRestoreReadinessChecker<M, D, K> {
    manifest: std::sync::Arc<M>,
    destination: std::sync::Arc<D>,
    kms: std::sync::Arc<K>,
    now_ms: std::sync::Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl<M, D, K> EngineRestoreReadinessChecker<M, D, K> {
    /// Build a checker with a system-clock-derived `now_ms`. Tests
    /// should prefer [`with_clock`].
    pub fn new(
        manifest: std::sync::Arc<M>,
        destination: std::sync::Arc<D>,
        kms: std::sync::Arc<K>,
    ) -> Self {
        let now_ms: std::sync::Arc<dyn Fn() -> i64 + Send + Sync> = std::sync::Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        });
        Self {
            manifest,
            destination,
            kms,
            now_ms,
        }
    }

    /// Build a checker with explicit clock. Tests pin observed RPO
    /// by injecting a constant.
    pub fn with_clock(
        manifest: std::sync::Arc<M>,
        destination: std::sync::Arc<D>,
        kms: std::sync::Arc<K>,
        now_ms: std::sync::Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            manifest,
            destination,
            kms,
            now_ms,
        }
    }
}

#[async_trait]
impl<M, D, K> DrRestoreReadinessChecker for EngineRestoreReadinessChecker<M, D, K>
where
    M: ManifestSource + 'static,
    D: DestinationPresenceCheck + 'static,
    K: KmsAccessibilityCheck + 'static,
{
    async fn check(
        &self,
        policy: &CollectionDrPolicy,
        target_lsn: u64,
    ) -> Result<RestoreReadiness, ProviderError> {
        // 1. Pull manifest entries for the collection.
        let entries = self
            .manifest
            .entries_for_collection(&policy.collection_id)
            .await?;

        // 2. Build the candidate-key list: restorable entries at or
        //    below target_lsn.
        let candidate_keys: Vec<String> = entries
            .iter()
            .filter(|e| e.global_lsn <= target_lsn && e.status.is_restorable())
            .map(|e| e.file_path.clone())
            .collect();

        // 3. Probe the destination. An adapter error short-circuits;
        //    the operator's caller decides whether to retry.
        let missing_objects = self
            .destination
            .missing_objects(policy, &candidate_keys)
            .await?;

        // 4. Probe KMS.
        let kms_accessible = self.kms.is_accessible(policy).await?;

        // 5. Pure assembly — same function the unit tests use.
        let now_ms = (self.now_ms)();
        Ok(assemble_readiness(
            policy,
            target_lsn,
            &entries,
            missing_objects,
            // Metadata-presence is operator territory (tenant
            // catalog export, namespace export). The reference
            // checker leaves the list empty; deployments that need
            // it wrap this impl or fork their own checker.
            Vec::new(),
            kms_accessible,
            now_ms,
        ))
    }
}

// ---------------------------------------------------------------------------
// Public testing surface
// ---------------------------------------------------------------------------

/// In-test impls of [`ManifestSource`], [`DestinationPresenceCheck`],
/// and [`KmsAccessibilityCheck`]. Downstream consumers use these to
/// exercise [`EngineRestoreReadinessChecker`] against deterministic
/// inputs without standing up a real manifest service or provider.
pub mod testing {
    use super::{
        CollectionDrPolicy, DestinationPresenceCheck, KmsAccessibilityCheck, ManifestEntryRef,
        ManifestSource, ProviderError,
    };
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// Returns a seeded list of manifest entries. The test seeds via
    /// `set_entries`; downstream code that uses this trait gets a
    /// deterministic universe.
    #[derive(Default)]
    pub struct StaticManifestSource {
        entries: Mutex<Vec<ManifestEntryRef>>,
        next_error: Mutex<Option<ProviderError>>,
    }

    impl StaticManifestSource {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        pub fn set_entries(&self, entries: Vec<ManifestEntryRef>) {
            *self.entries.lock() = entries;
        }
        pub fn inject_error(&self, err: ProviderError) {
            *self.next_error.lock() = Some(err);
        }
    }

    #[async_trait]
    impl ManifestSource for StaticManifestSource {
        async fn entries_for_collection(
            &self,
            collection_id: &str,
        ) -> Result<Vec<ManifestEntryRef>, ProviderError> {
            if let Some(e) = self.next_error.lock().take() {
                return Err(e);
            }
            Ok(self
                .entries
                .lock()
                .iter()
                .filter(|e| e.collection_id == collection_id)
                .cloned()
                .collect())
        }
    }

    /// Returns a fixed set of "missing" keys regardless of the
    /// candidate list, OR returns the intersection of the candidate
    /// list with a pre-seeded missing set.
    #[derive(Default)]
    pub struct StaticDestinationPresence {
        missing: Mutex<Vec<String>>,
        next_error: Mutex<Option<ProviderError>>,
    }

    impl StaticDestinationPresence {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        /// Configure which keys this probe reports as missing. Only
        /// keys also present in the candidate list will appear in
        /// the result of `missing_objects`.
        pub fn set_missing(&self, missing: Vec<String>) {
            *self.missing.lock() = missing;
        }
        pub fn inject_error(&self, err: ProviderError) {
            *self.next_error.lock() = Some(err);
        }
    }

    #[async_trait]
    impl DestinationPresenceCheck for StaticDestinationPresence {
        async fn missing_objects(
            &self,
            _policy: &CollectionDrPolicy,
            candidate_keys: &[String],
        ) -> Result<Vec<String>, ProviderError> {
            if let Some(e) = self.next_error.lock().take() {
                return Err(e);
            }
            let missing = self.missing.lock().clone();
            Ok(candidate_keys
                .iter()
                .filter(|k| missing.contains(k))
                .cloned()
                .collect())
        }
    }

    /// Reports a fixed accessibility result. Default is `true`
    /// (accessible).
    pub struct StaticKmsCheck {
        accessible: Mutex<bool>,
        next_error: Mutex<Option<ProviderError>>,
    }

    impl Default for StaticKmsCheck {
        fn default() -> Self {
            Self {
                accessible: Mutex::new(true),
                next_error: Mutex::new(None),
            }
        }
    }

    impl StaticKmsCheck {
        pub fn new(accessible: bool) -> Arc<Self> {
            Arc::new(Self {
                accessible: Mutex::new(accessible),
                next_error: Mutex::new(None),
            })
        }
        pub fn set_accessible(&self, accessible: bool) {
            *self.accessible.lock() = accessible;
        }
        pub fn inject_error(&self, err: ProviderError) {
            *self.next_error.lock() = Some(err);
        }
    }

    #[async_trait]
    impl KmsAccessibilityCheck for StaticKmsCheck {
        async fn is_accessible(&self, _policy: &CollectionDrPolicy) -> Result<bool, ProviderError> {
            if let Some(e) = self.next_error.lock().take() {
                return Err(e);
            }
            Ok(*self.accessible.lock())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoragePoolClass;
    use crate::collection_dr_policy::{
        DrBillingBinding, DrHealth, DrPlacement, DrReplicationBehavior, DrState, ObjectProvider,
    };

    fn policy_with_rpo(rpo_target_seconds: u32) -> CollectionDrPolicy {
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
                source_bucket_or_account: "src".into(),
                destination_bucket_or_account: "dst".into(),
                source_container: None,
                destination_container: None,
                source_prefix: "data/tnt_acme/ns_1/col_orders/".into(),
                destination_prefix: "data/tnt_acme/ns_1/col_orders/".into(),
            },
            replication: DrReplicationBehavior {
                rpo_target_seconds,
                ..DrReplicationBehavior::default()
            },
            billing: DrBillingBinding {
                cost_binding_ref: "dr-standard-binding".into(),
                cost_owner_tenant_id: "tnt_acme".into(),
                billing_approval_id: Some("appr_1".into()),
                operator_estimate_cents: None,
            },
            provider_binding: None,
            health: DrHealth::default(),
            requested_by: "u".into(),
            approved_by: None,
            created_at_ns: 0,
            updated_at_ns: 0,
            policy_version: 1,
        }
    }

    fn entry(
        lsn: u64,
        ts_ms: i64,
        status: ManifestEntryStatus,
        collection: &str,
    ) -> ManifestEntryRef {
        ManifestEntryRef {
            global_lsn: lsn,
            collection_id: collection.into(),
            batch_id: format!("batch_{lsn}"),
            file_path: format!("data/tnt_acme/ns_1/{collection}/segments/{lsn}.seg"),
            storage_url: None,
            status,
            checkpoint_id: None,
            timestamp_ms: ts_ms,
        }
    }

    // -- ManifestEntryStatus -------------------------------------------

    #[test]
    fn manifest_status_restorability_matches_contract() {
        assert!(!ManifestEntryStatus::Active.is_restorable());
        assert!(ManifestEntryStatus::Flushed.is_restorable());
        assert!(ManifestEntryStatus::Archived.is_restorable());
        assert!(!ManifestEntryStatus::RolledBack.is_restorable());
    }

    // -- classify ------------------------------------------------------

    #[test]
    fn classify_no_entries_returns_no_replicated_manifest() {
        let s = RestoreReadiness::classify(900, None, 0, 0, 0, true);
        assert_eq!(s, RestoreStatus::NoReplicatedManifest);
    }

    #[test]
    fn classify_kms_failure_dominates_everything() {
        // Even with present entries, current RPO, no missing — KMS
        // failure blocks restore.
        let e = entry(1, 1000, ManifestEntryStatus::Flushed, "col_orders");
        let s = RestoreReadiness::classify(900, Some(&e), 100, 0, 0, false);
        assert_eq!(s, RestoreStatus::KmsBlocked);
    }

    #[test]
    fn classify_missing_objects_returns_incomplete() {
        let e = entry(1, 1000, ManifestEntryStatus::Flushed, "col_orders");
        let s = RestoreReadiness::classify(900, Some(&e), 100, 2, 0, true);
        assert_eq!(s, RestoreStatus::Incomplete);
    }

    #[test]
    fn classify_missing_metadata_returns_incomplete() {
        let e = entry(1, 1000, ManifestEntryStatus::Flushed, "col_orders");
        let s = RestoreReadiness::classify(900, Some(&e), 100, 0, 1, true);
        assert_eq!(s, RestoreStatus::Incomplete);
    }

    #[test]
    fn classify_stale_observation_returns_stale() {
        // RPO target = 900s, observed = 1200s → Stale.
        let e = entry(1, 1000, ManifestEntryStatus::Flushed, "col_orders");
        let s = RestoreReadiness::classify(900, Some(&e), 1200, 0, 0, true);
        assert_eq!(s, RestoreStatus::Stale);
    }

    #[test]
    fn classify_fully_healthy_returns_ready() {
        let e = entry(1, 1000, ManifestEntryStatus::Flushed, "col_orders");
        let s = RestoreReadiness::classify(900, Some(&e), 100, 0, 0, true);
        assert_eq!(s, RestoreStatus::Ready);
    }

    #[test]
    fn classify_observed_eq_target_is_ready_not_stale() {
        // Boundary: exactly at the RPO target. Contract is "exceeds",
        // so equality counts as Ready.
        let e = entry(1, 1000, ManifestEntryStatus::Flushed, "col_orders");
        let s = RestoreReadiness::classify(900, Some(&e), 900, 0, 0, true);
        assert_eq!(s, RestoreStatus::Ready);
    }

    // -- assemble_readiness --------------------------------------------

    #[test]
    fn assemble_with_no_entries_is_no_replicated_manifest() {
        let p = policy_with_rpo(900);
        let r = assemble_readiness(&p, 100, &[], vec![], vec![], true, 5000);
        assert_eq!(r.status, RestoreStatus::NoReplicatedManifest);
        assert_eq!(r.observed_rpo_seconds, u32::MAX);
        assert!(r.latest_replicated_manifest_entry.is_none());
        assert_eq!(r.target_lsn, 100);
        assert_eq!(r.policy_id, "drp_1");
    }

    #[test]
    fn assemble_picks_highest_restorable_lsn_at_or_below_target() {
        let p = policy_with_rpo(900);
        let entries = vec![
            entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(2, 2_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(3, 3_000, ManifestEntryStatus::Active, "col_orders"), // not restorable
            entry(5, 5_000, ManifestEntryStatus::Flushed, "col_orders"), // above target
        ];
        let r = assemble_readiness(&p, 4, &entries, vec![], vec![], true, 2_500);
        let latest = r.latest_replicated_manifest_entry.expect("found");
        // LSN 3 is Active (skipped), LSN 5 is above target (skipped),
        // so the pick is LSN 2.
        assert_eq!(latest.global_lsn, 2);
        // RPO = (2500ms - 2000ms) / 1000 = 0s (rounded down)
        assert_eq!(r.observed_rpo_seconds, 0);
        assert_eq!(r.status, RestoreStatus::Ready);
    }

    #[test]
    fn assemble_skips_other_collections() {
        let p = policy_with_rpo(900);
        let entries = vec![
            entry(1, 1_000, ManifestEntryStatus::Flushed, "col_other"),
            entry(2, 2_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(3, 3_000, ManifestEntryStatus::Flushed, "col_third"),
        ];
        let r = assemble_readiness(&p, 100, &entries, vec![], vec![], true, 2_500);
        let latest = r.latest_replicated_manifest_entry.expect("found");
        assert_eq!(latest.global_lsn, 2);
        assert_eq!(latest.collection_id, "col_orders");
    }

    #[test]
    fn assemble_skips_rolled_back_entries() {
        let p = policy_with_rpo(900);
        let entries = vec![
            entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(2, 2_000, ManifestEntryStatus::RolledBack, "col_orders"),
        ];
        let r = assemble_readiness(&p, 100, &entries, vec![], vec![], true, 2_500);
        let latest = r.latest_replicated_manifest_entry.expect("found");
        assert_eq!(latest.global_lsn, 1, "rolled-back entry must be skipped");
    }

    #[test]
    fn assemble_propagates_missing_objects_and_metadata() {
        let p = policy_with_rpo(900);
        let entries = vec![entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders")];
        let r = assemble_readiness(
            &p,
            100,
            &entries,
            vec!["data/.../segments/missing.seg".to_string()],
            vec!["tenant-export-1.json".to_string()],
            true,
            2_000,
        );
        assert_eq!(r.missing_objects.len(), 1);
        assert_eq!(r.missing_metadata.len(), 1);
        assert_eq!(r.status, RestoreStatus::Incomplete);
    }

    #[test]
    fn assemble_flags_stale_when_observation_exceeds_rpo() {
        let p = policy_with_rpo(60); // 60s RPO target
        // entry timestamp at 0ms, now_ms at 120_000ms → lag = 120s
        let entries = vec![entry(1, 0, ManifestEntryStatus::Flushed, "col_orders")];
        let r = assemble_readiness(&p, 100, &entries, vec![], vec![], true, 120_000);
        assert_eq!(r.observed_rpo_seconds, 120);
        assert_eq!(r.status, RestoreStatus::Stale);
    }

    #[test]
    fn assemble_propagates_kms_failure_over_everything_else() {
        let p = policy_with_rpo(900);
        let entries = vec![entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders")];
        let r = assemble_readiness(&p, 100, &entries, vec![], vec![], false, 2_000);
        assert_eq!(r.status, RestoreStatus::KmsBlocked);
        assert!(!r.kms_accessible);
    }

    #[test]
    fn assemble_archived_entry_is_restorable() {
        // Archived entries can still drive a restore — their file
        // may have been compacted away from the source, but the
        // destination object should still be present until the
        // operator's lifecycle says otherwise.
        let p = policy_with_rpo(900);
        let entries = vec![entry(1, 1_000, ManifestEntryStatus::Archived, "col_orders")];
        let r = assemble_readiness(&p, 100, &entries, vec![], vec![], true, 1_500);
        assert!(r.latest_replicated_manifest_entry.is_some());
        assert_eq!(r.status, RestoreStatus::Ready);
    }

    #[test]
    fn assemble_serde_round_trips() {
        // The runbook log records full RestoreReadiness rows; make
        // sure the wire format round-trips so dashboards can rebuild.
        let p = policy_with_rpo(900);
        let entries = vec![entry(7, 7_000, ManifestEntryStatus::Flushed, "col_orders")];
        let r = assemble_readiness(&p, 10, &entries, vec![], vec![], true, 7_500);
        let json = serde_json::to_string(&r).unwrap();
        let back: RestoreReadiness = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn assemble_uses_target_lsn_inclusively() {
        let p = policy_with_rpo(900);
        let entries = vec![
            entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(2, 2_000, ManifestEntryStatus::Flushed, "col_orders"),
        ];
        // target = 2 → includes LSN 2.
        let r = assemble_readiness(&p, 2, &entries, vec![], vec![], true, 2_500);
        let latest = r.latest_replicated_manifest_entry.unwrap();
        assert_eq!(latest.global_lsn, 2);
        // target = 1 → only LSN 1.
        let r2 = assemble_readiness(&p, 1, &entries, vec![], vec![], true, 2_500);
        let latest2 = r2.latest_replicated_manifest_entry.unwrap();
        assert_eq!(latest2.global_lsn, 1);
    }

    // -- EngineRestoreReadinessChecker ---------------------------------

    use super::testing::{StaticDestinationPresence, StaticKmsCheck, StaticManifestSource};
    use std::sync::Arc as TestArc;

    fn make_checker(
        manifest: TestArc<StaticManifestSource>,
        destination: TestArc<StaticDestinationPresence>,
        kms: TestArc<StaticKmsCheck>,
        now_ms_val: i64,
    ) -> EngineRestoreReadinessChecker<
        StaticManifestSource,
        StaticDestinationPresence,
        StaticKmsCheck,
    > {
        let now_ms: TestArc<dyn Fn() -> i64 + Send + Sync> = TestArc::new(move || now_ms_val);
        EngineRestoreReadinessChecker::with_clock(manifest, destination, kms, now_ms)
    }

    #[tokio::test]
    async fn engine_checker_returns_ready_when_everything_passes() {
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        manifest.set_entries(vec![
            entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(2, 2_000, ManifestEntryStatus::Flushed, "col_orders"),
        ]);
        let destination = StaticDestinationPresence::new();
        // destination.set_missing left empty → nothing missing.
        let kms = StaticKmsCheck::new(true);
        let checker = make_checker(manifest, destination, kms, 2_500);

        let r = checker.check(&policy, 100).await.unwrap();
        assert_eq!(r.status, RestoreStatus::Ready);
        let latest = r.latest_replicated_manifest_entry.unwrap();
        assert_eq!(latest.global_lsn, 2);
        assert!(r.missing_objects.is_empty());
        assert!(r.kms_accessible);
    }

    #[tokio::test]
    async fn engine_checker_surfaces_destination_drops_as_incomplete() {
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        manifest.set_entries(vec![
            entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(2, 2_000, ManifestEntryStatus::Flushed, "col_orders"),
        ]);
        let destination = StaticDestinationPresence::new();
        destination.set_missing(vec!["data/tnt_acme/ns_1/col_orders/segments/2.seg".into()]);
        let kms = StaticKmsCheck::new(true);
        let checker = make_checker(manifest, destination, kms, 2_500);

        let r = checker.check(&policy, 100).await.unwrap();
        assert_eq!(r.status, RestoreStatus::Incomplete);
        assert_eq!(r.missing_objects.len(), 1);
        assert_eq!(
            r.missing_objects[0],
            "data/tnt_acme/ns_1/col_orders/segments/2.seg"
        );
    }

    #[tokio::test]
    async fn engine_checker_returns_kms_blocked_when_kms_check_fails() {
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        manifest.set_entries(vec![entry(
            1,
            1_000,
            ManifestEntryStatus::Flushed,
            "col_orders",
        )]);
        let destination = StaticDestinationPresence::new();
        let kms = StaticKmsCheck::new(false);
        let checker = make_checker(manifest, destination, kms, 2_000);

        let r = checker.check(&policy, 100).await.unwrap();
        assert_eq!(r.status, RestoreStatus::KmsBlocked);
        assert!(!r.kms_accessible);
    }

    #[tokio::test]
    async fn engine_checker_returns_stale_when_observation_exceeds_rpo() {
        let policy = policy_with_rpo(60); // 60s
        let manifest = StaticManifestSource::new();
        // Manifest at t=0; observation at t=120s (120_000 ms).
        manifest.set_entries(vec![entry(
            1,
            0,
            ManifestEntryStatus::Flushed,
            "col_orders",
        )]);
        let destination = StaticDestinationPresence::new();
        let kms = StaticKmsCheck::new(true);
        let checker = make_checker(manifest, destination, kms, 120_000);

        let r = checker.check(&policy, 100).await.unwrap();
        assert_eq!(r.status, RestoreStatus::Stale);
        assert_eq!(r.observed_rpo_seconds, 120);
    }

    #[tokio::test]
    async fn engine_checker_no_entries_yields_no_replicated_manifest() {
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        // Empty entries.
        let destination = StaticDestinationPresence::new();
        let kms = StaticKmsCheck::new(true);
        let checker = make_checker(manifest, destination, kms, 1_000);

        let r = checker.check(&policy, 100).await.unwrap();
        assert_eq!(r.status, RestoreStatus::NoReplicatedManifest);
        assert!(r.latest_replicated_manifest_entry.is_none());
    }

    #[tokio::test]
    async fn engine_checker_filters_candidate_keys_by_target_lsn_and_status() {
        // The checker should only ask the destination about
        // restorable entries ≤ target_lsn. An Active entry above
        // the target should never appear in the candidate list, so
        // even if the destination would have flagged it missing, it
        // shouldn't surface.
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        manifest.set_entries(vec![
            entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(2, 2_000, ManifestEntryStatus::Active, "col_orders"), // not restorable
            entry(5, 5_000, ManifestEntryStatus::Flushed, "col_orders"), // above target
        ]);
        let destination = StaticDestinationPresence::new();
        destination.set_missing(vec![
            // Both the Active entry's path and the above-target
            // path are seeded as "missing" — but the checker should
            // never ask about them, so neither appears.
            "data/tnt_acme/ns_1/col_orders/segments/2.seg".into(),
            "data/tnt_acme/ns_1/col_orders/segments/5.seg".into(),
        ]);
        let kms = StaticKmsCheck::new(true);
        let checker = make_checker(manifest, destination, kms, 2_000);

        let r = checker.check(&policy, 3).await.unwrap();
        // LSN 1 (the only restorable entry ≤ target=3) is present
        // at the destination (not in missing list), so the result
        // is Ready with no missing objects.
        assert_eq!(r.status, RestoreStatus::Ready);
        assert!(r.missing_objects.is_empty());
    }

    #[tokio::test]
    async fn engine_checker_propagates_manifest_source_errors() {
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        manifest.inject_error(ProviderError::Transient("store down".into()));
        let destination = StaticDestinationPresence::new();
        let kms = StaticKmsCheck::new(true);
        let checker = make_checker(manifest, destination, kms, 1_000);

        let err = checker.check(&policy, 100).await.unwrap_err();
        assert!(matches!(err, ProviderError::Transient(_)));
    }

    #[tokio::test]
    async fn engine_checker_propagates_destination_check_errors() {
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        manifest.set_entries(vec![entry(
            1,
            1_000,
            ManifestEntryStatus::Flushed,
            "col_orders",
        )]);
        let destination = StaticDestinationPresence::new();
        destination.inject_error(ProviderError::AuthDenied("403".into()));
        let kms = StaticKmsCheck::new(true);
        let checker = make_checker(manifest, destination, kms, 1_500);

        let err = checker.check(&policy, 100).await.unwrap_err();
        assert!(matches!(err, ProviderError::AuthDenied(_)));
    }

    #[tokio::test]
    async fn engine_checker_propagates_kms_errors() {
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        manifest.set_entries(vec![entry(
            1,
            1_000,
            ManifestEntryStatus::Flushed,
            "col_orders",
        )]);
        let destination = StaticDestinationPresence::new();
        let kms = StaticKmsCheck::new(true);
        kms.inject_error(ProviderError::Misconfiguration("kms missing".into()));
        let checker = make_checker(manifest, destination, kms, 1_500);

        let err = checker.check(&policy, 100).await.unwrap_err();
        assert!(matches!(err, ProviderError::Misconfiguration(_)));
    }

    #[tokio::test]
    async fn engine_checker_filters_entries_by_collection_id() {
        // The manifest source returns entries for ALL collections;
        // the impl filters by collection_id. If the StaticManifestSource
        // already filters (which it does), this also passes — but
        // the test pins the behavior at the trait boundary.
        let policy = policy_with_rpo(900);
        let manifest = StaticManifestSource::new();
        manifest.set_entries(vec![
            entry(1, 1_000, ManifestEntryStatus::Flushed, "col_orders"),
            entry(2, 2_000, ManifestEntryStatus::Flushed, "col_other"),
        ]);
        let destination = StaticDestinationPresence::new();
        let kms = StaticKmsCheck::new(true);
        let checker = make_checker(manifest, destination, kms, 2_500);

        let r = checker.check(&policy, 100).await.unwrap();
        let latest = r.latest_replicated_manifest_entry.unwrap();
        assert_eq!(latest.collection_id, "col_orders");
        assert_eq!(latest.global_lsn, 1);
    }
}
