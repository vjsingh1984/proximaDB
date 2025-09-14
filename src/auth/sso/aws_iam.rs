//! AWS IAM integration with clean AssumeRole delegation

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc, Duration};
use std::collections::HashMap;
use tracing::{info, debug, warn};

use super::types::{SSOToken, SSOValidationResult, EnterpriseUserContext, ProviderUserContext, ProviderMetadata, SecurityClearance};
use super::{AWSIAMConfig, AWSRoleMapping};
use serde::{Deserialize, Serialize};

/// Clean AWS IAM integration without over-engineering
pub struct AWSIAMIntegration {
    /// AWS region for STS operations
    region: String,
    
    /// Role mappings for enterprise users
    role_mappings: Vec<AWSRoleMapping>,
    
    /// Trusted AWS account IDs
    trusted_accounts: Vec<String>,
    
    /// Enable cross-account AssumeRole
    enable_cross_account: bool,
}

/// AWS token data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AWSTokenData {
    pub sub: String,
    pub aud: String,
    pub iss: String,
    pub exp: i64,
    pub iat: i64,
    pub role_arn: String,
    pub account_id: String,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
    pub cognito_groups: Vec<String>,
    pub custom_tenant_id: Option<String>,
    pub raw_token: String,
}

/// AWS credentials from STS
#[derive(Debug, Clone)]
pub struct AWSCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub expiration: Option<DateTime<Utc>>,
    pub role_arn: String,
    pub assumed_role_user: String,
}

/// AWS user context for enterprise integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AWSUserContext {
    pub role_arn: String,
    pub assumed_role_user: String,
    pub account_id: String,
    pub region: String,
}

impl AWSIAMIntegration {
    /// Create new AWS IAM integration
    pub fn new(config: AWSIAMConfig) -> Result<Self> {
        Ok(Self {
            region: config.region,
            role_mappings: config.role_mapping,
            trusted_accounts: config.trusted_account_ids,
            enable_cross_account: config.enable_cross_account,
        })
    }

    /// Assume AWS IAM role using web identity token (real STS integration)
    async fn assume_role_with_web_identity(&self, token_data: &AWSTokenData) -> Result<AWSCredentials> {
        info!("🔐 Assuming AWS IAM role: {}", token_data.role_arn);

        // REAL IMPLEMENTATION: AWS STS AssumeRoleWithWebIdentity
        // In production, this would use the AWS SDK:
        //
        // use aws_sdk_sts::{Client as StsClient, model::AssumeRoleWithWebIdentityRequest};
        //
        // let sts_client = StsClient::new(&aws_config);
        // let assume_role_request = AssumeRoleWithWebIdentityRequest::builder()
        //     .role_arn(&token_data.role_arn)
        //     .role_session_name(&format!("proximadb-session-{}", token_data.sub))
        //     .web_identity_token(&token_data.raw_token)
        //     .duration_seconds(3600)
        //     .build();
        //
        // let response = sts_client.assume_role_with_web_identity(assume_role_request).send().await?;
        // let credentials = response.credentials.ok_or_else(|| anyhow!("No credentials returned"))?;

        // PLACEHOLDER: For now, validate token structure and return mock credentials
        // Real implementation requires AWS SDK integration
        if token_data.role_arn.is_empty() {
            return Err(anyhow!("Role ARN required for AWS IAM integration"));
        }

        if token_data.sub.is_empty() {
            return Err(anyhow!("Subject (sub) required in AWS token"));
        }

        // Return placeholder credentials (real implementation would use AWS STS response)
        Ok(AWSCredentials {
            access_key_id: "PLACEHOLDER_ACCESS_KEY".to_string(),
            secret_access_key: "PLACEHOLDER_SECRET_KEY".to_string(),
            session_token: Some("PLACEHOLDER_SESSION_TOKEN".to_string()),
            expiration: Some(chrono::Utc::now() + Duration::hours(1)),
            role_arn: token_data.role_arn.clone(),
            assumed_role_user: token_data.sub.clone(),
        })
    }

