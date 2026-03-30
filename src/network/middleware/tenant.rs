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

//! Tenant extraction middleware for multi-tenant isolation
//!
//! This middleware extracts tenant_id from incoming requests and injects it
//! into the request extensions for downstream handlers to access.
//!
//! # Tenant ID Sources (Priority Order)
//!
//! 1. **HTTP Header**: `X-Tenant-ID` header (explicit tenant selection)
//! 2. **JWT Claims**: `tenant_id` claim in authenticated JWT token
//! 3. **API Key Mapping**: Tenant associated with the API key
//! 4. **Default Tenant**: Configurable fallback for single-tenant deployments
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::network::middleware::tenant::{TenantExtractor, TenantContext};
//!
//! // Create tenant extractor with optional default tenant
//! let extractor = TenantExtractor::new()
//!     .with_default_tenant("default");
//!
//! // In handlers, access tenant context:
//! async fn handler(Extension(tenant): Extension<TenantContext>) -> impl IntoResponse {
//!     println!("Request from tenant: {}", tenant.tenant_id);
//! }
//! ```

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use tracing::{debug, warn};

/// HTTP header name for explicit tenant ID
pub const X_TENANT_ID: &str = "X-Tenant-ID";

/// Tenant context extracted from request
///
/// This struct is injected into request extensions and can be accessed
/// by handlers using `Extension<TenantContext>`.
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// The tenant identifier
    pub tenant_id: String,
    /// Source of the tenant ID (for audit logging)
    pub source: TenantIdSource,
    /// Whether this is a system/admin tenant with elevated privileges
    pub is_system_tenant: bool,
}

impl TenantContext {
    /// Create a new tenant context
    pub fn new(tenant_id: impl Into<String>, source: TenantIdSource) -> Self {
        let tenant_id = tenant_id.into();
        let is_system_tenant = tenant_id == "system" || tenant_id == "admin";
        Self {
            tenant_id,
            source,
            is_system_tenant,
        }
    }

    /// Create a default/anonymous tenant context
    pub fn default_tenant() -> Self {
        Self::new("default", TenantIdSource::Default)
    }

    /// Check if this is a valid tenant (not empty)
    pub fn is_valid(&self) -> bool {
        !self.tenant_id.is_empty()
    }
}

/// Source of the tenant ID (for audit and debugging)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantIdSource {
    /// Extracted from X-Tenant-ID header
    Header,
    /// Extracted from JWT token claims
    JwtClaim,
    /// Derived from API key mapping
    ApiKey,
    /// Default tenant (single-tenant mode or fallback)
    Default,
    /// System/internal request
    System,
}

impl std::fmt::Display for TenantIdSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header => write!(f, "header"),
            Self::JwtClaim => write!(f, "jwt"),
            Self::ApiKey => write!(f, "api_key"),
            Self::Default => write!(f, "default"),
            Self::System => write!(f, "system"),
        }
    }
}

/// Configuration for tenant extraction
#[derive(Debug, Clone)]
pub struct TenantExtractorConfig {
    /// Default tenant ID for single-tenant deployments
    pub default_tenant: Option<String>,
    /// Whether to require tenant ID (reject requests without tenant)
    pub require_tenant: bool,
    /// Whether to validate tenant exists in TenantManager
    pub validate_tenant: bool,
    /// System tenant IDs that bypass normal validation
    pub system_tenants: Vec<String>,
}

impl Default for TenantExtractorConfig {
    fn default() -> Self {
        Self {
            default_tenant: Some("default".to_string()),
            require_tenant: false,  // Allow single-tenant mode by default
            validate_tenant: false, // Disable validation by default (enable in production)
            system_tenants: vec!["system".to_string(), "admin".to_string()],
        }
    }
}

impl TenantExtractorConfig {
    /// Create config for single-tenant deployment
    pub fn single_tenant(default_tenant: impl Into<String>) -> Self {
        Self {
            default_tenant: Some(default_tenant.into()),
            require_tenant: false,
            validate_tenant: false,
            system_tenants: vec!["system".to_string()],
        }
    }

