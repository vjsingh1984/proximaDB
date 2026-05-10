//! Enhanced authentication and authorization for multi-tenant enterprise
//
//! **DEPRECATION NOTICE (TD-AUTH-CONSOLIDATION)**:
//! This module is being consolidated into `crate::security`.
//! - SSO/OIDC/SAML → `crate::security::auth` + `crate::security::unified_auth`
//! - RBAC → `crate::security::unified_rbac`
//! - Network-layer auth (JWT, middleware) → `crate::network::auth`
//!
//! New code should import from `crate::security` directly.
//! This module will become a thin re-export shim and be removed in a future release.
//! See docs/10-quality/TECHNICAL_DEBT.adoc for tracking.

pub mod federated_delegation_complete;
pub mod rbac;
pub mod sso;

pub use federated_delegation_complete::{
    CompleteDelegationResult, CompleteFederatedIdentityDelegation,
};
#[deprecated(note = "Use `crate::security::unified_auth::AuthenticationResult` directly for canonical flows.")]
pub use federated_delegation_complete::FederatedAuthenticationResult;
pub use rbac::{EnhancedRBACManager, Permission, TenantRole};
pub use sso::{EnterpriseUserContext, SSOIntegrationManager, SSOProvider, SSOToken};

use anyhow::Result;
use crate::security::security_coordinator::{
    AuthorizedContext as SecurityAuthorizedContext,
    SessionMetadata as SecuritySessionMetadata,
};
#[deprecated(note = "Canonical security result type now lives in crate::security::SecurityAuthenticationResult.")]
pub type SecurityAuthenticationResult = crate::security::SecurityAuthenticationResult;
use crate::security::unified_rbac::{UnifiedAuthMethod, UnifiedPermission, UnifiedUserContext};

/// Enterprise authentication coordinator
pub struct EnterpriseAuthManager {
    /// SSO integration manager
    sso_manager: sso::SSOIntegrationManager,

    /// Enhanced RBAC manager
    rbac_manager: rbac::EnhancedRBACManager,
}

impl EnterpriseAuthManager {
    /// Create new enterprise auth manager
    pub fn new(
        sso_manager: sso::SSOIntegrationManager,
        rbac_manager: rbac::EnhancedRBACManager,
    ) -> Self {
        Self {
            sso_manager,
            rbac_manager,
        }
    }

    /// Validate and resolve SSO token to enterprise user context
    pub async fn validate_and_resolve_token(
        &self,
        sso_token: &SSOToken,
    ) -> Result<EnterpriseUserContext> {
        self.sso_manager.validate_and_resolve_token(sso_token).await
    }

    /// Authenticate and authorize user operation
    pub async fn authenticate_and_authorize(
        &self,
        sso_token: &SSOToken,
        tenant_id: &str,
        operation: AuthorizedOperation,
    ) -> Result<SecurityAuthorizedContext> {
        // Validate SSO token and resolve user context
        let enterprise_user = self
            .sso_manager
            .validate_and_resolve_token(sso_token)
            .await?;

        // Validate operation authorization with RBAC
        let operation_permission = map_authorized_operation_permission(&operation);
        let _authorization_result = match operation {
            AuthorizedOperation::CollectionAccess {
                collection_id,
                operation_type,
            } => {
                self.rbac_manager
                    .validate_collection_access(
                        tenant_id,
                        &collection_id,
                        operation_type,
                        &enterprise_user.clone().into(),
                    )
                    .await?
            } // Additional operation types to be added
                };

        Ok(SecurityAuthorizedContext {
            user_context: enterprise_user_to_unified_user_context(enterprise_user),
            granted_permission: operation_permission,
            session_metadata: SecuritySessionMetadata {
                authenticated_at: chrono::Utc::now(),
                auth_method: map_sso_provider(&sso_token.provider),
                requires_mfa: false,
            },
        })
    }
}

/// Operations requiring authorization
#[derive(Debug, Clone)]
pub enum AuthorizedOperation {
    /// Access a collection with a specific operation type.
    CollectionAccess {
        /// Target collection identifier.
        collection_id: String,
        /// Type of operation to perform on the collection.
        operation_type: rbac::CollectionOperation,
    },
    // Additional operations to be added
}

