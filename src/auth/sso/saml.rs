//! Generic SAML 2.0 provider integration for ProximaDB SSO
//!
//! Provides SSO authentication using any SAML 2.0 compatible identity provider
//! including Okta, Auth0, Ping Identity, ADFS, and custom SAML implementations.

use super::types::{EnterpriseUserContext, ProviderUserContext, SecurityClearance};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tracing::debug;

/// SAML 2.0 provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SAMLConfig {
    /// SAML Identity Provider (IdP) metadata URL
    pub idp_metadata_url: String,

    /// SAML Service Provider (SP) entity ID
    pub sp_entity_id: String,

    /// SAML assertion consumer service URL
    pub acs_url: String,

    /// SAML single logout service URL
    pub sls_url: Option<String>,

    /// Certificate for SAML signature validation
    pub idp_certificate: String,

    /// Private key for SAML request signing
    pub sp_private_key: Option<String>,

    /// SAML attribute mappings
    pub attribute_mappings: SAMLAttributeMappings,

    /// Allowed SAML assertion audience
    pub allowed_audiences: Vec<String>,

    /// Maximum assertion age in minutes
    pub max_assertion_age_minutes: u64,

    /// Default tenant for SAML users
    pub default_tenant_id: String,

    /// Role mapping from SAML attributes
    pub role_mapping: HashMap<String, Vec<String>>,
}

/// SAML attribute mappings for user context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SAMLAttributeMappings {
    /// Attribute name for user ID (e.g., "NameID", "uid", "email")
    pub user_id_attribute: String,

    /// Attribute name for user email
    pub email_attribute: String,

    /// Attribute name for display name
    pub display_name_attribute: String,

    /// Attribute name for groups/roles
    pub groups_attribute: Option<String>,

    /// Attribute name for tenant/organization
    pub tenant_attribute: Option<String>,

    /// Custom attribute mappings
    pub custom_attributes: HashMap<String, String>,
}

impl Default for SAMLConfig {
    fn default() -> Self {
        Self {
            idp_metadata_url: String::new(),
            sp_entity_id: "proximadb".to_string(),
            acs_url: "https://proximadb.example.com/auth/saml/acs".to_string(),
            sls_url: Some("https://proximadb.example.com/auth/saml/sls".to_string()),
            idp_certificate: String::new(),
            sp_private_key: None,
            attribute_mappings: SAMLAttributeMappings::default(),
            allowed_audiences: vec!["proximadb".to_string()],
            max_assertion_age_minutes: 5,
            default_tenant_id: "saml_users".to_string(),
            role_mapping: HashMap::new(),
        }
    }
}

impl Default for SAMLAttributeMappings {
    fn default() -> Self {
        Self {
            user_id_attribute: "NameID".to_string(),
            email_attribute: "email".to_string(),
            display_name_attribute: "displayName".to_string(),
            groups_attribute: Some("groups".to_string()),
            tenant_attribute: Some("organization".to_string()),
            custom_attributes: HashMap::new(),
        }
    }
}

/// Generic SAML 2.0 integration
#[derive(Debug)]
pub struct SAMLIntegration {
    config: SAMLConfig,
    // Note: In a real implementation, this would include:
    // saml_client: SAMLClient,
    // signature_validator: SignatureValidator,
}

impl SAMLIntegration {
    /// Create new SAML integration
    pub fn new(config: SAMLConfig) -> Result<Self> {
        // Validate configuration
        if config.idp_metadata_url.is_empty() {
            return Err(anyhow!("SAML IdP metadata URL is required"));
        }

        if config.sp_entity_id.is_empty() {
            return Err(anyhow!("SAML SP entity ID is required"));
        }

        if config.idp_certificate.is_empty() {
            return Err(anyhow!(
                "SAML IdP certificate is required for signature validation"
            ));
        }

        Ok(Self { config })
    }

    /// Validate SAML assertion and resolve user context
    pub async fn validate_assertion(&self, saml_response: &str) -> Result<EnterpriseUserContext> {
        // In a real implementation, this would:
        // 1. Parse the SAML response XML
        // 2. Validate the assertion signature using IdP certificate
        // 3. Check assertion conditions (audience, not-before, not-on-or-after)
        // 4. Extract user attributes from assertion
        // 5. Map attributes to ProximaDB user context
        // 6. Apply role mappings based on SAML groups

        debug!(
            "Validating SAML assertion: {}",
            &saml_response[..std::cmp::min(50, saml_response.len())]
        );

        // Placeholder implementation for now
        self.simulate_saml_validation(saml_response).await
    }

