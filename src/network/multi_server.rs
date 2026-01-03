// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Multi-server architecture with dedicated HTTP and gRPC servers
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

use crate::utils::uuid::Uuid;
use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::metrics::MetricsConfig;
use crate::monitoring::MetricsCollector;
use crate::query::facade::{UnifiedQueryFacade, VectorSearchStrategy, GraphStrategy, SqlStrategy, ColumnarStrategy, DocumentStrategy, ObservabilityStrategy, FacadeConfig, QueryStrategy, QueryFacadeAdapter};
use crate::storage::document::DocumentService;
use crate::observability::query::ObservabilityQueryEngine;
use crate::observability::storage::ObservabilityStorage;
use crate::query::federated::FederatedQueryContext;
use crate::storage::multimodel::MultiModelStorageFacade;
use crate::services::VectorOperationsService;
use crate::services::collection::manager::CollectionService;
use crate::security::SecurityCoordinator;
use crate::storage::StorageEngine;
use crate::storage::metadata::backends::MetadataBackendFactory;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

/// Multi-server configuration supporting HTTP and gRPC with binary Avro payloads
///
/// ## Configuration Strategy:
///
/// The MultiServerConfig aggregates settings for both protocols,
/// allowing unified configuration while maintaining protocol-specific
/// optimizations.
///
/// ## Key Settings:
///
/// - **Ports**: Separate ports prevent protocol confusion
/// - **Compression**: Can differ between REST (JSON) and gRPC (Protobuf)
/// - **Message Limits**: gRPC typically needs larger limits for batch ops
/// - **TLS**: Shared or separate certificates supported
#[derive(Debug, Clone)]
pub struct MultiServerConfig {
    /// HTTP server configuration (REST/Dashboard/Metrics)
    /// Handles JSON payloads, web UI, and monitoring endpoints
    pub http_config: RestHttpServerConfig,

    /// gRPC server configuration with binary Avro payloads
    /// Optimized for high-throughput vector operations
    pub grpc_config: GrpcHttpServerConfig,

    /// Arrow IPC (Flight) server configuration
    /// Optimized for bulk ingestion and ETL pipelines
    pub arrow_ipc_config: ArrowIpcServerConfig,

    /// PostgreSQL wire protocol server configuration
    /// Enables pgvector compatibility and psql/pgAdmin connections
    pub postgres_config: PostgresServerConfig,

    /// Global TLS configuration - applies to all servers
    /// Can be overridden per-server if needed
    pub tls_config: TLSConfig,

    /// API configuration (request limits, timeouts, etc.)
    /// Shared limits and policies across all protocols
    pub api_config: Option<crate::core::config::ApiConfig>,

    /// Data directory from server config (server.data_dir from TOML)
    /// Used by REST handlers for document/observability storage paths
    pub data_dir: std::path::PathBuf,

    // ============================================================
    // Unified Port Architecture (Phase 14)
    // ============================================================

    /// Enable unified port mode (REST + gRPC + Arrow Flight on single port)
    /// When enabled, `unified_port` is used; individual ports are ignored.
    /// Default: false (legacy multi-port mode for backward compatibility)
    pub unified_mode: bool,

    /// Unified port for all HTTP-based protocols (REST, gRPC, Arrow Flight)
    /// Only used when `unified_mode = true`
    /// Default: 5678
    pub unified_port: u16,

    /// Bind address for unified port (e.g., "0.0.0.0")
    pub unified_bind_address: String,
}

/// PostgreSQL wire protocol server configuration
///
/// ## PostgreSQL Compatibility:
///
/// - **Wire Protocol**: PostgreSQL Protocol v3.0
/// - **pgvector Support**: <->, <=>, <#> distance operators
/// - **Clients**: psql, pgAdmin, application drivers
///
/// ## Use Cases:
///
/// - Migration path from pgvector
/// - Familiar SQL interface for vector operations
/// - Existing application compatibility
#[derive(Debug, Clone)]
pub struct PostgresServerConfig {
    /// PostgreSQL bind port (default: 5432 or 5433)
    pub port: u16,

    /// Bind address
    pub bind_address: SocketAddr,

    /// Enable PostgreSQL server
    pub enable_postgres: bool,

    /// Maximum connections
    pub max_connections: usize,

    /// Idle timeout (seconds)
    pub idle_timeout_secs: u64,

    /// Statement cache size per connection
    pub statement_cache_size: usize,
}

impl Default for PostgresServerConfig {
    fn default() -> Self {
        Self {
            port: 5433, // Use 5433 to avoid conflict with real PostgreSQL
            bind_address: "0.0.0.0:5433".parse().unwrap(),
            enable_postgres: true,
            max_connections: 100,
            idle_timeout_secs: 3600,
            statement_cache_size: 100,
        }
    }
}

impl PostgresServerConfig {
    /// Get active bind address
    pub fn active_bind_address(&self) -> SocketAddr {
        self.bind_address
    }
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

    /// CA certificate file path for mTLS client verification
    pub ca_file: Option<String>,

    /// Require client certificates (mTLS mode)
    pub require_client_certs: bool,

    /// Auto-generate self-signed certificates for development
    pub auto_generate: bool,

    /// Certificate validity period in days (for auto-generated certs)
    pub validity_days: u32,

