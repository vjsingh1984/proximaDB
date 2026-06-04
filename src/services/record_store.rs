//! Canonical table/record write boundary for SQL, document, graph, vector, and
//! observability facades.
//!
//! This module is the service-layer ownership boundary for cataloged
//! `ProximaRecord` mutations. Protocol and modality facades should lower into
//! this API rather than depending on vector-specific operation names. The first
//! implementation delegates to `VectorOps` as a compatibility adapter until the
//! WAL/storage spine exposes a direct table-record writer.

use std::{
    fs,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};
use arrow_array::{
    Array, ArrayRef, BooleanArray, Date32Array, FixedSizeBinaryArray, Float32Array, Float64Array,
    Int8Array, Int16Array, Int32Array, Int64Array, LargeStringArray, RecordBatch, StringArray,
    UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::Schema as ArrowSchema;
use async_trait::async_trait;
use futures::StreamExt;
use proximadb_block_format::col_id;
use proximadb_block_format::{BlockCompression, BlockMode};
use proximadb_catalog::{
    CatalogPhysicalFormat, CatalogStorageLayout, CatalogStorageSpecialization, CatalogTableSchema,
    CatalogWorkloadProfile,
};
use proximadb_records::{
    EmbeddingCell, EmbeddingValues, ProximaRecord, ProximaTreeNode, ProximaValue, RecordKey,
    RecordScanOptions, RecordScanPredicate, RecordStorage,
};
use proximadb_storage_common::object_store_bridge::{
    BridgeObjectPath as ObjectPath, BridgeObjectStore, ObjectStoreBridge,
};
use proximadb_storage_common::{
    CanonicalOpenTableFormat, CanonicalOperation, CanonicalWalEntry, ProjectionDirective,
    pax_block::{
        PAX_SEGMENT_EXT, PaxSegmentScanner, PaxSegmentWriter, SEGMENT_MAGIC, ScanPredicate,
    },
};

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
#[derive(Debug, Clone)]
pub struct TableRecordScanRequest {
    /// xCatalog table or current compatibility collection identifier.
    pub table_id: String,
    /// Maximum number of records to scan. `None` means unbounded.
    pub limit: Option<usize>,
    /// Whether vector embeddings should be included.
    pub include_vector: bool,
    /// Whether scalar/document props should be included.
    pub include_props: bool,
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

fn primary_layout(schema: &CatalogTableSchema) -> Option<&CatalogStorageLayout> {
    schema
        .storage_layouts
        .iter()
        .rev()
        .find(|layout| layout.name == "primary")
        .or_else(|| schema.storage_layouts.first())
}

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

fn object_store_write_base_path(schema: &CatalogTableSchema) -> String {
    primary_layout(schema)
        .and_then(|layout| match layout.physical_format {
            CatalogPhysicalFormat::Iceberg | CatalogPhysicalFormat::Parquet => {
                layout.location.as_deref()
            }
            _ => None,
        })
        .or(schema.location.as_deref())
        .map(normalize_object_path_prefix)
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| format!("tables/{}", sanitize_object_path_segment(&schema.name)))
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
) -> ObjectPath {
    let base = object_store_write_base_path(schema);
    let table = sanitize_object_path_segment(&schema.name);
    let sequence = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    ObjectPath::from(format!(
        "{base}/data/{table}-{}-{sequence}.parquet",
        mutation_kind_label(kind)
    ))
}

fn object_store_pax_segment_path(
    schema: &CatalogTableSchema,
    kind: TableRecordMutationKind,
) -> ObjectPath {
    let base = object_store_write_base_path(schema);
    let table = sanitize_object_path_segment(&schema.name);
    let sequence = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    ObjectPath::from(format!(
        "{base}/segments/{table}-{}-{sequence}{PAX_SEGMENT_EXT}",
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

/// Effective primary-key column names, preferring the relational-capabilities
/// declaration and falling back to the legacy `primary_key` list.
fn effective_primary_key(schema: &CatalogTableSchema) -> Vec<&str> {
    if !schema.relational_capabilities.primary_key.is_empty() {
        schema
            .relational_capabilities
            .primary_key
            .iter()
            .map(String::as_str)
            .collect()
    } else {
        schema.primary_key.iter().map(String::as_str).collect()
    }
}

/// Stringify a scalar [`ProximaValue`] for primary-key reconstruction. Complex
/// values (which cannot be a PK column) yield `None`.
fn proxima_value_to_key_string(value: &ProximaValue) -> Option<String> {
    Some(match value {
        ProximaValue::String(s) | ProximaValue::Symbol(s) | ProximaValue::Decimal(s) => s.clone(),
        ProximaValue::Boolean(b) => b.to_string(),
        ProximaValue::Int8(x) => x.to_string(),
        ProximaValue::Int16(x) => x.to_string(),
        ProximaValue::Int32(x) => x.to_string(),
        ProximaValue::Int64(x) => x.to_string(),
        ProximaValue::UInt8(x) => x.to_string(),
        ProximaValue::UInt16(x) => x.to_string(),
        ProximaValue::UInt32(x) => x.to_string(),
        ProximaValue::UInt64(x) => x.to_string(),
        ProximaValue::Float32(x) => x.to_string(),
        ProximaValue::Float64(x) => x.to_string(),
        _ => return None,
    })
}

/// Recover the logical record key (`oid`) from a read-back record's `props`.
///
/// The schema-less `write_records_to_parquet` path does not persist `oid`
/// separately — it is the cataloged primary-key column value(s) in `props`
/// (matching `CatalogRow::primary_key_string`). Composite keys join with `::`.
/// Returns an empty string when the PK is absent/non-scalar (no fabricated key).
fn reconstruct_oid(record: &ProximaRecord, schema: &CatalogTableSchema) -> String {
    let pk = effective_primary_key(schema);
    if pk.is_empty() {
        return String::new();
    }
    let mut parts = Vec::with_capacity(pk.len());
    for col in &pk {
        match proximadb_records::tree_get(&record.props, col).and_then(proxima_value_to_key_string)
        {
            Some(part) => parts.push(part),
            None => return String::new(),
        }
    }
    parts.join("::")
}

/// Map a single Arrow cell to a [`ProximaValue`]; `null`/unsupported → `None`
/// (the key is simply omitted from `props`, matching the write-side mapping).
fn arrow_cell_to_proxima_value(array: &ArrayRef, row: usize) -> Option<ProximaValue> {
    if array.is_null(row) {
        return None;
    }
    let any = array.as_any();
    if let Some(a) = any.downcast_ref::<StringArray>() {
        Some(ProximaValue::String(a.value(row).to_string()))
    } else if let Some(a) = any.downcast_ref::<LargeStringArray>() {
        Some(ProximaValue::String(a.value(row).to_string()))
    } else if let Some(a) = any.downcast_ref::<BooleanArray>() {
        Some(ProximaValue::Boolean(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<Int64Array>() {
        Some(ProximaValue::Int64(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<Int32Array>() {
        Some(ProximaValue::Int32(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<Int16Array>() {
        Some(ProximaValue::Int16(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<Int8Array>() {
        Some(ProximaValue::Int8(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<UInt64Array>() {
        Some(ProximaValue::UInt64(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<UInt32Array>() {
        Some(ProximaValue::UInt32(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<UInt16Array>() {
        Some(ProximaValue::UInt16(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<UInt8Array>() {
        Some(ProximaValue::UInt8(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<Float64Array>() {
        Some(ProximaValue::Float64(a.value(row)))
    } else if let Some(a) = any.downcast_ref::<Float32Array>() {
        Some(ProximaValue::Float32(a.value(row)))
    } else {
        any.downcast_ref::<Date32Array>()
            .map(|a| ProximaValue::Date(a.value(row)))
    }
}

/// Reverse of `proxima_records_to_record_batch`: turn one Arrow [`RecordBatch`]
/// back into canonical [`ProximaRecord`]s. Scalar columns become `props`; a
/// `FixedSizeBinary` column is decoded as the little-endian fp32 dense vector
/// (the layout `proxima_arrow` writes for [`ProximaDataType::Vector`]).
fn record_batch_to_records(batch: &RecordBatch, schema: &CatalogTableSchema) -> Vec<ProximaRecord> {
    let n = batch.num_rows();
    let mut out: Vec<ProximaRecord> = (0..n).map(|_| ProximaRecord::default()).collect();
    let arrow_schema = batch.schema();

    for (col_idx, field) in arrow_schema.fields().iter().enumerate() {
        let array = batch.column(col_idx);
        if let Some(fsb) = array.as_any().downcast_ref::<FixedSizeBinaryArray>() {
            for (row, record) in out.iter_mut().enumerate() {
                if fsb.is_null(row) {
                    continue;
                }
                let bytes = fsb.value(row);
                let values: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                let dim = values.len() as u32;
                record.embeddings.push(EmbeddingCell {
                    values: EmbeddingValues::Fp32(values),
                    dim,
                    ..Default::default()
                });
            }
            continue;
        }
        let name = field.name();
        for (row, record) in out.iter_mut().enumerate() {
            if let Some(value) = arrow_cell_to_proxima_value(array, row) {
                record
                    .props
                    .insert(name.clone(), ProximaTreeNode::Value(value));
            }
        }
    }

    for record in &mut out {
        record.oid = reconstruct_oid(record, schema);
        record.variation_id = Some(schema.name.clone());
    }
    out
}

/// List object keys under `prefix` via the bridge's underlying store, keeping
/// only those that end with `suffix` (e.g. `.parquet` / `.pax`).
///
/// NOTE: listing goes through the raw `inner_store()` (the bridge trait exposes
/// no base-aware list), so the returned keys are correct for stores whose base
/// prefix is empty — the `memory://` / `InMemory` and bucket-root deployments.
/// A base-aware bridge list method is the follow-up for prefixed deployments.
async fn list_objects_with_suffix(
    bridge: &Arc<dyn ObjectStoreBridge>,
    prefix: &ObjectPath,
    suffix: &str,
) -> Result<Vec<ObjectPath>> {
    let store: Arc<dyn BridgeObjectStore> = bridge.inner_store();
    let mut listing = store.list(Some(prefix));
    let mut paths = Vec::new();
    while let Some(meta) = listing.next().await {
        let meta =
            meta.map_err(|err| anyhow!("object-store list under '{prefix}' failed: {err}"))?;
        if meta.location.as_ref().ends_with(suffix) {
            paths.push(meta.location);
        }
    }
    paths.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
    Ok(paths)
}

/// Canonical table-record store API.
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
            let keep = predicate.map_or(true, |p| p(record));
            if keep {
                kept += 1;
            }
            keep
        });
        Ok(all)
    }
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
pub struct DirectWalTableRecordStore {
    storage: Arc<dyn RecordStorage>,
    wal_appender: Arc<dyn TableWalAppender>,
}

impl DirectWalTableRecordStore {
    /// Create a direct writer over canonical storage and WAL appender.
    pub fn new(storage: Arc<dyn RecordStorage>, wal_appender: Arc<dyn TableWalAppender>) -> Self {
        Self {
            storage,
            wal_appender,
        }
    }
}

#[async_trait]
impl TableRecordStore for DirectWalTableRecordStore {
    async fn write_mutations(
        &self,
        table_schema: &CatalogTableSchema,
        mutations: Vec<TableRecordMutation>,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordWriteResult> {
        let mut operations = Vec::with_capacity(mutations.len());
        let mut storage_actions = Vec::with_capacity(mutations.len());
        let projections = projection_directives_for_schema(table_schema);

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
                    operations.push(CanonicalOperation::RecordUpsert {
                        collection_id: table_schema.name.clone(),
                        record: Box::new(record.clone()),
                        projections: projections.clone(),
                    });
                    storage_actions.push((kind, record));
                }
                TableRecordMutationKind::Update => {
                    if self.storage.get_record(&key).await?.is_none() {
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

        let mut record_ids = Vec::with_capacity(storage_actions.len());
        for (kind, record) in storage_actions {
            match kind {
                TableRecordMutationKind::Insert
                | TableRecordMutationKind::Upsert
                | TableRecordMutationKind::Update => {
                    let written = self.storage.upsert_record(record).await?;
                    record_ids.push(written.oid);
                }
                TableRecordMutationKind::Delete => {
                    self.storage
                        .delete_record(&RecordKey::from(&record))
                        .await?;
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
        RecordStorageTableRecordStore::new(self.storage.clone())
            .get_by_key(table_schema, request, tenant_context)
            .await
    }

    async fn scan_records(
        &self,
        table_schema: &CatalogTableSchema,
        request: TableRecordScanRequest,
        tenant_context: Option<&TenantContext>,
    ) -> Result<TableRecordScanResponse> {
        RecordStorageTableRecordStore::new(self.storage.clone())
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
        RecordStorageTableRecordStore::new(self.storage.clone())
            .scan_records_filtered(table_schema, request, predicate, tenant_context)
            .await
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
            .filter(|column| matches!(column.data_type, proximadb_catalog::CatalogDataType::Vector))
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
    use proximadb_catalog::{CatalogColumn, CatalogDataType, CatalogStorageLayout};
    use proximadb_data_model::ProximaValue;
    use proximadb_kernel::error::StorageError;
    use proximadb_records::{RecordScan, RecordStore};
    use proximadb_storage_common::object_store_bridge::{
        BridgeInMemoryObjectStore as InMemory, BridgeObjectStore,
    };
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
        ) -> std::result::Result<Vec<u8>, StorageError> {
            Ok(Vec::new())
        }

        async fn persist_vector_segment(
            &self,
            path: &ObjectPath,
            data: &[u8],
        ) -> std::result::Result<(), StorageError> {
            self.segments
                .lock()
                .unwrap()
                .push((path.clone(), data.to_vec()));
            Ok(())
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
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "amount", CatalogDataType::Float64))
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
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "embedding", CatalogDataType::Vector))
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
                .starts_with("warehouse/orders/data/orders-insert-")
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
                .starts_with("tables/vectors/segments/vectors-upsert-")
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
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "amount", CatalogDataType::Int64));
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

        let record = ProximaRecord {
            oid: "v1".to_string(),
            variation_id: Some("vectors".to_string()),
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
        assert_eq!(
            scanned[0]
                .embeddings
                .first()
                .map(|e| e.values.to_fp32_owned()),
            Some(vec![0.1, 0.2, 0.3, 0.4]),
            "dense embedding must round-trip through the PAX segment"
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
        assert_eq!(fetched.vector, vec![0.1, 0.2, 0.3, 0.4]);
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
    async fn read_all_records(&self, schema: &CatalogTableSchema) -> Result<Vec<ProximaRecord>> {
        let base = object_store_write_base_path(schema);
        let prefix = ObjectPath::from(format!("{base}/data"));
        let parquet_paths = list_objects_with_suffix(&self.bridge, &prefix, ".parquet").await?;

        let mut records = Vec::new();
        for path in parquet_paths {
            let mut stream = self
                .bridge
                .read_parquet_batches(
                    &path,
                    Arc::new(ArrowSchema::empty()),
                    OBJECT_STORE_READ_BATCH_SIZE,
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
        let path = object_store_parquet_mutation_path(schema, kind);
        self.bridge
            .write_records_to_parquet(&path, &records)
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
        let records = self.read_all_records(table_schema).await?;
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
        let mut records = self.read_all_records(table_schema).await?;
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
pub struct ObjectStoreVectorRecordStore {
    bridge: Arc<dyn ObjectStoreBridge>,
}

impl ObjectStoreVectorRecordStore {
    pub fn new(bridge: Arc<dyn ObjectStoreBridge>) -> Self {
        Self { bridge }
    }

    /// Read every current record for `schema` by listing the PAX segment objects
    /// the write path produced under the table's `segments/` prefix and decoding
    /// each via the bridge's `fetch_vector_segment`.
    async fn read_all_records(&self, schema: &CatalogTableSchema) -> Result<Vec<ProximaRecord>> {
        let base = object_store_write_base_path(schema);
        let prefix = ObjectPath::from(format!("{base}/segments"));
        let segment_paths =
            list_objects_with_suffix(&self.bridge, &prefix, PAX_SEGMENT_EXT).await?;

        let mut records = Vec::new();
        for path in segment_paths {
            let bytes = self
                .bridge
                .fetch_vector_segment(&path)
                .await
                .map_err(|err| {
                    anyhow!("ObjectStoreVectorRecordStore failed to fetch '{path}': {err}")
                })?;
            records.extend(pax_segment_to_records(bytes, schema)?);
        }
        Ok(records)
    }
}

/// Reconstruct identity + dense embedding for each row of a PAX segment.
///
/// The vector store's canonical payload is the (`oid`, embedding) pair, which we
/// recover from the `OID` string stripe and the first embedding's f32-vector
/// stripe. Props/labels/edges live in the msgpack `PROPS` stripe and require a
/// stripe→`FlatRow` decoder to rebuild — a follow-up (this path is the cold
/// OLAP/ANN segment store, not the document/relational read authority).
fn pax_segment_to_records(
    bytes: Vec<u8>,
    schema: &CatalogTableSchema,
) -> Result<Vec<ProximaRecord>> {
    let mut scanner = PaxSegmentScanner::from_bytes(bytes, ScanPredicate::default())
        .map_err(|err| anyhow!("ObjectStoreVectorRecordStore failed to open PAX segment: {err}"))?;

    let mut out = Vec::new();
    while let Some(block) = scanner.next_block() {
        let oids = block.decode_str_stripe(col_id::OID).unwrap_or_default();
        let embeddings = block.decode_f32_vec_stripe(col_id::EMBED_BASE);
        for (row, oid) in oids.into_iter().enumerate() {
            let mut record = ProximaRecord {
                oid: oid.unwrap_or_default(),
                variation_id: Some(schema.name.clone()),
                ..Default::default()
            };
            if let Some(embeds) = &embeddings
                && let Some(Some(values)) = embeds.get(row)
            {
                let dim = values.len() as u32;
                record.embeddings.push(EmbeddingCell {
                    values: EmbeddingValues::Fp32(values.clone()),
                    dim,
                    ..Default::default()
                });
            }
            out.push(record);
        }
    }
    Ok(out)
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
        let object_path = object_store_pax_segment_path(schema, kind);
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
        let bytes = fs::read(&segment_meta.path).map_err(|err| {
            anyhow!(
                "ObjectStoreVectorRecordStore failed to read staged PAX segment '{}': {err}",
                segment_meta.path.display()
            )
        })?;
        let remove_result = fs::remove_file(&segment_meta.path);
        self.bridge
            .persist_vector_segment(&object_path, &bytes)
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
        let records = self.read_all_records(table_schema).await?;
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
        let mut records = self.read_all_records(table_schema).await?;
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
