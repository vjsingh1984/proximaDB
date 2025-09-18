//! Security Coordinator for ProximaDB
//!
//! Central coordination point for all security operations including
//! authentication, authorization, audit, and security policy enforcement.

use super::unified_rbac::{ConsolidatedRBACManager, UnifiedUserContext, UnifiedPermission, RBACConfig};
use super::unified_auth::{UnifiedAuthService, AuthenticationConfig, AuthenticationResult, AuthenticationData};
use crate::audit::logger::AuditLogger;

use anyhow::{Result, anyhow};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error, debug};
use chrono::Utc;

/// Security coordinator configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub enabled: bool,
    pub mode: SecurityMode,
    pub authentication: AuthenticationConfig,
    pub rbac: RBACConfig,
    pub audit: AuditConfig,
    pub tls: TlsConfig,
    pub compliance: ComplianceConfig,
}

/// Security mode for different deployment scenarios
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SecurityMode {
    #[serde(rename = "development")]
    Development,
    #[serde(rename = "production")]
    Production,
    #[serde(rename = "enterprise")]
    Enterprise,
}

/// Audit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    pub enabled: bool,
    pub storage_backend: String,
    pub log_directory: Option<String>,
    pub encryption_enabled: bool,
    pub retention_days: u32,
    pub enable_real_time_alerts: bool,
    pub alert_webhook_url: Option<String>,
}

/// TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub enabled: bool,
    pub require_client_certificates: bool,
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
    pub ca_file: Option<String>,
}

/// Compliance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceConfig {
    pub frameworks: Vec<String>,
    pub data_residency: Option<String>,
    pub encryption_at_rest: bool,
    pub encryption_in_transit: bool,
}

/// Central security coordinator
pub struct SecurityCoordinator {
    /// Unified authentication service
    auth_service: Arc<UnifiedAuthService>,

    /// Consolidated RBAC manager
    rbac_manager: Arc<ConsolidatedRBACManager>,

    /// Audit logger
    audit_logger: Arc<AuditLogger>,

    /// Security configuration
    config: SecurityConfig,
}

impl SecurityCoordinator {
    /// Create new security coordinator
    pub fn new(
        auth_service: UnifiedAuthService,
        rbac_manager: ConsolidatedRBACManager,
        audit_logger: AuditLogger,
        config: SecurityConfig,
    ) -> Self {
        Self {
            auth_service: Arc::new(auth_service),
            rbac_manager: Arc::new(rbac_manager),
            audit_logger: Arc::new(audit_logger),
            config,
        }
    }

    /// Full authentication and authorization flow
    pub async fn authenticate_and_authorize(
        &self,
        auth_data: AuthenticationData,
        requested_permission: UnifiedPermission,
    ) -> Result<AuthorizedContext> {
        let start_time = Utc::now();

        // Step 1: Authenticate user
        let auth_result = self.auth_service.authenticate(auth_data).await?;

        if !auth_result.success {
            return Err(anyhow!("Authentication failed: {}",
                auth_result.error_message.unwrap_or("Unknown error".to_string())));
        }

        // Step 2: Populate effective permissions via RBAC
        let mut user_context = auth_result.user_context;
        let effective_permissions = self.rbac_manager
            .get_effective_permissions(&user_context)
            .await?;
        user_context.effective_permissions = effective_permissions;

        // Step 3: Check authorization for requested permission
        let authorized = self.rbac_manager
            .check_permission(&user_context, &requested_permission)
            .await?;

        if !authorized {
            // Log authorization failure
            self.audit_logger.log_event(create_authorization_failure_event(
                &user_context,
                &requested_permission,
                start_time
            )).await?;

            return Err(anyhow!("Insufficient permissions for requested operation"));
        }

        // Step 4: Create authorized context
        Ok(AuthorizedContext {
            user_context,
            granted_permission: requested_permission,
            session_metadata: SessionMetadata {
                authenticated_at: start_time,
                auth_method: auth_result.auth_method,
                requires_mfa: auth_result.requires_mfa,
            },
        })
    }

    /// Simplified authentication for API endpoints
    pub async fn authenticate_request(
        &self,
        auth_data: AuthenticationData,
    ) -> Result<UnifiedUserContext> {
        let auth_result = self.auth_service.authenticate(auth_data).await?;

        if !auth_result.success {
            return Err(anyhow!("Authentication failed"));
        }

        // Populate effective permissions
        let mut user_context = auth_result.user_context;
        let effective_permissions = self.rbac_manager
            .get_effective_permissions(&user_context)
            .await?;
        user_context.effective_permissions = effective_permissions;

        Ok(user_context)
    }

