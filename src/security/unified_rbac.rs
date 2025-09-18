//! Unified RBAC System for ProximaDB
//!
//! Consolidates the Enhanced RBAC Manager (storage/tenant/rbac.rs) and
//! Network RBAC (network/auth/rbac.rs) into a single, coherent permission system.

use anyhow::{Result, anyhow};
use dashmap::DashMap;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use tracing::{info, warn, debug};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unified permission model consolidating all permission types
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnifiedPermission {
    // === TENANT LEVEL PERMISSIONS ===
    TenantAdmin,
    TenantRead,
    TenantWrite,

    // === DOMAIN LEVEL PERMISSIONS ===
    DomainCreate,
    DomainRead(String),
    DomainWrite(String),
    DomainAdmin(String),

    // === COLLECTION LEVEL PERMISSIONS ===
    // Collection management
    CollectionCreate,
    CollectionRead(String),
    CollectionWrite(String),
    CollectionDelete(String),
    CollectionAdmin(String),

    // Collection metadata
    ReadCollectionMetadata(String),
    UpdateCollectionMetadata(String),
    ListCollections,

    // === VECTOR LEVEL PERMISSIONS ===
    VectorInsert(String),      // Collection-specific vector operations
    VectorDelete(String),
    VectorSearch(String),
    VectorUpdate(String),
    VectorRead(String),

    // === ENTITY LEVEL PERMISSIONS ===
    EntityRead(String),
    EntityWrite(String),
    EntityDelete(String),

    // === GRAPH LEVEL PERMISSIONS ===
    GraphCreateRelations(String),   // Collection-specific graph operations
    GraphDeleteRelations(String),
    GraphTraverse(String),
    GraphReadRelations(String),

    // === QUERY LEVEL PERMISSIONS ===
    ExecuteSqlQueries(String),      // Collection-specific SQL queries
    ExecuteSksFunctions(String),    // SKS function execution

    // === SYSTEM LEVEL PERMISSIONS ===
    ViewSystemMetrics,
    ViewSystemHealth,
    ConfigureSystem,
    AuditRead,
    SystemAdmin,

    // === BUSINESS CONTEXT PERMISSIONS ===
    // (From Enhanced RBAC for business intelligence)
    RiskDataAccess,
    FinancialDataAccess,
    ComplianceDataAccess,
    CustomerDataAccess,

    // === SPECIAL PERMISSIONS ===
    FieldLevelRead(String, String), // (collection, field)
    FieldLevelWrite(String, String),
}

/// Unified role definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedRole {
    pub role_id: String,
    pub role_name: String,
    pub tenant_id: Option<String>,  // None for system-wide roles
    pub permissions: HashSet<UnifiedPermission>,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub is_system_role: bool,
}

/// Unified user context for all authentication methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedUserContext {
    pub user_id: String,
    pub tenant_id: Option<String>,
    pub roles: Vec<String>,
    pub effective_permissions: HashSet<UnifiedPermission>,
    pub auth_method: AuthMethod,
    pub session_id: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}

/// Authentication method enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthMethod {
    SSO { provider: String },
    JWT,
    ApiKey,
    ClientCertificate,
    Internal,
}

/// Consolidated RBAC Manager
pub struct ConsolidatedRBACManager {
    /// Tenant-specific role definitions
    tenant_roles: Arc<DashMap<String, Arc<DashMap<String, UnifiedRole>>>>,

    /// System-wide roles (cross-tenant)
    system_roles: Arc<DashMap<String, UnifiedRole>>,

    /// Collection-specific permissions
    collection_permissions: Arc<DashMap<String, CollectionPermissions>>,

    /// User role assignments
    user_role_assignments: Arc<DashMap<String, UserRoleAssignment>>,

    /// Audit logger for RBAC events
    audit_logger: Option<Arc<dyn RBACEventLogger + Send + Sync>>,

    /// Configuration
    config: RBACConfig,
}

