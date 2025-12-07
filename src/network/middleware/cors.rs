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

//! CORS (Cross-Origin Resource Sharing) configuration middleware for ProximaDB.
//!
//! This module provides secure CORS configuration with whitelist-based origin control.
//! By default, CORS is configured to only allow same-origin requests in production,
//! with an explicit development mode that can be enabled for local testing.
//!
//! # Security Design
//!
//! The CORS implementation follows defense-in-depth principles:
//! - **Default Deny**: No cross-origin requests allowed unless explicitly configured
//! - **Whitelist Pattern**: Origins must be explicitly added to the allowed list
//! - **Development Mode**: Explicit flag required to enable permissive CORS
//! - **Method Restriction**: Only safe methods allowed by default
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::network::middleware::cors::{CorsConfig, create_cors_layer};
//!
//! // Production: Whitelist specific origins
//! let config = CorsConfig::production()
//!     .allow_origin("https://app.example.com")
//!     .allow_origin("https://admin.example.com");
//!
//! // Development: Allow all origins (for local testing only)
//! let dev_config = CorsConfig::development();
//!
//! let layer = create_cors_layer(&config);
//! ```

use axum::http::{header, HeaderValue, Method};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// CORS configuration for ProximaDB REST API.
///
/// This configuration determines which cross-origin requests are allowed.
/// By default, CORS is restrictive to prevent CSRF and data theft attacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsConfig {
    /// Enable CORS handling. When false, no CORS headers are sent.
    pub enabled: bool,

    /// Development mode - allows any origin. MUST be false in production.
    /// This field exists for explicit opt-in to insecure behavior during development.
    #[serde(default)]
    pub development_mode: bool,

    /// Whitelist of allowed origins.
    /// Only requests from these origins will include CORS headers.
    /// Origins should include protocol (e.g., "https://example.com")
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Allowed HTTP methods for cross-origin requests.
    /// Defaults to GET, POST, PUT, DELETE for API operations.
    #[serde(default = "default_allowed_methods")]
    pub allowed_methods: Vec<String>,

    /// Allowed request headers for cross-origin requests.
    /// Defaults to common API headers (Content-Type, Authorization, etc.)
    #[serde(default = "default_allowed_headers")]
    pub allowed_headers: Vec<String>,

    /// Headers to expose to the client in responses.
    #[serde(default)]
    pub expose_headers: Vec<String>,

    /// Maximum age (in seconds) for preflight request caching.
    /// Browser will cache preflight results for this duration.
    #[serde(default = "default_max_age")]
    pub max_age_secs: u64,

    /// Allow credentials (cookies, authorization headers) in cross-origin requests.
    /// When true, allowed_origins cannot be wildcard.
    #[serde(default)]
    pub allow_credentials: bool,
}

fn default_allowed_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "POST".to_string(),
        "PUT".to_string(),
        "DELETE".to_string(),
        "OPTIONS".to_string(),
    ]
}

fn default_allowed_headers() -> Vec<String> {
    vec![
        "Content-Type".to_string(),
        "Authorization".to_string(),
        "Accept".to_string(),
        "X-Request-ID".to_string(),
        "X-Correlation-ID".to_string(),
    ]
}

fn default_max_age() -> u64 {
    3600 // 1 hour
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self::production()
    }
}

impl CorsConfig {
    /// Create a production-safe CORS configuration.
    ///
    /// This configuration:
    /// - Denies all cross-origin requests by default
    /// - Requires explicit origin whitelisting
    /// - Uses restrictive method and header lists
    pub fn production() -> Self {
        Self {
            enabled: true,
            development_mode: false,
            allowed_origins: Vec::new(), // No origins allowed by default
            allowed_methods: default_allowed_methods(),
            allowed_headers: default_allowed_headers(),
            expose_headers: Vec::new(),
            max_age_secs: default_max_age(),
            allow_credentials: false,
        }
    }

    /// Create a development-only CORS configuration.
    ///
    /// **WARNING**: This configuration allows ANY origin and should NEVER
    /// be used in production. It exists solely for local development and testing.
    ///
    /// This configuration:
    /// - Allows requests from any origin
    /// - Allows all common methods and headers
    /// - Logs a warning when initialized
    pub fn development() -> Self {
        tracing::warn!(
            "🚨 CORS development mode enabled - allowing all origins. \
             Do NOT use in production!"
        );
        Self {
            enabled: true,
            development_mode: true,
            allowed_origins: Vec::new(), // Ignored when development_mode is true
            allowed_methods: default_allowed_methods(),
            allowed_headers: default_allowed_headers(),
            expose_headers: Vec::new(),
            max_age_secs: default_max_age(),
            allow_credentials: false,
        }
    }

    /// Add an allowed origin to the whitelist.
    ///
    /// Origins must include protocol (http:// or https://).
    /// Example: `config.allow_origin("https://app.example.com")`
    pub fn allow_origin(mut self, origin: &str) -> Self {
        self.allowed_origins.push(origin.to_string());
        self
    }

    /// Add multiple allowed origins to the whitelist.
    pub fn allow_origins(mut self, origins: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_origins
            .extend(origins.into_iter().map(Into::into));
        self
    }

    /// Enable credentials support.
    ///
    /// Note: When credentials are enabled, wildcard origins are not allowed.
    pub fn with_credentials(mut self) -> Self {
        self.allow_credentials = true;
        self
    }

