/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! JWT Token Management for ProximaDB Authentication

use crate::network::auth::{AuthError, JwtConfig};
use anyhow::Result;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// JWT claims structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,

    /// Issued at timestamp
    pub iat: i64,

    /// Expiration timestamp
    pub exp: i64,

    /// Not before timestamp
    pub nbf: i64,

    /// Issuer
    pub iss: String,

    /// Audience
    pub aud: String,

    /// JWT ID (unique identifier)
    pub jti: String,

    /// Tenant ID (optional)
    pub tenant_id: Option<String>,

    /// User roles
    pub roles: Vec<String>,

    /// Token type (access or refresh)
    pub typ: TokenType,
}

/// Token type for JWT claims
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TokenType {
    #[serde(rename = "access")]
    Access,
    #[serde(rename = "refresh")]
    Refresh,
}

/// JWT token pair (access + refresh)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
}

/// JWT service for token creation and verification
pub struct JwtService {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    header: Header,
    validation: Validation,
    config: JwtConfig,
    // In-memory blacklist for revoked tokens (in production, use Redis/DB)
    blacklist: tokio::sync::RwLock<HashSet<String>>,
}

impl JwtService {
    /// Create a new JWT service with configuration
    pub fn new(config: JwtConfig) -> Result<Self> {
        let secret = config
            .secret
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("JWT secret is required"))?;

        let algorithm = match config.algorithm {
            crate::network::auth::JwtAlgorithm::HS256 => Algorithm::HS256,
            crate::network::auth::JwtAlgorithm::HS384 => Algorithm::HS384,
            crate::network::auth::JwtAlgorithm::HS512 => Algorithm::HS512,
            crate::network::auth::JwtAlgorithm::RS256 => Algorithm::RS256,
            crate::network::auth::JwtAlgorithm::RS384 => Algorithm::RS384,
            crate::network::auth::JwtAlgorithm::RS512 => Algorithm::RS512,
        };

        let encoding_key = EncodingKey::from_secret(secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(secret.as_bytes());
        let header = Header::new(algorithm);

        let mut validation = Validation::new(algorithm);
        validation.set_issuer(&[config.issuer.clone()]);
        validation.set_audience(&[config.audience.clone()]);
        validation.validate_nbf = true;

        Ok(Self {
            encoding_key,
            decoding_key,
            header,
            validation,
            config,
            blacklist: tokio::sync::RwLock::new(HashSet::new()),
        })
    }

    /// Generate a token pair (access + refresh tokens)
    pub async fn generate_token_pair(
        &self,
        user_id: &str,
        tenant_id: Option<String>,
        roles: Vec<String>,
    ) -> Result<TokenPair, AuthError> {
        let now = chrono::Utc::now().timestamp();
        let jti = uuid::Uuid::new_v4().to_string();

        // Access token claims
        let access_claims = Claims {
            sub: user_id.to_string(),
            iat: now,
            exp: now + self.config.expiration_secs as i64,
            nbf: now,
            iss: self.config.issuer.clone(),
            aud: self.config.audience.clone(),
            jti: jti.clone(),
            tenant_id: tenant_id.clone(),
            roles: roles.clone(),
            typ: TokenType::Access,
        };

        // Refresh token claims (longer expiration, no roles)
        let refresh_claims = Claims {
            sub: user_id.to_string(),
            iat: now,
            exp: now + self.config.refresh_expiration_secs as i64,
            nbf: now,
            iss: self.config.issuer.clone(),
            aud: self.config.audience.clone(),
            jti: format!("{}-refresh", jti),
            tenant_id,
            roles: vec![], // Refresh tokens don't contain roles
            typ: TokenType::Refresh,
        };

        let access_token =
            encode(&self.header, &access_claims, &self.encoding_key).map_err(|e| {
                AuthError::InvalidToken(format!("Failed to encode access token: {}", e))
            })?;

        let refresh_token =
            encode(&self.header, &refresh_claims, &self.encoding_key).map_err(|e| {
                AuthError::InvalidToken(format!("Failed to encode refresh token: {}", e))
            })?;

        Ok(TokenPair {
            access_token,
            refresh_token,
            expires_in: self.config.expiration_secs as i64,
            token_type: "Bearer".to_string(),
        })
    }

    /// Verify and decode a JWT token
    pub async fn verify_token(&self, token: &str) -> Result<Claims, AuthError> {
        // Check if token is blacklisted
        let blacklist = self.blacklist.read().await;
        if blacklist.contains(token) {
            return Err(AuthError::InvalidToken(
                "Token has been revoked".to_string(),
            ));
        }
        drop(blacklist);

        let token_data =
            decode::<Claims>(token, &self.decoding_key, &self.validation).map_err(|e| {
                match e.kind() {
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::TokenExpired,
                    jsonwebtoken::errors::ErrorKind::InvalidToken => {
                        AuthError::InvalidToken("Token format is invalid".to_string())
                    }
                    jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                        AuthError::InvalidToken("Token signature is invalid".to_string())
                    }
                    _ => AuthError::InvalidToken(format!("Token validation failed: {}", e)),
                }
            })?;

        // Additional validation
        let now = chrono::Utc::now().timestamp();
        if token_data.claims.exp < now {
            return Err(AuthError::TokenExpired);
        }

        if token_data.claims.nbf > now {
            return Err(AuthError::InvalidToken("Token not yet valid".to_string()));
        }

