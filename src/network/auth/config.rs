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

//! Unified Authentication Configuration for ProximaDB

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Comprehensive authentication configuration for enterprise deployment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Enable authentication (if false, all requests pass through)
    pub enabled: bool,
    
    /// JWT configuration
    pub jwt: JwtConfig,
    
    /// API key authentication
    pub api_keys: HashMap<String, ApiKeyInfo>,
    
    /// Role-based access control settings
    pub rbac: RbacConfig,
    
    /// OAuth2 providers (optional)
    pub oauth2: Option<OAuth2Config>,
    
    /// Client certificate (mTLS) settings
    pub client_certificates: Option<ClientCertConfig>,
    
    /// Session management
    pub session: SessionConfig,
    
    /// Audit logging for authentication events
    pub audit_logging: AuditConfig,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            jwt: JwtConfig::default(),
            api_keys: HashMap::new(),
            rbac: RbacConfig::default(),
            oauth2: None,
            client_certificates: None,
            session: SessionConfig::default(),
            audit_logging: AuditConfig::default(),
        }
    }
}

/// JWT token configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtConfig {
    /// JWT signing secret (must be kept secure)
    pub secret: Option<String>,
    
    /// Token expiration time in seconds (default: 1 hour)
    pub expiration_secs: u64,
    
    /// Refresh token expiration (default: 7 days)
    pub refresh_expiration_secs: u64,
    
    /// JWT issuer claim
    pub issuer: String,
    
    /// JWT audience claim
    pub audience: String,
    
    /// Algorithm for signing tokens
    pub algorithm: JwtAlgorithm,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            secret: None,
            expiration_secs: 3600, // 1 hour
            refresh_expiration_secs: 604800, // 7 days
            issuer: "proximadb".to_string(),
            audience: "proximadb-api".to_string(),
            algorithm: JwtAlgorithm::HS256,
        }
    }
}

/// Supported JWT algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JwtAlgorithm {
    HS256,
    HS384,
    HS512,
    RS256,
    RS384,
    RS512,
}

/// API key information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyInfo {
    /// User ID associated with this API key
    pub user_id: String,
    
    /// Optional tenant/organization ID
    pub tenant_id: Option<String>,
    
    /// Human-readable description of the key
    pub description: Option<String>,
    
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    
    /// Optional expiration timestamp
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    
    /// Whether the key is active
    pub active: bool,
    
    /// Optional IP restrictions
    pub allowed_ips: Option<Vec<String>>,
    
    /// Rate limiting override for this key
    pub rate_limit: Option<u32>,
}

/// Role-based access control configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RbacConfig {
    /// Enable RBAC (if false, users get all permissions)
    pub enabled: bool,
    
    /// Default role for new users
    pub default_role: String,
    
    /// Whether to require auth for health endpoints
    pub require_auth_for_health: bool,
    
    /// Whether to require auth for system metrics
    pub require_auth_for_metrics: bool,
    
    /// Predefined roles and their permissions
    pub roles: HashMap<String, Vec<String>>,
}

