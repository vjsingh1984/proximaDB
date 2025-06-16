// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Network layer implementation for ProximaDB

pub mod grpc;
pub mod http_service;
pub mod metrics_service;
pub mod unified_server;
pub mod middleware;

use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};

use crate::core::VectorDBError;

type Result<T> = std::result::Result<T, VectorDBError>;
type ProximaDBError = VectorDBError;
use crate::storage::StorageEngine;
use crate::consensus::ConsensusEngine;
use crate::monitoring::MetricsCollector;

pub use http_service::*;
pub use metrics_service::*;
pub use unified_server::UnifiedServer;
pub use grpc::service::ProximaDbGrpcService;
pub use middleware::*;

/// Network server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Server bind address
    pub bind_address: String,
    
    /// Server port
    pub port: u16,
    
    /// Enable gRPC server
    pub enable_grpc: bool,
    
    /// Enable REST server
    pub enable_rest: bool,
    
    /// Enable web dashboard
    pub enable_dashboard: bool,
    
    /// Authentication configuration
    pub auth: AuthConfig,
    
    /// Rate limiting configuration
    pub rate_limit: RateLimitConfig,
    
    /// Request timeout in seconds
    pub request_timeout_secs: u64,
    
    /// Maximum request size in bytes
    pub max_request_size: usize,
    
    /// Keep-alive timeout in seconds
    pub keep_alive_timeout_secs: u64,
    
    /// TCP nodelay setting
    pub tcp_nodelay: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0".to_string(),
            port: 5678,
            enable_grpc: true,
            enable_rest: true,
            enable_dashboard: true,
            auth: AuthConfig::default(),
            rate_limit: RateLimitConfig::default(),
            request_timeout_secs: 30,
            max_request_size: 32 * 1024 * 1024, // 32MB
            keep_alive_timeout_secs: 60,
            tcp_nodelay: true,
        }
    }
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Enable authentication
    pub enabled: bool,
    
    /// JWT secret key
    pub jwt_secret: Option<String>,
    
    /// JWT expiration time in seconds
    pub jwt_expiration_secs: u64,
    
    /// API key validation
    pub api_keys: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            jwt_secret: None,
            jwt_expiration_secs: 3600, // 1 hour
            api_keys: Vec::new(),
        }
    }
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting
    pub enabled: bool,
    
    /// Requests per minute per client
    pub requests_per_minute: u32,
    
    /// Burst allowance
    pub burst_size: u32,
    
    /// Rate limit by IP address
    pub by_ip: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_minute: 1000,
            burst_size: 100,
            by_ip: true,
        }
    }
}

/// Network server manager
pub struct NetworkManager {
    /// Server configuration
    config: NetworkConfig,
    
    /// Unified server instance
    server: Option<UnifiedServer>,
    
    /// Storage engine reference
    storage: Arc<RwLock<StorageEngine>>,
    
    /// Consensus engine reference
    consensus: Arc<RwLock<ConsensusEngine>>,
    
    /// Metrics collector
    metrics: Arc<MetricsCollector>,
    
    /// Server handle for shutdown
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl NetworkManager {
    /// Create a new network manager
    pub fn new(
        config: NetworkConfig,
        storage: Arc<RwLock<StorageEngine>>,
        consensus: Arc<RwLock<ConsensusEngine>>,
        metrics: Arc<MetricsCollector>,
    ) -> Self {
        Self {
            config,
            server: None,
            storage,
            consensus,
            metrics,
            server_handle: None,
        }
    }
    
    /// Start the network server
    pub async fn start(&mut self) -> Result<()> {
        let bind_addr = format!("{}:{}", self.config.bind_address, self.config.port);
        let socket_addr: SocketAddr = bind_addr.parse().map_err(|e| {
            ProximaDBError::Network(crate::core::NetworkError::Connection(format!("Invalid bind address: {}", e)))
        })?;
        
        tracing::info!("Starting network server on {}", socket_addr);
        
        // Create unified server config from network config
        let unified_config = unified_server::UnifiedServerConfig {
            bind_address: socket_addr,
            enable_rest: self.config.enable_rest,
            enable_grpc: self.config.enable_grpc,
            enable_dashboard: self.config.enable_dashboard,
            enable_metrics: true,
            enable_health: true,
        };
        
        // Create unified server
        let mut server = UnifiedServer::new(
            unified_config,
            self.storage.clone(),
            Some(self.metrics.clone()),
        );
        
        // Start server
        server.start().await.map_err(|e| {
            ProximaDBError::Network(crate::core::NetworkError::Connection(format!("Server start failed: {}", e)))
        })?;
        
        self.server = Some(server);
        // Note: server_handle not used in direct server approach
        
        tracing::info!("Network server started successfully");
        Ok(())
    }
    
    /// Stop the network server
    pub async fn stop(&mut self) -> Result<()> {
        tracing::info!("Stopping network server...");
        
        if let Some(ref mut server) = self.server {
            server.stop().await.map_err(|e| {
                crate::core::VectorDBError::Internal(format!("Failed to stop server: {}", e))
            })?;
        }
        
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        
        self.server = None;
        
        tracing::info!("Network server stopped");
        Ok(())
    }
    
    /// Check if server is running
    pub fn is_running(&self) -> bool {
        self.server.is_some()
    }
    
    /// Get server statistics
    pub async fn get_stats(&self) -> Option<crate::monitoring::metrics::ServerMetrics> {
        if let Some(server) = &self.server {
            Some(server.get_stats().await)
        } else {
            None
        }
    }
}

// ServerStats struct removed - using crate::monitoring::metrics::ServerMetrics instead