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
//! use proximadb::network::middleware::tenant::{TenantExtractor, MiddlewareTenantContext};
//!
//! // Create tenant extractor with optional default tenant
//! let extractor = TenantExtractor::new()
//!     .with_default_tenant("default");
//!
//! // In handlers, access tenant context:
//! async fn handler(Extension(tenant): Extension<MiddlewareTenantContext>) -> impl IntoResponse {
//!     println!("Request from tenant: {}", tenant.tenant_id);
//! }
//! ```

use axum::{
    extract::Request,
    http::StatusCode,
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
///
/// Backwards-compat alias for [`MiddlewareTenantContext`].
pub type TenantContext = MiddlewareTenantContext;

#[derive(Debug, Clone)]
pub struct MiddlewareTenantContext {
    /// The tenant identifier
    pub tenant_id: String,
    /// Owning customer **account** — the SaaS billing/isolation boundary above
    /// `tenant_id` (a tenant is a workspace/sub-org inside an account). `None`
    /// is single-account / default mode (resolve via
    /// [`account_or_default`](Self::account_or_default)); the account tier is
    /// inert until provisioned (Phase 5 two-tier operator/account model).
    pub account_id: Option<String>,
    /// Source of the tenant ID (for audit logging)
    pub source: TenantIdSource,
    /// Whether this is a system/admin tenant with elevated privileges. Doubles
    /// as the operator (control-plane) marker.
    pub is_system_tenant: bool,
}

impl MiddlewareTenantContext {
    /// Create a new tenant context
    pub fn new(tenant_id: impl Into<String>, source: TenantIdSource) -> Self {
        let tenant_id = tenant_id.into();
        let is_system_tenant = tenant_id == "system" || tenant_id == "admin";
        Self {
            tenant_id,
            account_id: None,
            source,
            is_system_tenant,
        }
    }

    /// Create a default/anonymous tenant context. Uses the ONE canonical default
    /// (`proximadb_tenant::DEFAULT_TENANT`) shared by every surface.
    pub fn default_tenant() -> Self {
        Self::new(proximadb_tenant::DEFAULT_TENANT, TenantIdSource::Default)
    }

    /// Set the owning customer account (Phase 5). Activates the account-rooted
    /// isolation tree for this request; leaving it unset keeps single-account
    /// / default behaviour.
    pub fn with_account(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    /// The owning account ID, falling back to the reserved default-account ID
    /// when none was provisioned. Use at I/O boundaries that always need an
    /// account segment (e.g. `DrPathBuilder` account-rooted paths).
    pub fn account_or_default(&self) -> &str {
        self.account_id.as_deref().unwrap_or(
            crate::storage::trait_components::path_resolver::DrPathBuilder::DEFAULT_ACCOUNT_ID,
        )
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

/// Trust policy for the **bare** `X-Tenant-ID` header — i.e. a request that
/// asserts a tenant via header while carrying NO authenticated tenant binding
/// (no JWT claim / API-key mapping). When a binding exists, the header must
/// match it (403 otherwise) in every mode; `GatewayOnly` additionally lets an
/// authenticated system/gateway principal delegate — select an acting tenant
/// via the header (the trusted-gateway topology: the gateway authenticates
/// with a service credential and stamps the end user's tenant per request).
///
/// Env override at server construction: `PROXIMADB_TENANT_HEADER_TRUST` =
/// `open` | `authenticated-only` | `gateway-only` (see
/// [`TenantExtractorConfig::apply_env_overrides`]). Unset ⇒ the config value
/// (default-safe: `Open` preserves existing deployments).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeaderTrustPolicy {
    /// Accept the bare header verbatim. Correct for dev, single-tenant, and
    /// network-isolated trusted-gateway topologies. The default.
    #[default]
    Open,
    /// Reject (403 + audit log) any request that asserts a tenant via header
    /// without an authenticated binding. Credential-derived tenants
    /// (JWT/API-key) are unaffected. Default for
    /// [`TenantExtractorConfig::multi_tenant`] strict mode.
    AuthenticatedOnly,
    /// Like `AuthenticatedOnly`, but an authenticated **system/gateway
    /// principal** (its bound tenant is in
    /// [`TenantExtractorConfig::system_tenants`]) may set `X-Tenant-ID` to act
    /// on behalf of that tenant (delegation is audit-logged).
    GatewayOnly,
}

impl std::fmt::Display for HeaderTrustPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::AuthenticatedOnly => write!(f, "authenticated-only"),
            Self::GatewayOnly => write!(f, "gateway-only"),
        }
    }
}

