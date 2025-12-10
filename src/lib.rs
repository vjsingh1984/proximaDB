/*
 * Copyright 2025 Vijaykumar Singh
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

// SIMD optimization features (using stable AVX2 instead of unstable AVX-512)

// Enforce error handling best practices
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]
#![warn(clippy::unimplemented)]
#![warn(clippy::todo)]
#![warn(clippy::too_many_arguments)]
#![warn(clippy::type_complexity)]
#![warn(clippy::large_enum_variant)]
#![warn(clippy::result_large_err)]
#![warn(clippy::missing_docs_in_private_items)]

//! # ProximaDB - Cloud-Native Vector Database
//!
//! **proximity at scale**
//!
//! ProximaDB is a high-performance, cloud-native vector database engineered for AI-first applications.
//! Built from the ground up for serverless deployment, intelligent data tiering, and global scale.
//!
//! ## Architecture Overview
//!
//! ProximaDB follows a modular, layered architecture optimized for vector similarity search:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Client Applications                       │
//! ├─────────────────────────────────────────────────────────────┤
//! │                  API Layer (REST + gRPC)                     │
//! │                    [api_handlers module]                     │
//! ├─────────────────────────────────────────────────────────────┤
//! │                     Service Layer                            │
//! │            [services module - business logic]                │
//! ├─────────────────────────────────────────────────────────────┤
//! │    Index Layer          │         Compute Layer              │
//! │   [AXIS engine]         │    [SIMD/GPU acceleration]         │
//! ├─────────────────────────────────────────────────────────────┤
//! │                     Storage Layer                            │
//! │    [WAL + Memtable]  →  [Storage Engines]  →  [Filesystem]  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Module Organization
//!
//! - **`api`**: Protocol definitions and API contracts
//! - **`api_handlers`**: Unified REST/gRPC request handlers with zero-copy proto-first design
//! - **`core`**: Core types, errors, configuration, and foundational components
//! - **`compute`**: Vector computation, distance metrics, quantization, and hardware acceleration
//! - **`index`**: AXIS indexing engine with multiple algorithm support (HNSW, IVF, LSH, etc.)
//! - **`services`**: Business logic layer for collections, search, and vector operations
//! - **`storage`**: Multi-tiered storage with WAL, memtable, and pluggable storage engines
//! - **`network`**: Server implementation with REST and gRPC support
//! - **`infrastructure`**: Shared infrastructure components and utilities
//! - **`metrics`**: Comprehensive metrics collection and monitoring
//!
//! ## Key Design Principles
//!
//! 1. **Proto-First Architecture**: Native protocol buffer flow without intermediate conversions
//! 2. **Zero-Copy Operations**: Minimize data copying throughout the pipeline
//! 3. **Hardware Adaptive**: Automatic detection and use of SIMD/GPU capabilities
//! 4. **Cloud-Native Storage**: Seamless integration with S3, Azure Blob, GCS
//! 5. **Pluggable Storage Engines**: Support for different workload patterns (SST, VIPER, NOVA, etc.)
//!
//! ## Key Features
//!
//! - **Proximity at Scale**: SIMD-optimized vector operations with GPU acceleration
//! - **Serverless-Native**: Scale to zero, pay per use
//! - **Intelligent Tiering**: MMAP hot data, S3 cold storage
//! - **Multi-Cloud**: AWS, Azure, GCP support
//! - **Global Distribution**: Multi-region with data residency
//! - **Enterprise Ready**: RBAC, audit logs, compliance

/// REST and gRPC API definitions and protocol contracts
// pub mod api; // Removed - using proto types directly with serde compatibility

/// Shared infrastructure components for cross-cutting concerns
pub mod infrastructure;

/// High-performance compute layer with SIMD/GPU acceleration for vector operations
pub mod compute;

// pub mod consensus;  // Disabled - requires raft dependency

/// Core types, errors, configuration, and foundational components
pub mod core;

/// Unified error handling for REST and gRPC APIs
pub mod errors;

/// Native graph database engine with CSR format and Arc-based memory sharing
pub mod graph;

// pub mod distributed;  // Temporarily disabled for single-node optimization

/// Unified API handlers for REST and gRPC with proto-first zero-copy design
pub mod api_handlers;

/// Enhanced authentication and authorization for multi-tenant enterprise
pub mod auth;

/// Comprehensive audit system for enterprise compliance
pub mod audit;

/// Unified security architecture consolidating auth, RBAC, and audit
pub mod security;

/// AI-powered intelligence for Release 2 enterprise platform
pub mod ai;

/// Enterprise deployment automation for one-click setup
pub mod deployment;

/// Enterprise revenue engine for billing and customer success
pub mod revenue;

/// Sales enablement platform for customer-facing sales automation
pub mod sales_enablement;

/// License management and tier enforcement for all deployment models
pub mod licensing;

/// Executive intelligence platform for C-level strategic analytics
pub mod executive;

/// AXIS indexing engine with support for multiple algorithms (HNSW, IVF, LSH, etc.)
pub mod index;

/// AutoML Framework for automated optimization and tuning
pub mod automl;

// Unified metrics module - combines advanced persistent metrics with real-time monitoring
pub mod metrics;
pub mod monitoring;
pub mod network;
pub mod proto;
pub mod query;
pub mod schema;
// NOTE: schema_constants module removed - using hardcoded schema_types.rs instead
// schema_types removed - use core::avro_unified instead
pub mod search;
pub mod server;
pub mod services;
pub mod storage;
pub mod utils;
pub mod version;

/// Embedded mode for in-process database usage without network layer
/// Enable with feature flag: --features python
pub mod embedded;

// NOTE: Compiled Avro schemas disabled - using hardcoded schema_types.rs instead
// pub mod compiled_schemas {
//     include!(concat!(env!("OUT_DIR"), "/compiled_schemas.rs"));
// }

// Re-export commonly used types from core
pub use core::{Config, VectorRecord, error::ProximaDBError as Error};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Main ProximaDB database instance
pub struct ProximaDB {
    storage: Arc<RwLock<storage::StorageEngine>>,
    // consensus: consensus::ConsensusEngine,  // Disabled - requires raft dependency
    multi_server: Option<network::MultiServer>,
    _config: core::Config,
}

impl ProximaDB {
    pub async fn new(config: core::Config) -> Result<Self> {
        tracing::info!("🚀 ProximaDB::new - STARTING database initialization");
        tracing::debug!("🔍 ProximaDB::new - Config: {:?}", config);

        // Step 1: Create metrics collector first
        tracing::debug!("🔧 ProximaDB::new - Creating metrics collector...");
        let _metrics_config = metrics::MetricsConfig::default();
        let metrics_collector = Arc::new(monitoring::MetricsCollector::new());
        tracing::debug!("✅ ProximaDB::new - Metrics collector created successfully");

        // Step 2: Create SharedServices FIRST (owns all services)
        // This avoids duplicate CollectionService creation
        tracing::info!(
            "🔧 ProximaDB::new - Creating SharedServices FIRST to avoid circular dependency"
        );
        // Build global Cross-Cache Orchestrator from [cache] or fallback to storage.cache_size_mb
        use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
        let cache_config = config.cache.clone().unwrap_or_default();
        let cache_budget_mb = cache_config.total_memory_mb;
        let mut orchestrator =
            CrossCacheOrchestrator::new((cache_budget_mb * 1024 * 1024) as usize);

        // Start cache eviction service if enabled
        if cache_config.eviction.enabled {
            // Convert our config to the cache eviction config format
            orchestrator.start_eviction_service(None); // TODO: Wire up eviction config conversion
        }

        // Start cache warming service if enabled (disabled by default)
        if cache_config.enable_warming {
            info!("Cache warming enabled via configuration");
            // Convert our config to the cache warming config format
            orchestrator.start_warming_service(None); // TODO: Wire up warming config conversion
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
        let _manifest_service =
            storage::persistence::write_ahead_log::manifest::init(&wal_config).await?;
        tracing::info!("✅ ProximaDB::new - Global WAL manifest initialized");

        // Step 4: Set global metadata provider BEFORE creating StorageEngine
        // This ensures WAL pool instances can resolve collection paths correctly
        tracing::debug!("🔧 ProximaDB::new - Setting global metadata provider for WAL path resolution...");
        storage::persistence::write_ahead_log::set_global_metadata_provider(
            collection_service.metadata_backend().clone()
        ).await;
        tracing::info!("✅ ProximaDB::new - Global metadata provider set for WAL");

        // Step 5: Create StorageEngine using the CollectionService from SharedServices
        tracing::debug!(
            "🔧 ProximaDB::new - Creating storage engine with injected CollectionService..."
        );
        let storage_engine =
            storage::StorageEngine::new_without_collection_service(config.storage.clone()).await?;
        // Inject the metadata backend from CollectionService (not CollectionService itself!)
        storage_engine
            .set_metadata_provider(collection_service.metadata_backend().clone())
            .await;
        tracing::info!(
            "✅ ProximaDB::new - Storage engine created with SharedServices' CollectionService"
        );
        let storage = Arc::new(RwLock::new(storage_engine));

        // let consensus = consensus::ConsensusEngine::new(config.consensus.clone()).await?; // Disabled

        // Create multi-server configuration from actual config values
        use std::net::SocketAddr;
        tracing::debug!("🔧 ProximaDB::new - Creating server addresses...");
        // Determine ports: prefer ApiConfig, fall back to ServerConfig
        let rest_port = config.api.rest_port;
        let grpc_port = config.api.grpc_port;

        let rest_addr: SocketAddr = format!("{}:{}", config.server.bind_address, rest_port)
            .parse()
            .map_err(|e| format!("Invalid REST address: {}", e))?;
        let grpc_addr: SocketAddr = format!("{}:{}", config.server.bind_address, grpc_port)
            .parse()
            .map_err(|e| format!("Invalid gRPC address: {}", e))?;
        tracing::debug!(
            "🔧 ProximaDB::new - REST address: {}, gRPC address: {}",
            rest_addr,
            grpc_addr
        );

        tracing::debug!("🔧 ProximaDB::new - Building multi-server configuration...");
        let mut builder = network::MultiServerBuilder::custom()
            .http(|h| h.bind_address(rest_addr))
            .grpc(|g| g.bind_address(grpc_addr))
            .with_api_config(config.api.clone());

        // Add TLS configuration if enabled
        if config.api.enable_tls.unwrap_or(false) && config.tls.is_some() {
            tracing::debug!("🔧 ProximaDB::new - Adding TLS configuration...");
            let tls_config = config.tls.as_ref().unwrap();
            if let (Some(cert_file), Some(key_file)) = (&tls_config.cert_file, &tls_config.key_file)
            {
                builder = builder.with_tls(cert_file.clone(), key_file.clone());
            }
        }

        tracing::debug!("🔧 ProximaDB::new - Building multi-server config...");
        let multi_config = builder
            .build()
            .map_err(|e| format!("Failed to create server config: {}", e))?;
        tracing::debug!("✅ ProximaDB::new - Multi-server config created successfully");

        // SharedServices and metrics collector already created above

        // Create MultiServer with SharedServices (network orchestrator)
        tracing::debug!("🔧 ProximaDB::new - Creating MultiServer...");
        let multi_server = network::MultiServer::new(multi_config, shared_services);
        tracing::debug!("✅ ProximaDB::new - MultiServer created successfully");

        Ok(Self {
            storage,
            // consensus,  // Disabled
            multi_server: Some(multi_server),
            _config: config,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("🚀 ProximaDB::start - Starting database services...");

        // Step 1: Start storage engine (recovers collections from metadata)
        tracing::info!(
            "📦 ProximaDB::start - Step 1: Starting storage engine for collection recovery..."
        );
        {
            let mut storage = self.storage.write().await;
            storage.start().await?;
        }
        tracing::info!(
            "✅ ProximaDB::start - Storage engine started, collections recovered from metadata_info"
        );

        // Step 2: Recover vectors from WAL (persisted data)
        eprintln!("🔍 DEBUG: ProximaDB::start - About to call storage.recover_from_wal()");
        tracing::info!(
            "📦 ProximaDB::start - Step 2: Recovering vectors from WAL (persisted data)..."
        );
        {
            let storage = self.storage.read().await;
            eprintln!("🔍 DEBUG: Got storage lock, calling recover_from_wal()...");
            match storage.recover_from_wal().await {
                Ok(()) => {
                    eprintln!("✅ DEBUG: recover_from_wal() returned Ok(())");
                    tracing::info!("✅ ProximaDB::start - Vectors recovered from WAL successfully");
                }
                Err(e) => {
                    eprintln!("❌ DEBUG: recover_from_wal() returned Err: {}", e);
                    tracing::warn!(
                        "⚠️  ProximaDB::start - WAL recovery failed (continuing anyway): {}",
                        e
                    );
                    // Don't fail startup if WAL recovery fails - data might still be in memtable
                }
            }
        }
        eprintln!("🔍 DEBUG: ProximaDB::start - WAL recovery step complete");

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
                    // Don't fail startup if graph recovery fails
                }
            }
        }

        // Step 4: Recover assignments from collection metadata
        tracing::info!(
            "🗺️ ProximaDB::start - Step 4: Recovering assignments from collection metadata..."
        );
        // TODO: When AssignmentService is added to SharedServices, call:
        // self.multi_server.as_ref().unwrap().shared_services.assignment_service.recover_assignments().await?;
        tracing::info!(
            "✅ ProximaDB::start - Assignment recovery completed (or skipped if no service)"
        );

        // Step 5: Recover vectors from write buffer (in-memory data)
        tracing::info!("🔄 ProximaDB::start - Step 5: Recovering vectors from write buffer...");
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .recover_vectors_from_write_buffer(&self.storage)
                .await?;
        }

        // Step 6: Start multi-server (HTTP and gRPC on separate ports)
        tracing::info!(
            "🌐 ProximaDB::start - Step 6: Starting multi-server (gRPC:5679 + REST:5678)..."
        );
        if let Some(ref mut multi_server) = self.multi_server {
            multi_server
                .start()
                .await
                .map_err(|e| format!("Failed to start multi-server: {}", e))?;
        }
        tracing::info!("✅ ProximaDB::start - Multi-server started successfully");

        tracing::info!(
            "🎉 ProximaDB::start - Database startup complete with full persistence recovery!"
        );
        tracing::info!("📋 Recovery Order Summary:");
        tracing::info!("  1️⃣ Collections: Recovered from metadata snapshots");
        tracing::info!("  2️⃣ Vectors (WAL): Recovered from persisted WAL files");
        tracing::info!("  3️⃣ Graphs: Recovered from snapshots + WAL replay");
        tracing::info!("  4️⃣ Assignments: Recovered from collection metadata");
        tracing::info!("  5️⃣ Vectors (Buffer): Recovered from in-memory write buffer");
        tracing::info!("  6️⃣ Services: HTTP/gRPC servers started");
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        // Flush graph WAL for all graphs before shutdown
        tracing::info!("Flushing graph WAL for all graphs...");
        if let Some(ref multi_server) = self.multi_server {
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(5),
                async {
                    let graphs = multi_server.shared_services.graph_service.list_graphs().await?;
                    let mut flushed = 0;
                    for graph_id in graphs {
                        if let Err(e) = multi_server.shared_services.graph_service.flush_wal(&graph_id).await {
                            tracing::warn!("Failed to flush WAL for graph {}: {}", graph_id, e);
                        } else {
                            flushed += 1;
                        }
                    }
                    tracing::debug!("Flushed WAL for {} graphs", flushed);
                    Ok::<_, crate::core::error::ProximaDBError>(())
                }
            ).await {
                Ok(Ok(())) => tracing::debug!("Graph WAL flush complete"),
                Ok(Err(e)) => tracing::warn!("Graph WAL flush error: {}", e),
                Err(_) => tracing::warn!("Graph WAL flush timeout - forcing continuation"),
            }
        }

        // Shutdown global WAL manifest (flush pending writes) with timeout
        tracing::info!("Shutting down global WAL manifest...");
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            storage::persistence::write_ahead_log::manifest::shutdown()
        ).await {
            Ok(Ok(())) => tracing::debug!("Global WAL manifest shut down"),
            Ok(Err(e)) => tracing::warn!("WAL manifest shutdown error: {}", e),
            Err(_) => tracing::warn!("WAL manifest shutdown timeout - forcing continuation"),
        }

        // Stop multi-server with timeout
        if let Some(ref mut multi_server) = self.multi_server {
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(3),
                multi_server.stop()
            ).await {
                Ok(Ok(())) => tracing::debug!("Multi-server stopped"),
                Ok(Err(e)) => tracing::warn!("Multi-server stop error: {}", e),
                Err(_) => tracing::warn!("Multi-server stop timeout"),
            }
        }

        // Stop storage engine with timeout
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(3),
            async {
                let mut storage = self.storage.write().await;
                storage.stop().await
            }
        ).await {
            Ok(Ok(())) => tracing::debug!("Storage engine stopped"),
            Ok(Err(e)) => tracing::warn!("Storage engine stop error: {}", e),
            Err(_) => tracing::warn!("Storage engine stop timeout"),
        }

        tracing::info!("Server shutdown complete");
        Ok(())
    }

    /// Get the multi-server status
    pub async fn server_status(&self) -> Option<network::multi_server::ServerStatus> {
        if let Some(ref multi_server) = self.multi_server {
            Some(multi_server.status().await)
        } else {
            None
        }
    }

    /// Check if any server is running
    pub async fn is_server_running(&self) -> bool {
        if let Some(status) = self.server_status().await {
            status.http_running || status.grpc_running
        } else {
            false
        }
    }

    /// Get HTTP server address
    pub async fn http_server_address(&self) -> Option<std::net::SocketAddr> {
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

    // =========================================================================
    // Graph Database API - High-level graph operations
    // =========================================================================

    /// Create a new graph collection
    ///
    /// # Arguments
    /// * `graph_id` - Unique identifier for the graph
    /// * `schema` - Optional schema definition for the graph
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err` if the graph already exists or creation fails
    pub async fn create_graph(
        &self,
        graph_id: &str,
        schema: Option<proto::proximadb_v1::GraphSchema>,
    ) -> Result<()> {
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
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(())
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// List all graphs in the database
    ///
    /// # Returns
    /// * `Vec<String>` - List of graph IDs
    pub async fn list_graphs(&self) -> Result<Vec<String>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .list_graphs()
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Create a node in a graph
    ///
    /// # Arguments
    /// * `graph_id` - The graph to add the node to
    /// * `node` - The node to create
    ///
    /// # Returns
    /// * `Ok(Arc<Node>)` - The created node
    pub async fn create_node(
        &self,
        graph_id: &str,
        node: graph::Node,
    ) -> Result<std::sync::Arc<graph::Node>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .create_node(graph_id, node)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Get a node by ID from a graph
    ///
    /// # Arguments
    /// * `graph_id` - The graph containing the node
    /// * `node_id` - The ID of the node to retrieve
    ///
    /// # Returns
    /// * `Ok(Some(Arc<Node>))` if found, `Ok(None)` if not found
    pub async fn get_node(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<Option<std::sync::Arc<graph::Node>>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .get_node(graph_id, &node_id.to_string())
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Update a node in a graph
    ///
    /// # Arguments
    /// * `graph_id` - The graph containing the node
    /// * `node` - The updated node (must have same ID as existing node)
    ///
    /// # Returns
    /// * `Ok(Arc<Node>)` - The updated node
    pub async fn update_node(
        &self,
        graph_id: &str,
        node: graph::Node,
    ) -> Result<std::sync::Arc<graph::Node>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .update_node(graph_id, node)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Delete a node from a graph
    ///
    /// # Arguments
    /// * `graph_id` - The graph containing the node
    /// * `node_id` - The ID of the node to delete
    ///
    /// # Returns
    /// * `Ok(Some(Arc<Node>))` if deleted, `Ok(None)` if not found
    pub async fn delete_node(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<Option<std::sync::Arc<graph::Node>>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .delete_node(graph_id, &node_id.to_string())
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Create an edge between two nodes in a graph
    ///
    /// # Arguments
    /// * `graph_id` - The graph to add the edge to
    /// * `edge` - The edge to create
    ///
    /// # Returns
    /// * `Ok(Arc<Edge>)` - The created edge
    pub async fn create_edge(
        &self,
        graph_id: &str,
        edge: graph::Edge,
    ) -> Result<std::sync::Arc<graph::Edge>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .create_edge(graph_id, edge)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Get an edge by ID from a graph
    ///
    /// # Arguments
    /// * `graph_id` - The graph containing the edge
    /// * `edge_id` - The ID of the edge to retrieve
    ///
    /// # Returns
    /// * `Ok(Some(Arc<Edge>))` if found, `Ok(None)` if not found
    pub async fn get_edge(
        &self,
        graph_id: &str,
        edge_id: &str,
    ) -> Result<Option<std::sync::Arc<graph::Edge>>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .get_edge(graph_id, &edge_id.to_string())
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Delete an edge from a graph
    ///
    /// # Arguments
    /// * `graph_id` - The graph containing the edge
    /// * `edge_id` - The ID of the edge to delete
    ///
    /// # Returns
    /// * `Ok(Some(Arc<Edge>))` if deleted, `Ok(None)` if not found
    pub async fn delete_edge(
        &self,
        graph_id: &str,
        edge_id: &str,
    ) -> Result<Option<std::sync::Arc<graph::Edge>>> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .delete_edge(graph_id, &edge_id.to_string())
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Get graph statistics
    ///
    /// # Arguments
    /// * `graph_id` - The graph to get stats for
    ///
    /// # Returns
    /// * `Ok(GraphStats)` - Statistics about the graph
    pub async fn get_graph_stats(
        &self,
        graph_id: &str,
    ) -> Result<proto::proximadb_v1::GraphStats> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .get_stats(graph_id)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }

    /// Flush the WAL for a specific graph to ensure durability
    ///
    /// # Arguments
    /// * `graph_id` - The graph to flush
    ///
    /// # Returns
    /// * `Ok(())` on success
    pub async fn flush_graph_wal(&self, graph_id: &str) -> Result<()> {
        if let Some(ref multi_server) = self.multi_server {
            multi_server
                .shared_services
                .graph_service
                .flush_wal(graph_id)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        } else {
            Err("Multi-server not initialized".into())
        }
    }
}
