// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Shared service composition layer for ProximaDB protocol handlers.
//!
//! `SharedServices` owns and wires together all business-logic services
//! (storage, graph, document, observability, query) that are shared across
//! REST, gRPC, Arrow Flight, and PostgreSQL wire protocol handlers.
//! It is the composition root for the server-side service graph.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::metrics::MetricsConfig;
use crate::monitoring::MetricsCollector;
use crate::observability::query::ObservabilityQueryEngine;
use crate::observability::storage::ObservabilityStorage;
use crate::query::facade::strategies::{DistributedQueryStrategy, DistributedStrategyConfig};
use crate::query::facade::{
    ColumnarStrategy, DocumentStrategy, FacadeConfig, GraphStrategy, ObservabilityStrategy,
    QueryFacadeAdapter, SqlStrategy, UnifiedQueryFacade, VectorSearchStrategy,
};
use crate::query::federated::FederatedQueryContext;
use crate::services::collection::manager::CollectionService;
use crate::services::{DmlService, VectorOperationsService};
use crate::storage::MultiModelStorageFacade;
use crate::storage::StorageEngine;
use crate::storage::document::DocumentService;
use crate::storage::metadata::backends::MetadataBackendFactory;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb_graph_query::service::{GraphExecutionService, GraphQueryService};
use proximadb_kernel::uuid::Uuid;

/// Shared services for thin protocol handlers
/// Responsibilities: business logic, metadata configuration, service coordination
#[derive(Clone)]
pub struct SharedServices {
    /// Shared xCatalog control plane for REST, gRPC, Arrow Flight, SQL, and query routing.
    pub catalog_manager: Arc<crate::catalog::CatalogManager>,
    /// PAX segment registry — bridges write path with Iceberg REST snapshot stats.
    /// Shared with `AppState::segment_registry` via `Arc` clone in `build_router_for_unified`.
    pub segment_registry: Arc<crate::catalog::SegmentRegistry>,
    /// Collection lifecycle management service
    pub collection_service: Arc<CollectionService>,
    /// Vector CRUD and search operations service
    pub vector_operations_service: Arc<VectorOperationsService>,
    /// Concrete graph database operations service for native graph APIs and gRPC graph endpoints
    pub graph_service: Arc<crate::graph::GraphService>,
    /// Extracted graph query/traversal capability for query-facing orchestration layers
    pub graph_query_service: Arc<dyn GraphQueryService>,
    /// Extracted graph execution capability for planners/executors and API state holders
    pub graph_execution_service: Arc<dyn GraphExecutionService>,
    /// Document storage and retrieval service
    pub document_service: Arc<DocumentService>,
    /// Observability service for logs, metrics, and traces
    pub observability_service: Arc<crate::observability::ObservabilityService>,
    /// Unified request handlers shared across all protocol layers
    pub request_handlers: Arc<UnifiedHandlers>,
    /// Optional metrics collector for Prometheus/monitoring integration
    pub metrics_collector: Option<Arc<MetricsCollector>>,
    /// Optional internal metrics updater for background metric publishing
    pub metrics_updater: Option<Arc<dyn crate::metrics::InternalMetricsUpdater + 'static>>,
    /// Port-backed API handler for collection/vector routes (runtime crate handler).
    ///
    /// Backed by `CollectionPort`, `VectorOpsPort`, and `QueryAdapterPort` trait objects
    /// so the REST/gRPC API surface is decoupled from root-crate concrete services.
    pub api_handlers: Arc<dyn proximadb_runtime::ApiHandlersPort>,
    /// Unified query facade - single entry point for all query types
    /// Consolidates vector search, SQL, and graph query paths
    pub query_facade: Arc<UnifiedQueryFacade>,
    /// Optional cluster orchestration port (Phase 9.12 / Task #72).
    ///
    /// Production bootstrap currently passes `None` for single-node
    /// deployments. When `[distributed]` config is populated, a
    /// `ClusterManager` (which `impl proximadb_runtime::ClusterPort for`
    /// — see `src/cluster/mod.rs:305`) should be constructed and stored
    /// here. No consumer reads this field yet; the slot exists so that
    /// future health-endpoint / cluster-state-reporting code can pull
    /// cluster state via the port without re-plumbing.
    /// See `docs/_internal/status/PHASE9_REMAINING_2026_05_25.adoc`
    /// for the full Task #72 wiring plan.
    pub cluster_port: Option<Arc<dyn proximadb_runtime::ClusterPort>>,
    /// Port-typed view of `collection_service` for Phase 9.10 / Task #76
    /// consumer migration.
    ///
    /// Same underlying `CollectionService` instance as `collection_service`,
    /// just held behind the `CollectionPort` trait object so consumers can
    /// migrate off the concrete type incrementally. Once all consumers use
    /// `collection_port`, the concrete `collection_service` field can be
    /// dropped — that landing is what completes the Task #76 collection-service
    /// slice. Same parallel-field pattern as the existing `api_handlers`
    /// (which shadows `request_handlers`).
    pub collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
    /// Port-typed view of `vector_operations_service` for Phase 9.10 / Task #76
    /// consumer migration. Same pattern as `collection_port` above.
    pub vector_ops_port: Arc<dyn proximadb_runtime::VectorOpsPort>,
    /// Port-typed view of `document_service` for Phase 9.10 / Task #76
    /// consumer migration. Same pattern as `collection_port` above.
    ///
    /// Powered by `impl DocumentPort for DocumentService` directly on the
    /// bare service (ADR-015). The gRPC `DocumentServiceImpl` wrapper is
    /// no longer in the port chain.
    pub document_port: Arc<dyn proximadb_runtime::DocumentPort>,
    /// Port-typed view of the observability subsystem (Phase 9.10 / Task #76).
    ///
    /// **Suboptimal**: the port impl currently lives on the gRPC wrapper
    /// `ObservabilityServiceImpl`, not on the bare `ObservabilityService`
    /// (where ADR-015 says it should live). To unblock consumer migration
    /// now, this field constructs the wrapper and stores it as the port.
    /// The ADR-015 cleanup (move `impl ObservabilityPort` to the bare
    /// service, then update this field to coerce from `Arc<ObservabilityService>`
    /// directly) is a follow-up session — ~225 lines of port-impl bodies
    /// to lift from the 15 tonic methods.
    pub observability_port: Arc<dyn proximadb_runtime::ObservabilityPort>,
    /// Port-typed view of the graph subsystem (Phase 9.10 / Task #76).
    ///
    /// **Suboptimal**: same wrapper-as-port-host pattern as
    /// `observability_port`. ADR-015 cleanup is a follow-up session.
    pub graph_port: Arc<dyn proximadb_runtime::GraphPort>,

    /// Shared in-process full-text index map for hybrid retrieval
    /// (BM25 side of `/api/v1/hybrid/search`). REST and gRPC entry
    /// points read from this single map so an indexed document is
    /// immediately searchable on both protocols.
    ///
    /// Added 2026-05-26 (T3.2 Slice 1 of pre-release plan). Prior
    /// behavior had REST construct its own map locally in `AppState`
    /// and gRPC `HybridSearchServiceImpl` return mocks; this field
    /// gives both paths a shared backing.
    pub fulltext_indexes: crate::network::hybrid_search::HybridFullTextIndexMap,

    /// Process-wide cache of per-collection Vector Object Economy
    /// directories. Search paths fetch a [`CachedDirectoryHandle`] via
    /// `directory_cache.handle_for(collection_id)` and then `get_or_load`
    /// against a loader closure (typically wrapping
    /// [`load_directory_for`](crate::storage::engines::sst::object_economy_directory::load_directory_for)).
    /// First reader per collection pays the cost of loading the sidecar;
    /// subsequent readers reuse the cached `Arc<CachedDirectoryEntry>`.
    ///
    /// Writer/compactor will call
    /// [`VectorObjectEconomyDirectoryCache::invalidate`](crate::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache::invalidate)
    /// after `upsert_and_persist` lands a new directory version so the
    /// next reader picks up the change. That wiring is the next step
    /// after this slot exists — see the VECTOR_OBJECT_ECONOMY_ROUTE
    /// design doc.
    pub directory_cache: Arc<
        crate::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache,
    >,

    /// Process-wide per-collection pinning registry (Phase 6 control
    /// surface). Operators PATCH `/api/v1/collections/:id/pin` to
    /// record an explicit tier override; the `AxisTieringManager`
    /// consumer reads this registry during its evaluation loop and
    /// overrides its access-pattern policy when an operator pin is
    /// present. See `src/storage/collection_pinning.rs` for the
    /// control-plane / data-plane separation.
    pub pin_registry: Arc<crate::storage::collection_pinning::CollectionPinRegistry>,

    /// Process-wide cache-affinity registry (Phase 7.2). Tracks
    /// per-collection "which node most recently served queries" so
    /// that reads can be biased to whichever node owns the warm
    /// cache. Mirrors turbopuffer's published cache-affinity model
    /// ("subsequent queries route to the same query node for cache
    /// locality").
    ///
    /// The registry is process-wide and useful even in single-node
    /// deploys — it gives the operator API a place to inspect "which
    /// collections this node has been serving" and gives a future
    /// multi-node `RoutingService` an attach point via
    /// `with_affinity_registry`. The recording call lives in the
    /// data-plane search path so the registry reflects actual
    /// activity, not just routing decisions.
    pub affinity_registry: Arc<crate::cluster::cache_affinity::CacheAffinityRegistry>,

