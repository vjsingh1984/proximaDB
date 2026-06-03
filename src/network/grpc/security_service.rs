// DEPRECATED: This file has been migrated to crates/platform/proximadb-api/src/grpc/v1/security.rs
// Please use: use proximadb_api::grpc::SecurityServiceImpl;
// This compatibility shim will be removed in version 0.3.0

/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! gRPC Security Service for RBAC operations
//!
//! Implements the SecurityService for role-based access control,
//! permission validation, and audit logging.

use chrono::Utc;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, info};

use crate::proto::proximadb_v1;
use crate::security::auth_service::{AuthenticationData, UnifiedAuthService};
use crate::security::rbac_service::{
    ConsolidatedRBACManager, RBACConfig, UnifiedAuthMethod, UnifiedPermission, UnifiedUserContext,
};

/// Security service implementation for RBAC operations
pub struct SecurityServiceImpl {
    /// RBAC manager for permission validation
    rbac_manager: Arc<ConsolidatedRBACManager>,
    /// Optional unified auth service for `SecurityPort::authenticate`
    /// (ADR-016 / Task #69).  When `None`, the port's `authenticate`
    /// method returns "not configured"; the existing gRPC RBAC methods
    /// continue to work without it.
    auth_service: Option<Arc<UnifiedAuthService>>,
}

impl SecurityServiceImpl {
    /// Create a new security service
    pub fn new(rbac_manager: Arc<ConsolidatedRBACManager>) -> Self {
        Self {
            rbac_manager,
            auth_service: None,
        }
    }

    /// Create a new security service with default config
    pub fn with_default_config() -> Self {
        let config = RBACConfig::default();
        let rbac_manager = Arc::new(ConsolidatedRBACManager::new(config));
        Self::new(rbac_manager)
    }

    /// Wire in the unified auth service so the port's
    /// `authenticate(PortAuthCredential)` method can actually
    /// authenticate.  Without this, `authenticate` returns
    /// "not configured" — composition is opt-in.
    pub fn with_auth_service(mut self, auth_service: Arc<UnifiedAuthService>) -> Self {
        self.auth_service = Some(auth_service);
        self
    }

    /// Convert gRPC auth context to UnifiedUserContext
    fn auth_context_to_user_context(
        &self,
        auth_ctx: &Option<proximadb_v1::AuthContext>,
    ) -> Result<UnifiedUserContext, Status> {
        let auth_ctx = auth_ctx
            .as_ref()
            .ok_or_else(|| Status::unauthenticated("Missing auth context"))?;

        // Convert prost_types::Timestamp to chrono::DateTime<Utc>
        let expires_at = auth_ctx
            .expires_at
            .as_ref()
            .and_then(|ts| chrono::DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32));

        Ok(UnifiedUserContext {
            user_id: auth_ctx.user_id.clone(),
            tenant_id: if auth_ctx.tenant_id.is_empty() {
                None
            } else {
                Some(auth_ctx.tenant_id.clone())
            },
            roles: Vec::new(),
            effective_permissions: std::collections::HashSet::new(),
            auth_method: UnifiedAuthMethod::Internal,
            session_id: auth_ctx.session_id.clone(),
            expires_at,
            created_at: chrono::Utc::now(),
            metadata: std::collections::HashMap::new(),
        })
    }

    /// Validate that the caller has admin permissions
    async fn validate_admin_access(
        &self,
        auth_ctx: &Option<proximadb_v1::AuthContext>,
    ) -> Result<(), Status> {
        let user_ctx = self.auth_context_to_user_context(auth_ctx)?;
        let has_admin = self
            .rbac_manager
            .check_permission_cached(&user_ctx.user_id, &UnifiedPermission::SystemAdmin)
            .await
            .map_err(|e| Status::internal(format!("Failed to check admin permission: {}", e)))?;

        if !has_admin {
            return Err(Status::permission_denied("Insufficient permissions"));
        }

        Ok(())
    }

    /// Convert string operation to UnifiedPermission
    fn operation_to_permission(
        &self,
        resource_type: &str,
        resource_id: &str,
        operation: &str,
        _data_model: &str,
    ) -> Result<UnifiedPermission, Status> {
        match (resource_type, operation) {
            ("collection", "read") => {
                Ok(UnifiedPermission::CollectionRead(resource_id.to_string()))
            }
            ("collection", "write") => {
                Ok(UnifiedPermission::CollectionWrite(resource_id.to_string()))
            }
            ("collection", "delete") => {
                Ok(UnifiedPermission::CollectionDelete(resource_id.to_string()))
            }
            ("collection", "admin") => {
                Ok(UnifiedPermission::CollectionAdmin(resource_id.to_string()))
            }
            ("collection", "vector_search") => {
                Ok(UnifiedPermission::VectorSearch(resource_id.to_string()))
            }
            ("collection", "vector_insert") => {
                Ok(UnifiedPermission::VectorInsert(resource_id.to_string()))
            }
            ("graph", "traverse") => Ok(UnifiedPermission::GraphTraverse(resource_id.to_string())),
            ("graph", "create_relations") => Ok(UnifiedPermission::GraphCreateRelations(
                resource_id.to_string(),
            )),
            ("graph", "delete_relations") => Ok(UnifiedPermission::GraphDeleteRelations(
                resource_id.to_string(),
            )),
            _ => Ok(UnifiedPermission::SystemAdmin), // Fallback for unknown operations
        }
    }
}

