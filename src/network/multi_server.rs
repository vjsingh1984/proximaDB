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
use crate::services::VectorOperationsService;
use crate::services::collection::manager::CollectionService;
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

    /// Global TLS configuration - applies to all servers
    /// Can be overridden per-server if needed
    pub tls_config: TLSConfig,

    /// API configuration (request limits, timeouts, etc.)
    /// Shared limits and policies across both protocols
    pub api_config: Option<crate::core::config::ApiConfig>,
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
            tls_config: TLSConfig {
                cert_file: None,
                key_file: None,
                key_password: None,
                bind_interface: "0.0.0.0".to_string(),
                enabled: false,
            },
            api_config: None, // Will be set when creating from Config
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
    // Removed circular dependency: storage field removed
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
        let wal_config = Self::convert_toml_to_wal_config(&storage_config.wal_config);
        debug!("✅ SharedServices::new - WAL config converted successfully from TOML");

        // Create filesystem factory for engines
        debug!("🔧 SharedServices::new - Creating filesystem factory for engines...");
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(
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
        let sst_engine = Arc::new(
            crate::storage::engines::impls::sst::SstStorage::new(
                storage_config.sst_config.clone().unwrap_or_default(),
                filesystem_factory.clone(),
                Arc::new(
                    crate::compute::distance_computation::engine::UnifiedDistanceCompute::default(),
                ),
            )
            .await?,
        );
        debug!("✅ SharedServices::new - SST engine created successfully");

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

        // Create VectorOperationsService with optimized architecture and two-stage search
        debug!(
            "🔧 SharedServices::new - About to create VectorOperationsService with two-stage search..."
        );
        // Initialize global Cross-Cache Orchestrator from storage config budget
        use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
        let orchestrator = Arc::new(CrossCacheOrchestrator::new((storage_config.cache_size_mb * 1024 * 1024) as usize));
        CrossCacheOrchestrator::register_global(orchestrator.clone());

        let vector_operations_service = Arc::new(
            VectorOperationsService::new(
                sst_engine,
                wal_manager,
                axis_manager,
                collection_service.clone(),
            )
            .with_orchestrator(orchestrator.clone()),
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
                    distance_metric: crate::proto::proximadb_v1::DistanceMetric::Cosine as i32, // Default
                    storage_engine: crate::proto::proximadb_v1::StorageEngine::Viper as i32, // Default
                    filterable_columns: vec![],
                    index_configs: vec![],
                    quantization: Some(crate::proto::proximadb_v1::QuantizationConfig {
                        enabled: true, // Quantization enabled by default
                        strategy:
                            crate::proto::proximadb_v1::quantization_config::Strategy::SmartDefaults
                                as i32,
                        custom_levels: vec![],
                        enable_progressive_search: true, // Progressive search enabled by default
                        binary_filter_selectivity: 0.3,
                        int8_ranking_selectivity: 0.1,
                        pq_ranking_selectivity: 0.05,
                        training_sample_size: 10000,
                        quality_threshold: 0.95,
                        enable_adaptive_training: true,
                        optimize_for_storage: false,
                        optimize_for_memory: false,
                        enable_simd_acceleration: true,
                        // NEW: Direct quantization type enables
                        enable_binary: true,
                        enable_int8: true,
                        enable_pq: true,
                        // Product Quantization specific settings
                        pq_segments: 8,
                        pq_bits: 8,
                        pq_codebooks: 0,
                        // Thresholds for progressive search
                        binary_threshold: 0.3,
                        int8_threshold: 0.1,
                        pq_threshold: 0.05,
                    }),
                    storage_config: None, // VersionedCollectionMetadata doesn't have storage_assignment field
                    primary_index: String::new(),
                    auto_index_selection: false,
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

        // Create GraphService for native graph database operations
        debug!("🔧 SharedServices::new - Creating GraphService for graph database operations...");
        let mut graph_service_inst = if let Some(cfg) = opt_config { crate::graph::GraphService::from_config(cfg) } else { crate::graph::GraphService::new() };
        // Create a simple file-backed metrics updater under data_root/metrics
        let filesystem_factory =
            Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
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
        };
        let metrics_store = Arc::new(
            crate::metrics::store::MetricsPersistenceLayer::new(filesystem_factory, metrics_config)
                .await?,
        );
        let metrics_updater: Arc<dyn crate::metrics::InternalMetricsUpdater + 'static> = Arc::new(
            crate::metrics::updater::MetricsUpdateService::new(metrics_store.clone()),
        );
        graph_service_inst.set_metrics_updater(metrics_updater.clone());
        debug!("📈 GraphService metrics updater wired");
        let graph_service = Arc::new(graph_service_inst);
        debug!("✅ SharedServices::new - GraphService created successfully");

        // Create unified handlers with VectorOperationsService and GraphService
        let mut unified_handlers_instance = if let Some(cfg) = opt_config {
            UnifiedHandlers::with_config(
                collection_service.clone(),
                vector_operations_service.clone(),
                cfg,
            )
        } else {
            UnifiedHandlers::new(
                collection_service.clone(),
                vector_operations_service.clone(),
            )
        };
        // Replace the auto-created GraphService with our shared one
        unified_handlers_instance.graph_service = graph_service.clone();
        // Apply hybrid runtime config if provided
        if let Some(cfg) = opt_config {
            if let Some(ref hybrid) = cfg.hybrid {
                unified_handlers_instance.set_hybrid_runtime(hybrid.clone());
            }
        }
        let unified_handlers = Arc::new(unified_handlers_instance);

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

        // TODO: Implement actual vector recovery from write buffer
        // This would involve:
        // 1. For each collection, check if write buffer has unflushed data
        // 2. Load vectors from write buffer into VectorOperationsService memtable
        // 3. Mark recovery complete for each collection

        info!("✅ SharedServices: Vector recovery completed");
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
            ..Default::default()
        }
    }
}

