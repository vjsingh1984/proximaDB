//! Aligned REST API handlers using protobuf-first approach
//!
//! These handlers demonstrate the proper pattern for REST APIs that:
//! 1. Accept protobuf types directly as JSON
//! 2. Return protobuf responses as JSON
//! 3. Use unified ApiError for consistent error handling

use axum::{
    extract::{Extension, Json, Path, Query, State},
    http::StatusCode,
    response::Json as JsonResponse,
};
use proximadb_api::rest::v1::add_rest_v1_deprecation_headers;
use proximadb_graph_query::service::GraphExecutionService;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::api_handlers::UnifiedHandlers;
use crate::errors::{ApiError, ApiResult};
use crate::network::middleware::tenant::TenantContext;
use crate::network::rest::health;
use crate::network::rest::v1::analytics::{self, AnalyticsApiState};
use crate::network::rest::v1::aql::{self, AqlApiState};
use crate::network::rest::v1::nl::{self, NlApiState};
use crate::proto::proximadb_v1;
use crate::query::QueryFacadeAdapter;
use crate::query::aql::executor::AqlExecutor;
use crate::query::aql::sources::document::DocumentAqlSource;
use crate::query::aql::sources::graph::GraphAqlSource;
use crate::query::aql::sources::observability::ObservabilityAqlSource;
use crate::query::aql::sources::vector::VectorAqlSource;
use serde::{Deserialize, Serialize};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    /// Shared unified handlers for business logic delegation
    pub request_handlers: Arc<UnifiedHandlers>,
    /// Extracted graph execution capability for query planning/execution helpers
    pub graph_execution_service: Arc<dyn GraphExecutionService>,
    /// Optional security coordinator for authentication/authorization
    pub security_coordinator: Option<Arc<crate::security::SecurityCoordinator>>,
    /// Data directory from config (e.g., server.data_dir from TOML)
    pub data_dir: std::path::PathBuf,
    /// Query facade adapter for unified query execution
    /// Optional for backward compatibility during feature flag transition
    pub query_adapter: Option<Arc<QueryFacadeAdapter>>,
    /// Per-collection full-text indices for hybrid BM25+vector search
    pub fulltext_indexes: Option<FullTextIndexMap>,
    /// Catalog manager for external catalog integration
    pub catalog_manager: Arc<crate::catalog::CatalogManager>,
    /// Phase 6: per-collection pinning registry. Set from
    /// `SharedServices.pin_registry` so the REST handlers and the
    /// AxisTieringManager consumer share the same Arc.
    pub pin_registry: Arc<crate::storage::collection_pinning::CollectionPinRegistry>,
    /// Phase 7.2.4: per-collection cache-affinity registry. Set from
    /// `SharedServices.affinity_registry` so the REST operator
    /// endpoints and the `VectorOperationsService` data-plane
    /// recorder share the same `Arc`.
    pub affinity_registry: Arc<crate::cluster::cache_affinity::CacheAffinityRegistry>,
    /// Slice 3 of tenant-pod-affinity: per-(tenant, collection)
    /// primary-pod registry. Set from `SharedServices.primary_pod_registry`
    /// so the REST operator endpoints and the future gateway write
    /// router share the same `Arc`. The REST handlers in
    /// `crate::network::rest::v1::primary_pod` gate every read/write
    /// behind an operator-permission check — see
    /// [`crate::network::rest::v1::primary_pod::authorize_operator`].
    pub primary_pod_registry: Arc<crate::cluster::primary_pod_registry::PrimaryPodRegistry>,

    /// Slice 4 of tenant-pod-affinity: this pod's identity, used by
    /// the gateway write-router gate to compare against bindings in
    /// `primary_pod_registry`. Resolved via
    /// [`crate::cluster::primary_pod_registry::resolve_self_pod_id`]:
    /// explicit config override → `PROXIMADB_POD_ID` env var →
    /// `"self"` fallback. Stored as a plain `String` (not `Arc`)
    /// because it's cheap to clone and immutable for the process
    /// lifetime.
    pub self_pod_id: String,
    /// PAX segment registry shared with the write path (gRPC v2, Arrow Flight).
    /// Enables Iceberg REST snapshot summaries to reflect real PAX segment stats.
    pub segment_registry: Arc<crate::catalog::SegmentRegistry>,
    /// LLM engine for semantic operations
    pub llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
    /// Port-based document service (from proximadb-api migration)
    pub doc_port: Option<Arc<dyn proximadb_runtime::DocumentPort>>,
    /// Port-based graph service (from proximadb-api migration)
    pub graph_port: Option<Arc<dyn proximadb_runtime::GraphPort>>,
    /// Port-based observability service (from proximadb-api migration)
    pub obs_port: Option<Arc<dyn proximadb_runtime::ObservabilityPort>>,
    /// Port-backed unified query service (Phase 9.9)
    pub unified_query_port: Option<Arc<dyn proximadb_runtime::UnifiedQueryPort>>,
    /// Port-backed API handler for collection/vector routes (Phase 9.10).
    ///
    /// When set, `create_router` passes this as `RestAppState.handlers` so
    /// `create_collection_router` and `create_vector_router` from `proximadb-api`
    /// go through `CollectionPort`/`VectorOpsPort` trait objects rather than the
    /// concrete root-crate `UnifiedHandlers`.
    pub api_handlers: Option<Arc<dyn proximadb_runtime::ApiHandlersPort>>,
    /// Optional queue client for async ingest. When `Some`, the v3
    /// `/documents?mode=async` handler routes through `producer.send`
    /// on the `embed-ingest` topic and the embedding drainer consumes
    /// it asynchronously. When `None`, async mode degrades to inline
    /// embedding (still returns 202 but no real queue path). Production
    /// deployments wire this from `apps/proximadb-server` startup.
    pub queue_client: Option<Arc<proximadb_queue::QueueClient>>,
    /// Optional ranking framework services (R-7c). When `Some`, the
    /// `/api/v1/rank/search` route routes through the multi-phase
    /// pipeline; when `None`, the route returns 503. See
    /// `src/network/rest/v1/rank.rs` and
    /// `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md`.
    pub rank_services: Option<Arc<crate::network::rest::v1::rank::RankServices>>,

    /// Optional durable rank-profile catalog (R-7c.3 production wiring).
    /// When `Some`, the `/api/v1/rank/profiles` REST routes can install,
    /// fetch, and remove profiles end-to-end. When `None`, those routes
    /// return 503. Should always be wired alongside `rank_services` so
    /// installs reach both the catalog and the live registry.
    pub rank_profile_store: Option<Arc<dyn crate::services::RankProfileStore>>,

    /// Optional recall-probe gate (TD-064 / LLD §5). When `Some`, the v2
    /// route-health endpoint reports per-scope `gate_open` state and flips
    /// `recall_probe.live_state_in_app_state: true`. Production wires this
    /// from `SharedServices.recall_probe_gate` via `with_recall_probe_gate`
    /// in `src/network/multi_server.rs`. Search-path consultation
    /// (`wired_to_query_path: true`) is a separate follow-up; this slot
    /// only proves the gate is reachable from request handlers.
    pub recall_probe_gate: Option<Arc<crate::catalog::RecallProbeGate>>,

    /// Phase 8 (F1) Continuous Discovery service. Wired from
    /// `SharedServices.discovery_service` via `with_discovery_service` in
    /// `src/network/multi_server.rs` so the v2 `discovery-jobs` endpoints reach
    /// the same registry the background executor consumes.
    pub discovery_service: Option<Arc<crate::services::discovery::DiscoveryService>>,
}

impl AppState {
    /// Create REST app state with the standard shared runtime components.
    pub fn new(
        request_handlers: Arc<UnifiedHandlers>,
        graph_execution_service: Arc<dyn GraphExecutionService>,
        security_coordinator: Option<Arc<crate::security::SecurityCoordinator>>,
        data_dir: std::path::PathBuf,
        query_adapter: Option<Arc<QueryFacadeAdapter>>,
        llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
    ) -> Self {
        Self {
            request_handlers,
            graph_execution_service,
            security_coordinator,
            data_dir,
            query_adapter,
            fulltext_indexes: Some(Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            ))),
            catalog_manager: Arc::new(crate::catalog::CatalogManager::new()),
            // Default: standalone pin registry. Production wires
            // SharedServices.pin_registry via `with_pin_registry` so
            // REST handlers and the eventual AxisTieringManager
            // consumer share the same Arc.
            pin_registry: crate::storage::collection_pinning::new_shared(),
            // Default: standalone affinity registry. Production wires
            // SharedServices.affinity_registry via `with_affinity_registry`
            // so REST handlers and the search-path recorder share the
            // same Arc.
            affinity_registry: crate::cluster::cache_affinity::new_shared(),
            // Default: standalone primary-pod registry. Production
            // wires `SharedServices.primary_pod_registry` so REST
            // handlers and the future gateway write router share the
            // same `Arc`.
            primary_pod_registry: crate::cluster::primary_pod_registry::new_shared(),
            // Default pod identity: resolve from env var or fall
            // back to `"self"`. Production may override via
            // `with_self_pod_id` once the config field lands.
            self_pod_id: crate::cluster::primary_pod_registry::resolve_self_pod_id(None),
            segment_registry: Arc::new(crate::catalog::SegmentRegistry::new()),
            llm_engine,
            doc_port: None,
            graph_port: None,
            obs_port: None,
            unified_query_port: None,
            api_handlers: None,
            queue_client: None,
            rank_services: None,
            rank_profile_store: None,
            recall_probe_gate: None,
            discovery_service: None,
        }
    }

    /// Inject the process-wide pin registry (Phase 6 control surface).
    /// Wired from `SharedServices.pin_registry` in production so REST
    /// handlers and the eventual `AxisTieringManager` consumer share
    /// the same Arc.
    pub fn with_pin_registry(
        mut self,
        registry: Arc<crate::storage::collection_pinning::CollectionPinRegistry>,
    ) -> Self {
        self.pin_registry = registry;
        self
    }

    /// Inject the process-wide cache-affinity registry (Phase 7.2.4).
    /// Wired from `SharedServices.affinity_registry` so REST operator
    /// endpoints and the `VectorOperationsService` recorder share
    /// the same Arc.
    pub fn with_affinity_registry(
        mut self,
        registry: Arc<crate::cluster::cache_affinity::CacheAffinityRegistry>,
    ) -> Self {
        self.affinity_registry = registry;
        self
    }

    /// Inject the process-wide primary-pod registry (Slice 3 of
    /// tenant-pod-affinity). Wired from
    /// `SharedServices.primary_pod_registry` so REST operator
    /// endpoints and the future gateway write router share the same
    /// `Arc`.
    pub fn with_primary_pod_registry(
        mut self,
        registry: Arc<crate::cluster::primary_pod_registry::PrimaryPodRegistry>,
    ) -> Self {
        self.primary_pod_registry = registry;
        self
    }

    /// Override the resolved self-pod identity (Slice 4 of
    /// tenant-pod-affinity). Production wires this when the config
    /// field or env var sets a known pod_id; tests inject deterministic
    /// values so the write-routing gate behaves predictably.
    pub fn with_self_pod_id(mut self, pod_id: impl Into<String>) -> Self {
        self.self_pod_id = pod_id.into();
        self
    }

    /// Inject the shared full-text index map (T3.2 Slice 1b).
    /// Production wires this from `SharedServices.fulltext_indexes` so
    /// REST `/api/v1/hybrid/search` and gRPC `hybrid_search` share the
    /// same in-process map — an indexed document is searchable on
    /// both protocols.
    pub fn with_fulltext_indexes(mut self, indexes: FullTextIndexMap) -> Self {
        self.fulltext_indexes = Some(indexes);
        self
    }

    /// Inject the ranking framework services bundle. Production wires
    /// this from `apps/proximadb-server` startup after constructing the
    /// `ProfileRegistry`, registering built-in features into the
    /// `BlueprintFactory`, and selecting a `CandidateProvider` impl
    /// (production = adapter over the hybrid coordinator; tests +
    /// pre-R-7c.1 deployments = `MockRangeCandidateProvider`).
    pub fn with_rank_services(
        mut self,
        services: Arc<crate::network::rest::v1::rank::RankServices>,
    ) -> Self {
        self.rank_services = Some(services);
        self
    }

    /// Inject the durable rank-profile catalog so the REST install / fetch /
    /// remove endpoints can persist DDL-style operations through the canonical
    /// WAL. Production wires this from `SharedServices.rank_profile_store`.
    pub fn with_rank_profile_store(
        mut self,
        store: Arc<dyn crate::services::RankProfileStore>,
    ) -> Self {
        self.rank_profile_store = Some(store);
        self
    }

    /// Inject a queue client for async ingest. Production wires this
    /// from `apps/proximadb-server` startup after opening the queue
    /// subsystem at the configured root path.
    pub fn with_queue_client(mut self, client: Arc<proximadb_queue::QueueClient>) -> Self {
        self.queue_client = Some(client);
        self
    }

    /// Inject a shared segment registry (same `Arc` as in `SharedServices`).
    pub fn with_segment_registry(mut self, registry: Arc<crate::catalog::SegmentRegistry>) -> Self {
        self.segment_registry = registry;
        self
    }

    /// Inject the shared xCatalog manager from the server composition root.
    pub fn with_catalog_manager(mut self, manager: Arc<crate::catalog::CatalogManager>) -> Self {
        self.catalog_manager = manager;
        self
    }

    /// Inject the shared recall-probe gate from `SharedServices`. When set,
    /// `/api/v2/_diagnostics/collections/:id/route-health` resolves
    /// per-scope gate state and reports `recall_probe.gate_open` +
    /// `recall_probe.live_state_in_app_state: true`. Search-path
    /// consultation (`wired_to_query_path: true`) is separate.
    pub fn with_recall_probe_gate(mut self, gate: Arc<crate::catalog::RecallProbeGate>) -> Self {
        self.recall_probe_gate = Some(gate);
        self
    }

    /// Inject the Phase 8 Continuous Discovery service (F1) so the v2
    /// `discovery-jobs` endpoints can create/inspect jobs.
    pub fn with_discovery_service(
        mut self,
        discovery_service: Arc<crate::services::discovery::DiscoveryService>,
    ) -> Self {
        self.discovery_service = Some(discovery_service);
        self
    }

    /// Inject port-based service objects for API-crate-backed routes.
    pub fn with_ports(
        mut self,
        doc_port: Arc<dyn proximadb_runtime::DocumentPort>,
        graph_port: Arc<dyn proximadb_runtime::GraphPort>,
        obs_port: Arc<dyn proximadb_runtime::ObservabilityPort>,
    ) -> Self {
        self.doc_port = Some(doc_port);
        self.graph_port = Some(graph_port);
        self.obs_port = Some(obs_port);
        self
    }

    /// Inject unified query port (Phase 9.9).
    pub fn with_unified_query_port(
        mut self,
        port: Arc<dyn proximadb_runtime::UnifiedQueryPort>,
    ) -> Self {
        self.unified_query_port = Some(port);
        self
    }

    /// Inject port-backed API handler for collection/vector routes (Phase 9.10).
    pub fn with_api_handlers(
        mut self,
        handlers: Arc<dyn proximadb_runtime::ApiHandlersPort>,
    ) -> Self {
        self.api_handlers = Some(handlers);
        self
    }

    /// Create health-check state from the same explicit REST capability view.
    pub fn health_state(&self) -> health::HealthState {
        health::HealthState::new(
            self.request_handlers.clone(),
            self.graph_execution_service.clone(),
        )
    }
}