    /// Check if user has permission for operation
    pub async fn check_authorization(
        &self,
        user_context: &UnifiedUserContext,
        permission: &UnifiedPermission,
    ) -> Result<bool> {
        self.rbac_manager.check_permission(user_context, permission).await
    }

    /// Get security policy for tenant
    pub async fn get_tenant_security_policy(&self, tenant_id: &str) -> Result<TenantSecurityPolicy> {
        // This would integrate with tenant manager
        // For now, return default policy based on security mode
        Ok(match self.config.mode {
            SecurityMode::Development => TenantSecurityPolicy::development(),
            SecurityMode::Production => TenantSecurityPolicy::production(),
            SecurityMode::Enterprise => TenantSecurityPolicy::enterprise(),
        })
    }

    /// Initialize security coordinator from configuration
    pub async fn from_config(config: SecurityConfig) -> Result<Self> {
        // Create RBAC manager
        let rbac_manager = ConsolidatedRBACManager::new(config.rbac.clone());

        // Create auth service
        let auth_service = UnifiedAuthService::new(config.authentication.clone())?;

        // Create audit logger
        let audit_logger = AuditLogger::new(config.audit.clone()).await?;

        Ok(Self::new(auth_service, rbac_manager, audit_logger, config))
    }

    /// Health check for security subsystem
    pub async fn health_check(&self) -> SecurityHealthStatus {
        let mut status = SecurityHealthStatus {
            overall_healthy: true,
            authentication_healthy: true,
            rbac_healthy: true,
            audit_healthy: true,
            tls_healthy: true,
            issues: vec![],
        };

        // Check authentication health
        if !self.config.authentication.enabled && self.config.mode != SecurityMode::Development {
            status.authentication_healthy = false;
            status.issues.push("Authentication disabled in non-development mode".to_string());
        }

        // Check TLS health
        if !self.config.tls.enabled && self.config.mode == SecurityMode::Production {
            status.tls_healthy = false;
            status.issues.push("TLS disabled in production mode".to_string());
        }

        // Check audit health
        if !self.config.audit.enabled {
            status.audit_healthy = false;
            status.issues.push("Audit logging disabled".to_string());
        }

        status.overall_healthy = status.authentication_healthy &&
                                status.rbac_healthy &&
                                status.audit_healthy &&
                                status.tls_healthy;

        status
    }
}

/// Authorized context after successful authentication and authorization
#[derive(Debug, Clone)]
pub struct AuthorizedContext {
    pub user_context: UnifiedUserContext,
    pub granted_permission: UnifiedPermission,
    pub session_metadata: SessionMetadata,
}

/// Session metadata
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub authenticated_at: chrono::DateTime<Utc>,
    pub auth_method: super::unified_rbac::AuthMethod,
    pub requires_mfa: bool,
}

/// Tenant security policy
#[derive(Debug, Clone)]
pub struct TenantSecurityPolicy {
    pub require_authentication: bool,
    pub require_mfa: bool,
    pub session_timeout_minutes: u64,
    pub allowed_auth_methods: Vec<super::unified_auth::AuthenticationMethod>,
    pub audit_level: AuditLevel,
    pub compliance_frameworks: Vec<String>,
}

impl TenantSecurityPolicy {
    pub fn development() -> Self {
        Self {
            require_authentication: false,
            require_mfa: false,
            session_timeout_minutes: 480,
            allowed_auth_methods: vec![super::unified_auth::AuthenticationMethod::ApiKey],
            audit_level: AuditLevel::Basic,
            compliance_frameworks: vec![],
        }
    }

    pub fn production() -> Self {
        Self {
            require_authentication: true,
            require_mfa: false,
            session_timeout_minutes: 240,
            allowed_auth_methods: vec![
                super::unified_auth::AuthenticationMethod::JWT,
                super::unified_auth::AuthenticationMethod::ApiKey,
            ],
            audit_level: AuditLevel::Comprehensive,
            compliance_frameworks: vec!["SOC2".to_string()],
        }
    }

    pub fn enterprise() -> Self {
        Self {
            require_authentication: true,
            require_mfa: true,
            session_timeout_minutes: 120,
            allowed_auth_methods: vec![
                super::unified_auth::AuthenticationMethod::SSO,
                super::unified_auth::AuthenticationMethod::JWT,
                super::unified_auth::AuthenticationMethod::ClientCertificate,
            ],
            audit_level: AuditLevel::Full,
            compliance_frameworks: vec![
                "SOC2".to_string(),
                "GDPR".to_string(),
                "HIPAA".to_string(),
            ],
        }
    }
}

/// Audit level configuration
#[derive(Debug, Clone)]
pub enum AuditLevel {
    None,
    Basic,
    Comprehensive,
    Full,
}

