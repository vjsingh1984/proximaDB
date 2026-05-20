//! Canonical in-memory current-state store for direct record writes.
//!
//! This store is a neutral `ProximaRecord` memtable used by early pgwire
//! relational DML wiring while PAX/LSM current-state storage is still being
//! connected. Durability remains in the canonical WAL; this structure is
//! rebuildable from Layer 0 entries and must not grow independent persistence.

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use proximadb_records::{
    ProximaRecord, RecordKey, RecordRecoverySummary, RecordScan, RecordScanOptions, RecordStore,
    RecordStoreResult,
};
use proximadb_storage_common::{CanonicalOperation, CanonicalWalEntry};

/// Rebuildable current-state memtable keyed by canonical record OID.
#[derive(Debug, Default)]
pub struct MemtableRecordStorage {
    records: DashMap<String, ProximaRecord>,
}

impl MemtableRecordStorage {
    /// Create an empty current-state memtable.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild current state from canonical WAL entries.
    pub async fn replay_wal_entries<I>(&self, entries: I) -> Result<RecordRecoverySummary>
    where
        I: IntoIterator<Item = CanonicalWalEntry>,
    {
        let mut summary = RecordRecoverySummary::default();

        for entry in entries {
            match entry.operation {
                CanonicalOperation::RecordUpsert { record, .. } => {
                    self.upsert_record(*record).await?;
                    summary.upserts_replayed += 1;
                }
                CanonicalOperation::RecordDelete { oid, .. } => {
                    self.delete_record(&RecordKey::new(oid)).await?;
                    summary.deletes_replayed += 1;
                }
                CanonicalOperation::Checkpoint(_) | CanonicalOperation::CdcBarrier { .. } => {}
            }
        }

        Ok(summary)
    }

    /// Number of records currently visible in the memtable.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the memtable has no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[async_trait]
impl RecordStore for MemtableRecordStorage {
    async fn upsert_record(&self, record: ProximaRecord) -> RecordStoreResult<ProximaRecord> {
        self.records.insert(record.oid.clone(), record.clone());
        Ok(record)
    }

    async fn get_record(&self, key: &RecordKey) -> RecordStoreResult<Option<ProximaRecord>> {
        Ok(self
            .records
            .get(&key.oid)
            .map(|record| record.value().clone()))
    }

    async fn delete_record(&self, key: &RecordKey) -> RecordStoreResult<bool> {
        Ok(self.records.remove(&key.oid).is_some())
    }
}

#[async_trait]
impl RecordScan for MemtableRecordStorage {
    async fn scan_records(&self, limit: usize) -> RecordStoreResult<Vec<ProximaRecord>> {
        Ok(self
            .records
            .iter()
            .take(limit)
            .map(|record| record.value().clone())
            .collect())
    }

    async fn scan_records_with_options(
        &self,
        options: RecordScanOptions,
    ) -> RecordStoreResult<Vec<ProximaRecord>> {
        let limit = options.limit.unwrap_or(usize::MAX);
        Ok(self
            .records
            .iter()
            .filter_map(|record| {
                let record = record.value();
                options.matches_record(record).then(|| record.clone())
            })
            .take(limit)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::ProximaTreeNode;
    use proximadb_storage_common::ProjectionDirective;

    fn record(oid: &str, tenant_id: &str) -> ProximaRecord {
        let mut record = ProximaRecord {
            oid: oid.to_string(),
            tenant_id: tenant_id.to_string(),
            ..ProximaRecord::default()
        };
        record.props.insert(
            "status".to_string(),
            ProximaTreeNode::Value(proximadb_data_model::ProximaValue::String(
                "open".to_string(),
            )),
        );
        record
    }

    fn upsert_entry(seq: u64, record: ProximaRecord) -> CanonicalWalEntry {
        CanonicalWalEntry::new(
            seq,
            CanonicalOperation::RecordUpsert {
                collection_id: "orders".to_string(),
                record: Box::new(record),
                projections: vec![ProjectionDirective::ColumnarVariation {
                    collection_id: "orders".to_string(),
                    fields: vec!["status".to_string()],
                }],
            },
            None,
        )
    }

    fn delete_entry(seq: u64, oid: &str) -> CanonicalWalEntry {
        CanonicalWalEntry::new(
            seq,
            CanonicalOperation::RecordDelete {
                collection_id: "orders".to_string(),
                oid: oid.to_string(),
                projections: Vec::new(),
            },
            None,
        )
    }

    #[tokio::test]
    async fn memtable_record_storage_replays_canonical_wal_entries() -> Result<()> {
        let storage = MemtableRecordStorage::new();
        let summary = storage
            .replay_wal_entries(vec![
                upsert_entry(1, record("order-1", "tenant-a")),
                upsert_entry(2, record("order-2", "tenant-a")),
                delete_entry(3, "order-1"),
            ])
            .await?;

        assert_eq!(summary.upserts_replayed, 2);
        assert_eq!(summary.deletes_replayed, 1);
        assert_eq!(storage.len(), 1);
        assert!(
            storage
                .get_record(&RecordKey::new("order-1"))
                .await?
                .is_none()
        );
        assert!(
            storage
                .get_record(&RecordKey::new("order-2"))
                .await?
                .is_some()
        );

        let scanned = storage
            .scan_records_with_options(
                RecordScanOptions::unbounded()
                    .with_tenant_id("tenant-a")
                    .with_string_property("status", "open"),
            )
            .await?;
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].oid, "order-2");

        Ok(())
    }
}
