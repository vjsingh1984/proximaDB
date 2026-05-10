//! Unified Authentication Service for ProximaDB
//!
//! Consolidates authentication logic from multiple sources:
//! - EnterpriseAuthManager (src/auth/mod.rs)
//! - Network Auth Service (src/network/auth/mod.rs)
//! - Auth Middleware (src/network/middleware/auth.rs)

use super::unified_rbac::{UnifiedAuthMethod, UnifiedPermission, UnifiedUserContext};
use crate::audit::logger::AuditLogger;
use crate::auth::{EnterpriseAuthManager, EnterpriseUserContext, SSOToken};
use crate::network::auth::{JwtService, TokenPair};

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use proximadb_security::{AuditEvent, AuditEventType, AuditResource, AuditResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{info, warn};
use x509_parser::prelude::*;

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
    /// mTLS configuration for client certificate authentication
    #[serde(default)]
    pub mtls: MtlsConfig,
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

/// mTLS configuration for client certificate authentication
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MtlsConfig {
    /// Whether mTLS authentication is enabled
    pub enabled: bool,
    /// Path to the CA certificate used to verify client certificates
    pub ca_cert_path: Option<String>,
    /// Whether client certificates are required (vs optional)
    pub require_client_cert: bool,
    /// Maps Common Name patterns to RBAC role names.
    /// Patterns support prefix matching with a trailing '*' wildcard.
    /// Example: {"*.admin" => "admin", "service-*" => "service_role"}
    #[serde(default)]
    pub cn_role_mapping: HashMap<String, String>,
}

/// Validated client identity extracted from an X.509 certificate
#[derive(Debug, Clone)]
pub struct ClientIdentity {
    /// The Common Name (CN) from the certificate subject
    pub common_name: String,
    /// Subject Alternative Name entries (DNS names, emails, URIs, IP addresses)
    pub san_entries: Vec<String>,
    /// RBAC roles resolved from the CN via cn_role_mapping
    pub roles: Vec<String>,
}

/// Authentication result from unified service
#[derive(Debug, Clone)]
pub struct AuthenticationResult {
    pub user_context: UnifiedUserContext,
    pub auth_method: UnifiedAuthMethod,
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

    /// Cached CA certificate DER bytes for mTLS validation
    ca_cert_der: Option<Vec<u8>>,
}

impl std::fmt::Debug for UnifiedAuthService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedAuthService")
            .field("has_enterprise_auth", &self.enterprise_auth.is_some())
            .field("has_jwt_service", &self.jwt_service.is_some())
            .field("api_key_count", &self.api_keys.len())
            .field("has_ca_cert", &self.ca_cert_der.is_some())
            .finish()
    }
}