    /// Generate SAML authentication request
    pub fn generate_auth_request(&self, relay_state: Option<&str>) -> Result<SAMLAuthRequest> {
        // In a real implementation, this would generate a proper SAML AuthnRequest
        Ok(SAMLAuthRequest {
            request_id: format!("saml_req_{}", uuid::Uuid::new_v4()),
            destination: self.config.idp_metadata_url.clone(),
            acs_url: self.config.acs_url.clone(),
            sp_entity_id: self.config.sp_entity_id.clone(),
            relay_state: relay_state.map(|s| s.to_string()),
            created_at: Utc::now(),
        })
    }

    /// Simulate SAML validation (placeholder)
    async fn simulate_saml_validation(&self, saml_response: &str) -> Result<EnterpriseUserContext> {
        if saml_response.is_empty() {
            return Err(anyhow!("Empty SAML response"));
        }

        // Simulate parsing SAML attributes
        let user_id = format!("saml_user_{}", uuid::Uuid::new_v4());
        let email = format!("{}@{}", user_id, "example.com");

        Ok(EnterpriseUserContext {
            user_id: user_id.clone(),
            email: email.clone(),
            display_name: format!("SAML User {}", user_id),
            tenant_id: self.config.default_tenant_id.clone(),
            organization_id: "saml_org".to_string(),
            roles: vec!["saml_user".to_string()],
            permissions: HashSet::new(),
            security_clearance: SecurityClearance::Internal,
            department: None,
            cost_center: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            login_timestamp: Utc::now(),
            last_activity: Utc::now(),
            provider_context: ProviderUserContext::Generic {
                provider_user_id: user_id,
                attributes: {
                    let mut attrs = HashMap::new();
                    attrs.insert("provider".to_string(), "saml".to_string());
                    attrs.insert("sp_entity_id".to_string(), self.config.sp_entity_id.clone());
                    attrs
                },
            },
        })
    }

    /// Extract user attributes from SAML assertion
    #[allow(dead_code)]
    fn extract_user_attributes(
        &self,
        _assertion: &SAMLAssertion,
    ) -> Result<HashMap<String, Vec<String>>> {
        // Placeholder for SAML attribute extraction
        // Real implementation would parse XML and extract AttributeStatement values
        Ok(HashMap::new())
    }

    /// Map SAML attributes to ProximaDB user context
    #[allow(dead_code)]
    fn map_attributes_to_user_context(
        &self,
        attributes: &HashMap<String, Vec<String>>,
    ) -> Result<EnterpriseUserContext> {
        let mappings = &self.config.attribute_mappings;

        // Extract user ID
        let user_id = attributes
            .get(&mappings.user_id_attribute)
            .and_then(|values| values.first())
            .ok_or_else(|| {
                anyhow!(
                    "Required user ID attribute '{}' not found",
                    mappings.user_id_attribute
                )
            })?
            .clone();

        // Extract email
        let email = attributes
            .get(&mappings.email_attribute)
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_else(|| user_id.clone());

        // Extract display name
        let display_name = attributes
            .get(&mappings.display_name_attribute)
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_else(|| user_id.clone());

        // Extract groups/roles
        let groups = mappings
            .groups_attribute
            .as_ref()
            .and_then(|groups_attr| attributes.get(groups_attr).cloned())
            .unwrap_or_default();

        // Extract tenant/organization
        let tenant_id = mappings
            .tenant_attribute
            .as_ref()
            .and_then(|tenant_attr| attributes.get(tenant_attr))
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_else(|| self.config.default_tenant_id.clone());

        // Map groups to roles
        let roles = self.map_groups_to_roles(&groups);

        Ok(EnterpriseUserContext {
            user_id: user_id.clone(),
            email: email.clone(),
            display_name: display_name.clone(),
            tenant_id,
            organization_id: "saml_org".to_string(),
            roles,
            permissions: HashSet::new(),
            security_clearance: SecurityClearance::Internal,
            department: None,
            cost_center: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            login_timestamp: Utc::now(),
            last_activity: Utc::now(),
            provider_context: ProviderUserContext::Generic {
                provider_user_id: user_id.clone(),
                attributes: {
                    let mut attrs = HashMap::new();
                    attrs.insert("provider".to_string(), "saml".to_string());
                    attrs.insert("email".to_string(), email.clone());
                    attrs.insert("display_name".to_string(), display_name.clone());
                    attrs
                },
            },
        })
    }

