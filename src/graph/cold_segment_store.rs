//! Cold graph-payload **segment** store (TD-168 follow-up #3, Phase 1 — capability).
//!
//! [`ColdGraphRecordStore`](crate::graph::ColdGraphRecordStore) writes one object
//! per node/edge (`graph-cold/{oid}`) — simple, but one PUT per create and one GET
//! per cold fetch. This store **batches** many records into one object so the
//! object-store **op count** drops (the KRU/$ lever the ADR-034 I/O-trace audit
//! flagged): one PUT per *segment* (~thousands of records) instead of per record.
//!
//! ## What this is (Phase 1, unconditional win)
//! - **Write:** records buffer in RAM and flush to one segment object on a size /
//!   count threshold (or an explicit [`flush`](Self::flush)). ⇒ ~Nx fewer PUTs.
//! - **Read:** a point-get reads ONLY the record's bytes via the oid→byte-range
//!   index + a ranged GET — never the whole segment — so there is **no read
//!   regression** vs one-object-per-record. A batched [`get_records`] groups by
//!   segment and coalesces the ranges into one `get_ranges` per segment (so a
//!   frontier that happens to share a segment collapses to ~one round-trip).
//! - **Mixed-read-safe:** an oid not in the segment index falls back to the
//!   legacy `graph-cold/{oid}` point GET, so old data + a partial migration read
//!   correctly.
//!
//! Deferred to Phase 2 (the *conditional* read win): write-time **locality**
//! clustering (insertion-order → Louvain compaction) so a traversal frontier's
//! nodes co-locate in a segment and the GET-*count* drops on read. Phase 1 makes
//! that free to add later (the format + index already support range coalescing).
//!
//! Capability only (not yet wired into production) — like `put_with_tier` (#468)
//! was. Gating + the periodic/​shutdown flush + replacing `ColdGraphRecordStore`
//! in `shared_services` are a separate, separately-reviewed slice.
//!
//! ## Segment format (self-describing, little-endian)
//! ```text
//! [ rec_0 ][ rec_1 ] … [ rec_{n-1} ][ directory ][ dir_len: u64 ][ MAGIC: 8 ]
//! ```
//! `rec_i` = `bincode(ProximaRecordV2)`. `directory` = `bincode(Vec<(oid, off, len)>)`.
//! The trailer (`dir_len` + `MAGIC`) makes a segment self-describing so the index
//! can be rebuilt by scanning segment tails if the sidecar is lost.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use object_store::path::Path as ObjectPath;

use proximadb_kernel::error::StorageError;
use proximadb_object_store::ProximaObjectStore;
use proximadb_records::wire_v2::ProximaRecordV2;
use proximadb_records::{ProximaRecord, RecordKey, RecordStore, RecordStoreResult};
use proximadb_storage_filesystem_types::ObjectAccessTier;

const SEG_MAGIC: &[u8; 8] = b"GCSEGv1\0";
const SEG_PREFIX: &str = "graph-cold-seg";
const INDEX_KEY: &str = "graph-cold-seg/index.bin";
/// Legacy one-object-per-record prefix (mixed-read fallback).
const LEGACY_PREFIX: &str = "graph-cold";

const DEFAULT_FLUSH_BYTES: u64 = 16 * 1024 * 1024; // 16 MiB
const DEFAULT_FLUSH_COUNT: usize = 8192;

/// Where a record's bytes live within a segment object.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct RecordLoc {
    segment: String,
    offset: u64,
    len: u64,
}

#[derive(Default)]
struct Buffer {
    /// (oid, encoded ProximaRecordV2 bytes) in arrival order.
    records: Vec<(String, Vec<u8>)>,
    bytes: u64,
}

/// Segment-batched, object-storage-backed [`RecordStore`] for cold graph payloads.
pub struct ColdGraphSegmentStore {
    store: ProximaObjectStore,
    tier: ObjectAccessTier,
    /// oid → location within a segment (the read-side fast path).
    index: DashMap<String, RecordLoc>,
    buffer: Mutex<Buffer>,
    seq: AtomicU64,
    flush_bytes: u64,
    flush_count: usize,
}

