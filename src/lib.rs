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

//! # ProximaDB - Cloud-Native Vector Database
//!
//! **proximity at scale**
//!
//! ProximaDB is a high-performance, cloud-native vector database engineered for AI-first applications.
//! Built from the ground up for serverless deployment, intelligent data tiering, and global scale.
//!
//! ## Key Features
//!
//! - **Proximity at Scale**: SIMD-optimized vector operations with GPU acceleration
//! - **Serverless-Native**: Scale to zero, pay per use
//! - **Intelligent Tiering**: MMAP hot data, S3 cold storage
//! - **Multi-Cloud**: AWS, Azure, GCP support
//! - **Global Distribution**: Multi-region with data residency
//! - **Enterprise Ready**: RBAC, audit logs, compliance

pub mod api;
pub mod compute;
// pub mod consensus;  // Disabled - requires raft dependency
pub mod core;
// pub mod distributed;  // Temporarily disabled for single-node optimization
pub mod handlers;
pub mod index;
pub mod monitoring;
pub mod network;
pub mod proto;
pub mod query;
pub mod schema;
// NOTE: schema_constants module removed - using hardcoded schema_types.rs instead
// schema_types removed - use core::avro_unified instead
pub mod server;
pub mod services;
pub mod storage;
pub mod utils;

// NOTE: Compiled Avro schemas disabled - using hardcoded schema_types.rs instead
// pub mod compiled_schemas {
//     include!(concat!(env!("OUT_DIR"), "/compiled_schemas.rs"));
// }

pub use core::*;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Main ProximaDB database instance
pub struct ProximaDB {
    storage: Arc<RwLock<storage::StorageEngine>>,
    // consensus: consensus::ConsensusEngine,  // Disabled - requires raft dependency
    _query_engine: query::QueryEngine,
    multi_server: Option<network::MultiServer>,
    _config: core::Config,
}

impl ProximaDB {
    pub async fn new(config: core::Config) -> Result<Self> {
        tracing::info!("🚀 ProximaDB::new - STARTING database initialization");
        tracing::debug!("🔍 ProximaDB::new - Config: {:?}", config);

        // Create a temporary collection service with proper metadata config
        // SharedServices will create the real one with same config
        use crate::services::collection_service::CollectionService;
        use crate::storage::metadata::backends::filestore_backend::{
            FilestoreMetadataBackend, FilestoreMetadataConfig,
        };
        use crate::storage::persistence::filesystem::FilesystemFactory;
        use std::collections::HashMap;
        
        tracing::debug!("🔧 ProximaDB::new - Creating filesystem factory...");

        // Use metadata URL from storage config
        let metadata_url = &config.storage.metadata_url;
        tracing::info!("📂 Using metadata URL: {}", metadata_url);
        
        let filestore_config = FilestoreMetadataConfig {
            storage_url: metadata_url.clone(),
            enable_compression: true,
            enable_snapshots: true,
            snapshot_threshold: 1000,
            keep_snapshots: 5,
            backup_url: None,
            temp_dir: None,
        };
        // Create a default filesystem config for the factory
        // The actual filesystem backends will be configured based on storage URLs
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig {
            default_fs: Some(config.storage.metadata_url.clone()),
            s3: None,
            azure: None,
            gcs: None,
            local: None,
            hdfs: None,
            performance_config: Default::default(),
            auth_config: None,
            scheme_mapping: HashMap::new(),
            global_options: Default::default(),
        };

        let filesystem_factory = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .map_err(|e| format!("Failed to create filesystem factory: {}", e))?,
        );
        tracing::debug!("✅ ProximaDB::new - Filesystem factory created successfully");

        tracing::debug!("🔧 ProximaDB::new - Creating filestore backend...");
        let filestore_backend = Arc::new(
            FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .map_err(|e| format!("Failed to create filestore backend: {}", e))?,
        );
        tracing::debug!("✅ ProximaDB::new - Filestore backend created successfully");

        tracing::debug!("🔧 ProximaDB::new - Creating collection service...");
        let collection_service = Arc::new(CollectionService::new(filestore_backend, config.storage.clone()).await?);
        tracing::debug!("✅ ProximaDB::new - Collection service created successfully");

        tracing::debug!("🔧 ProximaDB::new - Creating storage engine...");
        let storage_engine =
            storage::StorageEngine::new(config.storage.clone(), collection_service.clone()).await?;
        tracing::info!("✅ ProximaDB::new - Storage engine created successfully");
        let storage = Arc::new(RwLock::new(storage_engine));

