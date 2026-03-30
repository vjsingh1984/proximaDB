use anyhow::Result;
use async_trait::async_trait;
use crate::security::unified_rbac::UnifiedUserContext;

pub mod oidc;
pub mod ldap;

/// Identity Provider trait for external authentication sources
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// Authenticate a user with credentials or token
    async fn authenticate(&self, credentials: &AuthCredentials) -> Result<UnifiedUserContext>;
    
    /// Get provider name
    fn name(&self) -> &str;
    
    /// Check if provider is healthy
    async fn health_check(&self) -> bool;
}

/// Authentication credentials for various providers
pub enum AuthCredentials {
    /// OIDC/OAuth2 token
    Token(String),
    /// Username and password (LDAP, Basic Auth)
    Password {
        username: String,
        password: String,
    },
    /// Client certificate (mTLS already handled, but could be integrated here)
    Certificate(Vec<u8>),
}