    /// Create config for multi-tenant deployment (strict mode)
    pub fn multi_tenant() -> Self {
        Self {
            default_tenant: None,
            require_tenant: true,
            validate_tenant: true,
            system_tenants: vec!["system".to_string(), "admin".to_string()],
        }
    }

    /// Builder: Set default tenant
    pub fn with_default_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.default_tenant = Some(tenant.into());
        self
    }

    /// Builder: Require tenant ID on all requests
    pub fn require_tenant(mut self, require: bool) -> Self {
        self.require_tenant = require;
        self
    }

    /// Builder: Validate tenant exists
    pub fn validate_tenant(mut self, validate: bool) -> Self {
        self.validate_tenant = validate;
        self
    }
}

/// Tenant extractor state (shared across requests)
#[derive(Clone)]
pub struct TenantExtractor {
    config: TenantExtractorConfig,
    /// Optional TenantManager for validation
    tenant_manager: Option<Arc<crate::storage::tenant::TenantManager>>,
}

impl TenantExtractor {
    /// Create new tenant extractor with default config
    pub fn new() -> Self {
        Self {
            config: TenantExtractorConfig::default(),
            tenant_manager: None,
        }
    }

    /// Create tenant extractor with custom config
    pub fn with_config(config: TenantExtractorConfig) -> Self {
        Self {
            config,
            tenant_manager: None,
        }
    }

    /// Set TenantManager for validation
    pub fn with_tenant_manager(
        mut self,
        manager: Arc<crate::storage::tenant::TenantManager>,
    ) -> Self {
        self.tenant_manager = Some(manager);
        self
    }

    /// Extract tenant ID from request
    fn extract_tenant_id(&self, req: &Request<Body>) -> Option<(String, TenantIdSource)> {
        // Priority 1: Explicit X-Tenant-ID header
        if let Some(header_value) = req.headers().get(X_TENANT_ID)
            && let Ok(tenant_id) = header_value.to_str() {
                let tenant_id = tenant_id.trim();
                if !tenant_id.is_empty() {
                    debug!("Extracted tenant_id from header: {}", tenant_id);
                    return Some((tenant_id.to_string(), TenantIdSource::Header));
                }
            }

        // Priority 2: JWT claims (if auth middleware ran first)
        // Check for UserInfo in extensions (set by auth middleware)
        if let Some(user_info) = req.extensions().get::<super::auth::UserInfo>()
            && let Some(ref tenant_id) = user_info.tenant_id {
                debug!("Extracted tenant_id from JWT: {}", tenant_id);
                return Some((tenant_id.clone(), TenantIdSource::JwtClaim));
            }

        // Priority 3: Default tenant (if configured)
        if let Some(ref default_tenant) = self.config.default_tenant {
            debug!("Using default tenant: {}", default_tenant);
            return Some((default_tenant.clone(), TenantIdSource::Default));
        }

        None
    }

    /// Validate tenant exists and is active
    fn validate_tenant(&self, tenant_id: &str) -> bool {
        // System tenants bypass validation
        if self.config.system_tenants.contains(&tenant_id.to_string()) {
            return true;
        }

        // If validation is disabled, allow all
        if !self.config.validate_tenant {
            return true;
        }

        // Validate with TenantManager if available
        if let Some(ref manager) = self.tenant_manager {
            match manager.get_tenant(tenant_id) {
                Ok(tenant) => {
                    // Check tenant status
                    matches!(
                        tenant.status,
                        crate::storage::tenant::context::TenantStatus::Active
                    )
                }
                Err(e) => {
                    // Tenant not found or lookup failed
                    warn!("Failed to validate tenant {}: {}", tenant_id, e);
                    false
                }
            }
        } else {
            // No manager configured, allow all
            true
        }
    }
}