    /// Validate the configuration.
    ///
    /// Returns an error if the configuration is invalid or insecure.
    pub fn validate(&self) -> Result<(), CorsConfigError> {
        // Warn if development mode is enabled
        if self.development_mode {
            tracing::warn!("CORS development mode is enabled - this is insecure for production");
        }

        // Check for credential + wildcard conflict
        if self.allow_credentials && self.development_mode {
            return Err(CorsConfigError::CredentialsWithWildcard);
        }

        // Validate origin URLs
        for origin in &self.allowed_origins {
            if !origin.starts_with("http://") && !origin.starts_with("https://") {
                return Err(CorsConfigError::InvalidOrigin(origin.clone()));
            }
        }

        Ok(())
    }

    /// Check if a given origin is allowed.
    pub fn is_origin_allowed(&self, origin: &str) -> bool {
        if self.development_mode {
            return true;
        }
        self.allowed_origins.iter().any(|o| o == origin)
    }
}

/// Errors that can occur during CORS configuration.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CorsConfigError {
    #[error("Cannot use credentials with wildcard origins")]
    CredentialsWithWildcard,

    #[error("Invalid origin URL (must start with http:// or https://): {0}")]
    InvalidOrigin(String),

    #[error("CORS configuration validation failed: {0}")]
    ValidationFailed(String),
}

/// Create a tower-http CorsLayer from the configuration.
///
/// This function builds the appropriate CORS layer based on the configuration:
/// - Development mode: Permissive CORS (any origin)
/// - Production mode: Whitelist-based origin checking
pub fn create_cors_layer(config: &CorsConfig) -> CorsLayer {
    if !config.enabled {
        // Return a minimal CORS layer that doesn't add headers
        return CorsLayer::new();
    }

    // Start building the CORS layer
    let mut layer = CorsLayer::new();

    // Configure origin policy
    if config.development_mode {
        // Development: Allow any origin (INSECURE)
        layer = layer.allow_origin(Any);
        tracing::debug!("CORS: Allowing any origin (development mode)");
    } else if config.allowed_origins.is_empty() {
        // No origins configured: Same-origin only (most restrictive)
        // Don't set allow_origin - this effectively blocks cross-origin requests
        tracing::debug!("CORS: No cross-origin requests allowed (no origins configured)");
    } else {
        // Production: Whitelist specific origins
        let origins: HashSet<String> = config.allowed_origins.iter().cloned().collect();
        let origins_clone = origins.clone();

        layer = layer.allow_origin(AllowOrigin::predicate(move |origin, _| {
            if let Ok(origin_str) = origin.to_str() {
                origins_clone.contains(origin_str)
            } else {
                false
            }
        }));

        tracing::info!(
            "CORS: Allowing origins: {:?}",
            config.allowed_origins
        );
    }

    // Configure allowed methods
    let methods: Vec<Method> = config
        .allowed_methods
        .iter()
        .filter_map(|m| m.parse().ok())
        .collect();
    layer = layer.allow_methods(methods);

    // Configure allowed headers
    let headers: Vec<header::HeaderName> = config
        .allowed_headers
        .iter()
        .filter_map(|h| h.parse().ok())
        .collect();
    layer = layer.allow_headers(headers);

    // Configure exposed headers
    if !config.expose_headers.is_empty() {
        let expose: Vec<header::HeaderName> = config
            .expose_headers
            .iter()
            .filter_map(|h| h.parse().ok())
            .collect();
        layer = layer.expose_headers(expose);
    }

    // Configure max age
    layer = layer.max_age(std::time::Duration::from_secs(config.max_age_secs));

    // Configure credentials
    if config.allow_credentials {
        layer = layer.allow_credentials(true);
    }

    layer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_production_config_is_restrictive() {
        let config = CorsConfig::production();
        assert!(!config.development_mode);
        assert!(config.allowed_origins.is_empty());
        assert!(!config.allow_credentials);
    }

    #[test]
    fn test_development_config_is_permissive() {
        let config = CorsConfig::development();
        assert!(config.development_mode);
    }

    #[test]
    fn test_allow_origin_builder() {
        let config = CorsConfig::production()
            .allow_origin("https://example.com")
            .allow_origin("https://app.example.com");

        assert_eq!(config.allowed_origins.len(), 2);
        assert!(config.is_origin_allowed("https://example.com"));
        assert!(config.is_origin_allowed("https://app.example.com"));
        assert!(!config.is_origin_allowed("https://evil.com"));
    }

    #[test]
    fn test_validation_rejects_invalid_origin() {
        let config = CorsConfig::production().allow_origin("invalid-origin");
        let result = config.validate();
        assert!(matches!(result, Err(CorsConfigError::InvalidOrigin(_))));
    }

    #[test]
    fn test_validation_rejects_credentials_with_development_mode() {
        let mut config = CorsConfig::development();
        config.allow_credentials = true;
        let result = config.validate();
        assert!(matches!(
            result,
            Err(CorsConfigError::CredentialsWithWildcard)
        ));
    }

    #[test]
    fn test_origin_checking() {
        let config = CorsConfig::production()
            .allow_origin("https://example.com");

        assert!(config.is_origin_allowed("https://example.com"));
        assert!(!config.is_origin_allowed("https://other.com"));

        // Development mode allows all
        let dev_config = CorsConfig::development();
        assert!(dev_config.is_origin_allowed("https://any-origin.com"));
    }

    #[test]
    fn test_default_methods() {
        let methods = default_allowed_methods();
        assert!(methods.contains(&"GET".to_string()));
        assert!(methods.contains(&"POST".to_string()));
        assert!(methods.contains(&"PUT".to_string()));
        assert!(methods.contains(&"DELETE".to_string()));
    }

    #[test]
    fn test_default_headers() {
        let headers = default_allowed_headers();
        assert!(headers.contains(&"Content-Type".to_string()));
        assert!(headers.contains(&"Authorization".to_string()));
    }
}
