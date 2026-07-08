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

    /// Point-get a single full [`ProximaRecord`] by id, tenant-scoped (labels + props intact) —
    /// an O(log n) bloom + B+ tree lookup, so a document point read need not scan the collection.
    /// `None` when the record (or collection) is absent, dead, or cross-tenant.
    async fn get_record(
        &self,
        collection_id: &str,
        record_id: &str,
        tenant: Option<&str>,
    ) -> Result<Option<ProximaRecord>>;

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

    /// True when `collection_id` resolves to a canonical (vector) collection for `tenant`
    /// in the record/vector catalog. The document facade uses this to route mixed-safely
    /// under the default-ON gate (TD-DOC-CONV-2): a collection that is NOT a canonical
    /// vector collection (e.g. a pure-document collection never created via REST v2/DDL)
    /// stays on the legacy path instead of hard-failing the canonical write. Best-effort:
    /// returns `false` on any resolution error (fail toward the safe legacy path).
    async fn collection_exists(&self, collection_id: &str, tenant: Option<&str>) -> bool;

    /// Idempotently ensure a canonical (record/vector) collection exists for `collection_id`
    /// (create-if-not-exists; an already-existing collection is treated as success). `dimension`
    /// is the embedding width — `0` ⇒ vectorless (a pure-document collection: documents are still
    /// stored, point-gettable and scannable, but excluded from ANN). The document facade calls
    /// this at collection-create so a document collection CONVERGES on the canonical store
    /// (P-Provision, ADR-055) instead of the legacy `document_wal`/DashMap path. Callers treat it
    /// best-effort (a failure leaves the legacy path intact — mixed-safe).
    async fn ensure_collection(
        &self,
        collection_id: &str,
        dimension: u32,
        tenant: Option<&str>,
    ) -> Result<()>;
}
