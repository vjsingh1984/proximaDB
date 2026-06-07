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
//! read_parquet_batches(path, schema, batch_size)
//!     = ParquetObjectReader::new(inner_store, full_key)      // F1 (footer + range reads)
//!     → ParquetRecordBatchStreamBuilder (projection by name) // parquet async reader
//!     → lazy RecordBatch stream (row groups read on demand)
//! ```
//!
//! ## v1 scope / follow-ups
//! - **Schema-less write.** The trait's `write_records_to_parquet` takes only `&[ProximaRecord]`,
//!   so the file schema is inferred from the records (see `infer_proxima_schema`). When a
//!   catalog-authoritative schema becomes available at the call site (F3 wiring), prefer
//!   passing it instead of inferring.
//! - **Read is footer-driven + projected.** `read_parquet_batches` opens a
//!   [`ParquetObjectReader`], so it fetches only the footer (suffix/range read) and then
//!   range-reads row groups lazily as the stream is polled — no whole-object download. A
//!   non-empty `schema` arg is applied as a column projection (by name); reorder/coercion
//!   to the requested schema and value-predicate row-group pruning (needs a predicate this
//!   signature lacks) remain follow-ups layered on the same metadata.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema as ArrowSchema;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use object_store::ObjectStore;
use object_store::path::Path;
use parquet::arrow::ParquetRecordBatchStreamBuilder;
use parquet::arrow::ProjectionMask;
use parquet::arrow::async_reader::ParquetObjectReader;
use proximadb_kernel::error::StorageError;
use proximadb_object_store::ProximaObjectStore;
use proximadb_records::ProximaRecord;
use proximadb_storage_common::object_store_bridge::ObjectStoreBridge;
use proximadb_storage_common::proxima_arrow::infer_proxima_schema;
use proximadb_storage_common::proxima_parquet::proxima_records_to_parquet_bytes;
use proximadb_storage_common::proxima_schema::ProximaSchema;

/// Map a Parquet read/decode failure into a [`StorageError`] with operation context.
fn read_err(context: &str, e: impl std::fmt::Display) -> StorageError {
    StorageError::Serialization(format!("iceberg-engine: {context}: {e}"))
}

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

    /// Write `records` to a Parquet object at `path` using a CALLER-PROVIDED
    /// (catalog-authoritative) [`ProximaSchema`] instead of inferring one from
    /// the records.
    ///
    /// This is the F3 wiring hook flagged in this module's docs: when the
    /// catalog knows a table's canonical schema, pass it here so the written
    /// file's column set, order, types, and nullability match the catalog
    /// exactly — including columns that are declared in the schema but absent
    /// from every record in this batch. Such columns are written as all-null
    /// columns (the canonical `ProximaSchema`-driven Arrow mapping appends a
    /// null for any record missing the prop), which schema *inference* would
    /// omit entirely. That makes the on-disk file shape stable across batches
    /// regardless of which optional fields happen to be populated.
    ///
    /// The trait [`ObjectStoreBridge::write_records_to_parquet`] delegates here
    /// after inferring a schema from the records.
    ///
    /// `tenant_id` carries the [`TenantContext`](proximadb_records) owner of this
    /// write to the I/O boundary so the billing follow-up can attribute
    /// `proximadb_object_store_ops_total` / `proximadb_storage_bytes_seconds` to a
    /// tenant. It is threaded through both write entry points (this schema-explicit
    /// one and the trait's schema-less one) so no write path is tenant-blind; the
    /// metric emission itself is a follow-up (no telemetry sink is wired here yet).
    pub async fn write_records_to_parquet_with_schema(
        &self,
        path: &Path,
        records: &[ProximaRecord],
        schema: &ProximaSchema,
        _tenant_id: Option<&str>,
    ) -> Result<(), StorageError> {
        let bytes = proxima_records_to_parquet_bytes(records, schema, None)?;
        self.store.put(path, Bytes::from(bytes)).await
    }
}

#[async_trait]
impl ObjectStoreBridge for IcebergObjectStoreBridge {
    fn inner_store(&self) -> Arc<dyn ObjectStore> {
        self.store.store()
    }

