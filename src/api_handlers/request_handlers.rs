#![allow(clippy::doc_lazy_continuation)]
// cosmetic: newer clippy lint on pre-existing doc list-rendering; no functional impact
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

//! # Unified API Handlers - Proto-First Zero-Copy Architecture
//!
//! This module is the cornerstone of ProximaDB's API layer, implementing a unified handler system
//! that serves both REST and gRPC endpoints with zero code duplication and minimal overhead.
//!
//! ## Role in ProximaDB Architecture
//!
//! The unified handlers serve as the single point of business logic execution for all API operations:
//! - **Protocol Agnostic**: Same handler code serves both REST and gRPC requests
//! - **Zero-Copy Design**: Direct protocol buffer flow without intermediate conversions
//! - **Performance Optimized**: Eliminates registry overhead with direct service access
//! - **Type Safety**: Strong typing with protocol buffer definitions
//!
//! ## Key Design Principles
//!
//! 1. **Proto-First**: All data flows as protocol buffers (VectorRecord, Collection, etc.)
//! 2. **Single Implementation**: One handler method for each operation, used by all protocols
//! 3. **Direct Service Access**: Bypasses registries for 40-60% performance improvement
//! 4. **Async Throughout**: Full async/await support for non-blocking operations
//!
//! ## Integration Points
//!
//! ```text
//! REST Handler ─┐
//!               ├─→ UnifiedHandlers ─→ Services ─→ Storage/Index
//! gRPC Handler ─┘
//! ```
//!
//! - **Upstream**: Called by `network::rest::handlers` and `network::grpc::service`
//! - **Downstream**: Delegates to `CollectionService` and `VectorOperationsService`
//! - **Data Flow**: Protocol buffers flow directly through all layers
//!
//! ## Performance Characteristics
//!
//! - **Latency**: Sub-millisecond overhead for handler routing
//! - **Throughput**: 100K+ ops/sec for vector operations
//! - **Memory**: Zero intermediate allocations with proto-first design
//! - **Concurrency**: Lock-free operation with Arc-based sharing

use anyhow::{Context, Result, anyhow};
use proximadb_graph_query::service::GraphExecutionService;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, info_span};

/// Global request counter for generating unique request IDs
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique request ID combining timestamp and counter
/// Format: hex timestamp (8 chars) + hex counter (8 chars) = 16 char ID
fn generate_request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0);
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed) as u32;
    format!("{:08x}{:08x}", timestamp, counter)
}

/// Default TTL for collection ID cache entries (5 minutes)
const COLLECTION_ID_CACHE_TTL_SECS: u64 = 300;

/// Maximum number of entries in the collection ID cache
const COLLECTION_ID_CACHE_MAX_SIZE: usize = 1000;

/// Cache entry for collection ID resolution
#[derive(Clone)]
struct CollectionIdCacheEntry {
    collection_id: String,
    cached_at: Instant,
}

/// Thread-safe TTL-based cache for collection ID resolution
///
/// Reduces latency from ~5ms/request (metadata backend lookup) to ~0.1ms (cache hit).
/// Uses a simple HashMap with RwLock for concurrent access.
pub struct CollectionIdCache {
    cache: std::sync::RwLock<HashMap<String, CollectionIdCacheEntry>>,
    ttl: Duration,
    max_size: usize,
}

impl CollectionIdCache {
    /// Create a new cache with default TTL and max size
    pub fn new() -> Self {
        Self {
            cache: std::sync::RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(COLLECTION_ID_CACHE_TTL_SECS),
            max_size: COLLECTION_ID_CACHE_MAX_SIZE,
        }
    }

    /// Create a new cache with custom TTL
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: std::sync::RwLock::new(HashMap::new()),
            ttl,
            max_size: COLLECTION_ID_CACHE_MAX_SIZE,
        }
    }

    /// Get a cached collection ID if it exists and is not expired
    pub fn get(&self, identifier: &str) -> Option<String> {
        let cache = self.cache.read().ok()?;
        if let Some(entry) = cache.get(identifier)
            && entry.cached_at.elapsed() < self.ttl
        {
            debug!(
                "Collection ID cache hit: '{}' -> '{}'",
                identifier, entry.collection_id
            );
            return Some(entry.collection_id.clone());
        }
        None
    }

    /// Insert a collection ID into the cache
    pub fn insert(&self, identifier: String, collection_id: String) {
        if let Ok(mut cache) = self.cache.write() {
            // Evict expired entries if cache is too large
            if cache.len() >= self.max_size {
                self.evict_expired(&mut cache);
            }

            // If still too large after eviction, remove oldest entries
            if cache.len() >= self.max_size {
                // Simple eviction: clear half the cache
                let keys_to_remove: Vec<_> = cache
                    .iter()
                    .take(cache.len() / 2)
                    .map(|(k, _)| k.clone())
                    .collect();
                for key in keys_to_remove {
                    cache.remove(&key);
                }
            }

            cache.insert(
                identifier,
                CollectionIdCacheEntry {
                    collection_id,
                    cached_at: Instant::now(),
                },
            );
        }
    }

    /// Invalidate a specific cache entry (call on collection delete/update)
    pub fn invalidate(&self, identifier: &str) {
        if let Ok(mut cache) = self.cache.write() {
            cache.remove(identifier);
            // Also remove any entries that might have the collection_id as the identifier
            // (since resolve_collection_id accepts both name and id)
            let keys_to_remove: Vec<_> = cache
                .iter()
                .filter(|(_, entry)| entry.collection_id == identifier)
                .map(|(k, _)| k.clone())
                .collect();
            for key in keys_to_remove {
                cache.remove(&key);
            }
        }
    }

    /// Evict expired entries from the cache
    fn evict_expired(&self, cache: &mut HashMap<String, CollectionIdCacheEntry>) {
        let keys_to_remove: Vec<_> = cache
            .iter()
            .filter(|(_, entry)| entry.cached_at.elapsed() >= self.ttl)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys_to_remove {
            cache.remove(&key);
        }
    }

    /// Clear the entire cache
    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
    }
}

impl Default for CollectionIdCache {
    fn default() -> Self {
        Self::new()
    }
}

// Import metrics service
use crate::metrics::query_service::{MetricsQueryOptions, MetricsQueryService};

use crate::observability::ObservabilityService;
use crate::proto::proximadb_v1::{
    Collection, CollectionOperation, CollectionRequest, CollectionResponse,
};
use crate::query::QueryFacadeAdapter;
use crate::services::DmlService;
use crate::services::collection::manager::CollectionService;
use crate::services::operations::BatchOperationResult;
use crate::services::operations::vectors::{
    RichRecordBatchRequest, RichRecordDeleteBatchRequest, RichRecordGetRequest,
    RichRecordGetResponse, RichSearchRequest, RichSearchResponse, VectorOperationsService,
};
use crate::services::{
    WriteDurabilityRequirement, WriteIntent, WriteLaneRouter, WriteOperationKind,
};
use crate::storage::document::DocumentService;

/// Unified handlers that implement all business logic for API operations
///
/// **Performance Enhancement**: Uses optimized VectorOperationsService for 40-60% faster vector operations
pub struct UnifiedHandlers {
    /// Collection CRUD service for create/list/delete/stats.
    pub collection_service: Arc<CollectionService>,
    /// Optimized vector service with eliminated registry overhead
    pub vector_operations_service: Arc<VectorOperationsService>,
    /// Document storage and retrieval service
    pub document_service: Arc<DocumentService>,
    /// Observability service for logs, metrics, and traces
    pub observability_service: Arc<ObservabilityService>,
    /// Event log engine for persistent audit trails (TD-050)
    pub event_log: Option<Arc<crate::storage::engines::eventlog::EventLogEngine>>,
    /// Graph collection service for metadata management
    pub graph_collection_service: Arc<crate::services::GraphCollectionService>,
    /// Concrete graph operations service for native graph API operations
    pub graph_operations_service: Arc<crate::graph::GraphOperationsService>,
    /// Extracted graph execution capability for planners/executors
    pub graph_execution_service: Arc<dyn GraphExecutionService>,
    /// Metrics query service for collection statistics and optimization hints
    pub metrics_query_service: Option<Arc<MetricsQueryService>>,
    /// Optional hybrid runtime configuration (weights, seeding). Thread-safe.
    pub hybrid_runtime:
        std::sync::Arc<std::sync::RwLock<Option<crate::core::config::HybridRuntimeConfig>>>,
    /// Query facade adapter for unified query execution
    /// Optional for backward compatibility during feature flag transition
    /// When set, SQL queries route through the unified facade for consistent routing and metrics
    /// Uses RwLock for thread-safe post-initialization setting (similar to hybrid_runtime)
    query_adapter: std::sync::RwLock<Option<Arc<QueryFacadeAdapter>>>,
    /// Canonical record-batch orchestration service (TD-104 S3-c).
    ///
    /// Owns the record insert/upsert/delete orchestration AND the shared
    /// resolution state that ROOT's other paths reuse: the collection-ID
    /// cache, the post-construction `DmlService` handle (schema validation +
    /// row-count stats and EXPLAIN/DML routing), and the set-once
    /// `CanonicalPrecisionResolver`. ROOT delegates its inherent
    /// record-batch handlers, its `RecordOpsPort` impl, and the
    /// resolution/cache/setter helpers here so there is ONE logical owner of
    /// this state and no duplicated logic (Convergence Gate). The Arrow Flight
    /// write path holds this same service as its `RecordOpsPort` directly, so
    /// it no longer routes through ROOT.
    record_ops: Arc<crate::api_handlers::record_ops_service::RecordOpsService>,
}

impl UnifiedHandlers {
    /// Create new unified handlers with optimized VectorOperationsService
    ///
    /// **CRITICAL: Graph Services Must Be Provided**
    ///
    /// This constructor now requires pre-created GraphCollectionService and GraphOperationsService
    /// to ensure SINGLE SHARED INSTANCE across all API endpoints (REST, gRPC).
    ///
    /// **Performance Benefits:**
    /// - 40-60% faster vector insert operations
    /// - Eliminates WAL Manager Registry overhead
    /// - Direct access to global memtable
    ///
    /// **Graph API Bug Fix:**
    /// - Ensures graph collections are visible to all operations
    /// - Single source of truth for graph metadata
    ///
    /// The extracted `GraphExecutionService` view is derived from that same shared
    /// graph service for query planning/execution paths.
    pub fn new(
        collection_service: Arc<CollectionService>,
        vector_operations_service: Arc<VectorOperationsService>,
        document_service: Arc<DocumentService>,
        observability_service: Arc<ObservabilityService>,
        event_log: Option<Arc<crate::storage::engines::eventlog::EventLogEngine>>,
        graph_collection_service: Arc<crate::services::GraphCollectionService>,
        graph_operations_service: Arc<crate::graph::GraphOperationsService>,
    ) -> Self {
        let graph_execution_service: Arc<dyn GraphExecutionService> =
            graph_operations_service.clone();
        let record_ops = Arc::new(
            crate::api_handlers::record_ops_service::RecordOpsService::new(
                collection_service.clone(),
                vector_operations_service.clone(),
            ),
        );
        Self {
            collection_service,
            vector_operations_service,
            document_service,
            observability_service,
            event_log,
            graph_collection_service,
            graph_operations_service,
            graph_execution_service,
            metrics_query_service: None,
            hybrid_runtime: std::sync::Arc::new(std::sync::RwLock::new(None)),
            query_adapter: std::sync::RwLock::new(None),
            record_ops,
        }
    }

    /// Create new unified handlers with metrics support
    pub fn with_metrics(
        collection_service: Arc<CollectionService>,
        vector_operations_service: Arc<VectorOperationsService>,
        document_service: Arc<DocumentService>,
        observability_service: Arc<ObservabilityService>,
        event_log: Option<Arc<crate::storage::engines::eventlog::EventLogEngine>>,
        graph_collection_service: Arc<crate::services::GraphCollectionService>,
        graph_operations_service: Arc<crate::graph::GraphOperationsService>,
        metrics_query_service: Arc<MetricsQueryService>,
    ) -> Self {
        let graph_execution_service: Arc<dyn GraphExecutionService> =
            graph_operations_service.clone();
        let record_ops = Arc::new(
            crate::api_handlers::record_ops_service::RecordOpsService::new(
                collection_service.clone(),
                vector_operations_service.clone(),
            ),
        );
        Self {
            collection_service,
            vector_operations_service,
            document_service,
            observability_service,
            event_log,
            graph_collection_service,
            graph_operations_service,
            graph_execution_service,
            metrics_query_service: Some(metrics_query_service),
            hybrid_runtime: std::sync::Arc::new(std::sync::RwLock::new(None)),
            query_adapter: std::sync::RwLock::new(None),
            record_ops,
        }
    }

    /// Set the query facade adapter for query routing (thread-safe; callable post-initialization)
    ///
    /// When set, SQL queries will be routed through the unified facade for:
    /// - Consistent query metrics across all query types
    /// - Unified strategy selection (SQL, federated, etc.)
    /// - Centralized query logging and tracing
    pub fn set_query_adapter(&self, adapter: Arc<QueryFacadeAdapter>) {
        if let Ok(mut guard) = self.query_adapter.write() {
            *guard = Some(adapter);
            tracing::info!("QueryFacadeAdapter set on UnifiedHandlers for unified SQL routing");
        }
    }

