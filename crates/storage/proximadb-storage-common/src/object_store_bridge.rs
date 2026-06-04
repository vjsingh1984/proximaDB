use arrow_array::RecordBatch;
use arrow_schema::Schema as ArrowSchema;
use async_trait::async_trait;
use futures::stream::BoxStream;
use object_store::ObjectStore;
use object_store::path::Path;
use proximadb_kernel::error::StorageError;
use proximadb_records::ProximaRecord;
use std::sync::Arc;

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
    ) -> Result<BoxStream<'static, Result<RecordBatch, StorageError>>, StorageError>;

    /// Writes a stream of `ProximaRecord`s into a Parquet file on object storage.
    /// This is the primary output path for DML/Analytics operations.
    async fn write_records_to_parquet(
        &self,
        path: &Path,
        records: &[ProximaRecord],
    ) -> Result<(), StorageError>;

    /// Fetches a specialized PAX block or Segment (SST, HELIX, etc.) from object storage
    /// into memory for high-performance Vector SIMD scans.
    async fn fetch_vector_segment(&self, path: &Path) -> Result<Vec<u8>, StorageError>;

    /// Persists a specialized PAX block or Segment to decoupled object storage.
    async fn persist_vector_segment(&self, path: &Path, data: &[u8]) -> Result<(), StorageError>;
}