#[deprecated(
    note = "Moved to crate::security::security_coordinator::AuthorizedContext. \
            Auth shim retained temporarily during consolidation."
)]
/// Temporary compatibility alias for phased auth/security migration.
pub type AuthorizedContext = crate::security::security_coordinator::AuthorizedContext;

fn map_authorized_operation_permission(
    operation: &AuthorizedOperation,
) -> UnifiedPermission {
    match operation {
        AuthorizedOperation::CollectionAccess {
            collection_id,
            operation_type,
        } => match operation_type {
            rbac::CollectionOperation::Read => {
                UnifiedPermission::CollectionRead(collection_id.to_string())
            }
            rbac::CollectionOperation::Write => {
                UnifiedPermission::CollectionWrite(collection_id.to_string())
            }
            rbac::CollectionOperation::Delete => {
                UnifiedPermission::CollectionDelete(collection_id.to_string())
            }
            rbac::CollectionOperation::Admin => {
                UnifiedPermission::CollectionAdmin(collection_id.to_string())
            }
        },
    }
}

fn map_sso_provider(provider: &SSOProvider) -> UnifiedAuthMethod {
    let provider_name = match provider {
        SSOProvider::AWSIAM => "aws_iam",
        SSOProvider::AzureAD => "azure_ad",
        SSOProvider::GoogleCloud => "google_cloud",
        SSOProvider::SAML => "saml",
        SSOProvider::OIDC => "oidc",
        SSOProvider::Okta => "okta",
        SSOProvider::Generic => "generic",
    };

    UnifiedAuthMethod::SSO {
        provider: provider_name.to_string(),
    }
}

fn enterprise_user_to_unified_user_context(
    enterprise_user: EnterpriseUserContext,
) -> UnifiedUserContext {
    use std::collections::{HashMap, HashSet};

    UnifiedUserContext {
        user_id: enterprise_user.user_id,
        tenant_id: Some(enterprise_user.tenant_id),
        roles: enterprise_user.roles,
        effective_permissions: HashSet::new(),
        auth_method: match &enterprise_user.provider_context {
            sso::types::ProviderUserContext::AWS { .. } => {
                map_sso_provider(&SSOProvider::AWSIAM)
            }
            sso::types::ProviderUserContext::Azure { .. } => {
                map_sso_provider(&SSOProvider::AzureAD)
            }
            sso::types::ProviderUserContext::Generic { .. } => {
                map_sso_provider(&SSOProvider::Generic)
            }
        },
        session_id: enterprise_user.session_id,
        expires_at: None,
        created_at: enterprise_user.login_timestamp,
        metadata: {
            let mut metadata = HashMap::new();
            metadata.insert(
                "email".to_string(),
                enterprise_user.email.clone(),
            );
            metadata.insert(
                "organization_id".to_string(),
                enterprise_user.organization_id.clone(),
            );
            metadata
        },
    }
}

