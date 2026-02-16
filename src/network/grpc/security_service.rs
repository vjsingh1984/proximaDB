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

use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, info};
use chrono::Utc;

use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::security_service_server::{SecurityService, SecurityServiceServer};
use crate::security::unified_rbac::{
    ConsolidatedRBACManager, RBACConfig, UnifiedPermission, UnifiedUserContext,
};

/// Security service implementation for RBAC operations
pub struct SecurityServiceImpl {
    /// RBAC manager for permission validation
    rbac_manager: Arc<ConsolidatedRBACManager>,
}

impl SecurityServiceImpl {
    /// Create a new security service
    pub fn new(rbac_manager: Arc<ConsolidatedRBACManager>) -> Self {
        Self { rbac_manager }
    }

    /// Create a new security service with default config
    pub fn with_default_config() -> Self {
        let config = RBACConfig::default();
        let rbac_manager = Arc::new(ConsolidatedRBACManager::new(config));
        Self::new(rbac_manager)
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
            auth_method: crate::security::unified_rbac::AuthMethod::Internal,
            session_id: auth_ctx.session_id.clone(),
            expires_at,
            created_at: chrono::Utc::now(),
            metadata: std::collections::HashMap::new(),
        })
    }

    /// Validate that the caller has admin permissions
    async fn validate_admin_access(&self, auth_ctx: &Option<proximadb_v1::AuthContext>) -> Result<(), Status> {
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
            ("collection", "read") => Ok(UnifiedPermission::CollectionRead(resource_id.to_string())),
            ("collection", "write") => Ok(UnifiedPermission::CollectionWrite(resource_id.to_string())),
            ("collection", "delete") => Ok(UnifiedPermission::CollectionDelete(resource_id.to_string())),
            ("collection", "admin") => Ok(UnifiedPermission::CollectionAdmin(resource_id.to_string())),
            ("collection", "vector_search") => Ok(UnifiedPermission::VectorSearch(resource_id.to_string())),
            ("collection", "vector_insert") => Ok(UnifiedPermission::VectorInsert(resource_id.to_string())),
            ("graph", "traverse") => Ok(UnifiedPermission::GraphTraverse(resource_id.to_string())),
            ("graph", "create_relations") => Ok(UnifiedPermission::GraphCreateRelations(resource_id.to_string())),
            ("graph", "delete_relations") => Ok(UnifiedPermission::GraphDeleteRelations(resource_id.to_string())),
            _ => Ok(UnifiedPermission::SystemAdmin), // Fallback for unknown operations
        }
    }

    /// Convert to gRPC service
    pub fn into_server(self) -> SecurityServiceServer<Self> {
        SecurityServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl SecurityService for SecurityServiceImpl {
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
            .operation_to_permission(&req.resource_type, &req.resource_id, &req.operation, &req.data_model)
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
            auth_method: crate::security::unified_rbac::AuthMethod::Internal,
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
            response.missing_permissions.push(format!("{:?}", permission));
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

        let role = req.role.ok_or_else(|| Status::invalid_argument("Missing role"))?;

        info!("CreateRole: role_name={}, tenant_id={}", role.role_name, role.tenant_id);

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
                    &req.auth_context.as_ref().map(|c| c.user_id.clone()).unwrap_or_default(),
                )
                .await
                .map_err(|e| Status::internal(format!("Failed to create role: {}", e)))?;
        }

        Ok(Response::new(proximadb_v1::CreateRoleResponse { role: Some(role) }))
    }

    async fn list_roles(
        &self,
        _request: Request<proximadb_v1::ListRolesRequest>,
    ) -> Result<Response<proximadb_v1::ListRolesResponse>, Status> {
        // TODO: Implement list roles from RBAC manager
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

        // TODO: Implement delete role in RBAC manager
        Ok(Response::new(proximadb_v1::DeleteRoleResponse { success: true }))
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
                    if req.tenant_id.is_empty() { None } else { Some(req.tenant_id.as_str()) },
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

        Ok(Response::new(proximadb_v1::AssignRoleResponse { assignment: Some(assignment) }))
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

        // TODO: Implement revoke role in RBAC manager
        Ok(Response::new(proximadb_v1::RevokeRoleResponse { success: true }))
    }

    async fn list_user_roles(
        &self,
        request: Request<proximadb_v1::ListUserRolesRequest>,
    ) -> Result<Response<proximadb_v1::ListUserRolesResponse>, Status> {
        let req = request.into_inner();

        debug!("ListUserRoles: user_id={}, tenant_id={}", req.user_id, req.tenant_id);

        // TODO: Implement list user roles from RBAC manager
        Ok(Response::new(proximadb_v1::ListUserRolesResponse {
            assignments: vec![],
        }))
    }

    async fn list_audit_events(
        &self,
        _request: Request<proximadb_v1::ListAuditEventsRequest>,
    ) -> Result<Response<proximadb_v1::ListAuditEventsResponse>, Status> {
        // TODO: Implement audit event listing
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

        // TODO: Implement tenant security policy retrieval
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

        Ok(Response::new(proximadb_v1::GetTenantSecurityPolicyResponse { policy: Some(policy) }))
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
            req.policy.as_ref().map(|p| &p.tenant_id).unwrap_or(&String::new())
        );

        // TODO: Implement tenant security policy setting
        Ok(Response::new(proximadb_v1::SetTenantSecurityPolicyResponse { success: true }))
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
}