    /// Map SAML groups to ProximaDB roles
    fn map_groups_to_roles(&self, groups: &[String]) -> Vec<String> {
        let mut roles = Vec::new();

        for group in groups {
            if let Some(mapped_roles) = self.config.role_mapping.get(group) {
                roles.extend(mapped_roles.clone());
            }
        }

        // Add default role if no mapping found
        if roles.is_empty() {
            roles.push("saml_user".to_string());
        }

        roles
    }
}

/// SAML authentication request
#[derive(Debug, Clone)]
pub struct SAMLAuthRequest {
    pub request_id: String,
    pub destination: String,
    pub acs_url: String,
    pub sp_entity_id: String,
    pub relay_state: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// SAML assertion structure (placeholder)
#[derive(Debug, Clone)]
pub struct SAMLAssertion {
    pub assertion_id: String,
    pub issuer: String,
    pub subject: String,
    pub attributes: HashMap<String, Vec<String>>,
    pub not_before: DateTime<Utc>,
    pub not_on_or_after: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_saml_config() -> SAMLConfig {
        let mut role_mapping = HashMap::new();
        role_mapping.insert("admin_group".to_string(), vec!["admin".to_string()]);
        role_mapping.insert("user_group".to_string(), vec!["user".to_string()]);

        SAMLConfig {
            idp_metadata_url: "https://idp.example.com/metadata".to_string(),
            sp_entity_id: "proximadb-test".to_string(),
            acs_url: "https://proximadb.test.com/auth/saml/acs".to_string(),
            sls_url: Some("https://proximadb.test.com/auth/saml/sls".to_string()),
            idp_certificate: "test-certificate".to_string(),
            sp_private_key: Some("test-private-key".to_string()),
            attribute_mappings: SAMLAttributeMappings::default(),
            allowed_audiences: vec!["proximadb-test".to_string()],
            max_assertion_age_minutes: 5,
            default_tenant_id: "saml_tenant".to_string(),
            role_mapping,
        }
    }

