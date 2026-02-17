//! Unified Authentication Service for ProximaDB
//!
//! Consolidates authentication logic from multiple sources:
//! - EnterpriseAuthManager (src/auth/mod.rs)
//! - Network Auth Service (src/network/auth/mod.rs)
//! - Auth Middleware (src/network/middleware/auth.rs)

use super::unified_rbac::{AuthMethod, UnifiedPermission, UnifiedUserContext};
use crate::audit::logger::AuditLogger;
use crate::audit::types::{AuditEvent, AuditEventType, AuditResource, AuditResult};
use crate::auth::{EnterpriseAuthManager, EnterpriseUserContext, SSOToken};
use crate::network::auth::{JwtService, TokenPair};

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::warn;

/// Unified authentication service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticationConfig {
    pub enabled: bool,
    pub methods: Vec<AuthenticationMethod>,
    pub require_authentication: bool,
    pub default_session_timeout_minutes: u64,
    pub api_keys: HashMap<String, ApiKeyInfo>,
    pub jwt: JwtConfig,
    pub sso: SSOConfig,
}

/// Authentication methods supported
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthenticationMethod {
    #[serde(rename = "sso")]
    SSO,
    #[serde(rename = "jwt")]
    JWT,
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "mtls")]
    ClientCertificate,
}

/// API key information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyInfo {
    pub user_id: String,
    pub tenant_id: Option<String>,
    pub permissions: Vec<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub rate_limit_per_minute: Option<u32>,
    pub ip_restrictions: Vec<String>,
}

/// JWT configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtConfig {
    pub enabled: bool,
    pub secret: String,
    pub access_token_expiration_minutes: u64,
    pub refresh_token_expiration_days: u64,
    pub issuer: String,
    pub audience: String,
    pub algorithm: String,
}

/// SSO configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SSOConfig {
    pub enabled: bool,
    pub providers: Vec<String>,
    pub token_cache_ttl_minutes: u64,
    pub aws_iam: Option<AWSIAMConfig>,
    pub azure_ad: Option<AzureADConfig>,
}

/// AWS IAM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AWSIAMConfig {
    pub role_arn: String,
    pub session_duration_minutes: u64,
    pub region: String,
}

/// Azure AD configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureADConfig {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: Vec<String>,
}

/// Authentication result from unified service
#[derive(Debug, Clone)]
pub struct AuthenticationResult {
    pub user_context: UnifiedUserContext,
    pub auth_method: AuthMethod,
    pub success: bool,
    pub error_message: Option<String>,
    pub requires_mfa: bool,
}

/// Unified authentication service
pub struct UnifiedAuthService {
    /// Enterprise auth manager for SSO
    enterprise_auth: Option<Arc<EnterpriseAuthManager>>,

    /// JWT service for token authentication
    jwt_service: Option<Arc<JwtService>>,

    /// API key store
    api_keys: Arc<DashMap<String, ApiKeyInfo>>,

    /// Configuration
    config: AuthenticationConfig,

    /// Audit logger for authentication events
    audit_logger: Option<Arc<AuditLogger>>,
}

impl UnifiedAuthService {
    /// Create new unified authentication service
    pub fn new(config: AuthenticationConfig) -> Result<Self> {
        let mut service = Self {
            enterprise_auth: None,
            jwt_service: None,
            api_keys: Arc::new(DashMap::new()),
            config: config.clone(),
            audit_logger: None,
        };

        // Initialize JWT service if enabled
        if config.jwt.enabled {
            // Convert unified JwtConfig to network JwtConfig
            let network_jwt_config = crate::network::auth::config::JwtConfig {
                secret: Some(config.jwt.secret.clone()),
                expiration_secs: config.jwt.access_token_expiration_minutes * 60,
                refresh_expiration_secs: config.jwt.refresh_token_expiration_days * 24 * 3600,
                issuer: config.jwt.issuer.clone(),
                audience: config.jwt.audience.clone(),
                algorithm: crate::network::auth::config::JwtAlgorithm::HS256, // Default to HS256
            };
            let jwt_service = JwtService::new(network_jwt_config)?;
            service.jwt_service = Some(Arc::new(jwt_service));
        }

        // Load API keys
        for (key, info) in config.api_keys {
            service.api_keys.insert(key, info);
        }

        Ok(service)
    }