    /// Shared canonical WAL appender at `<data_dir>/pgwire/canonical-records.wal`.
    ///
    /// Opened once in `SharedServices::new` (when `opt_config` is provided so
    /// `cfg.server.data_dir` is known) and held as a single instance so both
    /// graph checkpoint emission (`GraphOperationsService::flush_wal`, TD-066)
    /// and pgwire direct record writes (`multi_server.rs` pgwire setup) share
    /// the same `next_sequence` counter and append lock. Without this
    /// sharing, two `FramedTableWalAppender::open` calls on the same file
    /// would each maintain independent next-sequence state, risking
    /// duplicate sequence numbers and silent recovery corruption.
    ///
    /// When `opt_config` is `None` (some test paths), this is `None` and
    /// both consumers fall back to their respective opt-out behavior
    /// (graph: tracing-only; pgwire: opens its own appender locally).
    pub canonical_wal_appender: Option<Arc<crate::services::FramedTableWalAppender>>,

    /// Process-wide recall-probe gate (TD-064 / LLD §5). The gate enables
    /// the quantized candidate route only after the recall-probe set passes
    /// the tenant's target for three consecutive builds; a single failure
    /// resets the streak. Held here so REST/gRPC handlers can read
    /// per-collection state via `is_open(ProbeScope)` without re-constructing
    /// the state machine — and so the planned Phase 5 stats refresher has a
    /// single registry to persist from. The gate was test-only prior to
    /// being slotted here; this is the first production wiring.
    pub recall_probe_gate: Arc<crate::catalog::RecallProbeGate>,

    /// Process-wide rank-pipeline singleton (R-7c.3 production wiring).
    ///
    /// REST, gRPC, and Arrow Flight all pull the same `Arc<RankServices>`
    /// from here via `AppState::with_rank_services` / equivalent, so SQL
    /// `RERANK(...)`, the REST `/api/v1/rank/search` route, and the
    /// `rank_features_export` Arrow Flight action share the same profile
    /// registry, candidate provider, scorer registry, and metric handles.
    /// Built around `ProductionHybridBackend` so retrieval lights up
    /// automatically as soon as ingestion populates per-collection BM25 +
    /// vector state.
    pub rank_services: Arc<crate::network::rest::v1::rank::RankServices>,

    /// Durable rank-profile catalog backed by the canonical WAL spine.
    ///
    /// `RankServices` recovers profiles from this store at boot and
    /// `RankProfileStore::install` is the lowering target for `CREATE RANK
    /// PROFILE` DDL + the REST install endpoint. When `canonical_wal_appender`
    /// is `None` (some test paths), this store is backed by an in-memory
    /// appender that does not survive restart.
    pub rank_profile_store: Arc<dyn crate::services::RankProfileStore>,
}

impl SharedServices {
    /// Create shared services with full business logic configuration
    /// SharedServices owns all business logic and configuration decisions
    /// Returns (SharedServices, CollectionService) - the collection service is needed by StorageEngine
    pub async fn new(
        metrics_collector: Option<Arc<MetricsCollector>>,
        storage_config: &crate::core::config::StorageConfig,
        orchestrator: Option<Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>>,
        // Optional full runtime config for hybrid/graph overrides
        opt_config: Option<&crate::core::config::Config>,
    ) -> Result<(Self, Arc<CollectionService>)> {
        info!("🔧 SharedServices: Initializing business logic hub for ALL protocols");
        debug!(
            "🔧 SharedServices::new - Starting with storage_config: {:?}",
            storage_config
        );

        let catalog_manager = Arc::new(crate::catalog::CatalogManager::new());

        // SharedServices owns metadata configuration logic
        info!(
            "🔧 SharedServices: Metadata URL from config: {}",
            storage_config.metadata_url
        );
        info!(
            "📂 SharedServices: Configuring metadata backend from TOML: {}",
            storage_config.metadata_url
        );

        // Create metadata backend based on URL from config
        // Supports file://, s3://, gs://, adls://, rocksdb://
        // The MetadataBackendFactory handles all filesystem routing internally
        info!(
            "📁 SharedServices: Creating metadata backend from URL: {}",
            storage_config.metadata_url
        );

        let metadata_backend =
            Arc::from(MetadataBackendFactory::create_from_url(&storage_config.metadata_url).await?);
        debug!("✅ SharedServices: Metadata backend created successfully");

        if catalog_manager.list_catalogs().await.is_empty() {
            catalog_manager
                .create_native_catalog("default", &storage_config.metadata_url)
                .await
                .context("Failed to initialize default xCatalog backend")?;
        }

        let collection_service = Arc::new(
            CollectionService::new(metadata_backend, storage_config.clone())
                .await?
                .with_catalog_manager(catalog_manager.clone()),
        );
        debug!("✅ SharedServices: CollectionService created successfully");

        // Collection service will be injected into StorageEngine by ProximaDB::new
        info!("✅ SharedServices: Collection service created for injection into StorageEngine");

        // 🚀 Create VectorOperationsService directly for 40-60% performance improvement
        // Use WAL config from TOML configuration
        debug!("🔧 SharedServices::new - Converting WAL config from TOML...");
        let mut wal_config = Self::convert_toml_to_wal_config(&storage_config.wal_config);

        // Override data_directories with storage_locations if available
        // This ensures embedded mode and config-specified storage locations are honored
        if !storage_config.storage_locations.is_empty() {
            wal_config.multi_disk.data_directories = storage_config
                .storage_locations
                .iter()
                .map(|loc| {
                    // Ensure proper file:// URL format
                    let url = if loc.url.starts_with("file://") {
                        loc.url.clone()
                    } else if loc.url.starts_with("/") {
                        format!("file://{}", loc.url)
                    } else {
                        loc.url.clone()
                    };
                    debug!(
                        "🔧 SharedServices: WAL directory URL from storage_locations: {}",
                        url
                    );
                    url
                })
                .collect();
            info!(
                "📂 SharedServices: WAL data directories set from storage_locations: {:?}",
                wal_config.multi_disk.data_directories
            );
        }
        debug!("✅ SharedServices::new - WAL config converted successfully from TOML");

        // Create filesystem factory for engines
        debug!("🔧 SharedServices::new - Creating filesystem factory for engines...");
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await?,
        );
        debug!("✅ SharedServices::new - Filesystem factory for engines created successfully");

        // Create VIPER engine
        debug!("🔧 SharedServices::new - Creating VIPER engine...");
        let viper_config = crate::core::config::ViperConfig::default();
        debug!("🔧 SharedServices::new - VIPER config created, now creating engine...");
        let _viper_engine = Arc::new(
            crate::storage::engines::viper::ViperEngine::from_core_config(
                viper_config,
                filesystem_factory.clone(),
            )
            .await?,
        );
        debug!("✅ SharedServices::new - VIPER engine created successfully");

        // Vector Object Economy Phase 4 (1-B + 2-B): construct the
        // process-wide per-collection directory cache up front so both
        // the SST engine (producer side — emits directory updates after
        // atomic commit) and the vector operations service (consumer
        // side — loads cached directories during search) hold the same
        // `Arc`.
        let directory_cache = Arc::new(
            crate::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache::new(),
        );

        // Phase 6: per-collection pinning registry constructed up
        // front so REST handlers (control plane) and the SST
        // tier-migration integration (data plane) share the same
        // `Arc`. Operator PATCH calls land in the registry; the
        // tier-migration integration consults it during flush and
        // evaluate cycles, overriding policy when a pin is set.
        //
        // Slice 6.5: when `opt_config` is provided, the registry
        // auto-persists to `<data_dir>/pinning/registry.json` so
        // pins survive process restarts. Tests / embedded paths
        // without opt_config use the in-memory constructor.
        let pin_registry = match opt_config {
            Some(cfg) => {
                let registry_path = cfg.server.data_dir.join("pinning").join("registry.json");
                info!(
                    "📌 SharedServices: pin registry persistence enabled at {}",
                    registry_path.display()
                );
                crate::storage::collection_pinning::new_shared_at(registry_path)
            }
            None => crate::storage::collection_pinning::new_shared(),
        };

        // Phase 7.2: per-collection cache-affinity registry. In-memory
        // only (no persistence) — entries naturally re-populate from
        // the first query after a restart, so a stale persisted entry
        // would be more confusing than helpful. TTL defaults to 60s;
        // entries older than that are treated as cold.
        let affinity_registry = crate::cluster::cache_affinity::new_shared();
        info!("🧭 SharedServices: cache-affinity registry ready (TTL 60s)");

