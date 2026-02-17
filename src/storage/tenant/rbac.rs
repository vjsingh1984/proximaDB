//! Enhanced RBAC for multi-tenant architecture - clean implementation

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::info;

use super::{TenantManager, UserContext};

/// Enhanced RBAC manager for multi-tenant operations
pub struct EnhancedRBACManager {
    /// Tenant-specific role definitions
    tenant_roles: Arc<DashMap<String, Arc<DashMap<String, TenantRole>>>>,

    /// Collection permissions with tenant context
    collection_permissions: Arc<DashMap<String, CollectionPermissions>>,

    /// Domain permissions within tenants
    #[allow(dead_code)]
    domain_permissions: Arc<DashMap<String, DomainPermissions>>,

    /// User role assignments
    user_role_assignments: Arc<DashMap<String, UserRoleAssignment>>,

    /// RBAC audit logger
    rbac_audit_logger: Arc<RBACEventLogger>,

    /// Tenant manager reference
    tenant_manager: Arc<TenantManager>,
}

/// Clean tenant role definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantRole {
    pub role_name: String,
    pub tenant_id: String,
    pub permissions: HashSet<Permission>,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// Permission enumeration for clean RBAC
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    // Tenant-level permissions
    TenantAdmin,
    TenantRead,
    TenantWrite,

    // Domain-level permissions
    DomainCreate,
    DomainRead(String),
    DomainWrite(String),
    DomainAdmin(String),

    // Collection-level permissions
    CollectionCreate,
    CollectionRead(String),
    CollectionWrite(String),
    CollectionDelete(String),
    CollectionAdmin(String),

    // Entity-level permissions
    EntityRead(String),
    EntityWrite(String),
    EntityDelete(String),

    // Special permissions
    AuditRead,
    SystemAdmin,
}

/// Collection permissions with tenant context
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

/// Domain permissions within tenant
#[derive(Debug, Clone)]
pub struct DomainPermissions {
    pub tenant_id: String,
    pub domain_id: String,
    pub read_roles: HashSet<String>,
    pub write_roles: HashSet<String>,
    pub admin_roles: HashSet<String>,
    pub business_context_permissions: BusinessContextPermissions,
    pub created_at: DateTime<Utc>,
}

/// User role assignment
#[derive(Debug, Clone)]
pub struct UserRoleAssignment {
    pub user_id: String,
    pub tenant_id: String,
    pub roles: HashSet<String>,
    pub effective_permissions: HashSet<Permission>,
    pub assigned_at: DateTime<Utc>,
    pub assigned_by: String,
}

/// Field-level permissions for enhanced security
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldLevelPermissions {
    pub restricted_fields: HashSet<String>,
    pub role_field_access: HashMap<String, HashSet<String>>,
}

/// Business context permissions
#[derive(Debug, Clone)]
pub struct BusinessContextPermissions {
    pub risk_data_access: bool,
    pub customer_data_access: bool,
    pub financial_data_access: bool,
    pub compliance_data_access: bool,
}

impl EnhancedRBACManager {
    /// Create new RBAC manager
    pub fn new(tenant_manager: Arc<TenantManager>) -> Self {
        Self {
            tenant_roles: Arc::new(DashMap::new()),
            collection_permissions: Arc::new(DashMap::new()),
            domain_permissions: Arc::new(DashMap::new()),
            user_role_assignments: Arc::new(DashMap::new()),
            rbac_audit_logger: Arc::new(RBACEventLogger::new()),
            tenant_manager,
        }
    }

    /// Create tenant role - clean implementation
    pub async fn create_tenant_role(
        &self,
        tenant_id: &str,
        role_name: &str,
        permissions: HashSet<Permission>,
        description: String,
        creator_context: &UserContext,
    ) -> Result<TenantRole> {
        // Validate creator has admin permissions
        self.validate_admin_permission(creator_context, tenant_id)?;

        let role = TenantRole {
            role_name: role_name.to_string(),
            tenant_id: tenant_id.to_string(),
            permissions,
            description,
            created_at: Utc::now(),
            created_by: creator_context.user_id.clone(),
        };

        // Store role in tenant-specific storage
        let tenant_roles = self
            .tenant_roles
            .entry(tenant_id.to_string())
            .or_insert_with(|| Arc::new(DashMap::new()));

        tenant_roles.insert(role_name.to_string(), role.clone());

        // Log role creation
        self.rbac_audit_logger
            .log_role_created(tenant_id, role_name, &role.permissions, creator_context)
            .await?;

        info!(
            "Created role {} in tenant {} with {} permissions",
            role_name,
            tenant_id,
            role.permissions.len()
        );

        Ok(role)
    }

