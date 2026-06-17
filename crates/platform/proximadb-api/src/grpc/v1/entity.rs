//! # Entity Service (gRPC)
//!
//! gRPC implementation for entity operations in Semantic Knowledge Store (SKS).
//! Each RPC delegates to the injected `EntityPort`; when no port is provided
//! the service returns `UNIMPLEMENTED`.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use proximadb_proto::v1::{
    entity_service_server::{EntityService, EntityServiceServer},
    *,
};
use proximadb_runtime::EntityPort;

/// gRPC EntityService backed by an `EntityPort`.
pub struct EntityServiceImpl {
    port: Option<Arc<dyn EntityPort>>,
}

impl EntityServiceImpl {
    /// Construct with a concrete entity port.
    pub fn new(port: Arc<dyn EntityPort>) -> Self {
        Self { port: Some(port) }
    }

    /// Construct without a backend (all RPCs return UNIMPLEMENTED).
    pub fn without_backend() -> Self {
        Self { port: None }
    }

    /// Convert into a tonic gRPC server.
    pub fn into_service(self) -> EntityServiceServer<Self> {
        EntityServiceServer::new(self)
    }

    fn not_configured() -> Status {
        super::deprecated_status(Status::unimplemented(
            "Entity service not configured on this node",
        ))
    }

    fn port_err(e: anyhow::Error) -> Status {
        super::deprecated_status(Status::internal(e.to_string()))
    }
}

#[tonic::async_trait]
impl EntityService for EntityServiceImpl {
    async fn upsert_entity(
        &self,
        request: Request<UpsertEntityRequest>,
    ) -> Result<Response<UpsertEntityResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.upsert_entity(request.into_inner())
            .await
            .map(super::deprecated_response)
            .map_err(Self::port_err)
    }

    async fn get_entity(
        &self,
        request: Request<GetEntityRequest>,
    ) -> Result<Response<GetEntityResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_entity(request.into_inner())
            .await
            .map(super::deprecated_response)
            .map_err(Self::port_err)
    }

    async fn delete_entity(
        &self,
        request: Request<DeleteEntityRequest>,
    ) -> Result<Response<DeleteEntityResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.delete_entity(request.into_inner())
            .await
            .map(super::deprecated_response)
            .map_err(Self::port_err)
    }

    async fn search_entities(
        &self,
        request: Request<SearchEntitiesRequest>,
    ) -> Result<Response<SearchEntitiesResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.search_entities(request.into_inner())
            .await
            .map(super::deprecated_response)
            .map_err(Self::port_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    fn assert_unimplemented<T>(result: Result<Response<T>, Status>) {
        let err = match result {
            Ok(_) => panic!("backend-less entity service should reject RPC"),
            Err(err) => err,
        };
        assert_eq!(err.code(), Code::Unimplemented);
        assert!(err.message().contains("Entity service not configured"));
    }

    #[tokio::test]
    async fn backendless_entity_service_rejects_every_rpc_consistently() {
        let service = EntityServiceImpl::without_backend();

        assert_unimplemented(
            EntityService::upsert_entity(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            EntityService::get_entity(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            EntityService::delete_entity(&service, Request::new(Default::default())).await,
        );
        assert_unimplemented(
            EntityService::search_entities(&service, Request::new(Default::default())).await,
        );
    }

    #[test]
    fn backendless_entity_service_can_be_wrapped_as_tonic_server() {
        let _server = EntityServiceImpl::without_backend().into_service();
    }
}
