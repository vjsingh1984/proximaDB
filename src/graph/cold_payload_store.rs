//! Cold graph-payload record store (TD-168 Phase 2).
//!
//! A graph-dedicated [`RecordStore`] that makes node/edge **payloads** durable on
//! object storage at a cold access tier, so a graph whose payloads exceed RAM is
//! servable. It is the production backing store the cold-fetch read path
//! (`GraphOperationsService::cold_fetch_node`/`cold_fetch_edge`, #446) reads from
//! on a cache miss and the create/update path writes to
//! (`upsert_canonical_node_record`/`upsert_canonical_edge_record`).
//!
//! ## Why a dedicated store (not the PAX/bridge path)
//! Cold-fetch is a **point-get by `oid`** (`get_record(RecordKey)`); the
//! `ObjectStoreBridge`/`ObjectStoreVectorRecordStore` path is schema-driven PAX
//! *segments* (batched) and would need a segment→oid index plus a flip of the
//! shared relational DML path. A simple `oid → object` K/V matches the read
//! pattern exactly, stays **graph-only by construction** (so it can tier every
//! object Cool with no risk of mis-tiering hot relational data and no label
//! discriminator), and calls [`ProximaObjectStore::put_with_tier`] directly.
//!
//! ## Tiering
//! Each record is one object keyed `graph-cold/{oid}` written with a fixed
//! [`ObjectAccessTier`] (Cool in production — the at-rest GB-month cost lever,
//! ADR-036/TD-173). On an untiered backend (local/memory) `put_with_tier`
//! degrades to a plain write, so the same code path is correct in tests and on
//! `file://`.
//!
//! ## Durability / format
//! Records round-trip through the canonical [`ProximaRecordV2`] bincode wire (the
//! same encoding the WAL uses), so the `#[serde(skip)]` `schema_version` is
//! restamped on decode. Graph node/edge canonical records carry no embeddings, so
//! the payload is just identity + labels + properties + timestamps.
//!
//! Gated default-OFF: this store is only constructed and wired when
//! `PROXIMADB_GRAPH_COLD_PAYLOADS` is set (see `shared_services`). With the gate
//! off the graph keeps its all-RAM behavior unchanged.

use async_trait::async_trait;
use bytes::Bytes;
use object_store::path::Path as ObjectPath;

use proximadb_kernel::error::StorageError;
use proximadb_object_store::ProximaObjectStore;
use proximadb_records::wire_v2::ProximaRecordV2;
use proximadb_records::{ProximaRecord, RecordKey, RecordStore, RecordStoreResult};
use proximadb_storage_filesystem_types::ObjectAccessTier;

/// Key prefix isolating cold graph payloads from vector/relational object keys.
const COLD_PREFIX: &str = "graph-cold";

/// Object-storage-backed [`RecordStore`] for cold graph node/edge payloads.
///
/// Construct with [`Self::from_storage_root`] (production) or [`Self::new`]
/// (tests, over an in-memory/file store). Cheaply cloneable.
#[derive(Clone)]
pub struct ColdGraphRecordStore {
    store: ProximaObjectStore,
    tier: ObjectAccessTier,
}

impl ColdGraphRecordStore {
    /// Open a cold store over the object-storage root `url` (e.g. `az://acct/container`,
    /// `s3://bucket`, `file:///data`, `memory://`), writing every payload at `tier`.
    ///
    /// Credentials are the standard secret-less object-store env (Workload/Managed
    /// Identity for Azure, web-identity for AWS) — see [`ProximaObjectStore::from_url`].
    pub fn from_storage_root(url: &str, tier: ObjectAccessTier) -> RecordStoreResult<Self> {
        let store = ProximaObjectStore::from_url(url)
            .map_err(|e| anyhow::anyhow!("cold graph store: open `{url}` failed: {e}"))?;
        Ok(Self { store, tier })
    }

    /// Wrap an already-built [`ProximaObjectStore`] (the base prefix is honored).
    pub fn new(store: ProximaObjectStore, tier: ObjectAccessTier) -> Self {
        Self { store, tier }
    }

    /// The fixed access tier every payload is written at.
    pub fn tier(&self) -> ObjectAccessTier {
        self.tier
    }

    /// Object key for a record `oid`: `graph-cold/{oid}`. The `oid` is already
    /// path-shaped (`graph/{graph_id}/node/{node_id}`), so this nests cleanly and
    /// never collides with vector/relational keys.
    fn key_for(oid: &str) -> ObjectPath {
        ObjectPath::from(format!("{COLD_PREFIX}/{oid}"))
    }
}

#[async_trait]
impl RecordStore for ColdGraphRecordStore {
    async fn upsert_record(&self, record: ProximaRecord) -> RecordStoreResult<ProximaRecord> {
        let wire = ProximaRecordV2::from(&record);
        let bytes = bincode::serialize(&wire).map_err(|e| {
            anyhow::anyhow!("cold graph store: encode `{}` failed: {e}", record.oid)
        })?;
        let len = bytes.len() as u64;
        self.store
            .put_with_tier(&Self::key_for(&record.oid), Bytes::from(bytes), self.tier)
            .await
            .map_err(|e| anyhow::anyhow!("cold graph store: put `{}` failed: {e}", record.oid))?;
        // Write-time visibility into how much payload is going to the cold tier
        // (ADR-030 observability; the resident-bytes Cool-vs-Hot KSU split is a
        // separate metering change — see TD-168).
        crate::metrics::consumption_metrics::record_object_store_write_bytes_by_tier(
            &record.tenant_id,
            self.tier.as_str(),
            len,
        );
        Ok(record)
    }