        // Create WAL manager for two-stage search FIRST so the SST
        // engine can read its global manifest singleton when wiring the
        // Phase 5 freshness LSN source.
        debug!("🔧 SharedServices::new - Creating WAL manager for two-stage search...");
        let wal_manager = {
            use crate::storage::persistence::write_ahead_log::{
                WALBatchFactory, WriteAheadLogManager,
            };

            // Create WAL batch strategy
            let strategy_type = crate::storage::persistence::write_ahead_log::config::WriteBufferStrategyType::BincodeBatch;
            let strategy = WALBatchFactory::create_batch_serialization_strategy(
                strategy_type,
                &wal_config,
                filesystem_factory.clone(),
            )
            .await?;

            // Create WAL manager directly
            Arc::new(WriteAheadLogManager::new(strategy, wal_config.clone()).await?)
        };
        debug!("✅ SharedServices::new - WAL manager created successfully");

        // Phase 5 (Slice 5.2): try to resolve the global manifest
        // singleton and wrap it as a `FreshnessLsnSource`. When the
        // singleton hasn't been initialised yet (some embedded/test
        // paths), the engine falls back to emitting `freshness_lsn = 0`
        // — strong-route readers will simply always re-scan the WAL
        // delta, which is correct but more expensive.
        let freshness_lsn_source: Option<
            Arc<dyn crate::storage::engines::sst::object_economy_directory::FreshnessLsnSource>,
        > = crate::storage::persistence::write_ahead_log::manifest::get_service().map(|svc| {
            Arc::new(
                crate::storage::persistence::write_ahead_log::manifest::WalCursorLsnSource::new(
                    svc,
                ),
            )
                as Arc<
                    dyn crate::storage::engines::sst::object_economy_directory::FreshnessLsnSource,
                >
        });

        // Create SST engine
        debug!("🔧 SharedServices::new - Creating SST engine...");
        let sst_engine = {
            let mut engine = crate::storage::engines::sst::SstEngine::new()
                .await?
                .with_directory_cache(directory_cache.clone());
            if let Some(src) = freshness_lsn_source.clone() {
                engine = engine.with_freshness_lsn_source(src);
            }

            // Attach tier-migration integration when configured. Reads
            // the `[storage.sst_config.tiering]` block; defaults to
            // disabled. When `enabled = true`, the engine's
            // flush / search / compaction hooks start emitting access
            // events, flush-tier decisions, and migration evaluations
            // (see `src/storage/engines/sst/{search,flush}/coordinator.rs`).
            //
            // The integration's background evaluation loop is started
            // here so the policy engine can autonomously evaluate
            // pending migrations on its configured cadence.
            if let Some(tiering_cfg) = storage_config
                .sst_config
                .as_ref()
                .and_then(|sc| sc.tiering.clone())
            {
                if tiering_cfg.enabled {
                    use crate::storage::engines::sst::tiering_integration::SstTieringIntegration;
                    use crate::storage::tiering::TierMigrationExecutor;

                    // Build the migration executor first — it shares the
                    // filesystem factory with the engine so file://↔s3://
                    // moves use the same backend pool. Per-tier paths are
                    // pulled directly from the tiering config block.
                    let executor = Arc::new(TierMigrationExecutor::from_tiering_config(
                        filesystem_factory.clone(),
                        &tiering_cfg,
                    ));

                    match SstTieringIntegration::new(tiering_cfg) {
                        Ok(integration) => {
                            // Attach the executor BEFORE start() so the
                            // background eval loop, when it wakes, has
                            // somewhere to dispatch migration tasks.
                            // Attach the pin registry so flush-tier and
                            // evaluation honor operator pins (Phase 6
                            // data plane).
                            let mut integration = integration
                                .with_executor(executor)
                                .with_pin_registry(pin_registry.clone());
                            if let Err(e) = integration.start().await {
                                warn!(
                                    "⚠️ SharedServices: tier-migration integration failed to start ({}); continuing without tiering",
                                    e
                                );
                            } else {
                                info!(
                                    "🪜 SharedServices: SST tier-migration integration started — flush/search/compaction hooks active, executor dispatching tasks"
                                );
                                engine = engine.with_tiering_integration(Arc::new(integration));
                            }
                        }
                        Err(e) => warn!(
                            "⚠️ SharedServices: tier-migration integration could not be constructed ({}); continuing without tiering",
                            e
                        ),
                    }
                } else {
                    debug!(
                        "🪜 SharedServices: SST tier-migration configured but disabled (enabled=false); hooks remain no-ops"
                    );
                }
            }

            Arc::new(engine)
        };
        debug!("✅ SharedServices::new - SST engine created successfully");

        // Clone SST engine reference for DocumentService (used later for DocumentStrategy)
        let sst_engine_for_documents: Arc<dyn crate::storage::traits::UnifiedStorageEngine> =
            sst_engine.clone();

        // Create AxisManager for index operations
        debug!("🔧 SharedServices::new - Creating AxisManager for index operations...");
        let axis_manager =
            Arc::new(crate::index::AxisManager::new(crate::index::AxisConfig::default()).await?);
        debug!("✅ SharedServices::new - AxisManager created successfully");

        // Make AXIS manager available to graph-first entity store by default
        crate::storage::entity_store::orion_backend::set_global_axis_manager(axis_manager.clone());

        // Make AXIS manager available to SST engine for HNSW/IVF search
        crate::storage::engines::sst::core::set_sst_axis_manager(axis_manager.clone());
        debug!(
            "✅ SharedServices::new - AXIS manager registered with SST engine for HNSW/IVF search"
        );

        // Create VectorOperationsService with optimized architecture and two-stage search
        debug!(
            "🔧 SharedServices::new - About to create VectorOperationsService with two-stage search..."
        );
        // Use the passed orchestrator if available, otherwise create a default one
        use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
        let orchestrator = if let Some(orch) = orchestrator {
            orch
        } else {
            let mut default_orchestrator =
                CrossCacheOrchestrator::new((storage_config.cache_size_mb * 1024 * 1024) as usize);
            default_orchestrator.start_eviction_service(None);
            let orch = Arc::new(default_orchestrator);
            orch.clone().start_rebalancing_service();
            orch
        };
        // Always register globally — idempotent via OnceLock
        CrossCacheOrchestrator::register_global(orchestrator.clone());

        // =========================================================================
        // Initialize EventLog service and start AXIS consumer for async index building
        // This enables automatic AXIS index updates when data is flushed to storage
        // =========================================================================
        debug!("🔧 SharedServices::new - Initializing EventLog service for AXIS indexing...");

        // Use the global collection cache (shared across services)
        // Collections are registered in this cache when created via register_collection_in_cache()
        let collection_cache =
            crate::services::events::log::get_or_create_global_collection_cache();

        // Get base storage URL for EventLog persistence
        let base_storage_url = storage_config
            .storage_locations
            .first()
            .map(|loc| loc.url.clone());

        // Initialize the global EventLog service
        if let Err(e) = crate::services::events::log::initialize_event_log_service(
            collection_cache.clone(),
            filesystem_factory.clone(),
            base_storage_url.clone(),
        )
        .await
        {
            warn!(
                "⚠️ SharedServices: Failed to initialize EventLog service: {}. AXIS indexing will be disabled.",
                e
            );
        } else {
            info!("✅ SharedServices: EventLog service initialized successfully");

            // Start the AXIS EventLog consumer as a background task
            // This polls the EventLog and builds AXIS indexes when flush events occur
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

            // Store shutdown sender for graceful shutdown (could be stored in SharedServices if needed)
            // For now, the consumer will run until the process exits
            std::mem::forget(shutdown_tx); // Prevent sender from being dropped

            if let Some(event_log_service) = crate::services::events::log::event_log_service() {
                let _consumer_handle =
                    crate::index::axis::integration::eventlog_consumer::start_axis_consumer(
                        event_log_service.inner(),
                        axis_manager.clone(),
                        filesystem_factory.clone(),
                        collection_cache.clone(),
                        orchestrator.clone(),
                        shutdown_rx,
                    )
                    .await;

                info!(
                    "✅ SharedServices: AXIS EventLog consumer started - background index processing is available for collections that explicitly configure indexes"
                );
            } else {
                warn!(
                    "⚠️ SharedServices: EventLog service unavailable after initialization; AXIS consumer not started."
                );
            }
        }

        // `directory_cache` constructed earlier (before SstEngine) so the
        // engine, the vector ops service, and the SharedServices public
        // field all share the same `Arc`.
        let vector_operations_service = Arc::new(
            VectorOperationsService::new(
                sst_engine,
                wal_manager,
                axis_manager.clone(),
                collection_service.clone() as Arc<dyn proximadb_runtime::CollectionPort>,
            )
            .with_orchestrator(Some(orchestrator.clone()))
            .with_directory_cache(directory_cache.clone())
            // Phase 7.2: thread the same affinity registry held by
            // the SharedServices field so search-path recordings and
            // operator inspection share state.
            .with_affinity_registry(affinity_registry.clone()),
        );

        info!(
            "✅ SharedServices: VectorOperationsService created successfully - 40-60% performance boost enabled"
        );
        debug!("🔧 SharedServices::new - VectorOperationsService created successfully");

        info!(
            "🧠 SharedServices: Global Cross-Cache Orchestrator registered (budget={}MB)",
            storage_config.cache_size_mb
        );

        // Collection recovery will be handled by StorageEngine::start()
        // SharedServices no longer tries to recover before storage starts
        info!(
            "📋 SharedServices: Collection recovery will be handled by StorageEngine during startup"
        );

