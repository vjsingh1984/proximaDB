//! Unified Security Module for ProximaDB
//!
//! This module consolidates all security-related functionality including
//! authentication, authorization, RBAC, audit, and security coordination.

pub mod unified_rbac;
pub mod unified_auth;
pub mod security_coordinator;
pub mod advanced_features;

pub use unified_rbac::{
    ConsolidatedRBACManager, UnifiedPermission, UnifiedRole, UnifiedUserContext,
    AuthMethod, CollectionPermissionType, AuthorizationResult, TenantContext, RBACConfig
};

pub use unified_auth::{
    UnifiedAuthService, AuthenticationResult, AuthenticationMethod, AuthenticationData, AuthenticationConfig
};

pub use security_coordinator::{
    SecurityCoordinator, SecurityConfig, SecurityMode
};

pub use advanced_features::{
    MFAService, RateLimitingService, IPAccessControlService,
    MFAConfig, RateLimitConfig, IPAccessConfig,
    MFAProvider, MFAChallenge, MFAVerificationResult,
    RateLimitResult, IPAccessResult
};

/// Re-export common types for convenience
pub use crate::audit::logger::{AuditLogger, AuditConfig, AuditStorageBackend};
pub use crate::network::auth::{AuthError, JwtConfig};

use anyhow::Result;

/// Initialize security subsystem with configuration
pub async fn initialize_security(config: SecurityConfig) -> Result<SecurityCoordinator> {
    // Create consolidated RBAC manager
    let rbac_config = RBACConfig {
        enabled: config.rbac.enabled,
        enable_field_level_permissions: config.rbac.enable_field_level_permissions,
        enable_audit_logging: config.rbac.enable_audit_logging,
        default_deny: config.rbac.default_deny,
        cache_permissions: true,
        permission_cache_ttl_minutes: 15,
    };

    let rbac_manager = ConsolidatedRBACManager::new(rbac_config);

    // Create unified auth service
    let auth_service = UnifiedAuthService::new(config.authentication.clone())?;

    // Create audit logger - config.audit is now directly AuditConfig from audit::logger
    let audit_logger = AuditLogger::new(config.audit.clone()).await?;

    // Create security coordinator
    let security_coordinator = SecurityCoordinator::new(
        auth_service,
        rbac_manager,
        audit_logger,
        config,
    );

    Ok(security_coordinator)
}