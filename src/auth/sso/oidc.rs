//! OpenID Connect (OIDC) provider integration for ProximaDB SSO
//!
//! Provides SSO authentication using OpenID Connect standard
//! Compatible with providers like Auth0, Keycloak, Okta, and custom OIDC implementations.

use super::types::{EnterpriseUserContext, ProviderUserContext, SecurityClearance};
use anyhow::{Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tracing::debug;
use uuid::Uuid;

/// OpenID Connect provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OIDCConfig {
    /// OIDC provider discovery URL (e.g., https://provider.com/.well-known/openid_configuration)
    pub discovery_url: String,

    /// OAuth2 client ID
    pub client_id: String,

    /// OAuth2 client secret
    pub client_secret: String,

    /// Redirect URI for OAuth2 flow
    pub redirect_uri: String,

    /// OAuth2 scopes to request
    pub scopes: Vec<String>,

    /// OIDC claims mappings
    pub claims_mappings: OIDCClaimsMappings,

    /// JWT signature validation keys
    pub jwks_uri: Option<String>,

    /// Allowed OIDC issuers
    pub allowed_issuers: Vec<String>,

    /// Maximum ID token age in minutes
    pub max_token_age_minutes: u64,

    /// Default tenant for OIDC users
    pub default_tenant_id: String,

    /// Role mapping from OIDC claims
    pub role_mapping: HashMap<String, Vec<String>>,

    /// Additional OAuth2 parameters
    pub additional_params: HashMap<String, String>,
}

/// OIDC claims mappings for user context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OIDCClaimsMappings {
    /// Claim name for user ID (typically "sub")
    pub user_id_claim: String,

    /// Claim name for user email
    pub email_claim: String,

    /// Claim name for display name
    pub name_claim: String,

    /// Claim name for groups/roles
    pub groups_claim: Option<String>,

    /// Claim name for tenant/organization
    pub tenant_claim: Option<String>,

    /// Custom claim mappings
    pub custom_claims: HashMap<String, String>,
}

impl Default for OIDCConfig {
    fn default() -> Self {
        Self {
            discovery_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: "https://proximadb.example.com/auth/oidc/callback".to_string(),
            scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
            ],
            claims_mappings: OIDCClaimsMappings::default(),
            jwks_uri: None,
            allowed_issuers: vec![],
            max_token_age_minutes: 10,
            default_tenant_id: "oidc_users".to_string(),
            role_mapping: HashMap::new(),
            additional_params: HashMap::new(),
        }
    }
}

impl Default for OIDCClaimsMappings {
    fn default() -> Self {
        Self {
            user_id_claim: "sub".to_string(),
            email_claim: "email".to_string(),
            name_claim: "name".to_string(),
            groups_claim: Some("groups".to_string()),
            tenant_claim: Some("org".to_string()),
            custom_claims: HashMap::new(),
        }
    }
}

/// OpenID Connect integration
#[derive(Debug)]
pub struct OIDCIntegration {
    config: OIDCConfig,
    // Note: In a real implementation, this would include:
    // oidc_client: CoreClient,
    // jwks_client: JwksClient,
    // discovery_document: ProviderMetadata,
}

impl OIDCIntegration {
    /// Create new OIDC integration
    pub fn new(config: OIDCConfig) -> Result<Self> {
        // Validate configuration
        if config.discovery_url.is_empty() {
            return Err(anyhow!("OIDC discovery URL is required"));
        }

        if config.client_id.is_empty() {
            return Err(anyhow!("OIDC client ID is required"));
        }

        if config.client_secret.is_empty() {
            return Err(anyhow!("OIDC client secret is required"));
        }

        Ok(Self { config })
    }