    /// Get the query facade adapter if set
    pub fn get_query_adapter(&self) -> Option<Arc<QueryFacadeAdapter>> {
        self.query_adapter
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Canonical record-batch orchestration service shared with the Arrow
    /// Flight write path (injected as its `RecordOpsPort`). TD-104 S3-c.
    pub fn record_ops(&self) -> Arc<crate::api_handlers::record_ops_service::RecordOpsService> {
        self.record_ops.clone()
    }

    /// Wire a `DmlService` for EXPLAIN routing through the gRPC `ExecuteQuery` RPC.
    /// Callable post-initialization; thread-safe. Delegated to the shared
    /// `RecordOpsService` so the record-batch path (REST/gRPC/Arrow Flight) and
    /// ROOT's EXPLAIN/DML routing observe the same handle (TD-104 S3-c).
    pub fn set_dml_service(&self, svc: Arc<DmlService>) {
        self.record_ops.set_dml_service(svc);
        tracing::info!("DmlService set on UnifiedHandlers for EXPLAIN routing");
    }

    fn get_dml_service(&self) -> Option<Arc<DmlService>> {
        self.record_ops.get_dml_service()
    }

    /// Wire a `DdlService` so relational DDL (CREATE/ALTER/DROP) submitted over the
    /// gRPC `ExecuteQuery` RPC executes tenant-scoped (TD-135), mirroring pgwire.
    /// Callable post-initialization; thread-safe. Delegated to `RecordOpsService`.
    pub fn set_ddl_service(&self, svc: Arc<crate::services::DdlService>) {
        self.record_ops.set_ddl_service(svc);
        tracing::info!("DdlService set on UnifiedHandlers for relational DDL routing");
    }

    fn get_ddl_service(&self) -> Option<Arc<crate::services::DdlService>> {
        self.record_ops.get_ddl_service()
    }

    /// CDC change-feed: row-level changes for `table` with WAL sequence number strictly
    /// greater than `since_lsn`, oldest first. Backed by the unified canonical `DmlService`
    /// (the single cross-surface record store), so it reflects writes from EVERY protocol
    /// — REST/gRPC/pgwire. Returns empty when no DmlService is wired.
    pub async fn table_changes(
        &self,
        table: &str,
        since_lsn: u64,
    ) -> anyhow::Result<Vec<crate::services::record_store::ChangeRow>> {
        match self.get_dml_service() {
            Some(dml) => dml.changes_since(table, since_lsn).await,
            None => Ok(Vec::new()),
        }
    }

    /// Post-construction setter for the canonical-precision resolver.
    /// Called once at server bootstrap (`ProximaDB::new` in
    /// `src/database.rs`) so the v1 vector-batch path can coerce
    /// records to each collection's canonical precision before WAL
    /// append. Without this wire-up the REST insert path bypasses
    /// the precision-coercion that the queue-drainer ingest path
    /// gets via BulkLoadDrainerSink + the resolver — fp16
    /// collections receiving REST inserts would have their records
    /// stored as fp32.
    pub fn set_precision_resolver(
        &self,
        resolver: Arc<proximadb_catalog::canonical_precision::CanonicalPrecisionResolver>,
    ) {
        // Delegated to the shared `RecordOpsService` so the record-batch path
        // and ROOT's v1 vector-batch coercion observe the same resolver
        // (TD-080/TD-082, TD-104 S3-c).
        self.record_ops.set_precision_resolver(resolver);
        tracing::info!(
            "✅ CanonicalPrecisionResolver wired into UnifiedHandlers — \
             REST/gRPC inserts into non-fp32 collections will coerce records"
        );
    }

    /// Coerce every embedding cell on every record to the collection's
    /// canonical precision (resolved via xCatalog) **in place**.
    ///
    /// This is the single shared implementation of the v2 precision-coercion
    /// contract. Both `handle_record_batch_for_tenant` (upsert path, used by
    /// REST `/api/v2/.../records/batch`, gRPC v2, REST v3 docs) and
    /// `handle_record_insert_batch_for_tenant` (insert-only path, used by
    /// Arrow Flight `do_put`) must call this method so the two handlers
    /// cannot silently diverge again. The first divergence — Arrow Flight
    /// keeping records at fp32 input precision for fp16 collections — was
    /// closed in the 2026-05-28 round-2 reconciliation (commit `a446b3964`)
    /// and tracked as TD-082. Folding the duplicate into a shared helper is
    /// the TD-082 closure: parity is now enforced by code-sharing, not by a
    /// test that could later be deleted.
    ///
    /// Behaviour:
    /// * No-op when the resolver isn't wired (embedded test harness without
    ///   a default catalog — TD-080).
    /// * No-op when the resolved target precision is `Fp32` — input was
    ///   already canonical fp32 off the wire.
    /// * On resolver error, log a `warn` and keep records at input precision
    ///   (matches the pre-helper behaviour; cataloging mismatches surface
    ///   via the per-precision canonical_bytes metric).
    /// Delegates to the shared `RecordOpsService` (single source of truth for
    /// the v2 precision-coercion contract). ROOT's v1 vector-batch path still
    /// calls this; the record-batch handlers now call the service directly.
    /// TD-080/TD-082, TD-104 S3-c.
    async fn coerce_records_to_canonical_precision(
        &self,
        records: &mut [proximadb_records::ProximaRecord],
        collection_id: &str,
    ) {
        self.record_ops
            .coerce_records_to_canonical_precision(records, collection_id)
            .await
    }

    /// Set hybrid runtime configuration (thread-safe; callable post-initialization)
    pub fn set_hybrid_runtime(&self, cfg: crate::core::config::HybridRuntimeConfig) {
        if let Ok(mut guard) = self.hybrid_runtime.write() {
            *guard = Some(cfg);
        }
    }

    /// Get storage configuration from collection service
    ///
    /// Returns the storage configuration containing storage locations.
    /// Used by Arrow Flight service to locate .arrow files.
    pub fn storage_config(&self) -> Option<&crate::core::config::StorageConfig> {
        Some(self.collection_service.storage_config())
    }

    /// Resolve collection identifier to canonical ID with caching
    ///
    /// Uses TTL-based cache to reduce metadata backend lookups from ~5ms to ~0.1ms on cache hits.
    /// Cache is automatically invalidated on collection delete/update operations.
    ///
    /// # Arguments
    /// * `identifier` - Collection name or ID
    ///
    /// # Returns
    /// * `Ok(Some(id))` - Resolved collection ID
    /// * `Ok(None)` - Collection not found
    /// * `Err(_)` - Resolution failed
    /// Delegates to the shared `RecordOpsService` cache (single owner — TD-104 S3-c).
    pub async fn resolve_collection_id_cached(&self, identifier: &str) -> Result<Option<String>> {
        self.record_ops
            .resolve_collection_id_cached(identifier)
            .await
    }

    /// Invalidate collection ID cache entry
    ///
    /// Call this when a collection is deleted or renamed to ensure
    /// stale cache entries don't cause issues. Delegated to the shared
    /// `RecordOpsService` cache so the record-batch path sees invalidations.
    pub fn invalidate_collection_cache(&self, identifier: &str) {
        self.record_ops.invalidate_collection_cache(identifier);
    }

    /// Clear the entire collection ID cache
    ///
    /// Use this during testing or when a bulk cache invalidation is needed.
    pub fn clear_collection_cache(&self) {
        self.record_ops.clear_collection_cache();
    }

    /// Handle any collection operation with unified logic
    pub async fn handle_collection_operation(
        &self,
        request: CollectionRequest,
    ) -> Result<CollectionResponse> {
        self.handle_collection_operation_internal(request, None)
            .await
    }

    /// Handle collection operations with tenant-scoped access control.
    pub async fn handle_collection_operation_for_tenant(
        &self,
        request: CollectionRequest,
        tenant_id: Option<&str>,
    ) -> Result<CollectionResponse> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        self.handle_collection_operation_internal(request, tenant_context.as_ref())
            .await
    }