impl Default for TenantExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Tenant extraction middleware function
///
/// This middleware extracts tenant_id from the request and injects
/// TenantContext into request extensions.
pub async fn tenant_middleware(
    axum::extract::State(extractor): axum::extract::State<TenantExtractor>,
    mut req: Request<Body>,
    next: Next<Body>,
) -> Response {
    // Extract tenant ID
    match extractor.extract_tenant_id(&req) {
        Some((tenant_id, source)) => {
            // Validate tenant if configured
            if !extractor.validate_tenant(&tenant_id) {
                return (
                    StatusCode::FORBIDDEN,
                    format!("Tenant '{}' is not valid or not active", tenant_id),
                )
                    .into_response();
            }

            // Inject tenant context into request extensions
            let context = TenantContext::new(tenant_id, source);
            req.extensions_mut().insert(context);

            next.run(req).await
        }
        None => {
            if extractor.config.require_tenant {
                // Tenant required but not provided
                (
                    StatusCode::BAD_REQUEST,
                    "Tenant ID required. Provide X-Tenant-ID header or authenticate with tenant-bound credentials.",
                ).into_response()
            } else {
                // No tenant required, use anonymous context
                req.extensions_mut().insert(TenantContext::default_tenant());
                next.run(req).await
            }
        }
    }
}

/// Create tenant extractor with config for use with middleware
///
/// # Example
/// ```rust,ignore
/// use axum::middleware;
///
/// let extractor = create_tenant_extractor(TenantExtractorConfig::default());
/// let router = Router::new()
///     .route("/api/v1/collections", get(list_collections))
///     .layer(middleware::from_fn_with_state(extractor, tenant_middleware));
/// ```
pub fn create_tenant_extractor(config: TenantExtractorConfig) -> TenantExtractor {
    TenantExtractor::with_config(config)
}

/// Extension trait to easily get tenant context from request
pub trait TenantContextExt {
    /// Get tenant context from request extensions
    fn tenant_context(&self) -> Option<&TenantContext>;

    /// Get tenant ID or default
    fn tenant_id_or_default(&self) -> String;
}

impl<B> TenantContextExt for Request<B> {
    fn tenant_context(&self) -> Option<&TenantContext> {
        self.extensions().get::<TenantContext>()
    }

    fn tenant_id_or_default(&self) -> String {
        self.tenant_context().map_or_else(|| "default".to_string(), |ctx| ctx.tenant_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_context_creation() {
        let ctx = TenantContext::new("tenant1", TenantIdSource::Header);
        assert_eq!(ctx.tenant_id, "tenant1");
        assert_eq!(ctx.source, TenantIdSource::Header);
        assert!(!ctx.is_system_tenant);
        assert!(ctx.is_valid());
    }

    #[test]
    fn test_system_tenant_detection() {
        let ctx = TenantContext::new("system", TenantIdSource::Header);
        assert!(ctx.is_system_tenant);

        let ctx = TenantContext::new("admin", TenantIdSource::Header);
        assert!(ctx.is_system_tenant);

        let ctx = TenantContext::new("customer1", TenantIdSource::Header);
        assert!(!ctx.is_system_tenant);
    }

    #[test]
    fn test_default_tenant_context() {
        let ctx = TenantContext::default_tenant();
        assert_eq!(ctx.tenant_id, "default");
        assert_eq!(ctx.source, TenantIdSource::Default);
    }

    #[test]
    fn test_config_single_tenant() {
        let config = TenantExtractorConfig::single_tenant("my-tenant");
        assert_eq!(config.default_tenant, Some("my-tenant".to_string()));
        assert!(!config.require_tenant);
        assert!(!config.validate_tenant);
    }

    #[test]
    fn test_config_multi_tenant() {
        let config = TenantExtractorConfig::multi_tenant();
        assert!(config.default_tenant.is_none());
        assert!(config.require_tenant);
        assert!(config.validate_tenant);
    }

    #[test]
    fn test_tenant_id_source_display() {
        assert_eq!(TenantIdSource::Header.to_string(), "header");
        assert_eq!(TenantIdSource::JwtClaim.to_string(), "jwt");
        assert_eq!(TenantIdSource::ApiKey.to_string(), "api_key");
        assert_eq!(TenantIdSource::Default.to_string(), "default");
        assert_eq!(TenantIdSource::System.to_string(), "system");
    }
}
