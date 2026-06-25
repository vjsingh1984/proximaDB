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

// Server configuration types extracted to proximadb_runtime::bootstrap_config
// (Phase 9.9 / Task #70 pre-work). All existing call sites using
// `crate::network::multi_server::MultiServerConfig` etc continue to work via
// these re-exports — only the home crate changed.
#[cfg(feature = "cluster")]
pub use proximadb_runtime::bootstrap_config::ClusterServerConfig;
pub use proximadb_runtime::bootstrap_config::{
    ArrowIpcServerConfig, BindTarget, GrpcHttpServerConfig, MultiServerConfig,
    PostgresServerConfig, RestHttpServerConfig, ServerStatus, TLSConfig,
};

// SharedServices extracted to src/network/shared_services.rs
// All existing call sites using `crate::network::multi_server::SharedServices` continue to work.
pub use crate::network::shared_services::{ServiceProfile, SharedServices};

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
    rest_multi_tenant_required: bool,
    server_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// LLM engine for semantic operations
    llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
    /// Optional queue client threaded into REST `AppState` so the v3
    /// `/documents?mode=async` handler routes through the queue
    /// producer instead of falling back to inline embed. None when
    /// `PROXIMADB_QUEUE_ROOT` is unset.
    queue_client: Option<Arc<proximadb_queue::QueueClient>>,
}

impl MultiServer {
    /// Construct the always-on canonical v2 **record** gRPC service.
    ///
    /// Centralized so every server-startup entrypoint (`start`, `start_unified`,
    /// `start_with_cluster`) registers an identical canonical gRPC surface; see
    /// also [`Self::canonical_graph_grpc_service`]. Keeping the construction in
    /// one place means a wiring change (segment registry, primary-pod gate) can
    /// never drift between modes.
    fn canonical_record_grpc_service(
        services: &SharedServices,
    ) -> crate::proto::proximadb_v2::proxima_record_service_server::ProximaRecordServiceServer<
        crate::network::grpc::v2::ProximaRecordServiceImpl,
    > {
        crate::network::grpc::v2::ProximaRecordServiceImpl::new(services.request_handlers.clone())
            .with_segment_registry(services.segment_registry.clone())
            .with_primary_pod_gate(
                services.primary_pod_registry.clone(),
                services.self_pod_id.clone(),
            )
            .into_server()
    }

    /// Construct the always-on canonical v2 **graph** gRPC service
    /// (`proximadb.v2.ProximaGraphService`). Counterpart to
    /// [`Self::canonical_record_grpc_service`].
    fn canonical_graph_grpc_service(
        services: &SharedServices,
    ) -> crate::proto::proximadb_v2::proxima_graph_service_server::ProximaGraphServiceServer<
        crate::network::grpc::v2::ProximaGraphServiceImpl,
    > {
        crate::network::grpc::v2::ProximaGraphServiceImpl::new(
            services.request_handlers.graph_operations_service.clone(),
            Some(services.query_adapter.clone()),
        )
        .into_server()
    }

    /// Build the canonical v2 document gRPC service. Mirrors
    /// [`Self::canonical_graph_grpc_service`].
    fn canonical_document_grpc_service(
        services: &SharedServices,
    ) -> crate::proto::proximadb_v2::proxima_document_service_server::ProximaDocumentServiceServer<
        crate::network::grpc::v2::ProximaDocumentServiceImpl,
    >{
        crate::network::grpc::v2::ProximaDocumentServiceImpl::new(services.document_service.clone())
            .into_server()
    }

    /// Build the canonical v2 fusion gRPC service — the cross-modal retrieval
    /// surface over gRPC (`SEARCH_SURFACE_CONTRACT_2026_06_24.adoc`). A thin
    /// facade over the shared `FusionService` port; owns no ranking logic.
    fn canonical_fusion_grpc_service(
        services: &SharedServices,
    ) -> crate::proto::proximadb_v2::proxima_fusion_service_server::ProximaFusionServiceServer<
        crate::network::grpc::v2::ProximaFusionServiceImpl,
    > {
        crate::network::grpc::v2::ProximaFusionServiceImpl::new(services.request_handlers.clone())
            .into_server()
    }

    /// Build the canonical v2 entity gRPC service.
    ///
    /// EntityService is an orchestration facade over graph + vector + document,
    /// not a separate storage path (see SUPPORTED_SURFACES.adoc).
    fn canonical_entity_grpc_service(
        services: &SharedServices,
    ) -> crate::proto::proximadb_v2::proxima_entity_service_server::ProximaEntityServiceServer<
        crate::network::grpc::v2::ProximaEntityServiceImpl,
    > {
        crate::network::grpc::v2::ProximaEntityServiceImpl::new(services.request_handlers.clone())
            .into_server()
    }

