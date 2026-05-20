// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Multi-protocol server lifecycle orchestration (`MultiServer`).
//!
//! Owns server startup, TCP multiplexing, TLS wiring, and graceful shutdown
//! across REST, gRPC, Arrow Flight, and PostgreSQL wire protocol.
//!
//! Related modules:
//! - `server_config` — configuration types (`MultiServerConfig`, `TLSConfig`, …)
//! - `shared_services` — `SharedServices` service composition root
//!
//! ## Architecture overview
//!
//! ```text
//! MultiServer::start()
//!     ↓ unified mode
//! TCP mux (port 5678) → REST on 127.0.0.1:15678 (HTTP/1.1)
//!                     → gRPC on 127.0.0.1:15679 (HTTP/2)
//!     ↓ multi-port mode
//! REST on :5678 | gRPC on :5679 | Arrow Flight on :5680 | pgwire on :5433
//! ```
//!
//! ## Architecture Overview:
//!
//! ProximaDB runs two independent servers for optimal protocol handling:
//! - **REST Server**: HTTP/1.1 on port 5678 for web clients
//! - **gRPC Server**: HTTP/2 on port 5679 for high-performance clients
//!
//! ## Design Philosophy:
//!
//! Separate servers provide:
//! - **Protocol Optimization**: Each server tuned for its protocol
//! - **Independent Scaling**: Scale REST and gRPC independently
//! - **Fault Isolation**: One server failure doesn't affect the other
//! - **Resource Control**: Separate thread pools and memory limits
//!
//! ## Lifecycle Management:
//!
//! ```text
//! MultiServer::start()
//!     ↓
//! Spawn REST Task → Axum Server (5678)
//!     ↓
//! Spawn gRPC Task → Tonic Server (5679)
//!     ↓
//! await shutdown_signal()
//!     ↓
//! Graceful Shutdown Both
//! ```
//!
//! ## TLS Configuration:
//!
//! Both servers can use TLS independently or share certificates:
//! - **Shared Mode**: Single cert/key pair for both servers
//! - **Split Mode**: Different certificates per protocol
//! - **Mixed Mode**: TLS on one, plaintext on other
//!
//! ## Performance Characteristics:
//!
//! | Protocol | Throughput | Latency | Use Case |
//! |----------|------------|---------|----------|
//! | REST | 840 QPS | 5-10ms | Web apps, simple queries |
//! | gRPC | 1,770 QPS | 2-5ms | High-volume, streaming |

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
#[cfg(feature = "cluster")]
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

#[cfg(feature = "cluster")]
use crate::cluster::consensus::RaftConsensus;
#[cfg(feature = "cluster")]
use crate::cluster::replication::EngineReplication;
#[cfg(feature = "cluster")]
use crate::cluster::rpc::grpc_server::{
    ConsensusServiceImpl, HealthServiceImpl, ReplicationServiceImpl,
};
#[cfg(feature = "cluster")]
use crate::proto::proximadb_cluster_v1::{
    consensus_service_server::ConsensusServiceServer, health_service_server::HealthServiceServer,
    replication_service_server::ReplicationServiceServer,
};

use crate::security::SecurityCoordinator;

// Server configuration types extracted to src/network/server_config.rs
// All existing call sites using `crate::network::multi_server::MultiServerConfig` etc continue to work.
#[cfg(feature = "cluster")]
pub use crate::network::server_config::ClusterServerConfig;
pub use crate::network::server_config::{
    ArrowIpcServerConfig, GrpcHttpServerConfig, MultiServerConfig, PostgresServerConfig,
    RestHttpServerConfig, ServerStatus, TLSConfig,
};

// SharedServices extracted to src/network/shared_services.rs
// All existing call sites using `crate::network::multi_server::SharedServices` continue to work.
pub use crate::network::shared_services::SharedServices;

/// Apply 64 MB message limits and optional gzip compression to a tonic service.
///
/// Defines a local `compress` binding that must be in scope at each use site.
macro_rules! apply_limits {
    ($svc:expr, $compress:expr) => {{
        use tonic::codec::CompressionEncoding;
        const MSG_64MB: usize = 64 * 1024 * 1024;
        let s = $svc
            .max_decoding_message_size(MSG_64MB)
            .max_encoding_message_size(MSG_64MB);
        if $compress {
            s.accept_compressed(CompressionEncoding::Gzip)
                .send_compressed(CompressionEncoding::Gzip)
        } else {
            s
        }
    }};
}

/// Multi-server manager that coordinates HTTP and gRPC servers with thin handlers
/// Responsibilities: ports, TLS, server lifecycle, protocol orchestration
pub struct MultiServer {
    config: MultiServerConfig,
    /// Shared services accessible for WAL recovery during startup
    pub shared_services: SharedServices,
    security_coordinator: Option<Arc<SecurityCoordinator>>,
    rest_auth_enabled: bool,
    server_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// LLM engine for semantic operations
    llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
}

impl MultiServer {
    /// Create new multi-server instance (orchestrator only)
    /// MultiServer focuses on network orchestration, SharedServices handles business logic
    pub fn new(
        config: MultiServerConfig,
        shared_services: SharedServices,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
        rest_auth_enabled: bool,
        llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
    ) -> Self {
        info!("🚀 MultiServer: Initializing network orchestrator");
        info!(
            "📡 MultiServer: gRPC port: {}, REST port: {}",
            config.grpc_config.port, config.http_config.port
        );
        info!("🔒 MultiServer: TLS enabled: {}", config.is_tls_enabled());

        Self {
            config,
            shared_services,
            security_coordinator,
            rest_auth_enabled,
            server_handles: Arc::new(Mutex::new(Vec::new())),
            llm_engine,
        }
    }

