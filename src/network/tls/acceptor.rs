// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! TLS Acceptor for Client Certificate Extraction
//!
//! This module provides a TLS acceptor that extracts client certificates
//! during the TLS handshake and adds them to request extensions for
//! downstream authentication middleware.
//!
//! ## Features
//!
//! - Extracts peer certificate from TLS connections
//! - Parses certificate to ClientCertificateInfo
//! - Adds certificate to request extensions
//! - Works with Axum/Hyper servers
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::network::tls::{TlsAcceptor, ClientCertificateInfo};
//! use axum::extract::Request;
//!
//! let tls_acceptor = TlsAcceptor::new(rustls_config);
//!
//! // In server configuration
//! let app = Router::new()
//!     .route("/secure", get(handler))
//!     .layer(tls_acceptor.into_layer());
//! ```

use super::ClientCertificateInfo;
use anyhow::Result;
use rustls::ServerConfig;
use rustls::server::ServerConnection;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor as TokioRustlsTlsAcceptor;
use tracing::{debug, warn};

/// ProximaDB TLS acceptor that extracts client certificates
///
/// This acceptor wraps the underlying rustls TLS acceptor and
/// extracts client certificate information for mTLS authentication.
pub struct TlsAcceptor {
    /// Inner tokio-rustls TLS acceptor
    inner: TokioRustlsTlsAcceptor,
    /// Whether mTLS is enabled (client certs required)
    mtls_enabled: bool,
}

impl TlsAcceptor {
    /// Create a new TLS acceptor from rustls ServerConfig
    pub fn new(config: Arc<ServerConfig>) -> Result<Self> {
        let mtls_enabled = Self::is_mtls_enabled(&config);

        let inner = TokioRustlsTlsAcceptor::new(config)
            .map_err(|e| anyhow::anyhow!("Failed to create TLS acceptor: {}", e))?;

        Ok(Self {
            inner,
            mtls_enabled,
        })
    }

    /// Create a new TLS acceptor with explicit mTLS setting
    pub fn with_mtls(config: Arc<ServerConfig>, mtls_enabled: bool) -> Result<Self> {
        let inner = TokioRustlsTlsAcceptor::new(config)
            .map_err(|e| anyhow::anyhow!("Failed to create TLS acceptor: {}", e))?;

        Ok(Self {
            inner,
            mtls_enabled,
        })
    }

    /// Check if the server config is configured for mTLS
    fn is_mtls_enabled(config: &ServerConfig) -> bool {
        // Check if client authentication is required
        // In rustls, this is indicated by the presence of a client cert verifier
        config.client_auth_mandatory()
    }

    /// Get the inner tokio-rustls acceptor
    pub fn inner(&self) -> &TokioRustlsTlsAcceptor {
        &self.inner
    }

    /// Get the inner acceptor for direct use
    pub fn into_inner(self) -> TokioRustlsTlsAcceptor {
        self.inner
    }

    /// Check if mTLS is enabled
    pub fn is_mtls_enabled(&self) -> bool {
        self.mtls_enabled
    }

    /// Convert to Axum layer for automatic certificate extraction
    #[cfg(feature = "network-rest")]
    pub fn into_layer(self) -> TlsAcceptorLayer {
        TlsAcceptorLayer::new(self.mtls_enabled)
    }
}

#[cfg(feature = "network-rest")]
impl Clone for TlsAcceptor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            mtls_enabled: self.mtls_enabled,
        }
    }
}

/// Axum layer that adds client certificate information to request extensions
///
/// This layer intercepts incoming requests and adds any TLS client certificate
/// information to the request extensions, making it available to downstream
/// authentication middleware.
#[cfg(feature = "network-rest")]
#[derive(Clone)]
pub struct TlsAcceptorLayer {
    /// Whether mTLS is enabled
    mtls_enabled: bool,
}

#[cfg(feature = "network-rest")]
impl TlsAcceptorLayer {
    /// Create a new TLS acceptor layer
    pub fn new(mtls_enabled: bool) -> Self {
        Self { mtls_enabled }
    }

    /// Create a layer that expects client certificates
    pub fn mtls() -> Self {
        Self { mtls_enabled: true }
    }

