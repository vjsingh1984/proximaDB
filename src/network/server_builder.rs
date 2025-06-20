// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Server builder pattern for flexible server configuration

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::{info, warn};

use crate::network::multi_server::{GrpcHttpServerConfig, MultiServerConfig, RestHttpServerConfig};

/// Builder for HTTP server configuration
#[derive(Debug, Clone)]
pub struct RestHttpServerBuilder {
    bind_address: SocketAddr,
    tls_bind_address: Option<SocketAddr>,
    enable_rest: bool,
    enable_dashboard: bool,
    enable_metrics: bool,
    enable_health: bool,
    tls_cert_file: Option<String>,
    tls_key_file: Option<String>,
}

impl Default for RestHttpServerBuilder {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:5678".parse().unwrap(),
            tls_bind_address: None,
            enable_rest: true,
            enable_dashboard: true,
            enable_metrics: true,
            enable_health: true,
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

    /// Set bind address for TLS HTTPS server
    pub fn tls_bind_address<T: Into<SocketAddr>>(mut self, addr: T) -> Self {
        self.tls_bind_address = Some(addr.into());
        self
    }

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
                config.get_active_bind_address()
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
#[derive(Debug, Clone)]
pub struct GrpcHttpServerBuilder {
    bind_address: SocketAddr,
    tls_bind_address: Option<SocketAddr>,
    enable_grpc: bool,
    tls_cert_file: Option<String>,
    tls_key_file: Option<String>,
    max_message_size: usize,
    enable_reflection: bool,
}

impl Default for GrpcHttpServerBuilder {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:5680".parse().unwrap(),
            tls_bind_address: None,
            enable_grpc: true,
            tls_cert_file: None,
            tls_key_file: None,
            max_message_size: 4 * 1024 * 1024, // 4MB
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

    /// Set bind address for TLS gRPC server
    pub fn tls_bind_address<T: Into<SocketAddr>>(mut self, addr: T) -> Self {
        self.tls_bind_address = Some(addr.into());
        self
    }

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
            tls_bind_address: self.tls_bind_address,
            enable_grpc: self.enable_grpc,
            max_message_size: self.max_message_size,
            enable_reflection: self.enable_reflection,
            enable_compression: true, // Default to true
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
                config.get_active_bind_address()
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
}

impl Default for MultiServerBuilder {
    fn default() -> Self {
        Self {
            http_builder: RestHttpServerBuilder::default(),
            grpc_builder: GrpcHttpServerBuilder::default(),
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

    /// Build the complete multi-server configuration
    pub fn build(self) -> Result<MultiServerConfig> {
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
        })
    }

    /// Create configuration for development (non-TLS)
    pub fn development() -> Result<MultiServerConfig> {
        info!("🛠️  Building development configuration (non-TLS)");
        Self::new()
            .http(|h| h.bind_address("0.0.0.0:5678".parse::<SocketAddr>().unwrap()))
            .grpc(|g| g.bind_address("0.0.0.0:5680".parse::<SocketAddr>().unwrap()))
            .build()
    }

    /// Create configuration for production (TLS enabled)
    pub fn production() -> Result<MultiServerConfig> {
        info!("🔒 Building production configuration (TLS enabled)");
        Self::new()
            .with_default_tls()
            .http(|h| {
                h.bind_address("0.0.0.0:5678".parse::<SocketAddr>().unwrap())
                    .tls_bind_address("0.0.0.0:5679".parse::<SocketAddr>().unwrap())
            })
            .grpc(|g| {
                g.bind_address("0.0.0.0:5680".parse::<SocketAddr>().unwrap())
                    .tls_bind_address("0.0.0.0:5681".parse::<SocketAddr>().unwrap())
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
            .bind_address("127.0.0.1:8080".parse().unwrap())
            .enable_rest(true)
            .enable_dashboard(false)
            .build()
            .unwrap();

        assert_eq!(config.bind_address.port(), 8080);
        assert!(config.enable_rest);
        assert!(!config.enable_dashboard);
    }

    #[test]
    fn test_grpc_server_builder() {
        let config = GrpcHttpServerBuilder::new()
            .bind_address("127.0.0.1:9090".parse().unwrap())
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
        assert_eq!(config.http_config.bind_address.port(), 5678);
        assert_eq!(config.grpc_config.bind_address.port(), 5680);
        assert!(!config.http_config.is_tls_enabled());
        assert!(!config.grpc_config.is_tls_enabled());
    }
}
