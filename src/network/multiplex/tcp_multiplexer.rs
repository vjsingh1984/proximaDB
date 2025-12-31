// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! TCP-level protocol multiplexer for unified port
//!
//! This module implements TCP-level protocol detection to route connections
//! to either REST (HTTP/1.1) or gRPC (HTTP/2) servers. This approach avoids
//! http crate version conflicts between axum/hyper 0.14 and tonic 0.14.
//!
//! ## How it works
//!
//! 1. Accept TCP connection on unified port
//! 2. Peek first bytes to detect protocol:
//!    - HTTP/2 preface ("PRI * HTTP/2.0...") → gRPC server
//!    - HTTP/1.1 method ("GET ", "POST ", etc.) → REST server
//! 3. Route entire connection to appropriate server
//!
//! This allows REST and gRPC to have independent http stacks while sharing a port.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, error, info, trace, warn};

/// HTTP/2 connection preface (first 24 bytes)
/// "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"
const HTTP2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const HTTP2_PREFACE_LEN: usize = 24;

/// Common HTTP/1.1 method prefixes
const HTTP1_METHODS: &[&[u8]] = &[
    b"GET ",
    b"POST ",
    b"PUT ",
    b"DELETE ",
    b"HEAD ",
    b"OPTIONS ",
    b"PATCH ",
    b"CONNECT ",
    b"TRACE ",
];

/// Detected protocol at TCP level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpProtocol {
    /// HTTP/1.1 (REST)
    Http1,
    /// HTTP/2 (gRPC)
    Http2,
    /// Unknown - use fallback
    Unknown,
}

impl std::fmt::Display for TcpProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TcpProtocol::Http1 => write!(f, "HTTP/1.1"),
            TcpProtocol::Http2 => write!(f, "HTTP/2"),
            TcpProtocol::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Configuration for the TCP multiplexer
#[derive(Debug, Clone)]
pub struct TcpMultiplexConfig {
    /// Bind address for the unified port
    pub bind_address: SocketAddr,
    /// Address of the REST server (HTTP/1.1)
    pub rest_address: SocketAddr,
    /// Address of the gRPC server (HTTP/2)
    pub grpc_address: SocketAddr,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// Fallback protocol for unknown connections
    pub fallback_protocol: TcpProtocol,
    /// Buffer size for connection proxying
    pub proxy_buffer_size: usize,
}

impl Default for TcpMultiplexConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:5678".parse().expect("valid address"),
            rest_address: "127.0.0.1:15678".parse().expect("valid address"), // Internal REST port
            grpc_address: "127.0.0.1:15679".parse().expect("valid address"), // Internal gRPC port
            max_connections: 10000,
            fallback_protocol: TcpProtocol::Http1,
            proxy_buffer_size: 64 * 1024, // 64KB
        }
    }
}

/// TCP-level protocol multiplexer
///
/// Routes TCP connections to REST or gRPC servers based on protocol detection.
pub struct TcpMultiplexer {
    config: TcpMultiplexConfig,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl TcpMultiplexer {
    /// Create a new TCP multiplexer with default configuration
    pub fn new() -> Self {
        Self::with_config(TcpMultiplexConfig::default())
    }

    /// Create a new TCP multiplexer with custom configuration
    pub fn with_config(config: TcpMultiplexConfig) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            config,
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// Get a shutdown handle
    pub fn shutdown_handle(&self) -> TcpMultiplexShutdownHandle {
        TcpMultiplexShutdownHandle {
            tx: self.shutdown_tx.clone(),
        }
    }

    /// Run the TCP multiplexer
    ///
    /// This method:
    /// 1. Binds to the unified port
    /// 2. Accepts connections
    /// 3. Detects protocol and routes to appropriate backend
    pub async fn run(self) -> Result<(), TcpMultiplexError> {
        let listener = TcpListener::bind(self.config.bind_address)
            .await
            .map_err(|e| TcpMultiplexError::Bind(e.to_string()))?;

        info!(
            address = %self.config.bind_address,
            rest_backend = %self.config.rest_address,
            grpc_backend = %self.config.grpc_address,
            "TCP multiplexer started"
        );

        let config = Arc::new(self.config);
        let mut shutdown_rx = self.shutdown_rx;
        let mut connection_count = 0usize;

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("TCP multiplexer shutdown signal received");
                        break;
                    }
                }

                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer_addr)) => {
                            if connection_count >= config.max_connections {
                                warn!(
                                    current = connection_count,
                                    max = config.max_connections,
                                    "Max connections reached"
                                );
                                continue;
                            }

                            connection_count += 1;
                            let cfg = Arc::clone(&config);

                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, peer_addr, cfg).await {
                                    debug!(error = %e, peer = %peer_addr, "Connection error");
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

        info!("TCP multiplexer stopped");
        Ok(())
    }

    /// Get the bind address
    pub fn bind_address(&self) -> SocketAddr {
        self.config.bind_address
    }
}

