//! ProximaDB Database Instance
//!
//! This module contains the main ProximaDB database instance implementation,
//! including initialization, lifecycle management, and core database operations.
//!
//! **TD-GOD-FILE**: This file (~870 lines) handles initialization, lifecycle,
//! server orchestration, and maintenance scheduling. It should be split into:
//! - `database/instance.rs` — ProximaDB struct + constructor
//! - `database/lifecycle.rs` — start/shutdown/health
//! - `database/maintenance.rs` — background tasks, RL checkpointing
//! See docs/10-quality/TECHNICAL_DEBT.adoc for tracking.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

// RL Planner checkpoint interval (5 minutes default)
const RL_CHECKPOINT_INTERVAL_SECS: u64 = 300;

use crate::{core, graph, network, proto, query, security, storage};

/// Main ProximaDB database instance
pub struct ProximaDB {
    /// Storage engine for vector data
    storage: Arc<RwLock<storage::StorageEngine>>,
    // consensus: consensus::ConsensusEngine,  // Disabled - requires raft dependency
    /// Multi-server for REST and gRPC endpoints
    multi_server: Option<network::MultiServer>,
    /// Configuration (stored for RL planner and other runtime access)
    _config: core::Config,
    /// Security coordinator for auth/RBAC
    #[allow(dead_code)]
    security: Option<Arc<security::SecurityCoordinator>>,
    /// Handle for RL planner checkpoint task (if enabled)
    rl_checkpoint_handle: Option<tokio::task::JoinHandle<()>>,
    /// Path where RL planner policy is persisted
    rl_policy_path: Option<String>,
    /// Queue subsystem for async ingest. Activated when the
    /// `PROXIMADB_QUEUE_ROOT` env var is set; otherwise `None` and
    /// async ingest degrades to inline embedding via the v3 handler
    /// fallback. Closed on `shutdown()`.
    queue_client: Option<Arc<proximadb_queue::QueueClient>>,
    /// Drainer task handle + its shutdown channel. Present when
    /// `queue_client` is present. `shutdown()` signals the drainer
    /// and awaits the join handle so in-flight work completes
    /// before the queue subsystem closes.
    drainer: Option<(
        tokio::task::JoinHandle<()>,
        tokio::sync::oneshot::Sender<()>,
    )>,
}