/// SQL query request structure
#[derive(Debug, Serialize, Deserialize)]
pub struct SqlQueryRequest {
    /// SQL query string
    pub query: String,
    /// Optional parameters for parameterized queries (proto-aligned)
    pub parameters: Option<Vec<proximadb_v1::SqlValue>>,
    /// Optional collection to use as default context
    pub collection: Option<String>,
    /// Optional timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Optional seeding strategy for hybrid (average | per_seed | none)
    pub seeding: Option<String>,
}

fn sql_params_to_proxima_values(
    parameters: Option<Vec<proximadb_v1::SqlValue>>,
) -> Option<Vec<proximadb_data_model::ProximaValue>> {
    parameters.map(|values| {
        values
            .iter()
            .map(proximadb_records::conversions::sql_value_to_proxima)
            .collect()
    })
}

/// ADR-012 graph branch merge endpoint — extractor shim.
///
/// All actual logic lives in [`merge_graph_branch_inner`] so integration
/// tests can drive it through their own minimal axum Router without
/// constructing a full `AppState`. See
/// `tests/graph_branch_merge_rest_integration_test.rs`.
pub async fn merge_graph_branch(
    State(state): State<AppState>,
    Path((collection, branch)): Path<(String, String)>,
    Json(request): Json<GraphBranchMergeRequest>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    merge_graph_branch_inner(&state.data_dir, &collection, &branch, request).await
}

/// Pure-logic core of the branch-merge handler. Decoupled from `AppState`
/// so it can be driven from integration tests (and any future REST
/// endpoint that needs the same logic) without service-stub scaffolding.
///
/// Reads the canonical WAL under `data_dir/pgwire/canonical-records.wal`,
/// filters by `collection`, runs `merge_branches`, and (if `!dry_run`)
/// writes the resolutions back through `write_back_merge` with
/// `origin = "branch_merge:<branch>:<request.target_branch>"`.
pub async fn merge_graph_branch_inner(
    data_dir: &std::path::Path,
    collection: &str,
    branch: &str,
    request: GraphBranchMergeRequest,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    if collection.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "collection path parameter must not be empty".to_string(),
        ));
    }
    if branch.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "branch path parameter must not be empty".to_string(),
        ));
    }
    if request.target_branch.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "target_branch must not be empty".to_string(),
        ));
    }

    let wal_path = graph_branch_merge_wal_path(data_dir);
    if !tokio::fs::try_exists(&wal_path).await.map_err(|err| {
        ApiError::Internal(format!(
            "checking canonical WAL {} failed: {}",
            wal_path.display(),
            err
        ))
    })? {
        return Err(ApiError::NotFound(format!(
            "canonical WAL not found at {}",
            wal_path.display()
        )));
    }

    let entries = crate::services::FramedTableWalAppender::read_entries_from_path(&wal_path)
        .await
        .map_err(|err| {
            ApiError::Internal(format!(
                "reading canonical WAL {} failed: {}",
                wal_path.display(),
                err
            ))
        })?;
    let collection_entries = filter_canonical_wal_for_collection(entries, collection);
    let report =
        crate::graph::merge::merge_branches(&collection_entries, branch, &request.target_branch)
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "no mergeable branch entries found for collection '{}' source '{}' target '{}'",
                    collection, branch, request.target_branch
                ))
            })?;

    // If not dry-run, write the merged records to WAL
    let write_back_result = if !request.dry_run {
        match crate::graph::merge::write_back_merge(
            &collection_entries,
            &report,
            &wal_path,
            collection,
            branch,
            &request.target_branch,
            request.tenant_id.clone(),
        )
        .await
        {
            Ok(Some(result)) => Some(result),
            Ok(None) => None, // Nothing to write
            Err(err) => {
                return Err(ApiError::Internal(format!(
                    "merge write-back failed: {}",
                    err
                )));
            }
        }
    } else {
        None
    };

    Ok(JsonResponse(serde_json::json!({
        "collection": collection,
        "source_branch": branch,
        "target_branch": request.target_branch,
        "dry_run": request.dry_run,
        "merge_base_lsn": report.merge_base_lsn,
        "left_events": report.left_events,
        "right_events": report.right_events,
        "conflicts": report.conflicts,
        "resolutions": report.resolutions,
        "summary": {
            "left_event_count": report.left_events.len(),
            "right_event_count": report.right_events.len(),
            "conflict_count": report.conflicts.len(),
            "resolution_count": report.resolutions.len(),
            "wal_path": wal_path.display().to_string()
        },
        "write_back": match write_back_result {
            Some(result) => serde_json::json!({
                "first_lsn": result.first_lsn,
                "last_lsn": result.last_lsn,
                "entries_written": result.written_entries.len()
            }),
            None => serde_json::json!(null)
        }
    })))
}

fn graph_branch_merge_wal_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("pgwire").join("canonical-records.wal")
}

fn filter_canonical_wal_for_collection(
    entries: Vec<proximadb_storage_common::CanonicalWalEntry>,
    collection: &str,
) -> Vec<proximadb_storage_common::CanonicalWalEntry> {
    entries
        .into_iter()
        .filter(|entry| match &entry.operation {
            proximadb_storage_common::CanonicalOperation::RecordUpsert {
                collection_id, ..
            } => collection_id == collection,
            proximadb_storage_common::CanonicalOperation::RecordDelete {
                collection_id, ..
            } => collection_id == collection,
            proximadb_storage_common::CanonicalOperation::Checkpoint(_)
            | proximadb_storage_common::CanonicalOperation::CdcBarrier { .. } => false,
        })
        .collect()
}

// SQL query response structure
// For REST, we now return proximadb.v1 ExecuteSqlResponse directly, wrapped by ProtoApiResponse
/// Column information in SQL results
#[derive(Debug, Serialize, Deserialize)]
pub struct SqlColumnInfo {
    /// Column name
    pub name: String,
    /// Column data type
    pub data_type: String,
}

