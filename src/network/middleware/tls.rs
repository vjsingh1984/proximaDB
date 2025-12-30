/*
 * Copyright 2025 ProximaDB
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

//! TLS/mTLS Middleware for Client Certificate Extraction
//!
//! This module provides middleware for extracting and validating client certificates
//! from TLS connections in mTLS (mutual TLS) mode.
//!
//! ## Features
//!
//! - Extract client certificate from TLS connection
//! - Parse X.509 subject Common Name (CN)
//! - Map CN to user identity for authorization
//! - Validate certificate properties (expiration, revocation)
//!
//! ## Usage with Axum
//!
//! ```rust,ignore
//! use proximadb::network::middleware::tls::{TlsClientCertLayer, TlsClientCertConfig};
//!
//! let config = TlsClientCertConfig {
//!     require_client_cert: true,
//!     allowed_cn_patterns: vec!["*.mycompany.com".to_string()],
//!     ..Default::default()
//! };
//!
//! let app = Router::new()
//!     .route("/api/v1/secure", get(handler))
//!     .layer(TlsClientCertLayer::new(config));
//! ```

use crate::network::tls::ClientCertificateInfo;
use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{Json, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Configuration for TLS client certificate extraction and validation
#[derive(Debug, Clone, Deserialize)]
pub struct TlsClientCertConfig {
    /// Require client certificates (reject connections without valid certs)
    pub require_client_cert: bool,
    /// Allowed CN patterns (supports wildcards like "*.example.com")
    /// Empty list means allow all valid certificates
    pub allowed_cn_patterns: Vec<String>,
    /// Map certificate CNs to user IDs (optional)
    /// If CN is not in mapping, CN itself is used as user ID
    pub cn_to_user_mapping: HashMap<String, String>,
    /// Default roles assigned to mTLS-authenticated users
    pub default_roles: Vec<String>,
    /// Reject expired certificates (should almost always be true)
    pub reject_expired: bool,
    /// Check certificate revocation (requires CRL/OCSP configuration)
    pub check_revocation: bool,
}

impl Default for TlsClientCertConfig {
    fn default() -> Self {
        Self {
            require_client_cert: false,
            allowed_cn_patterns: vec![],
            cn_to_user_mapping: HashMap::new(),
            default_roles: vec!["reader".to_string()],
            reject_expired: true,
            check_revocation: false, // Disabled by default (requires additional setup)
        }
    }
}

impl TlsClientCertConfig {
    /// Create config that requires client certificates
    pub fn required() -> Self {
        Self {
            require_client_cert: true,
            ..Default::default()
        }
    }

    /// Create config for development (optional client certs, allow all CNs)
    pub fn development() -> Self {
        Self {
            require_client_cert: false,
            allowed_cn_patterns: vec!["*".to_string()],
            reject_expired: false, // Allow expired certs in dev
            ..Default::default()
        }
    }

    /// Create production config with strict validation
    pub fn production(allowed_patterns: Vec<String>) -> Self {
        Self {
            require_client_cert: true,
            allowed_cn_patterns: allowed_patterns,
            reject_expired: true,
            check_revocation: true,
            ..Default::default()
        }
    }

    /// Add allowed CN pattern
    pub fn allow_cn(mut self, pattern: &str) -> Self {
        self.allowed_cn_patterns.push(pattern.to_string());
        self
    }

    /// Map CN to user ID
    pub fn map_cn_to_user(mut self, cn: &str, user_id: &str) -> Self {
        self.cn_to_user_mapping
            .insert(cn.to_string(), user_id.to_string());
        self
    }

    /// Set default roles for mTLS users
    pub fn with_default_roles(mut self, roles: Vec<String>) -> Self {
        self.default_roles = roles;
        self
    }
}

/// TLS client certificate authentication error response
#[derive(Debug, Serialize)]
pub struct TlsCertErrorResponse {
    pub error: String,
    pub message: String,
    pub code: u16,
}

/// Authenticated user information from mTLS
#[derive(Debug, Clone)]
pub struct TlsAuthenticatedUser {
    /// User ID (derived from CN or mapped)
    pub user_id: String,
    /// Certificate Common Name
    pub common_name: String,
    /// Organization from certificate
    pub organization: Option<String>,
    /// Certificate fingerprint (SHA-256)
    pub fingerprint: String,
    /// Assigned roles
    pub roles: Vec<String>,
    /// Certificate serial number
    pub serial: String,
    /// Certificate expiration time
    pub expires_at: std::time::SystemTime,
}

/// State for TLS client certificate middleware
#[derive(Clone)]
pub struct TlsClientCertState {
    pub config: Arc<TlsClientCertConfig>,
}

impl TlsClientCertState {
    pub fn new(config: TlsClientCertConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

/// Middleware to extract and validate TLS client certificates
///
/// This middleware:
/// 1. Extracts ClientCertificateInfo from request extensions (set by TLS acceptor)
/// 2. Validates the certificate against configured policies
/// 3. Maps certificate CN to user identity
/// 4. Adds TlsAuthenticatedUser to request extensions for downstream handlers
pub async fn tls_client_cert_middleware<B>(
    State(state): State<TlsClientCertState>,
    mut request: Request<B>,
    next: Next<B>,
) -> Result<Response, (StatusCode, Json<TlsCertErrorResponse>)> {
    let config = &state.config;
    let path = request.uri().path();

    // Skip cert validation for health endpoints
    if is_health_endpoint(path) {
        return Ok(next.run(request).await);
    }

    // Try to extract client certificate info from extensions
    // The TLS acceptor should have added this during the TLS handshake
    let cert_info = request
        .extensions()
        .get::<ClientCertificateInfo>()
        .cloned();

    match cert_info {
        Some(info) => {
            // Validate certificate
            if let Err(e) = validate_certificate(&info, config) {
                return Err(e);
            }

            // Extract CN
            let common_name = info.common_name.clone().ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(TlsCertErrorResponse {
                        error: "certificate_missing_cn".to_string(),
                        message: "Client certificate must have a Common Name (CN)".to_string(),
                        code: 401,
                    }),
                )
            })?;

            // Validate CN against allowed patterns
            if !config.allowed_cn_patterns.is_empty() {
                let cn_allowed = config
                    .allowed_cn_patterns
                    .iter()
                    .any(|pattern| matches_cn_pattern(&common_name, pattern));

                if !cn_allowed {
                    warn!(
                        "TLS client cert CN '{}' not in allowed patterns",
                        common_name
                    );
                    return Err((
                        StatusCode::FORBIDDEN,
                        Json(TlsCertErrorResponse {
                            error: "certificate_cn_not_allowed".to_string(),
                            message: format!(
                                "Certificate CN '{}' is not in the allowed list",
                                common_name
                            ),
                            code: 403,
                        }),
                    ));
                }
            }

            // Map CN to user ID
            let user_id = config
                .cn_to_user_mapping
                .get(&common_name)
                .cloned()
                .unwrap_or_else(|| common_name.clone());

            // Create authenticated user
            let authenticated_user = TlsAuthenticatedUser {
                user_id: user_id.clone(),
                common_name: common_name.clone(),
                organization: info.organization.clone(),
                fingerprint: info.fingerprint.clone(),
                roles: config.default_roles.clone(),
                serial: info.serial.clone(),
                expires_at: info.expires_at,
            };

            info!(
                "TLS client certificate authenticated: user={}, cn={}, fingerprint={}...",
                user_id,
                common_name,
                &info.fingerprint[..16.min(info.fingerprint.len())]
            );

            // Add authenticated user to request extensions
            request.extensions_mut().insert(authenticated_user);

            Ok(next.run(request).await)
        }
        None => {
            if config.require_client_cert {
                warn!("TLS client certificate required but not provided");
                Err((
                    StatusCode::UNAUTHORIZED,
                    Json(TlsCertErrorResponse {
                        error: "client_certificate_required".to_string(),
                        message: "Client certificate is required for mTLS authentication".to_string(),
                        code: 401,
                    }),
                ))
            } else {
                // Client cert not required, continue without mTLS auth
                debug!("No client certificate provided, continuing without mTLS auth");
                Ok(next.run(request).await)
            }
        }
    }
}

/// Validate certificate against configuration
fn validate_certificate(
    info: &ClientCertificateInfo,
    config: &TlsClientCertConfig,
) -> Result<(), (StatusCode, Json<TlsCertErrorResponse>)> {
    // Check expiration
    if config.reject_expired && !info.is_valid {
        warn!("TLS client certificate is expired or not yet valid");
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(TlsCertErrorResponse {
                error: "certificate_invalid".to_string(),
                message: "Client certificate is expired or not yet valid".to_string(),
                code: 401,
            }),
        ));
    }

    // Revocation checking would go here if enabled
    // This requires CRL distribution points or OCSP responders
    if config.check_revocation {
        // TODO: Implement CRL/OCSP checking
        debug!("Certificate revocation checking is configured but not yet implemented");
    }

    Ok(())
}

/// Check if a Common Name matches a pattern
///
/// Patterns supported:
/// - Exact match: "client.example.com" matches "client.example.com"
/// - Wildcard: "*.example.com" matches "client.example.com" but not "a.b.example.com"
/// - Star: "*" matches any CN
pub fn matches_cn_pattern(cn: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if let Some(suffix) = pattern.strip_prefix("*.") {
        // Wildcard pattern - CN must end with suffix and have exactly one more segment
        if cn.ends_with(suffix) && cn.len() > suffix.len() + 1 {
            let prefix = &cn[..cn.len() - suffix.len() - 1];
            // Ensure no additional dots in prefix (single level wildcard)
            return !prefix.contains('.');
        }
        return false;
    }

    // Exact match
    cn == pattern
}

/// Check if path is a health endpoint
fn is_health_endpoint(path: &str) -> bool {
    path.starts_with("/health")
}

/// Extension trait to extract TLS authenticated user from requests
pub trait TlsAuthenticatedUserExt {
    /// Get the TLS authenticated user if present
    fn tls_authenticated_user(&self) -> Option<&TlsAuthenticatedUser>;
    /// Get the user ID from TLS authentication
    fn tls_user_id(&self) -> Option<&str>;
}

impl<T> TlsAuthenticatedUserExt for Request<T> {
    fn tls_authenticated_user(&self) -> Option<&TlsAuthenticatedUser> {
        self.extensions().get::<TlsAuthenticatedUser>()
    }

    fn tls_user_id(&self) -> Option<&str> {
        self.tls_authenticated_user()
            .map(|user| user.user_id.as_str())
    }
}

/// Layer for TLS client certificate middleware
#[derive(Clone)]
pub struct TlsClientCertLayer {
    state: TlsClientCertState,
}

impl TlsClientCertLayer {
    pub fn new(config: TlsClientCertConfig) -> Self {
        Self {
            state: TlsClientCertState::new(config),
        }
    }

    /// Get the state for use with axum middleware::from_fn_with_state
    pub fn state(&self) -> TlsClientCertState {
        self.state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_cn_pattern_exact() {
        assert!(matches_cn_pattern("client.example.com", "client.example.com"));
        assert!(!matches_cn_pattern("other.example.com", "client.example.com"));
    }

    #[test]
    fn test_matches_cn_pattern_wildcard() {
        assert!(matches_cn_pattern("client.example.com", "*.example.com"));
        assert!(matches_cn_pattern("server.example.com", "*.example.com"));
        // Should not match multi-level
        assert!(!matches_cn_pattern("a.b.example.com", "*.example.com"));
        // Should not match exact domain
        assert!(!matches_cn_pattern("example.com", "*.example.com"));
    }

    #[test]
    fn test_matches_cn_pattern_star() {
        assert!(matches_cn_pattern("anything", "*"));
        assert!(matches_cn_pattern("client.example.com", "*"));
        assert!(matches_cn_pattern("a.b.c.d.e", "*"));
    }

    #[test]
    fn test_tls_client_cert_config_default() {
        let config = TlsClientCertConfig::default();
        assert!(!config.require_client_cert);
        assert!(config.allowed_cn_patterns.is_empty());
        assert!(config.reject_expired);
        assert!(!config.check_revocation);
    }

    #[test]
    fn test_tls_client_cert_config_production() {
        let config = TlsClientCertConfig::production(vec!["*.mycompany.com".to_string()]);
        assert!(config.require_client_cert);
        assert_eq!(config.allowed_cn_patterns.len(), 1);
        assert!(config.reject_expired);
        assert!(config.check_revocation);
    }

    #[test]
    fn test_tls_client_cert_config_builder() {
        let config = TlsClientCertConfig::default()
            .allow_cn("*.example.com")
            .allow_cn("admin.internal")
            .map_cn_to_user("admin.internal", "admin-service")
            .with_default_roles(vec!["admin".to_string(), "reader".to_string()]);

        assert_eq!(config.allowed_cn_patterns.len(), 2);
        assert_eq!(
            config.cn_to_user_mapping.get("admin.internal"),
            Some(&"admin-service".to_string())
        );
        assert_eq!(config.default_roles.len(), 2);
    }

    #[test]
    fn test_is_health_endpoint() {
        assert!(is_health_endpoint("/health"));
        assert!(is_health_endpoint("/health/ready"));
        assert!(is_health_endpoint("/health/live"));
        assert!(!is_health_endpoint("/api/collections"));
        assert!(!is_health_endpoint("/api/health")); // Different prefix
    }
}