    /// Assign role to user - clean implementation
    pub async fn assign_user_role(
        &self,
        tenant_id: &str,
        user_id: &str,
        role_name: &str,
        assigner_context: &UserContext,
    ) -> Result<()> {
        // Validate assigner has admin permissions
        self.validate_admin_permission(assigner_context, tenant_id)?;

        // Get role definition
        let _role = self.get_tenant_role(tenant_id, role_name)?;

        // Get or create user assignment
        let user_key = format!("{}::{}", tenant_id, user_id);
        let mut assignment = self
            .user_role_assignments
            .get(&user_key)
            .map(|entry| entry.clone())
            .unwrap_or_else(|| UserRoleAssignment {
                user_id: user_id.to_string(),
                tenant_id: tenant_id.to_string(),
                roles: HashSet::new(),
                effective_permissions: HashSet::new(),
                assigned_at: Utc::now(),
                assigned_by: assigner_context.user_id.clone(),
            });

        // Add role
        assignment.roles.insert(role_name.to_string());

        // Recalculate effective permissions
        assignment.effective_permissions =
            self.calculate_effective_permissions(tenant_id, &assignment.roles)?;

        // Store assignment
        self.user_role_assignments.insert(user_key, assignment);

        // Log role assignment
        self.rbac_audit_logger
            .log_role_assigned(tenant_id, user_id, role_name, assigner_context)
            .await?;

        info!(
            "Assigned role {} to user {} in tenant {}",
            role_name, user_id, tenant_id
        );
        Ok(())
    }

    /// Validate collection access with tenant context
    pub async fn validate_collection_access(
        &self,
        tenant_id: &str,
        collection_id: &str,
        operation: CollectionOperation,
        user_context: &UserContext,
    ) -> Result<AccessValidationResult> {
        // Basic tenant validation
        self.tenant_manager
            .validate_user_tenant_access(&user_context.tenant_id, tenant_id)?;

        // Get collection permissions
        let collection_key = format!("{}::{}", tenant_id, collection_id);
        let permissions = self
            .collection_permissions
            .get(&collection_key)
            .ok_or_else(|| {
                anyhow!(
                    "Collection {} not found in tenant {}",
                    collection_id,
                    tenant_id
                )
            })?;

        // Get user effective permissions
        let user_key = format!("{}::{}", tenant_id, user_context.user_id);
        let user_assignment = self.user_role_assignments.get(&user_key).ok_or_else(|| {
            anyhow!(
                "User {} not found in tenant {}",
                user_context.user_id,
                tenant_id
            )
        })?;

        // Check operation permission
        let access_granted = match operation {
            CollectionOperation::Read => {
                self.check_collection_read_permission(&permissions, &user_assignment)
            }
            CollectionOperation::Write => {
                self.check_collection_write_permission(&permissions, &user_assignment)
            }
            CollectionOperation::Delete => {
                self.check_collection_admin_permission(&permissions, &user_assignment)
            }
            CollectionOperation::Admin => {
                self.check_collection_admin_permission(&permissions, &user_assignment)
            }
        };

        // Log access validation
        self.rbac_audit_logger
            .log_access_validation(
                tenant_id,
                collection_id,
                &operation,
                user_context,
                access_granted,
            )
            .await?;

        Ok(AccessValidationResult {
            granted: access_granted,
            user_context: user_context.clone(),
            collection_context: CollectionContext {
                tenant_id: tenant_id.to_string(),
                collection_id: collection_id.to_string(),
                operation,
            },
            validation_metadata: ValidationMetadata {
                validated_at: Utc::now(),
                permissions_checked: user_assignment.effective_permissions.clone(),
                validation_reason: if access_granted {
                    "Authorized".to_string()
                } else {
                    "Insufficient permissions".to_string()
                },
            },
        })
    }

    // Helper methods for permission checking
    fn check_collection_read_permission(
        &self,
        permissions: &CollectionPermissions,
        user_assignment: &UserRoleAssignment,
    ) -> bool {
        // Check if user has any read role for this collection
        user_assignment.roles.iter().any(|role| {
            permissions.read_roles.contains(role) || permissions.admin_roles.contains(role)
        }) || user_assignment
            .effective_permissions
            .contains(&Permission::TenantAdmin)
    }

    fn check_collection_write_permission(
        &self,
        permissions: &CollectionPermissions,
        user_assignment: &UserRoleAssignment,
    ) -> bool {
        user_assignment.roles.iter().any(|role| {
            permissions.write_roles.contains(role) || permissions.admin_roles.contains(role)
        }) || user_assignment
            .effective_permissions
            .contains(&Permission::TenantAdmin)
    }

    fn check_collection_admin_permission(
        &self,
        permissions: &CollectionPermissions,
        user_assignment: &UserRoleAssignment,
    ) -> bool {
        user_assignment
            .roles
            .iter()
            .any(|role| permissions.admin_roles.contains(role))
            || user_assignment
                .effective_permissions
                .contains(&Permission::TenantAdmin)
    }