    /// Footer-driven, range-read, lazily-streamed Parquet read.
    ///
    /// Instead of a full-object `get` followed by an in-memory decode, this opens
    /// a [`ParquetObjectReader`] over the wrapped object store, which fetches only
    /// the Parquet footer (via a suffix/range read), parses the file metadata, and
    /// then range-reads row groups **on demand** as the returned stream is polled.
    /// On a cloud store that turns a whole-file download into a footer read plus
    /// per-row-group ranges; on small in-memory objects it is equivalent.
    ///
    /// `schema` is honored as a **column projection**: when it carries fields, only
    /// the file columns whose (root) names appear in it are read (matched by name
    /// via [`ProjectionMask::columns`]; requested names absent from the file are
    /// skipped, and file order is preserved — reordering/coercion to the requested
    /// schema is a follow-up). An empty schema preserves the legacy "all columns"
    /// contract. Value-predicate row-group pruning needs a predicate this signature
    /// does not carry; it remains a follow-up layered on the same metadata.
    async fn read_parquet_batches(
        &self,
        path: &Path,
        schema: Arc<ArrowSchema>,
        batch_size: usize,
        _tenant_id: Option<&str>,
    ) -> Result<BoxStream<'static, Result<RecordBatch, StorageError>>, StorageError> {
        // Use the raw inner store + base-resolved key: ParquetObjectReader issues
        // its own range reads, so it must address the concrete object key the way
        // ProximaObjectStore's own methods do (which apply the base prefix).
        let reader = ParquetObjectReader::new(self.store.store(), self.full_object_path(path));
        let mut builder = ParquetRecordBatchStreamBuilder::new(reader)
            .await
            .map_err(|e| read_err("open parquet stream (footer)", e))?
            .with_batch_size(batch_size);

        if !schema.fields().is_empty() {
            let mask = ProjectionMask::columns(
                builder.parquet_schema(),
                schema.fields().iter().map(|f| f.name().as_str()),
            );
            builder = builder.with_projection(mask);
        }

