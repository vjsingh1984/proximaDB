//! RBAC re-export from storage tenant module for clean API

pub use crate::storage::tenant::rbac::{
    EnhancedRBACManager,
    Permission,
    TenantRole,
    CollectionPermissions,
    DomainPermissions,
    UserRoleAssignment,
    CollectionOperation,
    AccessValidationResult,
};

// Additional enterprise RBAC types can be added here as needed