    /// Set enterprise auth manager for SSO integration
    pub fn set_enterprise_auth(&mut self, enterprise_auth: Arc<EnterpriseAuthManager>) {
        self.enterprise_auth = Some(enterprise_auth);
    }

    /// Set audit logger
    pub fn set_audit_logger(&mut self, audit_logger: Arc<AuditLogger>) {
        self.audit_logger = Some(audit_logger);
    }

    /// Authenticate request using multiple methods
    pub async fn authenticate(
        &self,
        auth_data: AuthenticationData,
    ) -> Result<AuthenticationResult> {
        let start_time = Utc::now();

        let result = match &auth_data {
            AuthenticationData::SSOToken(token) => self.authenticate_sso_token(token).await,
            AuthenticationData::JWTToken(token) => self.authenticate_jwt_token(token).await,
            AuthenticationData::ApiKey(key) => self.authenticate_api_key(key).await,
            AuthenticationData::ClientCertificate(cert_data) => {
                self.authenticate_client_certificate(cert_data).await
            }
        };

        // Log authentication attempt
        if let Some(audit_logger) = &self.audit_logger {
            let auth_event = match &result {
                Ok(auth_result) => create_auth_audit_event(&auth_data, auth_result, start_time),
                Err(_) => {
                    // Create failed auth event
                    let failed_result = AuthenticationResult {
                        user_context: UnifiedUserContext::anonymous(),
                        auth_method: AuthMethod::Internal,
                        success: false,
                        error_message: Some("Authentication failed".to_string()),
                        requires_mfa: false,
                    };
                    create_auth_audit_event(&auth_data, &failed_result, start_time)
                }
            };
            let _ = audit_logger.log_event(auth_event).await;
        }

        result
    }

    /// Authenticate SSO token
    async fn authenticate_sso_token(&self, token: &SSOToken) -> Result<AuthenticationResult> {
        if !self.config.methods.contains(&AuthenticationMethod::SSO) {
            return Ok(AuthenticationResult {
                user_context: UnifiedUserContext::anonymous(),
                auth_method: AuthMethod::SSO {
                    provider: "disabled".to_string(),
                },
                success: false,
                error_message: Some("SSO authentication disabled".to_string()),
                requires_mfa: false,
            });
        }

        match &self.enterprise_auth {
            Some(enterprise_auth) => {
                match enterprise_auth.validate_and_resolve_token(token).await {
                    Ok(enterprise_user) => {
                        let user_context = self.convert_enterprise_user_to_unified(enterprise_user);
                        Ok(AuthenticationResult {
                            user_context,
                            auth_method: AuthMethod::SSO {
                                provider: format!("{:?}", token.provider),
                            },
                            success: true,
                            error_message: None,
                            requires_mfa: false,
                        })
                    }
                    Err(e) => {
                        warn!("SSO authentication failed: {}", e);
                        Ok(AuthenticationResult {
                            user_context: UnifiedUserContext::anonymous(),
                            auth_method: AuthMethod::SSO {
                                provider: format!("{:?}", token.provider),
                            },
                            success: false,
                            error_message: Some(e.to_string()),
                            requires_mfa: false,
                        })
                    }
                }
            }
            None => Err(anyhow!(
                "SSO authentication enabled but enterprise auth manager not configured"
            )),
        }
    }

    /// Authenticate JWT token
    async fn authenticate_jwt_token(&self, token: &str) -> Result<AuthenticationResult> {
        if !self.config.methods.contains(&AuthenticationMethod::JWT) {
            return Ok(AuthenticationResult {
                user_context: UnifiedUserContext::anonymous(),
                auth_method: AuthMethod::JWT,
                success: false,
                error_message: Some("JWT authentication disabled".to_string()),
                requires_mfa: false,
            });
        }

        match &self.jwt_service {
            Some(jwt_service) => match jwt_service.verify_token(token).await {
                Ok(claims) => {
                    let user_context = self.convert_jwt_claims_to_unified(claims);
                    Ok(AuthenticationResult {
                        user_context,
                        auth_method: AuthMethod::JWT,
                        success: true,
                        error_message: None,
                        requires_mfa: false,
                    })
                }
                Err(e) => {
                    warn!("JWT authentication failed: {}", e);
                    Ok(AuthenticationResult {
                        user_context: UnifiedUserContext::anonymous(),
                        auth_method: AuthMethod::JWT,
                        success: false,
                        error_message: Some(e.to_string()),
                        requires_mfa: false,
                    })
                }
            },
            None => Err(anyhow!(
                "JWT authentication enabled but JWT service not configured"
            )),
        }
    }

