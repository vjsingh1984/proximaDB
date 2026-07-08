//! Canonical rich record request types for the v2/internal record API.
//!
//! TD-104 Phase 0: relocated from the root crate
//! (`src/services/operations/vectors/legacy.rs`, now a re-export shim) so the
//! runtime `RecordOpsPort` contract surface is self-contained in this crate —
//! the root no longer needs to be in scope for the record-ops port types.
//! Pure data; no behavior change (mirrors the S3a `BatchOperationResult`
//! relocation pattern).

use proximadb_records::ProximaRecord;

/// Canonical rich record batch request for internal callers.
#[derive(Debug, Clone)]
pub struct RichRecordBatchRequest {
    pub collection_id: String,
    pub records: Vec<ProximaRecord>,
}

/// Canonical rich record delete request for v2 and internal callers.
#[derive(Debug, Clone)]
pub struct RichRecordDeleteBatchRequest {
    pub collection_id: String,
    pub record_ids: Vec<String>,
}

/// Canonical rich record get request for v2 and internal callers.
#[derive(Debug, Clone)]
pub struct RichRecordGetRequest {
    pub collection_id: String,
    pub record_id: String,
    pub include_vector: bool,
    pub include_props: bool,
}