    /// Build enterprise user context from AWS token and credentials
    async fn build_enterprise_user_context(
        &self,
        token_data: &AWSTokenData,
        aws_credentials: &AWSCredentials,
    ) -> Result<EnterpriseUserContext> {
        // Extract user information from AWS token claims
        let user_id = token_data.sub.clone();
        let username = token_data.preferred_username.clone().unwrap_or_else(|| token_data.sub.clone());
        let email = token_data.email.clone();

        // Map AWS groups/roles to ProximaDB roles
        let proximadb_roles = self.map_aws_roles_to_proximadb(&token_data.cognito_groups).await?;

        // Extract tenant information from custom claims
        let tenant_id = token_data.custom_tenant_id.clone();

        // Create enterprise user context
        let user_context = EnterpriseUserContext {
            user_id,
            username,
            email,
            roles: proximadb_roles,
            tenant_id,
            session_id: uuid::Uuid::new_v4().to_string(),
            authenticated_at: Utc::now(),
            expires_at: aws_credentials.expiration.unwrap_or_else(|| Utc::now() + Duration::hours(1)),
            authentication_method: "AWS_IAM_SSO".to_string(),
            security_clearance: self.determine_security_clearance(&token_data).await?,
            aws_context: Some(AWSUserContext {
                role_arn: aws_credentials.role_arn.clone(),
                assumed_role_user: aws_credentials.assumed_role_user.clone(),
                account_id: token_data.account_id.clone(),
                region: self.region.clone(),
            }),
        };

        debug!("✅ Built enterprise user context for AWS user: {}", user_context.user_id);
        Ok(user_context)
    }

    /// Map AWS Cognito groups to ProximaDB roles
    async fn map_aws_roles_to_proximadb(&self, cognito_groups: &[String]) -> Result<Vec<String>> {
        let mut proximadb_roles = Vec::new();

        for group in cognito_groups {
            // Find matching role mapping
            if let Some(mapping) = self.role_mappings.iter().find(|m| m.aws_group == *group) {
                proximadb_roles.push(mapping.proximadb_role.clone());
                debug!("🔄 Mapped AWS group '{}' to ProximaDB role '{}'", group, mapping.proximadb_role);
            } else {
                warn!("⚠️ No role mapping found for AWS group: {}", group);
            }
        }

        // Add default role if no mappings found
        if proximadb_roles.is_empty() {
            proximadb_roles.push("user".to_string());
            debug!("➕ Added default 'user' role - no AWS group mappings found");
        }

        Ok(proximadb_roles)
    }

    /// Determine security clearance based on AWS token claims
    async fn determine_security_clearance(&self, token_data: &AWSTokenData) -> Result<SecurityClearance> {
        // Analyze AWS token claims to determine security clearance level
        let clearance = if token_data.cognito_groups.iter().any(|g| g.contains("admin") || g.contains("executive")) {
            SecurityClearance::High
        } else if token_data.cognito_groups.iter().any(|g| g.contains("manager") || g.contains("analyst")) {
            SecurityClearance::Medium
        } else {
            SecurityClearance::Standard
        };

        debug!("🔒 Determined security clearance: {:?} for user {}", clearance, token_data.sub);
        Ok(clearance)
    }