#[derive(Debug, Deserialize)]
pub struct CatalogRoutingQuery {
    pub table_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TableWriteExplainRequest {
    pub target_table: String,
    pub source_table: Option<String>,
    pub source_sql: Option<String>,
    pub write_mode: Option<String>,
    pub distribution: Option<String>,
    pub target_columns: Option<Vec<String>>,
    pub tenant_id: Option<String>,
    pub actor: Option<String>,
    pub idempotency_key: Option<String>,
    pub row_count_hint: Option<u64>,
    pub estimated_bytes: Option<u64>,
    pub requires_row_level_semantics: Option<bool>,
    pub batch_local_constraints_sufficient: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct GraphBranchMergeRequest {
    /// Target branch to merge into. Defaults to `main`.
    #[serde(default = "default_graph_branch_merge_target")]
    pub target_branch: String,
    /// Dry-run returns the ADR-012 merge report without writing a merge commit.
    #[serde(default = "default_graph_branch_merge_dry_run")]
    pub dry_run: bool,
    /// Optional tenant ID for multi-tenant deployments.
    pub tenant_id: Option<String>,
}

fn default_graph_branch_merge_target() -> String {
    "main".to_string()
}

fn default_graph_branch_merge_dry_run() -> bool {
    true
}

/// Execute SQL query handler
///
/// Supports vector similarity queries like:
/// ```sql
/// SELECT id, metadata, COSINE_DISTANCE(embedding, [0.1, 0.2, 0.3]) as score
/// FROM my_collection
/// WHERE metadata.category = 'electronics'
/// ORDER BY score ASC
/// LIMIT 10
/// ```
pub async fn execute_sql(
    State(state): State<AppState>,
    Json(request): Json<SqlQueryRequest>,
) -> ApiResult<JsonResponse<serde_json::Value>> {
    let start_time = std::time::Instant::now();
    let request_id = Uuid::new_v4().to_string();

    info!(
        "SQL query request {} with query: {}",
        request_id,
        request.query.chars().take(100).collect::<String>()
    );

    // Validate request
    if request.query.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "SQL query cannot be empty".to_string(),
        ));
    }

    // Prefer unified query port (decoupled from concrete QueryFacadeAdapter type)
    if let Some(ref qp) = state.unified_query_port {
        debug!("Using unified query port for SQL query");
        let query_with_hint = if let Some(ref seeding) = request.seeding {
            format!(
                "-- SEEDING: {}\n{}",
                seeding.to_ascii_uppercase(),
                request.query
            )
        } else {
            request.query.clone()
        };
        return match qp
            .execute_unified_query(
                query_with_hint,
                sql_params_to_proxima_values(request.parameters.clone()),
                request.collection.clone(),
                None,
            )
            .await
        {
            Ok(port_result) => {
                let execution_time_ms = start_time.elapsed().as_millis() as u64;
                let records = port_result
                    .get("records")
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(vec![]));
                let total = port_result
                    .get("total_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let json_data = serde_json::json!({
                    "rows": records,
                    "execution_time_ms": execution_time_ms,
                    "rows_returned": total,
                    "row_count": total,
                    "request_id": request_id,
                });
                info!(
                    "SQL query {} (port) completed in {}ms",
                    request_id, execution_time_ms
                );
                Ok(JsonResponse(json_data))
            }
            Err(e) => {
                error!("SQL query {} (port) failed: {}", request_id, e);
                Err(ApiError::Internal(e.to_string()))
            }
        };
    }

    // Fallback: route through concrete query_adapter facade
    if let Some(ref adapter) = state.query_adapter {
        debug!("Using query_adapter fallback for SQL query");
        return match adapter.sql_query(&request.query).await {
            Ok(result) => {
                let execution_time_ms = start_time.elapsed().as_millis() as u64;
                let rows = match result.data {
                    crate::query::QueryResultData::Rows(rows) => rows,
                    crate::query::QueryResultData::Empty => vec![],
                    _ => vec![],
                };
                let json_data = serde_json::json!({
                    "rows": rows,
                    "execution_time_ms": execution_time_ms,
                    "rows_returned": rows.len(),
                    "row_count": rows.len(),
                    "request_id": request_id
                });
                info!(
                    "SQL query {} (adapter) completed in {}ms",
                    request_id, execution_time_ms
                );
                Ok(JsonResponse(json_data))
            }
            Err(e) => {
                error!("SQL query {} (adapter) failed: {}", request_id, e);
                Err(ApiError::Internal(e.to_string()))
            }
        };
    }

    // Legacy path: Execute through v1 path (typed params and rows)
    // Optional: read seeding strategy from HTTP header (X-Seeding-Strategy) or from request.parameters via a special key
    let _seeding_strategy = crate::query::execution::SeedingStrategy::Average; // default

    let query_with_hint = if let Some(seeding) = &request.seeding {
        let seed_upper = seeding.to_ascii_uppercase();
        format!("-- SEEDING: {}\n{}", seed_upper, request.query)
    } else {
        request.query.clone()
    };

    match state
        .request_handlers
        .execute_sql_v1(
            query_with_hint,
            request.parameters.clone(),
            request.collection,
        )
        .await
    {
        Ok(v1_resp) => {
            let execution_time_ms = start_time.elapsed().as_millis() as u64;

            // Convert SQL response to JSON value for now
            // Deferred: Create proper JsonExecuteSqlResponse wrapper if needed
            let json_data = serde_json::json!({
                "rows": v1_resp.rows.iter().map(|row| {
                    // Convert fields to a JSON object instead of list of key/value pairs
                    let mut obj = serde_json::Map::new();
                    for field in &row.fields {
                        let value = field.value.as_ref().map_or(serde_json::Value::Null, sql_value_to_json);
                        obj.insert(field.key.clone(), value);
                    }
                    serde_json::Value::Object(obj)
                }).collect::<Vec<_>>(),
                "columns": v1_resp.columns,
                "column_types": v1_resp.column_types,
                "execution_time_ms": execution_time_ms,
                "rows_returned": v1_resp.rows_returned,
                "row_count": v1_resp.rows_returned,  // Add row_count alias for compatibility
                "rows_scanned": v1_resp.rows_scanned,
                "request_id": request_id
            });

            info!(
                "SQL query {} completed in {}ms",
                request_id, execution_time_ms
            );

            Ok(JsonResponse(json_data))
        }
        Err(e) => {
            error!("SQL query {} failed: {}", request_id, e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

/// Return table-level xCatalog routing metadata for REST clients.
pub async fn get_catalog_table_routing(
    State(state): State<AppState>,
    Query(query): Query<CatalogRoutingQuery>,
) -> ApiResult<Json<crate::services::CatalogIntrospectionResult>> {
    let sql = match query.table_name.as_deref().map(str::trim) {
        Some(table_name) if !table_name.is_empty() => format!(
            "SELECT * FROM information_schema.table_routing WHERE table_name = '{}'",
            table_name.replace('\'', "''")
        ),
        _ => "SELECT * FROM information_schema.table_routing".to_string(),
    };

    let result = crate::services::CatalogIntrospectionService::new(state.catalog_manager.clone())
        .execute_select(&sql)
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?
        .unwrap_or_else(crate::services::CatalogIntrospectionResult::empty);

    Ok(Json(result))
}

/// Explain table-write route selection without executing the write.
pub async fn explain_table_write_route(
    State(state): State<AppState>,
    Json(request): Json<TableWriteExplainRequest>,
) -> ApiResult<Json<crate::query::table_write_plan::TableWriteRouteExplanation>> {
    use crate::query::table_write_plan::{
        ConflictPolicy, CopyIntoPlan, DmlWritePlanRequest, DmlWritePlanner, WriteIntentOverrides,
        WriteMode,
    };

    let target = logical_table_ref_from_name(&request.target_table)?;
    let target_table_name = target.qualified_name();
    let (catalog, table_id) = state
        .catalog_manager
        .resolve_table(&target_table_name)
        .await
        .map_err(|error| ApiError::NotFound(error.to_string()))?;
    if !catalog
        .table_exists(&table_id)
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?
    {
        return Err(ApiError::NotFound(format!(
            "Table '{}' does not exist",
            target_table_name
        )));
    }
    let target_schema = catalog
        .get_table(&table_id)
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let target_stats = catalog.get_statistics(&table_id).await.unwrap_or_default();
    let (source, source_schema, source_stats) =
        resolve_table_write_explain_source(&state, &request).await?;
    let write_mode = parse_table_write_mode(request.write_mode.as_deref())?;
    let distribution = parse_distribution_mode(request.distribution.as_deref())?;
    let conflict_policy = if matches!(write_mode, WriteMode::Upsert | WriteMode::Merge) {
        ConflictPolicy::Upsert
    } else {
        ConflictPolicy::Error
    };
    let plan = CopyIntoPlan {
        source,
        target,
        write_mode,
        conflict_policy,
        distribution,
    };
    let write_intent_overrides = WriteIntentOverrides {
        tenant_id: request.tenant_id.clone(),
        actor: request.actor.clone(),
        idempotency_key: request.idempotency_key.clone(),
        row_count_hint: request.row_count_hint,
        estimated_bytes: request.estimated_bytes,
        requires_row_level_semantics: request.requires_row_level_semantics,
        batch_local_constraints_sufficient: request.batch_local_constraints_sufficient,
    };
    let target_columns = request.target_columns.unwrap_or_default();
    let routed = DmlWritePlanner::default()
        .plan(DmlWritePlanRequest {
            target_schema: &target_schema,
            target_stats: Some(&target_stats),
            source_schema: source_schema.as_ref(),
            source_stats: source_stats.as_ref(),
            write_intent_overrides: Some(&write_intent_overrides),
            plan: &plan,
            target_columns: &target_columns,
        })
        .map_err(|error| ApiError::InvalidArgument(error.to_string()))?;

    Ok(Json(routed.route_explanation()))
}

async fn resolve_table_write_explain_source(
    state: &AppState,
    request: &TableWriteExplainRequest,
) -> ApiResult<(
    crate::query::table_write_plan::ReadSource,
    Option<proximadb_catalog::CatalogTableSchema>,
    Option<proximadb_catalog::CatalogTableStatistics>,
)> {
    use crate::query::table_write_plan::{ReadSource, SnapshotRef};

    let source_table = request
        .source_table
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let source_sql = request
        .source_sql
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (source_table, source_sql) {
        (Some(_), Some(_)) => Err(ApiError::InvalidArgument(
            "Provide either source_table or source_sql, not both".to_string(),
        )),
        (None, None) => Err(ApiError::InvalidArgument(
            "source_table or source_sql is required".to_string(),
        )),
        (Some(table_name), None) => {
            let table = logical_table_ref_from_name(table_name)?;
            let (catalog, table_id) = state
                .catalog_manager
                .resolve_table(&table.qualified_name())
                .await
                .map_err(|error| ApiError::NotFound(error.to_string()))?;
            if !catalog
                .table_exists(&table_id)
                .await
                .map_err(|error| ApiError::Internal(error.to_string()))?
            {
                return Err(ApiError::NotFound(format!(
                    "Source table '{}' does not exist",
                    table.qualified_name()
                )));
            }
            let schema = catalog
                .get_table(&table_id)
                .await
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            let stats = catalog.get_statistics(&table_id).await.unwrap_or_default();
            Ok((
                ReadSource::CatalogTable {
                    table,
                    snapshot: SnapshotRef::Latest,
                },
                Some(schema),
                Some(stats),
            ))
        }
        (None, Some(sql)) => Ok((ReadSource::QuerySql(sql.to_string()), None, None)),
    }
}

fn logical_table_ref_from_name(
    name: &str,
) -> ApiResult<crate::query::table_write_plan::LogicalTableRef> {
    let parts: Vec<_> = name
        .split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let Some(table_name) = parts.last() else {
        return Err(ApiError::InvalidArgument(
            "table name cannot be empty".to_string(),
        ));
    };
    Ok(crate::query::table_write_plan::LogicalTableRef {
        namespace: parts[..parts.len().saturating_sub(1)]
            .iter()
            .map(|part| (*part).to_string())
            .collect(),
        name: (*table_name).to_string(),
    })
}

fn parse_table_write_mode(
    mode: Option<&str>,
) -> ApiResult<crate::query::table_write_plan::WriteMode> {
    use crate::query::table_write_plan::WriteMode;

    match mode
        .unwrap_or("append")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "append" => Ok(WriteMode::Append),
        "insert" | "insert_only" | "insert-only" => Ok(WriteMode::InsertOnly),
        "upsert" => Ok(WriteMode::Upsert),
        "overwrite" | "insert_overwrite" | "insert-overwrite" | "overwrite_table"
        | "overwrite-table" => Ok(WriteMode::OverwriteTable),
        "merge" => Ok(WriteMode::Merge),
        other => Err(ApiError::InvalidArgument(format!(
            "Unsupported table write mode '{}'",
            other
        ))),
    }
}

fn parse_distribution_mode(
    mode: Option<&str>,
) -> ApiResult<crate::query::table_write_plan::DistributionMode> {
    use crate::query::table_write_plan::DistributionMode;

    match mode.unwrap_or("auto").trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(DistributionMode::Auto),
        "local" | "local_only" | "local-only" => Ok(DistributionMode::LocalOnly),
        "pseudo" | "pseudo_distributed" | "pseudo-distributed" => {
            Ok(DistributionMode::PseudoDistributed)
        }
        "distributed" => Ok(DistributionMode::Distributed),
        other => Err(ApiError::InvalidArgument(format!(
            "Unsupported distribution mode '{}'",
            other
        ))),
    }
}

/// Helper: convert proto SqlValue to serde_json::Value (temporary until full internal refactor)
fn sql_value_to_json(v: &proximadb_v1::SqlValue) -> serde_json::Value {
    use proximadb_v1::sql_value::Value as V;
    match v.value.as_ref() {
        Some(V::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(V::NumberValue(n)) => serde_json::Value::Number(
            serde_json::Number::from_f64(*n).unwrap_or(serde_json::Number::from(0)),
        ),
        Some(V::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(V::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(V::BytesValue(b)) => {
            // Represent bytes as JSON array of integers
            serde_json::Value::Array(
                b.iter()
                    .map(|x| serde_json::Value::Number((*x as u64).into()))
                    .collect(),
            )
        }
        Some(V::NullValue(_)) => serde_json::Value::Null,
        Some(V::ArrayValue(arr)) => {
            serde_json::Value::Array(arr.values.iter().map(sql_value_to_json).collect())
        }
        Some(V::ObjectValue(obj)) => {
            let mut map = serde_json::Map::new();
            for (k, sv) in &obj.fields {
                map.insert(k.clone(), sql_value_to_json(sv));
            }
            serde_json::Value::Object(map)
        }
        None => serde_json::Value::Null,
    }
}

// =============================================================================
// Hybrid Search (BM25 + Vector with RRF Fusion)
// =============================================================================

/// Compatibility alias for vector search input
/// Maps internal VectorResult to simple wrapper used by handlers
#[allow(dead_code)]
struct VectorSearchInput {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    score: f32,
}

/// Legacy request body for hybrid search — DEAD CODE kept only for
/// deserialization-shape tests.
///
/// The hybrid_search handler that used this struct was removed (see
/// comment ~line 805 in this file): "Routes are now served by
/// create_hybrid_search_router() from proximadb-api backed by
/// Bm25IndexPortImpl and RestHybridPortImpl."
///
/// Naming note: this type used to be called `HybridSearchRequest` and
/// collided with `crate::proto::v1::HybridSearchRequest` (proto wire form
/// for vector+graph hybrid) AND with
/// `src/network/rest/v1/hybrid.rs::ExperimentalHybridSearchRequest`.
/// Renamed to `LegacyHybridSearchRequest` to mark it as deprecated; the
/// struct can be deleted once the deserialization tests are moved to the
/// active proximadb-api hybrid handlers.
#[derive(Debug, Deserialize)]
pub struct LegacyHybridSearchRequest {
    /// Collection to search
    pub collection: String,
    /// Query vector for similarity search (optional if keyword-only)
    pub vector: Option<Vec<f32>>,
    /// Text query for BM25 keyword search (optional if vector-only)
    pub text_query: Option<String>,
    /// Number of results to return
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Weight for vector results (0.0-1.0). BM25 weight = 1.0 - vector_weight.
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f32,
    /// RRF constant k (default 60)
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u32,
    /// Minimum BM25 score threshold
    #[serde(default)]
    pub min_bm25_score: f64,
}

fn default_top_k() -> usize {
    10
}
fn default_vector_weight() -> f32 {
    0.5
}
fn default_rrf_k() -> u32 {
    60
}

/// Request body for indexing text documents for hybrid search
#[derive(Debug, Deserialize)]
pub struct HybridIndexRequest {
    /// Collection name
    pub collection: String,
    /// Documents to index: list of {id, text}
    pub documents: Vec<HybridDocument>,
}

/// A text document for hybrid search indexing
#[derive(Debug, Deserialize)]
pub struct HybridDocument {
    /// Document/vector ID
    pub id: String,
    /// Text content to index
    pub text: String,
}

/// Legacy response for hybrid search — DEAD CODE paired with
/// `LegacyHybridSearchRequest` (see ~line 695). The handler that produced
/// this response was removed (routes now served by proximadb-api's
/// create_hybrid_search_router backed by RestHybridPortImpl). The struct
/// survives only for serialization-shape tests in this file + tests/rest_api_v1_test.rs.
///
/// Naming note: renamed from `HybridSearchResponse` to
/// `LegacyHybridSearchResponse` to mark it as dead-code-eligible and to
/// distinguish from the proto wire form
/// `crate::proto::v1::HybridSearchResponse`.
#[derive(Debug, Serialize)]
pub struct LegacyHybridSearchResponse {
    /// Whether the search completed successfully
    pub success: bool,
    /// Fused search result hits
    pub results: Vec<HybridSearchHit>,
    /// Total number of results
    pub total: usize,
    /// Server-side processing time in microseconds
    pub processing_time_us: u64,
    /// Search mode used (e.g., "hybrid", "vector_only", "bm25_only")
    pub mode: String,
}

/// A single hybrid search result hit
#[derive(Debug, Serialize)]
pub struct HybridSearchHit {
    /// Vector/document identifier
    pub id: String,
    /// Fused score combining vector and BM25 signals
    pub combined_score: f64,
    /// Vector similarity score (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_score: Option<f32>,
    /// BM25 text relevance score (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_score: Option<f64>,
    /// Rank in vector-only results (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_rank: Option<usize>,
    /// Rank in BM25-only results (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_rank: Option<usize>,
    /// BM25 terms that matched the query
    pub matched_terms: Vec<String>,
}

/// Response for hybrid index operations
#[derive(Debug, Serialize)]
pub struct HybridIndexResponse {
    /// Whether the indexing operation succeeded
    pub success: bool,
    /// Collection that was indexed
    pub collection: String,
    /// Number of documents indexed in this operation
    pub documents_indexed: usize,
    /// Total number of documents in the full-text index
    pub total_documents: usize,
}

/// Shared state for per-collection full-text indices
pub type FullTextIndexMap = Arc<
    std::sync::RwLock<
        std::collections::HashMap<
            String,
            crate::storage::engines::core::formats::columnar::fulltext_index::FullTextIndex,
        >,
    >,
>;

// hybrid_index and hybrid_search handlers removed.
// Routes are now served by create_hybrid_search_router() from proximadb-api
// backed by Bm25IndexPortImpl and RestHybridPortImpl.

/// Create router with all REST endpoints
pub fn create_router(state: AppState) -> axum::Router {
    use axum::routing::{get, post};

    info!("🔵 REST API: Creating router with collection endpoints...");

    // Initialize SKS in-memory store (v1) using the same storage engine as vector operations
    let entities_router = {
        use crate::network::rest::v1::entities::{self, EntityApiState};
        use crate::storage::entity_store::{
            CsrRelationsStore, InMemoryProvenanceRegistry, ProximaEntityStore,
        };

        let engine = state
            .request_handlers
            .vector_operations_service
            .unified_engine();
        let legacy_store = ProximaEntityStore::with_vector_service(
            engine,
            Arc::new(CsrRelationsStore::new()),
            Arc::new(InMemoryProvenanceRegistry::new()),
            state.request_handlers.vector_operations_service.clone(),
        );

        // Register legacy store globally for compatibility (entity API currently uses legacy store).
        let legacy_arc = Arc::new(legacy_store);
        ProximaEntityStore::register_global(legacy_arc.clone());

        // Use the same Arc - no need to clone the inner value
        let store = legacy_arc.clone();
        // Register store globally for hybrid executor access (embedding catalog)
        crate::storage::entity_store::ProximaEntityStore::register_global(store.clone());
        let entity_state = EntityApiState { store };
        entities::configure_routes().with_state(entity_state)
    };

    let mut router = axum::Router::new()
        .route(
            "/api/v1/progressive/search/:collection_id",
            post(crate::network::rest::progressive_search_handler::progressive_search_handler),
        )
        // SQL query execution (explain added conditionally below)
        .route("/api/v1/sql/execute", post(execute_sql))
        .route(
            "/api/v1/catalog/table_routing",
            get(get_catalog_table_routing),
        )
        .route(
            "/api/v1/catalog/table_write/explain",
            post(explain_table_write_route),
        )
        .route(
            "/api/v1/collections/:collection/branches/:branch/merge",
            post(merge_graph_branch),
        )
        // Multi-phase ranking pipeline (R-7c.1).
        // `rank_search_dispatch` runs the arena-bearing first phase
        // inside `tokio::task::spawn_blocking` so the outer future
        // stays `Send` — required by axum's tokio multi-threaded
        // runtime, blocked previously by `bumpalo::Bump` being `!Sync`.
        // The route returns:
        //   200 — successful rank pipeline result
        //   404 — named profile not found
        //   501 — RankServices not injected at startup
        //   500 — pipeline failure (model load, expression compile, …)
        .route(
            "/api/v1/rank/search",
            post(|State(state): State<AppState>, Json(req): Json<crate::network::rest::v1::rank::RankSearchRequest>| async move {
                crate::network::rest::v1::rank::rank_search_dispatch(state, req)
                    .await
                    .map(Json)
            }),
        )
        // Phase 6: per-collection pinning control surface.
        // Operators PATCH a collection's pin state; the AxisTieringManager
        // honors the override on its next evaluation. See
        // src/storage/collection_pinning.rs for the control/data plane split.
        .route(
            "/api/v1/collections/:collection_id/pin",
            axum::routing::patch(crate::network::rest::v1::pinning::patch_pin)
                .get(crate::network::rest::v1::pinning::get_pin),
        )
        .route(
            "/api/v1/collections/pinning",
            get(crate::network::rest::v1::pinning::list_pins),
        )
        // Phase 7.2.4: per-collection cache-affinity inspect/invalidate.
        // Operators read or drop the affinity hint for routing
        // re-evaluation. See src/cluster/cache_affinity.rs.
        .route(
            "/api/v1/collections/:collection_id/affinity",
            get(crate::network::rest::v1::affinity::get_affinity)
                .delete(crate::network::rest::v1::affinity::delete_affinity),
        )
        .route(
            "/api/v1/collections/affinity",
            get(crate::network::rest::v1::affinity::list_affinity),
        )
        // Slice 3 of tenant-pod-affinity: per-(tenant, collection)
        // primary-pod operator API. Auth-gated inside each handler
        // to `SystemAdmin` or `ConfigureSystem` — these endpoints
        // expose cross-tenant placement and drive WAL write routing,
        // so they MUST NOT be reachable by regular tenants. The
        // auth_middleware_unified layer authenticates the request;
        // `authorize_operator` then checks for the operator-level
        // permission.
        .route(
            "/api/v1/primary-pod/:tenant_id/:collection_id",
            get(crate::network::rest::v1::primary_pod::get_primary_pod)
                .put(crate::network::rest::v1::primary_pod::put_primary_pod)
                .delete(crate::network::rest::v1::primary_pod::delete_primary_pod),
        )
        .route(
            "/api/v1/primary-pod",
            get(crate::network::rest::v1::primary_pod::list_primary_pods),
        )
        // Health check endpoints
        .route("/health", get(comprehensive_health_check))
        .route("/health/live", get(liveness_check))
        .route("/health/ready", get(readiness_check))
        // SKS entity endpoints (storage-coupled path)
        .nest("/api", entities_router);

    // Graph routes — port-backed (always wired in production since Phase 9.12).
    //
    // The same router is mounted at two prefixes: `/api/v1/graph` (legacy
    // mount kept for backwards compatibility with existing clients) and
    // `/api/v2` (canonical v2 mount added 2026-05-29 to align with the
    // v2 OpenAPI document — the spec paths are `/api/v2/graphs/...`).
    // New SDK code should target the v2 mount.
    if let Some(ref gp) = state.graph_port {
        use proximadb_api::rest::{GraphRestState, create_graph_router};
        let graph_state = GraphRestState {
            graph_port: gp.clone(),
        };
        router = router
            .nest(
                "/api/v1/graph",
                create_graph_router().with_state(graph_state.clone()),
            )
            .nest("/api/v2", create_graph_router().with_state(graph_state));
        info!("✅ Graph API routing via port-based handler (proximadb-api) — mounted at /api/v1/graph (legacy) + /api/v2 (canonical)");
    }

    // Collection and vector routes via port-backed handlers (proximadb-api).
    // Prefers state.api_handlers (runtime crate's UnifiedHandlers backed by CollectionPort/
    // VectorOpsPort trait objects) when available; falls back to the concrete root-crate
    // UnifiedHandlers which also implements ApiHandlersPort.
    {
        let api_handlers: Arc<dyn proximadb_runtime::ApiHandlersPort> =
            state.api_handlers.clone().unwrap_or_else(|| {
                state.request_handlers.clone() as Arc<dyn proximadb_runtime::ApiHandlersPort>
            });
        let api_rest_state = proximadb_api::rest::RestAppState::new(api_handlers);
        router = router
            .merge(
                proximadb_api::rest::create_collection_router().with_state(api_rest_state.clone()),
            )
            .merge(proximadb_api::rest::create_vector_router().with_state(api_rest_state));
    }
    info!("✅ Collection and Vector API routing via port-backed handlers (proximadb-api)");

    // Document routes — port-backed (always wired in production since Phase 9)
    if let Some(ref dp) = state.doc_port {
        use proximadb_api::rest::{DocumentRestState, create_document_router};
        router = router.nest(
            "/api/v1/documents",
            create_document_router().with_state(DocumentRestState {
                document_port: dp.clone(),
            }),
        );
        info!("✅ Document API routing via port-based handler (proximadb-api)");
    }

    // Observability routes — port-backed (always wired in production since Phase 9)
    if let Some(ref op) = state.obs_port {
        use proximadb_api::rest::{ObservabilityRestState, create_observability_router};
        router = router.nest(
            "/api/v1/observability",
            create_observability_router().with_state(ObservabilityRestState {
                observability_port: op.clone(),
            }),
        );
        info!("✅ Observability API routing via port-based handler (proximadb-api)");
    }

    // Unified multi-model query routes — port-backed (always wired in production since Phase 9.9)
    if let Some(ref uq_port) = state.unified_query_port {
        use proximadb_api::rest::UnifiedQueryRestState;
        let uq_state = UnifiedQueryRestState {
            unified_query_port: uq_port.clone(),
        };
        router = router.nest(
            "/api/v1/unified",
            proximadb_api::rest::create_multimodal_router().with_state(uq_state),
        );
        info!("✅ Unified Query API enabled at /api/v1/unified (port-backed)");
    }

    // Hybrid search — port-backed via RestHybridPortImpl (real BM25+vector fusion).
    // Shares the in-process HybridFullTextIndexMap with Bm25IndexPortImpl so indexed
    // documents are immediately searchable without a separate startup step.
    {
        use crate::network::hybrid_search::{Bm25IndexPortImpl, RestHybridPortImpl};
        use proximadb_api::rest::{HybridRestState, create_hybrid_search_router};

        let indexes = state
            .fulltext_indexes
            .clone()
            .unwrap_or_else(|| Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())));
        let vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort> =
            state.request_handlers.vector_operations_service.clone();
        let hybrid_port: Arc<dyn proximadb_runtime::HybridPort> =
            Arc::new(RestHybridPortImpl::new(vector_ops, indexes.clone()));
        let bm25_port: Arc<dyn proximadb_runtime::BM25IndexPort> =
            Arc::new(Bm25IndexPortImpl::new(indexes));
        let hybrid_state = HybridRestState {
            hybrid_port,
            bm25_port: Some(bm25_port),
        };
        router = router.merge(create_hybrid_search_router().with_state(hybrid_state));
        info!("✅ Hybrid search at /api/v1/hybrid/* via RestHybridPortImpl (real BM25+vector)");
    }

    // SQL explain — port-backed when unified_query_port is wired (production always)
    if let Some(ref uq_port) = state.unified_query_port {
        use proximadb_api::rest::{UnifiedQueryRestState, create_explain_router};
        let explain_state = UnifiedQueryRestState {
            unified_query_port: uq_port.clone(),
        };
        router = router.merge(create_explain_router().with_state(explain_state));
        info!("✅ SQL explain at /api/v1/sql/explain via port-backed handler (proximadb-api)");
    }

    // Optional enterprise catalog endpoints
    #[cfg(feature = "enterprise-catalogs")]
    {
        let catalog_router = {
            use crate::network::rest::v1::catalog::{self, CatalogApiState};

            let catalog_state = CatalogApiState::new(state.catalog_manager.clone());
            catalog::configure_routes().with_state(catalog_state)
        };
        router = router.nest("/api/v1/catalogs", catalog_router);
        info!("✅ External Catalog API endpoints enabled at /api/v1/catalogs");
    }

    // Iceberg REST Catalog server — always on, no feature gate needed.
    // External engines (Spark, Trino, DuckDB, PyIceberg) connect via /iceberg/v1.
    {
        use crate::network::rest::v1::iceberg_rest_catalog::{
            IcebergRestState, create_iceberg_rest_router,
        };
        let iceberg_state = IcebergRestState::with_defaults(state.catalog_manager.clone())
            .with_segment_registry(state.segment_registry.clone());
        router = router.nest(
            "/iceberg/v1",
            create_iceberg_rest_router().with_state(iceberg_state),
        );
        info!(
            "✅ Iceberg REST Catalog server at /iceberg/v1 (Spark/Trino/DuckDB/PyIceberg compatible)"
        );
    }

    // Experimental hybrid API removed 2026-05-26: it returned mock-backed
    // results that misled customers into thinking the endpoint computed real
    // BM25+vector fusion. The production hybrid endpoint at `/api/v1/hybrid/search`
    // (port-backed via `RestHybridPortImpl`) is the supported path; gRPC parity
    // landed in commit 6a73ead7f. The module at `src/network/rest/v1/hybrid.rs`
    // remains in-tree for reference but is no longer mounted.

    // Read-only collection analytics (Entanglement Index, etc.) — TD-043 sub-2
    let analytics_router = {
        let analytics_state = AnalyticsApiState::new(Some(
            state.request_handlers.vector_operations_service.clone(),
        ));
        analytics::create_router().with_state(analytics_state)
    };
    router = router.nest("/api/v1/analytics", analytics_router);
    info!("✅ Analytics API endpoints enabled at /api/v1/analytics");

    // Agentic Query Language (RUBICON / AQL) — TD-050
    let aql_router = {
        let mut executor = AqlExecutor::new();

        // Attach event log for persistent audit trails (TD-050 Phase 5)
        if let Some(log) = &state.request_handlers.event_log {
            executor = executor.with_event_log(log.clone());
        }

        // Register sources
        executor.register_source(
            "vector".to_string(),
            Arc::new(VectorAqlSource::new(
                state.request_handlers.vector_operations_service.clone(),
            )),
        );
        executor.register_source(
            "graph".to_string(),
            Arc::new(GraphAqlSource::new(state.graph_execution_service.clone())),
        );
        executor.register_source(
            "document".to_string(),
            Arc::new(DocumentAqlSource::new(
                state.request_handlers.document_service.clone(),
            )),
        );
        executor.register_source(
            "observability".to_string(),
            Arc::new(ObservabilityAqlSource::new(
                state.request_handlers.observability_service.clone(),
            )),
        );

        let aql_state = AqlApiState::new(executor);
        aql::create_router().with_state(aql_state)
    };
    router = router.nest("/api/v1/aql", aql_router);
    info!("✅ AQL (RUBICON) API endpoints enabled at /api/v1/aql");

    // Natural Language Query Translation (AV-SQL) — TD-048
    if let Some(llm) = &state.llm_engine {
        let nl_state = NlApiState::new(llm.clone());
        let nl_router = nl::create_router().with_state(nl_state);
        router = router.nest("/api/v1/nl", nl_router);
        info!("✅ Natural Language (AV-SQL) API endpoints enabled at /api/v1/nl");
    } else {
        warn!("⚠️ LLM engine not available; Natural Language (AV-SQL) endpoints disabled");
    }

    // Convert to Router<()> by providing state, with default tenant context for all routes
    let default_tenant = TenantContext::new(
        "default",
        crate::network::middleware::tenant::TenantIdSource::Default,
    );
    let default_api_tenant = proximadb_api::rest::TenantContext {
        tenant_id: "default".to_string(),
    };
    let router = router
        .with_state(state)
        .layer(Extension(default_tenant))
        .layer(axum::Extension(default_api_tenant))
        .layer(axum::middleware::from_fn(add_rest_v1_deprecation_headers));

    // Optional AI endpoints (disabled by default; enable with `--features ai_endpoints`)
    #[cfg(feature = "ai_endpoints")]
    {
        use crate::api_handlers::ai_endpoints;

        match tokio::runtime::Runtime::new()
            .and_then(|rt| rt.block_on(ai_endpoints::initialize_ai_service_state()))
        {
            Ok(ai_state) => {
                router = router.nest("/ai", ai_endpoints::create_ai_router(ai_state));
                info!("✅ AI endpoints enabled at /ai");
            }
            Err(e) => {
                warn!("AI endpoints disabled (initialization failed): {}", e);
            }
        }
    }

    // Optional Sales endpoints (disabled by default; enable with `--features sales_endpoints`)
    #[cfg(feature = "sales_endpoints")]
    {
        use crate::api_handlers::sales_endpoints;

        match tokio::runtime::Runtime::new()
            .and_then(|rt| rt.block_on(sales_endpoints::initialize_sales_service_state()))
        {
            Ok(sales_state) => {
                router = router.nest("/sales", sales_endpoints::create_sales_router(sales_state));
                info!("✅ Sales endpoints enabled at /sales");
            }
            Err(e) => {
                warn!("Sales endpoints disabled (initialization failed): {}", e);
            }
        }
    }

    info!("✅ REST API: Router created with canonical v2 routes and v1 compatibility adapters:");
    info!(
        "   POST   /api/v2/collections/:collection/records/batch (canonical ProximaRecord writes)"
    );
    info!("   POST   /api/v2/collections/:collection/search (canonical record/vector search)");
    info!(
        "   GET    /api/v2/collections/:collection/records/:record (canonical ProximaRecord fetch)"
    );
    info!("   POST   /api/v1/collections (deprecated compatibility via proximadb-api)");
    info!("   GET    /api/v1/collections (deprecated compatibility via proximadb-api)");
    info!("   GET    /api/v1/collections/:id (deprecated compatibility via proximadb-api)");
    info!("   DELETE /api/v1/collections/:id (deprecated compatibility via proximadb-api)");
    info!("   POST   /api/v1/search (deprecated compatibility via proximadb-api)");
    info!("   POST   /api/v1/search/with_metadata (deprecated compatibility via proximadb-api)");
    info!("   POST   /api/v1/vectors/batch (deprecated alias over record-native writes)");
    info!("   GET    /api/v1/vectors/:collection_id/:vector_id (deprecated compatibility)");
    info!("   DELETE /api/v1/vectors/:collection_id/:vector_id (deprecated compatibility)");
    info!("   POST   /api/v1/hybrid/search (deprecated compatibility; real BM25+vector)");
    info!("   POST   /api/v1/hybrid/index (deprecated compatibility)");
    // /api/v1/experimental/hybrid/search route removed 2026-05-26 — was
    // mock-backed; production path is /api/v1/hybrid/search above.

    router
}

/// Comprehensive health check handler
///
/// Wraps the health module's health_check function with our AppState
pub async fn comprehensive_health_check(
    State(state): State<AppState>,
    query: Query<health::HealthParams>,
) -> ApiResult<Json<health::HealthResponse>> {
    let health_state = state.health_state();
    health::health_check(axum::extract::State(health_state), query).await
}

/// Liveness check handler
///
/// Simple liveness check for load balancers
pub async fn liveness_check(
    State(state): State<AppState>,
) -> ApiResult<Json<health::LivenessResponse>> {
    let health_state = state.health_state();
    health::liveness_check(axum::extract::State(health_state)).await
}

/// Readiness check handler
///
/// Returns 200 when ready, 503 when not ready
pub async fn readiness_check(
    State(state): State<AppState>,
) -> Result<Json<health::ReadinessResponse>, (StatusCode, Json<health::ReadinessResponse>)> {
    let health_state = state.health_state();
    health::readiness_check(axum::extract::State(health_state)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::rest::proto_json::ProtoApiResponse;
    use crate::proto::proximadb_v1::{CollectionOperation, CollectionRequest};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use std::path::Path;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn branch_merge_test_entry(
        seq: u64,
        collection_id: &str,
        oid: &str,
        branch_id: Option<&str>,
    ) -> proximadb_storage_common::CanonicalWalEntry {
        let mut record = proximadb_records::ProximaRecord {
            oid: oid.to_string(),
            ..Default::default()
        };
        record.branch_id = branch_id.map(String::from);
        proximadb_storage_common::CanonicalWalEntry::new(
            seq,
            proximadb_storage_common::CanonicalOperation::RecordUpsert {
                collection_id: collection_id.to_string(),
                record: Box::new(record),
                projections: vec![],
            },
            None,
        )
    }

    #[test]
    fn branch_merge_request_defaults_to_main_dry_run() {
        let request: GraphBranchMergeRequest = serde_json::from_value(serde_json::json!({}))
            .expect("empty request should use defaults");
        assert_eq!(request.target_branch, "main");
        assert!(request.dry_run);
    }

    #[test]
    fn branch_merge_collection_filter_keeps_only_matching_wal_entries() {
        let entries = vec![
            branch_merge_test_entry(1, "graph_a", "base", None),
            branch_merge_test_entry(2, "graph_a", "left", Some("feature")),
            branch_merge_test_entry(3, "graph_b", "right", Some("feature")),
        ];

        let filtered = filter_canonical_wal_for_collection(entries, "graph_a");
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|entry| match &entry.operation {
            proximadb_storage_common::CanonicalOperation::RecordUpsert {
                collection_id, ..
            } => collection_id == "graph_a",
            _ => false,
        }));
    }

    #[tokio::test]
    async fn v1_deprecation_middleware_marks_api_v1_only() {
        let router = axum::Router::new()
            .route("/api/v1/ping", axum::routing::get(|| async { "ok" }))
            .route("/api/v2/ping", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(add_rest_v1_deprecation_headers));

        let v1_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(v1_response.status(), StatusCode::OK);
        assert_eq!(
            v1_response
                .headers()
                .get("deprecation")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
        assert_eq!(
            v1_response
                .headers()
                .get("x-proximadb-api-status")
                .and_then(|value| value.to_str().ok()),
            Some("deprecated-compatibility")
        );

        let v2_response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v2/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(v2_response.status(), StatusCode::OK);
        assert!(v2_response.headers().get("deprecation").is_none());
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.to_string_lossy())
    }

    // ── Mock ports ────────────────────────────────────────────────────────────
    // Minimal stubs that let tests build a fully-ported AppState without real
    // services.  Each method returns an error; the tested routes either fail
    // before reaching the port (JSON parse, empty-field guard) or are self-
    // contained redirects that never call the port at all.

    use async_trait::async_trait;

    struct MockDocumentPort;
    #[async_trait]
    impl proximadb_runtime::DocumentPort for MockDocumentPort {
        async fn create_collection(
            &self,
            _: crate::proto::v1::CreateDocumentCollectionRequest,
        ) -> anyhow::Result<crate::proto::v1::CreateDocumentCollectionResponse> {
            anyhow::bail!("mock")
        }
        async fn list_collections(
            &self,
            _: crate::proto::v1::ListDocumentCollectionsRequest,
        ) -> anyhow::Result<crate::proto::v1::ListDocumentCollectionsResponse> {
            anyhow::bail!("mock")
        }
        async fn delete_collection(
            &self,
            _: crate::proto::v1::DeleteDocumentCollectionRequest,
        ) -> anyhow::Result<crate::proto::v1::DeleteDocumentCollectionResponse> {
            anyhow::bail!("mock")
        }
        async fn insert_document(
            &self,
            _: crate::proto::v1::InsertDocumentRequest,
        ) -> anyhow::Result<crate::proto::v1::InsertDocumentResponse> {
            anyhow::bail!("mock")
        }
        async fn get_document(
            &self,
            _: crate::proto::v1::GetDocumentRequest,
        ) -> anyhow::Result<crate::proto::v1::GetDocumentResponse> {
            anyhow::bail!("mock")
        }
        async fn update_document(
            &self,
            _: crate::proto::v1::UpdateDocumentRequest,
        ) -> anyhow::Result<crate::proto::v1::UpdateDocumentResponse> {
            anyhow::bail!("mock")
        }
        async fn delete_document(
            &self,
            _: crate::proto::v1::DeleteDocumentRequest,
        ) -> anyhow::Result<crate::proto::v1::DeleteDocumentResponse> {
            anyhow::bail!("mock")
        }
        async fn query_documents(
            &self,
            _: crate::proto::v1::QueryDocumentsRequest,
        ) -> anyhow::Result<crate::proto::v1::QueryDocumentsResponse> {
            anyhow::bail!("mock")
        }
        async fn aggregate_documents(
            &self,
            _: crate::proto::v1::AggregateDocumentsRequest,
        ) -> anyhow::Result<crate::proto::v1::AggregateDocumentsResponse> {
            anyhow::bail!("mock")
        }
    }

    struct MockGraphPort;
    #[async_trait]
    impl proximadb_runtime::GraphPort for MockGraphPort {
        async fn create_node(
            &self,
            _: crate::proto::v1::CreateNodeRequest,
        ) -> anyhow::Result<crate::proto::v1::Node> {
            anyhow::bail!("mock")
        }
        async fn get_node(
            &self,
            _: crate::proto::v1::GetNodeRequest,
        ) -> anyhow::Result<crate::proto::v1::Node> {
            anyhow::bail!("mock")
        }
        async fn update_node(
            &self,
            _: crate::proto::v1::UpdateNodeRequest,
        ) -> anyhow::Result<crate::proto::v1::Node> {
            anyhow::bail!("mock")
        }
        async fn delete_node(
            &self,
            _: crate::proto::v1::DeleteNodeRequest,
        ) -> anyhow::Result<crate::proto::v1::Node> {
            anyhow::bail!("mock")
        }
        async fn create_edge(
            &self,
            _: crate::proto::v1::CreateEdgeRequest,
        ) -> anyhow::Result<crate::proto::v1::Edge> {
            anyhow::bail!("mock")
        }
        async fn get_edge(
            &self,
            _: crate::proto::v1::GetEdgeRequest,
        ) -> anyhow::Result<crate::proto::v1::Edge> {
            anyhow::bail!("mock")
        }
        async fn update_edge(
            &self,
            _: crate::proto::v1::UpdateEdgeRequest,
        ) -> anyhow::Result<crate::proto::v1::Edge> {
            anyhow::bail!("mock")
        }
        async fn delete_edge(
            &self,
            _: crate::proto::v1::DeleteEdgeRequest,
        ) -> anyhow::Result<crate::proto::v1::Edge> {
            anyhow::bail!("mock")
        }
        async fn query_nodes(
            &self,
            _: crate::proto::v1::NodeQuery,
        ) -> anyhow::Result<crate::proto::v1::BatchResponse> {
            anyhow::bail!("mock")
        }
        async fn query_edges(
            &self,
            _: crate::proto::v1::EdgeQuery,
        ) -> anyhow::Result<crate::proto::v1::BatchResponse> {
            anyhow::bail!("mock")
        }
        async fn execute_query(
            &self,
            _: crate::proto::v1::GraphQueryRequest,
        ) -> anyhow::Result<crate::proto::v1::GraphQueryResponse> {
            anyhow::bail!("mock")
        }
        async fn get_neighbors(
            &self,
            _: crate::proto::v1::GetNeighborsRequest,
        ) -> anyhow::Result<crate::proto::v1::BatchResponse> {
            anyhow::bail!("mock")
        }
        async fn traverse_graph(
            &self,
            _: crate::proto::v1::TraversalRequest,
        ) -> anyhow::Result<crate::proto::v1::TraversalResponse> {
            anyhow::bail!("mock")
        }
        async fn stream_traverse(
            &self,
            _: crate::proto::v1::TraversalRequest,
        ) -> anyhow::Result<Vec<crate::proto::v1::TraversalChunk>> {
            anyhow::bail!("mock")
        }
        async fn get_graph_stats(
            &self,
            _: crate::proto::v1::GetStatsRequest,
        ) -> anyhow::Result<crate::proto::v1::GraphStats> {
            anyhow::bail!("mock")
        }
        async fn shortest_path(
            &self,
            _: crate::proto::v1::ShortestPathRequest,
        ) -> anyhow::Result<crate::proto::v1::ShortestPathResponse> {
            anyhow::bail!("mock")
        }
        async fn get_connected_components(
            &self,
            _: crate::proto::v1::GetStatsRequest,
        ) -> anyhow::Result<crate::proto::v1::ConnectedComponentsResponse> {
            anyhow::bail!("mock")
        }
        async fn has_cycle(
            &self,
            _: crate::proto::v1::GetStatsRequest,
        ) -> anyhow::Result<crate::proto::v1::CycleCheckResponse> {
            anyhow::bail!("mock")
        }
        async fn add_unique_constraint(
            &self,
            _: crate::proto::v1::UniqueConstraintRequest,
        ) -> anyhow::Result<crate::proto::v1::UniqueConstraintResponse> {
            anyhow::bail!("mock")
        }
        async fn remove_unique_constraint(
            &self,
            _: crate::proto::v1::UniqueConstraintRequest,
        ) -> anyhow::Result<crate::proto::v1::UniqueConstraintResponse> {
            anyhow::bail!("mock")
        }
        async fn batch_create_nodes(
            &self,
            _: crate::proto::v1::BatchNodeRequest,
        ) -> anyhow::Result<crate::proto::v1::BatchResponse> {
            anyhow::bail!("mock")
        }
        async fn batch_create_edges(
            &self,
            _: crate::proto::v1::BatchEdgeRequest,
        ) -> anyhow::Result<crate::proto::v1::BatchResponse> {
            anyhow::bail!("mock")
        }
        async fn execute_hybrid_query(
            &self,
            _: crate::proto::v1::HybridSearchRequest,
        ) -> anyhow::Result<crate::proto::v1::HybridSearchResponse> {
            anyhow::bail!("mock")
        }
        async fn create_graph_collection(
            &self,
            _: crate::proto::v1::CreateGraphRequest,
        ) -> anyhow::Result<crate::proto::v1::GraphCollection> {
            anyhow::bail!("mock")
        }
        async fn get_graph_collection(
            &self,
            _: String,
        ) -> anyhow::Result<Option<crate::proto::v1::GraphCollection>> {
            anyhow::bail!("mock")
        }
        async fn delete_graph_collection(&self, _: String) -> anyhow::Result<bool> {
            anyhow::bail!("mock")
        }
        async fn list_graph_collections(
            &self,
        ) -> anyhow::Result<Vec<crate::proto::v1::GraphCollection>> {
            anyhow::bail!("mock")
        }
        async fn update_graph_schema(
            &self,
            _: String,
            _: crate::proto::v1::GraphSchema,
        ) -> anyhow::Result<crate::proto::v1::GraphCollection> {
            anyhow::bail!("mock")
        }
    }

    struct MockObservabilityPort;
    #[async_trait]
    impl proximadb_runtime::ObservabilityPort for MockObservabilityPort {
        async fn create_namespace(
            &self,
            _: crate::proto::v1::CreateObservabilityNamespaceRequest,
        ) -> anyhow::Result<crate::proto::v1::CreateObservabilityNamespaceResponse> {
            anyhow::bail!("mock")
        }
        async fn list_namespaces(
            &self,
            _: crate::proto::v1::ListNamespacesRequest,
        ) -> anyhow::Result<crate::proto::v1::ListNamespacesResponse> {
            anyhow::bail!("mock")
        }
        async fn delete_namespace(
            &self,
            _: crate::proto::v1::DeleteNamespaceRequest,
        ) -> anyhow::Result<crate::proto::v1::DeleteNamespaceResponse> {
            anyhow::bail!("mock")
        }
        async fn ingest_logs(
            &self,
            _: crate::proto::v1::IngestLogsRequest,
        ) -> anyhow::Result<crate::proto::v1::IngestLogsResponse> {
            anyhow::bail!("mock")
        }
        async fn query_logs(
            &self,
            _: crate::proto::v1::QueryLogsRequest,
        ) -> anyhow::Result<crate::proto::v1::QueryLogsResponse> {
            anyhow::bail!("mock")
        }
        async fn stream_logs(
            &self,
            _: crate::proto::v1::QueryLogsRequest,
        ) -> anyhow::Result<Vec<crate::proto::v1::LogEntry>> {
            anyhow::bail!("mock")
        }
        async fn ingest_metrics(
            &self,
            _: crate::proto::v1::IngestMetricsRequest,
        ) -> anyhow::Result<crate::proto::v1::IngestMetricsResponse> {
            anyhow::bail!("mock")
        }
        async fn query_metrics(
            &self,
            _: crate::proto::v1::QueryMetricsRequest,
        ) -> anyhow::Result<crate::proto::v1::QueryMetricsResponse> {
            anyhow::bail!("mock")
        }
        async fn aggregate_metrics(
            &self,
            _: crate::proto::v1::AggregateMetricsRequest,
        ) -> anyhow::Result<crate::proto::v1::AggregateMetricsResponse> {
            anyhow::bail!("mock")
        }
        async fn ingest_traces(
            &self,
            _: crate::proto::v1::IngestTracesRequest,
        ) -> anyhow::Result<crate::proto::v1::IngestTracesResponse> {
            anyhow::bail!("mock")
        }
        async fn query_traces(
            &self,
            _: crate::proto::v1::QueryTracesRequest,
        ) -> anyhow::Result<crate::proto::v1::QueryTracesResponse> {
            anyhow::bail!("mock")
        }
        async fn get_trace(
            &self,
            _: crate::proto::v1::GetTraceRequest,
        ) -> anyhow::Result<crate::proto::v1::GetTraceResponse> {
            anyhow::bail!("mock")
        }
        async fn upsert_alert_rule(
            &self,
            _: crate::proto::v1::UpsertAlertRuleRequest,
        ) -> anyhow::Result<crate::proto::v1::UpsertAlertRuleResponse> {
            anyhow::bail!("mock")
        }
        async fn delete_alert_rule(
            &self,
            _: crate::proto::v1::DeleteAlertRuleRequest,
        ) -> anyhow::Result<crate::proto::v1::DeleteAlertRuleResponse> {
            anyhow::bail!("mock")
        }
        async fn list_alerts(
            &self,
            _: crate::proto::v1::ListAlertsRequest,
        ) -> anyhow::Result<crate::proto::v1::ListAlertsResponse> {
            anyhow::bail!("mock")
        }
    }

    struct MockUnifiedQueryPort;
    #[async_trait]
    impl proximadb_runtime::UnifiedQueryPort for MockUnifiedQueryPort {
        async fn execute_unified_query(
            &self,
            _: String,
            _: Option<Vec<proximadb_data_model::ProximaValue>>,
            _: Option<String>,
            _: Option<u32>,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("mock")
        }
        async fn execute_multi_model_query(
            &self,
            _: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("mock")
        }
        async fn execute_federated_query(
            &self,
            _: String,
            _: Option<Vec<proximadb_data_model::ProximaValue>>,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("mock")
        }
        async fn execute_distributed_query(
            &self,
            _: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("mock")
        }
        async fn explain_unified_query(
            &self,
            _: String,
            _: Option<String>,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("mock")
        }
        async fn prepare_statement(
            &self,
            _: Option<String>,
            _: String,
            _: bool,
            _: Option<u64>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("mock")
        }
        async fn execute_prepared(
            &self,
            _: String,
            _: Option<Vec<proximadb_data_model::ProximaValue>>,
            _: Option<String>,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("mock")
        }
        async fn delete_prepared(&self, _: String) -> anyhow::Result<()> {
            anyhow::bail!("mock")
        }
        async fn get_prepared_stats(&self, _: Vec<String>) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("mock")
        }
    }

    async fn build_test_app_state() -> (AppState, TempDir) {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let storage_path = temp_dir.path().join("storage");
        let metadata_path = temp_dir.path().join("metadata");
        let data_dir = temp_dir.path().join("server_data");
        std::fs::create_dir_all(&storage_path).expect("failed to create storage path");
        std::fs::create_dir_all(&metadata_path).expect("failed to create metadata path");
        std::fs::create_dir_all(&data_dir).expect("failed to create data dir");

        let mut config = crate::core::config::Config::default();
        config.server.data_dir = data_dir.clone();
        config.storage.metadata_url = file_url(&metadata_path);
        config.storage.storage_locations = vec![crate::core::config::StorageLocation {
            url: file_url(&storage_path),
            weight: 1,
            tags: vec!["test".to_string()],
        }];

        let (shared_services, _) = crate::network::multi_server::SharedServices::new(
            None,
            &config.storage,
            None,
            Some(&config),
        )
        .await
        .expect("failed to initialize shared services for test app state");

        let state = AppState::new(
            shared_services.request_handlers,
            shared_services.graph_execution_service,
            None,
            data_dir,
            None,
            None,
        )
        .with_ports(
            Arc::new(MockDocumentPort),
            Arc::new(MockGraphPort),
            Arc::new(MockObservabilityPort),
        )
        .with_unified_query_port(Arc::new(MockUnifiedQueryPort));
        (state, temp_dir)
    }

    #[test]
    fn test_error_conversion() {
        let err = ApiError::CollectionNotFound("test_collection".to_string());
        let response = ProtoApiResponse::<()>::error(err);
        assert!(!response.success);
        assert!(response.error.is_some());
    }

    #[test]
    fn test_hybrid_search_request_deserialization() {
        let json = serde_json::json!({
            "collection": "test_col",
            "vector": [0.1, 0.2, 0.3],
            "text_query": "machine learning",
            "top_k": 5,
            "vector_weight": 0.7
        });
        let req: LegacyHybridSearchRequest =
            serde_json::from_value(json).expect("failed to deserialize HybridSearchRequest");
        assert_eq!(req.collection, "test_col");
        assert_eq!(req.vector.expect("vector should be present").len(), 3);
        assert_eq!(
            req.text_query.expect("text_query should be present"),
            "machine learning"
        );
        assert_eq!(req.top_k, 5);
        assert!((req.vector_weight - 0.7).abs() < 0.001);
        assert_eq!(req.rrf_k, 60); // default
    }

    #[test]
    fn test_hybrid_search_request_defaults() {
        let json = serde_json::json!({
            "collection": "test_col",
            "text_query": "hello"
        });
        let req: LegacyHybridSearchRequest =
            serde_json::from_value(json).expect("failed to deserialize HybridSearchRequest");
        assert_eq!(req.top_k, 10);
        assert!((req.vector_weight - 0.5).abs() < 0.001);
        assert_eq!(req.rrf_k, 60);
        assert!(req.vector.is_none());
    }

    #[test]
    fn test_hybrid_index_request_deserialization() {
        let json = serde_json::json!({
            "collection": "test_col",
            "documents": [
                {"id": "doc1", "text": "The quick brown fox"},
                {"id": "doc2", "text": "jumps over the lazy dog"}
            ]
        });
        let req: HybridIndexRequest =
            serde_json::from_value(json).expect("failed to deserialize HybridIndexRequest");
        assert_eq!(req.collection, "test_col");
        assert_eq!(req.documents.len(), 2);
        assert_eq!(req.documents[0].id, "doc1");
        assert_eq!(req.documents[1].text, "jumps over the lazy dog");
    }

    #[test]
    fn test_fulltext_index_map_operations() {
        use crate::storage::engines::core::formats::columnar::fulltext_index::{
            FullTextIndex, TokenizerConfig,
        };

        let map: FullTextIndexMap =
            Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        // Add an index
        {
            let mut indexes = map.write().expect("RwLock should not be poisoned");
            let mut index = FullTextIndex::new(TokenizerConfig::for_keyword_search());
            index
                .add_document("doc1", "machine learning neural networks")
                .expect("failed to add document to index");
            index
                .add_document("doc2", "deep learning transformers")
                .expect("failed to add document to index");
            index
                .add_document("doc3", "database systems query optimization")
                .expect("failed to add document to index");
            indexes.insert("test_col".to_string(), index);
        }

        // Search
        {
            let indexes = map.read().expect("RwLock should not be poisoned");
            let index = indexes
                .get("test_col")
                .expect("test_col index should exist");
            let results = index.search("learning", 10);
            assert_eq!(results.len(), 2);
            // doc1 and doc2 both contain "learning"
            let ids: Vec<&str> = results.iter().map(|r| r.doc_id.as_str()).collect();
            assert!(ids.contains(&"doc1"));
            assert!(ids.contains(&"doc2"));
        }
    }

    #[test]
    fn test_hybrid_search_response_serialization() {
        let response = LegacyHybridSearchResponse {
            success: true,
            results: vec![
                HybridSearchHit {
                    id: "doc1".to_string(),
                    combined_score: 0.05,
                    vector_score: Some(0.95),
                    bm25_score: Some(3.2),
                    vector_rank: Some(1),
                    bm25_rank: Some(2),
                    matched_terms: vec!["learning".to_string()],
                },
                HybridSearchHit {
                    id: "doc2".to_string(),
                    combined_score: 0.03,
                    vector_score: None,
                    bm25_score: Some(5.1),
                    vector_rank: None,
                    bm25_rank: Some(1),
                    matched_terms: vec!["machine".to_string(), "learning".to_string()],
                },
            ],
            total: 2,
            processing_time_us: 1234,
            mode: "hybrid".to_string(),
        };

        let json =
            serde_json::to_string(&response).expect("failed to serialize HybridSearchResponse");
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"mode\":\"hybrid\""));
        // doc2 should NOT have vector_score/vector_rank (skip_serializing_if = None)
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("failed to deserialize JSON value");
        let doc2 = &parsed["results"][1];
        assert!(doc2.get("vector_score").is_none());
        assert!(doc2.get("vector_rank").is_none());
    }

    // Test ApiError variants
    #[test]
    fn test_api_error_variants() {
        use std::io;

        // Test CollectionNotFound
        let err = ApiError::CollectionNotFound("test_col".to_string());
        assert_eq!(err.to_string(), "Collection not found: test_col");

        // Test InvalidArgument
        let err = ApiError::InvalidArgument("bad argument".to_string());
        assert_eq!(err.to_string(), "Invalid argument: bad argument");

        // Test Internal
        let err = ApiError::Internal("internal error".to_string());
        assert_eq!(err.to_string(), "Internal error: internal error");

        // Test IO error message propagation
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let api_err = ApiError::Internal(io_err.to_string());
        assert!(api_err.to_string().contains("file not found"));
    }

    // Test ApiDisplay trait implementation
    #[test]
    fn test_api_display() {
        let err = ApiError::CollectionNotFound("my_collection".to_string());
        let display = format!("{}", err);
        assert!(display.contains("my_collection"));
    }

    // Test error message formatting
    #[test]
    fn test_error_message_formatting() {
        let errors = vec![
            ApiError::CollectionNotFound("test".to_string()),
            ApiError::InvalidArgument("invalid".to_string()),
            ApiError::Internal("server error".to_string()),
        ];

        for err in errors {
            let msg = format!("{}", err);
            assert!(!msg.is_empty());
            assert!(!msg.contains("ApiError(")); // Should be user-friendly
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_hybrid_search_canonical_production_route_returns_bad_request_not_not_found() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/hybrid/search")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"collection":"","text_query":"hybrid route test"}"#,
            ))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling canonical hybrid route request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_vector_search_canonical_production_route_returns_bad_request_not_not_found() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/search")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"collection":"","vector":[0.1,0.2,0.3],"top_k":5}"#,
            ))
            .expect("failed to build request");
        request
            .extensions_mut()
            .insert(crate::network::middleware::tenant::TenantContext::default_tenant());

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling canonical vector route request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_document_index_canonical_production_route_returns_bad_request_not_not_found() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/documents/collections/ws1_docs/indexes")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"path":"content","index_type":"fulltext","unique":false}"#,
            ))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling canonical document route request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_graph_shortest_path_canonical_production_route_returns_unprocessable_entity_not_not_found()
     {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/graph/graphs/ws1_graph/shortest_path")
            .header("content-type", "application/json")
            .body(Body::from(r#"{}"#))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling canonical graph route request");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_graph_legacy_nodes_endpoint_redirects_to_canonical_multi_graph_route() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/graph/nodes")
            .header("content-type", "application/json")
            .body(Body::from(r#"{}"#))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling legacy graph nodes route request");
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok());
        assert_eq!(location, Some("/api/v1/graph/graphs/default/nodes"));
        let deprecation = response
            .headers()
            .get("deprecation")
            .and_then(|v| v.to_str().ok());
        assert_eq!(deprecation, Some("true"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_graph_legacy_edges_endpoint_redirects_to_canonical_multi_graph_route() {
        let (state, _temp_dir) = build_test_app_state().await;
        let router = create_router(state);
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/graph/edges")
            .header("content-type", "application/json")
            .body(Body::from(r#"{}"#))
            .expect("failed to build request");

        let response = router
            .oneshot(request)
            .await
            .expect("router failed handling legacy graph edges route request");
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok());
        assert_eq!(location, Some("/api/v1/graph/graphs/default/edges"));
        let deprecation = response
            .headers()
            .get("deprecation")
            .and_then(|v| v.to_str().ok());
        assert_eq!(deprecation, Some("true"));
    }

    // ============================================================
    // sql_value_to_json tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_sql_value_to_json_string() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::StringValue(
                "hello".to_string(),
            )),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!("hello"));
    }

    #[test]
    fn test_sql_value_to_json_number() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::NumberValue(42.5)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!(42.5));
    }

    #[test]
    fn test_sql_value_to_json_bool() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::BoolValue(true)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!(true));
    }

    #[test]
    fn test_sql_value_to_json_int64() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::Int64Value(9999)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!(9999));
    }

    #[test]
    fn test_sql_value_to_json_bytes() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::BytesValue(vec![0, 1, 255])),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!([0, 1, 255]));
    }

    #[test]
    fn test_sql_value_to_json_null() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::NullValue(0)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn test_sql_value_to_json_none() {
        let val = proximadb_v1::SqlValue { value: None };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn test_sql_value_to_json_array() {
        let arr = proximadb_v1::SqlArray {
            values: vec![
                proximadb_v1::SqlValue {
                    value: Some(proximadb_v1::sql_value::Value::Int64Value(1)),
                },
                proximadb_v1::SqlValue {
                    value: Some(proximadb_v1::sql_value::Value::Int64Value(2)),
                },
            ],
        };
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::ArrayValue(arr)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json, serde_json::json!([1, 2]));
    }

    #[test]
    fn test_sql_value_to_json_object() {
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            "name".to_string(),
            proximadb_v1::SqlValue {
                value: Some(proximadb_v1::sql_value::Value::StringValue(
                    "Alice".to_string(),
                )),
            },
        );
        fields.insert(
            "age".to_string(),
            proximadb_v1::SqlValue {
                value: Some(proximadb_v1::sql_value::Value::Int64Value(30)),
            },
        );
        let obj = proximadb_v1::SqlObject { fields };
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::ObjectValue(obj)),
        };
        let json = sql_value_to_json(&val);
        assert_eq!(json["name"], serde_json::json!("Alice"));
        assert_eq!(json["age"], serde_json::json!(30));
    }

    #[test]
    fn test_sql_value_to_json_nan_number() {
        let val = proximadb_v1::SqlValue {
            value: Some(proximadb_v1::sql_value::Value::NumberValue(f64::NAN)),
        };
        let json = sql_value_to_json(&val);
        // NaN cannot be represented in JSON, falls back to 0
        assert_eq!(json, serde_json::json!(0));
    }

    // ============================================================
    // SqlQueryRequest/SqlColumnInfo tests
    // ============================================================

    #[test]
    fn test_sql_query_request_deserialization() {
        let json = serde_json::json!({
            "query": "SELECT * FROM my_collection LIMIT 10",
            "collection": "my_collection",
            "timeout_ms": 5000
        });
        let req: SqlQueryRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.query, "SELECT * FROM my_collection LIMIT 10");
        assert_eq!(req.collection, Some("my_collection".to_string()));
        assert_eq!(req.timeout_ms, Some(5000));
        assert!(req.parameters.is_none());
        assert!(req.seeding.is_none());
    }

    #[test]
    fn test_sql_column_info_serialization() {
        let col = SqlColumnInfo {
            name: "embedding".to_string(),
            data_type: "vector".to_string(),
        };
        let json = serde_json::to_string(&col).unwrap();
        assert!(json.contains("embedding"));
        assert!(json.contains("vector"));
    }

    /// Verify search response (VectorOperationResponse) serializes correctly to JSON
    #[test]
    fn test_search_response_serialization() {
        let response = proximadb_v1::VectorOperationResponse {
            success: true,
            operation: 1, // search
            metrics: Some(proximadb_v1::OperationMetrics {
                total_processed: 100,
                successful_count: 95,
                failed_count: 5,
                updated_count: 0,
                processing_time_us: 1500,
                wal_write_time_us: 0,
                index_update_time_us: 0,
            }),
            results: Some(proximadb_v1::SearchResult {
                results: vec![
                    {
                        let mut r = proximadb_v1::SearchVectorRecord::default();
                        r.id = "result_1".to_string();
                        r.score = 0.95;
                        r.vector = vec![0.1, 0.2, 0.3];
                        r.version = Some(1);
                        r.similarity = Some(0.95);
                        r
                    },
                    {
                        let mut r = proximadb_v1::SearchVectorRecord::default();
                        r.id = "result_2".to_string();
                        r.score = 0.87;
                        r.vector = vec![0.4, 0.5, 0.6];
                        r.version = Some(1);
                        r.similarity = Some(0.87);
                        r
                    },
                ],
                total_found: 2,
                collection_id: Some("test_collection".to_string()),
            }),
            vector_ids: vec!["result_1".to_string(), "result_2".to_string()],
            error_message: None,
            error_code: None,
        };

        let json_str = serde_json::to_string(&response)
            .expect("VectorOperationResponse should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("serialized JSON should parse back");

        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["operation"], 1);
        assert_eq!(parsed["results"]["total_found"], 2);
        assert_eq!(parsed["results"]["results"][0]["id"], "result_1");
        assert_eq!(parsed["results"]["results"][1]["id"], "result_2");
        assert!(parsed["results"]["results"][0]["score"].as_f64().unwrap() > 0.9);
        assert_eq!(parsed["vector_ids"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["metrics"]["total_processed"], 100);
        assert_eq!(parsed["metrics"]["processing_time_us"], 1500);
    }

    // ============================================================
    // SQL operations tests (3 tests)
    // ============================================================

    /// Verify SqlQueryRequest deserializes correctly with all fields
    #[test]
    fn test_sql_query_request_parsing() {
        // Full request with all optional fields
        let json = serde_json::json!({
            "query": "SELECT id, metadata FROM vectors WHERE category = 'electronics' ORDER BY score LIMIT 20",
            "collection": "products",
            "timeout_ms": 10000,
            "seeding": "per_seed",
            "parameters": [
                {"value": {"StringValue": "electronics"}},
                {"value": {"Int64Value": 20}}
            ]
        });
        let req: SqlQueryRequest =
            serde_json::from_value(json).expect("full SqlQueryRequest should deserialize");
        assert_eq!(
            req.query,
            "SELECT id, metadata FROM vectors WHERE category = 'electronics' ORDER BY score LIMIT 20"
        );
        assert_eq!(req.collection, Some("products".to_string()));
        assert_eq!(req.timeout_ms, Some(10000));
        assert_eq!(req.seeding, Some("per_seed".to_string()));
        assert!(req.parameters.is_some());
        assert_eq!(req.parameters.as_ref().unwrap().len(), 2);

        // Minimal request with only required fields
        let minimal_json = serde_json::json!({
            "query": "SELECT * FROM test"
        });
        let minimal_req: SqlQueryRequest = serde_json::from_value(minimal_json)
            .expect("minimal SqlQueryRequest should deserialize");
        assert_eq!(minimal_req.query, "SELECT * FROM test");
        assert!(minimal_req.collection.is_none());
        assert!(minimal_req.timeout_ms.is_none());
        assert!(minimal_req.seeding.is_none());
        assert!(minimal_req.parameters.is_none());
    }

    /// Verify SQL response format serializes correctly
    #[test]
    fn test_sql_response_serialization() {
        // Simulate the JSON response structure returned by execute_sql handler
        let response_json = serde_json::json!({
            "rows": [
                {"id": "vec1", "score": 0.95, "category": "electronics"},
                {"id": "vec2", "score": 0.87, "category": "books"}
            ],
            "columns": ["id", "score", "category"],
            "column_types": ["string", "float", "string"],
            "execution_time_ms": 42,
            "rows_returned": 2,
            "row_count": 2,
            "rows_scanned": 100,
            "request_id": "req-123"
        });

        // Verify all fields are present and have correct types
        assert_eq!(response_json["rows"].as_array().unwrap().len(), 2);
        assert_eq!(response_json["rows"][0]["id"], "vec1");
        assert_eq!(response_json["rows"][1]["category"], "books");
        assert_eq!(response_json["columns"].as_array().unwrap().len(), 3);
        assert_eq!(response_json["execution_time_ms"], 42);
        assert_eq!(response_json["rows_returned"], 2);
        assert_eq!(response_json["row_count"], 2);
        assert_eq!(response_json["rows_scanned"], 100);
        assert_eq!(response_json["request_id"], "req-123");

        // Verify the response round-trips through serialization
        let serialized = serde_json::to_string(&response_json)
            .expect("SQL response JSON should serialize to string");
        let deserialized: serde_json::Value = serde_json::from_str(&serialized)
            .expect("serialized SQL response should deserialize back");
        assert_eq!(response_json, deserialized);
    }

    /// Empty query string produces an error (validated in handler, not in parsing)
    #[test]
    fn test_invalid_sql_request() {
        // Empty query string deserializes fine at the serde level
        let empty_query = serde_json::json!({
            "query": ""
        });
        let req: SqlQueryRequest =
            serde_json::from_value(empty_query).expect("empty query should still deserialize");
        assert_eq!(req.query, "");
        // Handler-level validation: query.trim().is_empty() returns true
        assert!(
            req.query.trim().is_empty(),
            "empty query should be detected by handler validation"
        );

        // Whitespace-only query
        let whitespace_query = serde_json::json!({
            "query": "   \t\n  "
        });
        let req: SqlQueryRequest = serde_json::from_value(whitespace_query)
            .expect("whitespace query should still deserialize");
        assert!(
            req.query.trim().is_empty(),
            "whitespace-only query should be detected by handler validation"
        );

        // Missing required 'query' field entirely should fail deserialization
        let missing_query = serde_json::json!({
            "collection": "test_col",
            "timeout_ms": 5000
        });
        let result = serde_json::from_value::<SqlQueryRequest>(missing_query);
        assert!(
            result.is_err(),
            "missing 'query' field should fail deserialization"
        );
    }

    // ============================================================
    // Collection operations tests (4 tests)
    // ============================================================

    /// Verify CollectionRequest (create) deserializes from JSON
    #[test]
    fn test_create_collection_request_parsing() {
        let json = serde_json::json!({
            "operation": 1, // CollectionCreate
            "collection_id": "new_collection",
            "collection_config": {
                "name": "new_collection",
                "dimension": 128,
                "distance_metric": 0,
                "tags": ["test", "development"],
                "description": "A test collection for unit testing"
            }
        });
        let req: CollectionRequest =
            serde_json::from_value(json).expect("CollectionRequest (create) should deserialize");
        assert_eq!(req.operation, CollectionOperation::CollectionCreate as i32);
        assert_eq!(req.collection_id, Some("new_collection".to_string()));
        assert!(req.collection_config.is_some());
        let config = req.collection_config.unwrap();
        assert_eq!(config.name, "new_collection");
        assert_eq!(config.dimension, 128);
        assert_eq!(config.tags.len(), 2);
        assert_eq!(config.tags[0], "test");
        assert_eq!(
            config.description,
            Some("A test collection for unit testing".to_string())
        );

        // Minimal create request (only operation and name)
        let minimal_json = serde_json::json!({
            "operation": 1,
            "collection_config": {
                "name": "minimal_col",
                "dimension": 64
            }
        });
        let minimal_req: CollectionRequest = serde_json::from_value(minimal_json)
            .expect("minimal CollectionRequest should deserialize");
        assert_eq!(minimal_req.operation, 1);
        assert!(minimal_req.collection_config.is_some());
        assert_eq!(minimal_req.collection_config.unwrap().dimension, 64);
    }

    /// Verify CollectionResponse serializes correctly to JSON
    #[test]
    fn test_collection_response_serialization() {
        let response = proximadb_v1::CollectionResponse {
            success: true,
            collection: Some(proximadb_v1::Collection {
                id: "col_123".to_string(),
                config: Some(proximadb_v1::CollectionConfig {
                    name: "test_collection".to_string(),
                    dimension: 128,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            collections: vec![],
            error_message: None,
            error_code: None,
            operation: CollectionOperation::CollectionCreate as i32,
            affected_count: 1,
            total_count: 1,
            metadata: HashMap::new(),
            processing_time_us: 250,
        };

        let json_str =
            serde_json::to_string(&response).expect("CollectionResponse should serialize to JSON");
        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .expect("serialized CollectionResponse should parse back");

        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["collection"]["id"], "col_123");
        assert_eq!(parsed["collection"]["config"]["name"], "test_collection");
        assert_eq!(parsed["collection"]["config"]["dimension"], 128);
        assert_eq!(
            parsed["operation"],
            CollectionOperation::CollectionCreate as i32
        );
        assert_eq!(parsed["affected_count"], 1);
        assert_eq!(parsed["processing_time_us"], 250);

        // Error response
        let error_response = proximadb_v1::CollectionResponse {
            success: false,
            collection: None,
            collections: vec![],
            error_message: Some("Collection already exists".to_string()),
            error_code: Some("ALREADY_EXISTS".to_string()),
            operation: CollectionOperation::CollectionCreate as i32,
            affected_count: 0,
            total_count: 0,
            metadata: HashMap::new(),
            processing_time_us: 10,
        };

        let err_json_str = serde_json::to_string(&error_response)
            .expect("error CollectionResponse should serialize");
        let err_parsed: serde_json::Value = serde_json::from_str(&err_json_str)
            .expect("serialized error response should parse back");
        assert_eq!(err_parsed["success"], false);
        assert_eq!(err_parsed["error_message"], "Collection already exists");
        assert_eq!(err_parsed["error_code"], "ALREADY_EXISTS");
    }

    /// Verify list collections response with multiple collections
    #[test]
    fn test_list_collections_response() {
        let response = proximadb_v1::CollectionResponse {
            success: true,
            collection: None,
            collections: vec![
                proximadb_v1::Collection {
                    id: "col_1".to_string(),
                    config: Some(proximadb_v1::CollectionConfig {
                        name: "vectors_prod".to_string(),
                        dimension: 128,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                proximadb_v1::Collection {
                    id: "col_2".to_string(),
                    config: Some(proximadb_v1::CollectionConfig {
                        name: "vectors_staging".to_string(),
                        dimension: 256,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                proximadb_v1::Collection {
                    id: "col_3".to_string(),
                    config: Some(proximadb_v1::CollectionConfig {
                        name: "embeddings_test".to_string(),
                        dimension: 512,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            error_message: None,
            error_code: None,
            operation: CollectionOperation::CollectionList as i32,
            affected_count: 0,
            total_count: 3,
            metadata: HashMap::new(),
            processing_time_us: 500,
        };

        let json_str =
            serde_json::to_string(&response).expect("list collections response should serialize");
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("serialized list response should parse back");

        assert_eq!(parsed["success"], true);
        assert_eq!(
            parsed["operation"],
            CollectionOperation::CollectionList as i32
        );
        assert_eq!(parsed["total_count"], 3);
        let collections = parsed["collections"]
            .as_array()
            .expect("collections should be an array");
        assert_eq!(collections.len(), 3);
        assert_eq!(collections[0]["id"], "col_1");
        assert_eq!(collections[0]["config"]["name"], "vectors_prod");
        assert_eq!(collections[1]["id"], "col_2");
        assert_eq!(collections[1]["config"]["name"], "vectors_staging");
        assert_eq!(collections[2]["id"], "col_3");
        assert_eq!(collections[2]["config"]["name"], "embeddings_test");

        // Empty list response
        let empty_response = proximadb_v1::CollectionResponse {
            success: true,
            collection: None,
            collections: vec![],
            error_message: None,
            error_code: None,
            operation: CollectionOperation::CollectionList as i32,
            affected_count: 0,
            total_count: 0,
            metadata: HashMap::new(),
            processing_time_us: 50,
        };

        let empty_json =
            serde_json::to_string(&empty_response).expect("empty list response should serialize");
        let empty_parsed: serde_json::Value =
            serde_json::from_str(&empty_json).expect("empty list should parse back");
        assert_eq!(empty_parsed["collections"].as_array().unwrap().len(), 0);
        assert_eq!(empty_parsed["total_count"], 0);
    }

    /// Verify delete collection request constructs correctly
    #[test]
    fn test_delete_collection_request() {
        // Verify the CollectionRequest for delete operation can be constructed and serialized
        let delete_request = CollectionRequest {
            operation: CollectionOperation::CollectionDelete as i32,
            collection_id: Some("col_to_delete".to_string()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };

        assert_eq!(
            delete_request.operation,
            CollectionOperation::CollectionDelete as i32
        );
        assert_eq!(
            delete_request.collection_id,
            Some("col_to_delete".to_string())
        );
        assert!(delete_request.collection_config.is_none());

        // Verify it serializes to JSON
        let json_str = serde_json::to_string(&delete_request)
            .expect("delete CollectionRequest should serialize");
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("serialized delete request should parse back");
        assert_eq!(
            parsed["operation"],
            CollectionOperation::CollectionDelete as i32
        );
        assert_eq!(parsed["collection_id"], "col_to_delete");

        // Verify deserialization round-trip
        let deserialized: CollectionRequest =
            serde_json::from_str(&json_str).expect("delete request should round-trip through JSON");
        assert_eq!(deserialized.operation, delete_request.operation);
        assert_eq!(deserialized.collection_id, delete_request.collection_id);

        // Verify CollectionOperation enum conversion
        let op = CollectionOperation::try_from(delete_request.operation);
        assert!(op.is_ok());
        assert_eq!(op.unwrap(), CollectionOperation::CollectionDelete);

        // Verify invalid operation value is rejected
        let invalid_op = CollectionOperation::try_from(999);
        assert!(
            invalid_op.is_err(),
            "invalid operation value 999 should fail enum conversion"
        );
    }

    // ============================================================
    // Hybrid search tests (3 tests)
    // ============================================================

    /// Verify HybridSearchRequest deserializes with all field combinations
    #[test]
    fn test_hybrid_search_request_parsing() {
        // Full hybrid request (vector + text)
        let full_json = serde_json::json!({
            "collection": "hybrid_col",
            "vector": [0.1, 0.2, 0.3, 0.4],
            "text_query": "machine learning algorithms",
            "top_k": 20,
            "vector_weight": 0.6,
            "rrf_k": 100,
            "min_bm25_score": 0.5
        });
        let full_req: LegacyHybridSearchRequest =
            serde_json::from_value(full_json).expect("full HybridSearchRequest should deserialize");
        assert_eq!(full_req.collection, "hybrid_col");
        assert_eq!(full_req.vector.as_ref().unwrap().len(), 4);
        assert!((full_req.vector.as_ref().unwrap()[0] - 0.1).abs() < 1e-6);
        assert_eq!(
            full_req.text_query,
            Some("machine learning algorithms".to_string())
        );
        assert_eq!(full_req.top_k, 20);
        assert!((full_req.vector_weight - 0.6).abs() < 0.001);
        assert_eq!(full_req.rrf_k, 100);
        assert!((full_req.min_bm25_score - 0.5).abs() < 0.001);

        // Vector-only request (no text_query)
        let vector_only_json = serde_json::json!({
            "collection": "vec_only",
            "vector": [1.0, 2.0, 3.0]
        });
        let vec_req: LegacyHybridSearchRequest = serde_json::from_value(vector_only_json)
            .expect("vector-only HybridSearchRequest should deserialize");
        assert_eq!(vec_req.collection, "vec_only");
        assert!(vec_req.vector.is_some());
        assert!(vec_req.text_query.is_none());
        assert_eq!(vec_req.top_k, 10); // default
        assert!((vec_req.vector_weight - 0.5).abs() < 0.001); // default
        assert_eq!(vec_req.rrf_k, 60); // default

        // Text-only request (no vector)
        let text_only_json = serde_json::json!({
            "collection": "text_only",
            "text_query": "database systems"
        });
        let text_req: LegacyHybridSearchRequest = serde_json::from_value(text_only_json)
            .expect("text-only HybridSearchRequest should deserialize");
        assert_eq!(text_req.collection, "text_only");
        assert!(text_req.vector.is_none());
        assert_eq!(text_req.text_query, Some("database systems".to_string()));
    }

    /// Verify HybridSearchResponse serializes correctly including skip_serializing_if behavior
    #[test]
    fn test_hybrid_search_response_serialization_extended() {
        // Response with mixed hit types: some have vector scores, some have bm25 only
        let response = LegacyHybridSearchResponse {
            success: true,
            results: vec![
                HybridSearchHit {
                    id: "doc_a".to_string(),
                    combined_score: 0.042,
                    vector_score: Some(0.92),
                    bm25_score: Some(4.5),
                    vector_rank: Some(1),
                    bm25_rank: Some(3),
                    matched_terms: vec!["neural".to_string(), "network".to_string()],
                },
                HybridSearchHit {
                    id: "doc_b".to_string(),
                    combined_score: 0.035,
                    vector_score: None, // BM25-only hit
                    bm25_score: Some(6.2),
                    vector_rank: None,
                    bm25_rank: Some(1),
                    matched_terms: vec!["deep".to_string(), "learning".to_string()],
                },
                HybridSearchHit {
                    id: "doc_c".to_string(),
                    combined_score: 0.028,
                    vector_score: Some(0.85), // Vector-only hit
                    bm25_score: None,
                    vector_rank: Some(2),
                    bm25_rank: None,
                    matched_terms: vec![],
                },
            ],
            total: 3,
            processing_time_us: 2500,
            mode: "hybrid".to_string(),
        };

        let json_str =
            serde_json::to_string(&response).expect("HybridSearchResponse should serialize");
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("serialized response should parse back");

        // Verify top-level fields
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["total"], 3);
        assert_eq!(parsed["processing_time_us"], 2500);
        assert_eq!(parsed["mode"], "hybrid");

        // doc_a: has all scores
        let doc_a = &parsed["results"][0];
        assert_eq!(doc_a["id"], "doc_a");
        assert!(doc_a.get("vector_score").is_some());
        assert!(doc_a.get("bm25_score").is_some());
        assert!(doc_a.get("vector_rank").is_some());
        assert!(doc_a.get("bm25_rank").is_some());
        assert_eq!(doc_a["matched_terms"].as_array().unwrap().len(), 2);

        // doc_b: BM25-only (vector_score and vector_rank should be absent due to skip_serializing_if)
        let doc_b = &parsed["results"][1];
        assert_eq!(doc_b["id"], "doc_b");
        assert!(
            doc_b.get("vector_score").is_none(),
            "BM25-only hit should omit vector_score"
        );
        assert!(
            doc_b.get("vector_rank").is_none(),
            "BM25-only hit should omit vector_rank"
        );
        assert!(doc_b.get("bm25_score").is_some());
        assert!(doc_b.get("bm25_rank").is_some());

        // doc_c: Vector-only (bm25_score and bm25_rank should be absent)
        let doc_c = &parsed["results"][2];
        assert_eq!(doc_c["id"], "doc_c");
        assert!(
            doc_c.get("bm25_score").is_none(),
            "vector-only hit should omit bm25_score"
        );
        assert!(
            doc_c.get("bm25_rank").is_none(),
            "vector-only hit should omit bm25_rank"
        );
        assert!(doc_c.get("vector_score").is_some());
        assert!(doc_c.get("vector_rank").is_some());
        assert_eq!(doc_c["matched_terms"].as_array().unwrap().len(), 0);
    }

    /// Verify HybridIndexRequest deserializes and validates correctly
    #[test]
    fn test_hybrid_index_request_parsing() {
        // Standard index request with multiple documents
        let json = serde_json::json!({
            "collection": "index_col",
            "documents": [
                {"id": "doc1", "text": "Introduction to machine learning"},
                {"id": "doc2", "text": "Advanced neural network architectures"},
                {"id": "doc3", "text": "Database query optimization techniques"},
                {"id": "doc4", "text": "Distributed systems and consensus algorithms"}
            ]
        });
        let req: HybridIndexRequest =
            serde_json::from_value(json).expect("HybridIndexRequest should deserialize");
        assert_eq!(req.collection, "index_col");
        assert_eq!(req.documents.len(), 4);
        assert_eq!(req.documents[0].id, "doc1");
        assert_eq!(req.documents[0].text, "Introduction to machine learning");
        assert_eq!(req.documents[3].id, "doc4");
        assert!(req.documents[3].text.contains("consensus"));

        // HybridIndexResponse serialization
        let response = HybridIndexResponse {
            success: true,
            collection: "index_col".to_string(),
            documents_indexed: 4,
            total_documents: 10,
        };
        let resp_json =
            serde_json::to_string(&response).expect("HybridIndexResponse should serialize");
        let resp_parsed: serde_json::Value =
            serde_json::from_str(&resp_json).expect("serialized index response should parse back");
        assert_eq!(resp_parsed["success"], true);
        assert_eq!(resp_parsed["collection"], "index_col");
        assert_eq!(resp_parsed["documents_indexed"], 4);
        assert_eq!(resp_parsed["total_documents"], 10);

        // Empty documents list should deserialize (validation happens in handler)
        let empty_docs = serde_json::json!({
            "collection": "empty_col",
            "documents": []
        });
        let empty_req: HybridIndexRequest = serde_json::from_value(empty_docs)
            .expect("empty documents HybridIndexRequest should deserialize");
        assert_eq!(empty_req.documents.len(), 0);
    }
}
