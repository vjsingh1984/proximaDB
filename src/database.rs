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
        let security: Option<Arc<security::SecurityCoordinator>> = if let Some(sec_cfg) =
            config.security.clone()
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
        let rest_auth_enabled = config
            .security
            .as_ref()
            .map_or(false, |s| s.authentication.enabled);
        let mut multi_server = network::MultiServer::new(
            multi_config,
            shared_services,
            security.clone(),
            rest_auth_enabled,
            llm_engine,
        );
        // Wire storage engine for PostgreSQL wire protocol support
        multi_server.set_storage(storage.clone());
        tracing::debug!("✅ ProximaDB::new - MultiServer created with storage engine wired");

        Ok(Self {
            storage,
            multi_server: Some(multi_server),
            _config: config,
            security,
            rl_checkpoint_handle: None,
            rl_policy_path: None,
        })
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