    /// Validate OIDC ID token and resolve user context
    pub async fn validate_id_token(&self, id_token: &str) -> Result<EnterpriseUserContext> {
        // In a real implementation, this would:
        // 1. Fetch JWKS from provider
        // 2. Validate JWT signature using provider's public keys
        // 3. Validate JWT claims (issuer, audience, expiration)
        // 4. Extract user claims from ID token
        // 5. Map claims to ProximaDB user context
        // 6. Apply role mappings based on groups claim

        debug!(
            "Validating OIDC ID token: {}",
            &id_token[..std::cmp::min(20, id_token.len())]
        );

        // Placeholder implementation
        self.simulate_oidc_validation(id_token).await
    }

    /// Generate OAuth2 authorization URL
    pub fn generate_auth_url(&self, state: Option<&str>) -> Result<String> {
        // In a real implementation, this would:
        // 1. Fetch discovery document from provider
        // 2. Build OAuth2 authorization URL with proper parameters
        // 3. Include PKCE challenge if supported

        let default_state = Uuid::new_v4().to_string();
        let state_param = state.unwrap_or(&default_state);
        let scopes = self.config.scopes.join(" ");

        // Placeholder authorization URL
        let auth_url = format!(
            "{}?client_id={}&redirect_uri={}&scope={}&response_type=code&state={}",
            "https://provider.example.com/auth", // Would come from discovery document
            urlencoding::encode(&self.config.client_id),
            urlencoding::encode(&self.config.redirect_uri),
            urlencoding::encode(&scopes),
            urlencoding::encode(state_param)
        );

        Ok(auth_url)
    }

    /// Exchange authorization code for tokens
    pub async fn exchange_code_for_tokens(
        &self,
        code: &str,
        _state: Option<&str>,
    ) -> Result<OIDCTokenResponse> {
        // In a real implementation, this would:
        // 1. POST to token endpoint with authorization code
        // 2. Validate state parameter if provided
        // 3. Return access token, ID token, and refresh token

        debug!(
            "Exchanging authorization code: {}",
            &code[..std::cmp::min(10, code.len())]
        );

        // Placeholder token response
        Ok(OIDCTokenResponse {
            access_token: format!("access_{}", Uuid::new_v4()),
            id_token: format!("id_{}", Uuid::new_v4()),
            refresh_token: Some(format!("refresh_{}", Uuid::new_v4())),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            scope: self.config.scopes.join(" "),
        })
    }

    /// Simulate OIDC validation (placeholder)
    async fn simulate_oidc_validation(&self, id_token: &str) -> Result<EnterpriseUserContext> {
        if id_token.is_empty() {
            return Err(anyhow!("Empty OIDC ID token"));
        }

        // Simulate ID token claims
        let user_id = format!("oidc_user_{}", Uuid::new_v4());
        let email = format!("{}@example.com", user_id);

        Ok(EnterpriseUserContext {
            user_id: user_id.clone(),
            email: email.clone(),
            display_name: format!("OIDC User {}", user_id),
            tenant_id: self.config.default_tenant_id.clone(),
            organization_id: "oidc_org".to_string(),
            roles: vec!["oidc_user".to_string()],
            permissions: HashSet::new(),
            security_clearance: SecurityClearance::Internal,
            department: None,
            cost_center: None,
            session_id: Uuid::new_v4().to_string(),
            login_timestamp: Utc::now(),
            last_activity: Utc::now(),
            provider_context: ProviderUserContext::Generic {
                provider_user_id: user_id,
                attributes: {
                    let mut attrs = HashMap::new();
                    attrs.insert("provider".to_string(), "oidc".to_string());
                    attrs.insert("client_id".to_string(), self.config.client_id.clone());
                    attrs
                },
            },
        })
    }

    /// Extract claims from ID token
    #[allow(dead_code)]
    fn extract_id_token_claims(
        &self,
        _id_token: &str,
    ) -> Result<HashMap<String, serde_json::Value>> {
        // Placeholder for JWT parsing and claims extraction
        // Real implementation would decode and validate JWT
        Ok(HashMap::new())
    }
}

