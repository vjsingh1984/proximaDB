// Document service - CRUD operations for JSON documents
//
// Provides MongoDB-like document operations:
// - Insert/upsert documents with automatic indexing
// - Get by ID with optional projection
// - Update with patch operations (set, unset, inc, push, pull, etc.)
// - Delete by ID or filter
// - Query with filters, sorting, and pagination
// - Aggregate with pipeline operations
//
// Durability: All write operations are WAL-logged before in-memory update

use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::metrics::collectors::DocumentMetricsCollector;
use crate::proto::proximadb_v1::{
    DocumentCollectionConfig, DocumentFilter, DocumentUpdate, SqlObject, SqlValue, UpdateOperation,
};
use crate::storage::persistence::write_ahead_log::wal_operations::{
    DocumentOperation, UnifiedWALOperation, UnifiedWALWriter,
};
use crate::storage::traits::UnifiedStorageFormat;
#[cfg(feature = "canonical-document-store")]
use proximadb_document::{DOCUMENT_COLLECTION_PROP, DOCUMENT_RECORD_LABEL, DocumentRecordKey};
#[cfg(feature = "canonical-document-store")]
use proximadb_records::{
    RecordKey, RecordRecoveryOperation, RecordScanOptions, RecordStorage,
    replay_record_recovery_operations,
};

use super::DocumentStorageEngine;
use super::aggregation_extensions::LookupFetcher;
use super::canonical_adapter::legacy_document_to_proxima_record;
use super::canonical_adapter::proxima_record_to_legacy_document;
use super::canonical_adapter::{proxima_tree_to_sql_object, sql_value_to_tree_node};
use super::indexes::IndexManager;
use super::query::QueryExecutor;
use super::{
    DocumentCollection, DocumentIngestResult, DocumentQueryParams, DocumentQueryResult,
    DocumentRecord, FlushToStorageResult,
};
use proximadb_data_model::ProximaValue;
use proximadb_records::conversions::sql_value_to_proxima;
use proximadb_records::{ProximaTree, ProximaTreeNode, tree_get};

/// Compose the structural per-tenant document collection key `{tenant}/{collection}` — the
/// document counterpart of graph's `scoped_graph_id`.
///
/// The DEFAULT tenant stays UNSCOPED (bare `collection`), matching the bare collections the
/// pgwire / REST v1 create paths register; named tenants scope. Validates the tenant as a path
/// segment (fail-closed) before it becomes part of a storage key. BOTH the storage-key parameter
/// AND the record's `collection_id` must be composed from this — they key the same canonical OID
/// (`document/{collection_id}/{doc_id}`), so a scoped write and its recovery/read agree
/// (TD-DOC-TENANT-1); scoping only one side mis-keys the live read immediately.
pub fn scoped_document_collection(tenant: &str, collection: &str) -> anyhow::Result<String> {
    proximadb_tenant::validate_request_tenant(tenant)
        .map_err(|e| anyhow::anyhow!("invalid tenant '{tenant}': {e}"))?;
    if tenant == proximadb_tenant::DEFAULT_TENANT {
        return Ok(collection.to_string());
    }
    Ok(format!("{tenant}/{collection}"))
}

/// Inverse of [`scoped_document_collection`]: recover `(tenant, clean_collection)` from a
/// scoped storage key.
///
/// The ADR-009 canonical-vector route needs the CLEAN collection name (the shared
/// record/vector catalog registers bare names, resolved per-tenant via `TenantContext`)
/// plus the tenant as a separate value — NOT the folded `{tenant}/{collection}` key.
/// Well-defined because a tenant is a validated path segment (no `/`) and collection
/// names match `^[a-z][a-z0-9_]*$` (no `/`): the first `/` is unambiguously the fold
/// separator, and a bare key (DEFAULT tenant) has none ⇒ `(None, key)`.
pub fn unscope_document_collection(scoped: &str) -> (Option<&str>, &str) {
    match scoped.split_once('/') {
        Some((tenant, collection)) => (Some(tenant), collection),
        None => (None, scoped),
    }
}

/// Document service for CRUD operations
pub struct DocumentService {
    /// Storage engine for persistence (vector-centric, used for legacy flush path)
    storage_engine: Arc<dyn UnifiedStorageFormat>,
    /// Document-native storage engine (CEDAR) for direct document operations.
    /// When present, CRUD operations delegate to this engine instead of
    /// the in-memory HashMap cache. (Phase 2: wire CRUD through this)
    #[allow(dead_code)]
    document_engine: Option<Arc<dyn DocumentStorageEngine>>,
    /// Index manager for path and full-text indexes
    index_manager: Arc<IndexManager>,
    /// Query executor for filter evaluation
    query_executor: Arc<QueryExecutor>,
    /// Collection metadata cache
    collections: Arc<RwLock<HashMap<String, DocumentCollection>>>,
    /// In-memory document store (hot cache, backed by WAL)
    /// Used when document_engine is None (legacy mode)
    documents: Arc<DashMap<String, HashMap<String, DocumentRecord>>>,
    /// Canonical durable record store for the Phase 2 document rebase.
    /// When the `canonical-document-store` feature is enabled and this is
    /// configured, document writes/read-throughs use `ProximaRecord` as the
    /// source of durable truth. The in-memory map and indexes remain hot
    /// compatibility/projection surfaces during migration.
    #[cfg(feature = "canonical-document-store")]
    canonical_record_store: Option<Arc<dyn RecordStorage>>,
    /// WAL writer for durability
    wal_writer: Arc<Mutex<Option<UnifiedWALWriter>>>,
    /// Base path for WAL files
    wal_path: String,
    /// Optional metrics collector for observability
    metrics_collector: Option<Arc<DocumentMetricsCollector>>,
    /// ADR-009 convergence route onto the shared record/vector store. When wired
    /// AND the runtime gate is ON for a collection (default-OFF), document
    /// writes/reads flow through the same tenant-scoped record surface REST v2 uses
    /// — so a document is visible cross-surface, metered, and stored once. When the
    /// gate is OFF the legacy `document_wal`/`documents` path is used unchanged,
    /// and pre-cutover docs remain reachable via the legacy read-fallback
    /// (mixed-read-safe). See `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc`.
    ///
    /// A `OnceLock` because `RecordOpsService` is constructed *after* this service (it takes
    /// the shared `document_service` handle), so the route is injected once, post-construction,
    /// through the already-`Arc`-shared facade — never mutated again.
    record_route: std::sync::OnceLock<Arc<dyn proximadb_runtime::RecordRoutePort>>,
}

/// Bound on the canonical-vector read scan for document point/query reads. The point-get
/// is currently satisfied by a bounded scan (the shared record surface exposes scan, not a
/// labelled point-get); a follow-up can push an `oid`/`local_id` predicate into the scan for
/// O(1) point reads. Documents are a beta surface behind a default-OFF gate, so the bound is
/// generous but finite — a silent truncation past it would drop reads, so it is logged.
const CANONICAL_DOC_SCAN_LIMIT: usize = 100_000;

/// Runtime gate for the canonical-vector document route (ADR-009), **DEFAULT-ON**
/// (post-bake cutover, TD-DOC-CONV-2). The bake proved recall / single-store /
/// restart-recovery, and the write path now meters per-tenant; the default is flipped ON
/// and shipped default-reversible via a global kill-switch.
///
/// `PROXIMADB_DOC_CANONICAL_VECTOR`:
/// * unset / empty / `1` / `true` / `on` / `all` / `*` ⇒ **ON** (default)
/// * `0` / `false` / `off` / `none` ⇒ **OFF** for every collection — the global
///   **kill-switch** (force back to the legacy path, e.g. to roll the cutover back)
/// * a comma-separated list ⇒ ON **only** for the named collections (a partial-rollback
///   scoping: unlisted collections are OFF)
///
/// NB this is only HALF the routing decision: the canonical route additionally requires
/// the collection to actually exist as a canonical/vector collection (see
/// [`DocumentService::canonical_route`]), so a pure-document (non-vector) collection stays
/// on the legacy path even with the gate ON — the flip is mixed-read/write-safe, no
/// flag-day. Legacy pre-cutover docs remain reachable via the read-fallback.
pub fn doc_canonical_vector_enabled(collection: &str) -> bool {
    match std::env::var("PROXIMADB_DOC_CANONICAL_VECTOR") {
        // Unset ⇒ ON (default-ON post-cutover).
        Err(_) => true,
        Ok(raw) => {
            let raw = raw.trim();
            match raw.to_ascii_lowercase().as_str() {
                // Empty ⇒ treat as unset ⇒ ON.
                "" | "1" | "true" | "on" | "all" | "*" => true,
                // Global kill-switch ⇒ OFF for every collection (default-reversible).
                "0" | "false" | "off" | "none" => false,
                // Explicit allowlist ⇒ ON only for the listed collections; an explicit
                // list is a deliberate scoping, so it overrides default-ON for the rest.
                _ => raw.split(',').map(str::trim).any(|c| c == collection),
            }
        }
    }
}

impl DocumentService {
    /// Create a new document service (legacy mode, uses in-memory HashMap)
    pub fn new(storage_engine: Arc<dyn UnifiedStorageFormat>) -> Self {
        Self {
            storage_engine,
            document_engine: None,
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(DashMap::new()),
            #[cfg(feature = "canonical-document-store")]
            canonical_record_store: None,
            wal_writer: Arc::new(Mutex::new(None)),
            wal_path: String::new(),
            metrics_collector: None,
            record_route: std::sync::OnceLock::new(),
        }
    }

    /// Create a document service backed by a DocumentStorageEngine (CEDAR).
    ///
    /// When a document engine is provided, all CRUD operations delegate to it
    /// instead of the in-memory HashMap cache.
    pub fn with_document_engine(
        storage_engine: Arc<dyn UnifiedStorageFormat>,
        document_engine: Arc<dyn DocumentStorageEngine>,
    ) -> Self {
        Self {
            storage_engine,
            document_engine: Some(document_engine),
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(DashMap::new()),
            #[cfg(feature = "canonical-document-store")]
            canonical_record_store: None,
            wal_writer: Arc::new(Mutex::new(None)),
            wal_path: String::new(),
            metrics_collector: None,
            record_route: std::sync::OnceLock::new(),
        }
    }

    /// Create a new document service with metrics collection
    pub fn new_with_metrics(
        storage_engine: Arc<dyn UnifiedStorageFormat>,
        metrics_collector: Arc<DocumentMetricsCollector>,
    ) -> Self {
        Self {
            storage_engine,
            document_engine: None,
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(DashMap::new()),
            #[cfg(feature = "canonical-document-store")]
            canonical_record_store: None,
            wal_writer: Arc::new(Mutex::new(None)),
            wal_path: String::new(),
            metrics_collector: Some(metrics_collector),
            record_route: std::sync::OnceLock::new(),
        }
    }

    /// Create a document service backed by canonical `ProximaRecord` storage.
    ///
    /// This is the Phase 2 migration path from
    /// `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc`.
    /// The service API still accepts/returns legacy document protocol shapes,
    /// but durable state is written/read as `ProximaRecord`.
    #[cfg(feature = "canonical-document-store")]
    pub fn with_canonical_record_store(
        storage_engine: Arc<dyn UnifiedStorageFormat>,
        record_store: Arc<dyn RecordStorage>,
    ) -> Self {
        Self {
            storage_engine,
            document_engine: None,
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(DashMap::new()),
            canonical_record_store: Some(record_store),
            wal_writer: Arc::new(Mutex::new(None)),
            wal_path: String::new(),
            metrics_collector: None,
            record_route: std::sync::OnceLock::new(),
        }
    }

    /// Create a canonical-record-backed document service with WAL recovery.
    ///
    /// Recovered canonical document WAL entries are replayed through the shared
    /// `proximadb_records::replay_record_recovery_operations` hook so the
    /// canonical record store, not the document facade map, owns durable state.
    #[cfg(feature = "canonical-document-store")]
    pub async fn with_canonical_record_store_and_wal(
        storage_engine: Arc<dyn UnifiedStorageFormat>,
        record_store: Arc<dyn RecordStorage>,
        wal_base_path: &str,
    ) -> Result<Self> {
        let wal_path = format!("{}/document_wal", wal_base_path);
        let wal_writer = UnifiedWALWriter::new(wal_path.clone())
            .await
            .context("Failed to create document WAL writer")?;

        let mut service = Self {
            storage_engine,
            document_engine: None,
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(DashMap::new()),
            canonical_record_store: Some(record_store),
            wal_writer: Arc::new(Mutex::new(Some(wal_writer))),
            wal_path,
            metrics_collector: None,
            record_route: std::sync::OnceLock::new(),
        };

        service.recover_from_wal().await?;

        Ok(service)
    }

