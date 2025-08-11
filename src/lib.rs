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
pub mod common;  // Shared infrastructure
pub mod compute;
// pub mod consensus;  // Disabled - requires raft dependency
pub mod core;
// pub mod distributed;  // Temporarily disabled for single-node optimization
pub mod api_handlers;
pub mod index;
// Unified metrics module - combines advanced persistent metrics with real-time monitoring
pub mod metrics;
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
    // 🔴 UNUSED FIELD - Query engine never used (placeholder only)
    // _query_engine: query::QueryEngine,
    multi_server: Option<network::MultiServer>,
    _config: core::Config,
}

impl ProximaDB {
    pub async fn new(config: core::Config) -> Result<Self> {
        tracing::info!("🚀 ProximaDB::new - STARTING database initialization");
        tracing::debug!("🔍 ProximaDB::new - Config: {:?}", config);

        // Step 1: Create metrics collector first
        tracing::debug!("🔧 ProximaDB::new - Creating metrics collector...");
        let metrics_config = metrics::MetricsConfig::default();
        let metrics_collector = Arc::new(monitoring::MetricsCollector::new());
        tracing::debug!("✅ ProximaDB::new - Metrics collector created successfully");

        // Step 2: Create SharedServices FIRST (owns all services)
        // This avoids duplicate CollectionService creation
        tracing::info!("🔧 ProximaDB::new - Creating SharedServices FIRST to avoid circular dependency");
        let (shared_services, collection_service) = network::multi_server::SharedServices::new(
            Some(metrics_collector.clone()),
            &config.storage,
        )
        .await?;
        tracing::info!("✅ ProximaDB::new - SharedServices created with unified CollectionService");

        // Step 3: Create StorageEngine using the CollectionService from SharedServices
        tracing::debug!("🔧 ProximaDB::new - Creating storage engine with injected CollectionService...");
        let storage_engine = storage::StorageEngine::new_without_collection_service(config.storage.clone()).await?;
        // Inject the collection service from SharedServices (no duplicate!)
        storage_engine.set_metadata_provider(collection_service.clone()).await;
        tracing::info!("✅ ProximaDB::new - Storage engine created with SharedServices' CollectionService");
        let storage = Arc::new(RwLock::new(storage_engine));

        // let consensus = consensus::ConsensusEngine::new(config.consensus.clone()).await?; // Disabled

        // 🔴 UNUSED MODULE - Query engine is only a placeholder
        // The entire SQL engine infrastructure appears unused
        // Vector search functionality is handled by VectorOperationsService
        // Note: query_engine needs to be updated to work with Arc<RwLock<StorageEngine>>
        // For now, we'll create a placeholder
        // tracing::debug!("🔧 ProximaDB::new - Creating query engine...");
        // let query_engine = query::QueryEngine::new_placeholder().await?;
        // tracing::debug!("✅ ProximaDB::new - Query engine created successfully");

        // Create multi-server configuration from actual config values
        use std::net::SocketAddr;
use tracing::{debug, error, info, warn};
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
            // _query_engine: query_engine,  // Commented out - unused
            multi_server: Some(multi_server),
            _config: config,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("🚀 ProximaDB::start - Starting database services...");
        
        // Step 1: Start storage engine (recovers collections from metadata)
        tracing::info!("📦 ProximaDB::start - Step 1: Starting storage engine for collection recovery...");
        {
            let mut storage = self.storage.write().await;
            storage.start().await?;
        }
        tracing::info!("✅ ProximaDB::start - Storage engine started, collections recovered from metadata");

        // Step 2: Recover assignments from collection metadata
        tracing::info!("🗺️ ProximaDB::start - Step 2: Recovering assignments from collection metadata...");
        // TODO: When AssignmentService is added to SharedServices, call:
        // self.multi_server.as_ref().unwrap().shared_services.assignment_service.recover_assignments().await?;
        tracing::info!("✅ ProximaDB::start - Assignment recovery completed (or skipped if no service)");

        // Step 3: Recover vectors from write buffer
        tracing::info!("🔄 ProximaDB::start - Step 3: Recovering vectors from write buffer...");
        if let Some(ref multi_server) = self.multi_server {
            multi_server.shared_services.recover_vectors_from_write_buffer(&self.storage).await?;
        }
        
        // Step 4: Start multi-server (HTTP and gRPC on separate ports)
        tracing::info!("🌐 ProximaDB::start - Step 4: Starting multi-server (gRPC:5679 + REST:5678)...");
        if let Some(ref mut multi_server) = self.multi_server {
            multi_server
                .start()
                .await
                .map_err(|e| format!("Failed to start multi-server: {}", e))?;
        }
        tracing::info!("✅ ProximaDB::start - Multi-server started successfully");

        tracing::info!("🎉 ProximaDB::start - Database startup complete with proper recovery order!");
        tracing::info!("📋 Recovery Order Summary:");
        tracing::info!("  1️⃣ Collections: Recovered from metadata snapshots");
        tracing::info!("  2️⃣ Assignments: Recovered from collection metadata"); 
        tracing::info!("  3️⃣ Vectors: Recovered from write buffer");
        tracing::info!("  4️⃣ Services: HTTP/gRPC servers started");
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
