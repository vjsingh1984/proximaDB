/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Canonical record-batch orchestration service (TD-104 / CONVERGENCE_AUDIT S1, slice S3-c)
//!
//! [`RecordOpsService`] owns the record-batch insert/upsert/delete orchestration that
//! the Arrow Flight ingest path (`do_put`) and the REST/gRPC v2 record-batch path drive.
//! It is the single canonical implementation of [`proximadb_runtime::RecordOpsPort`].
//!
//! ## Convergence
//!
//! The ROOT `UnifiedHandlers` no longer carries this logic inline — it holds an
//! `Arc<RecordOpsService>` and its inherent `handle_record_*_for_tenant` methods plus
//! its `RecordOpsPort` impl are thin delegations to this service (Convergence Gate: no
//! duplicated logic). This lets the Flight service depend on the canonical
//! [`proximadb_runtime::RecordOpsPort`] backed directly by the runtime
//! `RecordOpsService` rather than reaching through the ROOT handler.
//!
//! ## Authority / parity
//!
//! No durable authority lives here — this is a façade over the same vector/record
//! services. Write-path behaviour is preserved EXACTLY versus the prior ROOT inline
//! implementation:
//! - tenant-context load (`CollectionService::load_tenant_context`),
//! - collection-id resolution incl. the TTL cache (`resolve_collection_id_internal`),
//! - the WAL-lane gate ([`enforce_wal_lane_for_record_batch`] → `WAL_LANE_REJECTED`),
//! - schema validation (`SCHEMA_VALIDATION_FAILED`),
//! - canonical-precision coercion + the per-precision `canonical_bytes` metric
//!   (TD-080/TD-082),
//! - DML row-count stat bumps (+n insert / -n delete),
//! - `NOT_FOUND` / `RECORD_INSERT_FAILED` error shapes.

use anyhow::{Result, anyhow};
use std::sync::Arc;
use tracing::debug;

use crate::api_handlers::request_handlers::{CollectionIdCache, enforce_wal_lane_for_record_batch};
use crate::core::search::FilterExpression;
use crate::services::DmlService;
use crate::services::WriteOperationKind;
use crate::services::collection::manager::CollectionService;
use crate::services::operations::BatchOperationResult;
use crate::services::operations::vectors::{
    RichRecordBatchRequest, RichRecordDeleteBatchRequest, RichRecordGetRequest,
    RichRecordGetResponse, RichSearchRequest, RichSearchResponse, VectorOperationsService,
};
use crate::services::record_store::ChangeRow;
use crate::services::scan_cursor::ScanCursor;
use proximadb_records::ProximaRecord;

/// Canonical record-batch orchestration service.
///
/// Holds exactly the dependencies the record write path needs. It is the single
/// owner of the collection-id cache, the DML-service handle, and the
/// canonical-precision resolver for the record path; ROOT forwards its
/// post-construction setters here so there is one logical owner of this state.
pub struct RecordOpsService {
    collection_service: Arc<CollectionService>,
    vector_operations_service: Arc<VectorOperationsService>,
    /// Cache for collection ID resolution (one owner — ROOT delegates here).
    collection_id_cache: CollectionIdCache,
    /// DML service for schema validation + row-count stats. Settable
    /// post-construction (thread-safe), mirroring ROOT's prior behaviour.
    dml_service: std::sync::RwLock<Option<Arc<DmlService>>>,
    /// DDL service for relational CREATE/ALTER/DROP submitted over the gRPC
    /// `ExecuteQuery` RPC (TD-135). Settable post-construction (thread-safe).
    ddl_service: std::sync::RwLock<Option<Arc<crate::services::DdlService>>>,
    /// Set-once canonical-precision resolver — wired at server bootstrap so the
    /// record path coerces embeddings to each collection's canonical precision
    /// before WAL append (TD-080/TD-082).
    precision_resolver: std::sync::OnceLock<
        Arc<proximadb_catalog::canonical_precision::CanonicalPrecisionResolver>,
    >,
    /// Durable partition-lease manager for lease-on-write (Scenario-1 routing
    /// truth). When present, the record write path acquires/confirms this pod's
    /// collection lease before writing, so the shared primary-pod registry that
    /// the network gates consult is durably backed (not empty-after-restart).
    /// Settable post-construction (thread-safe); absent in embedded/test builds.
    lease_manager:
        std::sync::RwLock<Option<Arc<crate::cluster::partition_lease::PartitionLeaseManager>>>,
}