        // Placeholder for future assignment service recovery
        // Deferred: Add assignment service recovery after StorageEngine starts

        if false {
            // Disabled recovery code - will be moved to ProximaDB::new
            let recovered_collections = std::collections::HashMap::<
                String,
                crate::storage::metadata::VersionedCollectionMetadata,
            >::new();
            info!(
                "📦 SharedServices: Restoring {} collections to metadata backend",
                recovered_collections.len()
            );

            let collection_count = recovered_collections.len();
            for (collection_id, metadata) in recovered_collections {
                info!(
                    "📝 SharedServices: Restoring collection metadata for {}",
                    collection_id
                );

                // Convert storage metadata to proto collection format
                let collection_config = crate::proto::proximadb_v1::CollectionConfig {
                    name: metadata.name.clone(),
                    dimension: metadata.dimension as u32,
                    distance_metric: Some(
                        crate::proto::proximadb_v1::DistanceMetric::Cosine as i32,
                    ), // Default
                    storage_engine: Some(crate::proto::proximadb_v1::StorageEngine::Sst as i32), // Default: SST
                    filterable_columns: vec![],
                    index_configs: vec![],
                    quantization: Some(crate::proto::proximadb_v1::QuantizationConfig {
                        enabled: Some(true), // Quantization enabled by default
                        strategy: Some(
                            crate::proto::proximadb_v1::quantization_config::Strategy::SmartDefaults
                                as i32,
                        ),
                        custom_levels: vec![],
                        enable_progressive_search: Some(true), // Progressive search enabled by default
                        binary_filter_selectivity: Some(0.3),
                        int8_ranking_selectivity: Some(0.1),
                        pq_ranking_selectivity: Some(0.05),
                        training_sample_size: Some(10000),
                        quality_threshold: Some(0.95),
                        enable_adaptive_training: Some(true),
                        optimize_for_storage: Some(false),
                        optimize_for_memory: Some(false),
                        enable_simd_acceleration: Some(true),
                        // NEW: Direct quantization type enables
                        enable_binary: Some(true),
                        enable_int8: Some(true),
                        enable_pq: Some(true),
                        // Product Quantization specific settings
                        pq_segments: Some(8),
                        pq_bits: Some(8),
                        pq_codebooks: Some(0),
                        // Thresholds for progressive search
                        binary_threshold: Some(0.3),
                        int8_threshold: Some(0.1),
                        pq_threshold: Some(0.05),
                    }),
                    storage_config: None, // VersionedCollectionMetadata doesn't have storage_assignment field
                    primary_index: Some(String::new()),
                    auto_index_selection: Some(false),
                    description: None,
                    tags: vec![],
                    owner: None,
                    embedding_models: vec![], // No embedding models for imported collections
                    // ProximaRecord schema configuration (NEW)
                    record_schema: None,
                    enable_proxima_record: None,
                    text_columns: vec![],
                    text_storage_configs: vec![],
                    enable_dual_use_embeddings: None,
                    canonical_embedding_precision: None,
                };

                let proto_collection = crate::proto::proximadb_v1::Collection {
                    id: format!("recovered-{}", Uuid::new_v4()),
                    config: Some(collection_config),
                    stats: Some(crate::proto::proximadb_v1::CollectionStats {
                        vector_count: metadata.vector_count as i64,
                        index_size_bytes: metadata.total_size_bytes as i64,
                        data_size_bytes: metadata.total_size_bytes as i64,
                    }),
                    created_at: metadata.timestamp as i64,
                    updated_at: metadata.timestamp as i64, // VersionedCollectionMetadata doesn't have updated_at field
                    storage_assignment: None, // VersionedCollectionMetadata doesn't have storage_assignment field
                };

                // Store the recovered collection in the metadata backend
                match collection_service
                    .metadata_backend()
                    .upsert_collection_proto(&proto_collection)
                    .await
                {
                    Ok(_) => {
                        info!(
                            "✅ SharedServices: Successfully restored collection metadata for {}",
                            collection_id
                        );
                    }
                    Err(e) => {
                        warn!(
                            "⚠️ SharedServices: Failed to restore collection metadata for {}: {}",
                            collection_id, e
                        );
                    }
                }
            }

            info!(
                "✅ SharedServices: Metadata recovery completed - {} collections restored",
                collection_count
            );
        } else {
            info!("📋 SharedServices: No collections found in WAL to restore");
        }

        // ==================================================================================
        // CRITICAL FIX FOR GRAPH API BUG - Ensure Single Shared GraphCollectionService
        // ==================================================================================
        //
        // ROOT CAUSE ANALYSIS:
        //
        // The previous implementation had TWO SEPARATE GraphCollectionService instances:
        // 1. One created by UnifiedHandlers::new() for REST/gRPC graph collection endpoints
        // 2. One created by GraphOperationsService::new() for node/edge operations
        //
        // This caused graph collections created via REST API to be INVISIBLE to graph
        // operations because they were stored in different instances.
        //
        // SOLUTION:
        //
        // Create a SINGLE GraphCollectionService instance here and pass it to BOTH:
        // - GraphOperationsService (via new_with_collection_service)
        // - UnifiedHandlers and query orchestration layers (via extracted graph contracts)
        //
        // This ensures ALL graph endpoints and operations share the same state.
        // ==================================================================================

        debug!(
            "🔧 SharedServices::new - Creating SHARED GraphCollectionService instance with auto-recovery..."
        );
        let graph_collection_service =
            match crate::services::GraphCollectionService::new_with_recovery().await {
                Ok(svc) => Arc::new(svc),
                Err(e) => {
                    warn!(
                        "Failed to create GraphCollectionService with recovery: {}. Using non-persistent service.",
                        e
                    );
                    Arc::new(crate::services::GraphCollectionService::new())
                }
            };
        debug!(
            "✅ SharedServices::new - Shared GraphCollectionService created (with auto-recovery)"
        );

        // T2.3 / TD-066 production wiring: open the canonical WAL appender
        // ONCE here so it can be shared between the graph checkpoint emission
        // path (`GraphOperationsService::flush_wal`) and the pgwire direct
        // record write path (constructed in `multi_server.rs`). Sharing the
        // same `FramedTableWalAppender` instance is required for correctness:
        // two independent `open()` calls on the same WAL file would each
        // initialize their own `next_sequence: AtomicU64`, leading to
        // duplicate sequence numbers in the persisted log and silent
        // recovery corruption.
        //
        // When `opt_config` is `None` (test paths that don't supply a full
        // Config), skip the appender entirely — graph falls back to its
        // tracing-only behavior and pgwire (if enabled) opens its own
        // appender locally as it did before.
        let canonical_wal_appender: Option<Arc<crate::services::FramedTableWalAppender>> =
            if let Some(cfg) = opt_config {
                let wal_path = cfg
                    .server
                    .data_dir
                    .join("pgwire")
                    .join("canonical-records.wal");
                match crate::services::FramedTableWalAppender::open(&wal_path).await {
                    Ok(appender) => {
                        info!(
                            "✅ SharedServices: canonical WAL appender opened at {} (shared by graph checkpoint emission + pgwire direct writes)",
                            wal_path.display()
                        );
                        Some(Arc::new(appender))
                    }
                    Err(e) => {
                        warn!(
                            "SharedServices: failed to open canonical WAL at {}: {}. Graph flush_wal will fall back to tracing-only and pgwire (if enabled) will open its own appender.",
                            wal_path.display(),
                            e
                        );
                        None
                    }
                }
            } else {
                debug!(
                    "SharedServices: opt_config is None; skipping canonical WAL appender setup (test path?)"
                );
                None
            };

        // Create GraphOperationsService for native graph database operations
        // IMPORTANT: Pass the shared GraphCollectionService instance
        debug!(
            "🔧 SharedServices::new - Creating GraphOperationsService with SHARED collection service..."
        );
        // ALWAYS use new_with_collection_service to ensure shared GraphCollectionService
        // Even if config is provided, we must share the collection service
        // (Config-specific settings can be applied later if needed)
        let mut graph_service_inst =
            crate::graph::GraphOperationsService::new_with_collection_service(
                graph_collection_service.clone(),
            );
        // T2.3 / TD-066: inject the shared appender so flush_wal persists
        // canonical checkpoints to disk. Also inject the WAL path so the
        // graph engine factory can plumb it into ORION's persistence
        // layer for the read-side recovery hook (TD-066 (c) Part 1).
        if let Some(appender) = canonical_wal_appender.as_ref() {
            graph_service_inst =
                graph_service_inst
                    .with_canonical_wal_appender(appender.clone()
                        as Arc<dyn crate::services::record_store::TableWalAppender>);
            graph_service_inst =
                graph_service_inst.with_canonical_wal_path(appender.path().to_path_buf());
        }
        // Wire the storage root so graph engines persist under the same base path as vectors
        if let Some(first_loc) = storage_config.storage_locations.first() {
            graph_service_inst.set_base_storage_url(first_loc.url.clone());
        } else {
            graph_service_inst.set_base_storage_url(storage_config.metadata_url.clone());
        }

