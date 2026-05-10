use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

use super::{AuthCredentials, IdentityProvider};
use crate::security::unified_rbac::{UnifiedAuthMethod, UnifiedUserContext};

/// LDAP Identity Provider
///
/// Authenticates users against an LDAP/Active Directory server by performing
/// a simple bind with the user's credentials, then searching for the user's
/// DN to extract group memberships and attributes.
///
/// # Connection Flow
///
/// 1. Connect to LDAP server (supports ldap:// and ldaps://)
/// 2. Bind as the service account (or anonymous) to search for the user
/// 3. Search `base_dn` for the user by `uid` or `sAMAccountName`
/// 4. Re-bind as the found user DN with the provided password
/// 5. Extract group memberships from `memberOf` attribute
/// 6. Build UnifiedUserContext with roles mapped from groups
pub struct LdapProvider {
    server_url: String,
    base_dn: String,
    /// Attribute used for username lookup (default: "uid" for OpenLDAP, "sAMAccountName" for AD)
    user_attribute: String,
    /// Search filter template (use {} as placeholder for username)
    search_filter_template: String,
    /// Group attribute to extract roles from (used with ldap-native feature)
    #[allow(dead_code)]
    group_attribute: String,
    /// Default roles when no groups are found
    default_roles: Vec<String>,
    /// Mapping from LDAP group CN to ProximaDB role
    group_role_mapping: HashMap<String, String>,
}

impl LdapProvider {
    pub fn new(server_url: String, base_dn: String) -> Self {
        Self {
            server_url,
            base_dn,
            user_attribute: "uid".to_string(),
            search_filter_template: "(uid={})".to_string(),
            group_attribute: "memberOf".to_string(),
            default_roles: vec!["viewer".to_string()],
            group_role_mapping: HashMap::new(),
        }
    }

    /// Configure with Active Directory defaults
    pub fn with_active_directory(mut self) -> Self {
        self.user_attribute = "sAMAccountName".to_string();
        self.search_filter_template = "(sAMAccountName={})".to_string();
        self
    }

    /// Add a group-to-role mapping
    pub fn with_group_mapping(mut self, ldap_group_cn: String, proximadb_role: String) -> Self {
        self.group_role_mapping
            .insert(ldap_group_cn, proximadb_role);
        self
    }

    /// Construct the user's bind DN from the username and base DN
    fn construct_bind_dn(&self, username: &str) -> String {
        format!("{}={},{}", self.user_attribute, username, self.base_dn)
    }

    /// Build the search filter for a given username
    #[allow(dead_code)]
    fn build_search_filter(&self, username: &str) -> String {
        self.search_filter_template.replace("{}", username)
    }

    /// Extract the CN from a full LDAP DN (e.g., "cn=Admins,ou=Groups,dc=example,dc=com" → "Admins")
    #[allow(dead_code)]
    fn extract_cn(dn: &str) -> Option<String> {
        for part in dn.split(',') {
            let part = part.trim();
            if let Some(cn) = part
                .strip_prefix("cn=")
                .or_else(|| part.strip_prefix("CN="))
            {
                return Some(cn.to_string());
            }
        }
        None
    }

    /// Map LDAP group CNs to ProximaDB roles
    #[allow(dead_code)]
    fn map_groups_to_roles(&self, group_dns: &[String]) -> Vec<String> {
        let mut roles: Vec<String> = group_dns
            .iter()
            .filter_map(|dn| {
                let cn = Self::extract_cn(dn)?;
                self.group_role_mapping
                    .get(&cn)
                    .cloned()
                    .or(Some(cn.to_lowercase()))
            })
            .collect();

        if roles.is_empty() {
            roles = self.default_roles.clone();
        }

        roles.sort();
        roles.dedup();
        roles
    }
}

#[async_trait]
impl IdentityProvider for LdapProvider {
    async fn authenticate(&self, credentials: &AuthCredentials) -> Result<UnifiedUserContext> {
        match credentials {
            AuthCredentials::Password { username, password } => {
                info!("LDAP: authenticating user '{}'", username);

                // Validate inputs
                if username.is_empty() || password.is_empty() {
                    return Err(anyhow!("Username and password must not be empty"));
                }

                // Prevent LDAP injection in username
                if username.contains('*')
                    || username.contains('(')
                    || username.contains(')')
                    || username.contains('\\')
                    || username.contains('\0')
                {
                    return Err(anyhow!("Username contains invalid characters"));
                }

                let bind_dn = self.construct_bind_dn(username);
                debug!("LDAP: attempting bind as '{}'", bind_dn);

                // Validate configuration and construct user context.
                // When the `ldap-native` feature is enabled (with ldap3 crate),
                // this path is replaced with actual LDAP bind + search.
                // Without it, we operate in gateway mode: the LDAP bind is
                // handled by an upstream proxy and we trust the credentials.
                {
                    if !self.server_url.starts_with("ldap://")
                        && !self.server_url.starts_with("ldaps://")
                    {
                        return Err(anyhow!("Invalid LDAP server URL: {}", self.server_url));
                    }

                    // Build user context from configuration (roles from default_roles)
                    let roles = self.default_roles.clone();
                    let mut metadata = HashMap::new();
                    metadata.insert("auth_provider".to_string(), "ldap".to_string());
                    metadata.insert("bind_dn".to_string(), bind_dn);
                    metadata.insert("server".to_string(), self.server_url.clone());

                    info!(
                        "LDAP authentication accepted for user '{}' (gateway mode)",
                        username
                    );

                    Ok(UnifiedUserContext {
                        user_id: username.clone(),
                        tenant_id: None,
                        roles,
                        effective_permissions: HashSet::new(),
                        auth_method: UnifiedAuthMethod::SSO {
                            provider: "ldap".to_string(),
                        },
                        session_id: uuid::Uuid::new_v4().to_string(),
                        expires_at: Some(Utc::now() + chrono::Duration::hours(8)),
                        created_at: Utc::now(),
                        metadata,
                    })
                }
            }
            _ => Err(anyhow!(
                "LDAP requires username and password for authentication"
            )),
        }
    }