impl RecordOpsService {
    /// Build the service from the same Arcs ROOT holds.
    pub fn new(
        collection_service: Arc<CollectionService>,
        vector_operations_service: Arc<VectorOperationsService>,
    ) -> Self {
        Self {
            collection_service,
            vector_operations_service,
            collection_id_cache: CollectionIdCache::new(),
            dml_service: std::sync::RwLock::new(None),
            ddl_service: std::sync::RwLock::new(None),
            precision_resolver: std::sync::OnceLock::new(),
            lease_manager: std::sync::RwLock::new(None),
        }
    }

    /// Wire a `DmlService` for schema validation + row-count stats.
    /// Callable post-initialization; thread-safe.
    pub fn set_dml_service(&self, svc: Arc<DmlService>) {
        if let Ok(mut guard) = self.dml_service.write() {
            *guard = Some(svc);
        }
    }

    pub(crate) fn get_dml_service(&self) -> Option<Arc<DmlService>> {
        self.dml_service.read().ok().and_then(|guard| guard.clone())
    }

    /// Wire the durable partition-lease manager for lease-on-write.
    /// Callable post-initialization; thread-safe.
    pub fn set_lease_manager(
        &self,
        manager: Arc<crate::cluster::partition_lease::PartitionLeaseManager>,
    ) {
        if let Ok(mut guard) = self.lease_manager.write() {
            *guard = Some(manager);
        }
    }

    /// Lease-on-write: make the shared primary-pod registry truthful for
    /// `(tenant, collection)` by acquiring/confirming this pod's durable
    /// collection lease before the write proceeds. Keyed by the collection NAME
    /// (`request.collection_id`) and the transport tenant id — the exact pair the
    /// network gates' `consult_for_write` uses — so the binding this populates is
    /// the one the gates read. Idempotent + cheap after the first write (registry
    /// fast-path; the renew loop keeps the lease warm). Fail-open: a missing
    /// manager (embedded/tests) or a transient acquire error never blocks the
    /// write — the network gate + the A6 storage fence are the enforcement points.
    async fn ensure_collection_lease(&self, tenant_id: &str, collection_name: &str) {
        let Some(manager) = self.lease_manager.read().ok().and_then(|g| g.clone()) else {
            return;
        };
        let now_ms = chrono::Utc::now().timestamp_millis();
        match manager
            .ensure_owned(tenant_id, collection_name, now_ms)
            .await
        {
            Ok(true) => {}
            Ok(false) => tracing::warn!(
                target = "proximadb.primary_pod.lease_on_write",
                tenant_id = %tenant_id,
                collection_id = %collection_name,
                "lease-on-write: this pod is not the primary for the collection; \
                 shared registry repointed to the owner (gate should now 421 subsequent writes)"
            ),
            Err(e) => tracing::warn!(
                target = "proximadb.primary_pod.lease_on_write",
                error = %e,
                tenant_id = %tenant_id,
                collection_id = %collection_name,
                "lease-on-write acquire failed; proceeding fail-open"
            ),
        }
    }

    /// Wire a `DdlService` so gRPC `ExecuteQuery` can run relational DDL (TD-135).
    /// Callable post-initialization; thread-safe.
    pub fn set_ddl_service(&self, svc: Arc<crate::services::DdlService>) {
        if let Ok(mut guard) = self.ddl_service.write() {
            *guard = Some(svc);
        }
    }

