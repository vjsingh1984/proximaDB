//! Unified Security Module for ProximaDB
//!
//! This module consolidates all security-related functionality including
//! authentication, authorization, RBAC, RLS, audit, and security coordination.

pub mod advanced_features;
pub mod encryption;
pub mod rls;
pub mod security_coordinator;
pub mod unified_auth;
pub mod unified_rbac;

pub use unified_rbac::{
    AuthMethod, AuthorizationResult, CollectionPermissionType, ConsolidatedRBACManager, RBACConfig,
    TenantContext, UnifiedPermission, UnifiedRole, UnifiedUserContext,
};

pub use unified_auth::{
    AuthenticationConfig, AuthenticationData, AuthenticationMethod, AuthenticationResult,
    UnifiedAuthService,
};

pub use security_coordinator::{SecurityConfig, SecurityCoordinator, SecurityMode};

pub use advanced_features::{
    IPAccessConfig, IPAccessControlService, IPAccessResult, MFAChallenge, MFAConfig, MFAProvider,
    MFAService, MFAVerificationResult, RateLimitConfig, RateLimitResult, RateLimitingService,
};

pub use rls::{
    CollectionRLS, Operation as RLSOperation, RLSConfig, RLSFilterResult, RLSPolicy,
    RLSPolicyBuilder, SecurityPredicate, SecurityPredicateBuilder,
};

pub use encryption::{
    EncryptedField, EncryptionConfig, EncryptionType, FieldEncryption, FieldEncryptionError,
    KeyInfo, KeyStore, KeyStoreConfig, KeyStoreError,
};

/// Re-export common types for convenience
pub use crate::audit::logger::{AuditConfig, AuditLogger, AuditStorageBackend};
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
    let security_coordinator =
        SecurityCoordinator::new(auth_service, rbac_manager, audit_logger, config);

    Ok(security_coordinator)
}
