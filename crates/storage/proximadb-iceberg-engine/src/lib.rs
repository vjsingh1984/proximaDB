//! # proximadb-iceberg-engine — object-store `ObjectStoreBridge` implementation (F2)
//!
//! The durable **base tier** of the decoupled-storage warehouse: a concrete
//! [`ObjectStoreBridge`] over object storage that writes/reads Parquet for the
//! relational/analytics path and raw segment bytes for the vector/PAX path. It is the
//! capstone of the warehouse serialization stack — it owns no new serialization logic, it
//! *composes* the lower foundation:
//!
//! - **F1** [`ProximaObjectStore`] — the `object_store` plumbing (`put`/`get`/`get_range`).
//! - **F1.5** `proxima_arrow::proxima_records_to_record_batch` — schema-driven record→Arrow.
//! - **F1.6** `proxima_parquet` — in-memory `RecordBatch` ⇄ Parquet-`bytes`.
//! - schema inference (`proxima_arrow::infer_proxima_schema`) for the schema-less write API.
//!
//! ```text
//! write_records_to_parquet(path, records)
//!     = infer_proxima_schema(records)                       // F1.5 (inference)
//!     → proxima_records_to_parquet_bytes(records, &schema)  // F1.5 + F1.6
//!     → ProximaObjectStore::put(path, bytes)                // F1
//!
//! read_parquet_batches(path, _schema, batch_size)
//!     = ProximaObjectStore::get(path)                       // F1
//!     → parquet_bytes_to_record_batches_with_batch_size     // F1.6
//! ```
//!
//! ## v1 scope / follow-ups
//! - **Schema-less write.** The trait's `write_records_to_parquet` takes only `&[ProximaRecord]`,
//!   so the file schema is inferred from the records (see `infer_proxima_schema`). When a
//!   catalog-authoritative schema becomes available at the call site (F3 wiring), prefer
//!   passing it instead of inferring.
//! - **Read schema is advisory in v1.** `read_parquet_batches`' `schema` argument is the
//!   caller's expected shape; this impl returns the file's batches as written (the Parquet
//!   file embeds its own schema). Projection/coercion to the requested schema and Iceberg
//!   manifest/row-group pruning are follow-ups (F5), layered on the same `ProximaObjectStore`.
//! - **Eager read.** The returned `'static` stream is materialized eagerly (the whole object
//!   is already an in-memory `Bytes`); true streaming/range reads are an F5 optimization.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema as ArrowSchema;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use object_store::ObjectStore;
use object_store::path::Path;
use proximadb_kernel::error::StorageError;
use proximadb_object_store::ProximaObjectStore;
use proximadb_records::ProximaRecord;
use proximadb_storage_common::object_store_bridge::ObjectStoreBridge;
use proximadb_storage_common::proxima_arrow::infer_proxima_schema;
use proximadb_storage_common::proxima_parquet::{
    parquet_bytes_to_record_batches_with_batch_size, proxima_records_to_parquet_bytes,
};

/// An [`ObjectStoreBridge`] backed by a [`ProximaObjectStore`] (any `object_store` backend:
/// local `file://`, `memory://`, or cloud `s3://`/`gs://`/`az://` when the F1 crate's cloud
/// features are enabled). One bridge serves an entire store/bucket; paths select the object.
#[derive(Clone)]
pub struct IcebergObjectStoreBridge {
    store: ProximaObjectStore,
}

impl IcebergObjectStoreBridge {
    /// Wrap an existing object-store handle.
    pub fn new(store: ProximaObjectStore) -> Self {
        Self { store }
    }

    /// Open the bridge directly from a URL (see [`ProximaObjectStore::from_url`]).
    pub fn from_url(url: &str) -> Result<Self, StorageError> {
        Ok(Self {
            store: ProximaObjectStore::from_url(url)?,
        })
    }

    /// Borrow the underlying [`ProximaObjectStore`].
    pub fn object_store(&self) -> &ProximaObjectStore {
        &self.store
    }

    /// Resolve a bridge-relative path to the concrete key used by the wrapped
    /// object store. Range-based readers use this to avoid a full object GET.
    pub fn full_object_path(&self, path: &Path) -> Path {
        self.store.full_path(path)
    }
}

#[async_trait]
impl ObjectStoreBridge for IcebergObjectStoreBridge {
    fn inner_store(&self) -> Arc<dyn ObjectStore> {
        self.store.store()
    }