    /// Create new multi-server instance (orchestrator only)
    /// MultiServer focuses on network orchestration, SharedServices handles business logic
    pub fn new(
        config: MultiServerConfig,
        shared_services: SharedServices,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
        rest_auth_enabled: bool,
        rest_multi_tenant_required: bool,
        llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
    ) -> Self {
        Self::new_with_queue_client(
            config,
            shared_services,
            security_coordinator,
            rest_auth_enabled,
            rest_multi_tenant_required,
            llm_engine,
            None,
        )
    }

    /// Variant of `new` that also receives the async-ingest queue
    /// client. Production startup uses this; legacy callers continue
    /// through `new` and pass `None`.
    pub fn new_with_queue_client(
        config: MultiServerConfig,
        shared_services: SharedServices,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
        rest_auth_enabled: bool,
        rest_multi_tenant_required: bool,
        llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
        queue_client: Option<Arc<proximadb_queue::QueueClient>>,
    ) -> Self {
        info!("🚀 MultiServer: Initializing network orchestrator");
        info!(
            "📡 MultiServer: gRPC port: {}, REST port: {}",
            config.grpc_config.port, config.http_config.port
        );
        info!("🔒 MultiServer: TLS enabled: {}", config.is_tls_enabled());
        if queue_client.is_some() {
            info!("📬 MultiServer: queue client threaded for async ingest");
        }

        Self {
            config,
            shared_services,
            security_coordinator,
            rest_auth_enabled,
            rest_multi_tenant_required,
            server_handles: Arc::new(Mutex::new(Vec::new())),
            llm_engine,
            queue_client,
        }
    }