/// Collection permissions with unified model
#[derive(Debug, Clone)]
pub struct CollectionPermissions {
    pub tenant_id: String,
    pub collection_id: String,
    pub read_roles: HashSet<String>,
    pub write_roles: HashSet<String>,
    pub admin_roles: HashSet<String>,
    pub field_level_permissions: Option<FieldLevelPermissions>,
    pub created_at: DateTime<Utc>,
}

/// Field-level permissions for granular access control
#[derive(Debug, Clone)]
pub struct FieldLevelPermissions {
    pub read_fields: HashMap<String, HashSet<String>>,  // role -> fields
    pub write_fields: HashMap<String, HashSet<String>>, // role -> fields
    pub restricted_fields: HashSet<String>,             // Always restricted
}

/// User role assignment
#[derive(Debug, Clone)]
pub struct UserRoleAssignment {
    pub user_id: String,
    pub tenant_id: Option<String>,
    pub roles: HashSet<String>,
    pub direct_permissions: HashSet<UnifiedPermission>,
    pub assigned_at: DateTime<Utc>,
    pub assigned_by: String,
    pub expires_at: Option<DateTime<Utc>>,
}

/// RBAC configuration
#[derive(Debug, Clone)]
pub struct RBACConfig {
    pub enabled: bool,
    pub enable_field_level_permissions: bool,
    pub enable_audit_logging: bool,
    pub default_deny: bool,
    pub cache_permissions: bool,
    pub permission_cache_ttl_minutes: u64,
}

impl Default for RBACConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enable_field_level_permissions: true,
            enable_audit_logging: true,
            default_deny: false,
            cache_permissions: true,
            permission_cache_ttl_minutes: 15,
        }
    }
}

/// RBAC event logger trait
pub trait RBACEventLogger {
    async fn log_permission_check(
        &self,
        user_context: &UnifiedUserContext,
        permission: &UnifiedPermission,
        result: bool,
    ) -> Result<()>;

    async fn log_role_assignment(
        &self,
        user_id: &str,
        tenant_id: Option<&str>,
        roles: &[String],
        assigned_by: &str,
    ) -> Result<()>;

    async fn log_permission_denial(
        &self,
        user_context: &UnifiedUserContext,
        attempted_permission: &UnifiedPermission,
        reason: &str,
    ) -> Result<()>;
}

impl ConsolidatedRBACManager {
    /// Create new consolidated RBAC manager
    pub fn new(config: RBACConfig) -> Self {
        let manager = Self {
            tenant_roles: Arc::new(DashMap::new()),
            system_roles: Arc::new(DashMap::new()),
            collection_permissions: Arc::new(DashMap::new()),
            user_role_assignments: Arc::new(DashMap::new()),
            audit_logger: None,
            config,
        };

        // Initialize default system roles
        manager.initialize_default_system_roles();
        manager
    }

    /// Set audit logger for RBAC events
    pub fn set_audit_logger(&mut self, logger: Arc<dyn RBACEventLogger + Send + Sync>) {
        self.audit_logger = Some(logger);
    }

    /// Check if user has specific permission
    pub async fn check_permission(
        &self,
        user_context: &UnifiedUserContext,
        permission: &UnifiedPermission,
    ) -> Result<bool> {
        // Get effective permissions for user
        let effective_permissions = self.get_effective_permissions(user_context).await?;

        let has_permission = effective_permissions.contains(permission) ||
                           self.check_wildcard_permissions(&effective_permissions, permission);

        // Log permission check if audit enabled
        if let Some(audit_logger) = &self.audit_logger {
            audit_logger.log_permission_check(user_context, permission, has_permission).await?;
        }

        Ok(has_permission)
    }