    pub(crate) fn get_ddl_service(&self) -> Option<Arc<crate::services::DdlService>> {
        self.ddl_service.read().ok().and_then(|guard| guard.clone())
    }

    /// Post-construction setter for the canonical-precision resolver.
    /// Called once at server bootstrap (`ProximaDB::new` in `src/database.rs`).
    pub fn set_precision_resolver(
        &self,
        resolver: Arc<proximadb_catalog::canonical_precision::CanonicalPrecisionResolver>,
    ) {
        let _ = self.precision_resolver.set(resolver);
    }

    // ---- collection-id resolution (cache is owned here) ----

    /// Resolve a collection name or UUID to its canonical UUID with caching.
    pub async fn resolve_collection_id_cached(&self, identifier: &str) -> Result<Option<String>> {
        if let Some(cached_id) = self.collection_id_cache.get(identifier) {
            return Ok(Some(cached_id));
        }

        let result = self
            .collection_service
            .resolve_collection_id(identifier)
            .await?;

        if let Some(ref id) = result {
            self.collection_id_cache
                .insert(identifier.to_string(), id.clone());
            debug!(
                "Collection ID cache miss: '{}' -> '{}' (cached)",
                identifier, id
            );
        }

        Ok(result)
    }

    /// Invalidate a single collection ID cache entry.
    pub fn invalidate_collection_cache(&self, identifier: &str) {
        self.collection_id_cache.invalidate(identifier);
        debug!("Collection ID cache invalidated: '{}'", identifier);
    }

    /// Clear the entire collection ID cache.
    pub fn clear_collection_cache(&self) {
        self.collection_id_cache.clear();
        debug!("Collection ID cache cleared");
    }

