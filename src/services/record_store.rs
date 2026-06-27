//! Canonical table/record write boundary for SQL, document, graph, vector, and
//! observability facades.
//!
//! This module is the service-layer ownership boundary for cataloged
//! `ProximaRecord` mutations. Protocol and modality facades should lower into
//! this API rather than depending on vector-specific operation names. The first
//! implementation delegates to `VectorOps` as a compatibility adapter until the
//! WAL/storage spine exposes a direct table-record writer.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};
use arrow_array::RecordBatch;
use arrow_schema::Schema as ArrowSchema;
use async_trait::async_trait;
use futures::StreamExt;
use proximadb_block_format::{BlockCompression, BlockMode};
use proximadb_catalog::{
    CatalogPhysicalFormat, CatalogStorageLayout, CatalogStorageSpecialization, CatalogTableSchema,
    CatalogWorkloadProfile,
};
use proximadb_records::{
    ProximaRecord, ProximaTreeNode, RecordKey, RecordScanOptions, RecordScanPredicate,
    RecordStorage,
};
use proximadb_storage_common::object_store_bridge::{
    BridgeObjectPath as ObjectPath, ObjectStoreBridge,
};
use proximadb_storage_common::{
    CanonicalOpenTableFormat, CanonicalOperation, CanonicalWalEntry, ProjectionDirective,
    pax_block::{PAX_SEGMENT_EXT, PaxSegmentScanner, PaxSegmentWriter, ScanPredicate},
    proxima_arrow,
    ranged_segment::RangedSegmentReader,
};

use crate::metrics::consumption_metrics::{
    object_store_egress_locality, record_kou_bytes, record_object_store_op,
};
use crate::observability::io_trace;
use crate::services::operations::VectorOps;
use crate::services::operations::batch_result::OperationMetrics;
use crate::services::operations::vectors::{RichRecordGetRequest, RichSearchResult};
use crate::storage::tenant::context::TenantContext;

/// Logical mutation kind at the canonical table-record boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableRecordMutationKind {
    /// Insert only; callers should reject duplicates before or during write.
    Insert,
    /// Insert or replace the current visible version for a key.
    Upsert,
    /// Write a new version for an existing key.
    Update,
    /// Write a tombstone or close the current visible version.
    Delete,
    /// Replace the target table snapshot with a new result set.
    OverwriteSnapshot,
    /// Replace cataloged partitions with a new result set.
    ReplacePartitions,
    /// Merge source rows into target rows by key/predicate.
    Merge,
}

/// Canonical table-record mutation.
#[derive(Debug, Clone)]
pub struct TableRecordMutation {
    /// Mutation behavior requested by the facade/planner.
    pub kind: TableRecordMutationKind,
    /// Catalog-validated canonical record envelope.
    pub record: ProximaRecord,
}

impl TableRecordMutation {
    /// Create a mutation from a canonical record.
    pub fn new(kind: TableRecordMutationKind, record: ProximaRecord) -> Self {
        Self { kind, record }
    }
}

/// Point lookup request against a cataloged table/collection.
#[derive(Debug, Clone)]
pub struct TableRecordGetRequest {
    /// xCatalog table or current compatibility collection identifier.
    pub table_id: String,
    /// Logical row/record key.
    pub key: String,
    /// Whether vector embeddings should be included.
    pub include_vector: bool,
    /// Whether scalar/document props should be included.
    pub include_props: bool,
}

/// Current point-lookup result shape used by SQL DML. This remains intentionally
/// facade-neutral even though the compatibility adapter sources it from
/// `RichSearchResult` today.
pub type TableRecordGetResponse = Option<RichSearchResult>;

/// Scan request against a cataloged table/collection.
#[derive(Debug, Clone, Default)]
pub struct TableRecordScanRequest {
    /// xCatalog table or current compatibility collection identifier.
    pub table_id: String,
    /// Maximum number of records to scan. `None` means unbounded.
    pub limit: Option<usize>,
    /// Whether vector embeddings should be included.
    pub include_vector: bool,
    /// Whether scalar/document props should be included.
    pub include_props: bool,
    /// Optional structured predicate for **block/row-group pushdown** on stores
    /// that read PAX segments from object storage (e.g.
    /// [`ObjectStoreVectorRecordStore`]). When set, the store skips segment
    /// blocks the filter provably excludes before fetching their bodies; the
    /// row-exact `predicate` closure of `scan_records_filtered` is still applied
    /// on top. `None` ⇒ whole-segment read, preserving prior caller behavior.
    pub filter: Option<proximadb_filter_expression::FilterExpression>,
}

/// Scan result shape for canonical table-record reads.
pub type TableRecordScanResponse = Vec<ProximaRecord>;

/// Append-only canonical WAL boundary for table-record mutations.
///
/// This trait deliberately accepts canonical operations rather than protocol or
/// vector-specific requests. Production implementations can persist to the
/// shared WAL/log/manifest stack; tests and embedded adapters can provide
/// in-memory implementations without changing DML semantics.
#[async_trait]
pub trait TableWalAppender: Send + Sync {
    /// Append canonical operations and return the committed WAL entries with
    /// assigned sequence numbers.
    async fn append_operations(
        &self,
        operations: Vec<CanonicalOperation>,
        tenant_id: Option<String>,
    ) -> Result<Vec<CanonicalWalEntry>>;

    /// Read every canonical WAL entry this appender has durably recorded, oldest first.
    /// The default returns nothing (appenders without a readable log opt out); the framed
    /// appender overrides it. Used by the CDC change-feed to surface row-level changes.
    async fn read_all_entries(&self) -> Result<Vec<CanonicalWalEntry>> {
        Ok(Vec::new())
    }
}

/// Physical writer route selected from xCatalog table metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableRecordStoreRoute {
    /// Modern analytics target: Parquet files managed by Iceberg over Object Storage.
    ParquetIcebergStorage,
    /// Specialized target: PAX block format for high-performance Vector Search / ANN.
    PaxVectorStorage,
    /// Temporary compatibility route through the old vector operations facade.
    LegacyVectorCompatibility,
}

impl TableRecordStoreRoute {
    /// Select a writer route from cataloged workload and storage metadata.
    pub fn for_schema(schema: &CatalogTableSchema) -> Self {
        match (schema.workload_profile, schema.storage_specialization) {
            (_, CatalogStorageSpecialization::VectorAnn) => {
                // Vector-specialized storage implies we want the high-performance PAX layout
                Self::PaxVectorStorage
            }
            (CatalogWorkloadProfile::Vector, _) => {
                // Legacy fallback for vector workloads that haven't migrated to the new specialized flag
                Self::LegacyVectorCompatibility
            }
            (_, CatalogStorageSpecialization::LsmWriteOptimized) => Self::LegacyVectorCompatibility,
            _ => {
                // Default for relational, HTAP, and OLAP workloads
                Self::ParquetIcebergStorage
            }
        }
    }
}

/// Result of writing canonical table-record mutations.
#[derive(Debug, Clone)]
pub struct TableRecordWriteResult {
    /// Whether the write succeeded.
    pub success: bool,
    /// Logical record IDs written.
    pub record_ids: Vec<String>,
    /// Operation metrics.
    pub metrics: OperationMetrics,
    /// Error messages for failed records.
    pub errors: Vec<String>,
    /// Optional stable error code.
    pub error_code: Option<String>,
}

impl TableRecordWriteResult {
    fn success(record_ids: Vec<String>) -> Self {
        Self {
            success: true,
            record_ids,
            metrics: OperationMetrics::default(),
            errors: Vec::new(),
            error_code: None,
        }
    }

    fn failure(error: impl Into<String>, error_code: impl Into<String>) -> Self {
        Self {
            success: false,
            record_ids: Vec::new(),
            metrics: OperationMetrics::default(),
            errors: vec![error.into()],
            error_code: Some(error_code.into()),
        }
    }

    fn from_batch_result(
        result: crate::services::operations::batch_result::BatchOperationResult,
    ) -> Self {
        Self {
            success: result.success,
            record_ids: result.vector_ids,
            metrics: result.metrics,
            errors: result.errors,
            error_code: result.error_code,
        }
    }
}

fn record_id(record: &ProximaRecord) -> String {
    if record.oid.is_empty() {
        record.local_id.clone().unwrap_or_default()
    } else {
        record.oid.clone()
    }
}

#[allow(dead_code)] // pending wiring
fn primary_layout(schema: &CatalogTableSchema) -> Option<&CatalogStorageLayout> {
    schema
        .storage_layouts
        .iter()
        .rev()
        .find(|layout| layout.name == "primary")
        .or_else(|| schema.storage_layouts.first())
}

#[allow(dead_code)] // pending wiring
fn normalize_object_path_prefix(location: &str) -> String {
    let without_scheme = location
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(location);
    without_scheme.trim_matches('/').to_string()
}

fn sanitize_object_path_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "table".to_string()
    } else {
        sanitized
    }
}

#[allow(clippy::expect_used)] // DrPathBuilder::build is infallible for this internally-constructed namespace
fn object_store_write_base_path(
    schema: &CatalogTableSchema,
    tenant_context: Option<&TenantContext>,
) -> String {
    let tenant_id = tenant_context
        .map(|tc| tc.tenant_id.as_str())
        .unwrap_or("default_tenant");
    let mock_namespace = proximadb_catalog::CatalogNamespace::new(vec!["default".into()])
        .with_tenant(tenant_id)
        .with_namespace_id("ns_default");

    let dr_path = crate::storage::trait_components::path_resolver::DrPathBuilder::build(
        &mock_namespace,
        &schema.name,
    )
    .expect("DrPathBuilder failed to construct valid tenant-isolated path");

    dr_path.root_prefix()
}

fn mutation_kind_label(kind: TableRecordMutationKind) -> &'static str {
    match kind {
        TableRecordMutationKind::Insert => "insert",
        TableRecordMutationKind::Upsert => "upsert",
        TableRecordMutationKind::Update => "update",
        TableRecordMutationKind::Delete => "delete",
        TableRecordMutationKind::OverwriteSnapshot => "overwrite",
        TableRecordMutationKind::ReplacePartitions => "replace-partitions",
        TableRecordMutationKind::Merge => "merge",
    }
}

fn object_store_parquet_mutation_path(
    schema: &CatalogTableSchema,
    kind: TableRecordMutationKind,
    tenant_context: Option<&TenantContext>,
) -> ObjectPath {
    let base = object_store_write_base_path(schema, tenant_context);
    let table = sanitize_object_path_segment(&schema.name);
    let sequence = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    ObjectPath::from(format!(
        "{base}data/{table}-{}-{sequence}.parquet",
        mutation_kind_label(kind)
    ))
}

fn object_store_pax_segment_path(
    schema: &CatalogTableSchema,
    kind: TableRecordMutationKind,
    tenant_context: Option<&TenantContext>,
) -> ObjectPath {
    let base = object_store_write_base_path(schema, tenant_context);
    let table = sanitize_object_path_segment(&schema.name);
    let sequence = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    ObjectPath::from(format!(
        "{base}segments/{table}-{}-{sequence}{PAX_SEGMENT_EXT}",
        mutation_kind_label(kind)
    ))
}

fn temp_pax_segment_path(
    schema: &CatalogTableSchema,
    kind: TableRecordMutationKind,
) -> std::path::PathBuf {
    let table = sanitize_object_path_segment(&schema.name);
    let sequence = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir()
        .join("proximadb-object-store-vector")
        .join(format!(
            "{table}-{}-{sequence}{PAX_SEGMENT_EXT}",
            mutation_kind_label(kind)
        ))
}

fn embedding_count_for_records(records: &[ProximaRecord]) -> usize {
    records
        .iter()
        .map(|record| record.embeddings.len())
        .max()
        .unwrap_or(0)
}

/// Per-`RecordBatch` row cap for object-store read-back. The bridge materializes
/// whole objects in memory, so this only bounds the Arrow batch chunking.
const OBJECT_STORE_READ_BATCH_SIZE: usize = 4096;

/// Recover the logical record key (`oid`) from a read-back record's `props`.
///
/// The schema-less `write_records_to_parquet` path does not persist `oid`
/// separately — it is the cataloged primary-key column value(s) in `props`. This
/// projects the record's leaf props into a [`CatalogRow`] and delegates to the
/// canonical [`CatalogRow::primary_key_string`] so the recovered key byte-matches
/// the one `CatalogRow::to_proxima_record` wrote (same `stable_value_string`
/// encoding + unit separator). Returns an empty string when the PK is
/// absent/non-scalar (the catalog builder errors → no fabricated key).
fn reconstruct_oid(record: &ProximaRecord, schema: &CatalogTableSchema) -> String {
    let mut values = std::collections::HashMap::new();
    for (key, node) in &record.props {
        if let ProximaTreeNode::Value(value) = node {
            values.insert(key.clone(), value.clone());
        }
    }
    proximadb_catalog::relational::CatalogRow {
        table: schema.name.clone(),
        values,
    }
    .primary_key_string(schema)
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Turn one Arrow [`RecordBatch`] back into canonical [`ProximaRecord`]s via the
/// single canonical inverse converter
/// ([`proxima_arrow::record_batch_to_proxima_records`], the inverse of
/// `proxima_records_to_record_batch`), then stamp the catalog-derived identity:
/// `oid` recovered from the primary key and `variation_id` from the table name.
fn record_batch_to_records(batch: &RecordBatch, schema: &CatalogTableSchema) -> Vec<ProximaRecord> {
    let mut records = proxima_arrow::record_batch_to_proxima_records(batch);
    for record in &mut records {
        record.oid = reconstruct_oid(record, schema);
        record.variation_id = Some(schema.name.clone());
    }
    records
}

/// List object keys under `prefix` via the canonical base-aware bridge seam
/// ([`ObjectStoreBridge::list_objects`]), keeping only those that end with
/// `suffix` (e.g. `.parquet` / `.pax`). The bridge returns paths that are
/// directly consumable by its read methods, so this is correct for both
/// empty-base and base-prefixed (cloud) deployments.
async fn list_objects_with_suffix(
    bridge: &Arc<dyn ObjectStoreBridge>,
    prefix: &ObjectPath,
    suffix: &str,
) -> Result<Vec<ObjectPath>> {
    let mut paths: Vec<ObjectPath> = bridge
        .list_objects(prefix)
        .await
        .map_err(|err| anyhow!("object-store list under '{prefix}' failed: {err}"))?
        .into_iter()
        .filter(|path| path.as_ref().ends_with(suffix))
        .collect();
    paths.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
    Ok(paths)
}

/// Canonical table-record store API.
/// One row-level change surfaced by the CDC change-feed (P2). `lsn` is the WAL sequence
/// number (monotonic), `op` is `"upsert"` or `"delete"`, `key` is the canonical OID, and
/// `props` carries the after-image (JSON) for upserts. Serializable for the REST surface.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChangeRow {
    pub lsn: u64,
    pub op: String,
    pub collection: String,
    pub key: String,
    pub props: Option<serde_json::Value>,
}

/// Build a [`ChangeRow`] for `collection_id` from a canonical WAL entry, or `None`
/// if the entry is not an upsert/delete for that collection. Shared by the
/// tenant-agnostic and tenant-scoped change-feeds so they stay in lock-step.
fn change_row_from_entry(
    entry: &proximadb_storage_common::wal_entry::CanonicalWalEntry,
    collection_id: &str,
) -> Option<ChangeRow> {
    match &entry.operation {
        CanonicalOperation::RecordUpsert {
            collection_id: c,
            record,
            ..
        } if c == collection_id => Some(ChangeRow {
            lsn: entry.sequence_number,
            op: "upsert".to_string(),
            collection: c.clone(),
            key: record.oid.clone(),
            props: serde_json::to_value(&record.props).ok(),
        }),
        CanonicalOperation::RecordDelete {
            collection_id: c,
            oid,
            ..
        } if c == collection_id => Some(ChangeRow {
            lsn: entry.sequence_number,
            op: "delete".to_string(),
            collection: c.clone(),
            key: oid.clone(),
            props: None,
        }),
        _ => None,
    }
}