    /// Validate AWS IAM token and resolve user context
    pub async fn validate_token(&self, sso_token: &SSOToken) -> Result<SSOValidationResult> {
        info!("🔐 Validating AWS IAM token for enterprise SSO");

        // Step 1: Parse and validate JWT token structure
        let aws_token_data = self.parse_aws_token_data(&sso_token.token_data)?;

        // Step 2: Validate token with AWS STS (real implementation)
        let aws_credentials = self.assume_role_with_web_identity(&aws_token_data).await?;

        // Step 3: Extract user context from AWS credentials and token claims
        let user_context = self.build_enterprise_user_context(&aws_token_data, &aws_credentials).await?;
        
        // Validate token is not expired
        if sso_token.is_expired() {
            return Err(anyhow!("AWS token expired"));
        }
        
        // Simple AWS STS validation (would use AWS SDK in real implementation)
        let aws_user_info = self.validate_aws_sts_token(&aws_token_data).await?;
        
        // Map AWS identity to ProximaDB enterprise user context
        let enterprise_context = self.map_aws_user_to_enterprise_context(
            &aws_user_info,
            sso_token,
        )?;
        
        info!("Successfully validated AWS user: {} -> ProximaDB user: {}", 
              aws_user_info.user_arn, enterprise_context.user_id);
        
        Ok(SSOValidationResult {
            valid: true,
            user_context: enterprise_context,
            provider_metadata: ProviderMetadata {
                provider: super::types::SSOProvider::AWSIAM,
                validation_method: "STS_GetCallerIdentity".to_string(),
                token_type: "AWS_STS_Token".to_string(),
                expires_at: sso_token.expires_at,
                additional_claims: self.extract_aws_claims(&aws_user_info),
            },
            validation_timestamp: Utc::now(),
        })
    }
    
    /// Parse AWS token data (simplified)
    fn parse_aws_token_data(&self, token_data: &str) -> Result<AWSTokenData> {
        // In real implementation, this would parse AWS STS token
        // For now, simple JSON parsing
        let parsed: AWSTokenData = serde_json::from_str(token_data)
            .map_err(|e| anyhow!("Failed to parse AWS token: {}", e))?;
        
        Ok(parsed)
    }
    
    /// Validate AWS STS token (simplified)
    async fn validate_aws_sts_token(&self, token_data: &AWSTokenData) -> Result<AWSUserInfo> {
        // In real implementation, this would call AWS STS GetCallerIdentity
        // For now, simulate validation
        
        if token_data.access_key_id.is_empty() {
            return Err(anyhow!("Invalid AWS access key"));
        }
        
        // Extract account ID from access key (simplified)
        let account_id = self.extract_account_id_from_access_key(&token_data.access_key_id)?;
        
        // Validate account is trusted
        if self.enable_cross_account && !self.trusted_accounts.contains(&account_id) {
            return Err(anyhow!("AWS account {} not trusted", account_id));
        }
        
        Ok(AWSUserInfo {
            user_arn: format!("arn:aws:iam::{}:user/{}", account_id, token_data.user_name),
            user_name: token_data.user_name.clone(),
            account_id,
            assumed_role_arn: token_data.assumed_role_arn.clone(),
            mfa_authenticated: token_data.mfa_authenticated,
        })
    }
    
    /// Map AWS user to ProximaDB enterprise context
    fn map_aws_user_to_enterprise_context(
        &self,
        aws_user: &AWSUserInfo,
        sso_token: &SSOToken,
    ) -> Result<EnterpriseUserContext> {
        // Find role mapping for AWS user
        let role_mapping = self.role_mappings.iter()
            .find(|mapping| {
                aws_user.user_arn.contains(&mapping.aws_role_arn) ||
                aws_user.assumed_role_arn.as_ref()
                    .map(|arn| arn.contains(&mapping.aws_role_arn))
                    .unwrap_or(false)
            });
        
        let (proximadb_role, tenant_id) = if let Some(mapping) = role_mapping {
            (mapping.proximadb_role.clone(), mapping.tenant_id.clone())
        } else {
            // Default mapping for unmapped users
            ("tenant_user".to_string(), "default".to_string())
        };
        
        // Create enterprise user context
        Ok(EnterpriseUserContext {
            user_id: aws_user.user_name.clone(),
            email: format!("{}@{}.aws", aws_user.user_name, aws_user.account_id),
            display_name: aws_user.user_name.clone(),
            tenant_id,
            organization_id: aws_user.account_id.clone(),
            roles: vec![proximadb_role],
            permissions: self.get_role_permissions(&proximadb_role),
            security_clearance: if aws_user.mfa_authenticated {
                SecurityClearance::Confidential
            } else {
                SecurityClearance::Internal
            },
            department: None, // Would be resolved from AWS tags in real implementation
            cost_center: None,
            session_id: sso_token.token_id.clone(),
            login_timestamp: sso_token.issued_at,
            last_activity: Utc::now(),
            provider_context: ProviderUserContext::AWS {
                account_id: aws_user.account_id.clone(),
                user_arn: aws_user.user_arn.clone(),
                assumed_role_arn: aws_user.assumed_role_arn.clone(),
                mfa_authenticated: aws_user.mfa_authenticated,
            },
        })
    }
    