    async fn get_record(&self, key: &RecordKey) -> RecordStoreResult<Option<ProximaRecord>> {
        match self.store.get(&Self::key_for(&key.oid)).await {
            Ok(bytes) => {
                let wire: ProximaRecordV2 = bincode::deserialize(&bytes).map_err(|e| {
                    anyhow::anyhow!("cold graph store: decode `{}` failed: {e}", key.oid)
                })?;
                Ok(Some(ProximaRecord::from(wire)))
            }
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(e) => Err(anyhow::anyhow!(
                "cold graph store: get `{}` failed: {e}",
                key.oid
            )),
        }
    }

    async fn get_records(
        &self,
        keys: &[RecordKey],
    ) -> RecordStoreResult<Vec<Option<ProximaRecord>>> {
        // Issue the independent object-store GETs concurrently so K cold point-
        // lookups collapse to ~1 round-trip of latency (ADR-034 depth-collapse)
        // instead of K serial RTTs. Each `get_record` decodes the ProximaRecordV2
        // wire; `try_join_all` preserves input order and short-circuits on error.
        futures::future::try_join_all(keys.iter().map(|key| self.get_record(key))).await
    }

    async fn delete_record(&self, key: &RecordKey) -> RecordStoreResult<bool> {
        let path = Self::key_for(&key.oid);
        // `object_store` delete-of-missing is backend-inconsistent (InMemory/S3 are
        // idempotent `Ok`, LocalFile/Azure return `NotFound`), so probe existence
        // with a cheap metadata `head` first to return a stable "was-present"
        // result. Delete is off the hot read path, so the extra HEAD is fine.
        match self.store.head(&path).await {
            Ok(_) => {}
            Err(StorageError::NotFound(_)) => return Ok(false),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "cold graph store: head `{}` failed: {e}",
                    key.oid
                ));
            }
        }
        self.store
            .delete(&path)
            .await
            .map_err(|e| anyhow::anyhow!("cold graph store: delete `{}` failed: {e}", key.oid))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_object_store::ObjectBackendKind;

    fn mem_store() -> ColdGraphRecordStore {
        ColdGraphRecordStore::from_storage_root("memory://", ObjectAccessTier::Cool)
            .expect("open memory cold store")
    }

    fn record(oid: &str, tenant: &str) -> ProximaRecord {
        ProximaRecord {
            oid: oid.to_string(),
            tenant_id: tenant.to_string(),
            ..ProximaRecord::default()
        }
    }

    #[tokio::test]
    async fn upsert_then_get_round_trips_by_oid() {
        let store = mem_store();
        let rec = record("graph/g1/node/n1", "tenantA");
        store.upsert_record(rec.clone()).await.expect("upsert");

        let got = store
            .get_record(&RecordKey::new("graph/g1/node/n1"))
            .await
            .expect("get")
            .expect("present");
        assert_eq!(got.oid, rec.oid);
        assert_eq!(got.tenant_id, rec.tenant_id);
    }

    #[tokio::test]
    async fn get_records_batches_in_order_with_present_and_absent() {
        let store = mem_store();
        store
            .upsert_record(record("graph/g1/node/a", "t"))
            .await
            .expect("upsert a");
        store
            .upsert_record(record("graph/g1/node/c", "t"))
            .await
            .expect("upsert c");

        // Mixed batch: present, absent, present — order + slots preserved.
        let keys = [
            RecordKey::new("graph/g1/node/a"),
            RecordKey::new("graph/g1/node/b"),
            RecordKey::new("graph/g1/node/c"),
        ];
        let got = store.get_records(&keys).await.expect("get_records");
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0].as_ref().map(|r| r.oid.as_str()),
            Some("graph/g1/node/a")
        );
        assert!(got[1].is_none());
        assert_eq!(
            got[2].as_ref().map(|r| r.oid.as_str()),
            Some("graph/g1/node/c")
        );
    }

    #[tokio::test]
    async fn get_missing_oid_returns_none_not_error() {
        let store = mem_store();
        let got = store
            .get_record(&RecordKey::new("graph/g1/node/absent"))
            .await
            .expect("get must not error on miss");
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn delete_removes_record_and_is_idempotent() {
        let store = mem_store();
        let key = RecordKey::new("graph/g1/edge/e1");
        store
            .upsert_record(record("graph/g1/edge/e1", "tenantA"))
            .await
            .expect("upsert");

        assert!(store.delete_record(&key).await.expect("delete present"));
        assert!(
            store
                .get_record(&key)
                .await
                .expect("get after delete")
                .is_none()
        );
        // Deleting an absent object is not an error and reports "nothing removed".
        assert!(!store.delete_record(&key).await.expect("delete absent"));
    }

    #[tokio::test]
    async fn memory_backend_is_untiered_so_put_with_tier_degrades_and_bytes_land() {
        let store = mem_store();
        // The memory backend cannot carry an access tier; the write must still land.
        assert_eq!(store.store.backend(), ObjectBackendKind::Untiered);
        let key = RecordKey::new("graph/g1/node/tiered");
        store
            .upsert_record(record("graph/g1/node/tiered", "tenantA"))
            .await
            .expect("upsert on untiered backend");
        assert!(
            store.get_record(&key).await.expect("get").is_some(),
            "bytes must land even when the tier is meaningless"
        );
    }
}