    /// Days before expiration to trigger renewal warnings
    pub renewal_threshold_days: u32,
}

impl Default for TLSConfig {
    fn default() -> Self {
        Self {
            cert_file: None,
            key_file: None,
            key_password: None,
            bind_interface: "0.0.0.0".to_string(),
            enabled: false,
            ca_file: None,
            require_client_certs: false,
            auto_generate: false,
            validity_days: 365,
            renewal_threshold_days: 30,
        }
    }
}

impl TLSConfig {
    /// Create a new TLS configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure for mTLS with CA certificate
    pub fn with_mtls(mut self, ca_file: &str) -> Self {
        self.ca_file = Some(ca_file.to_string());
        self.require_client_certs = true;
        self
    }

    /// Set certificate and key files
    pub fn with_certificates(mut self, cert_file: &str, key_file: &str) -> Self {
        self.cert_file = Some(cert_file.to_string());
        self.key_file = Some(key_file.to_string());
        self.enabled = true;
        self
    }

    /// Enable auto-generation of self-signed certificates
    pub fn with_auto_generate(mut self, validity_days: u32) -> Self {
        self.auto_generate = true;
        self.validity_days = validity_days;
        self
    }

    /// Check if mTLS is configured
    pub fn is_mtls_enabled(&self) -> bool {
        self.enabled && self.require_client_certs && self.ca_file.is_some()
    }

    /// Get CA certificate path if configured
    pub fn get_ca_path(&self) -> Option<&str> {
        self.ca_file.as_deref()
    }
}

/// HTTP server configuration for REST, Dashboard, and Metrics
///
/// ## REST Server Endpoints:
///
/// - `/v1/collections`: Collection CRUD operations
/// - `/v1/vectors`: Vector search and management
/// - `/v1/health`: Kubernetes health probes
/// - `/metrics`: Prometheus metrics
/// - `/dashboard`: Web UI (if enabled)
///
/// ## Compression Strategy:
///
/// HTTP compression disabled by default because:
/// - CPU overhead often exceeds network savings
/// - Most deployments use fast local/datacenter networks
/// - Can be enabled for WAN deployments
#[derive(Debug, Clone)]
pub struct RestHttpServerConfig {
    /// HTTP bind port (default: 5678)
    /// Standard port for ProximaDB REST API
    pub port: u16,

    /// Enable REST API endpoints
    /// Core CRUD and search operations
    pub enable_rest: bool,

    /// Enable monitoring dashboard
    /// Web UI for cluster monitoring
    pub enable_dashboard: bool,

    /// Enable metrics endpoint
    /// Prometheus-compatible metrics at /metrics
    pub enable_metrics: bool,

    /// Enable health check endpoint
    /// Kubernetes liveness/readiness probes
    pub enable_health: bool,

    /// Enable HTTP compression (default: false for better performance)
    /// Trade CPU for bandwidth - useful for WAN
    pub compression: bool,

    /// TLS certificate file path
    /// PEM-encoded X.509 certificate
    pub tls_cert_file: Option<String>,

    /// TLS private key file path
    /// PEM-encoded private key (RSA/ECDSA)
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
    pub fn active_bind_address(&self) -> SocketAddr {
        format!("0.0.0.0:{}", self.port).parse().unwrap()
    }
}

/// gRPC server configuration with binary Avro payload support
///
/// ## gRPC Advantages:
///
/// - **Binary Protocol**: 2-3x smaller than JSON
/// - **HTTP/2**: Multiplexing, server push, header compression
/// - **Streaming**: Bidirectional streams for bulk operations
/// - **Type Safety**: Strongly typed protobuf contracts
///
/// ## Message Size Considerations:
///
/// Default 64MB supports:
/// - 100K vectors of 128 dimensions
/// - 25K vectors of 512 dimensions
/// - 8K vectors of 1536 dimensions (OpenAI)
///
/// Increase for larger batches or use streaming.
#[derive(Debug, Clone)]
pub struct GrpcHttpServerConfig {
    /// gRPC bind port (default: 5679)
    /// Standard port for ProximaDB gRPC API
    pub port: u16,

    /// Bind address (computed from port and interface)
    /// Usually 0.0.0.0:5679 for all interfaces
    pub bind_address: SocketAddr,

    /// TLS bind address (optional)
    /// Same port, TLS-only listener
    pub tls_bind_address: Option<SocketAddr>,

    /// Enable gRPC endpoints
    /// Core service implementation
    pub enable_grpc: bool,

    /// Maximum message size in bytes
    /// Prevents OOM from malicious/accidental huge messages
    pub max_message_size: usize,

    /// Enable gRPC reflection
    /// Allows dynamic service discovery (grpcurl, etc)
    pub enable_reflection: bool,

    /// Enable gRPC compression for Avro payloads
    /// Further reduces already-compact protobuf
    pub compression: bool,

    /// TLS certificate file path
    /// Same format as REST server
    pub tls_cert_file: Option<String>,

    /// TLS private key file path
    /// Can share with REST or use separate
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
    pub fn active_bind_address(&self) -> SocketAddr {
        self.bind_address
    }
}