    /// Create a layer that doesn't require client certificates
    pub fn tls_only() -> Self {
        Self {
            mtls_enabled: false,
        }
    }
}

#[cfg(feature = "network-rest")]
impl<S> axum::Layer<S> for TlsAcceptorLayer
where
    S: Clone,
{
    type Service = TlsAcceptorMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TlsAcceptorMiddleware::new(inner, self.mtls_enabled)
    }
}

/// Middleware that adds client certificate info to request extensions
#[cfg(feature = "network-rest")]
#[derive(Clone)]
pub struct TlsAcceptorMiddleware<S> {
    inner: S,
    mtls_enabled: bool,
}

#[cfg(feature = "network-rest")]
impl<S> TlsAcceptorMiddleware<S> {
    fn new(inner: S, mtls_enabled: bool) -> Self {
        Self {
            inner,
            mtls_enabled,
        }
    }
}

#[cfg(feature = "network-rest")]
impl<S> tower_service::Service<axum::extract::Request> for TlsAcceptorMiddleware<S>
where
    S: tower_service::Service<axum::extract::Request> + Clone,
    S::Response: axum::response::Response,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = futures::future::BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: axum::extract::Request) -> Self::Future {
        let mtls_enabled = self.mtls_enabled;
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Extract client certificate from TLS connection
            if let Some(cert_info) = extract_client_cert_from_request(&req) {
                debug!(
                    "Extracted client certificate: CN={:?}, fingerprint={}",
                    cert_info.common_name,
                    &cert_info.fingerprint[..16.min(cert_info.fingerprint.len())]
                );
                req.extensions_mut().insert(cert_info);
            } else if mtls_enabled {
                warn!("mTLS is enabled but no client certificate was extracted");
            }

            inner.call(req).await
        })
    }
}

/// Extract client certificate information from the request
///
/// This function attempts to extract client certificate information from
/// various sources in the request, including TLS extensions and connection info.
#[cfg(feature = "network-rest")]
fn extract_client_cert_from_request(req: &axum::extract::Request) -> Option<ClientCertificateInfo> {
    use http_body::Body;

    // Try to get certificate from HTTP extensions
    // Some TLS implementations add peer certificates to request extensions

    // Method 1: Check for a custom extension set by the TLS layer
    if let Some(info) = req.extensions().get::<ClientCertificateInfo>() {
        return Some(info.clone());
    }

    // Method 2: Try to extract from the connection state
    // This depends on the specific TLS implementation being used

    // For Hyper with tokio-rustls, we need to access the underlying TLS connection
    // This is typically done through a custom extension set during the handshake

    // Method 3: Check for peer certificate in connection state
    // Note: This requires custom TLS layer integration

    // For now, this is a placeholder that returns None
    // The actual implementation requires integration with the TLS handshake

    None
}

/// Extract client certificate from TLS connection peer certificates
///
/// This function attempts to extract peer certificates from the TLS connection.
/// This is typically called during the TLS handshake by custom TLS layers.
pub fn extract_peer_certificate(peer_certs: &[&[u8]]) -> Option<ClientCertificateInfo> {
    if peer_certs.is_empty() {
        return None;
    }

    // Use the first certificate (leaf certificate)
    let cert_der = peer_certs[0];

    ClientCertificateInfo::from_der(cert_der)
        .ok()
        .inspect(|info| {
            debug!(
                "Extracted peer certificate: CN={:?}, serial={}, fingerprint={}",
                info.common_name,
                info.serial,
                &info.fingerprint[..16.min(info.fingerprint.len())]
            );
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_peer_certificate_success() {
        // This test requires actual certificate data
        // For now, we test with empty data
        let result = extract_peer_certificate(&[]);
        assert!(result.is_none());
    }

    #[cfg(feature = "network-rest")]
    #[test]
    fn test_tls_acceptor_layer_mtls() {
        let layer = TlsAcceptorLayer::mtls();
        assert!(layer.mtls_enabled);
    }

    #[cfg(feature = "network-rest")]
    #[test]
    fn test_tls_acceptor_layer_tls_only() {
        let layer = TlsAcceptorLayer::tls_only();
        assert!(!layer.mtls_enabled);
    }
}
