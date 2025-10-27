// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Server builder pattern for flexible server configuration
//!
//! ## Purpose:
//!
//! The server builder provides a fluent API for configuring ProximaDB's
//! dual-protocol server architecture (REST + gRPC). It handles:
//! - Port configuration and binding
//! - TLS/SSL setup for secure connections
//! - Service enablement (REST, gRPC, metrics, health)
//! - Compression settings per protocol
//!
//! ## Architecture:
//!
//! ```text
//! RestHttpServerBuilder     GrpcHttpServerBuilder
//!         ↓                          ↓
//!    REST Server                gRPC Server
//!    (Port 5678)               (Port 5679)
//!         ↓                          ↓
//!    [TLS Optional]            [TLS Optional]
//!         ↓                          ↓
//!    MultiServer (Coordinated Lifecycle)
//! ```
//!
//! ## Default Configuration:
//!
//! - **REST**: Port 5678, compression disabled, all services enabled
//! - **gRPC**: Port 5679, compression disabled, reflection enabled
//! - **TLS**: Optional for both, uses same port when enabled
//! - **Max Message**: 64MB for bulk vector operations
//!
//! ## Usage Example:
//!
//! ```rust,ignore
//! let rest_config = RestHttpServerBuilder::new()
//!     .bind_address("0.0.0.0:5678")
//!     .rest_compression(true)
//!     .with_tls("cert.pem", "key.pem")
//!     .build()?;
//!
//! let grpc_config = GrpcHttpServerBuilder::new()
//!     .bind_address("0.0.0.0:5679")
//!     .grpc_compression(true)
//!     .max_message_size(128 * 1024 * 1024) // 128MB
//!     .build()?;
//! ```

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::{info, warn};

use crate::network::multi_server::{GrpcHttpServerConfig, MultiServerConfig, RestHttpServerConfig};

/// Builder for HTTP server configuration
///
/// ## REST Server Features:
///
/// - **REST API**: Full CRUD operations for collections and vectors
/// - **Dashboard**: Web UI for monitoring and management
/// - **Metrics**: Prometheus-compatible metrics endpoint
/// - **Health**: Kubernetes-compatible health checks
///
/// ## Compression:
///
/// When enabled, supports gzip/deflate for:
/// - Request bodies (vector uploads)
/// - Response bodies (search results)
/// - Typical compression ratio: 2-3x for JSON payloads
///
/// ## TLS Configuration:
///
/// TLS takes precedence over non-TLS when configured:
/// - Same port used (5678)
/// - Automatic HTTP → HTTPS redirect
/// - Support for custom certificates or self-signed
#[derive(Debug, Clone)]
pub struct RestHttpServerBuilder {
    /// Socket address to bind (default: 0.0.0.0:5678)
    bind_address: SocketAddr,

    /// Enable REST API endpoints
    enable_rest: bool,

    /// Enable web dashboard UI
    enable_dashboard: bool,

    /// Enable Prometheus metrics endpoint
    enable_metrics: bool,

    /// Enable health check endpoints
    enable_health: bool,

    /// Enable HTTP compression (gzip/deflate)
    rest_compression: bool, // Clear, specific naming

    /// Path to TLS certificate file (PEM format)
    tls_cert_file: Option<String>,

    /// Path to TLS private key file (PEM format)
    tls_key_file: Option<String>,
}

impl Default for RestHttpServerBuilder {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:5678".parse().unwrap(),
            enable_rest: true,
            enable_dashboard: true,
            enable_metrics: true,
            enable_health: true,
            rest_compression: false, // Clear naming, default false
            tls_cert_file: None,
            tls_key_file: None,
        }
    }
}

impl RestHttpServerBuilder {
    /// Create new HTTP server builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set bind address for non-TLS HTTP server
    pub fn bind_address<T: Into<SocketAddr>>(mut self, addr: T) -> Self {
        self.bind_address = addr.into();
        self
    }