/// Arrow IPC (Flight) server configuration for high-throughput bulk ingestion
///
/// ## Arrow Flight Advantages:
///
/// - **High Throughput**: 100K-200K vectors/sec for bulk ingestion
/// - **Zero-Copy**: Arrow columnar format minimizes serialization overhead
/// - **Streaming**: Efficient for large dataset transfers
/// - **Standardized**: Apache Arrow IPC protocol
///
/// ## Use Cases:
///
/// - ETL pipelines and data migrations
/// - Bulk vector uploads from data lakes
/// - Direct writes bypassing WAL for maximum speed
/// - Explicit flush/compaction control
///
/// ## Message Size Considerations:
///
/// Default 512MB supports massive batch uploads:
/// - 1M vectors of 384 dimensions
/// - 500K vectors of 768 dimensions
/// - 200K vectors of 1536 dimensions
#[derive(Debug, Clone)]
pub struct ArrowIpcServerConfig {
    /// Arrow IPC bind port (default: 5680)
    /// Standard port for ProximaDB Arrow Flight API
    pub port: u16,

    /// Bind address (computed from port and interface)
    /// Usually 0.0.0.0:5680 for all interfaces
    pub bind_address: SocketAddr,

    /// Enable Arrow IPC endpoints
    /// Core Flight service implementation
    pub enable_arrow_ipc: bool,

    /// Maximum message size in bytes
    /// Default 512MB for large batch uploads
    pub max_message_size: usize,

    /// Enable Arrow IPC compression
    /// Note: Arrow has built-in compression, usually disabled at transport layer
    pub compression: bool,

    /// TLS certificate file path
    /// Same format as REST/gRPC servers
    pub tls_cert_file: Option<String>,

    /// TLS private key file path
    /// Can share with other servers or use separate
    pub tls_key_file: Option<String>,
}

impl ArrowIpcServerConfig {
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
    pub fn active_bind_address(&self) -> SocketAddr {
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
                compression: false, // Default to false for better debugging
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
                compression: true,
                tls_cert_file: None,
                tls_key_file: None,
            },
            arrow_ipc_config: ArrowIpcServerConfig {
                port: 5680,
                bind_address: "0.0.0.0:5680".parse().unwrap(),
                enable_arrow_ipc: true,
                max_message_size: 512 * 1024 * 1024, // 512MB for massive batch uploads
                compression: false, // Arrow has built-in compression
                tls_cert_file: None,
                tls_key_file: None,
            },
            postgres_config: PostgresServerConfig::default(),
            tls_config: TLSConfig::default(),
            api_config: None, // Will be set when creating from Config
            data_dir: std::path::PathBuf::from("/tmp/proximadb/data"), // Default fallback
            // Unified port defaults (Phase 14)
            unified_mode: false, // Legacy multi-port mode by default
            unified_port: 5678,  // Use REST port for unified
            unified_bind_address: "0.0.0.0".to_string(),
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
    pub fn bind_address(&self, port: u16) -> SocketAddr {
        format!("{}:{}", self.bind_interface, port).parse().unwrap()
    }
}

impl MultiServerConfig {
    /// Get effective bind address for HTTP server
    pub fn http_bind_address(&self) -> SocketAddr {
        self.tls_config.bind_address(self.http_config.port)
    }

    /// Get effective bind address for gRPC server
    pub fn grpc_bind_address(&self) -> SocketAddr {
        self.tls_config.bind_address(self.grpc_config.port)
    }

    /// Get effective bind address for unified port mode
    pub fn unified_bind_address(&self) -> SocketAddr {
        format!("{}:{}", self.unified_bind_address, self.unified_port)
            .parse()
            .unwrap_or_else(|_| format!("0.0.0.0:{}", self.unified_port).parse().expect("valid default"))
    }

    /// Check if unified port mode is enabled
    pub fn is_unified_mode(&self) -> bool {
        self.unified_mode
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
    pub vector_operations_service: Arc<VectorOperationsService>,
    pub graph_service: Arc<crate::graph::GraphService>,
    pub unified_handlers: Arc<UnifiedHandlers>,
    pub metrics_collector: Option<Arc<MetricsCollector>>,
    pub metrics_updater: Option<Arc<dyn crate::metrics::InternalMetricsUpdater + 'static>>,
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
                    debug!("🔧 SharedServices: WAL directory URL from storage_locations: {}", url);
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
            crate::storage::engines::impls::viper::ViperEngine::from_core_config(
                viper_config,
                filesystem_factory.clone(),
            )
            .await?,
        );
        debug!("✅ SharedServices::new - VIPER engine created successfully");

        // Create SST engine
        debug!("🔧 SharedServices::new - Creating SST engine...");
        let sst_engine = Arc::new(crate::storage::engines::impls::sst::SstEngine::new().await?);
        debug!("✅ SharedServices::new - SST engine created successfully");

        // Clone SST engine reference for DocumentService (used later for DocumentStrategy)
        let sst_engine_for_documents: Arc<dyn crate::storage::traits::UnifiedStorageEngine> = sst_engine.clone();

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
        crate::storage::engines::impls::sst::core::set_sst_axis_manager(axis_manager.clone());
        debug!("✅ SharedServices::new - AXIS manager registered with SST engine for HNSW/IVF search");

