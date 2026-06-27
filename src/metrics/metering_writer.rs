// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Durable per-tenant `_metering` writer (TD-161).
//!
//! ADR-027 decided the differentiator: per-tenant billing meters are not just
//! scraped from Prometheus (pull-only, ephemeral) but **persisted durably to the
//! tenant's own object-store subtree** under [`DrResolvedPath::metering_subprefix`]
//! (`data/{tenant}/_metering/…`), where the control plane (AnvaiOps) reads them
//! and applies its rate card. This module is the OSS-native half of that sink:
//! it serializes the per-tenant resident-storage (KSU) snapshot the
//! storage-snapshot daemon already computes and writes it through the
//! [`FilesystemFactory`] — the *same* object-store abstraction the data path uses,
//! so it lands beside the tenant's data and inherits its isolation and DR rules.
//!
//! **Cadence (co-design).** KSU is an *accrual* (`resident_bytes · time`), so the
//! durable record is a coarse, **coalesced hourly snapshot**, not a per-event
//! write — matching how real clouds meter storage (AWS byte-hours, Snowflake
//! daily averages). Metering the bill must not itself become a material line on
//! the bill: object-store round-trips/egress are *the* dominant cost term this
//! co-design effort exists to minimize (CLAUDE.md co-design mandate), so an hourly
//! cadence keeps this at ~24 PUTs/tenant/day rather than the ~1440 a per-minute
//! poll would cost. The in-memory Prometheus *level gauge* refreshes more often;
//! the *durable* write is decoupled and coarse. The exact-integral flush/compaction
//! event hook is a tracked follow-up.
//!
//! **Isolation (structural, fail-closed).** The write path is resolved through
//! [`DrPathBuilder::build_tenant_system`], which validates the tenant id (rejecting
//! empty / traversal / reserved segments) before it is ever interpolated into an
//! object key. A tenant whose id fails validation is skipped and logged — never
//! written to an unvalidated path. The proto `Collection` carries only
//! `config.owner` (no account id), so the legacy `data/{tenant}/_metering/` render
//! is used; an account-rooted layout falls out automatically once account context
//! reaches the snapshot.

use std::sync::Arc;

use crate::metrics::consumption_metrics::TenantStorageUsage;
use crate::storage::persistence::filesystem::{FileOptions, FilesystemFactory};
use crate::storage::trait_components::path_resolver::DrPathBuilder;

/// On-disk schema version for a metering record. Bump on any incompatible change
/// to [`MeteringRecord`]; readers (AnvaiOps) branch on it (mixed-read-safe).
pub const METERING_RECORD_SCHEMA_VERSION: u32 = 1;

/// One durable per-tenant metering record — the serialized shape written under
/// `…/_metering/storage/`. Kept deliberately flat and self-describing (schema
/// version + kind + unit) so the consuming control plane needs no out-of-band
/// schema. JSON, not bincode: the metering sink is a cross-process contract read
/// by AnvaiOps, so human-readable + language-neutral beats compactness here (the
/// records are tiny and written hourly).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MeteringRecord {
    /// Record schema version ([`METERING_RECORD_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Meter discriminant. `"ksu_storage_snapshot"` for the resident-bytes accrual.
    pub kind: String,
    /// Owning tenant (validated path segment).
    pub tenant_id: String,
    /// Resident bytes at snapshot time (the KSU level being integrated downstream).
    pub resident_bytes: u64,
    /// Unit of `resident_bytes` — always `"bytes"` for this record kind.
    pub unit: String,
    /// Snapshot wall-clock time (unix seconds). The control plane integrates the
    /// level across successive snapshots to byte-seconds → GB-hours.
    pub snapshot_unix_secs: i64,
}

impl MeteringRecord {
    /// Build a KSU storage-snapshot record from one tenant's aggregated usage.
    pub fn ksu_storage(usage: &TenantStorageUsage, snapshot_unix_secs: i64) -> Self {
        Self {
            schema_version: METERING_RECORD_SCHEMA_VERSION,
            kind: "ksu_storage_snapshot".to_string(),
            tenant_id: usage.tenant_id.clone(),
            resident_bytes: usage.resident_bytes,
            unit: "bytes".to_string(),
            snapshot_unix_secs,
        }
    }
}