/// OIDC token response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OIDCTokenResponse {
    /// OAuth2 access token for resource access.
    pub access_token: String,
    /// OIDC ID token containing user identity claims.
    pub id_token: String,
    /// Refresh token for obtaining new access tokens.
    pub refresh_token: Option<String>,
    /// Token type (typically "Bearer").
    pub token_type: String,
    /// Time in seconds until the access token expires.
    pub expires_in: u64,
    /// Granted OAuth2 scopes.
    pub scope: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> OIDCConfig {
        let mut role_mapping = HashMap::new();
        role_mapping.insert("admins".to_string(), vec!["admin".to_string()]);
        role_mapping.insert("users".to_string(), vec!["user".to_string()]);

        OIDCConfig {
            discovery_url: "https://provider.example.com/.well-known/openid_configuration"
                .to_string(),
            client_id: "test-client-id".to_string(),
            client_secret: "test-client-secret".to_string(),
            redirect_uri: "https://proximadb.test.com/auth/oidc/callback".to_string(),
            scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
                "groups".to_string(),
            ],
            claims_mappings: OIDCClaimsMappings::default(),
            jwks_uri: Some("https://provider.example.com/.well-known/jwks.json".to_string()),
            allowed_issuers: vec!["https://provider.example.com".to_string()],
            max_token_age_minutes: 10,
            default_tenant_id: "oidc_tenant".to_string(),
            role_mapping,
            additional_params: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_oidc_integration_creation() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config);
        assert!(integration.is_ok());
    }

    #[tokio::test]
    async fn test_auth_url_generation() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let auth_url = integration.generate_auth_url(Some("test-state"));
        assert!(auth_url.is_ok());

        let url = auth_url.expect("Failed to generate auth URL");
        assert!(url.contains("client_id=test-client-id"));
        assert!(url.contains("scope=openid%20profile%20email%20groups"));
        assert!(url.contains("state=test-state"));
    }

    #[tokio::test]
    async fn test_code_exchange() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let token_response = integration
            .exchange_code_for_tokens("test-code", Some("test-state"))
            .await;
        assert!(token_response.is_ok());

        let tokens = token_response.expect("Failed to exchange code for tokens");
        assert!(tokens.access_token.starts_with("access_"));
        assert!(tokens.id_token.starts_with("id_"));
        assert_eq!(tokens.token_type, "Bearer");
    }

    #[tokio::test]
    async fn test_id_token_validation() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        // Test with non-empty ID token
        let result = integration.validate_id_token("test-id-token").await;
        assert!(result.is_ok());

        let user_context = result.expect("Failed to validate ID token");
        assert!(matches!(
            user_context.provider_context,
            ProviderUserContext::Generic { .. }
        ));
        assert_eq!(user_context.tenant_id, "oidc_tenant");
        assert!(user_context.roles.contains(&"oidc_user".to_string()));

        // Test with empty token
        let empty_result = integration.validate_id_token("").await;
        assert!(empty_result.is_err());
    }

    // ========================== Additional Tests for Coverage ==========================

    // --- Configuration Validation Tests ---

    #[test]
    fn test_oidc_config_missing_discovery_url() {
        let mut config = create_test_config();
        config.discovery_url = String::new();
        let result = OIDCIntegration::new(config);
        assert!(result.is_err());
        let err_msg = format!(
            "{}",
            result.expect_err("Expected error for missing discovery URL")
        );
        assert!(err_msg.contains("discovery URL is required"));
    }

    #[test]
    fn test_oidc_config_missing_client_id() {
        let mut config = create_test_config();
        config.client_id = String::new();
        let result = OIDCIntegration::new(config);
        assert!(result.is_err());
        let err_msg = format!(
            "{}",
            result.expect_err("Expected error for missing client ID")
        );
        assert!(err_msg.contains("client ID is required"));
    }

    #[test]
    fn test_oidc_config_missing_client_secret() {
        let mut config = create_test_config();
        config.client_secret = String::new();
        let result = OIDCIntegration::new(config);
        assert!(result.is_err());
        let err_msg = format!(
            "{}",
            result.expect_err("Expected error for missing client secret")
        );
        assert!(err_msg.contains("client secret is required"));
    }

    // --- Default Configuration Tests ---

    #[test]
    fn test_oidc_config_default() {
        let config = OIDCConfig::default();
        assert!(config.discovery_url.is_empty());
        assert!(config.client_id.is_empty());
        assert!(config.client_secret.is_empty());
        assert!(!config.redirect_uri.is_empty());
        assert_eq!(config.scopes.len(), 3);
        assert!(config.scopes.contains(&"openid".to_string()));
        assert!(config.scopes.contains(&"profile".to_string()));
        assert!(config.scopes.contains(&"email".to_string()));
        assert!(config.allowed_issuers.is_empty());
        assert_eq!(config.max_token_age_minutes, 10);
        assert_eq!(config.default_tenant_id, "oidc_users");
    }

    #[test]
    fn test_oidc_claims_mappings_default() {
        let mappings = OIDCClaimsMappings::default();
        assert_eq!(mappings.user_id_claim, "sub");
        assert_eq!(mappings.email_claim, "email");
        assert_eq!(mappings.name_claim, "name");
        assert_eq!(mappings.groups_claim, Some("groups".to_string()));
        assert_eq!(mappings.tenant_claim, Some("org".to_string()));
        assert!(mappings.custom_claims.is_empty());
    }

    // --- Auth URL Generation Tests ---

    #[tokio::test]
    async fn test_auth_url_generation_without_state() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let auth_url = integration.generate_auth_url(None);
        assert!(auth_url.is_ok());

        let url = auth_url.expect("Failed to generate auth URL");
        assert!(url.contains("client_id=test-client-id"));
        assert!(url.contains("response_type=code"));
        // When no state is provided, a UUID is generated
        assert!(url.contains("state="));
    }

    #[tokio::test]
    async fn test_auth_url_contains_redirect_uri() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let auth_url = integration
            .generate_auth_url(Some("state123"))
            .expect("Failed to generate auth URL");
        assert!(auth_url.contains("redirect_uri="));
        // URL encoded redirect URI
        assert!(auth_url.contains("proximadb.test.com"));
    }

    #[tokio::test]
    async fn test_auth_url_special_characters_in_state() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let auth_url = integration.generate_auth_url(Some("state=with&special/chars"));
        assert!(auth_url.is_ok());
        let url = auth_url.expect("Failed to generate auth URL");
        // State should be URL encoded
        assert!(url.contains("state="));
    }

    // --- Token Exchange Tests ---

    #[tokio::test]
    async fn test_code_exchange_without_state() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let token_response = integration
            .exchange_code_for_tokens("auth-code-123", None)
            .await;
        assert!(token_response.is_ok());

        let tokens = token_response.expect("Failed to exchange code for tokens");
        assert!(!tokens.access_token.is_empty());
        assert!(!tokens.id_token.is_empty());
        assert!(tokens.refresh_token.is_some());
        assert_eq!(tokens.expires_in, 3600);
    }

    #[tokio::test]
    async fn test_code_exchange_short_code() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        // Short authorization code
        let token_response = integration
            .exchange_code_for_tokens("abc", Some("state"))
            .await;
        assert!(token_response.is_ok());
    }

    // --- Token Response Tests ---

    #[test]
    fn test_token_response_serialization() {
        let token_response = OIDCTokenResponse {
            access_token: "access_12345".to_string(),
            id_token: "id_67890".to_string(),
            refresh_token: Some("refresh_abcde".to_string()),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            scope: "openid profile email".to_string(),
        };

        let json =
            serde_json::to_string(&token_response).expect("Failed to serialize token response");
        assert!(json.contains("access_12345"));
        assert!(json.contains("id_67890"));
        assert!(json.contains("refresh_abcde"));
        assert!(json.contains("Bearer"));

        let deserialized: OIDCTokenResponse =
            serde_json::from_str(&json).expect("Failed to deserialize token response");
        assert_eq!(deserialized.access_token, token_response.access_token);
        assert_eq!(deserialized.id_token, token_response.id_token);
        assert_eq!(deserialized.refresh_token, token_response.refresh_token);
        assert_eq!(deserialized.token_type, token_response.token_type);
        assert_eq!(deserialized.expires_in, token_response.expires_in);
        assert_eq!(deserialized.scope, token_response.scope);
    }

    #[test]
    fn test_token_response_without_refresh_token() {
        let token_response = OIDCTokenResponse {
            access_token: "access_token".to_string(),
            id_token: "id_token".to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            expires_in: 1800,
            scope: "openid".to_string(),
        };

        let json =
            serde_json::to_string(&token_response).expect("Failed to serialize token response");
        let deserialized: OIDCTokenResponse =
            serde_json::from_str(&json).expect("Failed to deserialize token response");
        assert!(deserialized.refresh_token.is_none());
    }

    // --- ID Token Validation Tests ---

    #[tokio::test]
    async fn test_id_token_validation_user_context_fields() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let result = integration
            .validate_id_token("valid-id-token")
            .await
            .expect("Failed to validate ID token");

        // Verify user context fields are properly populated
        assert!(!result.user_id.is_empty());
        assert!(!result.email.is_empty());
        assert!(result.email.contains("@"));
        assert!(!result.display_name.is_empty());
        assert_eq!(result.tenant_id, "oidc_tenant");
        assert_eq!(result.organization_id, "oidc_org");
        assert_eq!(result.security_clearance, SecurityClearance::Internal);
        assert!(!result.session_id.is_empty());
    }

    #[tokio::test]
    async fn test_id_token_validation_provider_context() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let result = integration
            .validate_id_token("test-token")
            .await
            .expect("Failed to validate ID token");

        match &result.provider_context {
            ProviderUserContext::Generic {
                provider_user_id,
                attributes,
            } => {
                assert!(!provider_user_id.is_empty());
                assert!(attributes.contains_key("provider"));
                assert_eq!(attributes.get("provider"), Some(&"oidc".to_string()));
                assert!(attributes.contains_key("client_id"));
                assert_eq!(
                    attributes.get("client_id"),
                    Some(&"test-client-id".to_string())
                );
            }
            _ => panic!("Expected Generic provider context"),
        }
    }

    #[tokio::test]
    async fn test_id_token_validation_with_whitespace_token() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        // Whitespace-only token is technically not empty
        let result = integration.validate_id_token("   ").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_id_token_validation_long_token() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        let long_token = "a".repeat(10000);
        let result = integration.validate_id_token(&long_token).await;
        assert!(result.is_ok());
    }

    // --- Claims Extraction Tests ---

    #[test]
    fn test_extract_id_token_claims() {
        let config = create_test_config();
        let integration = OIDCIntegration::new(config).expect("Failed to create OIDC integration");

        // Currently returns empty HashMap (placeholder)
        let claims = integration
            .extract_id_token_claims("test-token")
            .expect("Failed to extract claims");
        assert!(claims.is_empty());
    }

    // --- Config Serialization Tests ---

    #[test]
    fn test_oidc_config_serialization() {
        let config = create_test_config();
        let json = serde_json::to_string(&config).expect("Failed to serialize config");

        assert!(json.contains("discovery_url"));
        assert!(json.contains("client_id"));
        assert!(json.contains("client_secret"));
        assert!(json.contains("scopes"));
        assert!(json.contains("claims_mappings"));

        let deserialized: OIDCConfig =
            serde_json::from_str(&json).expect("Failed to deserialize config");
        assert_eq!(deserialized.discovery_url, config.discovery_url);
        assert_eq!(deserialized.client_id, config.client_id);
        assert_eq!(deserialized.scopes, config.scopes);
        assert_eq!(
            deserialized.max_token_age_minutes,
            config.max_token_age_minutes
        );
    }

    #[test]
    fn test_oidc_claims_mappings_serialization() {
        let mut mappings = OIDCClaimsMappings::default();
        mappings
            .custom_claims
            .insert("custom_attr".to_string(), "custom_value".to_string());

        let json = serde_json::to_string(&mappings).expect("Failed to serialize mappings");
        let deserialized: OIDCClaimsMappings =
            serde_json::from_str(&json).expect("Failed to deserialize mappings");

        assert_eq!(deserialized.user_id_claim, mappings.user_id_claim);
        assert_eq!(deserialized.email_claim, mappings.email_claim);
        assert_eq!(
            deserialized.custom_claims.get("custom_attr"),
            Some(&"custom_value".to_string())
        );
    }

    // --- Role Mapping Configuration Tests ---

    #[test]
    fn test_oidc_config_with_multiple_role_mappings() {
        let mut role_mapping = HashMap::new();
        role_mapping.insert(
            "super_admins".to_string(),
            vec!["admin".to_string(), "superuser".to_string()],
        );
        role_mapping.insert(
            "developers".to_string(),
            vec!["developer".to_string(), "reader".to_string()],
        );
        role_mapping.insert("viewers".to_string(), vec!["reader".to_string()]);

        let config = OIDCConfig {
            discovery_url: "https://provider.example.com".to_string(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            role_mapping,
            ..OIDCConfig::default()
        };

        assert_eq!(config.role_mapping.len(), 3);
        assert_eq!(
            config
                .role_mapping
                .get("super_admins")
                .expect("super_admins mapping missing")
                .len(),
            2
        );
        assert_eq!(
            config
                .role_mapping
                .get("developers")
                .expect("developers mapping missing")
                .len(),
            2
        );
        assert_eq!(
            config
                .role_mapping
                .get("viewers")
                .expect("viewers mapping missing")
                .len(),
            1
        );
    }

    // --- Additional Parameters Tests ---

    #[test]
    fn test_oidc_config_with_additional_params() {
        let mut additional_params = HashMap::new();
        additional_params.insert("prompt".to_string(), "consent".to_string());
        additional_params.insert("login_hint".to_string(), "user@example.com".to_string());

        let config = OIDCConfig {
            discovery_url: "https://provider.example.com".to_string(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            additional_params,
            ..OIDCConfig::default()
        };

        assert_eq!(
            config.additional_params.get("prompt"),
            Some(&"consent".to_string())
        );
        assert_eq!(
            config.additional_params.get("login_hint"),
            Some(&"user@example.com".to_string())
        );
    }

    // --- Allowed Issuers Tests ---

    #[test]
    fn test_oidc_config_multiple_allowed_issuers() {
        let config = OIDCConfig {
            discovery_url: "https://provider.example.com".to_string(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            allowed_issuers: vec![
                "https://issuer1.example.com".to_string(),
                "https://issuer2.example.com".to_string(),
                "https://issuer3.example.com".to_string(),
            ],
            ..OIDCConfig::default()
        };

        assert_eq!(config.allowed_issuers.len(), 3);
    }

    // --- Custom Claims Mappings Tests ---

    #[test]
    fn test_custom_claims_mappings() {
        let mut custom_claims = HashMap::new();
        custom_claims.insert("department".to_string(), "dept".to_string());
        custom_claims.insert("employee_id".to_string(), "emp_id".to_string());

        let mappings = OIDCClaimsMappings {
            user_id_claim: "sub".to_string(),
            email_claim: "email".to_string(),
            name_claim: "name".to_string(),
            groups_claim: Some("groups".to_string()),
            tenant_claim: None,
            custom_claims,
        };

        assert_eq!(mappings.custom_claims.len(), 2);
        assert!(mappings.tenant_claim.is_none());
    }
}