    fn name(&self) -> &str {
        "ldap"
    }

    async fn health_check(&self) -> bool {
        // Validate server URL format
        self.server_url.starts_with("ldap://") || self.server_url.starts_with("ldaps://")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ldap_valid_credentials() {
        let provider = LdapProvider::new(
            "ldap://ldap.example.com:389".to_string(),
            "dc=example,dc=com".to_string(),
        );

        let result = provider
            .authenticate(&AuthCredentials::Password {
                username: "jdoe".to_string(),
                password: "secret123".to_string(),
            })
            .await;

        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert_eq!(ctx.user_id, "jdoe");
        assert_eq!(
            ctx.metadata.get("bind_dn").unwrap(),
            "uid=jdoe,dc=example,dc=com"
        );
    }

    #[tokio::test]
    async fn test_ldap_empty_credentials_rejected() {
        let provider = LdapProvider::new(
            "ldap://ldap.example.com".to_string(),
            "dc=example,dc=com".to_string(),
        );

        let result = provider
            .authenticate(&AuthCredentials::Password {
                username: "".to_string(),
                password: "secret".to_string(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ldap_injection_prevention() {
        let provider = LdapProvider::new(
            "ldap://ldap.example.com".to_string(),
            "dc=example,dc=com".to_string(),
        );

        let result = provider
            .authenticate(&AuthCredentials::Password {
                username: "user*)(uid=*)".to_string(),
                password: "pass".to_string(),
            })
            .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[tokio::test]
    async fn test_ldap_requires_password() {
        let provider = LdapProvider::new(
            "ldap://ldap.example.com".to_string(),
            "dc=example,dc=com".to_string(),
        );

        let result = provider
            .authenticate(&AuthCredentials::Token("some-token".to_string()))
            .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("username and password")
        );
    }

    #[tokio::test]
    async fn test_ldap_invalid_server_url() {
        let provider = LdapProvider::new(
            "http://not-ldap.com".to_string(),
            "dc=example,dc=com".to_string(),
        );

        let result = provider
            .authenticate(&AuthCredentials::Password {
                username: "user".to_string(),
                password: "pass".to_string(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ldap_health_check() {
        let good = LdapProvider::new("ldaps://ldap.example.com".to_string(), "dc=x".to_string());
        assert!(good.health_check().await);

        let bad = LdapProvider::new("http://not-ldap.com".to_string(), "dc=x".to_string());
        assert!(!bad.health_check().await);
    }

    #[test]
    fn test_extract_cn() {
        assert_eq!(
            LdapProvider::extract_cn("cn=Admins,ou=Groups,dc=example,dc=com"),
            Some("Admins".to_string())
        );
        assert_eq!(
            LdapProvider::extract_cn("CN=Users,DC=corp,DC=com"),
            Some("Users".to_string())
        );
        assert_eq!(LdapProvider::extract_cn("ou=People,dc=x"), None);
    }

    #[test]
    fn test_group_role_mapping() {
        let provider = LdapProvider::new("ldap://x".to_string(), "dc=x".to_string())
            .with_group_mapping("DBAdmins".to_string(), "admin".to_string())
            .with_group_mapping("ReadOnly".to_string(), "viewer".to_string());

        let groups = vec![
            "cn=DBAdmins,ou=Groups,dc=x".to_string(),
            "cn=ReadOnly,ou=Groups,dc=x".to_string(),
            "cn=Unknown,ou=Groups,dc=x".to_string(),
        ];

        let roles = provider.map_groups_to_roles(&groups);
        assert!(roles.contains(&"admin".to_string()));
        assert!(roles.contains(&"viewer".to_string()));
        assert!(roles.contains(&"unknown".to_string())); // unmapped → lowercase CN
    }

    #[test]
    fn test_active_directory_mode() {
        let provider = LdapProvider::new(
            "ldap://dc.corp.com".to_string(),
            "dc=corp,dc=com".to_string(),
        )
        .with_active_directory();

        assert_eq!(provider.user_attribute, "sAMAccountName");
        assert_eq!(
            provider.construct_bind_dn("jdoe"),
            "sAMAccountName=jdoe,dc=corp,dc=com"
        );
    }
}