/// Resolve the durable object URL for one tenant's KSU storage snapshot.
///
/// `base_url` is the configured storage root (e.g. `file:///…/data` or
/// `s3://bucket/prefix`). The returned URL is
/// `{base}/data/{tenant}/_metering/storage/snapshot-{secs}.json` — the
/// `data/{tenant}/_metering/` portion comes verbatim from
/// [`DrResolvedPath::metering_subprefix`], so the metering sink stays under the
/// same `DrPathBuilder` isolation prefix as the tenant's data. Returns `Err` if
/// the tenant id fails path validation (fail-closed — never write to an
/// unvalidated key).
pub fn ksu_snapshot_object_url(
    base_url: &str,
    account_id: Option<&str>,
    tenant_id: &str,
    snapshot_unix_secs: i64,
) -> anyhow::Result<String> {
    let resolved = DrPathBuilder::build_tenant_system(account_id, tenant_id)
        .map_err(|e| anyhow::anyhow!("invalid tenant id for metering path: {e}"))?;
    let prefix = resolved.metering_subprefix(); // `data/{tenant}/_metering/`
    let base = base_url.trim_end_matches('/');
    Ok(format!(
        "{base}/{prefix}storage/snapshot-{snapshot_unix_secs}.json"
    ))
}

/// Writes the durable per-tenant metering snapshot through the object-store
/// abstraction. Holds an `Arc<FilesystemFactory>` (URL-routed, mirroring
/// `src/metrics/store.rs`) and the configured storage root.
#[derive(Clone)]
pub struct DurableMeteringWriter {
    factory: Arc<FilesystemFactory>,
    base_url: String,
}

impl DurableMeteringWriter {
    /// Construct a writer rooted at `base_url` (the configured storage location).
    pub fn new(factory: Arc<FilesystemFactory>, base_url: impl Into<String>) -> Self {
        Self {
            factory,
            base_url: base_url.into(),
        }
    }

    /// Persist a KSU storage snapshot for every tenant in `usage`.
    ///
    /// Best-effort and **never fatal**: a per-tenant serialize/validate/write
    /// failure is logged and skipped so one bad tenant cannot stall metering for
    /// the rest (KSU must keep accruing). Returns the number of records
    /// successfully written. `account_id` is threaded for the account-rooted
    /// layout; pass `None` for the legacy `data/{tenant}/` render (the current
    /// snapshot source carries no account id).
    pub async fn write_storage_snapshot(
        &self,
        usage: &[TenantStorageUsage],
        account_id: Option<&str>,
        snapshot_unix_secs: i64,
    ) -> usize {
        let mut written = 0usize;
        for u in usage {
            match self.write_one(u, account_id, snapshot_unix_secs).await {
                Ok(()) => written += 1,
                Err(e) => tracing::warn!(
                    tenant = %u.tenant_id,
                    "TD-161: durable metering write skipped for tenant: {e}"
                ),
            }
        }
        if written > 0 {
            tracing::debug!(
                "TD-161: persisted {written}/{} per-tenant metering snapshots",
                usage.len()
            );
        }
        written
    }

