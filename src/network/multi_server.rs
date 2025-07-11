// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Multi-server architecture with dedicated HTTP and gRPC servers

use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::monitoring::MetricsCollector;
use crate::services::collection_service::CollectionService;
use crate::services::vector_service::VectorService;
use crate::storage::metadata::backends::filestore_backend::{
    FilestoreMetadataBackend, FilestoreMetadataConfig,
};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::StorageEngine;

/// Multi-server configuration supporting HTTP and gRPC with binary Avro payloads
#[derive(Debug, Clone)]
pub struct MultiServerConfig {
    /// HTTP server configuration (REST/Dashboard/Metrics)
    pub http_config: RestHttpServerConfig,

    /// gRPC server configuration with binary Avro payloads
    pub grpc_config: GrpcHttpServerConfig,

    /// Global TLS configuration - applies to all servers
    pub tls_config: TLSConfig,
}

/// Global TLS configuration for all protocols
#[derive(Debug, Clone)]
pub struct TLSConfig {
    /// TLS certificate file path
    pub cert_file: Option<String>,

    /// TLS private key file path
    pub key_file: Option<String>,

    /// Private key password (for encrypted keys)
    pub key_password: Option<String>,

    /// Interface to bind (default: 0.0.0.0)
    pub bind_interface: String,

    /// Enable TLS (auto-detected from cert/key availability)
    pub enabled: bool,
}

impl Default for TLSConfig {
    fn default() -> Self {
        Self {
            cert_file: None,
            key_file: None,
            key_password: None,
            bind_interface: "0.0.0.0".to_string(),
            enabled: false,
        }
    }
}

/// HTTP server configuration for REST, Dashboard, and Metrics
#[derive(Debug, Clone)]
pub struct RestHttpServerConfig {
    /// HTTP bind port (default: 5678)
    pub port: u16,

    /// Enable REST API endpoints
    pub enable_rest: bool,

    /// Enable monitoring dashboard
    pub enable_dashboard: bool,

    /// Enable metrics endpoint
    pub enable_metrics: bool,

    /// Enable health check endpoint
    pub enable_health: bool,

    /// TLS certificate file path
    pub tls_cert_file: Option<String>,

    /// TLS private key file path
    pub tls_key_file: Option<String>,
}

impl RestHttpServerConfig {
    /// Check if TLS is enabled
    pub fn is_tls_enabled(&self) -> bool {
        self.tls_cert_file.is_some() && self.tls_key_file.is_some()
    }

    /// Verify TLS certificates exist
    pub fn verify_tls_certificates(&self) -> bool {
        if let (Some(cert), Some(key)) = (&self.tls_cert_file, &self.tls_key_file) {
            std::path::Path::new(cert).exists() && std::path::Path::new(key).exists()
        } else {
            false
        }
    }

    /// Get active bind address
    pub fn get_active_bind_address(&self) -> SocketAddr {
        format!("0.0.0.0:{}", self.port).parse().unwrap()
    }
}

/// gRPC server configuration with binary Avro payload support
#[derive(Debug, Clone)]
pub struct GrpcHttpServerConfig {
    /// gRPC bind port (default: 5679)
    pub port: u16,

    /// Bind address (computed from port and interface)
    pub bind_address: SocketAddr,

    /// TLS bind address (optional)
    pub tls_bind_address: Option<SocketAddr>,

    /// Enable gRPC endpoints
    pub enable_grpc: bool,

    /// Maximum message size in bytes
    pub max_message_size: usize,

    /// Enable gRPC reflection
    pub enable_reflection: bool,

    /// Enable gRPC compression for Avro payloads
    pub enable_compression: bool,

    /// TLS certificate file path
    pub tls_cert_file: Option<String>,

    /// TLS private key file path
    pub tls_key_file: Option<String>,
}

impl GrpcHttpServerConfig {
    /// Check if TLS is enabled
    pub fn is_tls_enabled(&self) -> bool {
        self.tls_cert_file.is_some() && self.tls_key_file.is_some()
    }

    /// Verify TLS certificates exist
    pub fn verify_tls_certificates(&self) -> bool {
        if let (Some(cert), Some(key)) = (&self.tls_cert_file, &self.tls_key_file) {
            std::path::Path::new(cert).exists() && std::path::Path::new(key).exists()
        } else {
            false
        }
    }

    /// Get active bind address
    pub fn get_active_bind_address(&self) -> SocketAddr {
        self.bind_address
    }
}

