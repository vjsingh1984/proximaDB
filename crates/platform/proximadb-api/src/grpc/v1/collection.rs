//! # Collection Service (gRPC)
//!
//! gRPC implementation for collection management operations.
//! Routes through `ApiHandlersPort` — the seam between this protocol adapter and
//! the business logic in `proximadb-runtime`.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::{self as v1, CollectionOperation};
use proximadb_runtime::ApiHandlersPort;

/// gRPC implementation of the CollectionService for managing vector collections.
///
/// Holds a port reference so the actual business logic can live in the root crate's
/// `UnifiedHandlers` (or any other `ApiHandlersPort` implementation) without this
/// crate importing root-crate concrete types.
pub struct CollectionServiceImpl {
    port: Arc<dyn ApiHandlersPort>,
}

impl CollectionServiceImpl {
    /// Create a new collection service backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self { port }
    }

    /// Convert this implementation into a tonic gRPC server.
    pub fn into_server(self) -> v1::collection_service_server::CollectionServiceServer<Self> {
        v1::collection_service_server::CollectionServiceServer::new(self)
    }

    fn extract_tenant_id<T>(request: &Request<T>) -> Option<String> {
        request
            .metadata()
            .get("x-tenant-id")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(ToOwned::to_owned)
    }
}

#[tonic::async_trait]
impl v1::collection_service_server::CollectionService for CollectionServiceImpl {
    async fn create_collection(
        &self,
        request: Request<v1::CollectionConfig>,
    ) -> Result<Response<v1::Collection>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let cfg = request.into_inner();
        let req = v1::CollectionRequest {
            operation: CollectionOperation::CollectionCreate as i32,
            collection_id: Some(cfg.name.clone()),
            collection_config: Some(cfg),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .port
            .handle_collection_operation_for_tenant(req, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("CreateCollection failed: {e}")))?;
        resp.collection
            .ok_or_else(|| Status::internal("CreateCollection returned no collection"))
            .map(Response::new)
    }

    async fn get_collection(
        &self,
        request: Request<v1::GetCollectionRequest>,
    ) -> Result<Response<v1::Collection>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let inner = request.into_inner();
        let req = v1::CollectionRequest {
            operation: CollectionOperation::CollectionGet as i32,
            collection_id: Some(inner.collection_id),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .port
            .handle_collection_operation_for_tenant(req, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("GetCollection failed: {e}")))?;
        resp.collection
            .ok_or_else(|| Status::not_found("Collection not found"))
            .map(Response::new)
    }

    async fn list_collections(
        &self,
        request: Request<v1::ListCollectionsRequest>,
    ) -> Result<Response<v1::ListCollectionsResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let req = v1::CollectionRequest {
            operation: CollectionOperation::CollectionList as i32,
            collection_id: None,
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .port
            .handle_collection_operation_for_tenant(req, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("ListCollections failed: {e}")))?;
        Ok(Response::new(v1::ListCollectionsResponse {
            collections: resp.collections,
        }))
    }

    async fn delete_collection(
        &self,
        request: Request<v1::DeleteCollectionRequest>,
    ) -> Result<Response<v1::DeleteCollectionResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let inner = request.into_inner();
        let req = v1::CollectionRequest {
            operation: CollectionOperation::CollectionDelete as i32,
            collection_id: Some(inner.collection_id),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        self.port
            .handle_collection_operation_for_tenant(req, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("DeleteCollection failed: {e}")))?;
        Ok(Response::new(v1::DeleteCollectionResponse {
            success: true,
        }))
    }
}