    async fn handle_collection_operation_internal(
        &self,
        request: CollectionRequest,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<CollectionResponse> {
        let request_id = generate_request_id();
        let start_time = std::time::Instant::now();

        let operation = CollectionOperation::try_from(request.operation)
            .context("Invalid collection operation")?;

        // Create tracing span for observability
        let span = info_span!(
            "collection_operation",
            request_id = %request_id,
            operation = ?operation,
        );
        let _guard = span.enter();

        let (success, collection, collections_opt, affected_count, _error_msg, error_code) =
            match operation {
                CollectionOperation::CollectionCreate => {
                    self.handle_create_collection(request, tenant_context)
                        .await?
                }
                CollectionOperation::CollectionGet => {
                    self.handle_collection(request, tenant_context).await?
                }
                CollectionOperation::CollectionList => {
                    self.handle_list_collections(request, tenant_context)
                        .await?
                }
                CollectionOperation::CollectionUpdate => {
                    self.handle_update_collection(request, tenant_context)
                        .await?
                }
                CollectionOperation::CollectionDelete => {
                    self.handle_delete_collection(request, tenant_context)
                        .await?
                }
                _ => {
                    return Ok(CollectionResponse {
                        success: false,
                        operation: operation as i32,
                        collection: None,
                        collections: vec![],
                        affected_count: 0,
                        total_count: 0,
                        metadata: Default::default(),
                        error_message: None,
                        error_code: Some("UNSUPPORTED_OPERATION".to_string()),
                        processing_time_us: start_time.elapsed().as_micros() as i64,
                    });
                }
            };

        let collections = collections_opt.clone();
        let total_count = collections.as_ref().map(|c| c.len() as i64);

        Ok(CollectionResponse {
            success,
            operation: operation as i32,
            collection,
            collections: collections.unwrap_or_else(Vec::new),
            affected_count,
            total_count: total_count.unwrap_or(0),
            metadata: Default::default(),
            error_message: None,
            error_code,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    // /// Handle vector batch operations with unified logic
    // ///
    // /// **OPTIMIZED**: Uses VectorOperationsService when available for 40-60% performance improvement
    // /// ✅ DUAL COLLECTION RESOLUTION: Supports both collection name and ID
    // Note: Non-v1 batch handler removed. Use handle_vector_batch_v1 directly.

    // Optimized non-v1 batch path removed. Use handle_vector_batch_v1.

    // /// Handle vector search operations with unified logic
    // Note: Non-v1 search handler removed. Use handle_vector_search_v1 directly.

    /// Resolve a collection name or UUID to its canonical UUID, scoping to tenant if provided.
    pub async fn resolve_collection_id_for_tenant(
        &self,
        collection_identifier: &str,
        tenant_id: Option<&str>,
    ) -> Result<Option<String>> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        self.resolve_collection_id_internal(collection_identifier, tenant_context.as_ref())
            .await
    }

    async fn resolve_collection_id_internal(
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

    /// v1 wrapper: accept v1::VectorSearchRequest and return v1 response using v1 builders
    pub async fn handle_vector_search_v1_for_tenant(
        &self,
        request: crate::proto::proximadb_v1::VectorSearchRequest,
        tenant_id: Option<&str>,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        self.handle_vector_search_v1_internal(request, tenant_context.as_ref())
            .await
    }

    /// Execute a v1 vector similarity search request.
    pub async fn handle_vector_search_v1(
        &self,
        request: crate::proto::proximadb_v1::VectorSearchRequest,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        self.handle_vector_search_v1_internal(request, None).await
    }

    async fn handle_vector_search_v1_internal(
        &self,
        request: crate::proto::proximadb_v1::VectorSearchRequest,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        let request_id = generate_request_id();
        let start_time = std::time::Instant::now();

        // Create tracing span for observability
        let span = info_span!(
            "vector_search",
            request_id = %request_id,
            collection_id = %request.collection_id,
            top_k = %request.top_k,
            query_count = %request.queries.len(),
        );
        let _guard = span.enter();

        // Resolve collection name/ID to canonical ID (with caching)
        let collection_identifier = &request.collection_id;
        let collection_id: String = match self
            .resolve_collection_id_internal(collection_identifier, tenant_context)
            .await?
        {
            Some(id) => id,
            None => {
                return Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success: false,
                    operation: crate::proto::proximadb_v1::VectorServiceOperation::VsSearch as i32,
                    metrics: None,
                    results: None,
                    vector_ids: vec![],
                    error_message: None,
                    error_code: Some("NOT_FOUND".to_string()),
                });
            }
        };

        // Execute through the canonical v1 service path so filters/search semantics
        // stay consistent across handlers and facade adapters.
        let mut canonical_request = request;
        canonical_request.collection_id = collection_id;

        let mut response = self
            .vector_operations_service
            .search_v1_with_tenant_context(canonical_request, tenant_context)
            .await?;

        if let Some(metrics) = response.metrics.as_mut() {
            metrics.processing_time_us = start_time.elapsed().as_micros() as i64;
        }

        Ok(response)
    }

    /// Execute hybrid search (BM25 full-text + Vector similarity) with parallel execution
    ///
    /// This method combines BM25 keyword search with vector similarity search
    /// using configurable fusion strategies (RRF, Weighted Linear, etc.).
    ///
    /// # Arguments
    /// * `collection_id` - Collection to search
    /// * `text_query` - Full-text search query for BM25
    /// * `query_vector` - Vector similarity query
    /// * `top_k` - Number of results to return
    /// * `fusion_strategy` - Strategy for combining results (RRF, WeightedLinear, RBP, etc.)
    /// * `filters` - Optional metadata filters
    ///
    /// # Returns
    /// Fused and ranked search results
    ///
    /// # Example
    /// ```ignore
    /// let results = handler.execute_hybrid_search(
    ///     "my_collection",
    ///     "machine learning algorithms",
    ///     vec![0.1, 0.2, 0.3],
    ///     10,
    ///     FusionStrategy::ReciprocalRank { k: 60 },
    ///     None,
    /// ).await?;
    /// ```
    pub async fn execute_hybrid_search(
        &self,
        collection_id: &str,
        text_query: &str,
        query_vector: &[f32],
        top_k: usize,
        fusion_strategy: crate::core::search::hybrid::FusionStrategy,
        _filters: Option<crate::core::search::FilterExpression>,
    ) -> anyhow::Result<Vec<crate::core::search::hybrid::FusedSearchResult>> {
        use crate::core::search::hybrid::{BM25Result, HybridCoordinator, VectorResult};

        let coordinator = HybridCoordinator::new(fusion_strategy);

        // BM25 search is temporarily disabled here until the document service is
        // explicitly plumbed into UnifiedHandlers. Keep the hybrid API callable
        // and let the fusion engine operate on the live vector branch.
        let bm25_search =
            |_query: String| async move { Ok::<Vec<BM25Result>, anyhow::Error>(Vec::new()) };

        // Vector search via the vector operations service
        let vec_service = self.vector_operations_service.clone();
        let coll_id_vec = collection_id.to_string();
        let k_vec = top_k;
        let vector_search = |vector: Vec<f32>| async move {
            let request = crate::proto::proximadb_v1::VectorSearchRequest {
                collection_id: coll_id_vec,
                queries: vec![crate::proto::proximadb_v1::SearchQuery {
                    vector,
                    filters: HashMap::new(),
                    advanced_filter: None,
                }],
                top_k: k_vec as u32,
                include_fields: Some(crate::proto::proximadb_v1::IncludeFields {
                    vector: false,
                    metadata: true,
                    score: true,
                    rank: false,
                    source: false,
                    source_options: HashMap::new(),
                }),
                search_params: None,
                distance_metric_override: None,
                search_optimization: None,
            };

            match vec_service.search_v1(request).await {
                Ok(results) => {
                    let vec_results: Vec<VectorResult> = results
                        .results
                        .unwrap_or_default()
                        .results
                        .into_iter()
                        .map(|r| VectorResult {
                            doc_id: r.id,
                            score: r.similarity.unwrap_or(r.score as f32) as f64,
                            distance: 1.0 - r.similarity.unwrap_or(r.score as f32) as f64,
                            metadata: crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(
                                &r.metadata,
                            ),
                        })
                        .collect();
                    Ok(vec_results)
                }
                Err(_) => Ok::<Vec<VectorResult>, anyhow::Error>(vec![]),
            }
        };

        let fused_results = coordinator
            .execute_hybrid_search(bm25_search, vector_search, text_query, query_vector)
            .await
            .map_err(|e| anyhow::anyhow!("Hybrid search fusion error: {}", e))?;

        Ok(fused_results)
    }

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

    /// Canonical rich-record scan handler used by v2 REST + connector
    /// callers (Hadoop / Spark / Trino) to drain a collection without
    /// the similarity bias of `searchRecords`.
    ///
    /// Returns visible canonical records from the WAL/memtable path
    /// (matches the rest of the v2 single-record surface today; the
    /// storage-inclusive scan is a separate API on
    /// [`VectorOperationsService::list_all_records_with_tenant_context`]).
    /// `cursor` is reserved for opaque pagination but not honored here
    /// yet — TD-099 acceptance (3); callers should bump `limit` to
    /// retrieve more rows in one round trip.
    pub async fn handle_record_scan_for_tenant(
        &self,
        collection_id: &str,
        limit: Option<usize>,
        include_vector: bool,
        include_props: bool,
        tenant_id: Option<&str>,
    ) -> Result<Vec<proximadb_records::ProximaRecord>> {
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
            .scan_records_with_tenant_context(
                &resolved_id,
                limit,
                include_vector,
                include_props,
                tenant_context.as_ref(),
            )
            .await
    }

    /// Paginated variant of [`Self::handle_record_scan_for_tenant`] (TD-099(3d)
    /// push-down): resolves tenant + collection id, then streams a single page
    /// from the deduped, time-ordered scan index and returns `(page,
    /// next_cursor)`.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_record_scan_paginated_for_tenant(
        &self,
        collection_id: &str,
        cursor: Option<&crate::services::scan_cursor::ScanCursor>,
        limit: usize,
        include_vector: bool,
        include_props: bool,
        tenant_id: Option<&str>,
        filter: Option<&crate::core::search::FilterExpression>,
        now_ns: i64,
    ) -> Result<(
        Vec<proximadb_records::ProximaRecord>,
        Option<crate::services::scan_cursor::ScanCursor>,
    )> {
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

    /// Canonical rich-record delete handler used by v2 REST/gRPC/internal callers.
    ///
    /// Delegates to the shared `RecordOpsService` (TD-104 S3-c) — no duplicated
    /// orchestration logic. Kept as a thin inherent wrapper so existing ROOT
    /// callers compile unchanged.
    pub async fn handle_record_delete_batch_for_tenant(
        &self,
        request: RichRecordDeleteBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<crate::services::operations::BatchOperationResult> {
        self.record_ops
            .handle_record_delete_batch_for_tenant(request, tenant_id)
            .await
    }

    /// Canonical rich-record batch (upsert) handler used by v2 REST/gRPC/internal callers.
    /// Delegates to the shared `RecordOpsService` (TD-104 S3-c).
    pub async fn handle_record_batch_for_tenant(
        &self,
        request: RichRecordBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<BatchOperationResult> {
        self.record_ops
            .handle_record_batch_for_tenant(request, tenant_id)
            .await
    }

    /// Canonical insert-only rich-record handler (Arrow Flight `do_put`).
    /// Delegates to the shared `RecordOpsService` (TD-104 S3-c).
    pub async fn handle_record_insert_batch_for_tenant(
        &self,
        request: RichRecordBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<BatchOperationResult> {
        self.record_ops
            .handle_record_insert_batch_for_tenant(request, tenant_id)
            .await
    }

    /// v1 native: accept v1::VectorBatchRequest, delegate to v1 services, and return v1 response
    ///
    /// REFACTORED: Now uses clean typed insert_batch() instead of JSON serialization
    pub async fn handle_vector_batch_v1_for_tenant(
        &self,
        request: crate::proto::proximadb_v1::VectorBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        self.handle_vector_batch_v1_internal(request, tenant_context.as_ref())
            .await
    }

    /// Execute a v1 vector batch upsert/delete request.
    pub async fn handle_vector_batch_v1(
        &self,
        request: crate::proto::proximadb_v1::VectorBatchRequest,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        self.handle_vector_batch_v1_internal(request, None).await
    }

    async fn handle_vector_batch_v1_internal(
        &self,
        request: crate::proto::proximadb_v1::VectorBatchRequest,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        let request_id = generate_request_id();
        let vector_count = request.vectors.len();

        // Create tracing span for observability
        let span = info_span!(
            "vector_batch",
            request_id = %request_id,
            collection_id = %request.collection_id,
            vector_count = %vector_count,
        );
        let _guard = span.enter();

        let collection_identifier = &request.collection_id;

        // Resolve to canonical collection ID (with caching)
        let collection_id: String = match self
            .resolve_collection_id_internal(collection_identifier, tenant_context)
            .await?
        {
            Some(id) => id,
            None => {
                return Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success: false,
                    operation: crate::proto::proximadb_v1::VectorServiceOperation::VsBatch as i32,
                    metrics: None,
                    results: None,
                    vector_ids: vec![],
                    error_message: None,
                    error_code: Some("NOT_FOUND".to_string()),
                });
            }
        };

        // Convert v1 wire VectorRecord → ProximaRecord at the protocol boundary.
        let mut records: Vec<proximadb_records::ProximaRecord> = request
            .vectors
            .into_iter()
            .map(crate::proto::defaults::vector_record_to_proxima_record)
            .collect();

        // Coerce embeddings through the shared RecordOpsService-owned resolver.
        // This keeps the v1 vector batch path aligned with v2 record batches
        // and Arrow Flight writes after the TD-104 S3-c extraction.
        self.coerce_records_to_canonical_precision(&mut records, collection_identifier)
            .await;

        match self
            .vector_operations_service
            .insert_batch_with_tenant_context(&collection_id, records, tenant_context)
            .await
        {
            Ok(result) => {
                // Convert typed result to proto response - simple, no JSON parsing!
                Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success: result.success,
                    operation: crate::proto::proximadb_v1::VectorServiceOperation::VsBatch as i32,
                    metrics: Some(crate::proto::proximadb_v1::OperationMetrics {
                        total_processed: result.metrics.total_processed,
                        successful_count: result.metrics.successful_count,
                        failed_count: result.metrics.failed_count,
                        updated_count: result.metrics.updated_count,
                        processing_time_us: result.metrics.processing_time_us,
                        wal_write_time_us: result.metrics.wal_write_time_us,
                        index_update_time_us: result.metrics.index_update_time_us,
                    }),
                    results: None,
                    vector_ids: result.vector_ids,
                    error_message: if result.errors.is_empty() {
                        None
                    } else {
                        Some(result.errors.join("; "))
                    },
                    error_code: result.error_code,
                })
            }
            Err(e) => {
                tracing::error!("Failed to process vector batch: {:?}", e);
                Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success: false,
                    operation: crate::proto::proximadb_v1::VectorServiceOperation::VsBatch as i32,
                    metrics: Some(crate::proto::proximadb_v1::OperationMetrics {
                        total_processed: 0,
                        successful_count: 0,
                        failed_count: 0,
                        updated_count: 0,
                        processing_time_us: 0,
                        wal_write_time_us: 0,
                        index_update_time_us: 0,
                    }),
                    results: None,
                    vector_ids: vec![],
                    error_message: Some(format!("Vector insert failed: {}", e)),
                    error_code: Some("VECTOR_INSERT_FAILED".to_string()),
                })
            }
        }
    }

    /// v1 wrapper for VectorGet → returns v1 response
    pub async fn handle_vector_v1_for_tenant(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
        tenant_id: Option<&str>,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        let tenant_context = self.collection_service.load_tenant_context(tenant_id)?;
        self.handle_vector_v1_internal(
            collection_id,
            vector_id,
            include_vector,
            include_metadata,
            tenant_context.as_ref(),
        )
        .await
    }

    /// Handle a single-vector v1 operation (get, insert, update, delete).
    pub async fn handle_vector_v1(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        self.handle_vector_v1_internal(
            collection_id,
            vector_id,
            include_vector,
            include_metadata,
            None,
        )
        .await
    }

    async fn handle_vector_v1_internal(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        let start_time = std::time::Instant::now();

        // Resolve canonical collection ID
        let resolved_collection_id: String = match self
            .resolve_collection_id_internal(collection_id, tenant_context)
            .await?
        {
            Some(id) => id,
            None => {
                return Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success: false,
                    operation: crate::proto::proximadb_v1::VectorServiceOperation::VsGet as i32,
                    metrics: None,
                    results: None,
                    vector_ids: vec![],
                    error_message: None,
                    error_code: Some("NOT_FOUND".to_string()),
                });
            }
        };

        match self
            .vector_operations_service
            .vector(
                &resolved_collection_id,
                vector_id,
                include_vector,
                include_metadata,
            )
            .await
        {
            Ok(Some(vector_record)) => {
                let vector_record =
                    proximadb_records::conversions::proxima_record_to_vector(&vector_record);
                let rec = crate::proto::proximadb_v1::SearchVectorRecord {
                    id: if vector_record.id.is_empty() {
                        "unknown".to_string()
                    } else {
                        vector_record.id
                    },
                    score: 1.0,
                    vector: vector_record.vector,
                    metadata: vector_record.metadata,
                    version: vector_record.version,
                    engine_stats: std::collections::HashMap::new(),
                    expanded_context: Vec::new(),
                    index_path: None,
                    timestamp: None,
                    source: None,
                    similarity: None,
                    semantic_similarity: None,
                    quantization_info: None,
                };

                Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success: true,
                    operation: crate::proto::proximadb_v1::VectorServiceOperation::VsGet as i32,
                    metrics: Some(crate::proto::proximadb_v1::OperationMetrics {
                        total_processed: 1,
                        successful_count: 1,
                        failed_count: 0,
                        updated_count: 0,
                        processing_time_us: start_time.elapsed().as_micros() as i64,
                        wal_write_time_us: 0,
                        index_update_time_us: 0,
                    }),
                    results: Some(crate::proto::proximadb_v1::SearchResult {
                        results: vec![rec],
                        total_found: 1,
                        collection_id: Some(collection_id.to_string()),
                    }),
                    vector_ids: vec![vector_id.to_string()],
                    error_message: None,
                    error_code: None,
                })
            }
            Ok(None) => Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                success: false,
                operation: crate::proto::proximadb_v1::VectorServiceOperation::VsGet as i32,
                metrics: None,
                results: None,
                vector_ids: vec![],
                error_message: None,
                error_code: Some("NOT_FOUND".to_string()),
            }),
            Err(e) => {
                tracing::error!("❌ Failed to get vector: {}", e);
                Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success: false,
                    operation: crate::proto::proximadb_v1::VectorServiceOperation::VsGet as i32,
                    metrics: None,
                    results: None,
                    vector_ids: vec![],
                    error_message: None,
                    error_code: Some("INTERNAL_ERROR".to_string()),
                })
            }
        }
    }

    // Non-v1 vector get removed. Use handle_vector_v1 directly.

    // Optimized non-v1 search path removed. Use handle_vector_search_v1.

    /// List unflushed vectors for a collection
    /// This queries the global partitioned memtable to get vectors that haven't been flushed yet
    pub async fn list_unflushed_vectors(
        &self,
        collection_id: &str,
    ) -> Result<Vec<proximadb_records::ProximaRecord>> {
        debug!(
            "UnifiedHandlers: Listing unflushed vectors for collection {}",
            collection_id
        );

        let unflushed = self
            .vector_operations_service
            .get_unflushed_vectors(collection_id)
            .await?;

        debug!(
            "Found {} unflushed vectors for collection {}",
            unflushed.len(),
            collection_id
        );

        Ok(unflushed)
    }

    /// Force flush all collections
    pub async fn force_flush_all(&self) -> Result<serde_json::Value> {
        debug!("⚡ UnifiedHandlers: Force flushing all collections");
        self.vector_operations_service.force_flush_all().await?;
        Ok(serde_json::json!({"success": true, "operation": "force_flush_all"}))
    }

    /// Force flush collection using VectorOperationsService
    pub async fn force_flush_collection(&self, collection_id: &str) -> Result<serde_json::Value> {
        debug!(
            "⚡ UnifiedHandlers: Force flushing collection {}",
            collection_id
        );
        self.vector_operations_service
            .force_flush_collection(collection_id)
            .await?;
        Ok(
            serde_json::json!({"success": true, "operation": "force_flush_collection", "collection_id": collection_id}),
        )
    }

    /// Get metrics using VectorOperationsService
    pub async fn metrics(&self) -> Result<serde_json::Value> {
        debug!("📊 UnifiedHandlers: Getting service metrics");
        self.vector_operations_service.metrics().await
    }

    /// Get collection-specific metrics using the metrics query service
    pub async fn collection_metrics(
        &self,
        collection_id: &str,
        include_hints: bool,
    ) -> Result<serde_json::Value> {
        debug!(
            "📊 UnifiedHandlers: Getting metrics for collection {}",
            collection_id
        );

        // Use metrics query service if available
        if let Some(ref metrics_service) = self.metrics_query_service {
            let options = MetricsQueryOptions {
                include_hints,
                include_history: false,
                from_timestamp: None,
                to_timestamp: None,
                metric_names: Vec::new(),
            };

            let metrics = metrics_service
                .collection_metrics(collection_id, options)
                .await
                .context("Failed to query collection metrics")?;

            let mut response = serde_json::json!({
                "collection_id": collection_id,
                "metrics": serde_json::to_value(&metrics)?,
            });

            // Include optimization hints if requested
            if include_hints {
                let hints_result = metrics_service.query_hints(collection_id, None).await;
                if let Ok(hints) = hints_result {
                    response["hints"] = serde_json::to_value(&hints)?;
                }
            }

            Ok(response)
        } else {
            // Fallback to collection service for basic metrics
            if let Ok(Some(collection)) = self.collection_service.collection(collection_id).await {
                let response = serde_json::json!({
                    "collection_id": collection_id,
                    "metrics": {
                        "basic": {
                            "vector_count": collection.stats.as_ref().map(|s| s.vector_count),
                            "dimension": collection.config.as_ref().map(|c| c.dimension),
                            "data_size_bytes": collection.stats.as_ref().map(|s| s.data_size_bytes),
                            "index_size_bytes": collection.stats.as_ref().map(|s| s.index_size_bytes),
                        }
                    },
                    "note": "Using basic metrics. Initialize with metrics service for full metrics."
                });
                Ok(response)
            } else {
                Err(anyhow::anyhow!("Collection {} not found", collection_id))
            }
        }
    }

    /// Get query optimization hints using the metrics query service
    pub async fn query_hints(
        &self,
        collection_id: &str,
        query_type: Option<String>,
    ) -> Result<serde_json::Value> {
        debug!(
            "📊 UnifiedHandlers: Getting query hints for collection {}",
            collection_id
        );

        // Use metrics query service if available
        if let Some(ref metrics_service) = self.metrics_query_service {
            let hints_result = metrics_service
                .query_hints(collection_id, query_type.clone())
                .await
                .context("Failed to get query hints")?;

            // The hints are already filtered by query type in the service
            let hints_vec = hints_result.hints;

            let response = serde_json::json!({
                "collection_id": collection_id,
                "hints": serde_json::to_value(&hints_vec)?,
                "generated_at": chrono::Utc::now().timestamp_millis()
            });

            Ok(response)
        } else {
            // Fallback response when metrics service not available
            let response = serde_json::json!({
                "collection_id": collection_id,
                "hints": [
                    {
                        "type": "info",
                        "priority": "low",
                        "recommendation": "Enable metrics service for query optimization hints",
                        "reason": "Metrics service not initialized"
                    }
                ],
                "generated_at": chrono::Utc::now().timestamp_millis()
            });

            Ok(response)
        }
    }

    /// Execute a hybrid vector-graph query
    pub async fn execute_hybrid_query(
        &self,
        request: crate::proto::proximadb_v1::HybridSearchRequest,
    ) -> Result<crate::proto::proximadb_v1::HybridSearchResponse> {
        let start_time = std::time::Instant::now();
        info!(
            "Executing hybrid query with strategy: {:?}",
            request.combination_strategy
        );

        let mut nodes: Vec<crate::proto::proximadb_v1::Node> = Vec::new();
        let mut edges: Vec<crate::proto::proximadb_v1::Edge> = Vec::new();
        let mut paths: Vec<crate::proto::proximadb_v1::GraphPath> = Vec::new();
        let mut vector_results: Vec<crate::proto::proximadb_v1::SearchVectorRecord> = Vec::new();

        match request.combination_strategy() {
            crate::proto::proximadb_v1::CombinationStrategy::VectorThenGraph => {
                // 1. Perform vector search
                let vector_search_response = self
                    .handle_vector_search_v1(
                        request.vector_search_request.clone().unwrap_or_default(),
                    )
                    .await?;
                if let Some(results) = vector_search_response.results {
                    vector_results.extend(results.results);

                    // Extract node IDs from vector search results (assuming vector IDs map to graph node IDs)
                    let start_node_ids: Vec<String> =
                        vector_results.iter().map(|rec| rec.id.clone()).collect();

                    // 2. Perform graph traversal from these nodes
                    if !start_node_ids.is_empty() {
                        let graph_req = request.graph_traversal_request.clone().unwrap_or_default();
                        let traversal_request = crate::graph::TraversalRequest {
                            graph_id: "default".to_string(), // Deferred: Extract from request or pass as parameter
                            start_node_id: start_node_ids.first().cloned().unwrap_or_default(), // Use first for now, need to handle multiple starts
                            max_depth: if graph_req.max_depth == 0 {
                                3
                            } else {
                                graph_req.max_depth
                            },
                            edge_types: graph_req.edge_types,
                            node_labels: graph_req.node_labels,
                            filters: graph_req.filters.into_iter().map(Into::into).collect(),
                            algorithm: if graph_req.algorithm == 0 {
                                1
                            } else {
                                graph_req.algorithm
                            }, // Default to BFS (1)
                            limit: request.limit,
                            max_frontier: None,
                            timeout_ms: None,
                        };

                        let traversal_response = self
                            .graph_operations_service
                            .traverse("default", traversal_request)
                            .await?;
                        nodes.extend(traversal_response.nodes.into_iter().map(Into::into));
                        edges.extend(traversal_response.edges.into_iter().map(Into::into));
                        paths.extend(traversal_response.paths.into_iter().map(Into::into));
                    }
                }
            }
            // Deferred: Implement other combination strategies
            _ => return Err(anyhow::anyhow!("Unsupported combination strategy")),
        }

        let elapsed_time = start_time.elapsed().as_micros() as u64;

        let nodes_count = nodes.len() as u32;
        Ok(crate::proto::proximadb_v1::HybridSearchResponse {
            nodes,
            edges,
            paths,
            stats: Some(crate::proto::proximadb_v1::HybridSearchStats {
                vector_results_count: vector_results.len() as u32,
                graph_traversal_count: nodes_count,
                execution_time_microseconds: elapsed_time,
            }),
            vector_results,
        })
    }

    /// Handle create collection operation
    async fn handle_create_collection(
        &self,
        request: CollectionRequest,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<(
        bool,
        Option<Collection>,
        Option<Vec<Collection>>,
        i64,
        Option<String>,
        Option<String>,
    )> {
        let mut config = request
            .collection_config
            .context("Missing collection config")?;

        // INFO: Log incoming config BEFORE applying defaults
        info!(
            "⚠️ INFO BEFORE DEFAULTS: name={}, distance_metric={:?}, storage_engine={:?}",
            config.name, config.distance_metric, config.storage_engine
        );

        // Apply smart defaults at API boundary
        crate::proto::defaults::apply_collection_config_defaults(&mut config);

        // INFO: Log config AFTER applying defaults
        info!(
            "✅ INFO AFTER DEFAULTS: name={}, distance_metric={:?}, storage_engine={:?}",
            config.name, config.distance_metric, config.storage_engine
        );

        let response = match tenant_context {
            Some(tenant_ctx) => {
                self.collection_service
                    .create_collection_with_tenant_context(&config, Some(tenant_ctx))
                    .await
            }
            None => self.collection_service.create_collection(&config).await,
        };

        match response {
            Ok(response) => {
                if response.success {
                    // DEBUG: Log the collection config being returned
                    if let Some(ref collection) = response.collection
                        && let Some(ref config) = collection.config
                    {
                        info!(
                            "🔍 DEBUG Returning collection: name={}, distance_metric={:?}, storage_engine={:?}",
                            config.name, config.distance_metric, config.storage_engine
                        );
                    }
                    Ok((true, response.collection, None, 1, None, None))
                } else {
                    Ok((
                        false,
                        None,
                        None,
                        0,
                        response.error_code.clone(),
                        response.error_code,
                    ))
                }
            }
            Err(e) => {
                error!("Failed to create collection: {:?}", e);
                Ok((
                    false,
                    None,
                    None,
                    0,
                    Some(e.to_string()),
                    Some("CREATE_FAILED".to_string()),
                ))
            }
        }
    }

    /// Handle get collection operation with dual resolution
    async fn handle_collection(
        &self,
        request: CollectionRequest,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<(
        bool,
        Option<Collection>,
        Option<Vec<Collection>>,
        i64,
        Option<String>,
        Option<String>,
    )> {
        let collection_identifier = request.collection_id.context("Missing collection ID")?;

        if let Some(tenant_ctx) = tenant_context {
            return match self
                .collection_service
                .get_collection_with_tenant_context(&collection_identifier, Some(tenant_ctx))
                .await
            {
                Ok(Some(collection)) => Ok((true, Some(collection), None, 1, None, None)),
                Ok(None) => Ok((
                    false,
                    None,
                    None,
                    0,
                    Some("Collection not found".to_string()),
                    Some("NOT_FOUND".to_string()),
                )),
                Err(e) => {
                    error!("Failed to get tenant-scoped collection: {:?}", e);
                    Ok((
                        false,
                        None,
                        None,
                        0,
                        Some(e.to_string()),
                        Some("GET_FAILED".to_string()),
                    ))
                }
            };
        }

        // Resolve collection name/ID to collection ID
        let collection_id = match self
            .collection_service
            .resolve_collection_id(&collection_identifier)
            .await?
        {
            Some(id) => id,
            None => {
                return Ok((
                    false,
                    None,
                    None,
                    0,
                    Some("Collection not found".to_string()),
                    Some("NOT_FOUND".to_string()),
                ));
            }
        };

        debug!(
            "🔍 Getting collection: '{}' -> collection_id: '{}'",
            collection_identifier, collection_id
        );

        match self.collection_service.collection(&collection_id).await {
            Ok(Some(collection)) => Ok((true, Some(collection), None, 1, None, None)),
            Ok(None) => Ok((
                false,
                None,
                None,
                0,
                Some("Collection not found".to_string()),
                Some("NOT_FOUND".to_string()),
            )),
            Err(e) => {
                error!("Failed to get collection: {:?}", e);
                Ok((
                    false,
                    None,
                    None,
                    0,
                    Some(e.to_string()),
                    Some("GET_FAILED".to_string()),
                ))
            }
        }
    }

    /// Handle list collections operation
    async fn handle_list_collections(
        &self,
        _request: CollectionRequest,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<(
        bool,
        Option<Collection>,
        Option<Vec<Collection>>,
        i64,
        Option<String>,
        Option<String>,
    )> {
        match self
            .collection_service
            .list_collections_with_tenant_context(tenant_context)
            .await
        {
            Ok(collections) => {
                let count = collections.len() as i64;
                Ok((true, None, Some(collections), count, None, None))
            }
            Err(e) => {
                error!("Failed to list collections: {:?}", e);
                Ok((
                    false,
                    None,
                    None,
                    0,
                    Some(e.to_string()),
                    Some("LIST_FAILED".to_string()),
                ))
            }
        }
    }

    /// Handle update collection operation with dual resolution
    async fn handle_update_collection(
        &self,
        request: CollectionRequest,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<(
        bool,
        Option<Collection>,
        Option<Vec<Collection>>,
        i64,
        Option<String>,
        Option<String>,
    )> {
        let collection_identifier = request.collection_id.context("Missing collection ID")?;
        let config = request
            .collection_config
            .context("Missing collection config")?;

        let collection_id = if let Some(tenant_ctx) = tenant_context {
            match self
                .collection_service
                .get_collection_with_tenant_context(&collection_identifier, Some(tenant_ctx))
                .await?
            {
                Some(collection) => collection.id,
                None => {
                    return Ok((
                        false,
                        None,
                        None,
                        0,
                        Some("Collection not found".to_string()),
                        Some("NOT_FOUND".to_string()),
                    ));
                }
            }
        } else {
            match self
                .collection_service
                .resolve_collection_id(&collection_identifier)
                .await?
            {
                Some(id) => id,
                None => {
                    return Ok((
                        false,
                        None,
                        None,
                        0,
                        Some("Collection not found".to_string()),
                        Some("NOT_FOUND".to_string()),
                    ));
                }
            }
        };

        debug!(
            "🔄 Updating collection: '{}' -> collection_id: '{}'",
            collection_identifier, collection_id
        );

        match self
            .collection_service
            .update_collection(&collection_id, Some(config))
            .await
        {
            Ok(response) => {
                if response.success {
                    // Invalidate cache entries for both the identifier and resolved ID
                    // (collection config may have changed, affecting future lookups)
                    self.invalidate_collection_cache(&collection_identifier);
                    self.invalidate_collection_cache(&collection_id);
                    Ok((true, response.collection, None, 1, None, None))
                } else {
                    Ok((
                        false,
                        None,
                        None,
                        0,
                        response.error_code.clone(),
                        response.error_code,
                    ))
                }
            }
            Err(e) => {
                error!("Failed to update collection: {:?}", e);
                Ok((
                    false,
                    None,
                    None,
                    0,
                    Some(e.to_string()),
                    Some("UPDATE_FAILED".to_string()),
                ))
            }
        }
    }

    /// Handle delete collection operation with dual resolution
    async fn handle_delete_collection(
        &self,
        request: CollectionRequest,
        tenant_context: Option<&crate::storage::tenant::TenantContext>,
    ) -> Result<(
        bool,
        Option<Collection>,
        Option<Vec<Collection>>,
        i64,
        Option<String>,
        Option<String>,
    )> {
        let collection_identifier = request.collection_id.context("Missing collection ID")?;

        if let Some(tenant_ctx) = tenant_context {
            return match self
                .collection_service
                .delete_collection_with_tenant_context(&collection_identifier, Some(tenant_ctx))
                .await
            {
                Ok(response) => {
                    if response.success {
                        self.invalidate_collection_cache(&collection_identifier);
                        if let Some(collection) = &response.collection {
                            self.invalidate_collection_cache(&collection.id);
                        }
                        Ok((true, None, None, 1, None, None))
                    } else if response.error_code.as_deref() == Some("NOT_FOUND") {
                        Ok((
                            false,
                            None,
                            None,
                            0,
                            Some("Collection not found".to_string()),
                            Some("NOT_FOUND".to_string()),
                        ))
                    } else {
                        Ok((
                            false,
                            None,
                            None,
                            0,
                            response.error_code.clone(),
                            response.error_code,
                        ))
                    }
                }
                Err(e) => {
                    error!("Failed to delete tenant-scoped collection: {:?}", e);
                    Ok((
                        false,
                        None,
                        None,
                        0,
                        Some(e.to_string()),
                        Some("DELETE_FAILED".to_string()),
                    ))
                }
            };
        }

        // Resolve collection name/ID to collection ID
        let collection_id = match self
            .collection_service
            .resolve_collection_id(&collection_identifier)
            .await?
        {
            Some(id) => id,
            None => {
                return Ok((
                    false,
                    None,
                    None,
                    0,
                    Some("Collection not found".to_string()),
                    Some("NOT_FOUND".to_string()),
                ));
            }
        };

        debug!(
            "🗑️ Deleting collection: '{}' -> collection_id: '{}'",
            collection_identifier, collection_id
        );

        match self
            .collection_service
            .delete_collection(&collection_id)
            .await
        {
            Ok(response) => {
                if response.success {
                    // Invalidate cache entries for both the identifier and resolved ID
                    self.invalidate_collection_cache(&collection_identifier);
                    self.invalidate_collection_cache(&collection_id);
                    Ok((true, None, None, 1, None, None))
                } else if response.error_code.as_deref() == Some("NOT_FOUND") {
                    Ok((
                        false,
                        None,
                        None,
                        0,
                        Some("Collection not found".to_string()),
                        Some("NOT_FOUND".to_string()),
                    ))
                } else {
                    Ok((
                        false,
                        None,
                        None,
                        0,
                        response.error_code.clone(),
                        response.error_code,
                    ))
                }
            }
            Err(e) => {
                error!("Failed to delete collection: {:?}", e);
                Ok((
                    false,
                    None,
                    None,
                    0,
                    Some(e.to_string()),
                    Some("DELETE_FAILED".to_string()),
                ))
            }
        }
    }

    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<Collection>> {
        debug!("📋 UnifiedHandlers: Listing all collections");
        self.collection_service.list_collections().await
    }

    /// Get a specific collection by ID
    pub async fn collection(&self, collection_id: &str) -> Result<Option<Collection>> {
        debug!("🔍 UnifiedHandlers: Getting collection {}", collection_id);

        // Get collection metadata from the metadata backend
        let collections = self.collection_service.list_collections().await?;

        // Find the collection by ID (could be name or UUID from the config)
        Ok(collections.into_iter().find(|c| {
            c.id == collection_id
                || c.config
                    .as_ref()
                    .is_some_and(|cfg| cfg.name == collection_id)
        }))
    }

    /// Execute SQL and return v1 ExecuteQueryResponse directly (typed rows and params)
    ///
    /// When the `unified-facade-routing` feature is enabled and a query adapter is set,
    /// SQL queries are routed through the UnifiedQueryFacade for consistent metrics
    /// and unified strategy selection.
    pub async fn execute_sql_v1(
        &self,
        query: String,
        parameters: Option<Vec<crate::proto::proximadb_v1::SqlValue>>,
        collection: Option<String>,
        // TD-121: the authenticated tenant (gRPC `x-tenant-id`, REST/JWT). Scopes
        // relational SQL to the tenant's partition (TD-064), mirroring pgwire.
        tenant_id: Option<&str>,
    ) -> Result<crate::proto::proximadb_v1::ExecuteQueryResponse> {
        let start_time = std::time::Instant::now();

        // EXPLAIN detection: route EXPLAIN [ANALYZE] <DML> through DmlService before any other path.
        if let Some((is_analyze, inner_query)) =
            crate::query::sql_frontend::parse_explain_kind(query.trim())
            && let Some(dml_svc) = self.get_dml_service()
        {
            let parser = crate::query::sql_frontend::SqlFrontendParser::new();
            match parser.parse_dml(inner_query) {
                Ok(Some(statement)) => {
                    let explain_result = if is_analyze {
                        dml_svc.explain_analyze_table_write(statement).await
                    } else {
                        dml_svc.explain_table_write(statement).await
                    };
                    match explain_result {
                        Ok(explanation) => {
                            let json = serde_json::to_string_pretty(&explanation)
                                .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e));
                            return Ok(crate::proto::proximadb_v1::ExecuteQueryResponse {
                                    rows: vec![crate::proto::proximadb_v1::SqlRow {
                                        fields: vec![crate::proto::proximadb_v1::SqlRowField {
                                            key: "QUERY PLAN".to_string(),
                                            value: Some(crate::proto::proximadb_v1::SqlValue {
                                                value: Some(
                                                    crate::proto::proximadb_v1::sql_value::Value::StringValue(json),
                                                ),
                                            }),
                                        }],
                                        similarity: None,
                                    }],
                                    rows_scanned: 0,
                                    rows_returned: 1,
                                    execution_time_ms: start_time.elapsed().as_millis() as u64,
                                    columns: vec!["QUERY PLAN".to_string()],
                                    column_types: vec!["jsonb".to_string()],
                                });
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("EXPLAIN failed: {}", e));
                        }
                    }
                }
                Ok(None) => {
                    return Err(anyhow::anyhow!("Invalid EXPLAIN statement"));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("EXPLAIN parse error: {}", e));
                }
            }
            // DmlService not wired — fall through to legacy path so EXPLAIN degrades gracefully.
        }

        // TD-135: route relational WRITES (DDL + DML) through the same tenant-scoped
        // service seams pgwire uses, instead of the legacy vector/graph engine which
        // rejects them. Cheap leading-keyword guards avoid re-parsing read queries.
        // The request tenant (x-tenant-id, threaded as `tenant_id`) scopes the write
        // to its partition (TD-064); a None tenant writes unscoped, exactly as pgwire
        // does for a connection with no `database` — never another tenant's partition.
        {
            let leading = query.trim_start().get(..7).map(str::to_ascii_uppercase);
            let leading = leading.as_deref().unwrap_or("");
            // DDL: CREATE / ALTER / DROP.
            if (leading.starts_with("CREATE ")
                || leading.starts_with("ALTER ")
                || leading.starts_with("DROP "))
                && let Some(ddl) = self.get_ddl_service()
            {
                let parser = crate::query::sql_frontend::SqlFrontendParser::new();
                match parser.parse_ddl(&query) {
                    Ok(Some(statement)) => {
                        let result = ddl
                            .execute_scoped(statement, tenant_id)
                            .await
                            .map_err(|e| anyhow::anyhow!("DDL failed: {e}"))?;
                        return Ok(crate::proto::proximadb_v1::ExecuteQueryResponse {
                            rows: vec![],
                            rows_scanned: 0,
                            rows_returned: result.affected_count as u64,
                            execution_time_ms: start_time.elapsed().as_millis() as u64,
                            columns: vec![],
                            column_types: vec![],
                        });
                    }
                    Ok(None) => {}
                    Err(e) => return Err(anyhow::anyhow!("DDL parse error: {e}")),
                }
            }
            // DML writes: INSERT / UPDATE / DELETE (+ UPSERT/MERGE shapes via parse_dml).
            if (leading.starts_with("INSERT ")
                || leading.starts_with("UPDATE ")
                || leading.starts_with("DELETE ")
                || leading.starts_with("UPSERT ")
                || leading.starts_with("MERGE "))
                && let Some(dml) = self.get_dml_service()
                && let Ok(Some(statement)) =
                    crate::query::sql_frontend::SqlFrontendParser::new().parse_dml(&query)
            {
                use crate::services::dml::DmlStatement;
                let is_write = matches!(
                    statement,
                    DmlStatement::Insert { .. }
                        | DmlStatement::Update { .. }
                        | DmlStatement::Delete { .. }
                        | DmlStatement::Upsert { .. }
                        | DmlStatement::InsertSelect { .. }
                        | DmlStatement::InsertOverwrite { .. }
                );
                if is_write {
                    let tenant_ctx = tenant_id
                        .map(crate::storage::tenant::context::TenantContext::for_tenant_id);
                    let result = dml
                        .execute_scoped(statement, tenant_ctx.as_ref())
                        .await
                        .map_err(|e| anyhow::anyhow!("DML failed: {e}"))?;
                    return Ok(crate::proto::proximadb_v1::ExecuteQueryResponse {
                        rows: vec![],
                        rows_scanned: 0,
                        rows_returned: result.rows_affected,
                        execution_time_ms: start_time.elapsed().as_millis() as u64,
                        columns: vec![],
                        column_types: vec![],
                    });
                }
            }
        }

        // TD-121: route RELATIONAL SQL through the same tenant-scoped relational
        // pipeline pgwire uses (ComputeScheduler::route_select → DataFusion/Volcano,
        // TD-064 tenant partitioning), instead of the legacy vector/graph engine
        // which rejects relational plans. `require_engagement = false` routes any
        // resolvable relational SELECT here; queries whose tables don't resolve
        // (vector/graph SQL) return None and fall through to the legacy engine.
        if let Some(dml) = self.get_dml_service()
            && let Some(outcome) = crate::network::postgres::relational_pipeline::try_run_select(
                &query,
                Some(&dml),
                None,
                tenant_id,
                crate::query::execution::ExecutionControls::default(),
                false,
            )
            .await
        {
            return match outcome {
                Ok(result) => Ok(Self::pipeline_result_to_sql_response(result, start_time)),
                Err(msg) => Err(anyhow::anyhow!("Relational query execution failed: {msg}")),
            };
        }

        // Route through unified facade when feature is enabled and adapter is available
        #[cfg(feature = "unified-facade-routing")]
        if let Some(adapter) = self.get_query_adapter() {
            tracing::debug!("Routing SQL query through unified facade");

            // Execute through the facade
            let query_result = adapter.sql_query(&query).await?;

            // Convert QueryResult to ExecuteQueryResponse
            return self.convert_query_result_to_sql_response(query_result, start_time);
        }

        // Legacy path: Use sql_frontend directly
        // Do not perform string substitution; pass params along for the frontend to bind
        let result = self
            .execute_sql_frontend(query.clone(), parameters.clone(), collection.clone())
            .await?;

        // Convert SqlQueryResult (JSON rows) to v1 ExecuteQueryResponse (typed rows)
        use crate::proto::proximadb_v1::{SqlRow, SqlRowField};
        let mut rows: Vec<SqlRow> = Vec::new();
        for row in result.rows {
            let mut fields_vec: Vec<SqlRowField> = Vec::new();
            if let serde_json::Value::Object(map) = row {
                for (k, v) in map {
                    let sv = Self::json_to_sql_value(&v);
                    fields_vec.push(SqlRowField {
                        key: k,
                        value: Some(sv),
                    });
                }
            }
            rows.push(SqlRow {
                fields: fields_vec,
                similarity: None,
            });
        }

        Ok(crate::proto::proximadb_v1::ExecuteQueryResponse {
            rows,
            rows_scanned: 0,
            rows_returned: result.row_count as u64,
            execution_time_ms: result.execution_time_ms,
            columns: result.columns.iter().map(|c| c.0.clone()).collect(),
            column_types: result.columns.iter().map(|c| c.1.clone()).collect(),
        })
    }

    /// Convert a relational [`ExecutionPipelineResult`] (the tenant-scoped pgwire
    /// pipeline output) into the v1 `ExecuteQueryResponse` carried by the gRPC/REST
    /// SQL surfaces (TD-121). Mirrors pgwire's `emit_pipeline_result`: column names
    /// + types come from the result schema; each `ProximaValue` cell maps to a
    /// typed `SqlValue` via the canonical converter.
    fn pipeline_result_to_sql_response(
        result: crate::query::execution::engine::ExecutionPipelineResult,
        start_time: std::time::Instant,
    ) -> crate::proto::proximadb_v1::ExecuteQueryResponse {
        use crate::proto::proximadb_v1::{SqlRow, SqlRowField};
        let columns: Vec<String> = result
            .schema
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect();
        let column_types: Vec<String> = result
            .schema
            .columns
            .iter()
            .map(|c| format!("{:?}", c.ty))
            .collect();
        let rows_returned = result.rows.len() as u64;
        let rows: Vec<SqlRow> = result
            .rows
            .into_iter()
            .map(|row| {
                let fields = row
                    .into_iter()
                    .enumerate()
                    .map(|(i, value)| SqlRowField {
                        key: columns.get(i).cloned().unwrap_or_else(|| format!("col{i}")),
                        value: Some(crate::core::search::results::proxima_value_to_sql_value(
                            value,
                        )),
                    })
                    .collect();
                SqlRow {
                    fields,
                    similarity: None,
                }
            })
            .collect();
        crate::proto::proximadb_v1::ExecuteQueryResponse {
            rows,
            rows_scanned: 0,
            rows_returned,
            execution_time_ms: start_time.elapsed().as_millis() as u64,
            columns,
            column_types,
        }
    }

    /// Convert QueryResult from unified facade to ExecuteQueryResponse
    #[cfg(feature = "unified-facade-routing")]
    fn convert_query_result_to_sql_response(
        &self,
        query_result: crate::query::QueryResult,
        start_time: std::time::Instant,
    ) -> Result<crate::proto::proximadb_v1::ExecuteQueryResponse> {
        use crate::proto::proximadb_v1::{SqlRow, SqlRowField};
        use crate::query::QueryResultData;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        // Extract rows from QueryResult
        let json_rows = match query_result.data {
            QueryResultData::Rows(rows) => rows,
            QueryResultData::VectorResults(matches) => {
                // Convert vector matches to JSON rows
                matches
                    .into_iter()
                    .map(|m| {
                        serde_json::json!({
                            "id": m.record.oid,
                            "score": m.score,
                            "metadata": m.record.props
                        })
                    })
                    .collect()
            }
            QueryResultData::Empty => vec![],
            QueryResultData::Graph(graph_result) => {
                // Convert graph results to JSON rows
                graph_result
                    .nodes
                    .into_iter()
                    .map(|node| serde_json::to_value(node).unwrap_or_default())
                    .collect()
            }
        };

        // Convert JSON rows to SqlRow format
        let mut rows: Vec<SqlRow> = Vec::new();
        let mut columns: Vec<String> = Vec::new();
        let mut column_types: Vec<String> = Vec::new();

        for row in &json_rows {
            let mut fields_vec: Vec<SqlRowField> = Vec::new();
            if let serde_json::Value::Object(map) = row {
                // Build column list from first row
                if columns.is_empty() {
                    for (k, v) in map {
                        columns.push(k.clone());
                        column_types.push(self.infer_json_type(v));
                    }
                }

                for (k, v) in map {
                    let sv = Self::json_to_sql_value(v);
                    fields_vec.push(SqlRowField {
                        key: k.clone(),
                        value: Some(sv),
                    });
                }
            }
            rows.push(SqlRow {
                fields: fields_vec,
                similarity: None,
            });
        }

        let row_count = rows.len() as u64;

        Ok(crate::proto::proximadb_v1::ExecuteQueryResponse {
            rows,
            rows_scanned: 0,
            rows_returned: row_count,
            execution_time_ms,
            columns,
            column_types,
        })
    }

    /// Apply parameters to a parameterized query
    /// Replaces $1, $2, etc. with actual parameter values
    #[allow(dead_code)]
    fn apply_query_parameters(
        &self,
        query: String,
        parameters: Vec<serde_json::Value>,
    ) -> Result<String> {
        let mut processed = query;

        for (index, param) in parameters.iter().enumerate() {
            let placeholder = format!("${}", index + 1);
            let value = self.format_sql_value(param)?;
            processed = processed.replace(&placeholder, &value);
        }

        // Also support ? placeholders (common in many SQL dialects)
        let mut result = String::new();
        let mut param_index = 0;

        for ch in processed.chars() {
            if ch == '?' && param_index < parameters.len() {
                result.push_str(&self.format_sql_value(&parameters[param_index])?);
                param_index += 1;
            } else {
                result.push(ch);
            }
        }

        Ok(result)
    }

    /// Format a JSON value for SQL
    #[allow(dead_code)]
    fn format_sql_value(&self, value: &serde_json::Value) -> Result<String> {
        match value {
            serde_json::Value::Null => Ok("NULL".to_string()),
            serde_json::Value::Bool(b) => Ok(b.to_string()),
            serde_json::Value::Number(n) => Ok(n.to_string()),
            serde_json::Value::String(s) => {
                // Escape single quotes and wrap in quotes
                let escaped = s.replace("'", "''");
                Ok(format!("'{}'", escaped))
            }
            serde_json::Value::Array(arr) => {
                // Format as SQL array literal
                let items: Result<Vec<_>> = arr.iter().map(|v| self.format_sql_value(v)).collect();
                Ok(format!("ARRAY[{}]", items?.join(", ")))
            }
            serde_json::Value::Object(_) => {
                // Convert to JSON string for object types
                Ok(format!("'{}'", value.to_string().replace("'", "''")))
            }
        }
    }

    /// Infer SQL type from JSON value
    fn infer_json_type(&self, value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Null => "NULL".to_string(),
            serde_json::Value::Bool(_) => "BOOLEAN".to_string(),
            serde_json::Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    "INTEGER".to_string()
                } else {
                    "FLOAT".to_string()
                }
            }
            serde_json::Value::String(_) => "TEXT".to_string(),
            serde_json::Value::Array(arr) => {
                if let Some(first) = arr.first() {
                    format!("ARRAY<{}>", self.infer_json_type(first))
                } else {
                    "ARRAY".to_string()
                }
            }
            serde_json::Value::Object(_) => "JSON".to_string(),
        }
    }
}

