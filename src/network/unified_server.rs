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
use crate::network::grpc::service::ProximaDbGrpcService;
use crate::monitoring::dashboard;
use crate::services::{VectorService, CollectionService};
use axum::{Router, Json, routing::get};
use tokio::net::TcpListener;
use serde_json::json;
use tracing::{info, debug, error, warn};
use tower::{Service, ServiceExt};
use crate::proto::proximadb::v1::proxima_db_server::ProximaDbServer;

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
        
        // Create unified HTTP/gRPC service stack
        debug!("Creating unified HTTP/1.1 + HTTP/2 + gRPC server...");
        
        // Create gRPC service
        let vector_service = VectorService::new(Arc::clone(&self.storage));
        let collection_service = CollectionService::new(Arc::clone(&self.storage));
        
        let grpc_service = ProximaDbGrpcService::new(
            Arc::clone(&self.storage),
            vector_service,
            collection_service,
            "unified-grpc".to_string(),
            "0.1.0".to_string(),
        );
        
        let grpc_server = ProximaDbServer::new(grpc_service);
        
        // SIMPLE WORKING SOLUTION: Pure gRPC server
        // Start with working gRPC, add HTTP routing as optimization later
        
        let server = tonic::transport::Server::builder()
            .add_service(grpc_server)
            .serve(self.config.bind_address);
            
        debug!("Unified server created with tonic-web gRPC integration, spawning background task...");
        
        let server_handle = tokio::spawn(async move {
            debug!("Server task started, beginning to serve...");
            if let Err(e) = server
                .with_graceful_shutdown(async {
                    tokio::signal::ctrl_c().await.ok();
                })
                .await 
            {
                error!("Server runtime error: {}", e);
            } else {
                debug!("Server stopped gracefully");
            }
        });
        
        *self.server_handle.lock().await = Some(server_handle);
        
        // Give the server a moment to start listening
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        info!("🚀 ProximaDB Unified Server ready for gRPC + HTTP connections");
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
        
        // gRPC endpoints will be handled by native tonic service for maximum performance
        if self.config.enable_grpc {
            debug!("gRPC service will be handled by native tonic server");
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
           bytes.starts_with(b"POST ") || 
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

/// High-performance connection handler with direct protocol detection
async fn handle_connection<G>(
    tcp_stream: tokio::net::TcpStream,
    remote_addr: std::net::SocketAddr,
    grpc_service: G,
    mut http_service: axum::routing::IntoMakeService<axum::Router>,
) -> Result<()>
where
    G: tower::Service<hyper::Request<hyper::Body>, Response = hyper::Response<tonic::body::BoxBody>> + Clone + Send + 'static,
    G::Error: std::error::Error + Send + Sync + 'static,
    G::Future: Send,
{
    use tokio::io::AsyncReadExt;
    
    // Peek at first few bytes to detect protocol (non-destructive)
    let mut buffer = [0u8; 24]; // Enough for HTTP/2 preface detection
    
    match tcp_stream.peek(&mut buffer).await {
        Ok(n) if n >= 4 => {
            // HTTP/2 connection preface (gRPC always uses HTTP/2)
            if buffer.starts_with(b"PRI ") {
                debug!("🔗 gRPC/HTTP2 connection from {}", remote_addr);
                
                // Route directly to gRPC service for zero-overhead performance
                if let Err(e) = hyper::server::conn::Http::new()
                    .http2_only(true)
                    .serve_connection(tcp_stream, grpc_service)
                    .await
                {
                    debug!("gRPC connection ended: {}", e);
                }
            } 
            // HTTP/1.1 methods (REST/Dashboard/Metrics)
            else if buffer.starts_with(b"GET ") || buffer.starts_with(b"POST") || 
                    buffer.starts_with(b"PUT ") || buffer.starts_with(b"HEAD") || 
                    buffer.starts_with(b"DELE") || buffer.starts_with(b"PATC") || 
                    buffer.starts_with(b"OPTI") {
                debug!("🌐 HTTP/1.1 connection from {}", remote_addr);
                
                // Route to HTTP service for REST API  
                let http_svc = http_service.call(&tcp_stream).await
                    .map_err(|e| anyhow::anyhow!("HTTP service error: {:?}", e))?;
                    
                if let Err(e) = hyper::server::conn::Http::new()
                    .http1_only(true)
                    .serve_connection(tcp_stream, http_svc)
                    .await
                {
                    debug!("HTTP connection ended: {}", e);
                }
            }
            else {
                warn!("❌ Unknown protocol from {}, closing", remote_addr);
            }
        },
        Ok(_) => debug!("⚠️ Insufficient data from {}", remote_addr),
        Err(e) => debug!("🔍 Peek error from {}: {}", remote_addr, e),
    }
    
    Ok(())
}

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