    /// Authenticate API key
    async fn authenticate_api_key(&self, api_key: &str) -> Result<AuthenticationResult> {
        if !self.config.methods.contains(&AuthenticationMethod::ApiKey) {
            return Ok(AuthenticationResult {
                user_context: UnifiedUserContext::anonymous(),
                auth_method: AuthMethod::ApiKey,
                success: false,
                error_message: Some("API key authentication disabled".to_string()),
                requires_mfa: false,
            });
        }

        match self.api_keys.get(api_key) {
            Some(api_key_info) => {
                // Check if API key has expired
                if let Some(expires_at) = api_key_info.expires_at {
                    if Utc::now() > expires_at {
                        return Ok(AuthenticationResult {
                            user_context: UnifiedUserContext::anonymous(),
                            auth_method: AuthMethod::ApiKey,
                            success: false,
                            error_message: Some("API key expired".to_string()),
                            requires_mfa: false,
                        });
                    }
                }

                let user_context = self.convert_api_key_to_unified(api_key_info.clone());
                Ok(AuthenticationResult {
                    user_context,
                    auth_method: AuthMethod::ApiKey,
                    success: true,
                    error_message: None,
                    requires_mfa: false,
                })
            }
            None => {
                warn!(
                    "Invalid API key attempted: {}",
                    &api_key[..std::cmp::min(8, api_key.len())]
                );
                Ok(AuthenticationResult {
                    user_context: UnifiedUserContext::anonymous(),
                    auth_method: AuthMethod::ApiKey,
                    success: false,
                    error_message: Some("Invalid API key".to_string()),
                    requires_mfa: false,
                })
            }
        }
    }

    /// Authenticate client certificate (placeholder for mTLS)
    async fn authenticate_client_certificate(
        &self,
        _cert_data: &ClientCertificateData,
    ) -> Result<AuthenticationResult> {
        if !self
            .config
            .methods
            .contains(&AuthenticationMethod::ClientCertificate)
        {
            return Ok(AuthenticationResult {
                user_context: UnifiedUserContext::anonymous(),
                auth_method: AuthMethod::ClientCertificate,
                success: false,
                error_message: Some("Client certificate authentication disabled".to_string()),
                requires_mfa: false,
            });
        }

        // TODO: Implement client certificate validation
        // For now, return placeholder implementation
        warn!("Client certificate authentication not yet implemented");
        Ok(AuthenticationResult {
            user_context: UnifiedUserContext::anonymous(),
            auth_method: AuthMethod::ClientCertificate,
            success: false,
            error_message: Some("Client certificate authentication not implemented".to_string()),
            requires_mfa: false,
        })
    }

    /// Convert enterprise user context to unified context
    fn convert_enterprise_user_to_unified(
        &self,
        enterprise_user: EnterpriseUserContext,
    ) -> UnifiedUserContext {
        // Determine SSO provider from provider_context
        let provider_name = match &enterprise_user.provider_context {
            crate::auth::sso::types::ProviderUserContext::AWS { .. } => "aws_iam",
            crate::auth::sso::types::ProviderUserContext::Azure { .. } => "azure_ad",
            crate::auth::sso::types::ProviderUserContext::Generic { .. } => "generic",
        };

        UnifiedUserContext {
            user_id: enterprise_user.user_id,
            tenant_id: Some(enterprise_user.tenant_id),
            roles: enterprise_user.roles,
            effective_permissions: HashSet::new(), // Will be populated by RBAC manager
            auth_method: AuthMethod::SSO {
                provider: provider_name.to_string(),
            },
            session_id: enterprise_user.session_id,
            expires_at: None, // SSO tokens handle their own expiration
            created_at: enterprise_user.login_timestamp,
            metadata: HashMap::new(), // No direct metadata on EnterpriseUserContext
        }
    }

