//! Enhanced authentication and authorization for multi-tenant enterprise

pub mod sso;
pub mod rbac;
pub mod federated_delegation_complete;

pub use sso::{SSOIntegrationManager, SSOToken, SSOProvider, EnterpriseUserContext};
pub use rbac::{EnhancedRBACManager, Permission, TenantRole};
pub use federated_delegation_complete::{CompleteFederatedIdentityDelegation, CompleteDelegationResult};

use anyhow::Result;

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
    pub async fn validate_and_resolve_token(&self, sso_token: &SSOToken) -> Result<EnterpriseUserContext> {
        self.sso_manager.validate_and_resolve_token(sso_token).await
    }

    /// Authenticate and authorize user operation
    pub async fn authenticate_and_authorize(
        &self,
        sso_token: &SSOToken,
        tenant_id: &str,
        operation: AuthorizedOperation,
    ) -> Result<AuthorizedContext> {
        // Validate SSO token and resolve user context
        let enterprise_user = self.sso_manager.validate_and_resolve_token(sso_token).await?;
        
        // Validate operation authorization with RBAC
        let authorization_result = match operation {
            AuthorizedOperation::CollectionAccess { collection_id, operation_type } => {
                self.rbac_manager.validate_collection_access(
                    tenant_id,
                    &collection_id,
                    operation_type,
                    &enterprise_user.clone().into(),
                ).await?
            },
            // Additional operation types to be added
        };

        Ok(AuthorizedContext {
            enterprise_user,
            authorization_result,
            authenticated_at: chrono::Utc::now(),
        })
    }
}

/// Operations requiring authorization
#[derive(Debug, Clone)]
pub enum AuthorizedOperation {
    CollectionAccess {
        collection_id: String,
        operation_type: rbac::CollectionOperation,
    },
    // Additional operations to be added
}

/// Authorized context for operations
#[derive(Debug, Clone)]
pub struct AuthorizedContext {
    pub enterprise_user: EnterpriseUserContext,
    pub authorization_result: rbac::AccessValidationResult,
    pub authenticated_at: chrono::DateTime<chrono::Utc>,
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
}