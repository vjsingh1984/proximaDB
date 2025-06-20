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
pub mod consensus;
pub mod core;
pub mod index;
pub mod monitoring;
pub mod network;
pub mod proto;
pub mod query;
pub mod schema;
// NOTE: schema_constants module removed - using hardcoded schema_types.rs instead
pub mod schema_types;
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
    consensus: consensus::ConsensusEngine,
    _query_engine: query::QueryEngine,
    multi_server: Option<network::MultiServer>,
    _config: core::Config,
}

impl ProximaDB {
    pub async fn new(config: core::Config) -> Result<Self> {
        tracing::info!("🚀 ProximaDB::new - STARTING database initialization");
        let storage_engine = storage::StorageEngine::new(config.storage.clone()).await?;
        tracing::info!("✅ ProximaDB::new - Storage engine created successfully");
        let storage = Arc::new(RwLock::new(storage_engine));

        let consensus = consensus::ConsensusEngine::new(config.consensus.clone()).await?;

        // Note: query_engine needs to be updated to work with Arc<RwLock<StorageEngine>>
        // For now, we'll create a placeholder
        let query_engine = query::QueryEngine::new_placeholder().await?;

        // Create multi-server configuration from actual config values
        use std::net::SocketAddr;
        let rest_addr: SocketAddr = format!("{}:{}", config.server.bind_address, config.api.rest_port)
            .parse()
            .map_err(|e| format!("Invalid REST address: {}", e))?;
        let grpc_addr: SocketAddr = format!("{}:{}", config.server.bind_address, config.api.grpc_port)
            .parse()
            .map_err(|e| format!("Invalid gRPC address: {}", e))?;

        let mut builder = network::MultiServerBuilder::custom()
            .http(|h| h.bind_address(rest_addr))
            .grpc(|g| g.bind_address(grpc_addr));

        // Add TLS configuration if enabled
        if config.api.enable_tls.unwrap_or(false) && config.tls.is_some() {
            let tls_config = config.tls.as_ref().unwrap();
            if let (Some(cert_file), Some(key_file)) = (&tls_config.cert_file, &tls_config.key_file) {
                builder = builder.with_tls(cert_file.clone(), key_file.clone());
            }
        }

        let multi_config = builder.build()
            .map_err(|e| format!("Failed to create server config: {}", e))?;

        // Create metrics collector for monitoring
        let metrics_config = monitoring::metrics::MetricsConfig::default();
        let (metrics_collector, _receiver) = monitoring::MetricsCollector::new(metrics_config)?;
        let metrics_collector = Arc::new(metrics_collector);

        // Create SharedServices first with metadata configuration (business logic hub)
        tracing::info!("🔧 ProximaDB::new: Creating SharedServices with metadata config: {:?}", 
                      config.storage.metadata_backend.as_ref().map(|c| &c.storage_url));
        let shared_services = network::multi_server::SharedServices::new(
            storage.clone(),
            Some(metrics_collector),
            config.storage.metadata_backend.clone()
        ).await?;
        tracing::info!("✅ ProximaDB::new: SharedServices created successfully");

        // Create MultiServer with SharedServices (network orchestrator)
        let multi_server = network::MultiServer::new(multi_config, shared_services);

        Ok(Self {
            storage,
            consensus,
            _query_engine: query_engine,
            multi_server: Some(multi_server),
            _config: config,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        // Start storage engine
        {
            let mut storage = self.storage.write().await;
            storage.start().await?;
        }

        // Start consensus engine
        self.consensus.start().await?;

        // Start multi-server (HTTP and gRPC on separate ports)
        if let Some(ref mut multi_server) = self.multi_server {
            multi_server
                .start()
                .await
                .map_err(|e| format!("Failed to start multi-server: {}", e))?;
        }

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

        // Stop consensus engine
        self.consensus.stop().await?;

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