impl UnifiedHandlers {
    /// Parse an EXPLAIN statement into `(is_analyze, inner_dml)`.
    /// Handles `EXPLAIN ANALYZE <dml>`, `EXPLAIN (ANALYZE) <dml>`, and plain `EXPLAIN <dml>`.
    fn json_to_sql_value(v: &serde_json::Value) -> crate::proto::proximadb_v1::SqlValue {
        use crate::proto::proximadb_v1::{self, sql_value::Value as V};
        match v {
            serde_json::Value::String(s) => proximadb_v1::SqlValue {
                value: Some(V::StringValue(s.clone())),
            },
            serde_json::Value::Number(n) => proximadb_v1::SqlValue {
                value: Some(V::NumberValue(n.as_f64().unwrap_or(0.0))),
            },
            serde_json::Value::Bool(b) => proximadb_v1::SqlValue {
                value: Some(V::BoolValue(*b)),
            },
            serde_json::Value::Null => proximadb_v1::SqlValue {
                value: Some(V::NullValue(0)),
            },
            serde_json::Value::Array(arr) => {
                let values = arr.iter().map(Self::json_to_sql_value).collect();
                proximadb_v1::SqlValue {
                    value: Some(V::ArrayValue(proximadb_v1::SqlArray { values })),
                }
            }
            serde_json::Value::Object(map) => {
                let mut fields = std::collections::BTreeMap::new();
                for (k, sv) in map {
                    fields.insert(k.clone(), Self::json_to_sql_value(sv));
                }
                let fields_hashmap: std::collections::HashMap<
                    String,
                    crate::proto::proximadb_v1::SqlValue,
                > = fields.into_iter().collect();
                proximadb_v1::SqlValue {
                    value: Some(V::ObjectValue(proximadb_v1::SqlObject {
                        fields: fields_hashmap,
                    })),
                }
            }
        }
    }

