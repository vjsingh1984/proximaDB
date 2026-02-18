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

    // ========================== Additional Tests for Coverage ==========================

    // --- SSOToken Tests ---

    #[test]
    fn test_sso_token_all_providers() {
        let providers = vec![
            SSOProvider::AWSIAM,
            SSOProvider::AzureAD,
            SSOProvider::GoogleCloud,
            SSOProvider::SAML,
            SSOProvider::OIDC,
            SSOProvider::Okta,
            SSOProvider::Generic,
        ];

        for provider in providers {
            let token = SSOToken::new(
                provider.clone(),
                "token_data".to_string(),
                "user".to_string(),
                3600,
            );

            assert_eq!(token.provider, provider);
            assert!(!token.token_id.is_empty());
            assert!(!token.is_expired());
        }
    }

    #[test]
    fn test_sso_token_expires_at() {
        let token = SSOToken::new(
            SSOProvider::AWSIAM,
            "data".to_string(),
            "user".to_string(),
            7200, // 2 hours
        );

        assert!(token.expires_at > token.issued_at);
        let expected_duration = chrono::Duration::seconds(7200);
        let actual_duration = token.expires_at - token.issued_at;
        assert_eq!(actual_duration, expected_duration);
    }

    #[test]
    fn test_sso_token_is_expired_with_zero_seconds() {
        let mut token = SSOToken::new(
            SSOProvider::AWSIAM,
            "data".to_string(),
            "user".to_string(),
            0,
        );

        // Force expiration in the past
        token.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        assert!(token.is_expired());
    }

    #[test]
    fn test_sso_token_expires_soon_threshold() {
        // Token expires in 4 minutes (within 5 minute threshold)
        let mut token = SSOToken::new(
            SSOProvider::AWSIAM,
            "data".to_string(),
            "user".to_string(),
            3600,
        );
        token.expires_at = chrono::Utc::now() + chrono::Duration::minutes(4);

        assert!(!token.is_expired());
        assert!(token.expires_soon());
    }

    #[test]
    fn test_sso_token_not_expires_soon() {
        // Token expires in 10 minutes (outside 5 minute threshold)
        let mut token = SSOToken::new(
            SSOProvider::AWSIAM,
            "data".to_string(),
            "user".to_string(),
            3600,
        );
        token.expires_at = chrono::Utc::now() + chrono::Duration::minutes(10);

        assert!(!token.is_expired());
        assert!(!token.expires_soon());
    }

    #[test]
    fn test_sso_token_unique_token_id() {
        let token1 = SSOToken::new(
            SSOProvider::AWSIAM,
            "data".to_string(),
            "user".to_string(),
            3600,
        );
        let token2 = SSOToken::new(
            SSOProvider::AWSIAM,
            "data".to_string(),
            "user".to_string(),
            3600,
        );

        assert_ne!(token1.token_id, token2.token_id);
    }

    // --- SSOProvider Tests ---

    #[test]
    fn test_sso_provider_serialization() {
        let providers = vec![
            (SSOProvider::AWSIAM, "\"aws_iam\""),
            (SSOProvider::AzureAD, "\"azure_ad\""),
            (SSOProvider::GoogleCloud, "\"google_cloud\""),
            (SSOProvider::SAML, "\"saml\""),
            (SSOProvider::OIDC, "\"oidc\""),
            (SSOProvider::Okta, "\"okta\""),
            (SSOProvider::Generic, "\"generic\""),
        ];

        for (provider, expected_json) in providers {
            let json = serde_json::to_string(&provider).unwrap();
            assert_eq!(json, expected_json);
        }
    }

    #[test]
    fn test_sso_provider_deserialization() {
        let test_cases = vec![
            ("\"aws_iam\"", SSOProvider::AWSIAM),
            ("\"azure_ad\"", SSOProvider::AzureAD),
            ("\"google_cloud\"", SSOProvider::GoogleCloud),
            ("\"saml\"", SSOProvider::SAML),
            ("\"oidc\"", SSOProvider::OIDC),
            ("\"okta\"", SSOProvider::Okta),
            ("\"generic\"", SSOProvider::Generic),
        ];

        for (json, expected_provider) in test_cases {
            let provider: SSOProvider = serde_json::from_str(json).unwrap();
            assert_eq!(provider, expected_provider);
        }
    }

    #[test]
    fn test_sso_provider_clone() {
        let provider = SSOProvider::AWSIAM;
        let cloned = provider.clone();
        assert_eq!(provider, cloned);
    }

    // --- SecurityClearance Tests ---

    #[test]
    fn test_security_clearance_all_levels() {
        let levels = vec![
            SecurityClearance::Public,
            SecurityClearance::Internal,
            SecurityClearance::Confidential,
            SecurityClearance::Secret,
            SecurityClearance::TopSecret,
        ];

        // Verify ordering
        for i in 0..levels.len() - 1 {
            assert!(levels[i] < levels[i + 1]);
        }
    }

    #[test]
    fn test_security_clearance_serialization() {
        let clearances = vec![
            (SecurityClearance::Public, "\"Public\""),
            (SecurityClearance::Internal, "\"Internal\""),
            (SecurityClearance::Confidential, "\"Confidential\""),
            (SecurityClearance::Secret, "\"Secret\""),
            (SecurityClearance::TopSecret, "\"TopSecret\""),
        ];

        for (clearance, expected) in clearances {
            let json = serde_json::to_string(&clearance).unwrap();
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn test_security_clearance_deserialization() {
        let test_cases = vec![
            ("\"Public\"", SecurityClearance::Public),
            ("\"Internal\"", SecurityClearance::Internal),
            ("\"Confidential\"", SecurityClearance::Confidential),
            ("\"Secret\"", SecurityClearance::Secret),
            ("\"TopSecret\"", SecurityClearance::TopSecret),
        ];

        for (json, expected) in test_cases {
            let clearance: SecurityClearance = serde_json::from_str(json).unwrap();
            assert_eq!(clearance, expected);
        }
    }

    #[test]
    fn test_security_clearance_equality() {
        assert_eq!(SecurityClearance::Public, SecurityClearance::Public);
        assert_ne!(SecurityClearance::Public, SecurityClearance::Internal);
    }

    // --- EnterpriseUserContext Tests ---

    #[test]
    fn test_enterprise_user_context_system_admin_details() {
        let context = EnterpriseUserContext::system_admin();

        assert_eq!(context.user_id, "system");
        assert_eq!(context.email, "system@proximadb.com");
        assert_eq!(context.display_name, "System Administrator");
        assert_eq!(context.tenant_id, "system");
        assert_eq!(context.organization_id, "proximadb");
        assert!(context.roles.contains(&"system_admin".to_string()));
        assert!(context.permissions.contains("system_admin"));
        assert_eq!(context.security_clearance, SecurityClearance::TopSecret);
        assert!(context.department.is_none());
        assert!(context.cost_center.is_none());
        assert_eq!(context.session_id, "system_session");
    }

    #[test]
    fn test_enterprise_user_context_has_permission_direct() {
        let mut context = EnterpriseUserContext::system_admin();
        context.permissions.clear();
        context
            .permissions
            .insert("specific_permission".to_string());

        assert!(context.has_permission("specific_permission"));
        assert!(!context.has_permission("other_permission"));
    }

    #[test]
    fn test_enterprise_user_context_has_permission_admin_bypass() {
        let context = EnterpriseUserContext::system_admin();

        // System admin should have access to any permission
        assert!(context.has_permission("any_random_permission"));
        assert!(context.has_permission("nonexistent_permission"));
    }

    #[test]
    fn test_enterprise_user_context_has_role() {
        let mut context = EnterpriseUserContext::system_admin();
        context.roles = vec!["admin".to_string(), "developer".to_string()];

        assert!(context.has_role("admin"));
        assert!(context.has_role("developer"));
        assert!(!context.has_role("manager"));
    }

    #[test]
    fn test_enterprise_user_context_update_activity() {
        let mut context = EnterpriseUserContext::system_admin();
        let original_activity = context.last_activity;

        // Sleep briefly to ensure time difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        context.update_activity();

        assert!(context.last_activity >= original_activity);
    }

    #[test]
    fn test_enterprise_user_context_clone() {
        let context = EnterpriseUserContext::system_admin();
        let cloned = context.clone();

        assert_eq!(context.user_id, cloned.user_id);
        assert_eq!(context.email, cloned.email);
        assert_eq!(context.tenant_id, cloned.tenant_id);
        assert_eq!(context.security_clearance, cloned.security_clearance);
    }

    // --- ProviderUserContext Tests ---

    #[test]
    fn test_provider_user_context_aws() {
        let context = ProviderUserContext::AWS {
            account_id: "123456789012".to_string(),
            user_arn: "arn:aws:iam::123456789012:user/TestUser".to_string(),
            assumed_role_arn: Some("arn:aws:iam::123456789012:role/TestRole".to_string()),
            mfa_authenticated: true,
        };

        match context {
            ProviderUserContext::AWS {
                account_id,
                user_arn,
                assumed_role_arn,
                mfa_authenticated,
            } => {
                assert_eq!(account_id, "123456789012");
                assert!(user_arn.contains("TestUser"));
                assert!(assumed_role_arn.is_some());
                assert!(mfa_authenticated);
            }
            _ => panic!("Expected AWS context"),
        }
    }

    #[test]
    fn test_provider_user_context_azure() {
        let context = ProviderUserContext::Azure {
            tenant_id: "tenant-guid".to_string(),
            object_id: "object-guid".to_string(),
            user_principal_name: "user@company.com".to_string(),
            group_memberships: vec!["group1".to_string(), "group2".to_string()],
        };

        match context {
            ProviderUserContext::Azure {
                tenant_id,
                object_id,
                user_principal_name,
                group_memberships,
            } => {
                assert_eq!(tenant_id, "tenant-guid");
                assert_eq!(object_id, "object-guid");
                assert_eq!(user_principal_name, "user@company.com");
                assert_eq!(group_memberships.len(), 2);
            }
            _ => panic!("Expected Azure context"),
        }
    }

    #[test]
    fn test_provider_user_context_generic() {
        let mut attributes = std::collections::HashMap::new();
        attributes.insert("custom_attr".to_string(), "custom_value".to_string());

        let context = ProviderUserContext::Generic {
            provider_user_id: "generic_user_123".to_string(),
            attributes,
        };

        match context {
            ProviderUserContext::Generic {
                provider_user_id,
                attributes,
            } => {
                assert_eq!(provider_user_id, "generic_user_123");
                assert_eq!(
                    attributes.get("custom_attr"),
                    Some(&"custom_value".to_string())
                );
            }
            _ => panic!("Expected Generic context"),
        }
    }

    #[test]
    fn test_provider_user_context_clone() {
        let context = ProviderUserContext::AWS {
            account_id: "123".to_string(),
            user_arn: "arn".to_string(),
            assumed_role_arn: None,
            mfa_authenticated: false,
        };

        let cloned = context.clone();

        match (context, cloned) {
            (
                ProviderUserContext::AWS {
                    account_id: id1, ..
                },
                ProviderUserContext::AWS {
                    account_id: id2, ..
                },
            ) => {
                assert_eq!(id1, id2);
            }
            _ => panic!("Cloning failed"),
        }
    }

    // --- ProviderMetadata Tests ---

    #[test]
    fn test_provider_metadata_creation() {
        let mut additional_claims = std::collections::HashMap::new();
        additional_claims.insert(
            "custom_claim".to_string(),
            serde_json::Value::String("value".to_string()),
        );

        let metadata = ProviderMetadata {
            provider: SSOProvider::AWSIAM,
            validation_method: "STS_GetCallerIdentity".to_string(),
            token_type: "Bearer".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            additional_claims,
        };

        assert_eq!(metadata.provider, SSOProvider::AWSIAM);
        assert_eq!(metadata.validation_method, "STS_GetCallerIdentity");
        assert_eq!(metadata.token_type, "Bearer");
        assert!(metadata.expires_at > chrono::Utc::now());
        assert!(metadata.additional_claims.contains_key("custom_claim"));
    }

    #[test]
    fn test_provider_metadata_clone() {
        let metadata = ProviderMetadata {
            provider: SSOProvider::AzureAD,
            validation_method: "OAuth2".to_string(),
            token_type: "JWT".to_string(),
            expires_at: chrono::Utc::now(),
            additional_claims: std::collections::HashMap::new(),
        };

        let cloned = metadata.clone();

        assert_eq!(metadata.provider, cloned.provider);
        assert_eq!(metadata.validation_method, cloned.validation_method);
        assert_eq!(metadata.token_type, cloned.token_type);
    }

    // --- SSOValidationResult Tests ---

    #[test]
    fn test_sso_validation_result_valid() {
        let result = SSOValidationResult {
            valid: true,
            user_context: EnterpriseUserContext::system_admin(),
            provider_metadata: ProviderMetadata {
                provider: SSOProvider::AWSIAM,
                validation_method: "Test".to_string(),
                token_type: "Bearer".to_string(),
                expires_at: chrono::Utc::now(),
                additional_claims: std::collections::HashMap::new(),
            },
            validation_timestamp: chrono::Utc::now(),
        };

        assert!(result.valid);
        assert_eq!(result.user_context.user_id, "system");
    }

    #[test]
    fn test_sso_validation_result_invalid() {
        let result = SSOValidationResult {
            valid: false,
            user_context: EnterpriseUserContext::system_admin(),
            provider_metadata: ProviderMetadata {
                provider: SSOProvider::Generic,
                validation_method: "Failed".to_string(),
                token_type: "Unknown".to_string(),
                expires_at: chrono::Utc::now(),
                additional_claims: std::collections::HashMap::new(),
            },
            validation_timestamp: chrono::Utc::now(),
        };

        assert!(!result.valid);
    }

    #[test]
    fn test_sso_validation_result_clone() {
        let result = SSOValidationResult {
            valid: true,
            user_context: EnterpriseUserContext::system_admin(),
            provider_metadata: ProviderMetadata {
                provider: SSOProvider::AWSIAM,
                validation_method: "Test".to_string(),
                token_type: "Bearer".to_string(),
                expires_at: chrono::Utc::now(),
                additional_claims: std::collections::HashMap::new(),
            },
            validation_timestamp: chrono::Utc::now(),
        };

        let cloned = result.clone();

        assert_eq!(result.valid, cloned.valid);
        assert_eq!(result.user_context.user_id, cloned.user_context.user_id);
    }

    // --- Custom EnterpriseUserContext Tests ---

    #[test]
    fn test_enterprise_user_context_custom() {
        let mut permissions = HashSet::new();
        permissions.insert("read".to_string());
        permissions.insert("write".to_string());

        let context = EnterpriseUserContext {
            user_id: "custom_user".to_string(),
            email: "custom@example.com".to_string(),
            display_name: "Custom User".to_string(),
            tenant_id: "custom_tenant".to_string(),
            organization_id: "custom_org".to_string(),
            roles: vec!["developer".to_string(), "reviewer".to_string()],
            permissions,
            security_clearance: SecurityClearance::Confidential,
            department: Some("Engineering".to_string()),
            cost_center: Some("CC001".to_string()),
            session_id: "session_123".to_string(),
            login_timestamp: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
            provider_context: ProviderUserContext::Generic {
                provider_user_id: "provider_123".to_string(),
                attributes: std::collections::HashMap::new(),
            },
        };

        assert_eq!(context.user_id, "custom_user");
        assert!(context.has_permission("read"));
        assert!(context.has_permission("write"));
        assert!(!context.has_permission("delete")); // Not system_admin, so no bypass
        assert!(context.has_role("developer"));
        assert!(context.has_role("reviewer"));
        assert_eq!(context.department, Some("Engineering".to_string()));
        assert_eq!(context.cost_center, Some("CC001".to_string()));
    }

    // --- Edge Cases ---

    #[test]
    fn test_empty_roles_and_permissions() {
        let context = EnterpriseUserContext {
            user_id: "limited_user".to_string(),
            email: "limited@example.com".to_string(),
            display_name: "Limited User".to_string(),
            tenant_id: "tenant".to_string(),
            organization_id: "org".to_string(),
            roles: vec![],
            permissions: HashSet::new(),
            security_clearance: SecurityClearance::Public,
            department: None,
            cost_center: None,
            session_id: "session".to_string(),
            login_timestamp: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
            provider_context: ProviderUserContext::Generic {
                provider_user_id: "user".to_string(),
                attributes: std::collections::HashMap::new(),
            },
        };

        assert!(!context.has_permission("any_permission"));
        assert!(!context.has_role("any_role"));
        assert!(context.roles.is_empty());
        assert!(context.permissions.is_empty());
    }
}