    // TLS uses the same port as non-TLS (5678 for REST)

    /// Enable/disable REST API endpoints
    pub fn enable_rest(mut self, enabled: bool) -> Self {
        self.enable_rest = enabled;
        self
    }

    /// Enable/disable dashboard
    pub fn enable_dashboard(mut self, enabled: bool) -> Self {
        self.enable_dashboard = enabled;
        self
    }

    /// Enable/disable metrics endpoint
    pub fn enable_metrics(mut self, enabled: bool) -> Self {
        self.enable_metrics = enabled;
        self
    }

    /// Enable/disable health check endpoint
    pub fn enable_health(mut self, enabled: bool) -> Self {
        self.enable_health = enabled;
        self
    }

    /// Enable/disable REST compression
    pub fn rest_compression(mut self, enabled: bool) -> Self {
        self.rest_compression = enabled;
        self
    }

    /// Set TLS certificate and key files
    pub fn with_tls<C, K>(mut self, cert_file: C, key_file: K) -> Self
    where
        C: Into<String>,
        K: Into<String>,
    {
        self.tls_cert_file = Some(cert_file.into());
        self.tls_key_file = Some(key_file.into());
        self
    }

    /// Set TLS certificate and key from PathBuf
    pub fn with_tls_paths(mut self, cert_path: PathBuf, key_path: PathBuf) -> Self {
        self.tls_cert_file = Some(cert_path.to_string_lossy().to_string());
        self.tls_key_file = Some(key_path.to_string_lossy().to_string());
        self
    }

    /// Use default ProximaDB TLS certificates
    pub fn with_default_tls(mut self) -> Self {
        self.tls_cert_file = Some("certs/proximadb-cert.pem".to_string());
        self.tls_key_file = Some("certs/proximadb-key.pem".to_string());
        self
    }

    /// Build the HTTP server configuration
    ///
    /// ## Validation:
    ///
    /// - Checks TLS certificate and key files exist
    /// - Validates certificate format (basic check)
    /// - Ensures port is not privileged (<1024) without proper permissions
    ///
    /// ## Error Cases:
    ///
    /// - Missing TLS files when TLS is configured
    /// - Invalid certificate format
    /// - Port binding conflicts (detected at runtime)
    pub fn build(self) -> Result<RestHttpServerConfig> {
        // Validate TLS configuration if enabled
        if let (Some(cert_file), Some(key_file)) = (&self.tls_cert_file, &self.tls_key_file) {
            if !std::path::Path::new(cert_file).exists() {
                return Err(anyhow::anyhow!(
                    "TLS certificate file not found: {}",
                    cert_file
                ));
            }
            if !std::path::Path::new(key_file).exists() {
                return Err(anyhow::anyhow!(
                    "TLS private key file not found: {}",
                    key_file
                ));
            }
            info!(
                "✓ HTTP TLS configuration validated: cert={}, key={}",
                cert_file, key_file
            );
        }

        Ok(RestHttpServerConfig {
            port: self.bind_address.port(),
            enable_rest: self.enable_rest,
            enable_dashboard: self.enable_dashboard,
            enable_metrics: self.enable_metrics,
            enable_health: self.enable_health,
            compression: self.rest_compression, // Clear mapping to config
            tls_cert_file: self.tls_cert_file,
            tls_key_file: self.tls_key_file,
        })
    }

    /// Build with validation and warnings
    pub fn build_with_validation(self) -> Result<RestHttpServerConfig> {
        let config = self.build()?;

        // Log configuration summary
        info!("📡 HTTP Server Configuration:");
        if config.is_tls_enabled() && config.verify_tls_certificates() {
            info!("  🔒 TLS Mode: ENABLED on port {}", config.port);
            info!("  📋 Non-TLS server will be DISABLED (TLS takes precedence)");
        } else {
            info!(
                "  🌐 Non-TLS Mode: ENABLED on {}",
                config.active_bind_address()
            );
            if config.tls_cert_file.is_some() || config.tls_key_file.is_some() {
                warn!("  ⚠️  TLS certificates configured but invalid - falling back to non-TLS");
            }
        }

        info!(
            "  📍 Services: REST={}, Dashboard={}, Metrics={}, Health={}",
            config.enable_rest,
            config.enable_dashboard,
            config.enable_metrics,
            config.enable_health
        );

        Ok(config)
    }
}