    #[allow(dead_code)]
    fn sql_value_to_json(v: &crate::proto::proximadb_v1::SqlValue) -> serde_json::Value {
        use crate::proto::proximadb_v1::sql_value::Value as V;
        match v.value.as_ref() {
            Some(V::StringValue(s)) => serde_json::Value::String(s.clone()),
            Some(V::NumberValue(n)) => serde_json::json!(*n),
            Some(V::BoolValue(b)) => serde_json::Value::Bool(*b),
            Some(V::Int64Value(i)) => serde_json::json!(*i),
            Some(V::BytesValue(b)) => {
                serde_json::Value::Array(b.iter().map(|x| serde_json::json!(*x)).collect())
            }
            Some(V::NullValue(_)) => serde_json::Value::Null,
            Some(V::ArrayValue(arr)) => {
                serde_json::Value::Array(arr.values.iter().map(Self::sql_value_to_json).collect())
            }
            Some(V::ObjectValue(obj)) => {
                let mut map = serde_json::Map::new();
                for (k, sv) in &obj.fields {
                    map.insert(k.clone(), Self::sql_value_to_json(sv));
                }
                serde_json::Value::Object(map)
            }
            None => serde_json::Value::Null,
        }
    }

    /// Execute SQL using sql_frontend (new authoritative path with HashMap optimization)
    ///
    /// This method implements the unified query layer specified in query_sql_alignment_consolidated.adoc
    /// providing 10x metadata filtering performance through HashMap.get() instead of linear scans.
    ///
    /// Key improvements:
    /// - Uses sqlparser-rs for comprehensive SQL support
    /// - HashMap metadata filtering for O(1) vs O(n) performance  
    /// - Integrated SKS functions (SIMILAR/FOLLOW/ASSEMBLE)
    /// - Hybrid vector + graph execution with advanced fusion
    pub async fn execute_sql_frontend(
        &self,
        sql: String,
        params: Option<Vec<crate::proto::proximadb_v1::SqlValue>>,
        _collection: Option<String>,
    ) -> Result<SqlQueryResult> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "🆕 Executing SQL via sql_frontend (HashMap optimized): {}",
            sql.chars().take(100).collect::<String>()
        );

        // 1. Create query lowering service with collection resolution
        let query_lowering = crate::query::sql_frontend::lowering::QueryLowering::new(
            self.collection_service.clone(),
        );

        // 2. Lower SQL to internal AST with validation and optimization
        let query_ast = query_lowering
            .lower_sql(&sql)
            .await
            .map_err(|e| anyhow::anyhow!("SQL lowering failed: {}", e))?;

        // 3. Analyze the query semantically
        let analyzer = crate::query::semantic_analysis::analyzer::Analyzer::new(
            self.collection_service.clone(),
        );
        analyzer
            .analyze(&query_ast)
            .await
            .map_err(|e| anyhow!("Semantic analysis failed: {}", e))?;

        // 4. Create unified query engine with vector and extracted graph execution services
        let graph_service = self.graph_execution_service.clone();
        // Resolve runtime hybrid config overrides (seeding + weights)
        let runtime = self.hybrid_runtime.read().ok().and_then(|g| g.clone());
        let (seeding, fusion_weights) = Self::resolve_hybrid_static(runtime, &sql);

        let query_engine = crate::query::execution::QueryEngine::new_with_options(
            self.vector_operations_service.clone(),
            graph_service,
            params.clone(),
            seeding,
            fusion_weights,
        );

        // 5. Execute query with new engine (uses HashMap metadata optimization)
        let query_result = query_engine
            .execute_frontend(query_ast)
            .await
            .map_err(|e| anyhow::anyhow!("Query execution failed: {}", e))?;

        // 6. Convert QueryResult to SqlQueryResult format (preserve API compatibility)
        let execution_time_ms = start_time.elapsed().as_secs_f64() * 1000.0;

        let rows: Vec<serde_json::Value> = query_result
            .rows
            .into_iter()
            .map(|row| {
                let mut json_obj = serde_json::Map::new();

                // Add all field values (efficiently from HashMap)
                for (key, value) in row.fields {
                    json_obj.insert(key, value);
                }

                // Add similarity score if present
                if let Some(score) = row.similarity_score {
                    json_obj.insert("_similarity_score".to_string(), serde_json::json!(score));
                }

                // Add graph distance if present
                if let Some(distance) = row.graph_distance {
                    json_obj.insert("_graph_distance".to_string(), serde_json::json!(distance));
                }

                // Add provenance if present
                if let Some(provenance) = row.provenance {
                    json_obj.insert("_provenance".to_string(), serde_json::json!(provenance));
                }

                serde_json::Value::Object(json_obj)
            })
            .collect();

        // Infer column types from first row
        let columns = if let Some(first_row) = rows.first()
            && let serde_json::Value::Object(map) = first_row
        {
            map.iter()
                .map(|(key, value)| {
                    let type_name = self.infer_json_type(value);
                    (key.clone(), type_name)
                })
                .collect()
        } else {
            vec![]
        };

        tracing::info!(
            "✅ sql_frontend execution completed in {:.2}ms with {} rows (HashMap optimized)",
            execution_time_ms,
            rows.len()
        );

        let row_count = rows.len();
        Ok(SqlQueryResult {
            rows,
            columns,
            row_count,
            execution_time_ms: execution_time_ms as u64,
        })
    }

    fn parse_seeding_strategy(sql: &str) -> crate::query::execution::SeedingStrategy {
        let s = sql.to_ascii_uppercase();
        // Accept simple inline hints in comments or statements, e.g.:
        // -- SEEDING: PER_SEED  or  /* SEEDING AVERAGE */ or  SEED USING PER_SEED
        if s.contains("SEEDING: PER_SEED") || s.contains("SEED USING PER_SEED") {
            return crate::query::execution::SeedingStrategy::PerSeed;
        }
        if s.contains("SEEDING: NONE") || s.contains("SEED USING NONE") {
            return crate::query::execution::SeedingStrategy::None;
        }
        if s.contains("SEEDING: AVERAGE") || s.contains("SEED USING AVERAGE") {
            return crate::query::execution::SeedingStrategy::Average;
        }
        crate::query::execution::SeedingStrategy::Average
    }

    pub(crate) fn resolve_hybrid_static(
        runtime: Option<crate::core::config::HybridRuntimeConfig>,
        sql: &str,
    ) -> (crate::query::execution::SeedingStrategy, Option<Vec<f64>>) {
        let seeding = if let Some(ref hr) = runtime {
            match hr.seeding_strategy.to_ascii_uppercase().as_str() {
                "PER_SEED" => crate::query::execution::SeedingStrategy::PerSeed,
                "NONE" => crate::query::execution::SeedingStrategy::None,
                _ => crate::query::execution::SeedingStrategy::Average,
            }
        } else {
            Self::parse_seeding_strategy(sql)
        };
        let weights = runtime.and_then(|hr| hr.fusion_weights);
        (seeding, weights)
    }
}