impl Default for TcpMultiplexer {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle a single connection
async fn handle_connection(
    mut client_stream: TcpStream,
    peer_addr: SocketAddr,
    config: Arc<TcpMultiplexConfig>,
) -> Result<(), TcpMultiplexError> {
    // Peek first bytes to detect protocol
    let mut peek_buf = [0u8; HTTP2_PREFACE_LEN];

    // Read the initial bytes (we'll forward them to the backend)
    let n = client_stream
        .peek(&mut peek_buf)
        .await
        .map_err(|e| TcpMultiplexError::Io(e.to_string()))?;

    if n == 0 {
        return Ok(()); // Client closed before sending data
    }

    // Detect protocol
    let protocol = detect_protocol(&peek_buf[..n]);
    trace!(
        peer = %peer_addr,
        protocol = %protocol,
        bytes_peeked = n,
        "Protocol detected"
    );

    // Determine backend address
    let backend_addr = match protocol {
        TcpProtocol::Http2 => config.grpc_address,
        TcpProtocol::Http1 => config.rest_address,
        TcpProtocol::Unknown => {
            match config.fallback_protocol {
                TcpProtocol::Http1 => config.rest_address,
                TcpProtocol::Http2 => config.grpc_address,
                TcpProtocol::Unknown => config.rest_address, // Default to REST
            }
        }
    };

    // Connect to backend
    let backend_stream = TcpStream::connect(backend_addr)
        .await
        .map_err(|e| TcpMultiplexError::Backend(format!("Failed to connect to {}: {}", backend_addr, e)))?;

    // Proxy the connection bidirectionally
    proxy_connection(client_stream, backend_stream, config.proxy_buffer_size).await
}

/// Detect protocol from peeked bytes
fn detect_protocol(data: &[u8]) -> TcpProtocol {
    // Check for HTTP/2 preface
    if data.len() >= HTTP2_PREFACE_LEN && data.starts_with(HTTP2_PREFACE) {
        return TcpProtocol::Http2;
    }

    // Check for HTTP/2 preface start (may not have full preface yet)
    if data.starts_with(b"PRI ") {
        return TcpProtocol::Http2;
    }

    // Check for HTTP/1.1 methods
    for method in HTTP1_METHODS {
        if data.starts_with(method) {
            return TcpProtocol::Http1;
        }
    }

    // Check if it looks like HTTP/2 settings frame (for prior knowledge HTTP/2)
    // HTTP/2 frames start with: length (3 bytes) + type (1 byte) + flags (1 byte) + stream id (4 bytes)
    // Settings frame type is 0x04
    if data.len() >= 9 && data[3] == 0x04 && data[4] == 0x00 {
        // Settings frame with no flags, likely HTTP/2 prior knowledge
        return TcpProtocol::Http2;
    }

    TcpProtocol::Unknown
}

/// Proxy connection bidirectionally between client and backend
async fn proxy_connection(
    client: TcpStream,
    backend: TcpStream,
    _buffer_size: usize,
) -> Result<(), TcpMultiplexError> {
    let (mut client_read, mut client_write) = client.into_split();
    let (mut backend_read, mut backend_write) = backend.into_split();

    // Spawn tasks for bidirectional copying
    let client_to_backend = tokio::spawn(async move {
        tokio::io::copy(&mut client_read, &mut backend_write).await
    });

    let backend_to_client = tokio::spawn(async move {
        tokio::io::copy(&mut backend_read, &mut client_write).await
    });

    // Wait for either direction to complete
    tokio::select! {
        result = client_to_backend => {
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => debug!(error = %e, "Client to backend copy error"),
                Err(e) => debug!(error = %e, "Client to backend task error"),
            }
        }
        result = backend_to_client => {
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => debug!(error = %e, "Backend to client copy error"),
                Err(e) => debug!(error = %e, "Backend to client task error"),
            }
        }
    }

    Ok(())
}

/// Shutdown handle for the TCP multiplexer
#[derive(Clone)]
pub struct TcpMultiplexShutdownHandle {
    tx: watch::Sender<bool>,
}

impl TcpMultiplexShutdownHandle {
    /// Signal shutdown
    pub fn shutdown(&self) {
        let _ = self.tx.send(true);
    }
}

/// Errors that can occur in the TCP multiplexer
#[derive(Debug)]
pub enum TcpMultiplexError {
    /// Failed to bind
    Bind(String),
    /// Backend connection error
    Backend(String),
    /// IO error
    Io(String),
}

impl std::fmt::Display for TcpMultiplexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TcpMultiplexError::Bind(msg) => write!(f, "bind error: {}", msg),
            TcpMultiplexError::Backend(msg) => write!(f, "backend error: {}", msg),
            TcpMultiplexError::Io(msg) => write!(f, "io error: {}", msg),
        }
    }
}

impl std::error::Error for TcpMultiplexError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_http2_preface() {
        let preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
        assert_eq!(detect_protocol(preface), TcpProtocol::Http2);
    }

    #[test]
    fn test_detect_http2_partial() {
        let partial = b"PRI ";
        assert_eq!(detect_protocol(partial), TcpProtocol::Http2);
    }

    #[test]
    fn test_detect_http1_get() {
        let get = b"GET / HTTP/1.1\r\n";
        assert_eq!(detect_protocol(get), TcpProtocol::Http1);
    }

    #[test]
    fn test_detect_http1_post() {
        let post = b"POST /api/v1/collections HTTP/1.1\r\n";
        assert_eq!(detect_protocol(post), TcpProtocol::Http1);
    }

    #[test]
    fn test_detect_unknown() {
        let unknown = b"UNKNOWN";
        assert_eq!(detect_protocol(unknown), TcpProtocol::Unknown);
    }

    #[test]
    fn test_config_default() {
        let config = TcpMultiplexConfig::default();
        assert_eq!(config.max_connections, 10000);
        assert_eq!(config.fallback_protocol, TcpProtocol::Http1);
    }
}