    async fn build_direct_pgwire_write_services(
        &self,
    ) -> Result<Option<crate::network::postgres::DirectPgwireWriteServices>> {
        if !self.config.postgres_config.enable_direct_record_writes {
            return Ok(None);
        }

        let default_wal_path = self
            .config
            .data_dir
            .join("pgwire")
            .join("canonical-records.wal");
        let wal_path = self
            .config
            .postgres_config
            .direct_record_wal_path
            .clone()
            .unwrap_or_else(|| default_wal_path.clone());

        // Cross-surface unification: when the pgwire WAL path is the default, reuse the
        // SHARED canonical record store that `SharedServices` already built + WAL-recovered.
        // pgwire and the REST/gRPC `DmlService` then operate on ONE instance — a write on
        // any protocol is visible to reads + the CDC change-feed on every protocol — and the
        // WAL is replayed exactly once. A custom WAL path opts out (builds its own below).
        if wal_path == default_wal_path
            && let Some(store) = self.shared_services.canonical_record_store.clone()
        {
            info!(
                "🐘 pgwire reusing shared canonical record store (unified cross-surface relational state)"
            );
            return Ok(Some(
                crate::network::postgres::DirectPgwireWriteServices::new(store),
            ));
        }

        // T2.3 / TD-066 production wiring: prefer the shared canonical WAL
        // appender held on `SharedServices` so graph checkpoint emission
        // and pgwire direct writes share the same `next_sequence` counter
        // (avoids duplicate-sequence corruption from two independent
        // `FramedTableWalAppender::open` calls on the same file). Falls
        // back to opening a local appender when (a) the pgwire path is
        // overridden to a non-default location, or (b) `SharedServices`
        // wasn't constructed with an appender (e.g. opt_config was None).
        let wal_appender = if wal_path == default_wal_path {
            if let Some(shared) = self.shared_services.canonical_wal_appender.clone() {
                info!(
                    "🐘 pgwire reusing shared canonical WAL appender at {} (TD-066 wiring)",
                    wal_path.display()
                );
                shared
            } else {
                Arc::new(
                    crate::services::FramedTableWalAppender::open(&wal_path)
                        .await
                        .with_context(|| {
                            format!(
                                "opening pgwire direct canonical WAL at {} (no shared appender in SharedServices)",
                                wal_path.display()
                            )
                        })?,
                )
            }
        } else {
            Arc::new(
                crate::services::FramedTableWalAppender::open(&wal_path)
                    .await
                    .with_context(|| {
                        format!(
                            "opening pgwire direct canonical WAL at {} (custom path; cannot share)",
                            wal_path.display()
                        )
                    })?,
            )
        };
        // TD-064: the canonical store is per-(tenant, collection) partitioned and
        // shared across all pgwire connections. Build it once, recover its
        // partitions from the canonical WAL (routing each entry by its
        // tenant_id + collection_id), then hand the shared store down.
        let canonical_store = Arc::new(
            crate::services::record_store::DirectWalTableRecordStore::new_partitioned(
                wal_appender.clone(),
            ),
        );
        let entries = wal_appender.read_entries().await.with_context(|| {
            format!(
                "reading pgwire direct canonical WAL at {}",
                wal_path.display()
            )
        })?;
        let summary = canonical_store
            .replay_wal_entries(entries)
            .await
            .context("replaying pgwire direct canonical WAL into record partitions")?;

        info!(
            "🐘 pgwire direct record writes enabled: WAL={}, replayed_upserts={}, replayed_deletes={}",
            wal_path.display(),
            summary.upserts_replayed,
            summary.deletes_replayed,
        );

        Ok(Some(
            crate::network::postgres::DirectPgwireWriteServices::new(canonical_store),
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

        // TD-104 S1: always carry the runtime port-based `api_handlers` so the
        // REST server uses it for collection/vector dispatch even when gRPC is
        // disabled (the document/graph/observability route ports are filled in
        // below when the gRPC block builds them).
        let mut rest_ports_opt: Option<crate::network::rest::server::RestServerPorts> =
            Some(crate::network::rest::server::RestServerPorts {
                doc_port: None,
                graph_port: None,
                obs_port: None,
                api_handlers: services.api_handlers.clone(),
            });

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
            let server_builder = if tls_enabled || mtls_enabled {
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
            let mut server_builder =
                server_builder.layer(tower::util::option_layer(if self.rest_auth_enabled {
                    self.security_coordinator
                        .clone()
                        .map(crate::network::grpc::auth::GrpcAuthLayer::new)
                } else {
                    None
                }));

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
            // ADR-015 step 2: bare DocumentService impls DocumentPort directly;
            // the gRPC DocumentServiceImpl wrapper is no longer in the port chain.
            let doc_port: Arc<dyn proximadb_runtime::DocumentPort> = doc_storage_service.clone();
            let obs_port: Arc<dyn proximadb_runtime::ObservabilityPort> = Arc::new(
                crate::network::grpc::ObservabilityServiceImpl::new(obs_service),
            );

            let streaming_port: Arc<dyn proximadb_runtime::StreamingPort> =
                Arc::new(crate::network::grpc::StreamingServiceImpl::new());
            let security_port: Arc<dyn proximadb_runtime::SecurityPort> =
                Arc::new(crate::network::grpc::SecurityServiceImpl::with_default_config());
            let hybrid_port: Arc<dyn proximadb_runtime::HybridPort> =
                Arc::new(crate::network::hybrid_search::RestHybridPortImpl::new(
                    self.shared_services.vector_ops_port.clone(),
                    self.shared_services.fulltext_indexes.clone(),
                ));

            // Clone ports for REST server before they are consumed by the gRPC factory
            rest_ports_opt = Some(crate::network::rest::server::RestServerPorts {
                doc_port: Some(doc_port.clone()),
                graph_port: Some(graph_port.clone()),
                obs_port: Some(obs_port.clone()),
                api_handlers: self.shared_services.api_handlers.clone(),
            });

            // ── Build all gRPC services through the port-based factory ─────────

            // TD-104 S2: gRPC's API service consumes only the ApiHandlersPort
            // trait (collection/vector/hybrid/sql); route it through the runtime
            // port-based handler instead of the root inherent one. Document/graph/
            // observability RPCs already use their own ports.
            let api_port: Arc<dyn proximadb_runtime::ApiHandlersPort> =
                services.api_handlers.clone();
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

            // Standard grpc.health.v1.Health service for k8s/LB probes.
            let (health_reporter, standard_health_server) = tonic_health::server::health_reporter();
            health_reporter
                .set_service_status("", tonic_health::ServingStatus::Serving)
                .await;

            // Canonical v2 surfaces are always registered: ProximaRecordService +
            // ProximaGraphService + ProximaEntityService + grpc.health.v1.Health
            // (+ optional reflection below). Service construction is centralized in the
            // `canonical_*_grpc_service` helpers so all startup modes match.
            let mut server = server_builder
                .add_service(Self::canonical_record_grpc_service(&services))
                .add_service(Self::canonical_graph_grpc_service(&services))
                .add_service(Self::canonical_document_grpc_service(&services))
                .add_service(Self::canonical_fusion_grpc_service(&services))
                .add_service(Self::canonical_entity_grpc_service(&services))
                .add_service(standard_health_server);

            // Deprecated gRPC v1 compatibility adapters are gated behind
            // `enable_grpc_v1_compat` (env `PROXIMADB_GRPC_V1_COMPAT`, default off).
            // Post-sunset these service impls are removed entirely.
            if self.config.grpc_config.enable_grpc_v1_compat {
                warn!(
                    "gRPC v1 services are registered as deprecated compatibility adapters; use proximadb.v2.ProximaRecordService for record writes/search"
                );
                server = server
                    .add_service(vector_service)
                    .add_service(sql_service)
                    .add_service(col_service)
                    .add_service(graph_service)
                    .add_service(hybrid_search_service)
                    .add_service(security_service)
                    .add_service(document_service)
                    .add_service(entity_service)
                    .add_service(observability_service)
                    .add_service(streaming_service);
            }

            // Optional grpc.reflection.v1.ServerReflection for runtime discovery.
            if self.config.grpc_config.enable_reflection {
                debug!("Adding gRPC reflection service");
                let reflection_service = build_grpc_reflection_service()?;
                server = server.add_service(reflection_service);
            }

            let grpc_bind_target = self.config.grpc_bind_target();

            // Determine the TLS mode for logging
            let mode = if mtls_enabled && cert_path.is_some() && ca_path.is_some() {
                "mTLS"
            } else if (tls_enabled || mtls_enabled) && cert_path.is_some() {
                "TLS"
            } else {
                "plaintext"
            };

            // Start the gRPC server (TLS already configured at builder level if needed).
            // TCP in server mode; a Unix-domain socket in portless embedded mode.
            let grpc_target_log = format!("{grpc_bind_target:?}");
            let grpc_handle = tokio::spawn(async move {
                let result = match grpc_bind_target {
                    BindTarget::Tcp(addr) => server.serve(addr).await,
                    BindTarget::Uds(path) => match crate::network::uds::bind_unix_listener(&path) {
                        Ok(listener) => {
                            let incoming =
                                tokio_stream::wrappers::UnixListenerStream::new(listener);
                            server.serve_with_incoming(incoming).await
                        }
                        Err(e) => {
                            tracing::error!("gRPC UDS bind failed at {}: {}", path.display(), e);
                            return;
                        }
                    },
                };
                if let Err(e) = result {
                    tracing::error!("gRPC server error: {}", e);
                }
            });
            handles.push(grpc_handle);
            info!("gRPC Server started on {} ({})", grpc_target_log, mode);
        }

        // Start Arrow IPC (Flight) server on port 5680 if configured
        if self.config.arrow_ipc_config.enable_arrow_ipc {
            info!("🔗 Starting Arrow IPC Server on port 5680");

            let arrow_bind_target = self.config.arrow_bind_target();
            let arrow_target_log = format!("{arrow_bind_target:?}");
            let request_handlers = services.request_handlers.clone();
            let catalog_manager = services.catalog_manager.clone();
            let security_coordinator = if self.rest_auth_enabled {
                self.security_coordinator.clone()
            } else {
                None
            };
            let max_message_size = self.config.arrow_ipc_config.max_message_size;
            // Slice 6.2 capture: same `Arc<PrimaryPodRegistry>` /
            // pod-id pair the REST and gRPC v2 surfaces hold, so all
            // three see identical routing decisions.
            let primary_pod_registry = services.primary_pod_registry.clone();
            let self_pod_id = services.self_pod_id.clone();

            let arrow_handle = tokio::spawn(async move {
                use crate::network::arrow_ipc::{ArrowFlightServer, service::ProximaFlightService};

                let flight_service = ProximaFlightService::from_unified_handlers(request_handlers);
                match ArrowFlightServer::new(arrow_bind_target, flight_service)
                    .with_security_coordinator(security_coordinator)
                    .with_catalog_manager(Some(catalog_manager))
                    .with_max_message_size(max_message_size)
                    .with_primary_pod_gate(primary_pod_registry, self_pod_id)
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
            info!("✅ Arrow IPC Server started on {}", arrow_target_log);
        }

        // Start REST server on port 5678 if configured
        if self.config.http_config.enable_rest {
            info!("📡 Starting REST Server on port 5678");

            let rest_bind_addr = self.config.http_bind_address();
            // Portless mode: serve REST over a Unix-domain socket. `/health`
            // stays reachable over it (the SDK readiness probe does an HTTP GET).
            let rest_uds_path = match self.config.rest_bind_target() {
                BindTarget::Uds(p) => Some(p),
                BindTarget::Tcp(_) => None,
            };
            let rest_target_log = rest_uds_path
                .as_ref()
                .map(|p| format!("unix:{}", p.display()))
                .unwrap_or_else(|| rest_bind_addr.to_string());
            let request_handlers = services.request_handlers.clone();
            let catalog_manager = services.catalog_manager.clone();
            let metrics_collector = services.metrics_collector.clone();
            let security_coordinator = self.security_coordinator.clone();
            let rest_auth_enabled = self.rest_auth_enabled;
            let rest_multi_tenant_required = self.rest_multi_tenant_required;
            let data_dir = self.config.data_dir.clone();
            let query_adapter = Some(services.query_adapter());
            let graph_execution_service = services.graph_execution_service.clone();

            let api_config = self.config.api_config.clone();
            // Compression disabled by default (field doesn't exist in config)
            let enable_compression = false;
            let llm_engine = self.llm_engine.clone();
            // Clone the queue client *before* the tokio::spawn so the spawned
            // task doesn't borrow `self` across the `'static` boundary. The
            // queue_client value is itself an Option<Arc<...>> so cloning is
            // cheap and preserves the same handle for the spawned task.
            let queue_client_for_rest = self.queue_client.clone();
            // T3.2 Slice 1b: same pattern — share the full-text index map with
            // gRPC's hybrid_search wiring (`HybridFullTextIndexMap` is an
            // `Arc<RwLock<...>>` so the clone is cheap and the spawned task
            // gets the same shared handle).
            let fulltext_indexes_for_rest = self.shared_services.fulltext_indexes.clone();
            let discovery_service_for_rest = self.shared_services.discovery_service.clone();
            let external_collection_service_for_rest =
                self.shared_services.external_collection_service.clone();
            let rest_handle = tokio::spawn(async move {
                use crate::network::rest::server::{RestServer, RestServerSecurityConfig};

                let max_request_size_mb = api_config.map(|c| c.max_request_size_mb);
                let auth_enabled = security_coordinator.is_some() && rest_auth_enabled;
                let mut rest_security = if auth_enabled && rest_multi_tenant_required {
                    RestServerSecurityConfig::multi_tenant()
                } else {
                    RestServerSecurityConfig::default()
                };
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
                    queue_client_for_rest,
                    Some(fulltext_indexes_for_rest),
                    Some(discovery_service_for_rest),
                    Some(external_collection_service_for_rest),
                )
                .with_uds_path(rest_uds_path)
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
            info!("✅ REST Server started on {}", rest_target_log);
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
            let rank_services = services.rank_services.clone();
            let rank_profile_store = services.rank_profile_store.clone();
            let function_store = services.function_store.clone();
            let direct_write_services = self.build_direct_pgwire_write_services().await?;
            // Slice 6.3 capture: clone the same registry + pod id
            // pair the REST / gRPC v2 / Arrow Flight surfaces hold.
            let primary_pod_registry = services.primary_pod_registry.clone();
            let self_pod_id = services.self_pod_id.clone();
            // Warehouse object-store root for `ALTER TABLE … MATERIALIZE`, derived from
            // the server data dir the same way document/observability roots are.
            let warehouse_root_url = format!("file://{}/warehouse", self.config.data_dir.display());

            // E0: env-gated per-IP rate limiter for the pgwire query path
            // (PROXIMADB_PGWIRE_RATE_LIMIT_RPM=<n>). Unset/0 → no limiting, so
            // default behavior is unchanged. Uses the converged RateLimitState
            // the REST middleware also checks against.
            let pgwire_rate_limiter = std::env::var("PROXIMADB_PGWIRE_RATE_LIMIT_RPM")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|rpm| *rpm > 0)
                .map(|rpm| {
                    std::sync::Arc::new(
                        crate::network::middleware::rate_limit::RateLimitState::new(
                            crate::network::middleware::RateLimitConfig::production(rpm, rpm)
                                .to_middleware_config(),
                        ),
                    )
                });

            let postgres_handle = tokio::spawn(async move {
                use crate::network::postgres::PostgresServer;
                let mut server = PostgresServer::new(
                    pg_bind_addr,
                    collection_service as std::sync::Arc<dyn proximadb_runtime::CollectionPort>,
                    vector_ops,
                    catalog_manager,
                    document_service,
                    graph_service,
                    observability_service,
                );
                if let Some(direct_write_services) = direct_write_services {
                    server = server.with_direct_write_services(direct_write_services);
                }
                server =
                    server.with_rank_pipeline(rank_services, rank_profile_store, function_store);
                server = server.with_primary_pod_gate(primary_pod_registry, self_pod_id);
                server = server.with_warehouse_materialization(warehouse_root_url);
                if let Some(limiter) = pgwire_rate_limiter {
                    server = server.with_rate_limiter(limiter);
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
            // ADR-015 step 2 (DocumentPort).
            let doc_port: Arc<dyn proximadb_runtime::DocumentPort> = doc_storage_service.clone();
            let obs_port: Arc<dyn proximadb_runtime::ObservabilityPort> = Arc::new(
                crate::network::grpc::ObservabilityServiceImpl::new(obs_service.clone()),
            );
            crate::network::rest::server::RestServerPorts {
                doc_port: Some(doc_port),
                graph_port: Some(graph_port),
                obs_port: Some(obs_port),
                api_handlers: services.api_handlers.clone(),
            }
        };

        // 1. Start REST server on internal port (HTTP/1.1) using axum with port-backed handlers
        {
            let services = self.shared_services.clone();
            let request_handlers = services.request_handlers.clone();
            let graph_execution_service = services.graph_execution_service.clone();
            let metrics_collector = services.metrics_collector.clone();
            // Mirror the Arrow IPC gate above: pass the coordinator only when
            // REST auth is enabled in config. `build_router_for_unified` reads
            // Some/None as the auth-attach signal — convergent with the
            // multi-port `start_with_security` path which gates on
            // `security_config.auth.enabled` (server.rs line 510).
            let security_coordinator = if self.rest_auth_enabled {
                self.security_coordinator.clone()
            } else {
                None
            };
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
                self.queue_client.clone(),
                Some(services.fulltext_indexes.clone()),
                Some(services.recall_probe_gate.clone()),
                Some(services.rank_services.clone()),
                Some(services.rank_profile_store.clone()),
                Some(services.discovery_service.clone()),
                Some(services.external_collection_service.clone()),
            );

            info!(
                "🌐 REST Server starting on {} (internal, port-backed handlers)",
                internal_rest_addr
            );

            let handle = tokio::spawn(async move {
                // axum 0.8 / hyper 1.0: bind a tokio listener, then axum::serve.
                match tokio::net::TcpListener::bind(&internal_rest_addr).await {
                    Ok(listener) => {
                        if let Err(e) = axum::serve(listener, router.into_make_service()).await {
                            tracing::error!("Internal REST server error: {}", e);
                        }
                    }
                    Err(e) => tracing::error!("Internal REST bind error: {}", e),
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
            // ADR-015 step 2 (DocumentPort).
            let grpc_doc_port: Arc<dyn proximadb_runtime::DocumentPort> =
                doc_storage_service.clone();
            let grpc_obs_port: Arc<dyn proximadb_runtime::ObservabilityPort> = Arc::new(
                crate::network::grpc::ObservabilityServiceImpl::new(obs_service.clone()),
            );
            let grpc_streaming_port: Arc<dyn proximadb_runtime::StreamingPort> =
                Arc::new(crate::network::grpc::StreamingServiceImpl::new());
            let grpc_security_port: Arc<dyn proximadb_runtime::SecurityPort> =
                Arc::new(crate::network::grpc::SecurityServiceImpl::with_default_config());
            let hybrid_port: Arc<dyn proximadb_runtime::HybridPort> =
                Arc::new(crate::network::hybrid_search::RestHybridPortImpl::new(
                    self.shared_services.vector_ops_port.clone(),
                    self.shared_services.fulltext_indexes.clone(),
                ));
            // TD-104 S2: gRPC's API service consumes only the ApiHandlersPort
            // trait (collection/vector/hybrid/sql); route it through the runtime
            // port-based handler instead of the root inherent one. Document/graph/
            // observability RPCs already use their own ports.
            let api_port: Arc<dyn proximadb_runtime::ApiHandlersPort> =
                services.api_handlers.clone();
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
            let flight_service =
                crate::network::arrow_ipc::service::ProximaFlightService::from_unified_handlers(
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

            let mut server_builder = tonic::transport::Server::builder().layer(
                tower::util::option_layer(if self.rest_auth_enabled {
                    self.security_coordinator
                        .clone()
                        .map(crate::network::grpc::auth::GrpcAuthLayer::new)
                } else {
                    None
                }),
            );

            // Standard grpc.health.v1.Health service for k8s/LB probes.
            let (health_reporter, standard_health_server) = tonic_health::server::health_reporter();
            health_reporter
                .set_service_status("", tonic_health::ServingStatus::Serving)
                .await;

            // Canonical surfaces always on: proximadb.v2.ProximaRecordService +
            // ProximaGraphService + ProximaEntityService + ProximaDocumentService,
            // Arrow Flight, and grpc.health.v1.Health (+
            // optional reflection below). Construction centralized in the
            // `canonical_*_grpc_service` helpers.
            let mut server = server_builder
                .add_service(Self::canonical_record_grpc_service(&services))
                .add_service(Self::canonical_graph_grpc_service(&services))
                .add_service(Self::canonical_document_grpc_service(&services))
                .add_service(Self::canonical_fusion_grpc_service(&services))
                .add_service(Self::canonical_entity_grpc_service(&services))
                .add_service(flight_server)
                .add_service(standard_health_server);

            // Deprecated gRPC v1 compatibility adapters are gated behind
            // `enable_grpc_v1_compat` (env `PROXIMADB_GRPC_V1_COMPAT`, default off).
            if self.config.grpc_config.enable_grpc_v1_compat {
                warn!(
                    "gRPC v1 services are registered as deprecated compatibility adapters; use proximadb.v2.ProximaRecordService for record writes/search"
                );
                server = server
                    .add_service(vector_service)
                    .add_service(sql_service)
                    .add_service(col_service)
                    .add_service(graph_service)
                    .add_service(hybrid_search_service)
                    .add_service(security_service);
            }

            // Optional grpc.reflection.v1.ServerReflection for runtime discovery.
            if self.config.grpc_config.enable_reflection {
                debug!("Adding gRPC reflection service (unified mode)");
                let reflection_service = build_grpc_reflection_service()?;
                server = server.add_service(reflection_service);
            }

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
            let warehouse_root_url = format!("file://{}/warehouse", self.config.data_dir.display());
            let postgres_handle = tokio::spawn(async move {
                use crate::network::postgres::PostgresServer;

                let mut server = PostgresServer::new(
                    pg_bind_addr,
                    services.collection_service.clone()
                        as std::sync::Arc<dyn proximadb_runtime::CollectionPort>,
                    services.vector_operations_service.clone(),
                    services.catalog_manager.clone(),
                    Some(services.document_service.clone()),
                    Some(services.graph_service.clone()),
                    Some(services.observability_service.clone()),
                );
                if let Some(direct_write_services) = direct_write_services {
                    server = server.with_direct_write_services(direct_write_services);
                }
                server = server.with_rank_pipeline(
                    services.rank_services.clone(),
                    services.rank_profile_store.clone(),
                    services.function_store.clone(),
                );
                // Slice 6.3: same gate the REST / gRPC v2 / Arrow
                // Flight surfaces hold — pgwire DML uses the
                // identical registry.
                server = server.with_primary_pod_gate(
                    services.primary_pod_registry.clone(),
                    services.self_pod_id.clone(),
                );
                server = server.with_warehouse_materialization(warehouse_root_url);

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
            let server_builder = if tls_enabled || mtls_enabled {
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
            let mut server_builder =
                server_builder.layer(tower::util::option_layer(if self.rest_auth_enabled {
                    self.security_coordinator
                        .clone()
                        .map(crate::network::grpc::auth::GrpcAuthLayer::new)
                } else {
                    None
                }));

            // ── Build standard services via factory ───────────────────────────
            let graph_port: Arc<dyn proximadb_runtime::GraphPort> =
                Arc::new(crate::network::grpc::GraphServiceImpl::with_adapter(
                    services.request_handlers.clone(),
                    services.query_adapter(),
                ));
            let hybrid_port: Arc<dyn proximadb_runtime::HybridPort> =
                Arc::new(crate::network::hybrid_search::RestHybridPortImpl::new(
                    self.shared_services.vector_ops_port.clone(),
                    self.shared_services.fulltext_indexes.clone(),
                ));
            // TD-104 S2: gRPC's API service consumes only the ApiHandlersPort
            // trait (collection/vector/hybrid/sql); route it through the runtime
            // port-based handler instead of the root inherent one. Document/graph/
            // observability RPCs already use their own ports.
            let api_port: Arc<dyn proximadb_runtime::ApiHandlersPort> =
                services.api_handlers.clone();
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

            // Standard grpc.health.v1.Health service for k8s/LB probes.
            let (mut std_health_reporter, standard_health_server) =
                tonic_health::server::health_reporter();
            std_health_reporter
                .set_service_status("", tonic_health::ServingStatus::Serving)
                .await;

            // Canonical v2 surfaces always on: ProximaRecordService +
            // ProximaGraphService + ProximaEntityService + ProximaDocumentService
            // + grpc.health.v1.Health (+ optional reflection /
            // cluster services below). Construction centralized in the
            // `canonical_*_grpc_service` helpers.
            let mut server = server_builder
                .add_service(Self::canonical_record_grpc_service(&services))
                .add_service(Self::canonical_graph_grpc_service(&services))
                .add_service(Self::canonical_document_grpc_service(&services))
                .add_service(Self::canonical_fusion_grpc_service(&services))
                .add_service(Self::canonical_entity_grpc_service(&services))
                .add_service(standard_health_server);

            // Deprecated gRPC v1 compatibility adapters are gated behind
            // `enable_grpc_v1_compat` (env `PROXIMADB_GRPC_V1_COMPAT`, default off).
            if self.config.grpc_config.enable_grpc_v1_compat {
                warn!(
                    "gRPC v1 services are registered as deprecated compatibility adapters; use proximadb.v2.ProximaRecordService for record writes/search"
                );
                server = server
                    .add_service(vector_service)
                    .add_service(sql_service)
                    .add_service(col_service)
                    .add_service(graph_service)
                    .add_service(hybrid_search_service)
                    .add_service(security_service);
            }

            // Optional grpc.reflection.v1.ServerReflection for runtime discovery.
            if self.config.grpc_config.enable_reflection {
                debug!("Adding gRPC reflection service (cluster mode)");
                let reflection_service = build_grpc_reflection_service()?;
                server = server.add_service(reflection_service);
            }

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

            // Cluster mode is TCP-only; portless (UDS) is a single-node embedded
            // path. Wrap the addr as a TCP bind target to match the generalized
            // `ArrowFlightServer::new` signature.
            let arrow_bind_target =
                BindTarget::Tcp(self.config.arrow_ipc_config.active_bind_address());
            let request_handlers = services.request_handlers.clone();
            let catalog_manager = services.catalog_manager.clone();
            let security_coordinator = if self.rest_auth_enabled {
                self.security_coordinator.clone()
            } else {
                None
            };
            let max_message_size = self.config.arrow_ipc_config.max_message_size;
            // Slice 6.2 capture: same `Arc<PrimaryPodRegistry>` /
            // pod-id pair the REST and gRPC v2 surfaces hold, so all
            // three see identical routing decisions.
            let primary_pod_registry = services.primary_pod_registry.clone();
            let self_pod_id = services.self_pod_id.clone();

            let arrow_handle = tokio::spawn(async move {
                use crate::network::arrow_ipc::{ArrowFlightServer, service::ProximaFlightService};

                let flight_service = ProximaFlightService::from_unified_handlers(request_handlers);
                match ArrowFlightServer::new(arrow_bind_target, flight_service)
                    .with_security_coordinator(security_coordinator)
                    .with_catalog_manager(Some(catalog_manager))
                    .with_max_message_size(max_message_size)
                    .with_primary_pod_gate(primary_pod_registry, self_pod_id)
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
            let rest_multi_tenant_required = self.rest_multi_tenant_required;
            let data_dir = self.config.data_dir.clone();
            let query_adapter = Some(services.query_adapter());
            let graph_execution_service = services.graph_execution_service.clone();
            let api_config = self.config.api_config.clone();
            // TD-104 item 2(a): wire the runtime port-based handler into the
            // cluster REST boot so collection/vector dispatch reaches
            // `api_handlers` instead of falling back to the concrete root handler
            // at rest/v1/handlers.rs:1337. `services.api_handlers` is the same
            // runtime handler the unified-port path uses (shared_services.rs).
            // Only `api_handlers` is wired here (doc/graph/obs route ports stay
            // `None`, preserving the current cluster route surface) — a
            // same-trait swap, behavior-preserving for every other route.
            let api_handlers_port = services.api_handlers.clone();

            let rest_handle = tokio::spawn(async move {
                use crate::network::rest::server::{
                    RestServer, RestServerPorts, RestServerSecurityConfig,
                };

                let max_request_size_mb = api_config.map(|c| c.max_request_size_mb);
                let auth_enabled = security_coordinator.is_some() && rest_auth_enabled;
                let mut rest_security = if auth_enabled && rest_multi_tenant_required {
                    RestServerSecurityConfig::multi_tenant()
                } else {
                    RestServerSecurityConfig::default()
                };
                rest_security.auth.enabled = auth_enabled;

                let rest_ports = RestServerPorts {
                    doc_port: None,
                    graph_port: None,
                    obs_port: None,
                    api_handlers: api_handlers_port,
                };

                match RestServer::with_security_and_config_and_ports(
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
                    None, // llm_engine
                    Some(rest_ports),
                    None, // catalog_manager (unchanged from prior cluster boot)
                    None, // queue_client
                    None, // fulltext_indexes
                    None, // discovery_service
                    None, // external_collection_service
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

/// Build a gRPC server-reflection service (grpc.reflection.v1.ServerReflection)
/// pre-loaded with the checked-in ProximaDB file descriptor sets so standard
/// gRPC clients (grpcurl, Postman, etc.) can introspect services at runtime.
fn build_grpc_reflection_service() -> Result<
    tonic_reflection::server::v1::ServerReflectionServer<
        impl tonic_reflection::server::v1::ServerReflection,
    >,
> {
    tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(include_bytes!(
            "../proto/proximadb_v1_descriptor.bin"
        ))
        .register_encoded_file_descriptor_set(include_bytes!("../proto/proximadb_descriptor.bin"))
        .build_v1()
        .context("failed to build gRPC reflection service")
}

// Deferred: Re-add TTL sweeper code in proper function context if needed
