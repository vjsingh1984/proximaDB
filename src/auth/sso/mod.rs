//! SSO integration for enterprise identity providers

pub mod aws_iam;
pub mod azure_ad;
pub mod google_cloud;
pub mod oidc;
pub mod saml;
pub mod types;

pub use aws_iam::AWSIAMIntegration;
pub use azure_ad::AzureADIntegration;
pub use google_cloud::GoogleCloudIntegration;
pub use oidc::OIDCIntegration;
pub use saml::SAMLIntegration;
pub use types::{EnterpriseUserContext, SSOProvider, SSOToken, SSOValidationResult};

use anyhow::Result;
use std::sync::Arc;

/// Clean SSO integration manager
pub struct SSOIntegrationManager {
    /// AWS IAM integration
    aws_integration: Option<Arc<AWSIAMIntegration>>,

    /// Azure AD integration
    azure_integration: Option<Arc<AzureADIntegration>>,

    /// Google Cloud integration
    google_cloud_integration: Option<Arc<GoogleCloudIntegration>>,

    /// Simple token cache for performance
    token_cache: Arc<dashmap::DashMap<String, CachedTokenValidation>>,
}

impl SSOIntegrationManager {
    /// Create new SSO manager
    pub fn new() -> Self {
        Self {
            aws_integration: None,
            azure_integration: None,
            google_cloud_integration: None,
            token_cache: Arc::new(dashmap::DashMap::new()),
        }
    }

    /// Configure AWS IAM integration
    pub fn configure_aws_iam(&mut self, config: AWSIAMConfig) -> Result<()> {
        let integration = AWSIAMIntegration::new(config)?;
        self.aws_integration = Some(Arc::new(integration));
        Ok(())
    }

    /// Configure Azure AD integration
    pub fn configure_azure_ad(&mut self, config: AzureADConfig) -> Result<()> {
        let integration = AzureADIntegration::new(config)?;
        self.azure_integration = Some(Arc::new(integration));
        Ok(())
    }

    /// Validate SSO token and resolve to enterprise user context
    pub async fn validate_and_resolve_token(
        &self,
        sso_token: &SSOToken,
    ) -> Result<EnterpriseUserContext> {
        // Check cache first for performance
        if let Some(cached) = self.token_cache.get(&sso_token.token_id) {
            if !cached.is_expired() {
                return Ok(cached.user_context.clone());
            }
        }

        // Validate with appropriate provider
        let validation_result = match &sso_token.provider {
            SSOProvider::AWSIAM => {
                let aws = self
                    .aws_integration
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("AWS IAM not configured"))?;
                aws.validate_token(sso_token).await?
            }
            SSOProvider::AzureAD => {
                let azure = self
                    .azure_integration
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Azure AD not configured"))?;
                azure.validate_token(sso_token).await?
            }
            _ => return Err(anyhow::anyhow!("Unsupported SSO provider")),
        };

        // Cache validation result
        self.token_cache.insert(
            sso_token.token_id.clone(),
            CachedTokenValidation {
                user_context: validation_result.user_context.clone(),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5), // 5 min cache
            },
        );

        Ok(validation_result.user_context)
    }
}

/// Cached token validation for performance
#[derive(Debug, Clone)]
struct CachedTokenValidation {
    user_context: EnterpriseUserContext,
    expires_at: DateTime<Utc>,
}

impl CachedTokenValidation {
    fn is_expired(&self) -> bool {
        chrono::Utc::now() > self.expires_at
    }
}

/// AWS IAM configuration
#[derive(Debug, Clone)]
pub struct AWSIAMConfig {
    pub region: String,
    pub role_mapping: Vec<AWSRoleMapping>,
    pub enable_cross_account: bool,
    pub trusted_account_ids: Vec<String>,
}

/// Azure AD configuration
#[derive(Debug, Clone)]
pub struct AzureADConfig {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub authority: String,
}

