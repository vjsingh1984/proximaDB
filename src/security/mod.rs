//! Unified Security Module for ProximaDB
//!
//! This module consolidates all security-related functionality including
//! authentication, authorization, RBAC, RLS, audit, and security coordination.

pub mod advanced_features;
pub mod auth;
pub mod auth_service;
pub mod encryption;
pub mod monitoring;
pub mod rbac_service;
pub mod request_identity;
pub mod rls;
pub mod security_coordinator;
pub mod tenant_stable_id;
pub mod validation;

pub use rbac_service::{
    AuthMethod, AuthorizationResult, CollectionPermissionType, ConsolidatedRBACManager, RBACConfig,
    TenantContext, UnifiedAuthMethod, UnifiedPermission, UnifiedRole, UnifiedUserContext,
};

pub use auth_service::{
    AuthenticationConfig, AuthenticationData, AuthenticationMethod, AuthenticationResult,
    ClientCertificateData, ClientIdentity, MtlsConfig, SecurityAuthenticationResult,
    UnifiedAuthService,
};

pub use security_coordinator::{
    AuthorizedContext, SecurityConfig, SecurityCoordinator, SecurityMode, SessionMetadata,
};

pub use advanced_features::{
    IPAccessConfig, IPAccessControlService, IPAccessResult, MFAChallenge, MFAConfig, MFAProvider,
    MFAService, MFAVerificationResult, RateLimitConfig, RateLimitResult, RateLimitingService,
};

pub use tenant_stable_id::CatalogTenantStableIdResolver;

pub use rls::{
    CollectionRLS, Operation as RLSOperation, RLSConfig, RLSFilterResult, RLSPolicy,
    RLSPolicyBuilder, SecurityPredicate, SecurityPredicateBuilder,
};

pub use encryption::{
    EncryptedField, EncryptionConfig, EncryptionType, FieldEncryption, FieldEncryptionError,
    KeyInfo, KeyStore, KeyStoreConfig, KeyStoreError,
};

pub use validation::{
    BinaryValidator, CollectionNameValidator, DecimalValidator, FieldValidationConfig,
    JsonValidator, MetadataValidationConfig, MetadataValidator, TimestampValidator,
    TypeValidationResult, TypedValueValidator, UuidValidator, ValidationError,
    contains_sql_injection_pattern, validate_collection_name, validate_record_metadata,
};

pub use monitoring::{
    AlertSeverity, AuthenticationMetrics, AuthorizationMetrics, SecurityAlertConfig,
    SecurityAlertManager, SecurityDashboard, SecurityMetricsCollector, SecurityMetricsSummary,
    SecurityMonitoringConfig, SecurityMonitoringService, ThreatAlert, ThreatAnalysis,
    ThreatDetectionConfig,
};

/// Re-export common types for convenience
pub use crate::audit::logger::AuditLogger;
pub use crate::network::auth::{AuthError, JwtConfig};
pub use proximadb_security::{AuditConfig, AuditStorageBackend};

use anyhow::Result;

/// Initialize security subsystem with configuration
/// Initialize the security subsystem with the ADR-090 identity registry
/// opened at `identity_dir` (canonically `<data_dir>/identity`, beside
/// `<data_dir>/abac`). Registry open failure is FAIL-CLOSED: a security-enabled
/// boot aborts rather than silently running without the durable key store.
pub async fn initialize_security_with_identity(
    config: SecurityConfig,
    identity_dir: Option<std::path::PathBuf>,
) -> Result<SecurityCoordinator> {
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
    let mut auth_service = UnifiedAuthService::new(config.authentication.clone())?;
    if let Some(dir) = identity_dir {
        let registry =
            proximadb_catalog::principal_registry::FileSystemPrincipalRegistry::open(&dir)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "failed to open principal registry at {}: {e}",
                        dir.display()
                    )
                })?;
        auth_service.set_principal_registry(std::sync::Arc::new(registry));
        tracing::info!(dir = %dir.display(), "principal registry attached (ADR-090 L0)");
    }

    // Create audit logger from the shared audit configuration contract.
    //
    // Auditing disabled is MODE-GATED, not blanket-tolerated:
    //  * Development  -> a no-op sink, so `security.enabled = true` boots with
    //    the shipped default `enable_audit_logging = false` instead of failing
    //    hard (today that combination refuses to start, which is a footgun).
    //  * Production   -> keep failing fast. Security without accountability is
    //    a misconfiguration there, and booting anyway would hide it.
    let audit_logger = if config.audit.enable_audit_logging {
        AuditLogger::new(config.audit.clone()).await?
    } else if !matches!(config.mode, SecurityMode::Development) {
        // Written as "not Development" rather than "is Production" on purpose:
        // Enterprise is production-grade too, and any mode added later must
        // fail CLOSED here rather than silently inherit the no-op sink.
        return Err(anyhow::anyhow!(
            "security is enabled in {:?} mode but audit logging is disabled \
             (set security.audit.enable_audit_logging = true)",
            config.mode
        ));
    } else {
        tracing::warn!(
            "audit logging DISABLED - authentication and authorization events will not be \
             recorded, and no security alert can fire. Development mode only."
        );
        AuditLogger::noop(config.audit.clone())
    };

    // ADR-090 / TD-SEC-2: give `set_audit_logger` its first production caller.
    // Until now `auth_service.audit_logger` was always `None`, so the emit hook
    // in `authenticate()` was dead and NO authentication success or failure was
    // ever persisted — which also left the brute-force detector querying a
    // store nothing wrote to.
    let audit_logger = std::sync::Arc::new(audit_logger);
    auth_service.set_audit_logger(std::sync::Arc::clone(&audit_logger));

    // Create security coordinator
    let security_coordinator =
        SecurityCoordinator::new(auth_service, rbac_manager, audit_logger, config);

    Ok(security_coordinator)
}

/// Back-compat initializer without an identity registry (tests, embedded).
pub async fn initialize_security(config: SecurityConfig) -> Result<SecurityCoordinator> {
    initialize_security_with_identity(config, None).await
}
