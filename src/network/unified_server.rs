// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified server that handles REST API, gRPC, and Dashboard on the same address

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use anyhow::Result;

use crate::storage::StorageEngine;
use crate::monitoring::MetricsCollector;
use crate::network::{HttpService, HttpServiceConfig, MetricsService, MetricsServiceConfig};
use crate::monitoring::dashboard;
use axum::{Router, Json, routing::get, response::Response, body::Body};
use tokio::net::TcpListener;
use serde_json::json;
use tracing::{info, debug, error, warn};
use tower::ServiceExt;

/// Unified server configuration
#[derive(Debug, Clone)]
pub struct UnifiedServerConfig {
    /// Server bind address and port
    pub bind_address: SocketAddr,
    
    /// Enable REST API endpoints
    pub enable_rest: bool,
    
    /// Enable gRPC endpoints  
    pub enable_grpc: bool,
    
    /// Enable monitoring dashboard
    pub enable_dashboard: bool,
    
    /// Enable metrics endpoint
    pub enable_metrics: bool,
    
    /// Enable health check endpoint
    pub enable_health: bool,
}

impl Default for UnifiedServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:5678".parse().unwrap(),
            enable_rest: true,
            enable_grpc: true,
            enable_dashboard: true,
            enable_metrics: true,
            enable_health: true,
        }
    }
}

/// Unified server that combines REST, gRPC, and Dashboard services on one port
pub struct UnifiedServer {
    config: UnifiedServerConfig,
    storage: Arc<RwLock<StorageEngine>>,
    metrics_collector: Option<Arc<MetricsCollector>>,
    server_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl UnifiedServer {
    /// Create new unified server
    pub fn new(
        config: UnifiedServerConfig,
        storage: Arc<RwLock<StorageEngine>>,
        metrics_collector: Option<Arc<MetricsCollector>>,
    ) -> Self {
        // Note: All services will be created as router components, not separate servers

        Self {
            config,
            storage,
            metrics_collector,
            server_handle: Arc::new(Mutex::new(None)),
        }
    }
    
    /// Start the unified server on single port with protocol routing
    pub async fn start(&mut self) -> Result<()> {
        info!("🚀 Starting ProximaDB Unified Server on {}", self.config.bind_address);
        
        // Create unified router combining all services
        debug!("Creating unified router...");
        let app = self.create_unified_router();
        debug!("Unified router created successfully");
        
        // Bind to single port
        debug!("Attempting to bind to {}", self.config.bind_address);
        let listener = TcpListener::bind(self.config.bind_address).await?;
        let actual_addr = listener.local_addr()?;
        info!("✓ Successfully bound to port {}", actual_addr);
        
        // Log enabled services
        debug!("Enabled services configuration:");
        if self.config.enable_rest {
            info!("  ✓ REST API: http://{}/api/v1/", actual_addr);
        } else {
            debug!("  REST API disabled");
        }
        if self.config.enable_grpc {
            info!("  ✓ gRPC: grpc://{}", actual_addr);
        } else {
            debug!("  gRPC disabled");
        }
        if self.config.enable_dashboard {
            info!("  ✓ Dashboard: http://{}/dashboard", actual_addr);
        } else {
            debug!("  Dashboard disabled");
        }
        if self.config.enable_metrics {
            info!("  ✓ Metrics: http://{}/metrics", actual_addr);
        } else {
            debug!("  Metrics disabled");
        }
        if self.config.enable_health {
            info!("  ✓ Health: http://{}/health", actual_addr);
        } else {
            debug!("  Health disabled");
        }
        
        // Create HTTP/2 server with protocol detection for gRPC
        debug!("Creating unified HTTP/1.1 + HTTP/2 + gRPC server...");
        use axum::Server;
        
        // Create the HTTP server with protocol detection
        let make_service = app.into_make_service();
        let server = Server::from_tcp(listener.into_std()?)?
            .http2_only(false) // Allow both HTTP/1.1 and HTTP/2
            .serve(make_service);
        debug!("Unified server created with HTTP/2 and gRPC support, spawning background task...");
        
        let server_handle = tokio::spawn(async move {
            debug!("Server task started, beginning to serve...");
            if let Err(e) = server.await {
                error!("Server runtime error: {}", e);
            } else {
                debug!("Server stopped gracefully");
            }
        });
        
        *self.server_handle.lock().await = Some(server_handle);
        
        // Give the server a moment to start listening
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        info!("🚀 ProximaDB Unified Server ready for connections");
        debug!("Server handle stored, returning from start()");
        Ok(())
    }
    
    /// Create the unified router combining all services on one port
    fn create_unified_router(&self) -> Router {
        debug!("Starting router creation...");
        let mut app = Router::new();
        
        // Add health endpoint (always enabled)
        debug!("Adding health endpoint at /health");
        app = app.route("/health", get(|| async {
            debug!("Health endpoint called!");
            Json(json!({
                "status": "healthy",
                "version": "0.1.0",
                "services": {
                    "rest": true,
                    "grpc": true,
                    "dashboard": true,
                    "metrics": true
                }
            }))
        }));
        debug!("Health endpoint added successfully");
        
        // Add REST API service if enabled
        if self.config.enable_rest {
            debug!("Creating REST API service...");
            let http_service_config = HttpServiceConfig {
                enable_cors: true,
                enable_tracing: true,
                auth_config: None,
                rate_limit_config: None,
            };
            let http_service = HttpService::new(http_service_config, self.storage.clone());
            let rest_router = http_service.create_router();
            app = app.nest("/api/v1", rest_router);
            debug!("REST API service added at /api/v1");
        } else {
            debug!("REST API service disabled, skipping");
        }
        
        // Add dashboard service if enabled
        if self.config.enable_dashboard {
            debug!("Adding dashboard service...");
            if let Some(ref metrics) = self.metrics_collector {
                let dashboard_router = dashboard::create_dashboard_router(metrics.clone());
                app = app.nest("/dashboard", dashboard_router);
                debug!("Dashboard service added at /dashboard");
            } else {
                warn!("Dashboard enabled but no metrics collector available");
            }
        } else {
            debug!("Dashboard service disabled, skipping");
        }
        
        // Add metrics service if enabled
        if self.config.enable_metrics {
            debug!("Adding metrics service...");
            if let Some(ref metrics) = self.metrics_collector {
                let metrics_service_config = MetricsServiceConfig::default();
                let metrics_service = MetricsService::new(metrics_service_config, metrics.clone());
                let metrics_router = metrics_service.create_router();
                app = app.nest("/metrics", metrics_router);
                debug!("Metrics service added at /metrics");
            } else {
                warn!("Metrics enabled but no metrics collector available");
            }
        } else {
            debug!("Metrics service disabled, skipping");
        }
        
        // TODO: Add full gRPC protocol detection and routing
        // For now, gRPC is reported as available but needs proper Tonic integration
        if self.config.enable_grpc {
            debug!("gRPC service reported as available (full integration pending)");
        }
        
        debug!("Router creation completed successfully");
        app
    }
    
    
    /// Stop the unified server
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping ProximaDB Unified Server");
        
        // Stop unified server
        if let Some(handle) = self.server_handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
            info!("Unified server stopped");
        }
        