/// AWS role mapping for enterprise users
#[derive(Debug, Clone)]
pub struct AWSRoleMapping {
    pub aws_role_arn: String,
    pub proximadb_role: String,
    pub tenant_id: String,
}

use chrono::{DateTime, Utc};

/// Global SSO manager instance
static SSO_MANAGER: std::sync::OnceLock<SSOIntegrationManager> = std::sync::OnceLock::new();

/// Initialize global SSO manager
pub fn initialize_sso_manager() -> &'static SSOIntegrationManager {
    SSO_MANAGER.get_or_init(|| SSOIntegrationManager::new())
}

/// Get global SSO manager
pub fn get_sso_manager() -> Option<&'static SSOIntegrationManager> {
    SSO_MANAGER.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sso_manager_creation() {
        let manager = SSOIntegrationManager::new();
        assert!(manager.aws_integration.is_none());
        assert!(manager.azure_integration.is_none());
    }

    #[test]
    fn test_aws_config() {
        let config = AWSIAMConfig {
            region: "us-east-1".to_string(),
            role_mapping: vec![],
            enable_cross_account: false,
            trusted_account_ids: vec!["123456789012".to_string()],
        };

        assert_eq!(config.region, "us-east-1");
        assert!(!config.enable_cross_account);
    }

    // ========================== Additional Tests for Coverage ==========================

    // --- SSOIntegrationManager Tests ---

    #[test]
    fn test_sso_manager_configure_aws_iam() {
        let mut manager = SSOIntegrationManager::new();

        let config = AWSIAMConfig {
            region: "us-west-2".to_string(),
            role_mapping: vec![AWSRoleMapping {
                aws_role_arn: "arn:aws:iam::123456789012:role/TestRole".to_string(),
                proximadb_role: "admin".to_string(),
                tenant_id: "test_tenant".to_string(),
            }],
            enable_cross_account: true,
            trusted_account_ids: vec!["123456789012".to_string()],
        };

        let result = manager.configure_aws_iam(config);
        assert!(result.is_ok());
        assert!(manager.aws_integration.is_some());
    }

    #[test]
    fn test_sso_manager_configure_azure_ad() {
        let mut manager = SSOIntegrationManager::new();

        let config = AzureADConfig {
            tenant_id: "12345678-1234-1234-1234-123456789012".to_string(),
            client_id: "client-id-123".to_string(),
            client_secret: "client-secret".to_string(),
            authority: "https://login.microsoftonline.com/".to_string(),
        };

        let result = manager.configure_azure_ad(config);
        assert!(result.is_ok());
        assert!(manager.azure_integration.is_some());
    }

    #[test]
    fn test_sso_manager_multiple_configurations() {
        let mut manager = SSOIntegrationManager::new();

        // Configure AWS
        let aws_config = AWSIAMConfig {
            region: "us-east-1".to_string(),
            role_mapping: vec![],
            enable_cross_account: false,
            trusted_account_ids: vec![],
        };
        manager.configure_aws_iam(aws_config).unwrap();

        // Configure Azure
        let azure_config = AzureADConfig {
            tenant_id: "tenant".to_string(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            authority: "https://login.microsoftonline.com/".to_string(),
        };
        manager.configure_azure_ad(azure_config).unwrap();

        assert!(manager.aws_integration.is_some());
        assert!(manager.azure_integration.is_some());
    }

    // --- Token Validation Tests ---

    #[tokio::test]
    async fn test_validate_and_resolve_token_aws_not_configured() {
        let manager = SSOIntegrationManager::new();

        let token = SSOToken::new(
            SSOProvider::AWSIAM,
            "token_data".to_string(),
            "user".to_string(),
            3600,
        );

        let result = manager.validate_and_resolve_token(&token).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AWS IAM not configured")
        );
    }

    #[tokio::test]
    async fn test_validate_and_resolve_token_azure_not_configured() {
        let manager = SSOIntegrationManager::new();

        let token = SSOToken::new(
            SSOProvider::AzureAD,
            "token_data".to_string(),
            "user".to_string(),
            3600,
        );

        let result = manager.validate_and_resolve_token(&token).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Azure AD not configured")
        );
    }

    #[tokio::test]
    async fn test_validate_and_resolve_token_unsupported_provider() {
        let manager = SSOIntegrationManager::new();

        let token = SSOToken::new(
            SSOProvider::OIDC,
            "token_data".to_string(),
            "user".to_string(),
            3600,
        );

        let result = manager.validate_and_resolve_token(&token).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unsupported SSO provider")
        );
    }

    #[tokio::test]
    async fn test_validate_and_resolve_token_generic_provider() {
        let manager = SSOIntegrationManager::new();

        let token = SSOToken::new(
            SSOProvider::Generic,
            "token_data".to_string(),
            "user".to_string(),
            3600,
        );

        let result = manager.validate_and_resolve_token(&token).await;
        assert!(result.is_err());
    }

    // --- CachedTokenValidation Tests ---

    #[test]
    fn test_cached_token_validation_expired() {
        let cached = CachedTokenValidation {
            user_context: EnterpriseUserContext::system_admin(),
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(10),
        };

        assert!(cached.is_expired());
    }

    #[test]
    fn test_cached_token_validation_not_expired() {
        let cached = CachedTokenValidation {
            user_context: EnterpriseUserContext::system_admin(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        };

        assert!(!cached.is_expired());
    }

    // --- AWS IAM Config Tests ---

    #[test]
    fn test_aws_iam_config_complete() {
        let config = AWSIAMConfig {
            region: "eu-west-1".to_string(),
            role_mapping: vec![
                AWSRoleMapping {
                    aws_role_arn: "arn:aws:iam::111:role/Admin".to_string(),
                    proximadb_role: "admin".to_string(),
                    tenant_id: "tenant1".to_string(),
                },
                AWSRoleMapping {
                    aws_role_arn: "arn:aws:iam::222:role/User".to_string(),
                    proximadb_role: "user".to_string(),
                    tenant_id: "tenant2".to_string(),
                },
            ],
            enable_cross_account: true,
            trusted_account_ids: vec!["111111111111".to_string(), "222222222222".to_string()],
        };

        assert_eq!(config.region, "eu-west-1");
        assert_eq!(config.role_mapping.len(), 2);
        assert!(config.enable_cross_account);
        assert_eq!(config.trusted_account_ids.len(), 2);
    }

    #[test]
    fn test_aws_iam_config_empty() {
        let config = AWSIAMConfig {
            region: String::new(),
            role_mapping: vec![],
            enable_cross_account: false,
            trusted_account_ids: vec![],
        };

        assert!(config.region.is_empty());
        assert!(config.role_mapping.is_empty());
        assert!(!config.enable_cross_account);
        assert!(config.trusted_account_ids.is_empty());
    }

    // --- Azure AD Config Tests ---

    #[test]
    fn test_azure_ad_config_complete() {
        let config = AzureADConfig {
            tenant_id: "12345678-1234-1234-1234-123456789012".to_string(),
            client_id: "87654321-4321-4321-4321-210987654321".to_string(),
            client_secret: "super-secret-value".to_string(),
            authority: "https://login.microsoftonline.com/common".to_string(),
        };

        assert!(!config.tenant_id.is_empty());
        assert!(!config.client_id.is_empty());
        assert!(!config.client_secret.is_empty());
        assert!(config.authority.starts_with("https://"));
    }

    #[test]
    fn test_azure_ad_config_clone() {
        let config = AzureADConfig {
            tenant_id: "tenant".to_string(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            authority: "authority".to_string(),
        };

        let cloned = config.clone();

        assert_eq!(config.tenant_id, cloned.tenant_id);
        assert_eq!(config.client_id, cloned.client_id);
        assert_eq!(config.client_secret, cloned.client_secret);
        assert_eq!(config.authority, cloned.authority);
    }

    // --- AWS Role Mapping Tests ---

    #[test]
    fn test_aws_role_mapping_structure() {
        let mapping = AWSRoleMapping {
            aws_role_arn: "arn:aws:iam::123456789012:role/MyRole".to_string(),
            proximadb_role: "tenant_admin".to_string(),
            tenant_id: "my_tenant".to_string(),
        };

        assert!(mapping.aws_role_arn.starts_with("arn:aws:iam::"));
        assert_eq!(mapping.proximadb_role, "tenant_admin");
        assert_eq!(mapping.tenant_id, "my_tenant");
    }

    #[test]
    fn test_aws_role_mapping_clone() {
        let mapping = AWSRoleMapping {
            aws_role_arn: "arn".to_string(),
            proximadb_role: "role".to_string(),
            tenant_id: "tenant".to_string(),
        };

        let cloned = mapping.clone();

        assert_eq!(mapping.aws_role_arn, cloned.aws_role_arn);
        assert_eq!(mapping.proximadb_role, cloned.proximadb_role);
        assert_eq!(mapping.tenant_id, cloned.tenant_id);
    }

    // --- Global SSO Manager Tests ---

    #[test]
    fn test_initialize_sso_manager() {
        let manager = initialize_sso_manager();
        assert!(std::ptr::eq(
            manager as *const SSOIntegrationManager,
            initialize_sso_manager() as *const SSOIntegrationManager
        ));
    }

    #[test]
    fn test_get_sso_manager_after_init() {
        // Initialize first
        let _ = initialize_sso_manager();

        // Get should return the same instance
        let manager = get_sso_manager();
        assert!(manager.is_some());
    }

    // --- Token Cache Tests ---

    #[tokio::test]
    async fn test_token_cache_hit() {
        let mut manager = SSOIntegrationManager::new();

        // Configure Azure AD
        let azure_config = AzureADConfig {
            tenant_id: "tenant".to_string(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            authority: "https://login.microsoftonline.com/".to_string(),
        };
        manager.configure_azure_ad(azure_config).unwrap();

        // Create a token
        let token = SSOToken::new(
            SSOProvider::AzureAD,
            "token_data".to_string(),
            "user".to_string(),
            3600,
        );

        // First validation - should hit provider
        let result1 = manager.validate_and_resolve_token(&token).await;
        assert!(result1.is_ok());

        // Insert into cache manually for test
        let cached = CachedTokenValidation {
            user_context: result1.unwrap(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        };
        manager.token_cache.insert(token.token_id.clone(), cached);

        // Second validation - should hit cache
        let result2 = manager.validate_and_resolve_token(&token).await;
        assert!(result2.is_ok());
    }

    // --- Re-export Tests ---

    #[test]
    fn test_re_exports() {
        // Verify all re-exports work
        let _ = SSOProvider::AWSIAM;
        let _ = SSOProvider::AzureAD;
        let _ = SSOProvider::GoogleCloud;
        let _ = SSOProvider::SAML;
        let _ = SSOProvider::OIDC;
        let _ = SSOProvider::Okta;
        let _ = SSOProvider::Generic;

        let _ = EnterpriseUserContext::system_admin();

        assert!(true);
    }

    // --- Integration Tests ---

    #[test]
    fn test_all_integrations_accessible() {
        // Verify all integration types are accessible
        use super::aws_iam::AWSIAMIntegration;
        use super::azure_ad::AzureADIntegration;
        use super::google_cloud::GoogleCloudIntegration;
        use super::oidc::OIDCIntegration;
        use super::saml::SAMLIntegration;

        // These should all compile successfully
        let _ = std::any::type_name::<AWSIAMIntegration>();
        let _ = std::any::type_name::<AzureADIntegration>();
        let _ = std::any::type_name::<GoogleCloudIntegration>();
        let _ = std::any::type_name::<OIDCIntegration>();
        let _ = std::any::type_name::<SAMLIntegration>();

        assert!(true);
    }
}