    async fn build_direct_pgwire_write_services(
        &self,
    ) -> Result<Option<crate::network::postgres::DirectPgwireWriteServices>> {
        if !self.config.postgres_config.enable_direct_record_writes {
            return Ok(None);
        }

        let wal_path = self
            .config
            .postgres_config
            .direct_record_wal_path
            .clone()
            .unwrap_or_else(|| {
                self.config
                    .data_dir
                    .join("pgwire")
                    .join("canonical-records.wal")
            });

        let wal_appender = Arc::new(
            crate::services::FramedTableWalAppender::open(&wal_path)
                .await
                .with_context(|| {
                    format!(
                        "opening pgwire direct canonical WAL at {}",
                        wal_path.display()
                    )
                })?,
        );
        let record_storage = Arc::new(crate::services::MemtableRecordStorage::new());
        let entries = wal_appender.read_entries().await.with_context(|| {
            format!(
                "reading pgwire direct canonical WAL at {}",
                wal_path.display()
            )
        })?;
        let summary = record_storage
            .replay_wal_entries(entries)
            .await
            .context("replaying pgwire direct canonical WAL into record memtable")?;

        info!(
            "🐘 pgwire direct record writes enabled: WAL={}, replayed_upserts={}, replayed_deletes={}, current_records={}",
            wal_path.display(),
            summary.upserts_replayed,
            summary.deletes_replayed,
            record_storage.len()
        );

        Ok(Some(
            crate::network::postgres::DirectPgwireWriteServices::new(record_storage, wal_appender),
        ))
    }

