//! Document-facade record route port (ADR-009 / RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE).
//!
//! The document modality is a *projection* over the canonical record/vector spine,
//! not an independent durable store. This port lets `DocumentService`'s canonical
//! branch route through the exact tenant-scoped record surface REST v2 already uses
//! (`RecordOpsService::handle_record_*_for_tenant`) without depending on the concrete
//! root-crate service — so a document written on either surface is structurally
//! visible on the other (closes the store-split), metered, and stored once.
//!
//! Shapes are facade-simple (`usize` / `Vec<ProximaRecord>`); durable authority stays
//! in the vector store — no state lives here. Reads use the **scan** surface (not the
//! point-get), because a full [`ProximaRecord`] — carrying `labels` + `variation_id` —
//! is required to rebuild the document facade via `canonical_document_from_record`; the
//! search-shaped point-get response drops those fields.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_records::ProximaRecord;

/// Route for the document facade's canonical branch onto the shared record/vector store.
///
/// Implemented by the root-crate `RecordOpsService`, delegating to its existing
/// `handle_record_batch_for_tenant` / `handle_record_scan_paginated_for_tenant`
/// methods (tenant + collection resolution, WAL lane, lease, metering all inherited).
#[async_trait]
pub trait RecordRoutePort: Send + Sync {
    /// Insert/upsert canonical records into `collection_id` for `tenant`
    /// (`None` ⇒ default tenant). Returns the number of records persisted.
    async fn insert_records(
        &self,
        collection_id: &str,
        records: Vec<ProximaRecord>,
        tenant: Option<&str>,
    ) -> Result<usize>;

    /// Scan up to `limit` live records from `collection_id`, tenant-scoped. Returns
    /// full [`ProximaRecord`]s (labels + props intact) so the document facade can be
    /// rebuilt. Dead/TTL-expired records are filtered by the underlying scan.
    async fn scan_records(
        &self,
        collection_id: &str,
        limit: usize,
        tenant: Option<&str>,
    ) -> Result<Vec<ProximaRecord>>;

    /// Delete records by canonical id from `collection_id`, tenant-scoped (writes a
    /// tombstone so the delete is visible cross-surface). Returns the number deleted.
    async fn delete_records(
        &self,
        collection_id: &str,
        record_ids: Vec<String>,
        tenant: Option<&str>,
    ) -> Result<usize>;
}
