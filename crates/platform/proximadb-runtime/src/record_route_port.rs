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
use proximadb_data_model::ProximaType;
use proximadb_records::ProximaRecord;

/// A PAX-native typed column exposed to a DataFusion scan of a document collection —
/// the seam that lets `documents(collection)` push predicates into the ranged PAX
/// reader (TD-DOC-PUSHDOWN-1). Runtime-safe (no arrow, no filesystem): the DataFusion
/// layer maps `data_type` to Arrow (via `catalog_arrow_type`) and resolves the segment
/// files under [`PaxScanInputs::base_path`] itself.
#[derive(Debug, Clone)]
pub struct PaxColumnDesc {
    /// DataFusion/SQL-facing column name: the prop key for a shredded column, or the
    /// canonical field name for a system column (e.g. `id`).
    pub sql_name: String,
    /// PAX `column_id` the reader keys the prune + decode on.
    pub col_id: i32,
    /// Catalog type, mapped to an Arrow `DataType` at the DataFusion boundary.
    pub data_type: ProximaType,
}

/// Inputs to build a `PaxTableProvider` over a document collection's `.pax` segments:
/// the `DrPathBuilder` base path + the typed columns queryable with predicate pushdown
/// (the system `id` + the collection's shredded `props__<key>` promoted columns). The
/// `props` JSON tail is served separately by the document layer, not listed here.
#[derive(Debug, Clone)]
pub struct PaxScanInputs {
    /// Object-store base prefix under which the collection's `.pax` segments live.
    pub base_path: String,
    /// The collection's shredded `props__<key>` promoted columns, exposed for typed
    /// predicate pushdown. The system `id` (col 0) and the `props` JSON tail (col 8)
    /// are always present and are added by the document layer, not listed here.
    pub columns: Vec<PaxColumnDesc>,
}

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
    ///
    /// `promote_keys` are declared hot prop keys (from the document collection's indexes) to seed
    /// as props-auto-promotion columns, so those fields shred into typed user-columns at flush
    /// (P-Shred follow-up, ADR-055). Empty ⇒ no seeded shredding.
    async fn ensure_collection(
        &self,
        collection_id: &str,
        dimension: u32,
        tenant: Option<&str>,
        promote_keys: &[String],
    ) -> Result<()>;

    /// Resolve the inputs to scan `collection_id`'s canonical `.pax` segments through the
    /// PAX-native ranged reader with predicate pushdown (TD-DOC-PUSHDOWN-1): the segment
    /// base path + the typed columns (system `id` + the collection's shredded promoted
    /// `props__<key>` columns). `None` ⇒ the collection isn't PAX-pushdown-eligible (no
    /// resolvable catalog schema / not converged), and the caller falls back to the
    /// in-memory document scan. Default `None` for impls that don't serve documents.
    async fn pax_scan_inputs(
        &self,
        _collection_id: &str,
        _tenant: Option<&str>,
    ) -> Option<PaxScanInputs> {
        None
    }

    /// Raw unflushed (WAL/memtable) records for `collection_id`, tenant-scoped but
    /// **NOT** dead-filtered — tombstones and TTL-expired rows are RETAINED. This is the
    /// unflushed delta of the storage-inclusive document PAX scan (TD-DOC-PUSHDOWN-1): the
    /// caller merges these with the flushed PAX segments by `oid` (freshest wins, WAL
    /// priority on ties) and then applies the canonical `is_record_dead` pass on the merged
    /// set. Retaining tombstones is what lets an unflushed delete suppress a still-flushed
    /// live copy (invariant #16d) — a dead-filtered scan cannot express that cross-source
    /// suppression. Default empty for impls that don't serve documents.
    async fn unflushed_records(
        &self,
        _collection_id: &str,
        _tenant: Option<&str>,
    ) -> Result<Vec<ProximaRecord>> {
        Ok(Vec::new())
    }
}