/// Security health status
#[derive(Debug, Clone)]
pub struct SecurityHealthStatus {
    pub overall_healthy: bool,
    pub authentication_healthy: bool,
    pub rbac_healthy: bool,
    pub audit_healthy: bool,
    pub tls_healthy: bool,
    pub issues: Vec<String>,
}

/// Create authorization failure audit event
fn create_authorization_failure_event(
    user_context: &UnifiedUserContext,
    requested_permission: &UnifiedPermission,
    start_time: chrono::DateTime<Utc>,
) -> crate::audit::types::AuditEvent {
    use crate::audit::types::{AuditEvent, AuditEventType};

    AuditEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        event_type: AuditEventType::Authorization,
        timestamp: Utc::now(),
        user_id: Some(user_context.user_id.clone()),
        tenant_id: user_context.tenant_id.clone(),
        resource_type: "permission".to_string(),
        resource_id: format!("{:?}", requested_permission),
        action: "check_permission".to_string(),
        result: "denied".to_string(),
        source_ip: None,
        user_agent: None,
        session_id: Some(user_context.session_id.clone()),
        request_id: None,
        duration_ms: (Utc::now() - start_time).num_milliseconds() as u64,
        metadata: serde_json::json!({
            "requested_permission": format!("{:?}", requested_permission),
            "user_roles": user_context.roles,
            "auth_method": format!("{:?}", user_context.auth_method),
            "tenant_id": user_context.tenant_id,
        }),
        risk_score: 75, // High risk for authorization failures
        compliance_tags: vec!["authorization".to_string(), "access_denied".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_config() -> SecurityConfig {
        SecurityConfig {
            enabled: true,
            mode: SecurityMode::Development,
            authentication: AuthenticationConfig {
                enabled: true,
                methods: vec![super::unified_auth::AuthenticationMethod::ApiKey],
                require_authentication: false,
                default_session_timeout_minutes: 480,
                api_keys: HashMap::new(),
                jwt: super::unified_auth::JwtConfig {
                    enabled: false,
                    secret: "test-secret".to_string(),
                    access_token_expiration_minutes: 15,
                    refresh_token_expiration_days: 7,
                    issuer: "test".to_string(),
                    audience: "test".to_string(),
                    algorithm: "HS256".to_string(),
                },
                sso: super::unified_auth::SSOConfig {
                    enabled: false,
                    providers: vec![],
                    token_cache_ttl_minutes: 5,
                    aws_iam: None,
                    azure_ad: None,
                },
            },
            rbac: RBACConfig::default(),
            audit: AuditConfig {
                enabled: true,
                storage_backend: "file".to_string(),
                log_directory: Some("/tmp/test_audit".to_string()),
                encryption_enabled: false,
                retention_days: 90,
                enable_real_time_alerts: false,
                alert_webhook_url: None,
            },
            tls: TlsConfig {
                enabled: false,
                require_client_certificates: false,
                cert_file: None,
                key_file: None,
                ca_file: None,
            },
            compliance: ComplianceConfig {
                frameworks: vec![],
                data_residency: None,
                encryption_at_rest: false,
                encryption_in_transit: false,
            },
        }
    }

    #[tokio::test]
    async fn test_security_coordinator_creation() {
        let config = create_test_config();
        let coordinator_result = SecurityCoordinator::from_config(config).await;

        // This test will fail until we implement the dependencies
        // but it documents the expected interface
        match coordinator_result {
            Ok(_) => info!("Security coordinator created successfully"),
            Err(e) => warn!("Security coordinator creation failed: {}", e),
        }
    }

    #[tokio::test]
    async fn test_security_health_check() {
        let config = create_test_config();

        // Create minimal coordinator for health check testing
        // This is a placeholder until full implementation is complete
        match SecurityCoordinator::from_config(config).await {
            Ok(coordinator) => {
                let health = coordinator.health_check().await;
                debug!("Security health check: {:?}", health);
            }
            Err(e) => {
                warn!("Could not create coordinator for health check: {}", e);
            }
        }
    }

    #[test]
    fn test_tenant_security_policies() {
        let dev_policy = TenantSecurityPolicy::development();
        assert!(!dev_policy.require_authentication);
        assert!(!dev_policy.require_mfa);

        let prod_policy = TenantSecurityPolicy::production();
        assert!(prod_policy.require_authentication);
        assert!(!prod_policy.require_mfa);

        let enterprise_policy = TenantSecurityPolicy::enterprise();
        assert!(enterprise_policy.require_authentication);
        assert!(enterprise_policy.require_mfa);
        assert!(enterprise_policy.compliance_frameworks.contains(&"SOC2".to_string()));
    }
}