    pub(crate) async fn resolve_collection_id_internal(
        &self,
        collection_identifier: &str,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<Option<String>> {
        if let Some(tenant_ctx) = tenant_context {
            Ok(self
                .collection_service
                .get_collection_with_tenant_context(collection_identifier, Some(tenant_ctx))
                .await?
                .map(|collection| collection.id))
        } else {
            self.resolve_collection_id_cached(collection_identifier)
                .await
        }
    }

    // ---- precision coercion (TD-080/TD-082) ----

    /// Mirror of `CollectionManager::collection_table_identifier` so the resolver
    /// hits the same catalog key the manager wrote at create time.
    fn collection_to_table_identifier(
        target_collection: &str,
    ) -> proximadb_catalog::TableIdentifier {
        let parsed = proximadb_catalog::TableIdentifier::parse(target_collection);
        if parsed.namespace.is_empty() {
            proximadb_catalog::TableIdentifier::new(vec!["default".to_string()], parsed.name)
        } else {
            parsed
        }
    }

    /// Coerce every embedding cell on every record to the collection's canonical
    /// precision (resolved via xCatalog) **in place**, then emit the per-precision
    /// `canonical_bytes` metric. Single source of truth for the v2 precision-coercion
    /// contract across the insert-only (Arrow Flight) and upsert (REST/gRPC v2) paths
    /// (TD-082 closure).
    pub(crate) async fn coerce_records_to_canonical_precision(
        &self,
        records: &mut [proximadb_records::ProximaRecord],
        collection_id: &str,
    ) {
        if let Some(resolver) = self.precision_resolver.get() {
            let table_id = Self::collection_to_table_identifier(collection_id);
            match resolver.resolve(&table_id).await {
                Ok(target_precision)
                    if target_precision != proximadb_records::EmbeddingScalarType::Fp32 =>
                {
                    for record in records.iter_mut() {
                        for cell in &mut record.embeddings {
                            cell.coerce_to_precision(target_precision);
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        collection = %collection_id,
                        error = %e,
                        "v2 record batch: precision resolver lookup failed; \
                         records will land at their input precision"
                    );
                }
            }
        }
        if let Some(pm) = crate::observability::precision_metrics::metrics() {
            let mut per_precision: std::collections::HashMap<
                proximadb_records::EmbeddingScalarType,
                i64,
            > = std::collections::HashMap::new();
            for record in records.iter() {
                for cell in &record.embeddings {
                    *per_precision.entry(cell.precision).or_insert(0) +=
                        cell.values_byte_size() as i64;
                }
            }
            for (precision, delta) in per_precision {
                pm.add_canonical_bytes(
                    collection_id,
                    crate::observability::precision_metrics::precision_label(precision),
                    delta,
                );
            }
        }
    }

    // ---- record-batch orchestration ----

    /// Canonical rich-record delete handler used by v2 REST/gRPC/internal callers.
    pub async fn handle_record_delete_batch_for_tenant(
        &self,
        request: RichRecordDeleteBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<BatchOperationResult> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        let collection_id = match self
            .resolve_collection_id_internal(&request.collection_id, tenant_context.as_ref())
            .await?
        {
            Some(id) => id,
            None => {
                return Err(anyhow!("Collection '{}' not found", request.collection_id));
            }
        };

        let collection_name = request.collection_id.clone();
        if let Err(e) = enforce_wal_lane_for_record_batch(
            &collection_name,
            WriteOperationKind::Delete,
            request.record_ids.len() as u64,
            "REST/gRPC handle_record_delete_batch_for_tenant",
        ) {
            return Ok(BatchOperationResult::failure(
                e,
                "WAL_LANE_REJECTED".to_string(),
            ));
        }
        self.ensure_collection_lease(tenant_id.unwrap_or(""), &collection_name)
            .await;
        let result = self
            .vector_operations_service
            .delete_records_with_tenant_context(
                &collection_id,
                request.record_ids,
                tenant_context.as_ref(),
            )
            .await?;
        if result.success {
            let n = result.vector_ids.len() as i64;
            if n > 0
                && let Some(dml_svc) = self.get_dml_service()
            {
                dml_svc.bump_row_count_stats(&collection_name, -n).await;
            }
        }
        Ok(result)
    }

    /// Canonical rich-record batch (upsert) handler used by v2 REST/gRPC/internal callers.
    pub async fn handle_record_batch_for_tenant(
        &self,
        request: RichRecordBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<BatchOperationResult> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        let collection_id = match self
            .resolve_collection_id_internal(&request.collection_id, tenant_context.as_ref())
            .await?
        {
            Some(id) => id,
            None => {
                return Ok(BatchOperationResult::failure(
                    format!("Collection '{}' not found", request.collection_id),
                    "NOT_FOUND".to_string(),
                ));
            }
        };

        let collection_name = request.collection_id.clone();
        if let Some(dml_svc) = self.get_dml_service()
            && let Err(e) = dml_svc
                .validate_record_batch_against_schema(&collection_name, &request.records)
                .await
        {
            return Ok(BatchOperationResult::failure(
                format!("Schema validation failed: {}", e),
                "SCHEMA_VALIDATION_FAILED".to_string(),
            ));
        }
        if let Err(e) = enforce_wal_lane_for_record_batch(
            &collection_name,
            WriteOperationKind::Insert,
            request.records.len() as u64,
            "REST/gRPC handle_record_batch_for_tenant",
        ) {
            return Ok(BatchOperationResult::failure(
                e,
                "WAL_LANE_REJECTED".to_string(),
            ));
        }
        self.ensure_collection_lease(tenant_id.unwrap_or(""), &collection_name)
            .await;

        let mut records = request.records;
        self.coerce_records_to_canonical_precision(&mut records, &request.collection_id)
            .await;

        match self
            .vector_operations_service
            .insert_records_with_tenant_context(&collection_id, records, tenant_context.as_ref())
            .await
        {
            Ok(result) => {
                if result.success {
                    let n = result.vector_ids.len() as i64;
                    if let Some(dml_svc) = self.get_dml_service() {
                        dml_svc.bump_row_count_stats(&collection_name, n).await;
                    }
                }
                Ok(result)
            }
            Err(error) => {
                tracing::error!("Failed to process rich record batch: {:?}", error);
                Ok(BatchOperationResult::failure(
                    format!("Record insert failed: {}", error),
                    "RECORD_INSERT_FAILED".to_string(),
                ))
            }
        }
    }

    /// Canonical insert-only rich-record handler used when callers require existing
    /// records to be rejected instead of upserted (Arrow Flight `do_put`).
    pub async fn handle_record_insert_batch_for_tenant(
        &self,
        request: RichRecordBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<BatchOperationResult> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        let collection_id = match self
            .resolve_collection_id_internal(&request.collection_id, tenant_context.as_ref())
            .await?
        {
            Some(id) => id,
            None => {
                return Ok(BatchOperationResult::failure(
                    format!("Collection '{}' not found", request.collection_id),
                    "NOT_FOUND".to_string(),
                ));
            }
        };

        let collection_name = request.collection_id.clone();
        if let Some(dml_svc) = self.get_dml_service()
            && let Err(e) = dml_svc
                .validate_record_batch_against_schema(&collection_name, &request.records)
                .await
        {
            return Ok(BatchOperationResult::failure(
                format!("Schema validation failed: {}", e),
                "SCHEMA_VALIDATION_FAILED".to_string(),
            ));
        }
        if let Err(e) = enforce_wal_lane_for_record_batch(
            &collection_name,
            WriteOperationKind::Insert,
            request.records.len() as u64,
            "REST/gRPC handle_record_insert_batch_for_tenant",
        ) {
            return Ok(BatchOperationResult::failure(
                e,
                "WAL_LANE_REJECTED".to_string(),
            ));
        }
        self.ensure_collection_lease(tenant_id.unwrap_or(""), &collection_name)
            .await;
        let mut records = request.records;
        self.coerce_records_to_canonical_precision(&mut records, &request.collection_id)
            .await;
        match self
            .vector_operations_service
            .insert_records_only_with_tenant_context(
                &collection_id,
                records,
                tenant_context.as_ref(),
            )
            .await
        {
            Ok(result) => {
                if result.success {
                    let n = result.vector_ids.len() as i64;
                    if let Some(dml_svc) = self.get_dml_service() {
                        dml_svc.bump_row_count_stats(&collection_name, n).await;
                    }
                }
                Ok(result)
            }
            Err(error) => {
                tracing::error!(
                    "Failed to process insert-only rich record batch: {:?}",
                    error
                );
                Ok(BatchOperationResult::failure(
                    format!("Record insert failed: {}", error),
                    "RECORD_INSERT_FAILED".to_string(),
                ))
            }
        }
    }

    // ---- record READ path (TD-104 REST phase 2) -------------------------------
    // Moved verbatim from ROOT `UnifiedHandlers` so the REST layer reaches these
    // via `state.record_ops` instead of `state.request_handlers`. ROOT keeps thin
    // delegating inherent wrappers for its gRPC callers (see request_handlers.rs).
    // Behaviour-identical: the `self.<svc>` references resolve to the same Arcs
    // ROOT held (collection_service / vector_operations_service / dml_service),
    // and `resolve_collection_id_internal` is this service's own (ROOT already
    // delegates the cache here — one logical owner).

    /// Canonical rich-record search handler used by v2 REST/gRPC/internal callers.
    pub async fn handle_record_search_for_tenant(
        &self,
        request: RichSearchRequest,
        tenant_id: Option<&str>,
    ) -> Result<RichSearchResponse> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        let request = RichSearchRequest {
            collection_id: match self
                .resolve_collection_id_internal(&request.collection_id, tenant_context.as_ref())
                .await?
            {
                Some(id) => id,
                None => {
                    return Err(anyhow!("Collection '{}' not found", request.collection_id));
                }
            },
            ..request
        };

        self.vector_operations_service
            .search_records_with_tenant_context(request, tenant_context.as_ref())
            .await
    }

    /// Canonical rich-record get handler used by v2 REST/gRPC/internal callers.
    pub async fn handle_record_get_for_tenant(
        &self,
        request: RichRecordGetRequest,
        tenant_id: Option<&str>,
    ) -> Result<RichRecordGetResponse> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        let request = RichRecordGetRequest {
            collection_id: match self
                .resolve_collection_id_internal(&request.collection_id, tenant_context.as_ref())
                .await?
            {
                Some(id) => id,
                None => {
                    return Err(anyhow!("Collection '{}' not found", request.collection_id));
                }
            },
            ..request
        };

        self.vector_operations_service
            .get_record_with_tenant_context(request, tenant_context.as_ref())
            .await
    }