impl ProximaDB {
    /// Create and initialize a new ProximaDB instance from the given configuration.
    pub async fn new(config: core::Config) -> anyhow::Result<Self> {
        tracing::info!("🚀 ProximaDB::new - STARTING database initialization");
        tracing::debug!("🔍 ProximaDB::new - Config: {:?}", config);

        // Step 1: Create metrics collector first
        tracing::debug!("🔧 ProximaDB::new - Creating metrics collector...");
        let metrics_collector = Arc::new(crate::metrics::UnifiedMetricsCollector::new());
        tracing::debug!("✅ ProximaDB::new - Metrics collector created successfully");

        // Step 2: Initialize SharedServices with orchestrator
        tracing::info!("🌐 ProximaDB::new - Initializing SharedServices...");

        // Build global Cross-Cache Orchestrator from [cache] or fallback to storage.cache_size_mb
        use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
        let cache_config = config.cache.clone().unwrap_or_default();
        let cache_budget_mb = cache_config.total_memory_mb;
        let mut orchestrator =
            CrossCacheOrchestrator::new((cache_budget_mb * 1024 * 1024) as usize);

        // Start cache eviction service if enabled
        if cache_config.eviction.enabled {
            // Convert our config to the cache eviction config format
            orchestrator.start_eviction_service(None); // Deferred: Wire up eviction config conversion
        }

        // Start cache warming service if enabled (disabled by default)
        if cache_config.enable_warming {
            info!("Cache warming enabled via configuration");
            // Convert our config to the cache warming config format
            orchestrator.start_warming_service(None); // Deferred: Wire up warming config conversion
        } else {
            info!("Cache warming disabled (default)");
        }

        let orchestrator = Arc::new(orchestrator);

        // Start periodic memory rebalancing service if enabled
        if cache_config.rebalancing.enabled {
            info!(
                "Starting cache memory rebalancing service (interval: {}s)",
                cache_config.rebalancing.interval_seconds
            );
            orchestrator.clone().start_rebalancing_service();
        }
        CrossCacheOrchestrator::register_global(orchestrator.clone());

        let (shared_services, collection_service) = network::multi_server::SharedServices::new(
            Some(metrics_collector.clone()),
            &config.storage,
            Some(orchestrator.clone()),
            Some(&config),
        )
        .await?;
        tracing::info!("✅ ProximaDB::new - SharedServices created with unified CollectionService");

        // Step 3: Initialize global WAL manifest
        tracing::info!("🌐 ProximaDB::new - Initializing global WAL manifest...");
        let mut wal_config = config.storage.wal_config.to_engine_config();
        // Set data directories from storage_locations
        wal_config.multi_disk.data_directories = config.storage.storage_urls();

        // Initialize global WAL manifest using WALConfig
        let _manifest_service = storage::persistence::write_ahead_log::manifest::init(&wal_config)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to init WAL manifest: {}", e))?;
        tracing::info!("✅ ProximaDB::new - Global WAL manifest initialized");

        // Step 4: Set global metadata provider BEFORE creating StorageEngine
        // This ensures WAL pool instances can resolve collection paths correctly
        tracing::debug!(
            "🔧 ProximaDB::new - Setting global metadata provider for WAL path resolution..."
        );
        storage::persistence::write_ahead_log::set_global_metadata_provider(
            collection_service.metadata_backend().clone(),
        )
        .await;
        tracing::info!("✅ ProximaDB::new - Global metadata provider set for WAL");

        // Step 5: Initialize the storage engine (SST/VIPER/etc)
        tracing::info!("🌐 ProximaDB::new - Creating StorageEngine...");
        let storage_engine =
            storage::StorageEngine::new_without_collection_service(config.storage.clone())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create storage engine: {}", e))?;

        storage_engine
            .set_metadata_provider(collection_service.metadata_backend().clone())
            .await;

        // Wire CanonicalPrecisionResolver into Compaction. The catalog
        // already exists (constructed inside SharedServices::new above)
        // but the storage engine was built without knowing about it;
        // `Compaction::set_precision_resolver` is the post-construction
        // hook that closes the gap. When no default catalog is
        // registered (degraded boot), the resolver isn't wired and
        // compaction keeps producing fp32 records.
        match shared_services.catalog_manager.default_catalog().await {
            Ok(catalog) => {
                let resolver_cache = Arc::new(
                    proximadb_catalog::cache::CatalogCache::new(10_000, 60),
                );
                let resolver = Arc::new(
                    proximadb_catalog::canonical_precision::CanonicalPrecisionResolver::new(
                        catalog,
                        resolver_cache,
                    ),
                );
                if storage_engine
                    .compaction_manager()
                    .set_precision_resolver(resolver)
                    .is_ok()
                {
                    tracing::info!(
                        "✅ Compaction wired with CanonicalPrecisionResolver — \
                         fp16 collections will preserve precision through compaction"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "compaction bootstrap: no default catalog; precision lookup \
                     disabled, compaction rewrites will produce fp32 records \
                     regardless of collection canonical precision"
                );
            }
        }

        let storage = Arc::new(RwLock::new(storage_engine));
        tracing::info!("✅ ProximaDB::new - StorageEngine initialized successfully");

        // Step 6: Create multi-server configuration from actual config values
        use std::net::SocketAddr;
        tracing::debug!("🔧 ProximaDB::new - Creating server addresses...");
        // Determine ports: prefer ApiConfig, fall back to ServerConfig
        let rest_port = config.api.rest_port;
        let grpc_port = config.api.grpc_port;

        let rest_addr: SocketAddr = format!("{}:{}", config.server.bind_address, rest_port)
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid REST address: {}", e))?;
        let grpc_addr: SocketAddr = format!("{}:{}", config.server.bind_address, grpc_port)
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid gRPC address: {}", e))?;
        tracing::debug!(
            "🔧 ProximaDB::new - REST address: {}, gRPC address: {}",
            rest_addr,
            grpc_addr
        );

        tracing::debug!("🔧 ProximaDB::new - Building multi-server configuration...");
        let mut builder = network::MultiServerBuilder::custom()
            .http(|h| h.bind_address(rest_addr))
            .grpc(|g| g.bind_address(grpc_addr))
            .with_api_config(config.api.clone())
            .with_data_dir(config.server.data_dir.clone());

        // Add TLS configuration if enabled
        if config.api.enable_tls.unwrap_or(false) {
            tracing::debug!("🔧 ProximaDB::new - Adding TLS configuration...");
            if let Some(tls_config) = config.tls.as_ref()
                && let (Some(cert_file), Some(key_file)) =
                    (&tls_config.cert_file, &tls_config.key_file)
            {
                builder = builder.with_tls(cert_file.clone(), key_file.clone());
            }
        }

        tracing::debug!("🔧 ProximaDB::new - Building multi-server config...");
        let multi_config = builder
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create server config: {}", e))?;
        tracing::debug!("✅ ProximaDB::new - Multi-server config created successfully");

        // Initialize security coordinator if configured
        let security_config = config
            .security
            .clone()
            .filter(|sec_cfg| sec_cfg.enabled && sec_cfg.authentication.enabled);
        let security: Option<Arc<security::SecurityCoordinator>> = if let Some(sec_cfg) =
            security_config.clone()
        {
            match security::initialize_security(sec_cfg).await {
                Ok(coordinator) => Some(Arc::new(coordinator)),
                Err(err) => {
                    tracing::warn!(
                        "Security initialization failed, continuing with security disabled: {:?}",
                        err
                    );
                    None
                }
            }
        } else {
            None
        };

        // Initialize LLM engine if configured
        let llm_engine = if let Some(llm_cfg) = config.llm.clone() {
            match crate::ai::llm_integration::LLMIntegrationEngine::new(llm_cfg).await {
                Ok(engine) => Some(Arc::new(engine)),
                Err(err) => {
                    tracing::warn!("LLM engine initialization failed: {:?}", err);
                    None
                }
            }
        } else {
            None
        };

        // Create MultiServer with SharedServices (network orchestrator)
        tracing::debug!("🔧 ProximaDB::new - Creating MultiServer...");
        let rest_auth_enabled = security_config.is_some();
        let rest_multi_tenant_required = security_config.as_ref().map_or(false, |s| {
            s.authentication.require_authentication && s.mode != security::SecurityMode::Development
        });
        // Capture an Arc to the request handlers BEFORE shared_services
        // is moved into MultiServer below. The async-ingest drainer
        // needs it as the production DrainerInsertSink target via the
        // BulkLoadDrainerSink wrapper.
        let handlers_for_drainer = shared_services.request_handlers.clone();
        // Capture the catalog_manager Arc the same way — the drainer
        // needs it to construct CanonicalPrecisionResolver so per-payload
        // canonical_embedding_precision lookup populates
        // EmbeddedRecord.target_precision (fp16 ingest end-to-end).
        let catalog_manager_for_drainer = shared_services.catalog_manager.clone();

        // Open the queue subsystem BEFORE MultiServer so its Arc can
        // thread into AppState (so the v3 `/documents?mode=async` REST
        // handler routes through `producer.send`). The drainer is
        // spawned AFTER MultiServer because it needs the same handlers
        // the REST/gRPC stacks consume.
        //
        // Configuration is resolved through `QueueRuntimeConfig::resolve`
        // which folds env > TOML > defaults (see `core::config` for the
        // documented precedence pyramid). Passing `config.queue.as_ref()`
        // means the TOML `[queue]` section is the canonical source and
        // env vars exist for emergency per-pod overrides.
        let resolved_queue = core::config::QueueRuntimeConfig::resolve(config.queue.as_ref());
        let queue_client = if let Some(ref rq) = resolved_queue {
            Some(open_queue_client_from_resolved(rq).await?)
        } else {
            tracing::info!(
                "no [queue] section and PROXIMADB_QUEUE_ROOT unset; async ingest \
                 will degrade to inline embed"
            );
            None
        };

        let multi_server = network::MultiServer::new_with_queue_client(
            multi_config,
            shared_services,
            security.clone(),
            rest_auth_enabled,
            rest_multi_tenant_required,
            llm_engine,
            queue_client.clone(),
        );
        tracing::debug!("✅ ProximaDB::new - MultiServer created");

        // Phase 2H wiring (drainer half): spawn the drainer only when
        // queue_client is Some — otherwise async ingest degrades to
        // inline embed in the REST handler.
        let drainer = if let Some(ref qc) = queue_client {
            // resolved_queue is always Some when queue_client is Some
            // (they're built from the same condition), so unwrapping
            // here is safe.
            let rq = resolved_queue.as_ref().expect("queue_client → resolved_queue");
            // BulkLoader's per-engine bulk-load fallback uses this
            // as the storage root when an override needs a base_path.
            // The canonical source is `config.storage.storage_locations[0].url`
            // — typically `adls://...`, `s3://...`, or `file:///tmp/proximadb`
            // depending on deployment; reads from the TOML loaded at startup.
            let default_storage_root = config
                .storage
                .storage_urls()
                .into_iter()
                .next()
                .unwrap_or_else(|| {
                    tracing::warn!(
                        "config.storage.storage_locations is empty; \
                         BulkLoader falling back to data_dir for engine overrides"
                    );
                    format!("file://{}", config.server.data_dir.display())
                });
            Some(
                spawn_embedding_drainer_from_resolved(
                    qc.clone(),
                    handlers_for_drainer,
                    catalog_manager_for_drainer,
                    rq,
                    default_storage_root,
                )
                .await?,
            )
        } else {
            None
        };

        Ok(Self {
            storage,
            multi_server: Some(multi_server),
            _config: config,
            security,
            rl_checkpoint_handle: None,
            rl_policy_path: None,
            queue_client,
            drainer,
        })
    }

    /// Expose the queue client to startup code that wires `AppState`
    /// for the v3 async-ingest producer side. Returns `None` when the
    /// queue subsystem isn't enabled — the REST handler then takes the
    /// inline-embed fallback path.
    pub fn queue_client(&self) -> Option<Arc<proximadb_queue::QueueClient>> {
        self.queue_client.clone()
    }

    /// Start all database services (network listeners, background tasks, WAL recovery).
    pub async fn start(&mut self) -> anyhow::Result<()> {
        tracing::info!("🚀 ProximaDB::start - Starting database services...");

        // Step 1: Start storage engine (recovers collections from metadata)
        tracing::info!(
            "📦 ProximaDB::start - Step 1: Starting storage engine for collection recovery..."
        );
        {
            let mut storage = self.storage.write().await;
            storage
                .start()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to start storage engine: {}", e))?;
        }
        tracing::info!(
            "✅ ProximaDB::start - Storage engine started, collections recovered from metadata_info"
        );

        // Step 2: Recover vectors from WAL (persisted data)
        tracing::info!(
            "📦 ProximaDB::start - Step 2: Recovering vectors from WAL (persisted data)..."
        );
        {
            let storage = self.storage.read().await;
            match storage.recover_from_wal().await {
                Ok(()) => {
                    tracing::info!("✅ ProximaDB::start - Vectors recovered from WAL successfully");
                }
                Err(e) => {
                    tracing::warn!(
                        "⚠️  ProximaDB::start - WAL recovery failed (continuing anyway): {}",
                        e
                    );
                }
            }
        }

        // Step 3: Recover graphs from snapshots + WAL
        tracing::info!(
            "🌳 ProximaDB::start - Step 3: Recovering graphs from persistent storage..."
        );
        if let Some(ref multi_server) = self.multi_server {
            match multi_server
                .shared_services
                .graph_service
                .recover_all_graphs()
                .await
            {
                Ok(()) => {
                    tracing::info!("✅ ProximaDB::start - Graphs recovered successfully");
                }
                Err(e) => {
                    tracing::warn!(
                        "⚠️  ProximaDB::start - Graph recovery failed (continuing anyway): {}",
                        e
                    );
                }
            }
        }

        // Step 4: Recover assignments from collection metadata
        tracing::info!(
            "🗺️ ProximaDB::start - Step 4: Recovering assignments from collection metadata..."
        );
        tracing::info!(
            "✅ ProximaDB::start - Assignment recovery completed (or skipped if no service)"
        );

        // Step 5: Recover vectors from write buffer (in-memory data)
        tracing::info!("🔄 ProximaDB::start - Step 5: Recovering vectors from write buffer...");
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .recover_vectors_from_write_buffer(&self.storage)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to recover vectors from write buffer: {}", e)
                })?;
        }

        // Step 6: Initialize RL Query Planner (if enabled)
        tracing::info!("🎯 ProximaDB::start - Step 6: Initializing RL Query Planner...");
        self.init_rl_planner().await?;

        // Step 7: Start multi-server (HTTP and gRPC on separate ports)
        tracing::info!(
            "🌐 ProximaDB::start - Step 7: Starting multi-server (gRPC:5679 + REST:5678)..."
        );
        if let Some(ref mut multi_server) = self.multi_server {
            multi_server
                .start()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to start multi-server: {}", e))?;
        }
        tracing::info!("✅ ProximaDB::start - Multi-server started successfully");

        tracing::info!(
            "🎉 ProximaDB::start - Database startup complete with full persistence recovery!"
        );
        Ok(())
    }

    /// Shutdown the database instance gracefully.
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        info!("Graceful shutdown requested");

        // 1a. Stop the async-ingest drainer first so it stops consuming
        //     before we shut down the storage layer it inserts into.
        //     The queue subsystem itself is stopped after — its
        //     background tasks (uploader, reaper) need a few ticks to
        //     finish in-flight work.
        if let Some((handle, tx)) = self.drainer.take() {
            tracing::info!("Stopping embedding drainer...");
            let _ = tx.send(());
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), handle).await;
        }
        if let Some(client) = self.queue_client.take() {
            tracing::info!("Stopping queue subsystem...");
            if let Err(e) = client.shutdown().await {
                tracing::warn!("queue shutdown error: {}", e);
            }
        }

        // 1. Stop RL planner checkpoint task and persist policy
        tracing::info!("Stopping RL Query Planner...");
        self.shutdown_rl_planner().await;

        // 2. Flush graph WAL for all graphs before shutdown
        tracing::info!("Flushing graph WAL for all graphs...");
        if let Some(ref multi_server) = self.multi_server {
            match tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
                let graphs = multi_server
                    .shared_services
                    .graph_service
                    .list_graphs()
                    .await?;
                let mut flushed = 0;
                for graph_id in graphs {
                    if let Err(e) = multi_server
                        .shared_services
                        .graph_service
                        .flush_wal(&graph_id)
                        .await
                    {
                        tracing::warn!("Failed to flush WAL for graph {}: {}", graph_id, e);
                    } else {
                        flushed += 1;
                    }
                }
                tracing::debug!("Flushed WAL for {} graphs", flushed);
                Ok::<_, anyhow::Error>(())
            })
            .await
            {
                Ok(Ok(())) => tracing::debug!("Graph WAL flush complete"),
                Ok(Err(e)) => tracing::warn!("Graph WAL flush error: {}", e),
                Err(_) => tracing::warn!("Graph WAL flush timeout - forcing continuation"),
            }
        }

        // 3. Shutdown servers
        if let Some(mut multi_server) = self.multi_server.take() {
            multi_server
                .stop()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to stop multi-server: {}", e))?;
        }

        // 4. Stop storage engine with timeout
        match tokio::time::timeout(tokio::time::Duration::from_secs(3), async {
            let mut storage = self.storage.write().await;
            storage.stop().await
        })
        .await
        {
            Ok(Ok(())) => tracing::debug!("Storage engine stopped"),
            Ok(Err(e)) => tracing::warn!("Storage engine stop error: {}", e),
            Err(_) => tracing::warn!("Storage engine stop timeout"),
        }

        tracing::info!("Server shutdown complete");
        Ok(())
    }

    /// Check if the database instance is healthy.
    pub async fn is_healthy(&self) -> bool {
        if let Some(ref multi_server) = self.multi_server {
            multi_server.status().await.http_running
        } else {
            false
        }
    }

    /// Get current server status including versions and uptime.
    pub async fn server_status(&self) -> Option<network::multi_server::ServerStatus> {
        if let Some(ref multi_server) = self.multi_server {
            Some(multi_server.status().await)
        } else {
            None
        }
    }

    /// Get REST server address
    pub async fn rest_server_address(&self) -> Option<std::net::SocketAddr> {
        if let Some(status) = self.server_status().await {
            status.http_address
        } else {
            None
        }
    }

    /// Get gRPC server address
    pub async fn grpc_server_address(&self) -> Option<std::net::SocketAddr> {
        if let Some(status) = self.server_status().await {
            status.grpc_address
        } else {
            None
        }
    }

    /// Initialize the RL Query Planner from configuration
    async fn init_rl_planner(&mut self) -> anyhow::Result<()> {
        let rl_config = self
            ._config
            .query
            .as_ref()
            .map(|q| q.rl_planner.to_rl_planner_config())
            .unwrap_or_default();

        if !rl_config.enabled {
            tracing::info!("RL Query Planner is disabled in configuration");
            return Ok(());
        }

        let policy_path = rl_config.log_path.clone().unwrap_or_else(|| {
            format!("{}/rl_policy.json", self._config.server.data_dir.display())
        });
        self.rl_policy_path = Some(policy_path.clone());

        query::rl_planner::init_rl_planner(rl_config.clone());
        tracing::info!("✅ RL Query Planner initialized");

        if let Some(planner) = query::rl_planner::get_rl_planner() {
            if std::path::Path::new(&policy_path).exists() {
                planner.load_policy(&policy_path).await?;
                tracing::info!("✅ RL policy loaded from {}", policy_path);
            }

            let checkpoint_path = policy_path.clone();
            let checkpoint_handle = tokio::spawn(async move {
                let interval = std::time::Duration::from_secs(RL_CHECKPOINT_INTERVAL_SECS);
                let mut ticker = tokio::time::interval(interval);
                ticker.tick().await;

                loop {
                    ticker.tick().await;
                    if let Some(planner) = query::rl_planner::get_rl_planner() {
                        if let Err(e) = planner.save_policy(&checkpoint_path).await {
                            tracing::warn!("Failed to save RL policy checkpoint: {}", e);
                        }
                    } else {
                        break;
                    }
                }
            });
            self.rl_checkpoint_handle = Some(checkpoint_handle);
        }

        Ok(())
    }

    /// Shutdown RL planner: stop checkpoint task and persist final policy
    async fn shutdown_rl_planner(&mut self) {
        if let Some(handle) = self.rl_checkpoint_handle.take() {
            handle.abort();
        }

        if let Some(ref policy_path) = self.rl_policy_path
            && let Some(planner) = query::rl_planner::get_rl_planner()
        {
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(5),
                planner.save_policy(policy_path),
            )
            .await
            {
                Ok(Ok(())) => tracing::info!("✅ RL policy persisted to {}", policy_path),
                Ok(Err(e)) => tracing::warn!("Failed to persist RL policy: {}", e),
                Err(_) => tracing::warn!("RL policy persist timeout"),
            }
        }
    }

    // =========================================================================
    // Graph Database API - High-level graph operations
    // =========================================================================

    /// Create a new graph collection
    pub async fn create_graph(
        &self,
        graph_id: &str,
        schema: Option<proto::proximadb_v1::GraphSchema>,
    ) -> anyhow::Result<()> {
        if let Some(ref multi_server) = self.multi_server {
            let request = proto::proximadb_v1::CreateGraphRequest {
                graph_id: graph_id.to_string(),
                name: Some(graph_id.to_string()),
                description: None,
                schema,
                storage_config: None,
                engine_config: None,
                access_control: None,
            };
            multi_server
                .shared_services
                .graph_service
                .create_graph_collection(request)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create graph collection: {}", e))?;
            Ok(())
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// List all graphs in the database
    pub async fn list_graphs(&self) -> anyhow::Result<Vec<String>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .list_graphs()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to list graphs: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Create a node in a graph
    pub async fn create_node(
        &self,
        graph_id: &str,
        node: graph::Node,
    ) -> anyhow::Result<std::sync::Arc<graph::Node>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .create_node(graph_id, node)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create node: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Get a node by ID from a graph
    pub async fn get_node(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> anyhow::Result<Option<std::sync::Arc<graph::Node>>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .get_node(graph_id, &node_id.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get node: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Update a node in a graph
    pub async fn update_node(
        &self,
        graph_id: &str,
        node: graph::Node,
    ) -> anyhow::Result<std::sync::Arc<graph::Node>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .update_node(graph_id, node)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to update node: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Delete a node from a graph
    pub async fn delete_node(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> anyhow::Result<Option<std::sync::Arc<graph::Node>>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .delete_node(graph_id, &node_id.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to delete node: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Create an edge between two nodes in a graph
    pub async fn create_edge(
        &self,
        graph_id: &str,
        edge: graph::Edge,
    ) -> anyhow::Result<std::sync::Arc<graph::Edge>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .create_edge(graph_id, edge)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create edge: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Get an edge by ID from a graph
    pub async fn get_edge(
        &self,
        graph_id: &str,
        edge_id: &str,
    ) -> anyhow::Result<Option<std::sync::Arc<graph::Edge>>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .get_edge(graph_id, &edge_id.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get edge: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Delete an edge from a graph
    pub async fn delete_edge(
        &self,
        graph_id: &str,
        edge_id: &str,
    ) -> anyhow::Result<Option<std::sync::Arc<graph::Edge>>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .delete_edge(graph_id, &edge_id.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to delete edge: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Get graph statistics
    pub async fn get_graph_stats(
        &self,
        graph_id: &str,
    ) -> anyhow::Result<proto::proximadb_v1::GraphStats> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .get_stats(graph_id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get graph stats: {}", e))
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }

    /// Flush the WAL for a specific graph to ensure durability
    pub async fn flush_graph_wal(&self, graph_id: &str) -> anyhow::Result<()> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .flush_wal(graph_id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to flush graph WAL: {}", e))?;
            Ok(())
        } else {
            anyhow::bail!("Multi-server not initialized")
        }
    }
}

/// Initialize the async-ingest queue + drainer subsystem when the
/// `PROXIMADB_QUEUE_ROOT` env var is set. Returns `(None, None)` when
/// the var is absent — production deployments that haven't migrated
/// to async ingest just see no behavior change.
///
/// Config knobs (all env-driven for now):
///
/// - `PROXIMADB_QUEUE_ROOT` (required to enable): filesystem root URL,
///   typically `file:///var/lib/proximadb/queue` (PVC-backed in k8s)
///   or `adls://...` once the proximadb-filesystem extraction lands.
/// - `PROXIMADB_QUEUE_OBJECT_ARCHIVE` (optional): archive URL for
///   sealed segments — proves out the ECS-node-failure recovery story.
/// - `PROXIMADB_EMBED_DRAINER_PARTITIONS` (optional, default "0..16"):
///   partition range this replica drains. Single-replica deployments
///   leave it default; multi-replica needs disjoint per-pod ranges.
/// Open the queue client from an already-resolved config. The
/// `resolve()` step (see `core::config::QueueRuntimeConfig::resolve`)
/// has already folded env > TOML > defaults into a `ResolvedQueueConfig`,
/// so this function only does the I/O.
async fn open_queue_client_from_resolved(
    rq: &core::config::ResolvedQueueConfig,
) -> anyhow::Result<Arc<proximadb_queue::QueueClient>> {
    tracing::info!(
        queue_root = %rq.root,
        object_archive = ?rq.object_archive,
        sync_mode = %rq.sync_mode,
        "Opening queue subsystem for async ingest"
    );

    let mut topics = std::collections::HashMap::new();
    topics.insert(
        crate::services::EMBED_INGEST_TOPIC.to_string(),
        proximadb_queue::TopicConfig::default(),
    );
    let sync_mode = if rq.sync_mode == "lazy" {
        proximadb_queue::SyncMode::Lazy
    } else {
        proximadb_queue::SyncMode::Strict
    };
    let queue_cfg = proximadb_queue::QueueConfig {
        root: rq.root.clone(),
        object_archive: rq.object_archive.clone(),
        default_sync_mode: sync_mode,
        topics,
    };

    // Build a FilesystemFactory-backed QueueFs adapter so the queue's
    // `root` URL can be `file://`, `adls://`, `s3://`, etc. (any
    // scheme the factory knows). For pure-local file:// roots the
    // adapter behaves identically to the queue's built-in LocalFs;
    // for cloud schemes it routes through the factory's per-scheme
    // backend. This unblocks the "object_archive on adls:// / s3:// /
    // gcs://" use case for operators deploying with cross-scheme storage.
    let fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    let factory = std::sync::Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config)
            .await
            .map_err(|e| anyhow::anyhow!("FilesystemFactory init: {}", e))?,
    );
    let fs_adapter = crate::services::queue_fs_adapter::FactoryQueueFs::new(factory, &rq.root);

    let queue = proximadb_queue::QueueClient::open_with_fs(queue_cfg, Some(fs_adapter))
        .await
        .map_err(|e| anyhow::anyhow!("queue open: {}", e))?;
    tracing::info!("✅ Queue subsystem opened (via FilesystemFactory adapter)");
    Ok(queue)
}

/// Spawn the embedding drainer using partitions from the resolved
/// queue config. The `drainer_partitions` field is the string form
/// (e.g. `"0..16"` or `"0,3..6,9"`); `parse_partition_list` turns it
/// into a `Vec<u32>`.
async fn spawn_embedding_drainer_from_resolved(
    queue: Arc<proximadb_queue::QueueClient>,
    handlers: Arc<crate::api_handlers::request_handlers::UnifiedHandlers>,
    catalog_manager: Arc<crate::catalog::CatalogManager>,
    rq: &core::config::ResolvedQueueConfig,
    default_storage_root: String,
) -> anyhow::Result<(
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Sender<()>,
)> {
    let bulk_loader = Arc::new(crate::services::bulk_load::BulkLoader::new(
        handlers,
        default_storage_root,
    ));
    let sink: Arc<dyn crate::services::DrainerInsertSink> = Arc::new(
        crate::services::bulk_load::BulkLoadDrainerSink::new(bulk_loader),
    );
    let embed_service = proximadb_embedding::EmbeddingService::global();
    let drainer_cfg = crate::services::EmbeddingDrainerConfig::default();

    // Build the canonical-precision resolver from the same catalog +
    // cache the rest of the server uses. When no default catalog is
    // registered (degraded boot), the drainer falls back to None and
    // ships every record at fp32 — same behavior as before.
    let mut drainer =
        crate::services::EmbeddingDrainer::new(queue, embed_service, sink, drainer_cfg);
    match catalog_manager.default_catalog().await {
        Ok(catalog) => {
            // Separate CatalogCache instance for the resolver — same type as
            // the runtime catalog's cache, but its own Arc so TTL/capacity
            // and hit-rate signal stay independent. A
            // canonical_embedding_precision change can lag by up to the TTL
            // (60s) before the drainer picks it up; acceptable because
            // precision changes are rare and the mismatched-batch path
            // errors safely.
            let resolver_cache = Arc::new(
                proximadb_catalog::cache::CatalogCache::new(10_000, 60),
            );
            let resolver = Arc::new(
                proximadb_catalog::canonical_precision::CanonicalPrecisionResolver::new(
                    catalog,
                    resolver_cache,
                ),
            );
            drainer = drainer.with_precision_resolver(resolver);
            tracing::info!(
                "✅ Embedding drainer wired with CanonicalPrecisionResolver — \
                 fp16 collections will produce fp16 records at ingest"
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "drainer bootstrap: no default catalog; precision lookup disabled, \
                 records will ship as fp32 regardless of collection canonical precision"
            );
        }
    }

    let partitions: Vec<u32> = parse_partition_list(&rq.drainer_partitions).unwrap_or_else(|| {
        tracing::warn!(
            range = %rq.drainer_partitions,
            "failed to parse drainer partitions; falling back to 0..16"
        );
        (0..16u32).collect()
    });
    let pair = drainer.start(partitions);
    tracing::info!("✅ Embedding drainer started");
    Ok(pair)
}

/// Open the queue client when `PROXIMADB_QUEUE_ROOT` is set. Returns
/// `None` otherwise — deployments not opted into async ingest see no
/// behavior change.
///
/// **DEPRECATED**: prefer `QueueRuntimeConfig::resolve` +
/// `open_queue_client_from_resolved` for proper precedence handling.
/// This helper is retained until callers migrate.
#[allow(dead_code)]
async fn open_queue_client_if_configured()
-> anyhow::Result<Option<Arc<proximadb_queue::QueueClient>>> {
    let Some(root) = std::env::var("PROXIMADB_QUEUE_ROOT").ok() else {
        tracing::info!("PROXIMADB_QUEUE_ROOT not set; async ingest will degrade to inline embed");
        return Ok(None);
    };
    tracing::info!(queue_root = %root, "Opening queue subsystem for async ingest");

    let mut topics = std::collections::HashMap::new();
    topics.insert(
        crate::services::EMBED_INGEST_TOPIC.to_string(),
        proximadb_queue::TopicConfig::default(),
    );
    let mut queue_cfg = proximadb_queue::QueueConfig::from_env();
    queue_cfg.root = root;
    queue_cfg.topics = topics;

    let queue = proximadb_queue::QueueClient::open(queue_cfg)
        .await
        .map_err(|e| anyhow::anyhow!("queue open: {}", e))?;
    tracing::info!("✅ Queue subsystem opened");
    Ok(Some(queue))
}

/// Spawn the embedding drainer with the production
/// `BulkLoadDrainerSink`. The drainer subscribes to
/// `0..partition_count` by default. For multi-replica deployments,
/// `PROXIMADB_EMBED_DRAINER_PARTITIONS` carves out a per-pod range.
///
/// **DEPRECATED**: prefer `spawn_embedding_drainer_from_resolved`
/// which reads `default_storage_root` from the TOML config rather
/// than hard-defaulting.
#[allow(dead_code)]
async fn spawn_embedding_drainer(
    queue: Arc<proximadb_queue::QueueClient>,
    handlers: Arc<crate::api_handlers::request_handlers::UnifiedHandlers>,
) -> anyhow::Result<(
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Sender<()>,
)> {
    // Legacy fallback path; the resolved-from-config helper supplies
    // the canonical storage_locations URL instead of this hard default.
    let default_storage_root = "file:///tmp/proximadb".to_string();
    let bulk_loader = Arc::new(crate::services::bulk_load::BulkLoader::new(
        handlers,
        default_storage_root,
    ));
    let sink: Arc<dyn crate::services::DrainerInsertSink> = Arc::new(
        crate::services::bulk_load::BulkLoadDrainerSink::new(bulk_loader),
    );
    let embed_service = proximadb_embedding::EmbeddingService::global();
    let drainer_cfg = crate::services::EmbeddingDrainerConfig::default();
    let drainer =
        crate::services::EmbeddingDrainer::new(queue, embed_service, sink, drainer_cfg);

    let partitions: Vec<u32> = std::env::var("PROXIMADB_EMBED_DRAINER_PARTITIONS")
        .ok()
        .and_then(|s| parse_partition_list(&s))
        .unwrap_or_else(|| (0..16u32).collect());
    let pair = drainer.start(partitions);
    tracing::info!("✅ Embedding drainer started");
    Ok(pair)
}

/// Parse "0,1,2" or "0..4" or "0,3..6,9" into a Vec<u32>.
fn parse_partition_list(s: &str) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for chunk in s.split(',') {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if let Some((lo, hi)) = chunk.split_once("..") {
            let lo: u32 = lo.parse().ok()?;
            let hi: u32 = hi.parse().ok()?;
            for p in lo..hi {
                out.push(p);
            }
        } else {
            out.push(chunk.parse().ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod async_ingest_wiring_tests {
    use super::parse_partition_list;

    #[test]
    fn parse_partition_list_handles_commas_and_ranges() {
        assert_eq!(parse_partition_list("0,1,2"), Some(vec![0, 1, 2]));
        assert_eq!(parse_partition_list("0..4"), Some(vec![0, 1, 2, 3]));
        assert_eq!(parse_partition_list("0,3..6,9"), Some(vec![0, 3, 4, 5, 9]));
        assert_eq!(parse_partition_list(""), Some(vec![]));
    }

    #[test]
    fn parse_partition_list_rejects_garbage() {
        assert_eq!(parse_partition_list("abc"), None);
        assert_eq!(parse_partition_list("0..abc"), None);
    }
}