    /// Convert JWT claims to unified context
    fn convert_jwt_claims_to_unified(
        &self,
        claims: crate::network::auth::Claims,
    ) -> UnifiedUserContext {
        UnifiedUserContext {
            user_id: claims.sub,
            tenant_id: claims.tenant_id,
            roles: claims.roles,
            effective_permissions: HashSet::new(), // Will be populated by RBAC manager
            auth_method: AuthMethod::JWT,
            session_id: claims.jti,
            expires_at: Some(DateTime::from_timestamp(claims.exp, 0).unwrap_or_else(|| Utc::now())),
            created_at: DateTime::from_timestamp(claims.iat, 0).unwrap_or_else(|| Utc::now()),
            metadata: HashMap::new(),
        }
    }

    /// Convert API key info to unified context
    fn convert_api_key_to_unified(&self, api_key_info: ApiKeyInfo) -> UnifiedUserContext {
        // Convert string permissions to UnifiedPermission enum
        let permissions = api_key_info
            .permissions
            .iter()
            .filter_map(|p| self.parse_permission_string(p))
            .collect();

        UnifiedUserContext {
            user_id: api_key_info.user_id,
            tenant_id: api_key_info.tenant_id,
            roles: vec!["api_user".to_string()], // Default role for API key users
            effective_permissions: permissions,
            auth_method: AuthMethod::ApiKey,
            session_id: format!("apikey_{}", uuid::Uuid::new_v4()),
            expires_at: api_key_info.expires_at,
            created_at: api_key_info.created_at.unwrap_or_else(|| Utc::now()),
            metadata: HashMap::new(),
        }
    }

    /// Parse permission string to UnifiedPermission enum
    fn parse_permission_string(&self, permission_str: &str) -> Option<UnifiedPermission> {
        match permission_str {
            "read" => Some(UnifiedPermission::TenantRead),
            "write" => Some(UnifiedPermission::TenantWrite),
            "admin" => Some(UnifiedPermission::TenantAdmin),
            "collection_create" => Some(UnifiedPermission::CollectionCreate),
            "system_admin" => Some(UnifiedPermission::SystemAdmin),
            _ => {
                warn!("Unknown permission string: {}", permission_str);
                None
            }
        }
    }

    /// Generate JWT token pair
    pub async fn generate_token_pair(
        &self,
        user_context: &UnifiedUserContext,
    ) -> Result<TokenPair> {
        match &self.jwt_service {
            Some(jwt_service) => jwt_service
                .generate_token_pair(
                    &user_context.user_id,
                    user_context.tenant_id.clone(),
                    user_context.roles.clone(),
                )
                .await
                .map_err(|e| anyhow!(e)),
            None => Err(anyhow!("JWT service not configured")),
        }
    }

    /// Refresh JWT token
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenPair> {
        match &self.jwt_service {
            Some(jwt_service) => jwt_service
                .refresh_token(refresh_token, None)
                .await
                .map_err(|e| anyhow!(e)),
            None => Err(anyhow!("JWT service not configured")),
        }
    }

    /// Validate if authentication method is enabled
    pub fn is_method_enabled(&self, method: &AuthenticationMethod) -> bool {
        self.config.methods.contains(method)
    }
}

/// Authentication data from request
#[derive(Debug)]
pub enum AuthenticationData {
    SSOToken(SSOToken),
    JWTToken(String),
    ApiKey(String),
    ClientCertificate(ClientCertificateData),
}

/// Client certificate data for mTLS
#[derive(Debug)]
pub struct ClientCertificateData {
    pub subject: String,
    pub issuer: String,
    pub serial_number: String,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
}

