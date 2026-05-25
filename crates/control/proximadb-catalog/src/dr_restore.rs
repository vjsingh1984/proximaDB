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
        matches!(self, ManifestEntryStatus::Flushed | ManifestEntryStatus::Archived)
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
    /// Identifier of the batch this entry committed.
    pub batch_id: u64,
    /// Storage object key the batch lives at. The destination
    /// presence check uses this verbatim.
    pub file_path: String,
    /// Optional storage URL (e.g. `s3://bucket/key`). Set when the
    /// engine knows the full provider URI; readers prefer
    /// `file_path` for the relative key.
    pub storage_url: Option<String>,
    pub status: ManifestEntryStatus,
    /// Optional checkpoint identifier. Multiple entries may share
    /// a checkpoint.
    pub checkpoint_id: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection_dr_policy::{
        DrBillingBinding, DrHealth, DrPlacement, DrReplicationBehavior,
        DrState, ObjectProvider,
    };
    use crate::StoragePoolClass;

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
                source_pool_class: StoragePoolClass::Business,
                destination_pool_class: StoragePoolClass::Business,
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
                billing_sku: "collection-dr-business".into(),
                cost_owner_tenant_id: "tnt_acme".into(),
                billing_approval_id: Some("appr_1".into()),
                estimated_monthly_cost_cents: None,
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
            batch_id: lsn,
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
        let r =
            assemble_readiness(&p, 100, &entries, vec![], vec![], true, 120_000);
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
        let r =
            assemble_readiness(&p, 10, &entries, vec![], vec![], true, 7_500);
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
}