/// Compute the WAL-lane decision for a fast-lane record batch and reject the write if the
/// router selects a non-WAL lane (such as ProjectionOnly) that this
/// adapter does not yet commit. Mirrors the gRPC path's pre-check (see
/// `network/grpc/v2/record_service.rs::insert_records`) so REST callers - which go directly
/// to `handle_record_batch_for_tenant` without a pre-check - get the same enforcement.
///
/// Returns the rejection reason string on failure. Callers wrap it in a
/// `BatchOperationResult::failure(reason, "WAL_LANE_REJECTED")`.
pub(crate) fn enforce_wal_lane_for_record_batch(
    collection_id: &str,
    operation_kind: WriteOperationKind,
    row_count: u64,
    context: &str,
) -> Result<(), String> {
    let intent = WriteIntent::new(collection_id, operation_kind)
        .with_durability(WriteDurabilityRequirement::WalRequired)
        .with_row_count_hint(row_count);
    let decision = WriteLaneRouter::new().route(&intent);
    decision
        .require_wal_lane(context)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod hybrid_tests {
    use super::*;

    #[test]
    fn test_resolve_hybrid_prefers_runtime_over_sql_hint() {
        // Runtime says PER_SEED; SQL hints NONE → runtime should win
        let runtime = crate::core::config::HybridRuntimeConfig {
            seeding_strategy: "PER_SEED".to_string(),
            fusion_weights: Some(vec![0.8, 0.2]),
            ..Default::default()
        };
        let sql = "-- SEEDING: NONE\nSELECT * FROM a";
        let (seeding, weights) = UnifiedHandlers::resolve_hybrid_static(Some(runtime), sql);
        match seeding {
            crate::query::execution::SeedingStrategy::PerSeed => {}
            _ => panic!("Expected PerSeed"),
        }
        assert_eq!(weights, Some(vec![0.8, 0.2]));
    }

    #[test]
    fn test_resolve_hybrid_uses_sql_when_no_runtime() {
        let sql = "-- SEEDING: NONE\nSELECT * FROM a";
        let (seeding, weights) = UnifiedHandlers::resolve_hybrid_static(None, sql);
        match seeding {
            crate::query::execution::SeedingStrategy::None => {}
            _ => panic!("Expected None"),
        }
        assert_eq!(weights, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_proxima_record(
        oid: impl Into<String>,
        vector: Vec<f32>,
        timestamp_ms: i64,
    ) -> proximadb_records::ProximaRecord {
        let timestamp_ns = timestamp_ms.saturating_mul(1_000_000);
        let dim = vector.len() as u32;
        proximadb_records::ProximaRecord {
            oid: oid.into(),
            created_at_ns: timestamp_ns,
            updated_at_ns: timestamp_ns,
            embeddings: vec![proximadb_records::EmbeddingCell {
                model_id: "default".to_string(),
                modality: "dense_vector".to_string(),
                dim,
                values: proximadb_records::EmbeddingValues::Fp32(vector),
                ..Default::default()
            }],
            ..proximadb_records::ProximaRecord::default()
        }
    }

    trait TestProximaRecordExt {
        fn record_version(self, version: u64) -> proximadb_records::ProximaRecord;
        fn origin(self, origin: impl Into<String>) -> proximadb_records::ProximaRecord;
        fn metadata(
            self,
            metadata: std::collections::HashMap<String, proximadb_data_model::ProximaValue>,
        ) -> proximadb_records::ProximaRecord;
    }

    impl TestProximaRecordExt for proximadb_records::ProximaRecord {
        fn record_version(mut self, version: u64) -> proximadb_records::ProximaRecord {
            self.record_version = version;
            self
        }

        fn origin(mut self, origin: impl Into<String>) -> proximadb_records::ProximaRecord {
            self.origin = Some(origin.into());
            self
        }

        fn metadata(
            mut self,
            metadata: std::collections::HashMap<String, proximadb_data_model::ProximaValue>,
        ) -> proximadb_records::ProximaRecord {
            self.props = metadata
                .into_iter()
                .map(|(key, value)| (key, proximadb_records::ProximaTreeNode::Value(value)))
                .collect();
            self
        }
    }

    // ==================== CollectionIdCache Tests ====================

    #[test]
    fn test_collection_id_cache_new() {
        let cache = CollectionIdCache::new();
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn test_collection_id_cache_with_ttl() {
        let cache = CollectionIdCache::with_ttl(Duration::from_secs(60));
        cache.insert("test_name".to_string(), "test_id_123".to_string());
        assert_eq!(cache.get("test_name"), Some("test_id_123".to_string()));
    }

    #[test]
    fn test_collection_id_cache_insert_and_get() {
        let cache = CollectionIdCache::new();
        cache.insert("collection_name".to_string(), "col_uuid_abc".to_string());

        let result = cache.get("collection_name");
        assert_eq!(result, Some("col_uuid_abc".to_string()));
    }

    #[test]
    fn test_collection_id_cache_get_nonexistent() {
        let cache = CollectionIdCache::new();
        assert!(cache.get("does_not_exist").is_none());
    }

    #[test]
    fn test_collection_id_cache_invalidate() {
        let cache = CollectionIdCache::new();
        cache.insert("my_collection".to_string(), "my_id".to_string());
        assert!(cache.get("my_collection").is_some());

        cache.invalidate("my_collection");
        assert!(cache.get("my_collection").is_none());
    }

    #[test]
    fn test_collection_id_cache_invalidate_by_id() {
        let cache = CollectionIdCache::new();
        cache.insert("name1".to_string(), "shared_id".to_string());
        cache.insert("name2".to_string(), "shared_id".to_string());

        // Invalidating by the ID should also remove entries that map to it
        cache.invalidate("shared_id");

        // The invalidate method removes by key AND by value matches
        // So both should be removed
        assert!(cache.get("name1").is_none());
        assert!(cache.get("name2").is_none());
    }

    #[test]
    fn test_collection_id_cache_clear() {
        let cache = CollectionIdCache::new();
        cache.insert("col1".to_string(), "id1".to_string());
        cache.insert("col2".to_string(), "id2".to_string());
        cache.insert("col3".to_string(), "id3".to_string());

        cache.clear();

        assert!(cache.get("col1").is_none());
        assert!(cache.get("col2").is_none());
        assert!(cache.get("col3").is_none());
    }

    #[test]
    fn test_collection_id_cache_default() {
        let cache = CollectionIdCache::default();
        // Should work the same as new()
        cache.insert("test".to_string(), "value".to_string());
        assert_eq!(cache.get("test"), Some("value".to_string()));
    }

    #[test]
    fn test_collection_id_cache_overwrite() {
        let cache = CollectionIdCache::new();
        cache.insert("key".to_string(), "old_value".to_string());
        cache.insert("key".to_string(), "new_value".to_string());

        assert_eq!(cache.get("key"), Some("new_value".to_string()));
    }

    #[test]
    fn test_collection_id_cache_multiple_entries() {
        let cache = CollectionIdCache::new();

        for i in 0..100 {
            cache.insert(format!("name_{}", i), format!("id_{}", i));
        }

        for i in 0..100 {
            assert_eq!(cache.get(&format!("name_{}", i)), Some(format!("id_{}", i)));
        }
    }

    // ==================== generate_request_id Tests ====================

    #[test]
    fn test_generate_request_id_format() {
        let id = generate_request_id();
        // Should be 16 characters (8 hex chars for timestamp + 8 hex chars for counter)
        assert_eq!(id.len(), 16);
        // Should be valid hex
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_request_id_unique() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();
        let id3 = generate_request_id();

        // All IDs should be unique (counter increments)
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_generate_request_id_counter_increments() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();

        // Extract counter portion (last 8 chars)
        let counter1 = u32::from_str_radix(&id1[8..], 16).unwrap();
        let counter2 = u32::from_str_radix(&id2[8..], 16).unwrap();

        // Counter should increment
        assert_eq!(counter2, counter1 + 1);
    }

    // ==================== UnifiedHandlers Helper Method Tests ====================

    #[test]
    fn test_json_to_sql_value_string() {
        let json = serde_json::json!("hello world");
        let sql_value = UnifiedHandlers::json_to_sql_value(&json);

        match sql_value.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                assert_eq!(s, "hello world");
            }
            _ => panic!("Expected StringValue"),
        }
    }

    #[test]
    fn test_json_to_sql_value_number() {
        let json = serde_json::json!(42.5);
        let sql_value = UnifiedHandlers::json_to_sql_value(&json);

        match sql_value.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => {
                assert!((n - 42.5).abs() < 0.0001);
            }
            _ => panic!("Expected NumberValue"),
        }
    }

    #[test]
    fn test_json_to_sql_value_bool_true() {
        let json = serde_json::json!(true);
        let sql_value = UnifiedHandlers::json_to_sql_value(&json);

        match sql_value.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                assert!(b);
            }
            _ => panic!("Expected BoolValue"),
        }
    }

    #[test]
    fn test_json_to_sql_value_bool_false() {
        let json = serde_json::json!(false);
        let sql_value = UnifiedHandlers::json_to_sql_value(&json);

        match sql_value.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                assert!(!b);
            }
            _ => panic!("Expected BoolValue"),
        }
    }

    #[test]
    fn test_json_to_sql_value_null() {
        let json = serde_json::Value::Null;
        let sql_value = UnifiedHandlers::json_to_sql_value(&json);

        match sql_value.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_)) => {}
            _ => panic!("Expected NullValue"),
        }
    }

    #[test]
    fn test_json_to_sql_value_array() {
        let json = serde_json::json!([1, 2, 3]);
        let sql_value = UnifiedHandlers::json_to_sql_value(&json);

        match sql_value.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(arr)) => {
                assert_eq!(arr.values.len(), 3);
            }
            _ => panic!("Expected ArrayValue"),
        }
    }

    #[test]
    fn test_json_to_sql_value_object() {
        let json = serde_json::json!({"key": "value", "num": 42});
        let sql_value = UnifiedHandlers::json_to_sql_value(&json);

        match sql_value.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(obj)) => {
                assert_eq!(obj.fields.len(), 2);
                assert!(obj.fields.contains_key("key"));
                assert!(obj.fields.contains_key("num"));
            }
            _ => panic!("Expected ObjectValue"),
        }
    }

    #[test]
    fn test_sql_value_to_json_string() {
        use crate::proto::proximadb_v1::sql_value::Value;
        let sql_value = crate::proto::proximadb_v1::SqlValue {
            value: Some(Value::StringValue("test".to_string())),
        };

        let json = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(json, serde_json::json!("test"));
    }

    #[test]
    fn test_sql_value_to_json_number() {
        use crate::proto::proximadb_v1::sql_value::Value;
        let sql_value = crate::proto::proximadb_v1::SqlValue {
            value: Some(Value::NumberValue(3.14)),
        };

        let json = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(json, serde_json::json!(3.14));
    }

    #[test]
    fn test_sql_value_to_json_bool() {
        use crate::proto::proximadb_v1::sql_value::Value;
        let sql_value = crate::proto::proximadb_v1::SqlValue {
            value: Some(Value::BoolValue(true)),
        };

        let json = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(json, serde_json::json!(true));
    }

    #[test]
    fn test_sql_value_to_json_int64() {
        use crate::proto::proximadb_v1::sql_value::Value;
        let sql_value = crate::proto::proximadb_v1::SqlValue {
            value: Some(Value::Int64Value(9999)),
        };

        let json = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(json, serde_json::json!(9999));
    }

    #[test]
    fn test_sql_value_to_json_null() {
        use crate::proto::proximadb_v1::sql_value::Value;
        let sql_value = crate::proto::proximadb_v1::SqlValue {
            value: Some(Value::NullValue(0)),
        };

        let json = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn test_sql_value_to_json_none() {
        let sql_value = crate::proto::proximadb_v1::SqlValue { value: None };

        let json = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(json, serde_json::Value::Null);
    }

    // ==================== parse_seeding_strategy Tests ====================

    #[test]
    fn test_parse_seeding_strategy_per_seed() {
        let sql = "-- SEEDING: PER_SEED\nSELECT * FROM table";
        let strategy = UnifiedHandlers::parse_seeding_strategy(sql);
        match strategy {
            crate::query::execution::SeedingStrategy::PerSeed => {}
            _ => panic!("Expected PerSeed strategy"),
        }
    }

    #[test]
    fn test_parse_seeding_strategy_none() {
        let sql = "-- SEEDING: NONE\nSELECT * FROM table";
        let strategy = UnifiedHandlers::parse_seeding_strategy(sql);
        match strategy {
            crate::query::execution::SeedingStrategy::None => {}
            _ => panic!("Expected None strategy"),
        }
    }

    #[test]
    fn test_parse_seeding_strategy_average() {
        let sql = "-- SEEDING: AVERAGE\nSELECT * FROM table";
        let strategy = UnifiedHandlers::parse_seeding_strategy(sql);
        match strategy {
            crate::query::execution::SeedingStrategy::Average => {}
            _ => panic!("Expected Average strategy"),
        }
    }

    #[test]
    fn test_parse_seeding_strategy_default() {
        let sql = "SELECT * FROM table";
        let strategy = UnifiedHandlers::parse_seeding_strategy(sql);
        match strategy {
            crate::query::execution::SeedingStrategy::Average => {}
            _ => panic!("Expected Average (default) strategy"),
        }
    }

    #[test]
    fn test_parse_seeding_strategy_seed_using() {
        let sql = "SEED USING PER_SEED SELECT * FROM table";
        let strategy = UnifiedHandlers::parse_seeding_strategy(sql);
        match strategy {
            crate::query::execution::SeedingStrategy::PerSeed => {}
            _ => panic!("Expected PerSeed strategy with SEED USING syntax"),
        }
    }

    #[test]
    fn test_parse_seeding_strategy_case_insensitive() {
        let sql = "-- seeding: per_seed\nSELECT * FROM table";
        let strategy = UnifiedHandlers::parse_seeding_strategy(sql);
        match strategy {
            crate::query::execution::SeedingStrategy::PerSeed => {}
            _ => panic!("Expected PerSeed strategy (case insensitive)"),
        }
    }

    // ==================== CollectionRequest Validation Tests ====================

    #[test]
    fn test_collection_operation_enum_values() {
        use crate::proto::proximadb_v1::CollectionOperation;

        assert_eq!(CollectionOperation::Unspecified as i32, 0);
        assert_eq!(CollectionOperation::CollectionCreate as i32, 1);
        assert_eq!(CollectionOperation::CollectionUpdate as i32, 2);
        assert_eq!(CollectionOperation::CollectionGet as i32, 3);
        assert_eq!(CollectionOperation::CollectionList as i32, 4);
        assert_eq!(CollectionOperation::CollectionDelete as i32, 5);
    }

    #[test]
    fn test_collection_request_create() {
        use crate::proto::proximadb_v1::{
            CollectionConfig, CollectionOperation, CollectionRequest,
        };

        let config = CollectionConfig {
            name: "test_collection".to_string(),
            dimension: 128,
            distance_metric: Some(0), // Cosine
            storage_engine: Some(0),  // SST
            ..Default::default()
        };

        let request = CollectionRequest {
            operation: CollectionOperation::CollectionCreate as i32,
            collection_id: None,
            collection_config: Some(config.clone()),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };

        assert_eq!(request.operation, 1);
        assert!(request.collection_config.is_some());
        assert_eq!(
            request.collection_config.as_ref().unwrap().name,
            "test_collection"
        );
        assert_eq!(request.collection_config.as_ref().unwrap().dimension, 128);
    }

    #[test]
    fn test_collection_request_get() {
        use crate::proto::proximadb_v1::{CollectionOperation, CollectionRequest};

        let request = CollectionRequest {
            operation: CollectionOperation::CollectionGet as i32,
            collection_id: Some("my_collection_id".to_string()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };

        assert_eq!(request.operation, 3);
        assert_eq!(request.collection_id, Some("my_collection_id".to_string()));
    }

    #[test]
    fn test_collection_request_delete() {
        use crate::proto::proximadb_v1::{CollectionOperation, CollectionRequest};

        let request = CollectionRequest {
            operation: CollectionOperation::CollectionDelete as i32,
            collection_id: Some("delete_me".to_string()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };

        assert_eq!(request.operation, 5);
        assert_eq!(request.collection_id, Some("delete_me".to_string()));
    }

    // ==================== VectorBatchRequest Tests ====================

    #[test]
    fn test_vector_batch_request_construction() {
        use crate::proto::proximadb_v1::VectorBatchRequest;

        let vector1 =
            test_proxima_record("vec_1", vec![0.1, 0.2, 0.3, 0.4], 1234567890).record_version(1);
        let vector2 =
            test_proxima_record("vec_2", vec![0.5, 0.6, 0.7, 0.8], 1234567891).record_version(1);

        let request = VectorBatchRequest {
            collection_id: "test_collection".to_string(),
            vectors: vec![vector1.into(), vector2.into()],
        };

        assert_eq!(request.collection_id, "test_collection");
        assert_eq!(request.vectors.len(), 2);
        assert_eq!(request.vectors[0].id, "vec_1");
        assert_eq!(request.vectors[1].id, "vec_2");
    }

    #[test]
    fn test_vector_batch_request_empty_vectors() {
        use crate::proto::proximadb_v1::VectorBatchRequest;

        let request = VectorBatchRequest {
            collection_id: "empty_collection".to_string(),
            vectors: vec![],
        };

        assert_eq!(request.vectors.len(), 0);
    }

    // ==================== VectorSearchRequest Tests ====================

    #[test]
    fn test_vector_search_request_construction() {
        use crate::proto::proximadb_v1::{IncludeFields, SearchQuery, VectorSearchRequest};

        let query = SearchQuery {
            vector: vec![0.1, 0.2, 0.3, 0.4],
            filters: Default::default(),
            advanced_filter: None,
        };

        let include_fields = IncludeFields {
            vector: true,
            metadata: true,
            score: true,
            rank: false,
            source: false,
            source_options: Default::default(),
        };

        let request = VectorSearchRequest {
            collection_id: "search_collection".to_string(),
            queries: vec![query],
            top_k: 10,
            include_fields: Some(include_fields),
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        assert_eq!(request.collection_id, "search_collection");
        assert_eq!(request.queries.len(), 1);
        assert_eq!(request.top_k, 10);
        assert!(request.include_fields.as_ref().unwrap().vector);
        assert!(request.include_fields.as_ref().unwrap().metadata);
    }

    #[test]
    fn test_vector_search_request_multiple_queries() {
        use crate::proto::proximadb_v1::{SearchQuery, VectorSearchRequest};

        let queries: Vec<SearchQuery> = (0..5)
            .map(|_i| SearchQuery {
                vector: vec![0.1, 0.2, 0.3],
                filters: Default::default(),
                advanced_filter: None,
            })
            .collect();

        let request = VectorSearchRequest {
            collection_id: "multi_query".to_string(),
            queries,
            top_k: 5,
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        assert_eq!(request.queries.len(), 5);
    }

    // ==================== ProximaRecord Metadata Tests ====================

    #[test]
    fn test_proxima_record_with_metadata() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "category".to_string(),
            proximadb_data_model::ProximaValue::String("electronics".to_string()),
        );
        metadata.insert(
            "price".to_string(),
            proximadb_data_model::ProximaValue::Float64(99.99),
        );
        metadata.insert(
            "in_stock".to_string(),
            proximadb_data_model::ProximaValue::Boolean(true),
        );

        let record = test_proxima_record("product_1", vec![0.1, 0.2, 0.3], 1234567890)
            .record_version(1)
            .origin("product_catalog")
            .metadata(metadata);

        assert_eq!(record.props.len(), 3);
        assert!(record.props.contains_key("category"));
        assert!(record.props.contains_key("price"));
        assert!(record.props.contains_key("in_stock"));
    }

    // ==================== CollectionConfig Validation Tests ====================

    #[test]
    fn test_collection_config_defaults() {
        use crate::proto::proximadb_v1::CollectionConfig;

        let config = CollectionConfig {
            name: "minimal_collection".to_string(),
            dimension: 256,
            ..Default::default()
        };

        assert_eq!(config.name, "minimal_collection");
        assert_eq!(config.dimension, 256);
        assert!(config.distance_metric.is_none()); // Optional, server applies default
        assert!(config.storage_engine.is_none()); // Optional, server applies default
    }

    #[test]
    fn test_collection_config_with_all_options() {
        use crate::proto::proximadb_v1::{CollectionConfig, DistanceMetric, StorageEngine};

        let config = CollectionConfig {
            name: "full_config_collection".to_string(),
            dimension: 768,
            distance_metric: Some(DistanceMetric::Euclidean as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            tags: vec!["production".to_string(), "ml".to_string()],
            description: Some("A fully configured collection".to_string()),
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            storage_config: None,
            primary_index: Some("hnsw_index".to_string()),
            auto_index_selection: Some(true),
            owner: Some("team_ml".to_string()),
            embedding_models: vec!["openai-ada-002".to_string()],
            record_schema: None,
            enable_proxima_record: Some(false),
            text_columns: vec![],
            text_storage_configs: vec![],
            enable_dual_use_embeddings: None,
            canonical_embedding_precision: None,
        };

        assert_eq!(config.dimension, 768);
        assert_eq!(config.tags.len(), 2);
        assert_eq!(
            config.description,
            Some("A fully configured collection".to_string())
        );
        assert_eq!(config.owner, Some("team_ml".to_string()));
    }

    // ==================== Error Response Tests ====================

    #[test]
    fn test_collection_response_error_codes() {
        use crate::proto::proximadb_v1::CollectionResponse;

        // Test NOT_FOUND error response
        let response = CollectionResponse {
            success: false,
            operation: 3, // CollectionGet
            collection: None,
            collections: vec![],
            affected_count: 0,
            total_count: 0,
            metadata: Default::default(),
            error_message: Some("Collection not found".to_string()),
            error_code: Some("NOT_FOUND".to_string()),
            processing_time_us: 1000,
        };

        assert!(!response.success);
        assert_eq!(response.error_code, Some("NOT_FOUND".to_string()));
    }

    #[test]
    fn test_vector_operation_response_error() {
        use crate::proto::proximadb_v1::{VectorOperationResponse, VectorServiceOperation};

        let response = VectorOperationResponse {
            success: false,
            operation: VectorServiceOperation::VsBatch as i32,
            metrics: None,
            results: None,
            vector_ids: vec![],
            error_message: Some("Vector insert failed: invalid dimension".to_string()),
            error_code: Some("VECTOR_INSERT_FAILED".to_string()),
        };

        assert!(!response.success);
        assert_eq!(
            response.error_code,
            Some("VECTOR_INSERT_FAILED".to_string())
        );
        assert!(
            response
                .error_message
                .as_ref()
                .unwrap()
                .contains("invalid dimension")
        );
    }

    // ==================== Input Validation Edge Cases ====================

    #[test]
    fn test_empty_collection_name() {
        use crate::proto::proximadb_v1::CollectionConfig;

        let config = CollectionConfig {
            name: "".to_string(), // Empty name - should be validated by handler
            dimension: 128,
            ..Default::default()
        };

        assert!(config.name.is_empty());
    }

    #[test]
    fn test_zero_dimension() {
        use crate::proto::proximadb_v1::CollectionConfig;

        let config = CollectionConfig {
            name: "zero_dim_collection".to_string(),
            dimension: 0, // Zero dimension - should be validated by handler
            ..Default::default()
        };

        assert_eq!(config.dimension, 0);
    }

    #[test]
    fn test_very_large_dimension() {
        use crate::proto::proximadb_v1::CollectionConfig;

        let config = CollectionConfig {
            name: "large_dim_collection".to_string(),
            dimension: 65536, // Very large dimension
            ..Default::default()
        };

        assert_eq!(config.dimension, 65536);
    }

    #[test]
    fn test_special_characters_in_collection_name() {
        use crate::proto::proximadb_v1::CollectionConfig;

        let config = CollectionConfig {
            name: "my-collection_v2.0".to_string(), // Special chars
            dimension: 128,
            ..Default::default()
        };

        assert_eq!(config.name, "my-collection_v2.0");
    }

    #[test]
    fn test_unicode_in_metadata() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "description".to_string(),
            proximadb_data_model::ProximaValue::String("Unicode test".to_string()),
        );
        metadata.insert(
            "emoji".to_string(),
            proximadb_data_model::ProximaValue::String("Test data".to_string()),
        );

        let record = test_proxima_record("unicode_test", vec![0.1, 0.2], 0).metadata(metadata);

        assert_eq!(record.props.len(), 2);
    }

    // ==================== Search Parameters Tests ====================

    #[test]
    fn test_search_params_defaults() {
        use crate::proto::proximadb_v1::SearchParams;

        let params = SearchParams {
            top_k: None,
            accuracy_threshold: None,
            include_expired: None,
            timeout_ms: None,
            enable_two_stage: None,
            enable_clustering_hint: None,
            enable_metadata_filtering_hint: None,
            custom_hints: Default::default(),
        };

        assert!(params.top_k.is_none());
        assert!(params.accuracy_threshold.is_none());
        assert!(params.timeout_ms.is_none());
    }

    #[test]
    fn test_search_params_with_values() {
        use crate::proto::proximadb_v1::SearchParams;

        let params = SearchParams {
            top_k: Some(100),
            accuracy_threshold: Some(0.95),
            include_expired: Some(false),
            timeout_ms: Some(5000),
            enable_two_stage: Some(true),
            enable_clustering_hint: Some(true),
            enable_metadata_filtering_hint: Some(false),
            custom_hints: Default::default(),
        };

        assert_eq!(params.top_k, Some(100));
        assert_eq!(params.accuracy_threshold, Some(0.95));
        assert_eq!(params.timeout_ms, Some(5000));
    }

    // ==================== SQL Value Roundtrip Tests ====================

    #[test]
    fn test_sql_value_roundtrip_string() {
        let original = serde_json::json!("test string");
        let sql_value = UnifiedHandlers::json_to_sql_value(&original);
        let roundtrip = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(original, roundtrip);
    }

    #[test]
    fn test_sql_value_roundtrip_number() {
        let original = serde_json::json!(123.456);
        let sql_value = UnifiedHandlers::json_to_sql_value(&original);
        let roundtrip = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(original, roundtrip);
    }

    #[test]
    fn test_sql_value_roundtrip_bool() {
        let original = serde_json::json!(true);
        let sql_value = UnifiedHandlers::json_to_sql_value(&original);
        let roundtrip = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(original, roundtrip);
    }

    #[test]
    fn test_sql_value_roundtrip_null() {
        let original = serde_json::Value::Null;
        let sql_value = UnifiedHandlers::json_to_sql_value(&original);
        let roundtrip = UnifiedHandlers::sql_value_to_json(&sql_value);
        assert_eq!(original, roundtrip);
    }

    // ==================== OperationMetrics Tests ====================

    #[test]
    fn test_operation_metrics_construction() {
        use crate::proto::proximadb_v1::OperationMetrics;

        let metrics = OperationMetrics {
            total_processed: 100,
            successful_count: 95,
            failed_count: 5,
            updated_count: 10,
            processing_time_us: 50000,
            wal_write_time_us: 1000,
            index_update_time_us: 5000,
        };

        assert_eq!(metrics.total_processed, 100);
        assert_eq!(metrics.successful_count, 95);
        assert_eq!(metrics.failed_count, 5);
        assert_eq!(metrics.processing_time_us, 50000);
    }

    // ==================== CollectionIdCache TTL Expiry Tests ====================

    #[test]
    fn test_collection_id_cache_ttl_expiry() {
        // Use a 0-second TTL so entries expire immediately
        let cache = CollectionIdCache::with_ttl(Duration::from_millis(0));
        cache.insert("key".to_string(), "value".to_string());

        // Entry should be expired on next get (TTL = 0ms)
        std::thread::sleep(Duration::from_millis(1));
        assert!(
            cache.get("key").is_none(),
            "Expired entry should return None"
        );
    }

    #[test]
    fn test_collection_id_cache_fresh_entry_within_ttl() {
        let cache = CollectionIdCache::with_ttl(Duration::from_secs(60));
        cache.insert("fresh_key".to_string(), "fresh_value".to_string());

        // Should still be valid
        assert_eq!(cache.get("fresh_key"), Some("fresh_value".to_string()));
    }

    #[test]
    fn test_collection_id_cache_evict_expired_via_insert() {
        // Use tiny TTL so entries expire quickly
        let cache = CollectionIdCache {
            cache: std::sync::RwLock::new(HashMap::new()),
            ttl: Duration::from_millis(0),
            max_size: 2,
        };

        cache.insert("a".to_string(), "1".to_string());
        cache.insert("b".to_string(), "2".to_string());

        // Wait for entries to expire
        std::thread::sleep(Duration::from_millis(1));

        // Inserting a new entry should evict expired ones first
        cache.insert("c".to_string(), "3".to_string());

        // Expired entries should be gone
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_none());
        // New entry might also be expired given 0ms TTL; the important thing
        // is that insert didn't panic and the cache didn't grow unbounded
    }

    #[test]
    fn test_collection_id_cache_max_size_eviction() {
        // Create a cache with max_size=3 and long TTL (entries won't expire)
        let cache = CollectionIdCache {
            cache: std::sync::RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(3600),
            max_size: 3,
        };

        cache.insert("a".to_string(), "1".to_string());
        cache.insert("b".to_string(), "2".to_string());
        cache.insert("c".to_string(), "3".to_string());

        // Cache is at max. Next insert should trigger eviction of half.
        cache.insert("d".to_string(), "4".to_string());

        // The new entry should be present
        assert_eq!(cache.get("d"), Some("4".to_string()));

        // Some old entries should have been evicted (half = 1 entry removed from 3)
        let cache_guard = cache.cache.read().unwrap();
        // After eviction of half (1) + insert of "d", we should have 3 entries
        assert!(cache_guard.len() <= 3);
    }

    #[test]
    fn test_collection_id_cache_invalidate_nonexistent() {
        let cache = CollectionIdCache::new();
        // Should not panic when invalidating a key that doesn't exist
        cache.invalidate("nonexistent_key");
        assert!(cache.get("nonexistent_key").is_none());
    }

    #[test]
    fn test_collection_id_cache_clear_empty() {
        let cache = CollectionIdCache::new();
        // Clearing an empty cache should not panic
        cache.clear();
        assert!(cache.get("any_key").is_none());
    }

    #[test]
    fn test_collection_id_cache_insert_same_key_updates_timestamp() {
        let cache = CollectionIdCache::with_ttl(Duration::from_secs(60));
        cache.insert("key".to_string(), "old_value".to_string());

        // Re-insert with new value (and new timestamp)
        cache.insert("key".to_string(), "new_value".to_string());

        // Should get the new value
        assert_eq!(cache.get("key"), Some("new_value".to_string()));
    }

    #[test]
    fn test_collection_id_cache_invalidate_removes_all_matching_values() {
        let cache = CollectionIdCache::new();
        cache.insert("name_a".to_string(), "id_x".to_string());
        cache.insert("name_b".to_string(), "id_x".to_string());
        cache.insert("name_c".to_string(), "id_y".to_string());

        // Invalidate by value "id_x" - should remove name_a and name_b
        cache.invalidate("id_x");

        assert!(cache.get("name_a").is_none());
        assert!(cache.get("name_b").is_none());
        // name_c maps to a different value, should remain
        assert_eq!(cache.get("name_c"), Some("id_y".to_string()));
    }

    // ==================== generate_request_id Stress Tests ====================

    #[test]
    fn test_generate_request_id_many_unique() {
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let id = generate_request_id();
            assert!(ids.insert(id), "Duplicate request ID generated");
        }
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn test_generate_request_id_timestamp_portion_is_hex() {
        let id = generate_request_id();
        let timestamp_hex = &id[..8];
        assert!(
            u32::from_str_radix(timestamp_hex, 16).is_ok(),
            "Timestamp portion should be valid hex"
        );
    }

    // ==================== COLLECTION_ID_CACHE Constants Tests ====================

    #[test]
    fn test_cache_constants() {
        assert_eq!(COLLECTION_ID_CACHE_TTL_SECS, 300);
        assert_eq!(COLLECTION_ID_CACHE_MAX_SIZE, 1000);
    }

    #[test]
    fn test_collection_id_cache_default_uses_correct_ttl() {
        let cache = CollectionIdCache::new();
        assert_eq!(cache.ttl, Duration::from_secs(300));
        assert_eq!(cache.max_size, 1000);
    }
}