impl Default for MultiServerConfig {
    fn default() -> Self {
        Self {
            http_config: RestHttpServerConfig {
                port: 5678,
                enable_rest: true,
                enable_dashboard: true,
                enable_metrics: true,
                enable_health: true,
                tls_cert_file: None,
                tls_key_file: None,
            },
            grpc_config: GrpcHttpServerConfig {
                port: 5679,
                bind_address: "0.0.0.0:5679".parse().unwrap(),
                tls_bind_address: None,
                enable_grpc: true,
                max_message_size: 64 * 1024 * 1024, // 64MB for bulk vector inserts with Avro
                enable_reflection: true,
                enable_compression: true,
                tls_cert_file: None,
                tls_key_file: None,
            },
            tls_config: TLSConfig {
                cert_file: None,
                key_file: None,
                key_password: None,
                bind_interface: "0.0.0.0".to_string(),
                enabled: false,
            },
        }
    }
}

impl TLSConfig {
    /// Auto-detect TLS capability from certificate availability
    pub fn auto_detect_tls(&mut self) -> bool {
        if let (Some(cert_file), Some(key_file)) = (&self.cert_file, &self.key_file) {
            let cert_exists = std::path::Path::new(cert_file).exists();
            let key_exists = std::path::Path::new(key_file).exists();

            if cert_exists && key_exists {
                // Additional validation: try to parse the certificate
                if self.validate_certificates() {
                    self.enabled = true;
                    info!("🔒 TLS enabled - certificates validated");
                    return true;
                } else {
                    warn!("⚠️ TLS certificates found but invalid - using non-TLS");
                }
            } else {
                if !cert_exists {
                    debug!("📋 TLS certificate not found: {}", cert_file);
                }
                if !key_exists {
                    debug!("📋 TLS private key not found: {}", key_file);
                }
            }
        }

        self.enabled = false;
        info!("🌐 TLS disabled - using non-TLS mode");
        false
    }

    /// Validate certificate files can be read and parsed
    pub fn validate_certificates(&self) -> bool {
        if let (Some(cert_file), Some(key_file)) = (&self.cert_file, &self.key_file) {
            // Basic file existence and readability check
            match (std::fs::read(cert_file), std::fs::read(key_file)) {
                (Ok(cert_data), Ok(key_data)) => {
                    // Basic validation - check if files contain PEM markers
                    let cert_str = String::from_utf8_lossy(&cert_data);
                    let key_str = String::from_utf8_lossy(&key_data);

                    cert_str.contains("-----BEGIN CERTIFICATE-----")
                        && cert_str.contains("-----END CERTIFICATE-----")
                        && (key_str.contains("-----BEGIN PRIVATE KEY-----")
                            || key_str.contains("-----BEGIN RSA PRIVATE KEY-----")
                            || key_str.contains("-----BEGIN EC PRIVATE KEY-----"))
                }
                _ => false,
            }
        } else {
            false
        }
    }

    /// Get bind address for given port
    pub fn get_bind_address(&self, port: u16) -> SocketAddr {
        format!("{}:{}", self.bind_interface, port).parse().unwrap()
    }
}

impl MultiServerConfig {
    /// Get effective bind address for HTTP server
    pub fn get_http_bind_address(&self) -> SocketAddr {
        self.tls_config.get_bind_address(self.http_config.port)
    }

    /// Get effective bind address for gRPC server
    pub fn get_grpc_bind_address(&self) -> SocketAddr {
        self.tls_config.get_bind_address(self.grpc_config.port)
    }

    /// Check if TLS is enabled globally
    pub fn is_tls_enabled(&self) -> bool {
        self.tls_config.enabled
    }
}

/// Shared services for thin protocol handlers
/// Responsibilities: business logic, metadata configuration, service coordination
#[derive(Clone)]
pub struct SharedServices {
    pub collection_service: Arc<CollectionService>,
    pub vector_service: Arc<VectorService>,
    pub metrics_collector: Option<Arc<MetricsCollector>>,
    // Internal state for business logic coordination
    storage: Arc<RwLock<StorageEngine>>,
}