#[async_trait]
pub trait TableRecordStore: Send + Sync {
    /// Write catalog-validated record mutations.
    async fn write_mutations(
        &self,
        table_schema: &CatalogTableSchema,
        mutations: Vec<TableRecordMutation>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordWriteResult>;

    /// Get the current visible record for a key.
    async fn get_by_key(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordGetRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordGetResponse>;

    /// Scan current visible records for a cataloged table.
    ///
    /// Implementations that only support point operations may return an error.
    /// Query and DML source readers use this method only after route selection
    /// chooses a native/catalog-table read path.
    async fn scan_records(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let _ = (request, tenant_context);
        Err(anyhow!(
            "TableRecordStore for '{}' does not support catalog-table scans yet",
            table_schema.name
        ))
    }

    /// Scan with a row `predicate` pushed into the store (relational `WHERE`
    /// push-down). Returns up to `request.limit` records matching `predicate`;
    /// the store applies it, so callers must NOT re-filter afterward. This lets
    /// the backing store filter during iteration and stop at the limit instead
    /// of materializing the whole table.
    ///
    /// Default delegates to `scan_records` (materialize) then applies `predicate`
    /// in-memory — correct for any impl; hot stores override for early-stop.
    async fn scan_records_filtered(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        predicate: Option<&RecordScanPredicate<'_>>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let limit = request.limit.unwrap_or(usize::MAX);
        let mut req = request;
        req.limit = None;
        let mut all = self.scan_records(table_schema, req, tenant_context).await?;
        let mut kept = 0usize;
        all.retain(|record| {
            if kept >= limit {
                return false;
            }
            let keep = predicate.is_none_or(|p| p(record));
            if keep {
                kept += 1;
            }
            keep
        });
        Ok(all)
    }

    /// TD-110 Slice C: detect a UNIQUE/PK conflict for a batch of candidate
    /// tuples against committed rows. `sets` carries one entry per unique column
    /// set with the (NULL-exempt, within-batch-deduped) candidate tuples the
    /// caller intends to insert; `primary_key` lets the PK column be read from
    /// `oid` rather than `props`.
    ///
    /// Default impl is a single short-circuiting `scan_records_filtered` per set
    /// (O(N)); index-backed stores (e.g. `DirectWalTableRecordStore`) override
    /// this with an O(1) probe.
    async fn check_unique_conflict(
        &self,
        table_schema: &CatalogTableSchema,
        table_id: &str,
        primary_key: Option<&str>,
        sets: &[UniqueCandidateSet],
        exclude_oids: &std::collections::HashSet<String>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<Option<UniqueConflict>> {
        for set in sets {
            if set.candidates.is_empty() {
                continue;
            }
            let cols = set.columns.clone();
            let pk = primary_key.map(str::to_string);
            let wanted = set.candidates.clone();
            let excluded = exclude_oids.clone();
            let pred = move |existing: &ProximaRecord| {
                !excluded.contains(&existing.oid)
                    && record_unique_tuple(existing, &cols, pk.as_deref())
                        .is_some_and(|tuple| wanted.contains(&tuple))
            };
            let predicate: Option<&RecordScanPredicate<'_>> = Some(&pred);
            let hits = self
                .scan_records_filtered(
                    table_schema,
                    TableRecordScanRequest {
                        filter: None,
                        table_id: table_id.to_string(),
                        limit: Some(1),
                        include_vector: false,
                        include_props: true,
                    },
                    predicate,
                    tenant_context,
                )
                .await?;
            if let Some(existing) = hits.first() {
                let tuple =
                    record_unique_tuple(existing, &set.columns, primary_key).unwrap_or_default();
                return Ok(Some(UniqueConflict {
                    columns: set.columns.clone(),
                    tuple,
                }));
            }
        }
        Ok(None)
    }

    /// TD-127: probe a single-column OLTP secondary index for the oids whose
    /// `column` value text is in `values`. Returns `None` when the store has no
    /// secondary index for `column` (or it is disabled) so the caller falls back
    /// to a scan; `Some(oids)` (possibly empty) when the index answered. The
    /// caller MUST still re-check each candidate against the full predicate — the
    /// index only narrows the candidate set.
    ///
    /// Default opts out (`None`); index-backed stores (e.g.
    /// `DirectWalTableRecordStore`) override this.
    async fn lookup_secondary(
        &self,
        table_schema: &CatalogTableSchema,
        column: &str,
        values: &std::collections::HashSet<String>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<Option<Vec<String>>> {
        let _ = (table_schema, column, values, tenant_context);
        Ok(None)
    }

    /// CDC change-feed: return row-level changes for `collection_id` with WAL sequence
    /// number strictly greater than `since_lsn`, oldest first. The default returns nothing
    /// (stores without a readable change log opt out); the WAL-backed store overrides it.
    async fn read_changes_since(
        &self,
        collection_id: &str,
        since_lsn: u64,
    ) -> Result<Vec<ChangeRow>> {
        let _ = (collection_id, since_lsn);
        Ok(Vec::new())
    }

    /// Tenant-scoped CDC change-feed: like [`read_changes_since`] but returns only
    /// changes belonging to `tenant` (matched against the WAL entry's `tenant_id`,
    /// with `None`/empty treated as the unscoped/default tenant). Required for the
    /// OLAP read-merge because the WAL `collection_id` is the bare table name (not
    /// tenant-unique), so two tenants sharing a table name would otherwise share
    /// the feed. The default delegates to the tenant-agnostic [`read_changes_since`];
    /// the WAL-backed store overrides it to enforce isolation.
    async fn read_changes_since_scoped(
        &self,
        collection_id: &str,
        tenant: Option<&str>,
        since_lsn: u64,
    ) -> Result<Vec<ChangeRow>> {
        let _ = tenant;
        self.read_changes_since(collection_id, since_lsn).await
    }
}

/// One unique column set plus the candidate tuples a write intends to insert
/// (already NULL-exempt + deduped within the statement). See
/// [`TableRecordStore::check_unique_conflict`].
#[derive(Debug, Clone)]
pub struct UniqueCandidateSet {
    /// The unique constraint/index columns, in catalog order.
    pub columns: Vec<String>,
    /// Candidate tuple-reprs (from [`record_unique_tuple`]) to check.
    pub candidates: std::collections::HashSet<Vec<String>>,
}

/// A detected uniqueness violation: which column set, and the existing tuple.
#[derive(Debug, Clone)]
pub struct UniqueConflict {
    /// The violated unique constraint/index columns.
    pub columns: Vec<String>,
    /// The committed tuple that the candidate collided with.
    pub tuple: Vec<String>,
}

/// Canonical comparable-text rendering of a scalar `ProximaValue` for UNIQUE/PK
/// tuple comparison and predicate evaluation. Shared by the record store and
/// `DmlService` so the value seen at write time (index maintenance) matches the
/// value seen at check time.
pub(crate) fn proxima_value_to_unique_text(value: &proximadb_data_model::ProximaValue) -> String {
    use proximadb_data_model::ProximaValue;
    match value {
        ProximaValue::Boolean(value) => {
            if *value {
                "t".to_string()
            } else {
                "f".to_string()
            }
        }
        ProximaValue::Int8(value) => value.to_string(),
        ProximaValue::Int16(value) => value.to_string(),
        ProximaValue::Int32(value) => value.to_string(),
        ProximaValue::Int64(value) => value.to_string(),
        ProximaValue::UInt8(value) => value.to_string(),
        ProximaValue::UInt16(value) => value.to_string(),
        ProximaValue::UInt32(value) => value.to_string(),
        ProximaValue::UInt64(value) => value.to_string(),
        ProximaValue::Float16(value) => value.to_string(),
        ProximaValue::Float32(value) => value.to_string(),
        ProximaValue::Float64(value) => value.to_string(),
        ProximaValue::Decimal(value) => value.clone(),
        ProximaValue::String(value) | ProximaValue::Symbol(value) => value.clone(),
        ProximaValue::DenseVector(values) => values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        ProximaValue::Null => String::new(),
        other => format!("{other:?}"),
    }
}

/// Render a record's value tuple for `columns` as comparable text for UNIQUE/PK
/// checks. Returns `None` when ANY column is NULL or absent — SQL UNIQUE permits
/// multiple NULL tuples, so such rows are exempt. The primary key is read from
/// `oid` (it is not stored in `props`); other columns must be scalar `props`.
pub(crate) fn record_unique_tuple(
    record: &ProximaRecord,
    columns: &[String],
    primary_key: Option<&str>,
) -> Option<Vec<String>> {
    use proximadb_data_model::ProximaValue;
    let mut tuple = Vec::with_capacity(columns.len());
    for column in columns {
        if primary_key.is_some_and(|pk| column.eq_ignore_ascii_case(pk)) {
            tuple.push(record.oid.clone());
            continue;
        }
        match record.props.get(column) {
            Some(ProximaTreeNode::Value(ProximaValue::Null)) | None => return None,
            Some(ProximaTreeNode::Value(value)) => {
                tuple.push(proxima_value_to_unique_text(value));
            }
            // Non-scalar (nested tree / array) — not a scalar unique key.
            Some(_) => return None,
        }
    }
    Some(tuple)
}

/// The column sets carrying a UNIQUE guarantee for a table — cataloged unique
/// indexes plus inline `UNIQUE (...)` constraints. Shared by `DmlService`
/// (candidate construction) and `DirectWalTableRecordStore` (index maintenance)
/// so both agree on exactly which sets are enforced. (TD-110 Slice C.)
pub(crate) fn schema_unique_column_sets(table_schema: &CatalogTableSchema) -> Vec<Vec<String>> {
    let mut sets: Vec<Vec<String>> = Vec::new();
    for index in &table_schema.relational_capabilities.unique_indexes {
        if !index.columns.is_empty() {
            sets.push(index.columns.clone());
        }
    }
    for constraint in &table_schema.relational_capabilities.constraints {
        if let proximadb_catalog::ColumnConstraint::Unique { columns } = constraint
            && !columns.is_empty()
        {
            sets.push(columns.clone());
        }
    }
    sets
}

/// The single-column primary key for a table (explicit, else conventional
/// `id`/`record_id`). The PK value lives in `oid`, not `props` — see
/// [`record_unique_tuple`].
pub(crate) fn schema_primary_key_column(table_schema: &CatalogTableSchema) -> Option<String> {
    table_schema.primary_key.first().cloned().or_else(|| {
        table_schema
            .columns
            .iter()
            .find(|column| column.name == "id" || column.name == "record_id")
            .map(|column| column.name.clone())
    })
}

/// Index-eligible scalar column types for an OLTP secondary (hash-equality)
/// index. Floats/decimals are excluded — equality on a float is semantically
/// fragile and `f32`/`f64` shortest-round-trip text can diverge for the same
/// decimal, which would make the probe text miss the indexed text — as are
/// non-scalar, temporal, and binary types; those columns fall back to the scan
/// path. (TD-127.)
fn is_secondary_indexable_type(data_type: &proximadb_data_model::ProximaType) -> bool {
    use proximadb_data_model::ProximaType;
    matches!(
        data_type,
        ProximaType::Boolean
            | ProximaType::Int8
            | ProximaType::Int16
            | ProximaType::Int32
            | ProximaType::Int64
            | ProximaType::UInt8
            | ProximaType::UInt16
            | ProximaType::UInt32
            | ProximaType::UInt64
            | ProximaType::String
            | ProximaType::Symbol
    )
}

/// The single-column non-unique secondary indexes declared on a table,
/// restricted to index-eligible scalar columns. The motivating consumer is the
/// code-graph workload (look up symbols by `name`/`file`, neither the PK). Unit
/// #1 indexes single columns only (composite is a follow-on). Shared by
/// `DmlService` (probe extraction) and `DirectWalTableRecordStore` (index
/// build/maintenance) so both agree on exactly which columns are indexed.
/// (TD-127.)
pub(crate) fn schema_secondary_index_columns(table_schema: &CatalogTableSchema) -> Vec<String> {
    let eligible: std::collections::HashMap<&str, &proximadb_data_model::ProximaType> =
        table_schema
            .columns
            .iter()
            .map(|column| (column.name.as_str(), &column.data_type))
            .collect();
    let mut columns: Vec<String> = Vec::new();
    for index in &table_schema.relational_capabilities.secondary_indexes {
        let [column] = index.columns.as_slice() else {
            continue; // single-column hash indexes only in unit #1
        };
        if columns.iter().any(|existing| existing == column) {
            continue;
        }
        if eligible
            .get(column.as_str())
            .is_some_and(|data_type| is_secondary_indexable_type(data_type))
        {
            columns.push(column.clone());
        }
    }
    columns
}

/// Render a record's scalar value for `column` as comparable text for the
/// secondary index — `None` when NULL/absent/non-scalar. Reuses
/// [`record_unique_tuple`] (with no primary key, so the value is read from
/// `props`) so the indexed text and the query-side probe text derive
/// identically through [`proxima_value_to_unique_text`]. (TD-127.)
fn record_secondary_text(record: &ProximaRecord, column: &str) -> Option<String> {
    let columns = [column.to_string()];
    record_unique_tuple(record, &columns, None).map(|mut tuple| tuple.remove(0))
}

/// TD-127 kill-switch: `PROXIMADB_SECONDARY_INDEX_DISABLE` forces the OLTP
/// secondary-index build/maintain/probe off (scan fallback), mirroring the
/// scan-index escape hatch (`PROXIMADB_SCAN_INDEX_DISABLE`).
fn secondary_index_disabled() -> bool {
    std::env::var_os("PROXIMADB_SECONDARY_INDEX_DISABLE").is_some()
}

/// Build the per-set candidate tuples for `records`, rejecting a tuple that
/// repeats within this statement (NULL tuples exempt). Shared by the INSERT /
/// UPDATE enforcement in `DmlService` and the INSERT-SELECT native executor so
/// every write path applies UNIQUE the same way. (TD-110.)
pub(crate) fn build_unique_candidate_sets(
    table_schema: &CatalogTableSchema,
    records: &[ProximaRecord],
    primary_key: Option<&str>,
) -> Result<Vec<UniqueCandidateSet>> {
    let mut candidate_sets = Vec::new();
    for columns in schema_unique_column_sets(table_schema) {
        let mut candidates: std::collections::HashSet<Vec<String>> =
            std::collections::HashSet::new();
        for record in records {
            let Some(tuple) = record_unique_tuple(record, &columns, primary_key) else {
                continue; // NULL/absent in the tuple → exempt
            };
            if !candidates.insert(tuple.clone()) {
                return Err(anyhow!(
                    "duplicate key value violates unique constraint on ({}) for table '{}': ({}) appears more than once in this statement",
                    columns.join(", "),
                    table_schema.name,
                    tuple.join(", ")
                ));
            }
        }
        if !candidates.is_empty() {
            candidate_sets.push(UniqueCandidateSet {
                columns,
                candidates,
            });
        }
    }
    Ok(candidate_sets)
}

/// xCatalog-routed table-record store.
///
/// The router makes the migration rule explicit: DML chooses a writer from
/// table/catalog definitions. Until the direct canonical writer exists, both
/// routes can delegate to the compatibility adapter, but callers no longer
/// depend on vector-specific APIs or naming.
pub struct CatalogRoutingTableRecordStore {
    iceberg_store: Arc<dyn TableRecordStore>,
    vector_store: Arc<dyn TableRecordStore>,
    legacy_vector_store: Arc<dyn TableRecordStore>,
}

impl CatalogRoutingTableRecordStore {
    /// Build a router with explicit object store and legacy implementations.
    pub fn new(
        iceberg_store: Arc<dyn TableRecordStore>,
        vector_store: Arc<dyn TableRecordStore>,
        legacy_vector_store: Arc<dyn TableRecordStore>,
    ) -> Self {
        Self {
            iceberg_store,
            vector_store,
            legacy_vector_store,
        }
    }

    /// Build the current migration router. The old vector adapter backs all
    /// routes until the direct object store writers are fully wired.
    pub fn with_vector_compatibility(vector_ops: Arc<VectorOps>) -> Self {
        let compatibility = Arc::new(VectorOpsTableRecordStore::new(vector_ops));
        Self::new(compatibility.clone(), compatibility.clone(), compatibility)
    }

    fn store_for_schema(&self, schema: &CatalogTableSchema) -> &Arc<dyn TableRecordStore> {
        match TableRecordStoreRoute::for_schema(schema) {
            TableRecordStoreRoute::ParquetIcebergStorage => &self.iceberg_store,
            TableRecordStoreRoute::PaxVectorStorage => &self.vector_store,
            TableRecordStoreRoute::LegacyVectorCompatibility => &self.legacy_vector_store,
        }
    }
}

#[async_trait]
impl TableRecordStore for CatalogRoutingTableRecordStore {
    async fn write_mutations(
        &self,
        table_schema: &CatalogTableSchema,
        mutations: Vec<TableRecordMutation>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordWriteResult> {
        self.store_for_schema(table_schema)
            .write_mutations(table_schema, mutations, tenant_context)
            .await
    }

    async fn get_by_key(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordGetRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordGetResponse> {
        self.store_for_schema(table_schema)
            .get_by_key(table_schema, request, tenant_context)
            .await
    }

    async fn scan_records(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        self.store_for_schema(table_schema)
            .scan_records(table_schema, request, tenant_context)
            .await
    }

    async fn scan_records_filtered(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        predicate: Option<&RecordScanPredicate<'_>>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        self.store_for_schema(table_schema)
            .scan_records_filtered(table_schema, request, predicate, tenant_context)
            .await
    }

    /// TD-127: forward the secondary-index probe to the schema's routed store, so
    /// an index-backed route (the WAL-backed native store) is actually consulted
    /// instead of falling through to the `None` trait default (which would force
    /// every reader behind the router onto the scan path).
    async fn lookup_secondary(
        &self,
        table_schema: &CatalogTableSchema,
        column: &str,
        values: &std::collections::HashSet<String>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<Option<Vec<String>>> {
        self.store_for_schema(table_schema)
            .lookup_secondary(table_schema, column, values, tenant_context)
            .await
    }

    /// CDC change-feed: relational changes live in the relational (iceberg) route — the
    /// WAL-backed store — so delegate there. Without this override the routing store would
    /// fall through to the empty trait default, hiding pgwire writes from the change-feed.
    async fn read_changes_since(
        &self,
        collection_id: &str,
        since_lsn: u64,
    ) -> Result<Vec<ChangeRow>> {
        self.iceberg_store
            .read_changes_since(collection_id, since_lsn)
            .await
    }
}

/// Compatibility implementation backed by `VectorOps`.
///
/// This adapter is intentionally narrow: it lets pgwire and other facades
/// depend on `TableRecordStore` immediately while the underlying WAL/storage
/// writer is extracted from the vector service.
pub struct VectorOpsTableRecordStore {
    vector_ops: Arc<VectorOps>,
}

impl VectorOpsTableRecordStore {
    /// Wrap the current vector operations service as a table-record store.
    pub fn new(vector_ops: Arc<VectorOps>) -> Self {
        Self { vector_ops }
    }
}

#[async_trait]
impl TableRecordStore for VectorOpsTableRecordStore {
    async fn write_mutations(
        &self,
        table_schema: &CatalogTableSchema,
        mutations: Vec<TableRecordMutation>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordWriteResult> {
        let records = mutations
            .into_iter()
            .map(|mutation| mutation.record)
            .collect::<Vec<_>>();
        let result = self
            .vector_ops
            .insert_records_with_tenant_context(&table_schema.name, records, tenant_context)
            .await?;
        Ok(TableRecordWriteResult::from_batch_result(result))
    }

    async fn get_by_key(
        &self,
        _table_schema: &CatalogTableSchema,
        request: TableRecordGetRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordGetResponse> {
        self.vector_ops
            .get_record_with_tenant_context(
                RichRecordGetRequest {
                    collection_id: request.table_id,
                    record_id: request.key,
                    include_vector: request.include_vector,
                    include_props: request.include_props,
                },
                tenant_context,
            )
            .await
    }

    async fn scan_records(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        self.vector_ops
            .scan_records_with_tenant_context(
                &table_schema.name,
                request.limit,
                request.include_vector,
                request.include_props,
                tenant_context,
            )
            .await
    }
}

/// Direct canonical table-record implementation over the shared
/// `proximadb_records::RecordStorage` contract.
///
/// This is the service-layer adapter for the target relational/document/graph
/// durable spine. It intentionally has no VectorOps dependency. Concrete WAL,
/// PAX, row-family, and projection stores can implement `RecordStorage` and be
/// exposed to SQL, REST/gRPC, Arrow Flight, and embedded facades through this
/// adapter.
pub struct RecordStorageTableRecordStore {
    storage: Arc<dyn RecordStorage>,
}

impl RecordStorageTableRecordStore {
    /// Wrap a canonical record storage implementation.
    pub fn new(storage: Arc<dyn RecordStorage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl TableRecordStore for RecordStorageTableRecordStore {
    async fn write_mutations(
        &self,
        table_schema: &CatalogTableSchema,
        mutations: Vec<TableRecordMutation>,
        _tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordWriteResult> {
        let mut record_ids = Vec::with_capacity(mutations.len());

        for mutation in mutations {
            let kind = mutation.kind;
            let record = mutation.record;
            let key = RecordKey::from(&record);

            match kind {
                TableRecordMutationKind::Insert => {
                    if self.storage.get_record(&key).await?.is_some() {
                        return Ok(TableRecordWriteResult::failure(
                            format!(
                                "Record '{}' already exists in table '{}'",
                                record.oid, table_schema.name
                            ),
                            "INSERT_CONFLICT",
                        ));
                    }
                    let written = self.storage.upsert_record(record).await?;
                    record_ids.push(written.oid);
                }
                TableRecordMutationKind::Upsert | TableRecordMutationKind::Update => {
                    let written = self.storage.upsert_record(record).await?;
                    record_ids.push(written.oid);
                }
                TableRecordMutationKind::Delete => {
                    self.storage.delete_record(&key).await?;
                    record_ids.push(record.oid);
                }
                TableRecordMutationKind::OverwriteSnapshot
                | TableRecordMutationKind::ReplacePartitions
                | TableRecordMutationKind::Merge => {
                    return Err(anyhow!(
                        "Mutation kind {:?} for table '{}' requires snapshot/merge commit support",
                        kind,
                        table_schema.name
                    ));
                }
            }
        }

        Ok(TableRecordWriteResult::success(record_ids))
    }

    async fn get_by_key(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordGetRequest,
        _tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordGetResponse> {
        let record = self
            .storage
            .get_record(&RecordKey::new(request.key.clone()))
            .await?;
        Ok(record.map(|record| {
            proxima_record_to_get_response(
                record,
                table_schema,
                request.include_vector,
                request.include_props,
            )
        }))
    }

    async fn scan_records(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let mut options = request
            .limit
            .map(RecordScanOptions::limit)
            .unwrap_or_else(RecordScanOptions::unbounded);
        if let Some(tenant_context) = tenant_context {
            options = options.with_tenant_id(tenant_context.tenant_id.clone());
        }

        let mut records = self.storage.scan_records_with_options(options).await?;
        records.retain(|record| {
            record
                .variation_id
                .as_deref()
                .map(|variation| variation == table_schema.name)
                .unwrap_or(true)
        });
        if !request.include_vector {
            for record in &mut records {
                record.embeddings.clear();
            }
        }
        if !request.include_props {
            for record in &mut records {
                record.props.clear();
            }
        }
        Ok(records)
    }

    async fn scan_records_filtered(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        predicate: Option<&RecordScanPredicate<'_>>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let mut options = request
            .limit
            .map(RecordScanOptions::limit)
            .unwrap_or_else(RecordScanOptions::unbounded);
        if let Some(tenant_context) = tenant_context {
            options = options.with_tenant_id(tenant_context.tenant_id.clone());
        }

        // Fold the table-membership (variation_id) check INTO the pushed-down
        // predicate so the store's early-stop counts only fully-matching rows —
        // a post-scan retain would under-return against the limit.
        let table_name = table_schema.name.as_str();
        let combined = |record: &ProximaRecord| {
            let belongs = record
                .variation_id
                .as_deref()
                .map(|variation| variation == table_name)
                .unwrap_or(true);
            belongs && predicate.is_none_or(|p| p(record))
        };
        let mut records = self
            .storage
            .scan_records_filtered(options, Some(&combined))
            .await?;

        if !request.include_vector {
            for record in &mut records {
                record.embeddings.clear();
            }
        }
        if !request.include_props {
            for record in &mut records {
                record.props.clear();
            }
        }
        Ok(records)
    }
}

/// Direct Proxima-authoritative table writer.
///
/// This is the native OLTP/HTAP commit route for mutable Proxima-owned tables:
/// append canonical WAL operations first, then apply the visible record state
/// to the canonical `RecordStorage` row/delta spine. Layer-2 projections such
/// as PAX stripes, columnar blocks, HNSW, JSON, graph topology, and open-format
/// manifests are driven from WAL `ProjectionDirective`s and remain rebuildable.
/// TD-110 Slice C: in-memory UNIQUE/PK index for one unique column set —
/// `tuple-repr → owning oids`. A set may transiently hold >1 oid only across the
/// pre-existing uniqueness TOCTOU (also present in the scan path); steady state
/// is one oid per tuple.
#[derive(Default)]
struct UniqueSetIndex {
    columns: Vec<String>,
    tuple_to_oids: std::collections::HashMap<Vec<String>, std::collections::HashSet<String>>,
}

/// Per-table UNIQUE/PK index. Each oid self-tracks its current per-set tuple
/// (`oid_tuples`) so an update or delete can remove the OLD tuples without
/// re-reading storage. Built lazily on first check (scanning the WAL-rebuilt
/// current state) and maintained incrementally on every subsequent write.
#[derive(Default)]
struct TableUniqueIndex {
    /// One per unique column set, in `schema_unique_column_sets` order.
    sets: Vec<UniqueSetIndex>,
    /// oid → its current per-set tuple (`None` = NULL-exempt for that set).
    oid_tuples: std::collections::HashMap<String, Vec<Option<Vec<String>>>>,
}

impl TableUniqueIndex {
    fn with_sets(set_columns: &[Vec<String>]) -> Self {
        Self {
            sets: set_columns
                .iter()
                .map(|columns| UniqueSetIndex {
                    columns: columns.clone(),
                    tuple_to_oids: std::collections::HashMap::new(),
                })
                .collect(),
            oid_tuples: std::collections::HashMap::new(),
        }
    }

    /// Insert/update `record`: drop its previous per-set tuples (if any) then add
    /// its current ones. Uniform across INSERT/UPSERT/UPDATE.
    fn upsert(&mut self, record: &ProximaRecord, primary_key: Option<&str>) {
        self.remove_oid_tuples(&record.oid);
        let mut per_set = Vec::with_capacity(self.sets.len());
        for set in &mut self.sets {
            let tuple = record_unique_tuple(record, &set.columns, primary_key);
            if let Some(tuple) = &tuple {
                set.tuple_to_oids
                    .entry(tuple.clone())
                    .or_default()
                    .insert(record.oid.clone());
            }
            per_set.push(tuple);
        }
        self.oid_tuples.insert(record.oid.clone(), per_set);
    }

    /// Remove `oid` entirely (DELETE).
    fn delete(&mut self, oid: &str) {
        self.remove_oid_tuples(oid);
        self.oid_tuples.remove(oid);
    }

    /// Detach `oid`'s currently-indexed tuples from `tuple_to_oids` (shared by
    /// upsert's replace and delete). Leaves `oid_tuples[oid]` for the caller.
    fn remove_oid_tuples(&mut self, oid: &str) {
        let Some(previous) = self.oid_tuples.get(oid).cloned() else {
            return;
        };
        for (set, tuple) in self.sets.iter_mut().zip(previous.iter()) {
            if let Some(tuple) = tuple
                && let Some(oids) = set.tuple_to_oids.get_mut(tuple)
            {
                oids.remove(oid);
                if oids.is_empty() {
                    set.tuple_to_oids.remove(tuple);
                }
            }
        }
    }

    /// First candidate tuple in `candidates` that already exists for the set
    /// matching `columns`, owned by some oid NOT in `exclude_oids`, if any.
    /// `exclude_oids` lets an UPDATE ignore the rows it is itself rewriting (so a
    /// row keeping or vacating its own unique value is not a self-conflict).
    fn conflict(
        &self,
        columns: &[String],
        candidates: &std::collections::HashSet<Vec<String>>,
        exclude_oids: &std::collections::HashSet<String>,
    ) -> Option<Vec<String>> {
        let set = self.sets.iter().find(|set| set.columns == columns)?;
        candidates
            .iter()
            .find(|candidate| {
                set.tuple_to_oids
                    .get(*candidate)
                    .is_some_and(|owners| owners.iter().any(|oid| !exclude_oids.contains(oid)))
            })
            .cloned()
    }
}

/// TD-127: one column's hash secondary index — `value-text → owning oids`.
#[derive(Default)]
struct ColumnSecondaryIndex {
    column: String,
    value_to_oids: std::collections::HashMap<String, std::collections::HashSet<String>>,
}

/// Per-table non-unique secondary index over single columns. Each oid
/// self-tracks its current per-column value text (`oid_values`) so an update or
/// delete drops the OLD value without re-reading storage — the same maintenance
/// shape as [`TableUniqueIndex`]. Built lazily on first probe (scanning the
/// WAL-rebuilt current state) and maintained incrementally on every write.
#[derive(Default)]
struct TableSecondaryIndex {
    /// One per indexed column, in `schema_secondary_index_columns` order.
    columns: Vec<ColumnSecondaryIndex>,
    /// oid → its current per-column value text (`None` = NULL/absent for that
    /// column, so it is not indexed).
    oid_values: std::collections::HashMap<String, Vec<Option<String>>>,
}

impl TableSecondaryIndex {
    fn with_columns(columns: &[String]) -> Self {
        Self {
            columns: columns
                .iter()
                .map(|column| ColumnSecondaryIndex {
                    column: column.clone(),
                    value_to_oids: std::collections::HashMap::new(),
                })
                .collect(),
            oid_values: std::collections::HashMap::new(),
        }
    }

    /// Insert/update `record`: drop its previous per-column values (if any) then
    /// add its current ones. Uniform across INSERT/UPSERT/UPDATE.
    fn upsert(&mut self, record: &ProximaRecord) {
        self.remove_oid(&record.oid);
        let mut per_column = Vec::with_capacity(self.columns.len());
        for column in &mut self.columns {
            let text = record_secondary_text(record, &column.column);
            if let Some(text) = &text {
                column
                    .value_to_oids
                    .entry(text.clone())
                    .or_default()
                    .insert(record.oid.clone());
            }
            per_column.push(text);
        }
        self.oid_values.insert(record.oid.clone(), per_column);
    }

    /// Remove `oid` entirely (DELETE).
    fn delete(&mut self, oid: &str) {
        self.remove_oid(oid);
        self.oid_values.remove(oid);
    }

    /// Detach `oid`'s currently-indexed values from `value_to_oids` (shared by
    /// upsert's replace and delete). Leaves `oid_values[oid]` for the caller.
    fn remove_oid(&mut self, oid: &str) {
        let Some(previous) = self.oid_values.get(oid).cloned() else {
            return;
        };
        for (column, value) in self.columns.iter_mut().zip(previous.iter()) {
            if let Some(value) = value
                && let Some(oids) = column.value_to_oids.get_mut(value)
            {
                oids.remove(oid);
                if oids.is_empty() {
                    column.value_to_oids.remove(value);
                }
            }
        }
    }

    /// Union of oids whose `column` value text is in `values`. `None` when the
    /// column is not indexed by this table index (caller scans); `Some` (possibly
    /// empty) when the column is indexed.
    fn probe(
        &self,
        column: &str,
        values: &std::collections::HashSet<String>,
    ) -> Option<std::collections::HashSet<String>> {
        let index = self.columns.iter().find(|c| c.column == column)?;
        let mut oids = std::collections::HashSet::new();
        for value in values {
            if let Some(owners) = index.value_to_oids.get(value) {
                oids.extend(owners.iter().cloned());
            }
        }
        Some(oids)
    }
}

pub struct DirectWalTableRecordStore {
    /// Per-(tenant_id, collection) record partitions, created on demand via
    /// `storage_factory`. TD-064: tenant + collection isolation is STRUCTURAL —
    /// selecting the partition by the catalog-resolved (tenant, collection)
    /// identity replaces per-record tenant/`variation_id` filtering on the hot
    /// path, and scopes oid point-lookups/insert-conflicts per (tenant, table).
    /// The empty tenant id (`""`) is just one more tenant key (single-tenant).
    partitions:
        parking_lot::RwLock<std::collections::HashMap<(String, String), Arc<dyn RecordStorage>>>,
    /// Factory for a fresh per-partition record store (default: in-memory memtable).
    storage_factory: Arc<dyn Fn() -> Arc<dyn RecordStorage> + Send + Sync>,
    wal_appender: Arc<dyn TableWalAppender>,
    /// TD-110 Slice C: UNIQUE/PK index keyed by `(tenant_id, collection)` so a
    /// table's UNIQUE/PK enforcement is per-tenant. Presence of a key == "index
    /// built". Lazily built on first `check_unique_conflict`, then maintained on
    /// every `write_mutations`.
    unique_index:
        parking_lot::RwLock<std::collections::HashMap<(String, String), TableUniqueIndex>>,
    /// TD-127: non-unique OLTP secondary index keyed by `(tenant_id, collection)`
    /// so a table's secondary indexes are per-tenant, mirroring `unique_index`.
    /// Presence of a key == "index built". Lazily built on first
    /// `lookup_secondary`, then maintained on every `write_mutations`.
    secondary_index:
        parking_lot::RwLock<std::collections::HashMap<(String, String), TableSecondaryIndex>>,
}

impl DirectWalTableRecordStore {
    /// Create a direct writer that routes every `(tenant, collection)` partition
    /// to the single supplied `storage`. This is the non-isolated shape used by
    /// single-tenant unit tests and callers that intentionally share one store;
    /// production multi-tenant paths use [`Self::new_partitioned`].
    pub fn new(storage: Arc<dyn RecordStorage>, wal_appender: Arc<dyn TableWalAppender>) -> Self {
        Self::with_storage_factory(wal_appender, Arc::new(move || storage.clone()))
    }

    /// Create a direct writer with per-(tenant, collection) partitions backed by
    /// in-memory memtables created on demand — the isolated production shape.
    pub fn new_partitioned(wal_appender: Arc<dyn TableWalAppender>) -> Self {
        Self::with_storage_factory(
            wal_appender,
            Arc::new(|| {
                Arc::new(crate::services::MemtableRecordStorage::new()) as Arc<dyn RecordStorage>
            }),
        )
    }

    /// Create a direct writer with a custom per-partition storage factory.
    pub fn with_storage_factory(
        wal_appender: Arc<dyn TableWalAppender>,
        storage_factory: Arc<dyn Fn() -> Arc<dyn RecordStorage> + Send + Sync>,
    ) -> Self {
        Self {
            partitions: parking_lot::RwLock::new(std::collections::HashMap::new()),
            storage_factory,
            wal_appender,
            unique_index: parking_lot::RwLock::new(std::collections::HashMap::new()),
            secondary_index: parking_lot::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Resolve the tenant scope key from an optional tenant context.
    fn tenant_key(tenant_context: Option<&TenantContext>) -> String {
        tenant_context
            .map(|tenant| tenant.tenant_id.clone())
            .unwrap_or_default()
    }

    /// Select (creating on demand) the record partition for `(tenant_id, collection)`.
    fn partition(&self, tenant_id: &str, collection: &str) -> Arc<dyn RecordStorage> {
        let key = (tenant_id.to_string(), collection.to_string());
        if let Some(partition) = self.partitions.read().get(&key) {
            return partition.clone();
        }
        self.partitions
            .write()
            .entry(key)
            .or_insert_with(|| (self.storage_factory)())
            .clone()
    }

    /// Replay canonical WAL entries into the correct `(tenant, table)` partitions
    /// on recovery, routing by the entry's `tenant_id` + the operation's
    /// `collection_id`. Reuses the per-store `RecordStore` point ops.
    pub async fn replay_wal_entries<I>(
        &self,
        entries: I,
    ) -> Result<proximadb_records::RecordRecoverySummary>
    where
        I: IntoIterator<Item = CanonicalWalEntry>,
    {
        let mut summary = proximadb_records::RecordRecoverySummary::default();
        for entry in entries {
            let tenant_id = entry.tenant_id.clone().unwrap_or_default();
            match entry.operation {
                CanonicalOperation::RecordUpsert {
                    collection_id,
                    record,
                    ..
                } => {
                    self.partition(&tenant_id, &collection_id)
                        .upsert_record(*record)
                        .await?;
                    summary.upserts_replayed += 1;
                }
                CanonicalOperation::RecordDelete {
                    collection_id, oid, ..
                } => {
                    self.partition(&tenant_id, &collection_id)
                        .delete_record(&RecordKey::new(oid))
                        .await?;
                    summary.deletes_replayed += 1;
                }
                // Checkpoints, CDC barriers, and system-catalog mutations carry
                // no record state for this partitioned store to replay.
                CanonicalOperation::Checkpoint(_)
                | CanonicalOperation::CdcBarrier { .. }
                | CanonicalOperation::CatalogMutation { .. } => {}
            }
        }
        Ok(summary)
    }

    /// Build the UNIQUE/PK index for `(tenant_id, table_schema)` if not already
    /// built, by scanning the tenant's current visible state (WAL recovery has
    /// rebuilt it). A no-op when the table has no UNIQUE/PK sets.
    async fn ensure_unique_index_built(
        &self,
        table_schema: &CatalogTableSchema,
        tenant_id: &str,
    ) -> Result<()> {
        let index_key = (tenant_id.to_string(), table_schema.name.clone());
        if self.unique_index.read().contains_key(&index_key) {
            return Ok(());
        }
        let set_columns = schema_unique_column_sets(table_schema);
        if set_columns.is_empty() {
            return Ok(());
        }
        let primary_key = schema_primary_key_column(table_schema);
        let existing =
            RecordStorageTableRecordStore::new(self.partition(tenant_id, &table_schema.name))
                .scan_records(
                    table_schema,
                    TableRecordScanRequest {
                        filter: None,
                        table_id: table_schema.name.clone(),
                        limit: None,
                        include_vector: false,
                        include_props: true,
                    },
                    None,
                )
                .await?;
        let mut index = TableUniqueIndex::with_sets(&set_columns);
        for record in &existing {
            index.upsert(record, primary_key.as_deref());
        }
        // Double-checked insert: keep an index another writer built meanwhile.
        self.unique_index.write().entry(index_key).or_insert(index);
        Ok(())
    }

    /// TD-127: build the secondary index for `(tenant_id, table_schema)` if not
    /// already built, by scanning the tenant's current visible state (WAL
    /// recovery has rebuilt it). A no-op when the table declares no eligible
    /// secondary-index columns or the kill-switch is set.
    async fn ensure_secondary_index_built(
        &self,
        table_schema: &CatalogTableSchema,
        tenant_id: &str,
    ) -> Result<()> {
        if secondary_index_disabled() {
            return Ok(());
        }
        let index_key = (tenant_id.to_string(), table_schema.name.clone());
        if self.secondary_index.read().contains_key(&index_key) {
            return Ok(());
        }
        let columns = schema_secondary_index_columns(table_schema);
        if columns.is_empty() {
            return Ok(());
        }
        let existing =
            RecordStorageTableRecordStore::new(self.partition(tenant_id, &table_schema.name))
                .scan_records(
                    table_schema,
                    TableRecordScanRequest {
                        filter: None,
                        table_id: table_schema.name.clone(),
                        limit: None,
                        include_vector: false,
                        include_props: true,
                    },
                    None,
                )
                .await?;
        let mut index = TableSecondaryIndex::with_columns(&columns);
        for record in &existing {
            index.upsert(record);
        }
        // Double-checked insert: keep an index another writer built meanwhile.
        self.secondary_index
            .write()
            .entry(index_key)
            .or_insert(index);
        Ok(())
    }
}

#[async_trait]
impl TableRecordStore for DirectWalTableRecordStore {
    /// CDC change-feed over the canonical WAL: surface every RecordUpsert/RecordDelete for
    /// `collection_id` (the table name) with sequence number > `since_lsn`, oldest first.
    /// Tenant-agnostic (all tenants) — see [`read_changes_since_scoped`] for the
    /// tenant-isolated feed the OLAP read-merge uses.
    async fn read_changes_since(
        &self,
        collection_id: &str,
        since_lsn: u64,
    ) -> Result<Vec<ChangeRow>> {
        let entries = self.wal_appender.read_all_entries().await?;
        let mut out: Vec<ChangeRow> = entries
            .iter()
            .filter(|e| e.sequence_number > since_lsn)
            .filter_map(|e| change_row_from_entry(e, collection_id))
            .collect();
        out.sort_by_key(|r| r.lsn);
        Ok(out)
    }

    /// Tenant-isolated change-feed: like [`read_changes_since`] but keeps only
    /// entries whose `tenant_id` matches `tenant` (None/"" = the unscoped tenant).
    /// The WAL `collection_id` is the bare table name (not tenant-unique), so this
    /// match is what isolates two tenants that share a table name.
    async fn read_changes_since_scoped(
        &self,
        collection_id: &str,
        tenant: Option<&str>,
        since_lsn: u64,
    ) -> Result<Vec<ChangeRow>> {
        let entries = self.wal_appender.read_all_entries().await?;
        let want = tenant.filter(|t| !t.is_empty());
        let mut out: Vec<ChangeRow> = entries
            .iter()
            .filter(|e| e.sequence_number > since_lsn)
            .filter(|e| e.tenant_id.as_deref().filter(|t| !t.is_empty()) == want)
            .filter_map(|e| change_row_from_entry(e, collection_id))
            .collect();
        out.sort_by_key(|r| r.lsn);
        Ok(out)
    }

    async fn write_mutations(
        &self,
        table_schema: &CatalogTableSchema,
        mutations: Vec<TableRecordMutation>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordWriteResult> {
        let mut operations = Vec::with_capacity(mutations.len());
        let mut storage_actions = Vec::with_capacity(mutations.len());
        let projections = projection_directives_for_schema(table_schema);
        // TD-064: structural per-(tenant, collection) partition selection.
        let tenant_scope = Self::tenant_key(tenant_context);
        let partition = self.partition(&tenant_scope, &table_schema.name);
        let index_key = (tenant_scope.clone(), table_schema.name.clone());

        for mutation in mutations {
            let kind = mutation.kind;
            let record = mutation.record;
            let key = RecordKey::from(&record);

            match kind {
                TableRecordMutationKind::Insert => {
                    if partition.get_record(&key).await?.is_some() {
                        return Ok(TableRecordWriteResult::failure(
                            format!(
                                "Record '{}' already exists in table '{}'",
                                record.oid, table_schema.name
                            ),
                            "INSERT_CONFLICT",
                        ));
                    }
                    operations.push(CanonicalOperation::RecordUpsert {
                        collection_id: table_schema.name.clone(),
                        record: Box::new(record.clone()),
                        projections: projections.clone(),
                    });
                    storage_actions.push((kind, record));
                }
                TableRecordMutationKind::Update => {
                    if partition.get_record(&key).await?.is_none() {
                        return Ok(TableRecordWriteResult::failure(
                            format!(
                                "Record '{}' does not exist in table '{}'",
                                record.oid, table_schema.name
                            ),
                            "UPDATE_NOT_FOUND",
                        ));
                    }
                    operations.push(CanonicalOperation::RecordUpsert {
                        collection_id: table_schema.name.clone(),
                        record: Box::new(record.clone()),
                        projections: projections.clone(),
                    });
                    storage_actions.push((kind, record));
                }
                TableRecordMutationKind::Upsert => {
                    operations.push(CanonicalOperation::RecordUpsert {
                        collection_id: table_schema.name.clone(),
                        record: Box::new(record.clone()),
                        projections: projections.clone(),
                    });
                    storage_actions.push((kind, record));
                }
                TableRecordMutationKind::Delete => {
                    operations.push(CanonicalOperation::RecordDelete {
                        collection_id: table_schema.name.clone(),
                        oid: record.oid.clone(),
                        projections: projections.clone(),
                    });
                    storage_actions.push((kind, record));
                }
                TableRecordMutationKind::OverwriteSnapshot
                | TableRecordMutationKind::ReplacePartitions
                | TableRecordMutationKind::Merge => {
                    return Err(anyhow!(
                        "Mutation kind {:?} for table '{}' requires snapshot/merge commit support",
                        kind,
                        table_schema.name
                    ));
                }
            }
        }

        let tenant_id = tenant_context.map(|tenant| tenant.tenant_id.clone());
        self.wal_appender
            .append_operations(operations, tenant_id)
            .await?;

        // TD-110 Slice C: maintain the UNIQUE/PK index incrementally — but only
        // once it has been built (first `check_unique_conflict`). Until then the
        // lazy build captures these writes from current state, so skipping is
        // safe; checking once avoids per-write work for tables never probed.
        let index_primary_key = schema_primary_key_column(table_schema);
        let maintain_index = !schema_unique_column_sets(table_schema).is_empty()
            && self.unique_index.read().contains_key(&index_key);
        // TD-127: maintain the secondary index on the same once-built basis as the
        // UNIQUE/PK index — until first `lookup_secondary` builds it, the lazy
        // build captures these writes from current state, so skipping is safe.
        let maintain_secondary = !secondary_index_disabled()
            && !schema_secondary_index_columns(table_schema).is_empty()
            && self.secondary_index.read().contains_key(&index_key);
        let maintain_any = maintain_index || maintain_secondary;

        let mut record_ids = Vec::with_capacity(storage_actions.len());
        for (kind, record) in storage_actions {
            match kind {
                TableRecordMutationKind::Insert
                | TableRecordMutationKind::Upsert
                | TableRecordMutationKind::Update => {
                    if maintain_any {
                        let written = partition.upsert_record(record.clone()).await?;
                        record_ids.push(written.oid);
                        if maintain_index
                            && let Some(index) = self.unique_index.write().get_mut(&index_key)
                        {
                            index.upsert(&record, index_primary_key.as_deref());
                        }
                        if maintain_secondary
                            && let Some(index) = self.secondary_index.write().get_mut(&index_key)
                        {
                            index.upsert(&record);
                        }
                    } else {
                        let written = partition.upsert_record(record).await?;
                        record_ids.push(written.oid);
                    }
                }
                TableRecordMutationKind::Delete => {
                    partition.delete_record(&RecordKey::from(&record)).await?;
                    if maintain_index
                        && let Some(index) = self.unique_index.write().get_mut(&index_key)
                    {
                        index.delete(&record.oid);
                    }
                    if maintain_secondary
                        && let Some(index) = self.secondary_index.write().get_mut(&index_key)
                    {
                        index.delete(&record.oid);
                    }
                    record_ids.push(record.oid);
                }
                TableRecordMutationKind::OverwriteSnapshot
                | TableRecordMutationKind::ReplacePartitions
                | TableRecordMutationKind::Merge => {
                    unreachable!("unsupported kinds rejected above")
                }
            }
        }

        Ok(TableRecordWriteResult::success(record_ids))
    }

    async fn get_by_key(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordGetRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordGetResponse> {
        let partition = self.partition(&Self::tenant_key(tenant_context), &table_schema.name);
        // Pass `None`: the partition already scopes the tenant structurally, so
        // no per-record tenant filter is needed (TD-064).
        RecordStorageTableRecordStore::new(partition)
            .get_by_key(table_schema, request, None)
            .await
    }

    async fn scan_records(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let partition = self.partition(&Self::tenant_key(tenant_context), &table_schema.name);
        RecordStorageTableRecordStore::new(partition)
            .scan_records(table_schema, request, None)
            .await
    }

    async fn scan_records_filtered(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        predicate: Option<&RecordScanPredicate<'_>>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let partition = self.partition(&Self::tenant_key(tenant_context), &table_schema.name);
        RecordStorageTableRecordStore::new(partition)
            .scan_records_filtered(table_schema, request, predicate, None)
            .await
    }

    /// TD-110 Slice C: O(1) index-backed override of the default scan. Builds the
    /// per-table index on first use (from WAL-recovered current state), then
    /// probes candidate tuples directly.
    async fn check_unique_conflict(
        &self,
        table_schema: &CatalogTableSchema,
        _table_id: &str,
        _primary_key: Option<&str>,
        sets: &[UniqueCandidateSet],
        exclude_oids: &std::collections::HashSet<String>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<Option<UniqueConflict>> {
        let tenant_scope = Self::tenant_key(tenant_context);
        self.ensure_unique_index_built(table_schema, &tenant_scope)
            .await?;
        let index_key = (tenant_scope, table_schema.name.clone());
        let index = self.unique_index.read();
        let Some(table_index) = index.get(&index_key) else {
            return Ok(None); // table has no UNIQUE/PK sets
        };
        for set in sets {
            if let Some(tuple) = table_index.conflict(&set.columns, &set.candidates, exclude_oids) {
                return Ok(Some(UniqueConflict {
                    columns: set.columns.clone(),
                    tuple,
                }));
            }
        }
        Ok(None)
    }

    /// TD-127: index-backed secondary lookup. Builds the per-table index on first
    /// use (from current state), then probes for candidate oids. Returns `None`
    /// (scan fallback) when `column` is not an indexed column or the kill-switch
    /// is set.
    async fn lookup_secondary(
        &self,
        table_schema: &CatalogTableSchema,
        column: &str,
        values: &std::collections::HashSet<String>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<Option<Vec<String>>> {
        if secondary_index_disabled() {
            return Ok(None);
        }
        if !schema_secondary_index_columns(table_schema)
            .iter()
            .any(|indexed| indexed == column)
        {
            return Ok(None);
        }
        let tenant_scope = Self::tenant_key(tenant_context);
        self.ensure_secondary_index_built(table_schema, &tenant_scope)
            .await?;
        let index_key = (tenant_scope, table_schema.name.clone());
        let index = self.secondary_index.read();
        let Some(table_index) = index.get(&index_key) else {
            return Ok(None);
        };
        Ok(table_index
            .probe(column, values)
            .map(|oids| oids.into_iter().collect()))
    }
}

fn projection_directives_for_schema(schema: &CatalogTableSchema) -> Vec<ProjectionDirective> {
    match schema.storage_specialization {
        CatalogStorageSpecialization::GenericRelational
        | CatalogStorageSpecialization::PaxRowFamily
        | CatalogStorageSpecialization::PaxOltp
        | CatalogStorageSpecialization::PaxOlap
        | CatalogStorageSpecialization::ColumnarAnalytics => {
            vec![ProjectionDirective::ColumnarVariation {
                collection_id: schema.name.clone(),
                fields: schema
                    .columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect(),
            }]
        }
        CatalogStorageSpecialization::VectorAnn => schema
            .columns
            .iter()
            .filter(|column| {
                matches!(
                    column.data_type,
                    proximadb_data_model::ProximaType::DenseVector { .. }
                )
            })
            .map(|column| ProjectionDirective::HnswIndex {
                collection_id: schema.name.clone(),
                embedding_field: column.name.clone(),
            })
            .collect(),
        CatalogStorageSpecialization::DocumentJson => {
            vec![ProjectionDirective::DocumentJsonPathIndex {
                collection_id: schema.name.clone(),
                indexed_paths: Vec::new(),
            }]
        }
        CatalogStorageSpecialization::GraphTopology => {
            vec![ProjectionDirective::CsrRebuild {
                graph_id: schema.name.clone(),
            }]
        }
        CatalogStorageSpecialization::ObservabilityTimeSeries => schema
            .columns
            .iter()
            .find(|column| column.name == "trace_id")
            .map(|trace_column| {
                vec![ProjectionDirective::ObservabilityTraceIndex {
                    collection_id: schema.name.clone(),
                    service_name: None,
                    trace_id_field: trace_column.name.clone(),
                    span_id_field: "span_id".to_string(),
                }]
            })
            .unwrap_or_default(),
        CatalogStorageSpecialization::ExternalOpenTable => schema
            .storage_layouts
            .iter()
            .find_map(|layout| canonical_open_table_format(&layout.physical_format))
            .map(|format| {
                vec![ProjectionDirective::OpenFormatManifest {
                    namespace: "default".to_string(),
                    table_name: schema.name.clone(),
                    format,
                }]
            })
            .unwrap_or_default(),
        CatalogStorageSpecialization::LsmWriteOptimized => Vec::new(),
    }
}

fn canonical_open_table_format(format: &CatalogPhysicalFormat) -> Option<CanonicalOpenTableFormat> {
    match format {
        CatalogPhysicalFormat::Iceberg => Some(CanonicalOpenTableFormat::Iceberg),
        CatalogPhysicalFormat::Delta => Some(CanonicalOpenTableFormat::Delta),
        CatalogPhysicalFormat::Hudi => Some(CanonicalOpenTableFormat::Hudi),
        _ => None,
    }
}

fn proxima_record_to_get_response(
    record: ProximaRecord,
    _table_schema: &CatalogTableSchema,
    include_vector: bool,
    include_props: bool,
) -> RichSearchResult {
    let vector = if include_vector {
        record
            .embeddings
            .first()
            .map(|embedding| embedding.values.to_fp32_owned())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let props = if include_props {
        record
            .props
            .iter()
            .filter_map(|(key, node)| match node {
                ProximaTreeNode::Value(value) => Some((key.clone(), value.clone())),
                ProximaTreeNode::Object(_) => None,
            })
            .collect()
    } else {
        Default::default()
    };

    RichSearchResult {
        id: if record.oid.is_empty() {
            "unknown".to_string()
        } else {
            record.oid
        },
        score: 1.0,
        similarity: None,
        vector,
        props,
        version: if record.record_version == 0 {
            None
        } else {
            Some(record.record_version as u32)
        },
        timestamp: if record.created_at_ns == 0 {
            None
        } else {
            Some(record.created_at_ns / 1_000_000)
        },
        source: record.origin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::RecordBatch;
    use arrow_schema::Schema as ArrowSchema;
    use futures::stream::BoxStream;
    use proximadb_catalog::{CatalogColumn, CatalogStorageLayout};
    use proximadb_data_model::ProximaValue;
    use proximadb_data_model::{ProximaType, VectorElement};
    use proximadb_kernel::error::StorageError;
    use proximadb_records::{EmbeddingCell, EmbeddingValues, RecordScan, RecordStore};
    use proximadb_storage_common::object_store_bridge::{
        BridgeInMemoryObjectStore as InMemory, BridgeObjectStore,
    };
    use proximadb_storage_common::pax_block::SEGMENT_MAGIC;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, RwLock};

    #[test]
    fn table_record_route_uses_catalog_storage_specialization() {
        let cases_parquet = [
            ("generic", CatalogStorageSpecialization::GenericRelational),
            ("columnar", CatalogStorageSpecialization::ColumnarAnalytics),
            ("document", CatalogStorageSpecialization::DocumentJson),
            ("graph", CatalogStorageSpecialization::GraphTopology),
        ];
        for (label, spec) in cases_parquet {
            let schema = CatalogTableSchema::new(label)
                .with_workload_profile(CatalogWorkloadProfile::Olap)
                .with_storage_specialization(spec);
            assert_eq!(
                TableRecordStoreRoute::for_schema(&schema),
                TableRecordStoreRoute::ParquetIcebergStorage,
                "{label} must route to ParquetIcebergStorage"
            );
        }

        let cases_pax_vector = [
            ("pax_row_family", CatalogStorageSpecialization::PaxRowFamily),
            ("pax_oltp", CatalogStorageSpecialization::PaxOltp),
            ("pax_olap", CatalogStorageSpecialization::PaxOlap),
            ("vector_ann", CatalogStorageSpecialization::VectorAnn),
        ];
        for (label, spec) in cases_pax_vector {
            let schema = CatalogTableSchema::new(label)
                .with_workload_profile(CatalogWorkloadProfile::Vector)
                .with_storage_specialization(spec);
            let expected = if spec == CatalogStorageSpecialization::VectorAnn {
                TableRecordStoreRoute::PaxVectorStorage
            } else {
                TableRecordStoreRoute::LegacyVectorCompatibility
            };
            assert_eq!(
                TableRecordStoreRoute::for_schema(&schema),
                expected,
                "{label} must route to {expected:?}"
            );
        }

        let legacy_schema = CatalogTableSchema::new("legacy_lsm")
            .with_workload_profile(CatalogWorkloadProfile::Htap)
            .with_storage_specialization(CatalogStorageSpecialization::LsmWriteOptimized);
        assert_eq!(
            TableRecordStoreRoute::for_schema(&legacy_schema),
            TableRecordStoreRoute::LegacyVectorCompatibility
        );
    }

    #[derive(Default)]
    struct MemoryRecordStorage {
        records: RwLock<HashMap<String, ProximaRecord>>,
    }

    #[async_trait]
    impl RecordStore for MemoryRecordStorage {
        async fn upsert_record(&self, record: ProximaRecord) -> Result<ProximaRecord> {
            self.records
                .write()
                .expect("memory storage write lock")
                .insert(record.oid.clone(), record.clone());
            Ok(record)
        }

        async fn get_record(&self, key: &RecordKey) -> Result<Option<ProximaRecord>> {
            Ok(self
                .records
                .read()
                .expect("memory storage read lock")
                .get(&key.oid)
                .cloned())
        }

        async fn delete_record(&self, key: &RecordKey) -> Result<bool> {
            Ok(self
                .records
                .write()
                .expect("memory storage write lock")
                .remove(&key.oid)
                .is_some())
        }
    }

    #[derive(Default)]
    struct RecordingWalAppender {
        next_sequence: AtomicU64,
        entries: Mutex<Vec<CanonicalWalEntry>>,
    }

    struct CapturingObjectStoreBridge {
        store: Arc<dyn BridgeObjectStore>,
        writes: Mutex<Vec<(ObjectPath, Vec<String>)>>,
        segments: Mutex<Vec<(ObjectPath, Vec<u8>)>>,
    }

    impl CapturingObjectStoreBridge {
        fn new() -> Self {
            Self {
                store: Arc::new(InMemory::new()),
                writes: Mutex::new(Vec::new()),
                segments: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ObjectStoreBridge for CapturingObjectStoreBridge {
        fn inner_store(&self) -> Arc<dyn BridgeObjectStore> {
            self.store.clone()
        }

        async fn read_parquet_batches(
            &self,
            _path: &ObjectPath,
            _schema: Arc<ArrowSchema>,
            _batch_size: usize,
            _tenant_id: Option<&str>,
        ) -> std::result::Result<
            BoxStream<'static, std::result::Result<RecordBatch, StorageError>>,
            StorageError,
        > {
            Ok(Box::pin(futures::stream::empty()))
        }

        async fn write_records_to_parquet(
            &self,
            path: &ObjectPath,
            records: &[ProximaRecord],
            _tenant_id: Option<&str>,
        ) -> std::result::Result<(), StorageError> {
            self.writes.lock().unwrap().push((
                path.clone(),
                records.iter().map(|record| record.oid.clone()).collect(),
            ));
            Ok(())
        }

        async fn fetch_vector_segment(
            &self,
            _path: &ObjectPath,
            _tenant_id: Option<&str>,
        ) -> std::result::Result<Vec<u8>, StorageError> {
            Ok(Vec::new())
        }

        async fn persist_vector_segment(
            &self,
            path: &ObjectPath,
            data: &[u8],
            _tenant_id: Option<&str>,
        ) -> std::result::Result<(), StorageError> {
            self.segments
                .lock()
                .unwrap()
                .push((path.clone(), data.to_vec()));
            Ok(())
        }

        async fn latest_manifest_version(
            &self,
            _manifest_prefix: &str,
        ) -> std::result::Result<Option<u64>, StorageError> {
            Ok(None)
        }

        async fn publish_snapshot(
            &self,
            _data_prefix: &ObjectPath,
            _manifest_prefix: &str,
            _parent: Option<u64>,
        ) -> std::result::Result<
            proximadb_storage_common::object_store_bridge::CommitOutcome,
            StorageError,
        > {
            use proximadb_storage_common::object_store_bridge::CommitOutcome;
            Ok(CommitOutcome::Committed(0))
        }
    }

    #[async_trait]
    impl TableWalAppender for RecordingWalAppender {
        async fn append_operations(
            &self,
            operations: Vec<CanonicalOperation>,
            tenant_id: Option<String>,
        ) -> Result<Vec<CanonicalWalEntry>> {
            let entries = operations
                .into_iter()
                .map(|operation| {
                    let sequence_number = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
                    CanonicalWalEntry::new(sequence_number, operation, tenant_id.clone())
                })
                .collect::<Vec<_>>();
            self.entries
                .lock()
                .expect("recording WAL append lock")
                .extend(entries.clone());
            Ok(entries)
        }
    }

    #[async_trait]
    impl RecordScan for MemoryRecordStorage {
        async fn scan_records(&self, limit: usize) -> Result<Vec<ProximaRecord>> {
            Ok(self
                .records
                .read()
                .expect("memory storage read lock")
                .values()
                .take(limit)
                .cloned()
                .collect())
        }
    }

    #[tokio::test]
    async fn record_storage_scan_records_filtered_pushes_predicate_and_table_membership() {
        let storage = Arc::new(MemoryRecordStorage::default());
        let store = RecordStorageTableRecordStore::new(storage.clone());
        let schema = CatalogTableSchema::new("orders");

        for (oid, variation, version) in [
            ("keep-1", "orders", 1u64),
            ("keep-2", "orders", 2),
            ("skip-pred", "orders", 3), // belongs to table but fails predicate
            ("skip-table", "other", 1), // matches predicate but is another table
        ] {
            storage
                .upsert_record(ProximaRecord {
                    oid: oid.to_string(),
                    variation_id: Some(variation.to_string()),
                    record_version: version,
                    ..Default::default()
                })
                .await
                .unwrap();
        }

        // Predicate (the "WHERE"): oid starts with "keep". The store must apply
        // BOTH table membership (variation_id) AND the predicate.
        let pred = |r: &ProximaRecord| r.oid.starts_with("keep");
        let got = store
            .scan_records_filtered(
                &schema,
                TableRecordScanRequest {
                    filter: None,
                    table_id: "orders".to_string(),
                    limit: None,
                    include_vector: true,
                    include_props: true,
                },
                Some(&pred),
                None,
            )
            .await
            .unwrap();

        let mut oids: Vec<_> = got.iter().map(|r| r.oid.clone()).collect();
        oids.sort();
        assert_eq!(
            oids,
            vec!["keep-1".to_string(), "keep-2".to_string()],
            "only rows that belong to the table AND match the predicate"
        );
    }

    #[tokio::test]
    async fn record_storage_table_store_writes_reads_scans_and_deletes() {
        let storage = Arc::new(MemoryRecordStorage::default());
        let store = RecordStorageTableRecordStore::new(storage);
        let schema = CatalogTableSchema::new("orders");
        let record = ProximaRecord {
            oid: "o1".to_string(),
            local_id: Some("o1".to_string()),
            variation_id: Some("orders".to_string()),
            ..Default::default()
        };

        let inserted = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Insert,
                    record.clone(),
                )],
                None,
            )
            .await
            .unwrap();
        assert!(inserted.success);
        assert_eq!(inserted.record_ids, vec!["o1"]);

        let duplicate = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Insert,
                    record.clone(),
                )],
                None,
            )
            .await
            .unwrap();
        assert!(!duplicate.success);
        assert_eq!(duplicate.error_code.as_deref(), Some("INSERT_CONFLICT"));

        let fetched = store
            .get_by_key(
                &schema,
                TableRecordGetRequest {
                    table_id: "orders".to_string(),
                    key: "o1".to_string(),
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap()
            .expect("record should exist");
        assert_eq!(fetched.id, "o1");

        let scanned = store
            .scan_records(
                &schema,
                TableRecordScanRequest {
                    filter: None,
                    table_id: "orders".to_string(),
                    limit: Some(10),
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(scanned.len(), 1);

        let deleted = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Delete,
                    record,
                )],
                None,
            )
            .await
            .unwrap();
        assert!(deleted.success);
        assert!(
            store
                .get_by_key(
                    &schema,
                    TableRecordGetRequest {
                        table_id: "orders".to_string(),
                        key: "o1".to_string(),
                        include_vector: true,
                        include_props: true,
                    },
                    None,
                )
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn direct_wal_table_store_appends_canonical_wal_before_storage_state() {
        let storage = Arc::new(MemoryRecordStorage::default());
        let wal = Arc::new(RecordingWalAppender::default());
        let store = DirectWalTableRecordStore::new(storage.clone(), wal.clone());
        let schema = CatalogTableSchema::new("orders")
            .with_column(CatalogColumn::new(1, "id", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "amount", ProximaType::Float64))
            .with_storage_specialization(CatalogStorageSpecialization::PaxRowFamily);
        let record = ProximaRecord {
            oid: "o1".to_string(),
            local_id: Some("o1".to_string()),
            variation_id: Some("orders".to_string()),
            ..Default::default()
        };

        let inserted = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Insert,
                    record.clone(),
                )],
                None,
            )
            .await
            .unwrap();

        assert!(inserted.success);
        assert_eq!(inserted.record_ids, vec!["o1"]);
        assert!(
            storage
                .get_record(&RecordKey::new("o1"))
                .await
                .unwrap()
                .is_some()
        );

        let entries = wal.entries.lock().expect("recording WAL read lock");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sequence_number, 1);
        match &entries[0].operation {
            CanonicalOperation::RecordUpsert {
                collection_id,
                record,
                projections,
            } => {
                assert_eq!(collection_id, "orders");
                assert_eq!(record.oid, "o1");
                assert_eq!(projections.len(), 1);
                match &projections[0] {
                    ProjectionDirective::ColumnarVariation {
                        collection_id,
                        fields,
                    } => {
                        assert_eq!(collection_id, "orders");
                        assert_eq!(fields, &vec!["id".to_string(), "amount".to_string()]);
                    }
                    other => panic!("expected columnar/PAX projection directive, got {other:?}"),
                }
            }
            other => panic!("expected upsert WAL entry, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn direct_wal_table_store_rejects_duplicate_insert_without_wal_append() {
        let storage = Arc::new(MemoryRecordStorage::default());
        let wal = Arc::new(RecordingWalAppender::default());
        let store = DirectWalTableRecordStore::new(storage, wal.clone());
        let schema = CatalogTableSchema::new("orders")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let record = ProximaRecord {
            oid: "o1".to_string(),
            variation_id: Some("orders".to_string()),
            ..Default::default()
        };

        assert!(
            store
                .write_mutations(
                    &schema,
                    vec![TableRecordMutation::new(
                        TableRecordMutationKind::Insert,
                        record.clone(),
                    )],
                    None,
                )
                .await
                .unwrap()
                .success
        );
        let duplicate = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Insert,
                    record,
                )],
                None,
            )
            .await
            .unwrap();

        assert!(!duplicate.success);
        assert_eq!(duplicate.error_code.as_deref(), Some("INSERT_CONFLICT"));
        assert_eq!(
            wal.entries.lock().expect("recording WAL read lock").len(),
            1
        );
    }

    // ── TD-127: OLTP secondary index (build / probe / IN-list / maintenance) ──────

    /// Build a `code_symbol`-shaped schema declaring non-unique secondary indexes
    /// on `name` and `file` (the code-graph lookup columns) plus an unindexed
    /// `lang` column, PK `oid`.
    fn secondary_index_schema() -> CatalogTableSchema {
        use proximadb_catalog::{CatalogIndex, CatalogIndexType, RelationalCapabilities};
        CatalogTableSchema::new("code_symbol")
            .with_column(CatalogColumn::new(1, "oid", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "name", ProximaType::String))
            .with_column(CatalogColumn::new(3, "file", ProximaType::String))
            .with_column(CatalogColumn::new(4, "lang", ProximaType::String))
            .with_primary_key(vec!["oid".to_string()])
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp)
            .with_relational_capabilities(RelationalCapabilities {
                primary_key: vec!["oid".to_string()],
                secondary_indexes: vec![
                    CatalogIndex::new(
                        "sym_name_idx",
                        vec!["name".to_string()],
                        CatalogIndexType::Hash,
                    ),
                    CatalogIndex::new(
                        "sym_file_idx",
                        vec!["file".to_string()],
                        CatalogIndexType::Hash,
                    ),
                ],
                ..Default::default()
            })
    }

    fn symbol_record(oid: &str, name: &str, file: &str) -> ProximaRecord {
        ProximaRecord {
            oid: oid.to_string(),
            local_id: Some(oid.to_string()),
            variation_id: Some("code_symbol".to_string()),
            props: proximadb_records::ProximaTree::from([
                (
                    "name".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String(name.to_string())),
                ),
                (
                    "file".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String(file.to_string())),
                ),
            ]),
            ..Default::default()
        }
    }

    async fn write_one(
        store: &DirectWalTableRecordStore,
        schema: &CatalogTableSchema,
        kind: TableRecordMutationKind,
        record: ProximaRecord,
    ) {
        let result = store
            .write_mutations(schema, vec![TableRecordMutation::new(kind, record)], None)
            .await
            .expect("write_mutations");
        assert!(result.success, "write failed: {:?}", result.error_code);
    }

    /// Probe `column IN values` and return the matching oids, sorted.
    async fn probe_sorted(
        store: &DirectWalTableRecordStore,
        schema: &CatalogTableSchema,
        column: &str,
        values: &[&str],
    ) -> Option<Vec<String>> {
        let set: std::collections::HashSet<String> = values.iter().map(|v| v.to_string()).collect();
        store
            .lookup_secondary(schema, column, &set, None)
            .await
            .expect("lookup_secondary")
            .map(|mut oids| {
                oids.sort();
                oids
            })
    }

    #[tokio::test]
    async fn secondary_index_probes_equality_and_in_list() {
        let store = DirectWalTableRecordStore::new(
            Arc::new(MemoryRecordStorage::default()),
            Arc::new(RecordingWalAppender::default()),
        );
        let schema = secondary_index_schema();
        for (oid, name, file) in [
            ("s1", "parse", "a.rs"),
            ("s2", "parse", "b.rs"), // same name in another file
            ("s3", "emit", "b.rs"),
            ("s4", "emit", "c.rs"),
        ] {
            write_one(
                &store,
                &schema,
                TableRecordMutationKind::Insert,
                symbol_record(oid, name, file),
            )
            .await;
        }

        // Equality on the indexed `name` column → both `parse` symbols.
        assert_eq!(
            probe_sorted(&store, &schema, "name", &["parse"]).await,
            Some(vec!["s1".to_string(), "s2".to_string()])
        );
        // IN-list on the indexed `file` column → union of a.rs + c.rs.
        assert_eq!(
            probe_sorted(&store, &schema, "file", &["a.rs", "c.rs"]).await,
            Some(vec!["s1".to_string(), "s4".to_string()])
        );
        // A value with no rows → an empty (but present) answer, NOT a scan fallback.
        assert_eq!(
            probe_sorted(&store, &schema, "name", &["missing"]).await,
            Some(vec![])
        );
        // An unindexed column → `None` so the caller scans.
        assert_eq!(probe_sorted(&store, &schema, "lang", &["rust"]).await, None);
    }

    #[tokio::test]
    async fn secondary_index_maintained_on_update_and_delete() {
        let store = DirectWalTableRecordStore::new(
            Arc::new(MemoryRecordStorage::default()),
            Arc::new(RecordingWalAppender::default()),
        );
        let schema = secondary_index_schema();
        write_one(
            &store,
            &schema,
            TableRecordMutationKind::Insert,
            symbol_record("s1", "parse", "a.rs"),
        )
        .await;
        // First probe builds the index from current state.
        assert_eq!(
            probe_sorted(&store, &schema, "name", &["parse"]).await,
            Some(vec!["s1".to_string()])
        );

        // UPDATE the indexed value: the OLD value must stop matching, the NEW one start.
        write_one(
            &store,
            &schema,
            TableRecordMutationKind::Update,
            symbol_record("s1", "lex", "a.rs"),
        )
        .await;
        assert_eq!(
            probe_sorted(&store, &schema, "name", &["parse"]).await,
            Some(vec![]),
            "old indexed value detached on update"
        );
        assert_eq!(
            probe_sorted(&store, &schema, "name", &["lex"]).await,
            Some(vec!["s1".to_string()]),
            "new indexed value attached on update"
        );

        // DELETE removes the oid from the index entirely.
        write_one(
            &store,
            &schema,
            TableRecordMutationKind::Delete,
            symbol_record("s1", "lex", "a.rs"),
        )
        .await;
        assert_eq!(
            probe_sorted(&store, &schema, "name", &["lex"]).await,
            Some(vec![])
        );
    }

    #[tokio::test]
    async fn secondary_index_kill_switch_disables_probe() {
        // SAFETY: single-threaded test; set + remove the process env around the probe.
        unsafe { std::env::set_var("PROXIMADB_SECONDARY_INDEX_DISABLE", "1") };
        let store = DirectWalTableRecordStore::new(
            Arc::new(MemoryRecordStorage::default()),
            Arc::new(RecordingWalAppender::default()),
        );
        let schema = secondary_index_schema();
        write_one(
            &store,
            &schema,
            TableRecordMutationKind::Insert,
            symbol_record("s1", "parse", "a.rs"),
        )
        .await;
        // Kill-switch on → `None` (scan fallback), even for an indexed column.
        let result = probe_sorted(&store, &schema, "name", &["parse"]).await;
        unsafe { std::env::remove_var("PROXIMADB_SECONDARY_INDEX_DISABLE") };
        assert_eq!(result, None, "kill-switch forces the scan fallback");
    }

    // ── Retirement gate #2: prior-version closure and tombstone visibility ─────────
    //
    // These tests verify that below all protocol facades, the canonical write path
    // (DirectWalTableRecordStore) correctly hides deleted records from scans and
    // point-lookups, replaces old field values after an update, and allows re-insert
    // after delete without prior-version interference.

    #[tokio::test]
    async fn tombstone_is_invisible_to_scan_after_delete() {
        let storage = Arc::new(MemoryRecordStorage::default());
        let wal = Arc::new(RecordingWalAppender::default());
        let store = DirectWalTableRecordStore::new(storage, wal);
        let schema = CatalogTableSchema::new("events")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);

        for id in ["e1", "e2", "e3"] {
            store
                .write_mutations(
                    &schema,
                    vec![TableRecordMutation::new(
                        TableRecordMutationKind::Upsert,
                        ProximaRecord {
                            oid: id.to_string(),
                            variation_id: Some("events".to_string()),
                            ..Default::default()
                        },
                    )],
                    None,
                )
                .await
                .unwrap();
        }

        store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Delete,
                    ProximaRecord {
                        oid: "e2".to_string(),
                        variation_id: Some("events".to_string()),
                        ..Default::default()
                    },
                )],
                None,
            )
            .await
            .unwrap();

        let rows = store
            .scan_records(
                &schema,
                TableRecordScanRequest {
                    filter: None,
                    table_id: "events".to_string(),
                    limit: Some(10),
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap();

        let ids: Vec<&str> = rows.iter().map(|r| r.oid.as_str()).collect();
        assert!(
            !ids.contains(&"e2"),
            "tombstoned record e2 must not appear in scan; got: {ids:?}"
        );
        assert_eq!(rows.len(), 2, "only e1 and e3 should remain; got: {ids:?}");
    }

    #[tokio::test]
    async fn prior_version_is_closed_after_update() {
        let storage = Arc::new(MemoryRecordStorage::default());
        let wal = Arc::new(RecordingWalAppender::default());
        let store = DirectWalTableRecordStore::new(storage, wal);
        let schema = CatalogTableSchema::new("users")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);

        let original = ProximaRecord {
            oid: "u1".to_string(),
            variation_id: Some("users".to_string()),
            local_id: Some("alice".to_string()),
            props: [(
                "display_name".to_string(),
                ProximaTreeNode::Value(ProximaValue::String("alice".to_string())),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Insert,
                    original,
                )],
                None,
            )
            .await
            .unwrap();

        let updated = ProximaRecord {
            oid: "u1".to_string(),
            variation_id: Some("users".to_string()),
            local_id: Some("alice-renamed".to_string()),
            props: [(
                "display_name".to_string(),
                ProximaTreeNode::Value(ProximaValue::String("alice-renamed".to_string())),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let result = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Update,
                    updated,
                )],
                None,
            )
            .await
            .unwrap();
        assert!(result.success, "update must succeed");

        let fetched = store
            .get_by_key(
                &schema,
                TableRecordGetRequest {
                    table_id: "users".to_string(),
                    key: "u1".to_string(),
                    include_vector: false,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap()
            .expect("record must exist after update");

        assert_eq!(
            fetched.props.get("display_name"),
            Some(&ProximaValue::String("alice-renamed".to_string())),
            "prior version (alice) must be closed; only updated version visible"
        );
    }

    #[tokio::test]
    async fn re_insert_succeeds_after_delete_no_prior_version_interference() {
        let storage = Arc::new(MemoryRecordStorage::default());
        let wal = Arc::new(RecordingWalAppender::default());
        let store = DirectWalTableRecordStore::new(storage, wal);
        let schema = CatalogTableSchema::new("items")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);

        let record = ProximaRecord {
            oid: "i1".to_string(),
            variation_id: Some("items".to_string()),
            local_id: Some("first-generation".to_string()),
            ..Default::default()
        };
        store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Insert,
                    record,
                )],
                None,
            )
            .await
            .unwrap();

        store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Delete,
                    ProximaRecord {
                        oid: "i1".to_string(),
                        variation_id: Some("items".to_string()),
                        ..Default::default()
                    },
                )],
                None,
            )
            .await
            .unwrap();

        let re_insert = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Insert,
                    ProximaRecord {
                        oid: "i1".to_string(),
                        variation_id: Some("items".to_string()),
                        local_id: Some("second-generation".to_string()),
                        props: [(
                            "generation".to_string(),
                            ProximaTreeNode::Value(ProximaValue::String(
                                "second-generation".to_string(),
                            )),
                        )]
                        .into_iter()
                        .collect(),
                        ..Default::default()
                    },
                )],
                None,
            )
            .await
            .unwrap();
        assert!(
            re_insert.success,
            "re-insert after delete must succeed without INSERT_CONFLICT"
        );

        let fetched = store
            .get_by_key(
                &schema,
                TableRecordGetRequest {
                    table_id: "items".to_string(),
                    key: "i1".to_string(),
                    include_vector: false,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap()
            .expect("re-inserted record must be visible");
        assert_eq!(
            fetched.props.get("generation"),
            Some(&ProximaValue::String("second-generation".to_string())),
            "re-inserted record must carry second-generation data, not prior-version ghost"
        );
    }

    #[tokio::test]
    async fn vector_bearing_pax_table_writes_canonical_without_vector_ops() {
        // Gate 4: vector-bearing relational tables write embeddings into the canonical
        // WAL path without VectorOps being the mutation owner.
        use proximadb_records::EmbeddingCell;

        let storage = Arc::new(MemoryRecordStorage::default());
        let wal = Arc::new(RecordingWalAppender::default());
        let store = DirectWalTableRecordStore::new(storage.clone(), wal.clone());
        let schema = CatalogTableSchema::new("products")
            .with_column(CatalogColumn::new(1, "id", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(
                2,
                "embedding",
                ProximaType::DenseVector {
                    element: VectorElement::Float32,
                    dim: 0,
                },
            ))
            .with_storage_specialization(CatalogStorageSpecialization::PaxRowFamily);

        let record = ProximaRecord {
            oid: "p1".to_string(),
            variation_id: Some("products".to_string()),
            embeddings: vec![EmbeddingCell {
                model_id: "default".to_string(),
                modality: "dense_vector".to_string(),
                dim: 4,
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.1, 0.2, 0.3, 0.4]),
                ..Default::default()
            }],
            ..Default::default()
        };

        let result = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Insert,
                    record,
                )],
                None,
            )
            .await
            .unwrap();
        assert!(result.success, "vector-bearing insert must succeed");

