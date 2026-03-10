// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified server that serves multiple protocols on a single port
//!
//! This module provides the main entry point for running ProximaDB's
//! unified port architecture.

use super::service::MultiplexService;
use hyper::Body;
use hyper::http::{Request, Response};
use hyper::server::conn::Http;
use hyper::service::Service;
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

/// Configuration for the unified server
#[derive(Debug, Clone)]
pub struct UnifiedServerConfig {
    /// Bind address (e.g., "0.0.0.0:5678")
    pub bind_address: SocketAddr,
    /// Enable HTTP/1.1 support (for REST)
    pub enable_http1: bool,
    /// Enable HTTP/2 support (for gRPC and Arrow Flight)
    pub enable_http2: bool,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// HTTP/2 max concurrent streams
    pub http2_max_concurrent_streams: u32,
    /// HTTP/2 initial connection window size
    pub http2_initial_connection_window_size: u32,
    /// HTTP/2 initial stream window size
    pub http2_initial_stream_window_size: u32,
    /// TCP keepalive interval in seconds
    pub tcp_keepalive_secs: Option<u64>,
    /// Request timeout in seconds
    pub request_timeout_secs: u64,
}

impl Default for UnifiedServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:5678"
                .parse()
                .unwrap_or_else(|_| std::net::SocketAddr::from(([0, 0, 0, 0], 5678))),
            enable_http1: true,
            enable_http2: true,
            max_connections: 10000,
            http2_max_concurrent_streams: 1000,
            http2_initial_connection_window_size: 1024 * 1024, // 1MB
            http2_initial_stream_window_size: 1024 * 1024,     // 1MB
            tcp_keepalive_secs: Some(60),
            request_timeout_secs: 30,
        }
    }
}

/// Wrapper to adapt MultiplexService for hyper 0.14
#[derive(Clone)]
struct HyperService {
    inner: MultiplexService,
}

impl Service<Request<Body>> for HyperService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        // Convert hyper::Body to axum::body::Body for the MultiplexService
        let (parts, body) = request.into_parts();
        let axum_body = axum::body::Body::from(body);
        let request = Request::from_parts(parts, axum_body);

        let mut service = self.inner.clone();
        Box::pin(async move {
            let response = tower::Service::call(&mut service, request).await;
            match response {
                Ok(resp) => {
                    // The response body is axum::body::Body which is compatible with hyper::Body
                    // in axum 0.6 + hyper 0.14
                    let (parts, body) = resp.into_parts();
                    // Convert axum::body::Body back to hyper::Body
                    let hyper_body = Body::wrap_stream(body);
                    Ok(Response::from_parts(parts, hyper_body))
                }
                Err(infallible) => match infallible {},
            }
        })
    }
}

/// Unified server that serves REST, gRPC, and Arrow Flight on a single port
pub struct UnifiedServer {
    /// The multiplex service handling protocol routing
    service: MultiplexService,
    /// Server configuration
    config: UnifiedServerConfig,
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver
    shutdown_rx: watch::Receiver<bool>,
}

impl UnifiedServer {
    /// Create a new unified server
    pub fn new(service: MultiplexService) -> Self {
        Self::with_config(service, UnifiedServerConfig::default())
    }

    /// Create a new unified server with custom configuration
    pub fn with_config(service: MultiplexService, config: UnifiedServerConfig) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            service,
            config,
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// Get a shutdown handle that can be used to stop the server
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            tx: self.shutdown_tx.clone(),
        }
    }

    /// Start the server and run until shutdown is signaled
    pub async fn serve(self) -> Result<(), UnifiedServerError> {
        let listener = TcpListener::bind(self.config.bind_address)
            .await
            .map_err(|e| UnifiedServerError::Bind(e.to_string()))?;

        info!(
            address = %self.config.bind_address,
            protocols = ?self.service.supported_protocols(),
            "Unified server started"
        );

        self.serve_with_listener(listener).await
    }

    /// Start the server with an existing listener
    pub async fn serve_with_listener(
        self,
        listener: TcpListener,
    ) -> Result<(), UnifiedServerError> {
        let service = Arc::new(self.service);
        let config = Arc::new(self.config);
        let mut shutdown_rx = self.shutdown_rx;

        let mut connection_count = 0usize;

        loop {
            tokio::select! {
                // Check for shutdown signal
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Shutdown signal received, stopping unified server");
                        break;
                    }
                }

                // Accept new connections
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, addr)) => {
                            if connection_count >= config.max_connections {
                                warn!(
                                    current = connection_count,
                                    max = config.max_connections,
                                    "Max connections reached, rejecting connection"
                                );
                                continue;
                            }

                            connection_count += 1;
                            debug!(
                                peer_addr = %addr,
                                connection_count,
                                "Accepted connection"
                            );

                            // Clone what we need for the connection handler
                            let svc = (*service).clone();
                            let cfg = Arc::clone(&config);

                            // Spawn a task to handle this connection
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, addr, svc, cfg).await {
                                    debug!(error = %e, peer_addr = %addr, "Connection error");
                                }
                            });
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to accept connection");
                        }
                    }
                }
            }
        }

        info!("Unified server stopped");
        Ok(())
    }

    /// Get the configured bind address
    pub fn bind_address(&self) -> SocketAddr {
        self.config.bind_address
    }
}