impl ColdGraphSegmentStore {
    /// Open a segment store over the object-storage root `url`, writing segments at
    /// `tier`. Loads the persisted oid→segment index if present (so reads work
    /// across restarts); absent index ⇒ empty (all reads fall back to legacy).
    pub async fn from_storage_root(url: &str, tier: ObjectAccessTier) -> RecordStoreResult<Self> {
        let store = ProximaObjectStore::from_url(url)
            .map_err(|e| anyhow::anyhow!("cold segment store: open `{url}` failed: {e}"))?;
        let me = Self::new(store, tier);
        me.load_index().await?;
        Ok(me)
    }

    /// Wrap an existing [`ProximaObjectStore`] (no index load — for tests).
    pub fn new(store: ProximaObjectStore, tier: ObjectAccessTier) -> Self {
        Self {
            store,
            tier,
            index: DashMap::new(),
            buffer: Mutex::new(Buffer::default()),
            seq: AtomicU64::new(0),
            flush_bytes: DEFAULT_FLUSH_BYTES,
            flush_count: DEFAULT_FLUSH_COUNT,
        }
    }

    /// Override the flush thresholds (mainly for tests).
    pub fn with_flush_thresholds(mut self, bytes: u64, count: usize) -> Self {
        self.flush_bytes = bytes.max(1);
        self.flush_count = count.max(1);
        self
    }

    fn lock_buffer(&self) -> RecordStoreResult<std::sync::MutexGuard<'_, Buffer>> {
        self.buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("cold segment store: buffer mutex poisoned"))
    }

    /// Flush any buffered records into a segment. No-op when the buffer is empty.
    pub async fn flush(&self) -> RecordStoreResult<()> {
        let pending = {
            let mut b = self.lock_buffer()?;
            b.bytes = 0;
            std::mem::take(&mut b.records)
        };
        self.write_segment(pending).await
    }

    /// Build + publish one segment from `pending`, then update the index + sidecar.
    async fn write_segment(&self, pending: Vec<(String, Vec<u8>)>) -> RecordStoreResult<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        // Deterministic, collision-free key; not time-based (offline-build constraint).
        let seg_path = format!("{SEG_PREFIX}/seg-{seq:016x}.gcseg");

        let mut body: Vec<u8> = Vec::new();
        let mut dir: Vec<(String, u64, u64)> = Vec::with_capacity(pending.len());
        for (oid, bytes) in &pending {
            let offset = body.len() as u64;
            body.extend_from_slice(bytes);
            dir.push((oid.clone(), offset, bytes.len() as u64));
        }
        let dir_bytes = bincode::serialize(&dir)
            .map_err(|e| anyhow::anyhow!("cold segment store: encode directory failed: {e}"))?;
        body.extend_from_slice(&dir_bytes);
        body.extend_from_slice(&(dir_bytes.len() as u64).to_le_bytes());
        body.extend_from_slice(SEG_MAGIC);

        self.store
            .put_with_tier(
                &ObjectPath::from(seg_path.clone()),
                Bytes::from(body),
                self.tier,
            )
            .await
            .map_err(|e| anyhow::anyhow!("cold segment store: put `{seg_path}` failed: {e}"))?;

        for (oid, offset, len) in dir {
            self.index.insert(
                oid,
                RecordLoc {
                    segment: seg_path.clone(),
                    offset,
                    len,
                },
            );
        }
        self.persist_index().await
    }

    /// Persist the oid→location index as a bincode sidecar (best-effort durable map
    /// so reads survive restart without rescanning every segment tail).
    async fn persist_index(&self) -> RecordStoreResult<()> {
        let snapshot: Vec<(String, RecordLoc)> = self
            .index
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| anyhow::anyhow!("cold segment store: encode index failed: {e}"))?;
        self.store
            .put_with_tier(&ObjectPath::from(INDEX_KEY), Bytes::from(bytes), self.tier)
            .await
            .map_err(|e| anyhow::anyhow!("cold segment store: persist index failed: {e}"))
    }

    async fn load_index(&self) -> RecordStoreResult<()> {
        match self.store.get(&ObjectPath::from(INDEX_KEY)).await {
            Ok(bytes) => {
                let snapshot: Vec<(String, RecordLoc)> = bincode::deserialize(&bytes)
                    .map_err(|e| anyhow::anyhow!("cold segment store: decode index failed: {e}"))?;
                // Resume the segment sequence past the highest seen, so new segments
                // never clobber existing ones.
                let mut max_seq: u64 = 0;
                for (oid, loc) in snapshot {
                    if let Some(seq) = parse_seg_seq(&loc.segment) {
                        max_seq = max_seq.max(seq + 1);
                    }
                    self.index.insert(oid, loc);
                }
                self.seq.store(max_seq, Ordering::Relaxed);
                Ok(())
            }
            // No sidecar yet ⇒ empty index (reads fall back to legacy). Not an error.
            Err(StorageError::NotFound(_)) => Ok(()),
            Err(e) => Err(anyhow::anyhow!(
                "cold segment store: load index failed: {e}"
            )),
        }
    }

    fn decode(bytes: &[u8], oid: &str) -> RecordStoreResult<ProximaRecord> {
        let wire: ProximaRecordV2 = bincode::deserialize(bytes)
            .map_err(|e| anyhow::anyhow!("cold segment store: decode `{oid}` failed: {e}"))?;
        Ok(ProximaRecord::from(wire))
    }

    /// Legacy one-object-per-record fallback (mixed-read-safety with the
    /// `ColdGraphRecordStore` format). Returns `None` if absent.
    async fn legacy_get(&self, oid: &str) -> RecordStoreResult<Option<ProximaRecord>> {
        let key = ObjectPath::from(format!("{LEGACY_PREFIX}/{oid}"));
        match self.store.get(&key).await {
            Ok(bytes) => Ok(Some(Self::decode(&bytes, oid)?)),
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(e) => Err(anyhow::anyhow!(
                "cold segment store: legacy get `{oid}` failed: {e}"
            )),
        }
    }

    /// Buffered (not-yet-flushed) record bytes for `oid`, if present.
    fn buffered(&self, oid: &str) -> RecordStoreResult<Option<Vec<u8>>> {
        let b = self.lock_buffer()?;
        Ok(b.records
            .iter()
            .rev() // last write wins
            .find(|(o, _)| o == oid)
            .map(|(_, bytes)| bytes.clone()))
    }
}

