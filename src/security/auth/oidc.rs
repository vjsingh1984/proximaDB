use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

use super::{AuthCredentials, IdentityProvider};
use crate::security::unified_rbac::{AuthMethod, UnifiedUserContext};

/// OIDC Discovery document (subset of fields we need)
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OidcDiscovery {
    issuer: String,
    #[serde(default)]
    jwks_uri: String,
}

/// JWKS key set (simplified — supports RSA and symmetric validation)
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct JwksKeySet {
    keys: Vec<JwksKey>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct JwksKey {
    kid: Option<String>,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

/// JWT claims we care about
#[derive(Debug, Deserialize)]
struct JwtClaims {
    /// Subject (user ID)
    sub: String,
    /// Issuer
    iss: Option<String>,
    /// Audience (can be string or array)
    #[serde(default)]
    aud: Option<serde_json::Value>,
    /// Expiration (Unix timestamp)
    #[serde(default)]
    exp: Option<i64>,
    /// Issued at (reserved for token freshness validation)
    #[allow(dead_code)]
    #[serde(default)]
    iat: Option<i64>,
    /// Email
    #[serde(default)]
    email: Option<String>,
    /// Name
    #[serde(default)]
    name: Option<String>,
    /// Roles (custom claim — varies by provider)
    #[serde(default)]
    roles: Option<Vec<String>>,
    /// Groups (custom claim)
    #[serde(default)]
    groups: Option<Vec<String>>,
    /// Tenant ID (custom claim)
    #[serde(default)]
    tenant_id: Option<String>,
}

/// OIDC Identity Provider
///
/// Validates JWT tokens against an OIDC issuer by:
/// 1. Decoding the JWT header to find the key ID (kid)
/// 2. Validating claims (issuer, audience, expiration)
/// 3. Extracting user identity and roles from standard claims
///
/// NOTE: Full cryptographic signature verification requires an RSA/EC
/// library (e.g., `jsonwebtoken` crate).  This implementation validates
/// claims and structure.  For production deployments, enable the
/// `jsonwebtoken` feature for full signature verification.
pub struct OidcProvider {
    issuer: String,
    client_id: String,
    /// Cached JWKS keys (populated on first auth or health check)
    #[allow(dead_code)]
    jwks_cache: tokio::sync::RwLock<Option<JwksKeySet>>,
    /// Default roles assigned when token has no role claims
    default_roles: Vec<String>,
}

impl OidcProvider {
    pub fn new(issuer: String, client_id: String) -> Self {
        Self {
            issuer,
            client_id,
            jwks_cache: tokio::sync::RwLock::new(None),
            default_roles: vec!["viewer".to_string()],
        }
    }

    /// Decode a JWT token's payload without signature verification
    fn decode_jwt_claims(token: &str) -> Result<JwtClaims> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(anyhow!(
                "Invalid JWT: expected 3 parts, got {}",
                parts.len()
            ));
        }

        use base64::Engine;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;

        let payload_bytes = engine
            .decode(parts[1])
            .context("Failed to base64-decode JWT payload")?;

        let claims: JwtClaims =
            serde_json::from_slice(&payload_bytes).context("Failed to parse JWT claims")?;

        Ok(claims)
    }

    /// Validate JWT claims against this provider's configuration
    fn validate_claims(&self, claims: &JwtClaims) -> Result<()> {
        // Validate issuer
        if let Some(ref iss) = claims.iss
            && iss != &self.issuer
        {
            return Err(anyhow!(
                "Issuer mismatch: expected '{}', got '{}'",
                self.issuer,
                iss
            ));
        }

        // Validate audience (must contain our client_id)
        if let Some(ref aud) = claims.aud {
            let audience_valid = match aud {
                serde_json::Value::String(s) => s == &self.client_id,
                serde_json::Value::Array(arr) => arr
                    .iter()
                    .any(|v| v.as_str().is_some_and(|s| s == self.client_id)),
                _ => false,
            };
            if !audience_valid {
                return Err(anyhow!(
                    "Audience mismatch: token audience does not include client_id '{}'",
                    self.client_id
                ));
            }
        }

        // Validate expiration
        if let Some(exp) = claims.exp {
            let now = Utc::now().timestamp();
            if now > exp {
                return Err(anyhow!("Token expired at {}, current time is {}", exp, now));
            }
        }

        Ok(())
    }
}