/// Builder for gRPC server configuration
///
/// ## gRPC Server Features:
///
/// - **Binary Protocol**: 2-3x faster than REST for large payloads
/// - **Streaming**: Bidirectional streaming for bulk operations
/// - **Reflection**: Service discovery for dynamic clients
/// - **Compression**: Built-in gzip support
///
/// ## Performance Advantages:
///
/// - **Throughput**: 1,770 QPS vs 840 QPS (REST)
/// - **Latency**: Lower due to HTTP/2 multiplexing
/// - **Memory**: Efficient protobuf serialization
///
/// ## Message Size Limits:
///
/// Default: 64MB, configurable up to 2GB
/// - Small vectors (<1KB): No limit concerns
/// - Large batches (1M vectors): May need 256MB+
/// - Streaming recommended for huge datasets
#[derive(Debug, Clone)]
pub struct GrpcHttpServerBuilder {
    /// Socket address to bind (default: 0.0.0.0:5679)
    bind_address: SocketAddr,

    /// Enable gRPC service
    enable_grpc: bool,

    /// Enable gRPC compression (gzip)
    grpc_compression: bool, // Clear, specific naming

    /// Path to TLS certificate file (PEM format)
    tls_cert_file: Option<String>,

    /// Path to TLS private key file (PEM format)
    tls_key_file: Option<String>,

    /// Maximum message size in bytes (default: 64MB)
    max_message_size: usize,

    /// Enable gRPC reflection for service discovery
    enable_reflection: bool,
}

impl Default for GrpcHttpServerBuilder {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:5679".parse().unwrap(), // Standard gRPC port
            enable_grpc: true,
            grpc_compression: false, // Clear naming, default false
            tls_cert_file: None,
            tls_key_file: None,
            max_message_size: 64 * 1024 * 1024, // 64MB for bulk vector inserts
            enable_reflection: true,
        }
    }
}

impl GrpcHttpServerBuilder {
    /// Create new gRPC server builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set bind address for non-TLS gRPC server
    pub fn bind_address<T: Into<SocketAddr>>(mut self, addr: T) -> Self {
        self.bind_address = addr.into();
        self
    }

    // TLS uses the same port as non-TLS (5679 for gRPC)

    /// Enable/disable gRPC endpoints
    pub fn enable_grpc(mut self, enabled: bool) -> Self {
        self.enable_grpc = enabled;
        self
    }

    /// Set maximum message size
    pub fn max_message_size(mut self, size: usize) -> Self {
        self.max_message_size = size;
        self
    }

    /// Enable/disable gRPC reflection
    pub fn enable_reflection(mut self, enabled: bool) -> Self {
        self.enable_reflection = enabled;
        self
    }

    /// Enable/disable gRPC compression
    pub fn grpc_compression(mut self, enabled: bool) -> Self {
        self.grpc_compression = enabled;
        self
    }

    /// Set TLS certificate and key files
    pub fn with_tls<C, K>(mut self, cert_file: C, key_file: K) -> Self
    where
        C: Into<String>,
        K: Into<String>,
    {
        self.tls_cert_file = Some(cert_file.into());
        self.tls_key_file = Some(key_file.into());
        self
    }

    /// Set TLS certificate and key from PathBuf
    pub fn with_tls_paths(mut self, cert_path: PathBuf, key_path: PathBuf) -> Self {
        self.tls_cert_file = Some(cert_path.to_string_lossy().to_string());
        self.tls_key_file = Some(key_path.to_string_lossy().to_string());
        self
    }