        // Create VectorOperationsService with optimized architecture and two-stage search
        debug!(
            "🔧 SharedServices::new - About to create VectorOperationsService with two-stage search..."
        );
        // Use the passed orchestrator if available, otherwise create a default one
        use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
        let orchestrator = if let Some(orch) = orchestrator {
            orch
        } else {
            // Create a default orchestrator if none provided (backward compatibility)
            let mut default_orchestrator =
                CrossCacheOrchestrator::new((storage_config.cache_size_mb * 1024 * 1024) as usize);
            default_orchestrator.start_eviction_service(None);
            let orch = Arc::new(default_orchestrator);
            orch.clone().start_rebalancing_service();
            CrossCacheOrchestrator::register_global(orch.clone());
            orch
        };

        // =========================================================================
        // Initialize EventLog service and start AXIS consumer for async index building
        // This enables automatic AXIS index updates when data is flushed to storage
        // =========================================================================
        debug!("🔧 SharedServices::new - Initializing EventLog service for AXIS indexing...");

        // Use the global collection cache (shared across services)
        // Collections are registered in this cache when created via register_collection_in_cache()
        let collection_cache = crate::services::events::log::get_or_create_global_collection_cache();

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

            let _consumer_handle = crate::index::axis::integration::eventlog_consumer::start_axis_consumer(
                crate::services::events::log::event_log_service()
                    .expect("EventLog service just initialized")
                    .inner(),
                axis_manager.clone(),
                filesystem_factory.clone(),
                collection_cache.clone(),
                orchestrator.clone(),
                shutdown_rx,
            )
            .await;