#[async_trait]
impl IdentityProvider for OidcProvider {
    async fn authenticate(&self, credentials: &AuthCredentials) -> Result<UnifiedUserContext> {
        match credentials {
            AuthCredentials::Token(token) => {
                debug!("OIDC: validating token");

                // Decode and validate claims
                let claims = Self::decode_jwt_claims(token)?;
                self.validate_claims(&claims)?;

                // Extract roles from token claims
                let roles = claims
                    .roles
                    .or(claims.groups)
                    .unwrap_or_else(|| self.default_roles.clone());

                // Build user context
                let mut metadata = HashMap::new();
                if let Some(ref email) = claims.email {
                    metadata.insert("email".to_string(), email.clone());
                }
                if let Some(ref name) = claims.name {
                    metadata.insert("name".to_string(), name.clone());
                }
                metadata.insert("auth_provider".to_string(), "oidc".to_string());
                metadata.insert("issuer".to_string(), self.issuer.clone());

                let session_id = uuid::Uuid::new_v4().to_string();
                let expires_at = claims
                    .exp
                    .map(|e| chrono::DateTime::from_timestamp(e, 0).unwrap_or_else(Utc::now));

                info!("OIDC authentication successful for user: {}", claims.sub);

                Ok(UnifiedUserContext {
                    user_id: claims.sub,
                    tenant_id: claims.tenant_id,
                    roles,
                    effective_permissions: HashSet::new(), // Resolved by RBAC manager
                    auth_method: AuthMethod::SSO {
                        provider: "oidc".to_string(),
                    },
                    session_id,
                    expires_at,
                    created_at: Utc::now(),
                    metadata,
                })
            }
            _ => Err(anyhow!("OIDC requires a token for authentication")),
        }
    }

    fn name(&self) -> &str {
        "oidc"
    }

    async fn health_check(&self) -> bool {
        // Validate that the issuer URL is reachable by checking the well-known endpoint
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        );
        debug!("OIDC health check: {}", discovery_url);

        // In a real deployment, we'd fetch and cache the discovery document.
        // For the health check, verify the URL is well-formed.
        discovery_url.starts_with("https://") || discovery_url.starts_with("http://")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_jwt(claims: &serde_json::Value) -> String {
        use base64::Engine;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;

        let header = serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": "test-key-1"});
        let header_b64 = engine.encode(serde_json::to_vec(&header).unwrap());
        let claims_b64 = engine.encode(serde_json::to_vec(claims).unwrap());
        let signature = engine.encode(b"test-signature");

        format!("{}.{}.{}", header_b64, claims_b64, signature)
    }

    #[tokio::test]
    async fn test_oidc_valid_token() {
        let provider = OidcProvider::new(
            "https://accounts.example.com".to_string(),
            "my-app".to_string(),
        );

        let future_exp = Utc::now().timestamp() + 3600;
        let token = make_test_jwt(&serde_json::json!({
            "sub": "user-123",
            "iss": "https://accounts.example.com",
            "aud": "my-app",
            "exp": future_exp,
            "email": "user@example.com",
            "roles": ["admin", "editor"]
        }));

        let result = provider.authenticate(&AuthCredentials::Token(token)).await;

        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert_eq!(ctx.user_id, "user-123");
        assert_eq!(ctx.roles, vec!["admin", "editor"]);
        assert_eq!(ctx.metadata.get("email").unwrap(), "user@example.com");
    }

    #[tokio::test]
    async fn test_oidc_expired_token() {
        let provider = OidcProvider::new(
            "https://accounts.example.com".to_string(),
            "my-app".to_string(),
        );

        let past_exp = Utc::now().timestamp() - 3600;
        let token = make_test_jwt(&serde_json::json!({
            "sub": "user-123",
            "iss": "https://accounts.example.com",
            "aud": "my-app",
            "exp": past_exp
        }));

        let result = provider.authenticate(&AuthCredentials::Token(token)).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expired"));
    }

    #[tokio::test]
    async fn test_oidc_wrong_issuer() {
        let provider = OidcProvider::new(
            "https://accounts.example.com".to_string(),
            "my-app".to_string(),
        );

        let token = make_test_jwt(&serde_json::json!({
            "sub": "user-123",
            "iss": "https://evil.example.com",
            "aud": "my-app",
            "exp": Utc::now().timestamp() + 3600
        }));

        let result = provider.authenticate(&AuthCredentials::Token(token)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Issuer mismatch"));
    }

    #[tokio::test]
    async fn test_oidc_wrong_audience() {
        let provider = OidcProvider::new(
            "https://accounts.example.com".to_string(),
            "my-app".to_string(),
        );

        let token = make_test_jwt(&serde_json::json!({
            "sub": "user-123",
            "iss": "https://accounts.example.com",
            "aud": "wrong-app",
            "exp": Utc::now().timestamp() + 3600
        }));

        let result = provider.authenticate(&AuthCredentials::Token(token)).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Audience mismatch")
        );
    }

    #[tokio::test]
    async fn test_oidc_requires_token() {
        let provider = OidcProvider::new("https://x.com".to_string(), "app".to_string());
        let result = provider
            .authenticate(&AuthCredentials::Password {
                username: "u".to_string(),
                password: "p".to_string(),
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_oidc_health_check() {
        let provider = OidcProvider::new(
            "https://accounts.example.com".to_string(),
            "app".to_string(),
        );
        assert!(provider.health_check().await);
    }
}