    /// Start all configured servers
    ///
    /// In unified mode: All protocols (REST, gRPC, Arrow Flight) on single port (default 5678)
    /// In legacy mode: gRPC on 5679, Arrow IPC on 5680, REST on 5678 (separate ports)
    pub async fn start(&mut self) -> Result<()> {
        // Check for unified mode (Phase 14)
        if self.config.is_unified_mode() {
            return self.start_unified().await;
        }

        // Legacy multi-port mode
        info!("🚀 Starting ProximaDB Multi-Server: gRPC:5679 + Arrow IPC:5680 + REST:5678");

        let services = self.shared_services.clone();
        let mut handles = Vec::new();

        // Holds port-backed service objects created inside the gRPC block so the
        // REST server (a sibling block) can also use them.
        let mut rest_ports_opt: Option<crate::network::rest::server::RestServerPorts> = None;

        // Start gRPC server on port 5679 if configured
        if self.config.grpc_config.enable_grpc {
            info!("🔗 Starting gRPC Server on port 5679");

            // Check TLS configuration early
            let tls_enabled = self.config.grpc_config.is_tls_enabled();
            let mtls_enabled = self.config.tls_config.is_mtls_enabled();
            let cert_path = self
                .config
                .grpc_config
                .tls_cert_file
                .clone()
                .or_else(|| self.config.tls_config.cert_file.clone());
            let key_path = self
                .config
                .grpc_config
                .tls_key_file
                .clone()
                .or_else(|| self.config.tls_config.key_file.clone());
            let ca_path = self.config.tls_config.ca_file.clone();

            // Create gRPC server builder with TLS if configured
            let mut server_builder = if tls_enabled || mtls_enabled {
                if let (Some(cert), Some(key)) = (&cert_path, &key_path) {
                    use tonic::transport::{Certificate, Identity, ServerTlsConfig};

                    // Load certificate and key
                    let cert_data = std::fs::read(cert)
                        .map_err(|e| anyhow::anyhow!("Failed to read TLS certificate: {}", e))?;
                    let key_data = std::fs::read(key)
                        .map_err(|e| anyhow::anyhow!("Failed to read TLS key: {}", e))?;

                    let identity = Identity::from_pem(cert_data, key_data);

                    // Build TLS config - with or without client CA for mTLS
                    let tls_config = if mtls_enabled {
                        if let Some(ref ca) = ca_path {
                            let ca_data = std::fs::read(ca).map_err(|e| {
                                anyhow::anyhow!("Failed to read CA certificate: {}", e)
                            })?;
                            let client_ca = Certificate::from_pem(ca_data);
                            info!("Configuring gRPC with mTLS (client certificates required)");
                            ServerTlsConfig::new()
                                .identity(identity)
                                .client_ca_root(client_ca)
                        } else {
                            warn!("mTLS enabled but no CA certificate - using standard TLS");
                            ServerTlsConfig::new().identity(identity)
                        }
                    } else {
                        info!("Configuring gRPC with TLS");
                        ServerTlsConfig::new().identity(identity)
                    };

                    tonic::transport::Server::builder()
                        .tls_config(tls_config)
                        .map_err(|e| anyhow::anyhow!("Failed to configure TLS: {}", e))?
                } else {
                    warn!("TLS enabled but certificate/key paths not configured - using plaintext");
                    tonic::transport::Server::builder()
                }
            } else {
                tonic::transport::Server::builder()
            };

            // ── Create backend services (doc + observability need data-dir paths) ──

            let doc_base_path = self.config.data_dir.join("documents");
            let doc_path_str = doc_base_path.to_string_lossy().to_string();
            let doc_storage_service = {
                let engine = services.vector_operations_service.unified_engine();
                match crate::storage::document::DocumentService::new_with_wal(engine, &doc_path_str)
                    .await
                {
                    Ok(svc) => Arc::new(svc),
                    Err(e) => {
                        warn!(
                            "Failed to create DocumentService with WAL: {}. Using non-durable storage.",
                            e
                        );
                        Arc::new(crate::storage::document::DocumentService::new(
                            services.vector_operations_service.unified_engine(),
                        ))
                    }
                }
            };

            let obs_base_path = self.config.data_dir.join("observability");
            let obs_path_str = obs_base_path.to_string_lossy().to_string();
            let obs_storage = match crate::observability::ObservabilityStorage::new_with_wal(
                &obs_path_str,
            )
            .await
            {
                Ok(storage) => Arc::new(storage),
                Err(e) => {
                    warn!(
                        "Failed to create ObservabilityStorage with WAL: {}. Using non-durable storage.",
                        e
                    );
                    Arc::new(crate::observability::ObservabilityStorage::new(
                        &obs_path_str,
                    ))
                }
            };
            let obs_service = match crate::observability::ObservabilityService::new(obs_storage)
                .await
            {
                Ok(svc) => Arc::new(svc),
                Err(e) => {
                    warn!(
                        "Failed to create ObservabilityService: {}. Creating minimal instance.",
                        e
                    );
                    let fallback_storage = Arc::new(
                        crate::observability::ObservabilityStorage::new(&obs_path_str),
                    );
                    match crate::observability::ObservabilityService::new(fallback_storage).await {
                        Ok(svc) => Arc::new(svc),
                        Err(fallback_err) => {
                            return Err(anyhow::anyhow!(
                                "Failed to create fallback observability service: {}",
                                fallback_err
                            ));
                        }
                    }
                }
            };

            // ── Wrap concrete impls as port objects for the factory ────────────

            let graph_port: Arc<dyn proximadb_runtime::GraphPort> =
                Arc::new(crate::network::grpc::GraphServiceImpl::with_adapter(
                    services.request_handlers.clone(),
                    services.query_adapter(),
                ));
            let doc_port: Arc<dyn proximadb_runtime::DocumentPort> = Arc::new(
                crate::network::grpc::DocumentServiceImpl::new(doc_storage_service),
            );
            let obs_port: Arc<dyn proximadb_runtime::ObservabilityPort> = Arc::new(
                crate::network::grpc::ObservabilityServiceImpl::new(obs_service),
            );

            let streaming_port: Arc<dyn proximadb_runtime::StreamingPort> =
                Arc::new(crate::network::grpc::StreamingServiceImpl::new());
            let security_port: Arc<dyn proximadb_runtime::SecurityPort> =
                Arc::new(crate::network::grpc::SecurityServiceImpl::with_default_config());
            let hybrid_port: Arc<dyn proximadb_runtime::HybridPort> =
                Arc::new(crate::network::grpc::HybridSearchServiceImpl::new());

            // Clone ports for REST server before they are consumed by the gRPC factory
            rest_ports_opt = Some(crate::network::rest::server::RestServerPorts {
                doc_port: doc_port.clone(),
                graph_port: graph_port.clone(),
                obs_port: obs_port.clone(),
                api_handlers: self.shared_services.api_handlers.clone(),
            });

            // ── Build all gRPC services through the port-based factory ─────────

            let api_port: Arc<dyn proximadb_runtime::ApiHandlersPort> =
                services.request_handlers.clone();
            let grpc_cfg = proximadb_api::grpc::builder::GrpcServiceConfig::default();
            let grpc_svcs = proximadb_api::grpc::builder::GrpcServiceFactory::new(api_port)
                .with_graph(graph_port)
                .with_document(doc_port)
                .with_observability(obs_port)
                .with_streaming(streaming_port)
                .with_security(security_port)
                .with_hybrid(hybrid_port)
                .with_config(grpc_cfg)
                .create_all_services_sync();

            debug!("✅ All gRPC services created via GrpcServiceFactory");

            // Apply 64 MB message limits and optional gzip compression per service.
            // Transport-level concerns (message size limits, gzip) applied here
            // at the composition root; the factory is protocol-agnostic.
            let compress = self.config.grpc_config.compression;

            let vector_service = apply_limits!(grpc_svcs.vector, compress);
            let sql_service = apply_limits!(grpc_svcs.sql, compress);
            let col_service = grpc_svcs.collection;
            let graph_service = grpc_svcs.graph;
            let hybrid_search_service = grpc_svcs.hybrid_search;
            let security_service = grpc_svcs.security;
            let document_service = grpc_svcs.document;
            let entity_service = grpc_svcs.entity;
            let observability_service = grpc_svcs.observability;
            let streaming_service = grpc_svcs.streaming;

            // Add V2 ProximaRecordService for typed fields and schema support
            let proxima_record_service_impl =
                crate::network::grpc::v2::ProximaRecordServiceImpl::new(
                    services.request_handlers.clone(),
                )
                .with_segment_registry(services.segment_registry.clone());
            let proxima_record_service = proxima_record_service_impl.into_server();

            // Build server with all services
            let server = server_builder
                .add_service(vector_service)
                .add_service(sql_service)
                .add_service(col_service)
                .add_service(graph_service)
                .add_service(hybrid_search_service)
                .add_service(security_service)
                .add_service(document_service)
                .add_service(entity_service)
                .add_service(observability_service)
                .add_service(streaming_service)
                .add_service(proxima_record_service);

            // Add reflection if enabled
            if self.config.grpc_config.enable_reflection {
                debug!("Adding gRPC reflection service");
                // Deferred: Add reflection service when descriptor binary is available
                // let file_descriptor_data = include_bytes!("../proto/proximadb_descriptor.bin");
                // server_builder = server_builder.add_service(
                //     tonic_reflection::server::Builder::configure()
                //         .register_encoded_file_descriptor_set(file_descriptor_data)
                //         .build()?,
                // );
            }

            let grpc_bind_addr = self.config.grpc_bind_address();

            // Determine the TLS mode for logging
            let mode = if mtls_enabled && cert_path.is_some() && ca_path.is_some() {
                "mTLS"
            } else if (tls_enabled || mtls_enabled) && cert_path.is_some() {
                "TLS"
            } else {
                "plaintext"
            };

            // Start the gRPC server (TLS already configured at builder level if needed)
            let grpc_handle = tokio::spawn(async move {
                if let Err(e) = server.serve(grpc_bind_addr).await {
                    tracing::error!("gRPC server error: {}", e);
                }
            });
            handles.push(grpc_handle);
            info!("gRPC Server started on {} ({})", grpc_bind_addr, mode);
        }

        // Start Arrow IPC (Flight) server on port 5680 if configured
        if self.config.arrow_ipc_config.enable_arrow_ipc {
            info!("🔗 Starting Arrow IPC Server on port 5680");

            let arrow_bind_addr = self.config.arrow_ipc_config.active_bind_address();
            let request_handlers = services.request_handlers.clone();
            let catalog_manager = services.catalog_manager.clone();
            let security_coordinator = if self.rest_auth_enabled {
                self.security_coordinator.clone()
            } else {
                None
            };
            let max_message_size = self.config.arrow_ipc_config.max_message_size;

            let arrow_handle = tokio::spawn(async move {
                use crate::network::arrow_ipc::ArrowFlightServer;

                match ArrowFlightServer::new(arrow_bind_addr, request_handlers)
                    .with_security_coordinator(security_coordinator)
                    .with_catalog_manager(Some(catalog_manager))
                    .with_max_message_size(max_message_size)
                    .start()
                    .await
                {
                    Ok(_) => {
                        info!("✅ Arrow IPC Server completed");
                    }
                    Err(e) => {
                        tracing::error!("❌ Arrow IPC Server error: {}", e);
                    }
                }
            });

            handles.push(arrow_handle);
            info!("✅ Arrow IPC Server started on {}", arrow_bind_addr);
        }

        // Start REST server on port 5678 if configured
        if self.config.http_config.enable_rest {
            info!("📡 Starting REST Server on port 5678");

            let rest_bind_addr = self.config.http_bind_address();
            let request_handlers = services.request_handlers.clone();
            let catalog_manager = services.catalog_manager.clone();
            let metrics_collector = services.metrics_collector.clone();
            let security_coordinator = self.security_coordinator.clone();
            let rest_auth_enabled = self.rest_auth_enabled;
            let data_dir = self.config.data_dir.clone();
            let query_adapter = Some(services.query_adapter());
            let graph_execution_service = services.graph_execution_service.clone();

            let api_config = self.config.api_config.clone();
            // Compression disabled by default (field doesn't exist in config)
            let enable_compression = false;
            let llm_engine = self.llm_engine.clone();
            let rest_handle = tokio::spawn(async move {
                use crate::network::rest::server::{RestServer, RestServerSecurityConfig};

                let max_request_size_mb = api_config.map(|c| c.max_request_size_mb);
                let mut rest_security = RestServerSecurityConfig::default();
                let auth_enabled = security_coordinator.is_some() && rest_auth_enabled;
                rest_security.auth.enabled = auth_enabled;

                // Pass port objects so document/graph/observability routes use
                // the port-backed handlers from proximadb-api.
                match RestServer::with_security_and_config_and_ports(
                    rest_bind_addr,
                    request_handlers,
                    graph_execution_service,
                    max_request_size_mb,
                    enable_compression,
                    metrics_collector,
                    security_coordinator,
                    rest_security,
                    data_dir,
                    query_adapter,
                    llm_engine,
                    rest_ports_opt,
                    Some(catalog_manager),
                )
                .start()
                .await
                {
                    Ok(_) => {
                        info!("✅ REST Server completed");
                    }
                    Err(e) => {
                        tracing::error!("❌ REST Server error: {}", e);
                    }
                }
            });

            handles.push(rest_handle);
            info!("✅ REST Server started on {}", rest_bind_addr);
        }

        // Start PostgreSQL wire protocol server on port 5433 if configured
        if self.config.postgres_config.enable_postgres {
            info!(
                "🐘 Starting PostgreSQL Server on port {}",
                self.config.postgres_config.port
            );

            let pg_bind_addr = self.config.postgres_config.active_bind_address();
            let collection_service = services.collection_service.clone();
            let vector_ops = services.vector_operations_service.clone();
            let catalog_manager = services.catalog_manager.clone();
            let document_service = Some(services.document_service.clone());
            let graph_service = Some(services.graph_service.clone());
            let observability_service = Some(services.observability_service.clone());
            let direct_write_services = self.build_direct_pgwire_write_services().await?;

            let postgres_handle = tokio::spawn(async move {
                use crate::network::postgres::PostgresServer;
                let mut server = PostgresServer::new(
                    pg_bind_addr,
                    collection_service,
                    vector_ops,
                    catalog_manager,
                    document_service,
                    graph_service,
                    observability_service,
                );
                if let Some(direct_write_services) = direct_write_services {
                    server = server.with_direct_write_services(direct_write_services);
                }
                if let Err(e) = server.start().await {
                    tracing::error!("❌ PostgreSQL Server error: {}", e);
                }
            });
            handles.push(postgres_handle);
            info!("✅ PostgreSQL Server started on {}", pg_bind_addr);
        }

        *self.server_handles.lock().await = handles;

        info!(
            "🎯 Multi-Server started successfully: gRPC:5679 + Arrow IPC:5680 + REST:5678 + PostgreSQL:5433"
        );
        Ok(())
    }