#[async_trait]
impl RecordStore for ColdGraphSegmentStore {
    async fn upsert_record(&self, record: ProximaRecord) -> RecordStoreResult<ProximaRecord> {
        let wire = ProximaRecordV2::from(&record);
        let bytes = bincode::serialize(&wire).map_err(|e| {
            anyhow::anyhow!("cold segment store: encode `{}` failed: {e}", record.oid)
        })?;
        let len = bytes.len() as u64;
        crate::metrics::consumption_metrics::record_object_store_write_bytes_by_tier(
            &record.tenant_id,
            self.tier.as_str(),
            len,
        );
        let pending = {
            let mut b = self.lock_buffer()?;
            b.records.push((record.oid.clone(), bytes));
            b.bytes += len;
            if b.bytes >= self.flush_bytes || b.records.len() >= self.flush_count {
                b.bytes = 0;
                std::mem::take(&mut b.records)
            } else {
                Vec::new()
            }
        };
        self.write_segment(pending).await?;
        Ok(record)
    }

    async fn get_record(&self, key: &RecordKey) -> RecordStoreResult<Option<ProximaRecord>> {
        // 1. Buffered (created but not yet flushed).
        if let Some(bytes) = self.buffered(&key.oid)? {
            return Ok(Some(Self::decode(&bytes, &key.oid)?));
        }
        // 2. Segment index → ranged GET of just this record's bytes.
        if let Some(loc) = self.index.get(&key.oid) {
            let range = loc.offset..(loc.offset + loc.len);
            let bytes = self
                .store
                .get_range(&ObjectPath::from(loc.segment.clone()), range)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("cold segment store: range get `{}` failed: {e}", key.oid)
                })?;
            return Ok(Some(Self::decode(&bytes, &key.oid)?));
        }
        // 3. Legacy one-object-per-record (mixed-read-safety).
        self.legacy_get(&key.oid).await
    }

    async fn get_records(
        &self,
        keys: &[RecordKey],
    ) -> RecordStoreResult<Vec<Option<ProximaRecord>>> {
        let mut out: Vec<Option<ProximaRecord>> = vec![None; keys.len()];
        // Group index-resident keys by segment so each segment is ONE coalesced
        // ranged read (the depth-collapse a co-located frontier earns).
        let mut by_segment: std::collections::HashMap<String, Vec<(usize, RecordLoc)>> =
            std::collections::HashMap::new();
        let mut misses: Vec<usize> = Vec::new();
        for (i, key) in keys.iter().enumerate() {
            if let Some(bytes) = self.buffered(&key.oid)? {
                out[i] = Some(Self::decode(&bytes, &key.oid)?);
            } else if let Some(loc) = self.index.get(&key.oid) {
                by_segment
                    .entry(loc.segment.clone())
                    .or_default()
                    .push((i, loc.clone()));
            } else {
                misses.push(i);
            }
        }
        for (segment, items) in by_segment {
            let ranges: Vec<std::ops::Range<u64>> = items
                .iter()
                .map(|(_, loc)| loc.offset..(loc.offset + loc.len))
                .collect();
            let bufs = self
                .store
                .get_ranges(&ObjectPath::from(segment.clone()), &ranges)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("cold segment store: get_ranges `{segment}` failed: {e}")
                })?;
            for ((slot, _), bytes) in items.into_iter().zip(bufs) {
                out[slot] = Some(Self::decode(&bytes, &keys[slot].oid)?);
            }
        }
        // Index misses → legacy fallback (concurrently).
        let legacy =
            futures::future::try_join_all(misses.iter().map(|&i| self.legacy_get(&keys[i].oid)))
                .await?;
        for (&i, rec) in misses.iter().zip(legacy) {
            out[i] = rec;
        }
        Ok(out)
    }

    async fn delete_record(&self, key: &RecordKey) -> RecordStoreResult<bool> {
        // Remove from the buffer and the index (segment bytes become garbage,
        // reclaimed by a future compaction — Phase 2). Also best-effort delete a
        // legacy object if one exists.
        let in_buffer = {
            let mut b = self.lock_buffer()?;
            let before = b.records.len();
            b.records.retain(|(o, _)| o != &key.oid);
            before != b.records.len()
        };
        let in_index = self.index.remove(&key.oid).is_some();
        if in_index {
            self.persist_index().await?;
        }
        let legacy_existed = match self
            .store
            .delete(&ObjectPath::from(format!("{LEGACY_PREFIX}/{}", key.oid)))
            .await
        {
            Ok(()) => true,
            Err(StorageError::NotFound(_)) => false,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "cold segment store: legacy delete `{}` failed: {e}",
                    key.oid
                ));
            }
        };
        Ok(in_buffer || in_index || legacy_existed)
    }
}