    fn validate_admin_permission(&self, user_context: &UserContext, tenant_id: &str) -> Result<()> {
        if user_context
            .permissions
            .contains(&"tenant_admin".to_string())
            || user_context
                .permissions
                .contains(&"system_admin".to_string())
        {
            Ok(())
        } else {
            Err(anyhow!(
                "User {} lacks admin permission for tenant {}",
                user_context.user_id,
                tenant_id
            ))
        }
    }

    fn get_tenant_role(&self, tenant_id: &str, role_name: &str) -> Result<TenantRole> {
        self.tenant_roles
            .get(tenant_id)
            .and_then(|roles| roles.get(role_name).map(|role| role.clone()))
            .ok_or_else(|| anyhow!("Role {} not found in tenant {}", role_name, tenant_id))
    }

    fn calculate_effective_permissions(
        &self,
        tenant_id: &str,
        role_names: &HashSet<String>,
    ) -> Result<HashSet<Permission>> {
        let mut effective_permissions = HashSet::new();

        for role_name in role_names {
            if let Ok(role) = self.get_tenant_role(tenant_id, role_name) {
                effective_permissions.extend(role.permissions);
            }
        }

        Ok(effective_permissions)
    }
}

/// Collection operation types
#[derive(Debug, Clone)]
pub enum CollectionOperation {
    Read,
    Write,
    Delete,
    Admin,
}

/// Access validation result
#[derive(Debug, Clone)]
pub struct AccessValidationResult {
    pub granted: bool,
    pub user_context: UserContext,
    pub collection_context: CollectionContext,
    pub validation_metadata: ValidationMetadata,
}

/// Collection context for operations
#[derive(Debug, Clone)]
pub struct CollectionContext {
    pub tenant_id: String,
    pub collection_id: String,
    pub operation: CollectionOperation,
}

/// Validation metadata for audit
#[derive(Debug, Clone)]
pub struct ValidationMetadata {
    pub validated_at: DateTime<Utc>,
    pub permissions_checked: HashSet<Permission>,
    pub validation_reason: String,
}

/// Permission result for RBAC operations
#[derive(Debug, Clone)]
pub struct PermissionResult {
    pub allowed: bool,
    pub reason: String,
}

/// RBAC event logger for audit trails
pub struct RBACEventLogger {
    audit_events: Arc<DashMap<String, RBACEvent>>,
}

impl RBACEventLogger {
    pub fn new() -> Self {
        Self {
            audit_events: Arc::new(DashMap::new()),
        }
    }

    pub async fn log_role_created(
        &self,
        tenant_id: &str,
        role_name: &str,
        permissions: &HashSet<Permission>,
        creator_context: &UserContext,
    ) -> Result<()> {
        let event = RBACEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: RBACEventType::RoleCreated {
                role_name: role_name.to_string(),
                permissions_count: permissions.len(),
            },
            tenant_id: tenant_id.to_string(),
            user_id: creator_context.user_id.clone(),
            timestamp: Utc::now(),
        };

        self.audit_events.insert(event.event_id.clone(), event);
        Ok(())
    }

    pub async fn log_role_assigned(
        &self,
        tenant_id: &str,
        assigned_user_id: &str,
        role_name: &str,
        assigner_context: &UserContext,
    ) -> Result<()> {
        let event = RBACEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: RBACEventType::RoleAssigned {
                assigned_user_id: assigned_user_id.to_string(),
                role_name: role_name.to_string(),
            },
            tenant_id: tenant_id.to_string(),
            user_id: assigner_context.user_id.clone(),
            timestamp: Utc::now(),
        };

        self.audit_events.insert(event.event_id.clone(), event);
        Ok(())
    }

    pub async fn log_access_validation(
        &self,
        tenant_id: &str,
        collection_id: &str,
        operation: &CollectionOperation,
        user_context: &UserContext,
        access_granted: bool,
    ) -> Result<()> {
        let event = RBACEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: RBACEventType::AccessValidation {
                collection_id: collection_id.to_string(),
                operation: format!("{:?}", operation),
                access_granted,
            },
            tenant_id: tenant_id.to_string(),
            user_id: user_context.user_id.clone(),
            timestamp: Utc::now(),
        };

        self.audit_events.insert(event.event_id.clone(), event);
        Ok(())
    }
}

/// RBAC audit event
#[derive(Debug, Clone)]
pub struct RBACEvent {
    pub event_id: String,
    pub event_type: RBACEventType,
    pub tenant_id: String,
    pub user_id: String,
    pub timestamp: DateTime<Utc>,
}