/// Handle an individual connection
async fn handle_connection(
    stream: tokio::net::TcpStream,
    addr: SocketAddr,
    service: MultiplexService,
    config: Arc<UnifiedServerConfig>,
) -> Result<(), UnifiedServerError> {
    // Configure TCP stream
    stream
        .set_nodelay(true)
        .map_err(|e| UnifiedServerError::Connection(e.to_string()))?;

    // Wrap the service for hyper 0.14 compatibility
    let hyper_service = HyperService { inner: service };

    // Build the HTTP connection handler
    let mut http = Http::new();

    // Configure HTTP/1.1 and HTTP/2 support
    if config.enable_http1 {
        http.http1_only(false);
    }
    if config.enable_http2 {
        http.http2_only(false);
        http.http2_max_concurrent_streams(config.http2_max_concurrent_streams);
        http.http2_initial_connection_window_size(config.http2_initial_connection_window_size);
        http.http2_initial_stream_window_size(config.http2_initial_stream_window_size);
    }

    // Serve the connection
    http.serve_connection(stream, hyper_service)
        .await
        .map_err(|e| UnifiedServerError::Connection(e.to_string()))?;

    debug!(peer_addr = %addr, "Connection closed");
    Ok(())
}

/// Handle for signaling server shutdown
#[derive(Clone)]
pub struct ShutdownHandle {
    tx: watch::Sender<bool>,
}

impl ShutdownHandle {
    /// Signal the server to shutdown
    pub fn shutdown(&self) {
        let _ = self.tx.send(true);
    }
}

/// Errors that can occur in the unified server
#[derive(Debug)]
pub enum UnifiedServerError {
    /// Failed to bind to address
    Bind(String),
    /// Connection error
    Connection(String),
    /// TLS error
    Tls(String),
    /// Internal error
    Internal(String),
}

impl std::fmt::Display for UnifiedServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnifiedServerError::Bind(msg) => write!(f, "bind error: {}", msg),
            UnifiedServerError::Connection(msg) => write!(f, "connection error: {}", msg),
            UnifiedServerError::Tls(msg) => write!(f, "TLS error: {}", msg),
            UnifiedServerError::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for UnifiedServerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::multiplex::builder::MultiplexServiceBuilder;
    use crate::network::multiplex::detectors::RestDetector;
    use crate::network::multiplex::handlers::RestHandler;
    use crate::network::multiplex::traits::DetectedProtocol;

    fn create_test_service() -> MultiplexService {
        MultiplexServiceBuilder::new()
            .add_detector(RestDetector::new())
            .add_handler(RestHandler::ready())
            .with_fallback(DetectedProtocol::Rest)
            .build()
    }

    #[test]
    fn test_unified_server_config_default() {
        let config = UnifiedServerConfig::default();
        assert!(config.enable_http1);
        assert!(config.enable_http2);
        assert_eq!(config.max_connections, 10000);
    }

    #[tokio::test]
    async fn test_shutdown_handle() {
        let service = create_test_service();
        let server = UnifiedServer::new(service);

        let handle = server.shutdown_handle();

        // Shutdown should not panic
        handle.shutdown();
    }

    #[test]
    fn test_unified_server_bind_address() {
        let service = create_test_service();
        let config = UnifiedServerConfig {
            bind_address: "127.0.0.1:9999".parse().expect("valid address"),
            ..Default::default()
        };
        let server = UnifiedServer::with_config(service, config);

        assert_eq!(
            server.bind_address(),
            "127.0.0.1:9999"
                .parse::<SocketAddr>()
                .expect("valid address")
        );
    }
}