impl std::str::FromStr for HeaderTrustPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "open" => Ok(Self::Open),
            "authenticated-only" | "authenticated-match" => Ok(Self::AuthenticatedOnly),
            "gateway-only" => Ok(Self::GatewayOnly),
            other => Err(format!(
                "invalid tenant header-trust policy '{other}' \
                 (expected open | authenticated-only | gateway-only)"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TenantExtractionError {
    HeaderAuthenticatedMismatch {
        requested: String,
        authenticated: String,
    },
    /// The bare header was rejected by a non-`Open` [`HeaderTrustPolicy`]:
    /// the request asserted a tenant without any authenticated binding.
    UnauthenticatedHeaderRejected { requested: String },
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
    /// Trust policy for the bare `X-Tenant-ID` header (see
    /// [`HeaderTrustPolicy`]). Default `Open` — default-safe for existing
    /// deployments; `multi_tenant()` strict mode uses `AuthenticatedOnly`.
    pub header_trust: HeaderTrustPolicy,
}

impl Default for TenantExtractorConfig {
    fn default() -> Self {
        Self {
            // The ONE canonical default shared by every surface (foundation).
            default_tenant: Some(proximadb_tenant::DEFAULT_TENANT.to_string()),
            require_tenant: false,  // Allow single-tenant mode by default
            validate_tenant: false, // Disable validation by default (enable in production)
            system_tenants: vec!["system".to_string(), "admin".to_string()],
            header_trust: HeaderTrustPolicy::Open,
        }
    }
}

impl TenantExtractorConfig {
    /// Create config from the explicit foundation deployment-mode contract (TD-CAT-3).
    pub fn from_deployment_mode(mode: proximadb_tenant::TenantDeploymentMode) -> Self {
        match mode {
            proximadb_tenant::TenantDeploymentMode::SingleTenant { default_tenant } => {
                Self::single_tenant(default_tenant)
            }
            proximadb_tenant::TenantDeploymentMode::MultiTenant => Self::multi_tenant(),
        }
    }

    /// Create config for single-tenant deployment
    pub fn single_tenant(default_tenant: impl Into<String>) -> Self {
        Self {
            default_tenant: Some(default_tenant.into()),
            require_tenant: false,
            validate_tenant: false,
            system_tenants: vec!["system".to_string()],
            header_trust: HeaderTrustPolicy::Open,
        }
    }

    /// Create config for multi-tenant deployment (strict mode)
    pub fn multi_tenant() -> Self {
        Self {
            default_tenant: None,
            require_tenant: true,
            validate_tenant: true,
            system_tenants: vec!["system".to_string(), "admin".to_string()],
            // Strict mode: a tenant asserted via bare header without an
            // authenticated binding is a masquerade vector, not an identity.
            // Relax per-deployment with PROXIMADB_TENANT_HEADER_TRUST=open
            // (trusted-gateway topologies should prefer gateway-only).
            header_trust: HeaderTrustPolicy::AuthenticatedOnly,
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

    /// Builder: Set the bare-header trust policy.
    pub fn with_header_trust(mut self, policy: HeaderTrustPolicy) -> Self {
        self.header_trust = policy;
        self
    }

    /// Apply deployment env overrides. Called at server construction (NOT in
    /// constructors, so tests and embedded uses stay hermetic). Currently:
    /// `PROXIMADB_TENANT_HEADER_TRUST` = `open` | `authenticated-only` |
    /// `gateway-only` overrides `header_trust`; an unparseable value is
    /// rejected loudly rather than silently weakening the policy.
    pub fn apply_env_overrides(mut self) -> Self {
        if let Ok(raw) = std::env::var("PROXIMADB_TENANT_HEADER_TRUST") {
            match raw.parse::<HeaderTrustPolicy>() {
                Ok(policy) => {
                    tracing::info!(
                        %policy,
                        "tenant header-trust policy set from PROXIMADB_TENANT_HEADER_TRUST"
                    );
                    self.header_trust = policy;
                }
                Err(e) => {
                    // Fail-closed: an operator explicitly set a policy we can't
                    // parse — tighten to AuthenticatedOnly instead of silently
                    // running Open.
                    warn!(
                        error = %e,
                        "invalid PROXIMADB_TENANT_HEADER_TRUST; tightening to authenticated-only"
                    );
                    self.header_trust = HeaderTrustPolicy::AuthenticatedOnly;
                }
            }
        }
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
    fn extract_tenant_id(
        &self,
        req: &Request,
    ) -> Result<Option<(String, TenantIdSource)>, TenantExtractionError> {
        let requested_tenant = req
            .headers()
            .get(X_TENANT_ID)
            .and_then(|header_value| header_value.to_str().ok())
            .map(str::trim)
            .filter(|tenant_id| !tenant_id.is_empty())
            .map(ToOwned::to_owned);

        if let Some((authenticated_tenant, source)) = Self::authenticated_tenant_id(req) {
            if let Some(requested) = requested_tenant
                && requested != authenticated_tenant
            {
                // GatewayOnly delegation: an authenticated system/gateway
                // principal may act on behalf of the header tenant (the
                // trusted-gateway topology — the gateway authenticates with a
                // service credential and stamps the end user's tenant).
                if self.config.header_trust == HeaderTrustPolicy::GatewayOnly
                    && self.config.system_tenants.contains(&authenticated_tenant)
                {
                    debug!(
                        gateway = %authenticated_tenant,
                        acting_tenant = %requested,
                        "gateway principal delegated tenant via X-Tenant-ID"
                    );
                    return Ok(Some((requested, TenantIdSource::Header)));
                }
                return Err(TenantExtractionError::HeaderAuthenticatedMismatch {
                    requested,
                    authenticated: authenticated_tenant,
                });
            }
            debug!(
                "Extracted tenant_id from authenticated context: {}",
                authenticated_tenant
            );
            return Ok(Some((authenticated_tenant, source)));
        }

        // Bare header — no authenticated tenant binding to compare against.
        // Whether it is an identity or a masquerade vector is the deployment's
        // header-trust policy call (see `HeaderTrustPolicy`).
        if let Some(tenant_id) = requested_tenant {
            return match self.config.header_trust {
                HeaderTrustPolicy::Open => {
                    debug!("Extracted tenant_id from header: {}", tenant_id);
                    Ok(Some((tenant_id, TenantIdSource::Header)))
                }
                HeaderTrustPolicy::AuthenticatedOnly | HeaderTrustPolicy::GatewayOnly => {
                    Err(TenantExtractionError::UnauthenticatedHeaderRejected {
                        requested: tenant_id,
                    })
                }
            };
        }

        // Priority 3: Default tenant (if configured)
        if let Some(ref default_tenant) = self.config.default_tenant {
            debug!("Using default tenant: {}", default_tenant);
            return Ok(Some((default_tenant.clone(), TenantIdSource::Default)));
        }

        Ok(None)
    }

    fn authenticated_tenant_id(req: &Request) -> Option<(String, TenantIdSource)> {
        if let Some(user_context) = req
            .extensions()
            .get::<crate::security::UnifiedUserContext>()
            && let Some(tenant_id) = user_context.tenant_id.as_ref()
        {
            let source = if matches!(
                &user_context.auth_method,
                crate::security::UnifiedAuthMethod::ApiKey
            ) {
                TenantIdSource::ApiKey
            } else {
                TenantIdSource::JwtClaim
            };
            return Some((tenant_id.clone(), source));
        }

        if let Some(user_info) = req.extensions().get::<super::auth::UserInfo>()
            && let Some(ref tenant_id) = user_info.tenant_id
        {
            return Some((tenant_id.clone(), TenantIdSource::ApiKey));
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
/// MiddlewareTenantContext into request extensions.
pub async fn tenant_middleware(
    axum::extract::State(extractor): axum::extract::State<TenantExtractor>,
    mut req: Request,
    next: Next,
) -> Response {
    // Extract tenant ID
    match extractor.extract_tenant_id(&req) {
        Ok(Some((tenant_id, source))) => {
            if let Err(err) = proximadb_tenant::validate_request_tenant(&tenant_id) {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid tenant id '{}': {err}", tenant_id),
                )
                    .into_response();
            }

            // Validate tenant if configured
            if !extractor.validate_tenant(&tenant_id) {
                return (
                    StatusCode::FORBIDDEN,
                    format!("Tenant '{}' is not valid or not active", tenant_id),
                )
                    .into_response();
            }

            // Open-core cache tier hook: if the control plane stamped a tier
            // claim (`X-Tenant-Tier`), record it in the process-global registry
            // the cache LimitsResolver reads. The id is opaque (commercial tier
            // names + their cache shares are operator config, not OSS code).
            let tier_claim = req
                .headers()
                .get("x-tenant-tier")
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            // Inject tenant context into request extensions
            let context = MiddlewareTenantContext::new(tenant_id, source);
            if let Some(tier) = tier_claim {
                crate::services::record_store::set_tenant_tier(&context.tenant_id, tier);
            }
            // Also inject api-crate MiddlewareTenantContext for port-backed handlers in proximadb-api
            req.extensions_mut()
                .insert(proximadb_api::rest::TenantContext {
                    tenant_id: context.tenant_id.clone(),
                });
            req.extensions_mut().insert(context);

            next.run(req).await
        }
        Ok(None) => {
            if extractor.config.require_tenant {
                // Tenant required but not provided
                (
                    StatusCode::BAD_REQUEST,
                    "Tenant ID required. Provide X-Tenant-ID header or authenticate with tenant-bound credentials.",
                ).into_response()
            } else {
                // No tenant required, use anonymous context
                let default_ctx = MiddlewareTenantContext::default_tenant();
                req.extensions_mut()
                    .insert(proximadb_api::rest::TenantContext {
                        tenant_id: default_ctx.tenant_id.clone(),
                    });
                req.extensions_mut().insert(default_ctx);
                next.run(req).await
            }
        }
        Err(TenantExtractionError::HeaderAuthenticatedMismatch {
            requested,
            authenticated,
        }) => {
            // Audit trail: a credentialed principal asserted a DIFFERENT
            // tenant via header — the masquerade signature.
            warn!(
                target: "proximadb::tenant_audit",
                requested = %requested,
                authenticated = %authenticated,
                source = %TenantIdSource::Header,
                "rejected X-Tenant-ID: does not match authenticated tenant binding"
            );
            (
                StatusCode::FORBIDDEN,
                format!(
                    "Tenant '{}' does not match authenticated tenant '{}'",
                    requested, authenticated
                ),
            )
                .into_response()
        }
        Err(TenantExtractionError::UnauthenticatedHeaderRejected { requested }) => {
            warn!(
                target: "proximadb::tenant_audit",
                requested = %requested,
                source = %TenantIdSource::Header,
                policy = %extractor.config.header_trust,
                "rejected bare X-Tenant-ID without authenticated tenant binding"
            );
            (
                StatusCode::FORBIDDEN,
                format!(
                    "Tenant '{}' asserted via X-Tenant-ID without authenticated credentials; \
                     this deployment requires a tenant-bound credential (JWT or API key)",
                    requested
                ),
            )
                .into_response()
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
    fn tenant_context(&self) -> Option<&MiddlewareTenantContext>;

    /// Get tenant ID or default
    fn tenant_id_or_default(&self) -> String;
}

impl TenantContextExt for Request {
    fn tenant_context(&self) -> Option<&MiddlewareTenantContext> {
        self.extensions().get::<MiddlewareTenantContext>()
    }

    fn tenant_id_or_default(&self) -> String {
        self.tenant_context()
            .map_or_else(|| "default".to_string(), |ctx| ctx.tenant_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_context_creation() {
        let ctx = MiddlewareTenantContext::new("tenant1", TenantIdSource::Header);
        assert_eq!(ctx.tenant_id, "tenant1");
        assert_eq!(ctx.source, TenantIdSource::Header);
        assert!(!ctx.is_system_tenant);
        assert!(ctx.is_valid());
    }

    #[test]
    fn test_system_tenant_detection() {
        let ctx = MiddlewareTenantContext::new("system", TenantIdSource::Header);
        assert!(ctx.is_system_tenant);

        let ctx = MiddlewareTenantContext::new("admin", TenantIdSource::Header);
        assert!(ctx.is_system_tenant);

        let ctx = MiddlewareTenantContext::new("customer1", TenantIdSource::Header);
        assert!(!ctx.is_system_tenant);
    }

    #[test]
    fn test_default_tenant_context() {
        let ctx = MiddlewareTenantContext::default_tenant();
        assert_eq!(ctx.tenant_id, "default");
        assert_eq!(ctx.source, TenantIdSource::Default);
    }

    #[test]
    fn rest_default_uses_the_one_canonical_constant() {
        // The middleware default context AND the extractor config default both
        // derive from the single `proximadb_tenant::DEFAULT_TENANT`, so REST can
        // never drift from pgwire/gRPC (which resolve the same constant).
        assert_eq!(
            MiddlewareTenantContext::default_tenant().tenant_id,
            proximadb_tenant::DEFAULT_TENANT
        );
        assert_eq!(
            TenantExtractorConfig::default().default_tenant.as_deref(),
            Some(proximadb_tenant::DEFAULT_TENANT)
        );
    }

    #[test]
    fn test_account_tier_inert_until_set() {
        // Unset account → resolves to the reserved default-account ID.
        let ctx = MiddlewareTenantContext::new("tnt_acme", TenantIdSource::Header);
        assert!(ctx.account_id.is_none());
        assert_eq!(ctx.account_or_default(), "default");

        // Provisioned account → used verbatim.
        let ctx = ctx.with_account("acct_acme");
        assert_eq!(ctx.account_id.as_deref(), Some("acct_acme"));
        assert_eq!(ctx.account_or_default(), "acct_acme");
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
    fn config_from_deployment_mode_matches_explicit_contract() {
        let single = TenantExtractorConfig::from_deployment_mode(
            proximadb_tenant::TenantDeploymentMode::single_tenant("tenant_a"),
        );
        assert_eq!(single.default_tenant.as_deref(), Some("tenant_a"));
        assert!(!single.require_tenant);
        assert!(!single.validate_tenant);

        let multi = TenantExtractorConfig::from_deployment_mode(
            proximadb_tenant::TenantDeploymentMode::MultiTenant,
        );
        assert!(multi.default_tenant.is_none());
        assert!(multi.require_tenant);
        assert!(multi.validate_tenant);
    }

    #[test]
    fn test_tenant_id_source_display() {
        assert_eq!(TenantIdSource::Header.to_string(), "header");
        assert_eq!(TenantIdSource::JwtClaim.to_string(), "jwt");
        assert_eq!(TenantIdSource::ApiKey.to_string(), "api_key");
        assert_eq!(TenantIdSource::Default.to_string(), "default");
        assert_eq!(TenantIdSource::System.to_string(), "system");
    }

    // ── HeaderTrustPolicy (bare X-Tenant-ID hardening) ──────────────────────

    /// Build a request with an optional bare X-Tenant-ID header and an
    /// optional authenticated tenant binding (UnifiedUserContext extension,
    /// as the auth middleware would inject it).
    fn trust_request(header: Option<&str>, authenticated: Option<&str>) -> Request {
        let mut builder = axum::http::Request::builder().uri("/api/v2/anything");
        if let Some(tenant) = header {
            builder = builder.header(X_TENANT_ID, tenant);
        }
        let mut req = builder.body(axum::body::Body::empty()).expect("request");
        if let Some(tenant) = authenticated {
            let mut ctx = crate::security::UnifiedUserContext::anonymous();
            ctx.tenant_id = Some(tenant.to_string());
            ctx.auth_method = crate::security::UnifiedAuthMethod::JWT;
            req.extensions_mut().insert(ctx);
        }
        req
    }

    fn extractor(policy: HeaderTrustPolicy) -> TenantExtractor {
        TenantExtractor::with_config(TenantExtractorConfig {
            header_trust: policy,
            ..TenantExtractorConfig::default()
        })
    }

    #[test]
    fn open_accepts_bare_header() {
        let result = extractor(HeaderTrustPolicy::Open)
            .extract_tenant_id(&trust_request(Some("demo1"), None))
            .expect("open mode accepts bare header");
        assert_eq!(result, Some(("demo1".to_string(), TenantIdSource::Header)));
    }

    #[test]
    fn authenticated_only_rejects_bare_header() {
        let err = extractor(HeaderTrustPolicy::AuthenticatedOnly)
            .extract_tenant_id(&trust_request(Some("demo1"), None))
            .expect_err("bare header must be rejected without a credential binding");
        assert_eq!(
            err,
            TenantExtractionError::UnauthenticatedHeaderRejected {
                requested: "demo1".to_string()
            }
        );
    }

    #[test]
    fn gateway_only_rejects_bare_header() {
        let err = extractor(HeaderTrustPolicy::GatewayOnly)
            .extract_tenant_id(&trust_request(Some("demo1"), None))
            .expect_err("bare header must be rejected without a credential binding");
        assert!(matches!(
            err,
            TenantExtractionError::UnauthenticatedHeaderRejected { .. }
        ));
    }

    #[test]
    fn credential_derived_tenant_flows_in_every_mode() {
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            let result = extractor(policy)
                .extract_tenant_id(&trust_request(None, Some("acme")))
                .expect("credential-derived tenant is always accepted");
            assert_eq!(
                result,
                Some(("acme".to_string(), TenantIdSource::JwtClaim)),
                "policy {policy}"
            );
        }
    }

    #[test]
    fn matching_header_and_binding_resolve_to_binding() {
        let result = extractor(HeaderTrustPolicy::AuthenticatedOnly)
            .extract_tenant_id(&trust_request(Some("acme"), Some("acme")))
            .expect("matching header+binding is fine");
        assert_eq!(result, Some(("acme".to_string(), TenantIdSource::JwtClaim)));
    }

    #[test]
    fn spoofed_header_with_credential_rejected_in_every_mode_for_non_gateway() {
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            let err = extractor(policy)
                .extract_tenant_id(&trust_request(Some("victim"), Some("acme")))
                .expect_err("header != binding is the masquerade signature");
            assert_eq!(
                err,
                TenantExtractionError::HeaderAuthenticatedMismatch {
                    requested: "victim".to_string(),
                    authenticated: "acme".to_string(),
                },
                "policy {policy}"
            );
        }
    }

    #[test]
    fn gateway_only_lets_system_principal_delegate_tenant() {
        // The gateway authenticates with a service credential bound to the
        // "system" tenant (in system_tenants) and stamps the end user's
        // tenant via X-Tenant-ID.
        let result = extractor(HeaderTrustPolicy::GatewayOnly)
            .extract_tenant_id(&trust_request(Some("demo1"), Some("system")))
            .expect("gateway delegation must be allowed");
        assert_eq!(result, Some(("demo1".to_string(), TenantIdSource::Header)));
    }

    #[test]
    fn non_gateway_modes_do_not_allow_system_delegation() {
        // Delegation is a GatewayOnly capability — Open/AuthenticatedOnly keep
        // the strict header==binding contract even for system principals.
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
        ] {
            let err = extractor(policy)
                .extract_tenant_id(&trust_request(Some("demo1"), Some("system")))
                .expect_err("delegation requires gateway-only mode");
            assert!(
                matches!(
                    err,
                    TenantExtractionError::HeaderAuthenticatedMismatch { .. }
                ),
                "policy {policy}"
            );
        }
    }

    #[test]
    fn no_header_no_credential_falls_back_to_default_in_every_mode() {
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            let result = extractor(policy)
                .extract_tenant_id(&trust_request(None, None))
                .expect("default fallback is not affected by header trust");
            assert_eq!(
                result,
                Some((
                    proximadb_tenant::DEFAULT_TENANT.to_string(),
                    TenantIdSource::Default
                )),
                "policy {policy}"
            );
        }
    }

    #[test]
    fn header_trust_policy_parses_and_displays() {
        use std::str::FromStr;
        assert_eq!(
            HeaderTrustPolicy::from_str("open").unwrap(),
            HeaderTrustPolicy::Open
        );
        assert_eq!(
            HeaderTrustPolicy::from_str("authenticated-only").unwrap(),
            HeaderTrustPolicy::AuthenticatedOnly
        );
        assert_eq!(
            HeaderTrustPolicy::from_str("AUTHENTICATED_MATCH").unwrap(),
            HeaderTrustPolicy::AuthenticatedOnly
        );
        assert_eq!(
            HeaderTrustPolicy::from_str("gateway-only").unwrap(),
            HeaderTrustPolicy::GatewayOnly
        );
        assert!(HeaderTrustPolicy::from_str("everything-goes").is_err());
        assert_eq!(HeaderTrustPolicy::default(), HeaderTrustPolicy::Open);
        assert_eq!(
            HeaderTrustPolicy::AuthenticatedOnly.to_string(),
            "authenticated-only"
        );
    }

    /// End-to-end through the axum layer: the policy maps to real HTTP
    /// statuses (403 for a rejected bare header, 200 for the default-tenant
    /// fallback and for gateway delegation).
    #[tokio::test]
    async fn middleware_maps_header_trust_to_http_statuses() {
        use axum::{Router, routing::get};
        use tower::ServiceExt;

        let app = |policy: HeaderTrustPolicy| {
            Router::new().route("/probe", get(|| async { "ok" })).layer(
                axum::middleware::from_fn_with_state(extractor(policy), tenant_middleware),
            )
        };

        // Bare header under authenticated-only → 403.
        let denied = app(HeaderTrustPolicy::AuthenticatedOnly)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/probe")
                    .header(X_TENANT_ID, "demo1")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        // No header → default-tenant fallback still flows (200).
        let allowed = app(HeaderTrustPolicy::AuthenticatedOnly)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/probe")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(allowed.status(), StatusCode::OK);

        // Bare header under open → 200 (legacy behavior preserved).
        let open = app(HeaderTrustPolicy::Open)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/probe")
                    .header(X_TENANT_ID, "demo1")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(open.status(), StatusCode::OK);
    }

    #[test]
    fn multi_tenant_strict_mode_defaults_to_authenticated_only() {
        assert_eq!(
            TenantExtractorConfig::multi_tenant().header_trust,
            HeaderTrustPolicy::AuthenticatedOnly
        );
        // Default + single_tenant stay Open (default-safe for existing deployments).
        assert_eq!(
            TenantExtractorConfig::default().header_trust,
            HeaderTrustPolicy::Open
        );
        assert_eq!(
            TenantExtractorConfig::single_tenant("t").header_trust,
            HeaderTrustPolicy::Open
        );
    }
}