impl SharedServices {
    /// Create shared services with full business logic configuration
    /// SharedServices owns all business logic and configuration decisions
    pub async fn new(
        storage: Arc<RwLock<StorageEngine>>,
        metrics_collector: Option<Arc<MetricsCollector>>,
        metadata_config: Option<crate::core::config::MetadataBackendConfig>,
    ) -> Result<Self> {
        info!("🔧 SharedServices: Initializing business logic hub for ALL protocols");

        // SharedServices owns metadata configuration logic
        let (filestore_config, filesystem_config) = if let Some(config) = metadata_config {
            info!(
                "🔧 SharedServices: Metadata config received - backend_type: {}, storage_url: {}",
                config.backend_type, config.storage_url
            );
            info!(
                "📂 SharedServices: Configuring metadata backend from TOML: {}",
                config.storage_url
            );

            let filestore_config = FilestoreMetadataConfig {
                storage_url: config.storage_url.clone(),
                enable_compression: config.cache_size_mb.map(|_| true).unwrap_or(true),
                enable_snapshots: true,
                snapshot_threshold: 1000,
                keep_snapshots: 5,
                backup_url: None,
                temp_dir: None,
            };

            // SharedServices handles cloud vs local filesystem logic
            let filesystem_config = if config.storage_url.starts_with("s3://")
                || config.storage_url.starts_with("gcs://")
                || config.storage_url.starts_with("adls://")
            {
                info!("☁️ SharedServices: Configuring cloud filesystem for metadata");
                // TODO: Use cloud_config from TOML for S3/GCS/Azure credentials
                crate::storage::persistence::filesystem::FilesystemConfig::default()
            } else {
                info!("📁 SharedServices: Configuring local filesystem for metadata");

                // Parse the base path from file:// URL for local filesystem
                let base_path = if config.storage_url.starts_with("file://") {
                    let path = config.storage_url.strip_prefix("file://").unwrap_or("");
                    Some(std::path::PathBuf::from(path))
                } else {
                    Some(std::path::PathBuf::from(&config.storage_url))
                };

                info!(
                    "📂 SharedServices: Setting local filesystem root_dir to: {:?}",
                    base_path
                );

                let mut fs_config =
                    crate::storage::persistence::filesystem::FilesystemConfig::default();
                if let Some(ref mut local_config) = fs_config.local {
                    local_config.root_dir = base_path;
                }
                fs_config
            };

            (filestore_config, filesystem_config)
        } else {
            info!("📂 SharedServices: Using default metadata configuration");
            (
                FilestoreMetadataConfig::default(),
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
        };

        info!(
            "📁 SharedServices: Unified metadata backend URL: {}",
            filestore_config.storage_url
        );

        // SharedServices creates the unified metadata backend for all protocols
        let filesystem_factory = Arc::new(FilesystemFactory::new(filesystem_config).await?);

        let filestore_backend =
            Arc::new(FilestoreMetadataBackend::new(filestore_config, filesystem_factory).await?);

        let collection_service = Arc::new(CollectionService::new(filestore_backend).await?);
        
        // 🔗 DEPENDENCY INJECTION: Inject collection service into storage engine
        {
            let storage_ref = storage.read().await;
            storage_ref.set_metadata_provider(collection_service.clone() as Arc<dyn crate::storage::traits::CollectionMetadataProvider>).await;
        }
        info!("✅ SharedServices: Collection service injected into StorageEngine");

        // SharedServices coordinates vector operations with WAL
        let avro_config = crate::services::vector_service::UnifiedServiceConfig::default();
        let wal_manager = {
            let storage_ref = storage.read().await;
            storage_ref.get_wal_manager()
        };

        let vector_service = Arc::new(
            VectorService::with_existing_wal(
                storage.clone(),
                wal_manager,
                avro_config,
            )
            .await?,
        );

        // 🔗 DEPENDENCY INJECTION: Inject collection service into VIPER for real-time schema generation
        vector_service.set_collection_service(collection_service.clone()).await;
        info!("✅ SharedServices: Collection service injected into VIPER engine for dynamic schema generation");

        // CRITICAL: Restore collection metadata from WAL during startup
        // This ensures collections are visible to gRPC service after server restart
        info!("🔄 SharedServices: Starting metadata recovery from WAL");
        let recovered_collections = {
            let storage_ref = storage.read().await;
            storage_ref.get_recovered_collections_metadata().await?
        };

        if !recovered_collections.is_empty() {
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
                let collection_config = crate::proto::proximadb::CollectionConfig {
                    name: metadata.name.clone(),
                    dimension: metadata.dimension as i32,
                    distance_metric: crate::proto::proximadb::DistanceMetric::Cosine as i32, // Default
                    storage_engine: crate::proto::proximadb::StorageEngine::Viper as i32, // Default
                    primary_indexing_algorithm: crate::proto::proximadb::IndexingAlgorithm::Hnsw as i32, // Default
                    filterable_columns: vec![],
                    index_configs: vec![],
                    quantization_config: None,
                    primary_index_name: String::new(),
                    enable_automatic_index_selection: false,
                    description: None,
                    tags: vec![],
                    owner: None,
                };

                let proto_collection = crate::proto::proximadb::Collection {
                    id: format!("recovered-{}", Uuid::new_v4()),
                    config: Some(collection_config),
                    stats: Some(crate::proto::proximadb::CollectionStats {
                        vector_count: metadata.vector_count as i64,
                        index_size_bytes: metadata.total_size_bytes as i64,
                        data_size_bytes: metadata.total_size_bytes as i64,
                    }),
                    created_at: metadata.created_at.timestamp_millis(),
                    updated_at: metadata.updated_at.timestamp_millis(),
                };

                // Store the recovered collection in the metadata backend
                match collection_service
                    .get_metadata_backend()
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

        info!("✅ SharedServices: Business logic hub ready for ALL protocols (gRPC, REST, WebSocket, etc.)");

        Ok(Self {
            collection_service,
            vector_service,
            metrics_collector,
            storage,
        })
    }

    /// Get storage engine (for advanced operations)
    pub fn storage(&self) -> &Arc<RwLock<StorageEngine>> {
        &self.storage
    }
}

/// Multi-server manager that coordinates HTTP and gRPC servers with thin handlers
/// Responsibilities: ports, TLS, server lifecycle, protocol orchestration
pub struct MultiServer {
    config: MultiServerConfig,
    shared_services: Option<SharedServices>,
    server_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl MultiServer {
    /// Create new multi-server instance (orchestrator only)
    /// MultiServer focuses on network orchestration, SharedServices handles business logic
    pub fn new(config: MultiServerConfig, shared_services: SharedServices) -> Self {
        info!("🚀 MultiServer: Initializing network orchestrator");
        info!(
            "📡 MultiServer: gRPC port: {}, REST port: {}",
            config.grpc_config.port, config.http_config.port
        );
        info!("🔒 MultiServer: TLS enabled: {}", config.is_tls_enabled());

        Self {
            config,
            shared_services: Some(shared_services),
            server_handles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start all configured servers: gRPC on 5679, REST on 5678
    pub async fn start(&mut self) -> Result<()> {
        info!("🚀 Starting ProximaDB Multi-Server: gRPC:5679 + REST:5678");

        let services = self.shared_services.as_ref().unwrap().clone();
        let mut handles = Vec::new();

        // Start gRPC server on port 5679 if configured
        if self.config.grpc_config.enable_grpc {
            info!("🔗 Starting gRPC Server on port 5679");

            // Create thin gRPC handler with shared services
            let grpc_handler =
                crate::network::grpc::service::ProximaDbGrpcService::new_with_services(
                    services.clone(),
                )
                .await;

            // Create gRPC server
            let grpc_service =
                crate::proto::proximadb::proxima_db_server::ProximaDbServer::new(grpc_handler);

            let mut server_builder = tonic::transport::Server::builder()
                .add_service(grpc_service);

            // Add reflection if enabled
            if self.config.grpc_config.enable_reflection {
                debug!("Adding gRPC reflection service");
                let file_descriptor_data = include_bytes!("../proto/proximadb_descriptor.bin");
                server_builder = server_builder.add_service(
                    tonic_reflection::server::Builder::configure()
                        .register_encoded_file_descriptor_set(file_descriptor_data)
                        .build()?,
                );
            }

            let grpc_bind_addr = self.config.get_grpc_bind_address();

            let grpc_handle = tokio::spawn(async move {
                if let Err(e) = server_builder
                    .serve_with_shutdown(grpc_bind_addr, async {
                        tokio::signal::ctrl_c().await.ok();
                        debug!("gRPC server graceful shutdown signal received");
                    })
                    .await
                {
                    tracing::error!("gRPC server error: {}", e);
                }
            });

            handles.push(grpc_handle);
            info!("✅ gRPC Server started on {}", grpc_bind_addr);
        }

        // Start REST server on port 5678 if configured
        if self.config.http_config.enable_rest {
            info!("📡 Starting REST Server on port 5678");

            let rest_bind_addr = self.config.get_http_bind_address();
            let unified_service = services.vector_service.clone();
            let collection_service = services.collection_service.clone();

            let rest_handle = tokio::spawn(async move {
                use crate::network::rest::server::RestServer;

                match RestServer::new(rest_bind_addr, unified_service, collection_service)
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

        *self.server_handles.lock().await = handles;

        info!("🎯 Multi-Server started successfully: gRPC:5679 + REST:5678");
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
    pub async fn get_status(&self) -> ServerStatus {
        let handles = self.server_handles.lock().await;
        let servers_running = !handles.is_empty();

        ServerStatus {
            http_running: self.config.http_config.enable_rest && servers_running,
            grpc_running: self.config.grpc_config.enable_grpc && servers_running,
            http_address: Some(self.config.get_http_bind_address()),
            grpc_address: Some(self.config.get_grpc_bind_address()),
            tls_enabled: self.config.is_tls_enabled(),
        }
    }
}

/// Server status information
#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub http_running: bool,
    pub grpc_running: bool,
    pub http_address: Option<SocketAddr>,
    pub grpc_address: Option<SocketAddr>,
    pub tls_enabled: bool,
}