    /// Get effective permissions for user (combining role permissions and direct permissions)
    pub async fn get_effective_permissions(
        &self,
        user_context: &UnifiedUserContext,
    ) -> Result<HashSet<UnifiedPermission>> {
        let mut effective_permissions = HashSet::new();

        // Get user role assignment
        if let Some(assignment) = self.user_role_assignments.get(&user_context.user_id) {
            // Add direct permissions
            effective_permissions.extend(assignment.direct_permissions.clone());

            // Add role-based permissions
            for role_name in &assignment.roles {
                if let Some(permissions) = self.get_role_permissions(
                    role_name,
                    user_context.tenant_id.as_deref()
                ).await? {
                    effective_permissions.extend(permissions);
                }
            }
        }

        // Apply default deny policy if configured
        if self.config.default_deny && effective_permissions.is_empty() {
            return Ok(HashSet::new());
        }

        Ok(effective_permissions)
    }

    /// Get permissions for a specific role
    async fn get_role_permissions(
        &self,
        role_name: &str,
        tenant_id: Option<&str>,
    ) -> Result<Option<HashSet<UnifiedPermission>>> {
        // Check tenant-specific roles first
        if let Some(tenant_id) = tenant_id {
            if let Some(tenant_roles) = self.tenant_roles.get(tenant_id) {
                if let Some(role) = tenant_roles.get(role_name) {
                    return Ok(Some(role.permissions.clone()));
                }
            }
        }

        // Check system-wide roles
        if let Some(role) = self.system_roles.get(role_name) {
            return Ok(Some(role.permissions.clone()));
        }

        Ok(None)
    }

    /// Check wildcard permissions (e.g., CollectionAdmin grants all collection permissions)
    fn check_wildcard_permissions(
        &self,
        user_permissions: &HashSet<UnifiedPermission>,
        requested_permission: &UnifiedPermission,
    ) -> bool {
        match requested_permission {
            // Collection permissions
            UnifiedPermission::CollectionRead(collection_id) |
            UnifiedPermission::CollectionWrite(collection_id) |
            UnifiedPermission::VectorRead(collection_id) |
            UnifiedPermission::VectorInsert(collection_id) |
            UnifiedPermission::VectorSearch(collection_id) => {
                user_permissions.contains(&UnifiedPermission::CollectionAdmin(collection_id.clone())) ||
                user_permissions.contains(&UnifiedPermission::SystemAdmin)
            }

            // Domain permissions
            UnifiedPermission::DomainRead(domain_id) |
            UnifiedPermission::DomainWrite(domain_id) => {
                user_permissions.contains(&UnifiedPermission::DomainAdmin(domain_id.clone())) ||
                user_permissions.contains(&UnifiedPermission::TenantAdmin) ||
                user_permissions.contains(&UnifiedPermission::SystemAdmin)
            }

            // Tenant permissions
            UnifiedPermission::TenantRead | UnifiedPermission::TenantWrite => {
                user_permissions.contains(&UnifiedPermission::TenantAdmin) ||
                user_permissions.contains(&UnifiedPermission::SystemAdmin)
            }

            _ => false,
        }
    }

    /// Initialize default system roles
    fn initialize_default_system_roles(&self) {
        let system_admin_role = UnifiedRole {
            role_id: "system_admin".to_string(),
            role_name: "System Administrator".to_string(),
            tenant_id: None,
            permissions: vec![UnifiedPermission::SystemAdmin].into_iter().collect(),
            description: "Full system administration access".to_string(),
            created_at: Utc::now(),
            created_by: "system".to_string(),
            is_system_role: true,
        };

        let tenant_admin_role = UnifiedRole {
            role_id: "tenant_admin".to_string(),
            role_name: "Tenant Administrator".to_string(),
            tenant_id: None,
            permissions: vec![
                UnifiedPermission::TenantAdmin,
                UnifiedPermission::CollectionCreate,
                UnifiedPermission::AuditRead,
            ].into_iter().collect(),
            description: "Tenant administration access".to_string(),
            created_at: Utc::now(),
            created_by: "system".to_string(),
            is_system_role: true,
        };

        let collection_user_role = UnifiedRole {
            role_id: "collection_user".to_string(),
            role_name: "Collection User".to_string(),
            tenant_id: None,
            permissions: vec![
                UnifiedPermission::ListCollections,
                UnifiedPermission::ReadCollectionMetadata("*".to_string()),
                UnifiedPermission::ViewSystemHealth,
            ].into_iter().collect(),
            description: "Basic collection access".to_string(),
            created_at: Utc::now(),
            created_by: "system".to_string(),
            is_system_role: true,
        };

        // Insert default roles
        self.system_roles.insert("system_admin".to_string(), system_admin_role);
        self.system_roles.insert("tenant_admin".to_string(), tenant_admin_role);
        self.system_roles.insert("collection_user".to_string(), collection_user_role);

        info!("Initialized {} default system roles", self.system_roles.len());
    }

