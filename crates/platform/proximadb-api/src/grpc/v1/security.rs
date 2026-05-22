//! # Security Service (gRPC)
//!
//! gRPC implementation for RBAC operations, permission validation, and audit
//! logging.  Each RPC delegates to the injected `SecurityPort`; when no port
//! is provided the service returns a safe "not configured" status so the
//! server can start without a security backend.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use proximadb_proto::v1;
use proximadb_proto::v1::security_service_server::{SecurityService, SecurityServiceServer};
use proximadb_runtime::SecurityPort;

/// gRPC SecurityService implementation backed by a `SecurityPort`.
pub struct SecurityServiceImpl {
    port: Option<Arc<dyn SecurityPort>>,
}

impl SecurityServiceImpl {
    /// Construct with a concrete security port.
    pub fn new(port: Arc<dyn SecurityPort>) -> Self {
        Self { port: Some(port) }
    }

    /// Construct without a security backend (all RPCs return NOT_FOUND).
    pub fn with_default_config() -> Self {
        Self { port: None }
    }

    /// Convert into a tonic gRPC server.
    pub fn into_server(self) -> SecurityServiceServer<Self> {
        SecurityServiceServer::new(self)
    }

    fn not_configured() -> Status {
        Status::not_found("Security service not configured on this node")
    }

    fn port_err(e: anyhow::Error) -> Status {
        Status::internal(e.to_string())
    }
}

#[tonic::async_trait]
impl SecurityService for SecurityServiceImpl {
    async fn validate_access(
        &self,
        request: Request<v1::ValidateAccessRequest>,
    ) -> Result<Response<v1::ValidateAccessResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.validate_access(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn batch_validate_access(
        &self,
        request: Request<v1::BatchValidateAccessRequest>,
    ) -> Result<Response<v1::BatchValidateAccessResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.batch_validate_access(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn create_role(
        &self,
        request: Request<v1::CreateRoleRequest>,
    ) -> Result<Response<v1::CreateRoleResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.create_role(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn list_roles(
        &self,
        request: Request<v1::ListRolesRequest>,
    ) -> Result<Response<v1::ListRolesResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.list_roles(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn delete_role(
        &self,
        request: Request<v1::DeleteRoleRequest>,
    ) -> Result<Response<v1::DeleteRoleResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.delete_role(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn assign_role(
        &self,
        request: Request<v1::AssignRoleRequest>,
    ) -> Result<Response<v1::AssignRoleResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.assign_role(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn revoke_role(
        &self,
        request: Request<v1::RevokeRoleRequest>,
    ) -> Result<Response<v1::RevokeRoleResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.revoke_role(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn list_user_roles(
        &self,
        request: Request<v1::ListUserRolesRequest>,
    ) -> Result<Response<v1::ListUserRolesResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.list_user_roles(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn list_audit_events(
        &self,
        request: Request<v1::ListAuditEventsRequest>,
    ) -> Result<Response<v1::ListAuditEventsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.list_audit_events(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn get_tenant_security_policy(
        &self,
        request: Request<v1::GetTenantSecurityPolicyRequest>,
    ) -> Result<Response<v1::GetTenantSecurityPolicyResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_tenant_security_policy(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn set_tenant_security_policy(
        &self,
        request: Request<v1::SetTenantSecurityPolicyRequest>,
    ) -> Result<Response<v1::SetTenantSecurityPolicyResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.set_tenant_security_policy(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    fn assert_not_configured<T>(result: Result<Response<T>, Status>) {
        let err = match result {
            Ok(_) => panic!("backend-less security service should reject RPC"),
            Err(err) => err,
        };
        assert_eq!(err.code(), Code::NotFound);
        assert!(err.message().contains("Security service not configured"));
    }

    #[tokio::test]
    async fn default_security_service_rejects_every_rpc_with_not_found() {
        let service = SecurityServiceImpl::with_default_config();

        assert_not_configured(
            SecurityService::validate_access(&service, Request::new(Default::default())).await,
        );
        assert_not_configured(
            SecurityService::batch_validate_access(&service, Request::new(Default::default()))
                .await,
        );
        assert_not_configured(
            SecurityService::create_role(&service, Request::new(Default::default())).await,
        );
        assert_not_configured(
            SecurityService::list_roles(&service, Request::new(Default::default())).await,
        );
        assert_not_configured(
            SecurityService::delete_role(&service, Request::new(Default::default())).await,
        );
        assert_not_configured(
            SecurityService::assign_role(&service, Request::new(Default::default())).await,
        );
        assert_not_configured(
            SecurityService::revoke_role(&service, Request::new(Default::default())).await,
        );
        assert_not_configured(
            SecurityService::list_user_roles(&service, Request::new(Default::default())).await,
        );
        assert_not_configured(
            SecurityService::list_audit_events(&service, Request::new(Default::default())).await,
        );
        assert_not_configured(
            SecurityService::get_tenant_security_policy(&service, Request::new(Default::default()))
                .await,
        );
        assert_not_configured(
            SecurityService::set_tenant_security_policy(&service, Request::new(Default::default()))
                .await,
        );
    }

    #[test]
    fn default_security_service_can_be_wrapped_as_tonic_server() {
        let _server = SecurityServiceImpl::with_default_config().into_server();
    }
}
