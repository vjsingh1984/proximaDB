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

/// Port-level user context (ADR-016): a minimal, lossy projection of the
/// root-crate `UnifiedUserContext`.  Production callers that need typed
/// permission objects continue to use the root-crate path via
/// `SecurityCoordinator` directly; cross-crate-boundary callers
/// (`proximadb-api` gRPC, future port-based middleware) consume this
/// projection.
///
/// All fields are owned strings or owned Vec<String>; no lifetime
/// parameters, no root-crate types, no proto types.
#[derive(Debug, Clone)]
pub struct PortUserContext {
    /// Opaque user identifier (string for forward-compat across auth providers).
    pub user_id: String,
    /// Tenant the request is being authenticated under.
    pub tenant_id: String,
    /// Roles assigned to the user — strings, not the typed root-crate enum.
    pub roles: Vec<String>,
    /// Scopes / capabilities granted to the request — strings.
    pub scopes: Vec<String>,
    /// JSON-serialized effective-permissions snapshot.  Opaque to the
    /// runtime layer; consumers either pass through or deserialize via
    /// the root-crate `UnifiedPermission` schema.
    pub effective_permissions_json: String,
    /// Authentication method label (e.g. "oidc", "ldap", "jwt", "apikey").
    pub auth_method: String,
    /// Optional session id (None for stateless auth).
    pub session_id: Option<String>,
}

/// Port-level authentication credential (ADR-016): a minimal projection
/// of the root-crate `AuthenticationData` enum.  Covers the two
/// most-common production paths (JWT for OIDC, API key for
/// service-to-service); SSO and mTLS remain on the typed root-crate
/// path until those flows have an articulated port need.
#[derive(Debug, Clone)]
pub enum PortAuthCredential {
    /// Bearer JWT (OIDC, custom).
    Jwt(String),
    /// API key for service-to-service auth.
    ApiKey(String),
}

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
    async fn list_user_roles(&self, request: ListUserRolesRequest)
    -> Result<ListUserRolesResponse>;

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

    /// Authenticate an incoming request and return a port-level user
    /// context.  Implementations on the root crate side construct this
    /// by lossy projection of `UnifiedUserContext` (ADR-016); full
    /// typed context stays accessible via `SecurityCoordinator`
    /// directly for callers that need typed permission objects.
    ///
    /// Default body returns "not implemented" so existing impls
    /// compile unchanged during the multi-step migration.  Step 2
    /// (separate commit) wires the real lossy-projection impl on
    /// `SecurityCoordinator`.
    async fn authenticate(&self, _credential: PortAuthCredential) -> Result<PortUserContext> {
        Err(anyhow::anyhow!(
            "SecurityPort::authenticate: not implemented on this SecurityPort impl (ADR-016 step 2 pending)"
        ))
    }
}