// Inherent security operation handlers. These hold the real RBAC/auth logic
// behind `SecurityPort` (below); the canonical tonic `SecurityService` wire
// adapter lives in crates/platform/proximadb-api/src/grpc/v1/security.rs. TD-105
// Phase B converted this from a (never-served) tonic `impl SecurityService` into
// plain inherent methods.
impl SecurityServiceImpl {
    async fn validate_access(
        &self,
        request: Request<proximadb_v1::ValidateAccessRequest>,
    ) -> Result<Response<proximadb_v1::ValidateAccessResponse>, Status> {
        let req = request.into_inner();

        debug!(
            "ValidateAccess: user_id={}, resource_type={}, resource_id={}, operation={}",
            req.user_id, req.resource_type, req.resource_id, req.operation
        );

        // Convert operation string to UnifiedPermission
        let permission = self
            .operation_to_permission(
                &req.resource_type,
                &req.resource_id,
                &req.operation,
                &req.data_model,
            )
            .map_err(|e| Status::invalid_argument(format!("Invalid operation: {}", e)))?;

        // Create user context
        let _user_ctx = UnifiedUserContext {
            user_id: req.user_id.clone(),
            tenant_id: if req.tenant_id.is_empty() {
                None
            } else {
                Some(req.tenant_id.clone())
            },
            roles: Vec::new(),
            effective_permissions: std::collections::HashSet::new(),
            auth_method: UnifiedAuthMethod::Internal,
            session_id: uuid::Uuid::new_v4().to_string(),
            expires_at: None,
            created_at: chrono::Utc::now(),
            metadata: std::collections::HashMap::new(),
        };

        // Check permission
        let allowed = self
            .rbac_manager
            .check_permission_cached(&req.user_id, &permission)
            .await
            .map_err(|e| Status::internal(format!("Permission check failed: {}", e)))?;

        let mut response = proximadb_v1::ValidateAccessResponse {
            allowed,
            reason: if allowed {
                String::new()
            } else {
                "Insufficient permissions".to_string()
            },
            missing_permissions: vec![],
            checked_permissions: vec![format!("{:?}", permission)],
        };

        if !allowed {
            response
                .missing_permissions
                .push(format!("{:?}", permission));
        }

        Ok(Response::new(response))
    }

    async fn batch_validate_access(
        &self,
        request: Request<proximadb_v1::BatchValidateAccessRequest>,
    ) -> Result<Response<proximadb_v1::BatchValidateAccessResponse>, Status> {
        let req = request.into_inner();
        let mut responses = Vec::new();

        for validate_req in req.requests {
            // Create a single-request wrapper
            let tonic_request = Request::new(validate_req);
            let response = self.validate_access(tonic_request).await?;
            responses.push(response.into_inner());
        }

        Ok(Response::new(proximadb_v1::BatchValidateAccessResponse {
            responses,
        }))
    }