        // Create a simple file-backed metrics updater under data_root/metrics
        let filesystem_factory =
            Arc::new(FilesystemFactory::create(FilesystemConfig::default()).await?);
        let metrics_config = MetricsConfig {
            enabled: true,
            collection_partitions: 16,
            storage_path: format!(
                "file://{}/metrics",
                &storage_config.metadata_url.replace("file://", "")
            ),
            flush_interval_seconds: 60,
            retention_days: 7,
            parallel_scan_threshold: 1000,
            sparsity_threshold: 0.5,
            quantization_size_threshold: 1024 * 1024, // 1MB
            max_memory_mb: 512,
            snapshot_interval_seconds: 300, // 5 minutes
        };
        let metrics_store = Arc::new(
            crate::metrics::store::MetricsPersistenceLayer::new(
                filesystem_factory.clone(),
                metrics_config,
            )
            .await?,
        );
        let metrics_updater: Arc<dyn crate::metrics::InternalMetricsUpdater + 'static> = Arc::new(
            crate::metrics::updater::MetricsUpdateService::new(metrics_store.clone()),
        );
        graph_service_inst.set_metrics_updater(metrics_updater.clone());
        debug!("📈 GraphOperationsService metrics updater wired");
        let graph_service = Arc::new(graph_service_inst);
        debug!(
            "✅ SharedServices::new - GraphOperationsService created with shared collection service"
        );

        // Create DocumentService (moved up for UnifiedHandlers)
        debug!("🔧 SharedServices::new - Creating DocumentService for document queries...");
        let document_base_path = storage_config.metadata_url.replace("file://", "");
        let document_service = match DocumentService::new_with_wal(
            sst_engine_for_documents,
            &document_base_path,
        )
        .await
        {
            Ok(service) => Arc::new(service),
            Err(e) => {
                warn!(
                    "Failed to create WAL-backed DocumentService: {}. Falling back to in-memory WAL-less service.",
                    e
                );
                Arc::new(DocumentService::new(
                    vector_operations_service.unified_engine(),
                ))
            }
        };

        // Create ObservabilityService (moved up for UnifiedHandlers)
        debug!(
            "🔧 SharedServices::new - Creating ObservabilityQueryEngine for observability queries..."
        );
        let observability_base_path = storage_config.metadata_url.replace("file://", "");
        let observability_storage = match ObservabilityStorage::new_with_wal(
            &observability_base_path,
        )
        .await
        {
            Ok(storage) => Arc::new(storage),
            Err(e) => {
                warn!(
                    "Failed to create WAL-backed ObservabilityStorage: {}. Falling back to non-WAL storage.",
                    e
                );
                Arc::new(ObservabilityStorage::new(&observability_base_path))
            }
        };
        let observability_service = Arc::new(
            crate::observability::ObservabilityService::new(observability_storage.clone()).await?,
        );
        let observability_query_engine =
            Arc::new(ObservabilityQueryEngine::new(observability_storage.clone()));

        // Create EventLogEngine for persistent audit trails (TD-050 Phase 5)
        debug!("🔧 SharedServices::new - Creating EventLogEngine for audit trails...");
        let event_log_base_path = storage_config.metadata_url.replace("file://", "") + "/auditlog";
        let event_log_config = crate::storage::engines::eventlog::EventLogConfig {
            base_dir: std::path::PathBuf::from(event_log_base_path),
            ..Default::default()
        };
        let event_log_filesystem = Arc::new(
            crate::storage::persistence::filesystem::caching_filesystem::UnifiedCachingFilesystem::new(
                filesystem_factory.get_filesystem(&storage_config.metadata_url)?,
                "auditlog".to_string(),
                "eventlog".to_string(),
            ),
        );
        let event_log = match crate::storage::engines::eventlog::EventLogEngine::new(
            event_log_config,
            event_log_filesystem,
        ) {
            Ok(engine) => Some(Arc::new(engine)),
            Err(e) => {
                warn!("Failed to create EventLogEngine for audit trails: {}", e);
                None
            }
        };

        // Derive extracted graph capability views once here so query/orchestration
        // layers depend on explicit contracts rather than the full concrete service.
        let graph_query_service = graph_service.clone();
        let graph_execution_service = graph_service.clone();

        // Create unified handlers with SHARED graph services
        // IMPORTANT: Pass the pre-created GraphCollectionService and graph execution service
        // to ensure ALL graph endpoints and operations share the same state
        debug!("🔧 SharedServices::new - Creating UnifiedHandlers with SHARED graph services...");
        let request_handlers_instance = UnifiedHandlers::new(
            collection_service.clone(),
            vector_operations_service.clone(),
            document_service.clone(),
            observability_service.clone(),
            event_log,
            graph_collection_service.clone(), // SHARED instance
            graph_service.clone(),            // Concrete native graph operations service
        );

        // Apply hybrid runtime config if provided
        if let Some(cfg) = opt_config
            && let Some(ref hybrid) = cfg.hybrid
        {
            request_handlers_instance.set_hybrid_runtime(hybrid.clone());
        }
        let request_handlers = Arc::new(request_handlers_instance);
        debug!("✅ SharedServices::new - UnifiedHandlers created with shared graph services");

        // ==================================================================================
        // Create UnifiedQueryFacade - single entry point for all query types
        // This consolidates the 5 parallel query paths into a single unified interface
        // ==================================================================================
        debug!("🔧 SharedServices::new - Creating UnifiedQueryFacade with real strategies...");

        // Create VectorSearchStrategy wrapping VectorOperationsService.
        // Task #76 consumer migration: VectorSearchStrategy now takes
        // Arc<dyn CollectionPort> instead of Arc<CollectionService>.
        // Coerce the existing concrete service to the port trait object
        // (the same SharedServices collection_port field uses the same
        // coercion at the field-init site below).
        let vector_strategy: Arc<dyn crate::query::facade::QueryStrategy> =
            Arc::new(VectorSearchStrategy::new(
                vector_operations_service.clone(),
                collection_service.clone() as Arc<dyn proximadb_runtime::CollectionPort>,
            ));

        // Create GraphStrategy wrapping the extracted graph query contract
        let graph_strategy: Arc<dyn crate::query::facade::QueryStrategy> =
            Arc::new(GraphStrategy::new(graph_query_service.clone()));

        // Create DocumentStrategy wrapping DocumentService for JSON document queries
        // DocumentService provides MongoDB-like document operations (CRUD, indexing, queries)
        let document_strategy: Arc<dyn crate::query::facade::QueryStrategy> =
            Arc::new(DocumentStrategy::new(document_service.clone()));
        debug!("✅ SharedServices::new - DocumentStrategy created for document queries");

        // Create ObservabilityStrategy wrapping ObservabilityQueryEngine for logs/metrics/traces
        // This enables unified query interface for observability data
        const QUERY_TELEMETRY_NAMESPACE: &str = "_proximadb_query";
        let telemetry_namespace_exists = observability_service
            .list_namespaces()
            .await
            .into_iter()
            .any(|namespace| namespace.name == QUERY_TELEMETRY_NAMESPACE);
        if !telemetry_namespace_exists {
            let telemetry_config = crate::proto::proximadb_v1::ObservabilityNamespaceConfig {
                name: QUERY_TELEMETRY_NAMESPACE.to_string(),
                retention: Some(crate::proto::proximadb_v1::RetentionConfig {
                    hot_retention_hours: 24,
                    warm_retention_days: 7,
                    cold_retention_days: 30,
                    archive_retention_days: 90,
                }),
                ingestion: None,
                alert_rules: Vec::new(),
                access: None,
            };
            if let Err(error) = observability_service
                .create_namespace(telemetry_config)
                .await
            {
                warn!(
                    "Failed to create internal query telemetry namespace '{}': {}",
                    QUERY_TELEMETRY_NAMESPACE, error
                );
            }
        }
        crate::query::utils::metrics::configure_query_telemetry(
            observability_service.clone(),
            QUERY_TELEMETRY_NAMESPACE,
        );

        let observability_strategy: Arc<dyn crate::query::facade::QueryStrategy> =
            Arc::new(ObservabilityStrategy::new(observability_query_engine));
        debug!(
            "✅ SharedServices::new - ObservabilityStrategy created for logs/metrics/traces queries"
        );

        // Create MultiModelStorageFacade for federated queries and wire the live stores
        debug!(
            "🔧 SharedServices::new - Creating MultiModelStorageFacade for federated queries..."
        );
        let vector_store = Arc::new(
            crate::storage::multimodel::VectorStore::with_engine(
                vector_operations_service.unified_engine(),
            )
            .with_index_manager(axis_manager.clone()),
        );
        let graph_store = Arc::new(
            crate::storage::multimodel::GraphStore::new(Default::default())
                .with_service(graph_service.clone()),
        );
        let document_store = Arc::new(
            crate::storage::multimodel::DocumentStore::new(Default::default())
                .with_service(document_service.clone()),
        );
        let obs_base_path = storage_config.metadata_url.replace("file://", "");
        let observability_store = Arc::new(
            crate::storage::multimodel::ObservabilityStore::new(
                crate::storage::multimodel::stores::observability_store::ObservabilityStoreConfig {
                    base_path: obs_base_path,
                    ..Default::default()
                },
            )
            .with_service(observability_service.clone()),
        );
        let multimodal_storage = Arc::new(
            MultiModelStorageFacade::new()
                .with_vector_store(vector_store)
                .with_graph_store(graph_store)
                .with_document_store(document_store)
                .with_observability_store(observability_store),
        );
        debug!("✅ SharedServices::new - MultiModelStorageFacade created and wired");

        // T3.2 Slice 1: shared full-text index map for hybrid retrieval.
        // Hoisted ahead of the FederatedQueryContext so the rank-pipeline
        // singleton can use the same Arc the SharedServices field holds.
        let fulltext_indexes: crate::network::hybrid_search::HybridFullTextIndexMap =
            Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        // R-7c.3 production wiring: construct the durable rank-profile store,
        // the production hybrid backend, the rank metrics handle, and the
        // singleton `RankServices` that REST / gRPC / Arrow Flight share.
        let (rank_services, rank_profile_store) =
            build_rank_services(
                vector_operations_service.clone() as Arc<dyn proximadb_runtime::VectorOpsPort>,
                fulltext_indexes.clone(),
                canonical_wal_appender.clone(),
            )
            .await;
        info!(
            "✅ SharedServices: RankServices ready (profile_count={}, metrics=on)",
            rank_services.profile_registry.len()
        );

        // Create FederatedQueryContext for SQL with multi-model extensions
        debug!("🔧 SharedServices::new - Creating FederatedQueryContext...");
        let federated_context = Arc::new(
            FederatedQueryContext::new(multimodal_storage)
                .with_collection_port(
                    collection_service.clone() as Arc<dyn proximadb_runtime::CollectionPort>
                )
                .with_vector_operations(vector_operations_service.clone())
                .with_rank_services(rank_services.clone()),
        );
        debug!("✅ SharedServices::new - FederatedQueryContext created");

        // Create SqlStrategy wrapping FederatedQueryContext
        let sql_strategy: Arc<dyn crate::query::facade::QueryStrategy> =
            Arc::new(SqlStrategy::new(federated_context));

        // Create ColumnarStrategy for analytical queries (M2 Dual Columnar Execution)
        // This strategy handles SQL queries with aggregations, GROUP BY, DISTINCT
        // by routing them through Arrow/Parquet columnar providers
        let columnar_strategy: Arc<dyn crate::query::facade::QueryStrategy> =
            Arc::new(ColumnarStrategy::new());
        debug!("✅ SharedServices::new - ColumnarStrategy created for analytical queries");

        // Create DistributedQueryStrategy for cluster-aware federated execution.
        // This is only selected when the execution path is explicitly forced to "distributed".
        let distributed_strategy: Arc<dyn crate::query::facade::QueryStrategy> = Arc::new(
            DistributedQueryStrategy::new(
                "local-node".to_string(),
                DistributedStrategyConfig::default(),
            )
            .with_vector_ops(vector_operations_service.clone())
            .with_document_service(document_service.clone())
            .with_graph_service(graph_query_service.clone())
            .with_observability_service(observability_service.clone()),
        );
        debug!(
            "✅ SharedServices::new - DistributedQueryStrategy created for forced distributed execution"
        );

        // Build the unified facade with all strategies
        // Priority order: vector (100) > graph (75) > document (70) > observability (60) > columnar (50) > sql (25)
        // Distributed strategy is force-path only and will not be selected automatically.
        let strategies = vec![
            vector_strategy,
            graph_strategy,
            document_strategy,
            observability_strategy,
            columnar_strategy,
            distributed_strategy,
            sql_strategy,
        ];
        let query_facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));

        info!(
            "✅ SharedServices: UnifiedQueryFacade created with 7 strategies (vector, graph, document, observability, columnar, distributed, sql)"
        );

        // Wire QueryFacadeAdapter to UnifiedHandlers for unified SQL routing
        // This enables SQL queries to flow through the facade when unified-facade-routing feature is enabled
        let query_adapter = Arc::new(QueryFacadeAdapter::new(query_facade.clone()));
        request_handlers.set_query_adapter(query_adapter.clone());
        debug!("✅ SharedServices::new - QueryFacadeAdapter wired to UnifiedHandlers");

        // Wire DmlService to UnifiedHandlers for gRPC EXPLAIN routing.
        // EXPLAIN INSERT … SELECT queries arriving on the ExecuteSql RPC are detected in
        // execute_sql_v1 and dispatched here instead of the legacy SQL frontend.
        let dml_service_for_grpc = Arc::new(DmlService::new(
            catalog_manager.clone(),
            vector_operations_service.clone(),
        ));
        request_handlers.set_dml_service(dml_service_for_grpc);
        debug!("✅ SharedServices::new - DmlService wired to UnifiedHandlers for EXPLAIN routing");

        // Build a port-backed runtime handler for collection/vector REST routes.
        // Uses trait objects so API routes are decoupled from root-crate concrete services.
        let runtime_api_handlers: Arc<dyn proximadb_runtime::ApiHandlersPort> =
            Arc::new(proximadb_runtime::UnifiedHandlers::new(
                collection_service.clone() as Arc<dyn proximadb_runtime::CollectionPort>,
                vector_operations_service.clone() as Arc<dyn proximadb_runtime::VectorOpsPort>,
                Some(query_adapter.clone() as Arc<dyn proximadb_runtime::QueryAdapterPort>),
            ));
        debug!("✅ SharedServices::new - Port-backed runtime API handlers created");

        info!(
            "✅ SharedServices: Business logic hub ready for ALL protocols (gRPC, REST, WebSocket, etc.)"
        );

        Ok((
            Self {
                catalog_manager,
                segment_registry: Arc::new(crate::catalog::SegmentRegistry::new()),
                collection_service: collection_service.clone(),
                vector_operations_service: vector_operations_service.clone(),
                graph_service,
                graph_query_service,
                graph_execution_service,
                document_service: document_service.clone(),
                observability_service: observability_service.clone(),
                request_handlers: request_handlers.clone(),
                metrics_collector,
                metrics_updater: Some(metrics_updater.clone()),
                query_facade,
                api_handlers: runtime_api_handlers,
                // Task #72: ClusterPort wiring slot. Defaults to None for
                // single-node bootstrap; populate via builder when [distributed]
                // config is present and a ClusterManager has been constructed.
                cluster_port: None,
                // Task #76 collection slice: port-typed view of the same
                // CollectionService instance held by `collection_service`.
                // Consumers should prefer `collection_port` going forward.
                collection_port: collection_service.clone()
                    as Arc<dyn proximadb_runtime::CollectionPort>,
                // Task #76 vector-ops slice: port-typed view of the same
                // VectorOperationsService instance held by `vector_operations_service`.
                vector_ops_port: vector_operations_service.clone()
                    as Arc<dyn proximadb_runtime::VectorOpsPort>,
                // Task #76 document slice (ADR-015 step 4): port-typed view
                // of the same DocumentService instance held by
                // `document_service`. Powered by the bare-service DocumentPort
                // impl in src/storage/document/service.rs (ADR-015 step 1).
                document_port: document_service.clone() as Arc<dyn proximadb_runtime::DocumentPort>,
                // Task #76 observability slice — wrapper-as-port-host pattern
                // (suboptimal vs ADR-015; cleanup is a follow-up session).
                observability_port: Arc::new(crate::network::grpc::ObservabilityServiceImpl::new(
                    observability_service.clone(),
                ))
                    as Arc<dyn proximadb_runtime::ObservabilityPort>,
                // Task #76 graph slice — same wrapper-as-port-host pattern.
                // Uses with_adapter() so search/explain methods have the query
                // adapter wired (matches the production wiring at
                // src/network/multi_server.rs:415).
                graph_port: Arc::new(crate::network::grpc::GraphServiceImpl::with_adapter(
                    request_handlers.clone(),
                    query_adapter.clone(),
                )) as Arc<dyn proximadb_runtime::GraphPort>,
                // T3.2 Slice 1: shared full-text index map for hybrid
                // retrieval. Same in-process map serves REST, gRPC, and the
                // R-7c rank-pipeline BM25 leg (`ProductionHybridBackend`).
                fulltext_indexes,
                // Vector Object Economy Phase 4 (2-B): process-wide
                // per-collection directory cache. The same Arc is also
                // attached to `vector_operations_service` via
                // `with_directory_cache` above so the search service can
                // touch the cache without re-resolving SharedServices.
                directory_cache,
                // Phase 6: per-collection pinning registry.
                // Constructed up front (line ~322) so REST handlers
                // (control plane) and the SST tier-migration
                // integration (data plane) hold the same `Arc`.
                pin_registry,
                // Phase 7.2: cache-affinity registry. Populated by
                // the unified search path; consumed by operator
                // inspection and future cluster-mode RoutingService
                // attach.
                affinity_registry,
                // T2.3 / TD-066 production wiring: the shared canonical
                // WAL appender opened earlier (Some when opt_config is
                // provided). Held here so multi_server.rs can clone it
                // for pgwire direct writes — guaranteeing both consumers
                // share the same next_sequence counter.
                canonical_wal_appender,
                // TD-064 / LLD §5: per-collection recall-probe gate. Empty
                // at startup; populated as the stats refresher / search path
                // observe probe outcomes. Route-health surfaces per-scope
                // state for operator visibility.
                recall_probe_gate: Arc::new(crate::catalog::RecallProbeGate::new()),
                // R-7c.3 production wiring: shared rank-pipeline singleton +
                // durable rank-profile catalog. Both are built ahead of the
                // FederatedQueryContext so SQL RERANK shares the registry.
                rank_services,
                rank_profile_store,
            },
            collection_service,
        ))
    }

    /// Optional metrics updater for wiring into services. Currently returns None
    /// unless a metrics updater is injected in the future.
    pub fn metrics_updater(
        &self,
    ) -> Option<Arc<dyn crate::metrics::InternalMetricsUpdater + 'static>> {
        self.metrics_updater.clone()
    }

    /// Set the cluster orchestration port. Phase 9.12 / Task #72.
    ///
    /// Use this when bootstrap detects `[distributed]` config and has
    /// constructed a `ClusterManager` (or any other `ClusterPort` impl —
    /// see `crates/platform/proximadb-runtime/src/cluster_port.rs`).
    /// Single-node deployments leave it as `None`.
    pub fn with_cluster_port(mut self, port: Arc<dyn proximadb_runtime::ClusterPort>) -> Self {
        self.cluster_port = Some(port);
        self
    }

    /// Get the unified query facade - single entry point for all query types
    ///
    /// The facade consolidates vector search, SQL, and graph queries into a unified
    /// interface with automatic strategy selection and routing.
    pub fn query_facade(&self) -> Arc<UnifiedQueryFacade> {
        self.query_facade.clone()
    }

    /// Create a QueryFacadeAdapter for protocol handlers
    ///
    /// The adapter provides protocol-agnostic methods that convert proto types
    /// to/from QueryRequest/QueryResult, enabling query routing.
    pub fn query_adapter(&self) -> Arc<QueryFacadeAdapter> {
        Arc::new(QueryFacadeAdapter::new(self.query_facade.clone()))
    }

    /// Recover vectors from write buffer after StorageEngine has started
    /// This should be called from ProximaDB::new after storage.start()
    pub async fn recover_vectors_from_write_buffer(
        &self,
        storage: &Arc<RwLock<StorageEngine>>,
    ) -> Result<()> {
        info!("🔄 SharedServices: Starting vector recovery from write buffer");

        // Get collections that need vector recovery
        let storage_ref = storage.read().await;
        let recovered_collections = storage_ref.recovered_collections_metadata().await?;

        if recovered_collections.is_empty() {
            info!("📋 SharedServices: No collections found for vector recovery");
            return Ok(());
        }

        info!(
            "📦 SharedServices: Found {} collections for potential vector recovery",
            recovered_collections.len()
        );

        // Implement comprehensive vector recovery from WAL to VectorOperationsService
        let mut total_vectors_recovered = 0u64;

        for (collection_id, _collection) in &recovered_collections {
            // 1. Check if write buffer has unflushed data for this collection
            let unflushed_batches = match storage_ref
                .write_ahead_log_manager()
                .read_all_batches(collection_id, None)
                .await
            {
                Ok(batches) => batches,
                Err(e) => {
                    warn!(
                        "Failed to read unflushed batches for collection {}: {}",
                        collection_id, e
                    );
                    continue;
                }
            };

            if unflushed_batches.is_empty() {
                debug!(
                    "No unflushed vectors found for collection: {}",
                    collection_id
                );
                continue;
            }

            // 2. Load vectors from write buffer into VectorOperationsService memtable
            let mut collection_vectors_recovered = 0u64;

            for batch in unflushed_batches {
                let batch_size = batch.vector_records.len();

                // Insert each vector into the VectorOperationsService memtable
                for vector_record in batch.vector_records.iter() {
                    match self
                        .vector_operations_service
                        .insert_vectors_direct(collection_id, Arc::new(vec![vector_record.clone()]))
                        .await
                    {
                        Ok(_) => {
                            collection_vectors_recovered += 1;
                        }
                        Err(e) => {
                            warn!(
                                "Failed to recover vector {} for collection {}: {}",
                                &vector_record.oid, collection_id, e
                            );
                        }
                    }
                }

                debug!(
                    "Recovered batch {} with {} vectors for collection {}",
                    batch.batch_id.to_base62(),
                    batch_size,
                    collection_id
                );
            }

            total_vectors_recovered += collection_vectors_recovered;

            // 3. Mark recovery complete for this collection
            info!(
                "✅ Collection '{}': Recovered {} vectors from WAL to memtable",
                collection_id, collection_vectors_recovered
            );
        }

        info!(
            "✅ SharedServices: Vector recovery completed - {} vectors across {} collections",
            total_vectors_recovered,
            recovered_collections.len()
        );

        Ok(())
    }

    /// Convert TOML WALConfig to internal WALConfig
    fn convert_toml_to_wal_config(
        toml_config: &crate::core::config::WriteBufferUserConfig,
    ) -> crate::storage::persistence::write_ahead_log::config::WALConfig {
        use crate::storage::persistence::write_ahead_log::config::{
            MemTableConfig, MemTableType, PerformanceConfig, SyncMode, WALConfig,
        };

        // Create performance config with values from TOML
        info!(
            "📋 Converting WALConfig from TOML: memory_flush_size_bytes={} ({}MB), vector_count_threshold={}, write_buffer_size_mb={}MB",
            toml_config.memory_flush_size_bytes,
            toml_config.memory_flush_size_bytes / (1024 * 1024),
            toml_config.vector_count_threshold,
            toml_config.write_buffer_size_mb
        );

        let performance = PerformanceConfig {
            memory_flush_size_bytes: toml_config.memory_flush_size_bytes,
            global_flush_threshold: toml_config.write_buffer_size_mb as usize * 1024 * 1024,
            batch_threshold: toml_config.vector_count_threshold,
            sync_mode: match toml_config.sync_mode.to_lowercase().as_str() {
                "perbatch" => SyncMode::PerBatch,
                "periodic" => SyncMode::Periodic,
                "none" => SyncMode::Never,
                _ => SyncMode::PerBatch,
            },
            ..Default::default()
        };

        // Create memtable config
        let memtable = MemTableConfig {
            global_memory_limit: toml_config.write_buffer_size_mb as usize * 1024 * 1024,
            memtable_type: match toml_config.memtable_type.to_lowercase().as_str() {
                "btree" => MemTableType::BTree,
                "skiplist" => MemTableType::SkipList,
                _ => MemTableType::BTree,
            },
            ..Default::default()
        };

        // Create multi-disk config with WAL directory
        let multi_disk = crate::storage::persistence::write_ahead_log::config::MultiDiskConfig {
            data_directories: vec![toml_config.write_buffer_directory.clone()],
            distribution_strategy: crate::storage::persistence::write_ahead_log::config::DiskDistributionStrategy::RoundRobin,
            collection_affinity: true,
        };

        WALConfig {
            performance,
            memtable,
            multi_disk,
            enable_mvcc: true,                  // Enable MVCC for consistency
            enable_ttl: true,                   // Enable TTL support
            enable_background_compaction: true, // Enable background compaction
            enable_optimized_writer: toml_config.enable_wal, // Use enable_wal to control optimized writer
            global_manifest_url: toml_config.global_manifest_url.clone(),
            ..Default::default()
        }
    }
}

