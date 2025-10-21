//! SSO type definitions - clean and simple

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// SSO token for authentication
#[derive(Debug, Clone)]
pub struct SSOToken {
    pub token_id: String,
    pub provider: SSOProvider,
    pub token_data: String, // Provider-specific token data
    pub user_id: String,
    pub expires_at: DateTime<Utc>,
    pub issued_at: DateTime<Utc>,
}

/// Supported SSO providers
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SSOProvider {
    #[serde(rename = "aws_iam")]
    AWSIAM,
    #[serde(rename = "azure_ad")]
    AzureAD,
    #[serde(rename = "google_cloud")]
    GoogleCloud,
    #[serde(rename = "saml")]
    SAML,
    #[serde(rename = "oidc")]
    OIDC,
    #[serde(rename = "okta")]
    Okta,
    #[serde(rename = "generic")]
    Generic,
}

/// SSO validation result
#[derive(Debug, Clone)]
pub struct SSOValidationResult {
    pub valid: bool,
    pub user_context: EnterpriseUserContext,
    pub provider_metadata: ProviderMetadata,
    pub validation_timestamp: DateTime<Utc>,
}

/// Enhanced user context for enterprise operations
#[derive(Debug, Clone)]
pub struct EnterpriseUserContext {
    /// Basic user information
    pub user_id: String,
    pub email: String,
    pub display_name: String,

    /// Tenant association
    pub tenant_id: String,
    pub organization_id: String,

    /// Roles and permissions
    pub roles: Vec<String>,
    pub permissions: HashSet<String>,

    /// Security context
    pub security_clearance: SecurityClearance,
    pub department: Option<String>,
    pub cost_center: Option<String>,

    /// Session information
    pub session_id: String,
    pub login_timestamp: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,

    /// Provider-specific context
    pub provider_context: ProviderUserContext,
}

/// Security clearance levels
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum SecurityClearance {
    Public,
    Internal,
    Confidential,
    Secret,
    TopSecret,
}

/// Provider-specific user context
#[derive(Debug, Clone)]
pub enum ProviderUserContext {
    AWS {
        account_id: String,
        user_arn: String,
        assumed_role_arn: Option<String>,
        mfa_authenticated: bool,
    },
    Azure {
        tenant_id: String,
        object_id: String,
        user_principal_name: String,
        group_memberships: Vec<String>,
    },
    Generic {
        provider_user_id: String,
        attributes: std::collections::HashMap<String, String>,
    },
}

/// Provider metadata for audit and troubleshooting
#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    pub provider: SSOProvider,
    pub validation_method: String,
    pub token_type: String,
    pub expires_at: DateTime<Utc>,
    pub additional_claims: std::collections::HashMap<String, serde_json::Value>,
}

impl SSOToken {
    /// Create new SSO token
    pub fn new(
        provider: SSOProvider,
        token_data: String,
        user_id: String,
        expires_in_seconds: u32,
    ) -> Self {
        let now = Utc::now();
        Self {
            token_id: uuid::Uuid::new_v4().to_string(),
            provider,
            token_data,
            user_id,
            expires_at: now + Duration::seconds(expires_in_seconds as i64),
            issued_at: now,
        }
    }

    /// Check if token is expired
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Check if token expires soon (within 5 minutes)
    pub fn expires_soon(&self) -> bool {
        Utc::now() + Duration::minutes(5) > self.expires_at
    }
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
                attributes: std::collections::HashMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sso_token_creation() {
        let token = SSOToken::new(
            SSOProvider::AWSIAM,
            "test_token_data".to_string(),
            "test_user".to_string(),
            3600, // 1 hour
        );

        assert_eq!(token.provider, SSOProvider::AWSIAM);
        assert_eq!(token.user_id, "test_user");
        assert!(!token.is_expired());
    }

    #[test]
    fn test_enterprise_user_context() {
        let context = EnterpriseUserContext::system_admin();

        assert_eq!(context.user_id, "system");
        assert!(context.has_permission("system_admin"));
        assert!(context.has_role("system_admin"));
        assert_eq!(context.security_clearance, SecurityClearance::TopSecret);
    }

    #[test]
    fn test_security_clearance_ordering() {
        assert!(SecurityClearance::TopSecret > SecurityClearance::Secret);
        assert!(SecurityClearance::Secret > SecurityClearance::Confidential);
        assert!(SecurityClearance::Confidential > SecurityClearance::Internal);
        assert!(SecurityClearance::Internal > SecurityClearance::Public);
    }
}