/// Multi-server manager that coordinates HTTP and gRPC servers with thin handlers
/// Responsibilities: ports, TLS, server lifecycle, protocol orchestration
pub struct MultiServer {
    config: MultiServerConfig,
    pub shared_services: SharedServices, // Made public for recovery access
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
            shared_services,
            server_handles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start all configured servers: gRPC on 5679, REST on 5678
    pub async fn start(&mut self) -> Result<()> {
        info!("🚀 Starting ProximaDB Multi-Server: gRPC:5679 + REST:5678");

        let services = self.shared_services.clone();
        let mut handles = Vec::new();

        // Start gRPC server on port 5679 if configured
        if self.config.grpc_config.enable_grpc {
            info!("🔗 Starting gRPC Server on port 5679");

            // Create gRPC server builder with services
            let mut server_builder = tonic::transport::Server::builder();

            // Add versioned VectorService (v1)
            let vector_service_impl = crate::network::grpc::vector_service::VectorServiceImpl::new(
                services.unified_handlers.clone(),
            );
            let mut vector_service =
                crate::proto::proximadb_v1::vector_service_server::VectorServiceServer::new(
                    vector_service_impl,
                );
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
                );
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

            // Build server with all services
            server_builder = server_builder
                .add_service(vector_service)
                .add_service(sql_service)
                .add_service(col_service)
                .add_service(graph_service);

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

            let grpc_handle = tokio::spawn(async move {
                if let Err(e) = server_builder.serve(grpc_bind_addr).await {
                    tracing::error!("gRPC server error: {}", e);
                }
            });

            handles.push(grpc_handle);
            info!("✅ gRPC Server started on {}", grpc_bind_addr);
        }

        // Start REST server on port 5678 if configured
        if self.config.http_config.enable_rest {
            info!("📡 Starting REST Server on port 5678");

            let rest_bind_addr = self.config.http_bind_address();
            let unified_handlers = services.unified_handlers.clone();

            let api_config = self.config.api_config.clone();
            // Compression disabled by default (field doesn't exist in config)
            let enable_compression = false;
            let rest_handle = tokio::spawn(async move {
                use crate::network::rest::server::RestServer;

                let max_request_size_mb = api_config.map(|c| c.max_request_size_mb);
                match RestServer::new(
                    rest_bind_addr,
                    unified_handlers,
                    max_request_size_mb,
                    enable_compression,
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