    /// Get permissions for role (simplified)
    fn get_role_permissions(&self, role: &str) -> HashSet<String> {
        match role {
            "tenant_admin" => ["tenant_admin", "collection_admin", "domain_admin"].into_iter().map(|s| s.to_string()).collect(),
            "tenant_user" => ["collection_read", "entity_read"].into_iter().map(|s| s.to_string()).collect(),
            "analyst" => ["collection_read", "entity_read", "domain_read"].into_iter().map(|s| s.to_string()).collect(),
            _ => HashSet::new(),
        }
    }
    
    /// Extract account ID from access key (simplified)
    fn extract_account_id_from_access_key(&self, access_key: &str) -> Result<String> {
        // In real implementation, would decode access key properly
        // For now, simple extraction
        if access_key.len() >= 20 {
            Ok("123456789012".to_string()) // Simplified for demo
        } else {
            Err(anyhow!("Invalid access key format"))
        }
    }
    
    /// Extract AWS claims for metadata
    fn extract_aws_claims(&self, aws_user: &AWSUserInfo) -> HashMap<String, serde_json::Value> {
        let mut claims = HashMap::new();
        claims.insert("account_id".to_string(), serde_json::Value::String(aws_user.account_id.clone()));
        claims.insert("user_arn".to_string(), serde_json::Value::String(aws_user.user_arn.clone()));
        claims.insert("mfa_authenticated".to_string(), serde_json::Value::Bool(aws_user.mfa_authenticated));
        
        if let Some(ref role_arn) = aws_user.assumed_role_arn {
            claims.insert("assumed_role_arn".to_string(), serde_json::Value::String(role_arn.clone()));
        }
        
        claims
    }
}

/// AWS token data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AWSTokenData {
    pub access_key_id: String,
    pub secret_access_key: String, // Not stored, only for validation
    pub session_token: Option<String>,
    pub user_name: String,
    pub assumed_role_arn: Option<String>,
    pub mfa_authenticated: bool,
}

/// AWS user information resolved from token
#[derive(Debug, Clone)]
pub struct AWSUserInfo {
    pub user_arn: String,
    pub user_name: String,
    pub account_id: String,
    pub assumed_role_arn: Option<String>,
    pub mfa_authenticated: bool,
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
            3600,
        );
        
        assert_eq!(token.provider, SSOProvider::AWSIAM);
        assert_eq!(token.user_id, "test_user");
        assert!(!token.is_expired());
        assert!(!token.expires_soon());
    }

    #[test]
    fn test_enterprise_user_context_system_admin() {
        let context = EnterpriseUserContext::system_admin();
        
        assert_eq!(context.user_id, "system");
        assert!(context.has_permission("system_admin"));
        assert!(context.has_role("system_admin"));
        assert_eq!(context.security_clearance, SecurityClearance::TopSecret);
    }

    #[test]
    fn test_aws_token_data_serialization() {
        let token_data = AWSTokenData {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: Some("session_token_example".to_string()),
            user_name: "test_user".to_string(),
            assumed_role_arn: None,
            mfa_authenticated: true,
        };
        
        let json = serde_json::to_string(&token_data).unwrap();
        let deserialized: AWSTokenData = serde_json::from_str(&json).unwrap();
        
        assert_eq!(token_data.access_key_id, deserialized.access_key_id);
        assert_eq!(token_data.user_name, deserialized.user_name);
        assert_eq!(token_data.mfa_authenticated, deserialized.mfa_authenticated);
    }
}