        // let consensus = consensus::ConsensusEngine::new(config.consensus.clone()).await?; // Disabled

        // Note: query_engine needs to be updated to work with Arc<RwLock<StorageEngine>>
        // For now, we'll create a placeholder
        tracing::debug!("🔧 ProximaDB::new - Creating query engine...");
        let query_engine = query::QueryEngine::new_placeholder().await?;
        tracing::debug!("✅ ProximaDB::new - Query engine created successfully");

        // Create multi-server configuration from actual config values
        use std::net::SocketAddr;
        tracing::debug!("🔧 ProximaDB::new - Creating server addresses...");
        let rest_addr: SocketAddr =
            format!("{}:{}", config.server.bind_address, config.api.rest_port)
                .parse()
                .map_err(|e| format!("Invalid REST address: {}", e))?;
        let grpc_addr: SocketAddr =
            format!("{}:{}", config.server.bind_address, config.api.grpc_port)
                .parse()
                .map_err(|e| format!("Invalid gRPC address: {}", e))?;
        tracing::debug!("🔧 ProximaDB::new - REST address: {}, gRPC address: {}", rest_addr, grpc_addr);

        tracing::debug!("🔧 ProximaDB::new - Building multi-server configuration...");
        let mut builder = network::MultiServerBuilder::custom()
            .http(|h| h.bind_address(rest_addr))
            .grpc(|g| g.bind_address(grpc_addr));

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

        // Create metrics collector for monitoring
        tracing::debug!("🔧 ProximaDB::new - Creating metrics collector...");
        let metrics_config = monitoring::metrics::MetricsConfig::default();
        let (metrics_collector, _receiver) = monitoring::MetricsCollector::new(metrics_config)?;
        let metrics_collector = Arc::new(metrics_collector);
        tracing::debug!("✅ ProximaDB::new - Metrics collector created successfully");

        // Create SharedServices first with metadata configuration (business logic hub)
        tracing::info!(
            "🔧 ProximaDB::new: Creating SharedServices with metadata URL: {}",
            config.storage.metadata_url
        );
        tracing::debug!("🔧 ProximaDB::new - About to create SharedServices...");
        let shared_services = network::multi_server::SharedServices::new(
            storage.clone(),
            Some(metrics_collector),
            &config.storage,
        )
        .await?;
        tracing::info!("✅ ProximaDB::new: SharedServices created successfully");

        // Create MultiServer with SharedServices (network orchestrator)
        tracing::debug!("🔧 ProximaDB::new - Creating MultiServer...");
        let multi_server = network::MultiServer::new(multi_config, shared_services);
        tracing::debug!("✅ ProximaDB::new - MultiServer created successfully");

        Ok(Self {
            storage,
            // consensus,  // Disabled
            _query_engine: query_engine,
            multi_server: Some(multi_server),
            _config: config,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("🚀 ProximaDB::start - Starting database services...");
        
        // Start storage engine
        tracing::debug!("🔧 ProximaDB::start - Starting storage engine...");
        {
            let mut storage = self.storage.write().await;
            storage.start().await?;
        }
        tracing::debug!("✅ ProximaDB::start - Storage engine started successfully");

        // Start consensus engine (disabled)
        // self.consensus.start().await?;

        // Start multi-server (HTTP and gRPC on separate ports)
        tracing::debug!("🔧 ProximaDB::start - Starting multi-server...");
        if let Some(ref mut multi_server) = self.multi_server {
            multi_server
                .start()
                .await
                .map_err(|e| format!("Failed to start multi-server: {}", e))?;
        }
        tracing::info!("✅ ProximaDB::start - Multi-server started successfully");

        tracing::info!("🎉 ProximaDB::start - Database startup complete!");
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        // Stop multi-server
        if let Some(ref mut multi_server) = self.multi_server {
            multi_server
                .stop()
                .await
                .map_err(|e| format!("Failed to stop multi-server: {}", e))?;
        }

        // Stop storage engine
        {
            let mut storage = self.storage.write().await;
            storage.stop().await?;
        }

        // Stop consensus engine (disabled)
        // self.consensus.stop().await?;

        Ok(())
    }

    /// Get the multi-server status
    pub async fn server_status(&self) -> Option<network::multi_server::ServerStatus> {
        if let Some(ref multi_server) = self.multi_server {
            Some(multi_server.get_status().await)
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
}