        let stream = builder
            .build()
            .map_err(|e| read_err("build parquet stream", e))?;
        Ok(stream
            .map(|batch| batch.map_err(|e| read_err("read parquet batch", e)))
            .boxed())
    }

    async fn write_records_to_parquet(
        &self,
        path: &Path,
        records: &[ProximaRecord],
        tenant_id: Option<&str>,
    ) -> Result<(), StorageError> {
        // Schema-less entry point: infer the schema from the records, then
        // delegate to the schema-explicit writer so both paths share one
        // implementation. The tenant flows through to the single I/O boundary.
        let schema = infer_proxima_schema(records);
        self.write_records_to_parquet_with_schema(path, records, &schema, tenant_id)
            .await
    }

    async fn fetch_vector_segment(&self, path: &Path, _tenant_id: Option<&str>) -> Result<Vec<u8>, StorageError> {
        Ok(self.store.get(path).await?.to_vec())
    }

    async fn persist_vector_segment(&self, path: &Path, data: &[u8], _tenant_id: Option<&str>) -> Result<(), StorageError> {
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
    use arrow_schema::{DataType, Field};
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

        b.write_records_to_parquet(&path, &[r0, r1], None).await.unwrap();

        let mut stream = b
            .read_parquet_batches(&path, Arc::new(ArrowSchema::empty()), 1024, None)
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

    /// A non-empty `schema` arg projects the read to only the named columns (by name),
    /// dropping the rest — and an absent requested name is silently skipped, not an error.
    #[tokio::test]
    async fn read_projects_to_requested_columns_by_name() {
        let b = bridge();
        let path = Path::from("warehouse/t/proj.parquet");

        let r0 = record(
            "r0",
            vec![
                ("name", ProximaValue::String("alice".into())),
                ("age", ProximaValue::Int64(30)),
                ("city", ProximaValue::String("paris".into())),
            ],
        );
        let r1 = record(
            "r1",
            vec![
                ("name", ProximaValue::String("bob".into())),
                ("age", ProximaValue::Int64(41)),
                ("city", ProximaValue::String("rome".into())),
            ],
        );
        b.write_records_to_parquet(&path, &[r0, r1], None).await.unwrap();

        // Project to {name, age} plus a name that does not exist in the file.
        let projection = Arc::new(ArrowSchema::new(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("age", DataType::Int64, true),
            Field::new("does_not_exist", DataType::Utf8, true),
        ]));

        let mut stream = b
            .read_parquet_batches(&path, projection, 1024, None)
            .await
            .unwrap();

        let mut batches = Vec::new();
        while let Some(batch) = stream.next().await {
            batches.push(batch.unwrap());
        }
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 2);

        let batch = &batches[0];
        // Only the two existing projected columns are present — `city` is pruned and the
        // bogus requested name does not materialize a column.
        assert_eq!(batch.num_columns(), 2, "projection must drop unrequested columns");
        assert!(batch.column_by_name("name").is_some());
        assert!(batch.column_by_name("age").is_some());
        assert!(batch.column_by_name("city").is_none(), "city was not requested");
        assert!(batch.column_by_name("does_not_exist").is_none());

        let names = batch
            .column_by_name("name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "alice");
        assert_eq!(names.value(1), "bob");
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
                None,
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

        b.persist_vector_segment(&path, &payload, None).await.unwrap();
        let got = b.fetch_vector_segment(&path, None).await.unwrap();
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
        b.write_records_to_parquet(&path, &[r0], None).await.unwrap();

        let listed = b.list_objects(&Path::from("t/data")).await.unwrap();
        assert_eq!(listed.len(), 1, "the written object must be discoverable");
        assert_eq!(
            listed[0].as_ref(),
            "t/data/part-0.parquet",
            "list_objects must strip the store base so the key round-trips through read"
        );

        // The listed key must be directly readable (the bridge re-applies the base).
        let mut stream = b
            .read_parquet_batches(&listed[0], Arc::new(ArrowSchema::empty()), 1024, None)
            .await
            .unwrap();
        let mut rows = 0usize;
        while let Some(batch) = stream.next().await {
            rows += batch.unwrap().num_rows();
        }
        assert_eq!(rows, 1, "the listed key must read back the written record");
    }

    async fn read_first_batch(b: &IcebergObjectStoreBridge, path: &Path) -> RecordBatch {
        let mut stream = b
            .read_parquet_batches(path, Arc::new(ArrowSchema::empty()), 1024, None)
            .await
            .unwrap();
        stream.next().await.expect("at least one batch").unwrap()
    }

    /// A schema-explicit write honors the CALLER's (catalog-authoritative)
    /// schema, not inference: a column declared in the schema but absent from
    /// every record is written as an all-null column, whereas the schema-less
    /// (inferred) write omits it entirely. This is the F3 contract — the
    /// on-disk file shape follows the catalog, not whichever optional fields a
    /// given batch happens to populate.
    #[tokio::test]
    async fn write_with_explicit_schema_null_fills_columns_absent_from_records() {
        use arrow_array::Float64Array;
        use proximadb_data_model::ProximaType;
        use proximadb_storage_common::proxima_schema::ProximaColumn;

        fn col(id: i32, name: &str, dt: ProximaType, nullable: bool) -> ProximaColumn {
            ProximaColumn {
                id,
                name: name.to_string(),
                data_type: dt,
                nullable,
                default_value: None,
                comment: None,
                properties: std::collections::HashMap::new(),
                is_deleted: false,
                original_id: None,
            }
        }

        let b = bridge();
        // Records carry only `name`; NONE carry `score`.
        let r0 = record("r0", vec![("name", ProximaValue::String("alice".into()))]);
        let r1 = record("r1", vec![("name", ProximaValue::String("bob".into()))]);

        // Catalog-authoritative schema declares `score` anyway.
        let schema = ProximaSchema::from_columns(
            "wh".to_string(),
            vec![
                col(1, "name", ProximaType::String, true),
                col(2, "score", ProximaType::Float64, true),
            ],
            vec![1],
        );

        // Schema-explicit write: `score` must survive as an all-null column.
        let explicit = Path::from("wh/explicit.parquet");
        b.write_records_to_parquet_with_schema(&explicit, &[r0.clone(), r1.clone()], &schema, None)
            .await
            .unwrap();
        let batch = read_first_batch(&b, &explicit).await;
        let score = batch
            .column_by_name("score")
            .expect("explicit schema must keep the declared `score` column")
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(score.len(), 2);
        assert!(
            score.is_null(0) && score.is_null(1),
            "a column absent from every record must be an all-null column"
        );

        // Schema-less (inferred) write: `score` is omitted (no record carries it).
        let inferred = Path::from("wh/inferred.parquet");
        b.write_records_to_parquet(&inferred, &[r0, r1], None).await.unwrap();
        let batch = read_first_batch(&b, &inferred).await;
        assert!(
            batch.column_by_name("score").is_none(),
            "inference must omit a column that no record carries"
        );
    }
}