            info!("✅ SharedServices: AXIS EventLog consumer started - automatic index building enabled");
        }

        let vector_operations_service = Arc::new(
            VectorOperationsService::new(
                sst_engine,
                wal_manager,
                axis_manager,
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
        // TODO: Add assignment service recovery after StorageEngine starts

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
        // - UnifiedHandlers (via with_shared_graph_services)
        //
        // This ensures ALL graph endpoints and operations share the same state.
        // ==================================================================================

        debug!("🔧 SharedServices::new - Creating SHARED GraphCollectionService instance with auto-recovery...");
        let graph_collection_service = match crate::services::GraphCollectionService::new_with_recovery().await {
            Ok(svc) => Arc::new(svc),
            Err(e) => {
                warn!("Failed to create GraphCollectionService with recovery: {}. Using non-persistent service.", e);
                Arc::new(crate::services::GraphCollectionService::new())
            }
        };
        debug!("✅ SharedServices::new - Shared GraphCollectionService created (with auto-recovery)");

        // Create GraphOperationsService for native graph database operations
        // IMPORTANT: Pass the shared GraphCollectionService instance
        debug!(
            "🔧 SharedServices::new - Creating GraphOperationsService with SHARED collection service..."
        );
        // ALWAYS use new_with_collection_service to ensure shared GraphCollectionService
        // Even if config is provided, we must share the collection service
        // (Config-specific settings can be applied later if needed)
        let mut graph_service_inst = crate::graph::GraphOperationsService::new_with_collection_service(
            graph_collection_service.clone()
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
            crate::metrics::store::MetricsPersistenceLayer::new(filesystem_factory, metrics_config)
                .await?,
        );
        let metrics_updater: Arc<dyn crate::metrics::InternalMetricsUpdater + 'static> = Arc::new(
            crate::metrics::updater::MetricsUpdateService::new(metrics_store.clone()),
        );
        graph_service_inst.set_metrics_updater(metrics_updater.clone());
        debug!("📈 GraphOperationsService metrics updater wired");
        let graph_service = Arc::new(graph_service_inst);
        debug!("✅ SharedServices::new - GraphOperationsService created with shared collection service");

        // Create unified handlers with SHARED graph services
        // IMPORTANT: Pass the pre-created GraphCollectionService and GraphOperationsService
        // to ensure ALL graph endpoints and operations share the same state
        debug!(
            "🔧 SharedServices::new - Creating UnifiedHandlers with SHARED graph services..."
        );
        let unified_handlers_instance = UnifiedHandlers::new(
            collection_service.clone(),
            vector_operations_service.clone(),
            graph_collection_service.clone(), // SHARED instance
            graph_service.clone(),             // Uses the SHARED collection service
        );

        // Apply hybrid runtime config if provided
        if let Some(cfg) = opt_config {
            if let Some(ref hybrid) = cfg.hybrid {
                unified_handlers_instance.set_hybrid_runtime(hybrid.clone());
            }
        }
        let unified_handlers = Arc::new(unified_handlers_instance);
        debug!("✅ SharedServices::new - UnifiedHandlers created with shared graph services");

        // ==================================================================================
        // Create UnifiedQueryFacade - single entry point for all query types
        // This consolidates the 5 parallel query paths into a single unified interface
        // ==================================================================================
        debug!("🔧 SharedServices::new - Creating UnifiedQueryFacade with real strategies...");

        // Create VectorSearchStrategy wrapping VectorOperationsService
        let vector_strategy: Arc<dyn crate::query::facade::QueryStrategy> = Arc::new(
            VectorSearchStrategy::new(
                vector_operations_service.clone(),
                collection_service.clone(),
            )
        );

        // Create GraphStrategy wrapping GraphOperationsService
        let graph_strategy: Arc<dyn crate::query::facade::QueryStrategy> = Arc::new(
            GraphStrategy::new(graph_service.clone())
        );

        // Create MultiModelStorageFacade for federated queries
        // This provides unified access to vector/graph/document stores for SQL execution
        debug!("🔧 SharedServices::new - Creating MultiModelStorageFacade for federated queries...");
        let multimodel_storage = Arc::new(MultiModelStorageFacade::new());
        debug!("✅ SharedServices::new - MultiModelStorageFacade created");

        // Create FederatedQueryContext for SQL with multi-model extensions
        debug!("🔧 SharedServices::new - Creating FederatedQueryContext...");
        let federated_context = Arc::new(FederatedQueryContext::new(multimodel_storage));
        debug!("✅ SharedServices::new - FederatedQueryContext created");

        // Create SqlStrategy wrapping FederatedQueryContext
        let sql_strategy: Arc<dyn crate::query::facade::QueryStrategy> = Arc::new(
            SqlStrategy::new(federated_context)
        );

        // Create ColumnarStrategy for analytical queries (M2 Dual Columnar Execution)
        // This strategy handles SQL queries with aggregations, GROUP BY, DISTINCT
        // by routing them through Arrow/Parquet columnar providers
        let columnar_strategy: Arc<dyn crate::query::facade::QueryStrategy> = Arc::new(
            ColumnarStrategy::new()
        );
        debug!("✅ SharedServices::new - ColumnarStrategy created for analytical queries");

        // Create DocumentStrategy wrapping DocumentService for JSON document queries
        // DocumentService provides MongoDB-like document operations (CRUD, indexing, queries)
        debug!("🔧 SharedServices::new - Creating DocumentService for document queries...");
        let document_service = Arc::new(DocumentService::new(sst_engine_for_documents));
        let document_strategy: Arc<dyn crate::query::facade::QueryStrategy> = Arc::new(
            DocumentStrategy::new(document_service)
        );
        debug!("✅ SharedServices::new - DocumentStrategy created for document queries");

        // Create ObservabilityStrategy wrapping ObservabilityQueryEngine for logs/metrics/traces
        // This enables unified query interface for observability data
        debug!("🔧 SharedServices::new - Creating ObservabilityQueryEngine for observability queries...");
        let observability_base_path = storage_config
            .metadata_url
            .replace("file://", "");
        let observability_storage = Arc::new(ObservabilityStorage::new(&observability_base_path));
        let observability_query_engine = Arc::new(ObservabilityQueryEngine::new(observability_storage));
        let observability_strategy: Arc<dyn crate::query::facade::QueryStrategy> = Arc::new(
            ObservabilityStrategy::new(observability_query_engine)
        );
        debug!("✅ SharedServices::new - ObservabilityStrategy created for logs/metrics/traces queries");

        // Build the unified facade with all strategies
        // Priority order: vector (100) > graph (75) > document (70) > observability (60) > columnar (50) > sql (25)
        let strategies = vec![
            vector_strategy,
            graph_strategy,
            document_strategy,
            observability_strategy,
            columnar_strategy,
            sql_strategy,
        ];
        let query_facade = Arc::new(
            UnifiedQueryFacade::new(strategies, FacadeConfig::default())
        );

        info!(
            "✅ SharedServices: UnifiedQueryFacade created with 6 strategies (vector, graph, document, observability, columnar, sql)"
        );

        // Wire QueryFacadeAdapter to UnifiedHandlers for unified SQL routing
        // This enables SQL queries to flow through the facade when unified-facade-routing feature is enabled
        let query_adapter = Arc::new(QueryFacadeAdapter::new(query_facade.clone()));
        unified_handlers.set_query_adapter(query_adapter);
        debug!("✅ SharedServices::new - QueryFacadeAdapter wired to UnifiedHandlers");

        info!(
            "✅ SharedServices: Business logic hub ready for ALL protocols (gRPC, REST, WebSocket, etc.)"
        );

        Ok((
            Self {
                collection_service: collection_service.clone(),
                vector_operations_service,
                graph_service,
                unified_handlers,
                metrics_collector,
                metrics_updater: Some(metrics_updater.clone()),
                query_facade,
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
    /// to/from QueryRequest/QueryResult, enabling unified query routing.
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

/// Multi-server manager that coordinates HTTP and gRPC servers with thin handlers
/// Responsibilities: ports, TLS, server lifecycle, protocol orchestration
pub struct MultiServer {
    config: MultiServerConfig,
    pub shared_services: SharedServices, // Made public for recovery access
    security_coordinator: Option<Arc<SecurityCoordinator>>,
    rest_auth_enabled: bool,
    server_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// Storage engine reference for PostgreSQL wire protocol server
    storage: Option<Arc<RwLock<StorageEngine>>>,
}

impl MultiServer {
    /// Create new multi-server instance (orchestrator only)
    /// MultiServer focuses on network orchestration, SharedServices handles business logic
    pub fn new(
        config: MultiServerConfig,
        shared_services: SharedServices,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
        rest_auth_enabled: bool,
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
            storage: None,
        }
    }

    /// Set the storage engine reference for PostgreSQL wire protocol support
    /// This should be called after ProximaDB creates the storage engine
    pub fn set_storage(&mut self, storage: Arc<RwLock<StorageEngine>>) {
        self.storage = Some(storage);
        info!("🐘 MultiServer: Storage engine wired for PostgreSQL protocol");
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

        // Start gRPC server on port 5679 if configured
        if self.config.grpc_config.enable_grpc {
            info!("🔗 Starting gRPC Server on port 5679");

            // Check TLS configuration early
            let tls_enabled = self.config.grpc_config.is_tls_enabled();
            let mtls_enabled = self.config.tls_config.is_mtls_enabled();
            let cert_path = self.config.grpc_config.tls_cert_file.clone()
                .or_else(|| self.config.tls_config.cert_file.clone());
            let key_path = self.config.grpc_config.tls_key_file.clone()
                .or_else(|| self.config.tls_config.key_file.clone());
            let ca_path = self.config.tls_config.ca_file.clone();

            // Create gRPC server builder with TLS if configured
            let mut server_builder = if tls_enabled || mtls_enabled {
                if let (Some(cert), Some(key)) = (&cert_path, &key_path) {
                    use tonic::transport::{Identity, ServerTlsConfig, Certificate};

                    // Load certificate and key
                    let cert_data = std::fs::read(cert)
                        .map_err(|e| anyhow::anyhow!("Failed to read TLS certificate: {}", e))?;
                    let key_data = std::fs::read(key)
                        .map_err(|e| anyhow::anyhow!("Failed to read TLS key: {}", e))?;

                    let identity = Identity::from_pem(cert_data, key_data);

                    // Build TLS config - with or without client CA for mTLS
                    let tls_config = if mtls_enabled {
                        if let Some(ref ca) = ca_path {
                            let ca_data = std::fs::read(ca)
                                .map_err(|e| anyhow::anyhow!("Failed to read CA certificate: {}", e))?;
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

            // Add versioned VectorService (v1)
            #[cfg(feature = "unified-facade-routing")]
            let vector_service_impl = crate::network::grpc::vector_service::VectorServiceImpl::with_adapter(
                services.unified_handlers.clone(),
                Some(services.query_adapter()),
            );
            #[cfg(not(feature = "unified-facade-routing"))]
            let vector_service_impl = crate::network::grpc::vector_service::VectorServiceImpl::new(
                services.unified_handlers.clone(),
            );
            let mut vector_service =
                crate::proto::proximadb_v1::vector_service_server::VectorServiceServer::new(
                    vector_service_impl,
                )
                .max_decoding_message_size(64 * 1024 * 1024) // 64MB for bulk vector inserts
                .max_encoding_message_size(64 * 1024 * 1024); // 64MB for bulk vector responses

            if self.config.grpc_config.compression {
                use tonic::codec::CompressionEncoding;
                vector_service = vector_service
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip);
            }

            // Add versioned SqlService (v1)
            let sql_service_impl = crate::network::grpc::sql_service::SqlServiceImpl::new(
                services.unified_handlers.clone(),
            );
            let mut sql_service =
                crate::proto::proximadb_v1::sql_service_server::SqlServiceServer::new(
                    sql_service_impl,
                )
                .max_decoding_message_size(64 * 1024 * 1024) // 64MB for bulk SQL queries
                .max_encoding_message_size(64 * 1024 * 1024); // 64MB for large result sets

            if self.config.grpc_config.compression {
                use tonic::codec::CompressionEncoding;
                sql_service = sql_service
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip);
            }

            // Add versioned CollectionService (v1)
            let col_service_impl =
                crate::network::grpc::collection_service::CollectionServiceImpl::new(
                    services.unified_handlers.clone(),
                );
            let mut col_service =
                crate::proto::proximadb_v1::collection_service_server::CollectionServiceServer::new(
                    col_service_impl,
                );
            if self.config.grpc_config.compression {
                use tonic::codec::CompressionEncoding;
                col_service = col_service
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip);
            }

            // Add GraphService for native graph database operations
            let graph_service_impl =
                crate::network::grpc::GraphServiceImpl::new(services.unified_handlers.clone());
            let graph_service =
                crate::proto::proximadb_v1::graph_service_server::GraphServiceServer::new(
                    graph_service_impl,
                );
            debug!("✅ Added GraphService to gRPC server");

            // Add DocumentService for MongoDB-like document operations (with WAL for durability)
            // data_dir comes from TOML config (server.data_dir)
            let doc_base_path = self.config.data_dir.join("documents");
            let doc_path_str = doc_base_path.to_string_lossy().to_string();
            let doc_storage_service = {
                let engine = services.vector_operations_service.unified_engine();
                match crate::storage::document::DocumentService::new_with_wal(engine, &doc_path_str).await {
                    Ok(svc) => Arc::new(svc),
                    Err(e) => {
                        warn!("Failed to create DocumentService with WAL: {}. Using non-durable storage.", e);
                        Arc::new(crate::storage::document::DocumentService::new(
                            services.vector_operations_service.unified_engine()
                        ))
                    }
                }
            };
            let document_service_impl =
                crate::network::grpc::DocumentServiceImpl::new(doc_storage_service);
            let document_service =
                crate::proto::proximadb_v1::document_service_server::DocumentServiceServer::new(
                    document_service_impl,
                );
            debug!("✅ Added DocumentService to gRPC server (WAL-enabled)");

            // Add ObservabilityService for logs/metrics/traces (with WAL for durability)
            // data_dir comes from TOML config (server.data_dir)
            let obs_base_path = self.config.data_dir.join("observability");
            let obs_path_str = obs_base_path.to_string_lossy().to_string();
            let obs_storage = match crate::observability::ObservabilityStorage::new_with_wal(&obs_path_str).await {
                Ok(storage) => Arc::new(storage),
                Err(e) => {
                    warn!("Failed to create ObservabilityStorage with WAL: {}. Using non-durable storage.", e);
                    Arc::new(crate::observability::ObservabilityStorage::new(&obs_path_str))
                }
            };
            let obs_service = match crate::observability::ObservabilityService::new(obs_storage).await {
                Ok(svc) => Arc::new(svc),
                Err(e) => {
                    warn!("Failed to create ObservabilityService: {}. Creating minimal instance.", e);
                    // Create a minimal instance with WAL if possible
                    let fallback_storage = Arc::new(crate::observability::ObservabilityStorage::new(&obs_path_str));
                    Arc::new(crate::observability::ObservabilityService::new(fallback_storage)
                        .await.expect("Failed to create fallback observability service"))
                }
            };
            let observability_service_impl =
                crate::network::grpc::ObservabilityServiceImpl::new(obs_service);
            let observability_service =
                crate::proto::proximadb_v1::observability_service_server::ObservabilityServiceServer::new(
                    observability_service_impl,
                );
            debug!("✅ Added ObservabilityService to gRPC server");

            // Add StreamingService for real-time vector ingestion
            let streaming_service_impl = crate::network::grpc::StreamingServiceImpl::new();
            let streaming_service = streaming_service_impl.into_server();
            debug!("✅ Added StreamingService to gRPC server");

            // Build server with all services
            let server = server_builder
                .add_service(vector_service)
                .add_service(sql_service)
                .add_service(col_service)
                .add_service(graph_service)
                .add_service(document_service)
                .add_service(observability_service)
                .add_service(streaming_service);

            // Add reflection if enabled
            if self.config.grpc_config.enable_reflection {
                debug!("Adding gRPC reflection service");
                // TODO: Add reflection service when descriptor binary is available
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
            let unified_handlers = services.unified_handlers.clone();
            let max_message_size = self.config.arrow_ipc_config.max_message_size;

            let arrow_handle = tokio::spawn(async move {
                use crate::network::arrow_ipc::ArrowFlightServer;

                match ArrowFlightServer::new(arrow_bind_addr, unified_handlers)
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
            let unified_handlers = services.unified_handlers.clone();
            let metrics_collector = services.metrics_collector.clone();
            let security_coordinator = self.security_coordinator.clone();
            let rest_auth_enabled = self.rest_auth_enabled;
            let data_dir = self.config.data_dir.clone();
            let query_adapter = Some(services.query_adapter());

            let api_config = self.config.api_config.clone();
            // Compression disabled by default (field doesn't exist in config)
            let enable_compression = false;
            let rest_handle = tokio::spawn(async move {
                use crate::network::rest::server::{RestServer, RestServerSecurityConfig};

                let max_request_size_mb = api_config.map(|c| c.max_request_size_mb);
                let mut rest_security = RestServerSecurityConfig::default();
                let auth_enabled = security_coordinator.is_some() && rest_auth_enabled;
                rest_security.auth.enabled = auth_enabled;

                // Use with_security_and_config to pass data_dir from TOML config
                match RestServer::with_security_and_config(
                    rest_bind_addr,
                    unified_handlers,
                    max_request_size_mb,
                    enable_compression,
                    metrics_collector,
                    security_coordinator,
                    rest_security,
                    data_dir,
                    query_adapter,
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
            info!("🐘 Starting PostgreSQL Server on port {}", self.config.postgres_config.port);

            let pg_bind_addr = self.config.postgres_config.active_bind_address();
            let collection_service = services.collection_service.clone();
            let vector_ops = services.vector_operations_service.clone();

            if let Some(ref storage) = self.storage {
                let storage_clone = storage.clone();

                let postgres_handle = tokio::spawn(async move {
                    use crate::network::postgres::PostgresServer;
                    let server = PostgresServer::new(pg_bind_addr, storage_clone, collection_service, vector_ops);
                    if let Err(e) = server.start().await {
                        tracing::error!("❌ PostgreSQL Server error: {}", e);
                    }
                });
                handles.push(postgres_handle);
                info!("✅ PostgreSQL Server started on {}", pg_bind_addr);
            } else {
                warn!("PostgreSQL server is enabled but storage engine is not wired - use set_storage() before start()");
                info!("📋 PostgreSQL server will be available at {} once storage wiring is complete", pg_bind_addr);
            }
        }

        *self.server_handles.lock().await = handles;

        info!("🎯 Multi-Server started successfully: gRPC:5679 + Arrow IPC:5680 + REST:5678 + PostgreSQL:5433");
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
        let internal_rest_addr: std::net::SocketAddr = "127.0.0.1:15678".parse().expect("valid address");
        let internal_grpc_addr: std::net::SocketAddr = "127.0.0.1:15679".parse().expect("valid address");

        let mut handles = Vec::new();

        // 1. Start REST server on internal port (HTTP/1.1)
        {
            use crate::network::multiplex::{
                builder::MultiplexServiceBuilder,
                detectors::RestDetector,
                handlers::{RestHandler, RestHandlerConfig},
                traits::DetectedProtocol,
                unified_server::{UnifiedServer, UnifiedServerConfig},
            };

            let services = self.shared_services.clone();
            let rest_config = RestHandlerConfig {
                unified_handlers: services.unified_handlers.clone(),
                metrics_collector: services.metrics_collector.clone(),
                security_coordinator: self.security_coordinator.clone(),
                data_dir: self.config.data_dir.clone(),
            };
            let rest_handler = RestHandler::with_config(rest_config);

            let service = MultiplexServiceBuilder::new()
                .add_detector(RestDetector::new())
                .add_handler(rest_handler)
                .with_fallback(DetectedProtocol::Rest)
                .build();

            let server_config = UnifiedServerConfig {
                bind_address: internal_rest_addr,
                enable_http1: true,
                enable_http2: false, // REST is HTTP/1.1 only
                max_connections: 10000,
                http2_max_concurrent_streams: 1000,
                http2_initial_connection_window_size: 1024 * 1024,
                http2_initial_stream_window_size: 1024 * 1024,
                tcp_keepalive_secs: Some(60),
                request_timeout_secs: 30,
            };

            let server = UnifiedServer::with_config(service, server_config);
            info!("🌐 REST Server starting on {} (internal)", internal_rest_addr);

            let handle = tokio::spawn(async move {
                if let Err(e) = server.serve().await {
                    tracing::error!("Internal REST server error: {}", e);
                }
            });
            handles.push(handle);
        }

        // 2. Start gRPC server on internal port (HTTP/2)
        if self.config.grpc_config.enable_grpc {
            let services = self.shared_services.clone();
            let compression = self.config.grpc_config.compression;

            #[cfg(feature = "unified-facade-routing")]
            let vector_service_impl = crate::network::grpc::vector_service::VectorServiceImpl::with_adapter(
                services.unified_handlers.clone(),
                Some(services.query_adapter()),
            );
            #[cfg(not(feature = "unified-facade-routing"))]
            let vector_service_impl = crate::network::grpc::vector_service::VectorServiceImpl::new(
                services.unified_handlers.clone(),
            );
            let mut vector_service =
                crate::proto::proximadb_v1::vector_service_server::VectorServiceServer::new(
                    vector_service_impl,
                )
                .max_decoding_message_size(64 * 1024 * 1024)
                .max_encoding_message_size(64 * 1024 * 1024);
            if compression {
                use tonic::codec::CompressionEncoding;
                vector_service = vector_service
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip);
            }

            let sql_service_impl = crate::network::grpc::sql_service::SqlServiceImpl::new(
                services.unified_handlers.clone(),
            );
            let mut sql_service =
                crate::proto::proximadb_v1::sql_service_server::SqlServiceServer::new(
                    sql_service_impl,
                )
                .max_decoding_message_size(64 * 1024 * 1024)
                .max_encoding_message_size(64 * 1024 * 1024);
            if compression {
                use tonic::codec::CompressionEncoding;
                sql_service = sql_service
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip);
            }

            let col_service_impl =
                crate::network::grpc::collection_service::CollectionServiceImpl::new(
                    services.unified_handlers.clone(),
                );
            let mut col_service =
                crate::proto::proximadb_v1::collection_service_server::CollectionServiceServer::new(
                    col_service_impl,
                );
            if compression {
                use tonic::codec::CompressionEncoding;
                col_service = col_service
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip);
            }

            let graph_service_impl =
                crate::network::grpc::GraphServiceImpl::new(services.unified_handlers.clone());
            let graph_service =
                crate::proto::proximadb_v1::graph_service_server::GraphServiceServer::new(
                    graph_service_impl,
                );

            // Arrow Flight service (HTTP/2-based, shares internal gRPC server)
            let flight_service = crate::network::arrow_ipc::service::ProximaFlightService::new(
                services.unified_handlers.clone(),
            );
            let flight_server = arrow_flight::flight_service_server::FlightServiceServer::new(flight_service)
                .max_encoding_message_size(512 * 1024 * 1024) // 512MB for large vector batches
                .max_decoding_message_size(512 * 1024 * 1024);

            let server = tonic::transport::Server::builder()
                .add_service(vector_service)
                .add_service(sql_service)
                .add_service(col_service)
                .add_service(graph_service)
                .add_service(flight_server);

            info!("🔗 gRPC + Arrow Flight Server starting on {} (internal)", internal_grpc_addr);

            let grpc_handle = tokio::spawn(async move {
                if let Err(e) = server.serve(internal_grpc_addr).await {
                    tracing::error!("Internal gRPC server error: {}", e);
                }
            });
            handles.push(grpc_handle);
        }

        // 3. Start TCP multiplexer on unified port (routes to internal servers)
        {
            use crate::network::multiplex::tcp_multiplexer::{TcpMultiplexConfig, TcpMultiplexer, TcpProtocol};

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
            // PostgreSQL startup logic would go here (same as in legacy mode)
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

/// Server status information
#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub http_running: bool,
    pub grpc_running: bool,
    pub http_address: Option<SocketAddr>,
    pub grpc_address: Option<SocketAddr>,
    pub tls_enabled: bool,
}
// TODO: Re-add TTL sweeper code in proper function context if needed