    /// Paginated rich-record scan (TD-099(3d) push-down): resolves tenant +
    /// collection id, then streams a single page from the deduped, time-ordered
    /// scan index and returns `(page, next_cursor)`.
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_record_scan_paginated_for_tenant(
        &self,
        collection_id: &str,
        cursor: Option<&ScanCursor>,
        limit: usize,
        include_vector: bool,
        include_props: bool,
        tenant_id: Option<&str>,
        filter: Option<&FilterExpression>,
        now_ns: i64,
    ) -> Result<(Vec<ProximaRecord>, Option<ScanCursor>)> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        let resolved_id = match self
            .resolve_collection_id_internal(collection_id, tenant_context.as_ref())
            .await?
        {
            Some(id) => id,
            None => {
                return Err(anyhow!("Collection '{}' not found", collection_id));
            }
        };

        self.vector_operations_service
            .scan_records_paginated(
                &resolved_id,
                cursor,
                limit,
                include_vector,
                include_props,
                tenant_context.as_ref(),
                filter,
                now_ns,
            )
            .await
    }

    /// Change feed: rows changed since `since_lsn` (delegates to the DML service).
    pub async fn table_changes(&self, table: &str, since_lsn: u64) -> Result<Vec<ChangeRow>> {
        match self.get_dml_service() {
            Some(dml) => dml.changes_since(table, since_lsn).await,
            None => Ok(Vec::new()),
        }
    }
}