    async fn create_role(
        &self,
        request: Request<proximadb_v1::CreateRoleRequest>,
    ) -> Result<Response<proximadb_v1::CreateRoleResponse>, Status> {
        let req = request.into_inner();

        // Validate admin access
        self.validate_admin_access(&req.auth_context).await?;

        let role = req
            .role
            .ok_or_else(|| Status::invalid_argument("Missing role"))?;

        info!(
            "CreateRole: role_name={}, tenant_id={}",
            role.role_name, role.tenant_id
        );

        // Convert proto permissions to UnifiedPermission
        let permissions: std::collections::HashSet<UnifiedPermission> = role
            .permissions
            .iter()
            .filter_map(|p| serde_json::from_str::<UnifiedPermission>(p).ok())
            .collect();

        // Create tenant role
        if !role.tenant_id.is_empty() {
            self.rbac_manager
                .create_tenant_role(
                    &role.tenant_id,
                    &role.role_name,
                    permissions,
                    &req.auth_context
                        .as_ref()
                        .map(|c| c.user_id.clone())
                        .unwrap_or_default(),
                )
                .await
                .map_err(|e| Status::internal(format!("Failed to create role: {}", e)))?;
        }

        Ok(Response::new(proximadb_v1::CreateRoleResponse {
            role: Some(role),
        }))
    }

    async fn list_roles(
        &self,
        _request: Request<proximadb_v1::ListRolesRequest>,
    ) -> Result<Response<proximadb_v1::ListRolesResponse>, Status> {
        // Deferred: Implement list roles from RBAC manager
        Ok(Response::new(proximadb_v1::ListRolesResponse {
            roles: vec![],
            next_page_token: String::new(),
        }))
    }

    async fn delete_role(
        &self,
        request: Request<proximadb_v1::DeleteRoleRequest>,
    ) -> Result<Response<proximadb_v1::DeleteRoleResponse>, Status> {
        let req = request.into_inner();

        // Validate admin access
        self.validate_admin_access(&req.auth_context).await?;

        info!("DeleteRole: role_id={}", req.role_id);

        // Deferred: Implement delete role in RBAC manager
        Ok(Response::new(proximadb_v1::DeleteRoleResponse {
            success: true,
        }))
    }

    async fn assign_role(
        &self,
        request: Request<proximadb_v1::AssignRoleRequest>,
    ) -> Result<Response<proximadb_v1::AssignRoleResponse>, Status> {
        let req = request.into_inner();

        // Validate admin access
        self.validate_admin_access(&req.auth_context).await?;

        info!(
            "AssignRole: user_id={}, tenant_id={}, roles={:?}",
            req.user_id, req.tenant_id, req.roles
        );

        let assigned_by = req
            .auth_context
            .as_ref()
            .map(|c| c.user_id.clone())
            .unwrap_or_default();

        // Assign each role
        for role_name in &req.roles {
            self.rbac_manager
                .assign_role_to_user(
                    &req.user_id,
                    if req.tenant_id.is_empty() {
                        None
                    } else {
                        Some(req.tenant_id.as_str())
                    },
                    role_name,
                    &assigned_by,
                )
                .await
                .map_err(|e| Status::internal(format!("Failed to assign role: {}", e)))?;
        }

        let assignment = proximadb_v1::UserRoleAssignment {
            user_id: req.user_id,
            tenant_id: req.tenant_id,
            roles: req.roles,
            assigned_at: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            assigned_by,
            expires_at: req.expires_at,
        };

        Ok(Response::new(proximadb_v1::AssignRoleResponse {
            assignment: Some(assignment),
        }))
    }

    async fn revoke_role(
        &self,
        request: Request<proximadb_v1::RevokeRoleRequest>,
    ) -> Result<Response<proximadb_v1::RevokeRoleResponse>, Status> {
        let req = request.into_inner();

        // Validate admin access
        self.validate_admin_access(&req.auth_context).await?;

        info!(
            "RevokeRole: user_id={}, tenant_id={}, roles={:?}",
            req.user_id, req.tenant_id, req.roles
        );

        // Deferred: Implement revoke role in RBAC manager
        Ok(Response::new(proximadb_v1::RevokeRoleResponse {
            success: true,
        }))
    }