/// SQL query result structure
#[derive(Debug)]
pub struct SqlQueryResult {
    /// JSON-encoded result rows.
    pub rows: Vec<serde_json::Value>,
    /// Column definitions as (name, type_name) pairs.
    pub columns: Vec<(String, String)>,
    /// Total number of rows returned.
    pub row_count: usize,
    /// Query execution wall-clock time in milliseconds.
    pub execution_time_ms: u64,
}

// ── ApiHandlersPort implementation ────────────────────────────────────────────
//
// Bridges the platform-level `ApiHandlersPort` trait (defined in `proximadb-runtime`)
// to the concrete `UnifiedHandlers` business logic. Protocol adapters in
// `proximadb-api` hold an `Arc<dyn ApiHandlersPort>` and call through this seam,
// so they never import root-crate concrete types directly.

// TD-104 S3: bulk record operations port for the Arrow Flight ingest path.
// Delegates to the existing `handle_record_*_for_tenant` business logic so the
// Flight service can hold an `Arc<dyn RecordOpsPort>` instead of the concrete
// `UnifiedHandlers`.
#[async_trait::async_trait]
impl proximadb_runtime::RecordOpsPort for UnifiedHandlers {
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

#[async_trait::async_trait]
impl proximadb_runtime::ApiHandlersPort for UnifiedHandlers {
    async fn handle_collection_operation_for_tenant(
        &self,
        request: crate::proto::proximadb_v1::CollectionRequest,
        tenant_id: Option<&str>,
    ) -> anyhow::Result<crate::proto::proximadb_v1::CollectionResponse> {
        // Inherent method has the same signature — delegate directly.
        UnifiedHandlers::handle_collection_operation_for_tenant(self, request, tenant_id).await
    }