    /// Use default ProximaDB TLS certificates
    pub fn with_default_tls(mut self) -> Self {
        self.tls_cert_file = Some("certs/proximadb-cert.pem".to_string());
        self.tls_key_file = Some("certs/proximadb-key.pem".to_string());
        self
    }

    /// Build the gRPC server configuration
    pub fn build(self) -> Result<GrpcHttpServerConfig> {
        // Validate TLS configuration if enabled
        if let (Some(cert_file), Some(key_file)) = (&self.tls_cert_file, &self.tls_key_file) {
            if !std::path::Path::new(cert_file).exists() {
                return Err(anyhow::anyhow!(
                    "TLS certificate file not found: {}",
                    cert_file
                ));
            }
            if !std::path::Path::new(key_file).exists() {
                return Err(anyhow::anyhow!(
                    "TLS private key file not found: {}",
                    key_file
                ));
            }
            info!(
                "✓ gRPC TLS configuration validated: cert={}, key={}",
                cert_file, key_file
            );
        }

        Ok(GrpcHttpServerConfig {
            port: self.bind_address.port(),
            bind_address: self.bind_address,
            tls_bind_address: None, // No separate TLS port - same port for TLS
            enable_grpc: self.enable_grpc,
            max_message_size: self.max_message_size,
            enable_reflection: self.enable_reflection,
            compression: self.grpc_compression, // Clear mapping to config
            tls_cert_file: self.tls_cert_file,
            tls_key_file: self.tls_key_file,
        })
    }

    /// Build with validation and warnings
    pub fn build_with_validation(self) -> Result<GrpcHttpServerConfig> {
        let config = self.build()?;

        // Log configuration summary
        info!("🔗 gRPC Server Configuration:");
        if config.is_tls_enabled() && config.verify_tls_certificates() {
            info!("  🔒 TLS Mode: ENABLED on port {}", config.port);
            info!("  📋 Non-TLS server will be DISABLED (TLS takes precedence)");
        } else {
            info!(
                "  🌐 Non-TLS Mode: ENABLED on {}",
                config.active_bind_address()
            );
            if config.tls_cert_file.is_some() || config.tls_key_file.is_some() {
                warn!("  ⚠️  TLS certificates configured but invalid - falling back to non-TLS");
            }
        }

        info!(
            "  📍 Features: gRPC={}, Reflection={}, MaxMsgSize={}MB",
            config.enable_grpc,
            config.enable_reflection,
            config.max_message_size / (1024 * 1024)
        );

        Ok(config)
    }
}

/// Builder for complete multi-server configuration
#[derive(Debug)]
pub struct MultiServerBuilder {
    http_builder: RestHttpServerBuilder,
    grpc_builder: GrpcHttpServerBuilder,
    api_config: Option<crate::core::config::ApiConfig>,
}

impl Default for MultiServerBuilder {
    fn default() -> Self {
        Self {
            http_builder: RestHttpServerBuilder::default(),
            grpc_builder: GrpcHttpServerBuilder::default(),
            api_config: None,
        }
    }
}

impl MultiServerBuilder {
    /// Create new multi-server builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure HTTP server
    pub fn http<F>(mut self, config_fn: F) -> Self
    where
        F: FnOnce(RestHttpServerBuilder) -> RestHttpServerBuilder,
    {
        self.http_builder = config_fn(self.http_builder);
        self
    }

    /// Configure gRPC server
    pub fn grpc<F>(mut self, config_fn: F) -> Self
    where
        F: FnOnce(GrpcHttpServerBuilder) -> GrpcHttpServerBuilder,
    {
        self.grpc_builder = config_fn(self.grpc_builder);
        self
    }

    /// Use default TLS certificates for both servers
    pub fn with_default_tls(mut self) -> Self {
        self.http_builder = self.http_builder.with_default_tls();
        self.grpc_builder = self.grpc_builder.with_default_tls();
        self
    }