/// Parse the `seq` out of a `graph-cold-seg/seg-{seq:016x}.gcseg` path.
fn parse_seg_seq(path: &str) -> Option<u64> {
    let name = path.rsplit('/').next()?;
    let hex = name.strip_prefix("seg-")?.strip_suffix(".gcseg")?;
    u64::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_store() -> ColdGraphSegmentStore {
        let store = ProximaObjectStore::from_url("memory://").expect("memory store");
        ColdGraphSegmentStore::new(store, ObjectAccessTier::Cool)
    }

    fn record(oid: &str) -> ProximaRecord {
        ProximaRecord {
            oid: oid.to_string(),
            tenant_id: "t".to_string(),
            ..ProximaRecord::default()
        }
    }

    #[tokio::test]
    async fn buffered_record_is_readable_before_flush() {
        let store = mem_store(); // high thresholds ⇒ stays buffered
        store
            .upsert_record(record("graph/g/node/a"))
            .await
            .expect("upsert");
        let got = store
            .get_record(&RecordKey::new("graph/g/node/a"))
            .await
            .expect("get")
            .expect("present in buffer");
        assert_eq!(got.oid, "graph/g/node/a");
        // Nothing flushed yet.
        assert!(store.index.is_empty());
    }

    #[tokio::test]
    async fn flush_batches_into_one_segment_and_reads_via_index() {
        let store = mem_store();
        for id in ["a", "b", "c"] {
            store
                .upsert_record(record(&format!("graph/g/node/{id}")))
                .await
                .expect("upsert");
        }
        store.flush().await.expect("flush");
        // All three share ONE segment.
        assert_eq!(
            store.seq.load(Ordering::Relaxed),
            1,
            "exactly one segment written"
        );
        for id in ["a", "b", "c"] {
            let oid = format!("graph/g/node/{id}");
            let got = store
                .get_record(&RecordKey::new(oid.clone()))
                .await
                .expect("get")
                .expect("present");
            assert_eq!(got.oid, oid);
        }
    }

    #[tokio::test]
    async fn count_threshold_triggers_flush() {
        let store = mem_store().with_flush_thresholds(u64::MAX, 2);
        store
            .upsert_record(record("graph/g/node/a"))
            .await
            .expect("a");
        assert_eq!(store.seq.load(Ordering::Relaxed), 0, "not flushed at 1");
        store
            .upsert_record(record("graph/g/node/b"))
            .await
            .expect("b");
        assert_eq!(store.seq.load(Ordering::Relaxed), 1, "flushed at count=2");
        assert!(store.index.contains_key("graph/g/node/a"));
    }

    #[tokio::test]
    async fn get_records_batches_one_segment_in_order_with_miss() {
        let store = mem_store();
        for id in ["a", "c"] {
            store
                .upsert_record(record(&format!("graph/g/node/{id}")))
                .await
                .expect("upsert");
        }
        store.flush().await.expect("flush");
        let keys = [
            RecordKey::new("graph/g/node/a"),
            RecordKey::new("graph/g/node/b"), // absent
            RecordKey::new("graph/g/node/c"),
        ];
        let got = store.get_records(&keys).await.expect("get_records");
        assert_eq!(
            got[0].as_ref().map(|r| r.oid.as_str()),
            Some("graph/g/node/a")
        );
        assert!(got[1].is_none());
        assert_eq!(
            got[2].as_ref().map(|r| r.oid.as_str()),
            Some("graph/g/node/c")
        );
    }

    #[tokio::test]
    async fn index_reloads_from_sidecar_across_reopen() {
        // Share one in-memory object store across two store instances.
        let backing = ProximaObjectStore::from_url("memory://").expect("mem");
        let s1 = ColdGraphSegmentStore::new(backing.clone(), ObjectAccessTier::Cool);
        s1.upsert_record(record("graph/g/node/a"))
            .await
            .expect("upsert");
        s1.flush().await.expect("flush");

        let s2 = ColdGraphSegmentStore::new(backing, ObjectAccessTier::Cool);
        s2.load_index().await.expect("load index");
        let got = s2
            .get_record(&RecordKey::new("graph/g/node/a"))
            .await
            .expect("get")
            .expect("present after reload");
        assert_eq!(got.oid, "graph/g/node/a");
        // Sequence resumed past the loaded segment.
        assert_eq!(s2.seq.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn delete_removes_from_index() {
        let store = mem_store();
        store
            .upsert_record(record("graph/g/node/a"))
            .await
            .expect("upsert");
        store.flush().await.expect("flush");
        assert!(
            store
                .delete_record(&RecordKey::new("graph/g/node/a"))
                .await
                .expect("delete")
        );
        assert!(
            store
                .get_record(&RecordKey::new("graph/g/node/a"))
                .await
                .expect("get")
                .is_none()
        );
    }
}