// Conversion from EnterpriseUserContext to storage::tenant::UserContext
impl From<EnterpriseUserContext> for crate::storage::tenant::UserContext {
    fn from(enterprise_user: EnterpriseUserContext) -> Self {
        Self {
            user_id: enterprise_user.user_id,
            tenant_id: enterprise_user.tenant_id,
            roles: enterprise_user.roles,
            permissions: enterprise_user.permissions.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_context_conversion() {
        let enterprise_user = EnterpriseUserContext::system_admin();
        let storage_user: crate::storage::tenant::UserContext = enterprise_user.into();

        assert_eq!(storage_user.user_id, "system");
        assert_eq!(storage_user.tenant_id, "system");
    }

    // ========================== Additional Tests for Coverage ==========================

    // --- AuthorizedOperation Tests ---

    #[test]
    fn test_authorized_operation_collection_access() {
        let operation = AuthorizedOperation::CollectionAccess {
            collection_id: "test_collection".to_string(),
            operation_type: rbac::CollectionOperation::Read,
        };

        match operation {
            AuthorizedOperation::CollectionAccess {
                collection_id,
                operation_type,
            } => {
                assert_eq!(collection_id, "test_collection");
                assert!(matches!(operation_type, rbac::CollectionOperation::Read));
            }
        }
    }

    #[test]
    fn test_authorized_operation_collection_access_write() {
        let operation = AuthorizedOperation::CollectionAccess {
            collection_id: "writable_collection".to_string(),
            operation_type: rbac::CollectionOperation::Write,
        };

        match operation {
            AuthorizedOperation::CollectionAccess {
                collection_id,
                operation_type,
            } => {
                assert_eq!(collection_id, "writable_collection");
                assert!(matches!(operation_type, rbac::CollectionOperation::Write));
            }
        }
    }

    #[test]
    fn test_authorized_operation_collection_access_delete() {
        let operation = AuthorizedOperation::CollectionAccess {
            collection_id: "deletable_collection".to_string(),
            operation_type: rbac::CollectionOperation::Delete,
        };

        match operation {
            AuthorizedOperation::CollectionAccess {
                collection_id,
                operation_type,
            } => {
                assert_eq!(collection_id, "deletable_collection");
                assert!(matches!(operation_type, rbac::CollectionOperation::Delete));
            }
        }
    }

    #[test]
    fn test_authorized_operation_clone() {
        let operation = AuthorizedOperation::CollectionAccess {
            collection_id: "test".to_string(),
            operation_type: rbac::CollectionOperation::Read,
        };

        let cloned = operation.clone();

        match (operation, cloned) {
            (
                AuthorizedOperation::CollectionAccess {
                    collection_id: id1,
                    operation_type: op1,
                },
                AuthorizedOperation::CollectionAccess {
                    collection_id: id2,
                    operation_type: op2,
                },
            ) => {
                assert_eq!(id1, id2);
                assert!(matches!(op1, rbac::CollectionOperation::Read));
                assert!(matches!(op2, rbac::CollectionOperation::Read));
            }
        }
    }

    // --- User Context Conversion Tests ---

    #[test]
    fn test_user_context_conversion_with_roles() {
        use std::collections::HashSet;

        let enterprise_user = sso::EnterpriseUserContext {
            user_id: "test_user".to_string(),
            email: "test@example.com".to_string(),
            display_name: "Test User".to_string(),
            tenant_id: "tenant_123".to_string(),
            organization_id: "org_456".to_string(),
            roles: vec!["admin".to_string(), "developer".to_string()],
            permissions: {
                let mut perms = HashSet::new();
                perms.insert("read".to_string());
                perms.insert("write".to_string());
                perms
            },
            security_clearance: sso::types::SecurityClearance::Confidential,
            department: Some("Engineering".to_string()),
            cost_center: Some("CC001".to_string()),
            session_id: "session_abc".to_string(),
            login_timestamp: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
            provider_context: sso::types::ProviderUserContext::Generic {
                provider_user_id: "provider_user_123".to_string(),
                attributes: std::collections::HashMap::new(),
            },
        };

        let storage_user: crate::storage::tenant::UserContext = enterprise_user.into();

        assert_eq!(storage_user.user_id, "test_user");
        assert_eq!(storage_user.tenant_id, "tenant_123");
        assert!(storage_user.roles.contains(&"admin".to_string()));
        assert!(storage_user.roles.contains(&"developer".to_string()));
        assert!(storage_user.permissions.contains(&"read".to_string()));
        assert!(storage_user.permissions.contains(&"write".to_string()));
    }

    #[test]
    fn test_user_context_conversion_empty_permissions() {
        use std::collections::HashSet;

        let enterprise_user = sso::EnterpriseUserContext {
            user_id: "minimal_user".to_string(),
            email: "minimal@example.com".to_string(),
            display_name: "Minimal User".to_string(),
            tenant_id: "default_tenant".to_string(),
            organization_id: "default_org".to_string(),
            roles: vec![],
            permissions: HashSet::new(),
            security_clearance: sso::types::SecurityClearance::Public,
            department: None,
            cost_center: None,
            session_id: "session_xyz".to_string(),
            login_timestamp: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
            provider_context: sso::types::ProviderUserContext::Generic {
                provider_user_id: "user".to_string(),
                attributes: std::collections::HashMap::new(),
            },
        };

        let storage_user: crate::storage::tenant::UserContext = enterprise_user.into();

        assert_eq!(storage_user.user_id, "minimal_user");
        assert!(storage_user.roles.is_empty());
        assert!(storage_user.permissions.is_empty());
    }

    // --- Integration Validation Tests ---

    #[tokio::test]
    async fn test_validate_and_resolve_token_no_aws_config() {
        let sso_manager = sso::SSOIntegrationManager::new();

        // Create an AWS SSO token
        let sso_token = sso::SSOToken::new(
            sso::SSOProvider::AWSIAM,
            "test_token_data".to_string(),
            "test_user".to_string(),
            3600,
        );

        // Without AWS configured, should fail
        let result = sso_manager.validate_and_resolve_token(&sso_token).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not configured"));
    }

    #[tokio::test]
    async fn test_validate_and_resolve_token_unsupported_provider() {
        let sso_manager = sso::SSOIntegrationManager::new();

        // Create a token with unsupported provider
        let sso_token = sso::SSOToken::new(
            sso::SSOProvider::Generic,
            "test_token_data".to_string(),
            "test_user".to_string(),
            3600,
        );

        // Unsupported provider should fail
        let result = sso_manager.validate_and_resolve_token(&sso_token).await;
        assert!(result.is_err());
    }

    // --- Re-exports Tests ---

    #[test]
    fn test_re_exports_available() {
        // Verify that re-exports are accessible
        let _provider = sso::SSOProvider::AWSIAM;
        let _context = EnterpriseUserContext::system_admin();

        // These should compile successfully
        assert!(true);
    }

    #[test]
    fn test_rbac_re_exports() {
        // Verify RBAC re-exports
        let _permission = rbac::Permission::CollectionRead;
        let _operation = rbac::CollectionOperation::Read;

        assert!(true);
    }

    // --- Enterprise User Context Tests ---

    #[test]
    fn test_enterprise_user_context_system_admin() {
        let context = EnterpriseUserContext::system_admin();

        assert_eq!(context.user_id, "system");
        assert!(context.has_permission("system_admin"));
        assert!(context.has_role("system_admin"));
    }

    #[test]
    fn test_enterprise_user_context_has_permission() {
        let mut context = EnterpriseUserContext::system_admin();
        context.permissions.insert("custom_permission".to_string());

        assert!(context.has_permission("custom_permission"));
        // System admin has bypass
        assert!(context.has_permission("any_permission"));
    }

    #[test]
    fn test_enterprise_user_context_has_role() {
        let mut context = EnterpriseUserContext::system_admin();
        context.roles.push("custom_role".to_string());

        assert!(context.has_role("custom_role"));
        assert!(context.has_role("system_admin"));
        assert!(!context.has_role("nonexistent_role"));
    }

    // --- SSO Token Tests ---

    #[test]
    fn test_sso_token_creation_and_expiration() {
        let token = sso::SSOToken::new(
            sso::SSOProvider::AWSIAM,
            "test_data".to_string(),
            "test_user".to_string(),
            3600,
        );

        assert!(!token.is_expired());
        assert!(!token.expires_soon());
        assert_eq!(token.provider, sso::SSOProvider::AWSIAM);
        assert_eq!(token.user_id, "test_user");
    }

    #[test]
    fn test_sso_token_providers() {
        let providers = vec![
            sso::SSOProvider::AWSIAM,
            sso::SSOProvider::AzureAD,
            sso::SSOProvider::GoogleCloud,
            sso::SSOProvider::SAML,
            sso::SSOProvider::OIDC,
            sso::SSOProvider::Okta,
            sso::SSOProvider::Generic,
        ];

        for provider in providers {
            let token = sso::SSOToken::new(
                provider.clone(),
                "data".to_string(),
                "user".to_string(),
                3600,
            );
            assert_eq!(token.provider, provider);
        }
    }
}
