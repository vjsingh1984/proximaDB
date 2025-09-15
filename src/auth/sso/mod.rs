//! SSO integration for enterprise identity providers

pub mod aws_iam;
pub mod azure_ad;
pub mod types;

pub use types::{SSOToken, SSOProvider, SSOValidationResult, EnterpriseUserContext};
pub use aws_iam::AWSIAMIntegration;
pub use azure_ad::AzureADIntegration;

use anyhow::Result;
use std::sync::Arc;

/// Clean SSO integration manager
pub struct SSOIntegrationManager {
    /// AWS IAM integration
    aws_integration: Option<Arc<AWSIAMIntegration>>,
    
    /// Azure AD integration
    azure_integration: Option<Arc<AzureADIntegration>>,
    
    /// Simple token cache for performance
    token_cache: Arc<dashmap::DashMap<String, CachedTokenValidation>>,
}

impl SSOIntegrationManager {
    /// Create new SSO manager
    pub fn new() -> Self {
        Self {
            aws_integration: None,
            azure_integration: None,
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
                let aws = self.aws_integration.as_ref()
                    .ok_or_else(|| anyhow::anyhow!("AWS IAM not configured"))?;
                aws.validate_token(sso_token).await?
            },
            SSOProvider::AzureAD => {
                let azure = self.azure_integration.as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Azure AD not configured"))?;
                azure.validate_token(sso_token).await?
            },
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
}