// TD-104 S3-c: the canonical `RecordOpsPort` implementation now lives on the
// dedicated `RecordOpsService` so the Arrow Flight write path can hold an
// `Arc<dyn RecordOpsPort>` backed directly by this service — no longer routed
// through the ROOT `UnifiedHandlers`. ROOT's own `RecordOpsPort` impl delegates
// here, so behaviour is identical regardless of which surface is wired.
#[async_trait::async_trait]
impl proximadb_runtime::RecordOpsPort for RecordOpsService {
    async fn insert_record_batch(
        &self,
        collection_id: &str,
        records: Vec<proximadb_records::ProximaRecord>,
        tenant_id: Option<&str>,
    ) -> Result<proximadb_runtime::BatchOperationResult> {
        self.handle_record_insert_batch_for_tenant(
            RichRecordBatchRequest {
                collection_id: collection_id.to_string(),
                records,
            },
            tenant_id,
        )
        .await
    }

    async fn upsert_record_batch(
        &self,
        collection_id: &str,
        records: Vec<proximadb_records::ProximaRecord>,
        tenant_id: Option<&str>,
    ) -> Result<proximadb_runtime::BatchOperationResult> {
        self.handle_record_batch_for_tenant(
            RichRecordBatchRequest {
                collection_id: collection_id.to_string(),
                records,
            },
            tenant_id,
        )
        .await
    }

    async fn delete_record_batch(
        &self,
        collection_id: &str,
        record_ids: Vec<String>,
        tenant_id: Option<&str>,
    ) -> Result<proximadb_runtime::BatchOperationResult> {
        self.handle_record_delete_batch_for_tenant(
            RichRecordDeleteBatchRequest {
                collection_id: collection_id.to_string(),
                record_ids,
            },
            tenant_id,
        )
        .await
    }
}