impl Default for RbacConfig {
    fn default() -> Self {
        let mut roles = HashMap::new();
        
        // Default roles
        roles.insert("admin".to_string(), vec![
            "CreateCollection".to_string(),
            "DeleteCollection".to_string(),
            "ListCollections".to_string(),
            "ReadCollectionMetadata".to_string(),
            "UpdateCollectionMetadata".to_string(),
            "InsertVectors".to_string(),
            "DeleteVectors".to_string(),
            "SearchVectors".to_string(),
            "UpdateVectors".to_string(),
            "ReadVectors".to_string(),
            "CreateGraphRelations".to_string(),
            "DeleteGraphRelations".to_string(),
            "TraverseGraph".to_string(),
            "ReadGraphRelations".to_string(),
            "ExecuteSqlQueries".to_string(),
            "ExecuteSksFunctions".to_string(),
            "ViewSystemMetrics".to_string(),
            "ViewSystemHealth".to_string(),
            "ConfigureSystem".to_string(),
            "ManageUsers".to_string(),
            "ManageRoles".to_string(),
            "ManageApiKeys".to_string(),
            "ViewAuditLogs".to_string(),
        ]);
        
        roles.insert("user".to_string(), vec![
            "ListCollections".to_string(),
            "ReadCollectionMetadata".to_string(),
            "InsertVectors".to_string(),
            "SearchVectors".to_string(),
            "ReadVectors".to_string(),
            "ReadGraphRelations".to_string(),
            "ExecuteSqlQueries".to_string(),
            "ExecuteSksFunctions".to_string(),
            "ViewSystemHealth".to_string(),
        ]);
        
        roles.insert("readonly".to_string(), vec![
            "ListCollections".to_string(),
            "ReadCollectionMetadata".to_string(),
            "SearchVectors".to_string(),
            "ReadVectors".to_string(),
            "ReadGraphRelations".to_string(),
            "ViewSystemHealth".to_string(),
        ]);
        
        Self {
            enabled: true,
            default_role: "user".to_string(),
            require_auth_for_health: false,
            require_auth_for_metrics: true,
            roles,
        }
    }
}

/// OAuth2 configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Config {
    pub providers: HashMap<String, OAuth2Provider>,
}

/// OAuth2 provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Provider {
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub user_info_url: String,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
}

/// Client certificate authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCertConfig {
    /// Enable client certificate authentication
    pub enabled: bool,
    
    /// Path to CA certificate for validating client certificates
    pub ca_cert_path: String,
    
    /// Whether to require client certificates for all requests
    pub required: bool,
    
    /// Certificate revocation list (CRL) path
    pub crl_path: Option<String>,
}

/// Session management configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Session timeout in seconds
    pub timeout_secs: u64,
    
    /// Enable session persistence
    pub persistent: bool,
    
    /// Session storage backend
    pub storage: SessionStorage,
    
    /// Maximum concurrent sessions per user
    pub max_sessions_per_user: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 3600, // 1 hour
            persistent: false,
            storage: SessionStorage::Memory,
            max_sessions_per_user: 5,
        }
    }
}

/// Session storage backends
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionStorage {
    Memory,
    Redis(RedisConfig),
    Database,
}

/// Redis configuration for session storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub key_prefix: String,
}

/// Audit logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Enable audit logging
    pub enabled: bool,
    
    /// Log authentication events
    pub log_auth_events: bool,
    
    /// Log authorization failures
    pub log_authz_failures: bool,
    
    /// Log admin operations
    pub log_admin_operations: bool,
    
    /// Audit log storage backend
    pub storage: AuditStorage,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            log_auth_events: true,
            log_authz_failures: true,
            log_admin_operations: true,
            storage: AuditStorage::File {
                path: "audit.log".to_string(),
                rotate: true,
                max_size_mb: 100,
            },
        }
    }
}

/// Audit storage backends
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditStorage {
    File {
        path: String,
        rotate: bool,
        max_size_mb: u64,
    },
    Database,
    Syslog {
        facility: String,
        tag: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.jwt.expiration_secs, 3600);
        assert!(config.rbac.enabled);
        assert!(!config.rbac.require_auth_for_health);
        assert!(config.rbac.require_auth_for_metrics);
        assert_eq!(config.rbac.default_role, "user");
    }
    
    #[test]
    fn test_default_roles() {
        let config = RbacConfig::default();
        assert!(config.roles.contains_key("admin"));
        assert!(config.roles.contains_key("user"));
        assert!(config.roles.contains_key("readonly"));
        
        let admin_permissions = &config.roles["admin"];
        assert!(admin_permissions.contains(&"ManageUsers".to_string()));
        
        let readonly_permissions = &config.roles["readonly"];
        assert!(!readonly_permissions.contains(&"DeleteVectors".to_string()));
    }
}