/// RBAC event types for audit
#[derive(Debug, Clone)]
pub enum RBACEventType {
    RoleCreated {
        role_name: String,
        permissions_count: usize,
    },
    RoleAssigned {
        assigned_user_id: String,
        role_name: String,
    },
    AccessValidation {
        collection_id: String,
        operation: String,
        access_granted: bool,
    },
    PermissionDenied {
        resource: String,
        operation: String,
        reason: String,
    },
}

/// Default role creation helpers
impl EnhancedRBACManager {
    /// Create standard enterprise roles for tenant
    pub async fn create_standard_roles(
        &self,
        tenant_id: &str,
        creator_context: &UserContext,
    ) -> Result<Vec<TenantRole>> {
        let mut created_roles = Vec::new();

        // Admin role
        let admin_role = self
            .create_tenant_role(
                tenant_id,
                "tenant_admin",
                [Permission::TenantAdmin].into_iter().collect(),
                "Full tenant administration access".to_string(),
                creator_context,
            )
            .await?;
        created_roles.push(admin_role);

        // User role
        let user_role = self
            .create_tenant_role(
                tenant_id,
                "tenant_user",
                [Permission::TenantRead].into_iter().collect(),
                "Basic tenant read access".to_string(),
                creator_context,
            )
            .await?;
        created_roles.push(user_role);

        // Analyst role
        let analyst_role = self
            .create_tenant_role(
                tenant_id,
                "analyst",
                [
                    Permission::TenantRead,
                    Permission::DomainRead("analytics".to_string()),
                    Permission::CollectionRead("*".to_string()),
                ]
                .into_iter()
                .collect(),
                "Analytics and reporting access".to_string(),
                creator_context,
            )
            .await?;
        created_roles.push(analyst_role);

        Ok(created_roles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tenant::context::ResourceLimits;
    use crate::storage::tenant::{ComplianceFramework, Industry, SecurityPolicies, TenantConfig};

    async fn create_test_rbac_setup() -> (EnhancedRBACManager, UserContext) {
        let tenant_manager = Arc::new(TenantManager::new());

        // Create test tenant
        let tenant_config = TenantConfig {
            organization_name: "Test Corp".to_string(),
            industry: Industry::Financial,
            compliance_requirements: vec![ComplianceFramework::SOC2, ComplianceFramework::SOX],
            resource_limits: ResourceLimits::default(),
            security_policies: SecurityPolicies::default(),
        };

        tenant_manager
            .create_tenant("test_tenant".to_string(), tenant_config)
            .await
            .unwrap();

        let rbac_manager = EnhancedRBACManager::new(tenant_manager);

        let admin_context = UserContext {
            user_id: "admin_user".to_string(),
            tenant_id: "test_tenant".to_string(),
            roles: vec!["system_admin".to_string()],
            permissions: vec!["tenant_admin".to_string(), "system_admin".to_string()],
        };

        (rbac_manager, admin_context)
    }

    #[tokio::test]
    async fn test_role_creation() {
        let (rbac_manager, admin_context) = create_test_rbac_setup().await;

        let permissions = [
            Permission::CollectionRead("products".to_string()),
            Permission::CollectionWrite("products".to_string()),
        ]
        .into_iter()
        .collect();

        let role = rbac_manager
            .create_tenant_role(
                "test_tenant",
                "product_manager",
                permissions,
                "Product management role".to_string(),
                &admin_context,
            )
            .await
            .unwrap();

        assert_eq!(role.role_name, "product_manager");
        assert_eq!(role.tenant_id, "test_tenant");
        assert_eq!(role.permissions.len(), 2);
    }

    #[tokio::test]
    async fn test_user_role_assignment() {
        let (rbac_manager, admin_context) = create_test_rbac_setup().await;

        // Create role first
        let permissions = [Permission::CollectionRead("test_collection".to_string())]
            .into_iter()
            .collect();
        rbac_manager
            .create_tenant_role(
                "test_tenant",
                "test_role",
                permissions,
                "Test role".to_string(),
                &admin_context,
            )
            .await
            .unwrap();

        // Assign role to user
        let result = rbac_manager
            .assign_user_role("test_tenant", "test_user", "test_role", &admin_context)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_standard_roles_creation() {
        let (rbac_manager, admin_context) = create_test_rbac_setup().await;

        let roles = rbac_manager
            .create_standard_roles("test_tenant", &admin_context)
            .await
            .unwrap();

        assert_eq!(roles.len(), 3); // admin, user, analyst

        let role_names: Vec<String> = roles.iter().map(|r| r.role_name.clone()).collect();
        assert!(role_names.contains(&"tenant_admin".to_string()));
        assert!(role_names.contains(&"tenant_user".to_string()));
        assert!(role_names.contains(&"analyst".to_string()));
    }
}
