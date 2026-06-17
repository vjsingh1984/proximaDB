//! DR restore-readiness bridge — root crate ↔ catalog crate.
//!
//! The catalog crate ships
//! [`proximadb_catalog::dr_restore::ManifestEntryRef`] as an abstract
//! manifest projection so the restore-readiness checker can be built
//! without depending on the WAL types in the root crate. This module
//! is the concrete bridge: it maps a [`GlobalManifestEntry`] (this
//! crate) into a `ManifestEntryRef` (catalog crate).
//!
//! Pure conversion — no I/O, no clock, no allocation beyond field
//! clones. Operators wiring `DrRestoreReadinessChecker` impls call
//! these helpers on each entry pulled from `GlobalManifestService`.
//!
//! See `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc`
//! §"LLD: Restore Primitives".

use super::types::{GlobalManifestEntry, WalEntryStatus};
use proximadb_catalog::dr_restore::{ManifestEntryRef, ManifestEntryStatus};

/// Translate the root-crate `WalEntryStatus` to the catalog-crate
/// `ManifestEntryStatus`. Both enums carry the same four variants
/// with identical semantics, so the mapping is total.
pub fn wal_status_to_manifest(status: WalEntryStatus) -> ManifestEntryStatus {
    match status {
        WalEntryStatus::Active => ManifestEntryStatus::Active,
        WalEntryStatus::Flushed => ManifestEntryStatus::Flushed,
        WalEntryStatus::Archived => ManifestEntryStatus::Archived,
        WalEntryStatus::RolledBack => ManifestEntryStatus::RolledBack,
    }
}

/// Translate a `GlobalManifestEntry` into a catalog-crate
/// `ManifestEntryRef`. The catalog crate stays oblivious to the
/// engine's serialization format, vector count, size, and checksum
/// — those don't affect restore readiness, so the bridge drops them.
///
/// Type adapters:
/// - `timestamp_ms: u64 → i64` via saturating cast (won't overflow
///   for ~292M years past the epoch).
/// - `storage_url: String → Option<String>`, wrapped as `Some(...)`
///   because the engine always populates it.
pub fn manifest_entry_to_ref(entry: &GlobalManifestEntry) -> ManifestEntryRef {
    ManifestEntryRef {
        global_lsn: entry.global_lsn,
        collection_id: entry.collection_id.clone(),
        batch_id: entry.batch_id.clone(),
        file_path: entry.file_path.clone(),
        storage_url: Some(entry.storage_url.clone()),
        status: wal_status_to_manifest(entry.status),
        checkpoint_id: entry.checkpoint_id,
        timestamp_ms: i64::try_from(entry.timestamp_ms).unwrap_or(i64::MAX),
    }
}