    /// Start unified server mode (Phase 14)
    ///
    /// All HTTP-based protocols (REST, gRPC, Arrow Flight) are served on a single port
    /// using TCP-level protocol detection:
    /// - HTTP/1.1 requests → REST server
    /// - HTTP/2 requests → gRPC server
    ///
    /// This approach works around http crate version incompatibilities between
    /// axum 0.6 (http 0.2) and tonic 0.14 (http 1.x) by routing at the TCP level.
    async fn start_unified(&mut self) -> Result<()> {
        let unified_addr = self.config.unified_bind_address();
        info!(
            "🚀 Starting ProximaDB Unified Server on {} (REST + gRPC + Arrow Flight via TCP multiplexing)",
            unified_addr
        );

        // Internal addresses for REST and gRPC servers
        // These are only accessible via the TCP multiplexer
        let internal_rest_addr: std::net::SocketAddr = "127.0.0.1:15678"
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 15678)));
        let internal_grpc_addr: std::net::SocketAddr = "127.0.0.1:15679"
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 15679)));

        let mut handles = Vec::new();

        // Create doc and observability backing services once; both REST and gRPC
        // use Arc clones so there is a single WAL-backed instance per service.
        let shared_services_ref = self.shared_services.clone();

        let doc_base_path = self.config.data_dir.join("documents");
        let doc_path_str = doc_base_path.to_string_lossy().to_string();
        let doc_storage_service: Arc<crate::storage::document::DocumentService> = {
            let engine = shared_services_ref
                .vector_operations_service
                .unified_engine();
            match crate::storage::document::DocumentService::new_with_wal(engine, &doc_path_str)
                .await
            {
                Ok(svc) => Arc::new(svc),
                Err(e) => {
                    warn!(
                        "Failed to create DocumentService with WAL: {}. Using non-durable storage.",
                        e
                    );
                    Arc::new(crate::storage::document::DocumentService::new(
                        shared_services_ref
                            .vector_operations_service
                            .unified_engine(),
                    ))
                }
            }
        };

        let obs_base_path = self.config.data_dir.join("observability");
        let obs_path_str = obs_base_path.to_string_lossy().to_string();
        let obs_storage =
            match crate::observability::ObservabilityStorage::new_with_wal(&obs_path_str).await {
                Ok(storage) => Arc::new(storage),
                Err(e) => {
                    warn!(
                        "Failed to create ObservabilityStorage with WAL: {}. Using non-durable.",
                        e
                    );
                    Arc::new(crate::observability::ObservabilityStorage::new(
                        &obs_path_str,
                    ))
                }
            };
        let obs_service: Arc<crate::observability::ObservabilityService> =
            match crate::observability::ObservabilityService::new(obs_storage).await {
                Ok(svc) => Arc::new(svc),
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to create ObservabilityService in unified mode: {}",
                        e
                    ));
                }
            };

        // Build REST port objects (each wraps its own ServiceImpl over a shared Arc).
        let rest_ports = {
            let services = self.shared_services.clone();
            let graph_port: Arc<dyn proximadb_runtime::GraphPort> =
                Arc::new(crate::network::grpc::GraphServiceImpl::with_adapter(
                    services.request_handlers.clone(),
                    services.query_adapter(),
                ));
            let doc_port: Arc<dyn proximadb_runtime::DocumentPort> = Arc::new(
                crate::network::grpc::DocumentServiceImpl::new(doc_storage_service.clone()),
            );
            let obs_port: Arc<dyn proximadb_runtime::ObservabilityPort> = Arc::new(
                crate::network::grpc::ObservabilityServiceImpl::new(obs_service.clone()),
            );
            crate::network::rest::server::RestServerPorts {
                doc_port,
                graph_port,
                obs_port,
                api_handlers: services.api_handlers.clone(),
            }
        };

        // 1. Start REST server on internal port (HTTP/1.1) using axum with port-backed handlers
        {
            let services = self.shared_services.clone();
            let request_handlers = services.request_handlers.clone();
            let graph_execution_service = services.graph_execution_service.clone();
            let metrics_collector = services.metrics_collector.clone();
            let security_coordinator = self.security_coordinator.clone();
            let data_dir = self.config.data_dir.clone();
            let query_adapter = Some(services.query_adapter());
            let llm_engine = self.llm_engine.clone();

            let router = crate::network::rest::server::RestServer::build_router_for_unified(
                request_handlers,
                graph_execution_service,
                metrics_collector,
                security_coordinator,
                data_dir,
                query_adapter,
                llm_engine,
                Some(rest_ports),
                Some(services.segment_registry.clone()),
                Some(services.catalog_manager.clone()),
            );

            info!(
                "🌐 REST Server starting on {} (internal, port-backed handlers)",
                internal_rest_addr
            );

            let handle = tokio::spawn(async move {
                if let Err(e) = axum::Server::bind(&internal_rest_addr)
                    .serve(router.into_make_service())
                    .await
                {
                    tracing::error!("Internal REST server error: {}", e);
                }
            });
            handles.push(handle);
        }

        // 2. Start gRPC server on internal port (HTTP/2)
        if self.config.grpc_config.enable_grpc {
            let services = self.shared_services.clone();
            let compress = self.config.grpc_config.compression;

            // ── Build core gRPC services via factory ──────────────────────────
            let graph_port: Arc<dyn proximadb_runtime::GraphPort> =
                Arc::new(crate::network::grpc::GraphServiceImpl::with_adapter(
                    services.request_handlers.clone(),
                    services.query_adapter(),
                ));
            let grpc_doc_port: Arc<dyn proximadb_runtime::DocumentPort> = Arc::new(
                crate::network::grpc::DocumentServiceImpl::new(doc_storage_service.clone()),
            );
            let grpc_obs_port: Arc<dyn proximadb_runtime::ObservabilityPort> = Arc::new(
                crate::network::grpc::ObservabilityServiceImpl::new(obs_service.clone()),
            );
            let grpc_streaming_port: Arc<dyn proximadb_runtime::StreamingPort> =
                Arc::new(crate::network::grpc::StreamingServiceImpl::new());
            let grpc_security_port: Arc<dyn proximadb_runtime::SecurityPort> =
                Arc::new(crate::network::grpc::SecurityServiceImpl::with_default_config());
            let hybrid_port: Arc<dyn proximadb_runtime::HybridPort> =
                Arc::new(crate::network::grpc::HybridSearchServiceImpl::new());
            let api_port: Arc<dyn proximadb_runtime::ApiHandlersPort> =
                services.request_handlers.clone();
            let grpc_cfg = proximadb_api::grpc::builder::GrpcServiceConfig::default();
            let grpc_svcs = proximadb_api::grpc::builder::GrpcServiceFactory::new(api_port)
                .with_graph(graph_port)
                .with_document(grpc_doc_port)
                .with_observability(grpc_obs_port)
                .with_streaming(grpc_streaming_port)
                .with_security(grpc_security_port)
                .with_hybrid(hybrid_port)
                .with_config(grpc_cfg)
                .create_all_services_sync();

            let vector_service = apply_limits!(grpc_svcs.vector, compress);
            let sql_service = apply_limits!(grpc_svcs.sql, compress);
            let col_service = grpc_svcs.collection;
            let graph_service = grpc_svcs.graph;
            let hybrid_search_service = grpc_svcs.hybrid_search;
            let security_service = grpc_svcs.security;

            // Arrow Flight service (HTTP/2-based, shares internal gRPC server)
            let flight_service = crate::network::arrow_ipc::service::ProximaFlightService::new(
                services.request_handlers.clone(),
            )
            .with_security_coordinator(if self.rest_auth_enabled {
                self.security_coordinator.clone()
            } else {
                None
            })
            .with_catalog_manager(Some(services.catalog_manager.clone()));
            let flight_server =
                arrow_flight::flight_service_server::FlightServiceServer::new(flight_service)
                    .max_encoding_message_size(512 * 1024 * 1024)
                    .max_decoding_message_size(512 * 1024 * 1024);

            let server = tonic::transport::Server::builder()
                .add_service(vector_service)
                .add_service(sql_service)
                .add_service(col_service)
                .add_service(graph_service)
                .add_service(hybrid_search_service)
                .add_service(security_service)
                .add_service(flight_server);

            info!(
                "🔗 gRPC + Arrow Flight Server starting on {} (internal)",
                internal_grpc_addr
            );

            let grpc_handle = tokio::spawn(async move {
                if let Err(e) = server.serve(internal_grpc_addr).await {
                    tracing::error!("Internal gRPC server error: {}", e);
                }
            });
            handles.push(grpc_handle);
        }

        // 3. Start TCP multiplexer on unified port (routes to internal servers)
        {
            use crate::network::multiplex::tcp_multiplexer::{
                TcpMultiplexConfig, TcpMultiplexer, TcpProtocol,
            };

            let multiplex_config = TcpMultiplexConfig {
                bind_address: unified_addr,
                rest_address: internal_rest_addr,
                grpc_address: internal_grpc_addr,
                max_connections: 10000,
                fallback_protocol: TcpProtocol::Http1, // Default to REST for unknown protocols
                proxy_buffer_size: 64 * 1024,
            };

            let multiplexer = TcpMultiplexer::with_config(multiplex_config);
            info!(
                "🎯 TCP Multiplexer starting on {} (routes HTTP/1.1 → REST, HTTP/2 → gRPC + Arrow Flight)",
                unified_addr
            );

            let multiplex_handle = tokio::spawn(async move {
                if let Err(e) = multiplexer.run().await {
                    tracing::error!("TCP multiplexer error: {}", e);
                }
            });
            handles.push(multiplex_handle);
        }

        // PostgreSQL is still on its own port (wire protocol is fundamentally different)
        if self.config.postgres_config.enable_postgres {
            info!(
                "🐘 Starting PostgreSQL Server on port {} (separate port for wire protocol)",
                self.config.postgres_config.port
            );

            let pg_bind_addr = self.config.postgres_config.active_bind_address();
            let services = self.shared_services.clone();
            let direct_write_services = self.build_direct_pgwire_write_services().await?;
            let postgres_handle = tokio::spawn(async move {
                use crate::network::postgres::PostgresServer;

                let mut server = PostgresServer::new(
                    pg_bind_addr,
                    services.collection_service.clone(),
                    services.vector_operations_service.clone(),
                    services.catalog_manager.clone(),
                    Some(services.document_service.clone()),
                    Some(services.graph_service.clone()),
                    Some(services.observability_service.clone()),
                );
                if let Some(direct_write_services) = direct_write_services {
                    server = server.with_direct_write_services(direct_write_services);
                }

                if let Err(e) = server.start().await {
                    tracing::error!("❌ PostgreSQL Server error: {}", e);
                }
            });
            handles.push(postgres_handle);
            info!("✅ PostgreSQL Server started on {}", pg_bind_addr);
        }

        *self.server_handles.lock().await = handles;

        info!(
            "✅ Unified Server started successfully on {} (REST + gRPC + Arrow Flight on single port via TCP multiplexing)",
            unified_addr
        );
        Ok(())
    }

    /// Start gRPC server with TLS configuration
    ///
    /// This helper method configures and starts the gRPC server with TLS/mTLS support.
    /// It loads certificates from PEM files and configures the server for secure communication.
    ///
    /// Note: To use TLS with gRPC, the server must be built with TLS from the start.
    /// This function provides a foundation for TLS configuration.
    #[allow(dead_code)]
    async fn start_grpc_with_tls_config(
        addr: SocketAddr,
        cert_path: &str,
        key_path: &str,
    ) -> Result<tonic::transport::Server> {
        use tonic::transport::{Identity, ServerTlsConfig};

        // Load certificate and key
        let cert = tokio::fs::read(cert_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read TLS certificate: {}", e))?;
        let key = tokio::fs::read(key_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read TLS key: {}", e))?;

        let identity = Identity::from_pem(cert, key);
        let tls_config = ServerTlsConfig::new().identity(identity);

        info!("Building gRPC server with TLS for {}", addr);
        info!("  Certificate: {}", cert_path);
        info!("  Private key: {}", key_path);

        let server = tonic::transport::Server::builder()
            .tls_config(tls_config)
            .map_err(|e| anyhow::anyhow!("Failed to configure TLS: {}", e))?;

        Ok(server)
    }

    /// Start gRPC server with full mTLS (mutual TLS) configuration
    ///
    /// This method configures bidirectional certificate verification where both
    /// the server and client present certificates for authentication.
    #[allow(dead_code)]
    async fn start_grpc_with_mtls_config(
        addr: SocketAddr,
        cert_path: &str,
        key_path: &str,
        ca_path: &str,
    ) -> Result<tonic::transport::Server> {
        use tonic::transport::{Certificate, Identity, ServerTlsConfig};

        // Load server certificate and key
        let cert = tokio::fs::read(cert_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read TLS certificate: {}", e))?;
        let key = tokio::fs::read(key_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read TLS key: {}", e))?;

        // Load CA certificate for client verification
        let ca_cert = tokio::fs::read(ca_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read CA certificate: {}", e))?;

        let identity = Identity::from_pem(cert, key);
        let client_ca = Certificate::from_pem(ca_cert);

        let tls_config = ServerTlsConfig::new()
            .identity(identity)
            .client_ca_root(client_ca);

        info!("Building gRPC server with mTLS for {}", addr);
        info!("  Server certificate: {}", cert_path);
        info!("  CA certificate: {}", ca_path);

        let server = tonic::transport::Server::builder()
            .tls_config(tls_config)
            .map_err(|e| anyhow::anyhow!("Failed to configure TLS: {}", e))?;

        Ok(server)
    }

    /// Start all configured servers with cluster services enabled
    ///
    /// This method extends the standard `start()` method by also registering
    /// cluster-specific gRPC services (Consensus, Replication, Health) when
    /// the `cluster` feature is enabled.
    ///
    /// # Arguments
    ///
    /// * `consensus` - The Raft consensus instance wrapped in Arc<RwLock>
    /// * `replication` - The replication manager instance wrapped in Arc<RwLock>
    /// * `node_id` - This node's unique identifier for health service
    ///
    /// # Feature Gate
    ///
    /// This method is only available when the `cluster` feature is enabled.
    #[cfg(feature = "cluster")]
    pub async fn start_with_cluster(
        &mut self,
        consensus: Arc<RwLock<RaftConsensus>>,
        replication: Arc<RwLock<EngineReplication>>,
        node_id: String,
    ) -> Result<()> {
        info!("Starting ProximaDB Multi-Server with Cluster Services");
        info!(
            "  Node ID: {}, Consensus: enabled, Replication: enabled, Health: enabled",
            node_id
        );

        // Get cluster config or use defaults
        let cluster_config =
            self.config
                .cluster_config
                .clone()
                .unwrap_or_else(|| ClusterServerConfig {
                    node_id: node_id.clone(),
                    enable_consensus: true,
                    enable_replication: true,
                    enable_health: true,
                });

        // Check for unified mode (Phase 14)
        if self.config.is_unified_mode() {
            // Deferred: Add cluster services to unified mode
            warn!("Cluster services in unified mode not yet implemented, using legacy mode");
        }

        // Start standard services first
        info!(
            "Starting ProximaDB Multi-Server: gRPC:5679 + Arrow IPC:5680 + REST:5678 + Cluster Services"
        );

        let services = self.shared_services.clone();
        let mut handles = Vec::new();

        // Start gRPC server with cluster services on port 5679
        if self.config.grpc_config.enable_grpc {
            info!("Starting gRPC Server on port 5679 with cluster services");

            // Check TLS configuration
            let tls_enabled = self.config.grpc_config.is_tls_enabled();
            let mtls_enabled = self.config.tls_config.is_mtls_enabled();
            let cert_path = self
                .config
                .grpc_config
                .tls_cert_file
                .clone()
                .or_else(|| self.config.tls_config.cert_file.clone());
            let key_path = self
                .config
                .grpc_config
                .tls_key_file
                .clone()
                .or_else(|| self.config.tls_config.key_file.clone());
            let ca_path = self.config.tls_config.ca_file.clone();

            // Create gRPC server builder with TLS if configured
            let mut server_builder = if tls_enabled || mtls_enabled {
                if let (Some(cert), Some(key)) = (&cert_path, &key_path) {
                    use tonic::transport::{Certificate, Identity, ServerTlsConfig};

                    let cert_data = std::fs::read(cert)
                        .map_err(|e| anyhow::anyhow!("Failed to read TLS certificate: {}", e))?;
                    let key_data = std::fs::read(key)
                        .map_err(|e| anyhow::anyhow!("Failed to read TLS key: {}", e))?;

                    let identity = Identity::from_pem(cert_data, key_data);

                    let tls_config = if mtls_enabled {
                        if let Some(ref ca) = ca_path {
                            let ca_data = std::fs::read(ca).map_err(|e| {
                                anyhow::anyhow!("Failed to read CA certificate: {}", e)
                            })?;
                            let client_ca = Certificate::from_pem(ca_data);
                            info!("Configuring gRPC with mTLS (client certificates required)");
                            ServerTlsConfig::new()
                                .identity(identity)
                                .client_ca_root(client_ca)
                        } else {
                            warn!("mTLS enabled but no CA certificate - using standard TLS");
                            ServerTlsConfig::new().identity(identity)
                        }
                    } else {
                        info!("Configuring gRPC with TLS");
                        ServerTlsConfig::new().identity(identity)
                    };

                    tonic::transport::Server::builder()
                        .tls_config(tls_config)
                        .map_err(|e| anyhow::anyhow!("Failed to configure TLS: {}", e))?
                } else {
                    warn!("TLS enabled but certificate/key paths not configured - using plaintext");
                    tonic::transport::Server::builder()
                }
            } else {
                tonic::transport::Server::builder()
            };

            // ── Build standard services via factory ───────────────────────────
            let graph_port: Arc<dyn proximadb_runtime::GraphPort> =
                Arc::new(crate::network::grpc::GraphServiceImpl::with_adapter(
                    services.request_handlers.clone(),
                    services.query_adapter(),
                ));
            let hybrid_port: Arc<dyn proximadb_runtime::HybridPort> =
                Arc::new(crate::network::grpc::HybridSearchServiceImpl::new());
            let api_port: Arc<dyn proximadb_runtime::ApiHandlersPort> =
                services.request_handlers.clone();
            let grpc_cfg = proximadb_api::grpc::builder::GrpcServiceConfig::default();
            let grpc_svcs = proximadb_api::grpc::builder::GrpcServiceFactory::new(api_port)
                .with_graph(graph_port)
                .with_hybrid(hybrid_port)
                .with_config(grpc_cfg)
                .create_all_services_sync();

            let compress = self.config.grpc_config.compression;

            let vector_service = apply_limits!(grpc_svcs.vector, compress);
            let sql_service = apply_limits!(grpc_svcs.sql, compress);
            let col_service = grpc_svcs.collection;
            let graph_service = grpc_svcs.graph;
            let hybrid_search_service = grpc_svcs.hybrid_search;
            let security_service = grpc_svcs.security;

            // Build cluster services
            let mut server = server_builder
                .add_service(vector_service)
                .add_service(sql_service)
                .add_service(col_service)
                .add_service(graph_service)
                .add_service(hybrid_search_service)
                .add_service(security_service);

            // Add consensus service if enabled
            if cluster_config.enable_consensus {
                let consensus_service = ConsensusServiceImpl::new(consensus.clone());
                let consensus_server = ConsensusServiceServer::new(consensus_service);
                server = server.add_service(consensus_server);
                info!("  ConsensusService registered");
            }

            // Add replication service if enabled
            if cluster_config.enable_replication {
                let replication_service = ReplicationServiceImpl::new(
                    replication.clone(),
                    cluster_config.node_id.clone(),
                );
                let replication_server = ReplicationServiceServer::new(replication_service);
                server = server.add_service(replication_server);
                info!("  ReplicationService registered");
            }

            // Add health service if enabled
            if cluster_config.enable_health {
                let health_service = HealthServiceImpl::with_consensus(
                    cluster_config.node_id.clone(),
                    consensus.clone(),
                );
                let health_server = HealthServiceServer::new(health_service);
                server = server.add_service(health_server);
                info!("  HealthService registered");
            }

            let grpc_bind_addr = self.config.grpc_bind_address();
            let mode = if mtls_enabled && cert_path.is_some() && ca_path.is_some() {
                "mTLS"
            } else if (tls_enabled || mtls_enabled) && cert_path.is_some() {
                "TLS"
            } else {
                "plaintext"
            };

            let grpc_handle = tokio::spawn(async move {
                if let Err(e) = server.serve(grpc_bind_addr).await {
                    tracing::error!("gRPC server error: {}", e);
                }
            });
            handles.push(grpc_handle);
            info!(
                "gRPC Server with cluster services started on {} ({})",
                grpc_bind_addr, mode
            );
        }

        // Start Arrow IPC server on port 5680 if configured
        if self.config.arrow_ipc_config.enable_arrow_ipc {
            info!("Starting Arrow IPC Server on port 5680");

            let arrow_bind_addr = self.config.arrow_ipc_config.active_bind_address();
            let request_handlers = services.request_handlers.clone();
            let catalog_manager = services.catalog_manager.clone();
            let security_coordinator = if self.rest_auth_enabled {
                self.security_coordinator.clone()
            } else {
                None
            };
            let max_message_size = self.config.arrow_ipc_config.max_message_size;

            let arrow_handle = tokio::spawn(async move {
                use crate::network::arrow_ipc::ArrowFlightServer;

                match ArrowFlightServer::new(arrow_bind_addr, request_handlers)
                    .with_security_coordinator(security_coordinator)
                    .with_catalog_manager(Some(catalog_manager))
                    .with_max_message_size(max_message_size)
                    .start()
                    .await
                {
                    Ok(_) => {
                        info!("Arrow IPC Server completed");
                    }
                    Err(e) => {
                        tracing::error!("Arrow IPC Server error: {}", e);
                    }
                }
            });

            handles.push(arrow_handle);
            info!("Arrow IPC Server started on {}", arrow_bind_addr);
        }

        // Start REST server on port 5678 if configured
        if self.config.http_config.enable_rest {
            info!("Starting REST Server on port 5678");

            let rest_bind_addr = self.config.http_bind_address();
            let request_handlers = services.request_handlers.clone();
            let metrics_collector = services.metrics_collector.clone();
            let security_coordinator = self.security_coordinator.clone();
            let rest_auth_enabled = self.rest_auth_enabled;
            let data_dir = self.config.data_dir.clone();
            let query_adapter = Some(services.query_adapter());
            let graph_execution_service = services.graph_execution_service.clone();
            let api_config = self.config.api_config.clone();

            let rest_handle = tokio::spawn(async move {
                use crate::network::rest::server::{RestServer, RestServerSecurityConfig};

                let max_request_size_mb = api_config.map(|c| c.max_request_size_mb);
                let mut rest_security = RestServerSecurityConfig::default();
                let auth_enabled = security_coordinator.is_some() && rest_auth_enabled;
                rest_security.auth.enabled = auth_enabled;

                match RestServer::with_security_and_config(
                    rest_bind_addr,
                    request_handlers,
                    graph_execution_service,
                    max_request_size_mb,
                    false, // compression disabled
                    metrics_collector,
                    security_coordinator,
                    rest_security,
                    data_dir,
                    query_adapter,
                    None,
                )
                .start()
                .await
                {
                    Ok(_) => {
                        info!("REST Server completed");
                    }
                    Err(e) => {
                        tracing::error!("REST Server error: {}", e);
                    }
                }
            });

            handles.push(rest_handle);
            info!("REST Server started on {}", rest_bind_addr);
        }

        *self.server_handles.lock().await = handles;

        info!(
            "Multi-Server with cluster services started successfully: gRPC:5679 + Arrow IPC:5680 + REST:5678"
        );
        Ok(())
    }

    /// Stop all servers
    pub async fn stop(&mut self) -> Result<()> {
        info!("🛑 Stopping ProximaDB Multi-Server");

        // NOTE: HTTP Server disabled - REST handlers removed
        // Stop HTTP server
        // if let Some(ref mut http_server) = self.http_server {
        //     http_server.stop().await?;
        //     info!("✅ HTTP Server stopped");
        // }

        // Stop all server handles
        let handles = std::mem::take(&mut *self.server_handles.lock().await);
        for handle in handles {
            handle.abort();
            let _ = handle.await;
        }

        info!("✅ All servers stopped");

        info!("🎯 All servers stopped successfully");
        Ok(())
    }

    /// Get server status
    pub async fn status(&self) -> ServerStatus {
        let handles = self.server_handles.lock().await;
        let servers_running = !handles.is_empty();

        ServerStatus {
            http_running: self.config.http_config.enable_rest && servers_running,
            grpc_running: self.config.grpc_config.enable_grpc && servers_running,
            http_address: Some(self.config.http_bind_address()),
            grpc_address: Some(self.config.grpc_bind_address()),
            tls_enabled: self.config.is_tls_enabled(),
        }
    }
}

// Deferred: Re-add TTL sweeper code in proper function context if needed
