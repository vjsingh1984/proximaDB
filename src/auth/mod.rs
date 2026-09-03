//! Enterprise identity types shared by the AI insights and audit surfaces.
//
//! History (TD-SSO-1 / TD-SSO-2 arc): this module previously hosted the
//! legacy enterprise-SSO stack — `EnterpriseAuthManager`, the
//! `federated_delegation_complete` AWS/Azure delegation path, a local RBAC
//! copy, and the `proximadb-auth-sso` crate re-export. All of it was
//! dead (zero route registrations, zero constructions, stub validators)
//! and was removed. Live authentication lives in `crate::security` (local
// JWT + OIDC resource server) and `crate::network::auth` (middleware).
//
//! What remains is `EnterpriseUserContext` — the identity parameter type
//! threaded through the AI/audit reporting surfaces — relocated verbatim
//! from the removed crate. Nothing constructs it from a verified token
//! today; when TD-SSO-2 (multi-provider OIDC) lands, map the verified
//! OIDC claims into this type at the AI/audit seams.

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};

/// Enhanced user context for enterprise operations
#[derive(Debug, Clone)]
pub struct EnterpriseUserContext {
    /// Unique user identifier from the identity provider.
    pub user_id: String,
    /// User email address.
    pub email: String,
    /// Human-readable display name.
    pub display_name: String,

    /// Tenant the user belongs to.
    pub tenant_id: String,
    /// Organization the user belongs to.
    pub organization_id: String,

    /// Assigned role names for RBAC.
    pub roles: Vec<String>,
    /// Granted permission strings for fine-grained access control.
    pub permissions: HashSet<String>,

    /// User's security clearance level.
    pub security_clearance: SecurityClearance,
    /// User's department within the organization.
    pub department: Option<String>,
    /// Cost center for billing attribution.
    pub cost_center: Option<String>,

    /// Active session identifier.
    pub session_id: String,
    /// Timestamp of the initial login.
    pub login_timestamp: DateTime<Utc>,
    /// Timestamp of the most recent activity.
    pub last_activity: DateTime<Utc>,

    /// Provider-specific identity context (AWS, Azure, or generic).
    pub provider_context: ProviderUserContext,
}

/// Security clearance levels
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub enum SecurityClearance {
    /// Publicly accessible, no clearance required.
    Public,
    /// Internal-only, requires organizational membership.
    Internal,
    /// Confidential data, requires explicit clearance.
    Confidential,
    /// Secret data, requires elevated clearance.
    Secret,
    /// Top secret data, requires maximum clearance.
    TopSecret,
}

/// Provider-specific user context
#[derive(Debug, Clone)]
pub enum ProviderUserContext {
    /// AWS IAM identity context.
    AWS {
        /// AWS account ID.
        account_id: String,
        /// Full IAM user ARN.
        user_arn: String,
        /// ARN of the assumed role, if any.
        assumed_role_arn: Option<String>,
        /// Whether MFA was used during authentication.
        mfa_authenticated: bool,
    },
    /// Azure AD identity context.
    Azure {
        /// Azure AD tenant identifier.
        tenant_id: String,
        /// Azure AD object identifier.
        object_id: String,
        /// User principal name (UPN).
        user_principal_name: String,
        /// Azure AD group memberships.
        group_memberships: Vec<String>,
    },
    /// Generic provider identity context.
    Generic {
        /// Provider-specific user identifier.
        provider_user_id: String,
        /// Arbitrary key-value attributes from the provider.
        attributes: HashMap<String, String>,
    },
}

impl EnterpriseUserContext {
    /// Create system admin context for internal operations
    pub fn system_admin() -> Self {
        let now = Utc::now();
        Self {
            user_id: "system".to_string(),
            email: "system@proximadb.com".to_string(),
            display_name: "System Administrator".to_string(),
            tenant_id: "system".to_string(),
            organization_id: "proximadb".to_string(),
            roles: vec!["system_admin".to_string()],
            permissions: ["system_admin".to_string()].into_iter().collect(),
            security_clearance: SecurityClearance::TopSecret,
            department: None,
            cost_center: None,
            session_id: "system_session".to_string(),
            login_timestamp: now,
            last_activity: now,
            provider_context: ProviderUserContext::Generic {
                provider_user_id: "system".to_string(),
                attributes: HashMap::new(),
            },
        }
    }

    /// Check if user has specific permission
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.contains(permission) || self.permissions.contains("system_admin")
    }

    /// Check if user has role
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.contains(&role.to_string())
    }

    /// Update last activity timestamp
    pub fn update_activity(&mut self) {
        self.last_activity = Utc::now();
    }
}

/// Convert an enterprise user context to the storage-layer user context
pub fn enterprise_to_storage_user_context(
    enterprise_user: EnterpriseUserContext,
) -> crate::storage::tenant::UserContext {
    crate::storage::tenant::UserContext {
        user_id: enterprise_user.user_id,
        tenant_id: enterprise_user.tenant_id,
        roles: enterprise_user.roles,
        permissions: enterprise_user.permissions.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_admin_context_has_admin_role_and_permission() {
        let ctx = EnterpriseUserContext::system_admin();
        assert!(ctx.has_role("system_admin"));
        assert!(ctx.has_permission("anything-else"));
        assert_eq!(ctx.security_clearance, SecurityClearance::TopSecret);
    }

    #[test]
    fn enterprise_context_converts_to_storage_user_context() {
        let storage_user =
            enterprise_to_storage_user_context(EnterpriseUserContext::system_admin());
        assert_eq!(storage_user.user_id, "system");
        assert_eq!(storage_user.tenant_id, "system");
    }
}