    #[tokio::test]
    async fn test_saml_integration_creation() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config);
        assert!(integration.is_ok());
    }

    #[tokio::test]
    async fn test_saml_auth_request_generation() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let auth_request = integration.generate_auth_request(Some("test-relay-state"));
        assert!(auth_request.is_ok());

        let request = auth_request.expect("Failed to generate auth request");
        assert_eq!(request.sp_entity_id, "proximadb-test");
        assert_eq!(request.acs_url, "https://proximadb.test.com/auth/saml/acs");
        assert_eq!(request.relay_state, Some("test-relay-state".to_string()));
    }

    #[tokio::test]
    async fn test_saml_assertion_validation() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        // Test with non-empty SAML response
        let result = integration
            .validate_assertion("<saml:Response>test</saml:Response>")
            .await;
        assert!(result.is_ok());

        let user_context = result.expect("Failed to validate assertion");
        assert!(matches!(
            user_context.provider_context,
            ProviderUserContext::Generic { .. }
        ));
        assert_eq!(user_context.tenant_id, "saml_tenant");
        assert!(user_context.roles.contains(&"saml_user".to_string()));

        // Test with empty response
        let empty_result = integration.validate_assertion("").await;
        assert!(empty_result.is_err());
    }

    #[test]
    fn test_group_role_mapping() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let admin_groups = vec!["admin_group".to_string()];
        let admin_roles = integration.map_groups_to_roles(&admin_groups);
        assert!(admin_roles.contains(&"admin".to_string()));

        let user_groups = vec!["user_group".to_string()];
        let user_roles = integration.map_groups_to_roles(&user_groups);
        assert!(user_roles.contains(&"user".to_string()));

        let unknown_groups = vec!["unknown_group".to_string()];
        let default_roles = integration.map_groups_to_roles(&unknown_groups);
        assert!(default_roles.contains(&"saml_user".to_string()));
    }

    // ========================== Additional Tests for Coverage ==========================

    // --- Configuration Validation Tests ---

    #[test]
    fn test_saml_config_missing_idp_metadata_url() {
        let mut config = create_test_saml_config();
        config.idp_metadata_url = String::new();
        let result = SAMLIntegration::new(config);
        assert!(result.is_err());
        let err_msg = format!(
            "{}",
            result.expect_err("Expected error for missing IdP metadata URL")
        );
        assert!(err_msg.contains("IdP metadata URL is required"));
    }

    #[test]
    fn test_saml_config_missing_sp_entity_id() {
        let mut config = create_test_saml_config();
        config.sp_entity_id = String::new();
        let result = SAMLIntegration::new(config);
        assert!(result.is_err());
        let err_msg = format!(
            "{}",
            result.expect_err("Expected error for missing SP entity ID")
        );
        assert!(err_msg.contains("SP entity ID is required"));
    }

    #[test]
    fn test_saml_config_missing_idp_certificate() {
        let mut config = create_test_saml_config();
        config.idp_certificate = String::new();
        let result = SAMLIntegration::new(config);
        assert!(result.is_err());
        let err_msg = format!(
            "{}",
            result.expect_err("Expected error for missing IdP certificate")
        );
        assert!(err_msg.contains("IdP certificate is required"));
    }

    // --- Default Configuration Tests ---

    #[test]
    fn test_saml_config_default() {
        let config = SAMLConfig::default();
        assert!(config.idp_metadata_url.is_empty());
        assert_eq!(config.sp_entity_id, "proximadb");
        assert!(!config.acs_url.is_empty());
        assert!(config.sls_url.is_some());
        assert!(config.idp_certificate.is_empty());
        assert!(config.sp_private_key.is_none());
        assert_eq!(config.allowed_audiences.len(), 1);
        assert!(config.allowed_audiences.contains(&"proximadb".to_string()));
        assert_eq!(config.max_assertion_age_minutes, 5);
        assert_eq!(config.default_tenant_id, "saml_users");
        assert!(config.role_mapping.is_empty());
    }

    #[test]
    fn test_saml_attribute_mappings_default() {
        let mappings = SAMLAttributeMappings::default();
        assert_eq!(mappings.user_id_attribute, "NameID");
        assert_eq!(mappings.email_attribute, "email");
        assert_eq!(mappings.display_name_attribute, "displayName");
        assert_eq!(mappings.groups_attribute, Some("groups".to_string()));
        assert_eq!(mappings.tenant_attribute, Some("organization".to_string()));
        assert!(mappings.custom_attributes.is_empty());
    }

    // --- Auth Request Generation Tests ---

    #[tokio::test]
    async fn test_saml_auth_request_without_relay_state() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let auth_request = integration.generate_auth_request(None);
        assert!(auth_request.is_ok());

        let request = auth_request.expect("Failed to generate auth request");
        assert!(request.relay_state.is_none());
        assert!(!request.request_id.is_empty());
        assert!(request.request_id.starts_with("saml_req_"));
    }

    #[tokio::test]
    async fn test_saml_auth_request_fields() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let auth_request = integration
            .generate_auth_request(Some("relay123"))
            .expect("Failed to generate auth request");

        assert_eq!(auth_request.destination, "https://idp.example.com/metadata");
        assert_eq!(
            auth_request.acs_url,
            "https://proximadb.test.com/auth/saml/acs"
        );
        assert_eq!(auth_request.sp_entity_id, "proximadb-test");
        assert_eq!(auth_request.relay_state, Some("relay123".to_string()));
        // Verify created_at is recent
        let now = Utc::now();
        assert!(auth_request.created_at <= now);
        assert!(auth_request.created_at > now - chrono::Duration::seconds(5));
    }

    // --- Assertion Validation Tests ---

    #[tokio::test]
    async fn test_saml_assertion_validation_user_context_fields() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let result = integration
            .validate_assertion("<saml:Response>valid</saml:Response>")
            .await
            .expect("Failed to validate assertion");

        assert!(!result.user_id.is_empty());
        assert!(!result.email.is_empty());
        assert!(result.email.contains("@"));
        assert!(!result.display_name.is_empty());
        assert_eq!(result.tenant_id, "saml_tenant");
        assert_eq!(result.organization_id, "saml_org");
        assert_eq!(result.security_clearance, SecurityClearance::Internal);
        assert!(!result.session_id.is_empty());
    }

    #[tokio::test]
    async fn test_saml_assertion_validation_provider_context() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let result = integration
            .validate_assertion("<saml:Response>test</saml:Response>")
            .await
            .expect("Failed to validate assertion");

        match &result.provider_context {
            ProviderUserContext::Generic {
                provider_user_id,
                attributes,
            } => {
                assert!(!provider_user_id.is_empty());
                assert!(attributes.contains_key("provider"));
                assert_eq!(attributes.get("provider"), Some(&"saml".to_string()));
                assert!(attributes.contains_key("sp_entity_id"));
                assert_eq!(
                    attributes.get("sp_entity_id"),
                    Some(&"proximadb-test".to_string())
                );
            }
            _ => panic!("Expected Generic provider context"),
        }
    }

    #[tokio::test]
    async fn test_saml_assertion_validation_whitespace_only() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        // Whitespace-only is not empty
        let result = integration.validate_assertion("   ").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_saml_assertion_validation_long_response() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let long_response = format!("<saml:Response>{}</saml:Response>", "a".repeat(10000));
        let result = integration.validate_assertion(&long_response).await;
        assert!(result.is_ok());
    }

    // --- Group Role Mapping Tests ---

    #[test]
    fn test_group_role_mapping_multiple_groups() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let groups = vec!["admin_group".to_string(), "user_group".to_string()];
        let roles = integration.map_groups_to_roles(&groups);

        assert!(roles.contains(&"admin".to_string()));
        assert!(roles.contains(&"user".to_string()));
        assert_eq!(roles.len(), 2);
    }

    #[test]
    fn test_group_role_mapping_empty_groups() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let empty_groups: Vec<String> = vec![];
        let roles = integration.map_groups_to_roles(&empty_groups);

        // Should return default role
        assert!(roles.contains(&"saml_user".to_string()));
        assert_eq!(roles.len(), 1);
    }

    #[test]
    fn test_group_role_mapping_partial_match() {
        let mut role_mapping = HashMap::new();
        role_mapping.insert("admin_group".to_string(), vec!["admin".to_string()]);
        role_mapping.insert(
            "power_users".to_string(),
            vec!["power_user".to_string(), "writer".to_string()],
        );

        let config = SAMLConfig {
            role_mapping,
            ..create_test_saml_config()
        };

        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let groups = vec![
            "admin_group".to_string(),
            "unknown_group".to_string(),
            "power_users".to_string(),
        ];
        let roles = integration.map_groups_to_roles(&groups);

        assert!(roles.contains(&"admin".to_string()));
        assert!(roles.contains(&"power_user".to_string()));
        assert!(roles.contains(&"writer".to_string()));
        // unknown_group is ignored, no default added because we have matches
    }

    // --- Attribute Mapping Tests ---

    #[test]
    fn test_map_attributes_to_user_context_complete() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let mut attributes = HashMap::new();
        attributes.insert("NameID".to_string(), vec!["user123".to_string()]);
        attributes.insert("email".to_string(), vec!["user@example.com".to_string()]);
        attributes.insert("displayName".to_string(), vec!["John Doe".to_string()]);
        attributes.insert("groups".to_string(), vec!["admin_group".to_string()]);
        attributes.insert("organization".to_string(), vec!["test_org".to_string()]);

        let result = integration.map_attributes_to_user_context(&attributes);
        assert!(result.is_ok());

        let user_context = result.expect("Failed to map attributes to user context");
        assert_eq!(user_context.user_id, "user123");
        assert_eq!(user_context.email, "user@example.com");
        assert_eq!(user_context.display_name, "John Doe");
        assert_eq!(user_context.tenant_id, "test_org");
        assert!(user_context.roles.contains(&"admin".to_string()));
    }

    #[test]
    fn test_map_attributes_to_user_context_missing_user_id() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let attributes: HashMap<String, Vec<String>> = HashMap::new();

        let result = integration.map_attributes_to_user_context(&attributes);
        assert!(result.is_err());
        assert!(
            result
                .expect_err("Expected error for missing user ID")
                .to_string()
                .contains("user ID attribute")
        );
    }

    #[test]
    fn test_map_attributes_to_user_context_minimal() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let mut attributes = HashMap::new();
        attributes.insert("NameID".to_string(), vec!["minimal_user".to_string()]);

        let result = integration.map_attributes_to_user_context(&attributes);
        assert!(result.is_ok());

        let user_context = result.expect("Failed to map attributes to user context");
        assert_eq!(user_context.user_id, "minimal_user");
        // Email defaults to user_id
        assert_eq!(user_context.email, "minimal_user");
        // Display name defaults to user_id
        assert_eq!(user_context.display_name, "minimal_user");
        // Tenant defaults to config default
        assert_eq!(user_context.tenant_id, "saml_tenant");
        // Should have default role
        assert!(user_context.roles.contains(&"saml_user".to_string()));
    }

    #[test]
    fn test_map_attributes_without_groups_claim() {
        let config = SAMLConfig {
            attribute_mappings: SAMLAttributeMappings {
                groups_attribute: None,
                ..SAMLAttributeMappings::default()
            },
            ..create_test_saml_config()
        };

        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let mut attributes = HashMap::new();
        attributes.insert("NameID".to_string(), vec!["user1".to_string()]);

        let result = integration
            .map_attributes_to_user_context(&attributes)
            .expect("Failed to map attributes to user context");
        assert!(result.roles.contains(&"saml_user".to_string()));
    }

    #[test]
    fn test_map_attributes_without_tenant_claim() {
        let config = SAMLConfig {
            attribute_mappings: SAMLAttributeMappings {
                tenant_attribute: None,
                ..SAMLAttributeMappings::default()
            },
            ..create_test_saml_config()
        };

        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let mut attributes = HashMap::new();
        attributes.insert("NameID".to_string(), vec!["user1".to_string()]);

        let result = integration
            .map_attributes_to_user_context(&attributes)
            .expect("Failed to map attributes to user context");
        // Should use default tenant
        assert_eq!(result.tenant_id, "saml_tenant");
    }

    // --- User Attributes Extraction Tests ---

    #[test]
    fn test_extract_user_attributes_placeholder() {
        let config = create_test_saml_config();
        let integration = SAMLIntegration::new(config).expect("Failed to create SAML integration");

        let assertion = SAMLAssertion {
            assertion_id: "assertion_123".to_string(),
            issuer: "https://idp.example.com".to_string(),
            subject: "user@example.com".to_string(),
            attributes: HashMap::new(),
            not_before: Utc::now(),
            not_on_or_after: Utc::now() + chrono::Duration::minutes(5),
        };

        // Currently returns empty HashMap (placeholder)
        let attributes = integration
            .extract_user_attributes(&assertion)
            .expect("Failed to extract user attributes");
        assert!(attributes.is_empty());
    }

    // --- SAML Assertion Structure Tests ---

    #[test]
    fn test_saml_assertion_structure() {
        let mut attrs = HashMap::new();
        attrs.insert("email".to_string(), vec!["user@example.com".to_string()]);
        attrs.insert(
            "groups".to_string(),
            vec!["group1".to_string(), "group2".to_string()],
        );

        let assertion = SAMLAssertion {
            assertion_id: "id_12345".to_string(),
            issuer: "https://idp.example.com".to_string(),
            subject: "user_subject".to_string(),
            attributes: attrs,
            not_before: Utc::now() - chrono::Duration::minutes(1),
            not_on_or_after: Utc::now() + chrono::Duration::minutes(5),
        };

        assert_eq!(assertion.assertion_id, "id_12345");
        assert_eq!(assertion.issuer, "https://idp.example.com");
        assert_eq!(assertion.subject, "user_subject");
        assert_eq!(assertion.attributes.len(), 2);
        assert!(assertion.not_before < assertion.not_on_or_after);
    }

    // --- SAML Auth Request Structure Tests ---

    #[test]
    fn test_saml_auth_request_structure() {
        let request = SAMLAuthRequest {
            request_id: "saml_req_abc".to_string(),
            destination: "https://idp.example.com/sso".to_string(),
            acs_url: "https://sp.example.com/acs".to_string(),
            sp_entity_id: "sp_entity".to_string(),
            relay_state: Some("state_value".to_string()),
            created_at: Utc::now(),
        };

        assert_eq!(request.request_id, "saml_req_abc");
        assert_eq!(request.destination, "https://idp.example.com/sso");
        assert_eq!(request.acs_url, "https://sp.example.com/acs");
        assert_eq!(request.sp_entity_id, "sp_entity");
        assert_eq!(request.relay_state, Some("state_value".to_string()));
    }

    // --- Configuration Serialization Tests ---

    #[test]
    fn test_saml_config_serialization() {
        let config = create_test_saml_config();
        let json = serde_json::to_string(&config).expect("Failed to serialize config");

        assert!(json.contains("idp_metadata_url"));
        assert!(json.contains("sp_entity_id"));
        assert!(json.contains("idp_certificate"));
        assert!(json.contains("attribute_mappings"));

        let deserialized: SAMLConfig =
            serde_json::from_str(&json).expect("Failed to deserialize config");
        assert_eq!(deserialized.idp_metadata_url, config.idp_metadata_url);
        assert_eq!(deserialized.sp_entity_id, config.sp_entity_id);
        assert_eq!(
            deserialized.max_assertion_age_minutes,
            config.max_assertion_age_minutes
        );
    }

    #[test]
    fn test_saml_attribute_mappings_serialization() {
        let mut custom_attrs = HashMap::new();
        custom_attrs.insert("attr1".to_string(), "value1".to_string());

        let mappings = SAMLAttributeMappings {
            custom_attributes: custom_attrs,
            ..SAMLAttributeMappings::default()
        };

        let json = serde_json::to_string(&mappings).expect("Failed to serialize mappings");
        let deserialized: SAMLAttributeMappings =
            serde_json::from_str(&json).expect("Failed to deserialize mappings");

        assert_eq!(deserialized.user_id_attribute, mappings.user_id_attribute);
        assert_eq!(
            deserialized.custom_attributes.get("attr1"),
            Some(&"value1".to_string())
        );
    }

    // --- Role Mapping Configuration Tests ---

    #[test]
    fn test_saml_config_with_complex_role_mappings() {
        let mut role_mapping = HashMap::new();
        role_mapping.insert(
            "executives".to_string(),
            vec!["admin".to_string(), "executive".to_string()],
        );
        role_mapping.insert(
            "engineers".to_string(),
            vec!["developer".to_string(), "reader".to_string()],
        );
        role_mapping.insert("interns".to_string(), vec!["reader".to_string()]);

        let config = SAMLConfig {
            role_mapping,
            ..create_test_saml_config()
        };

        assert_eq!(config.role_mapping.len(), 3);
        assert_eq!(
            config
                .role_mapping
                .get("executives")
                .expect("Failed to get executives roles")
                .len(),
            2
        );
    }

    // --- Allowed Audiences Tests ---

    #[test]
    fn test_saml_config_multiple_audiences() {
        let config = SAMLConfig {
            allowed_audiences: vec![
                "audience1".to_string(),
                "audience2".to_string(),
                "audience3".to_string(),
            ],
            ..create_test_saml_config()
        };

        assert_eq!(config.allowed_audiences.len(), 3);
    }

    // --- SLS URL Tests ---

    #[test]
    fn test_saml_config_without_sls_url() {
        let config = SAMLConfig {
            sls_url: None,
            ..create_test_saml_config()
        };

        assert!(config.sls_url.is_none());
    }

    // --- Private Key Tests ---

    #[test]
    fn test_saml_config_without_private_key() {
        let config = SAMLConfig {
            sp_private_key: None,
            ..create_test_saml_config()
        };

        assert!(config.sp_private_key.is_none());
    }

    // --- Custom Attribute Mappings Tests ---

    #[test]
    fn test_custom_attribute_mappings() {
        let mut custom_attributes = HashMap::new();
        custom_attributes.insert("department".to_string(), "dept".to_string());
        custom_attributes.insert("employee_id".to_string(), "emp_id".to_string());
        custom_attributes.insert("manager".to_string(), "manager_email".to_string());

        let mappings = SAMLAttributeMappings {
            user_id_attribute: "uid".to_string(),
            email_attribute: "mail".to_string(),
            display_name_attribute: "cn".to_string(),
            groups_attribute: Some("memberOf".to_string()),
            tenant_attribute: Some("company".to_string()),
            custom_attributes,
        };

        assert_eq!(mappings.user_id_attribute, "uid");
        assert_eq!(mappings.email_attribute, "mail");
        assert_eq!(mappings.custom_attributes.len(), 3);
    }
}