impl UnifiedUserContext {
    /// Create anonymous user context
    pub fn anonymous() -> Self {
        Self {
            user_id: "anonymous".to_string(),
            tenant_id: None,
            roles: vec!["anonymous".to_string()],
            effective_permissions: HashSet::new(),
            auth_method: AuthMethod::Internal,
            session_id: format!("anon_{}", uuid::Uuid::new_v4()),
            expires_at: None,
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Check if user is authenticated
    pub fn is_authenticated(&self) -> bool {
        self.user_id != "anonymous"
    }

    /// Check if user session is expired
    pub fn is_session_expired(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => Utc::now() > expires_at,
            None => false,
        }
    }

    /// Get user display name
    pub fn display_name(&self) -> String {
        self.metadata
            .get("display_name")
            .unwrap_or(&self.user_id)
            .clone()
    }
}

/// Create audit event for authentication attempt
fn create_auth_audit_event(
    auth_data: &AuthenticationData,
    result: &AuthenticationResult,
    _start_time: DateTime<Utc>,
) -> AuditEvent {
    let auth_method_name = match auth_data {
        AuthenticationData::SSOToken(_) => "sso",
        AuthenticationData::JWTToken(_) => "jwt",
        AuthenticationData::ApiKey(_) => "api_key",
        AuthenticationData::ClientCertificate(_) => "client_certificate",
    };

    AuditEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        event_type: AuditEventType::Authentication,
        timestamp: Utc::now(),
        user_id: Some(result.user_context.user_id.clone()),
        tenant_id: result.user_context.tenant_id.clone(),
        resource: AuditResource {
            resource_type: "authentication".to_string(),
            resource_id: result.user_context.session_id.clone(),
            parent_resource: None,
        },
        action: "authenticate".to_string(),
        result: if result.success {
            AuditResult::Success
        } else {
            AuditResult::Failure {
                error_code: "AUTH_FAILED".to_string(),
                error_message: result
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Authentication failed".to_string()),
            }
        },
        ip_address: None, // Would be populated by middleware
        user_agent: None,
        session_id: Some(result.user_context.session_id.clone()),
        request_id: None,
        details: {
            let mut details = HashMap::new();
            details.insert(
                "auth_method".to_string(),
                serde_json::json!(auth_method_name),
            );
            details.insert("success".to_string(), serde_json::json!(result.success));
            if let Some(err_msg) = &result.error_message {
                details.insert("error_message".to_string(), serde_json::json!(err_msg));
            }
            details.insert(
                "requires_mfa".to_string(),
                serde_json::json!(result.requires_mfa),
            );
            details.insert(
                "roles".to_string(),
                serde_json::json!(result.user_context.roles),
            );
            details
        },
        risk_score: if result.success { Some(0.0) } else { Some(0.5) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_unified_auth_service_creation() {
        let config = AuthenticationConfig {
            enabled: true,
            methods: vec![AuthenticationMethod::ApiKey],
            require_authentication: false,
            default_session_timeout_minutes: 480,
            api_keys: HashMap::new(),
            jwt: JwtConfig {
                enabled: false,
                secret: "test-secret".to_string(),
                access_token_expiration_minutes: 15,
                refresh_token_expiration_days: 7,
                issuer: "test".to_string(),
                audience: "test".to_string(),
                algorithm: "HS256".to_string(),
            },
            sso: SSOConfig {
                enabled: false,
                providers: vec![],
                token_cache_ttl_minutes: 5,
                aws_iam: None,
                azure_ad: None,
            },
        };

        let auth_service = UnifiedAuthService::new(config);
        assert!(auth_service.is_ok());
    }

    #[tokio::test]
    async fn test_anonymous_user_context() {
        let anonymous = UnifiedUserContext::anonymous();
        assert_eq!(anonymous.user_id, "anonymous");
        assert!(!anonymous.is_authenticated());
        assert!(!anonymous.is_session_expired());
    }

    #[tokio::test]
    async fn test_permission_string_parsing() {
        let config = AuthenticationConfig {
            enabled: true,
            methods: vec![AuthenticationMethod::ApiKey],
            require_authentication: false,
            default_session_timeout_minutes: 480,
            api_keys: HashMap::new(),
            jwt: JwtConfig {
                enabled: false,
                secret: "test".to_string(),
                access_token_expiration_minutes: 15,
                refresh_token_expiration_days: 7,
                issuer: "test".to_string(),
                audience: "test".to_string(),
                algorithm: "HS256".to_string(),
            },
            sso: SSOConfig {
                enabled: false,
                providers: vec![],
                token_cache_ttl_minutes: 5,
                aws_iam: None,
                azure_ad: None,
            },
        };

        let auth_service = UnifiedAuthService::new(config).unwrap();

        let read_perm = auth_service.parse_permission_string("read");
        assert_eq!(read_perm, Some(UnifiedPermission::TenantRead));

        let admin_perm = auth_service.parse_permission_string("admin");
        assert_eq!(admin_perm, Some(UnifiedPermission::TenantAdmin));

        let unknown_perm = auth_service.parse_permission_string("unknown");
        assert_eq!(unknown_perm, None);
    }
}
