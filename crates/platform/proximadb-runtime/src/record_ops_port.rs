//! Bulk record-operations port (TD-104 S3).
//!
//! The Arrow Flight ingest path (`do_put`) consumes record-batch insert/upsert/
//! delete. This port lets the Flight service depend on the contract instead of the
//! concrete root-crate `UnifiedHandlers`. Implemented by the root `UnifiedHandlers`,
//! delegating to its existing `handle_record_*_for_tenant` methods.
//!
//! Inputs are canonical (`ProximaRecord` from `proximadb-records`); the result is the
//! relocated [`crate::batch_result::BatchOperationResult`]. No durable authority lives
//! here — this is a façade over the same vector/record services.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_records::ProximaRecord;

use crate::batch_result::BatchOperationResult;

#[async_trait]
pub trait RecordOpsPort: Send + Sync {
    /// Insert a batch of canonical records into `collection_id`.
    async fn insert_record_batch(
        &self,
        collection_id: &str,
        records: Vec<ProximaRecord>,
        tenant_id: Option<&str>,
    ) -> Result<BatchOperationResult>;

    /// Upsert a batch of canonical records into `collection_id`.
    async fn upsert_record_batch(
        &self,
        collection_id: &str,
        records: Vec<ProximaRecord>,
        tenant_id: Option<&str>,
    ) -> Result<BatchOperationResult>;

    /// Delete records by id from `collection_id`.
    async fn delete_record_batch(
        &self,
        collection_id: &str,
        record_ids: Vec<String>,
        tenant_id: Option<&str>,
    ) -> Result<BatchOperationResult>;
}