    async fn write_one(
        &self,
        usage: &TenantStorageUsage,
        account_id: Option<&str>,
        snapshot_unix_secs: i64,
    ) -> anyhow::Result<()> {
        let url = ksu_snapshot_object_url(
            &self.base_url,
            account_id,
            &usage.tenant_id,
            snapshot_unix_secs,
        )?;
        let record = MeteringRecord::ksu_storage(usage, snapshot_unix_secs);
        let bytes = serde_json::to_vec(&record)
            .map_err(|e| anyhow::anyhow!("serialize metering record: {e}"))?;
        let options = FileOptions {
            overwrite: true,
            create_dirs: true,
            ..Default::default()
        };
        self.factory
            .write(&url, &bytes, Some(options))
            .await
            .map_err(|e| anyhow::anyhow!("write metering record to {url}: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ksu_url_uses_legacy_tenant_metering_prefix() {
        let url = ksu_snapshot_object_url("file:///srv/data", None, "tenant_acme", 1_700_000_000)
            .unwrap();
        assert_eq!(
            url,
            "file:///srv/data/data/tenant_acme/_metering/storage/snapshot-1700000000.json"
        );
    }

    #[test]
    fn ksu_url_account_rooted_when_account_present() {
        let url = ksu_snapshot_object_url(
            "s3://bucket/prefix",
            Some("acct1"),
            "tenant_acme",
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(
            url,
            "s3://bucket/prefix/accounts/acct1/tenant_acme/_metering/storage/snapshot-1700000000.json"
        );
    }

    #[test]
    fn ksu_url_trims_trailing_slash_on_base() {
        let url = ksu_snapshot_object_url("file:///srv/data/", None, "t1", 42).unwrap();
        assert_eq!(
            url,
            "file:///srv/data/data/t1/_metering/storage/snapshot-42.json"
        );
    }

    #[test]
    fn ksu_url_rejects_traversal_tenant_id_fail_closed() {
        // A tenant id that escapes its subtree must never resolve to a path.
        assert!(ksu_snapshot_object_url("file:///srv/data", None, "../escape", 1).is_err());
        assert!(ksu_snapshot_object_url("file:///srv/data", None, "", 1).is_err());
    }

    #[test]
    fn record_is_self_describing_and_versioned() {
        let usage = TenantStorageUsage {
            tenant_id: "t1".to_string(),
            resident_bytes: 4096,
        };
        let rec = MeteringRecord::ksu_storage(&usage, 1_700_000_000);
        assert_eq!(rec.schema_version, METERING_RECORD_SCHEMA_VERSION);
        assert_eq!(rec.kind, "ksu_storage_snapshot");
        assert_eq!(rec.unit, "bytes");
        assert_eq!(rec.resident_bytes, 4096);
        // Round-trips through JSON (the cross-process contract with AnvaiOps).
        let json = serde_json::to_vec(&rec).unwrap();
        let back: MeteringRecord = serde_json::from_slice(&json).unwrap();
        assert_eq!(rec, back);
    }

    /// Runtime-verify: the writer actually persists per-tenant records through the
    /// real `FilesystemFactory` and they read back intact under the
    /// `data/{tenant}/_metering/storage/` key — the durable-sink contract, not just
    /// path arithmetic.
    #[tokio::test]
    async fn durable_writer_round_trips_through_local_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let base = format!("file://{}", dir.path().display());
        let factory = Arc::new(FilesystemFactory::create_default().await.unwrap());
        let writer = DurableMeteringWriter::new(factory.clone(), base.clone());

        let usage = vec![
            TenantStorageUsage {
                tenant_id: "tenant_a".to_string(),
                resident_bytes: 1234,
            },
            TenantStorageUsage {
                tenant_id: "tenant_b".to_string(),
                resident_bytes: 5678,
            },
        ];
        let ts = 1_700_000_000;
        let written = writer.write_storage_snapshot(&usage, None, ts).await;
        assert_eq!(written, 2, "both tenants persisted");

        // Read tenant_a's record back via the same factory and assert content.
        let url = ksu_snapshot_object_url(&base, None, "tenant_a", ts).unwrap();
        let bytes = factory.read(&url).await.unwrap();
        let rec: MeteringRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rec.tenant_id, "tenant_a");
        assert_eq!(rec.resident_bytes, 1234);
        assert_eq!(rec.kind, "ksu_storage_snapshot");
        assert_eq!(rec.snapshot_unix_secs, ts);

        // The key lands under the per-tenant `_metering/` isolation subtree.
        assert!(url.contains("/data/tenant_a/_metering/storage/"));
    }
}
