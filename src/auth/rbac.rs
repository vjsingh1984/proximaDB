//! RBAC re-export from storage tenant module for clean API

pub use crate::storage::tenant::rbac::{
    AccessValidationResult, CollectionOperation, CollectionPermissions, DomainPermissions,
    EnhancedRBACManager, Permission, TenantRole, UserRoleAssignment,
};

// Additional enterprise RBAC types can be added here as needed