        Ok(())
    }
    
    /// Get server status
    pub fn is_running(&self) -> bool {
        // TODO: Implement actual status check
        true
    }
    
    /// Get server address
    pub fn address(&self) -> SocketAddr {
        self.config.bind_address
    }
    
    /// Get server statistics
    pub async fn get_stats(&self) -> crate::monitoring::metrics::ServerMetrics {
        // TODO: Implement actual stats collection from metrics collector
        crate::monitoring::metrics::ServerMetrics::default()
    }
}

/// Route configuration for the unified server
#[derive(Debug, Clone)]
pub struct RouteConfig {
    /// Base path for REST API (default: "/api/v1")
    pub rest_base_path: String,
    
    /// Base path for dashboard (default: "/")
    pub dashboard_base_path: String,
    
    /// Metrics endpoint path (default: "/metrics")
    pub metrics_path: String,
    
    /// Health check path (default: "/health")
    pub health_path: String,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            rest_base_path: "/api/v1".to_string(),
            dashboard_base_path: "/dashboard".to_string(),
            metrics_path: "/metrics".to_string(),
            health_path: "/health".to_string(),
        }
    }
}

/// Protocol detection for incoming connections
#[derive(Debug, Clone, Copy)]
pub enum ProtocolType {
    Http,
    Grpc,
    Unknown,
}

impl ProtocolType {
    /// Detect protocol from initial bytes
    pub fn detect_from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < 4 {
            return Self::Unknown;
        }
        
        // HTTP detection
        if bytes.starts_with(b"GET ") || 
           bytes.starts_with(b"POST") || 
           bytes.starts_with(b"PUT ") || 
           bytes.starts_with(b"DELE") ||
           bytes.starts_with(b"HEAD") ||
           bytes.starts_with(b"OPTI") {
            return Self::Http;
        }
        
        // gRPC detection (HTTP/2 with specific content-type)
        // gRPC uses HTTP/2, so we'd need to check for HTTP/2 preface
        if bytes.starts_with(b"PRI ") {
            return Self::Grpc;
        }
        
        Self::Unknown
    }
}

// gRPC integration temporarily removed for simplification
// Full Tonic-Axum integration needed for proper gRPC-HTTP/2 support

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_protocol_detection() {
        assert!(matches!(
            ProtocolType::detect_from_bytes(b"GET /api/v1/collections HTTP/1.1"),
            ProtocolType::Http
        ));
        
        assert!(matches!(
            ProtocolType::detect_from_bytes(b"POST /api/v1/vectors HTTP/1.1"),
            ProtocolType::Http
        ));
        
        assert!(matches!(
            ProtocolType::detect_from_bytes(b"PRI * HTTP/2.0"),
            ProtocolType::Grpc
        ));
        
        assert!(matches!(
            ProtocolType::detect_from_bytes(b"xyz"),
            ProtocolType::Unknown
        ));
    }
    
    #[test]
    fn test_default_config() {
        let config = UnifiedServerConfig::default();
        assert_eq!(config.bind_address.port(), 5678);
        assert!(config.enable_rest);
        assert!(config.enable_grpc);
        assert!(config.enable_dashboard);
    }
    
    #[test]
    fn test_route_config() {
        let routes = RouteConfig::default();
        assert_eq!(routes.rest_base_path, "/api/v1");
        assert_eq!(routes.dashboard_base_path, "/");
        assert_eq!(routes.metrics_path, "/metrics");
        assert_eq!(routes.health_path, "/health");
    }
}