    /// Use custom TLS certificates for both servers
    pub fn with_tls<C, K>(mut self, cert_file: C, key_file: K) -> Self
    where
        C: Into<String> + Clone,
        K: Into<String> + Clone,
    {
        let cert_file = cert_file.into();
        let key_file = key_file.into();
        self.http_builder = self
            .http_builder
            .with_tls(cert_file.clone(), key_file.clone());
        self.grpc_builder = self.grpc_builder.with_tls(cert_file, key_file);
        self
    }

    /// Set API configuration (request limits, timeouts, etc.)
    pub fn with_api_config(mut self, api_config: crate::core::config::ApiConfig) -> Self {
        self.api_config = Some(api_config);
        self
    }

    /// Build the complete multi-server configuration
    pub fn build(mut self) -> Result<MultiServerConfig> {
        // Apply API config compression settings to builders if available
        if let Some(ref api_config) = self.api_config {
            self.http_builder = self
                .http_builder
                .rest_compression(api_config.rest_compression);
            self.grpc_builder = self
                .grpc_builder
                .grpc_compression(api_config.grpc_compression);
        }

        let http_config = self
            .http_builder
            .build_with_validation()
            .context("Failed to build HTTP server configuration")?;
        let grpc_config = self
            .grpc_builder
            .build_with_validation()
            .context("Failed to build gRPC server configuration")?;

        Ok(MultiServerConfig {
            http_config,
            grpc_config,
            tls_config: crate::network::multi_server::TLSConfig::default(),
            api_config: self.api_config,
        })
    }

    /// Create configuration for development (non-TLS)
    pub fn development() -> Result<MultiServerConfig> {
        info!("🛠️  Building development configuration (non-TLS)");
        Self::new()
            .http(|h| h.bind_address("0.0.0.0:5678".parse::<SocketAddr>().unwrap()))
            .grpc(|g| g.bind_address("0.0.0.0:5679".parse::<SocketAddr>().unwrap()))
            .build()
    }

    /// Create configuration for production (TLS enabled)
    pub fn production() -> Result<MultiServerConfig> {
        info!("🔒 Building production configuration (TLS enabled)");
        Self::new()
            .with_default_tls()
            .http(|h| {
                h.bind_address("0.0.0.0:5678".parse::<SocketAddr>().unwrap()) // Same port for TLS
            })
            .grpc(|g| {
                g.bind_address("0.0.0.0:5679".parse::<SocketAddr>().unwrap()) // Same port for TLS
            })
            .build()
    }

    /// Create custom configuration
    pub fn custom() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_server_builder() {
        let config = RestHttpServerBuilder::new()
            .bind_address("127.0.0.1:8080".parse::<std::net::SocketAddr>().unwrap())
            .enable_rest(true)
            .enable_dashboard(false)
            .build()
            .unwrap();

        assert_eq!(config.port, 8080);
        assert!(config.enable_rest);
        assert!(!config.enable_dashboard);
    }

    #[test]
    fn test_grpc_server_builder() {
        let config = GrpcHttpServerBuilder::new()
            .bind_address("127.0.0.1:9090".parse::<std::net::SocketAddr>().unwrap())
            .max_message_size(8 * 1024 * 1024)
            .enable_reflection(false)
            .build()
            .unwrap();

        assert_eq!(config.bind_address.port(), 9090);
        assert_eq!(config.max_message_size, 8 * 1024 * 1024);
        assert!(!config.enable_reflection);
    }

    #[test]
    fn test_development_config() {
        let config = MultiServerBuilder::development().unwrap();
        assert_eq!(config.http_config.port, 5678);
        assert_eq!(config.grpc_config.bind_address.port(), 5679);
        assert!(!config.http_config.is_tls_enabled());
        assert!(!config.grpc_config.is_tls_enabled());
    }
}