/// Convenience: map a whole slice of entries.
pub fn manifest_entries_to_refs(entries: &[GlobalManifestEntry]) -> Vec<ManifestEntryRef> {
    entries.iter().map(manifest_entry_to_ref).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::write_ahead_log::serialization::SerializationFormat;

    fn sample_entry(
        lsn: u64,
        ts_ms: u64,
        status: WalEntryStatus,
        collection: &str,
        checkpoint_id: Option<u64>,
    ) -> GlobalManifestEntry {
        GlobalManifestEntry {
            global_lsn: lsn,
            collection_id: collection.into(),
            batch_id: format!("BATCH{lsn}"),
            file_path: format!("{collection}/wal/{lsn}.wal"),
            storage_url: "file:///tmp/proximadb/data".into(),
            size_bytes: 1024,
            checksum_crc32: 0xDEAD_BEEF,
            timestamp_ms: ts_ms,
            format: SerializationFormat::Bincode,
            vector_count: 42,
            status,
            checkpoint_id,
        }
    }

    // -- status mapping -------------------------------------------------

    #[test]
    fn wal_status_mapping_is_total() {
        assert_eq!(
            wal_status_to_manifest(WalEntryStatus::Active),
            ManifestEntryStatus::Active
        );
        assert_eq!(
            wal_status_to_manifest(WalEntryStatus::Flushed),
            ManifestEntryStatus::Flushed
        );
        assert_eq!(
            wal_status_to_manifest(WalEntryStatus::Archived),
            ManifestEntryStatus::Archived
        );
        assert_eq!(
            wal_status_to_manifest(WalEntryStatus::RolledBack),
            ManifestEntryStatus::RolledBack
        );
    }

    // -- entry mapping --------------------------------------------------

    #[test]
    fn entry_mapping_preserves_lsn_collection_batch_path() {
        let entry = sample_entry(7, 1_000, WalEntryStatus::Flushed, "col_orders", None);
        let r = manifest_entry_to_ref(&entry);
        assert_eq!(r.global_lsn, 7);
        assert_eq!(r.collection_id, "col_orders");
        assert_eq!(r.batch_id, "BATCH7");
        assert_eq!(r.file_path, "col_orders/wal/7.wal");
        assert_eq!(r.status, ManifestEntryStatus::Flushed);
    }

    #[test]
    fn entry_mapping_wraps_storage_url_in_some() {
        let entry = sample_entry(1, 100, WalEntryStatus::Active, "c", None);
        let r = manifest_entry_to_ref(&entry);
        assert_eq!(r.storage_url, Some("file:///tmp/proximadb/data".into()));
    }

    #[test]
    fn entry_mapping_passes_checkpoint_id_through() {
        let entry = sample_entry(1, 100, WalEntryStatus::Archived, "c", Some(42));
        let r = manifest_entry_to_ref(&entry);
        assert_eq!(r.checkpoint_id, Some(42));
        let entry = sample_entry(2, 200, WalEntryStatus::Archived, "c", None);
        let r = manifest_entry_to_ref(&entry);
        assert_eq!(r.checkpoint_id, None);
    }

    #[test]
    fn entry_mapping_drops_engine_only_fields() {
        // size_bytes, checksum, format, vector_count are present in
        // the source but intentionally dropped — they don't affect
        // restore readiness. The bridge does not expose them on
        // ManifestEntryRef; this test pins the field set so a future
        // catalog-crate change doesn't accidentally re-introduce
        // them.
        let entry = sample_entry(1, 1, WalEntryStatus::Flushed, "c", None);
        let r = manifest_entry_to_ref(&entry);
        // Type assertion: every ManifestEntryRef field is reachable
        // via the bridge, and only those fields. If a new field
        // gets added to ManifestEntryRef, this test won't compile
        // until the bridge populates it.
        let _: ManifestEntryRef = ManifestEntryRef {
            global_lsn: r.global_lsn,
            collection_id: r.collection_id.clone(),
            batch_id: r.batch_id.clone(),
            file_path: r.file_path.clone(),
            storage_url: r.storage_url.clone(),
            status: r.status,
            checkpoint_id: r.checkpoint_id,
            timestamp_ms: r.timestamp_ms,
        };
    }

    #[test]
    fn entry_mapping_handles_timestamp_overflow_gracefully() {
        // u64 timestamps fit in i64 until ~year 292M; the saturating
        // cast guarantees that pathological u64::MAX inputs don't
        // wrap to a negative i64.
        let mut entry = sample_entry(1, 100, WalEntryStatus::Flushed, "c", None);
        entry.timestamp_ms = u64::MAX;
        let r = manifest_entry_to_ref(&entry);
        assert_eq!(r.timestamp_ms, i64::MAX);
    }

    // -- slice helper ---------------------------------------------------

    #[test]
    fn slice_helper_preserves_order_and_count() {
        let entries = vec![
            sample_entry(1, 1_000, WalEntryStatus::Flushed, "a", None),
            sample_entry(2, 2_000, WalEntryStatus::Flushed, "a", None),
            sample_entry(3, 3_000, WalEntryStatus::Flushed, "b", Some(7)),
        ];
        let refs = manifest_entries_to_refs(&entries);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].global_lsn, 1);
        assert_eq!(refs[2].collection_id, "b");
        assert_eq!(refs[2].checkpoint_id, Some(7));
    }

    #[test]
    fn slice_helper_on_empty_returns_empty() {
        let refs = manifest_entries_to_refs(&[]);
        assert!(refs.is_empty());
    }

    // -- restorability invariant -------------------------------------

    #[test]
    fn flushed_and_archived_map_to_restorable_statuses() {
        // Engine contract: Flushed and Archived are restorable;
        // Active and RolledBack are not. Verified independently in
        // dr_restore::tests but pinned here against the bridge as
        // well so a future enum rename in either crate surfaces.
        for (engine, restorable) in [
            (WalEntryStatus::Active, false),
            (WalEntryStatus::Flushed, true),
            (WalEntryStatus::Archived, true),
            (WalEntryStatus::RolledBack, false),
        ] {
            let m = wal_status_to_manifest(engine);
            assert_eq!(m.is_restorable(), restorable, "engine status {engine:?}");
        }
    }
}