    /// Assign role to user
    pub async fn assign_role_to_user(
        &self,
        user_id: &str,
        tenant_id: Option<&str>,
        role_name: &str,
        assigned_by: &str,
    ) -> Result<()> {
        // Validate role exists
        let role_exists = if let Some(tenant_id) = tenant_id {
            self.tenant_roles.get(tenant_id)
                .and_then(|roles| roles.get(role_name))
                .is_some()
        } else {
            false
        };

        if !role_exists && !self.system_roles.contains_key(role_name) {
            return Err(anyhow!("Role '{}' does not exist", role_name));
        }

        // Create or update user role assignment
        let assignment = UserRoleAssignment {
            user_id: user_id.to_string(),
            tenant_id: tenant_id.map(|t| t.to_string()),
            roles: vec![role_name.to_string()].into_iter().collect(),
            direct_permissions: HashSet::new(),
            assigned_at: Utc::now(),
            assigned_by: assigned_by.to_string(),
            expires_at: None,
        };

        self.user_role_assignments.insert(user_id.to_string(), assignment);

        // Log role assignment
        if let Some(audit_logger) = &self.audit_logger {
            audit_logger.log_role_assignment(
                user_id,
                tenant_id,
                &[role_name.to_string()],
                assigned_by,
            ).await?;
        }

        info!("Assigned role '{}' to user '{}' in tenant '{:?}'",
              role_name, user_id, tenant_id);

        Ok(())
    }

    /// Create custom tenant role
    pub async fn create_tenant_role(
        &self,
        tenant_id: &str,
        role_name: &str,
        permissions: HashSet<UnifiedPermission>,
        created_by: &str,
    ) -> Result<()> {
        let role = UnifiedRole {
            role_id: format!("{}_{}", tenant_id, role_name),
            role_name: role_name.to_string(),
            tenant_id: Some(tenant_id.to_string()),
            permissions,
            description: format!("Custom role for tenant {}", tenant_id),
            created_at: Utc::now(),
            created_by: created_by.to_string(),
            is_system_role: false,
        };

        // Ensure tenant roles map exists
        if !self.tenant_roles.contains_key(tenant_id) {
            self.tenant_roles.insert(tenant_id.to_string(), Arc::new(DashMap::new()));
        }

        // Insert role
        if let Some(tenant_roles) = self.tenant_roles.get(tenant_id) {
            tenant_roles.insert(role_name.to_string(), role);
        }

        info!("Created custom role '{}' for tenant '{}'", role_name, tenant_id);
        Ok(())
    }

    /// Check collection-specific permission
    pub async fn check_collection_permission(
        &self,
        user_context: &UnifiedUserContext,
        collection_id: &str,
        permission_type: CollectionPermissionType,
    ) -> Result<bool> {
        let permission = match permission_type {
            CollectionPermissionType::Read => UnifiedPermission::CollectionRead(collection_id.to_string()),
            CollectionPermissionType::Write => UnifiedPermission::CollectionWrite(collection_id.to_string()),
            CollectionPermissionType::Admin => UnifiedPermission::CollectionAdmin(collection_id.to_string()),
            CollectionPermissionType::VectorInsert => UnifiedPermission::VectorInsert(collection_id.to_string()),
            CollectionPermissionType::VectorSearch => UnifiedPermission::VectorSearch(collection_id.to_string()),
        };

        self.check_permission(user_context, &permission).await
    }

