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

//! Unified Authentication and Authorization Framework for ProximaDB
//! 
//! This module provides enterprise-grade authentication and authorization
//! capabilities including API keys, JWT tokens, and role-based access control.

pub mod config;
pub mod jwt;
pub mod rbac;
pub mod middleware;
pub mod providers;

pub use config::*;
pub use jwt::*;
pub use rbac::*;
pub use middleware::*;
pub use providers::*;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Authentication result containing user information and permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResult {
    pub user_id: String,
    pub tenant_id: Option<String>,
    pub roles: Vec<String>,
    pub permissions: Vec<Permission>,
    pub auth_method: AuthMethod,
    pub token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Authentication methods supported by ProximaDB
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthMethod {
    /// API key authentication
    ApiKey,
    /// JWT token authentication
    JwtToken,
    /// OAuth2 bearer token
    OAuth2,
    /// Client certificate (mTLS)
    ClientCertificate,
}

/// Permissions for different operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Permission {
    // Collection permissions
    CreateCollection,
    DeleteCollection,
    ListCollections,
    ReadCollectionMetadata,
    UpdateCollectionMetadata,
    
    // Vector permissions  
    InsertVectors,
    DeleteVectors,
    SearchVectors,
    UpdateVectors,
    ReadVectors,
    
    // Graph permissions
    CreateGraphRelations,
    DeleteGraphRelations,
    TraverseGraph,
    ReadGraphRelations,
    
    // Query permissions
    ExecuteSqlQueries,
    ExecuteSksFunctions,
    
    // System permissions
    ViewSystemMetrics,
    ViewSystemHealth,
    ConfigureSystem,
    
    // Administrative permissions
    ManageUsers,
    ManageRoles,
    ManageApiKeys,
    ViewAuditLogs,
}

/// Error types for authentication and authorization
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),
    
    #[error("Authorization denied: missing permission {0:?}")]
    AuthorizationDenied(Permission),
    
    #[error("Invalid token: {0}")]
    InvalidToken(String),
    
    #[error("Token expired")]
    TokenExpired,
    
    #[error("Invalid credentials")]
    InvalidCredentials,
    
    #[error("User not found: {0}")]
    UserNotFound(String),
    
    #[error("Role not found: {0}")]
    RoleNotFound(String),
}

/// Main authentication service
pub struct AuthService {
    config: AuthConfig,
    rbac: RbacService,
    jwt_service: JwtService,
    providers: HashMap<String, Box<dyn AuthProvider>>,
}

impl AuthService {
    /// Create a new authentication service
    pub fn new(config: AuthConfig) -> Result<Self> {
        let rbac = RbacService::new();
        let jwt_service = JwtService::new(config.jwt.clone())?;
        let providers = HashMap::new();
        
        Ok(Self {
            config,
            rbac,
            jwt_service,
            providers,
        })
    }
    
    /// Authenticate a request and return user information
    pub async fn authenticate(&self, auth_header: &str) -> Result<AuthResult, AuthError> {
        if let Some(token) = auth_header.strip_prefix("Bearer ") {
            // JWT or OAuth2 token
            self.authenticate_jwt(token).await
        } else if let Some(api_key) = auth_header.strip_prefix("API-Key ") {
            // API key authentication
            self.authenticate_api_key(api_key).await
        } else {
            // Try as direct API key
            self.authenticate_api_key(auth_header).await
        }
    }
    
    /// Check if user has permission for an operation
    pub fn authorize(&self, auth_result: &AuthResult, permission: Permission) -> Result<(), AuthError> {
        if auth_result.permissions.contains(&permission) {
            Ok(())
        } else {
            Err(AuthError::AuthorizationDenied(permission))
        }
    }
    
    /// Authenticate using JWT token
    async fn authenticate_jwt(&self, token: &str) -> Result<AuthResult, AuthError> {
        let claims = self.jwt_service.verify_token(token)?;
        
        // Get user roles and permissions
        let roles = self.rbac.get_user_roles(&claims.sub)?;
        let permissions = self.rbac.get_permissions_for_roles(&roles)?;
        
        Ok(AuthResult {
            user_id: claims.sub,
            tenant_id: claims.tenant_id,
            roles,
            permissions,
            auth_method: AuthMethod::JwtToken,
            token_expires_at: Some(chrono::DateTime::from_timestamp(claims.exp, 0)
                .unwrap_or_default()
                .into()),
        })
    }
    
    /// Authenticate using API key
    async fn authenticate_api_key(&self, api_key: &str) -> Result<AuthResult, AuthError> {
        let api_key_info = self.config.api_keys.get(api_key)
            .ok_or_else(|| AuthError::InvalidCredentials)?;
            
        let roles = self.rbac.get_user_roles(&api_key_info.user_id)?;
        let permissions = self.rbac.get_permissions_for_roles(&roles)?;
        
        Ok(AuthResult {
            user_id: api_key_info.user_id.clone(),
            tenant_id: api_key_info.tenant_id.clone(),
            roles,
            permissions,
            auth_method: AuthMethod::ApiKey,
            token_expires_at: None, // API keys don't expire
        })
    }
}

/// Trait for authentication providers (LDAP, OAuth, etc.)
#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    async fn authenticate(&self, credentials: &str) -> Result<AuthResult, AuthError>;
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_auth_service_creation() {
        let config = AuthConfig::default();
        let auth_service = AuthService::new(config);
        assert!(auth_service.is_ok());
    }
    
    #[tokio::test]
    async fn test_permission_check() {
        let auth_result = AuthResult {
            user_id: "test_user".to_string(),
            tenant_id: None,
            roles: vec!["reader".to_string()],
            permissions: vec![Permission::ReadVectors, Permission::SearchVectors],
            auth_method: AuthMethod::ApiKey,
            token_expires_at: None,
        };
        
        let config = AuthConfig::default();
        let auth_service = AuthService::new(config).unwrap();
        
        // Should succeed for granted permission
        assert!(auth_service.authorize(&auth_result, Permission::ReadVectors).is_ok());
        
        // Should fail for denied permission
        assert!(auth_service.authorize(&auth_result, Permission::DeleteVectors).is_err());
    }
}