    async fn handle_vector_search_v1_for_tenant(
        &self,
        request: crate::proto::proximadb_v1::VectorSearchRequest,
        tenant_id: Option<&str>,
    ) -> anyhow::Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        UnifiedHandlers::handle_vector_search_v1_for_tenant(self, request, tenant_id).await
    }

    async fn handle_vector_search_v1(
        &self,
        request: crate::proto::proximadb_v1::VectorSearchRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        UnifiedHandlers::handle_vector_search_v1(self, request).await
    }

    async fn handle_vector_batch_v1_for_tenant(
        &self,
        request: crate::proto::proximadb_v1::VectorBatchRequest,
        tenant_id: Option<&str>,
    ) -> anyhow::Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        UnifiedHandlers::handle_vector_batch_v1_for_tenant(self, request, tenant_id).await
    }

    async fn handle_vector_v1_for_tenant(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
        tenant_id: Option<&str>,
    ) -> anyhow::Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        UnifiedHandlers::handle_vector_v1_for_tenant(
            self,
            collection_id,
            vector_id,
            include_vector,
            include_metadata,
            tenant_id,
        )
        .await
    }

    async fn execute_hybrid_query(
        &self,
        request: crate::proto::proximadb_v1::HybridSearchRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::HybridSearchResponse> {
        UnifiedHandlers::execute_hybrid_query(self, request).await
    }

    async fn execute_sql_v1(
        &self,
        query: String,
        parameters: Option<Vec<proximadb_data_model::ProximaValue>>,
        collection: Option<String>,
    ) -> anyhow::Result<crate::proto::proximadb_v1::ExecuteQueryResponse> {
        let legacy_parameters = parameters.map(|values| {
            values
                .iter()
                .map(proximadb_records::conversions::proxima_to_sql_value)
                .collect()
        });
        // ApiHandlersPort::execute_sql_v1 carries no tenant; relational SQL stays
        // unscoped on this trait path (callers that need TD-064 scoping use the
        // inherent method with a tenant — e.g. the gRPC ExecuteQuery handler).
        UnifiedHandlers::execute_sql_v1(self, query, legacy_parameters, collection, None).await
    }
}

#[cfg(test)]
mod wal_lane_enforcement_tests {
    use super::*;

    #[test]
    fn enforce_wal_lane_accepts_typical_row_level_insert() {
        // A modest INSERT batch routes to WalCurrentState and must be accepted.
        let result = enforce_wal_lane_for_record_batch(
            "tenants/t1/collections/c1",
            WriteOperationKind::Insert,
            10,
            "unit test",
        );
        assert!(
            result.is_ok(),
            "row-level INSERT must be accepted by WAL-lane enforcement: {result:?}"
        );
    }

    #[test]
    fn enforce_wal_lane_accepts_typical_upsert() {
        let result = enforce_wal_lane_for_record_batch(
            "tenants/t1/collections/c1",
            WriteOperationKind::Upsert,
            5,
            "unit test",
        );
        assert!(
            result.is_ok(),
            "row-level UPSERT must be accepted by WAL-lane enforcement: {result:?}"
        );
    }

    #[test]
    fn enforce_wal_lane_accepts_typical_delete() {
        let result = enforce_wal_lane_for_record_batch(
            "tenants/t1/collections/c1",
            WriteOperationKind::Delete,
            3,
            "unit test",
        );
        assert!(
            result.is_ok(),
            "row-level DELETE must be accepted by WAL-lane enforcement: {result:?}"
        );
    }

    #[test]
    fn enforce_wal_lane_rejects_projection_refresh() {
        // ProjectionRefresh always routes to ProjectionOnly regardless of durability;
        // the fast-lane REST/gRPC adapter does not commit projection state, so reject explicitly
        // rather than letting the call silently fall through to the wrong lane.
        let result = enforce_wal_lane_for_record_batch(
            "tenants/t1/collections/c1",
            WriteOperationKind::ProjectionRefresh,
            10,
            "unit test",
        );
        assert!(
            result.is_err(),
            "ProjectionRefresh must be rejected by fast-lane WAL enforcement"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("unit test"),
            "rejection message should carry the call-site context: {msg}"
        );
    }
}