    async fn read_parquet_batches(
        &self,
        path: &Path,
        _schema: Arc<ArrowSchema>,
        batch_size: usize,
    ) -> Result<BoxStream<'static, Result<RecordBatch, StorageError>>, StorageError> {
        let bytes = self.store.get(path).await?;
        let batches = parquet_bytes_to_record_batches_with_batch_size(bytes, batch_size)?;
        Ok(futures::stream::iter(batches.into_iter().map(Ok)).boxed())
    }

    async fn write_records_to_parquet(
        &self,
        path: &Path,
        records: &[ProximaRecord],
    ) -> Result<(), StorageError> {
        let schema = infer_proxima_schema(records);
        let bytes = proxima_records_to_parquet_bytes(records, &schema, None)?;
        self.store.put(path, Bytes::from(bytes)).await
    }

    async fn fetch_vector_segment(&self, path: &Path) -> Result<Vec<u8>, StorageError> {
        Ok(self.store.get(path).await?.to_vec())
    }

    async fn persist_vector_segment(&self, path: &Path, data: &[u8]) -> Result<(), StorageError> {
        self.store.put(path, Bytes::copy_from_slice(data)).await
    }

    /// Base-aware override of the trait default: list through [`ProximaObjectStore`]
    /// (which applies the store's base prefix) and strip that base from each
    /// returned key, so the paths are base-relative and round-trip through
    /// `read_parquet_batches` / `fetch_vector_segment` (which re-apply the base).
    async fn list_objects(&self, prefix: &Path) -> Result<Vec<Path>, StorageError> {
        let base = self.store.base().as_ref().to_string();
        let metas = self.store.list(Some(prefix)).await?;
        Ok(metas
            .into_iter()
            .map(|meta| {
                if base.is_empty() {
                    meta.location
                } else {
                    let key = meta.location.as_ref();
                    let relative = key
                        .strip_prefix(&base)
                        .map(|rest| rest.trim_start_matches('/'))
                        .unwrap_or(key);
                    Path::from(relative)
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, Int64Array, StringArray};
    use object_store::memory::InMemory;
    use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaTreeNode, ProximaValue};

    fn bridge() -> IcebergObjectStoreBridge {
        IcebergObjectStoreBridge::new(ProximaObjectStore::new(Arc::new(InMemory::new())))
    }

    fn record(oid: &str, props: Vec<(&str, ProximaValue)>) -> ProximaRecord {
        let mut r = ProximaRecord {
            oid: oid.to_string(),
            ..Default::default()
        };
        for (k, v) in props {
            r.props.insert(k.to_string(), ProximaTreeNode::Value(v));
        }
        r
    }

    /// records → write_records_to_parquet → read_parquet_batches must preserve rows + values.
    #[tokio::test]
    async fn records_round_trip_through_object_store() {
        let b = bridge();
        let path = Path::from("warehouse/t/data.parquet");

        let mut r0 = record(
            "r0",
            vec![
                ("name", ProximaValue::String("alice".into())),
                ("age", ProximaValue::Int64(30)),
            ],
        );
        r0.embeddings.push(EmbeddingCell {
            values: EmbeddingValues::Fp32(vec![1.0, 2.0, 3.0]),
            ..Default::default()
        });
        let r1 = record(
            "r1",
            vec![
                ("name", ProximaValue::String("bob".into())),
                ("age", ProximaValue::Int64(41)),
            ],
        );

        b.write_records_to_parquet(&path, &[r0, r1]).await.unwrap();

        let mut stream = b
            .read_parquet_batches(&path, Arc::new(ArrowSchema::empty()), 1024)
            .await
            .unwrap();

        let mut batches = Vec::new();
        while let Some(batch) = stream.next().await {
            batches.push(batch.unwrap());
        }
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 2);

        let batch = &batches[0];
        let names = batch
            .column_by_name("name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "alice");
        assert_eq!(names.value(1), "bob");
        let ages = batch
            .column_by_name("age")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ages.value(0), 30);
        assert_eq!(ages.value(1), 41);
        // The inferred dense-vector column round-tripped as a column too.
        assert!(batch.column_by_name("vector").is_some());
    }

    /// Reading a path that was never written must surface a NotFound, not panic.
    #[tokio::test]
    async fn read_missing_object_errors() {
        let b = bridge();
        let res = b
            .read_parquet_batches(
                &Path::from("nope.parquet"),
                Arc::new(ArrowSchema::empty()),
                64,
            )
            .await;
        assert!(res.is_err());
    }

    /// Raw vector-segment bytes round-trip through persist/fetch.
    #[tokio::test]
    async fn vector_segment_round_trip() {
        let b = bridge();
        let path = Path::from("warehouse/vec/seg_000.pax");
        let payload: Vec<u8> = (0u8..200).collect();

        b.persist_vector_segment(&path, &payload).await.unwrap();
        let got = b.fetch_vector_segment(&path).await.unwrap();
        assert_eq!(got, payload);
    }

    /// With a NON-EMPTY store base, `list_objects` must return base-relative keys
    /// that round-trip through the read methods. (A raw `inner_store().list` would
    /// return base-prefixed keys that then double-prepend the base on read.)
    #[tokio::test]
    async fn list_objects_is_base_relative_and_round_trips_with_non_empty_base() {
        let b = IcebergObjectStoreBridge::from_url("memory:///warehouse").unwrap();
        assert!(
            !b.object_store().base().as_ref().is_empty(),
            "base must be non-empty"
        );

        let path = Path::from("t/data/part-0.parquet");
        let r0 = record("r0", vec![("name", ProximaValue::String("alice".into()))]);
        b.write_records_to_parquet(&path, &[r0]).await.unwrap();

        let listed = b.list_objects(&Path::from("t/data")).await.unwrap();
        assert_eq!(listed.len(), 1, "the written object must be discoverable");
        assert_eq!(
            listed[0].as_ref(),
            "t/data/part-0.parquet",
            "list_objects must strip the store base so the key round-trips through read"
        );

        // The listed key must be directly readable (the bridge re-applies the base).
        let mut stream = b
            .read_parquet_batches(&listed[0], Arc::new(ArrowSchema::empty()), 1024)
            .await
            .unwrap();
        let mut rows = 0usize;
        while let Some(batch) = stream.next().await {
            rows += batch.unwrap().num_rows();
        }
        assert_eq!(rows, 1, "the listed key must read back the written record");
    }
}