impl UnifiedAuthService {
    /// Create new unified authentication service
    pub fn new(config: AuthenticationConfig) -> Result<Self> {
        // Load CA certificate if mTLS is configured
        let ca_cert_der = if config.mtls.enabled {
            if let Some(ref ca_path) = config.mtls.ca_cert_path {
                Some(Self::load_ca_certificate(ca_path)?)
            } else {
                return Err(anyhow!("mTLS is enabled but no ca_cert_path is configured"));
            }
        } else {
            None
        };

        let mut service = Self {
            enterprise_auth: None,
            jwt_service: None,
            api_keys: Arc::new(DashMap::new()),
            config: config.clone(),
            audit_logger: None,
            ca_cert_der,
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
                        auth_method: UnifiedAuthMethod::Internal,
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
                auth_method: UnifiedAuthMethod::SSO {
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
                            auth_method: UnifiedAuthMethod::SSO {
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
                            auth_method: UnifiedAuthMethod::SSO {
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
                auth_method: UnifiedAuthMethod::JWT,
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
                        auth_method: UnifiedAuthMethod::JWT,
                        success: true,
                        error_message: None,
                        requires_mfa: false,
                    })
                }
                Err(e) => {
                    warn!("JWT authentication failed: {}", e);
                    Ok(AuthenticationResult {
                        user_context: UnifiedUserContext::anonymous(),
                        auth_method: UnifiedAuthMethod::JWT,
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
                auth_method: UnifiedAuthMethod::ApiKey,
                success: false,
                error_message: Some("API key authentication disabled".to_string()),
                requires_mfa: false,
            });
        }

        match self.api_keys.get(api_key) {
            Some(api_key_info) => {
                // Check if API key has expired
                if let Some(expires_at) = api_key_info.expires_at
                    && Utc::now() > expires_at
                {
                    return Ok(AuthenticationResult {
                        user_context: UnifiedUserContext::anonymous(),
                        auth_method: UnifiedAuthMethod::ApiKey,
                        success: false,
                        error_message: Some("API key expired".to_string()),
                        requires_mfa: false,
                    });
                }

                let user_context = self.convert_api_key_to_unified(api_key_info.clone());
                Ok(AuthenticationResult {
                    user_context,
                    auth_method: UnifiedAuthMethod::ApiKey,
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
                    auth_method: UnifiedAuthMethod::ApiKey,
                    success: false,
                    error_message: Some("Invalid API key".to_string()),
                    requires_mfa: false,
                })
            }
        }
    }

    /// Authenticate client certificate via mTLS validation.
    ///
    /// When raw DER bytes are present in `cert_data`, the certificate is fully
    /// parsed and validated (expiry, CA signature, CN/SAN extraction, role mapping).
    /// Without raw bytes the method falls back to the pre-populated metadata fields
    /// on `ClientCertificateData`.
    async fn authenticate_client_certificate(
        &self,
        cert_data: &ClientCertificateData,
    ) -> Result<AuthenticationResult> {
        if !self
            .config
            .methods
            .contains(&AuthenticationMethod::ClientCertificate)
        {
            return Ok(AuthenticationResult {
                user_context: UnifiedUserContext::anonymous(),
                auth_method: UnifiedAuthMethod::ClientCertificate,
                success: false,
                error_message: Some("Client certificate authentication disabled".to_string()),
                requires_mfa: false,
            });
        }

        if !self.config.mtls.enabled {
            return Ok(AuthenticationResult {
                user_context: UnifiedUserContext::anonymous(),
                auth_method: UnifiedAuthMethod::ClientCertificate,
                success: false,
                error_message: Some("mTLS is not enabled in configuration".to_string()),
                requires_mfa: false,
            });
        }

        // Validate the certificate and extract identity
        let identity = match &cert_data.raw_cert_der {
            Some(der_bytes) => match self.validate_client_certificate(der_bytes) {
                Ok(id) => id,
                Err(e) => {
                    warn!("mTLS certificate validation failed: {}", e);
                    return Ok(AuthenticationResult {
                        user_context: UnifiedUserContext::anonymous(),
                        auth_method: UnifiedAuthMethod::ClientCertificate,
                        success: false,
                        error_message: Some(format!("Certificate validation failed: {}", e)),
                        requires_mfa: false,
                    });
                }
            },
            None => {
                // Fallback: use pre-populated metadata when raw bytes are unavailable.
                // Only basic expiry checking is possible in this path.
                let now = Utc::now();
                if now < cert_data.not_before || now > cert_data.not_after {
                    return Ok(AuthenticationResult {
                        user_context: UnifiedUserContext::anonymous(),
                        auth_method: UnifiedAuthMethod::ClientCertificate,
                        success: false,
                        error_message: Some(
                            "Client certificate has expired or is not yet valid".to_string(),
                        ),
                        requires_mfa: false,
                    });
                }

                let roles = self.resolve_roles_for_cn(&cert_data.subject);
                ClientIdentity {
                    common_name: cert_data.subject.clone(),
                    san_entries: Vec::new(),
                    roles,
                }
            }
        };

        if identity.roles.is_empty() {
            warn!(
                "No roles mapped for client certificate CN={}",
                identity.common_name
            );
            return Ok(AuthenticationResult {
                user_context: UnifiedUserContext::anonymous(),
                auth_method: UnifiedAuthMethod::ClientCertificate,
                success: false,
                error_message: Some(format!(
                    "No roles mapped for certificate CN={}",
                    identity.common_name
                )),
                requires_mfa: false,
            });
        }

        info!(
            "mTLS authentication succeeded for CN={}, roles={:?}",
            identity.common_name, identity.roles
        );

        let user_context = UnifiedUserContext {
            user_id: identity.common_name.clone(),
            tenant_id: None,
            roles: identity.roles,
            effective_permissions: HashSet::new(), // Populated by RBAC manager
            auth_method: UnifiedAuthMethod::ClientCertificate,
            session_id: format!("mtls_{}", uuid::Uuid::new_v4()),
            expires_at: None, // Certificate expiry is validated at authentication time
            created_at: Utc::now(),
            metadata: {
                let mut m = HashMap::new();
                m.insert("auth_cn".to_string(), identity.common_name);
                if !identity.san_entries.is_empty() {
                    m.insert("auth_sans".to_string(), identity.san_entries.join(","));
                }
                m
            },
        };

        Ok(AuthenticationResult {
            user_context,
            auth_method: UnifiedAuthMethod::ClientCertificate,
            success: true,
            error_message: None,
            requires_mfa: false,
        })
    }

    /// Load a CA certificate from a PEM or DER file and return its DER bytes.
    fn load_ca_certificate(path: &str) -> Result<Vec<u8>> {
        let file_bytes = std::fs::read(path)
            .map_err(|e| anyhow!("Failed to read CA certificate at '{}': {}", path, e))?;

        // Try PEM first
        if let Ok(pem_contents) = std::str::from_utf8(&file_bytes) {
            let mut reader = std::io::BufReader::new(pem_contents.as_bytes());
            let certs = rustls_pemfile::certs(&mut reader)
                .map_err(|e| anyhow!("Failed to parse PEM certificates: {}", e))?;

            if let Some(first_cert) = certs.into_iter().next() {
                return Ok(first_cert);
            }
        }

        // Fallback: treat as raw DER
        // Validate it parses as an X.509 certificate
        X509Certificate::from_der(&file_bytes)
            .map_err(|e| anyhow!("CA file is neither valid PEM nor DER: {:?}", e))?;
        Ok(file_bytes)
    }

    /// Validate a DER-encoded client certificate against the configured CA,
    /// check expiry, extract CN and SANs, and resolve RBAC roles.
    pub fn validate_client_certificate(&self, cert_der: &[u8]) -> Result<ClientIdentity> {
        let ca_der = self
            .ca_cert_der
            .as_ref()
            .ok_or_else(|| anyhow!("No CA certificate loaded for mTLS validation"))?;

        // Parse the CA certificate
        let (_, ca_cert) = X509Certificate::from_der(ca_der)
            .map_err(|e| anyhow!("Failed to parse CA certificate: {:?}", e))?;

        // Parse the client certificate
        let (_, client_cert) = X509Certificate::from_der(cert_der)
            .map_err(|e| anyhow!("Failed to parse client certificate: {:?}", e))?;

        // 1. Verify the client certificate is within its validity period
        let now = ASN1Time::now();
        if !client_cert.validity().is_valid_at(now) {
            let not_before = client_cert.validity().not_before;
            let not_after = client_cert.validity().not_after;
            return Err(anyhow!(
                "Client certificate is not valid at current time (valid {} to {})",
                not_before,
                not_after
            ));
        }

        // 2. Verify the client certificate was issued by the CA.
        //    Compare the client cert's issuer DN with the CA cert's subject DN.
        //    Additionally verify that the Authority Key Identifier (if present)
        //    matches the CA's Subject Key Identifier.
        if client_cert.issuer() != ca_cert.subject() {
            return Err(anyhow!(
                "Certificate issuer does not match the configured CA subject (issuer={}, ca_subject={})",
                client_cert.issuer(),
                ca_cert.subject()
            ));
        }

        // Cross-check key identifiers when both extensions are present
        // x509_parser exposes extensions via the parsed extensions map
        let client_aki = client_cert
            .tbs_certificate
            .extensions_map()
            .ok()
            .and_then(|map| {
                map.get(&x509_parser::oid_registry::OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER)
                    .cloned()
            })
            .and_then(|ext| {
                x509_parser::extensions::AuthorityKeyIdentifier::from_der(ext.value)
                    .ok()
                    .and_then(|(_, aki)| aki.key_identifier.map(|ki| ki.0.to_vec()))
            });
        let ca_ski = ca_cert
            .tbs_certificate
            .extensions_map()
            .ok()
            .and_then(|map| {
                map.get(&x509_parser::oid_registry::OID_X509_EXT_SUBJECT_KEY_IDENTIFIER)
                    .cloned()
            })
            .map(|ext| ext.value.to_vec());

        if let (Some(aki), Some(ski)) = (&client_aki, &ca_ski)
            && aki != ski
        {
            return Err(anyhow!(
                "Certificate Authority Key Identifier does not match CA Subject Key Identifier"
            ));
        }

        // 3. Extract Common Name from the subject
        let common_name = client_cert
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Client certificate has no Common Name (CN) in subject"))?;

        // 4. Extract Subject Alternative Names
        let san_entries = Self::extract_san_entries(&client_cert);

        // 5. Resolve RBAC roles from CN
        let roles = self.resolve_roles_for_cn(&common_name);

        Ok(ClientIdentity {
            common_name,
            san_entries,
            roles,
        })
    }

    /// Extract Subject Alternative Name entries from an X.509 certificate.
    fn extract_san_entries(cert: &X509Certificate<'_>) -> Vec<String> {
        let mut entries = Vec::new();
        if let Ok(Some(san_ext)) = cert.subject_alternative_name() {
            for name in &san_ext.value.general_names {
                match name {
                    GeneralName::DNSName(dns) => {
                        entries.push(format!("DNS:{}", dns));
                    }
                    GeneralName::RFC822Name(email) => {
                        entries.push(format!("email:{}", email));
                    }
                    GeneralName::URI(uri) => {
                        entries.push(format!("URI:{}", uri));
                    }
                    GeneralName::IPAddress(ip_bytes) => {
                        let ip_str = if ip_bytes.len() == 4 {
                            format!(
                                "IP:{}.{}.{}.{}",
                                ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]
                            )
                        } else {
                            format!("IP:{}", hex::encode(ip_bytes))
                        };
                        entries.push(ip_str);
                    }
                    _ => {
                        entries.push(format!("other:{:?}", name));
                    }
                }
            }
        }
        entries
    }