/// Build the process-wide `RankServices` singleton + the durable rank-profile
/// store that backs it. R-7c.3 production wiring.
///
/// When a canonical WAL appender is supplied the store is durable; otherwise
/// it falls back to an in-memory appender (sufficient for tests and the
/// embedded-mode boot path). Existing profiles in the canonical WAL are
/// replayed into the store and then compiled into the `ProfileRegistry` so
/// dashboards see them on a cold boot. Compile failures bump the
/// `proximadb_rank_profile_reload_total{outcome="error"}` counter but never
/// fail the boot — operators can repair the catalog entry without taking
/// the server down.
async fn build_rank_services(
    vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>,
    fulltext_indexes: crate::network::hybrid_search::HybridFullTextIndexMap,
    canonical_wal_appender: Option<Arc<crate::services::FramedTableWalAppender>>,
) -> (
    Arc<crate::network::rest::v1::rank::RankServices>,
    Arc<dyn crate::services::RankProfileStore>,
) {
    use crate::services::record_store::TableWalAppender;
    use crate::services::{FramedTableWalAppender, MemoryTableWalAppender};

    // Load existing profiles from the canonical WAL (when present) so the
    // store starts populated even before the registry is built.
    let (store_appender, recovered_entries): (Arc<dyn TableWalAppender>, _) =
        if let Some(appender) = canonical_wal_appender {
            let path = appender.path().to_path_buf();
            let entries = match FramedTableWalAppender::read_entries_from_path(&path).await {
                Ok(entries) => entries,
                Err(err) => {
                    warn!(
                        "SharedServices: failed to replay rank-profile WAL at {}: {} — starting with empty profile catalog",
                        path.display(),
                        err
                    );
                    Vec::new()
                }
            };
            (appender as Arc<dyn TableWalAppender>, entries)
        } else {
            (
                Arc::new(MemoryTableWalAppender::new()) as Arc<dyn TableWalAppender>,
                Vec::new(),
            )
        };

    build_rank_services_with_appender(
        vector_ops,
        fulltext_indexes,
        store_appender,
        &recovered_entries,
    )
    .await
}