    /// Create a new document service with WAL support.
    #[deprecated(
        since = "0.2.3",
        note = "legacy document_wal path is fallback-only after the ADR-055 default-ON cutover; use with_canonical_record_store_and_wal. Retirement: TD-DOC-RETIRE-1."
    )]
    pub async fn new_with_wal(
        storage_engine: Arc<dyn UnifiedStorageFormat>,
        wal_base_path: &str,
    ) -> Result<Self> {
        // TD-DOC-RETIRE-1 P2 rewires this to the canonical constructor; the deprecated
        // delegate is intentional until then (the runtime warn fires in the leaf below).
        #[allow(deprecated)]
        Self::new_with_wal_and_metrics(storage_engine, wal_base_path, None).await
    }

    /// Create a new document service with WAL support and optional metrics.
    #[deprecated(
        since = "0.2.3",
        note = "legacy document_wal path is fallback-only after the ADR-055 default-ON cutover; use with_canonical_record_store_and_wal. Retirement: TD-DOC-RETIRE-1."
    )]
    pub async fn new_with_wal_and_metrics(
        storage_engine: Arc<dyn UnifiedStorageFormat>,
        wal_base_path: &str,
        metrics_collector: Option<Arc<DocumentMetricsCollector>>,
    ) -> Result<Self> {
        tracing::warn!(
            "DocumentService::new_with_wal[_and_metrics] is deprecated: the legacy document_wal \
             path is fallback-only after the ADR-055 default-ON cutover. Migrate to \
             with_canonical_record_store_and_wal (retirement tracked by TD-DOC-RETIRE-1)."
        );
        let wal_path = format!("{}/document_wal", wal_base_path);
        let wal_writer = UnifiedWALWriter::new(wal_path.clone())
            .await
            .context("Failed to create document WAL writer")?;

        let mut service = Self {
            storage_engine,
            document_engine: None,
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(DashMap::new()),
            #[cfg(feature = "canonical-document-store")]
            canonical_record_store: None,
            wal_writer: Arc::new(Mutex::new(Some(wal_writer))),
            wal_path,
            metrics_collector,
            record_route: std::sync::OnceLock::new(),
        };

        // Recover from WAL on startup
        service.recover_from_wal().await?;

        Ok(service)
    }

    /// Wire the ADR-009 convergence route onto the shared record/vector store, once.
    ///
    /// Called post-construction (the `RecordOpsService` that backs the route is built after
    /// this service and takes its shared handle), through the already-`Arc`-shared facade.
    /// Idempotent-safe: a second call is ignored. The legacy `document_wal`/`documents` path
    /// is retained for the default-OFF gate and as the mixed-read-safe fallback for
    /// pre-cutover documents.
    pub fn set_record_route(&self, record_route: Arc<dyn proximadb_runtime::RecordRoutePort>) {
        let _ = self.record_route.set(record_route);
    }

    /// Select the canonical-vector route for `collection`, else `None` ⇒ legacy path. The single
    /// decision point for the store-split cutover — every write/read branches on this. Three
    /// conditions, all required (mixed-safe under the DEFAULT-ON gate, TD-DOC-CONV-2):
    ///
    /// 1. a canonical route is wired,
    /// 2. the runtime gate is ON for `collection` (default-ON; kill-switch / allowlist honored),
    /// 3. `collection` actually EXISTS as a canonical/vector collection.
    ///
    /// (3) is what makes default-ON safe: a pure-document (non-vector) collection — one never
    /// created via REST v2 / DDL — is not resolvable canonically, so it stays on the legacy path
    /// instead of hard-failing the canonical write (`insert_records` errors on an unresolvable
    /// collection). Legacy pre-cutover docs stay reachable via the read-fallback.
    async fn canonical_route(
        &self,
        collection: &str,
    ) -> Option<&Arc<dyn proximadb_runtime::RecordRoutePort>> {
        let route = self.record_route.get()?;
        if !doc_canonical_vector_enabled(collection) {
            return None;
        }
        let (tenant, clean_collection) = unscope_document_collection(collection);
        if route.collection_exists(clean_collection, tenant).await {
            Some(route)
        } else {
            None
        }
    }

    /// Build the durable canonical envelope for a document on the ADR-009 vector route.
    ///
    /// Stamps the CLEAN collection into the label prop (the catalog resolves it per-tenant),
    /// a raw-id OID (reconstruction reads the id from `local_id`/props, not the OID), and the
    /// tenant onto the record as structural-isolation defense-in-depth for the scan filter.
    fn canonical_document_record(
        record: &DocumentRecord,
        clean_collection: &str,
        tenant: Option<&str>,
    ) -> proximadb_records::ProximaRecord {
        let mut for_store = record.clone();
        for_store.collection_id = clean_collection.to_string();
        let mut proxima = legacy_document_to_proxima_record(&for_store);
        proxima.oid = record.id.clone();
        proxima.tenant_id = tenant
            .unwrap_or(proximadb_tenant::DEFAULT_TENANT)
            .to_string();
        proxima
    }

    /// Record insert metrics if collector is configured
    async fn record_insert_metrics(&self, start: std::time::Instant, is_error: bool) {
        if let Some(ref collector) = self.metrics_collector {
            let latency_us = start.elapsed().as_micros() as f64;
            collector.record_insert(latency_us, is_error).await;
        }
    }

    /// Record update metrics if collector is configured
    async fn record_update_metrics(&self, start: std::time::Instant, is_error: bool) {
        if let Some(ref collector) = self.metrics_collector {
            let latency_us = start.elapsed().as_micros() as f64;
            collector.record_update(latency_us, is_error).await;
        }
    }

    /// Record delete metrics if collector is configured
    async fn record_delete_metrics(&self, start: std::time::Instant, is_error: bool) {
        if let Some(ref collector) = self.metrics_collector {
            let latency_us = start.elapsed().as_micros() as f64;
            collector.record_delete(latency_us, is_error).await;
        }
    }

    /// Record query metrics if collector is configured
    async fn record_query_metrics(&self, start: std::time::Instant, is_error: bool) {
        if let Some(ref collector) = self.metrics_collector {
            let latency_us = start.elapsed().as_micros() as f64;
            collector.record_query(latency_us, is_error).await;
        }
    }

    /// Recover state from WAL on startup
    async fn recover_from_wal(&mut self) -> Result<()> {
        use crate::storage::persistence::write_ahead_log::wal_operations::UnifiedWALReader;

        info!("Recovering document service from WAL at: {}", self.wal_path);
        let reader = UnifiedWALReader::new(self.wal_path.clone()).await?;
        let entries = reader.read_all().await?;

        let mut recovered_docs = 0;
        let mut recovered_collections = 0;
        #[cfg(feature = "canonical-document-store")]
        let mut canonical_recovery_ops = Vec::new();

        for entry in entries {
            if entry.is_document_operation()
                && let UnifiedWALOperation::DocumentOp(op) = entry.operation
            {
                match op {
                    DocumentOperation::InsertDocument {
                        collection_id,
                        document,
                    } => {
                        // Replay insert
                        let documents = &*self.documents;
                        let mut collection_docs = documents.entry(collection_id).or_default();
                        collection_docs.insert(document.id.clone(), document);
                        recovered_docs += 1;
                    }
                    DocumentOperation::UpsertCanonicalDocumentRecord {
                        collection_id,
                        record,
                    } => {
                        #[cfg(feature = "canonical-document-store")]
                        canonical_recovery_ops
                            .push(RecordRecoveryOperation::Upsert(Box::new(record.clone())));

                        if let Some(document) = proxima_record_to_legacy_document(&record) {
                            let documents = &*self.documents;
                            let mut collection_docs = documents.entry(collection_id).or_default();
                            collection_docs.insert(document.id.clone(), document);
                            recovered_docs += 1;
                        } else {
                            warn!(
                                "Skipping canonical WAL record '{}' because it is not a document",
                                record.oid
                            );
                        }
                    }
                    DocumentOperation::UpdateDocument {
                        collection_id,
                        document_id,
                        new_version,
                        ..
                    } => {
                        // Update version (simplified recovery - full update replay would need stored doc)
                        let documents = &*self.documents;
                        if let Some(mut collection_docs) = documents.get_mut(&collection_id)
                            && let Some(doc) = collection_docs.get_mut(&document_id)
                        {
                            doc.version = new_version;
                        }
                    }
                    DocumentOperation::DeleteDocument {
                        collection_id,
                        document_id,
                    } => {
                        // Replay delete
                        let documents = &*self.documents;
                        if let Some(mut collection_docs) = documents.get_mut(&collection_id) {
                            collection_docs.remove(&document_id);
                        }
                    }
                    DocumentOperation::DeleteCanonicalDocumentRecord {
                        collection_id,
                        document_id,
                        record_oid,
                    } => {
                        #[cfg(feature = "canonical-document-store")]
                        canonical_recovery_ops
                            .push(RecordRecoveryOperation::Delete(RecordKey::new(record_oid)));
                        #[cfg(not(feature = "canonical-document-store"))]
                        let _ = record_oid;

                        let documents = &*self.documents;
                        if let Some(mut collection_docs) = documents.get_mut(&collection_id) {
                            collection_docs.remove(&document_id);
                        }
                    }
                    DocumentOperation::BatchDocuments {
                        collection_id,
                        documents: docs,
                    } => {
                        // Replay batch insert
                        let doc_store = &*self.documents;
                        let mut collection_docs = doc_store.entry(collection_id).or_default();
                        for doc in docs {
                            collection_docs.insert(doc.id.clone(), doc);
                            recovered_docs += 1;
                        }
                    }
                    DocumentOperation::CreateCollection {
                        collection_id,
                        config_json,
                    } => {
                        // Replay collection creation
                        if let Ok(config) =
                            serde_json::from_str::<DocumentCollectionConfig>(&config_json)
                        {
                            let collection = DocumentCollection::new(collection_id.clone(), config);
                            let mut collections = self.collections.write().await;
                            collections.insert(collection_id, collection);
                            recovered_collections += 1;
                        }
                    }
                    DocumentOperation::DeleteCollection { collection_id } => {
                        // Replay collection deletion
                        let mut collections = self.collections.write().await;
                        collections.remove(&collection_id);
                        let documents = &*self.documents;
                        documents.remove(&collection_id);
                    }
                }
            }
        }

        #[cfg(feature = "canonical-document-store")]
        if let Some(record_store) = &self.canonical_record_store {
            let summary =
                replay_record_recovery_operations(record_store.as_ref(), canonical_recovery_ops)
                    .await
                    .context("Failed to replay canonical document WAL into record store")?;

            if summary.upserts_replayed > 0 || summary.deletes_replayed > 0 {
                info!(
                    "Canonical document WAL recovery complete: {} upserts, {} deletes replayed into record store",
                    summary.upserts_replayed, summary.deletes_replayed
                );
            }
        } else if !canonical_recovery_ops.is_empty() {
            warn!(
                "Recovered {} canonical document WAL operations without a canonical record store",
                canonical_recovery_ops.len()
            );
        }

        info!(
            "WAL recovery complete: {} documents, {} collections recovered",
            recovered_docs, recovered_collections
        );
        Ok(())
    }

    /// Write operation to WAL (if enabled)
    async fn write_to_wal(&self, operation: DocumentOperation) -> Result<()> {
        let mut wal_guard = self.wal_writer.lock().await;
        if let Some(ref mut writer) = *wal_guard {
            let wal_op = UnifiedWALOperation::DocumentOp(operation);
            writer.append(wal_op).await?;
        }
        Ok(())
    }

    /// Write a document upsert to WAL using canonical record intent when the
    /// canonical store is active, otherwise use the legacy document entry.
    async fn write_document_upsert_to_wal(
        &self,
        collection: &str,
        record: &DocumentRecord,
    ) -> Result<()> {
        #[cfg(feature = "canonical-document-store")]
        if self.canonical_record_store.is_some() {
            return self
                .write_to_wal(DocumentOperation::UpsertCanonicalDocumentRecord {
                    collection_id: collection.to_string(),
                    record: legacy_document_to_proxima_record(record),
                })
                .await;
        }

        self.write_to_wal(DocumentOperation::InsertDocument {
            collection_id: collection.to_string(),
            document: record.clone(),
        })
        .await
    }

    /// Write a document delete to WAL using canonical record identity when the
    /// canonical store is active, otherwise use the legacy document entry.
    async fn write_document_delete_to_wal(&self, collection: &str, id: &str) -> Result<()> {
        #[cfg(feature = "canonical-document-store")]
        if self.canonical_record_store.is_some() {
            let record_oid = DocumentRecordKey::new(collection, id).canonical_oid();
            return self
                .write_to_wal(DocumentOperation::DeleteCanonicalDocumentRecord {
                    collection_id: collection.to_string(),
                    document_id: id.to_string(),
                    record_oid,
                })
                .await;
        }

        self.write_to_wal(DocumentOperation::DeleteDocument {
            collection_id: collection.to_string(),
            document_id: id.to_string(),
        })
        .await
    }

    /// Apply insert projection maintenance using canonical record-shaped input
    /// when the canonical store is active.
    async fn index_document_projection(
        &self,
        collection: &str,
        record: &DocumentRecord,
    ) -> Result<()> {
        #[cfg(feature = "canonical-document-store")]
        if self.canonical_record_store.is_some() {
            self.index_manager
                .index_record_projection(&legacy_document_to_proxima_record(record))
                .await?;
            return Ok(());
        }

        self.index_manager.index_document(collection, record).await
    }

    /// Refresh projection maintenance using canonical record-shaped input when
    /// the canonical store is active.
    async fn reindex_document_projection(
        &self,
        collection: &str,
        record: &DocumentRecord,
    ) -> Result<()> {
        #[cfg(feature = "canonical-document-store")]
        if self.canonical_record_store.is_some() {
            self.index_manager
                .reindex_record_projection(&legacy_document_to_proxima_record(record))
                .await?;
            return Ok(());
        }

        self.index_manager
            .reindex_document(collection, record)
            .await
    }

    /// Remove projection entries for a document facade id.
    async fn remove_document_projection(&self, collection: &str, id: &str) -> Result<()> {
        #[cfg(feature = "canonical-document-store")]
        if self.canonical_record_store.is_some() {
            let record_oid = DocumentRecordKey::new(collection, id).canonical_oid();
            self.index_manager
                .remove_record_projection(collection, &record_oid)
                .await?;
            return Ok(());
        }

        self.index_manager.remove_document(collection, id).await
    }

    /// Flush WAL to disk
    pub async fn flush_wal(&self) -> Result<()> {
        let mut wal_guard = self.wal_writer.lock().await;
        if let Some(ref mut writer) = *wal_guard {
            writer.flush().await?;
        }
        Ok(())
    }

    /// Flush documents from a collection to persistent storage (SST engine)
    ///
    /// This method converts in-memory documents to ProximaRecords and flushes them
    /// to the underlying storage engine for cold tier persistence.
    ///
    /// Documents are stored with:
    /// - id: original document ID prefixed with collection (e.g., "mycollection::doc1")
    /// - vector: [0.0] placeholder (documents don't have inherent vectors)
    /// - metadata: "_document" contains serialized JSON, "_collection" for routing
    pub async fn flush_to_storage(&self, collection: &str) -> Result<FlushToStorageResult> {
        use crate::storage::traits::FlushParameters;

        info!(
            "Flushing documents from collection '{}' to storage engine",
            collection
        );
        let start = std::time::Instant::now();

        // Get documents for this collection
        let docs_to_flush: Vec<DocumentRecord> = {
            let documents = &*self.documents;
            match documents.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => {
                    warn!(
                        "No documents found in collection '{}' for flush",
                        collection
                    );
                    return Ok(FlushToStorageResult {
                        documents_flushed: 0,
                        bytes_written: 0,
                        duration_ms: start.elapsed().as_millis() as u64,
                        success: true,
                    });
                }
            }
        };

        if docs_to_flush.is_empty() {
            return Ok(FlushToStorageResult {
                documents_flushed: 0,
                bytes_written: 0,
                duration_ms: start.elapsed().as_millis() as u64,
                success: true,
            });
        }

        let vector_records: Vec<proximadb_records::ProximaRecord> = docs_to_flush
            .iter()
            .filter_map(|doc| self.document_to_proxima_record(doc, collection))
            .collect();

        let record_count = vector_records.len();
        let estimated_size: usize = vector_records
            .iter()
            .map(|r| r.oid.len() + 4 + r.props.len() * 50)
            .sum();

        // Build flush parameters
        let params = FlushParameters {
            collection_id: Some(format!("_documents_{}", collection)),
            force: true,
            synchronous: true,
            vector_records,
            trigger_compaction: false,
            estimated_size,
            ..Default::default()
        };

        // Flush to storage engine
        let result = self.storage_engine.flush(params).await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        info!(
            "Flushed {} documents from '{}' to storage in {}ms",
            record_count, collection, duration_ms
        );

        Ok(FlushToStorageResult {
            documents_flushed: record_count,
            bytes_written: result.bytes_written.unwrap_or(0) as usize,
            duration_ms,
            success: result.success,
        })
    }

    /// Convert a DocumentRecord to ProximaRecord for storage engine persistence.
    fn document_to_proxima_record(
        &self,
        doc: &DocumentRecord,
        collection: &str,
    ) -> Option<proximadb_records::ProximaRecord> {
        let doc_json = match serde_json::to_string(&proxima_tree_to_sql_object(&doc.props)) {
            Ok(json) => json,
            Err(e) => {
                warn!("Failed to serialize document {}: {}", doc.id, e);
                return None;
            }
        };

        let mut props = proximadb_records::ProximaTree::new();
        props.insert(
            "_type".to_string(),
            proximadb_records::ProximaTreeNode::Value(proximadb_data_model::ProximaValue::String(
                "document".to_string(),
            )),
        );
        props.insert(
            "_collection".to_string(),
            proximadb_records::ProximaTreeNode::Value(proximadb_data_model::ProximaValue::String(
                collection.to_string(),
            )),
        );
        props.insert(
            "_document".to_string(),
            proximadb_records::ProximaTreeNode::Value(proximadb_data_model::ProximaValue::String(
                doc_json,
            )),
        );
        props.insert(
            "_version".to_string(),
            proximadb_records::ProximaTreeNode::Value(proximadb_data_model::ProximaValue::Int64(
                doc.version as i64,
            )),
        );

        Some(proximadb_records::ProximaRecord {
            oid: format!("{}::{}", collection, doc.id),
            props,
            created_at_ns: doc.updated_at_ns,
            updated_at_ns: doc.updated_at_ns,
            record_version: doc.version,
            ..Default::default()
        })
    }

    /// Read documents from storage engine (cold tier)
    ///
    /// This is used for documents that have been flushed to storage
    /// but evicted from the in-memory hot cache.
    ///
    /// The implementation follows SOLID principles:
    /// - Uses DocumentMetadataFilterBuilder to construct the search filter (SRP)
    /// - Uses ColdTierRetriever trait for storage access (DIP)
    /// - Can be extended for different storage backends (OCP)
    ///
    /// Documents are stored as ProximaRecords with properties:
    /// - `_type`: "document"
    /// - `_collection`: collection name
    /// - `_document`: serialized JSON content
    /// - `_version`: document version
    #[allow(dead_code)]
    pub async fn read_from_storage(
        &self,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<DocumentRecord>> {
        use crate::storage::document::storage::cold_tier::{
            ColdTierRetriever, StorageEngineColdTierRetriever,
        };

        if ids.is_empty() {
            return Ok(Vec::new());
        }

        debug!(
            "Cold tier: Reading {} documents from collection '{}'",
            ids.len(),
            collection
        );

        // Create the cold tier retriever using the storage engine
        let retriever = StorageEngineColdTierRetriever::new(self.storage_engine.clone());

        // Retrieve documents from cold storage
        match retriever.retrieve_documents(collection, ids).await {
            Ok(documents) => {
                debug!(
                    "Cold tier: Successfully retrieved {} of {} requested documents",
                    documents.len(),
                    ids.len()
                );
                Ok(documents)
            }
            Err(e) => {
                warn!(
                    "Cold tier: Failed to retrieve documents from collection '{}': {}",
                    collection, e
                );
                // Return empty result on error to allow graceful degradation
                // The caller can fall back to other retrieval mechanisms
                Ok(Vec::new())
            }
        }
    }

    /// Flush all collections to storage
    pub async fn flush_all_to_storage(&self) -> Result<FlushToStorageResult> {
        info!("Flushing all document collections to storage");
        let start = std::time::Instant::now();

        let collection_names: Vec<String> = {
            let collections = self.collections.read().await;
            collections.keys().cloned().collect()
        };

        let mut total_docs = 0;
        let mut total_bytes = 0;

        for collection_name in collection_names {
            let result = self.flush_to_storage(&collection_name).await?;
            total_docs += result.documents_flushed;
            total_bytes += result.bytes_written;
        }

        Ok(FlushToStorageResult {
            documents_flushed: total_docs,
            bytes_written: total_bytes,
            duration_ms: start.elapsed().as_millis() as u64,
            success: true,
        })
    }

    /// Create a new document collection
    pub async fn create_collection(
        &self,
        name: &str,
        config: DocumentCollectionConfig,
    ) -> Result<String> {
        info!("Creating document collection: {}", name);

        // Check if collection already exists
        {
            let collections = self.collections.read().await;
            if collections.contains_key(name) {
                return Err(anyhow!("Collection '{}' already exists", name));
            }
        }

        // Write to WAL first (durability before in-memory update)
        let config_json =
            serde_json::to_string(&config).context("Failed to serialize collection config")?;
        self.write_to_wal(DocumentOperation::CreateCollection {
            collection_id: name.to_string(),
            config_json,
        })
        .await?;

        // Create collection metadata
        let collection = DocumentCollection::new(name.to_string(), config.clone());

        // Initialize indexes
        for index_def in &config.indexes {
            self.index_manager
                .create_index(name, index_def)
                .await
                .context("Failed to create index")?;
        }

        // Store collection metadata
        {
            let mut collections = self.collections.write().await;
            collections.insert(name.to_string(), collection);
        }

        // P-Provision (ADR-055): also provision the canonical (record/vector) collection so this
        // document collection CONVERGES on the shared store instead of the legacy path — even for
        // pure-document (vectorless) use. Uses the CLEAN name (the catalog registers bare names,
        // resolved per-tenant); dimension 0 = vectorless (documents that carry vectors are inserted
        // through the metered /documents path). Best-effort + mixed-safe: on failure we warn and
        // keep the legacy path (the canonical route's capability check just stays false).
        // `ensure_or_create_collection` funnels through here too, so first-write auto-create is
        // covered without a second hook.
        if let Some(route) = self.record_route.get() {
            let (tenant, clean_collection) = unscope_document_collection(name);
            // P-Shred follow-up (ADR-055): seed props-auto-promotion from the collection's declared
            // indexes so those hot fields shred into typed columns at flush. Only simple top-level
            // SCALAR keys shred usefully (a nested `$.user.email` top-level is a Map, not a scalar);
            // skip nested/array paths rather than promoting a column that would never populate.
            let promote_keys: Vec<String> = config
                .indexes
                .iter()
                .filter_map(|idx| {
                    let key = idx.path.trim_start_matches('$').trim_start_matches('.');
                    if key.is_empty() || key.contains('.') || key.contains('[') {
                        None
                    } else {
                        Some(key.to_string())
                    }
                })
                .collect();
            if let Err(e) = route
                .ensure_collection(clean_collection, 0, tenant, &promote_keys)
                .await
            {
                warn!(
                    "P-Provision: canonical collection provision for '{}' failed (staying on legacy path): {}",
                    name, e
                );
            }
        }

        info!("Created document collection: {}", name);
        Ok(name.to_string())
    }

    /// Get collection metadata
    pub async fn get_collection(&self, name: &str) -> Result<Option<DocumentCollection>> {
        let collections = self.collections.read().await;
        Ok(collections.get(name).cloned())
    }

    /// Get the collection metadata for `name`, lazily provisioning it with defaults if absent —
    /// the get-or-create counterpart of [`get_collection`].
    ///
    /// This lets a tenant-scoped first write auto-register its collection under the SCOPED key
    /// (`{tenant}/{collection}`): the v2 document gRPC path scopes reads/writes, but there is no
    /// scoped create path (pgwire/REST-v1 create bare), so without this the scoped collection has
    /// no metadata and the existence gate rejects the insert (TD-DOC-TENANT-1). Provisioning goes
    /// through [`create_collection`] so the `CreateCollection` WAL op is logged and the collection
    /// metadata is rebuilt on restart (recovery replays collections only from that op). Idempotent
    /// and race-safe: a lost create race is resolved by re-fetch.
    pub async fn ensure_or_create_collection(&self, name: &str) -> Result<DocumentCollection> {
        if let Some(existing) = self.get_collection(name).await? {
            return Ok(existing);
        }
        let config = DocumentCollectionConfig {
            name: name.to_string(),
            ..Default::default()
        };
        // On success OR a lost create race, re-fetch the now-registered metadata.
        let _ = self.create_collection(name, config).await;
        self.get_collection(name)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' missing after provision", name))
    }

    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<DocumentCollection>> {
        let collections = self.collections.read().await;
        Ok(collections.values().cloned().collect())
    }

    /// Delete a collection
    pub async fn delete_collection(&self, name: &str) -> Result<bool> {
        info!("Deleting document collection: {}", name);

        // Check if collection exists first
        let exists = {
            let collections = self.collections.read().await;
            collections.contains_key(name)
        };

        if !exists {
            return Ok(false);
        }

        // Write to WAL first (durability before in-memory update)
        self.write_to_wal(DocumentOperation::DeleteCollection {
            collection_id: name.to_string(),
        })
        .await?;

        // Remove from cache
        {
            let mut collections = self.collections.write().await;
            collections.remove(name);
        }

        // Drop indexes
        self.index_manager.drop_collection_indexes(name).await?;

        // Remove documents
        {
            let documents = &*self.documents;
            documents.remove(name);
        }

        Ok(true)
    }

    /// Insert a single document
    pub async fn insert_document(
        &self,
        collection: &str,
        id: Option<&str>,
        document: SqlObject,
    ) -> Result<DocumentRecord> {
        // The legacy v1 `SqlObject` is converted into the neutral, ProximaTree-
        // native `DocumentRecord` here at the wire edge; all durability flows
        // through the `DocumentRecord`-native path below.
        let doc_id = id.map_or_else(|| uuid::Uuid::new_v4().to_string(), |s| s.to_string());
        let record = DocumentRecord::new(doc_id, document, collection.to_string());
        self.insert_document_record(collection, record).await
    }

    /// Insert a pre-built [`DocumentRecord`] — the ProximaTree-native (v2) write
    /// path that never touches the legacy v1 `SqlObject`. Same durability
    /// (WAL → canonical record store → indexes → hot cache → stats) as
    /// [`insert_document`], which now converts `SqlObject` → `DocumentRecord` at
    /// the wire edge and delegates here. The record's `id` is used verbatim
    /// (callers generate/normalize it).
    pub async fn insert_document_record(
        &self,
        collection: &str,
        record: DocumentRecord,
    ) -> Result<DocumentRecord> {
        let start = std::time::Instant::now();
        let doc_id = record.id.clone();
        debug!(
            "Inserting document {} into collection: {}",
            doc_id, collection
        );

        // Get — or lazily provision — the collection (get-or-create). A tenant-scoped first write
        // auto-registers its collection under the scoped key, so named-tenant document create→use
        // works without an out-of-band scoped create (TD-DOC-TENANT-1). The default tenant's
        // collections are bare and already exist, so they are found, not recreated.
        let _collection_meta = match self.ensure_or_create_collection(collection).await {
            Ok(meta) => meta,
            Err(e) => {
                self.record_insert_metrics(start, true).await;
                return Err(e);
            }
        };

        // ADR-009 canonical-vector route (gate ON, per-collection opt-in): persist to the
        // shared record/vector store — the exact path REST v2 uses — so the document is
        // visible cross-surface, metered, and stored once. The legacy `document_wal`/DashMap
        // is skipped on this path (the vector WAL owns recovery); pre-cutover docs stay
        // reachable via the read-fallback in `get_document`/`query_documents`.
        if let Some(route) = self.canonical_route(collection).await {
            let (tenant, clean_collection) = unscope_document_collection(collection);
            let proxima = Self::canonical_document_record(&record, clean_collection, tenant);
            match route
                .insert_records(clean_collection, vec![proxima], tenant)
                .await
            {
                Ok(_) => {
                    debug!(
                        "Inserted document {} into {} via canonical-vector route",
                        doc_id, clean_collection
                    );
                    self.record_insert_metrics(start, false).await;
                    return Ok(record);
                }
                Err(e) => {
                    self.record_insert_metrics(start, true).await;
                    return Err(e.context("canonical-vector document insert failed"));
                }
            }
        }

        // Write to WAL first (durability before in-memory update)
        if let Err(e) = self.write_document_upsert_to_wal(collection, &record).await {
            self.record_insert_metrics(start, true).await;
            return Err(e);
        }

        // Phase 2 migration path: when configured, canonical records are the
        // durable truth and legacy maps/indexes are compatibility projections.
        #[cfg(feature = "canonical-document-store")]
        let record = if let Some(record_store) = &self.canonical_record_store {
            let stored = record_store
                .upsert_record(legacy_document_to_proxima_record(&record))
                .await
                .context("Failed to upsert canonical document record")?;

            proxima_record_to_legacy_document(&stored).ok_or_else(|| {
                anyhow!(
                    "Canonical document record '{}' could not be rebuilt",
                    stored.oid
                )
            })?
        } else {
            record
        };

        // Update indexes
        if let Err(e) = self.index_document_projection(collection, &record).await {
            self.record_insert_metrics(start, true).await;
            return Err(e);
        }

        // Store document in memory (backed by WAL for durability)
        {
            let documents = &*self.documents;
            let mut collection_docs = documents.entry(collection.to_string()).or_default();
            collection_docs.insert(doc_id.clone(), record.clone());
        }

        // Update collection stats
        {
            let mut collections = self.collections.write().await;
            if let Some(coll) = collections.get_mut(collection) {
                coll.document_count += 1;
                coll.updated_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
            }
        }

        debug!("Inserted document {} into {}", doc_id, collection);
        self.record_insert_metrics(start, false).await;
        Ok(record)
    }

    /// Insert multiple documents in batch
    pub async fn insert_documents(
        &self,
        collection: &str,
        documents: Vec<(Option<String>, SqlObject)>,
    ) -> Result<DocumentIngestResult> {
        let start = std::time::Instant::now();
        let mut result = DocumentIngestResult::default();

        for (id, doc) in documents {
            match self.insert_document(collection, id.as_deref(), doc).await {
                Ok(_) => result.ingested += 1,
                Err(e) => {
                    result.failed += 1;
                    result.errors.push(e.to_string());
                }
            }
        }

        result.processing_time_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Get a document by ID
    pub async fn get_document(
        &self,
        collection: &str,
        id: &str,
        projection: Option<Vec<String>>,
    ) -> Result<Option<DocumentRecord>> {
        debug!("Getting document {} from {}", id, collection);

        // ADR-009 canonical-vector route: the shared vector store is authoritative. Point-get it
        // by id (O(log n) bloom + B+ tree — TD-DOC-CONV-1, no full-collection scan), rebuild the
        // doc, return on hit. On miss, fall back to the legacy in-memory map for pre-cutover docs
        // (mixed-read-safe). Note: no `get_collection` existence gate here — a canonical
        // collection may live only in the record/vector catalog (e.g. created + written via
        // REST v2), never in this facade's own map.
        if let Some(route) = self.canonical_route(collection).await {
            let (tenant, clean_collection) = unscope_document_collection(collection);
            if let Some(mut doc) = route
                .get_record(clean_collection, id, tenant)
                .await
                .context("canonical-vector document get failed")?
                .as_ref()
                .and_then(proxima_record_to_legacy_document)
            {
                if let Some(fields) = projection.as_ref().filter(|f| !f.is_empty()) {
                    doc.props = self.apply_projection(&doc.props, fields);
                }
                return Ok(Some(doc));
            }
            return Ok(self.legacy_dashmap_get(collection, id, projection));
        }

        // Verify collection exists
        self.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Phase 2 migration path: canonical record store is authoritative when
        // supplied. The legacy in-memory map remains the fallback path.
        #[cfg(feature = "canonical-document-store")]
        if let Some(record_store) = &self.canonical_record_store {
            let key = DocumentRecordKey::new(collection, id);
            let record = record_store
                .get_record(&RecordKey::new(key.canonical_oid()))
                .await
                .context("Failed to get canonical document record")?;

            if let Some(record) = record {
                let mut result = proxima_record_to_legacy_document(&record).ok_or_else(|| {
                    anyhow!(
                        "Canonical document record '{}' could not be rebuilt",
                        record.oid
                    )
                })?;

                if let Some(fields) = projection
                    && !fields.is_empty()
                {
                    result.props = self.apply_projection(&result.props, &fields);
                }

                return Ok(Some(result));
            }

            return Ok(None);
        }

        // Retrieve from in-memory store
        let documents = &*self.documents;
        if let Some(collection_docs) = documents.get(collection)
            && let Some(record) = collection_docs.get(id)
        {
            let mut result = record.clone();

            // Apply projection if specified
            if let Some(fields) = projection
                && !fields.is_empty()
            {
                result.props = self.apply_projection(&result.props, &fields);
            }

            return Ok(Some(result));
        }

        Ok(None)
    }

    /// Legacy in-memory (DashMap) point lookup — the mixed-read-safe fallback for
    /// pre-cutover documents still keyed under the scoped `collection`. Used only from
    /// the canonical-vector read path after a store miss; the gate-OFF path keeps its
    /// own inline lookup unchanged.
    fn legacy_dashmap_get(
        &self,
        collection: &str,
        id: &str,
        projection: Option<Vec<String>>,
    ) -> Option<DocumentRecord> {
        let documents = &*self.documents;
        let collection_docs = documents.get(collection)?;
        let mut result = collection_docs.get(id)?.clone();
        if let Some(fields) = projection.as_ref().filter(|f| !f.is_empty()) {
            result.props = self.apply_projection(&result.props, fields);
        }
        Some(result)
    }

    /// Apply field projection over the canonical props tree (TD-106 Slice 7e).
    ///
    /// Nested scalar paths (`a.b`) project under their last segment; top-level
    /// object/array fields are copied whole.
    fn apply_projection(&self, props: &ProximaTree, fields: &[String]) -> ProximaTree {
        let mut projected = ProximaTree::new();

        for field in fields {
            let key = field.split('.').next_back().unwrap_or(field);
            if let Some(value) = tree_get(props, field.trim_start_matches("$.")) {
                projected.insert(key.to_string(), ProximaTreeNode::Value(value.clone()));
            } else if let Some(node) = props.get(field) {
                // Top-level object/array (or a field whose path traverses an object).
                projected.insert(field.clone(), node.clone());
            }
        }

        projected
    }

    /// Update a document by ID
    pub async fn update_document(
        &self,
        collection: &str,
        id: &str,
        updates: Vec<DocumentUpdate>,
        expected_version: Option<u64>,
    ) -> Result<DocumentRecord> {
        let start = std::time::Instant::now();
        debug!("Updating document {} in {}", id, collection);

        // Get existing document
        let mut record = match self
            .get_document(collection, id, None)
            .await?
            .ok_or_else(|| anyhow!("Document '{}' not found", id))
        {
            Ok(r) => r,
            Err(e) => {
                self.record_update_metrics(start, true).await;
                return Err(e);
            }
        };

        // Check version for optimistic locking
        if let Some(expected) = expected_version
            && record.version != expected
        {
            self.record_update_metrics(start, true).await;
            return Err(anyhow!(
                "Version mismatch: expected {}, got {}",
                expected,
                record.version
            ));
        }

        // Apply updates onto the canonical props tree (TD-106 Slice 7d).
        for update in &updates {
            if let Err(e) = self.apply_update(&mut record.props, update) {
                self.record_update_metrics(start, true).await;
                return Err(e);
            }
        }

        // Increment version
        let new_version = record.version + 1;
        record.version = new_version;
        record.updated_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        // ADR-009 canonical-vector route: write the updated document back to the shared
        // vector store (upsert on the raw-id OID) so the update is visible cross-surface.
        if let Some(route) = self.canonical_route(collection).await {
            let (tenant, clean_collection) = unscope_document_collection(collection);
            let proxima = Self::canonical_document_record(&record, clean_collection, tenant);
            match route
                .insert_records(clean_collection, vec![proxima], tenant)
                .await
            {
                Ok(_) => {
                    debug!(
                        "Updated document {} in {} via canonical-vector route",
                        id, clean_collection
                    );
                    self.record_update_metrics(start, false).await;
                    return Ok(record);
                }
                Err(e) => {
                    self.record_update_metrics(start, true).await;
                    return Err(e.context("canonical-vector document update failed"));
                }
            }
        }

        // Write to WAL first (durability before in-memory update)
        // Store full updated document for proper recovery replay
        if let Err(e) = self.write_document_upsert_to_wal(collection, &record).await {
            self.record_update_metrics(start, true).await;
            return Err(e);
        }

        // Phase 2 migration path: when configured, persist the updated
        // document as canonical durable state before refreshing projections.
        #[cfg(feature = "canonical-document-store")]
        if let Some(record_store) = &self.canonical_record_store {
            let stored = record_store
                .upsert_record(legacy_document_to_proxima_record(&record))
                .await
                .context("Failed to upsert updated canonical document record")?;

            record = proxima_record_to_legacy_document(&stored).ok_or_else(|| {
                anyhow!(
                    "Canonical document record '{}' could not be rebuilt",
                    stored.oid
                )
            })?;
        }

        // Update indexes
        if let Err(e) = self.reindex_document_projection(collection, &record).await {
            self.record_update_metrics(start, true).await;
            return Err(e);
        }

        // Persist updated document to in-memory store
        {
            let documents = &*self.documents;
            if let Some(mut collection_docs) = documents.get_mut(collection) {
                collection_docs.insert(id.to_string(), record.clone());
            }
        }

        debug!("Updated document {} in {}", id, collection);
        self.record_update_metrics(start, false).await;
        Ok(record)
    }

    /// Apply a single update operation to the canonical props tree.
    ///
    /// TD-106 Slice 7d: navigation/mutation is `ProximaTree`-native; the proto
    /// `update.value` (SqlValue wire operand) is lifted to canonical at this
    /// boundary (`$set` keeps object structure via `sql_value_to_tree_node`;
    /// array/scalar ops lift via `sql_value_to_proxima`).
    fn apply_update(&self, props: &mut ProximaTree, update: &DocumentUpdate) -> Result<()> {
        use crate::proto::proximadb_v1::sql_value::Value as SqlVal;
        let path = &update.path;
        let value = update.value.as_ref();

        match UpdateOperation::try_from(update.operation).unwrap_or(UpdateOperation::Unspecified) {
            UpdateOperation::Set => {
                if let Some(v) = value {
                    self.set_path_value(props, path, sql_value_to_tree_node(v))?;
                }
            }
            UpdateOperation::Unset => {
                self.unset_path(props, path)?;
            }
            UpdateOperation::Inc => {
                if let Some(v) = value {
                    let inc = match &v.value {
                        Some(SqlVal::Int64Value(i)) => *i as f64,
                        Some(SqlVal::NumberValue(f)) => *f,
                        _ => return Err(anyhow!("Increment value must be numeric")),
                    };
                    self.increment_path(props, path, inc)?;
                }
            }
            UpdateOperation::Push => {
                if let Some(v) = value {
                    self.push_to_array(props, path, sql_value_to_proxima(v))?;
                }
            }
            UpdateOperation::Pull => {
                if let Some(v) = value {
                    self.pull_from_array(props, path, &sql_value_to_proxima(v))?;
                }
            }
            UpdateOperation::AddToSet => {
                if let Some(v) = value {
                    self.add_to_set(props, path, sql_value_to_proxima(v))?;
                }
            }
            UpdateOperation::Rename => {
                if let Some(v) = value {
                    // Value should be the new field name.
                    let new_name = match &v.value {
                        Some(SqlVal::StringValue(s)) => s.clone(),
                        _ => return Err(anyhow!("New name must be a string")),
                    };
                    self.rename_path(props, path, &new_name)?;
                }
            }
            UpdateOperation::Unspecified => {
                return Err(anyhow!("Unspecified update operation"));
            }
        }

        Ok(())
    }

    // Path manipulation helpers (canonical ProximaTree, TD-106 Slice 7d)

    /// Split `$.a.b.c` into segments; errors on an empty path.
    fn path_segments(path: &str) -> Result<Vec<&str>> {
        let parts: Vec<&str> = path.trim_start_matches("$.").split('.').collect();
        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return Err(anyhow!("Empty path"));
        }
        Ok(parts)
    }

    /// Set a node at a dotted path, creating intermediate objects as needed.
    fn set_path_value(
        &self,
        doc: &mut ProximaTree,
        path: &str,
        node: ProximaTreeNode,
    ) -> Result<()> {
        let parts = Self::path_segments(path)?;
        let (last, parents) = parts
            .split_last()
            .ok_or_else(|| anyhow!("path must have at least one segment"))?;

        let mut current = doc;
        for part in parents {
            let entry = current
                .entry(part.to_string())
                .or_insert_with(|| ProximaTreeNode::Object(ProximaTree::new()));
            match entry {
                ProximaTreeNode::Object(sub) => current = sub,
                _ => return Err(anyhow!("Path {} is not an object", part)),
            }
        }
        current.insert(last.to_string(), node);
        Ok(())
    }

    /// Remove a field at a dotted path.
    fn unset_path(&self, doc: &mut ProximaTree, path: &str) -> Result<()> {
        let parts = Self::path_segments(path)?;
        let (last, parents) = parts
            .split_last()
            .ok_or_else(|| anyhow!("path must have at least one segment"))?;

        let mut current = doc;
        for part in parents {
            match current.get_mut(*part) {
                Some(ProximaTreeNode::Object(sub)) => current = sub,
                Some(_) => return Err(anyhow!("Path {} is not an object", part)),
                None => return Ok(()), // path doesn't exist, nothing to unset
            }
        }
        current.remove(*last);
        Ok(())
    }

    /// Increment a numeric leaf at a dotted path (creating it if absent).
    fn increment_path(&self, doc: &mut ProximaTree, path: &str, inc: f64) -> Result<()> {
        let parts = Self::path_segments(path)?;
        let (last, parents) = parts
            .split_last()
            .ok_or_else(|| anyhow!("path must have at least one segment"))?;

        let mut current = doc;
        for part in parents {
            match current.get_mut(*part) {
                Some(ProximaTreeNode::Object(sub)) => current = sub,
                Some(_) => return Err(anyhow!("Path {} is not an object", part)),
                None => return Err(anyhow!("Path {} does not exist", part)),
            }
        }
        match current.get_mut(*last) {
            Some(ProximaTreeNode::Value(ProximaValue::Int64(v))) => *v += inc as i64,
            Some(ProximaTreeNode::Value(ProximaValue::Float64(v))) => *v += inc,
            Some(_) => return Err(anyhow!("Field at path {} is not numeric", path)),
            None => {
                current.insert(
                    last.to_string(),
                    ProximaTreeNode::Value(ProximaValue::Float64(inc)),
                );
            }
        }
        Ok(())
    }

    /// Push a value onto an array leaf at a dotted path (creating it if absent).
    fn push_to_array(&self, doc: &mut ProximaTree, path: &str, value: ProximaValue) -> Result<()> {
        let parts = Self::path_segments(path)?;
        let (last, parents) = parts
            .split_last()
            .ok_or_else(|| anyhow!("path must have at least one segment"))?;

        let mut current = doc;
        for part in parents {
            match current.get_mut(*part) {
                Some(ProximaTreeNode::Object(sub)) => current = sub,
                Some(_) => return Err(anyhow!("Path {} is not an object", part)),
                None => return Err(anyhow!("Path {} does not exist", part)),
            }
        }
        match current.get_mut(*last) {
            Some(ProximaTreeNode::Value(ProximaValue::Array(arr))) => arr.push(value),
            Some(_) => return Err(anyhow!("Field at path {} is not an array", path)),
            None => {
                current.insert(
                    last.to_string(),
                    ProximaTreeNode::Value(ProximaValue::Array(vec![value])),
                );
            }
        }
        Ok(())
    }

    /// Pull (remove) matching values from an array leaf at a dotted path.
    fn pull_from_array(
        &self,
        doc: &mut ProximaTree,
        path: &str,
        value: &ProximaValue,
    ) -> Result<()> {
        let parts = Self::path_segments(path)?;
        let (last, parents) = parts
            .split_last()
            .ok_or_else(|| anyhow!("path must have at least one segment"))?;

        let mut current = doc;
        for part in parents {
            match current.get_mut(*part) {
                Some(ProximaTreeNode::Object(sub)) => current = sub,
                Some(_) => return Err(anyhow!("Path {} is not an object", part)),
                None => return Ok(()), // path doesn't exist
            }
        }
        match current.get_mut(*last) {
            Some(ProximaTreeNode::Value(ProximaValue::Array(arr))) => arr.retain(|v| v != value),
            Some(_) => return Err(anyhow!("Field at path {} is not an array", path)),
            None => {} // field doesn't exist
        }
        Ok(())
    }

    /// Add a value to a set (array with unique values) at a dotted path.
    fn add_to_set(&self, doc: &mut ProximaTree, path: &str, value: ProximaValue) -> Result<()> {
        let parts = Self::path_segments(path)?;
        let (last, parents) = parts
            .split_last()
            .ok_or_else(|| anyhow!("path must have at least one segment"))?;

        let mut current = doc;
        for part in parents {
            match current.get_mut(*part) {
                Some(ProximaTreeNode::Object(sub)) => current = sub,
                Some(_) => return Err(anyhow!("Path {} is not an object", part)),
                None => return Err(anyhow!("Path {} does not exist", part)),
            }
        }
        match current.get_mut(*last) {
            Some(ProximaTreeNode::Value(ProximaValue::Array(arr))) => {
                if !arr.contains(&value) {
                    arr.push(value);
                }
            }
            Some(_) => return Err(anyhow!("Field at path {} is not an array", path)),
            None => {
                current.insert(
                    last.to_string(),
                    ProximaTreeNode::Value(ProximaValue::Array(vec![value])),
                );
            }
        }
        Ok(())
    }

    /// Rename a field at a dotted path. `new_name` is the new (leaf) field name.
    fn rename_path(&self, doc: &mut ProximaTree, old_path: &str, new_name: &str) -> Result<()> {
        let parts = Self::path_segments(old_path)?;
        let (last, parents) = parts
            .split_last()
            .ok_or_else(|| anyhow!("path must have at least one segment"))?;

        let mut current = doc;
        for part in parents {
            match current.get_mut(*part) {
                Some(ProximaTreeNode::Object(sub)) => current = sub,
                Some(_) => return Err(anyhow!("Path {} is not an object", part)),
                None => return Ok(()), // path doesn't exist
            }
        }
        if let Some(node) = current.remove(*last) {
            current.insert(new_name.to_string(), node);
        }
        Ok(())
    }

    /// Delete a document by ID
    pub async fn delete_document(&self, collection: &str, id: &str) -> Result<bool> {
        let start = std::time::Instant::now();
        debug!("Deleting document {} from {}", id, collection);

        // ADR-009 canonical-vector route: tombstone in the shared vector store (visible
        // cross-surface), and also drop any pre-cutover legacy copy under the scoped key so
        // it cannot resurface (mixed-read-safe). Returns true if either store held the doc.
        if let Some(route) = self.canonical_route(collection).await {
            let (tenant, clean_collection) = unscope_document_collection(collection);
            let deleted = route
                .delete_records(clean_collection, vec![id.to_string()], tenant)
                .await
                .context("canonical-vector document delete failed")?;
            let legacy_present = self
                .documents
                .get(collection)
                .is_some_and(|docs| docs.contains_key(id));
            if legacy_present {
                if let Err(e) = self.write_document_delete_to_wal(collection, id).await {
                    self.record_delete_metrics(start, true).await;
                    return Err(e);
                }
                let _ = self.remove_document_projection(collection, id).await;
                if let Some(mut docs) = self.documents.get_mut(collection) {
                    docs.remove(id);
                }
            }
            self.record_delete_metrics(start, false).await;
            return Ok(deleted > 0 || legacy_present);
        }

        // Verify collection exists
        if let Err(e) = self
            .get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))
        {
            self.record_delete_metrics(start, true).await;
            return Err(e);
        }

        // Check if document exists before WAL write. In canonical mode the
        // RecordStore is authoritative; otherwise use the legacy hot map.
        #[cfg(feature = "canonical-document-store")]
        let canonical_key = RecordKey::new(DocumentRecordKey::new(collection, id).canonical_oid());

        let exists = {
            #[cfg(feature = "canonical-document-store")]
            if let Some(record_store) = &self.canonical_record_store {
                record_store
                    .get_record(&canonical_key)
                    .await
                    .context("Failed to check canonical document record existence")?
                    .is_some()
            } else {
                let documents = &*self.documents;
                documents
                    .get(collection)
                    .is_some_and(|docs| docs.contains_key(id))
            }

            #[cfg(not(feature = "canonical-document-store"))]
            {
                let documents = &*self.documents;
                documents
                    .get(collection)
                    .is_some_and(|docs| docs.contains_key(id))
            }
        };

        if !exists {
            self.record_delete_metrics(start, false).await;
            return Ok(false);
        }

        // Write to WAL first (durability before in-memory update)
        if let Err(e) = self.write_document_delete_to_wal(collection, id).await {
            self.record_delete_metrics(start, true).await;
            return Err(e);
        }

        #[cfg(feature = "canonical-document-store")]
        if let Some(record_store) = &self.canonical_record_store {
            record_store
                .delete_record(&canonical_key)
                .await
                .context("Failed to delete canonical document record")?;
        }

        // Remove from indexes
        if let Err(e) = self.remove_document_projection(collection, id).await {
            self.record_delete_metrics(start, true).await;
            return Err(e);
        }

        // Remove from in-memory store
        {
            let documents = &*self.documents;
            if let Some(mut collection_docs) = documents.get_mut(collection) {
                collection_docs.remove(id);
            }
        }

        // Update collection stats
        {
            let mut collections = self.collections.write().await;
            if let Some(coll) = collections.get_mut(collection) {
                if coll.document_count > 0 {
                    coll.document_count -= 1;
                }
                coll.updated_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
            }
        }

        debug!("Deleted document {} from {}", id, collection);
        self.record_delete_metrics(start, false).await;
        Ok(true)
    }

    /// Query documents with filter, sort, and pagination
    pub async fn query_documents(
        &self,
        collection: &str,
        params: DocumentQueryParams,
    ) -> Result<DocumentQueryResult> {
        let start = std::time::Instant::now();
        debug!("Querying documents in {}", collection);

        // ADR-009 canonical-vector route: source documents from the shared vector store
        // (cross-surface visibility), then run the same in-memory query executor. A queried
        // collection is either opted-in (canonical) or not; pre-cutover point reads stay
        // served by `get_document`'s legacy fallback.
        if let Some(route) = self.canonical_route(collection).await {
            let (tenant, clean_collection) = unscope_document_collection(collection);
            let records = route
                .scan_records(clean_collection, CANONICAL_DOC_SCAN_LIMIT, tenant)
                .await
                .context("canonical-vector document scan failed")?;
            if records.len() == CANONICAL_DOC_SCAN_LIMIT {
                warn!(
                    "canonical-vector query scan hit the {} cap for collection {}; results may be incomplete",
                    CANONICAL_DOC_SCAN_LIMIT, clean_collection
                );
            }
            let documents: Vec<DocumentRecord> = records
                .iter()
                .filter_map(proxima_record_to_legacy_document)
                .collect();
            let (documents, total_count) = match self
                .query_executor
                .execute(collection, &documents, &params, &self.index_manager)
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    self.record_query_metrics(start, true).await;
                    return Err(e);
                }
            };
            let query_time_ms = start.elapsed().as_millis() as u64;
            self.record_query_metrics(start, false).await;
            return Ok(DocumentQueryResult {
                documents,
                total_count: if params.include_count {
                    Some(total_count)
                } else {
                    None
                },
                query_time_ms,
            });
        }

        // Verify collection exists
        let _collection_meta = match self
            .get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))
        {
            Ok(meta) => meta,
            Err(e) => {
                self.record_query_metrics(start, true).await;
                return Err(e);
            }
        };

        // Execute query
        #[cfg(feature = "canonical-document-store")]
        let documents: Vec<DocumentRecord> =
            if let Some(record_store) = &self.canonical_record_store {
                record_store
                    .scan_records_with_options(
                        RecordScanOptions::unbounded()
                            .with_required_label(DOCUMENT_RECORD_LABEL)
                            .with_property(
                                DOCUMENT_COLLECTION_PROP,
                                proximadb_data_model::ProximaValue::String(collection.to_string()),
                            ),
                    )
                    .await
                    .context("Failed to scan canonical document records")?
                    .iter()
                    .filter_map(proxima_record_to_legacy_document)
                    .collect()
            } else {
                let docs = &*self.documents;
                match docs.get(collection) {
                    Some(collection_docs) => collection_docs.values().cloned().collect(),
                    None => Vec::new(),
                }
            };

        #[cfg(not(feature = "canonical-document-store"))]
        let documents: Vec<DocumentRecord> = {
            let docs = &*self.documents;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        let (documents, total_count) = match self
            .query_executor
            .execute(collection, &documents, &params, &self.index_manager)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                self.record_query_metrics(start, true).await;
                return Err(e);
            }
        };

        let query_time_ms = start.elapsed().as_millis() as u64;
        self.record_query_metrics(start, false).await;

        Ok(DocumentQueryResult {
            documents,
            total_count: if params.include_count {
                Some(total_count)
            } else {
                None
            },
            query_time_ms,
        })
    }

    /// Aggregate documents with a pipeline
    ///
    /// Executes a MongoDB-style aggregation pipeline on documents in a collection.
    /// Supports stages: match, group, project, sort, limit, skip, unwind.
    ///
    /// # Arguments
    /// * `collection` - The collection name
    /// * `filter` - Optional filter to apply before the pipeline
    /// * `pipeline` - The aggregation pipeline stages
    ///
    /// # Returns
    /// A vector of aggregated result documents
    pub async fn aggregate_documents(
        &self,
        collection: &str,
        filter: Option<crate::proto::proximadb_v1::DocumentFilter>,
        pipeline: Vec<crate::proto::proximadb_v1::AggregationStage>,
    ) -> Result<crate::storage::document::AggregateResult> {
        use crate::storage::document::aggregation::AggregationExecutor;

        let start = std::time::Instant::now();
        debug!("Aggregating documents in {}", collection);

        // Get all documents from the collection — the shared vector store when routed
        // (ADR-009 cross-surface visibility), else the legacy in-memory map.
        let documents: Vec<DocumentRecord> = if let Some(route) =
            self.canonical_route(collection).await
        {
            let (tenant, clean_collection) = unscope_document_collection(collection);
            let records = route
                .scan_records(clean_collection, CANONICAL_DOC_SCAN_LIMIT, tenant)
                .await
                .context("canonical-vector document scan failed")?;
            if records.len() == CANONICAL_DOC_SCAN_LIMIT {
                warn!(
                    "canonical-vector aggregate scan hit the {} cap for collection {}; results may be incomplete",
                    CANONICAL_DOC_SCAN_LIMIT, clean_collection
                );
            }
            records
                .iter()
                .filter_map(proxima_record_to_legacy_document)
                .collect()
        } else {
            // Verify collection exists (legacy path only)
            self.get_collection(collection)
                .await?
                .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;
            let docs = &*self.documents;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        // Execute the aggregation pipeline
        let executor = AggregationExecutor::new();
        let results = executor.execute(documents, filter.as_ref(), &pipeline)?;

        let query_time_ms = start.elapsed().as_millis() as u64;
        debug!(
            "Aggregation complete: {} results in {}ms",
            results.len(),
            query_time_ms
        );

        Ok(crate::storage::document::AggregateResult {
            results,
            query_time_ms,
        })
    }

    /// Aggregate documents with $lookup support
    ///
    /// Extended aggregation that supports cross-collection joins via the $lookup stage.
    /// For each lookup stage in the pipeline, this method fetches matching documents
    /// from the foreign collection and merges them into the local documents.
    ///
    /// # Arguments
    /// * `collection` - Source collection name
    /// * `filter` - Optional filter to apply before aggregation
    /// * `pipeline` - Aggregation pipeline stages (may include $lookup)
    ///
    /// # Returns
    /// Aggregated results with lookup fields populated
    ///
    /// # Note
    /// For lookup stages, this method requires Arc<DocumentService>. Use the async variant
    /// that takes Arc<DocumentService> for full lookup support.
    pub async fn aggregate_documents_with_lookup(
        &self,
        collection: &str,
        filter: Option<DocumentFilter>,
        pipeline: Vec<crate::proto::proximadb_v1::AggregationStage>,
    ) -> Result<crate::storage::document::AggregateResult> {
        use crate::storage::document::aggregation::AggregationExecutor;

        let start = std::time::Instant::now();
        debug!(
            "Aggregating documents in {} with lookup support",
            collection
        );

        // Verify source collection exists
        self.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Get all documents from the source collection
        let documents: Vec<DocumentRecord> = {
            let docs = &*self.documents;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        // Check if pipeline contains lookup stages
        let has_lookup = pipeline.iter().any(|stage| {
            matches!(
                &stage.stage,
                Some(crate::proto::proximadb_v1::aggregation_stage::Stage::Lookup(_))
            )
        });

        if has_lookup {
            // For lookup stages, caller should use aggregate_documents_with_lookup_arc
            return Err(anyhow!(
                "Pipeline contains $lookup stages - use aggregate_documents_with_lookup_arc which takes Arc<DocumentService>"
            ));
        }

        // Execute the aggregation pipeline (without lookup support)
        let executor = AggregationExecutor::new();
        let results = executor.execute(documents, filter.as_ref(), &pipeline)?;

        let query_time_ms = start.elapsed().as_millis() as u64;
        debug!(
            "Aggregation complete: {} results in {}ms",
            results.len(),
            query_time_ms
        );

        Ok(crate::storage::document::AggregateResult {
            results,
            query_time_ms,
        })
    }

    /// Aggregate documents with $lookup support (Arc variant)
    ///
    /// This is the preferred method for aggregation pipelines that include $lookup stages.
    /// It takes Arc<DocumentService> which allows the lookup fetcher to query foreign collections.
    ///
    /// # Arguments
    /// * `collection` - Source collection name
    /// * `filter` - Optional filter to apply before aggregation
    /// * `pipeline` - Aggregation pipeline stages (may include $lookup)
    ///
    /// # Returns
    /// Aggregated results with lookup fields populated
    pub async fn aggregate_documents_with_lookup_arc(
        this: Arc<DocumentService>,
        collection: &str,
        filter: Option<DocumentFilter>,
        pipeline: Vec<crate::proto::proximadb_v1::AggregationStage>,
    ) -> Result<crate::storage::document::AggregateResult> {
        use crate::storage::document::aggregation::AggregationExecutor;

        let start = std::time::Instant::now();
        debug!(
            "Aggregating documents in {} with lookup support (Arc variant)",
            collection
        );

        // Verify source collection exists
        this.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Get all documents from the source collection
        let documents: Vec<DocumentRecord> = {
            let docs = &*this.documents;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        // Create a lookup fetcher that can query foreign collections
        let fetcher = DocumentServiceLookupFetcher {
            service: this.clone(),
        };

        // Execute aggregation pipeline with lookup support. The working set is the
        // canonical `ProximaTree` (TD-106 Slice 5); Slice 6 reads the record's
        // canonical `props` tree directly at the input edge.
        let executor = AggregationExecutor::new();
        let mut working_set: Vec<ProximaTree> = if let Some(f) = &filter {
            documents
                .iter()
                .filter(|doc| executor.matches_filter(doc, f))
                .map(|doc| doc.props.clone())
                .collect()
        } else {
            documents.into_iter().map(|doc| doc.props).collect()
        };

        // Process each stage, handling lookups specially
        for (stage_idx, stage) in pipeline.iter().enumerate() {
            use crate::proto::proximadb_v1::aggregation_stage::Stage;

            match &stage.stage {
                Some(Stage::Lookup(lookup_stage)) => {
                    working_set = executor.process_lookup(&working_set, lookup_stage, &fetcher)?;
                    debug!(
                        "After lookup stage {}: {} documents",
                        stage_idx,
                        working_set.len()
                    );
                }
                Some(_) => {
                    // Use the standard process_stage for non-lookup stages
                    working_set = executor.process_stage(&working_set, stage, stage_idx)?;
                    debug!("After stage {}: {} documents", stage_idx, working_set.len());
                }
                None => {
                    return Err(anyhow!("Empty stage at index {}", stage_idx));
                }
            }
        }

        let query_time_ms = start.elapsed().as_millis() as u64;
        debug!(
            "Aggregation with lookup complete: {} results in {}ms",
            working_set.len(),
            query_time_ms
        );

        // Output edge: lower the canonical working set back to the proto row shape
        // for the public `AggregateResult` contract.
        let results: Vec<SqlObject> = working_set.iter().map(proxima_tree_to_sql_object).collect();

        Ok(crate::storage::document::AggregateResult {
            results,
            query_time_ms,
        })
    }

    /// Calculate full-text search scores for documents matching a query
    ///
    /// This provides basic TF-IDF-like scoring for text matching.
    /// For production use, integrate with Tantivy index.
    ///
    /// # Arguments
    /// * `collection` - The collection name
    /// * `query_terms` - Terms to search for
    /// * `text_paths` - Document paths to search in (e.g., ["title", "body"])
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    /// Documents sorted by relevance score (highest first)
    pub async fn fulltext_search(
        &self,
        collection: &str,
        query_terms: Vec<String>,
        text_paths: Vec<String>,
        limit: usize,
    ) -> Result<Vec<(DocumentRecord, f32)>> {
        use crate::storage::document::aggregation::AggregationExecutor;

        debug!(
            "Full-text search in {} for terms: {:?}",
            collection, query_terms
        );

        // Verify collection exists
        self.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Get all documents from the collection
        let documents: Vec<DocumentRecord> = {
            let docs = &*self.documents;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        // Calculate scores
        let executor = AggregationExecutor::new();
        let mut scored = executor.calculate_fulltext_scores(&documents, &query_terms, &text_paths);

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Apply limit
        scored.truncate(limit);

        Ok(scored)
    }
}

// =============================================================================
// TRAIT IMPLEMENTATION: DocumentStorageOperations
// =============================================================================
// This implements the SOLID-compliant trait interface for document operations,
// bridging the existing DocumentService to the new multi-model storage traits.

use crate::storage::traits::{
    DocumentCollectionInfo as TraitDocCollectionInfo, DocumentRecord as TraitDocRecord,
    DocumentStorageOperations,
};
use async_trait::async_trait;
use proximadb_document_query::{
    DocumentQueryResult as ContractDocumentQueryResult, DocumentQueryService,
    DocumentSearchRequest, DocumentSearchResult, SortDirection,
};
use proximadb_kernel::error::ProximaDBError;

/// Convert internal DocumentRecord to trait DocumentRecord
fn to_trait_doc_record(doc: &DocumentRecord) -> TraitDocRecord {
    TraitDocRecord {
        id: doc.id.clone(),
        document: proxima_tree_to_sql_object(&doc.props),
        version: doc.version,
        // Use updated_at_ns for both since internal type doesn't have created_at_ns
        created_at_ns: doc.updated_at_ns,
        updated_at_ns: doc.updated_at_ns,
    }
}

#[async_trait]
impl DocumentStorageOperations for DocumentService {
    async fn insert_document(
        &self,
        collection: &str,
        id: &str,
        document: SqlObject,
        indexed_paths: Vec<String>,
    ) -> Result<TraitDocRecord> {
        // Use existing insert_document method (with impl block method name)
        let doc_record = DocumentService::insert_document(
            self,
            collection,
            Some(id), // Already &str, no conversion needed
            document,
        )
        .await?;

        // Note: index_document indexes all configured paths for the document
        // The indexed_paths parameter specifies which paths to index from config
        // The actual indexing is done during insert_document call above
        // We log if additional path-specific indexing is requested but not yet supported
        if !indexed_paths.is_empty() {
            debug!(
                "Additional indexed_paths specified: {:?} (already indexed via collection config)",
                indexed_paths
            );
        }

        Ok(to_trait_doc_record(&doc_record))
    }

    async fn get_document(&self, collection: &str, id: &str) -> Result<Option<TraitDocRecord>> {
        // Use existing get_document method with projection=None
        match DocumentService::get_document(self, collection, id, None).await? {
            Some(doc) => Ok(Some(to_trait_doc_record(&doc))),
            None => Ok(None),
        }
    }

    async fn query_documents(
        &self,
        collection: &str,
        filter: Option<DocumentFilter>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<TraitDocRecord>> {
        let params = DocumentQueryParams {
            filter,
            projection: Vec::new(),
            sort: Vec::new(),
            limit: limit as u32,
            offset: offset as u32,
            include_count: false,
        };

        let result = DocumentService::query_documents(self, collection, params).await?;

        Ok(result.documents.iter().map(to_trait_doc_record).collect())
    }

    async fn update_document(
        &self,
        collection: &str,
        id: &str,
        updates: Vec<DocumentUpdate>,
    ) -> Result<TraitDocRecord> {
        let doc = DocumentService::update_document(
            self, collection, id, updates, None, // expected_version
        )
        .await?;

        Ok(to_trait_doc_record(&doc))
    }

    async fn delete_document(&self, collection: &str, id: &str) -> Result<bool> {
        DocumentService::delete_document(self, collection, id).await
    }

    async fn create_document_collection(&self, config: DocumentCollectionConfig) -> Result<String> {
        // Internal create_collection takes (name, config) - clone name first to avoid borrow
        let name = config.name.clone();
        self.create_collection(&name, config).await
    }

    async fn list_document_collections(&self) -> Result<Vec<TraitDocCollectionInfo>> {
        let collections = self.list_collections().await?;

        Ok(collections
            .into_iter()
            .map(|coll| TraitDocCollectionInfo {
                name: coll.name,
                document_count: coll.document_count,
                storage_size_bytes: coll.storage_size_bytes,
                indexes: coll.indexes,
            })
            .collect())
    }
}

#[async_trait]
impl DocumentQueryService for DocumentService {
    async fn document_search(
        &self,
        request: DocumentSearchRequest,
    ) -> ContractDocumentQueryResult<DocumentSearchResult> {
        if request.filter.is_some() {
            return Err(ProximaDBError::Query(
                proximadb_kernel::error::QueryError::InvalidFilter(
                    "string document filters are not yet parsed by DocumentQueryService adapter"
                        .to_string(),
                ),
            ));
        }

        let sort = request
            .sort
            .map(|sort| crate::proto::proximadb_v1::SortField {
                path: sort.field,
                order: match sort.direction {
                    SortDirection::Ascending => crate::proto::proximadb_v1::SortOrder::Asc as i32,
                    SortDirection::Descending => crate::proto::proximadb_v1::SortOrder::Desc as i32,
                },
            })
            .into_iter()
            .collect();

        let params = DocumentQueryParams {
            filter: None,
            projection: request.projection.unwrap_or_default(),
            sort,
            limit: request.limit as u32,
            offset: request.offset as u32,
            include_count: true,
        };

        let query_result = DocumentService::query_documents(self, &request.collection_id, params)
            .await
            .map_err(|error| ProximaDBError::Internal(error.to_string()))?;

        let total_count = query_result
            .total_count
            .unwrap_or(query_result.documents.len() as u64) as usize;

        Ok(DocumentSearchResult {
            results: query_result
                .documents
                .iter()
                .map(|document| document.to_proto_result(None))
                .collect(),
            total_count,
            execution_time_ms: query_result.query_time_ms,
        })
    }

    async fn get_document(
        &self,
        collection_id: String,
        document_id: String,
    ) -> ContractDocumentQueryResult<Option<proximadb_document_query::DocumentRecord>> {
        DocumentService::get_document(self, &collection_id, &document_id, None)
            .await
            .map(|document| document.map(|document| document.to_proto_result(None)))
            .map_err(|error| ProximaDBError::Internal(error.to_string()))
    }
}

// =============================================================================
// LOOKUP FETCHER FOR AGGREGATION PIPELINE
// =============================================================================

/// Implementation of LookupFetcher that queries the DocumentService
pub struct DocumentServiceLookupFetcher {
    pub service: Arc<DocumentService>,
}

impl LookupFetcher for DocumentServiceLookupFetcher {
    /// Fetch documents from a collection where a field matches a value
    fn fetch_matching(
        &self,
        collection: &str,
        field_path: &str,
        match_value: &SqlValue,
    ) -> Result<Vec<SqlObject>> {
        // Use blocking API to query documents from another collection
        let rt = tokio::runtime::Handle::try_current()
            .map_err(|e| anyhow!("Failed to get runtime handle: {}", e))?;

        rt.block_on(async {
            // Create a filter for the field match
            let filter = DocumentFilter {
                conditions: vec![create_field_eq_filter(field_path, match_value.clone())],
                ..Default::default()
            };

            // Query documents with the filter
            let params = DocumentQueryParams {
                filter: Some(filter),
                limit: 1000, // Reasonable limit for lookup results
                ..Default::default()
            };

            let result = self.service.query_documents(collection, params).await?;

            Ok(result
                .documents
                .into_iter()
                .map(|doc| proxima_tree_to_sql_object(&doc.props))
                .collect())
        })
    }
}

/// Helper to create an equality filter for a field
fn create_field_eq_filter(
    field_path: &str,
    value: SqlValue,
) -> crate::proto::proximadb_v1::DocFilterCondition {
    use crate::proto::proximadb_v1::DocFilterOperator;

    crate::proto::proximadb_v1::DocFilterCondition {
        path: field_path.to_string(),
        operator: DocFilterOperator::Eq as i32,
        value: Some(value),
        values: Vec::new(), // Empty for equality operator
    }
}

// =============================================================================
// DOCUMENTPORT IMPL — ADR-015 (port impl lives on bare concrete service)
// =============================================================================
//
// Bare-service implementation of `proximadb_runtime::DocumentPort`. Each
// method takes the proto request shape directly, performs the same proto-
// conversion + bare-service-call logic that was previously in
// `impl DocumentService for DocumentServiceImpl` (the gRPC tonic wrapper),
// and returns the proto response wrapped in `anyhow::Result`.
//
// This impl coexists with the existing `impl DocumentPort for DocumentServiceImpl`
// at `src/network/grpc/document_service.rs:284` during the migration window.
// Subsequent commits will remove the wrapper's port impl + shrink the tonic
// `impl DocumentService for DocumentServiceImpl` methods to one-liners that
// delegate to this port impl. See ADR-015 for the full rationale.

#[async_trait::async_trait]
impl proximadb_runtime::DocumentPort for DocumentService {
    async fn create_collection(
        &self,
        request: crate::proto::proximadb_v1::CreateDocumentCollectionRequest,
    ) -> Result<crate::proto::proximadb_v1::CreateDocumentCollectionResponse> {
        let config = request.config.ok_or_else(|| anyhow!("Missing config"))?;
        let name = config.name.clone();
        let id = self
            .create_collection(&name, config)
            .await
            .map_err(|e| anyhow!("Failed to create collection: {}", e))?;
        Ok(
            crate::proto::proximadb_v1::CreateDocumentCollectionResponse {
                collection_id: id,
                success: true,
            },
        )
    }

    async fn list_collections(
        &self,
        _request: crate::proto::proximadb_v1::ListDocumentCollectionsRequest,
    ) -> Result<crate::proto::proximadb_v1::ListDocumentCollectionsResponse> {
        let collections = self
            .list_collections()
            .await
            .map_err(|e| anyhow!("Failed to list collections: {}", e))?;
        let infos: Vec<crate::proto::proximadb_v1::DocumentCollectionInfo> = collections
            .iter()
            .map(|c| crate::proto::proximadb_v1::DocumentCollectionInfo {
                name: c.name.clone(),
                document_count: c.document_count,
                storage_size_bytes: c.storage_size_bytes,
                indexes: c.indexes.clone(),
            })
            .collect();
        Ok(crate::proto::proximadb_v1::ListDocumentCollectionsResponse { collections: infos })
    }

    async fn delete_collection(
        &self,
        request: crate::proto::proximadb_v1::DeleteDocumentCollectionRequest,
    ) -> Result<crate::proto::proximadb_v1::DeleteDocumentCollectionResponse> {
        self.delete_collection(&request.collection)
            .await
            .map_err(|e| anyhow!("Failed to delete collection: {}", e))?;
        Ok(crate::proto::proximadb_v1::DeleteDocumentCollectionResponse { success: true })
    }

    async fn insert_document(
        &self,
        request: crate::proto::proximadb_v1::InsertDocumentRequest,
    ) -> Result<crate::proto::proximadb_v1::InsertDocumentResponse> {
        let document = request
            .document
            .ok_or_else(|| anyhow!("Missing document"))?;
        let id = request.id.as_deref();
        let record = self
            .insert_document(&request.collection, id, document)
            .await
            .map_err(|e| anyhow!("Failed to insert document: {}", e))?;
        Ok(crate::proto::proximadb_v1::InsertDocumentResponse {
            id: record.id,
            version: record.version,
        })
    }

    async fn get_document(
        &self,
        request: crate::proto::proximadb_v1::GetDocumentRequest,
    ) -> Result<crate::proto::proximadb_v1::GetDocumentResponse> {
        let projection = if request.projection.is_empty() {
            None
        } else {
            Some(request.projection)
        };
        match self
            .get_document(&request.collection, &request.id, projection)
            .await
            .map_err(|e| anyhow!("Failed to get document: {}", e))?
        {
            Some(record) => Ok(crate::proto::proximadb_v1::GetDocumentResponse {
                document: Some(proxima_tree_to_sql_object(&record.props)),
                version: record.version,
                found: true,
            }),
            None => Ok(crate::proto::proximadb_v1::GetDocumentResponse {
                document: None,
                version: 0,
                found: false,
            }),
        }
    }

    async fn update_document(
        &self,
        request: crate::proto::proximadb_v1::UpdateDocumentRequest,
    ) -> Result<crate::proto::proximadb_v1::UpdateDocumentResponse> {
        let record = self
            .update_document(
                &request.collection,
                &request.id,
                request.updates,
                request.expected_version,
            )
            .await
            .map_err(|e| anyhow!("Failed to update document: {}", e))?;
        Ok(crate::proto::proximadb_v1::UpdateDocumentResponse {
            new_version: record.version,
            success: true,
        })
    }

    async fn delete_document(
        &self,
        request: crate::proto::proximadb_v1::DeleteDocumentRequest,
    ) -> Result<crate::proto::proximadb_v1::DeleteDocumentResponse> {
        let deleted = self
            .delete_document(&request.collection, &request.id)
            .await
            .map_err(|e| anyhow!("Failed to delete document: {}", e))?;
        Ok(crate::proto::proximadb_v1::DeleteDocumentResponse { deleted })
    }

    async fn query_documents(
        &self,
        request: crate::proto::proximadb_v1::QueryDocumentsRequest,
    ) -> Result<crate::proto::proximadb_v1::QueryDocumentsResponse> {
        let params = DocumentQueryParams {
            filter: request.filter,
            projection: request.projection,
            sort: request.sort,
            limit: request.limit,
            offset: request.offset,
            include_count: request.include_count,
        };
        let result = self
            .query_documents(&request.collection, params)
            .await
            .map_err(|e| anyhow!("Failed to query documents: {}", e))?;
        let documents: Vec<crate::proto::proximadb_v1::DocumentResult> = result
            .documents
            .into_iter()
            .map(|d| crate::proto::proximadb_v1::DocumentResult {
                id: d.id,
                document: Some(proxima_tree_to_sql_object(&d.props)),
                version: d.version,
                score: None,
            })
            .collect();
        Ok(crate::proto::proximadb_v1::QueryDocumentsResponse {
            documents,
            total_count: result.total_count,
            query_time_ms: result.query_time_ms,
        })
    }

    async fn aggregate_documents(
        &self,
        request: crate::proto::proximadb_v1::AggregateDocumentsRequest,
    ) -> Result<crate::proto::proximadb_v1::AggregateDocumentsResponse> {
        let result = self
            .aggregate_documents(&request.collection, request.filter, request.pipeline)
            .await
            .map_err(|e| anyhow!("Failed to aggregate documents: {}", e))?;
        Ok(crate::proto::proximadb_v1::AggregateDocumentsResponse {
            results: result.results,
            query_time_ms: result.query_time_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The convergence tests call `RecordRoutePort` methods (collection_exists / ensure_collection)
    // directly on the mock, so the trait must be in scope (the impls use the fully-qualified path).
    use crate::proto::proximadb_v1::{
        DocFilterCondition, DocFilterOperator, DocIndexType, DocumentCollectionConfig,
        DocumentFilter, DocumentUpdate, IndexDefinition, SqlObject, SqlValue, UpdateOperation,
        sql_value,
    };
    use crate::storage::traits::{
        CompactionParameters, CompactionResult, FlushParameters, FlushResult,
        StorageFormatStrategy, UnifiedStorageFormat,
    };
    use async_trait::async_trait;
    use proximadb_runtime::RecordRoutePort;
    use std::collections::HashMap;
    use std::sync::Arc;

    // =========================================================================
    // Mock storage engine for document service tests
    // =========================================================================

    struct MockStorageEngine;

    #[async_trait]
    impl UnifiedStorageFormat for MockStorageEngine {
        fn engine_name(&self) -> &'static str {
            "MockEngine"
        }

        fn engine_version(&self) -> &'static str {
            "1.0.0"
        }

        fn strategy(&self) -> StorageFormatStrategy {
            StorageFormatStrategy::Sst
        }

        async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
            Ok(FlushResult {
                success: true,
                collections_affected: Vec::new(),
                entries_flushed: Some(0),
                bytes_written: Some(0),
                files_created: Some(0),
                file_paths: Vec::new(),
                duration_ms: Some(0),
                completed_at: chrono::Utc::now(),
                engine_metrics: HashMap::new(),
                compaction_triggered: false,
                compaction_error: None,
                flushed_batch_ids: Vec::new(),
            })
        }

        async fn do_compact(&self, _params: &CompactionParameters) -> Result<CompactionResult> {
            Ok(CompactionResult {
                success: true,
                collections_affected: Vec::new(),
                entries_processed: Some(0),
                entries_removed: Some(0),
                bytes_read: Some(0),
                bytes_written: Some(0),
                input_files: Some(0),
                output_files: Some(0),
                duration_ms: Some(0),
                completed_at: chrono::Utc::now(),
                engine_metrics: HashMap::new(),
            })
        }

        async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
            Ok(HashMap::new())
        }

        async fn vector_by_id(
            &self,
            _collection_id: &str,
            _base_path: &str,
            _vector_id: &str,
        ) -> Result<Option<proximadb_records::ProximaRecord>> {
            Ok(None)
        }

        async fn search_vectors_unified(
            &self,
            _ctx: &crate::storage::traits::StorageQueryContext,
        ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
            Ok(Vec::new())
        }

        fn get_filesystem_factory(
            &self,
        ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
            unimplemented!("MockEngine does not provide a filesystem factory")
        }
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    /// Create a DocumentService backed by the mock storage engine (no WAL)
    fn create_test_service() -> DocumentService {
        let engine: Arc<dyn UnifiedStorageFormat> = Arc::new(MockStorageEngine);
        DocumentService::new(engine)
    }

    /// Build an SqlObject from key-value pairs (string values)
    fn make_document(fields: Vec<(&str, SqlValue)>) -> SqlObject {
        SqlObject {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    /// Convenience: create a string SqlValue
    fn sql_string(s: &str) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::StringValue(s.to_string())),
        }
    }

    /// Convenience: create an i64 SqlValue
    fn sql_int(n: i64) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::Int64Value(n)),
        }
    }

    /// Convenience: create a numeric (f64) SqlValue
    #[allow(dead_code)]
    fn sql_number(n: f64) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::NumberValue(n)),
        }
    }

    /// Create a default collection config for testing
    fn test_collection_config() -> DocumentCollectionConfig {
        DocumentCollectionConfig {
            name: "test_collection".to_string(),
            ..Default::default()
        }
    }

    /// Set up a service with a pre-created collection, ready for document operations
    async fn service_with_collection(collection_name: &str) -> DocumentService {
        let svc = create_test_service();
        svc.create_collection(
            collection_name,
            DocumentCollectionConfig {
                name: collection_name.to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("collection creation should succeed");
        svc
    }

    // =========================================================================
    // ADR-009 canonical-vector route (document store convergence)
    // =========================================================================

    /// In-memory stand-in for the shared record/vector store: proves a document routed onto
    /// the canonical-vector path lands in — and is served from — the SAME store the REST v2
    /// record surface uses (cross-surface visibility), without a full network server. Keyed
    /// by (clean collection, raw-id oid).
    #[derive(Default)]
    struct MockRecordRoute {
        store: std::sync::Mutex<HashMap<String, HashMap<String, proximadb_records::ProximaRecord>>>,
        /// Collections the mock should report as NON-canonical (so `canonical_route` sends them
        /// to the legacy path). Empty ⇒ every collection is treated as an existing canonical
        /// collection — the common case for the convergence tests.
        non_canonical: std::sync::Mutex<std::collections::HashSet<String>>,
        /// Per-collection promote keys captured from `ensure_collection` (P-Shred follow-up):
        /// lets a test assert the document facade forwarded the declared index fields.
        promoted: std::sync::Mutex<HashMap<String, Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl proximadb_runtime::RecordRoutePort for MockRecordRoute {
        async fn insert_records(
            &self,
            collection_id: &str,
            records: Vec<proximadb_records::ProximaRecord>,
            _tenant: Option<&str>,
        ) -> anyhow::Result<usize> {
            let mut store = self.store.lock().expect("mock route lock");
            let coll = store.entry(collection_id.to_string()).or_default();
            let n = records.len();
            for r in records {
                coll.insert(r.oid.clone(), r);
            }
            Ok(n)
        }

        async fn get_record(
            &self,
            collection_id: &str,
            record_id: &str,
            _tenant: Option<&str>,
        ) -> anyhow::Result<Option<proximadb_records::ProximaRecord>> {
            let store = self.store.lock().expect("mock route lock");
            Ok(store
                .get(collection_id)
                .and_then(|c| c.get(record_id).cloned()))
        }

        async fn scan_records(
            &self,
            collection_id: &str,
            limit: usize,
            _tenant: Option<&str>,
        ) -> anyhow::Result<Vec<proximadb_records::ProximaRecord>> {
            let store = self.store.lock().expect("mock route lock");
            Ok(store
                .get(collection_id)
                .map(|c| c.values().take(limit).cloned().collect())
                .unwrap_or_default())
        }

        async fn delete_records(
            &self,
            collection_id: &str,
            record_ids: Vec<String>,
            _tenant: Option<&str>,
        ) -> anyhow::Result<usize> {
            let mut store = self.store.lock().expect("mock route lock");
            let mut n = 0;
            if let Some(coll) = store.get_mut(collection_id) {
                for id in record_ids {
                    if coll.remove(&id).is_some() {
                        n += 1;
                    }
                }
            }
            Ok(n)
        }

        async fn collection_exists(&self, collection_id: &str, _tenant: Option<&str>) -> bool {
            // Every collection is canonical unless a test explicitly marks it non-canonical.
            !self
                .non_canonical
                .lock()
                .expect("mock route lock")
                .contains(collection_id)
        }

        async fn ensure_collection(
            &self,
            collection_id: &str,
            _dimension: u32,
            _tenant: Option<&str>,
            promote_keys: &[String],
        ) -> anyhow::Result<()> {
            // Provisioning makes a (possibly previously non-canonical) collection canonical —
            // idempotent (removing an absent entry is a no-op). Record the seeded promote keys so
            // tests can assert the document facade forwarded the declared index fields.
            self.non_canonical
                .lock()
                .expect("mock route lock")
                .remove(collection_id);
            self.promoted
                .lock()
                .expect("mock route lock")
                .insert(collection_id.to_string(), promote_keys.to_vec());
            Ok(())
        }
    }

    /// Scope the process-global `PROXIMADB_DOC_CANONICAL_VECTOR` gate to one collection for
    /// the duration of a test; removes it on drop. A unique collection name keeps the gate
    /// scoped even if the env leaks across a shared-process `cargo test` (nextest — the
    /// mandated runner — isolates per process, so the set/remove is single-threaded there).
    struct GateGuard;
    impl GateGuard {
        fn on(collection: &str) -> Self {
            // SAFETY (edition 2024): under nextest each test owns its process, so this env
            // mutation is single-threaded; the value is a fixed collection name (allowlist mode).
            unsafe { std::env::set_var("PROXIMADB_DOC_CANONICAL_VECTOR", collection) };
            Self
        }
        /// Force the global kill-switch OFF (the gate is DEFAULT-ON, so a test that wants the
        /// legacy path must explicitly force OFF).
        fn off() -> Self {
            unsafe { std::env::set_var("PROXIMADB_DOC_CANONICAL_VECTOR", "off") };
            Self
        }
    }
    impl Drop for GateGuard {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("PROXIMADB_DOC_CANONICAL_VECTOR") };
        }
    }

    fn doc_record(id: &str, collection: &str, title: &str) -> DocumentRecord {
        DocumentRecord::from_tree(
            id.to_string(),
            crate::storage::document::canonical_adapter::sql_object_to_proxima_tree(
                &make_document(vec![("title", sql_string(title))]),
            ),
            collection.to_string(),
            None,
            None,
        )
    }

    #[tokio::test]
    async fn canonical_route_insert_is_visible_via_shared_store_and_not_legacy_map() {
        let _gate = GateGuard::on("conv_docs");
        let svc = service_with_collection("conv_docs").await;
        let route = Arc::new(MockRecordRoute::default());
        svc.set_record_route(route.clone());

        // Insert via the DocumentService (the gRPC surface's entry point).
        svc.insert_document_record("conv_docs", doc_record("d1", "conv_docs", "Alpha"))
            .await
            .expect("canonical insert");

        // It landed in the SHARED store with a raw-id OID + document label — NOT the legacy map.
        {
            let store = route.store.lock().expect("lock");
            let coll = store
                .get("conv_docs")
                .expect("collection present in shared store");
            let stored = coll.get("d1").expect("raw-id OID key");
            assert_eq!(
                stored.oid, "d1",
                "OID is the raw doc id, no document/ prefix"
            );
            assert!(
                stored
                    .labels
                    .contains(proximadb_document::DOCUMENT_RECORD_LABEL),
                "record carries the document facade label"
            );
        }
        assert!(
            !svc.documents
                .get("conv_docs")
                .is_some_and(|d| d.contains_key("d1")),
            "canonical write must NOT populate the legacy in-memory map"
        );

        // Read-back via the DocumentService reads THROUGH the shared store (cross-surface).
        let got = svc
            .get_document("conv_docs", "d1", None)
            .await
            .expect("get")
            .expect("present");
        assert_eq!(got.id, "d1");
        assert_eq!(
            got.props.get("title"),
            Some(&ProximaTreeNode::Value(ProximaValue::String(
                "Alpha".to_string()
            )))
        );

        // Query sees it too, sourced from the shared store.
        let queried = svc
            .query_documents("conv_docs", DocumentQueryParams::default())
            .await
            .expect("query");
        assert_eq!(queried.documents.len(), 1);

        // Delete tombstones in the shared store.
        assert!(
            svc.delete_document("conv_docs", "d1")
                .await
                .expect("delete")
        );
        assert!(
            svc.get_document("conv_docs", "d1", None)
                .await
                .expect("get2")
                .is_none()
        );
    }

    #[tokio::test]
    async fn kill_switch_forces_legacy_path_and_ignores_route() {
        // The gate is DEFAULT-ON, so force the global kill-switch OFF to exercise the legacy path.
        let _gate = GateGuard::off();
        let svc = service_with_collection("legacy_docs").await;
        let route = Arc::new(MockRecordRoute::default());
        svc.set_record_route(route.clone());

        svc.insert_document_record("legacy_docs", doc_record("d1", "legacy_docs", "Beta"))
            .await
            .expect("legacy insert");

        // The shared store was NOT touched; the legacy map serves the doc.
        assert!(
            route.store.lock().expect("lock").is_empty(),
            "kill-switch OFF must not route to the shared store"
        );
        let got = svc
            .get_document("legacy_docs", "d1", None)
            .await
            .expect("get")
            .expect("present");
        assert_eq!(got.id, "d1");
    }

    #[tokio::test]
    async fn default_on_but_non_canonical_collection_stays_legacy() {
        // Gate DEFAULT-ON (no guard), route wired — but the collection is NOT a canonical vector
        // collection, so the mixed-write-safe capability check must keep it on the legacy path
        // (a pure-document collection must not hard-fail the canonical write under default-ON).
        let svc = service_with_collection("plain_docs").await;
        let route = Arc::new(MockRecordRoute::default());
        route
            .non_canonical
            .lock()
            .expect("lock")
            .insert("plain_docs".to_string());
        svc.set_record_route(route.clone());

        svc.insert_document_record("plain_docs", doc_record("d1", "plain_docs", "Delta"))
            .await
            .expect("legacy insert for non-canonical collection");

        assert!(
            route.store.lock().expect("lock").is_empty(),
            "a non-canonical collection must stay on the legacy path even with the gate ON"
        );
        let got = svc
            .get_document("plain_docs", "d1", None)
            .await
            .expect("get")
            .expect("present");
        assert_eq!(got.id, "d1");
    }

    // =========================================================================
    // P-Provision (ADR-055): document-collection create provisions the canonical collection
    // =========================================================================

    #[tokio::test]
    async fn create_collection_provisions_canonical_collection() {
        // With a route wired, creating a document collection provisions the canonical (record/
        // vector) collection, so a pure-document collection converges on the shared store.
        let svc = create_test_service();
        let route = Arc::new(MockRecordRoute::default());
        // Start NON-canonical (legacy) so provisioning is observable as a flip.
        route
            .non_canonical
            .lock()
            .expect("lock")
            .insert("provdocs".to_string());
        svc.set_record_route(route.clone());

        assert!(
            !route.collection_exists("provdocs", None).await,
            "precondition: collection starts non-canonical (legacy)"
        );

        svc.create_collection(
            "provdocs",
            DocumentCollectionConfig {
                name: "provdocs".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("create document collection");

        assert!(
            route.collection_exists("provdocs", None).await,
            "create_collection must provision the canonical collection (P-Provision)"
        );

        // A subsequent document write now routes CANONICAL (shared store), not the legacy map.
        svc.insert_document_record("provdocs", doc_record("d1", "provdocs", "Zeta"))
            .await
            .expect("canonical insert after provisioning");
        assert!(
            route
                .store
                .lock()
                .expect("lock")
                .get("provdocs")
                .is_some_and(|c| c.contains_key("d1")),
            "insert routes canonical after provisioning (lands in the shared store)"
        );
        assert!(
            !svc.documents
                .get("provdocs")
                .is_some_and(|d| d.contains_key("d1")),
            "canonical write must NOT populate the legacy in-memory map"
        );
    }

    #[tokio::test]
    async fn create_collection_forwards_top_level_index_keys_as_promote_keys() {
        // P-Shred follow-up (ADR-055): the document facade extracts the TOP-LEVEL scalar key from
        // each declared index path and forwards it to `ensure_collection` as a promote key (which
        // the catalog then seeds as a props-auto-promotion column). Nested/array paths are skipped.
        let svc = create_test_service();
        let route = Arc::new(MockRecordRoute::default());
        svc.set_record_route(route.clone());

        svc.create_collection(
            "idxdocs",
            DocumentCollectionConfig {
                name: "idxdocs".to_string(),
                indexes: vec![
                    IndexDefinition {
                        path: "$.status".to_string(),
                        index_type: DocIndexType::Btree as i32,
                        ..Default::default()
                    },
                    IndexDefinition {
                        path: "priority".to_string(),
                        index_type: DocIndexType::Btree as i32,
                        ..Default::default()
                    },
                    IndexDefinition {
                        path: "$.user.email".to_string(), // nested ⇒ skipped for promotion
                        index_type: DocIndexType::Btree as i32,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        )
        .await
        .expect("create document collection");

        let promoted = route.promoted.lock().expect("lock");
        let keys = promoted.get("idxdocs").expect("promote keys captured");
        assert!(
            keys.contains(&"status".to_string()),
            "top-level $.status promoted"
        );
        assert!(keys.contains(&"priority".to_string()), "bare key promoted");
        assert!(
            !keys
                .iter()
                .any(|k| k.contains("email") || k.contains("user")),
            "nested $.user.email is skipped (would shred nothing useful)"
        );
    }

    #[tokio::test]
    async fn ensure_collection_is_idempotent() {
        let route = MockRecordRoute::default();
        route
            .non_canonical
            .lock()
            .expect("lock")
            .insert("c".to_string());
        route
            .ensure_collection("c", 0, None, &[])
            .await
            .expect("first ensure");
        route
            .ensure_collection("c", 0, None, &[])
            .await
            .expect("second ensure is idempotent");
        assert!(route.collection_exists("c", None).await);
    }

    #[tokio::test]
    async fn create_collection_without_route_stays_legacy_no_panic() {
        // No route wired ⇒ create provisions nothing and does not panic (pure legacy path).
        let svc = create_test_service();
        svc.create_collection(
            "legacyonly",
            DocumentCollectionConfig {
                name: "legacyonly".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("create without route");
        assert!(
            svc.get_collection("legacyonly")
                .await
                .expect("get")
                .is_some(),
            "collection created on the legacy path when no route is wired"
        );
    }

    /// Set up a canonical-record-backed service with a pre-created collection.
    #[cfg(feature = "canonical-document-store")]
    async fn canonical_service_with_collection(collection_name: &str) -> DocumentService {
        use crate::storage::engines::cedar::CedarEngine;
        use proximadb_records::RecordStorage;

        let cedar = Arc::new(CedarEngine::new().expect("cedar engine"));
        let storage_engine: Arc<dyn UnifiedStorageFormat> = cedar.clone();
        let record_store: Arc<dyn RecordStorage> = cedar;
        let svc = DocumentService::with_canonical_record_store(storage_engine, record_store);

        svc.create_collection(
            collection_name,
            DocumentCollectionConfig {
                name: collection_name.to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("collection creation should succeed");
        svc
    }

    #[allow(dead_code)]
    fn assert_same_document_shape(left: &DocumentRecord, right: &DocumentRecord) {
        assert_eq!(left.id, right.id);
        assert_eq!(left.collection_id, right.collection_id);
        assert_eq!(left.version, right.version);
        assert_eq!(left.schema_id, right.schema_id);
        assert_eq!(left.document_type, right.document_type);
        assert_eq!(left.props, right.props);
    }

    /// Read a top-level field from a record's canonical props as a legacy
    /// `SqlValue` (test convenience after TD-106 Slice 7e removed `document`).
    fn field(rec: &DocumentRecord, key: &str) -> SqlValue {
        proxima_tree_to_sql_object(&rec.props)
            .fields
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("{key} field"))
    }

    // =========================================================================
    // Document CRUD lifecycle tests
    // =========================================================================

    #[tokio::test]
    async fn test_insert_and_get_document() {
        let svc = service_with_collection("books").await;

        let doc = make_document(vec![
            ("title", sql_string("Rust Programming")),
            ("year", sql_int(2024)),
        ]);

        let inserted = svc
            .insert_document("books", Some("book-1"), doc)
            .await
            .expect("insert should succeed");

        assert_eq!(inserted.id, "book-1");
        assert_eq!(inserted.version, 1);
        assert_eq!(inserted.collection_id, "books");

        // Retrieve and verify
        let fetched = svc
            .get_document("books", "book-1", None)
            .await
            .expect("get should succeed")
            .expect("document should exist");

        assert_eq!(fetched.id, "book-1");
        assert_eq!(fetched.version, 1);

        // Verify field contents
        let title_val = field(&fetched, "title");
        assert_eq!(
            title_val.value,
            Some(sql_value::Value::StringValue(
                "Rust Programming".to_string()
            ))
        );

        let year_val = field(&fetched, "year");
        assert_eq!(year_val.value, Some(sql_value::Value::Int64Value(2024)));
    }

    #[tokio::test]
    async fn test_update_document() {
        let svc = service_with_collection("users").await;

        let doc = make_document(vec![
            ("name", sql_string("Alice")),
            ("email", sql_string("alice@example.com")),
        ]);
        svc.insert_document("users", Some("user-1"), doc)
            .await
            .expect("insert should succeed");

        // Update the email field
        let updates = vec![DocumentUpdate {
            operation: UpdateOperation::Set as i32,
            path: "email".to_string(),
            value: Some(sql_string("alice@newdomain.com")),
        }];

        let updated = svc
            .update_document("users", "user-1", updates, None)
            .await
            .expect("update should succeed");

        assert_eq!(updated.version, 2, "version should be incremented");

        // Verify the update persisted
        let fetched = svc
            .get_document("users", "user-1", None)
            .await
            .expect("get should succeed")
            .expect("document should exist");

        let email = field(&fetched, "email");
        assert_eq!(
            email.value,
            Some(sql_value::Value::StringValue(
                "alice@newdomain.com".to_string()
            ))
        );

        // Original field should still be present
        let name = field(&fetched, "name");
        assert_eq!(
            name.value,
            Some(sql_value::Value::StringValue("Alice".to_string()))
        );

        // TD-106 Slice 7: the update mutates the canonical props tree directly.
        match fetched.props.get("email") {
            Some(proximadb_records::ProximaTreeNode::Value(
                proximadb_data_model::ProximaValue::String(s),
            )) => assert_eq!(
                s, "alice@newdomain.com",
                "props must carry the updated value"
            ),
            other => panic!("expected updated email in props, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_delete_document() {
        let svc = service_with_collection("items").await;

        let doc = make_document(vec![("product", sql_string("Widget"))]);
        svc.insert_document("items", Some("item-1"), doc)
            .await
            .expect("insert should succeed");

        // Confirm it exists
        let before = svc
            .get_document("items", "item-1", None)
            .await
            .expect("get should succeed");
        assert!(before.is_some(), "document should exist before delete");

        // Delete
        let deleted = svc
            .delete_document("items", "item-1")
            .await
            .expect("delete should succeed");
        assert!(deleted, "delete should return true for existing doc");

        // Confirm it is gone
        let after = svc
            .get_document("items", "item-1", None)
            .await
            .expect("get should succeed");
        assert!(after.is_none(), "document should be gone after delete");
    }

    #[tokio::test]
    async fn test_insert_duplicate_id() {
        let svc = service_with_collection("dup").await;

        let doc1 = make_document(vec![("val", sql_string("first"))]);
        svc.insert_document("dup", Some("same-id"), doc1)
            .await
            .expect("first insert should succeed");

        // Inserting with the same ID acts as an upsert in the in-memory store
        // because insert_document unconditionally inserts into the HashMap.
        let doc2 = make_document(vec![("val", sql_string("second"))]);
        svc.insert_document("dup", Some("same-id"), doc2)
            .await
            .expect("second insert (upsert) should succeed");

        let fetched = svc
            .get_document("dup", "same-id", None)
            .await
            .expect("get should succeed")
            .expect("document should exist");

        // The second insert should have overwritten the first
        let val = field(&fetched, "val");
        assert_eq!(
            val.value,
            Some(sql_value::Value::StringValue("second".to_string())),
            "second insert should overwrite the first"
        );
    }

    #[tokio::test]
    async fn test_get_nonexistent_document() {
        let svc = service_with_collection("empty_coll").await;

        let result = svc
            .get_document("empty_coll", "does-not-exist", None)
            .await
            .expect("get should not error");

        assert!(result.is_none(), "nonexistent ID should return None");
    }

    #[tokio::test]
    async fn test_insert_batch_documents() {
        let svc = service_with_collection("batch").await;

        let batch: Vec<(Option<String>, SqlObject)> = (0..5)
            .map(|i| {
                (
                    Some(format!("doc-{}", i)),
                    make_document(vec![("index", sql_int(i))]),
                )
            })
            .collect();

        let result = svc
            .insert_documents("batch", batch)
            .await
            .expect("batch insert should succeed");

        assert_eq!(result.ingested, 5);
        assert_eq!(result.failed, 0);
        assert!(result.errors.is_empty());

        // Verify each document is retrievable
        for i in 0..5 {
            let doc = svc
                .get_document("batch", &format!("doc-{}", i), None)
                .await
                .expect("get should succeed")
                .expect("document should exist");
            let idx_val = field(&doc, "index");
            assert_eq!(idx_val.value, Some(sql_value::Value::Int64Value(i)));
        }
    }

    // =========================================================================
    // Query tests
    // =========================================================================

    #[tokio::test]
    async fn test_query_with_filter() {
        let svc = service_with_collection("products").await;

        // Insert 3 documents with different categories
        svc.insert_document(
            "products",
            Some("p1"),
            make_document(vec![
                ("name", sql_string("Laptop")),
                ("category", sql_string("electronics")),
            ]),
        )
        .await
        .expect("insert p1");

        svc.insert_document(
            "products",
            Some("p2"),
            make_document(vec![
                ("name", sql_string("Shirt")),
                ("category", sql_string("clothing")),
            ]),
        )
        .await
        .expect("insert p2");

        svc.insert_document(
            "products",
            Some("p3"),
            make_document(vec![
                ("name", sql_string("Phone")),
                ("category", sql_string("electronics")),
            ]),
        )
        .await
        .expect("insert p3");

        // Query with filter: category == "electronics"
        let filter = DocumentFilter {
            conditions: vec![DocFilterCondition {
                path: "category".to_string(),
                operator: DocFilterOperator::Eq as i32,
                value: Some(sql_string("electronics")),
                values: Vec::new(),
            }],
            ..Default::default()
        };

        let result = svc
            .query_documents(
                "products",
                DocumentQueryParams {
                    filter: Some(filter),
                    limit: 100,
                    include_count: true,
                    ..Default::default()
                },
            )
            .await
            .expect("query should succeed");

        assert_eq!(
            result.documents.len(),
            2,
            "should return only electronics items"
        );
        assert_eq!(result.total_count, Some(2));

        // Verify all returned docs are in the electronics category
        for doc in &result.documents {
            let cat = field(doc, "category");
            assert_eq!(
                cat.value,
                Some(sql_value::Value::StringValue("electronics".to_string()))
            );
        }
    }

    #[tokio::test]
    async fn test_query_with_pagination() {
        let svc = service_with_collection("paginated").await;

        // Insert 10 documents
        for i in 0..10 {
            svc.insert_document(
                "paginated",
                Some(&format!("item-{:02}", i)),
                make_document(vec![("seq", sql_int(i))]),
            )
            .await
            .expect("insert should succeed");
        }

        // Query with limit=3, offset=2
        let result = svc
            .query_documents(
                "paginated",
                DocumentQueryParams {
                    limit: 3,
                    offset: 2,
                    include_count: true,
                    ..Default::default()
                },
            )
            .await
            .expect("query should succeed");

        assert_eq!(
            result.documents.len(),
            3,
            "should return exactly 3 documents"
        );
        assert_eq!(
            result.total_count,
            Some(10),
            "total count should be 10 (before pagination)"
        );
    }

    #[tokio::test]
    async fn test_query_all_documents() {
        let svc = service_with_collection("all_docs").await;

        // Insert 4 documents
        for i in 0..4 {
            svc.insert_document(
                "all_docs",
                Some(&format!("d{}", i)),
                make_document(vec![("n", sql_int(i))]),
            )
            .await
            .expect("insert should succeed");
        }

        // Query with no filter (limit=0 means "all")
        let result = svc
            .query_documents(
                "all_docs",
                DocumentQueryParams {
                    include_count: true,
                    ..Default::default()
                },
            )
            .await
            .expect("query should succeed");

        assert_eq!(result.documents.len(), 4, "should return all 4 documents");
        assert_eq!(result.total_count, Some(4));
    }

    #[cfg(feature = "canonical-document-store")]
    #[tokio::test]
    async fn test_canonical_document_service_parity_with_legacy_path() {
        let legacy = service_with_collection("parity").await;
        let canonical = canonical_service_with_collection("parity").await;

        let doc = make_document(vec![
            ("title", sql_string("Record Spine")),
            ("category", sql_string("architecture")),
            ("revision", sql_int(1)),
        ]);

        let legacy_inserted = legacy
            .insert_document("parity", Some("doc-1"), doc.clone())
            .await
            .expect("legacy insert");
        let canonical_inserted = canonical
            .insert_document("parity", Some("doc-1"), doc)
            .await
            .expect("canonical insert");
        assert_same_document_shape(&legacy_inserted, &canonical_inserted);

        let legacy_fetched = legacy
            .get_document("parity", "doc-1", None)
            .await
            .expect("legacy get")
            .expect("legacy document");
        let canonical_fetched = canonical
            .get_document("parity", "doc-1", None)
            .await
            .expect("canonical get")
            .expect("canonical document");
        assert_same_document_shape(&legacy_fetched, &canonical_fetched);

        let updates = vec![DocumentUpdate {
            operation: UpdateOperation::Set as i32,
            path: "revision".to_string(),
            value: Some(sql_int(2)),
        }];
        let legacy_updated = legacy
            .update_document("parity", "doc-1", updates.clone(), None)
            .await
            .expect("legacy update");
        let canonical_updated = canonical
            .update_document("parity", "doc-1", updates, None)
            .await
            .expect("canonical update");
        assert_same_document_shape(&legacy_updated, &canonical_updated);

        legacy
            .insert_document(
                "parity",
                Some("doc-2"),
                make_document(vec![
                    ("title", sql_string("Projection")),
                    ("category", sql_string("architecture")),
                    ("revision", sql_int(1)),
                ]),
            )
            .await
            .expect("legacy insert second");
        canonical
            .insert_document(
                "parity",
                Some("doc-2"),
                make_document(vec![
                    ("title", sql_string("Projection")),
                    ("category", sql_string("architecture")),
                    ("revision", sql_int(1)),
                ]),
            )
            .await
            .expect("canonical insert second");

        let filter = DocumentFilter {
            conditions: vec![DocFilterCondition {
                path: "category".to_string(),
                operator: DocFilterOperator::Eq as i32,
                value: Some(sql_string("architecture")),
                values: Vec::new(),
            }],
            ..Default::default()
        };
        let query_params = DocumentQueryParams {
            filter: Some(filter),
            include_count: true,
            limit: 100,
            ..Default::default()
        };
        let legacy_query = legacy
            .query_documents("parity", query_params.clone())
            .await
            .expect("legacy query");
        let canonical_query = canonical
            .query_documents("parity", query_params)
            .await
            .expect("canonical query");
        assert_eq!(legacy_query.total_count, canonical_query.total_count);

        let mut legacy_ids: Vec<_> = legacy_query
            .documents
            .iter()
            .map(|document| document.id.as_str())
            .collect();
        let mut canonical_ids: Vec<_> = canonical_query
            .documents
            .iter()
            .map(|document| document.id.as_str())
            .collect();
        legacy_ids.sort_unstable();
        canonical_ids.sort_unstable();
        assert_eq!(legacy_ids, canonical_ids);

        assert!(legacy.delete_document("parity", "doc-1").await.unwrap());
        assert!(canonical.delete_document("parity", "doc-1").await.unwrap());
        assert!(
            legacy
                .get_document("parity", "doc-1", None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            canonical
                .get_document("parity", "doc-1", None)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[cfg(feature = "canonical-document-store")]
    #[tokio::test]
    async fn test_canonical_document_query_uses_record_oid_projection_keys() {
        use crate::storage::engines::cedar::CedarEngine;
        use proximadb_records::RecordStorage;

        let cedar = Arc::new(CedarEngine::new().expect("cedar engine"));
        let storage_engine: Arc<dyn UnifiedStorageFormat> = cedar.clone();
        let record_store: Arc<dyn RecordStorage> = cedar;
        let svc = DocumentService::with_canonical_record_store(storage_engine, record_store);

        svc.create_collection(
            "indexed",
            DocumentCollectionConfig {
                name: "indexed".to_string(),
                indexes: vec![IndexDefinition {
                    path: "category".to_string(),
                    index_type: DocIndexType::Btree as i32,
                    unique: false,
                    sparse: false,
                    name: Some("category_idx".to_string()),
                }],
                ..Default::default()
            },
        )
        .await
        .expect("collection creation should succeed");

        svc.insert_document(
            "indexed",
            Some("doc-1"),
            make_document(vec![
                ("title", sql_string("Canonical Index")),
                ("category", sql_string("architecture")),
            ]),
        )
        .await
        .expect("insert should succeed");

        let query_params = DocumentQueryParams {
            filter: Some(DocumentFilter {
                conditions: vec![DocFilterCondition {
                    path: "category".to_string(),
                    operator: DocFilterOperator::Eq as i32,
                    value: Some(sql_string("architecture")),
                    values: Vec::new(),
                }],
                ..Default::default()
            }),
            include_count: true,
            ..Default::default()
        };

        let query_result = svc
            .query_documents("indexed", query_params.clone())
            .await
            .expect("indexed canonical query should succeed");
        assert_eq!(query_result.total_count, Some(1));
        assert_eq!(query_result.documents[0].id, "doc-1");

        assert!(
            svc.delete_document("indexed", "doc-1")
                .await
                .expect("delete should succeed")
        );

        let query_after_delete = svc
            .query_documents("indexed", query_params)
            .await
            .expect("indexed canonical query after delete should succeed");
        assert_eq!(query_after_delete.total_count, Some(0));
        assert!(query_after_delete.documents.is_empty());
    }

    #[cfg(feature = "canonical-document-store")]
    #[tokio::test]
    async fn test_canonical_document_wal_recovery_replays_into_record_store() {
        use crate::storage::engines::cedar::CedarEngine;
        use proximadb_records::RecordStorage;

        let temp_dir = tempfile::tempdir().expect("temp wal dir");
        let wal_base_path = temp_dir.path().to_str().expect("utf-8 temp path");
        let collection_id = "wal_docs_upsert";
        let document_id = "doc-upsert";

        let first_cedar = Arc::new(CedarEngine::new().expect("cedar engine"));
        let first_storage_engine: Arc<dyn UnifiedStorageFormat> = first_cedar.clone();
        let first_record_store: Arc<dyn RecordStorage> = first_cedar;
        let first = DocumentService::with_canonical_record_store_and_wal(
            first_storage_engine,
            first_record_store,
            wal_base_path,
        )
        .await
        .expect("canonical wal service");

        first
            .create_collection(
                collection_id,
                DocumentCollectionConfig {
                    name: collection_id.to_string(),
                    ..Default::default()
                },
            )
            .await
            .expect("create collection");
        first
            .insert_document(
                collection_id,
                Some(document_id),
                make_document(vec![("title", sql_string("Recovered"))]),
            )
            .await
            .expect("insert");
        first.flush_wal().await.expect("flush wal");
        let wal_dir = format!("{}/document_wal", wal_base_path);
        let wal_files = std::fs::read_dir(&wal_dir)
            .expect("list document wal dir")
            .map(|entry| {
                let entry = entry.expect("wal dir entry");
                let len = entry.metadata().expect("wal entry metadata").len();
                format!("{}:{}", entry.file_name().to_string_lossy(), len)
            })
            .collect::<Vec<_>>();
        let durable_entries =
            crate::storage::persistence::write_ahead_log::wal_operations::UnifiedWALReader::new(
                wal_dir,
            )
            .await
            .expect("open wal reader")
            .read_all()
            .await
            .expect("read flushed wal");
        assert!(
            durable_entries.iter().any(|entry| matches!(
                &entry.operation,
                UnifiedWALOperation::DocumentOp(
                    DocumentOperation::UpsertCanonicalDocumentRecord {
                        collection_id: wal_collection,
                        ..
                    }
                ) if wal_collection == collection_id
            )),
            "flushed WAL should contain canonical document upsert; files {:?}; got {:?}",
            wal_files,
            durable_entries
                .iter()
                .map(|entry| &entry.operation)
                .collect::<Vec<_>>()
        );
        drop(first);

        let restarted_cedar = Arc::new(CedarEngine::new().expect("restarted cedar engine"));
        let restarted_storage_engine: Arc<dyn UnifiedStorageFormat> = restarted_cedar.clone();
        let restarted_record_store: Arc<dyn RecordStorage> = restarted_cedar;
        let restarted_record_probe = restarted_record_store.clone();
        let restarted = DocumentService::with_canonical_record_store_and_wal(
            restarted_storage_engine,
            restarted_record_store,
            wal_base_path,
        )
        .await
        .expect("restart from wal");

        let recovered_scan = restarted_record_probe
            .scan_records(10)
            .await
            .expect("scan recovered records");
        assert_eq!(
            recovered_scan.len(),
            1,
            "canonical WAL recovery should replay one record; got {:?}",
            recovered_scan
                .iter()
                .map(|record| record.oid.as_str())
                .collect::<Vec<_>>()
        );

        let recovered_key = DocumentRecordKey::new(collection_id, document_id);
        let recovered_record = restarted_record_probe
            .get_record(&RecordKey::new(recovered_key.canonical_oid()))
            .await
            .expect("get recovered canonical record")
            .unwrap_or_else(|| {
                panic!(
                    "canonical record recovered at {}; scanned {:?}",
                    recovered_key.canonical_oid(),
                    recovered_scan
                        .iter()
                        .map(|record| record.oid.as_str())
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(recovered_record.oid, recovered_key.canonical_oid());

        let recovered = restarted
            .get_document(collection_id, document_id, None)
            .await
            .expect("get recovered")
            .expect("document recovered through canonical store");
        assert_eq!(
            field(&recovered, "title").value,
            Some(sql_value::Value::StringValue("Recovered".to_string()))
        );
    }

    #[cfg(feature = "canonical-document-store")]
    #[tokio::test]
    async fn test_canonical_document_wal_recovery_replays_deletes_into_record_store() {
        use crate::storage::engines::cedar::CedarEngine;
        use proximadb_records::RecordStorage;

        let temp_dir = tempfile::tempdir().expect("temp wal dir");
        let wal_base_path = temp_dir.path().to_str().expect("utf-8 temp path");
        let collection_id = "wal_docs_delete";
        let document_id = "doc-delete";

        let first_cedar = Arc::new(CedarEngine::new().expect("cedar engine"));
        let first_storage_engine: Arc<dyn UnifiedStorageFormat> = first_cedar.clone();
        let first_record_store: Arc<dyn RecordStorage> = first_cedar;
        let first = DocumentService::with_canonical_record_store_and_wal(
            first_storage_engine,
            first_record_store,
            wal_base_path,
        )
        .await
        .expect("canonical wal service");

        first
            .create_collection(
                collection_id,
                DocumentCollectionConfig {
                    name: collection_id.to_string(),
                    ..Default::default()
                },
            )
            .await
            .expect("create collection");
        first
            .insert_document(
                collection_id,
                Some(document_id),
                make_document(vec![("title", sql_string("Deleted"))]),
            )
            .await
            .expect("insert");
        assert!(
            first
                .delete_document(collection_id, document_id)
                .await
                .expect("delete")
        );
        first.flush_wal().await.expect("flush wal");
        drop(first);

        let restarted_cedar = Arc::new(CedarEngine::new().expect("restarted cedar engine"));
        let restarted_storage_engine: Arc<dyn UnifiedStorageFormat> = restarted_cedar.clone();
        let restarted_record_store: Arc<dyn RecordStorage> = restarted_cedar;
        let restarted_record_probe = restarted_record_store.clone();
        let restarted = DocumentService::with_canonical_record_store_and_wal(
            restarted_storage_engine,
            restarted_record_store,
            wal_base_path,
        )
        .await
        .expect("restart from wal");

        let recovered_records = restarted_record_probe
            .scan_records(10)
            .await
            .expect("scan recovered records");
        assert!(
            recovered_records.is_empty(),
            "delete replay should remove canonical records"
        );

        assert!(
            restarted
                .get_document(collection_id, document_id, None)
                .await
                .expect("get after delete replay")
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_query_empty_collection() {
        let svc = service_with_collection("empty").await;

        let result = svc
            .query_documents(
                "empty",
                DocumentQueryParams {
                    include_count: true,
                    ..Default::default()
                },
            )
            .await
            .expect("query on empty collection should succeed");

        assert!(result.documents.is_empty(), "should return no documents");
        assert_eq!(result.total_count, Some(0));
    }

    #[tokio::test]
    async fn document_query_service_searches_via_contract() {
        use proximadb_document_query::{
            DocumentQueryService, DocumentSearchRequest, DocumentSortOrder, SortDirection,
        };

        let svc = service_with_collection("contract_docs").await;

        for i in 0..3 {
            svc.insert_document(
                "contract_docs",
                Some(&format!("doc-{i}")),
                make_document(vec![("seq", sql_int(i))]),
            )
            .await
            .expect("insert should succeed");
        }

        let result = DocumentQueryService::document_search(
            &svc,
            DocumentSearchRequest {
                collection_id: "contract_docs".to_string(),
                filter: None,
                limit: 2,
                offset: 1,
                projection: None,
                sort: Some(DocumentSortOrder {
                    field: "seq".to_string(),
                    direction: SortDirection::Ascending,
                }),
            },
        )
        .await
        .expect("contract search should succeed");

        assert_eq!(result.total_count, 3);
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.results[0].id, "doc-1");
        assert_eq!(result.results[1].id, "doc-2");
    }

    #[tokio::test]
    async fn document_query_service_gets_document_via_contract() {
        use proximadb_document_query::DocumentQueryService;

        let svc = service_with_collection("contract_get").await;
        svc.insert_document(
            "contract_get",
            Some("doc-1"),
            make_document(vec![("title", sql_string("Contract"))]),
        )
        .await
        .expect("insert should succeed");

        let result = DocumentQueryService::get_document(
            &svc,
            "contract_get".to_string(),
            "doc-1".to_string(),
        )
        .await
        .expect("contract get should succeed")
        .expect("document should exist");

        assert_eq!(result.id, "doc-1");
        assert_eq!(result.version, 1);
    }

    // =========================================================================
    // Collection management tests
    // =========================================================================

    #[tokio::test]
    async fn test_create_and_list_collections() {
        let svc = create_test_service();

        // No collections initially
        let before = svc.list_collections().await.expect("list should succeed");
        assert!(before.is_empty(), "should start with no collections");

        // Create two collections
        svc.create_collection("alpha", test_collection_config())
            .await
            .expect("create alpha should succeed");
        svc.create_collection(
            "beta",
            DocumentCollectionConfig {
                name: "beta".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("create beta should succeed");

        let after = svc.list_collections().await.expect("list should succeed");
        assert_eq!(after.len(), 2, "should have 2 collections");

        let names: Vec<&str> = after.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));

        // Verify get_collection returns metadata
        let alpha = svc
            .get_collection("alpha")
            .await
            .expect("get should succeed")
            .expect("alpha should exist");
        assert_eq!(alpha.name, "alpha");
        assert_eq!(alpha.document_count, 0);
    }

    #[tokio::test]
    async fn test_delete_collection() {
        let svc = create_test_service();

        svc.create_collection("ephemeral", test_collection_config())
            .await
            .expect("create should succeed");

        // Insert a document so we can verify data is also removed
        svc.insert_document(
            "ephemeral",
            Some("d1"),
            make_document(vec![("x", sql_int(1))]),
        )
        .await
        .expect("insert should succeed");

        // Delete the collection
        let deleted = svc
            .delete_collection("ephemeral")
            .await
            .expect("delete should succeed");
        assert!(deleted, "delete should return true for existing collection");

        // Verify it is gone
        let after = svc
            .get_collection("ephemeral")
            .await
            .expect("get should succeed");
        assert!(after.is_none(), "collection should be gone after delete");

        // Listing should not include it
        let list = svc.list_collections().await.expect("list should succeed");
        assert!(list.is_empty(), "no collections should remain");

        // Deleting again should return false
        let again = svc
            .delete_collection("ephemeral")
            .await
            .expect("delete should succeed");
        assert!(!again, "deleting non-existent collection returns false");
    }

    #[test]
    fn scoped_document_collection_default_bare_named_scoped_invalid_rejected() {
        assert_eq!(
            scoped_document_collection("default", "docs").unwrap(),
            "docs",
            "default tenant stays bare (matches bare-created collections)"
        );
        assert_eq!(
            scoped_document_collection("acme", "docs").unwrap(),
            "acme/docs",
            "named tenant is path-scoped"
        );
        assert!(
            scoped_document_collection("../evil", "docs").is_err(),
            "path traversal rejected"
        );
        assert!(
            scoped_document_collection("_system", "docs").is_err(),
            "reserved-prefix tenant rejected"
        );
    }

    /// Named-tenant document isolation with NO explicit provisioning (TD-DOC-TENANT-1): the first
    /// write to each scoped collection auto-provisions it, and two tenants using the SAME clean
    /// logical collection + doc id are isolated by their distinct `{tenant}/{collection}` keys.
    #[tokio::test]
    async fn scoped_document_collections_isolate_by_tenant_with_auto_provision() {
        let svc = create_test_service();
        let acme = scoped_document_collection("acme", "shared").unwrap();
        let globex = scoped_document_collection("globex", "shared").unwrap();

        // Same clean logical collection ("shared") AND same doc id ("d1"), different tenants.
        svc.insert_document(
            &acme,
            Some("d1"),
            make_document(vec![("owner", sql_string("acme"))]),
        )
        .await
        .expect("acme insert auto-provisions acme/shared");
        svc.insert_document(
            &globex,
            Some("d1"),
            make_document(vec![("owner", sql_string("globex"))]),
        )
        .await
        .expect("globex insert auto-provisions globex/shared");

        let a = svc
            .get_document(&acme, "d1", None)
            .await
            .expect("get acme")
            .expect("acme doc exists");
        let g = svc
            .get_document(&globex, "d1", None)
            .await
            .expect("get globex")
            .expect("globex doc exists");

        // Each tenant's doc lives under its own scoped collection — no cross-tenant bleed despite
        // identical clean collection name + doc id.
        assert_eq!(a.collection_id, "acme/shared");
        assert_eq!(g.collection_id, "globex/shared");
        assert_ne!(
            tree_get(&a.props, "owner"),
            tree_get(&g.props, "owner"),
            "same clean collection + doc id isolate by tenant"
        );
    }
}