    /// Get all collections user has access to
    pub async fn get_accessible_collections(
        &self,
        user_context: &UnifiedUserContext,
        permission_type: CollectionPermissionType,
    ) -> Result<Vec<String>> {
        let mut accessible_collections = Vec::new();

        // Check if user has system admin (access to all)
        if user_context.effective_permissions.contains(&UnifiedPermission::SystemAdmin) {
            // Return all collections for system admin
            // This would need integration with collection service
            return Ok(vec!["*".to_string()]); // Placeholder
        }

        // Check specific collection permissions
        for permission in &user_context.effective_permissions {
            match permission {
                UnifiedPermission::CollectionRead(coll_id) |
                UnifiedPermission::CollectionWrite(coll_id) |
                UnifiedPermission::CollectionAdmin(coll_id) => {
                    accessible_collections.push(coll_id.clone());
                }
                _ => {}
            }
        }

        Ok(accessible_collections)
    }
}

/// Collection permission types for easier API usage
#[derive(Debug, Clone)]
pub enum CollectionPermissionType {
    Read,
    Write,
    Admin,
    VectorInsert,
    VectorSearch,
}

/// Authorization result
#[derive(Debug, Clone)]
pub struct AuthorizationResult {
    pub allowed: bool,
    pub permissions: HashSet<UnifiedPermission>,
    pub tenant_context: Option<TenantContext>,
    pub reason: Option<String>,
}

/// Tenant context for authorized operations
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: String,
    pub tenant_name: String,
    pub security_policy: String,
    pub compliance_frameworks: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_unified_permission_checking() {
        let config = RBACConfig::default();
        let rbac_manager = ConsolidatedRBACManager::new(config);

        // Create test user context
        let user_context = UnifiedUserContext {
            user_id: "test_user".to_string(),
            tenant_id: Some("test_tenant".to_string()),
            roles: vec!["collection_user".to_string()],
            effective_permissions: vec![
                UnifiedPermission::CollectionRead("test_collection".to_string()),
                UnifiedPermission::VectorSearch("test_collection".to_string()),
            ].into_iter().collect(),
            auth_method: AuthMethod::JWT,
            session_id: "session_123".to_string(),
            expires_at: None,
            created_at: Utc::now(),
            metadata: HashMap::new(),
        };

        // Test permission checking
        let can_read = rbac_manager.check_permission(
            &user_context,
            &UnifiedPermission::CollectionRead("test_collection".to_string())
        ).await.unwrap();

        assert!(can_read);

        let cannot_admin = rbac_manager.check_permission(
            &user_context,
            &UnifiedPermission::SystemAdmin
        ).await.unwrap();

        assert!(!cannot_admin);
    }

    #[tokio::test]
    async fn test_wildcard_permissions() {
        let config = RBACConfig::default();
        let rbac_manager = ConsolidatedRBACManager::new(config);

        // User with admin permission should have access to specific operations
        let admin_permissions = vec![
            UnifiedPermission::CollectionAdmin("test_collection".to_string()),
        ].into_iter().collect();

        let has_read = rbac_manager.check_wildcard_permissions(
            &admin_permissions,
            &UnifiedPermission::CollectionRead("test_collection".to_string())
        );

        assert!(has_read);
    }

    #[tokio::test]
    async fn test_role_assignment() {
        let config = RBACConfig::default();
        let rbac_manager = ConsolidatedRBACManager::new(config);

        // Assign role to user
        let result = rbac_manager.assign_role_to_user(
            "test_user",
            Some("test_tenant"),
            "collection_user",
            "admin"
        ).await;

        assert!(result.is_ok());

        // Verify assignment exists
        let assignment = rbac_manager.user_role_assignments.get("test_user");
        assert!(assignment.is_some());
        assert!(assignment.unwrap().roles.contains("collection_user"));
    }
}