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
pub mod server;
pub mod services;
pub mod storage;
pub mod utils;

pub use core::*;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Main ProximaDB database instance
pub struct ProximaDB {
    storage: Arc<RwLock<storage::StorageEngine>>,
    consensus: consensus::ConsensusEngine,
    _query_engine: query::QueryEngine,
    unified_server: Option<network::UnifiedServer>,
    _config: core::Config,
}

impl ProximaDB {
    pub async fn new(config: core::Config) -> Result<Self> {
        let storage_engine = storage::StorageEngine::new(config.storage.clone()).await?;
        let storage = Arc::new(RwLock::new(storage_engine));
        
        let consensus = consensus::ConsensusEngine::new(config.consensus.clone()).await?;
        
        // Note: query_engine needs to be updated to work with Arc<RwLock<StorageEngine>>
        // For now, we'll create a placeholder
        let query_engine = query::QueryEngine::new_placeholder().await?;
        
        // Create unified server configuration
        let bind_addr = format!("{}:{}", config.server.bind_address, config.server.port)
            .parse()
            .map_err(|e| format!("Invalid bind address: {}", e))?;
            
        let unified_config = network::unified_server::UnifiedServerConfig {
            bind_address: bind_addr,
            enable_rest: true,
            enable_grpc: true,
            enable_dashboard: true,
            enable_metrics: true,
            enable_health: true,
        };
        
        // Create metrics collector for monitoring
        let metrics_config = monitoring::metrics::MetricsConfig::default();
        let (metrics_collector, _receiver) = monitoring::MetricsCollector::new(metrics_config)?;
        let metrics_collector = Arc::new(metrics_collector);
        
        let unified_server = network::UnifiedServer::new(
            unified_config, 
            storage.clone(),
            Some(metrics_collector)
        );

        Ok(Self {
            storage,
            consensus,
            _query_engine: query_engine,
            unified_server: Some(unified_server),
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
        
        // Start unified server (REST, gRPC, Dashboard)
        if let Some(ref mut unified_server) = self.unified_server {
            unified_server.start().await
                .map_err(|e| format!("Failed to start unified server: {}", e))?;
        }
        
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        // Stop unified server
        if let Some(ref mut unified_server) = self.unified_server {
            unified_server.stop().await
                .map_err(|e| format!("Failed to stop unified server: {}", e))?;
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
    
    /// Get the unified server address
    pub fn server_address(&self) -> Option<String> {
        self.unified_server.as_ref().map(|s| s.address().to_string())
    }
    
    /// Check if the unified server is running
    pub fn is_server_running(&self) -> bool {
        self.unified_server.as_ref().map(|s| s.is_running()).unwrap_or(false)
    }
    
    /// Get server statistics
    pub async fn get_server_stats(&self) -> Option<monitoring::metrics::ServerMetrics> {
        if let Some(ref unified_server) = self.unified_server {
            Some(unified_server.get_stats().await)
        } else {
            None
        }
    }
}