        let entries = wal.entries.lock().expect("recording WAL read lock");
        assert_eq!(entries.len(), 1);
        match &entries[0].operation {
            CanonicalOperation::RecordUpsert { record, .. } => {
                assert_eq!(record.embeddings.len(), 1);
                assert_eq!(
                    record.embeddings[0].values,
                    proximadb_records::EmbeddingValues::Fp32(vec![0.1_f32, 0.2, 0.3, 0.4])
                );
            }
            other => panic!("expected RecordUpsert WAL entry, got {other:?}"),
        }
        drop(entries);

        let fetched = store
            .get_by_key(
                &schema,
                TableRecordGetRequest {
                    table_id: "products".to_string(),
                    key: "p1".to_string(),
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap()
            .expect("inserted record must be visible");
        assert_eq!(fetched.vector, vec![0.1_f32, 0.2, 0.3, 0.4]);
    }

    #[tokio::test]
    async fn direct_wal_table_store_delete_writes_canonical_delete_entry() {
        let storage = Arc::new(MemoryRecordStorage::default());
        let wal = Arc::new(RecordingWalAppender::default());
        let store = DirectWalTableRecordStore::new(storage.clone(), wal.clone());
        let schema = CatalogTableSchema::new("orders")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let record = ProximaRecord {
            oid: "o1".to_string(),
            variation_id: Some("orders".to_string()),
            ..Default::default()
        };

        store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Upsert,
                    record.clone(),
                )],
                None,
            )
            .await
            .unwrap();
        let deleted = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Delete,
                    record,
                )],
                None,
            )
            .await
            .unwrap();

        assert!(deleted.success);
        assert!(
            storage
                .get_record(&RecordKey::new("o1"))
                .await
                .unwrap()
                .is_none()
        );
        let entries = wal.entries.lock().expect("recording WAL read lock");
        assert_eq!(entries.len(), 2);
        match &entries[1].operation {
            CanonicalOperation::RecordDelete {
                collection_id, oid, ..
            } => {
                assert_eq!(collection_id, "orders");
                assert_eq!(oid, "o1");
            }
            other => panic!("expected delete WAL entry, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn object_store_iceberg_record_store_writes_parquet_via_bridge() {
        let bridge = Arc::new(CapturingObjectStoreBridge::new());
        let store = ObjectStoreIcebergRecordStore::new(bridge.clone());
        let schema = CatalogTableSchema::new("orders").with_storage_layout(
            CatalogStorageLayout::projection_publication(
                "primary",
                CatalogPhysicalFormat::Iceberg,
                "warehouse/orders",
            ),
        );
        let records = vec![
            ProximaRecord {
                oid: "o1".to_string(),
                variation_id: Some("orders".to_string()),
                ..Default::default()
            },
            ProximaRecord {
                oid: "o2".to_string(),
                variation_id: Some("orders".to_string()),
                ..Default::default()
            },
        ];

        let result = store
            .write_mutations(
                &schema,
                records
                    .into_iter()
                    .map(|record| TableRecordMutation::new(TableRecordMutationKind::Insert, record))
                    .collect(),
                None,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.record_ids, vec!["o1".to_string(), "o2".to_string()]);
        let writes = bridge.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].1, vec!["o1".to_string(), "o2".to_string()]);
        assert!(
            writes[0]
                .0
                .as_ref()
                .starts_with("data/default_tenant/ns_default/orders/data/orders-insert-")
        );
        assert!(writes[0].0.as_ref().ends_with(".parquet"));
    }

    #[tokio::test]
    async fn object_store_vector_record_store_persists_pax_segment_via_bridge() {
        let bridge = Arc::new(CapturingObjectStoreBridge::new());
        let store = ObjectStoreVectorRecordStore::new(bridge.clone());
        let schema = CatalogTableSchema::new("vectors")
            .with_workload_profile(CatalogWorkloadProfile::Vector)
            .with_storage_specialization(CatalogStorageSpecialization::VectorAnn);
        let records = vec![ProximaRecord {
            oid: "v1".to_string(),
            variation_id: Some("vectors".to_string()),
            ..Default::default()
        }];

        let result = store
            .write_mutations(
                &schema,
                records
                    .into_iter()
                    .map(|record| TableRecordMutation::new(TableRecordMutationKind::Upsert, record))
                    .collect(),
                None,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.record_ids, vec!["v1".to_string()]);
        assert!(bridge.writes.lock().unwrap().is_empty());
        let segments = bridge.segments.lock().unwrap();
        assert_eq!(segments.len(), 1);
        assert!(
            segments[0]
                .0
                .as_ref()
                .starts_with("data/default_tenant/ns_default/vectors/segments/vectors-upsert-")
        );
        assert!(segments[0].0.as_ref().ends_with(PAX_SEGMENT_EXT));
        assert!(
            segments[0].1.ends_with(SEGMENT_MAGIC),
            "persisted bytes must be a PAX segment"
        );
    }

    fn props_record(oid: &str, table: &str, props: Vec<(&str, ProximaValue)>) -> ProximaRecord {
        let mut tree = HashMap::new();
        for (key, value) in props {
            tree.insert(key.to_string(), ProximaTreeNode::Value(value));
        }
        ProximaRecord {
            oid: oid.to_string(),
            variation_id: Some(table.to_string()),
            props: tree,
            ..Default::default()
        }
    }

    /// F3 relational round trip: write records through `ObjectStoreIcebergRecordStore`
    /// over a real `IcebergObjectStoreBridge` (in-memory object store) and read
    /// them back via the Parquet read path — props + dense vector must survive,
    /// and the primary key must be recovered as the record `oid`.
    #[tokio::test]
    async fn iceberg_record_store_round_trips_through_object_store() {
        use proximadb_iceberg_engine::IcebergObjectStoreBridge;

        let bridge: Arc<dyn ObjectStoreBridge> =
            Arc::new(IcebergObjectStoreBridge::from_url("memory://").unwrap());
        let store = ObjectStoreIcebergRecordStore::new(bridge);
        let mut schema = CatalogTableSchema::new("orders")
            .with_column(CatalogColumn::new(1, "id", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "amount", ProximaType::Int64));
        schema.primary_key = vec!["id".to_string()];

        let mut r0 = props_record(
            "o1",
            "orders",
            vec![
                ("id", ProximaValue::String("o1".into())),
                ("amount", ProximaValue::Int64(30)),
            ],
        );
        r0.embeddings.push(EmbeddingCell {
            values: EmbeddingValues::Fp32(vec![1.0, 2.0, 3.0]),
            dim: 3,
            ..Default::default()
        });
        let r1 = props_record(
            "o2",
            "orders",
            vec![
                ("id", ProximaValue::String("o2".into())),
                ("amount", ProximaValue::Int64(41)),
            ],
        );

        let written = store
            .write_mutations(
                &schema,
                vec![
                    TableRecordMutation::new(TableRecordMutationKind::Insert, r0),
                    TableRecordMutation::new(TableRecordMutationKind::Insert, r1),
                ],
                None,
            )
            .await
            .unwrap();
        assert!(written.success);

        let mut scanned = store
            .scan_records(
                &schema,
                TableRecordScanRequest {
                    filter: None,
                    table_id: "orders".to_string(),
                    limit: None,
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap();
        scanned.sort_by(|a, b| a.oid.cmp(&b.oid));
        assert_eq!(scanned.len(), 2, "both written records must read back");
        assert_eq!(scanned[0].oid, "o1", "primary key recovered as oid");
        assert_eq!(scanned[1].oid, "o2");
        assert_eq!(
            proximadb_records::tree_get(&scanned[0].props, "amount"),
            Some(&ProximaValue::Int64(30))
        );
        assert_eq!(
            scanned[0]
                .embeddings
                .first()
                .map(|e| e.values.to_fp32_owned()),
            Some(vec![1.0, 2.0, 3.0]),
            "dense vector must round-trip through Parquet"
        );

        let fetched = store
            .get_by_key(
                &schema,
                TableRecordGetRequest {
                    table_id: "orders".to_string(),
                    key: "o2".to_string(),
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap()
            .expect("get_by_key must find the persisted record");
        assert_eq!(fetched.id, "o2");
        assert_eq!(fetched.props.get("amount"), Some(&ProximaValue::Int64(41)));
    }

    /// F3 vector round trip: write a vector-bearing record through
    /// `ObjectStoreVectorRecordStore` over a real `IcebergObjectStoreBridge` and
    /// read it back via the PAX segment fetch path — `oid` and the dense
    /// embedding (the vector store's canonical payload) must survive.
    #[tokio::test]
    async fn vector_record_store_round_trips_pax_segment_through_object_store() {
        use proximadb_iceberg_engine::IcebergObjectStoreBridge;

        let bridge: Arc<dyn ObjectStoreBridge> =
            Arc::new(IcebergObjectStoreBridge::from_url("memory://").unwrap());
        let store = ObjectStoreVectorRecordStore::new(bridge);
        let schema = CatalogTableSchema::new("vectors")
            .with_workload_profile(CatalogWorkloadProfile::Vector)
            .with_storage_specialization(CatalogStorageSpecialization::VectorAnn);

        let mut props = HashMap::new();
        props.insert(
            "category".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("books".into())),
        );
        let record = ProximaRecord {
            oid: "v1".to_string(),
            variation_id: Some("vectors".to_string()),
            created_at_ns: 1_700_000_000_000_000_000,
            props,
            embeddings: vec![EmbeddingCell {
                modality: "dense".into(),
                dim: 4,
                values: EmbeddingValues::Fp32(vec![0.1, 0.2, 0.3, 0.4]),
                ..Default::default()
            }],
            ..Default::default()
        };

        let written = store
            .write_mutations(
                &schema,
                vec![TableRecordMutation::new(
                    TableRecordMutationKind::Upsert,
                    record,
                )],
                None,
            )
            .await
            .unwrap();
        assert!(written.success);

        let scanned = store
            .scan_records(
                &schema,
                TableRecordScanRequest {
                    filter: None,
                    table_id: "vectors".to_string(),
                    limit: None,
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(scanned.len(), 1, "the persisted record must read back");
        assert_eq!(scanned[0].oid, "v1", "oid must round-trip through PAX");
        // PAX v2 stores dense vectors with SQ8 scalar quantization (lossy, 4x), so
        // the embedding reconstructs within one quantization step rather than bit-exactly.
        let got = scanned[0]
            .embeddings
            .first()
            .map(|e| e.values.to_fp32_owned())
            .expect("dense embedding must round-trip through the PAX segment");
        let expected = [0.1_f32, 0.2, 0.3, 0.4];
        assert_eq!(got.len(), expected.len(), "embedding dim must round-trip");
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!(
                (g - e).abs() <= 0.02,
                "dense embedding must round-trip within SQ8 tolerance: got {got:?} vs {expected:?}"
            );
        }
        // Phase B: props + timestamps now round-trip (was oid+embedding only).
        assert_eq!(
            proximadb_records::tree_get(&scanned[0].props, "category"),
            Some(&ProximaValue::String("books".into())),
            "props must round-trip through the PAX segment"
        );
        assert_eq!(
            scanned[0].created_at_ns, 1_700_000_000_000_000_000,
            "created_at must round-trip through the PAX segment"
        );

        let fetched = store
            .get_by_key(
                &schema,
                TableRecordGetRequest {
                    table_id: "vectors".to_string(),
                    key: "v1".to_string(),
                    include_vector: true,
                    include_props: false,
                },
                None,
            )
            .await
            .unwrap()
            .expect("get_by_key must find the persisted vector record");
        // SQ8-quantized reconstruction: compare within one quantization step.
        let expected = [0.1_f32, 0.2, 0.3, 0.4];
        assert_eq!(
            fetched.vector.len(),
            expected.len(),
            "vector dim must round-trip"
        );
        for (g, e) in fetched.vector.iter().zip(expected.iter()) {
            assert!(
                (g - e).abs() <= 0.02,
                "get_by_key vector must round-trip within SQ8 tolerance: got {:?} vs {expected:?}",
                fetched.vector
            );
        }
    }

    /// Phase D: the recovered `oid` byte-matches the catalog's canonical
    /// composite-key encoding (`CatalogRow::primary_key_string`), not a divergent
    /// local join — so a read-back record carries the same oid the write path
    /// (`CatalogRow::to_proxima_record`) produced.
    #[test]
    fn reconstruct_oid_matches_catalog_canonical_composite_key() {
        let mut schema = CatalogTableSchema::new("orders")
            .with_column(CatalogColumn::new(1, "region", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "id", ProximaType::Int64).nullable(false));
        schema.primary_key = vec!["region".to_string(), "id".to_string()];

        let mut props = HashMap::new();
        props.insert(
            "region".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("us".into())),
        );
        props.insert(
            "id".to_string(),
            ProximaTreeNode::Value(ProximaValue::Int64(7)),
        );
        let record = ProximaRecord {
            props,
            ..Default::default()
        };

        let mut values = HashMap::new();
        values.insert("region".to_string(), ProximaValue::String("us".into()));
        values.insert("id".to_string(), ProximaValue::Int64(7));
        let canonical = proximadb_catalog::relational::CatalogRow {
            table: "orders".to_string(),
            values,
        }
        .primary_key_string(&schema)
        .unwrap()
        .expect("composite PK must produce a key");

        assert_eq!(
            reconstruct_oid(&record, &schema),
            canonical,
            "recovered oid must match CatalogRow::primary_key_string"
        );
    }
}

/// OLAP publication target: Parquet files and Iceberg metadata over object storage.
///
/// This path is for analytical/open-format publications and external-authority
/// tables. It is not the direct write authority for PostgreSQL-style mutable
/// OLTP tables; those commit through the WAL/row-delta path first.
pub struct ObjectStoreIcebergRecordStore {
    bridge: Arc<dyn ObjectStoreBridge>,
}

impl ObjectStoreIcebergRecordStore {
    pub fn new(bridge: Arc<dyn ObjectStoreBridge>) -> Self {
        Self { bridge }
    }

    /// Read every current record for `schema` by listing the Parquet data
    /// objects the write path produced under the table's `data/` prefix and
    /// decoding each via the bridge's `read_parquet_batches`.
    ///
    /// This is a full-scan leaf read: every data object is listed and decoded.
    /// The advisory schema is `empty()` because the bridge's v1 read returns the
    /// file's batches as written (the Parquet file embeds its own schema) and the
    /// canonical reverse converter is self-describing. The heavy read
    /// optimizations — catalog-authoritative projection/coercion, Iceberg
    /// manifest + row-group pruning, predicate pushdown, and true streaming/range
    /// reads — are deferred to F5/P5 and layer behind this same `ObjectStoreBridge`
    /// seam (see `iceberg-engine/src/lib.rs` v1-scope notes; routing these leaf
    /// reads through DataFusion via `ComputeScheduler` is P1/P5).
    async fn read_all_records(
        &self,
        schema: &CatalogTableSchema,
        tenant_context: Option<&TenantContext>,
    ) -> Result<Vec<ProximaRecord>> {
        let base = object_store_write_base_path(schema, tenant_context);
        let prefix = ObjectPath::from(format!("{base}data"));
        let parquet_paths = list_objects_with_suffix(&self.bridge, &prefix, ".parquet").await?;

        let tenant_id = tenant_context.map(|tc| tc.tenant_id.as_str());
        record_object_store_op(tenant_id, "list_parquet");

        let mut records = Vec::new();

        for path in parquet_paths {
            record_object_store_op(tenant_id, "read_parquet");
            let mut stream = self
                .bridge
                .read_parquet_batches(
                    &path,
                    Arc::new(ArrowSchema::empty()),
                    OBJECT_STORE_READ_BATCH_SIZE,
                    tenant_id,
                )
                .await
                .map_err(|err| {
                    anyhow!("ObjectStoreIcebergRecordStore failed to read '{path}': {err}")
                })?;
            while let Some(batch) = stream.next().await {
                let batch = batch.map_err(|err| {
                    anyhow!(
                        "ObjectStoreIcebergRecordStore failed to decode batch from '{path}': {err}"
                    )
                })?;
                records.extend(record_batch_to_records(&batch, schema));
            }
        }
        Ok(records)
    }
}

#[async_trait]
impl TableRecordStore for ObjectStoreIcebergRecordStore {
    async fn write_mutations(
        &self,
        schema: &CatalogTableSchema,
        mutations: Vec<TableRecordMutation>,
        _tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordWriteResult> {
        let records = mutations
            .iter()
            .map(|mutation| mutation.record.clone())
            .collect::<Vec<_>>();
        let record_ids = records.iter().map(record_id).collect::<Vec<_>>();

        if records.is_empty() {
            return Ok(TableRecordWriteResult::success(record_ids));
        }

        let kind = mutations
            .first()
            .map(|mutation| mutation.kind)
            .unwrap_or(TableRecordMutationKind::Insert);
        let path = object_store_parquet_mutation_path(schema, kind, _tenant_context);
        let tenant_id = _tenant_context.map(|tc| tc.tenant_id.as_str());
        record_object_store_op(tenant_id, "write_parquet");
        self.bridge
            .write_records_to_parquet(&path, &records, tenant_id)
            .await
            .map_err(|err| {
                anyhow!(
                    "ObjectStoreIcebergRecordStore failed to write '{}' to '{}': {err}",
                    schema.name,
                    path
                )
            })?;

        Ok(TableRecordWriteResult {
            success: true,
            record_ids,
            metrics: OperationMetrics::default(),
            errors: vec![],
            error_code: None,
        })
    }

    async fn get_by_key(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordGetRequest,
        _tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordGetResponse> {
        let records = self.read_all_records(table_schema, _tenant_context).await?;
        let found = records.into_iter().find(|record| record.oid == request.key);
        Ok(found.map(|record| {
            proxima_record_to_get_response(
                record,
                table_schema,
                request.include_vector,
                request.include_props,
            )
        }))
    }

    async fn scan_records(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        _tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let mut records = self.read_all_records(table_schema, _tenant_context).await?;
        if let Some(limit) = request.limit {
            records.truncate(limit);
        }
        if !request.include_vector {
            for record in &mut records {
                record.embeddings.clear();
            }
        }
        if !request.include_props {
            for record in &mut records {
                record.props.clear();
            }
        }
        Ok(records)
    }
}

/// Specialized Vector/ANN Target: PAX block formats.
/// Process-global multitenant footer/index caches for the PAX v2 ranged read
/// path. The cache is inherently a process singleton; `SharedServices` calls
/// [`init_segment_caches`] once at boot, and every `ObjectStoreVectorRecordStore`
/// auto-picks them up — no threading through every constructor.
static GLOBAL_SEGMENT_CACHES: std::sync::OnceLock<(
    Arc<proximadb_storage_common::ranged_segment::FooterCache>,
    Arc<proximadb_storage_common::ranged_segment::SegmentIndexCache>,
)> = std::sync::OnceLock::new();

/// Initialize the global footer/index caches from `budget` (idempotent — first
/// call wins). Call once during server boot.
///
/// `limits_resolver` is the open-core injection seam (Dependency Inversion): OSS
/// passes `None` → uniform elastic fair share. An enterprise/control-plane boot
/// path supplies a [`proximadb_cache::LimitsResolver`] (built from an
/// operator `cache_tiers.json` [`proximadb_cache::TierPolicy`] + a tenant→tier
/// authority such as `TenantContext.tier`) to get tier-weighted preference —
/// without any commercial policy baked into the OSS engine.
pub fn init_segment_caches(
    budget: proximadb_cache::CacheBudget,
    limits_resolver: Option<Arc<proximadb_cache::LimitsResolver>>,
) {
    let mut footer = proximadb_storage_common::ranged_segment::FooterCache::new(budget.clone());
    let mut index = proximadb_storage_common::ranged_segment::SegmentIndexCache::new(budget);
    if let Some(resolver) = limits_resolver {
        footer = footer.with_limits_resolver(resolver.clone());
        index = index.with_limits_resolver(resolver);
    }
    let _ = GLOBAL_SEGMENT_CACHES.set((Arc::new(footer), Arc::new(index)));
}

/// Per-tenant stats snapshot for the global footer cache (for metrics emission).
/// Empty when caches are not initialized.
pub fn segment_cache_tenant_stats() -> Vec<proximadb_cache::TenantCacheStat> {
    GLOBAL_SEGMENT_CACHES
        .get()
        .map(|(fc, _)| fc.tenant_stats())
        .unwrap_or_default()
}

fn segment_caches() -> (
    Option<Arc<proximadb_storage_common::ranged_segment::FooterCache>>,
    Option<Arc<proximadb_storage_common::ranged_segment::SegmentIndexCache>>,
) {
    match GLOBAL_SEGMENT_CACHES.get() {
        Some((fc, ic)) => (Some(fc.clone()), Some(ic.clone())),
        None => (None, None),
    }
}

/// Process-global tenant→tier map (the *authority* hook for the open-core cache
/// tier policy). OSS leaves it empty (→ uniform fair share); the auth/control
/// plane populates it from a request tier claim via [`set_tenant_tier`], and the
/// cache `LimitsResolver` (built from an operator `cache_tiers.json`) reads it.
/// Commercial tier *data* stays out of OSS — this only carries opaque tier ids.
static TENANT_TIERS: std::sync::LazyLock<dashmap::DashMap<String, String>> =
    std::sync::LazyLock::new(dashmap::DashMap::new);

/// Record (or update) a tenant's tier id — called by the auth layer from a
/// request claim/header. No-op semantics for unknown tenants (just inserts).
pub fn set_tenant_tier(tenant_id: impl Into<String>, tier: impl Into<String>) {
    TENANT_TIERS.insert(tenant_id.into(), tier.into());
}

/// The recorded tier id for a tenant, if the control plane has stamped one.
pub fn tenant_tier(tenant_id: &str) -> Option<String> {
    TENANT_TIERS.get(tenant_id).map(|r| r.clone())
}

/// Resolve a tenant to its [`Tier`] from the header-fed registry above — the
/// co-design C5 tenant→tier bridge. Parses the stamped claim via
/// [`Tier::from_claim`] (alias-aware) and falls back to the configured default
/// tier when no claim was stamped or it is unrecognized. This converges the
/// header-fed tier source (the cache `LimitsResolver`) with the route cost
/// model's tier multiplier — one tier registry, not two. Installed at startup
/// into `tenant_tier::set_tenant_tier_resolver` (see `database.rs`).
pub fn tenant_tier_resolved(tenant_id: &str) -> crate::catalog::tenant_tier::Tier {
    tenant_tier(tenant_id)
        .and_then(|claim| crate::catalog::tenant_tier::Tier::from_claim(&claim))
        .unwrap_or_else(crate::catalog::tenant_tier::default_tier)
}

#[cfg(test)]
mod tenant_tier_bridge_tests {
    use super::*;
    use crate::catalog::tenant_tier::{Tier, default_tier};

    #[test]
    fn resolves_stamped_tier_else_default() {
        // Unique tenant ids so this never races sibling tests on the global map.
        let unknown = "bridge-test-unknown-tenant";
        assert_eq!(tenant_tier_resolved(unknown), default_tier());

        let t = "bridge-test-pro-tenant";
        set_tenant_tier(t, "pro"); // control-plane stamped X-Tenant-Tier: pro
        assert_eq!(tenant_tier_resolved(t), Tier::Tier3);

        // An unrecognized claim falls back to the default tier (fail-safe).
        let bad = "bridge-test-bad-claim-tenant";
        set_tenant_tier(bad, "not-a-tier");
        assert_eq!(tenant_tier_resolved(bad), default_tier());
    }
}

pub struct ObjectStoreVectorRecordStore {
    bridge: Arc<dyn ObjectStoreBridge>,
    footer_cache: Option<Arc<proximadb_storage_common::ranged_segment::FooterCache>>,
    index_cache: Option<Arc<proximadb_storage_common::ranged_segment::SegmentIndexCache>>,
}

impl ObjectStoreVectorRecordStore {
    pub fn new(bridge: Arc<dyn ObjectStoreBridge>) -> Self {
        // Auto-pick up the process-global caches if SharedServices initialized them.
        let (footer_cache, index_cache) = segment_caches();
        Self {
            bridge,
            footer_cache,
            index_cache,
        }
    }

    /// Read every current record for `schema` by listing the PAX segment objects
    /// the write path produced under the table's `segments/` prefix and decoding
    /// each via the bridge's `fetch_vector_segment`.
    async fn read_all_records(
        &self,
        schema: &CatalogTableSchema,
        tenant_context: Option<&TenantContext>,
    ) -> Result<Vec<ProximaRecord>> {
        let base = object_store_write_base_path(schema, tenant_context);
        let prefix = ObjectPath::from(format!("{base}segments"));
        let segment_paths =
            list_objects_with_suffix(&self.bridge, &prefix, PAX_SEGMENT_EXT).await?;
        let tenant_id = tenant_context.map(|tc| tc.tenant_id.as_str());
        record_object_store_op(tenant_id, "list_pax");
        io_trace::record_op_str("list_pax");

        let mut records = Vec::new();
        for path in segment_paths {
            record_object_store_op(tenant_id, "fetch_pax");
            io_trace::record_op_str("fetch_pax");
            let bytes = self
                .bridge
                .fetch_vector_segment(&path, tenant_id)
                .await
                .map_err(|err| {
                    anyhow!("ObjectStoreVectorRecordStore failed to fetch '{path}': {err}")
                })?;
            io_trace::record_bytes_read(bytes.len() as u64);
            // KOU read-egress (Dimension 2): the fetched bytes left object storage
            // for compute. Metered per-(tenant, locality, direction); only
            // chargeable (cross-region/-cloud/on-prem/internet) localities feed the
            // cost model's egress term — same-region/AZ reads are free.
            record_kou_bytes(
                tenant_id,
                object_store_egress_locality(),
                "read",
                bytes.len() as u64,
            );
            records.extend(pax_segment_to_records(bytes, schema)?);
        }
        Ok(records)
    }
}

/// Reconstruct full [`ProximaRecord`]s from a PAX segment via the canonical
/// segment decoder ([`PaxSegmentScanner::read_records`], the inverse of
/// `PaxSegmentWriter::add_record`). Props, labels, edges, timestamps, and the
/// dense embedding all round-trip.
///
/// Embedding model ids and promoted user-column keys are not persisted
/// positionally by the format, so we pass empty slices (best-effort `model_N`
/// defaults); deriving them from the catalog schema is a follow-up. `variation_id`
/// (also not a canonical PAX column) is restamped from the table name.
fn pax_segment_to_records(
    bytes: Vec<u8>,
    schema: &CatalogTableSchema,
) -> Result<Vec<ProximaRecord>> {
    let mut scanner = PaxSegmentScanner::from_bytes(bytes, ScanPredicate::default())
        .map_err(|err| anyhow!("ObjectStoreVectorRecordStore failed to open PAX segment: {err}"))?;
    let mut records = scanner.read_records(&[], &[], None).map_err(|err| {
        anyhow!("ObjectStoreVectorRecordStore failed to decode PAX segment: {err}")
    })?;
    for record in &mut records {
        record.variation_id = Some(schema.name.clone());
    }
    Ok(records)
}

#[async_trait]
impl TableRecordStore for ObjectStoreVectorRecordStore {
    async fn write_mutations(
        &self,
        schema: &CatalogTableSchema,
        mutations: Vec<TableRecordMutation>,
        _tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordWriteResult> {
        let records = mutations
            .iter()
            .map(|mutation| mutation.record.clone())
            .collect::<Vec<_>>();
        let record_ids = records.iter().map(record_id).collect::<Vec<_>>();

        if records.is_empty() {
            return Ok(TableRecordWriteResult::success(record_ids));
        }

        let kind = mutations
            .first()
            .map(|mutation| mutation.kind)
            .unwrap_or(TableRecordMutationKind::Insert);
        let object_path = object_store_pax_segment_path(schema, kind, _tenant_context);
        let local_path = temp_pax_segment_path(schema, kind);
        let mut writer = PaxSegmentWriter::new(
            &local_path,
            BlockMode::Pax,
            BlockCompression::None,
            &schema.name,
            0,
            embedding_count_for_records(&records),
            None,
        );
        for record in &records {
            writer.add_record(record).map_err(|err| {
                anyhow!(
                    "ObjectStoreVectorRecordStore failed to encode '{}' as PAX: {err}",
                    schema.name
                )
            })?;
        }
        let segment_meta = writer.finish().map_err(|err| {
            anyhow!(
                "ObjectStoreVectorRecordStore failed to finish PAX segment for '{}': {err}",
                schema.name
            )
        })?;
        let bytes = std::fs::read(&segment_meta.path).map_err(|err| {
            anyhow!(
                "ObjectStoreVectorRecordStore failed to read staged PAX segment '{}': {err}",
                segment_meta.path.display()
            )
        })?;
        let remove_result = std::fs::remove_file(&segment_meta.path);

        let tenant_id = _tenant_context.map(|tc| tc.tenant_id.as_str());
        self.bridge
            .persist_vector_segment(&object_path, &bytes, tenant_id)
            .await
            .map_err(|err| {
                anyhow!(
                    "ObjectStoreVectorRecordStore failed to persist '{}' to '{}': {err}",
                    schema.name,
                    object_path
                )
            })?;
        if let Err(err) = remove_result {
            tracing::debug!(
                "failed to remove staged PAX segment '{}': {}",
                segment_meta.path.display(),
                err
            );
        }

        Ok(TableRecordWriteResult {
            success: true,
            record_ids,
            metrics: OperationMetrics::default(),
            errors: vec![],
            error_code: None,
        })
    }

    async fn get_by_key(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordGetRequest,
        _tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordGetResponse> {
        let records = self.read_all_records(table_schema, _tenant_context).await?;
        let found = records.into_iter().find(|record| record.oid == request.key);
        Ok(found.map(|record| {
            proxima_record_to_get_response(
                record,
                table_schema,
                request.include_vector,
                request.include_props,
            )
        }))
    }

    async fn scan_records(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        _tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let mut records = self.read_all_records(table_schema, _tenant_context).await?;
        if let Some(limit) = request.limit {
            records.truncate(limit);
        }
        if !request.include_vector {
            for record in &mut records {
                record.embeddings.clear();
            }
        }
        if !request.include_props {
            for record in &mut records {
                record.props.clear();
            }
        }
        Ok(records)
    }
    /// Override: when a structured `request.filter` is present, read segments via
    /// footer-first **ranged** reads and skip whole blocks the filter provably
    /// excludes (predicate pushdown) before fetching their bodies. The row-exact
    /// `predicate` closure is still applied on top (block pruning is coarse), and
    /// the scan stops at `request.limit`. With no structured filter, this falls
    /// back to the default materialize-then-filter behavior.
    async fn scan_records_filtered(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        predicate: Option<&RecordScanPredicate<'_>>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        let Some(filter) = request.filter.clone() else {
            let limit = request.limit.unwrap_or(usize::MAX);
            let mut req = request;
            req.limit = None;
            let mut all = self.scan_records(table_schema, req, tenant_context).await?;
            let mut kept = 0usize;
            all.retain(|record| {
                if kept >= limit {
                    return false;
                }
                let keep = predicate.is_none_or(|p| p(record));
                if keep {
                    kept += 1;
                }
                keep
            });
            return Ok(all);
        };

        let limit = request.limit.unwrap_or(usize::MAX);
        let base = object_store_write_base_path(table_schema, tenant_context);
        let prefix = ObjectPath::from(format!("{base}segments"));
        let segment_paths =
            list_objects_with_suffix(&self.bridge, &prefix, PAX_SEGMENT_EXT).await?;
        let tenant_id = tenant_context.map(|tc| tc.tenant_id.as_str());
        record_object_store_op(tenant_id, "list_pax");
        io_trace::record_op_str("list_pax");
        let field_to_col: &(dyn Fn(&str) -> Option<i32> + Sync) =
            &crate::storage::engines::sst::segment_format::pax_field_to_col;

        let mut out = Vec::new();
        'segments: for path in segment_paths {
            record_object_store_op(tenant_id, "fetch_pax_ranged");
            io_trace::record_op_str("fetch_pax_ranged");
            let reader = RangedSegmentReader::open_with_cache(
                self.bridge.as_ref(),
                path.clone(),
                tenant_id,
                self.footer_cache.clone(),
                self.index_cache.clone(),
            )
            .await
            .map_err(|err| anyhow!("ranged open '{path}': {err}"))?;
            let recs = reader
                .read_records_pruned(&filter, field_to_col, &[], &[])
                .await
                .map_err(|err| anyhow!("ranged pruned read '{path}': {err}"))?;
            // Forward this open's physical read accounting into the per-query
            // I/O trace (co-design C0: object-store bytes + footer-cache
            // effectiveness, Dimensions 1 & 3).
            let st = reader.read_stats();
            io_trace::record_bytes_read(st.bytes_read);
            io_trace::record_range_gets(st.range_gets);
            io_trace::record_footers(st.footer_hits, st.footer_misses);
            // KOU read-egress (Dimension 2): ranged-read bytes that left object storage.
            record_kou_bytes(
                tenant_id,
                object_store_egress_locality(),
                "read",
                st.bytes_read,
            );
            for mut record in recs {
                record.variation_id = Some(table_schema.name.clone());
                if predicate.is_none_or(|p| p(&record)) {
                    if !request.include_vector {
                        record.embeddings.clear();
                    }
                    if !request.include_props {
                        record.props.clear();
                    }
                    out.push(record);
                    if out.len() >= limit {
                        break 'segments;
                    }
                }
            }
        }
        Ok(out)
    }
}
