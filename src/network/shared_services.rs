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

use anyhow::Result;
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
use crate::services::VectorOperationsService;
use crate::services::collection::manager::CollectionService;
use crate::storage::MultiModalStorageFacade;
use crate::storage::StorageEngine;
use crate::storage::document::DocumentService;
use crate::storage::metadata::backends::MetadataBackendFactory;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb_kernel::uuid::Uuid;
use proximadb_graph_query::service::{GraphExecutionService, GraphQueryService};

/// Shared services for thin protocol handlers
/// Responsibilities: business logic, metadata configuration, service coordination
#[derive(Clone)]
pub struct SharedServices {
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

        let collection_service =
            Arc::new(CollectionService::new(metadata_backend, storage_config.clone()).await?);
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

        // Create SST engine
        debug!("🔧 SharedServices::new - Creating SST engine...");
        let sst_engine = Arc::new(crate::storage::engines::sst::SstEngine::new().await?);
        debug!("✅ SharedServices::new - SST engine created successfully");

        // Clone SST engine reference for DocumentService (used later for DocumentStrategy)
        let sst_engine_for_documents: Arc<dyn crate::storage::traits::UnifiedStorageEngine> =
            sst_engine.clone();

        // Create WAL manager for two-stage search
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

        let vector_operations_service = Arc::new(
            VectorOperationsService::new(
                sst_engine,
                wal_manager,
                axis_manager.clone(),
                collection_service.clone(),
            )
            .with_orchestrator(Some(orchestrator.clone())),
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

        // Create VectorSearchStrategy wrapping VectorOperationsService
        let vector_strategy: Arc<dyn crate::query::facade::QueryStrategy> =
            Arc::new(VectorSearchStrategy::new(
                vector_operations_service.clone(),
                collection_service.clone(),
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

        // Create MultiModalStorageFacade for federated queries and wire the live stores
        debug!(
            "🔧 SharedServices::new - Creating MultiModalStorageFacade for federated queries..."
        );
        let vector_store = Arc::new(
            crate::storage::multimodal::VectorStore::with_engine(
                vector_operations_service.unified_engine(),
            )
            .with_index_manager(axis_manager.clone()),
        );
        let graph_store = Arc::new(
            crate::storage::multimodal::GraphStore::new(Default::default())
                .with_service(graph_service.clone()),
        );
        let document_store = Arc::new(
            crate::storage::multimodal::DocumentStore::new(Default::default())
                .with_service(document_service.clone()),
        );
        let obs_base_path = storage_config.metadata_url.replace("file://", "");
        let observability_store = Arc::new(
            crate::storage::multimodal::ObservabilityStore::new(
                crate::storage::multimodal::stores::observability_store::ObservabilityStoreConfig {
                    base_path: obs_base_path,
                    ..Default::default()
                },
            )
            .with_service(observability_service.clone()),
        );
        let multimodal_storage = Arc::new(
            MultiModalStorageFacade::new()
                .with_vector_store(vector_store)
                .with_graph_store(graph_store)
                .with_document_store(document_store)
                .with_observability_store(observability_store),
        );
        debug!("✅ SharedServices::new - MultiModalStorageFacade created and wired");

        // Create FederatedQueryContext for SQL with multi-model extensions
        debug!("🔧 SharedServices::new - Creating FederatedQueryContext...");
        let federated_context = Arc::new(
            FederatedQueryContext::new(multimodal_storage)
                .with_collection_service(collection_service.clone())
                .with_vector_operations(vector_operations_service.clone()),
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
                collection_service: collection_service.clone(),
                vector_operations_service,
                graph_service,
                graph_query_service,
                graph_execution_service,
                document_service,
                observability_service,
                request_handlers,
                metrics_collector,
                metrics_updater: Some(metrics_updater.clone()),
                query_facade,
                api_handlers: runtime_api_handlers,
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
                                &vector_record.id, collection_id, e
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
