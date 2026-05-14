//! Canonical `ProximaRecord` store contracts.
//!
//! These traits describe the durable record spine used by modality facades.
//! Document, graph, vector, observability, SKS/entity, and event services can
//! adapt to these contracts without owning separate record envelopes or
//! modality-specific WAL/recovery semantics.

use async_trait::async_trait;

use crate::ProximaRecord;

/// Result type for canonical record-store operations.
pub type RecordStoreResult<T> = anyhow::Result<T>;

/// Key used to address one canonical record.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecordKey {
    /// Canonical record object id.
    pub oid: String,
}

impl RecordKey {
    pub fn new(oid: impl Into<String>) -> Self {
        Self { oid: oid.into() }
    }
}

impl From<&ProximaRecord> for RecordKey {
    fn from(record: &ProximaRecord) -> Self {
        Self::new(record.oid.clone())
    }
}

/// Batch write result for canonical record operations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordWriteResult {
    /// Number of records accepted by the durable store.
    pub records_written: usize,
    /// Canonical ids written by the operation.
    pub record_oids: Vec<String>,
}

/// Narrow canonical store contract over `ProximaRecord`.
///
/// This is intentionally modality-neutral. It does not describe document JSON
/// paths, graph adjacency, vector ANN, or observability rollups; those are
/// facades/projections layered above this trait.
#[async_trait]
pub trait RecordStore: Send + Sync {
    async fn upsert_record(&self, record: ProximaRecord) -> RecordStoreResult<ProximaRecord>;

    async fn get_record(&self, key: &RecordKey) -> RecordStoreResult<Option<ProximaRecord>>;

    async fn delete_record(&self, key: &RecordKey) -> RecordStoreResult<bool>;

    async fn upsert_records(
        &self,
        records: Vec<ProximaRecord>,
    ) -> RecordStoreResult<RecordWriteResult> {
        let mut record_oids = Vec::with_capacity(records.len());

        for record in records {
            let written = self.upsert_record(record).await?;
            record_oids.push(written.oid);
        }

        Ok(RecordWriteResult {
            records_written: record_oids.len(),
            record_oids,
        })
    }
}

/// Optional scan contract for stores that can expose canonical record ranges.
///
/// Keep this separate from `RecordStore` so point-write stores can implement
/// the minimal durable contract before scan/query planning is extracted.
#[async_trait]
pub trait RecordScan: Send + Sync {
    async fn scan_records(&self, limit: usize) -> RecordStoreResult<Vec<ProximaRecord>>;
}

/// Composite canonical storage contract for services that need both point
/// operations and scans.
///
/// Keep document/graph/vector semantics out of this trait. Facades filter and
/// project records after scanning, while the durable contract remains a shared
/// `ProximaRecord` spine.
pub trait RecordStorage: RecordStore + RecordScan {}

impl<T> RecordStorage for T where T: RecordStore + RecordScan + ?Sized {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::RwLock;

    #[derive(Default)]
    struct MemoryRecordStore {
        records: RwLock<HashMap<String, ProximaRecord>>,
    }

    #[async_trait]
    impl RecordStore for MemoryRecordStore {
        async fn upsert_record(&self, record: ProximaRecord) -> RecordStoreResult<ProximaRecord> {
            self.records
                .write()
                .expect("memory record store write lock")
                .insert(record.oid.clone(), record.clone());
            Ok(record)
        }

        async fn get_record(&self, key: &RecordKey) -> RecordStoreResult<Option<ProximaRecord>> {
            Ok(self
                .records
                .read()
                .expect("memory record store read lock")
                .get(&key.oid)
                .cloned())
        }

        async fn delete_record(&self, key: &RecordKey) -> RecordStoreResult<bool> {
            Ok(self
                .records
                .write()
                .expect("memory record store write lock")
                .remove(&key.oid)
                .is_some())
        }
    }

    #[async_trait]
    impl RecordScan for MemoryRecordStore {
        async fn scan_records(&self, limit: usize) -> RecordStoreResult<Vec<ProximaRecord>> {
            Ok(self
                .records
                .read()
                .expect("memory record store read lock")
                .values()
                .take(limit)
                .cloned()
                .collect())
        }
    }

    #[tokio::test]
    async fn record_store_contract_supports_point_lifecycle() {
        let store = MemoryRecordStore::default();
        let record = ProximaRecord {
            oid: "doc-1".to_string(),
            ..ProximaRecord::default()
        };

        let key = RecordKey::from(&record);
        let written = store.upsert_record(record).await.expect("upsert");
        assert_eq!(written.oid, "doc-1");

        let fetched = store.get_record(&key).await.expect("get");
        assert_eq!(fetched.map(|record| record.oid), Some("doc-1".to_string()));

        assert!(store.delete_record(&key).await.expect("delete"));
        assert!(store.get_record(&key).await.expect("get deleted").is_none());
    }

    #[tokio::test]
    async fn record_store_default_batch_upsert_reports_written_ids() {
        let store = MemoryRecordStore::default();
        let records = vec![
            ProximaRecord {
                oid: "r1".to_string(),
                ..ProximaRecord::default()
            },
            ProximaRecord {
                oid: "r2".to_string(),
                ..ProximaRecord::default()
            },
        ];

        let result = store.upsert_records(records).await.expect("batch upsert");
        assert_eq!(result.records_written, 2);
        assert_eq!(result.record_oids, vec!["r1", "r2"]);
    }

    #[tokio::test]
    async fn record_storage_composes_point_and_scan_contracts() {
        let store = MemoryRecordStore::default();
        let storage: &dyn RecordStorage = &store;

        storage
            .upsert_record(ProximaRecord {
                oid: "r1".to_string(),
                ..ProximaRecord::default()
            })
            .await
            .expect("upsert");

        let records = storage.scan_records(10).await.expect("scan");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].oid, "r1");
    }
}