    async fn list_user_roles(
        &self,
        request: Request<proximadb_v1::ListUserRolesRequest>,
    ) -> Result<Response<proximadb_v1::ListUserRolesResponse>, Status> {
        let req = request.into_inner();

        debug!(
            "ListUserRoles: user_id={}, tenant_id={}",
            req.user_id, req.tenant_id
        );

        // Deferred: Implement list user roles from RBAC manager
        Ok(Response::new(proximadb_v1::ListUserRolesResponse {
            assignments: vec![],
        }))
    }

    async fn list_audit_events(
        &self,
        _request: Request<proximadb_v1::ListAuditEventsRequest>,
    ) -> Result<Response<proximadb_v1::ListAuditEventsResponse>, Status> {
        // Deferred: Implement audit event listing
        Ok(Response::new(proximadb_v1::ListAuditEventsResponse {
            events: vec![],
            next_page_token: String::new(),
            total_count: 0,
        }))
    }

    async fn get_tenant_security_policy(
        &self,
        request: Request<proximadb_v1::GetTenantSecurityPolicyRequest>,
    ) -> Result<Response<proximadb_v1::GetTenantSecurityPolicyResponse>, Status> {
        let req = request.into_inner();

        debug!("GetTenantSecurityPolicy: tenant_id={}", req.tenant_id);

        // Deferred: Implement tenant security policy retrieval
        let policy = proximadb_v1::TenantSecurityPolicy {
            tenant_id: req.tenant_id,
            require_mfa: true,
            allowed_ip_ranges: vec![],
            session_timeout_seconds: 28800, // 8 hours
            require_password_confirmation: false,
            audit_all_operations: true,
            encryption_policy: Some(proximadb_v1::DataEncryptionPolicy {
                encrypt_at_rest: true,
                encrypt_in_transit: true,
                key_id: String::new(),
                key_rotation_days: 90,
            }),
        };

        Ok(Response::new(
            proximadb_v1::GetTenantSecurityPolicyResponse {
                policy: Some(policy),
            },
        ))
    }

    async fn set_tenant_security_policy(
        &self,
        request: Request<proximadb_v1::SetTenantSecurityPolicyRequest>,
    ) -> Result<Response<proximadb_v1::SetTenantSecurityPolicyResponse>, Status> {
        let req = request.into_inner();

        // Validate admin access
        self.validate_admin_access(&req.auth_context).await?;

        info!(
            "SetTenantSecurityPolicy: tenant_id={}",
            req.policy.as_ref().map_or(&String::new(), |p| &p.tenant_id)
        );

        // Deferred: Implement tenant security policy setting
        Ok(Response::new(
            proximadb_v1::SetTenantSecurityPolicyResponse { success: true },
        ))
    }
}

// ── SecurityPort impl ─────────────────────────────────────────────────────────
//
// Delegates every port method to the inherent handlers above, so the logic is
// written once and the port is a thin unwrap/wrap layer. `authenticate` is the
// one port-only method (no inherent handler) and carries its own logic.