        Ok(token_data.claims)
    }

    /// Refresh an access token using a refresh token
    pub async fn refresh_token(
        &self,
        refresh_token: &str,
        new_roles: Option<Vec<String>>,
    ) -> Result<TokenPair, AuthError> {
        let claims = self.verify_token(refresh_token).await?;

        // Verify this is a refresh token
        if claims.typ != TokenType::Refresh {
            return Err(AuthError::InvalidToken("Not a refresh token".to_string()));
        }

        // Revoke the old refresh token
        self.revoke_token(refresh_token).await?;

        // Use provided roles or empty vec for refresh tokens
        let roles = new_roles.unwrap_or_default();

        // Generate new token pair
        self.generate_token_pair(&claims.sub, claims.tenant_id, roles)
            .await
    }

    /// Revoke a token (add to blacklist)
    pub async fn revoke_token(&self, token: &str) -> Result<(), AuthError> {
        let mut blacklist = self.blacklist.write().await;
        blacklist.insert(token.to_string());
        Ok(())
    }

    /// Get remaining token validity time in seconds
    pub async fn get_token_ttl(&self, token: &str) -> Result<i64, AuthError> {
        let claims = self.verify_token(token).await?;
        let now = chrono::Utc::now().timestamp();
        Ok((claims.exp - now).max(0))
    }

    /// Extract user ID from token without full verification (for logging)
    pub fn extract_user_id(&self, token: &str) -> Option<String> {
        decode::<Claims>(token, &self.decoding_key, &Validation::new(self.header.alg))
            .ok()
            .map(|data| data.claims.sub)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::auth::JwtAlgorithm;

    fn test_jwt_config() -> JwtConfig {
        JwtConfig {
            secret: Some("test-secret-key-for-jwt-tokens".to_string()),
            expiration_secs: 3600,
            refresh_expiration_secs: 86400,
            issuer: "test-proximadb".to_string(),
            audience: "test-api".to_string(),
            algorithm: JwtAlgorithm::HS256,
        }
    }

    #[tokio::test]
    async fn test_jwt_service_creation() {
        let config = test_jwt_config();
        let jwt_service = JwtService::new(config);
        assert!(jwt_service.is_ok());
    }

    #[tokio::test]
    async fn test_token_generation_and_verification() {
        let config = test_jwt_config();
        let jwt_service = JwtService::new(config).unwrap();

        let user_id = "test_user";
        let roles = vec!["admin".to_string(), "user".to_string()];

        // Generate token pair
        let token_pair = jwt_service
            .generate_token_pair(user_id, None, roles.clone())
            .await
            .unwrap();

        // Verify access token
        let claims = jwt_service
            .verify_token(&token_pair.access_token)
            .await
            .unwrap();
        assert_eq!(claims.sub, user_id);
        assert_eq!(claims.roles, roles);
        assert_eq!(claims.typ, TokenType::Access);

        // Verify refresh token
        let refresh_claims = jwt_service
            .verify_token(&token_pair.refresh_token)
            .await
            .unwrap();
        assert_eq!(refresh_claims.sub, user_id);
        assert_eq!(refresh_claims.typ, TokenType::Refresh);
        assert!(refresh_claims.roles.is_empty()); // Refresh tokens don't have roles
    }

    #[tokio::test]
    async fn test_token_refresh() {
        let config = test_jwt_config();
        let jwt_service = JwtService::new(config).unwrap();

        let user_id = "test_user";
        let roles = vec!["user".to_string()];

        // Generate initial token pair
        let token_pair = jwt_service
            .generate_token_pair(user_id, None, roles.clone())
            .await
            .unwrap();

        // Refresh the token with new roles
        let new_roles = vec!["admin".to_string()];
        let new_token_pair = jwt_service
            .refresh_token(&token_pair.refresh_token, Some(new_roles.clone()))
            .await
            .unwrap();

        // Verify new access token has updated roles
        let claims = jwt_service
            .verify_token(&new_token_pair.access_token)
            .await
            .unwrap();
        assert_eq!(claims.sub, user_id);
        assert_eq!(claims.roles, new_roles);

        // Old refresh token should now be revoked
        assert!(
            jwt_service
                .verify_token(&token_pair.refresh_token)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_token_revocation() {
        let config = test_jwt_config();
        let jwt_service = JwtService::new(config).unwrap();

        let user_id = "test_user";
        let roles = vec!["user".to_string()];

        // Generate token pair
        let token_pair = jwt_service
            .generate_token_pair(user_id, None, roles)
            .await
            .unwrap();

        // Token should be valid initially
        assert!(
            jwt_service
                .verify_token(&token_pair.access_token)
                .await
                .is_ok()
        );

        // Revoke the token
        jwt_service
            .revoke_token(&token_pair.access_token)
            .await
            .unwrap();

        // Token should now be invalid
        assert!(
            jwt_service
                .verify_token(&token_pair.access_token)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_token_ttl() {
        let config = test_jwt_config();
        let jwt_service = JwtService::new(config).unwrap();

        let token_pair = jwt_service
            .generate_token_pair("test_user", None, vec![])
            .await
            .unwrap();

        let ttl = jwt_service
            .get_token_ttl(&token_pair.access_token)
            .await
            .unwrap();
        assert!(ttl > 0);
        assert!(ttl <= 3600); // Should be less than or equal to expiration time
    }
}