/// Inner builder that takes a pre-resolved appender + recovered entries. Split
/// out so tests can drive it with an in-memory appender without a temp-dir
/// round-trip.
async fn build_rank_services_with_appender(
    vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>,
    fulltext_indexes: crate::network::hybrid_search::HybridFullTextIndexMap,
    store_appender: Arc<dyn crate::services::record_store::TableWalAppender>,
    recovered_entries: &[proximadb_storage_common::CanonicalWalEntry],
) -> (
    Arc<crate::network::rest::v1::rank::RankServices>,
    Arc<dyn crate::services::RankProfileStore>,
) {
    use crate::core::search::hybrid::FusionStrategy;
    use crate::network::rest::v1::rank::{HybridCoordinatorAdapter, RankServices};
    use crate::network::rest::v1::rank_backend::ProductionHybridBackend;
    use crate::observability::rank_metrics::init_rank_pipeline_metrics;
    use crate::services::CanonicalWalRankProfileStore;

    let store: Arc<dyn crate::services::RankProfileStore> = Arc::new(
        CanonicalWalRankProfileStore::from_wal_entries(store_appender, recovered_entries),
    );

    // Build the production hybrid backend over the shared vector port + the
    // shared per-collection BM25 index map. Both surfaces are already
    // SharedServices fields.
    let backend = Arc::new(ProductionHybridBackend::new(vector_ops, fulltext_indexes));
    let adapter = Arc::new(HybridCoordinatorAdapter::new(
        FusionStrategy::ReciprocalRank { k: 60 },
        backend,
    ));

    // Register the spec §4.10 metric family against the process-wide
    // rank-metrics registry. Idempotent on hot-reload paths.
    let metrics = init_rank_pipeline_metrics();
    let services = Arc::new(RankServices::new(adapter).with_metrics(metrics));

    // Recover compiled profiles from the durable store. Validation /
    // compilation failures are logged + recorded as failed reloads — they do
    // not fail the boot.
    let recovered_profiles = match store.list_all().await {
        Ok(profiles) => profiles,
        Err(err) => {
            warn!(
                "SharedServices: rank-profile recovery list_all failed: {} — starting with empty registry",
                err
            );
            Vec::new()
        }
    };
    for profile in recovered_profiles {
        match recover_profile(&services, &profile) {
            Ok(()) => debug!(
                "SharedServices: recovered rank profile '{}' (version={})",
                profile.name, profile.version
            ),
            Err(err) => {
                warn!(
                    "SharedServices: failed to recover rank profile '{}' (version={}): {}",
                    profile.name, profile.version, err
                );
                services.record_profile_reload_error(&profile.name);
            }
        }
    }

    (services, store)
}

