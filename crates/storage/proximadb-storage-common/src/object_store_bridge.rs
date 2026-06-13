use arrow_array::RecordBatch;
use arrow_schema::Schema as ArrowSchema;
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use object_store::ObjectStore;
use object_store::path::Path;
use proximadb_kernel::error::StorageError;
use proximadb_records::ProximaRecord;
use std::sync::Arc;

use crate::proxima_schema::ProximaSchema;

pub use object_store::ObjectStore as BridgeObjectStore;
pub use object_store::memory::InMemory as BridgeInMemoryObjectStore;
pub use object_store::path::Path as BridgeObjectPath;

/// The canonical bridge interface between ProximaDB's compute/storage engines
/// and decoupled object storage (S3, GCS, Azure, Local).
///
/// This serves two primary architectural paths:
/// 1. Data Warehouse/Relational Workloads: Writing and reading Parquet files managed by Iceberg.
/// 2. Vector/ANN Workloads: Persisting and fetching specialized PAX blocks for high-performance retrieval.
#[async_trait]
pub trait ObjectStoreBridge: Send + Sync {
    /// Returns a reference to the underlying `ObjectStore` implementation.
    fn inner_store(&self) -> Arc<dyn ObjectStore>;

    /// Reads a Parquet file from object storage and yields a stream of Arrow `RecordBatch`es.
    /// This is the primary ingestion path for DataFusion logical plans.
    async fn read_parquet_batches(
        &self,
        path: &Path,
        schema: Arc<ArrowSchema>,
        batch_size: usize,
        tenant_id: Option<&str>,
    ) -> Result<BoxStream<'static, Result<RecordBatch, StorageError>>, StorageError>;

    /// Writes a stream of `ProximaRecord`s into a Parquet file on object storage.
    /// This is the primary output path for DML/Analytics operations.
    /// Implementers MUST emit `proximadb_object_store_ops_total` and `proximadb_storage_bytes_seconds` for billing.
    async fn write_records_to_parquet(
        &self,
        path: &Path,
        records: &[ProximaRecord],
        tenant_id: Option<&str>,
    ) -> Result<(), StorageError>;

    /// Schema-explicit Parquet write: writes `records` using the CALLER-PROVIDED
    /// (catalog-authoritative) [`ProximaSchema`] instead of inferring one from the
    /// records, so the file's column set, order, types, and nullability match the
    /// catalog exactly — including columns declared in the schema but absent from
    /// every record (written as all-null), which inference would silently drop.
    /// This is the correct write path for warehouse materialization, where the
    /// published Parquet must round-trip through `SELECT *` against the catalog shape.
    ///
    /// Default: falls back to the schema-less (inferred) [`write_records_to_parquet`]
    /// for bridges without a schema-aware path.
    ///
    /// [`write_records_to_parquet`]: ObjectStoreBridge::write_records_to_parquet
    async fn write_records_to_parquet_with_schema(
        &self,
        path: &Path,
        records: &[ProximaRecord],
        schema: &ProximaSchema,
        tenant_id: Option<&str>,
    ) -> Result<(), StorageError> {
        let _ = schema;
        self.write_records_to_parquet(path, records, tenant_id).await
    }

    /// Fetches a specialized PAX block or Segment (SST, HELIX, etc.) from object storage
    /// into memory for high-performance Vector SIMD scans.
    async fn fetch_vector_segment(&self, path: &Path, tenant_id: Option<&str>) -> Result<Vec<u8>, StorageError>;

    /// Persists a specialized PAX block or Segment to decoupled object storage.
    /// Implementers MUST emit `proximadb_object_store_ops_total` and `proximadb_storage_bytes_seconds` for billing.
    async fn persist_vector_segment(&self, path: &Path, data: &[u8], tenant_id: Option<&str>) -> Result<(), StorageError>;

    /// Enumerate the object keys under `prefix`, returned as paths that are
    /// directly consumable by this bridge's read methods (`read_parquet_batches`
    /// / `fetch_vector_segment`).
    ///
    /// This is the read-side discovery seam: a record store does not track which
    /// files the write path produced, so it lists them to read them back.
    ///
    /// The default implementation enumerates the raw [`inner_store`], returning
    /// each object's full key. This is correct for stores whose base prefix is
    /// empty (`memory://`, bucket-root). Implementations that wrap the store with
    /// a non-empty base prefix (e.g. `IcebergObjectStoreBridge` over
    /// `ProximaObjectStore`) MUST override this to return base-relative keys, so
    /// the returned paths round-trip through the bridge's read methods (which
    /// re-apply the base).
    ///
    /// [`inner_store`]: ObjectStoreBridge::inner_store
    async fn list_objects(&self, prefix: &Path) -> Result<Vec<Path>, StorageError> {
        let store = self.inner_store();
        let mut listing = store.list(Some(prefix));
        let mut paths = Vec::new();
        while let Some(meta) = listing.next().await {
            let meta = meta.map_err(|err| {
                StorageError::DiskIO(std::io::Error::other(format!(
                    "object-store list under `{prefix}` failed: {err}"
                )))
            })?;
            paths.push(meta.location);
        }
        Ok(paths)
    }
}