#[async_trait::async_trait]
impl proximadb_runtime::SecurityPort for SecurityServiceImpl {
    async fn validate_access(
        &self,
        request: crate::proto::proximadb_v1::ValidateAccessRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::ValidateAccessResponse> {
        self.validate_access(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn batch_validate_access(
        &self,
        request: crate::proto::proximadb_v1::BatchValidateAccessRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::BatchValidateAccessResponse> {
        self.batch_validate_access(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn create_role(
        &self,
        request: crate::proto::proximadb_v1::CreateRoleRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::CreateRoleResponse> {
        self.create_role(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn list_roles(
        &self,
        request: crate::proto::proximadb_v1::ListRolesRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::ListRolesResponse> {
        self.list_roles(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn delete_role(
        &self,
        request: crate::proto::proximadb_v1::DeleteRoleRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::DeleteRoleResponse> {
        self.delete_role(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn assign_role(
        &self,
        request: crate::proto::proximadb_v1::AssignRoleRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::AssignRoleResponse> {
        self.assign_role(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn revoke_role(
        &self,
        request: crate::proto::proximadb_v1::RevokeRoleRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::RevokeRoleResponse> {
        self.revoke_role(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn list_user_roles(
        &self,
        request: crate::proto::proximadb_v1::ListUserRolesRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::ListUserRolesResponse> {
        self.list_user_roles(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn list_audit_events(
        &self,
        request: crate::proto::proximadb_v1::ListAuditEventsRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::ListAuditEventsResponse> {
        self.list_audit_events(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn get_tenant_security_policy(
        &self,
        request: crate::proto::proximadb_v1::GetTenantSecurityPolicyRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::GetTenantSecurityPolicyResponse> {
        self.get_tenant_security_policy(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn set_tenant_security_policy(
        &self,
        request: crate::proto::proximadb_v1::SetTenantSecurityPolicyRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::SetTenantSecurityPolicyResponse> {
        self.set_tenant_security_policy(tonic::Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    /// ADR-016 / Task #69: lossy-projection authenticate.
    ///
    /// Converts `PortAuthCredential` → root-crate `AuthenticationData`,
    /// calls the unified auth service, fills in effective permissions
    /// via the RBAC manager (mirroring `SecurityCoordinator::authenticate_request`),
    /// then projects `UnifiedUserContext` → `PortUserContext`.
    ///
    /// Returns "not configured" when no auth service is wired —
    /// composition is opt-in via `with_auth_service`.
    async fn authenticate(
        &self,
        credential: proximadb_runtime::PortAuthCredential,
    ) -> anyhow::Result<proximadb_runtime::PortUserContext> {
        let auth_service = self.auth_service.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "SecurityPort::authenticate: auth_service not configured on this SecurityServiceImpl (call with_auth_service to enable)"
            )
        })?;

        let auth_data = match credential {
            proximadb_runtime::PortAuthCredential::Jwt(token) => {
                AuthenticationData::JWTToken(token)
            }
            proximadb_runtime::PortAuthCredential::ApiKey(key) => AuthenticationData::ApiKey(key),
        };

        let auth_result = auth_service.authenticate(auth_data).await?;
        if !auth_result.success {
            return Err(anyhow::anyhow!(
                "Authentication failed: {}",
                auth_result.error_message.as_deref().unwrap_or("unknown")
            ));
        }

        let mut user_context = auth_result.user_context;
        let effective_permissions = self
            .rbac_manager
            .get_effective_permissions(&user_context)
            .await?;
        user_context.effective_permissions = effective_permissions;

        Ok(project_unified_to_port_context(user_context))
    }
}

/// Lossy projection of root-crate `UnifiedUserContext` →
/// port-level `PortUserContext` (ADR-016).
fn project_unified_to_port_context(ctx: UnifiedUserContext) -> proximadb_runtime::PortUserContext {
    let effective_permissions_json =
        serde_json::to_string(&ctx.effective_permissions).unwrap_or_else(|_| "[]".to_string());
    let auth_method = match ctx.auth_method {
        UnifiedAuthMethod::SSO { .. } => "sso".to_string(),
        UnifiedAuthMethod::JWT => "jwt".to_string(),
        UnifiedAuthMethod::ApiKey => "apikey".to_string(),
        UnifiedAuthMethod::ClientCertificate => "mtls".to_string(),
        UnifiedAuthMethod::Internal => "internal".to_string(),
    };
    let session_id = if ctx.session_id.is_empty() {
        None
    } else {
        Some(ctx.session_id)
    };
    proximadb_runtime::PortUserContext {
        user_id: ctx.user_id,
        tenant_id: ctx.tenant_id.unwrap_or_default(),
        roles: ctx.roles,
        scopes: Vec::new(),
        effective_permissions_json,
        auth_method,
        session_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_security_service_creation() {
        let service = SecurityServiceImpl::with_default_config();
        assert!(Arc::strong_count(&service.rbac_manager) == 1);
    }

    #[tokio::test]
    async fn test_validate_access_basic() {
        let service = SecurityServiceImpl::with_default_config();

        let request = proximadb_v1::ValidateAccessRequest {
            user_id: "test_user".to_string(),
            tenant_id: "test_tenant".to_string(),
            resource_type: "collection".to_string(),
            resource_id: "test_collection".to_string(),
            operation: "read".to_string(),
            data_model: "vector".to_string(),
        };

        let result = service.validate_access(Request::new(request)).await;
        assert!(result.is_ok());

        let response = result.unwrap().into_inner();
        // Default behavior: default_deny = false, so unassigned users are allowed
        assert_eq!(response.allowed, true);
    }

    /// ADR-016 / Task #69: lock in the lossy-projection semantics so
    /// future step-4 consumer migrations can rely on the contract.
    mod port_user_context_projection {
        use super::*;
        use chrono::Utc;
        use std::collections::{HashMap, HashSet};

        fn ctx(
            tenant_id: Option<&str>,
            roles: &[&str],
            session_id: &str,
            auth_method: UnifiedAuthMethod,
            permissions: HashSet<UnifiedPermission>,
        ) -> UnifiedUserContext {
            UnifiedUserContext {
                user_id: "u1".to_string(),
                tenant_id: tenant_id.map(str::to_string),
                roles: roles.iter().map(|s| s.to_string()).collect(),
                effective_permissions: permissions,
                auth_method,
                session_id: session_id.to_string(),
                expires_at: None,
                created_at: Utc::now(),
                metadata: HashMap::new(),
            }
        }

        #[test]
        fn tenant_id_none_becomes_empty_string() {
            let p = project_unified_to_port_context(ctx(
                None,
                &[],
                "",
                UnifiedAuthMethod::Internal,
                HashSet::new(),
            ));
            assert_eq!(p.tenant_id, "");
        }

        #[test]
        fn tenant_id_some_passes_through() {
            let p = project_unified_to_port_context(ctx(
                Some("acme"),
                &[],
                "",
                UnifiedAuthMethod::Internal,
                HashSet::new(),
            ));
            assert_eq!(p.tenant_id, "acme");
        }

        #[test]
        fn empty_session_id_becomes_none() {
            let p = project_unified_to_port_context(ctx(
                None,
                &[],
                "",
                UnifiedAuthMethod::Internal,
                HashSet::new(),
            ));
            assert!(p.session_id.is_none());
        }

        #[test]
        fn nonempty_session_id_passes_through() {
            let p = project_unified_to_port_context(ctx(
                None,
                &[],
                "sess-42",
                UnifiedAuthMethod::Internal,
                HashSet::new(),
            ));
            assert_eq!(p.session_id.as_deref(), Some("sess-42"));
        }

        #[test]
        fn auth_method_stringification() {
            let cases = [
                (
                    UnifiedAuthMethod::SSO {
                        provider: "okta".into(),
                    },
                    "sso",
                ),
                (UnifiedAuthMethod::JWT, "jwt"),
                (UnifiedAuthMethod::ApiKey, "apikey"),
                (UnifiedAuthMethod::ClientCertificate, "mtls"),
                (UnifiedAuthMethod::Internal, "internal"),
            ];
            for (am, expected) in cases {
                let p = project_unified_to_port_context(ctx(None, &[], "", am, HashSet::new()));
                assert_eq!(p.auth_method, expected);
            }
        }

        #[test]
        fn roles_pass_through() {
            let p = project_unified_to_port_context(ctx(
                None,
                &["admin", "reader"],
                "",
                UnifiedAuthMethod::Internal,
                HashSet::new(),
            ));
            assert_eq!(p.roles, vec!["admin", "reader"]);
        }

        #[test]
        fn scopes_default_empty() {
            // No source field on UnifiedUserContext — projection always
            // emits an empty scopes vec.  Step 4 consumers needing
            // scopes will require a port-surface extension first.
            let p = project_unified_to_port_context(ctx(
                None,
                &["any"],
                "any",
                UnifiedAuthMethod::Internal,
                HashSet::new(),
            ));
            assert!(p.scopes.is_empty());
        }

        #[test]
        fn effective_permissions_serializes_to_json() {
            let mut perms = HashSet::new();
            perms.insert(UnifiedPermission::CollectionRead("c1".to_string()));
            let p = project_unified_to_port_context(ctx(
                None,
                &[],
                "",
                UnifiedAuthMethod::Internal,
                perms,
            ));
            // Opaque JSON to the runtime layer — assert it round-trips
            // through serde_json and contains the variant name so
            // consumers know they can re-deserialize with the
            // root-crate `UnifiedPermission` schema.
            let v: serde_json::Value =
                serde_json::from_str(&p.effective_permissions_json).expect("valid JSON");
            assert!(v.is_array());
            assert!(p.effective_permissions_json.contains("CollectionRead"));
        }
    }
}