    /// Resolve RBAC roles for a given Common Name using the configured cn_role_mapping.
    ///
    /// Supports exact matches and simple wildcard patterns:
    /// - Trailing wildcard: "service-*" matches "service-auth", "service-gateway"
    /// - Leading wildcard: "*.admin" matches "db.admin", "cluster.admin"
    /// - Exact match: "root" matches only "root"
    fn resolve_roles_for_cn(&self, cn: &str) -> Vec<String> {
        let mut roles = Vec::new();
        for (pattern, role) in &self.config.mtls.cn_role_mapping {
            let matched = if let Some(suffix) = pattern.strip_prefix('*') {
                // Leading wildcard: "*.admin" matches anything ending with ".admin"
                cn.ends_with(suffix)
            } else if pattern.ends_with('*') {
                // Trailing wildcard: "service-*" matches anything starting with "service-"
                let prefix = &pattern[..pattern.len() - 1];
                cn.starts_with(prefix)
            } else {
                // Exact match
                cn == pattern
            };

            if matched && !roles.contains(role) {
                roles.push(role.clone());
            }
        }
        roles
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
            auth_method: UnifiedAuthMethod::SSO {
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
            auth_method: UnifiedAuthMethod::JWT,
            session_id: claims.jti,
            expires_at: Some(DateTime::from_timestamp(claims.exp, 0).unwrap_or_else(Utc::now)),
            created_at: DateTime::from_timestamp(claims.iat, 0).unwrap_or_else(Utc::now),
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
            auth_method: UnifiedAuthMethod::ApiKey,
            session_id: format!("apikey_{}", uuid::Uuid::new_v4()),
            expires_at: api_key_info.expires_at,
            created_at: api_key_info.created_at.unwrap_or_else(Utc::now),
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
#[derive(Debug, Clone)]
pub struct ClientCertificateData {
    pub subject: String,
    pub issuer: String,
    pub serial_number: String,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
    /// Raw DER-encoded client certificate bytes for full validation.
    /// When provided, the mTLS validator will parse and verify the certificate
    /// against the configured CA, extract CN/SANs, and map to roles.
    pub raw_cert_der: Option<Vec<u8>>,
}

impl UnifiedUserContext {
    /// Create anonymous user context
    pub fn anonymous() -> Self {
        Self {
            user_id: "anonymous".to_string(),
            tenant_id: None,
            roles: vec!["anonymous".to_string()],
            effective_permissions: HashSet::new(),
            auth_method: UnifiedAuthMethod::Internal,
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
            mtls: MtlsConfig::default(),
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
            mtls: MtlsConfig::default(),
        };

        let auth_service = UnifiedAuthService::new(config).unwrap();

        let read_perm = auth_service.parse_permission_string("read");
        assert_eq!(read_perm, Some(UnifiedPermission::TenantRead));

        let admin_perm = auth_service.parse_permission_string("admin");
        assert_eq!(admin_perm, Some(UnifiedPermission::TenantAdmin));

        let unknown_perm = auth_service.parse_permission_string("unknown");
        assert_eq!(unknown_perm, None);
    }

    #[test]
    fn test_mtls_config_default() {
        let config = MtlsConfig::default();
        assert!(!config.enabled);
        assert!(config.ca_cert_path.is_none());
        assert!(!config.require_client_cert);
        assert!(config.cn_role_mapping.is_empty());
    }

    #[test]
    fn test_resolve_roles_exact_match() {
        let mut cn_role_mapping = HashMap::new();
        cn_role_mapping.insert("db-admin".to_string(), "admin".to_string());
        cn_role_mapping.insert("service-gateway".to_string(), "gateway".to_string());

        let config = AuthenticationConfig {
            enabled: true,
            methods: vec![AuthenticationMethod::ClientCertificate],
            require_authentication: true,
            default_session_timeout_minutes: 60,
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
            mtls: MtlsConfig {
                enabled: false,
                ca_cert_path: None,
                require_client_cert: false,
                cn_role_mapping,
            },
        };

        let service = UnifiedAuthService::new(config).unwrap_or_else(|_| {
            // Service creation does not fail when mtls.enabled is false
            unreachable!()
        });

        let roles = service.resolve_roles_for_cn("db-admin");
        assert_eq!(roles, vec!["admin".to_string()]);

        let roles = service.resolve_roles_for_cn("unknown-service");
        assert!(roles.is_empty());
    }

    #[test]
    fn test_resolve_roles_wildcard_prefix() {
        let mut cn_role_mapping = HashMap::new();
        cn_role_mapping.insert("service-*".to_string(), "service_role".to_string());

        let config = AuthenticationConfig {
            enabled: true,
            methods: vec![AuthenticationMethod::ClientCertificate],
            require_authentication: true,
            default_session_timeout_minutes: 60,
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
            mtls: MtlsConfig {
                enabled: false,
                ca_cert_path: None,
                require_client_cert: false,
                cn_role_mapping,
            },
        };

        let service = UnifiedAuthService::new(config).unwrap_or_else(|_| unreachable!());

        let roles = service.resolve_roles_for_cn("service-auth");
        assert_eq!(roles, vec!["service_role".to_string()]);

        let roles = service.resolve_roles_for_cn("service-gateway");
        assert_eq!(roles, vec!["service_role".to_string()]);

        // Does not match without the prefix
        let roles = service.resolve_roles_for_cn("other-auth");
        assert!(roles.is_empty());
    }

    #[test]
    fn test_resolve_roles_wildcard_suffix() {
        let mut cn_role_mapping = HashMap::new();
        cn_role_mapping.insert("*.admin".to_string(), "admin".to_string());

        let config = AuthenticationConfig {
            enabled: true,
            methods: vec![AuthenticationMethod::ClientCertificate],
            require_authentication: true,
            default_session_timeout_minutes: 60,
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
            mtls: MtlsConfig {
                enabled: false,
                ca_cert_path: None,
                require_client_cert: false,
                cn_role_mapping,
            },
        };

        let service = UnifiedAuthService::new(config).unwrap_or_else(|_| unreachable!());

        let roles = service.resolve_roles_for_cn("cluster.admin");
        assert_eq!(roles, vec!["admin".to_string()]);

        let roles = service.resolve_roles_for_cn("db.admin");
        assert_eq!(roles, vec!["admin".to_string()]);

        // Does not match without the suffix
        let roles = service.resolve_roles_for_cn("cluster.reader");
        assert!(roles.is_empty());
    }

    #[test]
    fn test_mtls_enabled_without_ca_path_fails() {
        let config = AuthenticationConfig {
            enabled: true,
            methods: vec![AuthenticationMethod::ClientCertificate],
            require_authentication: true,
            default_session_timeout_minutes: 60,
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
            mtls: MtlsConfig {
                enabled: true,
                ca_cert_path: None, // Missing CA path
                require_client_cert: true,
                cn_role_mapping: HashMap::new(),
            },
        };

        let result = UnifiedAuthService::new(config);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("ca_cert_path"));
    }

    #[tokio::test]
    async fn test_mtls_disabled_returns_failure() {
        let config = AuthenticationConfig {
            enabled: true,
            methods: vec![AuthenticationMethod::ClientCertificate],
            require_authentication: true,
            default_session_timeout_minutes: 60,
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
            mtls: MtlsConfig {
                enabled: false,
                ca_cert_path: None,
                require_client_cert: false,
                cn_role_mapping: HashMap::new(),
            },
        };

        let service = UnifiedAuthService::new(config).unwrap_or_else(|_| unreachable!());
        let cert_data = ClientCertificateData {
            subject: "test-service".to_string(),
            issuer: "test-ca".to_string(),
            serial_number: "1234".to_string(),
            not_before: Utc::now() - chrono::Duration::hours(1),
            not_after: Utc::now() + chrono::Duration::hours(1),
            raw_cert_der: None,
        };

        let result = service
            .authenticate_client_certificate(&cert_data)
            .await
            .unwrap_or_else(|_| unreachable!());
        assert!(!result.success);
        assert!(
            result
                .error_message
                .as_deref()
                .unwrap_or("")
                .contains("not enabled")
        );
    }
}
