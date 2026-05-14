//! Security composition port trait for `proximadb-runtime`.
//!
//! `SecurityPort` is the stable contract that protocol adapters
//! (`proximadb-api`) use to call into the security subsystem without
//! importing root-crate concrete types.  Every method maps directly to a
//! gRPC `SecurityService` RPC so the adapter layer is a thin translation
//! with no policy logic of its own.
//!
//! ## Tenant contract
//!
//! `tenant_id` flows through proto request fields.  The implementing
//! service (`SecurityCoordinator`) resolves tenant context internally so
//! no root-crate `TenantContext` crosses this boundary.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_proto::v1::{
    AssignRoleRequest, AssignRoleResponse, BatchValidateAccessRequest, BatchValidateAccessResponse,
    CreateRoleRequest, CreateRoleResponse, DeleteRoleRequest, DeleteRoleResponse,
    GetTenantSecurityPolicyRequest, GetTenantSecurityPolicyResponse, ListAuditEventsRequest,
    ListAuditEventsResponse, ListRolesRequest, ListRolesResponse, ListUserRolesRequest,
    ListUserRolesResponse, RevokeRoleRequest, RevokeRoleResponse, SetTenantSecurityPolicyRequest,
    SetTenantSecurityPolicyResponse, ValidateAccessRequest, ValidateAccessResponse,
};

/// Port for security operations (RBAC, audit, tenant policy).
///
/// Implemented by root-crate `SecurityCoordinator`.  The protocol adapter
/// layer holds `Option<Arc<dyn SecurityPort>>`; when absent, services
/// return a safe-default "access denied" or "not configured" response.
#[async_trait]
pub trait SecurityPort: Send + Sync {
    /// Validate whether a principal may perform an action on a resource.
    async fn validate_access(
        &self,
        request: ValidateAccessRequest,
    ) -> Result<ValidateAccessResponse>;

    /// Validate multiple access requests in one call.
    async fn batch_validate_access(
        &self,
        request: BatchValidateAccessRequest,
    ) -> Result<BatchValidateAccessResponse>;

    /// Create a new role with a set of permissions.
    async fn create_role(&self, request: CreateRoleRequest) -> Result<CreateRoleResponse>;

    /// List all roles, optionally filtered.
    async fn list_roles(&self, request: ListRolesRequest) -> Result<ListRolesResponse>;

    /// Delete a role by ID.
    async fn delete_role(&self, request: DeleteRoleRequest) -> Result<DeleteRoleResponse>;

    /// Assign a role to a principal.
    async fn assign_role(&self, request: AssignRoleRequest) -> Result<AssignRoleResponse>;

    /// Revoke a role from a principal.
    async fn revoke_role(&self, request: RevokeRoleRequest) -> Result<RevokeRoleResponse>;

    /// List all roles assigned to a principal.
    async fn list_user_roles(
        &self,
        request: ListUserRolesRequest,
    ) -> Result<ListUserRolesResponse>;

    /// Retrieve audit events matching the given filter.
    async fn list_audit_events(
        &self,
        request: ListAuditEventsRequest,
    ) -> Result<ListAuditEventsResponse>;

    /// Retrieve the security policy for a tenant.
    async fn get_tenant_security_policy(
        &self,
        request: GetTenantSecurityPolicyRequest,
    ) -> Result<GetTenantSecurityPolicyResponse>;

    /// Update the security policy for a tenant.
    async fn set_tenant_security_policy(
        &self,
        request: SetTenantSecurityPolicyRequest,
    ) -> Result<SetTenantSecurityPolicyResponse>;
}