fn recover_profile(
    services: &crate::network::rest::v1::rank::RankServices,
    profile: &crate::services::StoredRankProfile,
) -> Result<(), String> {
    use proximadb_rank_profile::{CompiledRankProfile, dsl::parse_single};

    let spec = parse_single(&profile.name, &profile.spec_toml).map_err(|e| e.to_string())?;
    let compiled = CompiledRankProfile::compile(spec, services.blueprint_factory.clone())
        .map_err(|e| e.to_string())?;
    services.install_profile(compiled);
    Ok(())
}

#[cfg(test)]
mod rank_services_wiring_tests {
    use super::*;
    use crate::services::record_store::TableWalAppender;
    use crate::services::{MemoryTableWalAppender, RankProfileStore};
    use async_trait::async_trait;
    use proximadb_runtime::VectorOpsPort;
    use serde_json::Value as JsonValue;
    use std::collections::HashMap;
    use std::sync::RwLock;

    // ── Minimal no-op vector port ────────────────────────────────────────────

    struct NoopVectorPort;

    #[async_trait]
    impl VectorOpsPort for NoopVectorPort {
        async fn search(
            &self,
            _request: crate::proto::proximadb_v1::VectorSearchRequest,
            _tenant_id: Option<&str>,
        ) -> anyhow::Result<crate::proto::proximadb_v1::VectorOperationResponse> {
            Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                success: true,
                operation: 0,
                metrics: None,
                results: Some(crate::proto::proximadb_v1::SearchResult {
                    results: Vec::new(),
                    total_found: 0,
                    collection_id: None,
                }),
                vector_ids: Vec::new(),
                error_message: None,
                error_code: None,
            })
        }

        async fn batch_upsert(
            &self,
            _request: crate::proto::proximadb_v1::VectorBatchRequest,
            _tenant_id: Option<&str>,
        ) -> anyhow::Result<crate::proto::proximadb_v1::VectorOperationResponse> {
            unimplemented!()
        }

        async fn get_vector(
            &self,
            _collection_id: &str,
            _vector_id: &str,
            _include_vector: bool,
            _include_metadata: bool,
            _tenant_id: Option<&str>,
        ) -> anyhow::Result<crate::proto::proximadb_v1::VectorOperationResponse> {
            unimplemented!()
        }

        async fn flush_all(&self) -> anyhow::Result<()> {
            Ok(())
        }

        async fn metrics(&self) -> anyhow::Result<JsonValue> {
            Ok(JsonValue::Null)
        }
    }

    fn empty_indexes() -> crate::network::hybrid_search::HybridFullTextIndexMap {
        Arc::new(RwLock::new(HashMap::new()))
    }

    fn valid_profile_toml() -> String {
        // Simplest possible profile: a constant first_phase expression
        // (`"1.0"`) that the default `BlueprintFactory` compiles without any
        // extra feature registrations.
        r#"
[first_phase]
expression = "1.0"
heap_size = 50
"#
        .to_string()
    }

    fn invalid_profile_toml() -> String {
        // Refers to a feature that the default `BlueprintFactory` knows
        // nothing about — compilation should fail.
        r#"
[first_phase]
expression = "definitely_not_a_feature(\"missing\")"
heap_size = 50
"#
        .to_string()
    }

    #[tokio::test]
    async fn empty_catalog_produces_empty_registry() {
        let appender: Arc<dyn TableWalAppender> = Arc::new(MemoryTableWalAppender::new());
        let (services, store) = build_rank_services_with_appender(
            Arc::new(NoopVectorPort),
            empty_indexes(),
            appender,
            &[],
        )
        .await;

        assert_eq!(services.profile_registry.len(), 0);
        assert_eq!(
            store.list_all().await.unwrap().len(),
            0,
            "store should also start empty"
        );
    }

    #[tokio::test]
    async fn one_valid_profile_recovers_into_registry() {
        // Step 1: install a profile through a primed store so the appender
        // accumulates a real `RecordUpsert` entry. We keep a concrete
        // `Arc<MemoryTableWalAppender>` so the test can read entries back; the
        // builder receives the same Arc upcast to `dyn TableWalAppender`.
        let memory_appender = Arc::new(MemoryTableWalAppender::new());
        let primed_appender: Arc<dyn TableWalAppender> = memory_appender.clone();
        let primed = crate::services::CanonicalWalRankProfileStore::new(primed_appender);
        primed
            .install("good", valid_profile_toml(), None, None)
            .await
            .unwrap();

        let entries = memory_appender.entries().await;
        assert_eq!(entries.len(), 1);

        let builder_appender: Arc<dyn TableWalAppender> = memory_appender.clone();
        let (services, store) = build_rank_services_with_appender(
            Arc::new(NoopVectorPort),
            empty_indexes(),
            builder_appender,
            &entries,
        )
        .await;

        assert_eq!(services.profile_registry.len(), 1);
        assert!(services.profile_registry.get("good").is_some());
        assert_eq!(store.list_all().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn invalid_profile_does_not_panic_boot() {
        // A profile that parses but fails compilation should be logged and
        // skipped — boot must succeed and the registry stays empty for that
        // profile name.
        let memory_appender = Arc::new(MemoryTableWalAppender::new());
        let primed_appender: Arc<dyn TableWalAppender> = memory_appender.clone();
        let primed = crate::services::CanonicalWalRankProfileStore::new(primed_appender);
        primed
            .install("broken", invalid_profile_toml(), None, None)
            .await
            .unwrap();
        primed
            .install("good", valid_profile_toml(), None, None)
            .await
            .unwrap();

        let entries = memory_appender.entries().await;

        let builder_appender: Arc<dyn TableWalAppender> = memory_appender.clone();
        let (services, store) =
            build_rank_services_with_appender(
                Arc::new(NoopVectorPort),
                empty_indexes(),
                builder_appender,
                &entries,
            )
            .await;

        // Only the valid profile makes it into the live registry.
        assert!(services.profile_registry.get("good").is_some());
        assert!(
            services.profile_registry.get("broken").is_none(),
            "broken profile must not appear in the live registry"
        );
        // But both still exist in the durable store — operators repair, not
        // the boot path.
        assert_eq!(store.list_all().await.unwrap().len(), 2);
    }
}
